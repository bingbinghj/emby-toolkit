use super::*;
use crate::{auth::SessionSigner, jobs::JobManager, logs::LogStore};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    routing::get,
};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicUsize},
};
use tokio::sync::RwLock;

#[derive(Clone)]
struct ImageFixture {
    live: Value,
    jpeg: Arc<RwLock<Option<Vec<u8>>>>,
    bif: Vec<u8>,
    native_bif: Arc<RwLock<Vec<u8>>>,
    reads: Arc<AtomicUsize>,
}

fn jpeg(color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(32, 18, image::Rgb(color));
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
        .encode_image(&image)
        .unwrap();
    bytes
}

async fn fixture() -> (
    tempfile::TempDir,
    AppState,
    ImageFixture,
    SavedTarget,
    tokio::task::JoinHandle<()>,
) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("episode.mkv");
    let original = jpeg([100, 150, 200]);
    let mut bif = vec![0; 80];
    bif[..8].copy_from_slice(b"\x89BIF\r\n\x1a\n");
    bif[12..16].copy_from_slice(&1_u32.to_le_bytes());
    bif[16..24].copy_from_slice(&1000_u64.to_le_bytes());
    bif[68..72].copy_from_slice(&80_u32.to_le_bytes());
    bif[72..76].copy_from_slice(&u32::MAX.to_le_bytes());
    bif[76..80].copy_from_slice(&(80 + original.len() as u32).to_le_bytes());
    bif.extend_from_slice(&original);
    tokio::fs::write(path.with_extension("bif"), &bif)
        .await
        .unwrap();
    let live = json!({"Id":"1","Path":path,"Type":"Episode","Size":100,"RunTimeTicks":10_000_000,
        "Chapters":[{"Name":"原章节","StartPositionTicks":0,"MarkerType":"Chapter","ImageTag":"unchanged","ChapterIndex":0}],
        "MediaStreams":[{"Type":"Video","Index":0}],
        "MediaSources":[{"Id":"1","ItemId":"1","Path":path,"Size":100,"RunTimeTicks":10_000_000,
            "MediaStreams":[{"Type":"Video","Index":0}]}]});
    let f = ImageFixture {
        live: live.clone(),
        jpeg: Arc::new(RwLock::new(Some(original))),
        bif,
        native_bif: Arc::new(RwLock::new(
            tokio::fs::read(path.with_extension("bif")).await.unwrap(),
        )),
        reads: Arc::default(),
    };
    let app = Router::new()
        .route(
            "/Users/admin/Items/1",
            get(|State(f): State<ImageFixture>| async move { Json(f.live) }),
        )
        .route(
            "/Items/1/Images/Chapter/{index}",
            get(
                |State(f): State<ImageFixture>, AxumPath(index): AxumPath<u64>| async move {
                    f.reads.fetch_add(1, Ordering::SeqCst);
                    if index == 1 {
                        return (StatusCode::OK, jpeg([10, 20, 30]));
                    }
                    match f.jpeg.read().await.clone() {
                        Some(bytes) => (StatusCode::OK, bytes),
                        None => (StatusCode::NOT_FOUND, vec![]),
                    }
                },
            ),
        )
        .route(
            "/Videos/1/index.bif",
            get(|State(f): State<ImageFixture>| async move { f.native_bif.read().await.clone() }),
        )
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(
            json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin"}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root.path()).unwrap()),
        recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
        data_dir: root.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::new(JobManager::default()),
        logs: LogStore::open(&root.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    let target = SavedTarget {
        id: "1".into(),
        path: string(&live, "Path"),
        source: None,
        sha: None,
        asset: None,
        info: json!([{"Chapters":media_info::clean_chapters(&live["Chapters"]).unwrap(),"MediaSourceInfo":live["MediaSources"][0]}]),
        refresh_required: true,
    };
    (root, state, f, target, server)
}

#[tokio::test]
async fn final_verification_rejects_changed_jpeg_with_unchanged_tag_and_bif() {
    let (root, state, f, target, server) = fixture().await;
    let recovery = root.path().join("recovery");
    let images = chapter_images::capture(&state, &target.id, &f.live, Some(&recovery))
        .await
        .unwrap();
    let images = BTreeMap::from([("1".into(), images)]);
    let original = f.jpeg.read().await.clone().unwrap();
    let replacement = jpeg([7, 8, 9]);
    assert_ne!(original, replacement);
    assert!(image::load_from_memory(&replacement).is_ok());
    *f.jpeg.write().await = Some(replacement);
    let before = BTreeMap::from([("1".into(), f.live.clone())]);
    let native_bifs = BTreeMap::from([(
        "1".into(),
        vec![BifEvidence {
            path: Path::new(&target.path).with_extension("bif"),
            bytes: f.bif.clone(),
            frames: 1,
        }],
    )]);
    let context = JobContext::for_recovery(None).await;
    let result = verify_reloaded(
        &state,
        &[&target],
        &context,
        &before,
        &BTreeMap::new(),
        &native_bifs,
        &images,
    )
    .await;
    server.abort();
    assert!(
        result.is_err(),
        "a changed valid chapter JPEG must fail even when its ImageTag and the entire BIF remain unchanged"
    );
    assert_eq!(f.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        tokio::fs::read(root.path().join("episode.bif"))
            .await
            .unwrap(),
        f.bif
    );
}

#[tokio::test]
async fn noop_reload_refuses_a_missing_tagged_chapter_jpeg_without_native_writes() {
    let (root, state, f, target, server) = fixture().await;
    *f.jpeg.write().await = None;
    let context = JobContext::for_recovery(None).await;
    let result = reload(
        &state,
        &[target],
        &context,
        &AtomicBool::new(false),
        &root.path().join("recovery"),
    )
    .await;
    server.abort();
    assert!(
        result.is_err(),
        "a no-op save must not verify an unavailable original tagged chapter JPEG"
    );
    assert!(root.path().join("episode.bif").is_file());
}

#[tokio::test]
async fn recovery_load_keeps_original_jpeg_baseline_after_endpoint_changes() {
    let (root, state, f, target, server) = fixture().await;
    let recovery = root.path().join("recovery");
    let originals = chapter_images::capture(&state, &target.id, &f.live, Some(&recovery))
        .await
        .unwrap();
    let record = json!({"chapter_images":{"1":originals}});
    *f.jpeg.write().await = Some(jpeg([7, 8, 9]));
    let loaded = chapter_images::load(&recovery, &record, &BTreeSet::from(["1".into()]))
        .await
        .unwrap();
    let result = chapter_images::difference(&state, "1", &f.live, &loaded["1"]).await;
    server.abort();
    assert!(result.unwrap_err().1.contains("图片内容与备份不一致"));
    assert!(Path::new(record["chapter_images"]["1"][0]["backup"].as_str().unwrap()).is_file());
}

#[tokio::test]
async fn recovery_load_refuses_changed_jpeg_backup_and_retains_it() {
    let (root, state, f, target, server) = fixture().await;
    let recovery = root.path().join("recovery");
    let originals = chapter_images::capture(&state, &target.id, &f.live, Some(&recovery))
        .await
        .unwrap();
    let record = json!({"chapter_images":{"1":originals}});
    let backup = Path::new(record["chapter_images"]["1"][0]["backup"].as_str().unwrap());
    let changed = jpeg([7, 8, 9]);
    tokio::fs::write(backup, &changed).await.unwrap();
    let result = chapter_images::load(&recovery, &record, &BTreeSet::from(["1".into()])).await;
    server.abort();
    assert!(result.is_err());
    assert_eq!(tokio::fs::read(backup).await.unwrap(), changed);
}

#[tokio::test]
async fn duplicate_chapter_occurrences_each_require_an_image_but_share_one_endpoint_read() {
    let (_root, state, f, target, server) = fixture().await;
    let mut live = f.live.clone();
    live["Chapters"]
        .as_array_mut()
        .unwrap()
        .push(f.live["Chapters"][0].clone());
    let originals = chapter_images::capture(&state, &target.id, &live, None)
        .await
        .unwrap();
    assert_eq!(originals.len(), 2);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
    assert!(
        chapter_images::difference(&state, "1", &live, &originals)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.reads.load(Ordering::SeqCst), 2);
    live["Chapters"].as_array_mut().unwrap().pop();
    assert!(
        chapter_images::difference(&state, "1", &live, &originals)
            .await
            .unwrap()
            .is_some()
    );
    server.abort();
}

#[tokio::test]
async fn jpeg_verification_follows_reordered_chapters_and_current_native_indexes() {
    let (_root, state, f, target, server) = fixture().await;
    let mut live = f.live.clone();
    live["Chapters"].as_array_mut().unwrap().push(json!({
        "Name":"片尾","StartPositionTicks":2911691541_u64,"MarkerType":"IntroEnd",
        "ImageTag":"unchanged-marker","ChapterIndex":1
    }));
    let originals = chapter_images::capture(&state, &target.id, &live, None)
        .await
        .unwrap();
    let signature = media_info::signature(&live["Chapters"]).unwrap();
    live["Chapters"].as_array_mut().unwrap().reverse();
    live["Chapters"][1]["ChapterIndex"] = json!(9);
    assert_eq!(media_info::signature(&live["Chapters"]).unwrap(), signature);
    assert!(
        chapter_images::difference(&state, "1", &live, &originals)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.reads.load(Ordering::SeqCst), 4);
    server.abort();
}

#[tokio::test]
async fn noop_reload_verifies_unchanged_original_jpeg_and_bif() {
    let (root, state, f, target, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    let result = reload(
        &state,
        &[target],
        &context,
        &AtomicBool::new(false),
        &root.path().join("recovery"),
    )
    .await;
    server.abort();
    assert!(result.unwrap().is_empty());
    assert_eq!(f.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        tokio::fs::read(root.path().join("episode.bif"))
            .await
            .unwrap(),
        f.bif
    );
    assert!(!root.path().join("recovery").exists());
}

#[tokio::test]
async fn noop_reload_refuses_changed_native_bif_payload_with_original_time_index() {
    let (root, state, f, target, server) = fixture().await;
    let replacement = jpeg([7, 8, 9]);
    let mut changed = f.bif[..80].to_vec();
    changed[76..80].copy_from_slice(&(80 + replacement.len() as u32).to_le_bytes());
    changed.extend_from_slice(&replacement);
    assert_eq!(
        recovery_bif_times(&changed).unwrap(),
        recovery_bif_times(&f.bif).unwrap()
    );
    assert_ne!(changed, f.bif);
    *f.native_bif.write().await = changed;
    let context = JobContext::for_recovery(None).await;
    let result = reload(
        &state,
        &[target],
        &context,
        &AtomicBool::new(false),
        &root.path().join("recovery"),
    )
    .await;
    server.abort();
    assert!(result.unwrap_err().1.contains("原生时间索引或图片内容"));
    assert_eq!(
        tokio::fs::read(root.path().join("episode.bif"))
            .await
            .unwrap(),
        f.bif
    );
}
