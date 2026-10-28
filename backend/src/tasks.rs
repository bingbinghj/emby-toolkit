use crate::{
    AppState,
    error::{ApiError, Result},
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tasks/run", post(run))
        .route("/api/tasks/available", get(available))
}
async fn available(Query(params): Query<std::collections::HashMap<String, String>>) -> Json<Value> {
    let catalog: Value = serde_json::from_str(include_str!("../assets/task_catalog.json")).unwrap();
    let context = params
        .get("context")
        .map(String::as_str)
        .filter(|s| ["chain", "manual", "all"].contains(s))
        .unwrap_or("chain");
    Json(catalog[context].clone())
}
async fn run(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let task = body["task_name"]
        .as_str()
        .ok_or_else(|| ApiError::bad("请求体中缺少 'task_name' 参数"))?;
    let result = submit_task(
        &state,
        task,
        body["force_full_update"].as_bool().unwrap_or(false),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

pub async fn submit_task(state: &AppState, task: &str, full: bool) -> Result<Value> {
    if ![
        "populate-metadata",
        "clear-mediainfo",
        "repair-p115-fingerprints",
        "role-translation",
        "actor-translation",
        "enrich-aliases",
        "backup-mediainfo",
        "restore-nfo-and-images",
        "purge-ghost-actors",
        "restore_mediainfo",
        "repair-truehd-core-shift",
        "restore-truehd-core-backups",
        "clear-truehd-core-backups",
        "generate-all-covers",
        "clear-emby-activity-log",
        "repair-video-thumbnails",
    ]
    .contains(&task)
    {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("未知的任务名称: {task}"),
        ));
    }
    if ["actor-translation", "role-translation"].contains(&task)
        && state.config.read().await["ai_translate_actor_role"] != true
    {
        return Err(ApiError(StatusCode::CONFLICT, "AI翻译功能未启用".into()));
    }
    if task == "generate-all-covers"
        && crate::db::setting(&state.db, "cover_generator_config")
            .await?
            .is_none_or(|config| config["enabled"] != true)
    {
        return Err(ApiError(StatusCode::CONFLICT, "封面生成器未启用".into()));
    }
    let task = task.to_string();
    let catalog: Value = serde_json::from_str(include_str!("../assets/task_catalog.json")).unwrap();
    let name = catalog["all"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["key"] == task)
        .and_then(|v| v["name"].as_str())
        .unwrap_or(&task)
        .to_string();
    let message = format!("任务 '{name}' 已成功提交。");
    let work = state.clone();
    let id = state
        .jobs
        .submit(&name, move |context| async move {
            match task.as_str() {
                "populate-metadata"=>crate::sync::cache(&work, full, &context).await,
                "clear-mediainfo"=>{context.checkpoint().await?;let count=sqlx::query("DELETE FROM p115_mediainfo_cache").execute(&work.db).await?.rows_affected();Ok(json!({"message":format!("媒体信息备份已清空，共删除 {count} 条记录。"),"deleted":count}))},
                "repair-p115-fingerprints"=>crate::revalidation::repair(&work,&context).await,
                "role-translation"=>translate_roles(&work,full,&context).await,
                "actor-translation"=>translate_persons(&work,&context).await,
                "enrich-aliases"=>enrich_persons(&work,full,&context).await,
                "restore-nfo-and-images"=>restore_metadata(&work,&context).await,
                "purge-ghost-actors"=>purge_persons(&work,&context).await,
                "restore_mediainfo"=>crate::media_restore::restore(&work,full,&context).await,
                "repair-truehd-core-shift"=>crate::audio_tasks::repair_all(&work,&context).await,
                "restore-truehd-core-backups"=>crate::audio_tasks::restore(&work,&context).await,
                "clear-truehd-core-backups"=>crate::audio_tasks::clear(&work,&context).await,
                "generate-all-covers"=>crate::covers::generate(&work,None,&context).await,
                "clear-emby-activity-log"=>crate::activity::clear(&work,&context).await,
                "repair-video-thumbnails"=>crate::thumbnails::repair(&work,&context).await,
                _=>backup_info(&work,&context).await,
            }
        })
        .await?;
    Ok(json!({"message":message,"task_id":id}))
}

