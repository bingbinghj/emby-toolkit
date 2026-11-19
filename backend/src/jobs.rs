use crate::error::{ApiError, Result};
use axum::http::StatusCode;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

type Activity = Arc<std::sync::Mutex<Option<String>>>;
type ProgressLog = Option<(tokio::time::Instant, Option<u64>, String)>;
type IngestDirectories = Arc<Mutex<(u64, [String; 3], BTreeMap<String, Vec<Value>>)>>;
tokio::task_local! {
    static TASK_ACTIVITY: Activity;
    static TASK_CONTEXT: JobContext;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub kind: String,
    pub payload: Value,
    #[serde(default)]
    pub operator_id: Option<String>,
    #[serde(default)]
    pub owned: bool,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub server_url: Option<String>,
}
impl JobSpec {
    pub fn new(kind: &str, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            operator_id: crate::notifications::current_operator(),
            owned: false,
            server_id: None,
            server_url: None,
        }
    }
    pub fn operator(mut self, operator_id: String) -> Self {
        self.operator_id = Some(operator_id);
        self
    }
}

pub(crate) fn server_url(config: &Value) -> Option<String> {
    let raw = config["emby_server_url"]
        .as_str()
        .filter(|value| !value.trim().is_empty())?;
    let mut url = url::Url::parse(raw.trim()).ok()?;
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Some(url.to_string().trim_end_matches('/').into())
}

