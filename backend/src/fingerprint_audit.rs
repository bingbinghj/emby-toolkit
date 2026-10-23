use crate::{
    AppState, db,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tokio::io::AsyncWriteExt;

const PROGRESS: &str = "fingerprint_scan_progress";
const INVENTORY: &str = "fingerprint_scan_inventory";
const REPORT: &str = "fingerprint-scan-latest.json";
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/tasks/fingerprint-scan", get(status).post(submit))
        .route("/api/tasks/fingerprint-scan/report", get(report))
}
async fn status(State(state): State<AppState>) -> Result<Json<Value>> {
    let progress = db::setting(&state.db, PROGRESS).await?.map(|p| summary(&p));
    Ok(Json(
        json!({"progress":progress,"report_available":state.data_dir.join(REPORT).is_file()}),
    ))
}
pub fn summary(progress: &Value) -> Value {
    let mut result = json!({});
    for key in [
        "run_id",
        "state",
        "total",
        "counts",
        "updated_at",
        "report_error",
    ] {
        result[key] = progress[key].clone();
    }
    let remaining = list(&progress["pending"]).len();
    result["remaining"] = json!(remaining);
    result["completed"] = json!(
        progress["total"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(remaining as u64)
    );
    result["failed"] = json!(list(&progress["failed"]).len());
    result
}
fn advice(reason: &str) -> &'static str {
    if reason.contains("大小不一致") {
        "重新提取这些分集或电影的 Emby 媒体信息，同步 ETK 后重试无法确认项；若仍不一致，核对 STRM 目标和视频版本。"
    } else if ["路径已变化", "绑定不", "缺少", "当前版本不存在"]
        .iter()
        .any(|s| reason.contains(s))
    {
        "先同步媒体数据，再重新扫描全库；旧扫描清单中的路径或绑定不能直接沿用。"
    } else if ["不唯一", "映射", "目标不存在"]
        .iter()
        .any(|s| reason.contains(s))
    {
        "核对 STRM/CD2 根目录映射、目标文件名和同名文件，确认唯一目标后重试。"
    } else if ["检查后变化", "检查期间变化"]
        .iter()
        .any(|s| reason.contains(s))
    {
        "文件或媒体记录在核验时发生变化；等待整理结束后重试，避免覆盖新数据。"
    } else {
        "查看该项原始错误，检查服务连接及文件身份信息，问题解决后重试无法确认项。"
    }
}
pub fn build_report(progress: &Value) -> Value {
    let mut keys = progress["entries"]
        .as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let order = list(&progress["entry_order"]);
    keys.sort_by_key(|key| {
        (
            order
                .iter()
                .position(|v| v.as_str() == Some(key))
                .unwrap_or(usize::MAX),
            key.parse::<u64>().unwrap_or(u64::MAX),
            key.clone(),
        )
    });
    let entries = keys
        .iter()
        .map(|key| progress["entries"][key].clone())
        .collect::<Vec<_>>();
    let mut reasons = Vec::<(String, usize)>::new();
    for entry in entries.iter().filter(|e| e["state"] == "unknown") {
        let reason = entry["error"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("原因未记录");
        if let Some((_, count)) = reasons.iter_mut().find(|(r, _)| r == reason) {
            *count += 1;
        } else {
            reasons.push((reason.into(), 1));
        }
    }
    reasons.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut result = json!({"version":1});
    for key in [
        "run_id",
        "state",
        "total",
        "counts",
        "updated_at",
        "note",
        "error",
    ] {
        result[key] = progress[key].clone();
    }
    let remaining = list(&progress["pending"]).len() as u64;
    result["remaining"] = json!(remaining);
    result["completed"] = json!(
        progress["total"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(remaining)
    );
    result["missing_details"] = json!(
        (progress["counts"]["repaired"].as_u64().unwrap_or(0)
            + progress["counts"]["unknown"].as_u64().unwrap_or(0))
        .saturating_sub(entries.len() as u64)
    );
    result["analysis"] = json!(
        reasons
            .iter()
            .map(|(r, n)| json!({"reason":r,"count":n,"advice":advice(r)}))
            .collect::<Vec<_>>()
    );
    result["entries"] = json!(entries);
    result
}
async fn report(
    State(state): State<AppState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Response> {
    let bytes = tokio::fs::read(state.data_dir.join(REPORT))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::NOT_FOUND,
                "尚无扫描报告，请先运行全库指纹扫描".into(),
            )
        })?;
    let mut response = (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response();
    if query.get("download").is_some_and(|s| s == "1") {
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename={REPORT}").parse().unwrap(),
        );
    }
    Ok(response)
}
async fn submit(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let mode = body["mode"].as_str().unwrap_or("start");
    if !["start", "resume", "retry"].contains(&mode) {
        return Err(ApiError::bad("不支持的扫描操作"));
    }
    if !body["run_id"].is_null() && !body["run_id"].is_string() {
        return Err(ApiError::bad("任务ID无效"));
    }
    if mode != "start" {
        let progress = db::setting(&state.db, PROGRESS)
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "扫描断点不存在".into()))?;
        if body["run_id"]
            .as_str()
            .is_some_and(|id| progress["run_id"] != id)
        {
            return Err(ApiError(StatusCode::NOT_FOUND, "扫描断点不存在".into()));
        }
        if mode == "retry" && !list(&progress["pending"]).is_empty() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "请先继续完成上次扫描".into(),
            ));
        }
    }
    let work = state.clone();
    let mode = mode.to_string();
    let run_id = body["run_id"].as_str().map(str::to_string);
    let task = state
        .jobs
        .submit("全库指纹校验与补齐", move |context| async move {
            scan(&work, &mode, run_id.as_deref(), &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"全库指纹校验与补齐已提交，可查看最新报告和日志"})),
    ))
}
async fn config_identity(state: &AppState) -> String {
    let config = state.config.read().await;
    let values = json!([
        config["emby_server_url"],
        config["emby_user_id"],
        config["p115_fingerprint_media_root_cid"],
        config["local_strm_root"],
        config["cd2_media_root"]
    ]);
    let encoded = format!(
        "[{}]",
        values
            .as_array()
            .unwrap()
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Sha256::digest(encoded.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub async fn save(state: &AppState, progress: &mut Value) -> Result<()> {
    progress["updated_at"] = json!(chrono::Utc::now().timestamp_millis() as f64 / 1000.0);
    progress.as_object_mut().unwrap().remove("report_error");
    db::save_setting(&state.db, PROGRESS, progress).await?;
    let path = state.data_dir.join(REPORT);
    let stage = path.with_file_name(format!(".{REPORT}.{}", uuid::Uuid::new_v4().simple()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)
            .await?;
        file.write_all(&serde_json::to_vec_pretty(&build_report(progress)).unwrap())
            .await?;
        file.sync_all().await?;
        tokio::fs::rename(&stage, path).await?;
        Ok::<_, ApiError>(())
    }
    .await;
    let _ = tokio::fs::remove_file(stage).await;
    if let Err(error) = result {
        let message = format!("保存指纹报告失败：{}", error.1);
        progress["report_error"] = json!(message);
        db::save_setting(&state.db, PROGRESS, progress).await?;
        return Err(ApiError::bad(message));
    }
    Ok(())
}
async fn snapshot(state: &AppState, item: &Value) -> Result<(Value, usize)> {
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND tmdb_id=$1 AND item_type=$2 AND emby_item_ids_json @> $3").bind(string(item,"tmdb_id")).bind(string(item,"item_type")).bind(json!([item["item_id"]])).fetch_all(&state.db).await?;
    let mut matches = vec![];
    for row in rows {
        for (index, asset) in list(&row["asset_details_json"]).iter().enumerate() {
            if asset["emby_item_id"] == item["item_id"] && asset["path"] == item["path"] {
                matches.push((row.clone(), index));
            }
        }
    }
    if matches.len() != 1 {
        return Err(ApiError::bad(
            "当前版本不存在、路径已变化或绑定不唯一，请同步后重新扫描",
        ));
    }
    Ok(matches.pop().unwrap())
}
async fn check(
    state: &AppState,
    item: &Value,
    reader: &crate::revalidation::AuditReader<'_>,
) -> Result<(String, Value)> {
    let (mut row, index) = snapshot(state, item).await?;
    let asset = &row["asset_details_json"][index];
    let id = string(item, "item_id");
    let old = json!({"pc":list(&row["file_pickcode_json"]).get(index),"sha1":list(&row["file_sha1_json"]).get(index),"size":asset["size_bytes"]});
    let details = details(state, &id, "Id,Type,Name,Path,Size,MediaSources").await?;
    let observed = crate::revalidation::observation(state, &details).await;
    if observed.0 != string(asset, "path") {
        return Err(ApiError::bad("Emby路径已变化，需先同步后重新扫描"));
    }
    let (info, relative) = crate::revalidation::resolve_with_size(
        state,
        &observed.1,
        &observed.0,
        observed.2,
        Some(reader),
    )
    .await?;
    let current = details_fn(state, &id).await?;
    if crate::revalidation::observation(state, &current).await != observed {
        return Err(ApiError::bad("Emby媒体信息在检查期间变化，请重试"));
    }
    let mut result = json!({"old":old,"new":info,"emby_size":observed.2,"differences":[]});
    if observed
        .2
        .is_some_and(|s| Some(s) != crate::identity::size(&info["size"]))
    {
        result["error"] = json!("Emby与云端大小不一致，需重新提取媒体信息后重试");
        return Ok(("unknown".into(), result));
    }
    let mut differences = vec![];
    if old["pc"] != info["pick_code"] {
        differences.push("PC");
    }
    if old["sha1"].as_str().unwrap_or("").to_uppercase() != string(&info, "sha1").to_uppercase() {
        differences.push("SHA1");
    }
    if crate::identity::size(&old["size"]) != crate::identity::size(&info["size"]) {
        differences.push("大小");
    }
    if asset["fingerprint_pending"] == true {
        differences.push("待校验状态");
    }
    let cached: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(c) FROM p115_filesystem_cache c WHERE pick_code=$1 LIMIT 1",
    )
    .bind(string(&info, "pick_code"))
    .fetch_optional(&state.db)
    .await?;
    if cached.is_none_or(|c| {
        ["id", "parent_id", "name", "pick_code", "sha1"]
            .iter()
            .any(|k| string(&c, k).to_uppercase() != string(&info, k).to_uppercase())
            || crate::identity::size(&c["size"]) != crate::identity::size(&info["size"])
    }) {
        differences.push("115 文件缓存");
    }
    result["differences"] = json!(differences);
    if differences.is_empty() {
        return Ok(("consistent".into(), result));
    }
    let verified = crate::revalidation::resolve_with_size(
        state,
        &observed.1,
        &observed.0,
        observed.2,
        Some(reader),
    )
    .await?
    .0;
    if verified != info {
        return Err(ApiError::bad("云端文件在检查后变化，未更新"));
    }
    if crate::revalidation::observation(state, &details_fn(state, &id).await?).await != observed {
        return Err(ApiError::bad("Emby媒体信息在检查期间变化，未更新"));
    }
    let latest = snapshot(state, item).await?;
    if latest.0["asset_details_json"] != row["asset_details_json"]
        || latest.0["file_sha1_json"] != row["file_sha1_json"]
        || latest.0["file_pickcode_json"] != row["file_pickcode_json"]
    {
        return Err(ApiError::bad("媒体记录在检查后变化，未更新"));
    }
    crate::revalidation::mark_pending(state, &mut row, index, Some(&observed.1)).await?;
    crate::revalidation::write(state, &mut row, index, &verified, relative).await?;
    Ok(("repaired".into(), result))
}
async fn details_fn(state: &AppState, id: &str) -> Result<Value> {
    details(state, id, "Id,Type,Name,Path,Size,MediaSources").await
}
pub async fn scan(
    state: &AppState,
    mode: &str,
    run_id: Option<&str>,
    context: &JobContext,
) -> Result<Value> {
    let identity = config_identity(state).await;
    let (inventory, mut progress) = if mode == "start" {
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Episode')").fetch_all(&state.db).await?;
        let mut items = vec![];
        for row in rows {
            let assets = list(&row["asset_details_json"]);
            let assets = if assets.is_empty() {
                vec![json!({})]
            } else {
                assets
            };
            for asset in assets {
                items.push(json!({"tmdb_id":row["tmdb_id"],"item_type":row["item_type"],"item_id":asset["emby_item_id"].as_str().unwrap_or(""),"path":asset["path"].as_str().unwrap_or("")}));
            }
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let inventory = json!({"run_id":id,"context":identity,"items":items});
        let progress = json!({"run_id":id,"total":items.len(),"pending":(0..items.len()).collect::<Vec<_>>(),"failed":[],"counts":{"consistent":0,"repaired":0,"unknown":0},"state":"running","entries":{}});
        let mut transaction = state.db.begin().await?;
        for (key, value) in [(INVENTORY, &inventory), (PROGRESS, &progress)] {
            sqlx::query("INSERT INTO app_settings(setting_key,value_json) VALUES($1,$2) ON CONFLICT(setting_key) DO UPDATE SET value_json=EXCLUDED.value_json").bind(key).bind(value).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        (inventory, progress)
    } else {
        let inventory = db::setting(&state.db, INVENTORY)
            .await?
            .ok_or_else(|| ApiError::bad("扫描断点不存在"))?;
        let progress = db::setting(&state.db, PROGRESS)
            .await?
            .ok_or_else(|| ApiError::bad("扫描断点不存在"))?;
        if inventory["context"] != identity
            || inventory["run_id"] != progress["run_id"]
            || run_id.is_some_and(|id| progress["run_id"] != id)
        {
            return Err(ApiError::bad("扫描断点不存在或配置已变化，请重新扫描"));
        }
        (inventory, progress)
    };
    if mode == "retry" {
        if !list(&progress["pending"]).is_empty() {
            return Err(ApiError::bad("请先继续完成上次扫描"));
        }
        progress["pending"] = progress["failed"].clone();
        progress["failed"] = json!([]);
        progress["counts"]["unknown"] = json!(0);
        for ordinal in list(&progress["pending"]) {
            progress["entries"]
                .as_object_mut()
                .unwrap()
                .remove(&ordinal.to_string());
            if let Some(order) = progress["entry_order"].as_array_mut() {
                order.retain(|key| key.as_str() != Some(&ordinal.to_string()));
            }
        }
    }
    progress["state"] = json!("running");
    progress.as_object_mut().unwrap().remove("error");
    save(state, &mut progress).await?;
    let reader = crate::revalidation::AuditReader {
        context,
        previous: tokio::sync::Mutex::new(None),
        invalid_roots: tokio::sync::Mutex::new(Default::default()),
    };
    let outcome = async {
        for ordinal in list(&progress["pending"]) {
            context.checkpoint().await?;
            if config_identity(state).await != identity {
                return Err(ApiError::bad("扫描期间配置变化，请重新扫描"));
            }
            let item = &inventory["items"][ordinal.as_u64().unwrap() as usize];
            let result = check(state, item, &reader).await;
            let (kind, details) = match result {
                Ok(v) => v,
                Err(error) if error.0 == StatusCode::CONFLICT => return Err(error),
                Err(error) => ("unknown".into(), json!({"error":error.1})),
            };
            if kind != "consistent" {
                let mut entry = item.clone();
                entry["state"] = json!(kind);
                entry
                    .as_object_mut()
                    .unwrap()
                    .extend(details.as_object().unwrap().clone());
                let key = ordinal.to_string();
                if progress["entries"].get(&key).is_none() {
                    if !progress["entry_order"].is_array() {
                        let mut keys = progress["entries"]
                            .as_object()
                            .unwrap()
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>();
                        keys.sort_by_key(|k| k.parse::<u64>().unwrap_or(u64::MAX));
                        progress["entry_order"] = json!(keys);
                    }
                    progress["entry_order"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(key));
                }
                progress["entries"][key] = entry;
            }
            progress["counts"][&kind] = json!(progress["counts"][&kind].as_u64().unwrap() + 1);
            if kind == "unknown" {
                progress["failed"].as_array_mut().unwrap().push(ordinal);
            }
            progress["pending"].as_array_mut().unwrap().remove(0);
            save(state, &mut progress).await?;
            context
                .update(
                    100 * (progress["total"].as_u64().unwrap()
                        - list(&progress["pending"]).len() as u64)
                        / progress["total"].as_u64().unwrap().max(1),
                    "全库指纹",
                    &format!(
                        "一致 {}，已更新 {}，未确认 {}",
                        progress["counts"]["consistent"],
                        progress["counts"]["repaired"],
                        progress["counts"]["unknown"]
                    ),
                )
                .await;
        }
        Ok::<_, ApiError>(())
    }
    .await;
    if let Err(error) = outcome {
        progress["state"] = json!("interrupted");
        progress["error"] = json!(error.1);
        save(state, &mut progress).await?;
        return Err(error);
    }
    progress["state"] = json!(if list(&progress["failed"]).is_empty() {
        "complete"
    } else {
        "attention"
    });
    save(state, &mut progress).await?;
    Ok(
        json!({"run_id":progress["run_id"],"counts":progress["counts"],"warnings":progress["counts"]["unknown"],"message":"全库指纹扫描已完成，可查看报告"}),
    )
}
