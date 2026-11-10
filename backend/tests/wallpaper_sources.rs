use axum::{Json, Router, routing::get};
use emby_toolkit::{AppState, auth::SessionSigner, logs::LogStore};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;

async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, task)
}

fn state(directory: &std::path::Path, config: Value) -> AppState {
    AppState {
        db: sqlx::PgPool::connect_lazy("postgres://unused:unused@127.0.0.1/unused").unwrap(),
        config: Arc::new(RwLock::new(config)),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(directory).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: directory.into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: LogStore::open(&directory.join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    }
}

#[tokio::test]
async fn emby_wallpapers_use_native_backdrops_without_forwarding_the_token() {
    let (upstream, mock) = server(Router::new().route("/Users/test/Items", get(|| async {
        Json(json!({"Items":[{"Id":"101","BackdropImageTags":["original-backdrop"]},{"Id":"102","ImageTags":{"Primary":"poster"}}]}))
    }))).await;
    let directory = tempfile::tempdir().unwrap();
    let app = state(
        directory.path(),
        json!({"emby_server_url":upstream,"emby_api_key":"private-key","emby_user_id":"test"}),
    );
    let (base, task) = server(emby_toolkit::wallpaper::router().with_state(app)).await;
    let response = reqwest::get(format!("{base}/api/wallpaper/emby"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["images"],
        json!(["/image_proxy/Items/101/Images/Backdrop/0?tag=original-backdrop&quality=90"])
    );
    assert!(!body.to_string().contains("private-key"));
    task.abort();
    mock.abort();
}

#[tokio::test]
async fn tmdb_wallpapers_use_the_existing_configured_provider() {
    let (upstream, mock) = server(Router::new().route("/movie/popular", get(|| async {
        Json(json!({"results":[{"backdrop_path":"/native-backdrop.jpg","poster_path":"/poster.jpg"}]}))
    }))).await;
    let directory = tempfile::tempdir().unwrap();
    let app = state(
        directory.path(),
        json!({"tmdb_api_base_url":upstream,"tmdb_api_key":"private-key"}),
    );
    let (base, task) = server(emby_toolkit::wallpaper::router().with_state(app)).await;
    let response = reqwest::get(format!("{base}/api/wallpaper/tmdb"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["images"],
        json!(["https://image.tmdb.org/t/p/original/native-backdrop.jpg"])
    );
    assert!(!body.to_string().contains("private-key"));
    task.abort();
    mock.abort();
}

#[tokio::test]
async fn empty_or_unconfigured_sources_return_an_error_instead_of_clearing_the_wallpaper() {
    let directory = tempfile::tempdir().unwrap();
    let app = state(directory.path(), json!({}));
    let (base, task) = server(emby_toolkit::wallpaper::router().with_state(app)).await;
    for source in ["tmdb", "emby", "unknown"] {
        let response = reqwest::get(format!("{base}/api/wallpaper/{source}"))
            .await
            .unwrap();
        assert!(!response.status().is_success());
        let body: Value = response.json().await.unwrap();
        assert!(body["error"].is_string());
        assert!(body.get("images").is_none());
    }
    task.abort();
}
