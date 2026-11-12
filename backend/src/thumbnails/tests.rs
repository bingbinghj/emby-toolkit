use super::*;
use crate::{auth::SessionSigner, jobs::JobManager, logs::LogStore};
use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
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

fn intro_chapters() -> Value {
    // These positions are the two frames successfully extracted for the supplied Mebius episode.
    json!([
        {"StartPositionTicks":870646766_u64,"MarkerType":"IntroStart","ImageTag":"intro-start"},
        {"StartPositionTicks":1854477611_u64,"MarkerType":"IntroEnd","ImageTag":"intro-end"}
    ])
}

#[tokio::test]
async fn marker_only_images_are_verified_and_each_marker_is_required() {
    let directory = tempfile::tempdir().unwrap();
    let state = state(directory.path(), "http://127.0.0.1:1");
    let mut item = json!({"Chapters":intro_chapters()});
    assert!(!missing(&state, &item, &json!({})).await.unwrap());
    item["Chapters"][1]
        .as_object_mut()
        .unwrap()
        .remove("ImageTag");
    assert!(
        missing(&state, &item, &json!({})).await.unwrap(),
        "a missing IntroEnd image must remain missing"
    );
    item["Chapters"][0]
        .as_object_mut()
        .unwrap()
        .remove("ImageTag");
    item["Chapters"][1]["ImageTag"] = json!("intro-end");
    assert!(
        missing(&state, &item, &json!({})).await.unwrap(),
        "a missing IntroStart image must remain missing"
    );
}

#[tokio::test]
async fn ordinary_chapters_markers_and_empty_chapters_keep_image_verification() {
    let directory = tempfile::tempdir().unwrap();
    let state = state(directory.path(), "http://127.0.0.1:1");
    let mut item = json!({"Chapters":[{"MarkerType":"Chapter","ImageTag":"chapter"}]});
    assert!(!missing(&state, &item, &json!({})).await.unwrap());
    item["Chapters"]
        .as_array_mut()
        .unwrap()
        .extend(intro_chapters().as_array().unwrap().clone());
    assert!(!missing(&state, &item, &json!({})).await.unwrap());
    item["Chapters"][2]
        .as_object_mut()
        .unwrap()
        .remove("ImageTag");
    assert!(
        missing(&state, &item, &json!({})).await.unwrap(),
        "an ordinary chapter image cannot hide a missing marker image"
    );
    item["Chapters"] = json!([]);
    assert!(missing(&state, &item, &json!({})).await.unwrap());
    item["Chapters"] = json!([{"ImageTag":"legacy"}]);
    assert!(!missing(&state, &item, &json!({})).await.unwrap());
}

