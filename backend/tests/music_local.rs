//! Real local audio through the Rust API and the standalone music worker.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use emby_toolkit::{AppState, auth::SessionSigner, logs::LogStore};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn request(app: &Router, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn finished(state: &AppState, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let value = state.jobs.result(id).await.unwrap();
            if value["is_running"] == false {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the Python dependencies from backend/music_engine/requirements.txt"]
async fn local_music_scan_preview_write_and_cleanup_preserve_seed() {
    let directory = tempfile::tempdir().unwrap();
    let seed = directory.path().join("seed");
    let library = directory.path().join("library");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::create_dir_all(&library).unwrap();
    let python = std::env::var("ETK_MUSIC_PYTHON").unwrap_or("python3".into());
    let output=std::process::Command::new(python).args(["-B","-c", "import sys,wave; p=wave.open(sys.argv[1],'wb'); p.setnchannels(2); p.setsampwidth(2); p.setframerate(44100); p.writeframes(bytes(176400)); p.close()"])
        .arg(seed.join("source.wav")).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = std::fs::read(seed.join("source.wav")).unwrap();
    std::fs::hard_link(seed.join("source.wav"), library.join("song.wav")).unwrap();
    let db = sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap();
    db.close().await;
    let state = AppState {
        db,
        config: Arc::new(RwLock::new(
            json!({"music_library_path":library,"music_seed_path":seed,
            "music_sources":["musicbrainz"],"music_cue_enable":true,"music_metadata_to_simplified":false,
            "music_scraping_policies":{"tags":"overwrite","cover":"skip","lyrics":"skip"}}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(directory.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: directory.path().join("config"),
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
    let app = emby_toolkit::music::router().with_state(state.clone());
    let (status, initial) = request(&app, "GET", "/api/music/library", json!({})).await;
    assert_eq!(status, 200, "{initial}");
    assert_eq!(initial["total"], 0);
    let (status, job) = request(&app, "POST", "/api/music/jobs/scan", json!({})).await;
    assert_eq!(status, 202, "{job}");
    let result = finished(&state, job["task_id"].as_str().unwrap()).await;
    assert_eq!(result["result"]["failed"], 0, "{result}");
    let (_, rows) = request(&app, "GET", "/api/music/library", json!({})).await;
    assert_eq!(rows["total"], 1, "{rows}");
    let (_,job)=request(&app,"POST","/api/music/jobs/preview",json!({"operation":"edit","paths":["song.wav"],"metadata":{"title":"测试曲名","artists":["测试歌手"],"album":"测试专辑"}})).await;
    let id = job["task_id"].as_str().unwrap();
    let preview = finished(&state, id).await;
    assert_eq!(
        preview["result"]["preview"]["items"][0]["success"], true,
        "{preview}"
    );
    let (_, job) = request(
        &app,
        "POST",
        "/api/music/jobs/execute",
        json!({"preview_task_id":id}),
    )
    .await;
    let result = finished(&state, job["task_id"].as_str().unwrap()).await;
    assert_eq!(result["result"]["failed"], 0, "{result}");
    assert_eq!(std::fs::read(seed.join("source.wav")).unwrap(), original);
    assert_ne!(std::fs::read(library.join("song.wav")).unwrap(), original);
    let (_, data) = request(
        &app,
        "POST",
        "/api/music/query/inspect",
        json!({"path":"song.wav"}),
    )
    .await;
    assert_eq!(data["title"], "测试曲名", "{data}");
    assert_eq!(std::fs::read_dir(&library).unwrap().count(), 1);
    let receipts = state.data_dir.join("music/operations");
    assert!(std::fs::read_dir(receipts).unwrap().all(|item| {
        item.unwrap()
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
    }));
    let (status, _) = request(
        &app,
        "POST",
        "/api/music/query/inspect",
        json!({"path":"../seed/source.wav"}),
    )
    .await;
    assert_eq!(status, 400);
}
