//! Persistent intent for the local organize queue; execution stays in Jobs.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use futures_util::FutureExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::{BTreeSet, HashMap}, time::Duration};
use tokio_util::sync::CancellationToken;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/media_organize/queue", get(queue))
        .route("/api/media_organize/queue/{id}", delete(cancel))
        .route("/api/media_organize/queue/{id}/retry", post(retry))
}

pub(crate) fn mode(value: Option<&Value>) -> Result<&'static str> {
    match value {
        None => Ok("reprocess"),
        Some(Value::String(value)) if value == "reprocess" => Ok("reprocess"),
        Some(Value::String(value)) if value == "rescrape" => Ok("rescrape"),
        _ => Err(ApiError::bad("整理模式仅支持重新整理入库或重新刮削")),
    }
}

pub(crate) fn label(mode: &str) -> &'static str {
    if mode == "rescrape" {
        "重新刮削"
    } else {
        "重新整理入库"
    }
}

pub async fn recover_unfinished(pool: &PgPool) -> Result<()> {
    // An older interrupted attempt may already have a queued/completed retry.
    // Recover only its latest intent, preserving the active uniqueness rule.
    sqlx::query("UPDATE media_organize_queue q SET status='queued',message='上次执行未确认完成，重新核对当前媒体后继续',completed_at=NULL WHERE status='running' OR (status='interrupted' AND NOT EXISTS(SELECT 1 FROM media_organize_queue other WHERE other.item_id=q.item_id AND other.mode=q.mode AND other.id<>q.id AND (other.status IN ('queued','running') OR other.queue_order>q.queue_order)))")
        .execute(pool).await?;
    Ok(())
}

