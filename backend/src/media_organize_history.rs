//! Admission history is immutable in scope and time; never infer it from a
//! whole-library query or from the mutable, series-level processed_log.
use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    media_paths,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, TimeZone};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/media_organize/history", get(history).delete(delete_history))
}

/// Explicitly remove selected terminal receipts; media and files are unchanged.
async fn delete_history(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let selected = body["history_ids"]
        .as_array()
        .filter(|ids| !ids.is_empty() && ids.len() <= 1000)
        .ok_or_else(|| ApiError::bad("请选择 1 至 1000 条整理记录"))?;
    let mut ids = BTreeSet::new();
    for id in selected {
        let id = id
            .as_str()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or_else(|| ApiError::bad("整理记录 ID 无效"))?;
        ids.insert(id.to_string());
    }
    let ids = ids.into_iter().collect::<Vec<_>>();
    let mut transaction = state.db.begin().await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'status',status) FROM media_organize_history WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&ids).fetch_all(&mut *transaction).await?;
    if rows.len() != ids.len() {
        return Err(ApiError(StatusCode::NOT_FOUND, "部分整理记录已不存在，未删除任何记录".into()));
    }
    let unfinished: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_organize_queue q WHERE q.history_ids ?| $1::text[] AND (q.status IN ('queued','running') OR q.status='interrupted' AND NOT EXISTS(SELECT 1 FROM media_organize_queue other WHERE other.item_id=q.item_id AND other.mode=q.mode AND other.id<>q.id AND (other.status IN ('queued','running') OR other.queue_order>q.queue_order))))")
        .bind(&ids).fetch_one(&mut *transaction).await?;
    if unfinished || rows.iter().any(|row| !matches!(row["status"].as_str(), Some("success" | "warning" | "failed"))) {
        return Err(ApiError(StatusCode::CONFLICT, "所选记录仍有关联未完成整理任务，未删除任何记录".into()));
    }
    let deleted: Vec<String> = sqlx::query_scalar("DELETE FROM media_organize_history WHERE id=ANY($1) RETURNING id")
        .bind(&ids).fetch_all(&mut *transaction).await?;
    if deleted.len() != ids.len() {
        return Err(ApiError(StatusCode::CONFLICT, "整理记录删除未完整确认，未提交删除".into()));
    }
    transaction.commit().await?;
    Ok(Json(json!({"deleted_count":deleted.len(),"deleted_ids":deleted,
        "message":format!("已删除 {} 条整理记录",ids.len())})))
}

/// MP counts successful transfer history in the server's current calendar month.
/// ETK records one file per row and both sides of MP/Emby admission, so a source
/// file is counted once rather than counting its preparation and finalization twice.
pub async fn monthly_statistics(
    state: &AppState,
    now: chrono::DateTime<chrono::Local>,
) -> Result<Value> {
    let date = now.date_naive().with_day(1).unwrap();
    let midnight = |date: chrono::NaiveDate| -> Result<chrono::DateTime<chrono::Utc>> {
        chrono::Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .map(|time| time.with_timezone(&chrono::Utc))
            .ok_or_else(|| ApiError::bad("无法确定本月入库统计的时间边界"))
    };
    let start = midnight(date)?;
    let end = midnight(
        date.checked_add_months(chrono::Months::new(1))
            .ok_or_else(|| ApiError::bad("本月入库统计日期无效"))?,
    )?;
    let records: Vec<Value> = sqlx::query_scalar("SELECT DISTINCT ON (source_identity) item_json || jsonb_build_object('item_id',item_id,'source_identity',source_identity) FROM media_organize_history WHERE status IN ('success','warning') AND created_at >= $1 AND created_at < $2 ORDER BY source_identity,created_at DESC,id DESC")
        .bind(start).bind(end).fetch_all(&state.db).await?;
    let mut movies = BTreeSet::new();
    let mut shows = BTreeSet::new();
    let mut episodes = 0_u64;
    for item in records {
        let year = item["production_year"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                item["production_year"]
                    .as_i64()
                    .map(|year| year.to_string())
            })
            .unwrap_or_default();
        let fallback = string(&item, "source_identity");
        match item["item_type"].as_str() {
            Some("Movie") => {
                let name = string(&item, "name");
                let source = string(&item, "media_source");
                let media_id = string(&item, "media_id");
                movies.insert((
                    source,
                    media_id.clone(),
                    if name.is_empty() && media_id.is_empty() {
                        fallback
                    } else {
                        name
                    },
                    year,
                ));
            }
            Some("Episode") => {
                let series_id = string(&item, "series_id");
                let name = string(&item, "series_name");
                shows.insert(if series_id.is_empty() {
                    (
                        series_id,
                        if name.is_empty() { fallback } else { name },
                        year,
                    )
                } else {
                    (series_id, String::new(), String::new())
                });
                episodes += 1;
            }
            _ => {}
        }
    }
    let since = crate::db::setting(&state.db, "media_organize_history_started_at")
        .await?
        .unwrap_or(Value::Null);
    let partial = since
        .as_str()
        .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
        .is_none_or(|time| time.with_timezone(&chrono::Utc) > start);
    Ok(
        json!({"movie_count":movies.len(),"tv_count":shows.len(),"episode_count":episodes,
        "month":{"period":now.format("%Y-%m").to_string(),"state":"recording","source":"etk-history","recording_since":since,"partial":partial}}),
    )
}

