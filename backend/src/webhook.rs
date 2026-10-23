use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::{details, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct Service {
    pending: Mutex<BTreeMap<String, Value>>,
    batches: Mutex<BTreeMap<String, Instant>>,
    enhanced: Mutex<BTreeMap<String, Instant>>,
    manual_deletes: Mutex<BTreeMap<String, Instant>>,
    draining: std::sync::atomic::AtomicBool,
}
impl Service {
    pub async fn mark_manual_delete(&self, ids: &[String]) {
        let now = Instant::now();
        let mut pending = self.manual_deletes.lock().await;
        pending.retain(|_, expires| *expires > now);
        for id in ids.iter().filter(|id| !id.is_empty()) {
            pending.insert(id.clone(), now + Duration::from_secs(180));
        }
    }
    pub async fn clear_manual_delete(&self, ids: &[String]) {
        let mut pending = self.manual_deletes.lock().await;
        for id in ids {
            pending.remove(id);
        }
    }
    pub async fn manual_delete_marked(&self, id: &str) -> bool {
        let now = Instant::now();
        let mut pending = self.manual_deletes.lock().await;
        pending.retain(|_, expires| *expires > now);
        pending.contains_key(id)
    }
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhook/emby", post(emby))
        .route("/webhook/moviepilot", post(moviepilot))
}
async fn verify(
    state: &AppState,
    headers: &HeaderMap,
    query: &BTreeMap<String, String>,
) -> Result<()> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let expected = string(&*state.config.read().await, "emby_webhook_secret");
    let provided = headers
        .get("X-Emby-Toolkit-Token")
        .or_else(|| headers.get("X-Webhook-Secret"))
        .and_then(|s| s.to_str().ok())
        .or_else(|| {
            query
                .get("token")
                .or_else(|| query.get("secret"))
                .map(String::as_str)
        })
        .unwrap_or("")
        .trim();
    let mut actual = Hmac::<Sha256>::new_from_slice(b"webhook verification").unwrap();
    actual.update(provided.as_bytes());
    let mut correct = Hmac::<Sha256>::new_from_slice(b"webhook verification").unwrap();
    correct.update(expected.trim().as_bytes());
    if expected.trim().is_empty()
        || correct
            .verify_slice(&actual.finalize().into_bytes())
            .is_err()
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Webhook 共享密钥无效".into(),
        ));
    }
    Ok(())
}
async fn enqueue(state: &AppState, key: String, event: Value) {
    let mut pending = state.webhooks.pending.lock().await;
    if let Some(old) = pending.get_mut(&key) {
        let ids = list(&old["episode_ids"])
            .into_iter()
            .chain(list(&event["episode_ids"]))
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect::<std::collections::BTreeSet<_>>();
        old["episode_ids"] = json!(ids);
    } else {
        pending.insert(key, event);
    }
    drop(pending);
    if state
        .webhooks
        .draining
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if state.jobs.status().await["is_running"] == true {
                continue;
            }
            let next = state.webhooks.pending.lock().await.pop_first();
            let Some((key, event)) = next else {
                let pending = state.webhooks.pending.lock().await;
                if pending.is_empty() {
                    state
                        .webhooks
                        .draining
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                continue;
            };
            let work = state.clone();
            let retry_event = event.clone();
            match state
                .jobs
                .submit("Webhook 入库", move |context| async move {
                    if event["source"] == "moviepilot" {
                        let result = process_moviepilot(&work, &event, &context).await;
                        if result.as_ref().map_or(true, |v| {
                            v["failures"].as_array().is_some_and(|f| !f.is_empty())
                        }) && let Some(key) = event["batch_key"].as_str()
                        {
                            work.webhooks.batches.lock().await.remove(key);
                        }
                        result
                    } else {
                        process_emby(&work, &event, &context).await
                    }
                })
                .await
            {
                Ok(_) => (),
                Err(error) => {
                    state
                        .webhooks
                        .pending
                        .lock()
                        .await
                        .entry(key)
                        .or_insert(retry_event);
                    tracing::warn!("Webhook 等待任务调度：{}", error.1);
                }
            }
        }
    });
}
async fn emby(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    verify(&state, &headers, &query).await?;
    let event = string(&body, "Event");
    if ![
        "item.add",
        "library.new",
        "metadata.update",
        "image.update",
        "deep.delete",
        "None",
    ]
    .contains(&event.as_str())
    {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"event_ignored_not_in_trigger_list"})),
        ));
    }
    let item = &body["Item"];
    let id = string(item, "Id");
    let kind = string(item, "Type");
    if id.is_empty()
        || !matches!(
            kind.as_str(),
            "Movie" | "Series" | "Season" | "Episode" | "BoxSet"
        )
    {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"event_ignored_no_id_or_wrong_type"})),
        ));
    }
    let config = state.config.read().await.clone();
    if matches!(event.as_str(), "item.add" | "library.new")
        && config["emby_webhook_ingest_enabled"] == false
    {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"emby_ingest_disabled"})),
        ));
    }
    if event == "deep.delete" {
        return Ok((StatusCode::OK, Json(deep_delete(&state, item).await?)));
    }
    let path = if string(item, "Path").is_empty() {
        string(&details(&state, &id, "Path").await?, "Path")
    } else {
        string(item, "Path")
    };
    let library = crate::emby::library_for_path(&state, &path).await?;
    if library
        .as_ref()
        .is_some_and(|library| crate::emby::library_ignored(&config, &string(library, "ItemId")))
    {
        return Ok((StatusCode::OK, Json(json!({"status":"ignored_library"}))));
    }
    let (mut top, mut ids) = (id.clone(), vec![]);
    if kind == "Episode" {
        top = string(item, "SeriesId");
        if top.is_empty() {
            let episode = details(&state, &id, "SeriesId").await?;
            top = string(&episode, "SeriesId");
        }
        ids.push(id.clone());
    }
    if kind == "Season" {
        let season = details(&state, &id, "SeriesId,ParentId").await?;
        top = season["SeriesId"]
            .as_str()
            .or_else(|| season["ParentId"].as_str())
            .unwrap_or("")
            .to_string();
    }
    enqueue(
        &state,
        format!("emby:{top}:{event}"),
        json!({"source":"emby","id":top,"episode_ids":ids,"event":event,"original":item}),
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(json!({"status":"accepted"}))))
}

