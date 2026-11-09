use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, db, titles};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Host {
    directory: PathBuf,
    metadata_root: PathBuf,
    item: Arc<Mutex<Value>>,
    settings: Arc<Mutex<Value>>,
    starts: Arc<AtomicUsize>,
    clears: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
    config_writes: Arc<AtomicUsize>,
    wrong_final_language: Arc<AtomicBool>,
    refresh_logs: Arc<Mutex<Vec<String>>>,
    clear_ids: Arc<Mutex<Vec<String>>>,
}

fn item_metadata_directory(host: &Host, item: &Value) -> PathBuf {
    let guid = uuid::Uuid::parse_str(item["Guid"].as_str().unwrap())
        .unwrap()
        .simple()
        .to_string();
    host.metadata_root
        .join("library")
        .join(&guid[..2])
        .join(guid)
}

fn chapter_image(host: &Host, item: &Value, chapter: &Value) -> PathBuf {
    item_metadata_directory(host, item)
        .join("chapters")
        .join(format!("{}.jpg", chapter["StartPositionTicks"]))
}

fn local_bif(item: &Value) -> PathBuf {
    let path = std::path::Path::new(item["Path"].as_str().unwrap());
    path.parent().unwrap().join(format!(
        "{}-320-1.bif",
        path.file_stem().unwrap().to_str().unwrap()
    ))
}

async fn seed_standard_bif(host: &Host, item: &Value) {
    // Independent golden writer for the native 64-byte BIF header and N+1 index.
    let frames = item["Chapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chapter| {
            let timestamp = chapter["StartPositionTicks"]
                .as_u64()
                .unwrap()
                .div_ceil(10_000) as u32;
            (
                timestamp,
                std::fs::read(chapter_image(host, item, chapter)).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut bytes = vec![0u8; 64];
    bytes[..8].copy_from_slice(b"\x89BIF\r\n\x1a\n");
    bytes[12..16].copy_from_slice(&(frames.len() as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    let mut offset = (64 + 8 * (frames.len() + 1)) as u32;
    for (timestamp, jpeg) in &frames {
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        offset += jpeg.len() as u32;
    }
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&offset.to_le_bytes());
    for (_, jpeg) in frames {
        bytes.extend(jpeg);
    }
    tokio::fs::write(local_bif(item), bytes).await.unwrap();
}

fn native_bif_frames(item: &Value) -> Option<Vec<(u64, Vec<u8>)>> {
    // Model the native reader using the documented binary index and JPEG decode.
    let path = std::path::Path::new(item["Path"].as_str()?);
    let prefix = path.file_stem()?.to_str()?.to_lowercase();
    for entry in std::fs::read_dir(path.parent()?).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !name.starts_with(&prefix) || !name.ends_with(".bif") {
            continue;
        }
        let fields = name
            .trim_end_matches(".bif")
            .rsplit('-')
            .collect::<Vec<_>>();
        let Some(width) = fields.get(1).and_then(|width| width.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        let word = |index: usize| {
            bytes
                .get(index..index + 4)
                .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
                .map(u32::from_le_bytes)
        };
        if bytes.get(..8) != Some(&b"\x89BIF\r\n\x1a\n"[..])
            || word(8) != Some(0)
            || word(20) != Some(0)
        {
            continue;
        }
        let (Some(count), Some(scale)) = (word(12), word(16)) else {
            continue;
        };
        if count == 0 || count > 100_000 || scale == 0 {
            continue;
        }
        let index_end = 64 + 8 * (count as usize + 1);
        if index_end > bytes.len()
            || word(64 + 8 * count as usize) != Some(u32::MAX)
            || word(68 + 8 * count as usize) != Some(bytes.len() as u32)
        {
            continue;
        }
        let mut frames = vec![];
        let mut previous = 0;
        for index in 0..count as usize {
            let (Some(timestamp), Some(start), Some(end)) = (
                word(64 + 8 * index),
                word(68 + 8 * index),
                word(68 + 8 * (index + 1)),
            ) else {
                break;
            };
            if timestamp > i32::MAX as u32
                || index > 0 && timestamp < previous
                || (start as usize) < index_end
                || end <= start
                || (end as usize) > bytes.len()
            {
                break;
            }
            let jpeg = &bytes[start as usize..end as usize];
            let Ok(decoded) = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
            else {
                break;
            };
            if decoded.width() != width {
                break;
            }
            frames.push((u64::from(timestamp) * u64::from(scale), jpeg.to_vec()));
            previous = timestamp;
        }
        if frames.len() == count as usize {
            return Some(frames);
        }
    }
    None
}

async fn seed_chapter_images(host: &Host, item: &Value) {
    for chapter in item["Chapters"].as_array().unwrap() {
        let file = chapter_image(host, item, chapter);
        tokio::fs::create_dir_all(file.parent().unwrap())
            .await
            .unwrap();
        let ticks = chapter["StartPositionTicks"].as_u64().unwrap();
        let image =
            image::RgbImage::from_pixel(320, 180, image::Rgb([(ticks % 251) as u8, 80, 160]));
        let mut bytes = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&image)
            .unwrap();
        tokio::fs::write(file, bytes).await.unwrap();
    }
}

fn native_chapter_images(host: &Host, item: &Value) -> Vec<bool> {
    let frames = native_bif_frames(item);
    item["Chapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chapter| {
            frames
                .as_ref()
                .and_then(|frames| {
                    frames.iter().find(|(ms, _)| {
                        ms.checked_mul(10_000).is_some_and(|ticks| {
                            ticks >= chapter["StartPositionTicks"].as_u64().unwrap()
                        })
                    })
                })
                .is_some_and(|(_, jpeg)| {
                    std::fs::read(chapter_image(host, item, chapter))
                        .is_ok_and(|stored| stored == *jpeg)
                })
        })
        .collect()
}

async fn delete_item_resources(host: &Host, item: &Value) {
    let metadata = item_metadata_directory(host, item);
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(metadata).await.unwrap();
    }
    let path = std::path::Path::new(item["Path"].as_str().unwrap());
    let stem = path.file_stem().unwrap().to_str().unwrap();
    let mut entries = tokio::fs::read_dir(path.parent().unwrap()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".bif")
            && name
                .strip_prefix(stem)
                .is_some_and(|suffix| suffix.starts_with('-') || suffix.starts_with('.'))
        {
            tokio::fs::remove_file(entry.path()).await.unwrap();
        }
    }
}

