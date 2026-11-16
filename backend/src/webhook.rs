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
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

fn scope_key(value: &Value) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(value.to_string().as_bytes()))
}

type IngestNotices =
    BTreeMap<(String, String), (Value, Vec<Value>, BTreeMap<String, Value>, Vec<String>)>;
fn collect_ingest_notice(
    notices: &mut IngestNotices,
    item: &Value,
    report: &Value,
    episode_ids: &[String],
) {
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    let name = string(item, "Name");
    let key = if item["Type"] == "Series" {
        format!("series:{}", string(item, "Id"))
    } else if !tmdb.trim().is_empty() {
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
    let notice = notices
        .entry((key, year.trim().to_string()))
        .or_insert_with(|| (item.clone(), vec![], BTreeMap::new(), vec![]));
    notice.1.extend(list(&report["pending"]));
    notice.2.insert(string(item, "Id"), item.clone());
    let ids = if item["Type"] == "Series" {
        episode_ids.to_vec()
    } else {
        vec![string(item, "Id")]
    };
    for id in ids {
        if !notice.3.contains(&id) {
            notice.3.push(id);
        }
    }
}

async fn deliver_notices(
    state: &AppState,
    notices: IngestNotices,
    updated: bool,
    identities: &[crate::moviepilot_ingest::Identity],
    context: &crate::jobs::JobContext,
    warnings: &mut Value,
) -> Result<()> {
    for (item, pending, native, ids) in notices.into_values() {
        let key = format!(
            "ingest_notified:{}",
            scope_key(&json!({"id":item["Id"],"ids":ids}))
        );
        if context.completed(&key).await?.is_some() {
            continue;
        }
        for identity in identities
            .iter()
            .filter(|identity| ids.contains(&identity.id))
        {
            crate::moviepilot_ingest::unchanged(state, identity, context).await?;
        }
        context.checkpoint().await?;
        let result = crate::notifications::media(
            state,
            &item,
            updated,
            &ids,
            &json!({"pending":pending}),
            &native.into_values().collect::<Vec<_>>(),
            Some(context),
        )
        .await;
        if let Err(error) = result {
            if context.is_cancelled() {
                return Err(error);
            }
            let reason = format!("《{}》入库通知失败：{}", string(&item, "Name"), error.1);
            tracing::warn!("{reason}");
            add_report_warnings(warnings, &json!({"warnings":1,"warning_details":[reason]}));
        } else {
            context.complete(&key, &json!({"queued":true})).await?;
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct Service {
    pending: Mutex<BTreeMap<String, Value>>,
    batches: Mutex<BTreeMap<String, Instant>>,
    manual_deletes: Mutex<BTreeMap<String, Instant>>,
    draining: std::sync::atomic::AtomicBool,
}
impl Service {
    async fn mark_enhanced(
        &self,
        state: &AppState,
        top_id: &str,
        members: &BTreeMap<String, String>,
        identities: &[crate::moviepilot_ingest::Identity],
    ) -> Result<()> {
        let mut transaction = state.db.begin().await?;
        for (id, path) in members
            .iter()
            .filter(|(id, path)| !id.is_empty() && !path.is_empty())
        {
            let identity = identities
                .iter()
                .find(|identity| identity.id == *id)
                .ok_or_else(|| ApiError::bad("预增强成员缺少原文件版本记录"))?;
            sqlx::query("INSERT INTO webhook_prepared(item_id,top_id,path,revision_json,expires_at) VALUES($1,$2,$3,$4,NOW()+INTERVAL '1 hour') ON CONFLICT(item_id) DO UPDATE SET top_id=EXCLUDED.top_id,path=EXCLUDED.path,revision_json=EXCLUDED.revision_json,expires_at=EXCLUDED.expires_at")
                .bind(id).bind(top_id).bind(path).bind(identity.revision()).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }
    async fn take_enhanced(
        &self,
        state: &AppState,
        top_id: &str,
        members: &[Value],
        context: &crate::jobs::JobContext,
    ) -> Result<BTreeSet<String>> {
        let key = format!("emby_prepared:{top_id}");
        if let Some(saved) = context.completed(&key).await? {
            let covered: BTreeSet<String> = serde_json::from_value(saved["ids"].clone())
                .map_err(|_| ApiError::bad("预增强成员的持久记录无效"))?;
            for member in members {
                let id = string(member, "Id");
                if covered.contains(&id)
                    && crate::moviepilot_ingest::observe(state, member, context)
                        .await?
                        .revision()
                        != saved["revisions"][&id]
                {
                    return Err(ApiError::bad(
                        "已预增强的文件在中断期间换版，请重新提交当前文件入库",
                    ));
                }
            }
            return Ok(covered);
        }
        let ids = members
            .iter()
            .map(|member| string(member, "Id"))
            .collect::<Vec<_>>();
        let mut transaction = state.db.begin().await?;
        let enhanced: BTreeMap<String,(String,Value)> = sqlx::query_as::<_,(String,String,Value)>(
            "SELECT p.item_id,p.path,COALESCE(p.revision_json,'null'::jsonb) FROM webhook_prepared p WHERE p.top_id=$1 AND p.item_id=ANY($2) AND (p.expires_at>NOW() OR EXISTS(SELECT 1 FROM media_organize_history h WHERE h.source='moviepilot' AND h.status='processing' AND h.item_id=p.item_id)) FOR UPDATE")
            .bind(top_id).bind(&ids).fetch_all(&mut *transaction).await?.into_iter().map(|(id,path,revision)|(id,(path,revision))).collect();
        let mut covered = BTreeSet::new();
        let mut revisions = BTreeMap::new();
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
                    .is_some_and(|(prepared_path, _)| prepared_path == &path)
            {
                let revision = &enhanced[&id].1;
                if revision.is_null()
                    || crate::moviepilot_ingest::observe(state, member, context)
                        .await?
                        .revision()
                        != *revision
                {
                    return Err(ApiError::bad(
                        "预增强原文件版本与当前来源不一致，请重新提交当前文件入库",
                    ));
                }
                revisions.insert(id.clone(), revision.clone());
                covered.insert(id);
            }
        }
        sqlx::query("DELETE FROM webhook_prepared WHERE top_id=$1 AND item_id=ANY($2)")
            .bind(top_id)
            .bind(covered.iter().cloned().collect::<Vec<_>>())
            .execute(&mut *transaction)
            .await?;
        context
            .complete_in_tx(
                &mut transaction,
                &key,
                &json!({"ids":covered,"revisions":revisions}),
            )
            .await?;
        transaction.commit().await?;
        context
            .complete(&key, &json!({"ids":covered,"revisions":revisions}))
            .await?;
        Ok(covered)
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
fn merge_pending_event(old: &mut Value, event: &Value) {
    let ids = list(&old["episode_ids"])
        .into_iter()
        .chain(list(&event["episode_ids"]))
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect::<std::collections::BTreeSet<_>>();
    old["episode_ids"] = json!(ids);
    old["history_ids"] = json!(
        list(&old["history_ids"])
            .into_iter()
            .chain(list(&event["history_ids"]))
            .collect::<Vec<_>>()
    );
}
pub(crate) async fn execute_persisted(
    state: &AppState,
    payload: &Value,
    context: &crate::jobs::JobContext,
) -> Result<(u16, Value)> {
    let event = &payload["event"];
    if !event.is_object() {
        return Err(ApiError::bad("持久 Webhook 事件无效"));
    }
    if let Some(result) = context.completed("webhook_result").await? {
        return Ok((200, result));
    }
    tracing::info!(
        "Webhook 任务处理开始：来源 {}，Emby {}，目标分集 {} 个",
        string(event, "source"),
        string(event, "id"),
        list(&event["episode_ids"]).len()
    );
    let mut result = std::panic::AssertUnwindSafe(async {
        if event["source"] == "moviepilot" {
            process_moviepilot(state, event, context).await
        } else {
            process_emby(state, event, context).await
        }
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "任务执行异常".into(),
        ))
    });
    if context.is_interrupted() {
        return result.map(|value| (200, value));
    }
    let history_result = if event["source"] == "moviepilot" {
        crate::media_organize_history::finish_moviepilot(state, event, context, &result).await
    } else {
        crate::media_organize_history::finish(state, event, &result).await
    };
    if let Err(error) = history_result {
        tracing::error!("入库处理记录未能保存：{}", error.1);
        if result.is_ok() {
            result = Err(error);
        }
    }
    if event["source"] == "moviepilot"
        && result.as_ref().map_or(true, |value| {
            value["failures"]
                .as_array()
                .is_some_and(|failures| !failures.is_empty())
        })
        && let Some(key) = event["batch_key"].as_str()
    {
        state.webhooks.batches.lock().await.remove(key);
    }
    let result = result?;
    context.complete("webhook_result", &result).await?;
    Ok((200, result))
}

async fn registered_event(state: &AppState, pending_id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM durable_jobs WHERE spec_json->>'kind'='webhook' AND spec_json#>>'{payload,event,_pending_id}'=$1)")
        .bind(pending_id).fetch_one(&state.db).await?)
}

async fn submit_queued_event(state: &AppState, key: &str, event: &Value) -> Result<()> {
    if !registered_event(state, &string(event, "_pending_id")).await? {
        let payload = json!({"event":event});
        let spec = crate::jobs::JobSpec::new("webhook", payload.clone());
        let work = state.clone();
        state
            .jobs
            .submit_persistent_response("Webhook 入库", spec, move |context| async move {
                execute_persisted(&work, &payload, &context).await
            })
            .await?;
    }
    sqlx::query("DELETE FROM webhook_pending WHERE event_key=$1 AND event_json=$2")
        .bind(key)
        .bind(event)
        .execute(&state.db)
        .await?;
    Ok(())
}

fn start_draining(state: &AppState) {
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
            if state.jobs.is_shutting_down().await {
                state
                    .webhooks
                    .draining
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                break;
            }
            if state.jobs.status().await["is_running"] == true {
                continue;
            }
            // Enqueue and registration share this lock so a merged member
            // cannot be lost between the durable task insert and pending delete.
            let mut pending = state.webhooks.pending.lock().await;
            let Some((key, event)) = pending
                .first_key_value()
                .map(|(key, event)| (key.clone(), event.clone()))
            else {
                state
                    .webhooks
                    .draining
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                break;
            };
            match submit_queued_event(&state, &key, &event).await {
                Ok(()) => {
                    pending.remove(&key);
                }
                Err(error) => tracing::warn!("Webhook 等待任务调度：{}", error.1),
            }
        }
    });
}

