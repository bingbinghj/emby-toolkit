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
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "chain".into());
    let context = Some(context.as_str())
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
    let allowed: &[&str] = match task {
        "populate-metadata" => &["force_full_update", "batch_size"],
        "enrich-aliases" | "role-translation" | "restore_mediainfo" | "extract-episode-credits" => &["force_full_update"],
        _ => &[],
    };
    let unsupported = body
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| key.as_str() != "task_name" && !allowed.contains(&key.as_str()))
        .cloned()
        .collect();
    let result = submit_with_batch(
        &state,
        task,
        task_truthy(&body["force_full_update"]),
        body.get("batch_size").cloned().unwrap_or(json!(10)),
        unsupported,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

pub async fn submit_task(state: &AppState, task: &str, full: bool) -> Result<Value> {
    submit_with_batch(state, task, full, json!(10), vec![]).await
}
fn task_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
async fn submit_with_batch(
    state: &AppState,
    task: &str,
    full: bool,
    batch_size: Value,
    unsupported: Vec<String>,
) -> Result<Value> {
    if task == "clear-music-comments" && unsupported.is_empty() {
        return crate::music::submit_clear_comments(state).await;
    }
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
        "extract-episode-credits",
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
    let payload =
        json!({"task":task,"full":full,"batch_size":batch_size,"unsupported":unsupported});
    let spec = crate::jobs::JobSpec::new("tasks", payload.clone());
    let id = state
        .jobs
        .submit_persistent_response(&name, spec, move |context| async move {
            execute_persisted(&work, &payload, &context).await
        })
        .await?;
    Ok(json!({"message":message,"task_id":id}))
}

pub(crate) async fn execute_persisted(
    work: &AppState,
    payload: &Value,
    context: &crate::jobs::JobContext,
) -> Result<(u16, Value)> {
    let task = payload["task"]
        .as_str()
        .ok_or_else(|| ApiError::bad("持久化任务名称无效"))?;
    let full = payload["full"] == true;
    let batch_size = payload["batch_size"].clone();
    let unsupported: Vec<String> = serde_json::from_value(payload["unsupported"].clone())
        .map_err(|_| ApiError::bad("持久化任务参数无效"))?;
    if !unsupported.is_empty() {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("任务 {task} 不支持参数：{}", unsupported.join(", ")),
        ));
    }
    let result = match task {
        "populate-metadata" => {
            let batch_size = batch_size
                .as_i64()
                .or_else(|| batch_size.as_bool().map(i64::from))
                .ok_or_else(|| {
                    ApiError(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "分批大小必须是整数".into(),
                    )
                })?;
            Box::pin(crate::sync::cache_with_batch(
                work, full, batch_size, context,
            ))
            .await
        }
        "clear-mediainfo" => {
            let key = "tasks:clear-mediainfo";
            if let Some(result) = context.completed(key).await? {
                Ok(result)
            } else {
                context.checkpoint().await?;
                let mut transaction = work.db.begin().await?;
                let count = sqlx::query("DELETE FROM p115_mediainfo_cache")
                    .execute(&mut *transaction)
                    .await?
                    .rows_affected();
                let result = json!({"message":format!("媒体信息备份已清空，共删除 {count} 条记录。"),"deleted":count});
                context
                    .complete_in_tx(&mut transaction, key, &result)
                    .await?;
                transaction.commit().await?;
                Ok(result)
            }
        }
        "repair-p115-fingerprints" => Box::pin(crate::revalidation::repair(work, context)).await,
        "role-translation" => Box::pin(translate_roles(work, full, context)).await,
        "actor-translation" => Box::pin(translate_persons(work, context)).await,
        "enrich-aliases" => Box::pin(enrich_persons(work, full, context)).await,
        "restore-nfo-and-images" => Box::pin(restore_metadata(work, context)).await,
        "purge-ghost-actors" => Box::pin(purge_persons(work, context)).await,
        "restore_mediainfo" => Box::pin(crate::media_restore::restore(work, full, context)).await,
        "repair-truehd-core-shift" => Box::pin(crate::audio_tasks::repair_all(work, context)).await,
        "restore-truehd-core-backups" => Box::pin(crate::audio_tasks::restore(work, context)).await,
        "clear-truehd-core-backups" => Box::pin(crate::audio_tasks::clear(work, context)).await,
        "generate-all-covers" => Box::pin(crate::covers::generate(work, None, context)).await,
        "clear-emby-activity-log" => Box::pin(crate::activity::clear(work, context)).await,
        "repair-video-thumbnails" => Box::pin(crate::thumbnails::repair(work, context)).await,
        "extract-episode-credits" => Box::pin(crate::credits::extract_all(work, full, context)).await,
        "backup-mediainfo" => Box::pin(backup_info(work, context)).await,
        _ => Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("未知的任务名称: {task}"),
        )),
    }?;
    Ok((200, result))
}