async fn cancel_owned_queue(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: &str,
    message: &str,
) -> Result<()> {
    // Terminal jobs load a trimmed payload; use the original persisted owner.
    sqlx::query("WITH cancelled AS (UPDATE media_organize_queue q SET status='cancelled',message=$2,completed_at=NOW() FROM durable_jobs j WHERE j.id=$1 AND j.spec_json->>'kind'='media_organize_queue' AND j.spec_json->>'owned'='true' AND q.task_id=j.id AND q.status IN ('queued','running','interrupted') RETURNING q.item_id,q.history_ids)
        UPDATE media_organize_history h SET status='failed',message=$2,completed_at=NOW() FROM cancelled q WHERE h.status='processing' AND h.item_id=q.item_id AND q.history_ids ? h.id")
        .bind(job_id).bind(message).execute(&mut **transaction).await?;
    Ok(())
}

#[derive(Clone)]
struct PersistedJob {
    id: String,
    order: i64,
    spec: JobSpec,
    state: Value,
    checkpoint: Value,
}

pub(crate) fn record_activity(message: &str) {
    let _ = TASK_ACTIVITY.try_with(|activity| {
        *activity.lock().unwrap_or_else(|error| error.into_inner()) =
            Some(crate::logs::redact(message).chars().take(500).collect());
    });
}

pub(crate) fn current_context() -> Option<JobContext> {
    TASK_CONTEXT.try_with(Clone::clone).ok()
}

#[derive(Clone)]
pub struct JobContext {
    pub id: String,
    cancel: CancellationToken,
    wake: Arc<Notify>,
    progress: Arc<Mutex<Value>>,
    activity: Activity,
    last_progress_log: Arc<std::sync::Mutex<ProgressLog>>,
    pause_generation: Arc<AtomicU64>,
    pub(crate) ingest_directories: IngestDirectories,
    persistence: Option<PgPool>,
    checkpoint_state: Arc<Mutex<Value>>,
    interrupted: Arc<AtomicBool>,
    resuming: bool,
    checkpoint_prefix: String,
    pause_intent: Option<bool>,
    cleanup_state: Option<String>,
    server_id: Option<String>,
    server_url: Option<String>,
}
impl JobContext {
    pub fn is_resuming(&self) -> bool {
        self.resuming
    }
    pub fn is_persistent(&self) -> bool {
        self.persistence.is_some()
    }
    pub fn is_terminal_cleanup(&self) -> bool {
        self.cleanup_state.is_some()
    }
    pub async fn ensure_server(&self, state: &crate::AppState) -> Result<()> {
        if let Some(expected) = &self.server_id {
            let live = state
                .emby_once(reqwest::Method::GET, "System/Info/Public", &[], None)
                .await?;
            let id = live["Id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    ApiError(
                        StatusCode::BAD_GATEWAY,
                        "Emby 未返回服务器身份，保留进度等待恢复".into(),
                    )
                })?;
            if id != expected {
                return Err(ApiError::bad(
                    "Emby 服务器身份已变化，已停止原任务并保留恢复材料",
                ));
            }
        } else if let Some(expected) = &self.server_url {
            if server_url(&*state.config.read().await).as_ref() != Some(expected) {
                return Err(ApiError::bad(
                    "Emby 服务器地址已变化且原身份未确认，已保留任务进度",
                ));
            }
        }
        Ok(())
    }
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }
    pub fn scoped(&self, prefix: &str) -> Self {
        let mut context = self.clone();
        context.checkpoint_prefix = format!("{}{prefix}:", self.checkpoint_prefix);
        context
    }
    pub(crate) async fn refresh_ingest_directories(&self) {
        self.ingest_directories.lock().await.2.clear();
    }
    pub async fn for_resume_cleanup(&self) -> Self {
        let mut context = self.clone();
        let mut progress = self.progress.lock().await.clone();
        context.pause_intent = Some(progress["pause_requested"] == true);
        context.cleanup_state = progress["state"]
            .as_str()
            .filter(|state| {
                matches!(
                    *state,
                    "completed" | "completed_with_warnings" | "failed" | "cancelled"
                )
            })
            .map(str::to_string);
        progress["pause_requested"] = json!(false);
        progress["stop_requested"] = json!(false);
        context.progress = Arc::new(Mutex::new(progress));
        context.cancel = CancellationToken::new();
        context.interrupted = Arc::new(AtomicBool::new(true));
        context
    }
    async fn stored(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
        let key = format!("{}{key}", self.checkpoint_prefix);
        if let Some(pool) = &self.persistence {
            let step_key = format!("{namespace}:{key}");
            let value: Option<Value> = sqlx::query_scalar("SELECT COALESCE((SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key=$2), (SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key=$3 AND $4='completed'), (SELECT checkpoint_json #> ARRAY[$4,$3] FROM durable_jobs WHERE id=$1))")
                .bind(&self.id).bind(step_key).bind(&key).bind(namespace).fetch_one(pool).await?;
            Ok(value)
        } else {
            Ok(self.checkpoint_state.lock().await[namespace]
                .get(&key)
                .cloned())
        }
    }
    pub async fn plan(&self, key: &str) -> Result<Option<Value>> {
        self.stored("plans", key).await
    }
    pub async fn saved_plans(
        &self,
        matches: impl Fn(&str) -> bool,
    ) -> Result<BTreeMap<String, Value>> {
        if let Some(pool) = &self.persistence {
            // Recovery callers need a few small plans. Fetch their keys first so
            // full-library inventories and image bytes stay in the database.
            let keys: Vec<String> = sqlx::query_scalar("SELECT substring(step_key from 7) FROM durable_job_steps WHERE job_id=$1 AND step_key LIKE 'plans:%' UNION SELECT jsonb_object_keys(CASE WHEN jsonb_typeof(checkpoint_json->'plans')='object' THEN checkpoint_json->'plans' ELSE '{}'::jsonb END) FROM durable_jobs WHERE id=$1")
                .bind(&self.id).fetch_all(pool).await?;
            let keys: Vec<_> = keys.into_iter().filter(|key| matches(key)).collect();
            if keys.is_empty() {
                return Ok(BTreeMap::new());
            }
            let legacy: Vec<(String, Value)> = sqlx::query_as("SELECT p.key,p.value FROM durable_jobs j, LATERAL jsonb_each(CASE WHEN jsonb_typeof(j.checkpoint_json->'plans')='object' THEN j.checkpoint_json->'plans' ELSE '{}'::jsonb END) p WHERE j.id=$1 AND p.key=ANY($2)")
                .bind(&self.id).bind(&keys).fetch_all(pool).await?;
            let mut plans: BTreeMap<_, _> = legacy.into_iter().collect();
            let step_keys: Vec<_> = keys.iter().map(|key| format!("plans:{key}")).collect();
            let rows: Vec<(String, Value)> = sqlx::query_as("SELECT step_key,value_json FROM durable_job_steps WHERE job_id=$1 AND step_key=ANY($2)")
                .bind(&self.id).bind(step_keys).fetch_all(pool).await?;
            for (key, value) in rows {
                plans.insert(key[6..].into(), value);
            }
            Ok(plans)
        } else {
            Ok(self.checkpoint_state.lock().await["plans"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| matches(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect())
        }
    }
    pub async fn completed(&self, key: &str) -> Result<Option<Value>> {
        self.stored("completed", key).await
    }
    async fn store(&self, namespace: &str, key: &str, value: &Value) -> Result<()> {
        let key = format!("{}{key}", self.checkpoint_prefix);
        if let Some(pool) = &self.persistence {
            let step_key = format!("{namespace}:{key}");
            let changed = sqlx::query("INSERT INTO durable_job_steps(job_id,step_key,value_json) VALUES($1,$2,$3) ON CONFLICT(job_id,step_key) DO UPDATE SET value_json=EXCLUDED.value_json")
                .bind(&self.id).bind(&step_key).bind(value).execute(pool).await?.rows_affected();
            if changed != 1 {
                return Err(ApiError::bad("任务持久记录不存在，已停止当前操作"));
            }
        } else {
            self.checkpoint_state.lock().await[namespace][&key] = value.clone();
        }
        Ok(())
    }
    pub async fn save_plan(&self, key: &str, value: &Value) -> Result<()> {
        self.store("plans", key, value).await
    }
    pub async fn complete(&self, key: &str, value: &Value) -> Result<()> {
        self.store("completed", key, value).await
    }
    pub async fn complete_in_tx(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        key: &str,
        value: &Value,
    ) -> Result<()> {
        if self.persistence.is_some() {
            let key = format!("completed:{}{key}", self.checkpoint_prefix);
            let changed = sqlx::query("INSERT INTO durable_job_steps(job_id,step_key,value_json) VALUES($1,$2,$3) ON CONFLICT(job_id,step_key) DO UPDATE SET value_json=EXCLUDED.value_json")
                .bind(&self.id).bind(&key).bind(value).execute(&mut **transaction).await?.rows_affected();
            if changed != 1 {
                return Err(ApiError::bad("任务持久记录不存在，已回滚当前操作"));
            }
        }
        Ok(())
    }
    async fn persist_state(&self, state: &Value) -> Result<()> {
        if let Some(pool) = &self.persistence {
            let mut state = state.clone();
            if let Some(paused) = self.pause_intent {
                state["pause_requested"] = json!(paused);
                state["state"] = json!(if paused { "paused" } else { "interrupted" });
                state["is_running"] = json!(false);
                if let Some(original) = &self.cleanup_state {
                    state["state"] = json!(original);
                }
            }
            sqlx::query("UPDATE durable_jobs SET state_json=$2,updated_at=NOW() WHERE id=$1")
                .bind(&self.id)
                .bind(&state)
                .execute(pool)
                .await?;
        }
        Ok(())
    }
    async fn persist_progress(&self, state: &Value) {
        if let Err(error) = self.persist_state(state).await {
            tracing::warn!(task_id=%self.id,"保存任务状态失败：{}",crate::logs::redact(&error.1));
        }
    }
    pub(crate) async fn for_recovery(context: Option<&Self>) -> Self {
        let mut progress = if let Some(context) = context {
            context.progress.lock().await.clone()
        } else {
            json!({"last_action":"恢复媒体信息"})
        };
        progress["pause_requested"] = json!(false);
        Self {
            id: context
                .map(|context| context.id.clone())
                .unwrap_or_default(),
            cancel: CancellationToken::new(),
            wake: Arc::new(Notify::new()),
            progress: Arc::new(Mutex::new(progress)),
            activity: context
                .map(|context| context.activity.clone())
                .unwrap_or_default(),
            last_progress_log: Arc::new(std::sync::Mutex::new(None)),
            pause_generation: Arc::new(AtomicU64::new(0)),
            ingest_directories: Arc::default(),
            persistence: None,
            checkpoint_state: Arc::new(Mutex::new(json!({"plans":{},"completed":{}}))),
            interrupted: Arc::new(AtomicBool::new(false)),
            resuming: false,
            checkpoint_prefix: String::new(),
            pause_intent: None,
            cleanup_state: None,
            server_id: context.and_then(|context| context.server_id.clone()),
            server_url: context.and_then(|context| context.server_url.clone()),
        }
    }
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }
    pub async fn sleep(&self, duration: std::time::Duration) -> Result<()> {
        self.checkpoint().await?;
        tokio::select! {
            _ = self.cancel.cancelled() => self.checkpoint().await,
            _ = tokio::time::sleep(duration) => self.checkpoint().await,
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    pub(crate) fn request_stop(&self) {
        self.cancel.cancel();
        self.wake.notify_waiters();
    }
    pub(crate) fn pause_generation(&self) -> u64 {
        self.pause_generation.load(Ordering::Acquire)
    }
    pub(crate) async fn is_pause_requested(&self) -> bool {
        self.progress.lock().await["pause_requested"] == true
    }
    pub(crate) async fn wait_for_pause(&self) {
        loop {
            let wake = self.wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            if self.is_pause_requested().await {
                return;
            }
            wake.await;
        }
    }
    pub async fn checkpoint(&self) -> Result<()> {
        loop {
            let wake = self.wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            let mut state = self.progress.lock().await;
            if self.cancel.is_cancelled() {
                return Err(ApiError(StatusCode::CONFLICT, "任务已取消".into()));
            }
            if state["pause_requested"] != true {
                return Ok(());
            }
            if state["state"] == "pausing" {
                state["state"] = json!("paused");
                self.pause_generation.fetch_add(1, Ordering::AcqRel);
                let name = state["last_action"].as_str().unwrap_or("后台任务");
                tracing::info!(task_id=%self.id,"[任务暂停] {name} · 已到达安全暂停点，保留当前进度");
                self.persist_state(&state).await?;
            }
            drop(state);
            tokio::select! { _ = self.cancel.cancelled()=>{}, _ = wake=>{} }
        }
    }
    pub async fn update(&self, progress: u64, phase: &str, message: &str) {
        self.update_status(Some(progress), None, phase, message)
            .await;
    }
    pub async fn update_counted(&self, completed: u64, total: u64, phase: &str, message: &str) {
        self.update_status(
            Some(
                completed
                    .saturating_mul(100)
                    .checked_div(total)
                    .unwrap_or(100),
            ),
            Some((completed, total)),
            phase,
            message,
        )
        .await;
    }
    pub async fn activity(&self, phase: &str, message: &str) {
        self.update_status(None, None, phase, message).await;
    }
    async fn update_status(
        &self,
        progress: Option<u64>,
        counts: Option<(u64, u64)>,
        phase: &str,
        message: &str,
    ) {
        let mut state = self.progress.lock().await;
        if let Some(progress) = progress {
            state["progress"] = json!(progress.min(100));
            state["progress_known"] = json!(true);
        }
        if let Some((completed, total)) = counts {
            state["completed"] = json!(completed);
            state["total"] = json!(total);
        }
        state["phase"] = json!(phase);
        state["message"] = json!(message);
        state["updated_at"] = json!(chrono::Utc::now().timestamp());
        self.persist_progress(&state).await;
        *self
            .activity
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        let name = state["last_action"].as_str().unwrap_or("后台任务");
        let count = counts
            .map(|(done, total)| format!(" · {done}/{total}"))
            .unwrap_or_default();
        let percent = progress
            .map(|p| format!(" · {}%", p.min(100)))
            .unwrap_or_default();
        let mut previous = self
            .last_progress_log
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((before, old_progress, old_phase)) = previous.as_ref() {
            let boundary = matches!(progress, Some(0 | 100)) && old_progress != &progress;
            if old_phase == phase
                && !boundary
                && before.elapsed() < std::time::Duration::from_secs(2)
            {
                return;
            }
        }
        *previous = Some((tokio::time::Instant::now(), progress, phase.into()));
        tracing::info!(task_id=%self.id,"[任务进度] {name} · {phase}{count}{percent} · {message}");
    }
}
#[derive(Default)]
struct Inner {
    active: Option<JobContext>,
    shutting_down: bool,
    latest: Value,
    results: HashMap<String, Value>,
    history_order: VecDeque<String>,
    recoverable: VecDeque<PersistedJob>,
    cleanup_records: Vec<PersistedJob>,
    restoring: bool,
    recovery_message: Option<String>,
}
#[derive(Default)]
pub struct JobManager {
    inner: Mutex<Inner>,
    finished: Notify,
    persistence: Option<PgPool>,
    lane: String,
    recovery_ready: AtomicBool,
    ready: Notify,
    config: std::sync::Mutex<Option<std::sync::Weak<tokio::sync::RwLock<Value>>>>,
}
impl JobManager {
    pub async fn open(pool: PgPool) -> Result<Self> {
        Self::open_lane(pool, "main").await
    }
    pub async fn open_lane(pool: PgPool, lane: &str) -> Result<Self> {
        let rows: Vec<(String, i64, Value, Value, Value, Option<Value>)> = sqlx::query_as(
            "SELECT j.id,j.job_order,CASE WHEN j.state_json->>'state' IN ('running','pausing','paused','stopping','interrupted') THEN j.spec_json ELSE (j.spec_json-'payload')||'{\"payload\":{}}'::jsonb END,j.state_json,j.checkpoint_json,s.value_json FROM durable_jobs j LEFT JOIN durable_job_steps s ON s.job_id=j.id AND s.step_key='completed:__job_result__' WHERE j.lane=$1 AND (j.state_json->>'state' IN ('running','pausing','paused','stopping','interrupted') OR j.state_json->>'cleanup_pending'='true' OR j.id IN (SELECT id FROM durable_jobs WHERE lane=$1 ORDER BY job_order DESC LIMIT 30)) ORDER BY j.job_order",
        )
        .bind(lane)
        .fetch_all(&pool)
        .await?;
        let mut inner = Inner::default();
        for (id, order, spec, mut state, checkpoint, proof) in rows {
            let spec: JobSpec =
                serde_json::from_value(spec).map_err(|_| ApiError::bad("持久任务参数格式无效"))?;
            let was_unfinished = matches!(
                state["state"].as_str(),
                Some("running" | "pausing" | "paused" | "stopping" | "interrupted")
            );
            if was_unfinished && let Some(proof) = proof {
                state = proof;
                sqlx::query("UPDATE durable_jobs SET state_json=$2,updated_at=NOW() WHERE id=$1")
                    .bind(&id)
                    .bind(&state)
                    .execute(&pool)
                    .await?;
            }
            if was_unfinished && spec.owned && spec.kind == "media_organize_queue" {
                let settled: Option<(String, Option<Value>, String)> = sqlx::query_as(
                    "SELECT status,result,message FROM media_organize_queue WHERE id=$1 AND task_id=$2 AND status IN ('completed','failed','cancelled')",
                ).bind(spec.payload["queue_id"].as_str().unwrap_or("")).bind(&id).fetch_optional(&pool).await?;
                if let Some((status, result, message)) = settled {
                    state["state"] = json!(if status == "completed"
                        && result.as_ref().is_some_and(|value| value["warnings"]
                            .as_u64()
                            .unwrap_or(0)
                            > 0
                            || value["warning_details"]
                                .as_array()
                                .is_some_and(|details| !details.is_empty()))
                    {
                        "completed_with_warnings"
                    } else {
                        &status
                    });
                    state["is_running"] = json!(false);
                    state["current_action"] = json!("空闲");
                    state["message"] = json!(message);
                    if let Some(result) = result {
                        state["result"] = result;
                    }
                    sqlx::query(
                        "UPDATE durable_jobs SET state_json=$2,updated_at=NOW() WHERE id=$1",
                    )
                    .bind(&id)
                    .bind(&state)
                    .execute(&pool)
                    .await?;
                }
            }
            let already_cancelled = state["state"] == "cancelled";
            if matches!(
                state["state"].as_str(),
                Some("running" | "pausing" | "paused" | "stopping" | "interrupted")
            ) {
                if state["stop_requested"] == true {
                    state["state"] = json!("cancelled");
                    state["is_running"] = json!(false);
                    state["message"] = json!("任务已按上次停止请求取消");
                    let mut transaction = pool.begin().await?;
                    sqlx::query(
                        "UPDATE durable_jobs SET state_json=$2,updated_at=NOW() WHERE id=$1",
                    )
                    .bind(&id)
                    .bind(&state)
                    .execute(&mut *transaction)
                    .await?;
                    if spec.owned && spec.kind == "media_organize_queue" {
                        cancel_owned_queue(&mut transaction, &id, "任务已按上次停止请求取消")
                            .await?;
                    }
                    transaction.commit().await?;
                } else {
                    inner.recoverable.push_back(PersistedJob {
                        id,
                        order,
                        spec,
                        state,
                        checkpoint,
                    });
                    continue;
                }
            }
            if already_cancelled && spec.owned && spec.kind == "media_organize_queue" {
                let mut transaction = pool.begin().await?;
                cancel_owned_queue(
                    &mut transaction,
                    &id,
                    state["message"].as_str().unwrap_or("任务已取消"),
                )
                .await?;
                transaction.commit().await?;
            }
            if was_unfinished
                && spec.owned
                && spec.kind == "media_organize_queue"
                && matches!(
                    state["state"].as_str(),
                    Some("completed" | "completed_with_warnings" | "failed" | "cancelled")
                )
            {
                let status = match state["state"].as_str().unwrap_or("") {
                    "completed_with_warnings" => "completed",
                    other => other,
                };
                sqlx::query("UPDATE media_organize_queue SET status=$3,result=$4,message=$5,completed_at=NOW() WHERE task_id=$2 AND EXISTS(SELECT 1 FROM media_organize_queue owner WHERE owner.id=$1 AND owner.task_id=$2) AND status IN ('queued','running','interrupted')")
                    .bind(spec.payload["queue_id"].as_str().unwrap_or("")).bind(&id).bind(status).bind(&state["result"]).bind(state["message"].as_str().unwrap_or("")).execute(&pool).await?;
            }
            if state["cleanup_pending"] == true {
                inner.cleanup_records.push(PersistedJob {
                    id: id.clone(),
                    order,
                    spec: spec.clone(),
                    state: state.clone(),
                    checkpoint: checkpoint.clone(),
                });
            }
            inner.latest = state.clone();
            inner.results.insert(id.clone(), state);
            inner.history_order.push_back(id);
            if inner.history_order.len() > 30
                && let Some(id) = inner.history_order.pop_front()
            {
                inner.results.remove(&id);
            }
        }
        inner.restoring = !inner.recoverable.is_empty() || !inner.cleanup_records.is_empty();
        if lane == "main" {
            let other_resources: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_jobs WHERE lane<>'main' AND (state_json->>'state' IN ('running','pausing','paused','stopping','interrupted') OR state_json->>'cleanup_pending'='true'))")
                .fetch_one(&pool).await?;
            inner.restoring |= other_resources;
        }
        let recovery_ready = !inner.restoring;
        Ok(Self {
            inner: Mutex::new(inner),
            finished: Notify::new(),
            persistence: Some(pool),
            lane: lane.into(),
            recovery_ready: AtomicBool::new(recovery_ready),
            ready: Notify::new(),
            config: std::sync::Mutex::new(None),
        })
    }
    pub fn bind_config(&self, config: &Arc<tokio::sync::RwLock<Value>>) {
        *self
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(config));
    }
    pub async fn submit_persistent<F, Fut>(
        self: &Arc<Self>,
        name: &str,
        spec: JobSpec,
        function: F,
    ) -> Result<String>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        self.submit_persistent_response(name, spec, move |context| async move {
            Ok((200, function(context).await?))
        })
        .await
    }
    pub async fn submit_persistent_response<F, Fut>(
        self: &Arc<Self>,
        name: &str,
        spec: JobSpec,
        function: F,
    ) -> Result<String>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(u16, Value)>> + Send + 'static,
    {
        self.start(name, Some(spec), None, function).await
    }
    pub async fn submit_owned<F, Fut>(
        self: &Arc<Self>,
        name: &str,
        owner: Value,
        restored_id: Option<&str>,
        function: F,
    ) -> Result<String>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let mut spec = JobSpec::new(owner["kind"].as_str().unwrap_or(""), owner.clone());
        spec.owned = true;
        let restored = if let Some(id) = restored_id {
            let inner = self.inner.lock().await;
            inner
                .recoverable
                .iter()
                .find(|job| job.id == id && job.spec.owned)
                .cloned()
        } else {
            None
        };
        self.start(name, Some(spec), restored, move |context| async move {
            Ok((200, function(context).await?))
        })
        .await
    }
    pub async fn resume_pending(
        self: &Arc<Self>,
        state: crate::AppState,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut pending: Vec<_> = {
            let inner = self.inner.lock().await;
            inner
                .recoverable
                .iter()
                .chain(inner.cleanup_records.iter())
                .cloned()
                .collect()
        };
        if self.lane == "main"
            && let Some(pool) = &self.persistence
        {
            let foreign: Vec<(String,i64,Value,Value,Value)> = sqlx::query_as("SELECT id,job_order,(spec_json-'payload')||'{\"payload\":{}}'::jsonb,state_json,checkpoint_json FROM durable_jobs WHERE lane<>'main' AND (state_json->>'state' IN ('running','pausing','paused','stopping','interrupted') OR state_json->>'cleanup_pending'='true') ORDER BY job_order")
                .fetch_all(pool).await?;
            for (id, order, spec, state, checkpoint) in foreign {
                pending.push(PersistedJob {
                    id,
                    order,
                    spec: serde_json::from_value(spec)
                        .map_err(|_| ApiError::bad("持久任务参数格式无效"))?,
                    state,
                    checkpoint,
                });
            }
        }
        pending.sort_by_key(|job| job.order);
        // A stopped Emby container must be restored before any lane validates
        // or resumes its native-media resources.
        for saved in &pending {
            let context = self.saved_context(saved, cancel.child_token()).await?;
            let cleanup = context.for_resume_cleanup().await;
            let result = tokio::select! { result=Box::pin(crate::activity::resume_cleanup(&state,&cleanup))=>result,_=cancel.cancelled()=>return Ok(())};
            if let Err(error) = result {
                if matches!(error.0.as_u16(), 400 | 401 | 403 | 404 | 409) {
                    self.park_saved(saved, &error, true).await?;
                    continue;
                }
                self.park_saved(saved, &error, false).await?;
                return Err(error);
            }
        }
        crate::strm::recover_pending(&state).await?;
        for saved in &pending {
            let context = self.saved_context(saved, cancel.child_token()).await?;
            let cleanup = context.for_resume_cleanup().await;
            let prepared = tokio::select! {
                result = crate::job_dispatch::prepare_resume(&state, &cleanup) => result,
                _ = cancel.cancelled() => return Ok(()),
            };
            if let Err(error) = prepared {
                if matches!(error.0.as_u16(), 400 | 401 | 403 | 404 | 409) {
                    self.park_saved(saved, &error, true).await?;
                    continue;
                }
                self.park_saved(saved, &error, false).await?;
                return Err(error);
            }
            if let Some(pool) = &self.persistence {
                sqlx::query("UPDATE durable_jobs SET state_json=jsonb_set(state_json,'{cleanup_pending}','false'::jsonb,true),updated_at=NOW() WHERE id=$1")
                    .bind(&saved.id).execute(pool).await?;
                let persisted: Value =
                    sqlx::query_scalar("SELECT state_json FROM durable_jobs WHERE id=$1")
                        .bind(&saved.id)
                        .fetch_one(pool)
                        .await?;
                let mut inner = self.inner.lock().await;
                if inner.results.contains_key(&saved.id) {
                    inner.results.insert(saved.id.clone(), persisted.clone());
                }
                if inner.latest["task_id"] == saved.id {
                    inner.latest = persisted;
                }
            }
        }
        self.inner.lock().await.cleanup_records.clear();
        self.inner.lock().await.recovery_message = None;
        self.recovery_ready.store(true, Ordering::Release);
        self.ready.notify_waiters();
        loop {
            if cancel.is_cancelled() || self.is_shutting_down().await {
                break;
            }
            let pending = {
                let inner = self.inner.lock().await;
                inner
                    .recoverable
                    .iter()
                    .find(|job| !job.spec.owned || job.spec.kind == "media_organize_queue")
                    .cloned()
            };
            let Some(pending) = pending else {
                break;
            };
            let name = pending.state["last_action"]
                .as_str()
                .unwrap_or("后台任务")
                .to_string();
            let spec = pending.spec.clone();
            let work = state.clone();
            let original_id = pending.id.clone();
            let started = self
                .start(
                    &name,
                    Some(spec.clone()),
                    Some(pending),
                    move |context| async move {
                        crate::job_dispatch::execute(&work, &spec, &context).await
                    },
                )
                .await;
            if let Err(error) = started {
                tracing::warn!(task_id=%original_id,"恢复任务暂未启动，保留原任务等待重试：{}",crate::logs::redact(&error.1));
                tokio::select! { _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}, _ = cancel.cancelled() => return Ok(()) }
                continue;
            }
            loop {
                let finished = self.finished.notified();
                tokio::pin!(finished);
                finished.as_mut().enable();
                if self.inner.lock().await.active.is_none() {
                    break;
                }
                tokio::select! { _ = finished => {}, _ = cancel.cancelled() => return Ok(()) }
            }
        }
        self.inner.lock().await.restoring = false;
        self.finished.notify_waiters();
        Ok(())
    }
    async fn saved_context(
        &self,
        saved: &PersistedJob,
        cancel: CancellationToken,
    ) -> Result<JobContext> {
        let progress = if let Some(pool) = &self.persistence {
            sqlx::query_scalar("SELECT state_json FROM durable_jobs WHERE id=$1")
                .bind(&saved.id)
                .fetch_one(pool)
                .await?
        } else {
            saved.state.clone()
        };
        Ok(JobContext {
            id: saved.id.clone(),
            cancel,
            wake: Arc::new(Notify::new()),
            progress: Arc::new(Mutex::new(progress)),
            activity: Arc::default(),
            last_progress_log: Arc::default(),
            pause_generation: Arc::new(AtomicU64::new(0)),
            ingest_directories: Arc::default(),
            persistence: self.persistence.clone(),
            checkpoint_state: Arc::new(Mutex::new(saved.checkpoint.clone())),
            interrupted: Arc::new(AtomicBool::new(false)),
            resuming: true,
            checkpoint_prefix: String::new(),
            pause_intent: None,
            cleanup_state: None,
            server_id: (saved.spec.kind != "database_admin")
                .then(|| saved.spec.server_id.clone())
                .flatten(),
            server_url: (saved.spec.kind != "database_admin")
                .then(|| saved.spec.server_url.clone())
                .flatten(),
        })
    }
    async fn park_saved(
        &self,
        saved: &PersistedJob,
        error: &ApiError,
        terminal: bool,
    ) -> Result<()> {
        let mut state = saved.state.clone();
        let already_terminal = matches!(
            state["state"].as_str(),
            Some("completed" | "completed_with_warnings" | "failed" | "cancelled")
        );
        if terminal && !already_terminal {
            state["state"] = json!("failed");
        }
        if !terminal && !already_terminal {
            state["state"] = json!(if state["pause_requested"] == true {
                "paused"
            } else {
                "interrupted"
            });
        }
        state["is_running"] = json!(false);
        state["message"] = json!(format!(
            "恢复{}：{}",
            if terminal {
                "已停止，保留原进度和恢复材料"
            } else {
                "暂未完成，将自动重试"
            },
            crate::logs::redact(&error.1)
        ));
        state["resume_error"] = json!(crate::logs::redact(&error.1));
        state["cleanup_pending"] = json!(true);
        if terminal && !already_terminal {
            state["result_status"] = json!(error.0.as_u16());
            state["result"] = json!({"error":crate::logs::redact(&error.1)});
        }
        if let Some(pool) = &self.persistence {
            let mut transaction = pool.begin().await?;
            sqlx::query("UPDATE durable_jobs SET state_json=$2,updated_at=NOW() WHERE id=$1")
                .bind(&saved.id)
                .bind(&state)
                .execute(&mut *transaction)
                .await?;
            if terminal
                && !already_terminal
                && saved.spec.owned
                && saved.spec.kind == "media_organize_queue"
            {
                sqlx::query("WITH failed AS (UPDATE media_organize_queue q SET status='failed',message=$2,result=$3,completed_at=NOW() FROM durable_jobs j WHERE j.id=$1 AND q.task_id=j.id AND q.status IN ('queued','running','interrupted') RETURNING q.item_id,q.history_ids)
                    UPDATE media_organize_history h SET status='failed',message=$2,completed_at=NOW() FROM failed q WHERE h.status='processing' AND h.item_id=q.item_id AND q.history_ids ? h.id")
                    .bind(&saved.id).bind(state["message"].as_str().unwrap_or("")).bind(&state["result"]).execute(&mut *transaction).await?;
            }
            transaction.commit().await?;
        }
        let mut inner = self.inner.lock().await;
        inner.recovery_message = state["message"].as_str().map(str::to_string);
        let belongs = inner.recoverable.iter().any(|job| job.id == saved.id)
            || inner.results.contains_key(&saved.id);
        if terminal {
            inner.recoverable.retain(|job| job.id != saved.id);
            if belongs {
                inner.results.insert(saved.id.clone(), state.clone());
                if !inner.history_order.contains(&saved.id) {
                    inner.history_order.push_back(saved.id.clone());
                }
                inner.latest = state;
            }
        } else if let Some(job) = inner.recoverable.iter_mut().find(|job| job.id == saved.id) {
            job.state = state;
        }
        Ok(())
    }
    pub async fn wait_recovery_ready(&self) -> Result<()> {
        if self.persistence.is_none() {
            return Ok(());
        }
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if self.recovery_ready.load(Ordering::Acquire) {
                return Ok(());
            }
            if self.is_shutting_down().await {
                return Err(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "服务正在关闭".into(),
                ));
            }
            ready.await;
        }
    }
    pub async fn submit<F, Fut>(self: &Arc<Self>, name: &str, function: F) -> Result<String>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        self.submit_response(name, move |context| async move {
            Ok((200, function(context).await?))
        })
        .await
    }
    pub async fn submit_response<F, Fut>(
        self: &Arc<Self>,
        name: &str,
        function: F,
    ) -> Result<String>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(u16, Value)>> + Send + 'static,
    {
        self.start(name, None, None, function).await
    }
    async fn start<F, Fut>(
        self: &Arc<Self>,
        name: &str,
        mut spec: Option<JobSpec>,
        restored: Option<PersistedJob>,
        function: F,
    ) -> Result<String>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(u16, Value)>> + Send + 'static,
    {
        let mut inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "服务正在关闭，不能启动新任务".into(),
            ));
        }
        if inner.active.is_some() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "后台有任务正在运行，请稍后再试".into(),
            ));
        }
        if inner.restoring && restored.is_none() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "正在恢复重启前的任务，请稍后再试".into(),
            ));
        }
        let resuming = restored.is_some();
        if let (Some(spec), Some(saved)) = (&mut spec, &restored) {
            spec.server_id = saved.spec.server_id.clone();
            spec.server_url = saved.spec.server_url.clone();
        }
        if !resuming {
            let config = self
                .config
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .and_then(std::sync::Weak::upgrade);
            if let (Some(spec), Some(config)) = (&mut spec, config) {
                let config = config.read().await;
                spec.server_id = (config["emby_server_id_verified"] == true)
                    .then(|| {
                        config["emby_server_id"]
                            .as_str()
                            .filter(|id| !id.is_empty())
                            .map(str::to_string)
                    })
                    .flatten();
                spec.server_url = server_url(&config);
            }
        }
        if let Some(spec) = spec.as_mut().filter(|spec| spec.kind == "database_admin") {
            spec.server_id = None;
            spec.server_url = None;
        }
        let id = restored
            .as_ref()
            .map(|job| job.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut state = restored.as_ref().map(|job| job.state.clone()).unwrap_or_else(|| json!({"task_id":id,"is_running":true,"state":"running","current_action":name,
            "last_action":name,"progress":0,"progress_known":false,"message":"任务初始化","phase":"执行",
            "started_at":chrono::Utc::now().timestamp(),"logs":[]}));
        state["is_running"] = json!(true);
        state["cleanup_pending"] = json!(true);
        state["stop_requested"] = json!(false);
        state["current_action"] = json!(name);
        if resuming {
            state["state"] = json!(if state["pause_requested"] == true {
                "paused"
            } else {
                "running"
            });
            state["resumed_at"] = json!(chrono::Utc::now().timestamp());
            state["message"] = json!(if state["pause_requested"] == true {
                "重启前的暂停状态已恢复"
            } else {
                "已恢复重启前任务，继续未完成项目"
            });
            if let Some(object) = state.as_object_mut() {
                for key in ["finished_at", "result_status", "result"] {
                    object.remove(key);
                }
            }
        }
        let checkpoint = restored
            .as_ref()
            .map(|job| job.checkpoint.clone())
            .unwrap_or_else(|| json!({"plans":{},"completed":{}}));
        let persistence = if let (Some(pool), Some(spec)) = (&self.persistence, &spec) {
            sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json,checkpoint_json,lane) VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET state_json=EXCLUDED.state_json,updated_at=NOW()")
                .bind(&id).bind(json!(spec)).bind(&state).bind(&checkpoint).bind(&self.lane).execute(pool).await?;
            Some(pool.clone())
        } else {
            None
        };
        let context = JobContext {
            id: id.clone(),
            cancel: CancellationToken::new(),
            wake: Arc::new(Notify::new()),
            progress: Arc::new(Mutex::new(state)),
            activity: Arc::new(std::sync::Mutex::new(None)),
            last_progress_log: Arc::new(std::sync::Mutex::new(None)),
            pause_generation: Arc::new(AtomicU64::new(0)),
            ingest_directories: Arc::default(),
            persistence,
            checkpoint_state: Arc::new(Mutex::new(checkpoint)),
            interrupted: Arc::new(AtomicBool::new(false)),
            resuming,
            checkpoint_prefix: String::new(),
            pause_intent: None,
            cleanup_state: None,
            server_id: spec.as_ref().and_then(|spec| spec.server_id.clone()),
            server_url: spec.as_ref().and_then(|spec| spec.server_url.clone()),
        };
        if let Some(restored) = &restored {
            inner.recoverable.retain(|job| job.id != restored.id);
        }
        inner.active = Some(context.clone());
        drop(inner);
        tracing::info!(task_id=%id,"[任务开始] {name}");
        let manager = self.clone();
        let owner = spec
            .as_ref()
            .filter(|spec| spec.owned && spec.kind == "media_organize_queue")
            .map(|spec| spec.payload.clone());
        let operator = spec.and_then(|spec| spec.operator_id);
        tokio::spawn(async move {
            let task_context = context.clone();
            let work = std::panic::AssertUnwindSafe(TASK_ACTIVITY.scope(
                context.activity.clone(),
                TASK_CONTEXT.scope(
                    context.clone(),
                    crate::notifications::with_delivery_key(context.id.clone(), async move {
                        task_context.checkpoint().await?;
                        let future = function(task_context).boxed();
                        if let Some(operator) = operator {
                            crate::notifications::with_operator(operator, future).await
                        } else {
                            future.await
                        }
                    }),
                ),
            ))
            .catch_unwind();
            tokio::pin!(work);
            let mut waiting = tokio::time::interval(std::time::Duration::from_secs(30));
            waiting.tick().await;
            let result = loop {
                tokio::select! {
                    result = &mut work => break result,
                    _ = waiting.tick() => {
                        let state = context.progress.lock().await;
                        let now = chrono::Utc::now().timestamp();
                        if state["state"] == "paused" || context.is_cancelled() {
                            continue;
                        }
                        let remaining = context.last_progress_log.lock().unwrap_or_else(|error| error.into_inner())
                            .as_ref().and_then(|(before, _, _)| std::time::Duration::from_secs(30).checked_sub(before.elapsed()));
                        if let Some(remaining) = remaining {
                            waiting.reset_after(remaining);
                            continue;
                        }
                        let name = state["last_action"].as_str().unwrap_or("后台任务");
                        let phase = state["phase"].as_str().unwrap_or("执行");
                        let activity = context.activity.lock().unwrap_or_else(|error| error.into_inner());
                        let message = activity.as_deref().or_else(|| state["message"].as_str()).unwrap_or("等待当前操作结束");
                        let elapsed = now - state["started_at"].as_i64().unwrap_or(now);
                        tracing::info!(task_id=%context.id,"[任务进度] {name} · {phase} · 已运行 {elapsed} 秒 · {message}");
                    }
                }
            }
                .unwrap_or_else(|_| {
                    Err(ApiError(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "任务执行异常".into(),
                    ))
                });
            let pending_notifications = context.is_persistent()
                && crate::notifications::has_pending_registration(&context)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::warn!(
                            task_id=%context.id,
                            "通知登记状态未确认，保留任务收尾：{}",
                            crate::logs::redact(&error.1)
                        );
                        true
                    });
            let mut state = context.progress.lock().await;
            let keep_paused = state["pause_requested"] == true;
            let stopped = context.is_cancelled() && !context.is_interrupted();
            state["is_running"] = json!(false);
            state["pause_requested"] = json!(context.is_interrupted() && keep_paused);
            state["stop_requested"] = json!(false);
            let failed = result.as_ref().is_ok_and(|(status, value)| {
                *status >= 400
                    || value["success"] == false
                    || value["error"]
                        .as_str()
                        .is_some_and(|error| !error.is_empty())
                    || value["failed"].as_u64().unwrap_or(0) > 0
                    || value["failures"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
            });
            let warnings = result.as_ref().is_ok_and(|(status, value)| {
                *status == 207
                    || value["warnings"].as_u64().unwrap_or(0) > 0
                    || value["warning_details"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
            });
            let mut cancellation_warning = false;
            if context.is_interrupted() {
                state["state"] = json!(if keep_paused { "paused" } else { "interrupted" });
                state["message"] = json!("服务重启中断，已保存任务状态和完成进度");
            } else if context.cancel.is_cancelled() {
                state["state"] = json!("cancelled");
                state["message"] = json!("任务已取消");
                match result {
                    Ok((status, value)) => {
                        cancellation_warning = failed || warnings;
                        if cancellation_warning {
                            let message = value["error"]
                                .as_str()
                                .filter(|error| !error.is_empty())
                                .or_else(|| value["message"].as_str())
                                .unwrap_or("请查看任务结果中的失败或提醒详情");
                            state["message"] =
                                json!(format!("任务已取消，收尾处理有提醒：{message}"));
                        }
                        state["result_status"] = json!(status);
                        state["result"] = value;
                    }
                    Err(error) if error.1 != "任务已取消" => {
                        cancellation_warning = true;
                        state["message"] =
                            json!(format!("任务已取消，收尾处理有提醒：{}", error.1));
                        state["result_status"] = json!(error.0.as_u16());
                        state["result"] = json!({"error":error.1});
                    }
                    Err(_) => (),
                }
            } else {
                match result {
                    Ok((status, value)) => {
                        state["state"] = json!(if failed {
                            "failed"
                        } else if warnings {
                            "completed_with_warnings"
                        } else {
                            "completed"
                        });
                        state["progress"] = json!(100);
                        state["result_status"] = json!(status);
                        let message = value["error"]
                            .as_str()
                            .filter(|error| !error.is_empty())
                            .or_else(|| value["message"].as_str())
                            .unwrap_or(if failed {
                                "部分项目处理失败，请查看任务结果"
                            } else if warnings {
                                "处理完成，有提醒，请查看任务结果"
                            } else {
                                "处理完成"
                            })
                            .to_string();
                        state["result"] = value;
                        state["message"] = json!(message);
                    }
                    Err(error) => {
                        state["state"] = json!("failed");
                        state["message"] = json!(error.1);
                        state["result_status"] = json!(error.0.as_u16());
                        state["result"] = json!({"error":error.1});
                    }
                }
            }
            state["finished_at"] = json!(chrono::Utc::now().timestamp());
            state["current_action"] = json!("空闲");
            state["cleanup_pending"] = json!(
                matches!(
                    state["state"].as_str(),
                    Some("failed" | "completed_with_warnings" | "interrupted" | "paused")
                ) || cancellation_warning || pending_notifications
            );
            if let Some(activity) = context
                .activity
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                state["last_activity"] = json!(activity);
            }
            if let (Some(owner), Some(pool)) = (&owner, &context.persistence) {
                let status = match state["state"].as_str() {
                    Some("completed" | "completed_with_warnings") => Some("completed"),
                    Some("failed") => Some("failed"),
                    Some("cancelled") => Some("cancelled"),
                    _ => None,
                };
                if let Some(status) = status {
                    if let Err(error)=sqlx::query("UPDATE media_organize_queue SET status=$3,result=$4,message=$5,completed_at=NOW() WHERE task_id=$2 AND EXISTS(SELECT 1 FROM media_organize_queue owner WHERE owner.id=$1 AND owner.task_id=$2) AND status IN ('queued','running','interrupted')")
                        .bind(owner["queue_id"].as_str().unwrap_or("")).bind(&context.id).bind(status).bind(&state["result"]).bind(state["message"].as_str().unwrap_or("")).execute(pool).await {
                        state["state"]=json!("interrupted");state["cleanup_pending"]=json!(true);
                        state["stop_requested"]=json!(stopped);
                        state["message"]=json!(format!("整理队列收尾未确认，保留原任务等待恢复：{}",crate::logs::redact(&error.to_string())));
                    }
                }
            }
            let saved = if context.persistence.is_some() {
                if matches!(
                    state["state"].as_str(),
                    Some("completed" | "completed_with_warnings" | "failed" | "cancelled")
                ) {
                    match context.complete("__job_result__", &state).await {
                        Ok(()) => context.persist_state(&state).await,
                        Err(error) => Err(error),
                    }
                } else {
                    context.persist_state(&state).await
                }
            } else {
                Ok(())
            };
            if let Err(error) = saved {
                state["state"] = json!("interrupted");
                state["cleanup_pending"] = json!(true);
                state["stop_requested"] = json!(stopped);
                state["message"] = json!(format!(
                    "任务结果持久化未确认，已保留进度等待恢复：{}",
                    crate::logs::redact(&error.1)
                ));
                state["progress_known"] = json!(false);
                tracing::error!(task_id=%context.id,"{}",state["message"].as_str().unwrap_or("任务结果未确认"));
            }
            let name = state["last_action"].as_str().unwrap_or("后台任务");
            let message = state["message"].as_str().unwrap_or("任务结束");
            let elapsed = state["finished_at"].as_i64().unwrap_or(0)
                - state["started_at"].as_i64().unwrap_or(0);
            match state["state"].as_str() {
                Some("failed") => {
                    tracing::error!(task_id=%context.id,"[任务结束] {name} · 失败 · 耗时 {elapsed} 秒 · {message}")
                }
                Some("completed_with_warnings") => {
                    tracing::warn!(task_id=%context.id,"[任务结束] {name} · 完成，有提醒 · 耗时 {elapsed} 秒 · {message}")
                }
                Some("cancelled") if cancellation_warning => {
                    tracing::warn!(task_id=%context.id,"[任务结束] {name} · 已停止，有提醒 · 耗时 {elapsed} 秒 · {message}")
                }
                Some("cancelled") => {
                    tracing::info!(task_id=%context.id,"[任务结束] {name} · 已停止 · 耗时 {elapsed} 秒 · {message}")
                }
                Some("interrupted" | "paused") => {
                    tracing::info!(task_id=%context.id,"[任务中断] {name} · 已保存进度，启动后恢复")
                }
                _ => {
                    tracing::info!(task_id=%context.id,"[任务结束] {name} · 完成 · 耗时 {elapsed} 秒 · {message}")
                }
            }
            let mut inner = manager.inner.lock().await;
            inner.latest = state.clone();
            inner.results.insert(context.id.clone(), state.clone());
            inner.history_order.push_back(context.id.clone());
            inner.active = None;
            if inner.history_order.len() > 30
                && let Some(key) = inner.history_order.pop_front()
            {
                inner.results.remove(&key);
            }
            manager.finished.notify_waiters();
        });
        Ok(id)
    }
    pub(crate) async fn is_shutting_down(&self) -> bool {
        self.inner.lock().await.shutting_down
    }
    pub(crate) async fn wait_idle(&self) -> Result<()> {
        loop {
            let finished = self.finished.notified();
            tokio::pin!(finished);
            finished.as_mut().enable();
            let inner = self.inner.lock().await;
            if inner.shutting_down {
                return Err(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "服务正在关闭".into(),
                ));
            }
            if inner.active.is_none() && !inner.restoring {
                return Ok(());
            }
            drop(inner);
            finished.await;
        }
    }
    pub async fn shutdown(&self) {
        loop {
            let finished = self.finished.notified();
            tokio::pin!(finished);
            finished.as_mut().enable();
            let mut inner = self.inner.lock().await;
            inner.shutting_down = true;
            self.ready.notify_waiters();
            let active = inner.active.clone();
            drop(inner);
            let Some(active) = active else {
                return;
            };
            let mut state = active.progress.lock().await;
            if state["is_running"] == true {
                if active.persistence.is_some() && state["stop_requested"] != true {
                    active.interrupted.store(true, Ordering::Release);
                    state["state"] = json!("interrupted");
                    state["stop_requested"] = json!(false);
                    active.persist_progress(&state).await;
                } else {
                    state["pause_requested"] = json!(false);
                    state["stop_requested"] = json!(true);
                    state["state"] = json!("stopping");
                }
                active.cancel.cancel();
                active.wake.notify_waiters();
            }
            drop(state);
            finished.await;
        }
    }
    pub async fn status(&self) -> Value {
        let inner = self.inner.lock().await;
        let active = inner.active.clone();
        let latest = inner.latest.clone();
        let restoring = inner.restoring;
        let recovery_message = inner.recovery_message.clone();
        let pending = inner
            .recoverable
            .iter()
            .find(|job| !job.spec.owned)
            .map(|job| job.state.clone());
        let history: Vec<_> = inner
            .history_order
            .iter()
            .rev()
            .filter_map(|id| inner.results.get(id))
            .map(|value| {
                let mut value = value.clone();
                value.as_object_mut().unwrap().remove("result");
                value
            })
            .collect();
        drop(inner);
        let mut status = if let Some(active) = active {
            let mut state = active.progress.lock().await.clone();
            if let Some(activity) = active
                .activity
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                state["last_activity"] = json!(activity);
            }
            state
        } else if let Some(mut pending) = pending {
            pending["is_running"] = json!(false);
            pending["state"] = json!("interrupted");
            if pending["resume_error"].is_null() {
                pending["message"] = json!("正在恢复重启前任务");
            }
            pending
        } else if restoring {
            json!({"is_running":false,"state":"interrupted","progress":0,"progress_known":false,"current_action":"恢复资源","phase":"恢复准备","message":recovery_message.unwrap_or_else(||"正在恢复重启前后台任务的资源".into()),"logs":[]})
        } else if latest.is_null() {
            json!({"is_running":false,"state":"idle","progress":0,"message":"空闲","current_action":"空闲","logs":[]})
        } else {
            latest
        };
        status.as_object_mut().unwrap().remove("result");
        status["history"] = json!(history);
        status
    }
    pub async fn control(&self, id: Option<&str>, action: &str) -> Result<()> {
        let active = self
            .inner
            .lock()
            .await
            .active
            .clone()
            .ok_or_else(|| ApiError(StatusCode::CONFLICT, "没有正在运行的任务".into()))?;
        if id.is_some_and(|id| id != active.id) {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "任务已变化，请刷新后重试".into(),
            ));
        }
        let mut state = active.progress.lock().await;
        if state["is_running"] != true {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "任务已结束，请刷新后重试".into(),
            ));
        }
        if state["state"] == "stopping" && action != "stop" {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "任务正在停止，无法暂停或恢复".into(),
            ));
        }
        let name = state["last_action"]
            .as_str()
            .unwrap_or("后台任务")
            .to_string();
        let previous = state.clone();
        match action {
            "pause" => {
                state["pause_requested"] = json!(true);
                if state["state"] != "paused" && state["state"] != "pausing" {
                    state["state"] = json!("pausing");
                    tracing::info!(task_id=%active.id,"[任务控制] {name} · 已请求暂停，等待当前操作结束");
                }
                active.wake.notify_waiters();
            }
            "resume" => {
                state["pause_requested"] = json!(false);
                if state["state"] == "paused" || state["state"] == "pausing" {
                    state["state"] = json!("running");
                    tracing::info!(task_id=%active.id,"[任务恢复] {name} · 继续原任务");
                }
                active.wake.notify_waiters();
            }
            "stop" => {
                state["pause_requested"] = json!(false);
                state["stop_requested"] = json!(true);
                if state["state"] != "stopping" {
                    state["state"] = json!("stopping");
                    tracing::info!(task_id=%active.id,"[任务控制] {name} · 已请求停止，等待当前操作安全结束");
                }
                active.wake.notify_waiters();
            }
            _ => return Err(ApiError::bad("未知任务控制操作")),
        }
        if let Err(error) = active.persist_state(&state).await {
            *state = previous;
            return Err(error);
        }
        if action == "stop" {
            active.interrupted.store(false, Ordering::Release);
            active.cancel.cancel();
        }
        active.wake.notify_waiters();
        Ok(())
    }
    pub async fn result(&self, id: &str) -> Option<Value> {
        let inner = self.inner.lock().await;
        if let Some(result) = inner.results.get(id) {
            return Some(result.clone());
        }
        if let Some(pending) = inner.recoverable.iter().find(|job| job.id == id) {
            return Some(pending.state.clone());
        }
        let active = inner.active.clone().filter(|v| v.id == id);
        drop(inner);
        if let Some(active) = active {
            let mut state = active.progress.lock().await.clone();
            if let Some(activity) = active
                .activity
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                state["last_activity"] = json!(activity);
            }
            Some(state)
        } else {
            if let Some(pool) = &self.persistence {
                sqlx::query_scalar("SELECT state_json FROM durable_jobs WHERE id=$1 AND lane=$2")
                    .bind(id)
                    .bind(&self.lane)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            }
        }
    }
}
