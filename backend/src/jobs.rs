use crate::error::{ApiError, Result};
use axum::http::StatusCode;
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

type Activity = Arc<std::sync::Mutex<Option<String>>>;
type ProgressLog = Option<(tokio::time::Instant, Option<u64>, String)>;
tokio::task_local! {
    static TASK_ACTIVITY: Activity;
}

pub(crate) fn record_activity(message: &str) {
    let _ = TASK_ACTIVITY.try_with(|activity| {
        *activity.lock().unwrap_or_else(|error| error.into_inner()) =
            Some(crate::logs::redact(message).chars().take(500).collect());
    });
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
}
impl JobContext {
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
    pub(crate) fn pause_generation(&self) -> u64 {
        self.pause_generation.load(Ordering::Acquire)
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
}
#[derive(Default)]
pub struct JobManager {
    inner: Mutex<Inner>,
    finished: Notify,
}
impl JobManager {
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
        let id = uuid::Uuid::new_v4().to_string();
        let state = json!({"task_id":id,"is_running":true,"state":"running","current_action":name,
            "last_action":name,"progress":0,"progress_known":false,"message":"任务初始化","phase":"执行",
            "started_at":chrono::Utc::now().timestamp(),"logs":[]});
        let context = JobContext {
            id: id.clone(),
            cancel: CancellationToken::new(),
            wake: Arc::new(Notify::new()),
            progress: Arc::new(Mutex::new(state)),
            activity: Arc::new(std::sync::Mutex::new(None)),
            last_progress_log: Arc::new(std::sync::Mutex::new(None)),
            pause_generation: Arc::new(AtomicU64::new(0)),
        };
        inner.active = Some(context.clone());
        drop(inner);
        tracing::info!(task_id=%id,"[任务开始] {name}");
        let manager = self.clone();
        tokio::spawn(async move {
            let work = std::panic::AssertUnwindSafe(
                TASK_ACTIVITY.scope(context.activity.clone(), function(context.clone())),
            )
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
            let mut state = context.progress.lock().await;
            state["is_running"] = json!(false);
            state["pause_requested"] = json!(false);
            state["stop_requested"] = json!(false);
            if context.cancel.is_cancelled() {
                state["state"] = json!("cancelled");
                state["message"] = json!("任务已取消");
            } else {
                match result {
                    Ok((status, value)) => {
                        let failed = status >= 400
                            || value["failed"].as_u64().unwrap_or(0) > 0
                            || value["failures"].as_array().is_some_and(|v| !v.is_empty());
                        let warnings = value["warnings"].as_u64().unwrap_or(0) > 0;
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
                            .or_else(|| value["message"].as_str())
                            .unwrap_or(if failed {
                                "部分项目处理失败，请查看任务结果"
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
            if let Some(activity) = context
                .activity
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                state["last_activity"] = json!(activity);
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
                Some("cancelled") => {
                    tracing::info!(task_id=%context.id,"[任务结束] {name} · 已停止 · 耗时 {elapsed} 秒 · {message}")
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
    pub async fn shutdown(&self) {
        loop {
            let finished = self.finished.notified();
            tokio::pin!(finished);
            finished.as_mut().enable();
            let mut inner = self.inner.lock().await;
            inner.shutting_down = true;
            let active = inner.active.clone();
            drop(inner);
            let Some(active) = active else {
                return;
            };
            let mut state = active.progress.lock().await;
            if state["is_running"] == true {
                active.cancel.cancel();
                state["pause_requested"] = json!(false);
                state["stop_requested"] = json!(true);
                state["state"] = json!("stopping");
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
        match action {
            "pause" => {
                state["pause_requested"] = json!(true);
                if state["state"] != "paused" && state["state"] != "pausing" {
                    state["state"] = json!("pausing");
                    tracing::info!(task_id=%active.id,"[任务控制] {name} · 已请求暂停，等待当前操作结束");
                }
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
                active.cancel.cancel();
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
        Ok(())
    }
    pub async fn result(&self, id: &str) -> Option<Value> {
        let inner = self.inner.lock().await;
        if let Some(result) = inner.results.get(id) {
            return Some(result.clone());
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
            None
        }
    }
}
