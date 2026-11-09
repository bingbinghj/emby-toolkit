use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, media_extra};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};

fn native_guid(path: &std::path::Path) -> String {
    use md5::{Digest, Md5};
    let bytes = format!(
        "MediaBrowser.Controller.Entities.TV.Episode{}",
        path.display()
    )
    .encode_utf16()
    .flat_map(u16::to_le_bytes)
    .collect::<Vec<_>>();
    uuid::Uuid::from_bytes_le(Md5::digest(bytes).into()).to_string()
}
fn api_item(mut item: Value) -> Value {
    item.as_object_mut().unwrap().remove("Guid");
    item
}

#[derive(Clone)]
struct Host {
    item: Arc<Mutex<Value>>,
    settings: Arc<Mutex<Value>>,
    writes: Arc<Mutex<Vec<Value>>>,
    logs: Arc<Mutex<Vec<String>>>,
    events: Arc<Mutex<Vec<String>>>,
    mode: Arc<Mutex<String>>,
    started: Arc<AtomicBool>,
    release: Arc<Notify>,
    root: PathBuf,
    original: Value,
}
impl Host {
    async fn finish_capture(&self) {
        let mode = self.mode.lock().await.clone();
        let mut extracted = self.original.clone();
        extracted[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("Raw 提取覆盖");
        if mode != "native-only" {
            tokio::fs::write(
                self.root.join("e1-mediainfo.json"),
                serde_json::to_vec(&extracted).unwrap(),
            )
            .await
            .unwrap();
        }
        let mut item = self.item.lock().await;
        item["MediaStreams"] = extracted[0]["MediaSourceInfo"]["MediaStreams"].clone();
        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        item["ImageTags"]["Primary"] = json!("new-picture");
        drop(item);
        tokio::fs::write(self.root.join("e1-thumb.jpg"), b"new picture")
            .await
            .unwrap();
        if mode != "capture-log-error" {
            self.logs
                .lock()
                .await
                .push("RefreshItem Complete: e1 episode".into());
        }
    }
}
async fn api(State(host): State<Host>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if request.method() == axum::http::Method::GET {
        if path == "/Items/e1/Images/Primary" {
            return (
                [("Content-Type", "image/jpeg")],
                std::fs::read(host.root.join("e1-thumb.jpg")).unwrap(),
            )
                .into_response();
        }
        if path == "/Users/user/Items/e1/SpecialFeatures" {
            return Json(json!([])).into_response();
        }
        if path == "/Users/user/Items/e1" {
            if host.started.load(Ordering::SeqCst)
                && host.settings.lock().await["PersistMediaInfoMode"] == "Restore"
            {
                host.events.lock().await.push("read-in-restore".into());
                if *host.mode.lock().await == "restore-read-error"
                    && host.events.lock().await.contains(&"restore:e1".into())
                {
                    return StatusCode::BAD_REQUEST.into_response();
                }
            }
            return Json(api_item(host.item.lock().await.clone())).into_response();
        }
        if path == "/Items" {
            let item = host.item.lock().await.clone();
            let matches = query.get("Ids").is_none_or(|ids| {
                ids.split(',').any(|id| {
                    item["Id"] == id
                        || uuid::Uuid::parse_str(id).is_ok_and(|guid| {
                            uuid::Uuid::parse_str(item["Guid"].as_str().unwrap()) == Ok(guid)
                        })
                })
            });
            let items = if matches {
                vec![api_item(item)]
            } else {
                vec![]
            };
            return Json(json!({"Items":items,"TotalRecordCount":items.len()})).into_response();
        }
        if path == "/Items/e1/ThumbnailSet" {
            return Json(json!({"Thumbnails":[]})).into_response();
        }
        if path == "/System/Info" {
            return Json(
                json!({"Id":"fixture", "InternalMetadataPath":host.root.join("metadata")}),
            )
            .into_response();
        }
        if path == "/Library/VirtualFolders" {
            return Json(json!([{"ItemId":"library","Locations":[host.root]}])).into_response();
        }
        if path == "/ScheduledTasks" {
            return Json(json!([{"Id":"info","Key":"MediaInfoExtractTask","State":"Idle"}]))
                .into_response();
        }
        if path == "/web/ConfigurationPages" {
            return Json(json!([{"Name":"MediaInfoExtractPageView","Href":"/UI/View?PageId=fixture:MediaInfoExtractPageView"}])).into_response();
        }
        if path == "/UI/View" {
            return Json(
                json!({"EditObjectContainer":{"Object":host.settings.lock().await.clone()}}),
            )
            .into_response();
        }
        if path == "/System/Logs/embyserver.txt/Lines" {
            if query.get("Limit").map(String::as_str) == Some("0")
                && *host.mode.lock().await == "post-delete-log-error"
                && host.events.lock().await.contains(&"delete-primary".into())
            {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if query.get("Limit").map(String::as_str) != Some("0")
                && host.started.load(Ordering::SeqCst)
                && *host.mode.lock().await == "capture-log-error"
            {
                return StatusCode::BAD_REQUEST.into_response();
            }
            let lines = host.logs.lock().await.clone();
            let items: Vec<_> = if query.get("Limit").map(String::as_str) == Some("0") {
                vec![]
            } else {
                lines.iter().rev().cloned().collect()
            };
            return Json(json!({"Items":items,"TotalRecordCount":lines.len()})).into_response();
        }
    } else if request.method() == axum::http::Method::DELETE && path == "/Items/e1/Images/Primary" {
        host.events.lock().await.push("delete-primary".into());
        host.item.lock().await["ImageTags"]["Primary"] = Value::Null;
        return StatusCode::NO_CONTENT.into_response();
    } else if request.method() == axum::http::Method::POST {
        if path == "/Items/e1/Images/Primary" {
            let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap();
            tokio::fs::write(host.root.join("e1-thumb.jpg"), bytes)
                .await
                .unwrap();
            host.item.lock().await["ImageTags"]["Primary"] = json!("old-picture");
            host.events.lock().await.push("restore-primary".into());
            return StatusCode::NO_CONTENT.into_response();
        }
        if path == "/UI/Command" {
            let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let desired: Value = serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
            host.writes.lock().await.push(desired.clone());
            *host.settings.lock().await = desired;
            return StatusCode::NO_CONTENT.into_response();
        }
        if path == "/Items/e1/ClearMediaInfo" {
            assert_eq!(
                host.settings.lock().await["PersistMediaInfoMode"],
                "Restore"
            );
            host.events.lock().await.push("clear:e1".into());
            let mut item = host.item.lock().await;
            item["MediaStreams"] = json!([]);
            item["MediaSources"][0]["MediaStreams"] = json!([]);
            item["Chapters"] = json!([]);
            item["Size"] = json!(0);
            tokio::fs::remove_file(host.root.join("e1-mediainfo.json"))
                .await
                .unwrap();
            return StatusCode::NO_CONTENT.into_response();
        }
        if path == "/Items/e1/Refresh" {
            host.logs
                .lock()
                .await
                .push("RefreshItem Start: e1 episode".into());
            if query["ImageRefreshMode"] == "FullRefresh" {
                let settings = host.settings.lock().await.clone();
                assert_eq!(settings["EnableImageCapture"], true);
                assert!(
                    settings["ExclusiveControlFeatures"]
                        .as_str()
                        .unwrap()
                        .split(',')
                        .any(|flag| flag == "CatchAllAllow")
                );
                host.started.store(true, Ordering::SeqCst);
                if *host.mode.lock().await == "hold-capture" {
                    tokio::spawn(async move {
                        host.release.notified().await;
                        host.finish_capture().await;
                    });
                } else {
                    host.finish_capture().await;
                }
            } else {
                assert_eq!(query["ImageRefreshMode"], "ValidationOnly");
                assert_eq!(query["Recursive"], "false");
                assert_eq!(
                    host.settings.lock().await["PersistMediaInfoMode"],
                    "Restore"
                );
                host.events.lock().await.push("restore:e1".into());
                let info: Value = serde_json::from_slice(
                    &tokio::fs::read(host.root.join("e1-mediainfo.json"))
                        .await
                        .unwrap(),
                )
                .unwrap();
                let mut item = host.item.lock().await;
                item["MediaStreams"] = info[0]["MediaSourceInfo"]["MediaStreams"].clone();
                item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
                item["Chapters"] = info[0]["Chapters"].clone();
                item["Size"] = info[0]["MediaSourceInfo"]["Size"].clone();
                host.logs
                    .lock()
                    .await
                    .push("RefreshItem Complete: e1 episode".into());
            }
            return StatusCode::NO_CONTENT.into_response();
        }
        if path.starts_with("/ScheduledTasks/Running/") {
            panic!("image media info recovery must not run a library task");
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

struct Fixture {
    state: AppState,
    host: Host,
    admin: sqlx::PgPool,
    schema: String,
    temp: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    original_settings: Value,
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let admin = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("capture_guard_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let db = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |conn, _| {
                let q = search.clone();
                Box::pin(async move {
                    sqlx::query(&q).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("e1.strm");
        tokio::fs::write(&path, "/cloud/e1.mkv").await.unwrap();
        let original = json!([{"MediaSourceInfo":{"Container":"mkv","Size":1000,"RunTimeTicks":6000000000_u64,"MediaStreams":[{"Type":"Video","Codec":"h264","Index":0},{"Type":"Audio","Codec":"aac","Index":1,"Language":"jpn","Title":"Native 用户标题","Channels":2}]},"Chapters":[{"Name":"原片头","StartPositionTicks":0,"MarkerType":"IntroStart"}]}]);
        let mut local = original.clone();
        local[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("cache 旧标题");
        tokio::fs::write(
            temp.path().join("e1-mediainfo.json"),
            serde_json::to_vec(&local).unwrap(),
        )
        .await
        .unwrap();
        let mut old_picture = vec![];
        image::codecs::jpeg::JpegEncoder::new(&mut old_picture)
            .encode(&[100; 12], 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        tokio::fs::write(temp.path().join("e1-thumb.jpg"), &old_picture)
            .await
            .unwrap();
        tokio::fs::create_dir_all(temp.path().join("metadata"))
            .await
            .unwrap();
        let streams = original[0]["MediaSourceInfo"]["MediaStreams"].clone();
        let original_settings = json!({"ExclusiveControlFeatures":"CatchAllBlock,ExtractOnFileChange,Custom","EnableImageCapture":false,"PersistMediaInfoMode":"Default","LibraryScope":"library","MediaInfoJsonRootFolder":"","MediaInfoExtractionTimeoutMs":1000,"ImageExtractionTimeoutMultiplier":1});
        let host = Host {
            item: Arc::new(Mutex::new(
                json!({"Id":"e1","Guid":native_guid(&path),"PresentationUniqueKey":"episode-e1","Type":"Episode","Name":"分集","IndexNumber":1,"ParentIndexNumber":1,"Path":path,"Size":1000,"PartCount":1,"MediaStreams":streams,"MediaSources":[{"Id":"mediasource_e1","ItemId":"e1","Path":"/cloud/e1.mkv","MediaStreams":streams}],"Chapters":original[0]["Chapters"],"ImageTags":{"Primary":"old-picture"}}),
            )),
            settings: Arc::new(Mutex::new(original_settings.clone())),
            writes: Arc::default(),
            logs: Arc::new(Mutex::new(vec!["fixture log baseline".into()])),
            events: Arc::default(),
            mode: Arc::new(Mutex::new(mode.into())),
            started: Arc::default(),
            release: Arc::default(),
            root: temp.path().into(),
            original,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(api).with_state(host.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = AppState::new(db, temp.path().join("config")).await.unwrap();
        state.save_config(&json!({"emby_server_url":format!("http://{addr}"),"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":temp.path(),"emby_metadata_path":temp.path().join("metadata"),"monitor_sha1_pc_search":false})).await.unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('capture','Episode',$1,$2,$3,$3)").bind(json!(["e1"])).bind(json!([{"emby_item_id":"e1","path":path,"size_bytes":1000}])).bind(json!([null])).execute(&state.db).await.unwrap();
        Self {
            state,
            host,
            admin,
            schema,
            temp,
            server,
            original_settings,
        }
    }
    async fn start(&self) -> String {
        let state = self.state.clone();
        self.state
            .jobs
            .submit_response("capture regression", move |context| async move {
                media_extra::replace_images(&state, "e1", &context).await
            })
            .await
            .unwrap()
    }
    async fn wait(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(100), async {
            loop {
                let value = self.state.jobs.result(id).await.unwrap();
                if value["is_running"] == false {
                    return value;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    fn backup_root(&self) -> PathBuf {
        self.state.data_dir.join("backups/media-info-reload")
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
        drop(self.temp);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs are localhost"]
async fn capture_enables_images_then_restores_native_info_and_settings() {
    for mode in ["normal", "native-only"] {
        let f = Fixture::new(mode).await;
        let id = f.start().await;
        let result = f.wait(&id).await;
        let item = f.host.item.lock().await.clone();
        let settings = f.host.settings.lock().await.clone();
        let expected = f.original_settings.clone();
        let events = f.host.events.lock().await.clone();
        let clean = std::fs::read_dir(f.backup_root()).unwrap().next().is_none();
        let saved: Value = serde_json::from_slice(
            &tokio::fs::read(f.host.root.join("e1-mediainfo.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        f.close().await;
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(item["MediaStreams"][1]["Title"], "Native 用户标题");
        assert_eq!(
            saved[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
            "Native 用户标题"
        );
        assert_eq!(item["ImageTags"]["Primary"], "new-picture");
        assert_eq!(settings, expected);
        assert!(clean);
        assert!(events.contains(&"read-in-restore".into()));
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs are localhost"]
async fn unconfirmed_capture_keeps_config_and_original_backup() {
    let f = Fixture::new("capture-log-error").await;
    let id = f.start().await;
    let result = f.wait(&id).await;
    let settings = f.host.settings.lock().await.clone();
    let events = f.host.events.lock().await.clone();
    let root = std::fs::read_dir(f.backup_root())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let marked =
        root.join("image-capture.json").exists() && root.join("pending-scan.json").exists();
    let backup: Value =
        serde_json::from_slice(&std::fs::read(root.join("0.json")).unwrap()).unwrap();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(settings["EnableImageCapture"], true);
    assert!(
        settings["ExclusiveControlFeatures"]
            .as_str()
            .unwrap()
            .contains("CatchAllAllow")
    );
    assert!(marked);
    assert_eq!(
        backup[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
        "Native 用户标题"
    );
    assert!(!events.contains(&"clear:e1".into()));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs are localhost"]
async fn cancellation_after_capture_acceptance_finishes_media_restore_before_cleanup() {
    let f = Fixture::new("hold-capture").await;
    let id = f.start().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.host.started.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.state.jobs.control(Some(&id), "stop").await.unwrap();
    assert_eq!(f.host.settings.lock().await["EnableImageCapture"], true);
    f.host.release.notify_one();
    let result = f.wait(&id).await;
    let item = f.host.item.lock().await.clone();
    let settings = f.host.settings.lock().await.clone();
    let expected = f.original_settings.clone();
    let clean = std::fs::read_dir(f.backup_root()).unwrap().next().is_none();
    f.close().await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(item["MediaStreams"][1]["Title"], "Native 用户标题");
    assert_eq!(settings, expected);
    assert!(clean);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs are localhost"]
async fn restoration_readback_failure_keeps_restore_guard_and_backups() {
    let f = Fixture::new("restore-read-error").await;
    let id = f.start().await;
    let result = f.wait(&id).await;
    let settings = f.host.settings.lock().await.clone();
    let records = std::fs::read_dir(f.backup_root())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    let media_pending = records
        .iter()
        .any(|path| path.join("media-reload.json").is_file());
    let capture_backup = records
        .iter()
        .any(|path| path.join("manifest.json").is_file());
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(settings["PersistMediaInfoMode"], "Restore");
    assert!(
        settings["ExclusiveControlFeatures"]
            .as_str()
            .unwrap()
            .contains("IgnoreFileChange")
    );
    assert!(media_pending && capture_backup);
    let reason = result["result"]["error"].as_str().unwrap();
    assert!(
        reason.contains("HTTP 400")
            && reason.contains("/Users/user/Items/e1")
            && reason.contains("恢复说明"),
        "{reason}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs are localhost"]
async fn failure_after_deleting_images_before_refresh_restores_original_primary_and_file() {
    let f = Fixture::new("post-delete-log-error").await;
    let before = tokio::fs::read(f.host.root.join("e1-thumb.jpg"))
        .await
        .unwrap();
    let id = f.start().await;
    let result = f.wait(&id).await;
    let restored = tokio::fs::read(f.host.root.join("e1-thumb.jpg"))
        .await
        .unwrap();
    let primary = f.host.item.lock().await["ImageTags"]["Primary"].clone();
    let events = f.host.events.lock().await.clone();
    let settings = f.host.settings.lock().await.clone();
    let expected = f.original_settings.clone();
    let clean = std::fs::read_dir(f.backup_root()).unwrap().next().is_none();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(restored, before);
    assert_eq!(primary, "old-picture");
    assert!(events.contains(&"restore-primary".into()));
    assert!(!events.contains(&"clear:e1".into()) && !events.contains(&"restore:e1".into()));
    assert_eq!(settings, expected);
    assert!(clean);
}
