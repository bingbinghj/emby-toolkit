use crate::error::{ApiError, Result};
use axum::http::StatusCode;
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, sync::Arc};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct JobContext {
    pub id: String,
    cancel: CancellationToken,
    wake: Arc<Notify>,
    progress: Arc<Mutex<Value>>,
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
                tracing::info!(task_id=%self.id,"[任务暂停] 已到达安全暂停点");
            }
            drop(state);
            tokio::select! { _ = self.cancel.cancelled()=>{}, _ = wake=>{} }
        }
    }
    pub async fn update(&self, progress: u64, phase: &str, message: &str) {
        let mut state = self.progress.lock().await;
        state["progress"] = json!(progress.min(100));
        state["progress_known"] = json!(true);
        state["phase"] = json!(phase);
        state["message"] = json!(message);
        tracing::info!(task_id=%self.id,progress,%phase,"[任务进度] {message}");
    }
}
#[derive(Default)]
struct Inner {
    active: Option<JobContext>,
    shutting_down: bool,
    latest: Value,
    results: HashMap<String, Value>,
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
        };
        inner.active = Some(context.clone());
        drop(inner);
        tracing::info!(task_id=%id,"[任务开始] {name}");
        let manager = self.clone();
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(async { function(context.clone()).await })
                .catch_unwind()
                .await
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
            tracing::info!(task_id=%context.id,state=%state["state"],"[任务结束] {}",state["message"]);
            let mut inner = manager.inner.lock().await;
            inner.latest = state.clone();
            inner.results.insert(context.id.clone(), state.clone());
            inner.active = None;
            if inner.results.len() > 100
                && let Some(key) = inner
                    .results
                    .keys()
                    .find(|key| **key != context.id)
                    .cloned()
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
        let mut history: Vec<_> = inner.results.values().cloned().collect();
        history.sort_by_key(|v| std::cmp::Reverse(v["finished_at"].as_i64().unwrap_or(0)));
        drop(inner);
        let mut status = if let Some(active) = active {
            active.progress.lock().await.clone()
        } else if latest.is_null() {
            json!({"is_running":false,"state":"idle","progress":0,"message":"空闲","current_action":"空闲","logs":[]})
        } else {
            latest
        };
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
        match action {
            "pause" => {
                state["pause_requested"] = json!(true);
                if state["state"] != "paused" && state["state"] != "pausing" {
                    state["state"] = json!("pausing");
                    tracing::info!(task_id=%active.id,"[任务控制] 已请求暂停，等待当前操作结束");
                }
            }
            "resume" => {
                state["pause_requested"] = json!(false);
                if state["state"] == "paused" || state["state"] == "pausing" {
                    state["state"] = json!("running");
                    tracing::info!(task_id=%active.id,"[任务恢复] 继续原任务");
                }
                active.wake.notify_waiters();
            }
            "stop" => {
                active.cancel.cancel();
                state["pause_requested"] = json!(false);
                state["stop_requested"] = json!(true);
                if state["state"] != "stopping" {
                    state["state"] = json!("stopping");
                    tracing::info!(task_id=%active.id,"[任务控制] 已请求停止，等待当前操作安全结束");
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
            Some(active.progress.lock().await.clone())
        } else {
            None
        }
    }
}
