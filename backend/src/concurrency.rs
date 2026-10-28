use crate::{AppState, error::Result, jobs::JobContext, state::string};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

struct Limits {
    counts: [usize; 2],
    active: [usize; 2],
    cooldown: Duration,
    ready: Instant,
    refresh_at: Instant,
    from_server: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            counts: [1, 1],
            active: [0, 0],
            cooldown: Duration::ZERO,
            ready: Instant::now(),
            refresh_at: Instant::now(),
            from_server: false,
        }
    }
}
#[derive(Default)]
pub(crate) struct Service {
    gates: Mutex<HashMap<(String, String), Arc<Gate>>>,
}
#[derive(Default)]
struct Gate {
    limits: Mutex<Limits>,
    refresh: tokio::sync::Mutex<()>,
    changed: Notify,
}
pub struct Permit {
    service: Arc<Gate>,
    index: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut limits = self
            .service
            .limits
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        limits.active[self.index] -= 1;
        if self.index == 0 && limits.counts[0] == 1 {
            limits.ready = Instant::now() + limits.cooldown;
        }
        drop(limits);
        self.service.changed.notify_waiters();
    }
}
fn index(kind: &str) -> usize {
    match kind {
        "main" => 0,
        "secondary" => 1,
        _ => panic!("线程类别无效"),
    }
}
async fn refresh(state: &AppState) -> Arc<Gate> {
    let config = state.config.read().await.clone();
    let connection = (
        string(&config, "emby_server_url")
            .trim_end_matches('/')
            .to_string(),
        string(&config, "emby_api_key"),
    );
    let service = state
        .strm
        .threads
        .gates
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(connection.clone())
        .or_default()
        .clone();
    {
        let limits = service.limits.lock().unwrap_or_else(|e| e.into_inner());
        if Instant::now() < limits.refresh_at {
            return service.clone();
        }
    }
    let _refresh = service.refresh.lock().await;
    {
        let mut limits = service.limits.lock().unwrap_or_else(|e| e.into_inner());
        if Instant::now() < limits.refresh_at {
            return service.clone();
        }
        limits.refresh_at = Instant::now() + Duration::from_secs(60);
    }
    if connection.0.is_empty() || connection.1.is_empty() {
        return service.clone();
    }
    match crate::strm::threads(state).await {
        Ok(settings) => {
            let mut limits = service.limits.lock().unwrap_or_else(|e| e.into_inner());
            let counts = [
                settings["main"].as_u64().unwrap_or(1) as usize,
                settings["secondary"].as_u64().unwrap_or(1) as usize,
            ];
            let cooldown = Duration::from_secs(settings["cooldown"].as_u64().unwrap_or(0));
            if limits.counts != counts || limits.cooldown != cooldown || !limits.from_server {
                tracing::info!(
                    "[神医线程] ETK 共用设置：主={}，次={}，单主线程冷却={}秒。",
                    counts[0],
                    counts[1],
                    cooldown.as_secs()
                );
            }
            limits.from_server = true;
            limits.counts = counts;
            limits.cooldown = cooldown;
            drop(limits);
            service.changed.notify_waiters();
        }
        Err(_) => tracing::warn!("[神医线程] 暂时无法读取设置，沿用当前主/次并发上限；稍后重试。"),
    }
    drop(_refresh);
    service
}
pub async fn thread_limit(state: &AppState, kind: &str) -> usize {
    let service = refresh(state).await;
    service
        .limits
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .counts[index(kind)]
}
pub async fn slot(state: &AppState, kind: &str, context: Option<&JobContext>) -> Result<Permit> {
    let service = refresh(state).await;
    let index = index(kind);
    loop {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        let notified = service.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let mut limits = service.limits.lock().unwrap_or_else(|e| e.into_inner());
            let cooling = index == 0 && limits.counts[0] == 1 && Instant::now() < limits.ready;
            if limits.active[index] < limits.counts[index] && !cooling {
                limits.active[index] += 1;
                return Ok(Permit {
                    service: service.clone(),
                    index,
                });
            }
        }
        let wait = async {
            if let Some(context) = context {
                context.sleep(Duration::from_millis(200)).await
            } else {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(())
            }
        };
        tokio::select! { _ = notified => {}, result = wait => result? }
    }
}