async fn translate_persons(state: &AppState, context: &crate::jobs::JobContext) -> Result<Value> {
    if state.config.read().await["ai_translate_actor_role"] != true {
        return Ok(json!({"updated":0,"message":"AI翻译功能未启用，跳过任务。"}));
    }
    context
        .activity("阶段1/3：扫描人物", "正在扫描 Emby，收集待翻译人物")
        .await;
    let people = crate::emby::persons(state, context).await?;
    tracing::info!("[人物中文化] 已读取 {} 名媒体库关联人物", people.len());
    let mut inputs = std::collections::BTreeMap::new();
    let mut source_map = std::collections::HashMap::new();
    for person in &people {
        let name = crate::state::string(person, "Name");
        if name.is_empty() || crate::translation::chinese(&name) {
            continue;
        }
        let tmdb = crate::state::string(&person["ProviderIds"], "Tmdb")
            .parse::<i32>()
            .ok();
        let original: Option<String> = if let Some(id) = tmdb {
            sqlx::query_scalar("SELECT original_name FROM person_metadata WHERE tmdb_person_id=$1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?
                .flatten()
        } else {
            None
        };
        let source = original
            .filter(|s| !s.is_empty() && !crate::translation::chinese(s))
            .unwrap_or(name.clone());
        inputs.insert(source.clone(), source.clone());
        source_map.insert(name, source);
    }
    context
        .activity(
            "阶段2/3：翻译人物",
            &format!("正在翻译 {} 个外文人物名", inputs.len()),
        )
        .await;
    let translated =
        crate::ai::translate(state, &inputs, "fast_mode", &json!({}), &Value::Null).await?;
    tracing::info!(
        "[人物中文化] 请求翻译 {} 个名称，取得 {} 个结果，开始回写人物",
        inputs.len(),
        translated.len()
    );
    let mut updated = 0;
    let mut failures = vec![];
    let total = people.len();
    for (index, person) in people.into_iter().enumerate() {
        context.checkpoint().await?;
        let name = crate::state::string(&person, "Name");
        context
            .update_counted(
                index as u64,
                total as u64,
                "阶段3/3：回写人物",
                &format!("正在处理人物：{name}"),
            )
            .await;
        if let Some(value) = source_map.get(&name).and_then(|s| translated.get(s)) {
            if value.is_empty() || value == &name {
                continue;
            }
            let id = crate::state::string(&person, "Id");
            let result = async {
                let mut person = crate::media_edit::details(state, &id, "ProviderIds,Name").await?;
                person["Name"] = json!(value);
                state
                    .emby(
                        reqwest::Method::POST,
                        &format!("Items/{id}"),
                        &[],
                        Some(person),
                    )
                    .await?;
                Ok::<_, ApiError>(())
            }
            .await;
            match result {
                Ok(()) => {
                    updated += 1;
                    tracing::info!("[人物中文化] '{name}' → '{value}' 已更新（Item {id}）");
                }
                Err(error) => {
                    if context.is_cancelled() {
                        return Err(error);
                    }
                    tracing::warn!(
                        "[人物中文化] '{name}' → '{value}' 更新失败（Item {id}）：{}",
                        error.1
                    );
                    failures.push(format!("{id}: {}", error.1));
                }
            }
        }
    }
    context.checkpoint().await?;
    context
        .update_counted(
            total as u64,
            total as u64,
            "人物中文化",
            &format!(
                "人物中文化完成：更新 {updated} 人，失败 {} 人",
                failures.len()
            ),
        )
        .await;
    Ok(
        json!({"updated":updated,"failures":failures,"message":format!("人物中文化完成：更新 {updated} 人，失败 {} 人", failures.len())}),
    )
}
async fn translate_roles(
    state: &AppState,
    full: bool,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let config = state.config.read().await.clone();
    if config["ai_translate_actor_role"] != true {
        return Ok(json!({"updated":0,"message":"AI翻译功能未启用，跳过任务。"}));
    }
    if full {
        sqlx::query("DELETE FROM processed_log")
            .execute(&state.db)
            .await?;
    }
    let mut media = vec![];
    context
        .activity("扫描媒体", "正在收集需要角色中文化的电影和电视剧")
        .await;
    for library in crate::emby::processing_libraries(state).await? {
        media.extend(
            crate::emby::items(state, &library, "Id,Type,Name", context)
                .await?
                .into_iter()
                .filter(|i| matches!(i["Type"].as_str(), Some("Movie" | "Series"))),
        );
    }
    let mut updated = 0;
    let mut failures = vec![];
    let mut processed = false;
    let total = media.len();
    for (index, item) in media.into_iter().enumerate() {
        context.checkpoint().await?;
        let id = crate::state::string(&item, "Id");
        let name = item["Name"].as_str().unwrap_or(&id);
        context
            .update_counted(
                index as u64,
                total as u64,
                "角色中文化",
                &format!("正在处理《{name}》（Item {id}）"),
            )
            .await;
        if !full
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM processed_log WHERE item_id=$1)",
            )
            .bind(&id)
            .fetch_one(&state.db)
            .await?
        {
            tracing::info!("[角色中文化] 跳过《{name}》：已处理，快速模式保留现有数据");
            continue;
        }
        if processed {
            state.item_delay(context).await?;
        }
        processed = true;
        let result = if full {
            let item = crate::media_edit::details(
                state,
                &id,
                "Id,Type,Name,Path,ProviderIds,MediaSources,People,Genres",
            )
            .await?;
            crate::ingest::scrape(state, &item, &[], None, context).await
        } else {
            crate::webhook::process_emby(
                state,
                &json!({"id":id,"episode_ids":[],"event":"item.add"}),
                context,
            )
            .await
        };
        match result {
            Ok(_) => {
                updated += 1;
                tracing::info!("[角色中文化] 《{name}》处理完成（Item {id}）");
            }
            Err(error) => {
                if context.is_cancelled() {
                    return Err(error);
                }
                tracing::warn!("[角色中文化] 《{name}》处理失败（Item {id}）：{}", error.1);
                failures.push(error.1);
            }
        }
    }
    context.checkpoint().await?;
    context
        .update_counted(
            total as u64,
            total as u64,
            "角色中文化",
            &format!(
                "角色中文化完成：处理 {updated} 项，失败 {} 项",
                failures.len()
            ),
        )
        .await;
    Ok(
        json!({"updated":updated,"failures":failures,"message":format!("角色中文化完成：处理 {updated} 项，失败 {} 项", failures.len())}),
    )
}

