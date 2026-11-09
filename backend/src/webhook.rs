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

struct EnhancedItem {
    top_id: String,
    path: String,
    expires: Instant,
}
type MovieNotices = BTreeMap<(String, String), (Value, Vec<Value>)>;
fn collect_movie_notice(notices: &mut MovieNotices, item: &Value, report: &Value) {
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    let name = string(item, "Name");
    let key = if !tmdb.trim().is_empty() {
        format!("tmdb:{}", tmdb.trim())
    } else if !name.trim().is_empty() {
        format!("name:{}", name.trim())
    } else {
        format!("id:{}", string(item, "Id"))
    };
    let year = item["ProductionYear"]
        .as_i64()
        .filter(|year| *year != 0)
        .map(|year| year.to_string())
        .unwrap_or_else(|| string(item, "ProductionYear"));
    notices
        .entry((key, year.trim().to_string()))
        .or_insert_with(|| (item.clone(), vec![]))
        .1
        .extend(list(&report["pending"]));
}

#[derive(Default)]
pub struct Service {
    pending: Mutex<BTreeMap<String, Value>>,
    batches: Mutex<BTreeMap<String, Instant>>,
    enhanced: Mutex<BTreeMap<String, EnhancedItem>>,
    manual_deletes: Mutex<BTreeMap<String, Instant>>,
    draining: std::sync::atomic::AtomicBool,
}
impl Service {
    async fn mark_enhanced(&self, top_id: &str, members: &BTreeMap<String, String>) {
        let now = Instant::now();
        let mut enhanced = self.enhanced.lock().await;
        enhanced.retain(|_, item| item.expires > now);
        for (id, path) in members
            .iter()
            .filter(|(id, path)| !id.is_empty() && !path.is_empty())
        {
            enhanced.insert(
                id.clone(),
                EnhancedItem {
                    top_id: top_id.into(),
                    path: path.clone(),
                    expires: now + Duration::from_secs(3600),
                },
            );
        }
    }
    async fn take_enhanced(&self, top_id: &str, members: &[Value]) -> BTreeSet<String> {
        let now = Instant::now();
        let mut enhanced = self.enhanced.lock().await;
        enhanced.retain(|_, item| item.expires > now);
        let mut covered = BTreeSet::new();
        for member in members {
            let id = string(member, "Id");
            let path = string(member, "Path");
            let owner = match member["Type"].as_str() {
                Some("Episode") => string(member, "SeriesId"),
                Some("Video") if member["ExtraType"] == "AdditionalPart" => {
                    string(member, "ParentId")
                }
                Some("Movie") => id.clone(),
                _ => String::new(),
            };
            if owner == top_id
                && enhanced
                    .get(&id)
                    .is_some_and(|item| item.top_id == top_id && item.path == path)
            {
                enhanced.remove(&id);
                covered.insert(id);
            }
        }
        covered
    }
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
        tracing::warn!("Webhook 请求拒绝：共享密钥未配置或验证失败");
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
        tracing::info!(
            "Webhook 排队合并：任务 {key}，目标分集 {} 个",
            list(&old["episode_ids"]).len()
        );
    } else {
        tracing::info!(
            "Webhook 排队：任务 {key}，来源 {}，目标分集 {} 个",
            string(&event, "source"),
            list(&event["episode_ids"]).len()
        );
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
                    tracing::info!(
                        "Webhook 任务处理开始：来源 {}，Emby {}，目标分集 {} 个",
                        string(&event, "source"),
                        string(&event, "id"),
                        list(&event["episode_ids"]).len()
                    );
                    if event["source"] == "moviepilot" {
                        let result = process_moviepilot(&work, &event, &context).await;
                        if let Err(error) = &result {
                            tracing::error!("MoviePilot Webhook 任务失败：{}", error.1);
                        }
                        if result.as_ref().map_or(true, |v| {
                            v["failures"].as_array().is_some_and(|f| !f.is_empty())
                        }) && let Some(key) = event["batch_key"].as_str()
                        {
                            work.webhooks.batches.lock().await.remove(key);
                        }
                        result
                    } else {
                        process_emby(&work, &event, &context)
                            .await
                            .inspect_err(|error| {
                                tracing::error!(
                                    "Emby Webhook 任务失败：Emby {}，{}",
                                    string(&event, "id"),
                                    error.1
                                )
                            })
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
    if event == "deep.delete" {
        tracing::info!("收到神医助手深度删除通知，准备清理本地数据库与缓存...");
        let result = match deep_delete(&state, item).await {
            Ok(result) => result,
            Err(error) => {
                tracing::error!("深度删除清理本地数据库与缓存失败：{}", error.1);
                json!({"status":"deep_delete_processed"})
            }
        };
        return Ok((StatusCode::OK, Json(result)));
    }
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
    tracing::info!(
        "Emby Webhook 已收到：事件 {event}，Emby {id}，类型 {kind}，名称 {}",
        string(item, "Name")
    );
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
    tracing::info!(
        "Emby Webhook 已接受：事件 {event}，Emby {id}，名称 {}，处理父项 {top}，目标分集 {} 个",
        string(item, "Name"),
        ids.len()
    );
    let queue_id = if event == "metadata.update" {
        &id
    } else {
        &top
    };
    enqueue(
        &state,
        format!("emby:{queue_id}:{event}"),
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
    if id.is_empty() {
        return Ok(json!({"status":"deep_delete_processed"}));
    }
    if state.webhooks.manual_delete_marked(&id).await {
        tracing::info!("[深度删除] Item {id} 由 ETK 版本删除流程处理，跳过重复的 115/数据库清理。");
        return Ok(json!({"status":"deep_delete_managed_by_etk"}));
    }
    let config = state.config.read().await.clone();
    let kind = string(item, "Type");
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1",
    )
    .bind(json!([id]))
    .fetch_all(&state.db)
    .await?;
    let mut deleted = BTreeSet::from([id.clone()]);
    if !matches!(kind.as_str(), "Series" | "Season") {
        let linked = rows
            .first()
            .map(|row| {
                list(&row["emby_item_ids_json"])
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !linked.is_empty() {
            match crate::versions::existing(state, &linked).await {
                Ok(existing) => {
                    deleted.extend(linked.into_iter().filter(|id| !existing.contains(id)));
                }
                Err(_) => tracing::warn!("Emby 关联版本查询失败，仅清理本次通知的版本"),
            }
        }
    }
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
        let mut candidates = vec![];
        for child in children {
            candidates.extend(delete_slots(&child).into_iter().filter(|slot| {
                let asset_id = string(&slot.asset, "emby_item_id");
                (asset_id.is_empty() || asset_id == slot.id)
                    && delete_path(&string(&slot.asset, "path"), &config).is_some_and(|path| {
                        roots
                            .iter()
                            .any(|root| path != *root && path.starts_with(root))
                    })
            }));
        }
        let child_ids = candidates
            .iter()
            .map(|slot| slot.id.clone())
            .collect::<Vec<_>>();
        if child_ids.is_empty() {
            continue;
        }
        match crate::versions::existing(state, &child_ids).await {
            Ok(existing) => {
                deleted.extend(child_ids.into_iter().filter(|id| !existing.contains(id)));
                slots.extend(candidates);
            }
            Err(_) => tracing::warn!("Emby 子项存活核对失败，保留子项文件和缓存"),
        }
    }
    let mut removed = vec![];
    for id in deleted {
        if !state.webhooks.manual_delete_marked(&id).await {
            removed.push(id);
        }
    }
    if config["p115_enable_sync_delete"] == true {
        let mut fids = BTreeSet::new();
        let mut retained_fids = BTreeSet::new();
        for slot in &slots {
            if let Some(fid) = delete_fid(state, slot, &config).await? {
                if removed.contains(&slot.id) {
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
                    tracing::warn!(
                        "[115联动删除] 已删除 115 文件: {} ({kind})，FID={}",
                        string(item, "Name"),
                        fids.join(",")
                    );
                }
                Err(error) => tracing::error!("115 深度联动删除失败，保留文件缓存: {}", error.1),
            }
        }
    }
    if config["p115_enable_sync_delete"] != true {
        tracing::info!("[115联动删除] 未启用，跳过 115 文件删除。");
    }
    tracing::info!(
        "[深度删除] 开始清理本地数据库与缓存: {} ({kind})",
        string(item, "Name")
    );
    crate::versions::unbind(state, &removed).await?;
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
        tracing::info!("MoviePilot Webhook 跳过：上游刮削未成功");
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"ignored","message":"moviepilot scrape failed"})),
        ));
    }
    let root = string(&body, "root_path");
    if root.is_empty() {
        tracing::warn!("MoviePilot Webhook 拒绝：缺少 root_path");
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
        tracing::warn!("MoviePilot Webhook 拒绝：剧集缺少 episode_group 字段");
        return Err(ApiError::bad("episode_group is required for TV"));
    }
    tracing::info!(
        "MoviePilot Webhook 已收到：事件 {}，TMDb {}，类型 {}，文件 {} 个，根路径 {root}",
        string(&body, "event"),
        string(&body, "tmdb_id"),
        string(&body, "media_type"),
        list(&body["file_list"]).len()
    );
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
        tracing::info!("MoviePilot Webhook 跳过：重复批次 {key}");
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
    tracing::info!(
        "MoviePilot Webhook 已接受：批次 {key}，有效文件 {} 个，根路径 {root}",
        files.len()
    );
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
    let item = details(state, &id, "Id,Type,Name,Path,MediaSources,MediaStreams").await?;
    let media_name = string(&item, "Name");
    if crate::emby::path_ignored(state, &string(&item, "Path")).await? {
        return Ok(json!({"updated":0,"status":"ignored_library"}));
    }
    tracing::info!(
        "Emby 入库处理开始：《{media_name}》，Emby {id}，事件 {}",
        string(event, "event")
    );
    let episodes = list(&event["episode_ids"])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if event["event"] == "metadata.update" {
        tracing::info!("Emby 元数据更新事件：《{media_name}》，Emby {id}，同步元数据库");
        return mirror(state, &string(&event["original"], "Id")).await;
    }
    if event["event"] == "image.update" {
        return Ok(json!({"message":"图片更新已接收"}));
    }
    let mut members = vec![];
    if episodes.is_empty() {
        let original_id = string(&event["original"], "Id");
        if !original_id.is_empty() && original_id != id && item["Type"] == "Movie" {
            members.push(
                details(
                    state,
                    &original_id,
                    "Id,Type,Name,Path,SeriesId,ParentId,ExtraType,MediaSources,MediaStreams",
                )
                .await?,
            );
        } else if item["Type"] == "Movie" {
            members.push(item.clone());
        }
    } else {
        for episode in &episodes {
            context.checkpoint().await?;
            members.push(
                details(
                    state,
                    episode,
                    "Id,Type,Name,Path,SeriesId,ParentId,ExtraType,MediaSources,MediaStreams",
                )
                .await?,
            );
        }
    }
    // A prepared member is consumed once. Different batches in the same series
    // cannot inherit its shortcut, and a failed finalization leaves no claimed marker.
    let covered = state.webhooks.take_enhanced(&id, &members).await;
    let enhanced = !members.is_empty()
        && members
            .iter()
            .all(|member| covered.contains(&string(member, "Id")));
    if !enhanced && event["source"] == "emby" {
        crate::media_preflight::run(state, &item, context).await?;
        for member in &members {
            if string(member, "Id") != id && !covered.contains(&string(member, "Id")) {
                context.checkpoint().await?;
                crate::media_preflight::run(state, member, context).await?;
            }
        }
    }
    let mut result = match crate::ingest::feedback(state, &id, &episodes, false, context).await {
        Ok(result) => Ok(result),
        Err(error) => {
            if enhanced {
                tracing::error!(
                    "已预增强项目缓存回流失败：《{media_name}》，Emby {id}，{}",
                    error.1
                );
                return Err(error);
            }
            tracing::info!(
                "缓存回流不可用，执行完整刮削：《{media_name}》，Emby {id}，{}",
                error.1
            );
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
            crate::ingest::scrape_for_ingest(state, &item, &episodes, group.as_deref(), context)
                .await
        }
    }?;
    let finalization = finalize(state, &id, &episodes, !episodes.is_empty(), context).await?;
    let titles_completed = finalization["title_fill"]["completed"] == true;
    add_report_warnings(&mut result, &finalization);
    result["finalization"] = finalization;
    tracing::info!(
        "Emby 入库处理完成：《{media_name}》，Emby {id}，更新 {} 条记录",
        result["updated"].as_u64().unwrap_or(0)
    );
    let automatic = automatic_extraction(
        state,
        item["Type"] == "Series" && !episodes.is_empty(),
        item["Type"] == "Movie" || item["Type"] == "Series" && !episodes.is_empty(),
        titles_completed,
        &format!("《{media_name}》"),
        context,
    )
    .await?;
    add_automatic_result(&mut result, automatic);
    if result["warnings"].as_u64().unwrap_or(0) > 0 {
        result["message"] = json!(format!(
            "Emby 入库完成：已更新 {} 条记录，{}",
            result["updated"].as_u64().unwrap_or(0),
            warning_summary(&result)
        ));
    }
    Ok(result)
}

