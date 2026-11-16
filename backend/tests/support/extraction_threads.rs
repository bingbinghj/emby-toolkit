use crate::{AppState, auth::SessionSigner, logs::LogStore};
use axum::{
    Json, Router,
    body::Bytes,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::RwLock;

pub struct Fixture {
    pub state: AppState,
    pub calls: Arc<AtomicUsize>,
    pub bodies: Arc<tokio::sync::Mutex<Vec<Bytes>>>,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}
impl Fixture {
    pub async fn new() -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let bodies = Arc::new(tokio::sync::Mutex::new(vec![]));
        let received_bodies = bodies.clone();
        let router = Router::new()
            .route("/web/ConfigurationPages", get(|| async {
                Json(json!([{"Name":"MediaInfoExtractPageView","Href":"?PageId=fixture:MediaInfoExtractPageView"}]))
            }))
            .route("/UI/View", get(|| async {
                Json(json!({"EditObjectContainer":{"Object":{"GeneralOptions":{
                    "MaxConcurrentCount":1,"Tier2MaxConcurrentCount":2,"CooldownDurationSeconds":1
                }}}}))
            }))
            .route("/emby/Items/SyncMediaInfo", post(move |body: Bytes| {
                let received = received.clone();
                let received_bodies = received_bodies.clone();
                async move {
                    received.fetch_add(1, Ordering::SeqCst);
                    received_bodies.lock().await.push(body.clone());
                    if body.as_ref() == b"null" {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Object reference not set to an instance of an object.").into_response();
                    }
                    Json(json!([{"MediaSources":[]}])).into_response()
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = AppState {
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
            jobs: Arc::default(),
            logs: LogStore::open(&root.join("logs")).unwrap(),
            p115: Arc::default(),
            strm: Arc::default(),
            douban: Arc::default(),
            docker: Arc::default(),
        };
        Self {
            state,
            calls,
            bodies,
            server,
            _directory: directory,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