async fn details(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let mut item = host.item.lock().await.clone();
    let fields = query.get("Fields").map(String::as_str).unwrap_or("");
    if host.wrong_final_language.load(Ordering::SeqCst)
        && (host.starts.load(Ordering::SeqCst) > 0 || host.refreshes.load(Ordering::SeqCst) > 0)
        && fields.contains("MediaStreams")
        && !fields.contains("Chapters")
        && !fields.contains("PartCount")
        && !fields.contains("Name")
    {
        item["MediaStreams"][1]["Language"] = json!("chi");
        item["MediaSources"][0]["MediaStreams"][1]["Language"] = json!("chi");
    }
    Json(item)
}

async fn clear(State(host): State<Host>) -> StatusCode {
    host.clears.fetch_add(1, Ordering::SeqCst);
    host.clear_ids.lock().await.push("61616".into());
    let mut item = host.item.lock().await;
    delete_item_resources(&host, &item).await;
    item["MediaStreams"] = json!([]);
    item["MediaSources"][0]["MediaStreams"] = json!([]);
    item["Chapters"] = json!([]);
    item["Size"] = json!(0);
    drop(item);
    tokio::fs::remove_file(host.directory.join("61616-mediainfo.json"))
        .await
        .unwrap();
    StatusCode::NO_CONTENT
}

