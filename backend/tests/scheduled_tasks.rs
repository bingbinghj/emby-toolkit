use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::SessionSigner, emby, jobs::JobManager, logs::LogStore};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::RwLock;

fn state(root: &std::path::Path, base: &str) -> AppState {
    AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(
            json!({"emby_server_url":base,"emby_api_key":"fixture"}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root).unwrap()),
        recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
        data_dir: root.into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::new(JobManager::default()),
        logs: LogStore::open(&root.join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    }
}

async fn finish(state: &AppState, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let result = state.jobs.result(id).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("scheduled task must terminate")
}

async fn scheduled(sequence: Vec<Value>) -> (Value, usize, usize) {
    let reads = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let query_reads = reads.clone();
    let submitted = starts.clone();
    let app = Router::new()
        .route(
            "/ScheduledTasks",
            get(move || {
                let index = query_reads
                    .fetch_add(1, Ordering::SeqCst)
                    .min(sequence.len() - 1);
                let task = sequence[index].clone();
                async move { Json(json!([task])) }
            }),
        )
        .route(
            "/ScheduledTasks/Running/extract",
            post(move || {
                submitted.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::NO_CONTENT }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let state = state(directory.path(), &format!("http://{address}"));
    let work = state.clone();
    let id = state
        .jobs
        .submit("提取验收", move |context| async move {
            emby::scheduled(&work, "MediaInfoExtractTask", &context).await?;
            Ok(json!({"message":"已完成提取"}))
        })
        .await
        .unwrap();
    let result = finish(&state, &id).await;
    server.abort();
    (
        result,
        reads.load(Ordering::SeqCst),
        starts.load(Ordering::SeqCst),
    )
}

fn task(state: &str, run: &str, status: Value) -> Value {
    json!({"Id":"extract","Key":"MediaInfoExtractTask","State":state,
        "LastExecutionResult":{"StartTimeUtc":run,"Status":status}})
}

#[tokio::test]
async fn scheduled_rejects_terminal_results_that_are_not_completed() {
    for status in [
        json!("Failed"),
        json!("Aborted"),
        json!("Cancelled"),
        Value::Null,
    ] {
        let (result, _, starts) = scheduled(vec![
            task("Idle", "old", json!("Completed")),
            task("Idle", "new", status.clone()),
        ])
        .await;
        assert_eq!(result["state"], "failed", "{status}: {result}");
        assert_eq!(starts, 1);
    }
}

#[tokio::test]
async fn scheduled_waits_through_cancelling_without_reusing_previous_success() {
    let (result, reads, starts) = scheduled(vec![
        task("Running", "old", json!("Completed")),
        task("Cancelling", "old", json!("Completed")),
        task("Idle", "new", json!("Aborted")),
    ])
    .await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(reads >= 3);
    assert_eq!(starts, 0);
}

#[tokio::test]
async fn scheduled_accepts_a_current_completed_execution() {
    let (result, _, starts) = scheduled(vec![
        task("Idle", "old", json!("Completed")),
        task("Idle", "new", json!("Completed")),
    ])
    .await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(starts, 1);
}

#[tokio::test]
async fn scheduled_does_not_restart_a_cancelling_execution() {
    let (result, _, starts) = scheduled(vec![
        task("Cancelling", "old", json!("Completed")),
        task("Idle", "new", json!("Aborted")),
    ])
    .await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(starts, 0);
}

#[tokio::test]
async fn cancelled_local_job_does_not_start_emby_extraction() {
    let directory = tempfile::tempdir().unwrap();
    let state = state(directory.path(), "http://127.0.0.1:1");
    let work = state.clone();
    let (release, released) = tokio::sync::oneshot::channel();
    let (observe, observed) = tokio::sync::oneshot::channel();
    let id = state
        .jobs
        .submit("取消验收", move |context| async move {
            released.await.unwrap();
            let error = emby::scheduled(&work, "MediaInfoExtractTask", &context)
                .await
                .unwrap_err();
            observe.send(error.1.clone()).unwrap();
            Err(error)
        })
        .await
        .unwrap();
    state.jobs.control(Some(&id), "stop").await.unwrap();
    release.send(()).unwrap();
    assert_eq!(observed.await.unwrap(), "任务已取消");
    assert_eq!(finish(&state, &id).await["state"], "cancelled");
}