#[tokio::test]
async fn bif_verification_uses_thumbnail_set_and_rejects_invalid_responses() {
    let result = Arc::new(RwLock::new(json!({"Thumbnails":[]})));
    let response = result.clone();
    let router = Router::new().route(
        "/Items/1/ThumbnailSet",
        get(move || {
            let response = response.clone();
            async move { Json(response.read().await.clone()) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = state(directory.path(), &base);
    let item = json!({"Id":"1","Chapters":intro_chapters()});
    let options = json!({"ThumbnailImagesIntervalSeconds":10});
    assert!(
        missing(&state, &item, &options).await.unwrap(),
        "chapter images cannot substitute for a missing BIF"
    );
    *result.write().await = json!({"Thumbnails":[{"StartPositionTicks":0}]});
    assert!(!missing(&state, &item, &options).await.unwrap());
    *result.write().await = json!({});
    assert!(missing(&state, &item, &options).await.is_err());
    server.abort();
}

#[test]
fn end_markers_receive_the_same_safe_seek_repair_as_ordinary_chapters() {
    let item = json!({"Id":"1","Path":"/media/1.strm","RunTimeTicks":600000000_u64,
        "MediaSources":[{"ItemId":"1","Path":"/video/1.mkv"}],
        "Chapters":[{"MarkerType":"IntroEnd","StartPositionTicks":600000000_u64}]});
    let probe = json!({"format":{"duration":"60","format_name":"matroska"},"streams":[{"codec_type":"video","duration":"60"}]});
    assert_eq!(
        repair_rule(&item, &probe, "").unwrap().unwrap()["clamp_end"],
        true
    );
}

async fn uncertain_native_start(executes: bool, complete_log: bool) -> (Value, usize) {
    let original_log = "2026-11-12 00:00:00.000 Info App: previous native execution".to_string();
    let native = Arc::new(RwLock::new(
        json!({"Id":"extract","Key":"VideoThumbnailExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":"old","Status":"Completed"}}),
    ));
    let logs = Arc::new(RwLock::new(vec![original_log.clone()]));
    let starts = Arc::new(AtomicUsize::new(0));
    let tasks = native.clone();
    let lines = logs.clone();
    let submitted = starts.clone();
    let app = Router::new()
        .route("/ScheduledTasks", get(move || {
            let tasks = tasks.clone();
            async move { Json(json!([tasks.read().await.clone()])) }
        }))
        .route("/System/Logs/embyserver.txt/Lines", get(move || {
            let lines = lines.clone();
            async move { Json(json!({"Items":lines.read().await.clone()})) }
        }))
        .route("/ScheduledTasks/Running/extract", post(move || {
            let native = native.clone();
            let logs = logs.clone();
            let attempt = submitted.fetch_add(1, Ordering::SeqCst) + 1;
            let original_log = original_log.clone();
            async move {
                if executes {
                    native.write().await["LastExecutionResult"] = json!({"StartTimeUtc":format!("run-{attempt}"),"Status":"Completed"});
                    if complete_log {
                        *logs.write().await = vec![
                            "2026-11-12 00:00:03.000 Info App: VideoThumbnailExtract - Scheduled Task Complete".into(),
                            "2026-11-12 00:00:02.000 Info App: VideoThumbnailExtract - Number of items: 0".into(),
                            "2026-11-12 00:00:01.000 Info App: VideoThumbnailExtract - Scheduled Task Execute".into(),
                            original_log,
                        ];
                    }
                }
                if attempt == 1 { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::NO_CONTENT }
            }
        }).delete(|| async { StatusCode::NO_CONTENT }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = state(directory.path(), &base);
    let work = state.clone();
    let id = state
        .jobs
        .submit("缩略图单次启动验收", move |context| async move {
            let mut trace = ExtractionLog::default();
            let paths = run_with_trace(
                &work,
                Duration::from_millis(250),
                5,
                50,
                None,
                &context,
                &mut trace,
            )
            .await?;
            Ok(json!({"paths":paths,"message":"本次缩略图执行及日志已确认"}))
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let result = state.jobs.result(&id).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native start verification must finish");
    server.abort();
    (result, starts.load(Ordering::SeqCst))
}

#[tokio::test]
async fn native_start_accepted_with_a_lost_response_is_not_replayed() {
    let (result, starts) = uncertain_native_start(true, true).await;
    assert_eq!(
        starts, 1,
        "an accepted extraction start must not be replayed"
    );
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["paths"], json!([]));
}

#[tokio::test]
async fn native_start_unconfirmed_with_a_lost_response_is_not_replayed() {
    let (result, starts) = uncertain_native_start(false, false).await;
    assert_eq!(
        starts, 1,
        "an unconfirmed extraction start must not be replayed"
    );
    assert_eq!(result["state"], "failed", "{result}");
    assert!(
        result["message"].as_str().unwrap().contains("启动未确认"),
        "{result}"
    );
}

#[tokio::test]
async fn native_start_accepted_without_current_complete_logs_is_not_verified() {
    let (result, starts) = uncertain_native_start(true, false).await;
    assert_eq!(starts, 1, "a log failure must not replay the start");
    assert_eq!(result["state"], "failed", "{result}");
}