fn delete_path(raw: &str, config: &Value) -> Option<PathBuf> {
    let mut path = crate::media_paths::clean(raw);
    let local = crate::media_paths::clean(&string(config, "local_strm_root"));
    let cd2 = crate::media_paths::clean(&string(config, "cd2_media_root"));
    if !local.is_empty() && !cd2.is_empty() {
        if path == cd2.trim_end_matches('/') {
            path = local;
        } else if let Some(suffix) = path.strip_prefix(&format!("{}/", cd2.trim_end_matches('/'))) {
            path = format!("{}/{suffix}", local.trim_end_matches('/'));
        }
    }
    if path.is_empty() || path.starts_with("http://") || path.starts_with("https://") {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in std::path::Path::new(&path).components() {
        if component == std::path::Component::ParentDir {
            if !normalized.pop() {
                return None;
            }
        } else {
            normalized.push(component);
        }
    }
    (normalized.parent().is_some() && normalized != std::path::Path::new(".")).then_some(normalized)
}

struct DeleteSlot {
    id: String,
    kind: String,
    asset: Value,
    pc: String,
}
fn delete_slots(row: &Value) -> Vec<DeleteSlot> {
    let ids = list(&row["emby_item_ids_json"]);
    let assets = list(&row["asset_details_json"]);
    let pcs = list(&row["file_pickcode_json"]);
    ids.iter()
        .enumerate()
        .filter_map(|(index, id)| {
            let id = id.as_str()?.to_string();
            Some(DeleteSlot {
                id,
                kind: string(row, "item_type"),
                asset: assets.get(index).cloned().unwrap_or(Value::Null),
                pc: if pcs.len() == ids.len() {
                    pcs[index].as_str().unwrap_or("").trim().to_string()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}
async fn delete_fid(state: &AppState, slot: &DeleteSlot, config: &Value) -> Result<Option<String>> {
    if !matches!(slot.kind.as_str(), "Movie" | "Episode")
        || (!string(&slot.asset, "emby_item_id").is_empty()
            && string(&slot.asset, "emby_item_id") != slot.id)
    {
        return Ok(None);
    }
    if slot.asset["fingerprint_pending"] == true {
        return Err(ApiError::bad("文件身份待校验，停止 115 联动删除"));
    }
    let cached: Vec<String> = if !slot.pc.is_empty() {
        sqlx::query_scalar("SELECT id FROM p115_filesystem_cache WHERE pick_code=$1")
            .bind(&slot.pc)
            .fetch_all(&state.db)
            .await?
    } else {
        let path = string(&slot.asset, "path");
        let target = crate::media_paths::strm_target(&path)
            .await
            .unwrap_or_default();
        let paths = crate::media_paths::local_candidates(
            &path,
            &target,
            &[
                &string(config, "local_strm_root"),
                &string(config, "cd2_media_root"),
            ],
        );
        sqlx::query_scalar("SELECT id FROM p115_filesystem_cache WHERE trim(both '/' from replace(local_path,chr(92),'/'))=ANY($1)")
            .bind(paths).fetch_all(&state.db).await?
    };
    let fid = match cached.as_slice() {
        [fid] => Some(fid.clone()),
        [] if !slot.pc.is_empty() => {
            crate::fingerprints::file_id(&slot.pc).map(|id| id.to_string())
        }
        _ => None,
    };
    Ok(fid.filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())))
}
pub async fn deep_delete(state: &AppState, item: &Value) -> Result<Value> {
    let id = string(item, "Id");
    if state.webhooks.manual_delete_marked(&id).await {
        return Ok(json!({"status":"deep_delete_managed_by_etk"}));
    }
    // Neither a webhook nor an empty list response proves that an Item is gone.
    if crate::versions::existing(state, std::slice::from_ref(&id))
        .await?
        .contains(&id)
    {
        return Ok(json!({"status":"deep_delete_processed"}));
    }
    let config = state.config.read().await.clone();
    let kind = string(item, "Type");
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1",
    )
    .bind(json!([id]))
    .fetch_all(&state.db)
    .await?;
    let mut slots = vec![];
    for row in rows {
        let exact = delete_slots(&row);
        slots.extend(delete_slots(&row).into_iter().filter(|slot| {
            slot.id == id || matches!(kind.as_str(), "Movie" | "Episode") && slot.kind == kind
        }));
        if !matches!(kind.as_str(), "Series" | "Season") || row["item_type"] != kind {
            continue;
        }
        let roots: BTreeSet<_> = if !string(item, "Path").is_empty() {
            delete_path(&string(item, "Path"), &config)
                .into_iter()
                .collect()
        } else {
            exact
                .iter()
                .filter(|slot| slot.id == id)
                .filter_map(|slot| delete_path(&string(&slot.asset, "path"), &config))
                .collect()
        };
        if roots.is_empty() {
            continue;
        }
        let parent = if kind == "Series" {
            string(&row, "tmdb_id")
        } else {
            string(&row, "parent_series_tmdb_id")
        };
        let children: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE parent_series_tmdb_id=$1 AND in_library=true AND item_type IN ('Season','Episode') AND ($2='Series' OR (item_type='Episode' AND season_number=$3))")
            .bind(parent).bind(&kind).bind(row["season_number"].as_i64().map(|n| n as i32)).fetch_all(&state.db).await?;
        for child in children {
            slots.extend(delete_slots(&child).into_iter().filter(|slot| {
                let asset_id = string(&slot.asset, "emby_item_id");
                (asset_id.is_empty() || asset_id == slot.id)
                    && delete_path(&string(&slot.asset, "path"), &config).is_some_and(|path| {
                        roots
                            .iter()
                            .any(|root| path != *root && path.starts_with(root))
                    })
            }));
        }
    }
    let mut ids: BTreeSet<_> = slots.iter().map(|slot| slot.id.clone()).collect();
    ids.insert(id);
    let existing =
        crate::versions::existing(state, &ids.iter().cloned().collect::<Vec<_>>()).await?;
    let mut deleted = vec![];
    for id in ids {
        if !existing.contains(&id) && !state.webhooks.manual_delete_marked(&id).await {
            deleted.push(id);
        }
    }
    if config["p115_enable_sync_delete"] == true {
        let mut fids = BTreeSet::new();
        let mut retained_fids = BTreeSet::new();
        for slot in &slots {
            if let Some(fid) = delete_fid(state, slot, &config).await? {
                if deleted.contains(&slot.id) {
                    fids.insert(fid);
                } else {
                    retained_fids.insert(fid);
                }
            }
        }
        let fids = fids.difference(&retained_fids).cloned().collect::<Vec<_>>();
        if !fids.is_empty() {
            match crate::p115::delete_files(state, &fids).await {
                Ok(()) => {
                    sqlx::query("DELETE FROM p115_filesystem_cache WHERE id=ANY($1)")
                        .bind(&fids)
                        .execute(&state.db)
                        .await?;
                }
                Err(error) => tracing::error!("115 深度联动删除失败，保留文件缓存: {}", error.1),
            }
        }
    }
    crate::versions::unbind(state, &deleted).await?;
    Ok(json!({"status":"deep_delete_processed"}))
}
pub async fn collect_files(
    state: &AppState,
    root: &str,
    provided: &[Value],
) -> Result<Vec<String>> {
    let config = state.config.read().await.clone();
    let allowed = list(&config["monitor_paths"])
        .iter()
        .filter_map(|p| p.as_str())
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect::<Vec<_>>();
    if allowed.is_empty() {
        return Err(ApiError::bad(
            "ETK未配置实时监控目录，无法校验MoviePilot媒体路径",
        ));
    }
    let root = tokio::fs::canonicalize(root).await?;
    if !allowed.iter().any(|p| root.starts_with(p)) {
        return Err(ApiError::bad("MoviePilot根路径不在ETK实时监控目录内"));
    }
    let mut candidates = provided
        .iter()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        if root.is_file() {
            candidates.push(root.clone());
        } else {
            let mut pending = vec![root.clone()];
            while let Some(directory) = pending.pop() {
                let mut entries = tokio::fs::read_dir(directory).await?;
                while let Some(entry) = entries.next_entry().await? {
                    let kind = entry.file_type().await?;
                    if kind.is_file() {
                        candidates.push(entry.path());
                    } else if kind.is_dir() {
                        pending.push(entry.path());
                    }
                    if candidates.len() > 10000 {
                        return Err(ApiError::bad("MoviePilot完成批次超过10000个文件"));
                    }
                }
            }
        }
    }
    let extensions = list(&config["monitor_extensions"])
        .iter()
        .filter_map(Value::as_str)
        .map(|s| s.trim().trim_start_matches('.').to_lowercase())
        .collect::<Vec<_>>();
    let mut result = std::collections::BTreeSet::new();
    for candidate in candidates {
        let Ok(path) = tokio::fs::canonicalize(candidate).await else {
            continue;
        };
        if path.is_file()
            && path.starts_with(&root)
            && allowed.iter().any(|p| path.starts_with(p))
            && path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| extensions.contains(&s.to_lowercase()))
        {
            result.insert(path.to_string_lossy().into_owned());
        }
    }
    Ok(result.into_iter().collect())
}
async fn moviepilot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
    Json(mut body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    verify(&state, &headers, &query).await?;
    if body["event"] != "metadata.scrape.complete" {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"ignored","message":"unsupported event"})),
        ));
    }
    if body["success"] != true {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"ignored","message":"moviepilot scrape failed"})),
        ));
    }
    let root = string(&body, "root_path");
    if root.is_empty() {
        return Err(ApiError::bad("root_path is required"));
    }
    let media = string(&body, "media_type").to_lowercase();
    if !["movie", "电影", "tv", "series", "电视剧"].contains(&media.as_str()) {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"ignored","message":"unsupported media type"})),
        ));
    }
    if ["tv", "series", "电视剧"].contains(&media.as_str()) && body.get("episode_group").is_none()
    {
        return Err(ApiError::bad("episode_group is required for TV"));
    }
    let files = collect_files(&state, &root, &list(&body["file_list"])).await?;
    if files.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"ignored","message":"no media files"})),
        ));
    }
    let batch = string(&body, "batch_id");
    let key = if batch.is_empty() {
        use sha2::Digest;
        format!("digest:{:x}",sha2::Sha256::digest(json!({"root_path":root,"files":files,"media_type":body["media_type"],"tmdb_id":body["tmdb_id"]}).to_string().as_bytes()))
    } else {
        format!("batch:{batch}")
    };
    let mut batches = state.webhooks.batches.lock().await;
    batches.retain(|_, time| time.elapsed() < Duration::from_secs(3600));
    if batches.contains_key(&key) {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"duplicate","batch_key":key})),
        ));
    }
    batches.insert(key.clone(), Instant::now());
    drop(batches);
    body["source"] = json!("moviepilot");
    body["batch_key"] = json!(key);
    body["files"] = json!(files);
    enqueue(&state, key.clone(), body).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"status":"accepted","batch_key":key,"file_count":files.len(),"group_count":1})),
    ))
}
pub async fn process_emby(
    state: &AppState,
    event: &Value,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let id = string(event, "id");
    let item = details(state, &id, "Path").await?;
    if crate::emby::path_ignored(state, &string(&item, "Path")).await? {
        return Ok(json!({"updated":0,"status":"ignored_library"}));
    }
    let episodes = list(&event["episode_ids"])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if event["event"] == "metadata.update" {
        return mirror(state, &string(&event["original"], "Id")).await;
    }
    if event["event"] == "image.update" {
        return Ok(json!({"message":"图片更新已接收"}));
    }
    let enhanced = {
        let mut ids = state.webhooks.enhanced.lock().await;
        ids.retain(|_, t| t.elapsed() < Duration::from_secs(3600));
        ids.contains_key(&id)
    };
    let mut result = match crate::ingest::feedback(state, &id, &episodes, false, context).await {
        Ok(result) => Ok(result),
        Err(error) => {
            if enhanced {
                return Err(error);
            }
            tracing::info!("缓存回流不可用，执行完整刮削：{}", error.1);
            let item = details(
                state,
                &id,
                "Id,Type,Name,Path,People,Genres,ProviderIds,MediaSources,DateCreated",
            )
            .await?;
            let group =
                crate::metadata::row(state, &string(&item["ProviderIds"], "Tmdb"), "Series")
                    .await?
                    .map(|r| string(&r, "episode_group_id"));
            crate::ingest::scrape(state, &item, &episodes, group.as_deref(), context).await
        }
    }?;
    let finalization = finalize(state, &id, &episodes, !episodes.is_empty(), context).await?;
    result["finalization"] = finalization;
    if enhanced {
        state.webhooks.enhanced.lock().await.remove(&id);
    }
    Ok(result)
}

