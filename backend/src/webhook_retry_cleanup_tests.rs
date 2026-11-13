use super::*;
use std::sync::Arc;
use tokio::sync::RwLock;

async fn closed_database_state() -> (AppState, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let db = sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap();
    db.close().await;
    let state = AppState {
        db,
        config: Arc::new(RwLock::new(json!({}))),
        http: reqwest::Client::new(),
        sessions: Arc::new(crate::auth::SessionSigner::load(directory.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: directory.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: crate::logs::LogStore::open(&directory.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    (state, directory)
}

async fn completed_job(state: &AppState) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = state.jobs.status().await;
            if status["is_running"] == false {
                return state
                    .jobs
                    .result(status["task_id"].as_str().unwrap())
                    .await
                    .unwrap();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the failed history write must finish and release the job slot")
}

#[tokio::test]
async fn failed_moviepilot_history_write_releases_the_batch_for_retry() {
    let (state, _directory) = closed_database_state().await;
    let key = "moviepilot-history-write-failure";
    state
        .webhooks
        .batches
        .lock()
        .await
        .insert(key.into(), Instant::now());
    submit_queued_event(
        &state,
        key.into(),
        json!({
            "source":"moviepilot", "batch_key":key, "files":[], "history_ids":["history"]
        }),
    )
    .await;
    let result = completed_job(&state).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result_status"], 500, "{result}");
    assert!(
        !state.webhooks.batches.lock().await.contains_key(key),
        "a final failed task must not reject the same MoviePilot batch as duplicate"
    );
}

#[tokio::test]
async fn successful_moviepilot_task_keeps_its_batch_deduplication_marker() {
    let (state, _directory) = closed_database_state().await;
    let key = "moviepilot-success";
    state
        .webhooks
        .batches
        .lock()
        .await
        .insert(key.into(), Instant::now());
    submit_queued_event(
        &state,
        key.into(),
        json!({
            "source":"moviepilot", "batch_key":key, "files":[], "history_ids":[]
        }),
    )
    .await;
    let result = completed_job(&state).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["status"], "ignored_library", "{result}");
    assert!(state.webhooks.batches.lock().await.contains_key(key));
}
