use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Path,
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use emby_toolkit::{AppState, auth::SessionSigner, logs::LogStore, media_edit};
use serde_json::{Map, Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Notify, RwLock};
use tower::ServiceExt;

const TARGET: &str = "61838";
const MEDIA_PATH: &str = "/media/Nexus/Season 01/Nexus S01E08.mkv";

fn episode_record() -> Value {
    json!({"target_ids":[TARGET],"targets":[{"id":TARGET,"path":MEDIA_PATH}],
        "observations":{TARGET:{"Id":TARGET,"Type":"Episode","Path":MEDIA_PATH}}})
}

fn native_items() -> Map<String, Value> {
    json!({
        TARGET:{"Id":TARGET,"Type":"Episode","Path":MEDIA_PATH,"ParentId":"61789","SeasonId":"61789","SeriesId":"61788"},
        "61789":{"Id":"61789","Type":"Season","ParentId":"61788","SeriesId":"61788"},
        "61788":{"Id":"61788","Type":"Series"}
    }).as_object().unwrap().clone()
}

struct Fixture {
    state: AppState,
    directory: tempfile::TempDir,
    native: Arc<RwLock<Map<String, Value>>>,
    reads: Arc<Mutex<Vec<String>>>,
    keys: Arc<Mutex<Vec<String>>>,
    blocked: Arc<AtomicBool>,
    arrived: Arc<Notify>,
    resume: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(record: Value) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let recovery = directory.path().join("backups/media-info-reload/scope");
        std::fs::create_dir_all(&recovery).unwrap();
        std::fs::write(recovery.join("media-reload.json"), record.to_string()).unwrap();
        // Display scope must also work for blocked recovery materials. Keeping
        // settings invalid isolates this contract from recovery POST/DB work.
        std::fs::write(recovery.join("original-settings.json"), "{}").unwrap();
        let native = Arc::new(RwLock::new(native_items()));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let keys = Arc::new(Mutex::new(Vec::new()));
        let blocked = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let source = native.clone();
        let observed = reads.clone();
        let received_keys = keys.clone();
        let held = blocked.clone();
        let started = arrived.clone();
        let released = resume.clone();
        let app = Router::new().route(
            "/Users/user/Items/{id}",
            get(move |Path(id): Path<String>, headers: HeaderMap| {
                let source = source.clone();
                let observed = observed.clone();
                let received_keys = received_keys.clone();
                let held = held.clone();
                let started = started.clone();
                let released = released.clone();
                async move {
                    observed.lock().unwrap().push(id.clone());
                    received_keys
                        .lock()
                        .unwrap()
                        .push(headers["X-Emby-Token"].to_str().unwrap().to_string());
                    if id == TARGET && held.load(Ordering::SeqCst) {
                        started.notify_one();
                        released.notified().await;
                    }
                    match source.read().await.get(&id).cloned() {
                        Some(item) => Json(item).into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap();
        pool.close().await;
        let state = AppState {
            db: pool,
            config: Arc::new(RwLock::new(
                json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user"}),
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
        Self {
            state,
            directory,
            native,
            reads,
            keys,
            blocked,
            arrived,
            resume,
            server,
        }
    }

    fn material(&self) -> std::path::PathBuf {
        self.directory
            .path()
            .join("backups/media-info-reload/scope/media-reload.json")
    }

    async fn status(&self) -> Value {
        status(self.state.clone()).await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn status(state: AppState) -> Value {
    let response = media_edit::router()
        .with_state(state)
        .oneshot(
            Request::get("/api/media_info/recovery")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let result: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(status, StatusCode::OK, "{result}");
    result
}

#[tokio::test]
async fn database_unavailable_keeps_local_scope_visible_and_blocks_recovery() {
    let f = Fixture::new(episode_record()).await;
    let before = std::fs::read(f.material()).unwrap();
    let result = f.status().await;
    let unknown = result["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["kind"] == "database-unavailable")
        .unwrap();
    assert_eq!(unknown["can_recover"], false);
    assert_eq!(unknown["target_ids"], json!([]));
    assert!(unknown["reason"].as_str().unwrap().contains("数据库"));
    scope(&result, json!([TARGET]), json!(["61788", "61789", TARGET]));
    let response = media_edit::router()
        .with_state(f.state.clone())
        .oneshot(
            Request::post("/api/media_info/recovery")
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"recovery_id":"media:scope"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(std::fs::read(f.material()).unwrap(), before);
    assert_ne!(f.state.jobs.status().await["is_running"], true);
}

#[tokio::test]
async fn database_unavailable_cannot_save_media_after_status_display() {
    let f = Fixture::new(episode_record()).await;
    let path = f.directory.path().join("episode.strm");
    let source = f.directory.path().join("episode.mkv");
    std::fs::write(&source, [0_u8; 1000]).unwrap();
    std::fs::write(&path, source.to_string_lossy().as_bytes()).unwrap();
    let original = json!([{"MediaSourceInfo":{"Container":"mkv","Size":1000,
        "MediaStreams":[{"Type":"Video","Codec":"h264","Width":1920,"Height":1080}]},"Chapters":[]}]);
    let sidecar = f.directory.path().join("episode-mediainfo.json");
    std::fs::write(&sidecar, original.to_string()).unwrap();
    f.state.config.write().await["local_strm_root"] = json!(f.directory.path());
    f.native.write().await.get_mut(TARGET).unwrap()["Path"] = json!(path);
    let result = f.status().await;
    assert!(
        result["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["kind"] == "database-unavailable")
    );
    let mut requested = original.clone();
    requested[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("不应保存");
    let response = media_edit::router()
        .with_state(f.state.clone())
        .oneshot(
            Request::post(format!("/api/media_info/edit/{TARGET}"))
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"mediainfo":requested}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let result: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    if status == StatusCode::ACCEPTED {
        let task = result["task_id"].as_str().unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let outcome = f.state.jobs.result(task).await.unwrap();
                if outcome["is_running"] == false {
                    break outcome;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(outcome["state"], "failed", "{outcome}");
        assert!(outcome["result_status"].as_u64().unwrap() >= 400);
    } else {
        assert!(status.is_server_error(), "{status}: {result}");
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(sidecar).unwrap()).unwrap(),
        original
    );
    assert_eq!(std::fs::read(source).unwrap(), [0_u8; 1000]);
}

fn scope(result: &Value, targets: Value, related: Value) {
    assert_eq!(result["pending"], true, "{result}");
    assert_eq!(
        result["records"][0]["target_ids"], targets,
        "real recovery targets must stay unchanged: {result}"
    );
    assert_eq!(
        result["records"][0]["related_item_ids"], related,
        "{result}"
    );
}

#[tokio::test]
async fn episode_recovery_is_visible_on_its_verified_season_and_series_without_expanding_restore_targets()
 {
    let f = Fixture::new(episode_record()).await;
    scope(
        &f.status().await,
        json!([TARGET]),
        json!(["61788", "61789", TARGET]),
    );
    assert_eq!(*f.reads.lock().unwrap(), vec![TARGET, "61789", "61788"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(f.material()).unwrap()).unwrap(),
        episode_record()
    );
}

#[tokio::test]
async fn legacy_observations_and_pending_scan_use_their_original_native_path() {
    for pending_scan in [false, true] {
        let mut record = episode_record();
        record.as_object_mut().unwrap().remove("targets");
        let f = Fixture::new(record.clone()).await;
        if pending_scan {
            std::fs::remove_file(f.material()).unwrap();
            std::fs::write(
                f.material().with_file_name("pending-scan.json"),
                json!({"ids":[TARGET],"verification":record}).to_string(),
            )
            .unwrap();
        }
        scope(
            &f.status().await,
            json!([TARGET]),
            json!(["61788", "61789", TARGET]),
        );
    }
}

#[tokio::test]
async fn invalid_target_or_ancestor_identity_keeps_the_scope_target_only() {
    for fault in [
        "path",
        "id",
        "missing",
        "season type",
        "series type",
        "parent chain",
        "season id",
    ] {
        let f = Fixture::new(episode_record()).await;
        {
            let mut items = f.native.write().await;
            match fault {
                "path" => items.get_mut(TARGET).unwrap()["Path"] = json!("/different/video.mkv"),
                "id" => items.get_mut(TARGET).unwrap()["Id"] = json!("different-id"),
                "missing" => {
                    items.remove(TARGET);
                }
                "season type" => items.get_mut("61789").unwrap()["Type"] = json!("Movie"),
                "series type" => items.get_mut("61788").unwrap()["Type"] = json!("Movie"),
                "parent chain" => {
                    items.get_mut("61789").unwrap()["ParentId"] = json!("different-series")
                }
                _ => items.get_mut(TARGET).unwrap()["SeasonId"] = json!("different-season"),
            }
        }
        scope(&f.status().await, json!([TARGET]), json!([TARGET]));
    }
}

#[tokio::test]
async fn conflicting_or_missing_original_paths_cannot_infer_ancestors() {
    for record in [
        json!({"target_ids":[TARGET]}),
        json!({"target_ids":[TARGET],"targets":[{"id":TARGET,"path":MEDIA_PATH}],"observations":{TARGET:{"Path":"/conflicting/video.mkv"}}}),
    ] {
        let f = Fixture::new(record).await;
        scope(&f.status().await, json!([TARGET]), json!([TARGET]));
        assert!(f.reads.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn season_and_additional_part_show_only_their_verified_parent_media() {
    for (kind, extra_type, parent, expected) in [
        ("Season", "", "61788", json!(["61788", TARGET])),
        ("Video", "AdditionalPart", "movie", json!([TARGET, "movie"])),
        ("Video", "", "movie", json!([TARGET])),
        ("Movie", "", "movie", json!([TARGET])),
    ] {
        let f = Fixture::new(episode_record()).await;
        let mut items = f.native.write().await;
        items.insert(TARGET.into(), json!({"Id":TARGET,"Type":kind,"ExtraType":extra_type,"Path":MEDIA_PATH,"ParentId":parent}));
        items.insert("movie".into(), json!({"Id":"movie","Type":"Movie"}));
        drop(items);
        scope(&f.status().await, json!([TARGET]), expected);
    }
}

#[tokio::test]
async fn a_connection_change_during_scope_reads_discards_the_verified_ancestors() {
    let f = Fixture::new(episode_record()).await;
    f.blocked.store(true, Ordering::SeqCst);
    let reading = tokio::spawn(status(f.state.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(2), f.arrived.notified())
        .await
        .unwrap();
    f.state.config.write().await["emby_api_key"] = json!("different-fixture");
    f.resume.notify_one();
    scope(&reading.await.unwrap(), json!([TARGET]), json!([TARGET]));
    assert!(
        f.keys.lock().unwrap().iter().all(|key| key == "fixture"),
        "scope requests must stay pinned to their initial connection"
    );
}

#[tokio::test]
async fn episodes_share_parent_reads_without_changing_the_verified_display_scope() {
    let next = "61839";
    let next_path = "/media/Nexus/Season 01/Nexus S01E09.mkv";
    let mut record = episode_record();
    record["target_ids"] = json!([TARGET, next]);
    record["targets"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":next,"path":next_path}));
    let f = Fixture::new(record).await;
    let mut items = f.native.write().await;
    let mut item = items[TARGET].clone();
    item["Id"] = json!(next);
    item["Path"] = json!(next_path);
    items.insert(next.into(), item);
    drop(items);
    scope(
        &f.status().await,
        json!([TARGET, next]),
        json!(["61788", "61789", TARGET, next]),
    );
    let reads = f.reads.lock().unwrap();
    assert_eq!(reads.len(), 4);
    assert_eq!(reads.iter().filter(|id| *id == "61789").count(), 1);
    assert_eq!(reads.iter().filter(|id| *id == "61788").count(), 1);
}

#[tokio::test]
async fn stalled_optional_ancestor_reads_do_not_hold_the_recovery_status_open() {
    let f = Fixture::new(episode_record()).await;
    f.blocked.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), f.status())
        .await
        .expect("optional scope reads must stay bounded");
    scope(&result, json!([TARGET]), json!([TARGET]));
}

#[tokio::test]
async fn an_old_verified_cleanup_marker_without_original_paths_stays_target_only() {
    let f = Fixture::new(episode_record()).await;
    std::fs::remove_dir_all(f.material().parent().unwrap()).unwrap();
    let marker = f
        .material()
        .parent()
        .unwrap()
        .with_file_name("scope.cleanup.json");
    std::fs::write(marker, json!({"kind":"verified-cleanup","version":1,"directory_name":"scope",
        "target_ids":[TARGET],"completed_at":chrono::Utc::now().to_rfc3339(),"conflict_backup_directories":[]}).to_string()).unwrap();
    scope(&f.status().await, json!([TARGET]), json!([TARGET]));
    assert!(f.reads.lock().unwrap().is_empty());
}