pub async fn interrupt_unfinished(pool: &sqlx::PgPool) -> Result<()> {
    // Keep admissions whose accepted event or durable job can still resume;
    // only legacy orphaned history has no executable work after a restart.
    // This runs before queue recovery, including its latest interrupted intent.
    sqlx::query("UPDATE media_organize_history h
        SET status='failed',message='上次 ETK 入库处理缺少可恢复任务，请核对后重新整理',completed_at=NOW()
        WHERE h.status='processing'
        AND NOT EXISTS(SELECT 1 FROM media_organize_queue q WHERE q.history_ids ? h.id AND (q.status IN ('queued','running') OR q.status='interrupted' AND NOT EXISTS(SELECT 1 FROM media_organize_queue other WHERE other.item_id=q.item_id AND other.mode=q.mode AND other.id<>q.id AND (other.status IN ('queued','running') OR other.queue_order>q.queue_order))))
        AND NOT EXISTS(SELECT 1 FROM webhook_pending p WHERE p.event_json->'history_ids' ? h.id
            AND NOT EXISTS(SELECT 1 FROM durable_jobs registered WHERE registered.spec_json#>>'{payload,event,_pending_id}'=p.event_json->>'_pending_id'
                AND (registered.state_json->>'state' IN ('failed','cancelled') OR registered.state_json->>'stop_requested'='true')))
        AND NOT EXISTS(SELECT 1 FROM durable_jobs j WHERE j.spec_json->>'kind' IN ('webhook','monitor')
            AND COALESCE(j.state_json->>'stop_requested','false')<>'true'
            AND (j.state_json->>'state' IN ('running','interrupted','paused','pausing','stopping')
                AND (j.spec_json#>'{payload,event,history_ids}' ? h.id
                    OR jsonb_path_exists(j.checkpoint_json,'$.completed.*.ids[*] ? (@ == $id)',jsonb_build_object('id',h.item_id)))
                OR j.spec_json#>'{payload,event,history_ids}' ? h.id AND j.state_json#>'{result,awaiting_emby}'='true'::jsonb
                    AND j.state_json->>'state' IN ('completed','completed_with_warnings')
                    AND EXISTS(SELECT 1 FROM webhook_prepared p WHERE p.item_id=h.item_id)
                    AND NOT EXISTS(SELECT 1 FROM durable_jobs failed WHERE failed.spec_json->>'kind'='webhook'
                        AND failed.spec_json#>>'{payload,event,source}'='emby' AND failed.spec_json#>'{payload,event,history_ids}' ? h.id
                        AND failed.state_json->>'state' IN ('failed','cancelled'))))")
        .execute(pool).await?;
    Ok(())
}

fn identity(path: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(crate::media_paths::clean(path).as_bytes())
    )
}
pub(crate) async fn retry_files(state: &AppState, event: &Value) -> Result<BTreeSet<String>> {
    let paths = list(&event["files"])
        .into_iter()
        .filter_map(|file| file.as_str().map(str::to_string))
        .map(|path| (identity(&path), path))
        .collect::<HashMap<_, _>>();
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let current_ids = list(&event["history_ids"])
        .into_iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    // Only the latest finished admission for each explicit file determines
    // whether its cached assets still have unfinished work to retry.
    let failed: Vec<String> = sqlx::query_scalar("SELECT source_identity FROM (SELECT DISTINCT ON (source_identity) source_identity,status FROM media_organize_history WHERE source_identity=ANY($1) AND id<>ALL($2) AND status<>'processing' ORDER BY source_identity,created_at DESC,id DESC) previous WHERE status='failed'")
        .bind(paths.keys().cloned().collect::<Vec<_>>())
        .bind(current_ids)
        .fetch_all(&state.db)
        .await?;
    Ok(failed
        .into_iter()
        .filter_map(|id| paths.get(&id).cloned())
        .collect())
}
fn safe_text(text: &str) -> String {
    static URLS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?i)https?://[^\s\"'<>，；。)]+"#)
            .expect("static history URL expression")
    });
    let clean = URLS.replace_all(text, |capture: &regex::Captures<'_>| {
        safe_url(&capture[0]).unwrap_or_else(|| "[媒体源URL]".into())
    });
    crate::logs::redact(&clean).chars().take(2000).collect()
}
fn safe_url(path: &str) -> Option<String> {
    if let Ok(mut url) = url::Url::parse(path)
        && matches!(url.scheme(), "http" | "https")
    {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        return Some(url.to_string());
    }
    None
}
fn safe_path(path: &str) -> String {
    safe_url(path).unwrap_or_else(|| safe_text(path))
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
/// Upgrade original MoviePilot receipts using only their own retained job snapshot.
pub(crate) async fn migrate_file_names(tx: &mut sqlx::PgConnection) -> Result<()> {
    let first_run = sqlx::query("INSERT INTO app_settings(setting_key,value_json) VALUES('history_source_filenames_v1','true') ON CONFLICT DO NOTHING")
        .execute(&mut *tx).await?.rows_affected();
    if first_run == 0 { return Ok(()); }
    let snapshots: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT h.id, s.value_json#>>'{identity,source}', s.value_json#>>'{identity,cloud,name}'
         FROM durable_jobs j
         JOIN durable_job_steps s ON s.job_id=j.id AND starts_with(s.step_key,'plans:moviepilot_file:')
         JOIN media_organize_history h ON h.item_id=s.value_json#>>'{identity,id}'
            AND h.item_json->>'source_path'=s.value_json#>>'{identity,path}'
            AND (j.spec_json#>'{payload,event,history_ids}') ? h.id
         WHERE h.source='moviepilot' AND lower(h.item_json->>'file_name') LIKE '%.strm'").fetch_all(&mut *tx).await?;
    let mut ids = Vec::new();
    let mut names = Vec::new();
    for (id, source, cloud_name) in snapshots {
        if let Some(name) = [cloud_name, source].iter().flatten()
            .find_map(|path| media_paths::media_file_name(path))
        {
            ids.push(id);
            names.push(safe_text(&name));
        }
    }
    sqlx::query("UPDATE media_organize_history h SET item_json=jsonb_set(item_json,'{file_name}',to_jsonb(f.name))
        FROM unnest($1::text[], $2::text[]) f(id,name) WHERE h.id=f.id")
        .bind(ids).bind(names).execute(tx).await?;
    Ok(())
}

fn item_projection(item: &Value) -> Value {
    let id = string(item, "Id");
    let kind = string(item, "Type");
    let mut result = json!({"item_id": if valid_id(&id) && matches!(kind.as_str(), "Movie" | "Episode") {Some(id)} else {None},
        "name":safe_text(&string(item,"Name")), "item_type":kind});
    let path = string(item, "Path");
    if media_paths::extension(&path) == ".strm"
        && let Some(source) = crate::media_info::item_source(item, &[&path])
        && let Some(name) = media_paths::media_file_name(&string(source, "Path"))
    {
        result["file_name"] = json!(safe_text(&name));
    }
    if result["item_type"] == "Movie" {
        let tmdb = string(&item["ProviderIds"], "Tmdb");
        if !tmdb.is_empty() && tmdb.bytes().all(|byte| byte.is_ascii_digit()) {
            result["media_source"] = json!("tmdb");
            result["media_id"] = json!(tmdb);
        }
    }
    if let Some(tag) = item["ImageTags"]["Primary"].as_str() {
        result["image_tag"] = json!(tag);
    }
    for (field, native) in [
        ("series_name", "SeriesName"),
        ("series_id", "SeriesId"),
        ("series_image_tag", "SeriesPrimaryImageTag"),
        ("season_number", "ParentIndexNumber"),
        ("episode_number", "IndexNumber"),
        ("production_year", "ProductionYear"),
    ] {
        if item[native].is_string() || item[native].is_number() {
            result[field] = item[native].clone();
        }
    }
    result
}

async fn insert(
    connection: &mut sqlx::PgConnection,
    source: &str,
    path: &str,
    mut item: Value,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    item["source_path"] = json!(safe_path(path));
    let display_path = safe_path(path);
    let file_name = if let Ok(url) = url::Url::parse(&display_path)
        && matches!(url.scheme(), "http" | "https")
    {
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("")
            .to_string()
    } else {
        std::path::Path::new(&display_path)
            .file_name()
            .map(|name| safe_text(&name.to_string_lossy()))
            .unwrap_or_default()
    };
    if item["file_name"].is_null() {
        item["file_name"] = json!(file_name);
    }
    sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_id,item_json,status,message) VALUES($1,$2,$3,$4,$5,'processing','等待 ETK 入库处理')")
        .bind(&id).bind(source).bind(identity(path)).bind(item["item_id"].as_str()).bind(&item).execute(connection).await?;
    Ok(id)
}

pub async fn begin_emby(
    state: &AppState,
    event: &str,
    item: &Value,
    path: &str,
    library: Option<&Value>,
) -> Result<Vec<String>> {
    if !matches!(event, "item.add" | "library.new")
        || !matches!(item["Type"].as_str(), Some("Movie" | "Episode"))
    {
        return Ok(vec![]);
    }
    let mut projection = item_projection(item);
    projection["library_name"] = json!(
        library
            .map(|v| safe_text(&string(v, "Name")))
            .unwrap_or_default()
    );
    let mut transaction = state.db.begin().await?;
    let id = insert(&mut transaction, "emby", path, projection).await?;
    transaction.commit().await?;
    Ok(vec![id])
}

pub async fn begin_moviepilot(state: &AppState, event: &Value) -> Result<Vec<String>> {
    let movie = matches!(
        string(event, "media_type").to_lowercase().as_str(),
        "movie" | "电影"
    );
    let mut accepted = vec![];
    for file in list(&event["files"]) {
        let Some(path) = file.as_str() else { continue };
        // This uses only the accepted webhook's explicit files, never a scan.
        if crate::emby::path_ignored(state, path).await? {
            continue;
        }
        let file_name = std::path::Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let name = string(event, "title");
        accepted.push((
            path.to_string(),
            json!({"item_id":null,
            "item_type":if movie {"Movie"} else {"Episode"},
            "name":safe_text(if name.is_empty() {&file_name} else {&name}),"library_name":""}),
        ));
    }
    let mut transaction = state.db.begin().await?;
    let mut ids = vec![];
    for (path, item) in accepted {
        ids.push(insert(&mut transaction, "moviepilot", &path, item).await?);
    }
    transaction.commit().await?;
    Ok(ids)
}

pub async fn bind_item(state: &AppState, event: &Value, path: &str, item: &Value) -> Result<()> {
    let ids = list(&event["history_ids"])
        .into_iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(());
    }
    let mut projection = item_projection(item);
    if projection["item_id"].is_null() {
        return Ok(());
    }
    if projection.get("file_name").is_none()
        && let Some(target) = media_paths::strm_target(path).await
        && let Some(name) = media_paths::media_file_name(&target)
    {
        projection["file_name"] = json!(safe_text(&name));
    }
    let item_id = string(&projection, "item_id");
    sqlx::query("UPDATE media_organize_history SET item_id=$1,item_json=item_json || $2 WHERE id=ANY($3) AND source_identity=$4")
        .bind(item_id).bind(projection).bind(ids).bind(identity(path)).execute(&state.db).await?;
    Ok(())
}

pub async fn finish(state: &AppState, event: &Value, result: &Result<Value>) -> Result<()> {
    finish_scoped(state, event, result, None).await
}

pub async fn finish_moviepilot(
    state: &AppState,
    event: &Value,
    context: &crate::jobs::JobContext,
    result: &Result<Value>,
) -> Result<()> {
    if context.is_interrupted() {
        return Ok(());
    }
    // Original file plans retain admissions when the same job resumes. A new
    // event's unchanged files have only skipped attempts and add no history.
    let paths = context
        .saved_plans(|key| key.starts_with("moviepilot_file:") || key.starts_with("moviepilot_attempt:"))
        .await?
        .into_iter()
        .filter_map(|(key, value)| {
            if key.starts_with("moviepilot_file:") {
                value["identity"]["path"].as_str().map(identity)
            } else if key.starts_with("moviepilot_attempt:") && value["skipped"] == false {
                value["path"].as_str().map(identity)
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();
    finish_scoped(state, event, result, Some(&paths)).await
}

async fn finish_scoped(
    state: &AppState,
    event: &Value,
    result: &Result<Value>,
    paths: Option<&BTreeSet<String>>,
) -> Result<()> {
    let ids = list(&event["history_ids"])
        .into_iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(());
    }
    let (mut status, mut message) = outcome(result);
    let mut preparation = json!({});
    if event["source"] == "moviepilot"
        && status != "failed"
        && result
            .as_ref()
            .is_ok_and(|report| report["updated"].as_u64().unwrap_or(0) > 0)
        && match result
            .as_ref()
            .ok()
            .and_then(|report| report["awaiting_emby"].as_bool())
        {
            Some(awaiting) => awaiting,
            None => state.config.read().await["emby_webhook_ingest_enabled"] == true,
        }
    {
        if status == "warning" {
            preparation["_preparation_warning"] = json!(message);
        }
        status = "processing";
        message = "ETK 预增强已完成，等待 Emby 入库回流验收".into();
    }
    let mut transaction = state.db.begin().await?;
    if let Some(paths) = paths {
        sqlx::query("DELETE FROM media_organize_history WHERE id=ANY($1) AND source='moviepilot' AND status='processing' AND NOT(source_identity=ANY($2))")
            .bind(&ids).bind(paths.iter().cloned().collect::<Vec<_>>()).execute(&mut *transaction).await?;
    }
    let completed: Vec<Value> = sqlx::query_scalar("UPDATE media_organize_history SET status=$1,message=$2,completed_at=CASE WHEN $1='processing' THEN NULL ELSE NOW() END,item_json=item_json || $4 WHERE id=ANY($3) AND status='processing' AND ($5=false OR source='moviepilot') RETURNING item_json || jsonb_build_object('item_id',item_id)")
        .bind(status).bind(&message).bind(&ids).bind(preparation).bind(paths.is_some()).fetch_all(&mut *transaction).await?;
    if status == "warning" && let Ok(report) = result {
        for item in &completed {
            if let Some(id) = item["item_id"].as_str() {
                sqlx::query("UPDATE media_organize_history SET item_json=item_json || $3 WHERE id=ANY($1) AND item_id=$2")
                    .bind(&ids).bind(id).bind(warning_receipt(report,id)).execute(&mut *transaction).await?;
            }
        }
    }
    if completed.is_empty() {
        transaction.commit().await?;
        return Ok(());
    }
    if event["source"] == "emby" {
        // A MoviePilot admission remains pending until this exact file/episode
        // reaches Emby's finalization. Never promote its entire parent series.
        sqlx::query("UPDATE media_organize_history SET status=CASE WHEN $1='success' AND COALESCE(item_json->>'_preparation_warning','')<>'' THEN 'warning' ELSE $1 END,message=CASE WHEN COALESCE(item_json->>'_preparation_warning','')<>'' THEN $2 || '；预增强提醒：' || (item_json->>'_preparation_warning') ELSE $2 END,completed_at=NOW() WHERE source='moviepilot' AND status='processing' AND item_id IN (SELECT item_id FROM media_organize_history WHERE id=ANY($3) AND item_id IS NOT NULL)")
            .bind(status).bind(&message).bind(ids).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    if status == "failed" {
        notify_failures(state, completed, &message).await;
    }
    Ok(())
}

async fn notify_failures(state: &AppState, items: Vec<Value>, reason: &str) {
    if items.is_empty() || reason == "任务已取消" {
        return;
    }
    // Save the intent before completing the task; the worker handles delivery.
    // Awaiting only the database keeps the original operator and task scope.
    for item in items {
        let native = json!({"Id":item["item_id"],"Name":item["name"],"Type":item["item_type"]});
        if let Err(error) = crate::notifications::enqueue_failure(state, &native, reason).await {
            tracing::warn!("整理失败通知未能入队：{}", error.1);
        }
    }
}

fn outcome(result: &Result<Value>) -> (&'static str, String) {
    let (status, mut message) = match result {
        Err(error) => ("failed", safe_text(&error.1)),
        Ok(report)
            if report["success"] == false
                || report["error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty())
                || report["failed"].as_u64().unwrap_or(0) > 0
                || report["failures"]
                    .as_array()
                    .is_some_and(|errors| !errors.is_empty()) =>
        {
            ("failed", safe_text(&string(report, "message")))
        }
        Ok(report)
            if report["warnings"].as_u64().unwrap_or(0) > 0
                || report["warning_details"]
                    .as_array()
                    .is_some_and(|warnings| !warnings.is_empty()) =>
        {
            ("warning", safe_text(&string(report, "message")))
        }
        Ok(report) => ("success", safe_text(&string(report, "message"))),
    };
    if let Ok(report) = result
        && let Some(error) = report["error"].as_str().filter(|error| !error.is_empty())
    {
        message = safe_text(error);
    }
    if status == "failed"
        && let Ok(report) = result
        && string(report, "error").is_empty()
    {
        let reasons = list(&report["failures"])
            .iter()
            .filter_map(|failure| {
                failure.as_str().map(str::to_string).or_else(|| {
                    let reason = ["reason", "error", "message"]
                        .iter()
                        .find_map(|key| failure[*key].as_str().filter(|value| !value.is_empty()))?;
                    let id = string(failure, "item_id");
                    Some(if id.is_empty() {
                        reason.into()
                    } else {
                        format!("{id}：{reason}")
                    })
                })
            })
            .collect::<Vec<_>>();
        if !reasons.is_empty() {
            message = safe_text(&reasons.join("；"));
        }
    }
    if message.is_empty() {
        message = match status {
            "failed" => "ETK 入库处理失败",
            "warning" => "ETK 入库处理完成，有提醒",
            _ => "ETK 入库处理完成",
        }
        .into();
    }
    (status, message)
}

pub async fn selected_history(
    state: &AppState,
    body: &Value,
    item_ids: &[String],
) -> Result<Vec<String>> {
    let Some(selected) = body.get("history_ids") else {
        return Ok(vec![]);
    };
    let selected = selected
        .as_array()
        .filter(|ids| !ids.is_empty() && ids.len() <= 1000)
        .ok_or_else(|| ApiError::bad("请选择 1 至 1000 条真实入库记录"))?;
    let mut ids = vec![];
    for id in selected {
        let id = id
            .as_str()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or_else(|| ApiError::bad("入库记录 ID 无效"))?;
        if !ids.iter().any(|old| old == id) {
            ids.push(id.to_string());
        }
    }
    let linked:Vec<String> = sqlx::query_scalar("SELECT h.item_id FROM media_organize_history h WHERE h.id=ANY($1) AND h.item_id=ANY($2) AND (h.status<>'processing' OR EXISTS (SELECT 1 FROM media_organize_queue q WHERE q.item_id=h.item_id AND q.mode=$3 AND q.status IN ('queued','running') AND q.history_ids ? h.id))")
        .bind(&ids).bind(item_ids).bind(crate::media_organize_queue::mode(body.get("mode"))?).fetch_all(&state.db).await?;
    if linked.len() != ids.len()
        || linked.iter().collect::<BTreeSet<_>>() != item_ids.iter().collect::<BTreeSet<_>>()
    {
        return Err(ApiError::bad(
            "入库记录不存在、仍在处理中或与选择的媒体不一致",
        ));
    }
    Ok(ids)
}

pub async fn begin_reorganize(state: &AppState, ids: &[String], item_id: &str) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("UPDATE media_organize_history SET item_json=(item_json - 'item_warnings' - 'batch_warnings'),status='processing',message='正在重新执行 ETK 整理流程',completed_at=NULL WHERE id=ANY($1) AND item_id=$2")
        .bind(ids).bind(item_id).execute(&state.db).await?;
    Ok(())
}

// Retain the batch outcome separately so one episode's warning is not reported
// as a failure of every other episode in the same queue run.
fn episode_warnings(report: &Value, id: &str) -> Vec<String> {
    let mut details = Vec::new();
    for entry in list(&report["finalizations"]) {
        for track in list(&entry["report"]["title_fill"]["pending"]) {
            if track["item_id"] == id {
                let kind = if track["type"] == "Audio" { "音轨" } else { "字幕" };
                details.push(format!("{kind}标题：{}", safe_text(&string(&track, "reason"))));
            }
        }
    }
    for item in list(&report["automatic_extraction"]["credits"]["items"]) {
        if item["item_id"] == id && item["status"] == "failed" {
            details.push(format!("片尾提取：{}", safe_text(&string(&item, "error"))));
        }
    }
    details
}

fn warning_receipt(report: &Value, id: &str) -> Value {
    json!({"item_warnings":episode_warnings(report,id),
        "batch_warnings":list(&report["warning_details"]).iter().filter_map(Value::as_str)
            .map(safe_text).collect::<Vec<_>>()})
}

pub(crate) async fn migrate_warning_receipts(tx: &mut sqlx::PgConnection) -> Result<()> {
    let added = sqlx::query("INSERT INTO app_settings(setting_key,value_json) VALUES('history_warning_receipts_v1','true') ON CONFLICT DO NOTHING")
        .execute(&mut *tx).await?.rows_affected();
    if added == 0 { return Ok(()); }
    let rows: Vec<(String,String,Value)> = sqlx::query_as(
        "SELECT DISTINCT ON (h.id) h.id,h.item_id,q.result FROM media_organize_history h
         JOIN media_organize_queue q ON q.item_id=h.item_id AND q.history_ids ? h.id
         AND h.completed_at BETWEEN q.started_at AND q.completed_at AND q.result->>'message'=h.message
         WHERE h.status='warning' AND q.status='completed' ORDER BY h.id,q.queue_order DESC")
        .fetch_all(&mut *tx).await?;
    let webhook_rows: Vec<(String,String,Value)> = sqlx::query_as(
        "SELECT DISTINCT ON (h.id) h.id,h.item_id,j.state_json->'result' FROM durable_jobs j
         CROSS JOIN LATERAL jsonb_array_elements_text(j.spec_json#>'{payload,event,history_ids}') AS selected(id)
         JOIN media_organize_history h ON h.id=selected.id
         WHERE h.status='warning' AND h.item_id IS NOT NULL AND j.spec_json->>'kind'='webhook'
         AND j.state_json#>>'{result,message}'=h.message ORDER BY h.id,j.updated_at DESC")
        .fetch_all(&mut *tx).await?;
    for (id,item,report) in rows.into_iter().chain(webhook_rows) {
        sqlx::query("UPDATE media_organize_history SET item_json=item_json || $2 WHERE id=$1")
            .bind(id).bind(warning_receipt(&report,&item)).execute(&mut *tx).await?;
    }
    Ok(())
}

pub async fn finish_reorganize(
    state: &AppState,
    ids: &[String],
    item_id: &str,
    result: &Result<Value>,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let (status, message) = outcome(result);
    let details = match result {
        Ok(report) if status == "warning" => warning_receipt(report,item_id),
        _ => json!({}),
    };
    let completed: Vec<Value> = sqlx::query_scalar("UPDATE media_organize_history SET status=$1,message=$2,completed_at=NOW(),item_json=item_json || $5 WHERE id=ANY($3) AND item_id=$4 AND status='processing' RETURNING item_json || jsonb_build_object('item_id',item_id)")
        .bind(status).bind(&message).bind(ids).bind(item_id).bind(details).fetch_all(&state.db).await?;
    if status == "failed" {
        notify_failures(state, completed, &message).await;
    }
    Ok(())
}

async fn history(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let number = |key: &str, default: i64| -> Result<i64> {
        params.get(key).map_or(Ok(default), |value| {
            value.parse().map_err(|_| ApiError::bad("历史分页参数无效"))
        })
    };
    let page = number("page", 1)?.max(1);
    let size = number("page_size", 50)?.clamp(1, 1000);
    let offset = page
        .checked_sub(1)
        .and_then(|page| page.checked_mul(size))
        .ok_or_else(|| ApiError::bad("历史分页范围无效"))?;
    let status = params.get("status").map_or("all", String::as_str);
    if !matches!(
        status,
        "all" | "admitted" | "success" | "warning" | "failed" | "processing"
    ) {
        return Err(ApiError::bad("入库历史状态筛选无效"));
    }
    let search = params.get("search").map_or("", String::as_str).trim();
    if search.chars().count() > 200 {
        return Err(ApiError::bad("历史搜索关键词过长"));
    }
    let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM media_organize_history WHERE ($1='all' OR status=$1 OR ($1='admitted' AND status IN ('success','warning'))) AND ($2='' OR strpos(lower(COALESCE(item_json->>'name','') || ' ' || COALESCE(item_json->>'series_name','') || ' ' || COALESCE(item_json->>'file_name','')),lower($2))>0)")
        .bind(status).bind(search).fetch_one(&state.db).await?;
    let items:Vec<Value>=sqlx::query_scalar("SELECT (item_json - '_preparation_warning') || jsonb_build_object('id',id,'item_id',item_id,'source',source,'status',status,'message',message,'created_at',created_at,'completed_at',completed_at) FROM media_organize_history WHERE ($1='all' OR status=$1 OR ($1='admitted' AND status IN ('success','warning'))) AND ($2='' OR strpos(lower(COALESCE(item_json->>'name','') || ' ' || COALESCE(item_json->>'series_name','') || ' ' || COALESCE(item_json->>'file_name','')),lower($2))>0) ORDER BY created_at DESC,id DESC LIMIT $3 OFFSET $4")
        .bind(status).bind(search).bind(size).bind(offset).fetch_all(&state.db).await?;
    let tracking_since = crate::db::setting(&state.db, "media_organize_history_started_at")
        .await?
        .unwrap_or(Value::Null);
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size,"tracking_since":tracking_since,"legacy_records_available":false}),
    ))
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    #[test]
    fn actual_failures_replace_a_generic_completion_message() {
        let (status, message) = outcome(&Ok(
            json!({"message":"本批已处理","failures":["电影原生导入失败",{"item_id":"episode-2","reason":"媒体 JSON 验收失败 password=private"}]}),
        ));
        assert_eq!(status, "failed");
        assert!(message.contains("电影原生导入失败"));
        assert!(message.contains("episode-2：媒体 JSON 验收失败"));
        assert!(!message.contains("private"));
        assert!(!message.contains("本批已处理"));
    }

    #[test]
    fn legacy_quality_numbers_never_turn_success_into_failure() {
        let result = Ok(json!({"updated":1,"score":0,"message":"入库已验收"}));
        assert_eq!(outcome(&result), ("success", "入库已验收".into()));
    }

    #[test]
    fn optional_enhancement_warnings_keep_their_warning_outcome() {
        let result = Ok(
            json!({"updated":1,"warning_details":["未启用可选字幕翻译"],"message":"媒体入库完成，有提醒"}),
        );
        assert_eq!(outcome(&result), ("warning", "媒体入库完成，有提醒".into()));
    }
}