async fn mirror(state: &AppState, id: &str) -> Result<Value> {
    let item=details(state,id,"ProviderIds,Type,Name,OriginalTitle,Overview,Tags,TagItems,OfficialRating,CustomRating,PremiereDate,ProductionYear").await?;
    let kind = string(&item, "Type");
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    let Some(mut row) = crate::metadata::row(state, &tmdb, &kind).await? else {
        return Ok(json!({"updated":0}));
    };
    for (source, target) in [
        ("Name", "title"),
        ("OriginalTitle", "original_title"),
        ("Overview", "overview"),
        ("CustomRating", "custom_rating"),
    ] {
        row[target] = item[source].clone();
    }
    if let Some(rating) = item.get("OfficialRating").filter(|v| !v.is_null()) {
        if !row["official_rating_json"].is_object() {
            row["official_rating_json"] = json!({});
        }
        row["official_rating_json"]["US"] = rating.clone();
    }
    let tags = list(&item["Tags"])
        .into_iter()
        .chain(
            list(&item["TagItems"])
                .iter()
                .map(|v| v.get("Name").unwrap_or(v).clone()),
        )
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect::<std::collections::BTreeSet<_>>();
    row["tags_json"] = json!(tags);
    for (source, target) in [
        ("PremiereDate", "release_date"),
        ("ProductionYear", "release_year"),
    ] {
        if item[source] != Value::Null && item[source] != "" {
            row[target] = item[source].clone();
        }
    }
    let fields = [
        "title",
        "original_title",
        "overview",
        "custom_rating",
        "official_rating_json",
        "tags_json",
        "release_date",
        "release_year",
    ];
    let update = fields
        .iter()
        .map(|key| format!("{key}=r.{key}"))
        .collect::<Vec<_>>()
        .join(",");
    sqlx::query(&format!("UPDATE media_metadata SET {update},last_synced_at=NOW() FROM jsonb_populate_record(NULL::media_metadata,$1) r WHERE media_metadata.tmdb_id=$2 AND media_metadata.item_type=$3")).bind(row).bind(tmdb).bind(kind).execute(&state.db).await?;
    Ok(json!({"updated":1,"message":"Emby 元数据已同步"}))
}