async fn enrich_persons(
    state: &AppState,
    full: bool,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM person_metadata p WHERE $1 OR ((imdb_id IS NULL OR profile_path IS NULL OR gender IS NULL OR original_name IS NULL) AND (last_updated_at IS NULL OR last_updated_at<NOW()-INTERVAL '30 days')) ORDER BY last_updated_at ASC NULLS FIRST,tmdb_person_id").bind(full).fetch_all(&state.db).await?;
    let mut updated = 0;
    let mut deleted = 0;
    let mut failures = vec![];
    let total = rows.len();
    for (index, row) in rows.into_iter().enumerate() {
        context.checkpoint().await?;
        let id = row["tmdb_person_id"].as_i64().unwrap() as i32;
        let name = row["primary_name"]
            .as_str()
            .or_else(|| row["original_name"].as_str())
            .unwrap_or("未命名人物");
        context
            .update_counted(
                index as u64,
                total as u64,
                "演员数据补充",
                &format!("正在补充人物：{name}（TMDb {id}）"),
            )
            .await;
        match crate::tmdb::person(state, id).await {
            Ok(person) => {
                let original = person["english_name_from_translations"]
                    .as_str()
                    .or_else(|| {
                        person["original_name"]
                            .as_str()
                            .filter(|s| !crate::translation::chinese(s))
                    });
                let mut transaction = state.db.begin().await?;
                // Serialize the unique IMDb/Douban identity merge before changing either row.
                sqlx::query("LOCK TABLE person_metadata IN SHARE ROW EXCLUSIVE MODE")
                    .execute(&mut *transaction)
                    .await?;
                let imdb = person["external_ids"]["imdb_id"].as_str();
                let target: Option<i32> = sqlx::query_scalar("SELECT tmdb_person_id FROM person_metadata WHERE imdb_id=$1 AND tmdb_person_id<>$2")
                    .bind(imdb).bind(id).fetch_optional(&mut *transaction).await?;
                if let Some(target) = target {
                    let douban: Option<String> = sqlx::query_scalar("DELETE FROM person_metadata WHERE tmdb_person_id=$1 RETURNING douban_celebrity_id")
                        .bind(id).fetch_optional(&mut *transaction).await?.flatten();
                    sqlx::query("UPDATE person_metadata SET douban_celebrity_id=COALESCE(douban_celebrity_id,$1) WHERE tmdb_person_id=$2")
                        .bind(douban).bind(target).execute(&mut *transaction).await?;
                }
                sqlx::query("UPDATE person_metadata SET imdb_id=COALESCE($1,imdb_id),profile_path=$2,gender=$3,adult=$4,popularity=$5,original_name=$6,last_updated_at=NOW() WHERE tmdb_person_id=$7")
                .bind(imdb).bind(person["profile_path"].as_str()).bind(person["gender"].as_i64().map(|n|n as i32)).bind(person["adult"].as_bool().unwrap_or(false)).bind(person["popularity"].as_f64().map(|n|n as f32)).bind(original).bind(target.unwrap_or(id)).execute(&mut *transaction).await?;
                transaction.commit().await?;
                updated += 1;
                tracing::info!("[演员数据补充] {name}（TMDb {id}）数据已更新");
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) if error.0 == StatusCode::NOT_FOUND => {
                deleted += sqlx::query("DELETE FROM person_metadata WHERE tmdb_person_id=$1")
                    .bind(id)
                    .execute(&state.db)
                    .await?
                    .rows_affected();
                tracing::info!(
                    "[演员数据补充] {name}（TMDb {id}）在 TMDb 已不存在，已清理本地记录"
                );
            }
            Err(error) => {
                tracing::warn!("[演员数据补充] {name}（TMDb {id}）失败：{}", error.1);
                failures.push(format!("{id}: {}", error.1));
            }
        }
    }
    context.checkpoint().await?;
    let message = format!(
        "演员数据补充完成：更新 {updated} 人，清理 {deleted} 人，失败 {} 人",
        failures.len()
    );
    context
        .update_counted(total as u64, total as u64, "演员数据补充", &message)
        .await;
    Ok(json!({"updated":updated,"deleted":deleted,"failures":failures,"message":message}))
}