pub(crate) async fn enqueue(
    state: &AppState,
    ids: &[String],
    history_ids: &[String],
    mode: &str,
    operator: &str,
) -> Result<Value> {
    if state.jobs.is_shutting_down().await {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "服务正在关闭，无法加入整理队列".into(),
        ));
    }
    let mut transaction = state.db.begin().await?;
    let mut items = vec![];
    let mut duplicate_count = 0;
    let batch_id = uuid::Uuid::new_v4().to_string();
    let mut lock_ids = ids.iter().collect::<Vec<_>>();
    lock_ids.sort();
    for id in lock_ids {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(format!("emby-toolkit:organize-queue:{id}:{mode}"))
            .execute(&mut *transaction)
            .await?;
    }
    // Keep cancel's queue -> history lock order, including interrupted entries
    // that the worker can make active again. Reuse only these locked rows below.
    let existing_queues: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE item_id=ANY($1) AND mode=$2 AND status IN ('queued','running','interrupted') ORDER BY id FOR UPDATE")
        .bind(ids).bind(mode).fetch_all(&mut *transaction).await?;
    if !history_ids.is_empty() {
        let expected = history_ids.iter().collect::<BTreeSet<_>>();
        let linked: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'item_id',item_id,'status',status) FROM media_organize_history WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(history_ids).fetch_all(&mut *transaction).await?;
        if linked.len() != expected.len()
            || linked.iter().map(|row| string(row, "item_id")).collect::<BTreeSet<_>>() != ids.iter().cloned().collect::<BTreeSet<_>>()
            || linked.iter().any(|row| row["status"] == "processing" && !existing_queues.iter().any(|queue| {
                matches!(queue["status"].as_str(), Some("queued" | "running"))
                    && queue["item_id"] == row["item_id"]
                    && queue["history_ids"].as_array().is_some_and(|ids| ids.contains(&row["id"]))
            }))
        {
            return Err(ApiError(StatusCode::CONFLICT, "整理记录已删除、仍在处理中或媒体绑定变化，未加入整理队列".into()));
        }
    }
    for id in ids {
        if let Some(existing) = existing_queues.iter().find(|queue| queue["item_id"] == id.as_str() && matches!(queue["status"].as_str(), Some("queued" | "running"))) {
            duplicate_count += 1;
            items.push(existing.clone());
            continue;
        }
        let selected: Vec<String> = sqlx::query_scalar("SELECT id FROM media_organize_history WHERE id=ANY($1) AND item_id=$2 ORDER BY created_at,id")
            .bind(history_ids).bind(id).fetch_all(&mut *transaction).await?;
        let snapshot: Option<Value> = sqlx::query_scalar("SELECT item_json FROM media_organize_history WHERE item_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(id).fetch_optional(&mut *transaction).await?;
        let item = snapshot.unwrap_or_else(|| json!({"item_id":id,"name":id}));
        let queued: Value = sqlx::query_scalar("INSERT INTO media_organize_queue(id,item_id,mode,item_json,history_ids,operator_id,message,batch_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING to_jsonb(media_organize_queue)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(id).bind(mode).bind(item)
            .bind(json!(selected)).bind(operator).bind("已加入队列，等待处理").bind(&batch_id)
            .fetch_one(&mut *transaction).await?;
        items.push(queued);
    }
    transaction.commit().await?;
    Ok(
        json!({"mode":mode,"queued_count":items.len()-duplicate_count,"duplicate_count":duplicate_count,
        "queue_ids":items.iter().map(|item|item["id"].clone()).collect::<Vec<_>>(),"items":items,
        "message":format!("{}已加入整理队列；新加入 {} 项，已有 {} 项等待或正在处理",label(mode),ids.len()-duplicate_count,duplicate_count)}),
    )
}

async fn queue(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let number = |key: &str, default: i64| -> Result<i64> {
        params.get(key).map_or(Ok(default), |value| {
            value.parse().map_err(|_| ApiError::bad("队列分页参数无效"))
        })
    };
    let page = number("page", 1)?.max(1);
    let size = number("page_size", 100)?.clamp(1, 100);
    let offset = (page - 1)
        .checked_mul(size)
        .ok_or_else(|| ApiError::bad("队列分页范围无效"))?;
    let counts: Value = sqlx::query_scalar("SELECT jsonb_build_object('total',COUNT(*),'queued_count',COUNT(*) FILTER (WHERE status='queued'),'running_count',COUNT(*) FILTER (WHERE status='running')) FROM media_organize_queue WHERE status IN ('queued','running','interrupted')")
        .fetch_one(&state.db).await?;
    let mut items: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(q)-'operator_id'-'history_ids' FROM media_organize_queue q WHERE status IN ('queued','running','interrupted') ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END,CASE WHEN status IN ('queued','running') THEN queue_order END ASC,queue_order DESC LIMIT $1 OFFSET $2")
        .bind(size).bind(offset).fetch_all(&state.db).await?;
    let busy = state.jobs.status().await["is_running"] == true;
    for item in &mut items {
        item["progress_known"] = json!(false);
        if item["status"] == "running" {
            if let Some(progress) = state.jobs.result(&string(item, "task_id")).await {
                if let Some(job_state) = progress.get("state") {
                    item["job_state"] = job_state.clone();
                }
                for key in [
                    "progress",
                    "progress_known",
                    "current_action",
                    "last_activity",
                    "phase",
                    "stop_requested",
                    "pause_requested",
                ] {
                    if let Some(value) = progress.get(key) {
                        item[key] = value.clone();
                    }
                }
                if let Some(message) = progress.get("message") {
                    item["message"] = message.clone();
                }
            }
        } else if item["status"] == "queued" && busy {
            item["message"] = json!("等待当前后台任务结束");
        }
    }
    Ok(Json(
        json!({"items":items,"total":counts["total"],"page":page,"page_size":size,
        "queued_count":counts["queued_count"],"running_count":counts["running_count"]}),
    ))
}

async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?;
    let mut transaction = state.db.begin().await?;
    let started_batch: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_organize_queue q WHERE q.id=$1 AND q.status='queued' AND q.task_id IS NOT NULL AND EXISTS(SELECT 1 FROM media_organize_queue member WHERE member.task_id=q.task_id AND member.id<>q.id))")
        .bind(&id).fetch_one(&mut *transaction).await?;
    if started_batch {
        return Err(ApiError(StatusCode::CONFLICT, "该分集所属批次已开始执行，请通过任务中心停止整个批次".into()));
    }
    let cancelled: Option<Value> = sqlx::query_scalar("UPDATE media_organize_queue SET status='cancelled',message='已移出队列，未执行媒体操作',completed_at=NOW() WHERE id=$1 AND status='queued' RETURNING to_jsonb(media_organize_queue)")
        .bind(&id).fetch_optional(&mut *transaction).await?;
    if let Some(mut item) = cancelled {
        let history = list(&item["history_ids"])
            .into_iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        // A resumed waiting item may still own processing history from its
        // previous process. Cancel both intents together without failure alerts.
        sqlx::query("UPDATE media_organize_history SET status='failed',message='任务已取消',completed_at=NOW() WHERE id=ANY($1) AND item_id=$2 AND status='processing'")
            .bind(&history).bind(string(&item,"item_id")).execute(&mut *transaction).await?;
        transaction.commit().await?;
        for key in ["operator_id", "history_ids"] {
            item.as_object_mut().unwrap().remove(key);
        }
        return Ok(Json(
            json!({"message":"已取消等待中的整理任务","item":item}),
        ));
    }
    transaction.commit().await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM media_organize_queue WHERE id=$1")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    Err(ApiError(
        if status.is_some() {
            StatusCode::CONFLICT
        } else {
            StatusCode::NOT_FOUND
        },
        if status.as_deref() == Some("running") {
            "任务正在执行，不能作为等待任务移出队列"
        } else {
            "任务不存在或已不在等待队列中"
        }
        .into(),
    ))
}

