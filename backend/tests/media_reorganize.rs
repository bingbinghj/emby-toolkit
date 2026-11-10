use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::get,
};
use emby_toolkit::AppState;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Notify, RwLock};
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    requests: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
    waiting: Arc<Notify>,
    release: Arc<Notify>,
}

#[derive(Clone)]
struct Host {
    requests: Arc<Mutex<Vec<String>>>,
    waiting: Arc<Notify>,
    release: Arc<Notify>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let waiting = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let upstream = Router::new()
            .route(
                "/Users/user/Items/{id}",
                get(
                    |State(host): State<Host>,
                     axum::extract::Path(id): axum::extract::Path<String>| async move {
                        host.requests.lock().await.push(id.clone());
                        if id == "wrong" {
                            return (
                                StatusCode::OK,
                                Json(json!({"Id":"202","Type":"Movie","Name":"another movie"})),
                            );
                        }
                        if id == "wait" {
                            host.waiting.notify_one();
                            host.release.notified().await;
                        }
                        (
                            StatusCode::NOT_FOUND,
                            Json(json!({"error":"fixture item disappeared"})),
                        )
                    },
                ),
            )
            .with_state(Host {
                requests: requests.clone(),
                waiting: waiting.clone(),
                release: release.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let state = AppState {
            db: sqlx::postgres::PgPoolOptions::new()
                .acquire_timeout(Duration::from_millis(10))
                .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
                .unwrap(),
            config: Arc::new(RwLock::new(
                json!({"emby_server_url":base,"emby_user_id":"user","emby_api_key":"fixture"}),
            )),
            http: reqwest::Client::new(),
            sessions: Arc::new(emby_toolkit::auth::SessionSigner::load(directory.path()).unwrap()),
            recovery_tokens: Arc::default(),
            data_dir: directory.path().into(),
            notifications: Arc::default(),
            webhooks: Arc::default(),
            ai: Arc::default(),
            jobs: Arc::default(),
            logs: emby_toolkit::logs::LogStore::open(&directory.path().join("logs")).unwrap(),
            p115: Arc::default(),
            strm: Arc::default(),
            douban: Arc::default(),
            docker: Arc::default(),
        };
        Self {
            state,
            requests,
            server,
            _directory: directory,
            waiting,
            release,
        }
    }

    async fn submit(&self, body: Value) -> (StatusCode, Value) {
        let response = emby_toolkit::actions::router()
            .with_state(self.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/actions/reorganize")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn result(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn an_emby_response_for_another_item_cannot_change_the_selected_scope() {
    let f = Fixture::new().await;
    let (_, accepted) = f.submit(json!({"item_ids":["wrong"]})).await;
    let result = f.result(accepted["task_id"].as_str().unwrap()).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(
        result["result"]["failures"][0]
            .as_str()
            .unwrap()
            .contains("媒体 ID 与整理目标不一致")
    );
    assert_eq!(*f.requests.lock().await, vec!["wrong"]);
}

#[tokio::test]
async fn cancellation_retains_previous_failures_and_does_not_start_the_remaining_item() {
    let f = Fixture::new().await;
    let (_, accepted) = f.submit(json!({"item_ids":["101","wait","303"]})).await;
    tokio::time::timeout(Duration::from_secs(2), f.waiting.notified())
        .await
        .unwrap();
    let task = accepted["task_id"].as_str().unwrap();
    f.state.jobs.control(Some(task), "stop").await.unwrap();
    f.release.notify_one();
    let result = f.result(task).await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(result["result"]["items"][0]["status"], "failed");
    assert_eq!(result["result"]["items"][1]["status"], "cancelled");
    assert_eq!(result["result"]["failures"].as_array().unwrap().len(), 1);
    assert_eq!(result["result"]["remaining_ids"], json!(["303"]));
    assert_eq!(*f.requests.lock().await, vec!["101", "wait"]);
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn an_explicit_valid_selection_is_required_before_starting_any_work() {
    let f = Fixture::new().await;
    for body in [
        json!({}),
        json!({"item_ids":[]}),
        json!({"item_ids":"all"}),
        json!({"item_ids":["101", null]}),
        json!({"item_ids":["../101"]}),
        json!({"item_ids":[""]}),
        json!({"item_ids":vec!["101";501]}),
    ] {
        let (status, result) = f.submit(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{result}");
        assert_eq!(f.state.jobs.status().await["is_running"], false);
    }
    assert!(f.requests.lock().await.is_empty());
}

#[tokio::test]
async fn duplicate_selections_run_once_and_a_failed_item_does_not_hide_other_results() {
    let f = Fixture::new().await;
    let (status, accepted) = f
        .submit(json!({"item_ids":["101","101","202"],"reason":"缺失媒体信息"}))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let result = f.result(accepted["task_id"].as_str().unwrap()).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result_status"], 207);
    assert_eq!(result["result"]["selected_count"], 2);
    assert_eq!(result["result"]["updated"], 0);
    assert_eq!(result["result"]["failed"], 2);
    assert_eq!(result["result"]["items"][0]["item_id"], "101");
    assert_eq!(result["result"]["items"][1]["item_id"], "202");
    assert_eq!(result["result"]["failures"].as_array().unwrap().len(), 2);
    assert_eq!(*f.requests.lock().await, vec!["101", "202"]);
}