async fn task_plan(
    context: &crate::jobs::JobContext,
    key: &str,
    build: impl std::future::Future<Output = Result<Value>>,
) -> Result<Value> {
    if let Some(plan) = context.plan(key).await? {
        return Ok(plan);
    }
    let plan = build.await?;
    context.save_plan(key, &plan).await?;
    Ok(plan)
}

async fn translate_persons(state: &AppState, context: &crate::jobs::JobContext) -> Result<Value> {
    context.checkpoint().await?;
    if state.config.read().await["ai_translate_actor_role"] != true {
        return Ok(json!({"updated":0,"message":"AI翻译功能未启用，跳过任务。"}));
    }
    let plan = task_plan(context, "tasks:actor-translation:plan", async {
        context
            .activity("阶段1/3：扫描人物", "正在扫描 Emby，收集待翻译人物")
            .await;
        let mut people = crate::emby::persons(state, context).await?;
        crate::media_order::sort(&mut people);
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
                sqlx::query_scalar(
                    "SELECT original_name FROM person_metadata WHERE tmdb_person_id=$1",
                )
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
        Ok(json!({"people":people,"inputs":inputs,"source_map":source_map}))
    })
    .await?;
    let people = crate::media_edit::list(&plan["people"]);
    let inputs: std::collections::BTreeMap<String, String> =
        serde_json::from_value(plan["inputs"].clone())
            .map_err(|_| ApiError::bad("人物翻译计划无效"))?;
    let source_map: std::collections::HashMap<String, String> =
        serde_json::from_value(plan["source_map"].clone())
            .map_err(|_| ApiError::bad("人物翻译来源计划无效"))?;
    let mut updated = 0;
    let mut failures = vec![];
    let mut warning_details = vec![];
    let total = people
        .iter()
        .filter(|person| source_map.contains_key(&crate::state::string(person, "Name")))
        .count();
    let mut processed = 0;
    let inputs = inputs.into_iter().collect::<Vec<_>>();
    for (index, batch) in inputs.chunks(50).enumerate() {
        context.checkpoint().await?;
        context
            .activity(
                "阶段2/3：翻译人物",
                &format!(
                    "正在翻译第 {}/{} 批外文人物名，本批 {} 个",
                    index + 1,
                    inputs.len().div_ceil(50),
                    batch.len()
                ),
            )
            .await;
        let inputs: std::collections::BTreeMap<String, String> = batch.iter().cloned().collect();
        let batch_key = format!("tasks:actor-translation:batch:{index}");
        let saved = if let Some(result) = context.completed(&batch_key).await? {
            Some(result)
        } else {
            context.plan(&batch_key).await?
        };
        let mut translated: std::collections::BTreeMap<String, String> =
            if let Some(result) = &saved {
                serde_json::from_value(result["translated"].clone())
                    .map_err(|_| ApiError::bad("人物翻译结果无效"))?
            } else {
                std::collections::BTreeMap::new()
            };
        translated.retain(|source, value| {
            inputs.contains_key(source)
                && !value.trim().is_empty()
                && value.as_str() != source.as_str()
                && crate::translation::chinese(value)
        });
        let known_before_request = translated
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let mut warnings: Vec<String> = if let Some(result) = &saved {
            serde_json::from_value(result["warnings"].clone())
                .map_err(|_| ApiError::bad("人物翻译提醒无效"))?
        } else {
            vec![]
        };
        let missing = inputs
            .iter()
            .filter(|(source, _)| !translated.contains_key(*source))
            .map(|(source, value)| (source.clone(), value.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        if !missing.is_empty() && (saved.is_none() || context.is_resuming()) {
            let (values, details) = Box::pin(crate::ai::translate_report(
                state,
                &missing,
                "fast_mode",
                &json!({}),
                &Value::Null,
            ))
            .await?;
            translated.extend(values);
            translated.retain(|source, value| {
                inputs.contains_key(source)
                    && !value.trim().is_empty()
                    && value.as_str() != source.as_str()
                    && crate::translation::chinese(value)
            });
            warnings.extend(details);
            let mut seen = std::collections::HashSet::new();
            warnings.retain(|warning| seen.insert(warning.clone()));
            context
                .save_plan(
                    &batch_key,
                    &json!({"translated":translated,"warnings":warnings}),
                )
                .await?;
        }
        if inputs.keys().all(|source| translated.contains_key(source)) {
            context
                .complete(
                    &batch_key,
                    &json!({"translated":translated,"warnings":warnings}),
                )
                .await?;
        }
        warning_details.extend(warnings);
        tracing::info!(
            "[人物中文化] 第 {} 批请求 {} 个名称，取得 {} 个结果，开始回写人物",
            index + 1,
            batch.len(),
            translated.len()
        );
        for person in &people {
            let name = crate::state::string(person, "Name");
            let Some(source) = source_map
                .get(&name)
                .filter(|source| inputs.contains_key(*source))
            else {
                continue;
            };
            context.checkpoint().await?;
            processed += 1;
            let key = format!(
                "tasks:actor-translation:person:{}",
                crate::state::string(person, "Id")
            );
            if let Some(result) = context.completed(&key).await? {
                if result["updated"].as_u64().unwrap_or(0) > 0
                    || matches!(result["status"].as_str(), Some("completed" | "skipped"))
                    || known_before_request.contains(source)
                {
                    updated += result["updated"].as_u64().unwrap_or(0);
                    continue;
                }
            }
            context
                .update_counted(
                    processed as u64,
                    total as u64,
                    "阶段3/3：回写人物",
                    &format!("正在处理人物：{name}"),
                )
                .await;
            let mut changed = false;
            if let Some(value) = translated.get(source) {
                if value.is_empty() || value == &name {
                    context.complete(&key, &json!({"updated":0,"status":"skipped","warning":"姓名已是目标译文，未重复写入"})).await?;
                    continue;
                }
                let id = crate::state::string(person, "Id");
                let result = crate::persons::update_name(state, &id, value, Some(context)).await;
                match result {
                    Ok(true) => {
                        updated += 1;
                        changed = true;
                        tracing::info!("[人物中文化] '{name}' → '{value}' 已更新（Item {id}）");
                    }
                    Ok(false) => (),
                    Err(error) => {
                        if context.is_cancelled() {
                            return Err(error);
                        }
                        tracing::warn!(
                            "[人物中文化] '{name}' → '{value}' 更新失败（Item {id}）：{}",
                            error.1
                        );
                        failures.push(format!("{id}: {}", error.1));
                        continue;
                    }
                }
            } else {
                failures.push(format!(
                    "{}: 未取得通过校验的姓名译文，已保留原值",
                    crate::state::string(person, "Id")
                ));
                continue;
            }
            context
                .complete(
                    &key,
                    &json!({"updated":u64::from(changed),"status":"completed"}),
                )
                .await?;
        }
    }
    context.checkpoint().await?;
    let mut message = format!(
        "人物中文化完成：更新 {updated} 人，失败 {} 人，警告 {} 条",
        failures.len(),
        warning_details.len()
    );
    if let Some(reason) = warning_details.first() {
        message.push_str(&format!("；{reason}；完整明细见任务结果"));
    }
    context
        .update_counted(total as u64, total as u64, "人物中文化", &message)
        .await;
    Ok(
        json!({"updated":updated,"failures":failures,"warnings":warning_details.len(),"warning_details":warning_details,"message":message}),
    )
}
async fn translate_roles(
    state: &AppState,
    full: bool,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    context.checkpoint().await?;
    if state.config.read().await["ai_translate_actor_role"] != true {
        return Ok(json!({"updated":0,"message":"AI翻译功能未启用，跳过任务。"}));
    }
    let mut warnings = 0_u64;
    let mut warning_details = vec![];
    let plan = task_plan(context, "tasks:role-translation:plan", async {
    let config = state.config.read().await.clone();
    let mut ignored_ids = std::collections::HashSet::<String>::new();
    if !crate::media_edit::list(&config["libraries_to_ignore"]).is_empty() {
        let rows: Vec<(Value, Value)> = sqlx::query_as(
            "SELECT COALESCE(emby_item_ids_json,'[]'::jsonb),asset_details_json FROM media_metadata WHERE asset_details_json IS NOT NULL",
        )
        .fetch_all(&state.db)
        .await?;
        for (ids, assets) in rows {
            let ids = crate::media_edit::list(&ids);
            for (index, asset) in crate::media_edit::list(&assets).iter().enumerate() {
                if crate::emby::library_ignored(
                    &config,
                    &crate::state::string(asset, "source_library_id"),
                ) && let Some(id) = asset["emby_item_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .or_else(|| ids.get(index).and_then(Value::as_str))
                {
                    ignored_ids.insert(id.to_string());
                }
            }
        }
    }
    let mut media = vec![];
    context
        .activity("扫描媒体", "正在收集需要角色中文化的电影和电视剧")
        .await;
    for library in crate::emby::processing_libraries(state).await? {
        media.extend(
            crate::emby::items(state, &library, "Id,Type,Name,MediaSources", context)
                .await?
                .into_iter()
                .filter(|i| matches!(i["Type"].as_str(), Some("Movie" | "Series"))),
        );
    }
    crate::media_order::sort(&mut media);
    Ok(json!({"media":media,"ignored_ids":ignored_ids}))
    }).await?;
    let mut media = crate::media_edit::list(&plan["media"]);
    let ignored_ids: std::collections::HashSet<String> =
        serde_json::from_value(plan["ignored_ids"].clone())
            .map_err(|_| ApiError::bad("角色中文化媒体范围无效"))?;
    let reset_key = "tasks:role-translation:reset";
    if full && context.completed(reset_key).await?.is_none() {
        context.checkpoint().await?;
        let mut transaction = state.db.begin().await?;
        sqlx::query("DELETE FROM processed_log WHERE NOT (item_id=ANY($1))")
            .bind(ignored_ids.iter().cloned().collect::<Vec<_>>())
            .execute(&mut *transaction)
            .await?;
        context
            .complete_in_tx(&mut transaction, reset_key, &json!({"done":true}))
            .await?;
        transaction.commit().await?;
    }
    let cleanup_key = "tasks:role-translation:cleanup";
    if let Some(report) = context.completed(cleanup_key).await? {
        warnings += report["warnings"].as_u64().unwrap_or(0);
        warning_details.extend(crate::media_edit::list(&report["warning_details"]));
    } else {
        if !media.is_empty() {
            let mut alive = ignored_ids;
            for item in &media {
                alive.insert(crate::state::string(item, "Id"));
                if item["Type"] == "Movie" {
                    for source in crate::media_edit::list(&item["MediaSources"]) {
                        let id = source["ItemId"]
                            .as_str()
                            .filter(|id| !id.is_empty())
                            .or_else(|| source["Id"].as_str())
                            .unwrap_or("")
                            .trim_start_matches("mediasource_")
                            .to_string();
                        if !id.is_empty() {
                            alive.insert(id);
                        }
                    }
                }
            }
            let candidates: Vec<String> =
                sqlx::query_scalar("SELECT item_id FROM processed_log WHERE NOT (item_id=ANY($1))")
                    .bind(alive.into_iter().collect::<Vec<_>>())
                    .fetch_all(&state.db)
                    .await?;
            // A source ID is not proof that its native version disappeared.
            let removed = match crate::emby::existing_ids(state, &candidates, Some(context)).await {
                Ok(existing) => candidates
                    .into_iter()
                    .filter(|id| !existing.contains(id))
                    .collect::<Vec<_>>(),
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => {
                    let reason = format!(
                        "角色中文化清理未确认媒体存活状态，已保留处理和复核记录：{}",
                        error.1
                    );
                    tracing::warn!("{reason}");
                    warnings += 1;
                    warning_details.push(json!(reason));
                    vec![]
                }
            };
            let mut transaction = state.db.begin().await?;
            if !removed.is_empty() {
                for table in ["failed_log", "processed_log"] {
                    sqlx::query(&format!("DELETE FROM {table} WHERE item_id=ANY($1)"))
                        .bind(&removed)
                        .execute(&mut *transaction)
                        .await?;
                }
                tracing::info!(
                    "[角色中文化] 已清理 {} 个已删除媒体项的处理和复核记录",
                    removed.len()
                );
            }
            context
                .complete_in_tx(
                    &mut transaction,
                    cleanup_key,
                    &json!({"warnings":warnings,"warning_details":warning_details}),
                )
                .await?;
            transaction.commit().await?;
        } else {
            context
                .complete(cleanup_key, &json!({"warnings":0,"warning_details":[]}))
                .await?;
        }
    }
    let mut updated = 0;
    let mut failures = vec![];
    let mut processed = false;
    crate::media_order::sort(&mut media);
    let total = media.len();
    for (index, item) in media.into_iter().enumerate() {
        context.checkpoint().await?;
        let id = crate::state::string(&item, "Id");
        let name = item["Name"].as_str().unwrap_or(&id);
        let key = format!("tasks:role-translation:item:{id}");
        if let Some(report) = context.completed(&key).await? {
            updated += report["updated"].as_u64().unwrap_or(0);
            warnings += report["warnings"].as_u64().unwrap_or(0);
            warning_details.extend(crate::media_edit::list(&report["warning_details"]));
            continue;
        }
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
            context.complete(&key, &json!({"updated":0})).await?;
            continue;
        }
        if processed {
            state.item_delay(context).await?;
        }
        processed = true;
        let item_context = context.scoped(&format!("tasks:role-translation:work:{id}"));
        let result = if full {
            let item = crate::media_edit::details(
                state,
                &id,
                "Id,Type,Name,Path,ProviderIds,MediaSources,People,Genres",
            )
            .await?;
            Box::pin(crate::ingest::scrape(
                state,
                &item,
                &[],
                None,
                &item_context,
            ))
            .await
        } else {
            Box::pin(crate::webhook::process_emby(
                state,
                &json!({"id":id,"episode_ids":[],"event":"item.add"}),
                &item_context,
            ))
            .await
        };
        match result {
            Ok(report) => {
                updated += 1;
                warnings += report["warnings"].as_u64().unwrap_or(0);
                warning_details.extend(crate::media_edit::list(&report["warning_details"]));
                context.complete(&key, &json!({"updated":1,"warnings":report["warnings"],"warning_details":report["warning_details"]})).await?;
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
        json!({"updated":updated,"failures":failures,"warnings":warnings,"warning_details":warning_details,"message":format!("角色中文化完成：处理 {updated} 项，失败 {} 项，警告 {warnings} 条", failures.len())}),
    )
}

async fn enrich_persons(
    state: &AppState,
    full: bool,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    context.checkpoint().await?;
    let plan = task_plan(context, "tasks:enrich-aliases:plan", async {
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM person_metadata p WHERE $1 OR ((imdb_id IS NULL OR profile_path IS NULL OR gender IS NULL OR original_name IS NULL) AND (last_updated_at IS NULL OR last_updated_at<NOW()-INTERVAL '30 days')) ORDER BY last_updated_at ASC NULLS FIRST,tmdb_person_id").bind(full).fetch_all(&state.db).await?;
        Ok(json!(rows))
    }).await?;
    let rows = crate::media_edit::list(&plan);
    let mut updated = 0;
    let mut deleted = 0;
    let mut failures = vec![];
    let total = rows.len();
    let mut index = 0;
    let mut pending_rows = vec![];
    for row in rows {
        let key = format!("tasks:enrich-aliases:person:{}", row["tmdb_person_id"]);
        if let Some(result) = context.completed(&key).await? {
            updated += result["updated"].as_u64().unwrap_or(0);
            deleted += result["deleted"].as_u64().unwrap_or(0);
            index += 1;
        } else {
            pending_rows.push(row);
        }
    }
    use futures_util::{FutureExt, StreamExt, stream};
    let limit = crate::strm::thread_limit(state, "secondary").await;
    let pending = stream::iter(pending_rows)
        .map(|row| {
            async move {
                let id = row["tmdb_person_id"].as_i64().unwrap() as i32;
                let result = crate::tmdb::person(state, id).await;
                (row, result)
            }
            .boxed()
        })
        .buffer_unordered(limit);
    tokio::pin!(pending);
    while let Some((row, result)) = pending.next().await {
        context.checkpoint().await?;
        let id = row["tmdb_person_id"].as_i64().unwrap() as i32;
        let key = format!("tasks:enrich-aliases:person:{id}");
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
        match result {
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
                context
                    .complete_in_tx(&mut transaction, &key, &json!({"updated":1,"deleted":0}))
                    .await?;
                transaction.commit().await?;
                updated += 1;
                tracing::info!("[演员数据补充] {name}（TMDb {id}）数据已更新");
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) if error.0 == StatusCode::NOT_FOUND => {
                let mut transaction = state.db.begin().await?;
                let count = sqlx::query("DELETE FROM person_metadata WHERE tmdb_person_id=$1")
                    .bind(id)
                    .execute(&mut *transaction)
                    .await?
                    .rows_affected();
                context
                    .complete_in_tx(
                        &mut transaction,
                        &key,
                        &json!({"updated":0,"deleted":count}),
                    )
                    .await?;
                transaction.commit().await?;
                deleted += count;
                tracing::info!(
                    "[演员数据补充] {name}（TMDb {id}）在 TMDb 已不存在，已清理本地记录"
                );
            }
            Err(error) => {
                tracing::warn!("[演员数据补充] {name}（TMDb {id}）失败：{}", error.1);
                failures.push(format!("{id}: {}", error.1));
            }
        }
        index += 1;
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
    context.checkpoint().await?;
    let plan = task_plan(context, "tasks:restore-nfo-and-images:plan", async {
        let mut rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Series') AND tmdb_id NOT IN ('0','None','null','')").fetch_all(&state.db).await?;
        crate::media_order::sort(&mut rows);
        Ok(json!(rows))
    }).await?;
    let rows = crate::media_edit::list(&plan);
    let config = state.config.read().await.clone();
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let studio = crate::db::setting(&state.db, "studio_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_STUDIO_MAPPING"].clone());
    let keywords = crate::db::setting(&state.db, "keyword_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_KEYWORD_MAPPING"].clone());
    let mut updated = 0;
    let mut written = 0;
    let mut images = 0;
    let mut failures = vec![];
    let mut warning_details = vec![];
    for (index, row) in rows.iter().enumerate() {
        context.checkpoint().await?;
        let id = crate::state::string(row, "tmdb_id");
        let kind = crate::state::string(row, "item_type");
        let title = crate::state::string(row, "title");
        let key = format!("tasks:restore-nfo-and-images:{kind}:{id}");
        if let Some(result) = context.completed(&key).await? {
            updated += result["updated"].as_u64().unwrap_or(0);
            written += result["written"].as_u64().unwrap_or(0);
            images += result["images"].as_u64().unwrap_or(0);
            warning_details.extend(crate::media_edit::list(&result["warning_details"]));
            continue;
        }
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
                return Ok((0, 0, vec![]));
            }
            let (payload, cast) =
                Box::pin(crate::metadata::reconstruct_full(state, &id, &kind, true))
                    .await?
                    .ok_or_else(|| ApiError::bad("缺失元数据缓存"))?;
            let mut movie_paths = std::collections::BTreeSet::new();
            if kind == "Movie" {
                for asset in &assets {
                    let path = crate::state::string(asset, "path");
                    if !path.is_empty()
                        && !path.starts_with("http")
                        && !crate::emby::library_ignored(
                            &config,
                            &crate::state::string(asset, "source_library_id"),
                        )
                        && !crate::emby::path_ignored(state, &path).await?
                    {
                        movie_paths.insert(std::path::PathBuf::from(path));
                    }
                }
            }
            let movie_paths = movie_paths.into_iter().collect::<Vec<_>>();
            let options = crate::nfo_files::Options {
                movie_paths: &movie_paths,
                config: &config,
                studio_mapping: &studio,
                keyword_mapping: &keywords,
                ..Default::default()
            };
            context.checkpoint().await?;
            let item = json!({"Path":path,"Type":kind});
            let nfo_key = format!("{key}:nfo");
            let nfo = if let Some(result) = context.completed(&nfo_key).await? {
                result["written"].as_u64().unwrap_or(0)
            } else {
                verify_restore_bindings(state, row, &config).await?;
                let count = Box::pin(crate::nfo_files::sync(
                    &kind, &item, &payload, &cast, &options,
                ))
                .await? as u64;
                context
                    .complete(&nfo_key, &json!({"written":count}))
                    .await?;
                count
            };
            let images_key = format!("{key}:images");
            let images = if let Some(result) = context.completed(&images_key).await? {
                result
            } else {
                verify_restore_bindings(state, row, &config).await?;
                let report = Box::pin(crate::images::download_report(
                    state, &kind, &id, &item, &payload, context,
                ))
                .await?;
                context.complete(&images_key, &report).await?;
                report
            };
            Ok::<_, ApiError>((
                nfo,
                images["downloaded"].as_u64().unwrap_or(0),
                crate::media_edit::list(&images["warning_details"]),
            ))
        }
        .await;
        match result {
            Ok((nfo, count, warnings)) => {
                updated += 1;
                written += nfo;
                images += count;
                context.complete(&key, &json!({"updated":1,"written":nfo,"images":count,"warning_details":warnings})).await?;
                warning_details.extend(warnings);
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
        "NFO和封面还原完成：处理 {updated} 项，写入 {written} 个 NFO、{images} 张图片，失败 {} 项，警告 {} 条",
        failures.len(),
        warning_details.len()
    );
    tracing::info!("[NFO和封面还原] {message}");
    context
        .update_counted(
            rows.len() as u64,
            rows.len() as u64,
            "恢复NFO和封面",
            &message,
        )
        .await;
    Ok(
        json!({"updated":updated,"written":written,"images":images,"failures":failures,"warnings":warning_details.len(),"warning_details":warning_details,"message":message}),
    )
}

async fn verify_restore_bindings(state: &AppState, frozen: &Value, config: &Value) -> Result<()> {
    let id = crate::state::string(frozen, "tmdb_id");
    let kind = crate::state::string(frozen, "item_type");
    let current = crate::metadata::row(state, &id, &kind)
        .await?
        .filter(|row| row["in_library"] == true)
        .ok_or_else(|| ApiError::bad("原NFO恢复目标已不在库，未追随新媒体路径"))?;
    let original_ids = crate::media_edit::list(&frozen["emby_item_ids_json"]);
    let current_ids = crate::media_edit::list(&current["emby_item_ids_json"]);
    let current_assets = crate::media_edit::list(&current["asset_details_json"]);
    for (index, asset) in crate::media_edit::list(&frozen["asset_details_json"])
        .iter()
        .enumerate()
    {
        let path = crate::state::string(asset, "path");
        if path.is_empty()
            || path.starts_with("http")
            || crate::emby::library_ignored(
                config,
                &crate::state::string(asset, "source_library_id"),
            )
        {
            continue;
        }
        let native_id = asset["emby_item_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .or_else(|| original_ids.get(index).and_then(Value::as_str))
            .unwrap_or("");
        if native_id.is_empty() || !current_ids.contains(&json!(native_id)) {
            return Err(ApiError::bad(
                "原NFO恢复目标的媒体ID绑定已变化，未写入旧路径",
            ));
        }
        let matches = current_assets
            .iter()
            .enumerate()
            .filter(|(index, current)| {
                let current_id = current["emby_item_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .or_else(|| current_ids.get(*index).and_then(Value::as_str))
                    .unwrap_or("");
                current_id == native_id
                    && crate::media_paths::clean(&crate::state::string(current, "path"))
                        == crate::media_paths::clean(&path)
                    && !crate::emby::library_ignored(
                        config,
                        &crate::state::string(current, "source_library_id"),
                    )
            })
            .count();
        if matches != 1 {
            return Err(ApiError::bad(
                "原NFO恢复目标的路径绑定已变化，未写入旧目录或新路径",
            ));
        }
        let native =
            crate::media_edit::details(state, native_id, "Id,Type,Path,ProviderIds").await?;
        let tmdb = crate::state::string(&native["ProviderIds"], "Tmdb");
        if native["Id"] != native_id
            || native["Type"] != kind
            || crate::media_paths::clean(&crate::state::string(&native, "Path"))
                != crate::media_paths::clean(&path)
            || (!tmdb.is_empty() && tmdb != id)
        {
            return Err(ApiError::bad(
                "原NFO恢复目标的原生ID、路径或作品绑定已变化，未覆盖旧目录",
            ));
        }
    }
    Ok(())
}

async fn purge_persons(state: &AppState, context: &crate::jobs::JobContext) -> Result<Value> {
    context.checkpoint().await?;
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
    let plan = task_plan(context, "tasks:purge-ghost-actors:plan", async {
        Ok(json!(
            people
                .iter()
                .filter(|p| !used.contains(&crate::state::string(p, "Id")))
                .cloned()
                .collect::<Vec<_>>()
        ))
    })
    .await?;
    let orphans = crate::media_edit::list(&plan);
    let existing_people = people
        .iter()
        .map(|person| crate::state::string(person, "Id"))
        .collect::<std::collections::HashSet<_>>();
    if orphans.is_empty() {
        return Ok(json!({"deleted":0,"message":"未发现幽灵演员"}));
    }
    context.checkpoint().await?;
    let login: Value = crate::emby::send(state.http.post(format!("{}/Users/AuthenticateByName",crate::state::string(&config,"emby_server_url").trim_end_matches('/')))
        .header("X-Emby-Authorization","MediaBrowser Client=\"EmbyToolkit\", Device=\"Server\", DeviceId=\"etk-rust\", Version=\"1.0\"")
        .json(&json!({"Username":config["emby_admin_user"],"Pw":config["emby_admin_pass"]}))).await?.error_for_status()?.json().await?;
    let token = crate::state::string(&login, "AccessToken");
    let user = crate::state::string(&login["User"], "Id");
    if token.is_empty() || user.is_empty() || login["User"]["Policy"]["IsAdministrator"] != true {
        return Err(ApiError::forbidden());
    }
    let mut deleted = 0;
    let mut failures = vec![];
    let total = orphans.len();
    for (index, person) in orphans.iter().enumerate() {
        context.checkpoint().await?;
        let id = crate::state::string(person, "Id");
        let name = crate::state::string(person, "Name");
        let key = format!("tasks:purge-ghost-actors:person:{id}");
        if let Some(result) = context.completed(&key).await? {
            deleted += result["deleted"].as_u64().unwrap_or(0);
            continue;
        }
        context
            .update_counted(
                index as u64,
                total as u64,
                "清理幽灵演员",
                &format!("正在删除未被引用的演员：{name}（Item {id}）"),
            )
            .await;
        // A resumed deletion may have already succeeded, or the person may now
        // be referenced. Never replay a delete from the old scan alone.
        if used.contains(&id) || !existing_people.contains(&id) {
            context
                .complete(&key, &json!({"deleted":0,"skipped":true}))
                .await?;
            continue;
        }
        let response = crate::emby::send(
            state
                .http
                .post(format!(
                    "{}/Items/{id}/DeletePerson",
                    crate::state::string(&config, "emby_server_url").trim_end_matches('/')
                ))
                .header("X-Emby-Token", &token)
                .query(&[("UserId", &user)]),
        )
        .await;
        match response {
            Ok(response) if response.status().is_success() => {
                match crate::emby::existing_ids(state, &[id.clone()], Some(context)).await {
                    Ok(existing) if !existing.contains(&id) => {
                        context.complete(&key, &json!({"deleted":1})).await?;
                        deleted += 1;
                        tracing::info!("[幽灵演员清理] 已删除 {name}（Item {id}）");
                    }
                    Err(error) if context.is_cancelled() => return Err(error),
                    _ => {
                        tracing::warn!(
                            "[幽灵演员清理] {name}（Item {id}）删除后未确认消失，未认定完成"
                        );
                        failures.push(id);
                    }
                }
            }
            Ok(response) => {
                context.checkpoint().await?;
                tracing::warn!(
                    "[幽灵演员清理] 删除 {name}（Item {id}）失败：HTTP {}",
                    response.status()
                );
                failures.push(id);
            }
            Err(error) => {
                context.checkpoint().await?;
                tracing::warn!(
                    "[幽灵演员清理] 删除 {name}（Item {id}）请求失败：{}",
                    error.without_url()
                );
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
    context.checkpoint().await?;
    context
        .activity("扫描媒体信息", "正在扫描需要备份的在库电影和分集")
        .await;
    let plan = task_plan(context, "tasks:backup-mediainfo:plan", async {
        let mut rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Episode')").fetch_all(&state.db).await?;
        crate::media_order::sort_rows(state, &mut rows).await?;
        Ok(json!(rows))
    }).await?;
    let rows = crate::media_edit::list(&plan);
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
            let id = asset["emby_item_id"]
                .as_str()
                .or_else(|| ids.get(index).and_then(Value::as_str))
                .unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let key = format!(
                "tasks:backup-mediainfo:{}:{}:{index}:{id}",
                crate::state::string(&row, "item_type"),
                crate::state::string(&row, "tmdb_id")
            );
            if let Some(result) = context.completed(&key).await? {
                updated += result["updated"].as_u64().unwrap_or(0);
                continue;
            }
            if crate::emby::path_ignored(state, &crate::state::string(asset, "path")).await? {
                context.complete(&key, &json!({"updated":0})).await?;
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
                let changed=target.asset["fingerprint_pending"]==true
                    || observed.0!=target.path
                    || matches!((crate::identity::size(&target.asset["size_bytes"]),observed.2),(Some(a),Some(b)) if a!=b)
                    || target.asset["fingerprint_target"].as_str().is_some_and(|t|!t.is_empty()&&t!=observed.1)
                    || pc.as_ref().is_some_and(|p|target.pc.as_ref()!=Some(p));
                if changed || target.sha.is_none() {
                    use sha2::Digest;
                    let revision = format!("{:x}",sha2::Sha256::digest(json!(observed).to_string().as_bytes()));
                    let verification = context.scoped(&format!("{key}:verify:{revision}"));
                    let result=Box::pin(crate::revalidation::ids(state,&[id.to_string()],&verification)).await?;
                    if result["failed"].as_u64().unwrap_or(0)>0{return Err(ApiError::bad("文件身份待校验，未备份"));}
                    target=crate::media_edit::target(state,id).await?;
                }
                verify_backup_target(&target,&observed)?;
                let reread = crate::media_edit::details(state,id,"Id,Type,Path,Size,MediaSources").await?;
                if crate::revalidation::observation(state,&reread).await!=observed{return Err(ApiError::bad("媒体文件在指纹验证期间变化，未按旧SHA1备份"));}
                let sha=target.sha.as_ref().ok_or_else(||ApiError::bad("缺失SHA1"))?;
                if target.asset["fingerprint_pending"]==true{return Err(ApiError::bad("文件身份待校验"));}
                let cached=sqlx::query_scalar::<_,Option<Value>>("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1").bind(sha).fetch_optional(&state.db).await?.flatten();
                if cached.as_ref().is_some_and(|info|crate::media_info::recoverable(info)
                    && crate::media_info::restore_json(info).is_ok()
                    && crate::identity::verify(&target.asset,info).is_ok()
                    && crate::identity::verify(&json!({"size_bytes":observed.2}),info).is_ok()) {return Ok(false);}
                if target.path.starts_with("http"){return Ok(false);}
                for path in &target.candidates {
                    if !path.is_file(){continue;}
                    let info:Value=serde_json::from_slice(&tokio::fs::read(path).await?).map_err(|_|ApiError::bad("媒体信息JSON无效"))?;
                    if !crate::media_info::recoverable(&info){continue;}
                    let info=crate::media_info::restore_json(&info)?;
                    crate::identity::verify(&target.asset,&info)?;
                    crate::identity::verify(&json!({"size_bytes":observed.2}),&info)?;
                    if !crate::media_paths::matches_container(&info,&observed.1){return Err(ApiError::bad("媒体信息容器与当前文件不一致"));}
                    let current=crate::media_edit::details(state,id,"Id,Type,Path,Size,MediaSources").await?;
                    if crate::revalidation::observation(state,&current).await!=observed{return Err(ApiError::bad("文件在备份期间变化"));}
                    let latest = crate::media_edit::target(state,id).await?;
                    verify_backup_target(&latest,&observed)?;
                    if latest.sha!=target.sha || latest.pc!=target.pc || latest.asset!=target.asset{return Err(ApiError::bad("备份目标的指纹或版本绑定在保存前变化"));}
                    let mut transaction = state.db.begin().await?;
                    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json").bind(sha).bind(info).execute(&mut *transaction).await?;
                    context.complete_in_tx(&mut transaction, &key, &json!({"updated":1})).await?;
                    transaction.commit().await?;
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
                    context.complete(&key, &json!({"updated":0})).await?;
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
                            sqlx::query("INSERT INTO failed_log(item_id,item_name,item_type,reason,error_message) VALUES($1,$2,$3,$4,$4) ON CONFLICT(item_id) DO UPDATE SET reason=EXCLUDED.reason,error_message=EXCLUDED.error_message,failed_at=NOW()")
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

fn verify_backup_target(
    target: &crate::media_edit::Target,
    observed: &(String, String, Option<u64>),
) -> Result<()> {
    let pc = crate::fingerprints::pickcode(&observed.1);
    if target.path != observed.0
        || target.asset["fingerprint_pending"] == true
        || matches!((crate::identity::size(&target.asset["size_bytes"]),observed.2),(Some(a),Some(b)) if a!=b)
        || pc.as_ref().is_some_and(|pc| target.pc.as_ref() != Some(pc))
        || target.asset["fingerprint_target"]
            .as_str()
            .is_some_and(|value| !value.is_empty() && value != observed.1)
    {
        return Err(ApiError::bad(
            "当前媒体观察与指纹目标不一致，未按旧SHA1备份",
        ));
    }
    Ok(())
}