async fn retry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Value>)> {
    let operator = state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?
        .user_id;
    let row: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let row = row.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "未找到整理任务".into()))?;
    if !matches!(
        row["status"].as_str(),
        Some("failed" | "interrupted" | "cancelled")
    ) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "仅失败、中断或已取消的任务可以重新加入队列".into(),
        ));
    }
    let ids = vec![string(&row, "item_id")];
    let history = list(&row["history_ids"])
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let result = enqueue(&state, &ids, &history, mode(row.get("mode"))?, &operator).await?;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

fn failed(report: &Value) -> bool {
    report["success"] == false
        || report["failed"].as_u64().unwrap_or(0) > 0
        || !list(&report["failures"]).is_empty()
        || report["error"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
}

// 只合并同次提交、同剧的重新入库；队列仍保留每集记录和取消入口。
async fn claim_batch(state: &AppState, queued: &Value, context: &JobContext) -> Result<Vec<Value>> {
    let mut transaction = state.db.begin().await?;
    let entries: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1 OR (batch_id=$2 AND mode='reprocess' AND $3='reprocess' AND item_json->>'item_type'='Episode' AND item_json->>'series_id'=$4 AND $4<>'' AND status='queued' AND (task_id IS NULL OR task_id=$5)) ORDER BY id FOR UPDATE")
        .bind(string(queued,"id")).bind(queued["batch_id"].as_str()).bind(string(queued,"mode"))
        .bind(string(&queued["item_json"],"series_id")).bind(&context.id).fetch_all(&mut *transaction).await?;
    if !entries.iter().any(|entry| {
        entry["id"] == queued["id"]
            && entry["status"] == "queued"
            && entry["task_id"] == queued["task_id"]
    }) {
        return Ok(vec![]);
    }
    let ids = entries
        .iter()
        .map(|entry| string(entry, "id"))
        .collect::<Vec<_>>();
    sqlx::query("UPDATE media_organize_queue SET status='running',task_id=$2,started_at=COALESCE(started_at,NOW()),message='正在执行' WHERE id=ANY($1)")
        .bind(&ids).bind(&context.id).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(entries)
}

async fn execute(state: &AppState, queued: &Value, context: &JobContext) -> Result<Value> {
    let entries = claim_batch(state, queued, context).await?;
    let Some(entry) = entries.iter().find(|entry| entry["id"] == queued["id"]) else {
        context.request_stop();
        return Ok(json!({"cancelled":true,"message":"等待任务已取消，未执行媒体操作"}));
    };
    let ids = entries
        .iter()
        .map(|entry| string(entry, "item_id"))
        .collect::<Vec<_>>();
    let history = entries
        .iter()
        .flat_map(|entry| list(&entry["history_ids"]))
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let count = ids.len() as u64;
    let operation = label(&string(&entry, "mode"));
    context
        .update_counted(
            0,
            count,
            operation,
            &format!("{operation}：本批 {count} 项媒体"),
        )
        .await;
    let result = std::panic::AssertUnwindSafe(crate::notifications::with_operator(
        string(&entry, "operator_id"),
        async {
            for id in &ids {
                crate::media_organize_history::begin_reorganize(state, &history, id).await?;
            }
            for entry in &entries {
                let id = string(entry, "item_id");
                context.checkpoint().await?;
                let current = details(state, &id, "Id,Type,Name,Path,ProviderIds,SeriesId").await?;
                if current["Id"] != id
                    || !(matches!(current["Type"].as_str(), Some("Movie" | "Episode"))
                        || entry["mode"] == "rescrape" && matches!(current["Type"].as_str(), Some("Series" | "Season")))
                {
                    return Err(ApiError::bad(
                        "整理目标的原生媒体身份已变化，请重新选择媒体",
                    ));
                }
                let expected = &entry["item_json"];
                if expected["item_type"]
                    .as_str()
                    .is_some_and(|kind| current["Type"] != kind)
                    || (current["Type"] == "Movie"
                        && expected["media_source"] == "tmdb"
                        && expected["media_id"]
                            .as_str()
                            .is_some_and(|tmdb| current["ProviderIds"]["Tmdb"] != tmdb))
                    || (current["Type"] == "Episode"
                        && expected["series_id"]
                            .as_str()
                            .is_some_and(|series| current["SeriesId"] != series))
                {
                    return Err(ApiError::bad(
                        "排队期间媒体的作品绑定已变化，未处理其他作品",
                    ));
                }
            }
            if ids.len() > 1 {
                crate::actions::reorganize_ingest(state, &ids, context).await
            } else {
                crate::actions::run_organize_item(state, &ids[0], &string(entry, "mode"), context)
                    .await
            }
        },
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "整理任务执行异常".into(),
        ))
    });
    let interrupted = context.is_interrupted() && result.is_err();
    let history_result = if interrupted {
        Ok(())
    } else {
        crate::notifications::with_operator(string(&entry, "operator_id"), async {
            for id in &ids {
                crate::media_organize_history::finish_reorganize(state, &history, id, &result)
                    .await?;
            }
            Ok::<_, ApiError>(())
        })
        .await
    };
    let mut report = match &result {
        Ok(report) => report.clone(),
        Err(error) => {
            json!({"error":crate::logs::redact(&error.1),"failures":[crate::logs::redact(&error.1)]})
        }
    };
    if let Err(error) = history_result {
        report["warnings"] = json!(report["warnings"].as_u64().unwrap_or(0) + 1);
        let mut warnings = list(&report["warning_details"]);
        warnings.push(json!(format!(
            "入库记录保存失败：{}",
            crate::logs::redact(&error.1)
        )));
        report["warning_details"] = json!(warnings);
    }
    let status = if interrupted {
        "queued"
    } else if context.is_cancelled() && !context.is_interrupted() {
        "cancelled"
    } else if result.is_err() || failed(&report) {
        "failed"
    } else {
        "completed"
    };
    let message = if status == "queued" {
        "服务正在关闭，当前媒体将在重启后重新核对并继续"
    } else {
        report["message"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                if status == "completed" {
                    "处理已完成"
                } else if status == "cancelled" {
                    "处理已安全结束，已保存取消前结果"
                } else {
                    report["error"]
                        .as_str()
                        .unwrap_or("处理失败，请查看任务结果")
                }
            })
    };
    sqlx::query("UPDATE media_organize_queue SET status=$2,result=$3,message=$4,completed_at=CASE WHEN $2='queued' THEN NULL ELSE NOW() END WHERE id=ANY($1) AND status='running' AND task_id=$5")
        .bind(entries.iter().map(|entry| string(entry,"id")).collect::<Vec<_>>()).bind(status).bind(&report).bind(message).bind(&context.id).execute(&state.db).await?;
    if status != "queued" {
        context
            .update_counted(count, count, operation, message)
            .await;
    }
    if result.is_err() {
        return result;
    }
    Ok(report)
}