async fn restore_metadata(state: &AppState, context: &crate::jobs::JobContext) -> Result<Value> {
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Series') AND tmdb_id NOT IN ('0','None','null','')").fetch_all(&state.db).await?;
    let config = state.config.read().await.clone();
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let studio = crate::db::setting(&state.db, "studio_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_STUDIO_MAPPING"].clone());
    let keywords = crate::db::setting(&state.db, "keyword_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_KEYWORD_MAPPING"].clone());
    let options = crate::nfo_files::Options {
        config: &config,
        studio_mapping: &studio,
        keyword_mapping: &keywords,
        ..Default::default()
    };
    let mut updated = 0;
    let mut written = 0;
    let mut images = 0;
    let mut failures = vec![];
    for (index, row) in rows.iter().enumerate() {
        context.checkpoint().await?;
        let id = crate::state::string(row, "tmdb_id");
        let kind = crate::state::string(row, "item_type");
        let title = crate::state::string(row, "title");
        context
            .update_counted(
                index as u64,
                rows.len() as u64,
                "恢复NFO和封面",
                &format!("正在恢复《{title}》的 NFO 和封面"),
            )
            .await;
        let result = async {
            let assets = crate::media_edit::list(&row["asset_details_json"]);
            let path = assets
                .iter()
                .find(|asset| {
                    !crate::emby::library_ignored(
                        &config,
                        &crate::state::string(asset, "source_library_id"),
                    )
                })
                .and_then(|a| a["path"].as_str())
                .unwrap_or("")
                .to_string();
            if path.is_empty() || path.starts_with("http") {
                return Err(ApiError::bad("缺失本地媒体路径"));
            }
            if crate::emby::path_ignored(state, &path).await? {
                return Ok((0, 0));
            }
            let (payload, cast) = crate::metadata::reconstruct_full(state, &id, &kind, true)
                .await?
                .ok_or_else(|| ApiError::bad("缺失元数据缓存"))?;
            let item = json!({"Path":path,"Type":kind});
            let nfo = crate::nfo_files::sync(&kind, &item, &payload, &cast, &options).await?;
            let images =
                crate::images::download(state, &kind, &id, &item, &payload, context).await?;
            Ok::<_, ApiError>((nfo, images))
        }
        .await;
        match result {
            Ok((nfo, count)) => {
                updated += 1;
                written += nfo;
                images += count;
                tracing::info!(
                    "[NFO和封面还原] 《{title}》完成：写入 {nfo} 个 NFO、{count} 张图片"
                );
            }
            Err(error) => {
                if context.is_cancelled() {
                    return Err(error);
                }
                tracing::warn!("[NFO和封面还原] 《{title}》失败：{}", error.1);
                failures.push(format!("{id}: {}", error.1));
            }
        }
    }
    context.checkpoint().await?;
    let message = format!(
        "NFO和封面还原完成：处理 {updated} 项，写入 {written} 个 NFO、{images} 张图片，失败 {} 项",
        failures.len()
    );
    context
        .update_counted(
            rows.len() as u64,
            rows.len() as u64,
            "恢复NFO和封面",
            &message,
        )
        .await;
    Ok(
        json!({"updated":updated,"written":written,"images":images,"failures":failures,"message":message}),
    )
}