async fn enqueue(state: &AppState, key: String, mut event: Value) -> Result<()> {
    let mut pending = state.webhooks.pending.lock().await;
    let previous: Option<Value> =
        sqlx::query_scalar("SELECT event_json FROM webhook_pending WHERE event_key=$1")
            .bind(&key)
            .fetch_optional(&state.db)
            .await?;
    let previous = if let Some(old) = previous {
        if registered_event(state, &string(&old, "_pending_id")).await? {
            None
        } else {
            Some(old)
        }
    } else {
        None
    };
    let event = if let Some(mut old) = previous {
        merge_pending_event(&mut old, &event);
        old
    } else {
        event["_pending_id"] = json!(uuid::Uuid::new_v4().to_string());
        event["_queue_key"] = json!(key);
        event
    };
    sqlx::query("INSERT INTO webhook_pending(event_key,event_json) VALUES($1,$2) ON CONFLICT(event_key) DO UPDATE SET event_json=EXCLUDED.event_json,updated_at=NOW()")
        .bind(&key).bind(&event).execute(&state.db).await?;
    tracing::info!(
        "Webhook 排队已持久保存：任务 {key}，来源 {}，目标分集 {} 个",
        string(&event, "source"),
        list(&event["episode_ids"]).len()
    );
    pending.insert(key, event);
    drop(pending);
    start_draining(state);
    Ok(())
}

