use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, error::ApiError, language, strm, titles, webhook};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};

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
    file_watcher: Arc<AtomicBool>,
    final_calls: Arc<AtomicUsize>,
    final_scan_complete: Arc<AtomicBool>,
    guarded_run: Arc<AtomicBool>,
    guard_violations: Arc<AtomicUsize>,
    configuration: Arc<Mutex<Value>>,
    original_configuration: Arc<Mutex<Option<Value>>>,
    configuration_writes: Arc<Mutex<Vec<Value>>>,
    write_fault_injected: Arc<AtomicBool>,
    refresh_log: Arc<Mutex<Vec<String>>>,
    refresh_accepted: Arc<AtomicBool>,
    release_scan: Arc<Notify>,
    refresh_saw_ready_titles: Arc<AtomicBool>,
    nfo_scan_complete: Arc<AtomicBool>,
    stable_year_reads: Arc<AtomicUsize>,
    post_nfo_title_reads: Arc<AtomicUsize>,
    title_reads_before_year_check: Arc<AtomicUsize>,
}
fn read_info(root: &Path, id: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root.join(format!("{id}-mediainfo.json"))).unwrap())
        .unwrap()
}
async fn restore_write_fault(h: &Host) {
    let backup = h.root.join("10001-write-fault-original.json");
    if backup.exists() {
        tokio::fs::remove_dir(h.root.join("10001-mediainfo.json"))
            .await
            .unwrap();
        tokio::fs::rename(backup, h.root.join("10001-mediainfo.json"))
            .await
            .unwrap();
    }
}
async fn import_json(h: &Host, ids: &[String]) {
    let mut items = h.items.lock().await;
    for (id, item) in items.iter_mut().filter(|(id, _)| ids.contains(id)) {
        let data = match std::fs::read(h.root.join(format!("{id}-mediainfo.json")))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        {
            Some(data) => data,
            None => continue,
        };
        item["MediaStreams"] = data[0]["MediaSourceInfo"]["MediaStreams"].clone();
        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        // The modeled final refresh preserves existing chapter-image references.
    }
}
async fn ignore_file_change_active(h: &Host) -> bool {
    let configuration = h.configuration.lock().await;
    let flags = configuration["ExclusiveControlFeatures"]
        .as_str()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>();
    flags.contains(&"IgnoreFileChange") && !flags.contains(&"ExtractOnFileChange")
}
async fn finish_scan(h: &Host, ids: &[String], root_refresh: bool, fault: &str) {
    if root_refresh && !ignore_file_change_active(h).await {
        h.guard_violations.fetch_add(1, Ordering::SeqCst);
    } else if fault == "regress-final" {
        for (id, item) in h
            .items
            .lock()
            .await
            .iter_mut()
            .filter(|(id, _)| ids.contains(id))
        {
            let _ = id;
            for stream in item["MediaStreams"].as_array_mut().unwrap() {
                if stream["Type"] == "Audio" {
                    stream.as_object_mut().unwrap().remove("Title");
                }
            }
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        }
    } else if fault != "no-import" {
        import_json(h, ids).await;
    }
    if root_refresh {
        h.final_scan_complete.store(true, Ordering::SeqCst);
        let root_id = if ids.len() == 1 && h.items.lock().await[&ids[0]]["Type"] == "Movie" {
            ids[0].clone()
        } else {
            "series".into()
        };
        h.refresh_log.lock().await.push(format!(
            "Info ProviderManager: RefreshItem Complete: {root_id} item"
        ));
    }
}
async fn api(State(h): State<Host>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if request.method() == axum::http::Method::GET {
        if path == "/ScheduledTasks" {
            return Json(json!([{"Id":"media","Key":"MediaInfoExtractTask","Name":"Extract MediaInfo","State":"Idle"}])).into_response();
        }
        if path == "/System/Logs/embyserver.txt/Lines" {
            let lines = h.refresh_log.lock().await.clone();
            let limit = query
                .get("Limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(500);
            return Json(json!({"TotalRecordCount":lines.len(),"Items":lines.iter().rev().take(limit).cloned().collect::<Vec<_>>()})).into_response();
        }
        if path == "/web/ConfigurationPages" {
            return Json(json!([{"Name":"MediaInfoExtractPageView","Href":"/UI/View?PageId=fixture:MediaInfoExtractPageView"}])).into_response();
        }
        if path == "/UI/View" {
            let configuration = h.configuration.lock().await.clone();
            h.original_configuration
                .lock()
                .await
                .get_or_insert_with(|| configuration.clone());
            return Json(json!({"EditObjectContainer":{"Object":configuration}})).into_response();
        }
        if let Some(id) = path.strip_prefix("/Users/user/Items/") {
            if h.guarded_run.load(Ordering::SeqCst) && !ignore_file_change_active(&h).await {
                h.guard_violations.fetch_add(1, Ordering::SeqCst);
            }
            if id == "series"
                && query
                    .get("Fields")
                    .is_some_and(|fields| fields == "ProductionYear,EndDate,Status")
            {
                h.stable_year_reads.fetch_add(1, Ordering::SeqCst);
            }
            if matches!(id, "10001" | "10002")
                && h.nfo_scan_complete.load(Ordering::SeqCst)
                && query
                    .get("Fields")
                    .is_some_and(|fields| fields == "Id,Path,Size,MediaSources,MediaStreams")
            {
                h.post_nfo_title_reads.fetch_add(1, Ordering::SeqCst);
                if h.stable_year_reads.load(Ordering::SeqCst) < 3 {
                    h.title_reads_before_year_check
                        .fetch_add(1, Ordering::SeqCst);
                }
            }
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
            if id == "10001"
                && query
                    .get("Fields")
                    .is_some_and(|fields| fields == "Id,Path,Size,MediaSources,MediaStreams")
                && *h.fault.lock().await == "write-title-json"
                && !h.write_fault_injected.swap(true, Ordering::SeqCst)
            {
                tokio::fs::rename(
                    h.root.join("10001-mediainfo.json"),
                    h.root.join("10001-write-fault-original.json"),
                )
                .await
                .unwrap();
                tokio::fs::create_dir(h.root.join("10001-mediainfo.json"))
                    .await
                    .unwrap();
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
            let full_refresh = query
                .get("MetadataRefreshMode")
                .is_some_and(|mode| mode == "FullRefresh");
            let requested_id = path
                .strip_prefix("/Items/")
                .and_then(|value| value.strip_suffix("/Refresh"));
            let movie_id = h
                .items
                .lock()
                .await
                .iter()
                .find(|(id, item)| Some(id.as_str()) == requested_id && item["Type"] == "Movie")
                .map(|(id, _)| id.clone());
            let root_refresh =
                full_refresh && (path == "/Items/series/Refresh" || movie_id.is_some());
            let nfo_refresh = path == "/Items/series/Refresh"
                && query.get("Recursive").is_some_and(|value| value == "false")
                && query
                    .get("MetadataRefreshMode")
                    .is_some_and(|value| value == "Default")
                && h.items
                    .lock()
                    .await
                    .get("series")
                    .is_some_and(|item| item["Type"] == "Series");
            let watcher_import =
                path == "/Items/directory/Refresh" && h.file_watcher.load(Ordering::SeqCst);
            h.refreshes.lock().await.push((path.clone(), query));
            let fault = h.fault.lock().await.clone();
            if fault == "denied" {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if nfo_refresh {
                if !ignore_file_change_active(&h).await
                    || !h.final_scan_complete.load(Ordering::SeqCst)
                {
                    h.guard_violations.fetch_add(1, Ordering::SeqCst);
                }
                let xml = tokio::fs::read_to_string(h.root.join("tvshow.nfo"))
                    .await
                    .unwrap();
                let year = xml
                    .split("<year>")
                    .nth(1)
                    .unwrap()
                    .split("</year>")
                    .next()
                    .unwrap()
                    .parse::<i64>()
                    .unwrap();
                let mut items = h.items.lock().await;
                items.get_mut("series").unwrap()["ProductionYear"] = json!(year);
                items.get_mut("series").unwrap()["EndDate"] = Value::Null;
                drop(items);
                h.refresh_log.lock().await.extend([
                    "Info ProviderManager: RefreshItem Start: series Series".to_string(),
                    "Info ProviderManager: RefreshItem Complete: series Series".to_string(),
                ]);
                h.nfo_scan_complete.store(true, Ordering::SeqCst);
                return StatusCode::NO_CONTENT.into_response();
            }
            // Default alone does not import existing episode streams. The older
            // standalone positive cases explicitly opt into a file-watcher model.
            if !root_refresh && !watcher_import {
                return StatusCode::NO_CONTENT.into_response();
            }
            let ids = h
                .items
                .lock()
                .await
                .iter()
                .filter(|(id, item)| {
                    !root_refresh
                        || movie_id
                            .as_ref()
                            .map_or(item["SeriesId"] == "series", |movie| *id == movie)
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if root_refresh {
                let ready = ids.iter().all(|id| {
                    let path = h.root.join(format!("{id}-mediainfo.json"));
                    let info = std::fs::read(path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                    info.is_some_and(|info| {
                        language::fill(&info, &json!([]), true).1["changed"] == 0
                    })
                });
                h.refresh_saw_ready_titles.store(ready, Ordering::SeqCst);
                h.refresh_accepted.store(true, Ordering::SeqCst);
                let root_id = movie_id.as_deref().unwrap_or("series");
                h.refresh_log.lock().await.push(format!(
                    "Info ProviderManager: RefreshItem Start: {root_id} item"
                ));
            }
            if fault == "change-other-setting" {
                h.configuration.lock().await["OtherOption"] = json!("changed elsewhere");
            }
            if matches!(
                fault.as_str(),
                "delayed" | "late-import" | "cancel-after-accept"
            ) {
                tokio::spawn(async move {
                    if fault == "cancel-after-accept" {
                        h.release_scan.notified().await;
                    } else {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    finish_scan(&h, &ids, root_refresh, &fault).await;
                });
            } else {
                finish_scan(&h, &ids, root_refresh, &fault).await;
            }
            return StatusCode::NO_CONTENT.into_response();
        }
        if path.ends_with("/ClearMediaInfo") {
            h.clears.fetch_add(1, Ordering::SeqCst);
        } else if path.starts_with("/ScheduledTasks/Running/") {
            h.native_starts.fetch_add(1, Ordering::SeqCst);
        } else if path == "/UI/Command" {
            h.config_writes.fetch_add(1, Ordering::SeqCst);
            let body: Value =
                serde_json::from_slice(&to_bytes(request.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["CommandId"], "PageSave");
            let configuration: Value =
                serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
            h.configuration_writes
                .lock()
                .await
                .push(configuration.clone());
            *h.configuration.lock().await = configuration;
            if ignore_file_change_active(&h).await {
                h.guarded_run.store(true, Ordering::SeqCst);
            }
            return Json(json!({})).into_response();
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
fn gaia_info(index: usize) -> Value {
    json!([{"MediaSourceInfo":{"Container":"mp4","Size":1000+index,"RunTimeTicks":14330238330_u64,"MediaStreams":[
        {"Type":"Video","Codec":"h264","Index":0,"Width":2880,"Height":2160},
        {"Type":"Audio","Codec":"aac","Channels":2,"SampleRate":48000,"BitRate":320005,"Index":1,"Language":"jpn","IsDefault":true,"IsForced":false,"DisplayTitle":"Japanese AAC stereo (默认)"},
        {"Type":"Audio","Codec":"aac","Channels":2,"SampleRate":48000,"BitRate":112704,"Index":3,"Language":"chi","IsDefault":true,"IsForced":false,"DisplayTitle":"Chinese AAC stereo (默认)"}
    ]},"Chapters":[
        {"MarkerType":"Chapter","Name":"片头","StartPositionTicks":0},
        {"MarkerType":"Chapter","Name":"正片","StartPositionTicks":850000000_u64},
        {"MarkerType":"Chapter","Name":"片尾","StartPositionTicks":13440000000_u64}
    ],"EpisodeProof":index}])
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
            file_watcher: Arc::default(),
            final_calls: Arc::default(),
            final_scan_complete: Arc::default(),
            guarded_run: Arc::default(),
            guard_violations: Arc::default(),
            configuration: Arc::new(Mutex::new(json!({
                "ExclusiveControlFeatures":"CatchAllAllow,ExtractOnFileChange,CustomFlag",
                "PersistMediaInfoMode":"Default","LibraryScope":"existing-library",
                "MediaInfoJsonRootFolder":"","OtherOption":"original"
            }))),
            original_configuration: Arc::default(),
            configuration_writes: Arc::default(),
            write_fault_injected: Arc::default(),
            refresh_log: Arc::default(),
            refresh_accepted: Arc::default(),
            release_scan: Arc::default(),
            refresh_saw_ready_titles: Arc::default(),
            nfo_scan_complete: Arc::default(),
            stable_year_reads: Arc::default(),
            post_nfo_title_reads: Arc::default(),
            title_reads_before_year_check: Arc::default(),
        };
        for (index, mode) in modes.iter().enumerate() {
            let id = format!("{}", 10001 + index);
            let path = directory.path().join(format!("{id}.strm"));
            let playback = format!("/cd2/{id}.{}", if *mode == "gaia" { "mp4" } else { "mkv" });
            let mut backup = if *mode == "gaia" {
                gaia_info(index)
            } else {
                info(index, *mode == "unknown")
            };
            let mut streams = backup[0]["MediaSourceInfo"]["MediaStreams"].clone();
            if matches!(*mode, "live" | "same") {
                let (language, title) = normalized(index);
                streams[1]["Language"] = json!(language);
                streams[1]["Title"] = json!(title);
            }
            if *mode == "same" {
                backup[0]["MediaSourceInfo"]["MediaStreams"] = streams.clone();
            }
            if *mode != "gaia" {
                streams[2]["Title"] = json!(format!("用户中文字幕{index}"));
                streams[2]["Language"] = json!(if index % 2 == 0 { "cht" } else { "chi" });
            }
            let mut chapters = backup[0]["Chapters"].clone();
            chapters[0]["ImageTag"] = json!(format!("image-{index}"));
            std::fs::write(&path, &playback).unwrap();
            std::fs::write(
                directory.path().join(format!("{id}-mediainfo.json")),
                serde_json::to_vec(&backup).unwrap(),
            )
            .unwrap();
            host.items.lock().await.insert(id.clone(),json!({"Id":id,"Type":"Episode","SeriesId":"series","Name":format!("episode{index}"),"IndexNumber":index+1,"ParentIndexNumber":1,"Path":path,"Size":1000+index,"PartCount":1,"MediaStreams":streams,"MediaSources":[{"Id":format!("mediasource_{id}"),"ItemId":id,"Path":playback,"MediaStreams":streams}],"Chapters":chapters}));
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
        let ids = self
            .host
            .items
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        self.run_ids(ids).await
    }
    async fn run_with_file_watcher(&self) -> Value {
        // Model an explicitly configured external JSON importer for the legacy
        // standalone contract, not an inherent effect of Emby's Default scan.
        self.host.file_watcher.store(true, Ordering::SeqCst);
        self.run().await
    }
    async fn run_ids(&self, ids: Vec<String>) -> Value {
        let state = self.state.clone();
        let task = self
            .state
            .jobs
            .submit("file title regression", move |context| async move {
                titles::fill_in_job(&state, &ids, true, &context).await
            })
            .await
            .unwrap();
        self.wait_job(&task).await["result"].clone()
    }
    async fn wait_job(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let result = self.state.jobs.result(task).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn run_with_final_refresh(&self, ids: Vec<String>) -> Result<Value, String> {
        let state = self.state.clone();
        let host = self.host.clone();
        host.guarded_run.store(true, Ordering::SeqCst);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = self
            .state
            .jobs
            .submit(
                "ingest final refresh regression",
                move |context| async move {
                    let refresh = async {
                        host.final_calls.fetch_add(1, Ordering::SeqCst);
                        let fault = host.fault.lock().await.clone();
                        if fault == "write-title-json" {
                            restore_write_fault(&host).await;
                        }
                        for id in &ids {
                            if id == "10001"
                                && matches!(fault.as_str(), "read-title-json" | "write-title-json")
                            {
                                continue;
                            }
                            let (_, report) =
                                language::fill(&read_info(&host.root, id), &json!([]), true);
                            if report["changed"] != 0 {
                                return Err(ApiError::bad(
                                    "final refresh ran before title JSON was saved",
                                ));
                            }
                        }
                        if !ignore_file_change_active(&host).await {
                            return Err(ApiError::bad(
                                "file-change guard was not held through final refresh",
                            ));
                        }
                        if *host.fault.lock().await == "final-error" {
                            return Err(ApiError::bad("fixture root metadata refresh failed"));
                        }
                        state
                            .emby(
                                reqwest::Method::POST,
                                "Items/series/Refresh",
                                &[
                                    ("Recursive", "true".into()),
                                    ("MetadataRefreshMode", "FullRefresh".into()),
                                    ("ImageRefreshMode", "FullRefresh".into()),
                                    ("ReplaceAllMetadata", "false".into()),
                                    ("ReplaceAllImages", "false".into()),
                                ],
                                None,
                            )
                            .await?;
                        // The production callback already waits for final refresh. This
                        // explicit late-import fault also exercises the title readback wait.
                        if *host.fault.lock().await != "late-import" {
                            while !host.final_scan_complete.load(Ordering::SeqCst) {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                        }
                        Ok(())
                    };
                    let result = strm::with_settings(&state, "ignore_file_change", || {
                        titles::fill_in_job_with_refresh(&state, &ids, true, &context, refresh)
                    })
                    .await;
                    let _ = sender.send(
                        result
                            .as_ref()
                            .map(Clone::clone)
                            .map_err(|error| error.1.clone()),
                    );
                    result
                },
            )
            .await
            .unwrap();
        self.wait_job(&task).await;
        receiver
            .await
            .expect("the tested workflow must return its result")
    }
    fn files(&self) -> Vec<Value> {
        (0..self.count)
            .map(|i| read_info(self.directory.path(), &format!("{}", 10001 + i)))
            .collect()
    }
    async fn make_movie(&self) {
        {
            let mut items = self.host.items.lock().await;
            let item = items.get_mut("10001").unwrap();
            item["Type"] = json!("Movie");
            item["ProductionYear"] = json!(1998);
            item["ProviderIds"] = json!({"Tmdb":"gaia-movie-fixture"});
        }
        sqlx::query(
            "UPDATE media_metadata SET item_type='Movie' WHERE emby_item_ids_json @> $1::jsonb",
        )
        .bind(json!(["10001"]))
        .execute(&self.state.db)
        .await
        .unwrap();
        self.enable_real_finalize().await;
    }
    async fn enable_real_finalize(&self) {
        self.state
            .save_config(&json!({
                "auto_fill_language_titles_on_ingest":true,"monitor_sha1_pc_search":false,
                "auto_repair_truehd_core_on_ingest":false,"telegram_bot_token":""
            }))
            .await
            .unwrap();
        emby_toolkit::db::save_setting(
            &self.state.db,
            "cover_generator_config",
            &json!({"enabled":false}),
        )
        .await
        .unwrap();
    }
    async fn start_real_finalize(&self) -> String {
        let state = self.state.clone();
        self.state
            .jobs
            .submit("real webhook title finalize", move |context| async move {
                webhook::finalize(&state, "10001", &[], false, &context).await
            })
            .await
            .unwrap()
    }
    async fn close(self) {
        let counts = (
            self.host.clears.load(Ordering::SeqCst),
            self.host.native_starts.load(Ordering::SeqCst),
            self.host.config_writes.load(Ordering::SeqCst),
        );
        let original_configuration = self.host.original_configuration.lock().await.clone();
        let configuration = self.host.configuration.lock().await.clone();
        let writes = self.host.configuration_writes.lock().await.clone();
        let guard_violations = self.host.guard_violations.load(Ordering::SeqCst);
        let fault = self.host.fault.lock().await.clone();
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
        assert_eq!(counts.2, writes.len());
        assert_eq!(
            guard_violations, 0,
            "guard must cover JSON preparation, refresh and readback"
        );
        if let Some(original) = original_configuration {
            for key in [
                "ExclusiveControlFeatures",
                "PersistMediaInfoMode",
                "LibraryScope",
                "MediaInfoJsonRootFolder",
            ] {
                assert_eq!(
                    configuration[key], original[key],
                    "original {key} must be restored"
                );
            }
            if fault != "change-other-setting" {
                assert_eq!(configuration, original);
            }
            for write in &writes {
                for key in [
                    "PersistMediaInfoMode",
                    "LibraryScope",
                    "MediaInfoJsonRootFolder",
                ] {
                    assert_eq!(write[key], original[key], "{key} must never be changed");
                }
            }
            if let Some(first) = writes.first() {
                let flags = first["ExclusiveControlFeatures"]
                    .as_str()
                    .unwrap()
                    .split(',')
                    .map(str::trim)
                    .collect::<Vec<_>>();
                assert!(flags.contains(&"IgnoreFileChange"));
                assert!(!flags.contains(&"ExtractOnFileChange"));
                for preserved in ["CatchAllAllow", "CatchAllBlock", "CustomFlag"] {
                    if original["ExclusiveControlFeatures"]
                        .as_str()
                        .unwrap()
                        .split(',')
                        .map(str::trim)
                        .any(|flag| flag == preserved)
                    {
                        assert!(flags.contains(&preserved), "{preserved} must be preserved");
                    }
                }
                assert_eq!(
                    writes.last().unwrap()["ExclusiveControlFeatures"],
                    original["ExclusiveControlFeatures"]
                );
            }
        } else {
            assert_eq!(
                counts.2, 0,
                "standalone tests do not modify plugin settings"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn gaia_fifty_one_episode_json_titles_reach_emby_without_refreshing_other_episodes() {
    let mut modes = vec!["gaia"; 51];
    modes.push("raw");
    let f = Fixture::new(&modes).await;
    f.host.items.lock().await.get_mut("10052").unwrap()["SeriesId"] = json!("other-series");
    *f.host.fault.lock().await = "late-import".into();
    let unrelated_before = f.host.items.lock().await["10052"].clone();
    let mut targets = (10001..=10051).map(|id| id.to_string()).collect::<Vec<_>>();
    targets.extend(["10001".into(), "10051".into()]);
    let report = f.run_with_final_refresh(targets).await.unwrap();
    let files = f.files();
    let live = f.host.items.lock().await.clone();
    let refreshes = f.host.refreshes.lock().await.clone();
    let final_calls = f.host.final_calls.load(Ordering::SeqCst);
    let configuration_writes = f.host.config_writes.load(Ordering::SeqCst);
    let cached: Vec<Value> =
        sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache ORDER BY sha1")
            .fetch_all(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["scanned"], 51);
    assert_eq!(report["changed_streams"], 102);
    assert_eq!(report["verified_items"], 51);
    assert_eq!(report["verified_streams"], 102);
    assert_eq!(report["changed_languages"], 0);
    assert_eq!(cached.len(), 51);
    assert_eq!(final_calls, 1);
    assert_eq!(configuration_writes, 2);
    assert_eq!(
        refreshes.len(),
        1,
        "the existing final refresh is reused once"
    );
    assert_eq!(refreshes[0].0, "/Items/series/Refresh");
    assert_eq!(refreshes[0].1["MetadataRefreshMode"], "FullRefresh");
    assert_eq!(refreshes[0].1["Recursive"], "true");
    assert_eq!(refreshes[0].1["ReplaceAllMetadata"], "false");
    assert_eq!(refreshes[0].1["ReplaceAllImages"], "false");
    for index in 0..51 {
        let id = format!("{}", 10001 + index);
        let saved = &files[index][0]["MediaSourceInfo"]["MediaStreams"];
        assert_eq!(saved[1]["Title"], "日语");
        assert_eq!(saved[1]["Language"], "jpn");
        assert_eq!(saved[2]["Title"], "国语");
        assert_eq!(saved[2]["Language"], "chi");
        assert_eq!(saved[2]["Index"], 3);
        let mut expected = gaia_info(index)[0]["MediaSourceInfo"]["MediaStreams"].clone();
        expected[1]["Title"] = json!("日语");
        expected[2]["Title"] = json!("国语");
        assert_eq!(*saved, expected, "only the two audio titles may change");
        assert_eq!(live[&id]["MediaStreams"], *saved);
        assert_eq!(files[index], cached[index]);
        assert_eq!(files[index][0]["Chapters"], gaia_info(index)[0]["Chapters"]);
        assert_eq!(
            live[&id]["Chapters"][0]["ImageTag"],
            format!("image-{index}")
        );
    }
    assert_eq!(live["10052"], unrelated_before);
    assert_eq!(files[51], info(51, false));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn final_refresh_failure_propagates_after_json_save_and_restores_ignore_guard() {
    let f = Fixture::new(&["gaia"]).await;
    *f.host.fault.lock().await = "final-error".into();
    let error = f
        .run_with_final_refresh(vec!["10001".into()])
        .await
        .unwrap_err();
    let saved = f.files();
    let live = f.host.items.lock().await["10001"].clone();
    let calls = f.host.final_calls.load(Ordering::SeqCst);
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert!(
        error.contains("fixture root metadata refresh failed"),
        "{error}"
    );
    assert_eq!(calls, 1);
    assert!(
        refreshes.is_empty(),
        "a failed callback must not trigger a fallback scan"
    );
    assert_eq!(
        saved[0][0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
        "日语"
    );
    assert!(live["MediaStreams"][1].get("Title").is_none());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn initially_matching_titles_must_still_be_verified_after_final_refresh() {
    let f = Fixture::new(&["same", "same"]).await;
    *f.host.fault.lock().await = "regress-final".into();
    let report = f
        .run_with_final_refresh(vec!["10001".into(), "10002".into()])
        .await
        .unwrap();
    let calls = f.host.final_calls.load(Ordering::SeqCst);
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(calls, 1);
    assert_eq!(refreshes.len(), 1);
    assert_eq!(report["changed_streams"], 0);
    assert_eq!(report["completed"], false, "{report}");
    assert_eq!(report["verified_streams"], 0, "{report}");
    assert_eq!(report["failed"], 2, "{report}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn ignore_guard_preserves_existing_control_flags_and_later_other_setting_changes() {
    let f = Fixture::new(&["raw"]).await;
    f.host.configuration.lock().await["ExclusiveControlFeatures"] =
        json!("CatchAllAllow,CatchAllBlock,ExtractOnFileChange,CustomFlag");
    *f.host.fault.lock().await = "change-other-setting".into();
    let report = f
        .run_with_final_refresh(vec!["10001".into()])
        .await
        .unwrap();
    let configuration = f.host.configuration.lock().await.clone();
    let writes = f.host.config_writes.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(writes, 2);
    assert_eq!(configuration["OtherOption"], "changed elsewhere");
    assert_eq!(
        configuration["ExclusiveControlFeatures"],
        "CatchAllAllow,CatchAllBlock,ExtractOnFileChange,CustomFlag"
    );
}

async fn verify_ordinary_title_io_failure(fault: &str) {
    let f = Fixture::new(&["gaia", "gaia"]).await;
    let original = std::fs::read(f.directory.path().join("10001-mediainfo.json")).unwrap();
    *f.host.fault.lock().await = fault.into();
    if fault == "read-title-json" {
        std::fs::write(f.directory.path().join("10001-mediainfo.json"), b"not JSON").unwrap();
    }
    let result = f
        .run_with_final_refresh(vec!["10001".into(), "10002".into()])
        .await;
    restore_write_fault(&f.host).await;
    let callbacks = f.host.final_calls.load(Ordering::SeqCst);
    let refreshes = f.host.refreshes.lock().await.clone();
    let cache: Vec<Value> =
        sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache ORDER BY sha1")
            .fetch_all(&f.state.db)
            .await
            .unwrap();
    let original_after = std::fs::read(f.directory.path().join("10001-mediainfo.json")).unwrap();
    f.close().await;
    assert_eq!(callbacks, 1, "{fault}: {result:?}");
    let report = result.unwrap();
    assert_eq!(refreshes.len(), 1);
    assert_eq!(refreshes[0].0, "/Items/series/Refresh");
    assert_eq!(report["completed"], false, "{fault}: {report}");
    assert_eq!(report["failed"], 1, "{fault}: {report}");
    assert_eq!(report["verified_items"], 1, "{fault}: {report}");
    assert_eq!(cache.len(), 1, "failed JSON must not be written to cache");
    assert_eq!(cache[0][0]["EpisodeProof"], 1);
    if fault == "write-title-json" {
        assert_eq!(original_after, original);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn ordinary_title_json_read_failure_still_runs_final_refresh() {
    verify_ordinary_title_io_failure("read-title-json").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn ordinary_title_json_write_failure_still_runs_final_refresh() {
    verify_ordinary_title_io_failure("write-title-json").await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn native_titles_without_etk_bindings_are_saved_and_verified() {
    let f = Fixture::new(&["gaia"]).await;
    f.make_movie().await;
    sqlx::query("DELETE FROM media_metadata")
        .execute(&f.state.db)
        .await
        .unwrap();
    let task = f.start_real_finalize().await;
    let result = f.wait_job(&task).await;
    let saved = f.files().remove(0);
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(
        result["result"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert_eq!(result["result"]["title_fill"]["verified_streams"], 2);
    assert_eq!(refreshes.len(), 1);
    assert_eq!(
        saved[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
        "日语"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn native_title_fallback_keeps_file_identity_checks() {
    let f = Fixture::new(&["gaia"]).await;
    f.make_movie().await;
    sqlx::query("DELETE FROM media_metadata")
        .execute(&f.state.db)
        .await
        .unwrap();
    f.host.items.lock().await.get_mut("10001").unwrap()["Size"] = json!(999999);
    let before = f.files();
    let task = f.start_real_finalize().await;
    let result = f.wait_job(&task).await;
    let after = f.files();
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(before, after);
    assert!(refreshes.is_empty());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn fingerprint_query_failure_does_not_prevent_titles_or_final_refresh() {
    let f = Fixture::new(&["gaia"]).await;
    f.make_movie().await;
    f.state
        .save_config(&json!({"monitor_sha1_pc_search":true}))
        .await
        .unwrap();
    // Fail only the global fingerprint query; native identity/JSON reads still work.
    sqlx::query("ALTER TABLE media_metadata DROP COLUMN in_library")
        .execute(&f.state.db)
        .await
        .unwrap();
    let task = f.start_real_finalize().await;
    let result = f.wait_job(&task).await;
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(
        result["result"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert_eq!(result["result"]["fingerprints"]["failed"], 1, "{result}");
    assert!(result["result"]["warnings"].as_u64().unwrap() > 0);
    assert_eq!(refreshes.len(), 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn real_webhook_finalization_saves_custom_audio_titles_before_movie_refresh() {
    let f = Fixture::new(&["gaia"]).await;
    f.make_movie().await;
    let mut info = f.files().remove(0);
    let streams = &mut info[0]["MediaSourceInfo"]["MediaStreams"];
    streams[1]["Language"] = json!("chi");
    streams[1]["Title"] = json!("台");
    streams[2]["Language"] = json!("ace");
    streams[2]["Title"] = json!("DVD粤");
    std::fs::write(
        f.directory.path().join("10001-mediainfo.json"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();
    {
        let mut items = f.host.items.lock().await;
        items.get_mut("10001").unwrap()["MediaStreams"] =
            info[0]["MediaSourceInfo"]["MediaStreams"].clone();
        items.get_mut("10001").unwrap()["MediaSources"][0]["MediaStreams"] =
            info[0]["MediaSourceInfo"]["MediaStreams"].clone();
    }
    let task = f.start_real_finalize().await;
    let result = f.wait_job(&task).await;
    let saved = f.files().remove(0);
    let live = f.host.items.lock().await["10001"].clone();
    let refreshes = f.host.refreshes.lock().await.clone();
    let ready = f.host.refresh_saw_ready_titles.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(
        result["result"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert_eq!(result["result"]["title_fill"]["verified_streams"], 2);
    assert_eq!(refreshes.len(), 1);
    assert_eq!(refreshes[0].0, "/Items/10001/Refresh");
    assert_eq!(refreshes[0].1["Recursive"], "false");
    assert!(
        ready,
        "real webhook must finish title writes before its final refresh"
    );
    assert_eq!(
        saved[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
        "台配国语"
    );
    assert_eq!(
        saved[0]["MediaSourceInfo"]["MediaStreams"][2]["Title"],
        "DVD粤语"
    );
    assert_eq!(live["MediaStreams"][2]["Language"], "yue");
    assert_eq!(live["MediaStreams"][2]["Title"], "DVD粤语");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn real_series_webhook_saves_titles_before_full_refresh_and_verifies_after_top_level_nfo() {
    let f = Fixture::new(&["gaia", "gaia", "raw"]).await;
    f.enable_real_finalize().await;
    {
        let mut items = f.host.items.lock().await;
        items.get_mut("10003").unwrap()["SeriesId"] = json!("other-series");
        items.insert(
            "series".into(),
            json!({
                "Id":"series","Type":"Series","Name":"盖亚奥特曼","Path":f.directory.path(),
                "ProductionYear":2004,"EndDate":"2004-12-01","Status":"Continuing",
                "ProviderIds":{"Tmdb":"4426"},"DateLastRefreshed":"old"
            }),
        );
    }
    std::fs::write(
        f.directory.path().join("tvshow.nfo"),
        "<tvshow><year>1998</year><premiered>1998-09-05</premiered></tvshow>",
    )
    .unwrap();
    let other_before = f.host.items.lock().await["10003"].clone();
    let state = f.state.clone();
    let task = f
        .state
        .jobs
        .submit(
            "real Series/NFO title finalize",
            move |context| async move {
                webhook::finalize(
                    &state,
                    "series",
                    &["10001".into(), "10002".into()],
                    false,
                    &context,
                )
                .await
            },
        )
        .await
        .unwrap();
    let result = f.wait_job(&task).await;
    let live = f.host.items.lock().await.clone();
    let files = f.files();
    let refreshes = f.host.refreshes.lock().await.clone();
    let ready = f.host.refresh_saw_ready_titles.load(Ordering::SeqCst);
    let year_reads = f.host.stable_year_reads.load(Ordering::SeqCst);
    let title_reads = f.host.post_nfo_title_reads.load(Ordering::SeqCst);
    let early_reads = f.host.title_reads_before_year_check.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(
        result["result"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert_eq!(result["result"]["title_fill"]["verified_streams"], 4);
    assert!(
        ready,
        "both JSON files must be normalized before the root Full refresh"
    );
    assert_eq!(refreshes.len(), 2);
    assert_eq!(refreshes[0].0, "/Items/series/Refresh");
    assert_eq!(refreshes[0].1["MetadataRefreshMode"], "FullRefresh");
    assert_eq!(refreshes[0].1["Recursive"], "true");
    assert_eq!(refreshes[1].0, "/Items/series/Refresh");
    assert_eq!(refreshes[1].1["MetadataRefreshMode"], "Default");
    assert_eq!(refreshes[1].1["Recursive"], "false");
    assert_eq!(year_reads, 3);
    assert!(title_reads >= 2);
    assert_eq!(
        early_reads, 0,
        "titles must be read back after the full Series/NFO refresh"
    );
    assert_eq!(live["series"]["ProductionYear"], 1998);
    assert_eq!(live["series"]["EndDate"], Value::Null);
    assert_eq!(live["series"]["Status"], "Continuing");
    for index in 0..2 {
        let id = (10001 + index).to_string();
        assert_eq!(
            files[index][0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
            "日语"
        );
        assert_eq!(
            files[index][0]["MediaSourceInfo"]["MediaStreams"][2]["Title"],
            "国语"
        );
        assert_eq!(
            live[&id]["MediaStreams"],
            files[index][0]["MediaSourceInfo"]["MediaStreams"]
        );
    }
    assert_eq!(live["10003"], other_before);
    assert_eq!(files[2], info(2, false));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn cancellation_after_real_refresh_acceptance_keeps_ignore_until_completion_is_confirmed() {
    let f = Fixture::new(&["gaia"]).await;
    f.make_movie().await;
    *f.host.fault.lock().await = "cancel-after-accept".into();
    let task = f.start_real_finalize().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.host.refresh_accepted.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.state.jobs.control(Some(&task), "stop").await.unwrap();
    let result = f.wait_job(&task).await;
    let ignored_while_running = ignore_file_change_active(&f.host).await;
    let writes_before_complete = f.host.config_writes.load(Ordering::SeqCst);
    let recovery = f.state.data_dir.join("rename-recovery");
    let pending = std::fs::read_dir(&recovery)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("pending-scan.json"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    f.host.release_scan.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.host.final_scan_complete.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let ignored_before_followup = ignore_file_change_active(&f.host).await;
    let followup = strm::with_settings(&f.state, "ignore_file_change", || async { Ok(()) }).await;
    let configuration = f.host.configuration.lock().await.clone();
    let still_pending = pending.iter().any(|path| path.exists());
    f.close().await;
    assert_ne!(
        result["result"]["title_fill"]["completed"], true,
        "{result}"
    );
    assert!(
        ignored_while_running,
        "cancellation must not restore during an accepted scan"
    );
    assert_eq!(
        writes_before_complete, 1,
        "only enabling Ignore is allowed before scan completion"
    );
    assert_eq!(pending.len(), 1);
    assert!(
        ignored_before_followup,
        "completion must be verified before restoring settings"
    );
    assert!(followup.is_ok(), "{followup:?}");
    assert!(!still_pending);
    assert_eq!(
        configuration["ExclusiveControlFeatures"],
        "CatchAllAllow,ExtractOnFileChange,CustomFlag"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn asynchronous_file_refresh_waits_for_live_titles_without_repeating_notifications() {
    let f = Fixture::new(&["raw", "raw"]).await;
    *f.host.fault.lock().await = "delayed".into();
    let report = f.run_with_file_watcher().await;
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["verified_items"], 2, "{report}");
    assert_eq!(refreshes.len(), 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn distinct_episode_files_and_cache_refresh_once_without_losing_chapter_images() {
    let count = std::env::var("TITLE_BATCH_EPISODES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(39);
    let f = Fixture::new(&vec!["raw"; count]).await;
    let report = f.run_with_file_watcher().await;
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
        "one directory notification to the explicit file-watcher model"
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
    let report = f.run_with_file_watcher().await;
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
    let report = f.run_with_file_watcher().await;
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
    assert_eq!(report["verified_items"], 3, "{report}");
    assert_eq!(refreshes.len(), 1);
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
        let report = f.run_with_file_watcher().await;
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
    let report = f.run_with_file_watcher().await;
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
async fn nexus_short_audio_sources_and_mislabeled_chinese_subtitles_survive_file_refresh() {
    let f = Fixture::new(&["raw"]).await;
    let mut backup = f.files().remove(0);
    backup[0]["MediaSourceInfo"]["MediaStreams"] = json!([
        {"Type":"Video","Codec":"h264","Index":0},
        {"Type":"Audio","Codec":"ac3","Language":"chi","Title":"台","Index":1},
        {"Type":"Audio","Codec":"aac","Language":"chi","Title":"东和兴","Index":2},
        {"Type":"Audio","Codec":"aac","Language":"chi","Title":"福建文艺","Index":3},
        {"Type":"Audio","Codec":"ac3","Language":"chi","Title":"TVB粤","Index":4},
        {"Type":"Audio","Codec":"ac3","Language":"chi","Title":"日语","Index":5},
        {"Type":"Subtitle","Codec":"ass","Language":"chi","Title":"台对","Index":6},
        {"Type":"Audio","Codec":"ac3","Language":"ace","Title":"DVD粤","Index":7},
        {"Type":"Subtitle","Codec":"subrip","Language":"ace","Title":"TVB粤对","Index":8},
        {"Type":"Subtitle","Codec":"ass","Language":"jpn","Title":"日对","Index":9},
        {"Type":"Subtitle","Codec":"pgssub","Language":"ace","Title":"DVD粤对","Index":10},
        {"Type":"Subtitle","Codec":"subrip","Language":"eng","Title":"英文字幕","Index":11}
    ]);
    let streams = backup[0]["MediaSourceInfo"]["MediaStreams"].clone();
    std::fs::write(
        f.directory.path().join("10001-mediainfo.json"),
        serde_json::to_vec(&backup).unwrap(),
    )
    .unwrap();
    {
        let mut items = f.host.items.lock().await;
        let item = items.get_mut("10001").unwrap();
        item["MediaStreams"] = streams.clone();
        item["MediaSources"][0]["MediaStreams"] = streams;
    }
    let report = f.run_with_file_watcher().await;
    let saved = f.files().remove(0);
    let live = f.host.items.lock().await["10001"]["MediaStreams"].clone();
    let cache: Value = sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    let refreshes = f.host.refreshes.lock().await.clone();
    f.close().await;
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["skipped_chinese_streams"], 3);
    assert_eq!(refreshes.len(), 1);
    assert_eq!(saved, cache);
    assert_eq!(saved[0]["Chapters"], backup[0]["Chapters"]);
    let saved_streams = &saved[0]["MediaSourceInfo"]["MediaStreams"];
    for (index, title, language) in [
        (1, "台配国语", "chi"),
        (2, "东和兴国语", "chi"),
        (3, "福建文艺国语", "chi"),
        (4, "TVB粤语", "yue"),
        (5, "日语", "jpn"),
        (7, "DVD粤语", "yue"),
    ] {
        assert_eq!(saved_streams[index]["Title"], title);
        assert_eq!(saved_streams[index]["Language"], language);
        assert_eq!(live[index], saved_streams[index]);
    }
    for index in [6, 8, 10] {
        assert_eq!(
            saved_streams[index],
            backup[0]["MediaSourceInfo"]["MediaStreams"][index]
        );
        assert_eq!(live[index], saved_streams[index]);
    }
    assert_eq!(saved_streams[9]["Title"], "日文（日对）");
    assert_eq!(saved_streams[11]["Title"], "英文");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn failed_or_ineffective_refresh_never_claims_live_titles_are_updated() {
    for fault in ["denied", "no-import"] {
        let f = Fixture::new(&["raw", "raw"]).await;
        *f.host.fault.lock().await = fault.into();
        let report = f.run_with_file_watcher().await;
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
