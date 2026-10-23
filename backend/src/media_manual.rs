use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/media_for_editing/{id}", get(edit))
        .route("/api/update_media_cast_sa/{id}", post(submit))
        .route("/api/actions/translate_cast_sa", post(translate))
}
async fn record(state: &AppState, id: &str, item: &Value) -> Result<Value> {
    let bound: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1 LIMIT 1",
    )
    .bind(json!([id]))
    .fetch_optional(&state.db)
    .await?;
    if let Some(bound) = bound {
        return Ok(bound);
    }
    crate::metadata::row(
        state,
        &string(&item["ProviderIds"], "Tmdb"),
        &string(item, "Type"),
    )
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "未找到媒体元数据缓存".into()))
}
async fn cast(state: &AppState, row: &Value, item: &Value) -> Result<Vec<Value>> {
    let mut links = list(&row["actors_json"]);
    if links.is_empty() && row["item_type"] == "Season" {
        let rows:Vec<Value>=sqlx::query_scalar("SELECT actors_json FROM media_metadata WHERE parent_series_tmdb_id=$1 AND item_type='Episode' AND season_number=$2 AND actors_json IS NOT NULL ORDER BY episode_number").bind(string(row,"parent_series_tmdb_id")).bind(row["season_number"].as_i64().map(|n|n as i32)).fetch_all(&state.db).await?;
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            links.extend(
                list(&row)
                    .into_iter()
                    .filter(|a| seen.insert(a["tmdb_id"].to_string())),
            );
        }
    }
    if links.is_empty()
        && !string(row, "parent_series_tmdb_id").is_empty()
        && let Some(parent) =
            crate::metadata::row(state, &string(row, "parent_series_tmdb_id"), "Series").await?
    {
        links = list(&parent["actors_json"]);
    }
    if links.is_empty() {
        links = list(&item["People"])
            .iter()
            .enumerate()
            .filter(|(_, p)| p["Type"] == "Actor")
            .filter_map(|(i, p)| {
                let id = string(&p["ProviderIds"], "Tmdb").parse::<i32>().ok()?;
                Some(json!({"tmdb_id":id,"name":p["Name"],"character":p["Role"],"order":i}))
            })
            .collect();
    }
    let mut result = vec![];
    for link in links {
        let Some(id) = link["tmdb_id"].as_i64().and_then(|n| i32::try_from(n).ok()) else {
            continue;
        };
        let cached:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('name',primary_name,'id',tmdb_person_id) FROM person_metadata p WHERE tmdb_person_id=$1").bind(id).fetch_optional(&state.db).await?;
        let mut actor = cached.unwrap_or(json!({}));
        if (string(&actor, "name").is_empty()
            || actor["name"] == "Unknown"
            || string(&actor, "profile_path").is_empty())
            && !string(&*state.config.read().await, "tmdb_api_key").is_empty()
            && let Ok(person) = crate::tmdb::person(state, id).await
        {
            crate::persons::upsert(state, &person).await?;
            actor = person;
        }
        let person = list(&item["People"])
            .into_iter()
            .find(|p| string(&p["ProviderIds"], "Tmdb") == id.to_string())
            .unwrap_or(json!({}));
        actor["id"] = json!(id);
        if string(&actor, "name").is_empty() {
            actor["name"] = link
                .get("name")
                .or_else(|| person.get("Name"))
                .cloned()
                .unwrap_or(json!(format!("TMDb {id}")));
        }
        actor["character"] = link
            .get("character")
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            .or_else(|| person.get("Role"))
            .cloned()
            .unwrap_or(json!(""));
        actor["order"] = link.get("order").cloned().unwrap_or(json!(result.len()));
        actor["emby_person_id"] = person["Id"].clone();
        result.push(actor);
    }
    result.sort_by_key(|a| a["order"].as_i64().unwrap_or(999));
    Ok(result)
}
fn front(actor: &Value) -> Value {
    let path = string(actor, "profile_path");
    json!({"tmdbId":actor["id"],"name":actor["name"],"role":crate::cast::clean_character(&string(actor,"character")),"imageUrl":if path.is_empty(){Value::Null}else{json!(if path.starts_with("http"){path}else{format!("https://image.tmdb.org/t/p/w185{path}")})},"emby_person_id":actor["emby_person_id"]})
}
async fn edit(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    let result = edit_data(&state, &id).await;
    match &result {
        Ok(Json(data)) => {
            tracing::info!(item_id = %id, actors = data["current_emby_cast"].as_array().map_or(0, Vec::len), "打开演员编辑页完成")
        }
        Err(error) => tracing::warn!(item_id = %id, error = %error.1, "打开演员编辑页失败"),
    }
    result
}
async fn edit_data(state: &AppState, id: &str) -> Result<Json<Value>> {
    let item = details(
        state,
        id,
        "Name,Type,ProviderIds,People,ImageTags,ProductionYear",
    )
    .await?;
    let row = record(state, id, &item).await?;
    let actors = cast(state, &row, &item).await?;
    let review: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(f) FROM failed_log f WHERE item_id=$1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let review = review.unwrap_or(json!({}));
    let title = string(&item, "Name");
    let year = item["ProductionYear"].as_i64();
    let query = if let Some(year) = year {
        format!("{title} {year}")
    } else {
        title.clone()
    };
    let encoded: String = url_encode(&query);
    Ok(Json(
        json!({"item_id":id,"item_name":item["Name"],"item_type":item["Type"],"image_tag":item["ImageTags"]["Primary"],"original_score":review["score"],"review_reason":review["error_message"],"current_emby_cast":actors.iter().map(front).collect::<Vec<_>>(),"search_links":{"baidu":format!("https://www.baidu.com/s?wd={encoded}"),"wikipedia":format!("https://www.google.com/search?q={encoded}+site%3Azh.wikipedia.org"),"google":format!("https://www.google.com/search?q={encoded}")}}),
    ))
}
fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    if !body["cast"].is_array() {
        return Err(ApiError::bad("请求体中缺少有效的 'cast' 列表"));
    }
    let work = state.clone();
    let task = state
        .jobs
        .submit("手动更新演员与NFO", move |context| async move {
            save(&work, &id, &body, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"手动更新任务已在后台启动。"})),
    ))
}
pub async fn save(state: &AppState, id: &str, body: &Value, context: &JobContext) -> Result<Value> {
    context.checkpoint().await?;
    let item=details(state,id,"Id,Type,Name,Path,People,ProviderIds,Genres,ParentId,ParentIndexNumber,IndexNumber,SeriesId").await?;
    let row = record(state, id, &item).await?;
    let kind = string(&row, "item_type");
    let tmdb = string(&row, "tmdb_id");
    let original = cast(state, &row, &item).await?;
    let (payload, _) = crate::metadata::reconstruct_full(state, &tmdb, &kind, true)
        .await?
        .ok_or_else(|| ApiError::bad("缺失元数据缓存"))?;
    let mut actors = vec![];
    for (order, input) in list(&body["cast"]).iter().enumerate() {
        let actor_id = input["tmdbId"]
            .as_i64()
            .or_else(|| input["tmdbId"].as_str().and_then(|s| s.parse().ok()))
            .filter(|n| *n > 0 && *n <= i32::MAX as i64)
            .ok_or_else(|| ApiError::bad("演员TMDb ID无效"))?;
        let mut actor =
            if let Some(actor) = original.iter().find(|a| a["id"].as_i64() == Some(actor_id)) {
                actor.clone()
            } else {
                crate::tmdb::person(state, actor_id as i32).await?
            };
        actor["id"] = json!(actor_id);
        actor["name"] = input["name"].clone();
        actor["character"] = json!(crate::cast::clean_character(&string(input, "role")));
        actor["order"] = json!(order);
        if string(&actor, "name").trim().is_empty() {
            return Err(ApiError::bad("演员名不能为空"));
        }
        actors.push(actor);
    }
    let config = state.config.read().await.clone();
    let actors = crate::cast::manual(&actors, &item, &config);
    context.checkpoint().await?;
    for actor in &actors {
        crate::persons::upsert(state, actor).await?;
    }
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let studio = crate::db::setting(&state.db, "studio_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_STUDIO_MAPPING"].clone());
    let keywords = crate::db::setting(&state.db, "keyword_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_KEYWORD_MAPPING"].clone());
    let options = crate::nfo_files::Options {
        force: true,
        config: &config,
        studio_mapping: &studio,
        keyword_mapping: &keywords,
        ..Default::default()
    };
    let written = crate::nfo_files::sync(&kind, &item, &payload, &actors, &options).await?;
    let path = std::path::PathBuf::from(string(&item, "Path"));
    let nfo = match kind.as_str() {
        "Season" => path.join("season.nfo"),
        "Series" => path.join("tvshow.nfo"),
        _ => path.with_extension("nfo"),
    };
    if !nfo.is_file() {
        return Err(ApiError::bad("目标NFO未生成"));
    }
    // Update only people referenced by the current item, with a fresh complete Person record.
    for actor in &actors {
        let actor_id = actor["id"].as_i64().unwrap().to_string();
        if let Some(person) = list(&item["People"])
            .iter()
            .find(|p| string(&p["ProviderIds"], "Tmdb") == actor_id)
            && person["Name"] != actor["name"]
        {
            let person_id = string(person, "Id");
            let mut person = details(state, &person_id, "Name").await?;
            person["Name"] = actor["name"].clone();
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Items/{person_id}"),
                    &[],
                    Some(person),
                )
                .await?;
        }
    }
    let refresh = if kind == "Episode" {
        string(&item, "ParentId")
    } else {
        id.to_string()
    };
    if refresh.is_empty() {
        return Err(ApiError::bad("无法定位目标Season刷新"));
    }
    crate::strm::with_settings(state, "preserve", || {
        crate::emby::refresh_item(state, &refresh, true, context)
    })
    .await?;
    let mut transaction = state.db.begin().await?;
    sqlx::query("UPDATE media_metadata SET actors_json=$1,last_updated_at=NOW() WHERE tmdb_id=$2 AND item_type=$3").bind(crate::cast::relations(&actors)).bind(&tmdb).bind(&kind).execute(&mut *transaction).await?;
    for actor in &actors {
        if let Some(old) = original.iter().find(|a| a["id"] == actor["id"]) {
            let before = crate::cast::clean_character(&string(old, "character"));
            let after = crate::cast::clean_character(&string(actor, "character"));
            if !after.is_empty() && before != after {
                sqlx::query("UPDATE translation_cache SET translated_text=$1,engine_used='manual',last_updated_at=NOW() WHERE translated_text=$2 AND original_text NOT LIKE 'ai-result:%'").bind(after).bind(before).execute(&mut *transaction).await?;
            }
        }
    }
    sqlx::query("INSERT INTO processed_log(item_id,item_name,score) VALUES($1,$2,10) ON CONFLICT(item_id) DO UPDATE SET score=10,processed_at=NOW()") .bind(id).bind(string(&item,"Name")).execute(&mut *transaction).await?;
    sqlx::query("DELETE FROM failed_log WHERE item_id=$1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(json!({"written":written,"message":"演员与目标NFO已更新"}))
}
async fn translate(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    if !body["cast"].is_array() {
        return Err(ApiError::bad("请求体必须包含 'cast' 列表。"));
    }
    let work = state.clone();
    let task = state
        .jobs
        .submit("翻译演员表", move |context| async move {
            context.checkpoint().await?;
            translate_cast(&work, &body).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"翻译演员表任务已提交"})),
    ))
}
pub async fn translate_cast(state: &AppState, body: &Value) -> Result<Value> {
    let mut actors = list(&body["cast"]);
    let config = state.config.read().await.clone();
    if config["ai_translate_actor_role"] != true {
        if let Some(actor) = actors.iter_mut().find(|a| {
            ["name", "role"]
                .iter()
                .any(|f| !string(a, f).is_empty() && !crate::translation::chinese(&string(a, f)))
        }) {
            actor["matchStatus"] = json!("AI未启用");
        }
        return Ok(json!(actors));
    }
    let mut inputs = BTreeMap::new();
    let mut contexts = HashMap::new();
    for actor in &actors {
        for field in ["name", "role"] {
            let mut text = string(actor, field).trim().to_string();
            if field == "role" {
                text = crate::cast::clean_character(&text);
            }
            if crate::translation::needs_translation(&text) {
                inputs.insert(text.clone(), text.clone());
                if field == "role" {
                    contexts.insert(text, string(actor, "name"));
                }
            }
        }
    }
    let quality = config["ai_translation_mode"] == "quality";
    let mut cached = BTreeMap::new();
    if !quality {
        for text in inputs.keys() {
            let value: Option<String> = sqlx::query_scalar(
                "SELECT translated_text FROM translation_cache WHERE original_text=$1",
            )
            .bind(text)
            .fetch_optional(&state.db)
            .await?
            .flatten();
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                cached.insert(text.clone(), value);
            }
        }
    }
    let missing = inputs
        .into_iter()
        .filter(|(s, _)| !cached.contains_key(s))
        .collect();
    let revision = state
        .ai
        .cache_revision
        .load(std::sync::atomic::Ordering::SeqCst);
    let translated = crate::ai::translate(
        state,
        &missing,
        if quality { "quality_mode" } else { "fast_mode" },
        &json!({"title":body["title"],"year":body["year"],"media_type":body["media_type"]}),
        &json!(contexts),
    )
    .await?;
    let _gate = state.ai.cache_gate.read().await;
    if !quality
        && revision
            == state
                .ai
                .cache_revision
                .load(std::sync::atomic::Ordering::SeqCst)
    {
        for (source, value) in &translated {
            sqlx::query("INSERT INTO translation_cache(original_text,translated_text,engine_used) VALUES($1,$2,$3) ON CONFLICT(original_text) DO UPDATE SET translated_text=EXCLUDED.translated_text,last_updated_at=NOW()") .bind(source).bind(value).bind(string(&config,"ai_provider")).execute(&state.db).await?;
        }
    }
    cached.extend(translated);
    for actor in &mut actors {
        let old = actor.clone();
        for field in ["name", "role"] {
            let text = if field == "role" {
                crate::cast::clean_character(&string(actor, field))
            } else {
                string(actor, field).trim().to_string()
            };
            if let Some(value) = cached.get(&text) {
                actor[field] = json!(value);
            }
        }
        if old["name"] != actor["name"] || old["role"] != actor["role"] {
            actor["matchStatus"] = json!("已翻译");
        }
    }
    Ok(json!(actors))
}