async fn native_start(State(host): State<Host>) -> StatusCode {
    assert_eq!(
        host.settings.lock().await["PersistMediaInfoMode"],
        "Restore"
    );
    let data: Value = serde_json::from_slice(
        &tokio::fs::read(host.directory.join("61616-mediainfo.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let source = &data[0]["MediaSourceInfo"];
    let mut item = host.item.lock().await;
    item["MediaStreams"] = source["MediaStreams"].clone();
    item["MediaSources"][0]["MediaStreams"] = source["MediaStreams"].clone();
    item["Chapters"] = data[0]["Chapters"].clone();
    item["Size"] = source["Size"].clone();
    host.starts.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn file_refresh(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> StatusCode {
    assert_eq!(query["Recursive"], "true");
    assert_eq!(query["ReplaceAllMetadata"], "false");
    assert_eq!(query["ReplaceAllImages"], "false");
    // A directory notification does not replace existing media information.
    host.refreshes.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn exact_refresh(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> StatusCode {
    assert_eq!(query["Recursive"], "false");
    assert_eq!(query["MetadataRefreshMode"], "FullRefresh");
    assert_eq!(query["ImageRefreshMode"], "ValidationOnly");
    assert_eq!(query["ReplaceAllMetadata"], "false");
    assert_eq!(query["ReplaceAllImages"], "false");
    host.refresh_logs
        .lock()
        .await
        .push("2026-11-09 00:00:01.000 RefreshItem Start: 61616 fixture".into());
    let mut item = host.item.lock().await;
    if item["MediaStreams"].as_array().unwrap().is_empty()
        && host.settings.lock().await["PersistMediaInfoMode"] == "Restore"
    {
        let info: Value = serde_json::from_slice(
            &tokio::fs::read(host.directory.join("61616-mediainfo.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        item["MediaStreams"] = info[0]["MediaSourceInfo"]["MediaStreams"].clone();
        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        item["Size"] = info[0]["MediaSourceInfo"]["Size"].clone();
        item["Chapters"] = info[0]["Chapters"].clone();
        let images = native_chapter_images(&host, &item);
        for (chapter, exists) in item["Chapters"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(images)
        {
            if exists {
                chapter["ImageTag"] = json!("old-thumbnail");
            }
        }
    }
    host.refreshes.fetch_add(1, Ordering::SeqCst);
    host.refresh_logs
        .lock()
        .await
        .push("2026-11-09 00:00:02.000 RefreshItem Complete: 61616 fixture".into());
    StatusCode::NO_CONTENT
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    host: Host,
    server: tokio::task::JoinHandle<()>,
    original: Value,
    original_settings: Value,
}

impl Fixture {
    async fn new(wrong_final_language: bool) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("title_reverse_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let query = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&query).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let original: Value =
            serde_json::from_str(include_str!("fixtures/tiga_original_track_titles.json")).unwrap();
        let path = directory.path().join("61616.strm");
        tokio::fs::write(&path, "/cd2/61616.mkv").await.unwrap();
        tokio::fs::write(
            directory.path().join("61616-mediainfo.json"),
            serde_json::to_vec(&original).unwrap(),
        )
        .await
        .unwrap();
        let mut streams = original[0]["MediaSourceInfo"]["MediaStreams"].clone();
        for stream in streams.as_array_mut().unwrap() {
            if stream["Type"] == "Subtitle" && stream["Language"] == "chi" {
                stream["Title"] = json!(format!("Emby 用户修改的字幕{}", stream["Index"]));
                if stream["Index"] == 13 {
                    stream["Language"] = json!("cht");
                }
            }
        }
        let original_settings = json!({"ExclusiveControlFeatures":"NONE","PersistMediaInfoMode":"Default","LibraryScope":"library","MediaInfoJsonRootFolder":"","IncludeExtra":false});
        let host = Host {
            directory: directory.path().into(),
            metadata_root: directory.path().join("metadata"),
            item: Arc::new(Mutex::new(
                json!({"Id":"61616","Guid":"00000000-0000-0000-0000-00000000f0b0","PresentationUniqueKey":"fixture-key-61616","Type":"Movie","Name":"迪迦奥特曼：最终圣战","Path":path,"Size":original[0]["MediaSourceInfo"]["Size"],"PartCount":1,"MediaStreams":streams,"MediaSources":[{"Id":"mediasource_61616","ItemId":"61616","Path":"/cd2/61616.mkv","MediaStreams":streams}],"Chapters":original[0]["Chapters"]}),
            )),
            settings: Arc::new(Mutex::new(original_settings.clone())),
            starts: Arc::default(),
            clears: Arc::default(),
            refreshes: Arc::default(),
            config_writes: Arc::default(),
            wrong_final_language: Arc::new(AtomicBool::new(wrong_final_language)),
            refresh_logs: Arc::new(Mutex::new(vec![
                "2026-11-09 00:00:00.000 fixture log anchor".into(),
            ])),
            clear_ids: Arc::default(),
        };
        tokio::fs::create_dir_all(&host.metadata_root)
            .await
            .unwrap();
        seed_chapter_images(&host, &*host.item.lock().await).await;
        let api = Router::new()
            .route("/Users/user/Items/61616", get(details))
            .route("/Items", get(|State(host):State<Host>,Query(query):Query<HashMap<String,String>>|async move {
                let items = if query.get("Path").is_some_and(|path| PathBuf::from(path) == host.directory) {
                    vec![json!({"Id":"directory","IsFolder":true,"Path":host.directory})]
                } else { vec![host.item.lock().await.clone()] };
                Json(json!({"Items":items,"TotalRecordCount":items.len()}))
            }))
            .route("/Items/directory/Refresh", post(file_refresh))
            .route("/Items/61616/Refresh", post(exact_refresh))
            .route("/Items/61616/ThumbnailSet", get(|State(host): State<Host>| async move { let frames=native_bif_frames(&*host.item.lock().await).unwrap_or_default();Json(json!({"Thumbnails":frames.iter().map(|(ms,_)|json!({"Time":ms,"Tag":"old-thumbnail"})).collect::<Vec<_>>()})) }))
            .route("/Users/user/Items/61616/SpecialFeatures", get(|| async { Json(json!([])) }))
            .route("/Videos/61616/AdditionalParts", get(|| async { Json(json!({"Items":[]})) }))
            .route("/Items/61616/ClearMediaInfo", post(clear))
            .route("/ScheduledTasks", get(|State(host): State<Host>| async move {
                Json(json!([{"Id":"media","Key":"MediaInfoExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":host.starts.load(Ordering::SeqCst).to_string(),"Status":"Completed"}}]))
            }))
            .route("/ScheduledTasks/Running/media", post(native_start))
            .route("/Library/VirtualFolders", get(|State(host): State<Host>| async move {
                Json(json!([{"ItemId":"library","Locations":[host.directory]}]))
            }))
            .route("/System/Info", get(|State(host): State<Host>| async move { Json(json!({"InternalMetadataPath":host.metadata_root})) }))
            .route("/System/Logs/embyserver.txt/Lines", get(|State(host): State<Host>| async move {let mut lines=host.refresh_logs.lock().await.clone();lines.reverse();Json(json!({"TotalRecordCount":lines.len(),"Items":lines}))}))
            .route("/web/ConfigurationPages", get(||async {Json(json!([{"Name":"MediaInfoExtractPageView","Href":"configurationpage?PageId=plugin:MediaInfoExtractPageView"}]))}))
            .route("/UI/View", get(|State(host): State<Host>|async move {Json(json!({"EditObjectContainer":{"Object":host.settings.lock().await.clone()}}))}))
            .route("/UI/Command", post(|State(host): State<Host>,Json(body):Json<Value>|async move {host.config_writes.fetch_add(1,Ordering::SeqCst);*host.settings.lock().await=serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();StatusCode::NO_CONTENT}))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":directory.path(),"monitor_sha1_pc_search":false,"emby_metadata_path":""})).await.unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('113094','Movie',$1,$2,$3,$3)")
            .bind(json!(["61616"])).bind(json!([{"emby_item_id":"61616","path":path,"size_bytes":original[0]["MediaSourceInfo"]["Size"]}])).bind(json!([null])).execute(&state.db).await.unwrap();
        Self {
            state,
            admin,
            schema,
            directory,
            host,
            server,
            original,
            original_settings,
        }
    }

    async fn run(&self) -> Value {
        let state = self.state.clone();
        let task = self
            .state
            .jobs
            .submit("原生标题与语言回填", move |context| async move {
                titles::fill_in_job(&state, &["61616".into()], true, &context).await
            })
            .await
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let result = self.state.jobs.result(&task).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        match outcome {
            Ok(result) => result,
            Err(error) => panic!(
                "{error}; task status: {}",
                self.state.jobs.result(&task).await.unwrap()
            ),
        }
    }

    async fn single_audio(&self, language: &str, title: &str) {
        let mut info = self.original.clone();
        let streams = info[0]["MediaSourceInfo"]["MediaStreams"]
            .as_array_mut()
            .unwrap();
        streams.truncate(2);
        streams[1]["Language"] = json!(language);
        streams[1]["Title"] = json!(title);
        let streams = json!(streams);
        let mut item = self.host.item.lock().await;
        item["MediaStreams"] = streams.clone();
        item["MediaSources"][0]["MediaStreams"] = streams;
        drop(item);
        tokio::fs::write(
            self.directory.path().join("61616-mediainfo.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .await
        .unwrap();
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

    async fn assert_file_refresh_only(&self) {
        assert_eq!(
            self.host.clears.load(Ordering::SeqCst),
            self.host.refreshes.load(Ordering::SeqCst)
        );
        assert_eq!(self.host.starts.load(Ordering::SeqCst), 0);
        assert_eq!(*self.host.settings.lock().await, self.original_settings);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn original_tiga_audio_titles_restore_languages_and_preserve_live_chinese_subtitles() {
    let f = Fixture::new(false).await;
    let result = f.run().await;
    let live = f.host.item.lock().await.clone();
    let saved: Value = serde_json::from_slice(
        &tokio::fs::read(f.directory.path().join("61616-mediainfo.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let report = &result["result"];
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(report["completed"], true, "{report}");
    assert_eq!(report["pending"], json!([]), "{report}");
    assert_eq!(report["skipped_chinese_streams"], 9);
    let streams = live["MediaStreams"].as_array().unwrap();
    assert_eq!(streams[1]["Language"], "yue");
    assert_eq!(streams[1]["Title"], "粤语");
    assert_eq!(streams[2]["Title"], "台配国语");
    assert_eq!(streams[3]["Title"], "东和台配国语");
    assert_eq!(streams[4]["Title"], "日语");
    for stream in streams.iter().filter(|stream| {
        stream["Type"] == "Subtitle"
            && ["chi", "cht"].contains(&stream["Language"].as_str().unwrap_or(""))
    }) {
        assert_eq!(
            stream["Title"],
            format!("Emby 用户修改的字幕{}", stream["Index"])
        );
        assert_eq!(
            stream["Language"],
            if stream["Index"] == 13 { "cht" } else { "chi" }
        );
    }
    for (original, updated) in f.original[0]["MediaSourceInfo"]["MediaStreams"]
        .as_array()
        .unwrap()
        .iter()
        .zip(streams)
    {
        for key in [
            "Codec",
            "Index",
            "Channels",
            "IsDefault",
            "IsForced",
            "IsExternal",
        ] {
            assert_eq!(original[key], updated[key], "{key}");
        }
    }
    assert_eq!(live["Chapters"], f.original[0]["Chapters"]);
    assert_eq!(saved[0]["Chapters"], f.original[0]["Chapters"]);
    assert_eq!(*f.host.settings.lock().await, f.original_settings);
    assert_eq!(f.host.refreshes.load(Ordering::SeqCst), 1);
    f.assert_file_refresh_only().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn matching_titles_do_not_complete_when_final_language_readback_is_wrong() {
    let f = Fixture::new(true).await;
    let result = f.run().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["completed"], false, "{result}");
    assert!(
        result["result"]["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["reason"].as_str().unwrap_or("").contains("语言")),
        "{result}"
    );
    assert_eq!(*f.host.settings.lock().await, f.original_settings);
    assert_eq!(f.host.refreshes.load(Ordering::SeqCst), 1);
    f.assert_file_refresh_only().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn local_language_corrections_are_not_forgotten_when_file_refresh_is_retried() {
    let f = Fixture::new(false).await;
    f.single_audio("yue", "粤语").await;
    {
        let mut item = f.host.item.lock().await;
        item["MediaStreams"][1]["Language"] = json!("chi");
        item["MediaSources"][0]["MediaStreams"][1]["Language"] = json!("chi");
    }
    let result = f.run().await;
    assert_eq!(result["result"]["completed"], true, "{result}");
    assert_eq!(result["result"]["changed_streams"], 0, "{result}");
    let live = f.host.item.lock().await.clone();
    assert_eq!(live["MediaStreams"][1]["Title"], "粤语");
    assert_eq!(live["MediaStreams"][1]["Language"], "yue", "{result}");
    assert_eq!(f.host.refreshes.load(Ordering::SeqCst), 1, "{result}");
    f.assert_file_refresh_only().await;
    assert_eq!(*f.host.settings.lock().await, f.original_settings);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn equivalent_language_aliases_do_not_trigger_unnecessary_file_refresh() {
    let f = Fixture::new(false).await;
    db::save_setting(
        &f.state.db,
        "language_mapping",
        &json!([{"label":"英语","value":"en","aliases":[]}]),
    )
    .await
    .unwrap();
    for (local_language, live_language, title) in [
        ("eng", "en", "英语"),
        ("chi", "zho", "国语"),
        ("yue", "zh-yue", "粤语"),
    ] {
        f.single_audio(local_language, title).await;
        {
            let mut item = f.host.item.lock().await;
            item["MediaStreams"][1]["Language"] = json!(live_language);
            item["MediaSources"][0]["MediaStreams"][1]["Language"] = json!(live_language);
        }
        let result = f.run().await;
        assert_eq!(result["result"]["completed"], true, "{result}");
        assert_eq!(result["result"]["changed_streams"], 0, "{result}");
        assert_eq!(f.host.refreshes.load(Ordering::SeqCst), 0, "{result}");
        f.assert_file_refresh_only().await;
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn commentary_without_number_fills_title_without_a_language_prefix() {
    let f = Fixture::new(false).await;
    f.single_audio("eng", "COMMENTARY").await;
    let result = f.run().await;
    assert_eq!(result["result"]["completed"], true, "{result}");
    let live = f.host.item.lock().await.clone();
    assert_eq!(live["MediaStreams"][1]["Title"], "导评");
    assert_eq!(live["MediaStreams"][1]["Language"], "eng");
    assert_eq!(*f.host.settings.lock().await, f.original_settings);
    assert_eq!(f.host.refreshes.load(Ordering::SeqCst), 1);
    f.assert_file_refresh_only().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn numbered_commentary_title_is_saved_while_unknown_language_remains_incomplete() {
    let f = Fixture::new(false).await;
    f.single_audio("und", "COMMENTARY3").await;
    let result = f.run().await;
    assert_eq!(result["result"]["completed"], false, "{result}");
    assert_eq!(result["result"]["failed"], 0, "{result}");
    assert_eq!(result["result"]["changed_streams"], 1, "{result}");
    assert_eq!(
        result["result"]["pending"].as_array().unwrap().len(),
        1,
        "{result}"
    );
    let live = f.host.item.lock().await.clone();
    assert_eq!(live["MediaStreams"][1]["Title"], "导评3");
    assert_eq!(live["MediaStreams"][1]["Language"], "und");
    assert_eq!(*f.host.settings.lock().await, f.original_settings);
    assert_eq!(f.host.refreshes.load(Ordering::SeqCst), 1);
    f.assert_file_refresh_only().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn title_cache_failure_is_reported_as_pending_after_local_json_save() {
    let f = Fixture::new(false).await;
    sqlx::query("UPDATE media_metadata SET file_sha1_json=$1")
        .bind(json!([format!("{:040X}", 42)]))
        .execute(&f.state.db)
        .await
        .unwrap();
    sqlx::query("DROP TABLE p115_mediainfo_cache")
        .execute(&f.state.db)
        .await
        .unwrap();
    let result = f.run().await;
    let saved: Value = serde_json::from_slice(
        &tokio::fs::read(f.directory.path().join("61616-mediainfo.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    f.close().await;
    assert_eq!(result["result"]["completed"], false, "{result}");
    assert!(
        result["result"]["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pending| pending["reason"]
                .as_str()
                .unwrap_or("")
                .contains("缓存同步失败")),
        "{result}"
    );
    assert!(
        result["result"]["changed_streams"].as_u64().unwrap() > 0,
        "{result}"
    );
    assert_eq!(
        saved[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"],
        "粤语"
    );
}
