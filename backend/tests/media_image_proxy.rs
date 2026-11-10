use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use emby_toolkit::{AppState, auth::SessionSigner, logs::LogStore, media_images};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn proxy_response(upstream_status: StatusCode) -> axum::response::Response {
    let upstream = Router::new().route(
        "/Items/movie/Images/Primary",
        get(move || async move {
            (
                upstream_status,
                [("Content-Type", "image/jpeg")],
                b"native-emby-image".to_vec(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(
            json!({"emby_server_url":base,"emby_api_key":"fixture"}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(directory.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: directory.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: LogStore::open(&directory.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    let response = media_images::router()
        .with_state(state)
        .oneshot(
            Request::builder()
                .uri("/image_proxy/Items/movie/Images/Primary?tag=original")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    server.abort();
    response
}

#[tokio::test]
async fn missing_or_failed_emby_art_is_an_error_instead_of_a_successful_transparent_image() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::FORBIDDEN,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let response = proxy_response(status).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "upstream {status}"
        );
    }
}

#[tokio::test]
async fn actual_emby_image_bytes_and_content_type_are_preserved() {
    let response = proxy_response(StatusCode::OK).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["Content-Type"], "image/jpeg");
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .as_ref(),
        b"native-emby-image"
    );
}