pub(crate) async fn resume_owned(
    state: &AppState,
    payload: &Value,
    context: &JobContext,
) -> Result<Value> {
    let entry: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1 AND task_id=$2",
    )
    .bind(string(payload, "queue_id"))
    .bind(&context.id)
    .fetch_optional(&state.db)
    .await?;
    let entry =
        entry.ok_or_else(|| ApiError::bad("原整理队列与持久任务身份不一致，未重新处理媒体"))?;
    match entry["status"].as_str() {
        Some("queued") => Box::pin(execute(state, &entry, context)).await,
        Some("completed" | "failed" | "cancelled") => {
            if entry["status"] == "cancelled" {
                context.request_stop();
            }
            if entry["result"].is_object() {
                Ok(entry["result"].clone())
            } else if entry["status"] == "failed" {
                Ok(json!({"error":entry["message"]}))
            } else {
                Ok(json!({"message":entry["message"],"cancelled":entry["status"]=="cancelled"}))
            }
        }
        _ => Err(ApiError(
            StatusCode::CONFLICT,
            "原整理任务已不在可恢复的等待状态，未重复执行".into(),
        )),
    }
}

pub async fn run(state: AppState, cancel: CancellationToken) {
    loop {
        let available = tokio::select! {
            _=cancel.cancelled()=>return,
            result=state.jobs.wait_idle()=>result,
        };
        if available.is_err() {
            return;
        }
        if let Err(error) = recover_unfinished(&state.db).await {
            tracing::error!("整理队列恢复异常结束项失败，保留原状态：{}", error.1);
        }
        let next: Result<Option<Value>> = async {
            Ok(sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE status='queued' ORDER BY queue_order LIMIT 1")
                .fetch_optional(&state.db).await?)
        }.await;
        if let Ok(Some(entry)) = next {
            let work = state.clone();
            let name = format!("媒体{}", label(&string(&entry, "mode")));
            let owner = json!({"kind":"media_organize_queue","queue_id":entry["id"]});
            let restored_id = entry["task_id"].as_str().map(str::to_string);
            match state
                .jobs
                .submit_owned(
                    &name,
                    owner,
                    restored_id.as_deref(),
                    move |context| async move { Box::pin(execute(&work, &entry, &context)).await },
                )
                .await
            {
                Ok(_) => continue,
                Err(error) if error.0 == StatusCode::SERVICE_UNAVAILABLE => return,
                Err(error) if error.0 == StatusCode::CONFLICT => {}
                Err(error) => tracing::error!("整理队列启动任务失败：{}", error.1),
            }
        } else if let Err(error) = next {
            tracing::error!("读取整理队列失败，保留待处理任务：{}", error.1);
        }
        tokio::select! {_=cancel.cancelled()=>return,_=tokio::time::sleep(Duration::from_millis(500))=>{}}
    }
}
