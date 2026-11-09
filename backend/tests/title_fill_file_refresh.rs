use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, titles};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Host {
    root: PathBuf,
    db: sqlx::PgPool,
    items: Arc<Mutex<BTreeMap<String, Value>>>,
    clears: Arc<AtomicUsize>,
    native_starts: Arc<AtomicUsize>,
    config_writes: Arc<AtomicUsize>,
    refreshes: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
    fault: Arc<Mutex<String>>,
    drift: Arc<Mutex<String>>,
}
fn read_info(root: &Path, id: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root.join(format!("{id}-mediainfo.json"))).unwrap())
        .unwrap()
}
async fn import_json(h: &Host) {
    for (id, item) in h.items.lock().await.iter_mut() {
        let data = read_info(&h.root, id);
        item["MediaStreams"] = data[0]["MediaSourceInfo"]["MediaStreams"].clone();
        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        // A normal refresh preserves existing chapters and their image tags.
    }
}
async fn api(State(h): State<Host>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if request.method() == axum::http::Method::GET {
        if let Some(id) = path.strip_prefix("/Users/user/Items/") {
            // Change the first target after it was processed, before batch notification.
            if id == "10002"
                && query
                    .get("Fields")
                    .is_some_and(|fields| fields.contains("Name"))
            {
                match std::mem::take(&mut *h.drift.lock().await).as_str() {
                    "source" => {
                        tokio::fs::write(h.root.join("10001.strm"), "/cd2/replaced.mkv")
                            .await
                            .unwrap();
                    }
                    "sha" => {
                        sqlx::query("UPDATE media_metadata SET file_sha1_json=$1 WHERE emby_item_ids_json @> $2::jsonb")
                            .bind(json!([format!("{:040X}", 999)]))
                            .bind(json!(["10001"]))
                            .execute(&h.db)
                            .await
                            .unwrap();
                    }
                    "path" => {
                        h.items.lock().await.get_mut("10001").unwrap()["Path"] =
                            json!(h.root.join("replaced.strm"));
                    }
                    _ => {}
                }
            }
            return h
                .items
                .lock()
                .await
                .get(id)
                .cloned()
                .map(Json)
                .map(IntoResponse::into_response)
                .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response());
        }
        if path == "/Items" {
            let items = if query.get("Path").is_some_and(|p| Path::new(p) == h.root) {
                vec![json!({"Id":"directory","IsFolder":true,"Path":h.root})]
            } else {
                h.items
                    .lock()
                    .await
                    .values()
                    .filter(|item| {
                        query
                            .get("Ids")
                            .is_some_and(|ids| ids.split(',').any(|id| item["Id"] == id))
                            || query.get("Path").is_some_and(|p| item["Path"] == *p)
                    })
                    .cloned()
                    .collect()
            };
            return Json(json!({"Items":items,"TotalRecordCount":items.len()})).into_response();
        }
    } else if request.method() == axum::http::Method::POST {
        if path.ends_with("/Refresh") {
            h.refreshes.lock().await.push((path.clone(), query));
            let fault = h.fault.lock().await.clone();
            if fault == "denied" {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if fault == "delayed" {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    import_json(&h).await;
                });
            } else if fault != "no-import" {
                import_json(&h).await;
            }
            return StatusCode::NO_CONTENT.into_response();
        }
        if path.ends_with("/ClearMediaInfo") {
            h.clears.fetch_add(1, Ordering::SeqCst);
        } else if path.starts_with("/ScheduledTasks/Running/") {
            h.native_starts.fetch_add(1, Ordering::SeqCst);
        } else if path == "/UI/Command" {
            h.config_writes.fetch_add(1, Ordering::SeqCst);
        }
    }
    StatusCode::NOT_FOUND.into_response()
}
fn info(index: usize, unknown: bool) -> Value {
    let (codec, language, title) = match index % 3 {
        0 => ("aac", "jpn", "AAC.原生日语"),
        1 => ("ac3", "chi", "AC3.DVD粤语"),
        _ => ("flac", "chi", "FLAC.东和台配"),
    };
    json!([{"MediaSourceInfo":{"Container":"mkv","Size":1000+index,"RunTimeTicks":6000000000_u64+index as u64,"MediaStreams":[
        {"Type":"Video","Codec":if index%2==0{"h264"}else{"hevc"},"Index":0},
        {"Type":"Audio","Codec":codec,"Channels":index%3+2,"Index":1,"Language":if unknown{"und"}else{language},"Title":if unknown{"unidentified track"}else{title}},
        {"Type":"Subtitle","Codec":"subrip","Index":2,"Language":"chi","Title":format!("原视频字幕{index}")}
    ]},"Chapters":[{"MarkerType":"Chapter","Name":format!("第{index}集章节"),"StartPositionTicks":index as u64*1000000}],"EpisodeProof":index}])
}
fn normalized(index: usize) -> (&'static str, &'static str) {
    match index % 3 {
        0 => ("jpn", "日语"),
        1 => ("yue", "DVD粤语"),
        _ => ("chi", "东和台配国语"),
    }
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    host: Host,
    server: tokio::task::JoinHandle<()>,
    count: usize,
}
impl Fixture {
    async fn new(modes: &[&str]) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("title_file_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let q = search.clone();
                Box::pin(async move {
                    sqlx::query(&q).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        let host = Host {
            root: directory.path().into(),
            db: state.db.clone(),
            items: Arc::default(),
            clears: Arc::default(),
            native_starts: Arc::default(),
            config_writes: Arc::default(),
            refreshes: Arc::default(),
            fault: Arc::default(),
            drift: Arc::default(),
        };
        for (index, mode) in modes.iter().enumerate() {
            let id = format!("{}", 10001 + index);
            let path = directory.path().join(format!("{id}.strm"));
            let playback = format!("/cd2/{id}.mkv");
            let mut backup = info(index, *mode == "unknown");
            let mut streams = backup[0]["MediaSourceInfo"]["MediaStreams"].clone();
            if matches!(*mode, "live" | "same") {
                let (language, title) = normalized(index);
                streams[1]["Language"] = json!(language);
                streams[1]["Title"] = json!(title);
            }
            if *mode == "same" {
                backup[0]["MediaSourceInfo"]["MediaStreams"] = streams.clone();
            }
            streams[2]["Title"] = json!(format!("用户中文字幕{index}"));
            streams[2]["Language"] = json!(if index % 2 == 0 { "cht" } else { "chi" });
            let mut chapters = backup[0]["Chapters"].clone();
            chapters[0]["ImageTag"] = json!(format!("image-{index}"));
            std::fs::write(&path, &playback).unwrap();
            std::fs::write(
                directory.path().join(format!("{id}-mediainfo.json")),
                serde_json::to_vec(&backup).unwrap(),
            )
            .unwrap();
            host.items.lock().await.insert(id.clone(),json!({"Id":id,"Type":"Episode","Name":format!("episode{index}"),"IndexNumber":index+1,"ParentIndexNumber":1,"Path":path,"Size":1000+index,"PartCount":1,"MediaStreams":streams,"MediaSources":[{"Id":format!("mediasource_{id}"),"ItemId":id,"Path":playback,"MediaStreams":streams}],"Chapters":chapters}));
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Episode',$2,$3,$4,$5)").bind(format!("fixture-{index}")).bind(json!([id])).bind(json!([{"emby_item_id":id,"path":path,"size_bytes":1000+index}])).bind(json!([format!("{:040X}",index+1)])).bind(json!([null])).execute(&state.db).await.unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().fallback(api).with_state(host.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state.save_config(&json!({"emby_server_url":format!("http://{address}"),"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":directory.path(),"monitor_sha1_pc_search":false})).await.unwrap();
        Self {
            state,
            admin,
            schema,
            directory,
            host,
            server,
            count: modes.len(),
        }
    }
    async fn run(&self) -> Value {
        let state = self.state.clone();
        let ids = self
            .host
            .items
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let task = self
            .state
            .jobs
            .submit("file title regression", move |context| async move {
                titles::fill_in_job(&state, &ids, true, &context).await
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let result = self.state.jobs.result(&task).await.unwrap();
                if result["is_running"] == false {
                    return result["result"].clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    fn files(&self) -> Vec<Value> {
        (0..self.count)
            .map(|i| read_info(self.directory.path(), &format!("{}", 10001 + i)))
            .collect()
    }
    async fn close(self) {
        let counts = (
            self.host.clears.load(Ordering::SeqCst),
            self.host.native_starts.load(Ordering::SeqCst),
            self.host.config_writes.load(Ordering::SeqCst),
        );
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
        assert_eq!(
            counts.0, 0,
            "automatic title fill must never clear media info"
        );
        assert_eq!(
            counts.1, 0,
            "automatic title fill must never run native Restore"
        );
        assert_eq!(
            counts.2, 0,
            "automatic title fill must not modify plugin configuration"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn asynchronous_file_refresh_waits_for_live_titles_without_repeating_notifications() {
    let f = Fixture::new(&["raw", "raw"]).await;
    *f.host.fault.lock().await = "delayed".into();
    let report = f.run().await;
    let refreshes = f.host.refreshes.lock().await.len();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["verified_items"], 2, "{report}");
    assert_eq!(refreshes, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn distinct_episode_files_and_cache_refresh_once_without_losing_chapter_images() {
    let count = std::env::var("TITLE_BATCH_EPISODES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(39);
    let f = Fixture::new(&vec!["raw"; count]).await;
    let report = f.run().await;
    let files = f.files();
    let live = f.host.items.lock().await.clone();
    let refreshes = f.host.refreshes.lock().await.clone();
    let caches: Vec<Value> =
        sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache ORDER BY sha1")
            .fetch_all(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["refresh_state"], "submitted", "{report}");
    assert_eq!(report["verified_items"], count, "{report}");
    assert_eq!(report["skipped_chinese_streams"], count);
    assert_eq!(
        refreshes.len(),
        1,
        "one directory notification must serve the whole batch"
    );
    assert_eq!(refreshes[0].0, "/Items/directory/Refresh");
    assert_eq!(caches.len(), count);
    for (index, file) in files.iter().enumerate() {
        let id = format!("{}", 10001 + index);
        let (language, title) = normalized(index);
        assert_eq!(file[0]["EpisodeProof"], index);
        assert_eq!(caches[index], *file);
        assert_eq!(file[0]["Chapters"], info(index, false)[0]["Chapters"]);
        assert_eq!(
            live[&id]["Chapters"][0]["ImageTag"],
            format!("image-{index}")
        );
        assert_eq!(
            file[0]["MediaSourceInfo"]["MediaStreams"][1]["Codec"],
            info(index, false)[0]["MediaSourceInfo"]["MediaStreams"][1]["Codec"]
        );
        assert_eq!(live[&id]["MediaStreams"][1]["Language"], language);
        assert_eq!(live[&id]["MediaStreams"][1]["Title"], title);
        assert_eq!(
            live[&id]["MediaStreams"][2]["Title"],
            format!("用户中文字幕{index}")
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn matching_live_titles_allow_local_json_updates_without_refresh() {
    let f = Fixture::new(&["live", "live"]).await;
    let report = f.run().await;
    let files = f.files();
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["refresh_state"], "not_requested", "{report}");
    assert_eq!(report["changed_items"], 2);
    assert_eq!(report["verified_items"], 2);
    assert!(refreshes.is_empty());
    for (index, file) in files.iter().enumerate() {
        assert_eq!(
            file[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
            normalized(index).1
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn shared_directory_refresh_preserves_subtitles_of_matching_and_unchanged_targets() {
    let f = Fixture::new(&["live", "raw", "same"]).await;
    let report = f.run().await;
    let files = f.files();
    let live = f.host.items.lock().await.clone();
    let refreshes = f.host.refreshes.lock().await.len();
    let caches: Vec<Value> =
        sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache ORDER BY sha1")
            .fetch_all(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["refresh_state"], "submitted", "{report}");
    assert_eq!(report["verified_items"], 3, "{report}");
    assert_eq!(refreshes, 1);
    assert_eq!(caches.len(), 3);
    for index in [0, 2] {
        let id = format!("{}", 10001 + index);
        let subtitle = &files[index][0]["MediaSourceInfo"]["MediaStreams"][2];
        assert_eq!(subtitle["Title"], format!("用户中文字幕{index}"));
        assert_eq!(subtitle["Language"], "cht");
        assert_eq!(live[&id]["MediaStreams"][2]["Title"], subtitle["Title"]);
        assert_eq!(
            live[&id]["MediaStreams"][2]["Language"],
            subtitle["Language"]
        );
        assert_eq!(caches[index], files[index]);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn source_sha_and_path_drift_stop_batch_notification_before_reading_new_targets() {
    for drift in ["source", "sha", "path"] {
        let f = Fixture::new(&["raw", "raw"]).await;
        *f.host.drift.lock().await = drift.into();
        let report = f.run().await;
        let refreshes = f.host.refreshes.lock().await.len();
        let old_cache: Option<Value> =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 1))
                .fetch_optional(&f.state.db)
                .await
                .unwrap();
        let new_cache: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(format!("{:040X}", 999))
                .fetch_one(&f.state.db)
                .await
                .unwrap();
        f.close().await;
        assert_eq!(report["completed"], false, "{drift}: {report}");
        assert_eq!(report["refresh_state"], "failed", "{drift}: {report}");
        assert!(report["failed"].as_u64().unwrap() > 0, "{drift}: {report}");
        assert_eq!(refreshes, 0, "{drift}: {report}");
        assert_eq!(
            new_cache, 0,
            "drift must not associate old JSON with a new SHA"
        );
        assert_eq!(
            old_cache.unwrap()[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
            "日语",
            "the original target was saved before {drift} drift"
        );
        assert!(
            report["pending"].as_array().unwrap().iter().any(|entry| {
                let reason = entry["reason"].as_str().unwrap_or("");
                ["变化", "漂移", "身份", "来源"]
                    .iter()
                    .any(|word| reason.contains(word))
            }),
            "{drift}: {report}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn unknown_titles_keep_batch_incomplete_without_destructive_fallback() {
    let f = Fixture::new(&["raw", "same", "unknown"]).await;
    let report = f.run().await;
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(report["completed"], false, "{report}");
    assert_eq!(report["failed"], 0, "{report}");
    assert_eq!(refreshes.len(), 1);
    assert!(
        report["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["item_id"] == "10003")
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn failed_or_ineffective_refresh_never_claims_live_titles_are_updated() {
    for fault in ["denied", "no-import"] {
        let f = Fixture::new(&["raw", "raw"]).await;
        *f.host.fault.lock().await = fault.into();
        let report = f.run().await;
        let live = f.host.items.lock().await.clone();
        f.close().await;
        assert_eq!(report["completed"], false, "{fault}: {report}");
        assert_eq!(
            report["refresh_state"],
            if fault == "denied" {
                "failed"
            } else {
                "submitted"
            },
            "{fault}: {report}"
        );
        assert!(!report["pending"].as_array().unwrap().is_empty());
        assert_eq!(live["10001"]["MediaStreams"][1]["Title"], "AAC.原生日语");
    }
}
