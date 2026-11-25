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

#[test]
fn music_rename_defaults_off_and_only_accepts_boolean() {
    let defaults: Value = serde_json::from_str(include_str!("../assets/config_defaults.json")).unwrap();
    assert_eq!(defaults["music_rename_on_save"], false);
    for value in [json!(true), json!(false)] {
        assert!(emby_toolkit::music::validate_config(&json!({"music_rename_on_save":value})).is_ok());
    }
    for value in [json!("false"), json!(null), json!(0)] {
        assert!(emby_toolkit::music::validate_config(&json!({"music_rename_on_save":value})).is_err());
    }
}

#[test]
fn music_search_default_only_accepts_supported_entities() {
    for kind in ["album", "recording"] {
        assert!(emby_toolkit::music::validate_config(&json!({"music_search_type":kind})).is_ok());
    }
    for kind in [json!("artist_collection"), json!(""), json!(null), json!(false)] {
        assert!(emby_toolkit::music::validate_config(&json!({"music_search_type":kind})).is_err());
    }
}

#[test]
fn automatic_music_alignment_rejects_sequential_and_unknown_modes() {
    for mode in ["strict", "fallback_ignore_duration", "ignore_duration"] {
        assert!(emby_toolkit::music::validate_config(&json!({"music_track_match":mode})).is_ok());
    }
    for mode in [json!("sequential"), json!("auto"), json!(false), json!([])] {
        assert!(emby_toolkit::music::validate_config(&json!({"music_track_match":mode})).is_err());
    }
    assert!(emby_toolkit::music::validate_config(&json!({})).is_ok());
}

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
    let output=std::process::Command::new(python).args(["-B","-c", "import sys,wave; from mutagen.wave import WAVE; from mutagen.id3 import COMM,TXXX; p=wave.open(sys.argv[1],'wb'); p.setnchannels(2); p.setsampwidth(2); p.setframerate(44100); p.writeframes(bytes(176400)); p.close(); a=WAVE(sys.argv[1]); a.add_tags(); a.tags.add(COMM(encoding=3,lang='eng',desc='',text=['Original comment'])); a.tags.add(TXXX(encoding=3,desc='DESCRIPTION',text=['Original description'])); a.save()"])
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
    let app = emby_toolkit::music::router()
        .merge(emby_toolkit::tasks::router())
        .with_state(state.clone());
    // 试听为只读，支持 Range/HEAD；不能读取库外文件或非音频旁挂文件。
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/music/audio?path=song.wav")
                .header("range", "bytes=4-15")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 206);
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes 4-15/{}", original.len())
    );
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap().as_ref(),
        &original[4..16]
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/api/music/audio?path=song.wav")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.headers()["content-length"],
        original.len().to_string()
    );
    assert!(to_bytes(response.into_body(), 1).await.unwrap().is_empty());
    for path in ["../seed/source.wav", "/etc/passwd", "song.txt"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/music/audio?path={path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
    }
    assert_eq!(std::fs::read(seed.join("source.wav")).unwrap(), original);
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
    // Changing automatic alignment invalidates the old preview before any write.
    state.config.write().await["music_track_match"] = json!("ignore_duration");
    let (_, stale) = request(&app, "POST", "/api/music/jobs/execute", json!({"preview_task_id":id})).await;
    let rejected = finished(&state, stale["task_id"].as_str().unwrap()).await;
    assert_eq!(rejected["state"], "failed", "{rejected}");
    assert!(rejected.to_string().contains("音乐刮削设置已变化"), "{rejected}");
    assert_eq!(std::fs::read(library.join("song.wav")).unwrap(), original);
    state.config.write().await.as_object_mut().unwrap().remove("music_track_match");
    // A rename toggle after preview must not silently change the confirmed paths.
    state.config.write().await["music_rename_on_save"] = json!(true);
    let (_, stale) = request(&app, "POST", "/api/music/jobs/execute", json!({"preview_task_id":id})).await;
    let rejected = finished(&state, stale["task_id"].as_str().unwrap()).await;
    assert_eq!(rejected["state"], "failed", "{rejected}");
    assert!(rejected.to_string().contains("音乐刮削设置已变化"), "{rejected}");
    assert_eq!(std::fs::read(library.join("song.wav")).unwrap(), original);
    state.config.write().await["music_rename_on_save"] = json!(false);
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
    // No existing sidecar: apply lyrics, inspect preview, then explicitly save.
    let mut lyrics_metadata=data.clone();
    lyrics_metadata["lyrics"]=json!("[00:01.00]新增歌词\n");
    let (status,job)=request(&app,"POST","/api/music/jobs/preview",json!({"operation":"edit","paths":["song.wav"],"metadata":lyrics_metadata})).await;
    assert_eq!(status,202,"{job}");
    let preview_id=job["task_id"].as_str().unwrap();
    let preview=finished(&state,preview_id).await;
    assert_eq!(preview["result"]["preview"]["items"][0]["success"],true,"{preview}");
    assert!(!library.join("song.lrc").exists());
    let (status,job)=request(&app,"POST","/api/music/jobs/execute",json!({"preview_task_id":preview_id})).await;
    assert_eq!(status,202,"{job}");
    let result=finished(&state,job["task_id"].as_str().unwrap()).await;
    assert_eq!(result["result"]["failed"],0,"{result}");
    assert_eq!(std::fs::read_to_string(library.join("song.lrc")).unwrap(),"[00:01.00]新增歌词\n");
    assert_eq!(std::fs::read(seed.join("source.wav")).unwrap(),original);

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
    // 任务中心批量清空复用音乐持久任务，做种内容和其它标签必须保持不变。
    std::fs::hard_link(seed.join("source.wav"), library.join("linked.wav")).unwrap();
    for context in ["manual", "all"] {
        let (_, catalog) = request(
            &app,
            "GET",
            &format!("/api/tasks/available?context={context}"),
            json!({}),
        )
        .await;
        assert!(
            catalog
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["key"] == "clear-music-comments")
        );
    }
    for repeat in 0..2 {
        let (status, job) = request(
            &app,
            "POST",
            "/api/tasks/run",
            json!({"task_name":"clear-music-comments"}),
        )
        .await;
        assert_eq!(status, 202, "{job}");
        let result = finished(&state, job["task_id"].as_str().unwrap()).await;
        assert_eq!(result["result"]["failed"], 0, "{result}");
        assert_eq!(result["result"]["items"].as_array().unwrap().len(), 2);
        assert_eq!(result["result"]["skipped"], if repeat == 0 { 0 } else { 2 });
        assert_eq!(std::fs::read(seed.join("source.wav")).unwrap(), original);
        let (_, song) = request(
            &app,
            "POST",
            "/api/music/query/inspect",
            json!({"path":"song.wav"}),
        )
        .await;
        assert!(song["comment"].is_null() && song["description"].is_null());
        assert_eq!(song["title"], "测试曲名");
        assert_eq!(song["artists"], json!(["测试歌手"]));
        let (_, linked) = request(
            &app,
            "POST",
            "/api/music/query/inspect",
            json!({"path":"linked.wav"}),
        )
        .await;
        assert!(linked["comment"].is_null() && linked["description"].is_null());
        assert_eq!(linked["link_count"], 1);
    }
}