async fn moviepilot_item(state: &AppState, path: &str) -> Result<Option<Value>> {
    let response = state
        .emby(
            reqwest::Method::GET,
            "Items",
            &[
                ("Path", path.into()),
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Movie,Episode,Video".into()),
                (
                    "Fields",
                    "Id,Type,SeriesId,ParentId,ExtraType,ProviderIds,Path,MediaSources,MediaStreams"
                        .into(),
                ),
            ],
            None,
        )
        .await?;
    Ok(list(&response["Items"]).into_iter().find(|item| {
        item["Path"] == path
            && !string(item, "Id").is_empty()
            && (matches!(item["Type"].as_str(), Some("Movie" | "Episode"))
                || item["Type"] == "Video" && item["ExtraType"] == "AdditionalPart")
    }))
}

async fn missing_moviepilot_media_info(
    state: &AppState,
    files: &[Value],
    context: &crate::jobs::JobContext,
) -> Result<BTreeMap<String, Option<String>>> {
    let mut pending = BTreeMap::new();
    for file in files {
        context.checkpoint().await?;
        let path = file.as_str().unwrap_or("");
        let media_path = std::path::Path::new(path);
        let info_path = media_path.with_file_name(format!(
            "{}-mediainfo.json",
            media_path.file_stem().unwrap_or_default().to_string_lossy()
        ));
        if tokio::fs::try_exists(info_path).await.unwrap_or(false) {
            continue;
        }
        let item = moviepilot_item(state, path).await.ok().flatten();
        let has_video = item.as_ref().is_some_and(|item| {
            list(&item["MediaStreams"])
                .into_iter()
                .chain(
                    list(&item["MediaSources"])
                        .iter()
                        .flat_map(|s| list(&s["MediaStreams"])),
                )
                .any(|stream| {
                    stream["Type"] == "Video"
                        && stream["Width"].as_f64().is_some_and(|v| v > 0.0)
                        && stream["Height"].as_f64().is_some_and(|v| v > 0.0)
                })
        });
        // Python requires the JSON sidecar when the original local file is visible.
        if tokio::fs::try_exists(media_path).await.unwrap_or(false) || !has_video {
            pending.insert(path.into(), item.as_ref().map(|item| string(item, "Id")));
        }
    }
    Ok(pending)
}

