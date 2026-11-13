use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::SessionSigner, logs::LogStore, p115};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[tokio::test]
async fn oauth_token_storage_failure_is_reported_without_caching_success() {
    let exchanges = Arc::new(AtomicUsize::new(0));
    let exchanged = exchanges.clone();
    let upstream = Router::new()
        .route(
            "/open/authDeviceCode",
            post(|| async {
                Json(json!({"state":true,"data":{
                    "uid":"fixture","time":"1","sign":"fixture","qrcode":"fixture"
                }}))
            }),
        )
        .route(
            "/get/status/",
            get(|| async { Json(json!({"state":true,"data":{"status":2}})) }),
        )
        .route(
            "/open/deviceCodeToToken",
            post(move || {
                let exchanged = exchanged.clone();
                async move {
                    exchanged.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"state":true,"data":{
                        "access_token":"fixture-access","refresh_token":"fixture-refresh"
                    }}))
                }
            }),
        )
        .route(
            "/open/user/info",
            get(|| async { Json(json!({"state":true,"data":{"user_id":"fixture"}})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap();
    pool.close().await;
    let state = AppState {
        db: pool,
        config: Arc::new(RwLock::new(json!({"p115_app_id":"fixture"}))),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(directory.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: directory.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: LogStore::open(&directory.path().join("logs")).unwrap(),
        p115: Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            qrcode: base.clone(),
            passport: base.clone(),
            openapi: base.clone(),
            webapi: base.clone(),
            profile: base,
        })),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    let router = p115::router().with_state(state);
    let response = router
        .clone()
        .oneshot(
            Request::post("/api/p115/qrcode")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for _ in 0..2 {
        let response = router
            .clone()
            .oneshot(
                Request::get("/api/p115/qrcode/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(body["success"], false, "{body}");
        assert_eq!(body["status"], "error", "{body}");
        assert!(!body.to_string().contains("fixture-access"));
        assert!(!body.to_string().contains("fixture-refresh"));
    }
    assert_eq!(
        exchanges.load(Ordering::SeqCst),
        2,
        "failed storage must not cache authorization success"
    );
    server.abort();
}
