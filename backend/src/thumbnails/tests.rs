use super::*;
use crate::{auth::SessionSigner, jobs::JobManager, logs::LogStore};
use axum::{Json, Router, routing::get};
use std::{collections::HashMap, sync::Arc};
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