async fn sync_moviepilot_media_info(state: &AppState, path: &str) -> Result<()> {
    let config = state.config.read().await.clone();
    let base = string(&config, "emby_server_url");
    let key = string(&config, "emby_api_key");
    for endpoint in ["emby/Items/SyncMediaInfo", "Items/SyncMediaInfo"] {
        let response = state
            .http
            .post(format!("{}/{endpoint}", base.trim_end_matches('/')))
            .header("X-Emby-Token", &key)
            .query(&[("Path", path)])
            .json(&Value::Null)
            .timeout(Duration::from_secs(600))
            .send()
            .await?;
        if endpoint.starts_with("emby/") && response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        response.error_for_status()?;
        return Ok(());
    }
    unreachable!()
}

async fn ensure_moviepilot_media_info(
    state: &AppState,
    files: &[Value],
    context: &crate::jobs::JobContext,
) -> Result<()> {
    let config = state.config.read().await.clone();
    let attempts = config["media_info_retry_attempts"]
        .as_i64()
        .or_else(|| config["media_info_retry_attempts"].as_str()?.parse().ok())
        .unwrap_or(3)
        .clamp(0, 20);
    let interval = config["media_info_retry_interval"]
        .as_f64()
        .or_else(|| config["media_info_retry_interval"].as_str()?.parse().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(15.0)
        .max(1.0);
    let mut pending = missing_moviepilot_media_info(state, files, context).await?;
    for _ in 0..attempts {
        if pending.is_empty() {
            return Ok(());
        }
        for (path, id) in &pending {
            context.checkpoint().await?;
            if id.is_some()
                && let Err(error) = sync_moviepilot_media_info(state, path).await
            {
                tracing::warn!("MoviePilot 单项媒体信息提取失败：{}", error.1);
            }
        }
        tokio::time::sleep(Duration::from_secs_f64(interval)).await;
        pending = missing_moviepilot_media_info(state, files, context).await?;
    }
    if pending.is_empty() {
        Ok(())
    } else {
        Err(ApiError::bad(format!(
            "MoviePilot 本批媒体信息验收失败，仍有 {} 个文件缺失：{}",
            pending.len(),
            pending.keys().cloned().collect::<Vec<_>>().join("、")
        )))
    }
}

pub async fn process_moviepilot(
    state: &AppState,
    event: &Value,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let mut files = vec![];
    for file in list(&event["files"]) {
        if !crate::emby::path_ignored(state, file.as_str().unwrap_or("")).await? {
            files.push(file);
        }
    }
    if files.is_empty() {
        return Ok(json!({"updated":0,"status":"ignored_library"}));
    }
    let mut updated = 0;
    let mut failures = vec![];
    let tmdb = string(event, "tmdb_id");
    let rows: Vec<Value> = if tmdb.is_empty() {
        vec![]
    } else {
        sqlx::query_scalar("SELECT asset_details_json FROM media_metadata WHERE in_library=true AND ((tmdb_id=$1 AND item_type='Movie') OR (parent_series_tmdb_id=$1 AND item_type='Episode'))").bind(&tmdb).fetch_all(&state.db).await?
    };
    let known = rows
        .iter()
        .flat_map(list)
        .filter_map(|asset| {
            let path = string(&asset, "path");
            std::path::Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
        })
        .collect::<std::collections::BTreeSet<_>>();
    let new_files = files
        .iter()
        .filter(|file| {
            let stem = std::path::Path::new(file.as_str().unwrap_or(""))
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            !known.contains(&stem)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !new_files.is_empty() {
        let mut libraries = std::collections::BTreeSet::new();
        for file in &new_files {
            let library = crate::emby::library_for_path(state, file.as_str().unwrap_or(""))
                .await?
                .ok_or_else(|| ApiError::bad("MoviePilot 文件无法匹配 Emby 媒体库"))?;
            libraries.insert(string(&library, "ItemId"));
        }
        for library in libraries {
            crate::emby::refresh_item(state, &library, true, context).await?;
        }
        crate::emby::idle(state, context).await?;
    }
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut current = vec![];
    for file in &files {
        context.checkpoint().await?;
        let path = file.as_str().unwrap_or("");
        let mut found = None;
        for attempt in 0..90 {
            context.checkpoint().await?;
            found = moviepilot_item(state, path).await?;
            if found.is_some() {
                break;
            }
            if attempt < 89 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        let Some(item) = found else {
            return Err(ApiError::bad(format!("尚未找到Emby入库项目：{path}")));
        };
        current.push(string(&item, "Id"));
        if !new_files.contains(file) {
            continue;
        }
        let id = if item["Type"] == "Episode" {
            string(&item, "SeriesId")
        } else if item["Type"] == "Video" {
            string(&item, "ParentId")
        } else {
            string(&item, "Id")
        };
        if id.is_empty() {
            return Err(ApiError::bad(format!(
                "MoviePilot 入库项目缺少父项目：{path}"
            )));
        }
        let episodes = groups.entry(id).or_default();
        if item["Type"] == "Episode" {
            let episode = string(&item, "Id");
            if !episodes.contains(&episode) {
                episodes.push(episode);
            }
        }
    }
    if !new_files.is_empty() {
        crate::emby::scheduled(state, "MediaInfoExtractTask", context).await?;
        let tasks = state
            .emby(reqwest::Method::GET, "ScheduledTasks", &[], None)
            .await?;
        if !list(&tasks).iter().any(|task| {
            task["Key"] == "MediaInfoExtractTask"
                && task["State"] != "Running"
                && task["LastExecutionResult"]["Status"] == "Completed"
        }) {
            return Err(ApiError::bad("MoviePilot 媒体信息提取任务未成功完成"));
        }
        ensure_moviepilot_media_info(state, &new_files, context).await?;
    }
    if groups.is_empty() {
        crate::versions::reconcile(state, &current).await?;
        return Ok(json!({"updated":0,"message":"未发现新增媒体文件，已核对关联旧版本"}));
    }
    for (id, episodes) in groups {
        let item = details(
            state,
            &id,
            "Id,Type,Name,Path,People,Genres,ProviderIds,MediaSources,DateCreated",
        )
        .await?;
        let prepared = crate::ingest::prepare_cached(
            state,
            &item,
            &episodes,
            event["episode_group"].as_str().filter(|s| !s.is_empty()),
            context,
        )
        .await;
        let preparation = match prepared {
            Ok(true) => Ok(()),
            Ok(false) => crate::ingest::prepare_new(
                state,
                &item,
                &episodes,
                event["episode_group"].as_str().filter(|s| !s.is_empty()),
                context,
            )
            .await
            .map(|_| ()),
            Err(error) => Err(error),
        };
        match preparation {
            Ok(()) => match crate::ingest::feedback(state, &id, &episodes, true, context).await {
                Ok(result) => {
                    updated += result["updated"].as_u64().unwrap_or(0);
                    if state.config.read().await["emby_webhook_ingest_enabled"] == true {
                        state
                            .webhooks
                            .enhanced
                            .lock()
                            .await
                            .insert(id.clone(), Instant::now());
                    }
                    let current = if episodes.is_empty() {
                        vec![id.clone()]
                    } else {
                        episodes.clone()
                    };
                    if let Err(error) = crate::versions::reconcile(state, &current).await {
                        failures.push(error.1);
                    }
                    if state.config.read().await["emby_webhook_ingest_enabled"] != true
                        && let Err(error) =
                            finalize(state, &id, &episodes, !known.is_empty(), context).await
                    {
                        failures.push(error.1);
                    }
                }
                Err(error) => failures.push(error.1),
            },
            Err(error) => failures.push(error.1),
        }
    }
    Ok(json!({"updated":updated,"failures":failures,"message":"MoviePilot 后置增强处理结束"}))
}

pub async fn finalize(
    state: &AppState,
    id: &str,
    episode_ids: &[String],
    updated: bool,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let item = details(
        state,
        id,
        "Id,Type,Name,Path,Overview,ProductionYear,ProviderIds",
    )
    .await?;
    let targets = if item["Type"] == "Movie" {
        vec![id.into()]
    } else {
        episode_ids.to_vec()
    };
    let config = state.config.read().await.clone();
    let fingerprint_report = if !targets.is_empty() && config["monitor_sha1_pc_search"] != false {
        let mut report = crate::revalidation::ids(state, &targets, context).await?;
        let failed = list(&report["failed_ids"])
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        if !failed.is_empty() {
            tracing::warn!(
                "首轮有 {} 个资产指纹未补齐，等待 5 秒后重试一次",
                failed.len()
            );
            context.checkpoint().await?;
            tokio::time::sleep(Duration::from_secs(5)).await;
            context.checkpoint().await?;
            let retry = crate::revalidation::ids(state, &failed, context).await?;
            let updated =
                report["updated"].as_u64().unwrap_or(0) + retry["updated"].as_u64().unwrap_or(0);
            report["updated"] = json!(updated);
            report["failed"] = retry["failed"].clone();
            report["failed_ids"] = retry["failed_ids"].clone();
            report["message"] = json!(format!(
                "指纹重新校验结束：成功 {updated}，未完成 {}",
                retry["failed"]
            ));
            report["retry"] = retry;
        }
        report
    } else {
        json!({})
    };
    let audio_report = if config["auto_repair_truehd_core_on_ingest"] == true {
        match crate::audio_tasks::repair(state, &targets, context).await {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!("当前入库音轨检查失败：{}", error.1);
                json!({"failed":1,"error":error.1})
            }
        }
    } else {
        json!({})
    };
    crate::emby::finalize_refresh(state, &item, context).await?;
    // Fingerprint/audio repair and the final metadata refresh may replace media-info data.
    // Fill afterwards, then confirm the actual stream titles returned by Emby.
    let title_report = if config["auto_fill_language_titles_on_ingest"] == true {
        crate::titles::fill_in_job(state, &targets, true, context).await?
    } else {
        json!({})
    };
    if let Some(cover_config) = crate::db::setting(&state.db, "cover_generator_config").await?
        && cover_config["enabled"] == true
        && cover_config["transfer_monitor"] != false
        && let Some(library) = crate::emby::library_for_path(state, &string(&item, "Path")).await?
        && let Err(error) =
            crate::covers::generate(state, Some(&[string(&library, "ItemId")]), context).await
    {
        tracing::warn!("入库封面生成失败：{}", error.1);
    }
    if let Err(error) =
        crate::notifications::media(state, &item, updated, episode_ids, &title_report).await
    {
        tracing::warn!("入库通知失败：{}", error.1);
    }
    Ok(
        json!({"title_fill":title_report,"fingerprints":fingerprint_report,"audio_repair":audio_report}),
    )
}