fn add_report_warnings(result: &mut Value, report: &Value) {
    let warnings = report["warnings"].as_u64().unwrap_or(0);
    if warnings > 0 {
        result["warnings"] = json!(result["warnings"].as_u64().unwrap_or(0) + warnings);
    }
    if let Some(details) = report["warning_details"].as_array() {
        if !result["warning_details"].is_array() {
            result["warning_details"] = json!([]);
        }
        result["warning_details"]
            .as_array_mut()
            .unwrap()
            .extend(details.iter().cloned());
    }
}

fn warning_summary(report: &Value) -> String {
    let count = report["warnings"].as_u64().unwrap_or(0);
    let reason = list(&report["warning_details"])
        .into_iter()
        .find_map(|detail| {
            detail
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
        });
    match reason {
        Some(reason) => format!(
            "后处理有 {count} 条警告：{}；完整明细见任务结果",
            crate::logs::redact(&reason)
                .replace(['\r', '\n'], " ")
                .chars()
                .take(300)
                .collect::<String>()
        ),
        None => format!("后处理有 {count} 条警告，请查看任务结果"),
    }
}

fn title_warning_detail(report: &Value) -> String {
    let mut detail = format!(
        "音轨字幕标题：失败 {} 个媒体，待处理 {} 项",
        report["failed"].as_u64().unwrap_or(0),
        list(&report["pending"]).len()
    );
    if let Some(reason) = list(&report["pending"])
        .iter()
        .map(|entry| string(entry, "reason"))
        .find(|reason| !reason.trim().is_empty())
    {
        detail.push_str(&format!("；{reason}"));
    }
    detail
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod warning_tests {
    use super::*;

    #[test]
    fn movie_notice_groups_merge_versions_and_keep_years_and_pending_separate() {
        let mut notices = MovieNotices::new();
        for (id, name, year) in [
            ("1", "译名", 1998),
            ("2", "原名", 1998),
            ("3", "译名", 1999),
        ] {
            collect_movie_notice(
                &mut notices,
                &json!({"Id":id,"Name":name,"ProductionYear":year,"ProviderIds":{"Tmdb":"4426"}}),
                &json!({"pending":[{"item_id":id,"reason":"待回读"}]}),
            );
        }
        assert_eq!(notices.len(), 2);
        assert_eq!(
            notices[&("tmdb:4426".into(), "1998".into())].1,
            json!([{"item_id":"1","reason":"待回读"},{"item_id":"2","reason":"待回读"}])
                .as_array()
                .unwrap()
                .clone()
        );
        assert_eq!(
            notices[&("tmdb:4426".into(), "1999".into())].1[0]["item_id"],
            "3"
        );
        collect_movie_notice(
            &mut notices,
            &json!({"Id":"4","Name":"4426","ProductionYear":1998}),
            &json!({}),
        );
        collect_movie_notice(
            &mut notices,
            &json!({"Id":"4426","ProductionYear":1998}),
            &json!({}),
        );
        assert_eq!(
            notices.len(),
            4,
            "数字片名及原生ID不能与TMDb编号混为一条通知"
        );
    }

    #[test]
    fn ingest_summary_explains_title_failure_and_keeps_all_warnings() {
        let reason = "Emby 路径与当前媒体资产不一致，已停止填充标题";
        let title = json!({"failed":1,"pending":[{"item_id":"61588","reason":reason}]});
        let mut result = json!({"updated":1});
        add_report_warnings(
            &mut result,
            &json!({"warnings":1,"warning_details":[title_warning_detail(&title)]}),
        );
        add_report_warnings(
            &mut result,
            &json!({"warnings":1,"warning_details":["视频预览缩略图未确认"]}),
        );
        let summary = warning_summary(&result);
        assert!(summary.contains("2 条警告"));
        assert!(summary.contains("音轨字幕标题"));
        assert!(summary.contains(reason));
        assert_eq!(result["warning_details"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn warning_summary_redacts_credentials_and_handles_missing_details() {
        let summary = warning_summary(&json!({
            "warnings":1,"warning_details":["通知失败\npassword=fixture-value"]
        }));
        assert!(summary.contains("通知失败"));
        assert!(!summary.contains("fixture-value"));
        assert!(!summary.contains('\n'));
        assert_eq!(
            warning_summary(&json!({"warnings":1})),
            "后处理有 1 条警告，请查看任务结果"
        );
    }
}

fn add_automatic_result(result: &mut Value, automatic: Value) {
    if automatic.is_null() {
        return;
    }
    add_report_warnings(result, &automatic);
    result["automatic_extraction"] = automatic;
}

fn extraction_state_label(report: &Value) -> &'static str {
    match report["state"].as_str() {
        Some("processed") => "已处理",
        Some("completed") => "已完成",
        Some("skipped") => "已跳过",
        Some("failed") => "失败",
        _ => "未确认",
    }
}

async fn automatic_extraction(
    state: &AppState,
    series: bool,
    videos: bool,
    titles_completed: bool,
    label: &str,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let config = state.config.read().await.clone();
    let intro_enabled = series && config["auto_extract_intro_fingerprint_on_ingest"] == true;
    let thumbnails_enabled = videos && config["auto_extract_video_thumbnails_on_ingest"] == true;
    if !intro_enabled && !thumbnails_enabled {
        return Ok(Value::Null);
    }
    if !titles_completed {
        let reason = "标题填充与 Emby 回读尚未完成，已跳过入库片头声纹和视频预览缩略图提取";
        tracing::info!("[入库自动处理] {label}{reason}");
        let mut report =
            json!({"state":"skipped","reason":reason,"warnings":0,"warning_details":[]});
        if intro_enabled {
            report["intro"] = json!({"state":"skipped","reason":reason});
        }
        if thumbnails_enabled {
            report["video_thumbnails"] = json!({"state":"skipped","reason":reason});
        }
        return Ok(report);
    }
    let mut report = json!({});
    let mut warning_details = vec![];
    if intro_enabled {
        context.checkpoint().await?;
        context
            .activity(
                "片头声纹提取",
                &format!("{label}入库已完成，开始原生声纹任务并核对本次日志"),
            )
            .await;
        let intro = match crate::thumbnails::extract_intro_verified(state, context).await {
            Ok(result) => result,
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                json!({"state":"unverified","native_ended":false,"failed":1,"reason":error.1})
            }
        };
        let failed = intro["failed"].as_u64().unwrap_or(0) > 0;
        if failed {
            let warning = format!(
                "{label}片头声纹处理未全部确认：{}；保留已完成的入库结果",
                string(&intro, "reason")
            );
            tracing::warn!("[入库自动处理] {warning}");
            warning_details.push(warning);
        } else {
            tracing::info!(
                "[入库自动处理] {label}片头声纹处理结果：{}；{}",
                extraction_state_label(&intro),
                string(&intro, "reason")
            );
        }
        let ended = intro["native_ended"] == true || intro["native_state"] == "not_started";
        report["intro"] = intro;
        if thumbnails_enabled && !ended {
            let reason = "片头声纹任务尚未确认结束，跳过视频预览缩略图提取";
            report["video_thumbnails"] = json!({"state":"skipped","reason":reason});
            tracing::warn!("[入库自动处理] {label}{reason}");
            report["warnings"] = json!(warning_details.len());
            report["warning_details"] = json!(warning_details);
            return Ok(report);
        }
    }
    if thumbnails_enabled {
        context.checkpoint().await?;
        context
            .activity(
                "视频预览缩略图提取",
                &format!("{label}入库已完成，运行原生任务并核对本次目标图片"),
            )
            .await;
        match crate::thumbnails::extract_verified(state, context).await {
            Ok(extraction) => {
                let failed = extraction.report["failed"].as_u64().unwrap_or(0) > 0;
                if failed {
                    let warning = format!(
                        "{label}视频预览缩略图提取或验收未全部通过：{}；保留已完成的入库结果",
                        string(&extraction.report, "reason")
                    );
                    tracing::warn!("[入库自动处理] {warning}");
                    warning_details.push(warning);
                } else {
                    tracing::info!(
                        "[入库自动处理] {label}视频预览缩略图处理结果：{}，逐项验收 {} 个目标",
                        extraction_state_label(&extraction.report),
                        extraction.report["verified"]
                    );
                }
                report["video_thumbnails"] = extraction.report.clone();
                if extraction.report["needs_repair"] == true {
                    context.checkpoint().await?;
                    context
                        .activity(
                            "修复视频预览缩略图",
                            &format!("{label}复用本次缺失图片目标修复，不重复全库提取"),
                        )
                        .await;
                    match crate::thumbnails::repair_extraction(state, &extraction, context).await {
                        Ok(repair) => {
                            if repair["failed"].as_u64().unwrap_or(0) > 0 {
                                let warning = format!(
                                    "{label}视频预览缩略图修复后仍有 {} 项失败",
                                    repair["failed"]
                                );
                                tracing::error!("[入库自动处理] {warning}");
                                warning_details.push(warning);
                            }
                            report["video_thumbnails_repair"] = repair;
                        }
                        Err(error) if context.is_cancelled() => return Err(error),
                        Err(error) => {
                            let warning = format!(
                                "{label}视频预览缩略图修复失败：{}；不再自动重复重试",
                                error.1
                            );
                            tracing::error!("[入库自动处理] {warning}");
                            warning_details.push(warning);
                            report["video_thumbnails_repair"] =
                                json!({"state":"failed","reason":error.1});
                        }
                    }
                } else if failed {
                    report["video_thumbnails_repair"] = json!({"state":"skipped","reason":"本次原生任务状态或缺失目标范围未确认，未自动扩大修复范围"});
                }
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                let warning = format!(
                    "{label}视频预览缩略图处理未确认：{}；未自动扩大修复范围",
                    error.1
                );
                tracing::warn!("[入库自动处理] {warning}");
                warning_details.push(warning);
                report["video_thumbnails"] = json!({"state":"unverified","reason":error.1});
                report["video_thumbnails_repair"] =
                    json!({"state":"skipped","reason":"未取得本次可验证的原生处理范围"});
            }
        }
    }
    if report.as_object().unwrap().is_empty() {
        return Ok(Value::Null);
    }
    report["warnings"] = json!(warning_details.len());
    report["warning_details"] = json!(warning_details);
    Ok(report)
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
    tracing::info!(
        "Emby 元数据同步完成：《{}》，Emby {id}",
        string(&item, "Name")
    );
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
        if crate::media_preflight::valid_video_sidecar(&info_path).await {
            continue;
        }
        let item = moviepilot_item(state, path).await.ok().flatten();
        let has_video = item
            .as_ref()
            .is_some_and(crate::media_preflight::valid_video);
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
        let response = crate::emby::send(
            state
                .http
                .post(format!("{}/{endpoint}", base.trim_end_matches('/')))
                .header("X-Emby-Token", &key)
                .query(&[("Path", path)])
                .json(&Value::Null)
                .timeout(Duration::from_secs(600)),
        )
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
            if id.is_some() {
                let result = tokio::select! {
                    _ = context.cancelled() => return context.checkpoint().await,
                    result = sync_moviepilot_media_info(state, path) => result,
                };
                if let Err(error) = result {
                    tracing::warn!("MoviePilot 单项媒体信息提取失败：{}", error.1);
                }
            }
        }
        context.sleep(Duration::from_secs_f64(interval)).await?;
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
    tracing::info!(
        "MoviePilot 后置增强开始：TMDb {}，批次 {}，输入文件 {} 个，根路径 {}",
        string(event, "tmdb_id"),
        string(event, "batch_key"),
        list(&event["files"]).len(),
        string(event, "root_path")
    );
    let mut updated = 0;
    let mut failures = vec![];
    let mut finalizations = vec![];
    let mut movie_notices = MovieNotices::new();
    let mut postprocessing = json!({});
    let mut automatic_series = false;
    let mut automatic_videos = false;
    let mut titles_completed = true;
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
    tracing::info!(
        "MoviePilot 新文件核对完成：TMDb {tmdb}，有效文件 {} 个，新增 {} 个，已关联 {} 个",
        files.len(),
        new_files.len(),
        files.len() - new_files.len()
    );
    if !new_files.is_empty() {
        let mut libraries = std::collections::BTreeSet::new();
        for file in &new_files {
            let library = crate::emby::library_for_path(state, file.as_str().unwrap_or(""))
                .await?
                .ok_or_else(|| ApiError::bad("MoviePilot 文件无法匹配 Emby 媒体库"))?;
            libraries.insert(string(&library, "ItemId"));
        }
        for library in libraries {
            tracing::info!("MoviePilot 等待媒体库入库扫描：媒体库 {library}");
            crate::emby::refresh_item(state, &library, true, context).await?;
        }
        crate::emby::idle(state, context).await?;
    }
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut prepared_members = BTreeMap::<String, BTreeMap<String, String>>::new();
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
            tracing::warn!(
                "MoviePilot 入库等待未找到项目：文件 {}",
                std::path::Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
            return Err(ApiError::bad(format!("尚未找到Emby入库项目：{path}")));
        };
        tracing::debug!(
            "MoviePilot 入库项目已找到：Emby {}，名称 {}，文件 {}",
            string(&item, "Id"),
            string(&item, "Name"),
            std::path::Path::new(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
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
        prepared_members
            .entry(id.clone())
            .or_default()
            .insert(string(&item, "Id"), string(&item, "Path"));
        let episodes = groups.entry(id).or_default();
        if item["Type"] == "Episode" {
            let episode = string(&item, "Id");
            if !episodes.contains(&episode) {
                episodes.push(episode);
            }
        }
    }
    if !new_files.is_empty() {
        tracing::info!(
            "MoviePilot 媒体信息提取开始：新增文件 {} 个",
            new_files.len()
        );
        crate::emby::scheduled(state, "MediaInfoExtractTask", context).await?;
        let tasks = state
            .emby(reqwest::Method::GET, "ScheduledTasks", &[], None)
            .await?;
        if !list(&tasks).iter().any(|task| {
            task["Key"] == "MediaInfoExtractTask"
                && task["State"] != "Running"
                && task["LastExecutionResult"]["Status"] == "Completed"
        }) {
            tracing::error!("MoviePilot 媒体信息提取失败：Emby 任务未成功完成");
            return Err(ApiError::bad("MoviePilot 媒体信息提取任务未成功完成"));
        }
        ensure_moviepilot_media_info(state, &new_files, context).await?;
        tracing::info!(
            "MoviePilot 媒体信息提取完成：新增文件 {} 个",
            new_files.len()
        );
    }
    if groups.is_empty() {
        crate::versions::reconcile(state, &current).await?;
        tracing::info!(
            "MoviePilot 后置增强完成：没有新增媒体文件，已核对 {} 个旧版本",
            current.len()
        );
        return Ok(json!({"updated":0,"message":"未发现新增媒体文件，已核对关联旧版本"}));
    }
    tracing::info!(
        "MoviePilot 后置增强范围确认：TMDb {tmdb}，媒体组 {} 个",
        groups.len()
    );
    for (index, (id, episodes)) in groups.into_iter().enumerate() {
        if index > 0 {
            state.item_delay(context).await?;
        }
        let item = details(
            state,
            &id,
            "Id,Type,Name,Path,People,Genres,ProviderIds,MediaSources,DateCreated",
        )
        .await?;
        let media_name = string(&item, "Name");
        tracing::info!(
            "MoviePilot 单组增强开始：《{media_name}》，Emby {id}，目标分集 {} 个",
            episodes.len()
        );
        let prepared = crate::ingest::prepare_cached_report(
            state,
            &item,
            &episodes,
            event["episode_group"].as_str().filter(|s| !s.is_empty()),
            context,
        )
        .await;
        let preparation = match prepared {
            Ok(Some(report)) => Ok(report),
            Ok(None) => {
                crate::ingest::prepare_new(
                    state,
                    &item,
                    &episodes,
                    event["episode_group"].as_str().filter(|s| !s.is_empty()),
                    context,
                )
                .await
            }
            Err(error) => Err(error),
        }
        .map(|report| add_report_warnings(&mut postprocessing, &report));
        match preparation {
            Ok(()) => match crate::ingest::feedback(state, &id, &episodes, true, context).await {
                Ok(result) => {
                    updated += result["updated"].as_u64().unwrap_or(0);
                    add_report_warnings(&mut postprocessing, &result);
                    if state.config.read().await["emby_webhook_ingest_enabled"] == true
                        && let Some(members) = prepared_members.get(&id)
                    {
                        state.webhooks.mark_enhanced(&id, members).await;
                    }
                    let current = if episodes.is_empty() {
                        vec![id.clone()]
                    } else {
                        episodes.clone()
                    };
                    if let Err(error) = crate::versions::reconcile(state, &current).await {
                        tracing::warn!(
                            "MoviePilot 版本核对失败：《{media_name}》，Emby {id}，{}",
                            error.1
                        );
                        failures.push(error.1);
                    }
                    if state.config.read().await["emby_webhook_ingest_enabled"] != true {
                        match finalize_inner(
                            state,
                            &id,
                            &episodes,
                            !known.is_empty(),
                            context,
                            Some(&mut movie_notices),
                        )
                        .await
                        {
                            Ok(report) => {
                                if item["Type"] == "Movie"
                                    || item["Type"] == "Series" && !episodes.is_empty()
                                {
                                    titles_completed &= report["title_fill"]["completed"] == true;
                                }
                                add_report_warnings(&mut postprocessing, &report);
                                finalizations
                                    .push(json!({"item_id":id,"name":media_name,"report":report}));
                                automatic_series |=
                                    item["Type"] == "Series" && !episodes.is_empty();
                                automatic_videos |= item["Type"] == "Movie"
                                    || item["Type"] == "Series" && !episodes.is_empty();
                            }
                            Err(error) => {
                                tracing::warn!(
                                    "MoviePilot 入库收尾失败：《{media_name}》，Emby {id}，{}",
                                    error.1
                                );
                                failures.push(error.1);
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::error!(
                        "MoviePilot 缓存回流失败：《{media_name}》，Emby {id}，{}",
                        error.1
                    );
                    failures.push(error.1);
                }
            },
            Err(error) => {
                tracing::error!(
                    "MoviePilot 单组增强失败：《{media_name}》，Emby {id}，{}",
                    error.1
                );
                failures.push(error.1);
            }
        }
    }
    for (item, pending) in movie_notices.into_values() {
        context.checkpoint().await?;
        if let Err(error) = crate::notifications::media(
            state,
            &item,
            !known.is_empty(),
            &[],
            &json!({"pending":pending}),
        )
        .await
        {
            if context.is_cancelled() {
                return Err(error);
            }
            let reason = format!("《{}》入库通知失败：{}", string(&item, "Name"), error.1);
            tracing::warn!("{reason}");
            add_report_warnings(
                &mut postprocessing,
                &json!({"warnings":1,"warning_details":[reason]}),
            );
        }
    }
    tracing::info!(
        "MoviePilot 后置增强处理结束：TMDb {tmdb}，更新 {updated} 条，入库失败 {} 项，后处理警告 {} 条",
        failures.len(),
        postprocessing["warnings"].as_u64().unwrap_or(0)
    );
    let message = if failures.is_empty() {
        format!("MoviePilot 后置增强处理完成：已更新 {updated} 条记录")
    } else {
        format!(
            "MoviePilot 后置增强未完全成功：已更新 {updated} 条记录，失败 {} 项；{}",
            failures.len(),
            failures[0]
        )
    };
    let automatic = if failures.is_empty() {
        automatic_extraction(
            state,
            automatic_series,
            automatic_videos,
            titles_completed,
            "本批媒体",
            context,
        )
        .await?
    } else {
        Value::Null
    };
    let mut result = json!({"updated":updated,"failures":failures,"finalizations":finalizations,"message":message});
    add_report_warnings(&mut result, &postprocessing);
    add_automatic_result(&mut result, automatic);
    if failures.is_empty() && result["warnings"].as_u64().unwrap_or(0) > 0 {
        result["message"] = json!(format!(
            "MoviePilot 入库完成：已更新 {updated} 条记录，{}",
            warning_summary(&result)
        ));
    }
    Ok(result)
}

pub async fn finalize(
    state: &AppState,
    id: &str,
    episode_ids: &[String],
    updated: bool,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    finalize_inner(state, id, episode_ids, updated, context, None).await
}
async fn finalize_inner(
    state: &AppState,
    id: &str,
    episode_ids: &[String],
    updated: bool,
    context: &crate::jobs::JobContext,
    movie_notices: Option<&mut MovieNotices>,
) -> Result<Value> {
    tracing::info!(
        "入库收尾开始：Emby {id}，目标分集 {} 个，更新模式 {updated}",
        episode_ids.len()
    );
    let item = details(
        state,
        id,
        "Id,Type,Name,Path,Overview,ProductionYear,ProviderIds",
    )
    .await?;
    let media_name = string(&item, "Name");
    let targets = if item["Type"] == "Movie" {
        vec![id.into()]
    } else {
        episode_ids.to_vec()
    };
    let config = state.config.read().await.clone();
    let fingerprint_report = if !targets.is_empty() && config["monitor_sha1_pc_search"] != false {
        tracing::info!(
            "入库资产指纹补齐开始：《{media_name}》，Emby {id}，目标 {} 个",
            targets.len()
        );
        let mut report = match crate::revalidation::repair_ids(state, &targets, context).await {
            Ok(report) => report,
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!(
                    "入库指纹补齐未完成：《{media_name}》，{}；继续标题处理及最终刷新",
                    error.1
                );
                json!({"updated":0,"failed":targets.len(),"failed_ids":[],"message":format!("指纹补齐未完成：{}",error.1)})
            }
        };
        let failed = list(&report["failed_ids"])
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        if !failed.is_empty() {
            tracing::warn!(
                "《{media_name}》首轮有 {} 个资产指纹未补齐，等待 5 秒后重试一次",
                failed.len()
            );
            context.checkpoint().await?;
            tokio::time::sleep(Duration::from_secs(5)).await;
            context.checkpoint().await?;
            let retry = match crate::revalidation::repair_ids(state, &failed, context).await {
                Ok(report) => report,
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => {
                    tracing::warn!(
                        "入库指纹重试未完成：《{media_name}》，{}；继续标题处理及最终刷新",
                        error.1
                    );
                    json!({"updated":0,"failed":failed.len(),"failed_ids":failed,"message":format!("指纹重试未完成：{}",error.1)})
                }
            };
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
        tracing::info!(
            "入库资产指纹补齐结束：《{media_name}》，Emby {id}，成功 {}，未完成 {}",
            report["updated"].as_u64().unwrap_or(0),
            report["failed"].as_u64().unwrap_or(0)
        );
        report
    } else {
        json!({})
    };
    let audio_report = if config["auto_repair_truehd_core_on_ingest"] == true {
        tracing::info!(
            "入库音轨检查开始：《{media_name}》，Emby {id}，目标 {} 个",
            targets.len()
        );
        match crate::audio_tasks::repair(state, &targets, context).await {
            Ok(report) => {
                tracing::info!(
                    "入库音轨检查结束：《{media_name}》，Emby {id}，失败 {} 项",
                    report["failed"].as_u64().unwrap_or(0)
                );
                report
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!("当前入库音轨检查失败：《{media_name}》，{}", error.1);
                json!({"failed":1,"error":error.1})
            }
        }
    } else {
        json!({})
    };
    let confirmed = std::sync::atomic::AtomicBool::new(true);
    let entered = std::sync::atomic::AtomicBool::new(false);
    let recovery = state
        .data_dir
        .join("backups/media-info-reload")
        .join(format!("titles-{}", uuid::Uuid::new_v4()));
    let refresh_guard = (&confirmed, recovery.as_path());
    let refresh_item = &item;
    let refresh_name = &media_name;
    let final_refresh = |guarded: bool| async move {
        context
            .activity(
                "首次元数据刷新",
                &format!("《{refresh_name}》（媒体ID：{id}）：指纹及后处理结束，开始入库刷新验收"),
            )
            .await;
        tracing::info!(
            "入库最终元数据刷新开始：Emby {id}，名称 {}",
            string(refresh_item, "Name")
        );
        let result = if guarded {
            crate::emby::finalize_refresh_guarded(
                state,
                refresh_item,
                context,
                refresh_guard.0,
                refresh_guard.1,
            )
            .await
        } else {
            crate::emby::finalize_refresh(state, refresh_item, context).await
        };
        result.inspect_err(|error| {
            tracing::error!(
                "入库最终元数据刷新失败：《{refresh_name}》，Emby {id}，{}",
                error.1
            )
        })?;
        tracing::info!("入库最终元数据刷新完成：《{refresh_name}》，Emby {id}");
        Ok::<_, ApiError>(())
    };
    // Prepare title JSON before the existing final refresh, then verify Emby.
    let mut reload_warnings = vec![];
    let title_report = if config["auto_fill_language_titles_on_ingest"] == true {
        tracing::info!(
            "入库音轨字幕标题填充开始：《{media_name}》，Emby {id}，目标 {} 个",
            targets.len()
        );
        let warning_target = &mut reload_warnings;
        let reload_targets = &targets;
        let reload_confirmed = &confirmed;
        let reload_recovery = &recovery;
        let outcome = crate::strm::with_settings_guarded(
            state,
            "restore_media_info",
            || {
                entered.store(true, std::sync::atomic::Ordering::Release);
                crate::titles::fill_in_job_with_refresh(
                    state,
                    &targets,
                    true,
                    context,
                    async move {
                        *warning_target = crate::media_save::reload_filled_titles(
                            state,
                            reload_targets,
                            true,
                            context,
                            reload_confirmed,
                            reload_recovery,
                        )
                        .await?;
                        final_refresh(true).await
                    },
                )
            },
            &confirmed,
            &recovery,
        )
        .await;
        match outcome {
            Ok(mut report) => {
                if !reload_warnings.is_empty() {
                    report["completed"] = json!(false);
                    report["pending"].as_array_mut().unwrap().extend(
                        reload_warnings
                            .into_iter()
                            .map(|reason| json!({"reason":reason})),
                    );
                }
                let _ = tokio::fs::remove_dir_all(&recovery).await;
                report
            }
            Err(error)
                if !entered.load(std::sync::atomic::Ordering::Acquire)
                    && !context.is_cancelled()
                    && error.0 != StatusCode::CONFLICT =>
            {
                tracing::warn!(
                    "入库标题临时配置不可用：《{media_name}》，{}；继续最终元数据刷新",
                    error.1
                );
                final_refresh(false).await?;
                let _ = tokio::fs::remove_dir_all(&recovery).await;
                json!({"completed":false,"failed":1,"pending":[{"reason":error.1}]})
            }
            Err(error) => {
                if confirmed.load(std::sync::atomic::Ordering::Acquire)
                    && (error.0 != StatusCode::CONFLICT || error.1 == "任务已取消")
                {
                    let _ = tokio::fs::remove_dir_all(&recovery).await;
                }
                return Err(error);
            }
        }
    } else {
        final_refresh(false).await?;
        json!({"completed":false,"skipped":true,"reason":"入库语言标题填充未启用"})
    };
    let mut warning_details = vec![];
    let title_warnings = title_report["failed"]
        .as_u64()
        .unwrap_or(0)
        .max(list(&title_report["pending"]).len() as u64);
    let fingerprint_warnings = fingerprint_report["failed"].as_u64().unwrap_or(0);
    let audio_failed = audio_report["failed"].as_u64().unwrap_or(0);
    let audio_pending = audio_report["warnings"].as_u64().unwrap_or(0);
    let audio_warnings = audio_failed + audio_pending;
    let mut warnings = title_warnings + fingerprint_warnings + audio_warnings;
    for (count, reason) in [
        (title_warnings, title_warning_detail(&title_report)),
        (
            fingerprint_warnings,
            format!(
                "资产指纹：仍有 {fingerprint_warnings} 个未补齐；{}",
                string(&fingerprint_report, "message")
            ),
        ),
        (
            audio_warnings,
            format!(
                "音轨修复：失败 {audio_failed} 个，未确认 {audio_pending} 个；{}",
                list(&audio_report["warning_details"])
                    .first()
                    .and_then(Value::as_str)
                    .or_else(|| audio_report["failed_items"][0]["reason"].as_str())
                    .or_else(|| audio_report["error"].as_str())
                    .unwrap_or("请查看任务结果")
            ),
        ),
    ] {
        if count > 0 {
            warning_details.push(format!("《{media_name}》（媒体ID：{id}）{reason}"));
        }
    }
    let cover = async {
        if let Some(cover_config) = crate::db::setting(&state.db, "cover_generator_config").await?
            && cover_config["enabled"] == true
            && cover_config["transfer_monitor"] != false
            && let Some(library) =
                crate::emby::library_for_path(state, &string(&item, "Path")).await?
        {
            return crate::covers::generate(state, Some(&[string(&library, "ItemId")]), context)
                .await;
        }
        Ok::<_, ApiError>(json!({}))
    }
    .await;
    if let Ok(report) = &cover {
        let failures = list(&report["failures"]);
        warnings += report["warnings"].as_u64().unwrap_or(0) + failures.len() as u64;
        for reason in list(&report["warning_details"]).into_iter().chain(failures) {
            if let Some(reason) = reason.as_str() {
                warning_details.push(format!(
                    "《{media_name}》（媒体ID：{id}）入库封面：{reason}"
                ));
            }
        }
    } else if let Err(error) = cover {
        if context.is_cancelled() {
            return Err(error);
        }
        warnings += 1;
        let reason = format!(
            "《{media_name}》（媒体ID：{id}）入库封面生成失败：{}",
            error.1
        );
        tracing::warn!("{reason}");
        warning_details.push(reason);
    }
    let notification = if let Some(notices) = movie_notices.filter(|_| item["Type"] == "Movie") {
        collect_movie_notice(notices, &item, &title_report);
        Ok(())
    } else {
        crate::notifications::media(state, &item, updated, episode_ids, &title_report).await
    };
    if let Err(error) = notification {
        if context.is_cancelled() {
            return Err(error);
        }
        warnings += 1;
        let reason = format!("《{media_name}》（媒体ID：{id}）入库通知失败：{}", error.1);
        tracing::warn!("{reason}");
        warning_details.push(reason);
    }
    tracing::info!(
        "入库收尾处理结束：Emby {id}，名称 {}，指纹未完成 {} 个，音轨修复失败 {audio_failed} 个、未确认 {audio_pending} 个，标题失败 {} 个、待处理 {} 项，后处理警告 {warnings} 条",
        string(&item, "Name"),
        fingerprint_warnings,
        title_report["failed"].as_u64().unwrap_or(0),
        list(&title_report["pending"]).len()
    );
    Ok(
        json!({"title_fill":title_report,"fingerprints":fingerprint_report,"audio_repair":audio_report,"warnings":warnings,"warning_details":warning_details}),
    )
}