pub async fn resume_pending_with_retry(
    state: &AppState,
    cancel: &tokio_util::sync::CancellationToken,
) {
    let mut delay = 1_u64;
    loop {
        let result = tokio::select! {
            _ = cancel.cancelled() => return,
            result = resume_pending(state) => result,
        };
        match result {
            Ok(()) => return,
            Err(error) => {
                tracing::warn!("待入库事件恢复尚未完成，保留原事件并自动重试：{}", error.1)
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
        }
        delay = (delay * 2).min(8);
    }
}

pub async fn resume_pending(state: &AppState) -> Result<()> {
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT event_key,event_json FROM webhook_pending ORDER BY updated_at,event_key",
    )
    .fetch_all(&state.db)
    .await?;
    let mut pending = state.webhooks.pending.lock().await;
    pending.extend(rows);
    drop(pending);
    start_draining(state);
    let prepared:Vec<(String,String,String,Value,Vec<String>)>=sqlx::query_as(
        "SELECT p.top_id,p.item_id,p.path,COALESCE(p.revision_json,'null'::jsonb),array_agg(h.id ORDER BY h.id) FROM webhook_prepared p JOIN media_organize_history h ON h.item_id=p.item_id AND h.source='moviepilot' AND h.status='processing' GROUP BY p.top_id,p.item_id,p.path,p.revision_json ORDER BY p.top_id,p.item_id")
        .fetch_all(&state.db).await?;
    let mut groups = BTreeMap::<String, (Vec<Value>, BTreeSet<String>)>::new();
    for (top, id, path, revision, history) in prepared {
        let group = groups.entry(top).or_default();
        group
            .0
            .push(json!({"id":id,"path":path,"revision":revision}));
        group.1.extend(history);
    }
    for (top, (members, history_ids)) in groups {
        let hash = scope_key(&json!(members));
        let registered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_jobs WHERE spec_json->>'kind'='webhook' AND spec_json#>>'{payload,event,_resume_prepared}'=$1 UNION ALL SELECT 1 FROM webhook_pending WHERE event_json->>'_resume_prepared'=$1)")
            .bind(&hash).fetch_one(&state.db).await?;
        if registered {
            continue;
        }
        let first = &members[0];
        let episodes = members
            .iter()
            .filter(|member| member["id"] != top)
            .map(|member| member["id"].clone())
            .collect::<Vec<_>>();
        enqueue(
            state,
            format!("emby:{top}:item.add"),
            json!({"source":"emby","id":top,
            "episode_ids":episodes,"event":"item.add","history_ids":history_ids,
            "original":{"Id":first["id"],"Path":first["path"],
                "Type":if episodes.is_empty(){"Movie"}else{"Episode"},"SeriesId":top},
            "_resume_prepared":hash}),
        )
        .await?;
    }
    Ok(())
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
    let history_ids =
        crate::media_organize_history::begin_emby(&state, &event, item, &path, library.as_ref())
            .await?;
    enqueue(
        &state,
        format!("emby:{queue_id}:{event}"),
        json!({"source":"emby","id":top,"episode_ids":ids,"event":event,"original":item,"history_ids":history_ids}),
    )
    .await?;
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
async fn delete_fid(state: &AppState, slot: &DeleteSlot) -> Result<Option<String>> {
    if !matches!(slot.kind.as_str(), "Movie" | "Episode")
        || (!string(&slot.asset, "emby_item_id").is_empty()
            && string(&slot.asset, "emby_item_id") != slot.id)
    {
        return Ok(None);
    }
    if slot.asset["fingerprint_pending"] == true || slot.asset["moviepilot_media_pending"] == true {
        return Err(ApiError::bad("文件身份待校验，停止 115 联动删除"));
    }
    if slot.pc.is_empty() {
        // A final path can already refer to the replacement while its old
        // deletion event is still in flight. Only a recorded entity is usable.
        tracing::warn!(
            "[深度删除] Item {} 缺少旧文件 PickCode，保留云端文件。",
            slot.id
        );
        return Ok(None);
    }
    let cached: Vec<String> =
        sqlx::query_scalar("SELECT id FROM p115_filesystem_cache WHERE pick_code=$1")
            .bind(&slot.pc)
            .fetch_all(&state.db)
            .await?;
    let fid = match cached.as_slice() {
        [fid] => Some(fid.clone()),
        [] => crate::fingerprints::file_id(&slot.pc).map(|id| id.to_string()),
        _ => None,
    };
    Ok(fid.filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())))
}
async fn delete_path_reused(state: &AppState, path: &str) -> Result<bool> {
    if path.is_empty() {
        return Err(ApiError::bad(
            "深度删除缺少可核验的原文件路径，保留云端文件",
        ));
    }
    if std::path::Path::new(path).is_absolute() {
        match tokio::fs::symlink_metadata(path).await {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    let response = state
        .emby(
            reqwest::Method::GET,
            "Items",
            &[
                ("Path", path.into()),
                ("Recursive", "true".into()),
                (
                    "IncludeItemTypes",
                    "Movie,Episode,Video,MusicVideo,Audio".into(),
                ),
                ("Fields", "Id,Path".into()),
                ("EnableImages", "false".into()),
                ("EnableUserData", "false".into()),
                ("GroupItemsIntoCollections", "false".into()),
            ],
            None,
        )
        .await?;
    let items = response["Items"]
        .as_array()
        .ok_or_else(|| ApiError::bad("深度删除路径查询返回无效媒体列表，保留云端文件"))?;
    let total = response["TotalRecordCount"]
        .as_u64()
        .or_else(|| response["TotalRecordCount"].as_str()?.parse().ok());
    if total != Some(items.len() as u64)
        || items
            .iter()
            .any(|item| string(item, "Id").is_empty() || item["Path"] != path)
    {
        return Err(ApiError::bad(
            "深度删除路径查询未证明当前归属，保留云端文件",
        ));
    }
    Ok(!items.is_empty())
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
    if crate::versions::existing(state, std::slice::from_ref(&id))
        .await?
        .contains(&id)
    {
        tracing::warn!("[深度删除] Item {id} 仍在 Emby 中，保留当前媒体和云端文件。");
        return Ok(json!({"status":"deep_delete_ignored_live"}));
    }
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
    let existing = crate::versions::existing(state, &removed).await?;
    removed.retain(|id| !existing.contains(id));
    if config["p115_enable_sync_delete"] == true {
        let mut fids = BTreeSet::new();
        let mut retained_fids = BTreeSet::new();
        for slot in &slots {
            if let Some(fid) = delete_fid(state, slot).await? {
                if removed.contains(&slot.id)
                    && !delete_path_reused(state, &string(&slot.asset, "path")).await?
                {
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
    let existing = crate::versions::existing(state, &removed).await?;
    removed.retain(|id| !existing.contains(id));
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
    let persisted: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM webhook_pending WHERE event_json->>'batch_key'=$1 UNION ALL SELECT 1 FROM durable_jobs WHERE spec_json->>'kind'='webhook' AND spec_json#>>'{payload,event,batch_key}'=$1 AND state_json->>'state' NOT IN ('failed','cancelled') AND (state_json->>'state' IN ('running','interrupted','paused','pausing') OR updated_at>NOW()-INTERVAL '1 hour'))")
        .bind(&key).fetch_one(&state.db).await?;
    if batches.contains_key(&key) || persisted {
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
    body["history_ids"] = match crate::media_organize_history::begin_moviepilot(&state, &body).await
    {
        Ok(ids) => json!(ids),
        Err(error) => {
            state.webhooks.batches.lock().await.remove(&key);
            return Err(error);
        }
    };
    tracing::info!(
        "MoviePilot Webhook 已接受：批次 {key}，有效文件 {} 个，根路径 {root}",
        files.len()
    );
    if let Err(error) = enqueue(&state, key.clone(), body).await {
        state.webhooks.batches.lock().await.remove(&key);
        return Err(error);
    }
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
    context.checkpoint().await?;
    if event["_resume_prepared"].is_string() {
        let ids = list(&event["history_ids"])
            .into_iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        let remaining:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_organize_history WHERE id=ANY($1) AND status='processing')")
            .bind(&ids).fetch_one(&state.db).await?;
        if !remaining {
            return Ok(
                json!({"updated":0,"status":"already_completed","message":"原批次已由Emby回流完成"}),
            );
        }
    }
    let id = string(event, "id");
    let item = details(
        state,
        &id,
        "Id,Type,Name,Path,DateCreated,ImageTags,ProductionYear,MediaSources,MediaStreams",
    )
    .await?;
    let media_name = string(&item, "Name");
    if crate::emby::path_ignored(state, &string(&item, "Path")).await? {
        return Ok(json!({"updated":0,"status":"ignored_library"}));
    }
    tracing::info!(
        "Emby 入库处理开始：《{media_name}》，Emby {id}，事件 {}",
        string(event, "event")
    );
    let episode_batch = !list(&event["episode_ids"]).is_empty();
    let saved_scope = context.plan("emby_scope").await?;
    let mut episodes = list(&event["episode_ids"])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if event["event"] == "metadata.update" {
        tracing::info!("Emby 元数据更新事件：《{media_name}》，Emby {id}，同步元数据库");
        return mirror(state, &string(&event["original"], "Id"), context).await;
    }
    if event["event"] == "image.update" {
        return Ok(json!({"message":"图片更新已接收"}));
    }
    if let Some(scope) = &saved_scope {
        episodes = serde_json::from_value(scope["episodes"].clone())
            .map_err(|_| ApiError::bad("Emby原入库分集范围记录无效"))?;
    } else if item["Type"] == "Series" && episodes.is_empty() {
        episodes = crate::emby::items(state, &id, "Id,Type,Path", context)
            .await?
            .into_iter()
            .filter(|child| child["Type"] == "Episode")
            .map(|child| string(&child, "Id"))
            .filter(|id| !id.is_empty())
            .collect();
    }
    let mut members = vec![];
    if episodes.is_empty() {
        let original_id = string(&event["original"], "Id");
        if !original_id.is_empty() && original_id != id && item["Type"] == "Movie" {
            members.push(
                details(
                    state,
                    &original_id,
                    "Id,Type,Name,Path,DateCreated,SeriesId,SeriesName,SeriesPrimaryImageTag,ImageTags,ProductionYear,ParentIndexNumber,IndexNumber,ParentId,ExtraType,MediaSources,MediaStreams",
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
                    "Id,Type,Name,Path,DateCreated,SeriesId,SeriesName,SeriesPrimaryImageTag,ImageTags,ProductionYear,ParentIndexNumber,IndexNumber,ParentId,ExtraType,MediaSources,MediaStreams",
                )
                .await?,
            );
        }
    }
    let identities = members
        .iter()
        .map(|member| {
            json!({"id":member["Id"],"path":member["Path"],
        "created":member["DateCreated"],"sources":list(&member["MediaSources"]).iter()
            .map(|source|crate::media_paths::clean(&string(source,"Path"))).collect::<Vec<_>>()})
        })
        .collect::<Vec<_>>();
    let scope = json!({"id":id,"path":item["Path"],"created":item["DateCreated"],"episodes":episodes,"members":identities});
    if let Some(original) = &saved_scope {
        if original != &scope {
            return Err(ApiError::bad(
                "Emby原入库ID、路径或文件范围在中断期间变化，未扩大处理范围",
            ));
        }
    } else {
        context.save_plan("emby_scope", &scope).await?;
    }
    for member in &members {
        crate::media_organize_history::bind_item(state, event, &string(member, "Path"), member)
            .await?;
    }
    // A prepared member is consumed once. Different batches in the same series
    // cannot inherit its shortcut, and a failed finalization leaves no claimed marker.
    let covered = state
        .webhooks
        .take_enhanced(state, &id, &members, context)
        .await?;
    let enhanced = !members.is_empty()
        && members
            .iter()
            .all(|member| covered.contains(&string(member, "Id")));
    if !enhanced && event["source"] == "emby" {
        let key = format!("emby_preflight:{id}");
        if context.completed(&key).await?.is_none() {
            let report = crate::media_preflight::run(state, &item, &context.scoped(&key)).await?;
            context.complete(&key, &report).await?;
        }
        for member in &members {
            if string(member, "Id") != id && !covered.contains(&string(member, "Id")) {
                context.checkpoint().await?;
                let key = format!("emby_preflight:{}", string(member, "Id"));
                if context.completed(&key).await?.is_none() {
                    let report =
                        crate::media_preflight::run(state, member, &context.scoped(&key)).await?;
                    context.complete(&key, &report).await?;
                }
            }
            crate::moviepilot_ingest::clear_pending(state, member).await?;
        }
    }
    let mut result = if let Some(report) = context.completed("emby_feedback").await? {
        report
    } else {
        let report = match crate::ingest::feedback(state, &id, &episodes, false, context).await {
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
        context.complete("emby_feedback", &report).await?;
        report
    };
    let mut notices = IngestNotices::new();
    let mut finalization = if let Some(report) = context.completed("emby_finalized").await? {
        let current = details(
            state,
            &id,
            "Id,Type,Name,Path,ProductionYear,ProviderIds,ImageTags,MediaSources,MediaStreams",
        )
        .await?;
        collect_ingest_notice(&mut notices, &current, &report["title_fill"], &episodes);
        report
    } else {
        let report = finalize_inner(
            state,
            &id,
            &episodes,
            !episodes.is_empty(),
            context,
            Some(&mut notices),
            None,
        )
        .await?;
        context.complete("emby_finalized", &report).await?;
        report
    };
    let extraction_ready = extraction_ready(&finalization);
    tracing::info!(
        "Emby 入库处理完成：《{media_name}》，Emby {id}，更新 {} 条记录",
        result["updated"].as_u64().unwrap_or(0)
    );
    let automatic = if let Some(report) = context.completed("emby_automatic").await? {
        report
    } else {
        let automatic_context = context.scoped(&format!("emby_automatic:{}", scope_key(&scope)));
        let report = automatic_extraction(
            state,
            item["Type"] == "Series" && episode_batch,
            item["Type"] == "Movie" || item["Type"] == "Series" && episode_batch,
            &episodes,
            extraction_ready,
            &format!("《{media_name}》"),
            &automatic_context,
        )
        .await?;
        context.complete("emby_automatic", &report).await?;
        report
    };
    deliver_notices(
        state,
        notices,
        !episodes.is_empty(),
        &[],
        context,
        &mut finalization,
    )
    .await?;
    add_report_warnings(&mut result, &finalization);
    result["finalization"] = finalization;
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
        let mut notices = IngestNotices::new();
        for (id, name, year) in [
            ("1", "译名", 1998),
            ("2", "原名", 1998),
            ("3", "译名", 1999),
        ] {
            collect_ingest_notice(
                &mut notices,
                &json!({"Id":id,"Name":name,"ProductionYear":year,"ProviderIds":{"Tmdb":"4426"}}),
                &json!({"pending":[{"item_id":id,"reason":"待回读"}]}),
                &[],
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
        collect_ingest_notice(
            &mut notices,
            &json!({"Id":"4","Name":"4426","ProductionYear":1998}),
            &json!({}),
            &[],
        );
        collect_ingest_notice(
            &mut notices,
            &json!({"Id":"4426","ProductionYear":1998}),
            &json!({}),
            &[],
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

fn extraction_ready(finalization: &Value) -> bool {
    finalization["metadata_refresh"]["completed"] == true
}

async fn automatic_extraction(
    state: &AppState,
    series: bool,
    videos: bool,
    episode_ids: &[String],
    finalization_ready: bool,
    label: &str,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let config = state.config.read().await.clone();
    let intro_enabled = series && config["auto_extract_intro_fingerprint_on_ingest"] == true;
    let credits_enabled = series && config["auto_extract_credits_on_ingest"] == true;
    let thumbnails_enabled = videos && config["auto_extract_video_thumbnails_on_ingest"] == true;
    if !intro_enabled && !credits_enabled && !thumbnails_enabled {
        return Ok(Value::Null);
    }
    if !finalization_ready {
        let reason = "入库元数据刷新或已启用的标题填充尚未完成验收，已跳过片头声纹、片尾和视频预览缩略图提取";
        tracing::info!("[入库自动处理] {label}{reason}");
        let mut report =
            json!({"state":"skipped","reason":reason,"warnings":0,"warning_details":[]});
        if intro_enabled {
            report["intro"] = json!({"state":"skipped","reason":reason});
        }
        if credits_enabled {
            report["credits"] = json!({"state":"skipped","reason":reason});
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
        if (credits_enabled || thumbnails_enabled) && !ended {
            let reason = "片头声纹任务尚未确认结束，跳过片尾和视频预览缩略图提取";
            if credits_enabled {
                report["credits"] = json!({"state":"skipped","reason":reason});
            }
            if thumbnails_enabled {
                report["video_thumbnails"] = json!({"state":"skipped","reason":reason});
            }
            tracing::warn!("[入库自动处理] {label}{reason}");
            report["warnings"] = json!(warning_details.len());
            report["warning_details"] = json!(warning_details);
            return Ok(report);
        }
    }
    if credits_enabled {
        context.checkpoint().await?;
        context
            .activity(
                "片尾提取",
                &format!("{label}开始提取本次入库电视剧分集片尾，逐集核对 Emby 标记"),
            )
            .await;
        let credits = if episode_ids.is_empty() {
            json!({"state":"failed","failed":1,"native_ended":true,"reason":"本次入库分集范围为空，未扩大至全库提取"})
        } else {
            match crate::credits::extract_ids(state, episode_ids, context).await {
                Ok(result) => result,
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => {
                    json!({"state":"failed","failed":1,"native_ended":true,"reason":error.1})
                }
            }
        };
        let failed = credits["failed"].as_u64().unwrap_or(0) > 0
            || matches!(credits["state"].as_str(), Some("failed" | "unverified"));
        warning_details.extend(
            list(&credits["warning_details"])
                .iter()
                .filter_map(|warning| warning.as_str().map(str::to_string)),
        );
        if failed {
            let warning = format!(
                "{label}片尾提取或 Emby 标记回读未全部通过：{}；保留已完成的入库结果",
                string(&credits, "reason")
            );
            tracing::warn!("[入库自动处理] {warning}");
            warning_details.push(warning);
        } else {
            tracing::info!(
                "[入库自动处理] {label}片尾处理结果：{}；{}",
                extraction_state_label(&credits),
                string(&credits, "reason")
            );
        }
        let ended = credits["native_ended"] == true;
        report["credits"] = credits;
        if thumbnails_enabled && (failed || !ended) {
            report["video_thumbnails"] = json!({"state":"skipped","reason":"片尾提取或标记回读尚未完成验收，跳过视频预览缩略图提取"});
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

#[cfg(test)]
mod automatic_credits_tests {
    use super::*;
    mod fixture {
        include!("../tests/support/extraction_threads.rs");
    }

    #[test]
    fn title_field_warnings_do_not_gate_configured_extraction_after_metadata_ingestion() {
        let disabled = json!({"title_fill":{"completed":false,"skipped":true},"metadata_refresh":{"completed":true}});
        assert!(extraction_ready(&disabled));
        let mut unverified = disabled;
        unverified["metadata_refresh"]["completed"] = json!(false);
        assert!(!extraction_ready(&unverified));
        assert!(extraction_ready(
            &json!({"title_fill":{"completed":false,"skipped":1},"metadata_refresh":{"completed":true}})
        ));
        assert!(extraction_ready(
            &json!({"title_fill":{"completed":true},"metadata_refresh":{"completed":true}})
        ));
        assert!(!extraction_ready(
            &json!({"title_fill":{"completed":true},"metadata_refresh":{"completed":false}})
        ));
    }

    #[tokio::test]
    async fn unconfirmed_metadata_refresh_skips_all_three_extraction_stages() {
        let fixture = fixture::Fixture::new().await;
        *fixture.state.config.write().await = json!({"auto_extract_intro_fingerprint_on_ingest":true,"auto_extract_credits_on_ingest":true,"auto_extract_video_thumbnails_on_ingest":true});
        let context = crate::jobs::JobContext::for_recovery(None).await;
        let report = automatic_extraction(
            &fixture.state,
            true,
            true,
            &["101".into()],
            false,
            "测试分集",
            &context,
        )
        .await
        .unwrap();
        for key in ["intro", "credits", "video_thumbnails"] {
            assert_eq!(report[key]["state"], "skipped", "{report}");
        }
    }

    #[tokio::test]
    async fn unconfirmed_credits_do_not_start_thumbnails_or_expand_to_whole_library() {
        let fixture = fixture::Fixture::new().await;
        fixture.state.config.write().await["auto_extract_credits_on_ingest"] = json!(true);
        fixture.state.config.write().await["auto_extract_video_thumbnails_on_ingest"] = json!(true);
        let context = crate::jobs::JobContext::for_recovery(None).await;
        let report =
            automatic_extraction(&fixture.state, true, true, &[], true, "测试分集", &context)
                .await
                .unwrap();
        assert_eq!(report["credits"]["state"], "failed", "{report}");
        assert!(
            report["credits"]["reason"]
                .as_str()
                .unwrap()
                .contains("分集范围为空")
        );
        assert_eq!(report["video_thumbnails"]["state"], "skipped", "{report}");
        assert_eq!(report["warnings"], 1);
        assert_eq!(fixture.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

async fn mirror(state: &AppState, id: &str, context: &crate::jobs::JobContext) -> Result<Value> {
    context.checkpoint().await?;
    let item=details(state,id,"ProviderIds,Type,Name,OriginalTitle,Overview,Tags,TagItems,OfficialRating,CustomRating,PremiereDate,ProductionYear").await?;
    context.checkpoint().await?;
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
    context.checkpoint().await?;
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
                    "Id,Type,Name,SeriesId,SeriesName,SeriesPrimaryImageTag,ImageTags,ProductionYear,ParentIndexNumber,IndexNumber,ParentId,ExtraType,ProviderIds,Path,MediaSources,MediaStreams"
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

async fn sync_moviepilot_media_info(
    state: &AppState,
    path: &str,
    context: &crate::jobs::JobContext,
) -> Result<()> {
    let _slot = crate::strm::slot(state, "main", Some(context)).await?;
    context.checkpoint().await?;
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod extraction_thread_tests {
    use super::*;
    use std::sync::atomic::Ordering;
    mod support {
        include!("../tests/support/extraction_threads.rs");
    }

    #[tokio::test]
    async fn moviepilot_sync_waits_for_shared_main_thread_and_plugin_cooldown() {
        let fixture = support::Fixture::new().await;
        let context = crate::jobs::JobContext::for_recovery(None).await;
        let primary = crate::strm::slot(&fixture.state, "main", Some(&context))
            .await
            .unwrap();
        let request = sync_moviepilot_media_info(&fixture.state, "/media/episode.strm", &context);
        tokio::pin!(request);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut request)
                .await
                .is_err()
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
        let released = tokio::time::Instant::now();
        drop(primary);
        assert!(
            tokio::time::timeout(Duration::from_millis(700), &mut request)
                .await
                .is_err()
        );
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
        tokio::time::timeout(Duration::from_secs(2), &mut request)
            .await
            .unwrap()
            .unwrap();
        assert!(released.elapsed() >= Duration::from_millis(900));
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    }
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
                    result = sync_moviepilot_media_info(state, path, context) => result,
                };
                if let Err(error) = result {
                    tracing::warn!("MoviePilot 单项媒体信息提取失败：{}", error.1);
                }
            }
        }
        pending = missing_moviepilot_media_info(state, files, context).await?;
        if pending.is_empty() {
            return Ok(());
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
    process_moviepilot_inner(state, event, context, false).await
}

pub(crate) async fn process_reorganize(
    state: &AppState,
    event: &Value,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    process_moviepilot_inner(state, event, context, true).await
}

async fn process_moviepilot_inner(
    state: &AppState,
    event: &Value,
    context: &crate::jobs::JobContext,
    reorganize: bool,
) -> Result<Value> {
    let saved_scope = context.plan("moviepilot_scope").await?;
    let awaiting_emby = saved_scope
        .as_ref()
        .and_then(|scope| scope["awaiting_emby"].as_bool())
        .unwrap_or(!reorganize && state.config.read().await["emby_webhook_ingest_enabled"] == true);
    let mut files = saved_scope
        .as_ref()
        .map(|scope| list(&scope["files"]))
        .unwrap_or_default();
    if saved_scope.is_none() {
        for file in list(&event["files"]) {
            if !crate::emby::path_ignored(state, file.as_str().unwrap_or("")).await? {
                files.push(file);
            }
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
    let mut notices = IngestNotices::new();
    let mut postprocessing = json!({});
    let mut automatic_series = false;
    let mut automatic_videos = false;
    let mut automatic_episode_ids = vec![];
    let mut extraction_ready_for_batch = true;
    let tmdb = string(event, "tmdb_id");
    let rows: Vec<Value> = if tmdb.is_empty() {
        vec![]
    } else {
        sqlx::query_scalar("SELECT asset_details_json FROM media_metadata WHERE in_library=true AND ((tmdb_id=$1 AND item_type='Movie') OR (parent_series_tmdb_id=$1 AND item_type='Episode'))").bind(&tmdb).fetch_all(&state.db).await?
    };
    let mut known = rows
        .iter()
        .flat_map(list)
        .map(|asset| crate::media_paths::clean(&string(&asset, "path")))
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>();
    if let Some(scope) = &saved_scope {
        known = serde_json::from_value(scope["known"].clone())
            .map_err(|_| ApiError::bad("MoviePilot原媒体范围记录无效"))?;
    }
    let new_files = files
        .iter()
        .filter(|file| !known.contains(&crate::media_paths::clean(file.as_str().unwrap_or(""))))
        .cloned()
        .collect::<Vec<_>>();
    let retry_files = if let Some(scope) = &saved_scope {
        serde_json::from_value(scope["retry_files"].clone())
            .map_err(|_| ApiError::bad("MoviePilot重试范围记录无效"))?
    } else {
        crate::media_organize_history::retry_files(state, event).await?
    };
    if saved_scope.is_none() {
        context
            .save_plan(
                "moviepilot_scope",
                &json!({"files":files,"known":known,
            "retry_files":retry_files,"awaiting_emby":awaiting_emby}),
            )
            .await?;
    }
    tracing::info!(
        "MoviePilot 新文件核对完成：TMDb {tmdb}，有效文件 {} 个，新增 {} 个，已关联 {} 个",
        files.len(),
        new_files.len(),
        files.len() - new_files.len()
    );
    if !new_files.is_empty()
        && !reorganize
        && context
            .completed("moviepilot_library_scan")
            .await?
            .is_none()
    {
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
        context
            .complete("moviepilot_library_scan", &json!({"completed":true}))
            .await?;
    }
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut prepared_members = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut current = vec![];
    let mut changed = Vec::new();
    let mut initial_files = Vec::new();
    for file in &files {
        context.checkpoint().await?;
        let path = file.as_str().unwrap_or("");
        let attempt_key = format!("moviepilot_attempt:{}", scope_key(&json!(path)));
        context
            .save_plan(&attempt_key, &json!({"path":path,"skipped":false}))
            .await?;
        let mut found = None;
        let attempts = if reorganize { 1 } else { 90 };
        for attempt in 0..attempts {
            context.checkpoint().await?;
            found = moviepilot_item(state, path).await?;
            if found.is_some() {
                break;
            }
            if attempt + 1 < attempts {
                context.sleep(Duration::from_secs(2)).await?;
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
        crate::media_organize_history::bind_item(state, event, path, &item).await?;
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
        let identity = crate::moviepilot_ingest::observe(state, &item, context).await?;
        let file_key = format!("moviepilot_file:{}", scope_key(&json!(path)));
        let original = context.plan(&file_key).await?;
        if let Some(original) = &original {
            let expected: crate::moviepilot_ingest::Identity =
                serde_json::from_value(original["identity"].clone())
                    .map_err(|_| ApiError::bad("MoviePilot原文件实体记录无效"))?;
            if !identity.same_file(&expected) {
                return Err(ApiError::bad(
                    "媒体在任务中断期间变化，已停止原任务，请重新提交当前文件",
                ));
            }
        }
        if crate::moviepilot_ingest::completed(state, &identity).await?
            && !identity.entity.is_null()
            && !reorganize
            && !retry_files.contains(path)
            && original.is_none()
        {
            context
                .save_plan(&attempt_key, &json!({"path":path,"skipped":true}))
                .await?;
            continue;
        }
        if original.is_none() {
            context
                .save_plan(&file_key, &json!({"identity":identity}))
                .await?;
        }
        initial_files.push(file.clone());
        changed.push(identity);
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
    if !initial_files.is_empty()
        && context
            .completed("moviepilot_initial_media")
            .await?
            .is_none()
    {
        tracing::info!(
            "MoviePilot 媒体信息提取开始：本批文件 {} 个",
            initial_files.len()
        );
        let revisions = changed
            .iter()
            .filter(|identity| initial_files.contains(&json!(identity.path)))
            .map(crate::moviepilot_ingest::Identity::revision)
            .collect::<Vec<_>>();
        let initial_context = context.scoped(&format!(
            "mp_initial_media:{}",
            scope_key(&json!(revisions))
        ));
        crate::emby::scheduled(state, "MediaInfoExtractTask", &initial_context).await?;
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
        ensure_moviepilot_media_info(state, &initial_files, context).await?;
        tracing::info!(
            "MoviePilot 媒体信息提取完成：本批文件 {} 个",
            initial_files.len()
        );
    }
    for identity in &changed {
        crate::moviepilot_ingest::remember_initial(state, identity, context).await?;
    }
    if !initial_files.is_empty() {
        context
            .complete("moviepilot_initial_media", &json!({"files":initial_files}))
            .await?;
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
        for identity in changed
            .iter()
            .filter(|identity| identity.id == id || episodes.contains(&identity.id))
        {
            crate::moviepilot_ingest::unchanged(state, identity, context).await?;
        }
        let preparation = async {
            let key = format!("moviepilot_prepared:{id}");
            if let Some(report) = context.completed(&key).await? {
                return Ok(report);
            }
            let prepared = if reorganize {
                None
            } else {
                crate::ingest::prepare_cached_report(
                state,
                &item,
                &episodes,
                event["episode_group"].as_str().filter(|s| !s.is_empty()),
                context,
            )
            .await?
            };
            let report = if let Some(report) = prepared {
                report
            } else {
                crate::ingest::prepare_new(
                    state,
                    &item,
                    &episodes,
                    event["episode_group"].as_str().filter(|s| !s.is_empty()),
                    context,
                )
                .await?
            };
            context.complete(&key, &report).await?;
            Ok::<Value, ApiError>(report)
        }
        .await
        .map(|report| add_report_warnings(&mut postprocessing, &report));
        match preparation {
            Ok(()) => match async {
                let key = format!("moviepilot_feedback:{id}");
                if let Some(result) = context.completed(&key).await? {
                    return Ok(result);
                }
                for identity in changed
                    .iter()
                    .filter(|identity| identity.id == id || episodes.contains(&identity.id))
                {
                    crate::moviepilot_ingest::unchanged(state, identity, context).await?;
                }
                let current = if episodes.is_empty() {
                    vec![id.clone()]
                } else {
                    episodes.clone()
                };
                crate::versions::reconcile(state, &current).await?;
                let result = crate::ingest::feedback(state, &id, &episodes, true, context).await?;
                // 新绑定可能直到回流才建立；登记神医提取阶段，不增加字段回读准入检查。
                for identity in changed.iter().filter(|identity| {
                    identity.id == id || episodes.contains(&identity.id)
                }) {
                    crate::moviepilot_ingest::remember_initial(state, identity, context).await?;
                }
                crate::versions::reconcile(state, &current).await?;
                if awaiting_emby && let Some(members) = prepared_members.get(&id) {
                    state
                        .webhooks
                        .mark_enhanced(state, &id, members, &changed)
                        .await?;
                }
                context.complete(&key, &result).await?;
                Ok::<Value, ApiError>(result)
            }
            .await
            {
                Ok(result) => {
                    updated += result["updated"].as_u64().unwrap_or(0);
                    add_report_warnings(&mut postprocessing, &result);
                    if !awaiting_emby {
                        for identity in changed
                            .iter()
                            .filter(|identity| identity.id == id || episodes.contains(&identity.id))
                        {
                            crate::moviepilot_ingest::unchanged(state, identity, context).await?;
                        }
                        let key = format!("moviepilot_finalized:{id}");
                        let finalized = if let Some(report) = context.completed(&key).await? {
                            let current=details(state,&id,"Id,Type,Name,Path,ProviderIds,ProductionYear,ImageTags,MediaSources,MediaStreams").await?;
                            collect_ingest_notice(
                                &mut notices,
                                &current,
                                &report["title_fill"],
                                &episodes,
                            );
                            Ok(report)
                        } else {
                            let report = finalize_inner(
                                state,
                                &id,
                                &episodes,
                                !known.is_empty(),
                                context,
                                Some(&mut notices),
                                Some(&changed),
                            )
                            .await;
                            if let Ok(report) = &report {
                                context.complete(&key, report).await?;
                            }
                            report
                        };
                        match finalized {
                            Ok(report) => {
                                if item["Type"] == "Movie"
                                    || item["Type"] == "Series" && !episodes.is_empty()
                                {
                                    extraction_ready_for_batch &= extraction_ready(&report);
                                }
                                add_report_warnings(&mut postprocessing, &report);
                                finalizations
                                    .push(json!({"item_id":id,"name":media_name,"report":report}));
                                automatic_series |=
                                    item["Type"] == "Series" && !episodes.is_empty();
                                if item["Type"] == "Series" {
                                    for episode in &episodes {
                                        if !automatic_episode_ids.contains(episode) {
                                            automatic_episode_ids.push(episode.clone());
                                        }
                                    }
                                }
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
    for identity in &changed {
        crate::moviepilot_ingest::unchanged(state, identity, context).await?;
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
        if let Some(report) = context.completed("moviepilot_automatic").await? {
            report
        } else {
            let revisions = changed
                .iter()
                .map(crate::moviepilot_ingest::Identity::revision)
                .collect::<Vec<_>>();
            let automatic_context =
                context.scoped(&format!("mp_automatic:{}", scope_key(&json!(revisions))));
            let report = automatic_extraction(
                state,
                automatic_series,
                automatic_videos,
                &automatic_episode_ids,
                extraction_ready_for_batch,
                "本批媒体",
                &automatic_context,
            )
            .await?;
            context.complete("moviepilot_automatic", &report).await?;
            report
        }
    } else {
        Value::Null
    };
    if failures.is_empty() {
        for identity in &changed {
            let key = format!("moviepilot_finished:{}", identity.id);
            if context.completed(&key).await?.is_none() {
                crate::moviepilot_ingest::finish(state, identity, context).await?;
                context.complete(&key, &identity.revision()).await?;
            } else if !crate::moviepilot_ingest::completed(state, identity).await? {
                return Err(ApiError::bad(
                    "重启后原文件入库完成记录发生变化，未发送成功通知",
                ));
            }
        }
        deliver_notices(
            state,
            notices,
            !known.is_empty(),
            &changed,
            context,
            &mut postprocessing,
        )
        .await?;
    }
    let mut result = json!({"updated":updated,"failures":failures,"finalizations":finalizations,"awaiting_emby":awaiting_emby,"message":message});
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
    finalize_inner(state, id, episode_ids, updated, context, None, None).await
}
async fn finalize_inner(
    state: &AppState,
    id: &str,
    episode_ids: &[String],
    updated: bool,
    context: &crate::jobs::JobContext,
    notices: Option<&mut IngestNotices>,
    identities: Option<&[crate::moviepilot_ingest::Identity]>,
) -> Result<Value> {
    tracing::info!(
        "入库收尾开始：Emby {id}，目标分集 {} 个，更新模式 {updated}",
        episode_ids.len()
    );
    let item = details(
        state,
        id,
        "Id,Type,Name,Path,Overview,ProductionYear,ProviderIds,Genres,MediaSources",
    )
    .await?;
    if let Some(identities) = identities {
        for identity in identities
            .iter()
            .filter(|identity| identity.id == id || episode_ids.contains(&identity.id))
        {
            crate::moviepilot_ingest::unchanged(state, identity, context).await?;
        }
    }
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
    let recovery = state
        .data_dir
        .join("backups/media-info-reload")
        .join(uuid::Uuid::new_v4().simple().to_string());
    context
        .scoped(&format!("ingest-finalize:{id}"))
        .save_plan("rescrape-recovery", &json!(recovery))
        .await?;
    let refresh_guard = (&confirmed, recovery.as_path());
    let refresh_item = &item;
    let refresh_name = &media_name;
    let refresh_completed = std::sync::atomic::AtomicBool::new(false);
    let refresh_verified = &refresh_completed;
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
        let protection = crate::media_reload::ingest_preservation::capture_targets(
            state,
            refresh_item,
            episode_ids,
            context,
            refresh_guard.1,
        )
        .await;
        let protection = match protection {
            Ok(protection) => protection,
            Err(error)
                if !context.is_cancelled()
                    && refresh_guard.0.load(std::sync::atomic::Ordering::Acquire) =>
            {
                // 原生 Refresh 会展开关联版本；只读准备失败后改走原 ID 元数据接口。
                tracing::info!("入库元数据使用定点更新，媒体字段问题不阻断流程：{}", error.1);
                if !tokio::fs::try_exists(refresh_guard.1.join("original-settings.json")).await? {
                    tokio::fs::remove_dir_all(refresh_guard.1).await?;
                }
                Box::pin(crate::emby::finalize_metadata_only_targets(
                    state,
                    refresh_item,
                    episode_ids,
                    context,
                ))
                .await?;
                refresh_verified.store(true, std::sync::atomic::Ordering::Release);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let refresh = || async {
            if let Some(identities) = identities {
                for identity in identities
                    .iter()
                    .filter(|identity| identity.id == id || episode_ids.contains(&identity.id))
                {
                    crate::moviepilot_ingest::unchanged(state, identity, context).await?;
                }
            }
            protection.check(state, context).await?;
            // Publish after entering the existing settings guard, whose entry
            // correctly refuses unresolved recovery records from earlier work.
            protection.publish().await?;
            if guarded || protection.protected() {
                crate::emby::finalize_refresh_targets_guarded(
                    state,
                    refresh_item,
                    episode_ids,
                    context,
                    refresh_guard.0,
                    refresh_guard.1,
                )
                .await?;
            } else {
                crate::emby::finalize_refresh_targets(state, refresh_item, episode_ids, context)
                    .await?;
            }
            if protection.protected() {
                // A completion log establishes that native refresh ended; it
                // does not establish that the original resources survived.
                refresh_guard
                    .0
                    .store(false, std::sync::atomic::Ordering::Release);
                let finishing = crate::jobs::JobContext::for_recovery(Some(context)).await;
                protection.verify(state, &finishing).await?;
                refresh_guard
                    .0
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Ok::<_, ApiError>(())
        };
        let result = if protection.protected() && !guarded {
            crate::strm::with_settings_guarded(
                state,
                "restore_media_info",
                refresh,
                refresh_guard.0,
                refresh_guard.1,
            )
            .await
        } else {
            refresh().await
        };
        result.inspect_err(|error| {
            tracing::error!(
                "入库最终元数据刷新失败：《{refresh_name}》，Emby {id}，{}",
                error.1
            )
        })?;
        tracing::info!("入库最终元数据刷新完成：《{refresh_name}》，Emby {id}");
        refresh_verified.store(true, std::sync::atomic::Ordering::Release);
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
                Box::pin(crate::titles::fill_in_job_with_refresh(
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
                ))
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
            Err(error) if !context.is_cancelled() => {
                tracing::warn!(
                    "入库标题处理未完成：《{media_name}》，{}；继续最终元数据更新",
                    error.1
                );
                Box::pin(crate::emby::finalize_metadata_only_targets(
                    state, &item, episode_ids, context,
                )).await?;
                refresh_completed.store(true, std::sync::atomic::Ordering::Release);
                if confirmed.load(std::sync::atomic::Ordering::Acquire) {
                    let _ = tokio::fs::remove_dir_all(&recovery).await;
                }
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
        let _ = tokio::fs::remove_dir_all(&recovery).await;
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
    if let Some(identities) = identities {
        for identity in identities
            .iter()
            .filter(|identity| identity.id == id || episode_ids.contains(&identity.id))
        {
            crate::moviepilot_ingest::unchanged(state, identity, context).await?;
        }
    }
    let notification = if let Some(notices) = notices {
        collect_ingest_notice(notices, &item, &title_report, episode_ids);
        Ok(())
    } else {
        crate::notifications::media(
            state,
            &item,
            updated,
            episode_ids,
            &title_report,
            std::slice::from_ref(&item),
            Some(context),
        )
        .await
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
        json!({"title_fill":title_report,"metadata_refresh":{"completed":refresh_completed.load(std::sync::atomic::Ordering::Acquire)},"fingerprints":fingerprint_report,"audio_repair":audio_report,"warnings":warnings,"warning_details":warning_details}),
    )
}

#[cfg(test)]
#[path = "webhook_tests.rs"]
mod tests;
