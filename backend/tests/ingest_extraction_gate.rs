use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, db, media_recovery, p115, webhook};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_real_attempts_finish_only_failed_target_and_exclude_new_noop_and_unvisited_history()
 {
    let mut fixture = Fixture::new(&["complete", "complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let identity = fixture
        .cloud_version("10001", &"A".repeat(40), "h264")
        .await;
    let mut first = fixture.cloud_event(identity, "initial-before-history-scope");
    first["files"] = json!([fixture.files[0]]);
    let first_result = fixture.run_moviepilot_event(first).await;
    assert_eq!(first_result["state"], "completed", "{first_result}");
    let original_json = std::fs::read(fixture.sidecar_path()).unwrap();
    let original_native = fixture.host.items.lock().await["101"].clone();
    let original_imports = fixture.host.file_imports.lock().await.len();
    let original_source = std::fs::read_to_string(fixture.files[1].as_str().unwrap()).unwrap();
    let original_identity = json!({"id":"102","path":fixture.files[1],"source":original_source.trim(),"entity":null,"cloud":null,"relative":null});
    let file_key = format!(
        "moviepilot_file:{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(fixture.files[1].to_string().as_bytes())
    );
    std::fs::write(fixture.files[1].as_str().unwrap(), "/cd2/102.mkv").unwrap();
    fixture.host.items.lock().await.get_mut("102").unwrap()["MediaSources"][0]["Path"] =
        json!("/cd2/102.mkv");
    let unvisited = std::path::Path::new(fixture.files[0].as_str().unwrap())
        .parent()
        .unwrap()
        .join("unvisited.strm");
    std::fs::write(&unvisited, "/cd2/unvisited.mkv").unwrap();
    let mut event = json!({"source":"moviepilot","media_type":"movie","tmdb_id":"101",
        "files":[fixture.files[0],fixture.files[1],unvisited],"batch_key":"history-scope-real-attempts"});
    let history_ids =
        emby_toolkit::media_organize_history::begin_moviepilot(&fixture.state, &event)
            .await
            .unwrap();
    assert_eq!(history_ids.len(), 3);
    event["history_ids"] = json!(history_ids);
    let work = fixture.state.clone();
    let task = fixture
        .state
        .jobs
        .submit("MoviePilot真实尝试范围", move |context| async move {
            // An interrupted attempt recorded the old source before replacement.
            // Missing optional fingerprints are not an admission failure.
            context
                .save_plan(&file_key, &json!({"identity":original_identity}))
                .await?;
            let result = Box::pin(webhook::process_moviepilot(&work, &event, &context)).await;
            emby_toolkit::media_organize_history::finish_moviepilot(
                &work, &event, &context, &result,
            )
            .await?;
            result
        })
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let result = fixture.state.jobs.result(&task).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(outcome["state"], "failed", "{outcome}");
    let remaining: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(h) FROM media_organize_history h WHERE id=ANY($1)")
            .bind(&history_ids)
            .fetch_all(&fixture.state.db)
            .await
            .unwrap();
    assert_eq!(remaining.len(), 1, "{remaining:?}");
    assert_eq!(remaining[0]["item_id"], "102");
    assert_eq!(remaining[0]["status"], "failed");
    assert_eq!(
        std::fs::read(fixture.sidecar_path()).unwrap(),
        original_json
    );
    assert_eq!(fixture.host.items.lock().await["101"], original_native);
    assert_eq!(
        fixture.host.file_imports.lock().await.len(),
        original_imports
    );
    fixture.close().await;
}

type ChapterImages = Arc<Mutex<BTreeMap<(String, u64), Vec<u8>>>>;

fn import_fixture_info(item: &mut Value, info: &Value) {
    let index = item["MediaSources"]
        .as_array()
        .unwrap()
        .iter()
        .position(|source| {
            source["ItemId"] == item["Id"]
                || source["ItemId"].is_null()
                    && source["Id"] == format!("mediasource_{}", item["Id"].as_str().unwrap())
        })
        .expect("native extraction must update the selected source");
    let mut source = info[0]["MediaSourceInfo"].clone();
    for field in ["Id", "ItemId", "Path", "Type"] {
        if let Some(value) = item["MediaSources"][index].get(field) {
            source[field] = value.clone();
        }
    }
    item["MediaSources"][index] = source;
    item["MediaStreams"] = info[0]["MediaSourceInfo"]["MediaStreams"].clone();
    item["Chapters"] = info[0]["Chapters"].clone();
    for field in ["Size", "RunTimeTicks"] {
        if let Some(value) = info[0]["MediaSourceInfo"].get(field) {
            item[field] = value.clone();
        }
    }
}

#[derive(Clone, Default)]
struct Host {
    items: Arc<Mutex<HashMap<String, Value>>>,
    title_failures: Arc<Mutex<Vec<String>>>,
    refresh_failures: Arc<Mutex<Vec<String>>>,
    refresh_attempts: Arc<Mutex<Vec<(String, String)>>>,
    refresh_queries: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
    native_starts: Arc<Mutex<Vec<String>>>,
    chain_enabled: Arc<AtomicBool>,
    stage_order: Arc<Mutex<Vec<String>>>,
    intro_runs: Arc<Mutex<u64>>,
    year_readbacks: Arc<AtomicUsize>,
    info_runs: Arc<Mutex<u64>>,
    logs: Arc<Mutex<Vec<String>>>,
    configuration: Arc<Mutex<Value>>,
    clear_chapters_on_final_refresh: Arc<AtomicBool>,
    other_chapter_loss_target: Arc<Mutex<Option<String>>>,
    chapter_images: ChapterImages,
    bifs: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    resource_fault: Arc<Mutex<String>>,
    metadata_hook_reached: Arc<tokio::sync::Notify>,
    metadata_hook_release: Arc<tokio::sync::Notify>,
    cover: Arc<Mutex<Option<std::path::PathBuf>>>,
    cloud_files: Arc<Mutex<Vec<Value>>>,
    cloud_file_reads: Arc<AtomicUsize>,
    cloud_swap_after_read: Arc<Mutex<Option<(usize, Value)>>>,
    cloud_swap_after_import: Arc<Mutex<Option<Value>>>,
    cloud_unavailable: Arc<AtomicBool>,
    file_imports: Arc<Mutex<Vec<String>>>,
    sync_imports: Arc<Mutex<Vec<String>>>,
    skip_sync_import: Arc<AtomicBool>,
    enrich_sync_native: Arc<AtomicBool>,
    native_empty_titles: Arc<AtomicBool>,
    fail_final_sync_readback: Arc<AtomicBool>,
    skip_file_import: Arc<AtomicBool>,
    native_generated_info: Arc<Mutex<HashMap<String, Value>>>,
    probe_downloads: Arc<AtomicUsize>,
    deleted_chapter_images: Arc<Mutex<Vec<(String, u64)>>>,
    chapter_image_paths: Arc<Mutex<BTreeMap<(String, u64), std::path::PathBuf>>>,
    generated_intro_end: Arc<AtomicUsize>,
    wash_bif_paths: Arc<Mutex<HashMap<String, std::path::PathBuf>>>,
    corrupt_size_on_native_start: Arc<AtomicBool>,
}

async fn denied_probe_download(State(host): State<Host>) -> StatusCode {
    host.probe_downloads.fetch_add(1, Ordering::SeqCst);
    StatusCode::BAD_REQUEST
}

async fn delete_chapter_image(
    State(host): State<Host>,
    Path((id, index)): Path<(String, u64)>,
) -> StatusCode {
    host.deleted_chapter_images
        .lock()
        .await
        .push((id.clone(), index));
    host.chapter_images
        .lock()
        .await
        .remove(&(id.clone(), index));
    if let Some(path) = host
        .chapter_image_paths
        .lock()
        .await
        .remove(&(id.clone(), index))
    {
        std::fs::remove_file(path).unwrap();
    }
    if let Some(item) = host.items.lock().await.get_mut(&id)
        && let Some(chapter) = item["Chapters"]
            .as_array_mut()
            .and_then(|chapters| chapters.get_mut(index as usize))
    {
        chapter.as_object_mut().unwrap().remove("ImageTag");
    }
    StatusCode::NO_CONTENT
}

async fn cloud_files(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    if host.cloud_unavailable.load(Ordering::SeqCst) {
        return Json(json!({"state":false,"errno":20018}));
    }
    let files = match query.get("cid").map(String::as_str) {
        Some("0") => vec![json!({"cid":"9","name":"媒体库","fc":"0"})],
        Some("9") => {
            let read = host.cloud_file_reads.fetch_add(1, Ordering::SeqCst) + 1;
            let mut swap = host.cloud_swap_after_read.lock().await;
            if swap.as_ref().is_some_and(|(target, _)| *target == read) {
                let (_, replacement) = swap.take().unwrap();
                *host.cloud_files.lock().await = vec![replacement];
            }
            host.cloud_files.lock().await.clone()
        }
        _ => vec![],
    };
    Json(json!({"state":true,"count":files.len(),"data":files}))
}

async fn cloud_file(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let file = host
        .cloud_files
        .lock()
        .await
        .iter()
        .find(|file| query.get("file_id").is_some_and(|id| file["id"] == *id))
        .cloned();
    match file {
        Some(file) if !host.cloud_unavailable.load(Ordering::SeqCst) => {
            Json(json!({"state":true,"data":file}))
        }
        _ => Json(json!({"state":false,"errno":20018})),
    }
}
async fn details(
    State(host): State<Host>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if host.fail_final_sync_readback.load(Ordering::SeqCst)
        && !host.sync_imports.lock().await.is_empty()
        && query.get("Fields").is_some_and(|fields| {
            fields == "Id,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters"
        })
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if host.chain_enabled.load(Ordering::SeqCst) {
        if id == "101"
            && query
                .get("Fields")
                .is_some_and(|fields| fields.split(',').any(|field| field == "SeasonId"))
        {
            let mut stages = host.stage_order.lock().await;
            if !stages.iter().any(|stage| stage == "credits-read") {
                stages.push("credits-read".into());
            }
        }
        if id == "301"
            && query
                .get("Fields")
                .is_some_and(|fields| fields.split(',').any(|field| field == "EndDate"))
        {
            host.year_readbacks.fetch_add(1, Ordering::SeqCst);
        }
    }
    if query.get("Fields").is_some_and(|fields| {
        [
            "Name",
            "MediaSources",
            "MediaStreams",
            "PartCount",
            "ExtraType",
        ]
        .iter()
        .all(|expected| fields.split(',').any(|field| field == *expected))
    }) && !query
        .get("Fields")
        .is_some_and(|fields| fields.split(',').any(|field| field == "Guid"))
        && host.title_failures.lock().await.contains(&id)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match host.items.lock().await.get(&id) {
        Some(item) => Json(metadata_only_mock_controls(
            item,
            query.get("Fields").map_or("", String::as_str),
        ))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn items(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    // Emby lists only root children by default, so a whole-library media
    // inventory is empty without Recursive=true.
    if query.get("IncludeItemTypes").is_some_and(|types| types == "Movie,Episode")
        && !["Path", "Ids", "ParentId"].iter().any(|key| query.contains_key(*key))
        && query.get("Recursive").is_none_or(|recursive| recursive != "true")
    {
        return Json(json!({"TotalRecordCount":0,"Items":[]}));
    }
    let items = host
        .items
        .lock()
        .await
        .values()
        .filter(|item| {
            (!query.contains_key("Path")
                && !query.contains_key("Ids")
                && !query.contains_key("ParentId"))
                || query.get("Path").is_some_and(|path| item["Path"] == *path)
                || query.get("Ids").is_some_and(|ids| {
                    ids.split(',')
                        .any(|id| item["Id"] == id || item["Guid"] == id)
                })
                || query.get("ParentId").is_some_and(|parent| {
                    item["ParentId"] == *parent || item["SeriesId"] == *parent
                })
        })
        .filter(|item| {
            query
                .get("IncludeItemTypes")
                .is_none_or(|types| types.split(',').any(|kind| item["Type"] == kind))
        })
        .cloned()
        .collect::<Vec<_>>();
    Json(json!({"TotalRecordCount":items.len(),"Items":items}))
}
async fn refresh(
    State(host): State<Host>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> StatusCode {
    host.refresh_queries
        .lock()
        .await
        .push((id.clone(), query.clone()));
    let mode = query
        .get("MetadataRefreshMode")
        .cloned()
        .unwrap_or_default();
    host.refresh_attempts
        .lock()
        .await
        .push((id.clone(), mode.clone()));
    if mode == "FullRefresh" && host.refresh_failures.lock().await.contains(&id) {
        return StatusCode::BAD_REQUEST;
    }
    if mode == "FullRefresh" && id == "101"
        && *host.resource_fault.lock().await == "metadata-cloud-swap"
        && let Some(next) = host.cloud_swap_after_import.lock().await.take()
    {
        *host.cloud_files.lock().await = vec![next];
    }
    if host.chain_enabled.load(Ordering::SeqCst) && id == "301" {
        if mode == "FullRefresh" && host.clear_chapters_on_final_refresh.load(Ordering::SeqCst) {
            host.items.lock().await.get_mut("101").unwrap()["Chapters"] = json!([]);
        }
        if mode == "FullRefresh"
            && query.get("Recursive").is_some_and(|value| value == "true")
            && let Some(target) = host.other_chapter_loss_target.lock().await.as_ref()
        {
            host.items.lock().await.get_mut(target).unwrap()["Chapters"] = json!([]);
        }
        if mode == "FullRefresh" {
            let fault = host.resource_fault.lock().await.clone();
            match fault.as_str() {
                "jpeg" => {
                    host.chapter_images
                        .lock()
                        .await
                        .insert(("101".into(), 0), jpeg([0, 0, 255]));
                }
                "bif" => {
                    host.bifs
                        .lock()
                        .await
                        .insert("101".into(), bif(&jpeg([0, 0, 255])));
                }
                "new-cover-and-chapter-loss" => {
                    let mut items = host.items.lock().await;
                    let episode = items.get_mut("101").unwrap();
                    episode["Name"] = json!("新的分集标题");
                    episode["Overview"] = json!("最终刷新后的新简介");
                    episode["Chapters"] = json!([]);
                    std::fs::write(host.cover.lock().await.as_ref().unwrap(), jpeg([0, 0, 255]))
                        .unwrap();
                }
                "native-db-markers" | "native-db-markers-crash" => {
                    let mut items = host.items.lock().await;
                    let episode = items.get_mut("101").unwrap();
                    episode["Chapters"].as_array_mut().unwrap().extend([
                        json!({"Name":"IntroStart#DB","MarkerType":"IntroStart","StartPositionTicks":0}),
                        json!({"Name":"IntroEnd#DB","MarkerType":"IntroEnd","StartPositionTicks":1240000000_u64}),
                        json!({"Name":"CreditsStart#DB","MarkerType":"CreditsStart","StartPositionTicks":10660000000_u64}),
                    ]);
                    let path = std::path::Path::new(episode["Path"].as_str().unwrap());
                    let path = path.with_file_name("101-mediainfo.json");
                    let mut info: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    info[0]["Chapters"] = episode["Chapters"].clone();
                    std::fs::write(&path, serde_json::to_vec(&info).unwrap()).unwrap();
                    drop(items);
                    if fault == "native-db-markers-crash" {
                        host.metadata_hook_reached.notify_one();
                        host.metadata_hook_release.notified().await;
                    }
                }
                _ => {}
            }
        }
        host.stage_order.lock().await.push(
            if mode == "FullRefresh" {
                "metadata-refresh"
            } else {
                "nfo-refresh"
            }
            .into(),
        );
    }
    if mode == "Default" && query.contains_key("ReplaceThumbnailImages") {
        host.file_imports.lock().await.push(id.clone());
        if host.skip_file_import.load(Ordering::SeqCst) {
            host.logs.lock().await.extend([
                format!("Info fixture: RefreshItem Start: {id} metadata"),
                format!("Info fixture: RefreshItem Complete: {id} metadata"),
            ]);
            return StatusCode::NO_CONTENT;
        }
        let mut items = host.items.lock().await;
        let item = items.get_mut(&id).unwrap();
        let path = std::path::Path::new(item["Path"].as_str().unwrap())
            .with_file_name(format!("{id}-mediainfo.json"));
        let info: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        import_fixture_info(item, &info);
        if host.native_empty_titles.load(Ordering::SeqCst) {
            for stream in item["MediaStreams"].as_array_mut().unwrap() {
                if stream["Title"] == "" {
                    stream.as_object_mut().unwrap().remove("Title");
                }
            }
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        }
        for (index, chapter) in item["Chapters"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            chapter["ChapterIndex"] = json!(index);
            if host
                .chapter_images
                .lock()
                .await
                .contains_key(&(id.clone(), index as u64))
            {
                chapter["ImageTag"] = json!("original-same-tag");
            }
        }
        if let Some(replacement) = host.cloud_swap_after_import.lock().await.take() {
            *host.cloud_files.lock().await = vec![replacement];
        }
        if host
            .wash_bif_paths
            .lock()
            .await
            .get(&id)
            .is_some_and(|path| !path.exists())
        {
            host.bifs.lock().await.remove(&id);
        }
    }
    host.logs.lock().await.extend([
        format!("Info fixture: RefreshItem Start: {id} metadata"),
        format!("Info fixture: RefreshItem Complete: {id} metadata"),
    ]);
    StatusCode::NO_CONTENT
}
async fn clear(State(host): State<Host>, Path(id): Path<String>) -> StatusCode {
    let mut items = host.items.lock().await;
    let item = items.get_mut(&id).unwrap();
    let path = std::path::Path::new(item["Path"].as_str().unwrap());
    std::fs::remove_file(path.with_file_name(format!("{id}-mediainfo.json"))).unwrap();
    let bif = path.with_file_name(format!("{id}-320-4.bif"));
    if bif.exists() {
        std::fs::remove_file(bif).unwrap();
    }
    if let Some(cover) = host.cover.lock().await.as_ref() {
        std::fs::remove_dir_all(cover.parent().unwrap()).unwrap();
    }
    item["MediaStreams"] = json!([]);
    item["MediaSources"][0]["MediaStreams"] = json!([]);
    item["Chapters"] = json!([]);
    item["Size"] = json!(0);
    item["RunTimeTicks"] = Value::Null;
    item["MediaSources"][0]["Size"] = json!(0);
    item["MediaSources"][0]["RunTimeTicks"] = Value::Null;
    item["MediaSources"][0]["Chapters"] = json!([]);
    StatusCode::NO_CONTENT
}

async fn sync_empty_media(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
    Json(info): Json<Value>,
) -> Json<Value> {
    let id = query
        .get("Id")
        .expect("Sync must select a native Id")
        .clone();
    host.sync_imports.lock().await.push(id.clone());
    let mut items = host.items.lock().await;
    let item = items.get_mut(&id).unwrap();
    let own = item["MediaSources"]
        .as_array()
        .unwrap()
        .iter()
        .position(|source| source["ItemId"] == id)
        .unwrap();
    if !host.skip_sync_import.load(Ordering::SeqCst) && item["Size"] == 0 {
        assert_eq!(
            item["Size"], 0,
            "the verified PRO importer only fills empty media"
        );
        let mut source = info[0]["MediaSourceInfo"].clone();
        source["Id"] = item["MediaSources"][own]["Id"].clone();
        source["ItemId"] = json!(id);
        source["Type"] = json!("Default");
        if host.enrich_sync_native.load(Ordering::SeqCst) {
            source["IsRemote"] = json!(false);
            source["MimeType"] = json!("video/x-matroska");
            source["Chapters"] = info[0]["Chapters"].clone();
            source["DefaultAudioStreamIndex"] = source["MediaStreams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stream| stream["Type"] == "Audio")
                .map(|stream| stream["Index"].clone())
                .unwrap_or(Value::Null);
        }
        if host.native_empty_titles.load(Ordering::SeqCst) {
            for stream in source["MediaStreams"].as_array_mut().unwrap() {
                if stream["Title"] == "" {
                    stream.as_object_mut().unwrap().remove("Title");
                }
            }
        }
        item["MediaStreams"] = source["MediaStreams"].clone();
        item["Size"] = source["Size"].clone();
        item["RunTimeTicks"] = source["RunTimeTicks"].clone();
        item["Chapters"] = info[0]["Chapters"].clone();
        item["MediaSources"][own] = source;
    }
    // PRO ExportDTO removes the native binding fields before returning or
    // writing the bundle; the native source itself keeps its identity.
    let mut exported_source = item["MediaSources"][own].clone();
    for field in ["Id", "ItemId", "Path", "Name"] {
        exported_source[field] = Value::Null;
    }
    if host.enrich_sync_native.load(Ordering::SeqCst) {
        // The pinned core exporter uses fillChapters=false and its source
        // constructor has no user-specific default stream selection.
        exported_source["Chapters"] = json!([]);
        exported_source["DefaultAudioStreamIndex"] = Value::Null;
    }
    let mut exported_chapters = item["Chapters"].clone();
    for chapter in exported_chapters.as_array_mut().unwrap() {
        chapter["ImageTag"] = Value::Null;
    }
    let exported = json!([{"MediaSourceInfo":exported_source,"Chapters":exported_chapters}]);
    let path = std::path::Path::new(item["Path"].as_str().unwrap());
    std::fs::write(
        path.with_file_name(format!("{id}-mediainfo.json")),
        serde_json::to_vec(&exported).unwrap(),
    )
    .unwrap();
    Json(exported)
}
async fn logs(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let logs = host.logs.lock().await;
    let items = if query.get("Limit").is_some_and(|limit| limit == "0") {
        vec![]
    } else {
        logs.iter().rev().cloned().collect::<Vec<_>>()
    };
    Json(json!({"TotalRecordCount":logs.len(),"Items":items}))
}
async fn config_view(State(host): State<Host>) -> Json<Value> {
    Json(json!({"EditObjectContainer":{"Object":host.configuration.lock().await.clone()}}))
}
async fn config_save(State(host): State<Host>, Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["CommandId"], "PageSave");
    *host.configuration.lock().await =
        serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
    Json(json!({}))
}

fn jpeg(color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(4, 4, image::Rgb(color));
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode_image(&image)
        .unwrap();
    bytes
}

fn bif(jpeg: &[u8]) -> Vec<u8> {
    let times = [0_u32, 20, 30, 75];
    let mut bytes = vec![0; 64];
    bytes[..8].copy_from_slice(b"\x89BIF\r\n\x1a\n");
    bytes[12..16].copy_from_slice(&(times.len() as u32).to_le_bytes());
    bytes[16..24].copy_from_slice(&1000_u64.to_le_bytes());
    let mut offset = 64 + (times.len() + 1) * 8;
    for time in times {
        bytes.extend(time.to_le_bytes());
        bytes.extend((offset as u32).to_le_bytes());
        offset += jpeg.len();
    }
    bytes.extend(u32::MAX.to_le_bytes());
    bytes.extend((offset as u32).to_le_bytes());
    for _ in times {
        bytes.extend(jpeg);
    }
    bytes
}

fn pro_swagger_with_file_refresh() -> Value {
    let mut contract: Value =
        serde_json::from_str(include_str!("fixtures/pro_sync_contract.json")).unwrap();
    contract["paths"]["/Items/{Id}/Refresh"] = json!({"post":{"parameters":[
        {"name":"Id","in":"path"},{"name":"MetadataRefreshMode","in":"query"},
        {"name":"ImageRefreshMode","in":"query"},{"name":"ReplaceAllMetadata","in":"query"},
        {"name":"ReplaceAllImages","in":"query"}
    ]}});
    contract
}
async fn tasks(State(host): State<Host>) -> Json<Value> {
    let mut tasks = json!([
        {"Id":"info","Name":"Extract MediaInfo","Key":"MediaInfoExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":host.info_runs.lock().await.to_string(),"Status":"Completed"}},
        {"Id":"thumb","Name":"Extract Video Thumbnail","Key":"VideoThumbnailExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":"previous","Status":"Completed"}}
    ]);
    if host.chain_enabled.load(Ordering::SeqCst) {
        tasks.as_array_mut().unwrap().push(json!({"Id":"intro","Name":"Extract Intro Fingerprint","Key":"IntroFingerprintExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":host.intro_runs.lock().await.to_string(),"Status":"Completed"}}));
    }
    Json(tasks)
}
async fn start(State(host): State<Host>, Path(id): Path<String>) -> StatusCode {
    if id == "info" {
        *host.info_runs.lock().await += 1;
        for (native_id, info) in host.native_generated_info.lock().await.iter() {
            let mut items = host.items.lock().await;
            let path = std::path::Path::new(items[native_id]["Path"].as_str().unwrap())
                .with_file_name(format!("{native_id}-mediainfo.json"));
            import_fixture_info(items.get_mut(native_id).unwrap(), info);
            std::fs::write(path, serde_json::to_vec(info).unwrap()).unwrap();
        }
        let mut items = host.items.lock().await;
        for item in items.values_mut() {
            if !matches!(item["Type"].as_str(), Some("Movie" | "Episode"))
                || emby_toolkit::media_preflight::valid_video(item)
            {
                continue;
            }
            let path = std::path::Path::new(item["Path"].as_str().unwrap());
            let sidecar = path.with_file_name(format!(
                "{}-mediainfo.json",
                path.file_stem().unwrap().to_string_lossy()
            ));
            if let Ok(bytes) = std::fs::read(sidecar) {
                let info: Value = serde_json::from_slice(&bytes).unwrap();
                import_fixture_info(item, &info);
            }
        }
        return StatusCode::NO_CONTENT;
    }
    host.native_starts.lock().await.push(id.clone());
    if host.corrupt_size_on_native_start.load(Ordering::SeqCst)
        && matches!(id.as_str(), "intro" | "thumb")
    {
        let mut items = host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        item["Size"] = json!(2000);
        item["MediaSources"][0]["Size"] = json!(2000);
    }
    if host.chain_enabled.load(Ordering::SeqCst) && id == "intro" {
        *host.intro_runs.lock().await += 1;
        let end = host.generated_intro_end.load(Ordering::SeqCst);
        if end > 0 {
            let mut items = host.items.lock().await;
            let item = items.get_mut("101").unwrap();
            for chapter in item["Chapters"].as_array_mut().unwrap() {
                if chapter["MarkerType"] == "IntroEnd" {
                    chapter["StartPositionTicks"] = json!(end);
                }
            }
            let path = std::path::Path::new(item["Path"].as_str().unwrap())
                .with_file_name("101-mediainfo.json");
            let mut info: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            info[0]["Chapters"] = item["Chapters"].clone();
            std::fs::write(path, serde_json::to_vec(&info).unwrap()).unwrap();
        }
        host.stage_order.lock().await.push("intro".into());
        host.logs.lock().await.extend([
            "Info fixture: IntroFingerprintExtract - Scheduled Task Execute".into(),
            "Info fixture: IntroFingerprintExtract - Number of episodes: 1".into(),
            "Info fixture: IntroFingerprintExtract - Number of seasons: 1".into(),
            "Info fixture: IntroFingerprintExtract - Episode Progress 1/1 - Task 1: episode".into(),
            "Info fixture: IntroFingerprintExtract - Season Progress 1/1 - Task 1: season".into(),
            "Info fixture: IntroFingerprintExtract - Scheduled Task Complete".into(),
        ]);
        return StatusCode::NO_CONTENT;
    }
    if host.chain_enabled.load(Ordering::SeqCst) && id == "thumb" {
        host.stage_order.lock().await.push("thumb".into());
    }
    // Stop the fixture immediately after proving the real native entry was reached.
    StatusCode::BAD_REQUEST
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    files: Vec<Value>,
    host: Host,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(modes: &[&str], enabled: bool, cover_warning: bool) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("extraction_gate_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&search).execute(connection).await?;
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
        sqlx::query("INSERT INTO person_metadata(tmdb_person_id,primary_name) VALUES(1,'演员')")
            .execute(&state.db)
            .await
            .unwrap();
        let host = Host {
            configuration: Arc::new(Mutex::new(
                json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"}),
            )),
            ..Host::default()
        };
        let mut files = vec![];
        for (index, mode) in modes.iter().enumerate() {
            let id = (101 + index).to_string();
            let root = directory.path().join(&id);
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join(format!("{id}.strm"));
            let playback = directory.path().join(format!("{id}.mkv"));
            std::fs::write(&playback, vec![0_u8; 1000]).unwrap();
            let playback = playback.to_string_lossy().into_owned();
            std::fs::write(&path, &playback).unwrap();
            std::fs::write(root.join("poster.jpg"), "existing").unwrap();
            let mut streams = vec![
                json!({"Type":"Video","Codec":"hevc","Width":1920,"Height":1080}),
                json!({"Type":"Audio","Codec":"aac","Language":"jpn","Title":"日语"}),
            ];
            for ordinal in 1..=9 {
                streams.push(json!({"Type":"Subtitle","Codec":"subrip","Language":"chi","Title":format!("简中特效{ordinal}")}));
            }
            if *mode == "pending" {
                streams
                    .push(json!({"Type":"Subtitle","Codec":"subrip","Language":"und","Title":""}));
            }
            if *mode == "failed" {
                host.title_failures.lock().await.push(id.clone());
            }
            if *mode == "refresh_failed" {
                host.refresh_failures.lock().await.push(id.clone());
            }
            let item = json!({"Id":id,"Type":"Movie","PresentationUniqueKey":format!("key-{id}"),"Name":format!("电影{id}"),"Path":path,"PartCount":1,"Size":1000,"Chapters":[],"ProviderIds":{"Tmdb":id},"People":[],"Genres":[],"MediaStreams":streams,"MediaSources":[{"Id":format!("mediasource_{id}"),"ItemId":id,"Path":playback,"Size":1000,"Container":"mkv","MediaStreams":streams}]});
            host.items.lock().await.insert(id.clone(), item);
            let info = json!([{"MediaSourceInfo":{"Path":playback,"Size":1000,"Container":"mkv","RunTimeTicks":600000000_u64,"MediaStreams":streams},"Chapters":[]}]);
            std::fs::write(
                root.join(format!("{id}-mediainfo.json")),
                serde_json::to_vec(&info).unwrap(),
            )
            .unwrap();
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,actors_json,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Movie',$2,$3,false,$4,$5,$6,$6)")
                .bind(&id).bind(format!("电影{id}")).bind(json!([{"tmdb_id":1,"character":"演员","order":0}])).bind(json!([id])).bind(json!([{"emby_item_id":id,"path":path,"size_bytes":1000}])).bind(json!([null])).execute(&state.db).await.unwrap();
            files.push(json!(path));
        }
        let location = directory.path().to_string_lossy().into_owned();
        let router = Router::new()
            .route("/files", get(cloud_files))
            .route("/files/file", get(cloud_file))
            .route("/app/chrome/downurl", post(denied_probe_download))
            .route("/open/ufile/downurl", get(denied_probe_download).post(denied_probe_download))
            .route("/Users/user/Items/{id}", get(details))
            .route("/Users/user", get(|| async { Json(json!({"Id":"user","Policy":{"IsAdministrator":true,"MaxParentalRating":null,"BlockUnratedItems":[],"BlockedTags":[],"IsTagBlockingModeInclusive":false,"AllowTagOrRating":false}})) }))
            .route("/Videos/{id}/AdditionalParts", get(|| async { Json(json!({"Items":[],"TotalRecordCount":0})) }))
            .route("/Users/user/Items/{id}/SpecialFeatures", get(|| async { Json(json!([])) }))
            .route("/Items/{id}/ThumbnailSet", get(|State(host): State<Host>, Path(id): Path<String>| async move { Json(json!({"Thumbnails":if host.bifs.lock().await.contains_key(&id) {vec![json!({"PositionTicks":0})]} else {vec![]}})) }))
            .route("/Items/{id}/Images/Chapter/{index}", get(|State(host): State<Host>, Path((id,index)):Path<(String,u64)>| async move { host.chapter_images.lock().await.get(&(id,index)).cloned().unwrap_or_default() }).delete(delete_chapter_image))
            .route("/Videos/{id}/index.bif", get(|State(host):State<Host>, Path(id):Path<String>| async move { host.bifs.lock().await.get(&id).cloned().unwrap_or_default() }))
            .route("/Items", get(items))
            .route("/Items/{id}", post(metadata_only_mock_post))
            .route("/Items/SyncMediaInfo", post(sync_empty_media))
            .route("/Items/{id}/Refresh", post(refresh))
            .route("/Items/{id}/ClearMediaInfo", post(clear))
            .route("/Plugins", get(|| async { Json(json!([{"Id":"63c322b7-a371-41a3-b11f-04f8418b37d8","Name":"Strm Assistant PRO","Version":"3.0.0.57"}])) }))
            .route("/swagger.json", get(|| async { Json(pro_swagger_with_file_refresh()) }))
            .route("/Items/{id}/Ancestors", get(|| async { Json(json!([])) }))
            .route("/Library/VirtualFolders", get(move || { let location = location.clone(); async move { Json(json!([{"ItemId":"library","Guid":"fixture","Locations":[location]}])) } }))
            .route("/ScheduledTasks", get(tasks))
            .route("/ScheduledTasks/Running/{id}", post(start))
            .route("/System/Logs/embyserver.txt/Lines", get(logs))
            .route("/web/ConfigurationPages", get(|| async { Json(json!([{"Name":"MediaInfoExtractPageView","Href":"configurationpage?PageId=fixture:MediaInfoExtractPageView"}])) }))
            .route("/UI/View", get(config_view))
            .route("/UI/Command", post(config_save))
            .route("/emby/Users/user/Views", get(|| async { StatusCode::FORBIDDEN }))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":directory.path(),"monitor_sha1_pc_search":false,"auto_fill_language_titles_on_ingest":enabled,"auto_repair_truehd_core_on_ingest":false,"auto_extract_intro_fingerprint_on_ingest":true,"auto_extract_video_thumbnails_on_ingest":true,"emby_webhook_ingest_enabled":false,"tmdb_api_key":"","ai_api_key":"","telegram_bot_token":"","telegram_channel_id":"","item_delay":0})).await.unwrap();
        if cover_warning {
            db::save_setting(
                &state.db,
                "cover_generator_config",
                &json!({"enabled":true,"transfer_monitor":true}),
            )
            .await
            .unwrap();
        }
        Self {
            state,
            admin,
            schema,
            _directory: directory,
            files,
            host,
            server,
        }
    }
    async fn configure_series_chain(&self) {
        let path = self.files[0].as_str().unwrap();
        let root = std::path::Path::new(path).parent().unwrap();
        let season_root = root.join("Season 01");
        std::fs::create_dir_all(&season_root).unwrap();
        std::fs::write(root.join("tvshow.nfo"),"<tvshow><title>测试电视剧</title><year>2005</year><premiered>2005-01-01</premiered></tvshow>").unwrap();
        let sha = "1".repeat(40);
        let chapters = json!([
            {"Name":"片头","MarkerType":"IntroStart","StartPositionTicks":0},
            {"Name":"片头结束","MarkerType":"IntroEnd","StartPositionTicks":200_000_000_u64}
        ]);
        let mut native = self.host.items.lock().await;
        let episode = native.get_mut("101").unwrap();
        episode["Type"] = json!("Episode");
        episode["Name"] = json!("测试分集");
        episode["ProviderIds"] = json!({"Tmdb":"1011"});
        episode["SeriesId"] = json!("301");
        episode["SeasonId"] = json!("302");
        episode["ParentId"] = json!("302");
        episode["SeriesName"] = json!("测试电视剧");
        episode["ParentIndexNumber"] = json!(1);
        episode["IndexNumber"] = json!(1);
        episode["RunTimeTicks"] = json!(1_200_000_000_u64);
        episode["MediaSources"][0]["RunTimeTicks"] = json!(1_200_000_000_u64);
        episode["Chapters"] = chapters.clone();
        episode["Chapters"].as_array_mut().unwrap().push(
            json!({"Name":"片尾","MarkerType":"CreditsStart","StartPositionTicks":750_000_000_u64}),
        );
        let streams = episode["MediaStreams"].clone();
        let source = episode["MediaSources"][0]["Path"].clone();
        native.insert("301".into(),json!({"Id":"301","Type":"Series","Name":"测试电视剧","Path":root,"ProviderIds":{"Tmdb":"101"},"ProductionYear":2005,"EndDate":null,"Status":"Continuing","People":[],"Genres":[]}));
        native.insert("302".into(),json!({"Id":"302","Type":"Season","Name":"第 1 季","Path":season_root,"ParentId":"301","SeriesId":"301","IndexNumber":1,"ProviderIds":{"Tmdb":"1001"}}));
        drop(native);
        let info = json!([{"MediaSourceInfo":{"Path":source,"Size":1000,"Container":"mkv","RunTimeTicks":1_200_000_000_u64,"MediaStreams":streams},"Chapters":chapters}]);
        let sidecar = std::path::Path::new(path).with_file_name("101-mediainfo.json");
        std::fs::write(&sidecar, serde_json::to_vec(&info).unwrap()).unwrap();
        sqlx::query("DELETE FROM media_metadata WHERE item_type='Movie'")
            .execute(&self.state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,release_date,release_year,actors_json,in_library,emby_item_ids_json,asset_details_json) VALUES('101','Series','测试电视剧','2005-01-01',2005,$1,true,$2,$3)")
            .bind(json!([{"tmdb_id":1,"character":"演员","order":0}])).bind(json!(["301"])).bind(json!([{"path":root}])).execute(&self.state.db).await.unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,parent_series_tmdb_id,season_number,in_library,emby_item_ids_json) VALUES('1001','Season','第1季','101',1,true,$1)")
            .bind(json!(["302"])).execute(&self.state.db).await.unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('1011','Episode','测试分集','101',1,1,true,$1,$2,$3,$4)")
            .bind(json!(["101"])).bind(json!([{"emby_item_id":"101","path":path,"size_bytes":1000}])).bind(json!([sha])).bind(json!(["pc101"])).execute(&self.state.db).await.unwrap();
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
            .bind(&sha)
            .bind(info)
            .execute(&self.state.db)
            .await
            .unwrap();
        self.state.save_config(&json!({"auto_fill_language_titles_on_ingest":false,"auto_extract_intro_fingerprint_on_ingest":true,"auto_extract_credits_on_ingest":true,"auto_extract_video_thumbnails_on_ingest":true,"emby_webhook_ingest_enabled":false})).await.unwrap();
        self.host.chain_enabled.store(true, Ordering::SeqCst);
    }

    async fn configure_original_chapter_images(&self) -> (Vec<u8>, Vec<u8>) {
        let root = self._directory.path().join("metadata");
        let guid = uuid::Uuid::new_v4().simple().to_string();
        let directory = root
            .join("library")
            .join(&guid[..2])
            .join(&guid)
            .join("chapters");
        std::fs::create_dir_all(&directory).unwrap();
        let original_jpeg = jpeg([255, 0, 0]);
        let original_bif = bif(&original_jpeg);
        let mut native = self.host.items.lock().await;
        let episode = native.get_mut("101").unwrap();
        episode["Guid"] = json!(guid);
        let chapters = episode["Chapters"].as_array_mut().unwrap();
        chapters.push(
            json!({"Name":"普通章节","MarkerType":"Chapter","StartPositionTicks":300_000_000_u64}),
        );
        chapters.sort_by_key(|chapter| chapter["StartPositionTicks"].as_u64().unwrap());
        for (index, chapter) in chapters.iter_mut().enumerate() {
            chapter["ImageTag"] = json!("original-same-tag");
            chapter["ChapterIndex"] = json!(index);
            self.host
                .chapter_images
                .lock()
                .await
                .insert(("101".into(), index as u64), original_jpeg.clone());
            std::fs::write(directory.join(format!("{index}.jpg")), &original_jpeg).unwrap();
        }
        drop(native);
        let local_bif =
            std::path::Path::new(self.files[0].as_str().unwrap()).with_file_name("101-320-4.bif");
        std::fs::write(local_bif, &original_bif).unwrap();
        self.host
            .bifs
            .lock()
            .await
            .insert("101".into(), original_bif.clone());
        self.state
            .save_config(&json!({"emby_metadata_path":root}))
            .await
            .unwrap();
        (original_jpeg, original_bif)
    }

    async fn wait_for_job(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    async fn run_series(&self, moviepilot: bool) -> Value {
        if moviepilot {
            self.stage_initial_native_sidecars().await;
        }
        let work = self.state.clone();
        let event = if moviepilot {
            json!({"files":self.files,"tmdb_id":"","batch_key":"ordered-series"})
        } else {
            json!({"id":"301","episode_ids":["101"],"event":"item.add","source":"fixture","original":{"Id":"101"}})
        };
        let task = self
            .state
            .jobs
            .submit("真实入库片头片尾顺序验收", move |context| async move {
                if moviepilot {
                    webhook::process_moviepilot(&work, &event, &context).await
                } else {
                    webhook::process_emby(&work, &event, &context).await
                }
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = self.state.jobs.result(&task).await.unwrap();
                if job["is_running"] == false {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn run(&self, moviepilot: bool) -> Value {
        if moviepilot {
            self.stage_initial_native_sidecars().await;
        }
        let work = self.state.clone();
        let event = if moviepilot {
            json!({"files":self.files,"tmdb_id":"","batch_key":"fixture"})
        } else {
            json!({"id":"101","event":"item.add","source":"fixture","original":{"Id":"101"}})
        };
        let id = self
            .state
            .jobs
            .submit("入库提取门槛验证", move |context| async move {
                if moviepilot {
                    webhook::process_moviepilot(&work, &event, &context).await
                } else {
                    webhook::process_emby(&work, &event, &context).await
                }
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let job = self.state.jobs.result(&id).await.unwrap();
                if job["is_running"] == false {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn run_moviepilot_event(&self, event: Value) -> Value {
        let work = self.state.clone();
        let id = self
            .state
            .jobs
            .submit("MoviePilot 洗版身份验证", move |context| async move {
                webhook::process_moviepilot(&work, &event, &context).await
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = self.state.jobs.result(&id).await.unwrap();
                if job["is_running"] == false {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn stage_initial_native_sidecars(&self) {
        let mut generated = self.host.native_generated_info.lock().await;
        for file in &self.files {
            let path = std::path::Path::new(file.as_str().unwrap());
            let id = path.file_stem().unwrap().to_string_lossy().into_owned();
            if generated.contains_key(&id) {
                continue;
            }
            let sidecar = path.with_file_name(format!("{id}-mediainfo.json"));
            if let Ok(bytes) = std::fs::read(&sidecar) {
                let mut info: Value = serde_json::from_slice(&bytes).unwrap();
                info[0]["Chapters"] = self.host.items.lock().await[&id]["Chapters"].clone();
                generated.insert(id, info);
                std::fs::remove_file(sidecar).unwrap();
            }
        }
    }
    async fn configure_cloud_wash(&mut self) {
        self.host.configuration.lock().await["ExclusiveExtract"] = json!(true);
        self.host.configuration.lock().await["IsModSupported"] = json!(true);
        let base = self.state.config.read().await["emby_server_url"]
            .as_str()
            .unwrap()
            .to_string();
        self.state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            qrcode: base.clone(),
            passport: base.clone(),
            openapi: base.clone(),
            webapi: base.clone(),
            profile: base,
        }));
        db::save_setting(
            &self.state.db,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
        )
        .await
        .unwrap();
        self.state
            .save_config(&json!({
                "monitor_sha1_pc_search":true,"cd2_media_root":"/cd2",
                "p115_fingerprint_media_root_cid":"9","p115_api_priority":"cookie",
                "auto_extract_intro_fingerprint_on_ingest":false,
                "auto_extract_credits_on_ingest":false,
                "auto_extract_video_thumbnails_on_ingest":false
            }))
            .await
            .unwrap();
        let source = "/cd2/101.mkv";
        std::fs::write(self.files[0].as_str().unwrap(), source).unwrap();
        let mut items = self.host.items.lock().await;
        items.get_mut("101").unwrap()["MediaSources"][0]["Path"] = json!(source);
        drop(items);
        let path = self.sidecar_path();
        let mut info: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        info[0]["MediaSourceInfo"]["Path"] = json!(source);
        std::fs::write(path, serde_json::to_vec(&info).unwrap()).unwrap();
    }
    fn sidecar_path(&self) -> std::path::PathBuf {
        std::path::Path::new(self.files[0].as_str().unwrap()).with_file_name("101-mediainfo.json")
    }
    async fn cloud_version(&self, fid: &str, sha: &str, codec: &str) -> Value {
        let raw = json!({
            "format":{"format_name":"matroska,webm","size":"1000","duration":"60"},
            "streams":[
                {"index":0,"codec_type":"video","codec_name":codec,"width":if codec=="hevc" {3840} else {1920},"height":if codec=="hevc" {2160} else {1080},"pix_fmt":"yuv420p"},
                {"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"language":if codec=="hevc" {"eng"} else {"jpn"},"title":if codec=="hevc" {"新版英语"} else {"旧版日语"}},"disposition":{"default":1}}
            ],
            "chapters":[{"start_time":"1.25","tags":{"title":if codec=="hevc" {"新版章节"} else {"旧版章节"}}}]
        });
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,raw_ffprobe_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET raw_ffprobe_json=EXCLUDED.raw_ffprobe_json")
            .bind(sha).bind(raw).execute(&self.state.db).await.unwrap();
        *self.host.cloud_files.lock().await = vec![json!({
            "id":fid,"parent_id":"9","name":"101.mkv","pick_code":fid,
            "sha1":sha,"size":1000,"fc":"1"
        })];
        json!({"path":self.files[0],"storage":"115网盘Plus","cloud_path":"/媒体库/101.mkv",
            "fileid":fid,"pickcode":fid,"sha1":sha,"size":1000})
    }
    async fn native_cloud_media_info(&self) {
        // Model Emby's already completed extraction before an incoming webhook.
        // ETK ingestion consumes this native data; it does not supplement fields.
        let sha = self.host.cloud_files.lock().await[0]["sha1"].clone();
        let raw: Value =
            sqlx::query_scalar("SELECT raw_ffprobe_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(sha.as_str().unwrap())
                .fetch_one(&self.state.db)
                .await
                .unwrap();
        let mut info =
            emby_toolkit::probe::build(&raw, &json!({"fn":"101.mkv"}), &json!([])).unwrap();
        let mut items = self.host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        import_fixture_info(item, &info);
        info[0]["MediaSourceInfo"]["Path"] = item["MediaSources"][0]["Path"].clone();
        std::fs::write(self.sidecar_path(), serde_json::to_vec(&info).unwrap()).unwrap();
    }
    fn cloud_event(&self, identity: Value, batch: &str) -> Value {
        json!({"source":"moviepilot","files":self.files,"tmdb_id":"101",
            "batch_key":batch,"file_identities":[identity]})
    }
    async fn wash_previews(
        &self,
        id: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf, Vec<u8>, Vec<u8>) {
        let guid = uuid::Uuid::new_v4().simple().to_string();
        let jpeg_path = self
            ._directory
            .path()
            .join("metadata/library")
            .join(&guid[..2])
            .join(&guid)
            .join("chapters/0.jpg");
        std::fs::create_dir_all(jpeg_path.parent().unwrap()).unwrap();
        let bytes = jpeg([255, 0, 0]);
        let bif_bytes = bif(&bytes);
        std::fs::write(&jpeg_path, &bytes).unwrap();
        let mut items = self.host.items.lock().await;
        let item = items.get_mut(id).unwrap();
        item["Guid"] = json!(guid);
        if item["Chapters"].as_array().is_none_or(Vec::is_empty) {
            item["Chapters"] = json!([{ "Name":"旧版章节图", "MarkerType":"Chapter", "StartPositionTicks":0, "ChapterIndex":0 }]);
        }
        item["Chapters"][0]["ImageTag"] = json!("old-media-preview");
        item["Chapters"][0]["ChapterIndex"] = json!(0);
        let path = std::path::Path::new(item["Path"].as_str().unwrap());
        let bif_path = path.with_file_name(format!(
            "{}-320-4.bif",
            path.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::write(&bif_path, &bif_bytes).unwrap();
        drop(items);
        self.host
            .chapter_images
            .lock()
            .await
            .insert((id.into(), 0), bytes.clone());
        self.host
            .chapter_image_paths
            .lock()
            .await
            .insert((id.into(), 0), jpeg_path.clone());
        self.host
            .bifs
            .lock()
            .await
            .insert(id.into(), bif_bytes.clone());
        self.host
            .wash_bif_paths
            .lock()
            .await
            .insert(id.into(), bif_path.clone());
        self.state
            .save_config(&json!({"emby_metadata_path":self._directory.path().join("metadata")}))
            .await
            .unwrap();
        (jpeg_path, bif_path, bytes, bif_bytes)
    }
    async fn close(self) {
        assert_eq!(
            self.host.probe_downloads.load(Ordering::SeqCst),
            0,
            "these cloud identity fixtures have current-SHA raw data; no download/probe fallback is expected"
        );
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

struct TelegramCapture {
    messages: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}

async fn known_moviepilot_versions(fixture: &Fixture, old_path: &str, old_online: bool) {
    fixture
        .state
        .save_config(&json!({
            "auto_extract_intro_fingerprint_on_ingest":false,
            "auto_extract_credits_on_ingest":false,
            "auto_extract_video_thumbnails_on_ingest":false
        }))
        .await
        .unwrap();
    let other_path = fixture._directory.path().join("other-library/101.strm");
    let original = fixture.host.items.lock().await["101"].clone();
    for (id, path) in [
        ("900", old_path.to_string()),
        ("901", other_path.to_string_lossy().into_owned()),
    ] {
        if id == "900" && !old_online {
            continue;
        }
        let media_path = std::path::Path::new(&path);
        std::fs::create_dir_all(media_path.parent().unwrap()).unwrap();
        let physical = fixture._directory.path().join(format!("version-{id}.mkv"));
        let size = if id == "900" { 800 } else { 900 };
        std::fs::write(&physical, vec![0_u8; size]).unwrap();
        std::fs::write(media_path, physical.to_string_lossy().as_bytes()).unwrap();
        let mut item = original.clone();
        item["Id"] = json!(id);
        item["Path"] = json!(path);
        item["Size"] = json!(size);
        item["PresentationUniqueKey"] = json!(format!("key-{id}"));
        item["MediaSources"][0]["Id"] = json!(format!("mediasource_{id}"));
        item["MediaSources"][0]["ItemId"] = json!(id);
        item["MediaSources"][0]["Path"] = json!(physical);
        item["MediaSources"][0]["Size"] = json!(size);
        item["MediaStreams"][0]["Codec"] = json!(if id == "900" { "h264" } else { "av1" });
        item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        let info = json!([{"MediaSourceInfo":item["MediaSources"][0],"Chapters":[]}]);
        std::fs::write(
            media_path.with_file_name("101-mediainfo.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .unwrap();
        fixture.host.items.lock().await.insert(id.into(), item);
    }
    sqlx::query("UPDATE media_metadata SET in_library=true,emby_item_ids_json=$1,asset_details_json=$2,file_pickcode_json=$3,file_sha1_json=$4 WHERE tmdb_id='101' AND item_type='Movie'")
        .bind(json!(["900","901"]))
        .bind(json!([
            {"emby_item_id":"900","path":old_path,"size_bytes":800,"video_codec":"h264","fingerprint_pending":false,"fingerprint_revision":7},
            {"emby_item_id":"901","path":other_path,"size_bytes":900,"video_codec":"av1","fingerprint_pending":false,"fingerprint_revision":9}
        ]))
        .bind(json!(["old-pc","other-pc"]))
        .bind(json!(["old-sha","other-sha"]))
        .execute(&fixture.state.db)
        .await
        .unwrap();
}

async fn moviepilot_version_row(fixture: &Fixture) -> Value {
    sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='101' AND item_type='Movie'",
    )
    .fetch_one(&fixture.state.db)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn moviepilot_same_filename_in_another_path_is_new_and_keeps_online_versions() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    let old_path = fixture._directory.path().join("old-library/101.strm");
    known_moviepilot_versions(&fixture, &old_path.to_string_lossy(), true).await;
    fixture.configure_cloud_wash().await;
    let identity = fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    fixture.native_cloud_media_info().await;
    let before = moviepilot_version_row(&fixture).await;
    let mut event = fixture.cloud_event(identity, "same-name-new-path");
    let job = fixture.run_moviepilot_event(event.clone()).await;
    let row = moviepilot_version_row(&fixture).await;
    let info_runs = fixture.host.file_imports.lock().await.len();
    let native_runs = *fixture.host.info_runs.lock().await;
    event["batch_key"] = json!("same-file-another-notification");
    let duplicate = fixture.run_moviepilot_event(event).await;
    let duplicate_info_runs = fixture.host.file_imports.lock().await.len();
    let duplicate_native_runs = *fixture.host.info_runs.lock().await;
    let new_path = fixture.files[0].clone();
    fixture.close().await;

    assert_eq!(
        job["result"]["updated"], 1,
        "the new file must be enhanced and bound: {job}"
    );
    assert_eq!(
        info_runs, 0,
        "same filename in a different path is a new media file: {job}"
    );
    assert_eq!(row["emby_item_ids_json"], json!(["101", "900", "901"]));
    assert_eq!(row["asset_details_json"][0]["path"], new_path);
    assert_eq!(row["asset_details_json"][0]["video_codec"], "hevc");
    for (index, original_index) in [(1, 0), (2, 1)] {
        for field in ["asset_details_json", "file_pickcode_json", "file_sha1_json"] {
            assert_eq!(
                row[field][index], before[field][original_index],
                "online versions must retain their complete slots: {row}"
            );
        }
    }
    assert_eq!(
        duplicate["result"]["updated"], 0,
        "a completed unchanged file must stay idempotent: {duplicate}"
    );
    assert_eq!(
        duplicate_info_runs, info_runs,
        "duplicate notification must not rerun media extraction"
    );
    assert_eq!(
        duplicate_native_runs, native_runs,
        "duplicate notification cannot run a library-wide extraction"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn moviepilot_same_path_with_new_emby_id_reextracts_and_unbinds_only_offline_old_version() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    let path = fixture.files[0].as_str().unwrap().to_string();
    known_moviepilot_versions(&fixture, &path, false).await;
    fixture.configure_cloud_wash().await;
    let identity = fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    fixture.native_cloud_media_info().await;
    let before = moviepilot_version_row(&fixture).await;
    let job = fixture
        .run_moviepilot_event(fixture.cloud_event(identity, "same-path-replaced-id"))
        .await;
    let row = moviepilot_version_row(&fixture).await;
    let info_runs = fixture.host.file_imports.lock().await.len();
    fixture.close().await;

    assert_eq!(
        job["result"]["updated"], 1,
        "the replacement native ID must enter the full ingest flow: {job}"
    );
    assert_eq!(
        info_runs, 0,
        "the replacement must not reuse the old identity solely by path: {job}"
    );
    assert_eq!(row["emby_item_ids_json"], json!(["101", "901"]));
    assert_eq!(row["asset_details_json"][0]["path"], path);
    assert_eq!(row["asset_details_json"][0]["video_codec"], "hevc");
    for field in ["asset_details_json", "file_pickcode_json", "file_sha1_json"] {
        assert_eq!(
            row[field][1], before[field][1],
            "only the confirmed offline old ID can be removed: {row}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_fuse_same_path_binds_current_native_content_and_keeps_other_versions() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    let old_path = fixture._directory.path().join("old-library/101.strm");
    known_moviepilot_versions(&fixture, &old_path.to_string_lossy(), true).await;
    fixture.configure_cloud_wash().await;
    let old_identity = fixture
        .cloud_version("10001", &"A".repeat(40), "h264")
        .await;
    fixture.native_cloud_media_info().await;
    let first = fixture
        .run_moviepilot_event(fixture.cloud_event(old_identity.clone(), "initial-fuse"))
        .await;
    assert_eq!(first["result"]["updated"], 1, "{first}");
    let before = moviepilot_version_row(&fixture).await;
    let strm_before = std::fs::read(fixture.files[0].as_str().unwrap()).unwrap();
    let old_info: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    assert_eq!(
        old_info[0]["MediaSourceInfo"]["MediaStreams"][0]["Codec"],
        "h264"
    );
    let new_identity = fixture
        .cloud_version("10002", &"B".repeat(40), "hevc")
        .await;
    fixture.native_cloud_media_info().await;
    let wash = fixture
        .run_moviepilot_event(fixture.cloud_event(new_identity.clone(), "fuse-wash"))
        .await;
    let row = moviepilot_version_row(&fixture).await;
    let info: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    let native = fixture.host.items.lock().await["101"].clone();
    let imports = fixture.host.file_imports.lock().await.clone();
    let native_runs = *fixture.host.info_runs.lock().await;
    let mut duplicate = fixture.cloud_event(new_identity, "another-uuid-same-fuse-file");
    let duplicate_job = fixture.run_moviepilot_event(duplicate.clone()).await;
    let duplicate_imports = fixture.host.file_imports.lock().await.clone();
    let duplicate_native_runs = *fixture.host.info_runs.lock().await;
    duplicate["file_identities"] = json!([old_identity]);
    duplicate["batch_key"] = json!("delayed-old-fuse-event");
    let stale = fixture.run_moviepilot_event(duplicate).await;
    let after_stale = moviepilot_version_row(&fixture).await;
    let after_stale_info = std::fs::read(fixture.sidecar_path()).unwrap();
    let strm_after = std::fs::read(fixture.files[0].as_str().unwrap()).unwrap();
    fixture.close().await;

    assert_eq!(wash["result"]["updated"], 1, "{wash}");
    assert_eq!(
        strm_after, strm_before,
        "the FUSE STRM bytes deliberately did not change"
    );
    assert_eq!(row["emby_item_ids_json"], before["emby_item_ids_json"]);
    assert_eq!(row["file_sha1_json"][0], "B".repeat(40));
    assert_eq!(row["file_pickcode_json"][0], "10002");
    assert_eq!(
        info[0]["MediaSourceInfo"]["MediaStreams"][0]["Codec"],
        "hevc"
    );
    assert_eq!(info[0]["MediaSourceInfo"]["MediaStreams"][0]["Width"], 3840);
    assert_eq!(
        native["MediaStreams"],
        info[0]["MediaSourceInfo"]["MediaStreams"]
    );
    assert_eq!(
        emby_toolkit::media_info::signature(&native["Chapters"]).unwrap(),
        emby_toolkit::media_info::signature(&info[0]["Chapters"]).unwrap()
    );
    assert_eq!(native["Chapters"][0]["Name"], "新版章节");
    for field in ["asset_details_json", "file_pickcode_json", "file_sha1_json"] {
        for index in [1, 2] {
            assert_eq!(
                row[field][index], before[field][index],
                "other online slots must survive: {field}"
            );
        }
    }
    assert!(
        imports.iter().all(|id| id == "101"),
        "only the replacement may be imported: {imports:?}"
    );
    assert_eq!(duplicate_job["result"]["updated"], 0, "{duplicate_job}");
    assert_eq!(
        duplicate_imports, imports,
        "a new notification UUID cannot repeat extraction/import"
    );
    assert_eq!(
        duplicate_native_runs, native_runs,
        "a duplicate cannot rerun full-library media extraction"
    );
    assert!(
        stale["state"] == "failed" || stale["result"]["updated"] == 0,
        "{stale}"
    );
    for field in ["asset_details_json", "file_pickcode_json", "file_sha1_json"] {
        assert_eq!(
            after_stale[field], row[field],
            "a delayed old manifest cannot bind old content"
        );
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&after_stale_info).unwrap(),
        info
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_legacy_fuse_event_checks_current_cloud_content_without_manifest() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    fixture
        .cloud_version("10001", &"A".repeat(40), "h264")
        .await;
    fixture.native_cloud_media_info().await;
    let mut event = json!({"source":"moviepilot","files":fixture.files,"tmdb_id":"101","batch_key":"legacy-initial"});
    let first = fixture.run_moviepilot_event(event.clone()).await;
    assert_eq!(first["result"]["updated"], 1, "{first}");
    fixture
        .cloud_version("10002", &"B".repeat(40), "hevc")
        .await;
    fixture.native_cloud_media_info().await;
    event["batch_key"] = json!("legacy-replacement");
    let replacement = fixture.run_moviepilot_event(event.clone()).await;
    let row = moviepilot_version_row(&fixture).await;
    let info: Value =
        serde_json::from_slice(&std::fs::read(fixture.sidecar_path()).unwrap()).unwrap();
    let native = fixture.host.items.lock().await["101"].clone();
    let imports = fixture.host.file_imports.lock().await.len();
    let native_runs = *fixture.host.info_runs.lock().await;
    event["batch_key"] = json!("legacy-duplicate-new-uuid");
    let duplicate = fixture.run_moviepilot_event(event).await;
    let final_imports = fixture.host.file_imports.lock().await.len();
    let final_native_runs = *fixture.host.info_runs.lock().await;
    fixture.close().await;
    assert_eq!(replacement["result"]["updated"], 1, "{replacement}");
    assert_eq!(row["file_sha1_json"][0], "B".repeat(40));
    assert_eq!(row["asset_details_json"][0]["video_codec"], "hevc");
    assert_eq!(
        info[0]["MediaSourceInfo"]["MediaStreams"][0]["Codec"],
        "hevc"
    );
    assert_eq!(info[0]["MediaSourceInfo"]["MediaStreams"][0]["Width"], 3840);
    assert_eq!(
        native["MediaStreams"],
        info[0]["MediaSourceInfo"]["MediaStreams"]
    );
    assert_eq!(
        emby_toolkit::media_info::signature(&native["Chapters"]).unwrap(),
        emby_toolkit::media_info::signature(&info[0]["Chapters"]).unwrap()
    );
    assert_eq!(duplicate["result"]["updated"], 0, "{duplicate}");
    assert_eq!(final_imports, imports);
    assert_eq!(final_native_runs, native_runs);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_missing_cloud_identity_keeps_media_data_and_allows_metadata_ingestion() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let identity = fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    let before = moviepilot_version_row(&fixture).await;
    let old_json = std::fs::read(fixture.sidecar_path()).unwrap();
    fixture.host.cloud_unavailable.store(true, Ordering::SeqCst);
    let failed = fixture
        .run_moviepilot_event(fixture.cloud_event(identity.clone(), "cloud-unavailable"))
        .await;
    let failed_row = moviepilot_version_row(&fixture).await;
    let failed_json = std::fs::read(fixture.sidecar_path()).unwrap();
    let failed_imports = fixture.host.file_imports.lock().await.len();
    fixture
        .host
        .cloud_unavailable
        .store(false, Ordering::SeqCst);
    let retried = fixture
        .run_moviepilot_event(fixture.cloud_event(identity, "cloud-available-retry"))
        .await;
    fixture.close().await;
    assert_eq!(failed["state"], "completed_with_warnings", "{failed}");
    assert_ne!(
        failed_row["file_sha1_json"][0],
        "B".repeat(40),
        "an unavailable provider cannot confirm a new file hash"
    );
    assert_eq!(failed_row["file_sha1_json"], before["file_sha1_json"]);
    assert_eq!(failed_json, old_json);
    assert_eq!(failed_imports, 0);
    assert_eq!(retried["result"]["updated"], 1, "{retried}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_cloud_replacement_during_metadata_refresh_never_marks_old_content_completed() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let first_identity = fixture
        .cloud_version("10001", &"A".repeat(40), "h264")
        .await;
    fixture.native_cloud_media_info().await;
    let first = fixture
        .run_moviepilot_event(fixture.cloud_event(first_identity, "before-race"))
        .await;
    assert_eq!(first["result"]["updated"], 1, "{first}");
    let latest_identity = fixture.cloud_version("10003", &"C".repeat(40), "av1").await;
    let latest_file = fixture.host.cloud_files.lock().await[0].clone();
    let stale_identity = fixture
        .cloud_version("10002", &"B".repeat(40), "hevc")
        .await;
    fixture.native_cloud_media_info().await;
    *fixture.host.resource_fault.lock().await = "metadata-cloud-swap".into();
    *fixture.host.cloud_swap_after_import.lock().await = Some(latest_file);
    let raced = fixture
        .run_moviepilot_event(fixture.cloud_event(stale_identity, "replace-during-import"))
        .await;
    let failed_row = moviepilot_version_row(&fixture).await;
    let swap_consumed = fixture.host.cloud_swap_after_import.lock().await.is_none();
    fixture.native_cloud_media_info().await;
    let retry = fixture
        .run_moviepilot_event(fixture.cloud_event(latest_identity.clone(), "latest-after-race"))
        .await;
    let row = moviepilot_version_row(&fixture).await;
    let imports = fixture.host.file_imports.lock().await.len();
    let duplicate = fixture
        .run_moviepilot_event(fixture.cloud_event(latest_identity, "latest-repeat"))
        .await;
    let final_imports = fixture.host.file_imports.lock().await.len();
    fixture.close().await;
    assert_eq!(
        raced["state"], "failed",
        "file changed during the actual metadata refresh: {raced}"
    );
    assert!(
        swap_consumed,
        "the fixture must actually replace the cloud file during the metadata refresh"
    );
    assert_ne!(
        failed_row["asset_details_json"][0]["moviepilot_ingest_identity"]["entity"]["sha1"],
        "B".repeat(40),
        "the raced B revision cannot be stamped complete before retry"
    );
    assert_eq!(
        retry["result"]["updated"], 1,
        "the actual current file must remain retryable: {retry}"
    );
    assert_eq!(row["file_sha1_json"][0], "C".repeat(40));
    assert_eq!(row["asset_details_json"][0]["video_codec"], "av1");
    assert_eq!(duplicate["result"]["updated"], 0, "{duplicate}");
    assert_eq!(final_imports, imports);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_cloud_replacement_before_publish_keeps_formal_json_unchanged() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let initial = fixture
        .cloud_version("10001", &"A".repeat(40), "h264")
        .await;
    let first = fixture
        .run_moviepilot_event(fixture.cloud_event(initial, "before-publish-race"))
        .await;
    assert_eq!(first["result"]["updated"], 1, "{first}");
    let old_json = std::fs::read(fixture.sidecar_path()).unwrap();
    let latest_identity = fixture.cloud_version("10003", &"C".repeat(40), "av1").await;
    let latest_file = fixture.host.cloud_files.lock().await[0].clone();
    let replacement = fixture
        .cloud_version("10002", &"B".repeat(40), "hevc")
        .await;
    let next_verification = fixture.host.cloud_file_reads.load(Ordering::SeqCst) + 2;
    *fixture.host.cloud_swap_after_read.lock().await = Some((next_verification, latest_file));
    let raced = fixture
        .run_moviepilot_event(fixture.cloud_event(replacement, "replace-before-publish"))
        .await;
    let swap_consumed = fixture.host.cloud_swap_after_read.lock().await.is_none();
    let after_race_json = std::fs::read(fixture.sidecar_path()).unwrap();
    let retry = fixture
        .run_moviepilot_event(fixture.cloud_event(latest_identity, "current-after-publish-race"))
        .await;
    fixture.close().await;
    assert_eq!(raced["state"], "failed", "{raced}");
    assert!(
        swap_consumed,
        "the fixture must replace the cloud identity at the fresh pre-publication check"
    );
    assert_eq!(
        after_race_json, old_json,
        "a pre-publication identity change cannot publish stale JSON"
    );
    assert_eq!(retry["result"]["updated"], 1, "{retry}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_same_sha_new_file_id_keeps_previews_without_probe_or_native_import() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let initial = fixture
        .cloud_version("10001", &"A".repeat(40), "h264")
        .await;
    let first = fixture
        .run_moviepilot_event(fixture.cloud_event(initial, "before-same-bytes-copy"))
        .await;
    assert_eq!(first["result"]["updated"], 1, "{first}");
    let (jpeg_path, bif_path, jpeg_bytes, bif_bytes) = fixture.wash_previews("101").await;
    let imports_before = fixture.host.file_imports.lock().await.len();
    let scheduled_before = *fixture.host.info_runs.lock().await;
    let copied = fixture
        .cloud_version("10002", &"A".repeat(40), "h264")
        .await;
    let job = fixture
        .run_moviepilot_event(fixture.cloud_event(copied, "same-sha-new-cloud-id"))
        .await;
    let row = moviepilot_version_row(&fixture).await;
    let imports_after = fixture.host.file_imports.lock().await.len();
    let scheduled_after = *fixture.host.info_runs.lock().await;
    let deleted = fixture.host.deleted_chapter_images.lock().await.clone();
    let preview_bytes = (
        std::fs::read(jpeg_path).unwrap(),
        std::fs::read(bif_path).unwrap(),
    );
    fixture.close().await;
    assert_eq!(
        job["result"]["updated"], 1,
        "changed physical file ID still needs binding update: {job}"
    );
    assert_eq!(row["file_pickcode_json"][0], "10002");
    assert_eq!(row["file_sha1_json"][0], "A".repeat(40));
    assert_eq!(
        imports_after, imports_before,
        "verified same SHA bytes can reuse imported media information"
    );
    assert_eq!(
        scheduled_after,
        scheduled_before + 1,
        "a changed file ID reruns native ingestion while preserving identical bytes"
    );
    assert!(
        deleted.is_empty(),
        "same byte content cannot invalidate its chapter previews"
    );
    assert_eq!(preview_bytes, (jpeg_bytes, bif_bytes));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_repeated_series_notification_keeps_derived_markers_without_reextracting() {
    let fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_series_chain().await;
    fixture
        .state
        .save_config(&json!({"auto_extract_video_thumbnails_on_ingest":false}))
        .await
        .unwrap();
    fixture
        .host
        .generated_intro_end
        .store(220_000_000, Ordering::SeqCst);
    let first = fixture.run_series(true).await;
    assert_eq!(first["state"], "completed", "{first}");
    let before = std::fs::read(fixture.sidecar_path()).unwrap();
    let info: Value = serde_json::from_slice(&before).unwrap();
    assert!(
        info[0]["Chapters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|chapter| chapter["MarkerType"] == "IntroEnd"
                && chapter["StartPositionTicks"] == 220_000_000)
    );
    assert!(
        info[0]["Chapters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|chapter| chapter["MarkerType"] == "CreditsStart")
    );
    let info_runs = *fixture.host.info_runs.lock().await;
    let native_starts = fixture.host.native_starts.lock().await.clone();
    let file_imports = fixture.host.file_imports.lock().await.clone();
    let duplicate = fixture.run_moviepilot_event(json!({
        "source":"moviepilot","files":fixture.files,"tmdb_id":"101","batch_key":"series-repeat-new-uuid"
    })).await;
    let after = std::fs::read(fixture.sidecar_path()).unwrap();
    let final_info_runs = *fixture.host.info_runs.lock().await;
    let final_native_starts = fixture.host.native_starts.lock().await.clone();
    let final_file_imports = fixture.host.file_imports.lock().await.clone();
    fixture.close().await;
    assert_eq!(duplicate["result"]["updated"], 0, "{duplicate}");
    assert_eq!(final_info_runs, info_runs);
    assert_eq!(
        final_native_starts, native_starts,
        "already generated intro/credits cannot restart on another scrape event"
    );
    assert_eq!(final_file_imports, file_imports);
    assert_eq!(
        after, before,
        "the completed current marker JSON must remain exact on duplicate notification"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_native_size_difference_does_not_add_an_ingestion_field_gate() {
    let fixture = Fixture::new(&["complete"], false, false).await;
    fixture
        .state
        .save_config(&json!({
            "auto_extract_intro_fingerprint_on_ingest":false,
            "auto_extract_credits_on_ingest":false,
            "auto_extract_video_thumbnails_on_ingest":false
        }))
        .await
        .unwrap();
    let source = fixture.host.items.lock().await["101"]["MediaSources"][0]["Path"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::write(source, vec![0_u8; 2000]).unwrap();
    let job = fixture.run(true).await;
    let row = moviepilot_version_row(&fixture).await;
    let info_runs = *fixture.host.info_runs.lock().await;
    fixture.close().await;
    assert_eq!(
        info_runs, 1,
        "the stale result must come from this completed native extraction task"
    );
    assert_eq!(
        job["state"], "completed",
        "technical size differences do not block the native ingestion flow: {job}"
    );
    assert!(
        !row["asset_details_json"][0]["moviepilot_ingest_identity"].is_null(),
        "the actual file receives its completed identity without a field gate"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn moviepilot_native_cloud_path_never_writes_a_media_sidecar() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    let directory = fixture._directory.path().join("115");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("101.mkv");
    std::fs::write(&path, vec![0_u8; 1000]).unwrap();
    let before = std::fs::read(&path).unwrap();
    fixture.files = vec![json!(path)];
    {
        let mut items = fixture.host.items.lock().await;
        let item = items.get_mut("101").unwrap();
        item["Path"] = json!(path);
        item["MediaSources"][0]["Path"] = json!(path);
    }
    let job = fixture.run_moviepilot_event(json!({
        "source":"moviepilot","files":fixture.files,"tmdb_id":"101","batch_key":"cloud-native-path"
    })).await;
    let row = moviepilot_version_row(&fixture).await;
    let writes = fixture.host.file_imports.lock().await.clone();
    let native_runs = *fixture.host.info_runs.lock().await;
    let sidecar_exists = directory.join("101-mediainfo.json").exists();
    let after = std::fs::read(&path).unwrap();
    fixture.close().await;
    assert!(matches!(job["state"].as_str(), Some("completed" | "completed_with_warnings")), "{job}");
    assert_eq!(job["result"]["updated"], 1, "{job}");
    assert!(job["result"]["failures"].as_array().unwrap().is_empty());
    assert_eq!(row["asset_details_json"][0]["path"], json!(path));
    assert!(!sidecar_exists);
    assert_eq!(after, before);
    assert!(writes.is_empty());
    assert_eq!(native_runs, 1);
}

impl TelegramCapture {
    async fn attach(fixture: &mut Fixture) -> Self {
        let messages = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = messages.clone();
        let router = Router::new().route(
            "/botfixture/{method}",
            post(move |Path(method): Path<String>, Json(body): Json<Value>| {
                let messages = captured.clone();
                async move {
                    assert!(matches!(method.as_str(), "sendMessage" | "sendPhoto"));
                    messages.lock().await.push(body);
                    Json(json!({"ok":true,"result":{"message_id":1}}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let service = emby_toolkit::notifications::Service::with_api_base(api_base);
        fixture.state.notifications = Arc::new(service);
        fixture
            .state
            .save_config(&json!({
                "telegram_bot_token":"fixture","telegram_channel_id":"fixture-channel",
                "telegram_notify_types":["library_new"]
            }))
            .await
            .unwrap();
        Self { messages, server }
    }
    async fn finish(self) -> Vec<Value> {
        let messages = self.messages.lock().await.clone();
        self.server.abort();
        messages
    }
}

async fn configure_unmerged_movie_versions(fixture: &Fixture, years: [i64; 2]) {
    {
        let mut items = fixture.host.items.lock().await;
        for (index, id) in ["101", "102"].into_iter().enumerate() {
            let item = items.get_mut(id).unwrap();
            item["ProviderIds"]["Tmdb"] = json!("101");
            item["ProductionYear"] = json!(years[index]);
        }
    }
    let assets = fixture.files.iter().enumerate().map(|(index, path)| {
        json!({"emby_item_id":(101+index).to_string(),"path":path,"size_bytes":1000})
    }).collect::<Vec<_>>();
    // One logical metadata cache can reference independent native Movie IDs;
    // this does not merge their Emby MediaSources or physical files.
    sqlx::query("DELETE FROM media_metadata WHERE tmdb_id='102' AND item_type='Movie'")
        .execute(&fixture.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$3 WHERE tmdb_id='101' AND item_type='Movie'")
        .bind(json!(["101","102"])).bind(json!(assets)).bind(json!([null,null]))
        .execute(&fixture.state.db).await.unwrap();
}

fn message_text(message: &Value) -> &str {
    message["text"]
        .as_str()
        .or_else(|| message["caption"].as_str())
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and Telegram are localhost"]
async fn moviepilot_native_technical_field_changes_do_not_block_completed_ingestion_notifications()
{
    for kind in ["Series", "Movie"] {
        let mut fixture = Fixture::new(&["complete"], false, false).await;
        if kind == "Series" {
            fixture.configure_series_chain().await;
        }
        fixture
            .state
            .save_config(&json!({
                "auto_extract_intro_fingerprint_on_ingest":kind == "Series",
                "auto_extract_credits_on_ingest":false,
                "auto_extract_video_thumbnails_on_ingest":kind == "Movie"
            }))
            .await
            .unwrap();
        let telegram = TelegramCapture::attach(&mut fixture).await;
        fixture
            .host
            .corrupt_size_on_native_start
            .store(true, Ordering::SeqCst);
        let job = if kind == "Series" {
            fixture.run_series(true).await
        } else {
            fixture.run(true).await
        };
        let starts = fixture.host.native_starts.lock().await.clone();
        let expected_native = if kind == "Series" { "intro" } else { "thumb" };
        let assets: Value = sqlx::query_scalar("SELECT asset_details_json FROM media_metadata WHERE item_type=$1 AND emby_item_ids_json @> $2::jsonb")
            .bind(if kind == "Series" { "Episode" } else { "Movie" })
            .bind(json!(["101"]))
            .fetch_one(&fixture.state.db).await.unwrap();
        let messages = telegram.finish().await;
        fixture.close().await;
        assert!(
            starts.contains(&expected_native.to_string()),
            "{kind}: the fault must occur after native extraction starts: {job}"
        );
        assert!(
            matches!(
                job["state"].as_str(),
                Some("completed" | "completed_with_warnings")
            ),
            "{kind}: {job}"
        );
        assert!(
            !assets[0]["moviepilot_ingest_identity"].is_null(),
            "{kind}: {assets}"
        );
        assert!(
            messages.len() == 1,
            "{kind}: technical differences do not block the completed ingestion notice: {messages:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn moviepilot_restart_after_business_completion_keeps_task_id_and_does_not_reimport() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_cloud_wash().await;
    let identity = fixture
        .cloud_version("10001", &"A".repeat(40), "hevc")
        .await;
    let event = fixture.cloud_event(identity, "restart-after-completion");
    let payload = json!({"event":event});
    let ready = Arc::new(tokio::sync::Notify::new());
    let signal = ready.clone();
    let work = fixture.state.clone();
    let task = fixture
        .state
        .jobs
        .submit_persistent(
            "入库完成点后的重启",
            emby_toolkit::jobs::JobSpec::new("webhook", payload),
            move |context| async move {
                let report = webhook::process_moviepilot(&work, &event, &context).await?;
                context.complete("test_business_finished", &report).await?;
                signal.notify_one();
                context.cancelled().await;
                context.checkpoint().await?;
                Ok(report)
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(120), ready.notified())
        .await
        .unwrap();
    let imports_before = fixture.host.file_imports.lock().await.clone();
    let native_before = fixture.host.native_starts.lock().await.clone();
    let json_before = std::fs::read(fixture.sidecar_path()).unwrap();
    fixture.state.jobs.shutdown().await;
    fixture.state.jobs = Arc::new(
        emby_toolkit::jobs::JobManager::open(fixture.state.db.clone())
            .await
            .unwrap(),
    );
    fixture
        .state
        .jobs
        .resume_pending(
            fixture.state.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = fixture.state.jobs.result(&task).await.unwrap();
    let imports_after = fixture.host.file_imports.lock().await.clone();
    let native_after = fixture.host.native_starts.lock().await.clone();
    let json_after = std::fs::read(fixture.sidecar_path()).unwrap();
    let registered: Vec<String> = sqlx::query_scalar("SELECT id FROM durable_jobs")
        .fetch_all(&fixture.state.db)
        .await
        .unwrap();
    fixture.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["task_id"], task);
    assert_eq!(registered, vec![task]);
    assert_eq!(result["result"]["updated"], 1, "{result}");
    assert_eq!(imports_after, imports_before);
    assert_eq!(native_after, native_before);
    assert_eq!(json_after, json_before);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn awaiting_moviepilot_history_resumes_only_its_prepared_episode_after_restart() {
    let mut fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_series_chain().await;
    fixture
        .state
        .save_config(&json!({"emby_webhook_ingest_enabled":true,
        "auto_extract_intro_fingerprint_on_ingest":false,"auto_extract_credits_on_ingest":false,
        "auto_extract_video_thumbnails_on_ingest":false}))
        .await
        .unwrap();
    fixture.stage_initial_native_sidecars().await;
    let mut event = json!({"source":"moviepilot","files":fixture.files,"tmdb_id":"",
        "batch_key":"awaiting-native-restart","media_type":"tv","episode_group":""});
    let history = emby_toolkit::media_organize_history::begin_moviepilot(&fixture.state, &event)
        .await
        .unwrap();
    event["history_ids"] = json!(history);
    let work = fixture.state.clone();
    let payload = json!({"event":event});
    let task = fixture
        .state
        .jobs
        .submit_persistent_response(
            "预增强待回流",
            emby_toolkit::jobs::JobSpec::new("webhook", payload.clone()),
            move |context| async move {
                let result = webhook::process_moviepilot(&work, &payload["event"], &context).await;
                emby_toolkit::media_organize_history::finish(&work, &payload["event"], &result)
                    .await?;
                Ok((200, result?))
            },
        )
        .await
        .unwrap();
    fixture.wait_for_job(&task).await;
    let info_runs_before = *fixture.host.info_runs.lock().await;
    let state_before: Vec<String> =
        sqlx::query_scalar("SELECT status FROM media_organize_history WHERE id=ANY($1)")
            .bind(&history)
            .fetch_all(&fixture.state.db)
            .await
            .unwrap();
    fixture.state.jobs.shutdown().await;
    fixture
        .state
        .save_config(&json!({"emby_webhook_ingest_enabled":false}))
        .await
        .unwrap();
    let restarted = AppState::new(fixture.state.db.clone(), fixture.state.data_dir.clone())
        .await
        .unwrap();
    fixture.state = restarted;
    webhook::resume_pending(&fixture.state).await.unwrap();
    tokio::time::timeout(Duration::from_secs(120),async {
        loop {
            let remaining:i64=sqlx::query_scalar("SELECT COUNT(*) FROM media_organize_history WHERE id=ANY($1) AND status='processing'")
                .bind(&history).fetch_one(&fixture.state.db).await.unwrap();
            if remaining==0 { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();
    let current = fixture.state.jobs.status().await;
    if let Some(id) = current["task_id"].as_str() {
        fixture.wait_for_job(id).await;
    }
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM media_organize_history WHERE id=ANY($1)")
            .bind(&history)
            .fetch_all(&fixture.state.db)
            .await
            .unwrap();
    let info_runs_after = *fixture.host.info_runs.lock().await;
    let stages = fixture.host.stage_order.lock().await.clone();
    fixture.close().await;
    assert!(!state_before.is_empty());
    assert!(state_before.iter().all(|status| status == "processing"));
    assert!(
        statuses
            .iter()
            .all(|status| status == "success" || status == "warning"),
        "{statuses:?}"
    );
    assert_eq!(
        info_runs_after, info_runs_before,
        "prepared media must not extract again"
    );
    assert_eq!(
        stages
            .iter()
            .filter(|stage| stage.as_str() == "metadata-refresh")
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and Telegram are localhost"]
async fn moviepilot_notifies_unmerged_same_year_movie_versions_once_with_both_pending_lists() {
    let mut fixture = Fixture::new(&["pending", "pending"], true, false).await;
    configure_unmerged_movie_versions(&fixture, [1998, 1998]).await;
    let telegram = TelegramCapture::attach(&mut fixture).await;
    let job = fixture.run(true).await;
    let native = fixture.host.items.lock().await.clone();
    let messages = telegram.finish().await;
    fixture.close().await;
    let result = &job["result"];
    assert_eq!(result["updated"], 2, "{job}");
    let finalizations = result["finalizations"].as_array().unwrap();
    assert_eq!(finalizations.len(), 2, "{job}");
    let mut pending_ids = finalizations
        .iter()
        .flat_map(|entry| entry["report"]["title_fill"]["pending"].as_array().unwrap())
        .map(|entry| entry["item_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    pending_ids.sort();
    assert_eq!(pending_ids, vec!["101", "102"]);
    assert_eq!(native["101"]["MediaSources"][0]["ItemId"], "101");
    assert_eq!(native["102"]["MediaSources"][0]["ItemId"], "102");
    assert_eq!(
        messages.len(),
        1,
        "one logical movie, one notification: {messages:?}"
    );
    let text = message_text(&messages[0]);
    assert!(text.contains("1998"));
    assert!(
        text.contains("📂 已入库2个文件"),
        "only this batch's two native movie files: {text}"
    );
    assert!(
        text.contains("电影101"),
        "first version pending must be retained: {text}"
    );
    assert!(
        text.contains("电影102"),
        "second version pending must be retained: {text}"
    );
    assert_eq!(
        text.matches("字幕10").count(),
        2,
        "both pending tracks must appear: {text}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and Telegram are localhost"]
async fn moviepilot_keeps_different_year_movie_notices_separate_despite_same_tmdb() {
    let mut fixture = Fixture::new(&["pending", "pending"], true, false).await;
    configure_unmerged_movie_versions(&fixture, [1998, 1999]).await;
    let telegram = TelegramCapture::attach(&mut fixture).await;
    let job = fixture.run(true).await;
    let messages = telegram.finish().await;
    fixture.close().await;
    assert_eq!(job["result"]["updated"], 2, "{job}");
    assert_eq!(
        messages.len(),
        2,
        "different years must not merge: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message_text(message).contains("1998"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message_text(message).contains("1999"))
    );
    for message in &messages {
        assert_eq!(message_text(message).matches("字幕10").count(), 1);
        assert!(message_text(message).contains("📂 已入库1个文件"));
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn both_ingest_entries_allow_disabled_unfinished_and_complete_titles_after_metadata_refresh()
{
    for moviepilot in [false, true] {
        for (mode, enabled) in [
            ("complete", false),
            ("pending", true),
            ("failed", true),
            ("complete", true),
        ] {
            let f = Fixture::new(&[mode], enabled, false).await;
            let job = f.run(moviepilot).await;
            let result = &job["result"];
            let title = if moviepilot {
                &result["finalizations"][0]["report"]["title_fill"]
            } else {
                &result["finalization"]["title_fill"]
            };
            let refresh = if moviepilot {
                &result["finalizations"][0]["report"]["metadata_refresh"]
            } else {
                &result["finalization"]["metadata_refresh"]
            };
            let starts = f.host.native_starts.lock().await.clone();
            assert_eq!(
                result["updated"], 1,
                "moviepilot={moviepilot} mode={mode} enabled={enabled}: {job}"
            );
            assert_eq!(
                refresh["completed"], true,
                "a real final refresh must be verified: {job}"
            );
            assert!(
                f.host
                    .refresh_attempts
                    .lock()
                    .await
                    .iter()
                    .any(|(_, mode)| mode == "FullRefresh"),
                "the fixture must receive the actual full refresh request"
            );
            if mode == "complete" {
                if enabled {
                    assert_eq!(title["completed"], true, "{job}");
                    assert_eq!(title["skipped_chinese_streams"], 9, "{job}");
                } else {
                    assert_eq!(title["completed"], false, "{job}");
                    assert_eq!(title["skipped"], true, "{job}");
                }
                assert_eq!(starts, vec!["thumb"], "{job}");
            } else {
                assert_eq!(title["completed"], false, "{job}");
                assert_eq!(
                    starts,
                    vec!["thumb"],
                    "pending titles do not gate native ingestion: {job}"
                );
            }
            f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn both_ingest_entries_block_native_extraction_when_the_actual_final_refresh_request_fails() {
    for moviepilot in [false, true] {
        for titles_enabled in [false, true] {
            let fixture = Fixture::new(&["refresh_failed"], titles_enabled, false).await;
            *fixture.host.resource_fault.lock().await = "metadata-post-fail".into();
            let job = fixture.run(moviepilot).await;
            assert!(
                fixture
                    .host
                    .refresh_attempts
                    .lock()
                    .await
                    .iter()
                    .any(|(id, mode)| id == "101" && mode == "FullRefresh"),
                "the actual final refresh rejection must be reached: {job}"
            );
            assert!(
                fixture.host.native_starts.lock().await.is_empty(),
                "a failed final refresh must not start intro, credits, or thumbnails: {job}"
            );
            if moviepilot {
                assert_eq!(job["result"]["updated"], 1, "{job}");
                assert!(
                    !job["result"]["failures"].as_array().unwrap().is_empty(),
                    "{job}"
                );
                assert!(job["result"]["automatic_extraction"].is_null(), "{job}");
            } else {
                assert_eq!(job["state"], "failed", "{job}");
            }
            fixture.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and all media requests stay localhost"]
async fn disabled_titles_still_allow_actual_series_refresh_then_intro_credits_and_thumbnails_in_both_ingest_entries()
 {
    for moviepilot in [false, true] {
        let fixture = Fixture::new(&["complete"], false, false).await;
        fixture.configure_series_chain().await;
        let original_cache: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind("1".repeat(40))
                .fetch_one(&fixture.state.db)
                .await
                .unwrap();
        assert!(
            !original_cache[0]["Chapters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|chapter| chapter["MarkerType"] == "CreditsStart")
        );
        let job = fixture.run_series(moviepilot).await;
        let result = &job["result"];
        let finalization = if moviepilot {
            &result["finalizations"][0]["report"]
        } else {
            &result["finalization"]
        };
        assert_eq!(
            finalization["metadata_refresh"]["completed"], true,
            "moviepilot={moviepilot}: {job}"
        );
        assert_eq!(finalization["title_fill"]["completed"], false, "{job}");
        assert_eq!(finalization["title_fill"]["skipped"], true, "{job}");
        assert!(
            fixture.host.year_readbacks.load(Ordering::SeqCst) >= 3,
            "actual Series NFO year readback must complete before extraction"
        );
        assert_eq!(
            *fixture.host.stage_order.lock().await,
            vec![
                "metadata-refresh",
                "nfo-refresh",
                "intro",
                "credits-read",
                "thumb"
            ],
            "moviepilot={moviepilot}: {job}"
        );
        assert_eq!(
            *fixture.host.native_starts.lock().await,
            vec!["intro", "thumb"],
            "{job}"
        );
        assert_eq!(
            result["automatic_extraction"]["intro"]["failed"], 0,
            "intro must pass native task and current execution log verification: {job}"
        );
        assert_eq!(
            result["automatic_extraction"]["credits"]["failed"], 0,
            "credits must pass actual Emby marker readback: {job}"
        );
        assert_eq!(
            result["automatic_extraction"]["credits"]["skipped"], 1,
            "an existing valid marker should be synced without re-extracting audio: {job}"
        );
        let cached: Value =
            sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind("1".repeat(40))
                .fetch_one(&fixture.state.db)
                .await
                .unwrap();
        assert!(
            cached[0]["Chapters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|chapter| chapter["MarkerType"] == "CreditsStart"
                    && chapter["StartPositionTicks"] == 750_000_000_u64),
            "the real credits stage must update its cache"
        );
        // This fixture deliberately refuses the thumbnail start after recording
        // the native entry. New audio extraction and writeback are tested with
        // real ffmpeg in credits_extraction.rs.
        assert!(
            result["automatic_extraction"]["warnings"].as_u64().unwrap() > 0,
            "the refused thumbnail start must remain visible: {job}"
        );
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn both_ingest_entries_reject_chapter_loss_after_final_refresh_before_automatic_extraction() {
    for moviepilot in [false, true] {
        let fixture = Fixture::new(&["complete"], false, false).await;
        fixture.configure_series_chain().await;
        fixture
            .state
            .save_config(&json!({
                "auto_extract_credits_on_ingest":false,
                "auto_extract_video_thumbnails_on_ingest":false
            }))
            .await
            .unwrap();
        let original = fixture.host.items.lock().await["101"]["Chapters"].clone();
        let original_settings = fixture.host.configuration.lock().await.clone();
        assert!(original.as_array().unwrap().len() >= 3);
        fixture
            .host
            .clear_chapters_on_final_refresh
            .store(true, Ordering::SeqCst);
        let job = fixture.run_series(moviepilot).await;
        let native_starts = fixture.host.native_starts.lock().await.clone();
        let recovery = media_recovery::status(State(fixture.state.clone()))
            .await
            .unwrap()
            .0;
        assert_eq!(
            *fixture.host.configuration.lock().await, original_settings,
            "a finished refresh must restore settings even when chapter verification fails"
        );
        fixture.close().await;
        let finalization = if moviepilot {
            &job["result"]["finalizations"][0]["report"]
        } else {
            &job["result"]["finalization"]
        };
        assert_ne!(
            finalization["metadata_refresh"]["completed"], true,
            "a completion log cannot certify preserved chapters: moviepilot={moviepilot}: {job}"
        );
        assert!(
            native_starts.is_empty(),
            "chapter loss must block subsequent native extraction: moviepilot={moviepilot}: {native_starts:?}"
        );
        assert_eq!(recovery["pending"], true, "{recovery}");
        assert_eq!(
            recovery["records"][0]["target_ids"],
            json!(["101"]),
            "{recovery}"
        );
        assert_eq!(
            recovery["records"][0]["can_recover"], true,
            "usable recovery materials must remain: {recovery}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn moviepilot_title_and_cover_warnings_do_not_block_the_completed_native_flow() {
    for modes in [["complete", "pending"], ["pending", "complete"]] {
        let f = Fixture::new(&modes, true, false).await;
        let job = f.run(true).await;
        let result = &job["result"];
        assert_eq!(result["updated"], 2, "{job}");
        assert_eq!(
            result["finalizations"].as_array().unwrap().len(),
            2,
            "{job}"
        );
        assert_eq!(
            *f.host.native_starts.lock().await,
            vec!["thumb"],
            "title warnings do not block native ingestion: {job}"
        );
        f.close().await;
    }
    let f = Fixture::new(&["complete"], true, true).await;
    let job = f.run(false).await;
    assert_eq!(
        job["result"]["finalization"]["title_fill"]["completed"], true,
        "{job}"
    );
    assert!(
        job["result"]["finalization"]["warnings"].as_u64().unwrap() > 0,
        "{job}"
    );
    assert_eq!(
        *f.host.native_starts.lock().await,
        vec!["thumb"],
        "封面警告不得阻止已完成标题的提取：{job}"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn moviepilot_webhook_retries_failed_finalization_and_deduplicates_completed_media() {
    async fn post_and_wait(fixture: &Fixture, body: &Value) -> Value {
        fixture.stage_initial_native_sidecars().await;
        let previous_task = fixture.state.jobs.status().await["task_id"].clone();
        let response = webhook::router()
            .with_state(fixture.state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/moviepilot?token=fixture-shared")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let status = fixture.state.jobs.status().await;
                if status["task_id"] != previous_task
                    && let Some(id) = status["task_id"].as_str()
                    && let Some(job) = fixture.state.jobs.result(id).await
                    && job["is_running"] == false
                {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("accepted webhook must finish a new task")
    }

    let fixture = Fixture::new(&["refresh_failed"], false, false).await;
    fixture
        .state
        .save_config(&json!({
            "emby_webhook_secret":"fixture-shared",
            "monitor_paths":[fixture._directory.path()],
            "monitor_extensions":["strm"],
            "auto_extract_intro_fingerprint_on_ingest":false,
            "auto_extract_credits_on_ingest":false,
            "auto_extract_video_thumbnails_on_ingest":false
        }))
        .await
        .unwrap();
    let mut body = json!({
        "event":"metadata.scrape.complete",
        "success":true,
        "root_path":fixture._directory.path(),
        "file_list":fixture.files,
        "media_type":"movie",
        "tmdb_id":"101",
        "batch_id":"retry-finalization"
    });
    let mut jobs = vec![];
    let mut histories: Vec<String> = vec![];
    let mut refresh_counts = vec![];
    let mut info_counts = vec![];
    let mut in_library: Vec<bool> = vec![];
    for attempt in 0..3 {
        if attempt == 1 {
            fixture.host.refresh_failures.lock().await.clear();
        } else if attempt == 2 {
            body["batch_id"] = json!("completed-known-media");
        }
        jobs.push(post_and_wait(&fixture, &body).await);
        histories.push(
            sqlx::query_scalar(
                "SELECT status FROM media_organize_history ORDER BY created_at DESC,id DESC LIMIT 1",
            )
            .fetch_one(&fixture.state.db)
            .await
            .unwrap(),
        );
        in_library.push(
            sqlx::query_scalar(
                "SELECT in_library FROM media_metadata WHERE tmdb_id='101' AND item_type='Movie'",
            )
            .fetch_one(&fixture.state.db)
            .await
            .unwrap(),
        );
        refresh_counts.push(
            fixture
                .host
                .refresh_attempts
                .lock()
                .await
                .iter()
                .filter(|(id, mode)| id == "101" && mode == "FullRefresh")
                .count(),
        );
        info_counts.push(*fixture.host.info_runs.lock().await);
    }
    fixture.close().await;

    assert_eq!(jobs[0]["state"], "failed", "{}", jobs[0]);
    assert!(
        in_library[0],
        "failed finalization must follow asset binding"
    );
    assert!(
        refresh_counts[0] > 0,
        "first attempt must reach final refresh"
    );
    assert_eq!(histories[0], "failed");
    assert_eq!(
        info_counts,
        vec![1, 2, 2],
        "failed work retries the native flow; completed media remains idempotent"
    );
    assert_eq!(
        jobs[1]["result"]["finalizations"][0]["report"]["metadata_refresh"]["completed"], true,
        "same-batch retry must complete the failed finalization: {}",
        jobs[1]
    );
    assert!(refresh_counts[1] > refresh_counts[0]);
    assert_eq!(histories[1], "success");
    assert_eq!(histories[2], "success");
    assert_eq!(jobs[2]["result"]["updated"], 0, "{}", jobs[2]);
    assert_eq!(
        refresh_counts[2], refresh_counts[1],
        "completed media must not be finalized twice"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn moviepilot_history_uses_the_completed_work_when_ingest_switch_changes_before_finish() {
    for awaiting_emby in [false, true] {
        let fixture = Fixture::new(&["complete"], false, false).await;
        fixture.stage_initial_native_sidecars().await;
        fixture
            .state
            .save_config(&json!({
                "emby_webhook_ingest_enabled":awaiting_emby,
                "auto_extract_intro_fingerprint_on_ingest":false,
                "auto_extract_credits_on_ingest":false,
                "auto_extract_video_thumbnails_on_ingest":false
            }))
            .await
            .unwrap();
        let mut event = json!({"source":"moviepilot","media_type":"movie",
            "files":fixture.files,"tmdb_id":"101","batch_key":"ownership"});
        event["history_ids"] = json!(
            emby_toolkit::media_organize_history::begin_moviepilot(&fixture.state, &event)
                .await
                .unwrap()
        );
        let work = fixture.state.clone();
        let id = fixture
            .state
            .jobs
            .submit("入库收尾归属", move |context| async move {
                let result = webhook::process_moviepilot(&work, &event, &context).await;
                work.save_config(&json!({"emby_webhook_ingest_enabled":!awaiting_emby}))
                    .await?;
                emby_toolkit::media_organize_history::finish(&work, &event, &result).await?;
                result
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = fixture.state.jobs.result(&id).await.unwrap();
                if job["is_running"] == false {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM media_organize_history")
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
        fixture.close().await;
        assert_eq!(job["result"]["updated"], 1, "{job}");
        if awaiting_emby {
            assert!(
                job["result"]["finalizations"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                status, "processing",
                "preparation still awaits actual finalization"
            );
        } else {
            assert_eq!(
                job["result"]["finalizations"][0]["report"]["metadata_refresh"]["completed"],
                true
            );
            assert_eq!(
                status, "success",
                "completed finalization must remain complete"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all resource requests stay localhost"]
async fn both_ingest_entries_preserve_jpeg_bif_bytes_and_reject_same_tag_or_same_index_changes() {
    for moviepilot in [false, true] {
        for fault in ["", "jpeg", "bif"] {
            let fixture = Fixture::new(&["complete"], false, false).await;
            fixture.configure_series_chain().await;
            fixture.state.save_config(&json!({"auto_extract_intro_fingerprint_on_ingest":false,"auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false})).await.unwrap();
            let (original_jpeg, original_bif) = fixture.configure_original_chapter_images().await;
            let original_chapters = fixture.host.items.lock().await["101"]["Chapters"].clone();
            let original_settings = fixture.host.configuration.lock().await.clone();
            *fixture.host.resource_fault.lock().await = fault.into();
            let job = fixture.run_series(moviepilot).await;
            let finalization = if moviepilot {
                &job["result"]["finalizations"][0]["report"]
            } else {
                &job["result"]["finalization"]
            };
            assert_eq!(
                fixture.host.items.lock().await["101"]["Chapters"],
                original_chapters,
                "identical ImageTags must not conceal changed bytes"
            );
            let recovery = media_recovery::status(State(fixture.state.clone()))
                .await
                .unwrap()
                .0;
            if fault.is_empty() {
                assert_eq!(
                    finalization["metadata_refresh"]["completed"], true,
                    "moviepilot={moviepilot}: {job}"
                );
                assert_eq!(
                    recovery["pending"], false,
                    "verified materials must be cleaned: {recovery}"
                );
                assert_eq!(*fixture.host.configuration.lock().await, original_settings);
                assert!(
                    fixture
                        .host
                        .chapter_images
                        .lock()
                        .await
                        .values()
                        .all(|bytes| bytes == &original_jpeg)
                );
                assert_eq!(fixture.host.bifs.lock().await["101"], original_bif);
            } else {
                assert_ne!(
                    finalization["metadata_refresh"]["completed"], true,
                    "fault={fault}: moviepilot={moviepilot}: {job}"
                );
                assert_eq!(recovery["pending"], true, "{recovery}");
                assert_eq!(
                    *fixture.host.configuration.lock().await, original_settings,
                    "a completed native refresh must not retain Restore for a resource mismatch"
                );
                assert_eq!(recovery["records"][0]["can_recover"], true, "{recovery}");
                let directory =
                    std::path::Path::new(recovery["records"][0]["directory"].as_str().unwrap());
                let record: Value = serde_json::from_slice(
                    &std::fs::read(directory.join("media-reload.json")).unwrap(),
                )
                .unwrap();
                let images = record["chapter_images"]["101"].as_array().unwrap();
                assert_eq!(images.len(), 4);
                for image in images {
                    assert_eq!(
                        std::fs::read(image["backup"].as_str().unwrap()).unwrap(),
                        original_jpeg
                    );
                }
                assert!(
                    record["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|file| file["path"].as_str().unwrap().ends_with(".bif")
                            && std::fs::read(file["backup"].as_str().unwrap()).unwrap()
                                == original_bif)
                );
            }
            assert!(fixture.host.native_starts.lock().await.is_empty());
            fixture.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn recursive_ingest_refresh_protects_old_episode_outside_the_selected_ingest_ids() {
    let fixture = Fixture::new(&["complete", "complete"], false, false).await;
    fixture.configure_series_chain().await;
    fixture.state.save_config(&json!({"auto_extract_intro_fingerprint_on_ingest":false,"auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false})).await.unwrap();
    let old_chapters = json!([{"Name":"旧集普通章","MarkerType":"Chapter","StartPositionTicks":0},{"Name":"旧集片尾","MarkerType":"CreditsStart","StartPositionTicks":750_000_000_u64}]);
    let old_path = fixture.files[1].as_str().unwrap();
    {
        let mut items = fixture.host.items.lock().await;
        let old = items.get_mut("102").unwrap();
        old["Type"] = json!("Episode");
        old["SeriesId"] = json!("301");
        old["ParentId"] = json!("302");
        old["ParentIndexNumber"] = json!(1);
        old["IndexNumber"] = json!(2);
        old["Chapters"] = old_chapters.clone();
    }
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json) VALUES('1021','Episode','旧集','101',1,2,true,$1,$2)")
        .bind(json!(["102"])).bind(json!([{"emby_item_id":"102","path":old_path,"size_bytes":1000}])).execute(&fixture.state.db).await.unwrap();
    *fixture.host.other_chapter_loss_target.lock().await = Some("102".into());
    let old_native = fixture.host.items.lock().await["102"].clone();
    let old_files = [
        std::path::PathBuf::from(old_path),
        std::path::Path::new(old_path).with_extension("nfo"),
        std::path::Path::new(old_path).with_file_name("102-mediainfo.json"),
        std::path::Path::new(old_path).with_file_name("102-320-10.bif"),
        std::path::Path::new(old_path).with_file_name("102-thumb.jpg"),
    ];
    // Existing media JSON/STRM and unrelated sidecars must all remain exact.
    for (path, contents) in old_files.iter().skip(1).zip([
        b"<episodedetails><title>original old-episode</title></episodedetails>".to_vec(),
        b"original old-episode JSON".to_vec(),
        bif(&jpeg([255, 0, 0])),
        jpeg([255, 0, 0]),
    ]) {
        if !path.exists() {
            std::fs::write(path, contents).unwrap();
        }
    }
    let original_files = old_files
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let job = fixture.run_series(false).await;
    let recovery = media_recovery::status(State(fixture.state.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(
        job["state"], "completed",
        "only 101 and its parent metadata may be refreshed: {job}"
    );
    assert_eq!(fixture.host.items.lock().await["102"], old_native);
    for (path, original) in old_files.iter().zip(original_files) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            original,
            "unselected sidecar {}",
            path.display()
        );
    }
    let refreshes = fixture.host.refresh_queries.lock().await;
    assert!(!refreshes.iter().any(|(id, _)| id == "102"));
    let full = refreshes
        .iter()
        .filter(|(_, query)| {
            query
                .get("MetadataRefreshMode")
                .is_some_and(|mode| mode == "FullRefresh")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        full.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["101", "302", "301"]
    );
    assert!(
        full.iter()
            .all(|(_, query)| query.get("Recursive").is_some_and(|value| value == "false"))
    );
    drop(refreshes);
    assert_eq!(recovery["pending"], false, "{recovery}");
    assert!(recovery["records"].as_array().unwrap().is_empty());
    assert!(fixture.host.native_starts.lock().await.is_empty());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn disabled_title_ingest_refuses_empty_live_chapters_with_nonempty_formal_backup_before_refresh()
 {
    for custom_root in [false, true] {
        let fixture = Fixture::new(&["complete"], false, false).await;
        fixture.configure_series_chain().await;
        fixture.state.save_config(&json!({"auto_extract_intro_fingerprint_on_ingest":false,"auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false})).await.unwrap();
        fixture.host.items.lock().await.get_mut("101").unwrap()["Chapters"] = json!([]);
        let mut sidecar = std::path::Path::new(fixture.files[0].as_str().unwrap())
            .with_file_name("101-mediainfo.json");
        let original_json = std::fs::read(&sidecar).unwrap();
        let parsed: Value = serde_json::from_slice(&original_json).unwrap();
        assert_eq!(parsed[0]["Chapters"].as_array().unwrap().len(), 2);
        if custom_root {
            let root = fixture._directory.path().join("custom-json-root");
            let native_parent = std::path::Path::new(fixture.files[0].as_str().unwrap())
                .parent()
                .unwrap();
            // PRO makes the native absolute containing directory relative to its
            // filesystem root before joining MediaInfoJsonRootFolder.
            let formal = root
                .join(native_parent.strip_prefix("/").unwrap())
                .join("101-mediainfo.json");
            std::fs::create_dir_all(formal.parent().unwrap()).unwrap();
            std::fs::rename(&sidecar, &formal).unwrap();
            assert!(
                !sidecar.exists(),
                "default candidates must contain no chapter backup"
            );
            fixture.host.configuration.lock().await["MediaInfoJsonRootFolder"] = json!(root);
            sidecar = formal;
        }
        let job = fixture.run_series(false).await;
        let refreshes = fixture.host.refresh_attempts.lock().await.clone();
        let starts = fixture.host.native_starts.lock().await.clone();
        assert_eq!(
            job["state"], "failed",
            "custom_root={custom_root}: empty native chapters cannot certify that the backup may be discarded: {job}"
        );
        assert!(
            refreshes.is_empty(),
            "must refuse before a recursive refresh can overwrite the formal backup: {refreshes:?}"
        );
        assert!(starts.is_empty());
        assert_eq!(std::fs::read(sidecar).unwrap(), original_json);
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; real recovery POST and Emby remain localhost"]
async fn ingest_recovery_post_restores_original_chapter_resources_and_keeps_final_metadata_and_new_cover()
 {
    let fixture = Fixture::new(&["complete"], false, false).await;
    fixture.configure_series_chain().await;
    fixture.state.save_config(&json!({"auto_extract_intro_fingerprint_on_ingest":false,"auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false})).await.unwrap();
    let (original_jpeg, original_bif) = fixture.configure_original_chapter_images().await;
    let chapters = fixture.host.items.lock().await["101"]["Chapters"].clone();
    let guid = fixture.host.items.lock().await["101"]["Guid"]
        .as_str()
        .unwrap()
        .to_string();
    let cover = fixture
        ._directory
        .path()
        .join("metadata/library")
        .join(&guid[..2])
        .join(guid)
        .join("poster.jpg");
    std::fs::write(&cover, &original_jpeg).unwrap();
    *fixture.host.cover.lock().await = Some(cover.clone());
    fixture.host.configuration.lock().await["ExclusiveExtract"] = json!(true);
    fixture.host.configuration.lock().await["IsModSupported"] = json!(true);
    let original_settings = fixture.host.configuration.lock().await.clone();
    *fixture.host.resource_fault.lock().await = "new-cover-and-chapter-loss".into();
    let failed = fixture.run_series(false).await;
    assert_eq!(
        failed["state"], "failed",
        "the native final refresh must really lose chapters: {failed}"
    );
    assert_eq!(
        fixture.host.items.lock().await["101"]["Name"],
        "新的分集标题"
    );
    assert_eq!(std::fs::read(&cover).unwrap(), jpeg([0, 0, 255]));
    *fixture.host.resource_fault.lock().await = String::new();
    let pending = media_recovery::status(State(fixture.state.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(pending["records"][0]["can_recover"], true, "{pending}");
    let recovery_id = pending["records"][0]["id"].as_str().unwrap();
    let response = emby_toolkit::media_edit::router()
        .with_state(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/media_info/recovery")
                .header("Content-Type", "application/json")
                .body(Body::from(json!({"recovery_id":recovery_id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let receipt: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap(),
    )
    .unwrap();
    let task = receipt["task_id"].as_str().unwrap();
    let recovered = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let job = fixture.state.jobs.result(task).await.unwrap();
            if job["is_running"] == false {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        recovered["state"], "completed",
        "actual recovery must keep a legitimate new cover: {recovered}"
    );
    assert_eq!(recovered["result"]["pending"], false, "{recovered}");
    let native = fixture.host.items.lock().await["101"].clone();
    assert_eq!(native["Chapters"], chapters);
    assert_eq!(native["Name"], "新的分集标题");
    assert_eq!(native["Overview"], "最终刷新后的新简介");
    assert_eq!(std::fs::read(cover).unwrap(), jpeg([0, 0, 255]));
    assert_eq!(fixture.host.bifs.lock().await["101"], original_bif);
    assert!(
        fixture
            .host
            .chapter_images
            .lock()
            .await
            .values()
            .all(|bytes| bytes == &original_jpeg)
    );
    assert_eq!(*fixture.host.configuration.lock().await, original_settings);
    assert_eq!(
        media_recovery::status(State(fixture.state.clone()))
            .await
            .unwrap()
            .0["pending"],
        false
    );
    fixture.close().await;
}

include!("fixtures/gaia_empty_ingest.rs");
include!("fixtures/reorganize_ingest_flow.rs");
include!("fixtures/legacy_video_range.rs");
