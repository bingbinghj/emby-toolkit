use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use base64::Engine;
use emby_toolkit::{AppState, media_images};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Notify, Semaphore};
use tower::ServiceExt;

fn jpeg(color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(2, 2, image::Rgb(color));
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode_image(&image)
        .unwrap();
    bytes
}

#[derive(Clone)]
struct Host {
    item: Value,
    replacement: Vec<u8>,
    native: Arc<Mutex<Vec<u8>>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    downloading: Arc<Notify>,
    deleted: Arc<Notify>,
    download_gate: Arc<Semaphore>,
    delete_gate: Arc<Semaphore>,
    fail_raw_upload: Arc<Mutex<bool>>,
    fail_all_uploads: Arc<Mutex<bool>>,
}
async fn picture(State(host): State<Host>) -> Response {
    host.events.lock().await.push("download");
    host.downloading.notify_one();
    host.download_gate.acquire().await.unwrap().forget();
    ([("Content-Type", "image/jpeg")], host.replacement).into_response()
}
async fn remove(State(host): State<Host>) -> StatusCode {
    host.events.lock().await.push("delete");
    host.native.lock().await.clear();
    host.deleted.notify_one();
    host.delete_gate.acquire().await.unwrap().forget();
    StatusCode::NO_CONTENT
}
async fn upload(State(host): State<Host>, request: Request<Body>) -> Response {
    host.events.lock().await.push("upload");
    let body = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let decoded = base64::engine::general_purpose::STANDARD.decode(&body);
    if *host.fail_all_uploads.lock().await || decoded.is_err() && *host.fail_raw_upload.lock().await
    {
        return (StatusCode::SERVICE_UNAVAILABLE, [("Retry-After", "1")]).into_response();
    }
    let Ok(bytes) = decoded else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    image::load_from_memory(&bytes).unwrap();
    *host.native.lock().await = bytes;
    StatusCode::NO_CONTENT.into_response()
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    host: Host,
    server: tokio::task::JoinHandle<()>,
    directory: tempfile::TempDir,
    base: String,
    original: Vec<u8>,
}
impl Fixture {
    async fn new(block_download: bool) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("image_cancel_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let path = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&path).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("movie.strm");
        tokio::fs::write(&path, "/fixture/movie.mkv").await.unwrap();
        let original = jpeg([255, 0, 0]);
        tokio::fs::write(directory.path().join("poster.jpg"), &original)
            .await
            .unwrap();
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        let host = Host {
            item: json!({"Id":"movie","Type":"Movie","Path":path}),
            replacement: jpeg([0, 0, 255]),
            native: Arc::new(Mutex::new(original.clone())),
            events: Arc::default(),
            downloading: Arc::default(),
            deleted: Arc::default(),
            download_gate: Arc::new(Semaphore::new(usize::from(!block_download))),
            delete_gate: Arc::new(Semaphore::new(usize::from(block_download))),
            fail_raw_upload: Arc::default(),
            fail_all_uploads: Arc::default(),
        };
        let app = Router::new()
            .route("/picture", get(picture))
            .route(
                "/Users/user/Items/movie",
                get(|State(host): State<Host>| async move { Json(host.item) }),
            )
            .route("/Items/movie/Images/Primary", delete(remove).post(upload))
            .route(
                "/Items/movie/Refresh",
                post(|State(host): State<Host>| async move {
                    host.events.lock().await.push("refresh");
                    StatusCode::NO_CONTENT
                }),
            )
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state
            .save_config(
                &json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user"}),
            )
            .await
            .unwrap();
        Self {
            state,
            admin,
            schema,
            host,
            server,
            directory,
            base,
            original,
        }
    }
    async fn submit(&self) -> String {
        let response = media_images::router()
            .with_state(self.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/update_media_image/movie")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        json!({"image_type":"poster","image_url":format!("{}/picture",self.base)})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let data: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        data["task_id"].as_str().unwrap().to_string()
    }
    async fn finished(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let result = self.state.jobs.result(task).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and image source are localhost"]
async fn cancellation_during_download_preserves_local_and_native_images() {
    let f = Fixture::new(true).await;
    let task = f.submit().await;
    tokio::time::timeout(Duration::from_secs(5), f.host.downloading.notified())
        .await
        .unwrap();
    f.state.jobs.control(Some(&task), "stop").await.unwrap();
    f.host.download_gate.add_permits(1);
    let result = f.finished(&task).await;
    let local = tokio::fs::read(f.directory.path().join("poster.jpg"))
        .await
        .unwrap();
    let native = f.host.native.lock().await.clone();
    let events = f.host.events.lock().await.clone();
    let original = f.original.clone();
    f.close().await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(local, original);
    assert_eq!(native, original);
    assert_eq!(events, vec!["download"]);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and image source are localhost"]
async fn cancellation_after_native_delete_finishes_image_commit_and_keeps_result() {
    let f = Fixture::new(false).await;
    let task = f.submit().await;
    tokio::time::timeout(Duration::from_secs(5), f.host.deleted.notified())
        .await
        .unwrap();
    assert!(f.host.native.lock().await.is_empty());
    f.state.jobs.control(Some(&task), "stop").await.unwrap();
    f.host.delete_gate.add_permits(1);
    let result = f.finished(&task).await;
    let local = tokio::fs::read(f.directory.path().join("poster.jpg"))
        .await
        .unwrap();
    let native = f.host.native.lock().await.clone();
    let replacement = f.host.replacement.clone();
    let events = f.host.events.lock().await.clone();
    f.close().await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(result["result_status"], 200);
    assert_eq!(result["result"]["message"], "poster 替换成功！");
    assert_eq!(local, replacement);
    assert_eq!(native, replacement);
    assert_eq!(
        events,
        vec!["download", "delete", "upload", "upload", "refresh"]
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and image source are localhost"]
async fn exhausted_raw_upload_still_tries_base64_and_finishes_refresh() {
    let f = Fixture::new(false).await;
    *f.host.fail_raw_upload.lock().await = true;
    f.host.delete_gate.add_permits(1);
    let task = f.submit().await;
    let result = f.finished(&task).await;
    let native = f.host.native.lock().await.clone();
    let events = f.host.events.lock().await.clone();
    let replacement = f.host.replacement.clone();
    f.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(native, replacement);
    assert_eq!(events.iter().filter(|event| **event == "upload").count(), 7);
    assert_eq!(events.last(), Some(&"refresh"));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and image source are localhost"]
async fn exhausted_uploads_still_refresh_saved_local_image_and_keep_failure() {
    let f = Fixture::new(false).await;
    *f.host.fail_all_uploads.lock().await = true;
    f.host.delete_gate.add_permits(1);
    let task = f.submit().await;
    let result = f.finished(&task).await;
    let local = tokio::fs::read(f.directory.path().join("poster.jpg"))
        .await
        .unwrap();
    let events = f.host.events.lock().await.clone();
    let replacement = f.host.replacement.clone();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(
        result["result"]["error"].as_str().unwrap().contains("503"),
        "{result}"
    );
    assert_eq!(local, replacement);
    assert_eq!(events.last(), Some(&"refresh"));
}