async fn purge_persons(state: &AppState, context: &crate::jobs::JobContext) -> Result<Value> {
    context
        .activity("扫描媒体和演员", "正在扫描媒体库，建立被引用的演员名单")
        .await;
    let config = state.config.read().await.clone();
    let user = crate::state::string(&config, "emby_user_id");
    let libraries = state
        .emby(
            reqwest::Method::GET,
            &format!("Users/{user}/Views"),
            &[],
            None,
        )
        .await?;
    let libraries = libraries["Items"]
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 媒体库列表无效"))?;
    let mut media = vec![];
    for library in libraries.iter().filter(|l| {
        matches!(
            l["CollectionType"].as_str(),
            Some("movies" | "tvshows" | "homevideos" | "musicvideos")
        )
    }) {
        media.extend(
            crate::emby::query_items(
                state,
                &[
                    ("ParentId", crate::state::string(library, "Id")),
                    ("IncludeItemTypes", "Movie,Series".into()),
                    ("Recursive", "true".into()),
                    ("Fields", "People".into()),
                ],
                context,
            )
            .await?,
        );
    }
    if media.is_empty() {
        return Ok(json!({"deleted":0,"message":"服务器中未找到媒体项"}));
    }
    let used = media
        .iter()
        .flat_map(|m| crate::media_edit::list(&m["People"]))
        .map(|p| crate::state::string(&p, "Id"))
        .collect::<std::collections::HashSet<_>>();
    let people = crate::emby::query_items(
        state,
        &[
            ("IncludeItemTypes", "Person".into()),
            ("Recursive", "true".into()),
            ("Fields", "Name".into()),
        ],
        context,
    )
    .await?;
    let orphans = people
        .iter()
        .filter(|p| !used.contains(&crate::state::string(p, "Id")))
        .collect::<Vec<_>>();
    if orphans.is_empty() {
        return Ok(json!({"deleted":0,"message":"未发现幽灵演员"}));
    }
    context.checkpoint().await?;
    let login: Value = state.http.post(format!("{}/Users/AuthenticateByName",crate::state::string(&config,"emby_server_url").trim_end_matches('/')))
        .header("X-Emby-Authorization","MediaBrowser Client=\"EmbyToolkit\", Device=\"Server\", DeviceId=\"etk-rust\", Version=\"1.0\"")
        .json(&json!({"Username":config["emby_admin_user"],"Pw":config["emby_admin_pass"]})).send().await?.error_for_status()?.json().await?;
    let token = crate::state::string(&login, "AccessToken");
    let user = crate::state::string(&login["User"], "Id");
    if token.is_empty() || user.is_empty() || login["User"]["Policy"]["IsAdministrator"] != true {
        return Err(ApiError::forbidden());
    }
    let mut deleted = 0;
    let mut failures = vec![];
    let total = orphans.len();
    for (index, person) in orphans.into_iter().enumerate() {
        context.checkpoint().await?;
        let id = crate::state::string(person, "Id");
        let name = crate::state::string(person, "Name");
        context
            .update_counted(
                index as u64,
                total as u64,
                "清理幽灵演员",
                &format!("正在删除未被引用的演员：{name}（Item {id}）"),
            )
            .await;
        let response = state
            .http
            .post(format!(
                "{}/Items/{id}/DeletePerson",
                crate::state::string(&config, "emby_server_url").trim_end_matches('/')
            ))
            .header("X-Emby-Token", &token)
            .query(&[("UserId", &user)])
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                deleted += 1;
                tracing::info!("[幽灵演员清理] 已删除 {name}（Item {id}）");
            }
            _ => {
                context.checkpoint().await?;
                tracing::warn!("[幽灵演员清理] 删除 {name}（Item {id}）失败");
                failures.push(id);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    context.checkpoint().await?;
    let message = format!(
        "幽灵演员清理完成：删除 {deleted} 人，失败 {} 人",
        failures.len()
    );
    context
        .update_counted(total as u64, total as u64, "清理幽灵演员", &message)
        .await;
    Ok(json!({"deleted":deleted,"failures":failures,"message":message}))
}
async fn backup_info(state: &AppState, context: &crate::jobs::JobContext) -> Result<Value> {
    context
        .activity("扫描媒体信息", "正在扫描需要备份的在库电影和分集")
        .await;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Episode')").fetch_all(&state.db).await?;
    let mut updated = 0;
    let mut failures = vec![];
    let total = rows.len();
    for (row_index, row) in rows.into_iter().enumerate() {
        let ids = crate::media_edit::list(&row["emby_item_ids_json"]);
        for (index, asset) in crate::media_edit::list(&row["asset_details_json"])
            .iter()
            .enumerate()
        {
            context.checkpoint().await?;
            if crate::emby::path_ignored(state, &crate::state::string(asset, "path")).await? {
                continue;
            }
            let id = asset["emby_item_id"]
                .as_str()
                .or_else(|| ids.get(index).and_then(Value::as_str))
                .unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let label = crate::revalidation::media_label(&row, asset, index);
            context
                .update_counted(
                    row_index as u64,
                    total as u64,
                    "备份媒体信息",
                    &format!("正在备份 {label}"),
                )
                .await;
            let result=async {
                let mut target=crate::media_edit::target(state,id).await?;
                let item=crate::media_edit::details(state,id,"Id,Type,Path,Size,MediaSources").await?;
                let observed=crate::revalidation::observation(state,&item).await;
                let pc=crate::fingerprints::pickcode(&observed.1);
                let old_pc=crate::media_edit::list(&row["file_pickcode_json"]).get(index).and_then(Value::as_str).unwrap_or("").to_string();
                let changed=target.asset["fingerprint_pending"]==true
                    || observed.0!=target.path
                    || matches!((crate::identity::size(&target.asset["size_bytes"]),observed.2),(Some(a),Some(b)) if a!=b)
                    || target.asset["fingerprint_target"].as_str().is_some_and(|t|!t.is_empty()&&t!=observed.1)
                    || pc.as_ref().is_some_and(|p|!old_pc.is_empty()&&p!=&old_pc);
                if changed || target.sha.is_none() {
                    let result=crate::revalidation::ids(state,&[id.to_string()],context).await?;
                    if result["failed"].as_u64().unwrap_or(0)>0{return Err(ApiError::bad("文件身份待校验，未备份"));}
                    target=crate::media_edit::target(state,id).await?;
                }
                let sha=target.sha.as_ref().ok_or_else(||ApiError::bad("缺失SHA1"))?;
                if target.asset["fingerprint_pending"]==true{return Err(ApiError::bad("文件身份待校验"));}
                if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM p115_mediainfo_cache WHERE sha1=$1 AND mediainfo_json IS NOT NULL)").bind(sha).fetch_one(&state.db).await? {return Ok(false);}
                if target.path.starts_with("http"){return Ok(false);}
                for path in &target.candidates {
                    if !path.is_file(){continue;}
                    let info:Value=serde_json::from_slice(&tokio::fs::read(path).await?).map_err(|_|ApiError::bad("媒体信息JSON无效"))?;
                    if !info.is_array() || !crate::media_info::recoverable(&info){continue;}
                    crate::identity::verify(&target.asset,&info)?;
                    crate::identity::verify(&json!({"size_bytes":observed.2}),&info)?;
                    if !crate::media_paths::matches_container(&info,&observed.1){return Err(ApiError::bad("媒体信息容器与当前文件不一致"));}
                    let current=crate::media_edit::details(state,id,"Id,Type,Path,Size,MediaSources").await?;
                    if crate::revalidation::observation(state,&current).await!=observed{return Err(ApiError::bad("文件在备份期间变化"));}
                    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json").bind(sha).bind(info).execute(&state.db).await?;
                    return Ok::<_,ApiError>(true);
                }
                Err(ApiError::bad("缺失媒体信息"))
            }.await;
            match result {
                Ok(true) => {
                    updated += 1;
                    tracing::info!("[媒体信息备份] {label} 已成功备份至数据库");
                }
                Ok(false) => {
                    tracing::info!("[媒体信息备份] 跳过 {label}：已有备份或当前为 HTTP 媒体路径")
                }
                Err(error) => {
                    if context.is_cancelled() {
                        return Err(error);
                    }
                    tracing::warn!("[媒体信息备份] {label} 未备份：{}", error.1);
                    if error.1.contains("缺失媒体信息") {
                        let owner = if row["item_type"] == "Episode" {
                            crate::metadata::row(
                                state,
                                &crate::state::string(&row, "parent_series_tmdb_id"),
                                "Series",
                            )
                            .await?
                        } else {
                            Some(row.clone())
                        };
                        if let Some(owner) = owner
                            && let Some(owner_id) =
                                crate::media_edit::list(&owner["emby_item_ids_json"])
                                    .first()
                                    .and_then(Value::as_str)
                        {
                            let reason = if row["item_type"] == "Episode" {
                                format!(
                                    "缺失媒体信息: S{:02}E{:02}",
                                    row["season_number"].as_i64().unwrap_or(0),
                                    row["episode_number"].as_i64().unwrap_or(0)
                                )
                            } else {
                                "缺失媒体信息".into()
                            };
                            sqlx::query("INSERT INTO failed_log(item_id,item_name,item_type,reason,error_message,score) VALUES($1,$2,$3,$4,$4,0) ON CONFLICT(item_id) DO UPDATE SET reason=EXCLUDED.reason,error_message=EXCLUDED.error_message,score=0,failed_at=NOW()")
                                    .bind(owner_id).bind(crate::state::string(&owner,"title")).bind(crate::state::string(&owner,"item_type")).bind(reason).execute(&state.db).await?;
                        }
                    }
                    failures.push(format!("{id}: {}", error.1));
                }
            }
        }
    }
    context.checkpoint().await?;
    let message = format!(
        "媒体信息备份完成：新增备份 {updated} 项，失败 {} 项",
        failures.len()
    );
    context
        .update_counted(total as u64, total as u64, "备份媒体信息", &message)
        .await;
    Ok(json!({"updated":updated,"failures":failures,"message":message}))
}
