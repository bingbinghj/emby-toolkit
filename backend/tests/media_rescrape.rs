//! The metadata-only path must not rebuild or discard existing media resources.
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::Session};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone, Default)]
struct Host {
    items: Arc<Mutex<HashMap<String, Value>>>,
    configuration: Arc<Mutex<Value>>,
    logs: Arc<Mutex<Vec<String>>>,
    writes: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
    tmdb_reads: Arc<Mutex<Vec<String>>>,
    chapter_jpeg: Arc<Mutex<Vec<u8>>>,
    bif: Arc<Mutex<Vec<u8>>>,
    fault: Arc<Mutex<String>>,
    chapter_path: Arc<Mutex<PathBuf>>,
    bif_path: Arc<Mutex<PathBuf>>,
}

async fn details(State(host): State<Host>, Path(id): Path<String>) -> Response {
    host.items.lock().await.get(&id).cloned().map_or_else(
        || StatusCode::NOT_FOUND.into_response(),
        |item| Json(item).into_response(),
    )
}

async fn items(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let items = host
        .items
        .lock()
        .await
        .values()
        .filter(|item| {
            query.get("Ids").is_none_or(|ids| {
                ids.split(',')
                    .any(|id| item["Id"] == id || item["Guid"] == id)
            }) && query
                .get("ParentId")
                .is_none_or(|id| item["ParentId"] == *id || item["SeriesId"] == *id)
                && query
                    .get("IncludeItemTypes")
                    .is_none_or(|types| types.split(',').any(|kind| item["Type"] == kind))
        })
        .cloned()
        .collect::<Vec<_>>();
    Json(json!({"TotalRecordCount":items.len(),"Items":items}))
}

fn nfo_element(xml: &str, tag: &str) -> String {
    regex::Regex::new(&format!(r"(?s)<{tag}[^>]*>(.*?)</{tag}>"))
        .unwrap()
        .captures(xml)
        .map(|value| value[1].to_string())
        .unwrap_or_default()
}

async fn refresh(
    State(host): State<Host>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> StatusCode {
    host.writes
        .lock()
        .await
        .push((format!("refresh:{id}"), query));
    let mut items = host.items.lock().await;
    let item = items.get_mut(&id).unwrap();
    let path = PathBuf::from(item["Path"].as_str().unwrap());
    let xml = std::fs::read_to_string(path.with_extension("nfo")).unwrap();
    item["Name"] = json!(nfo_element(&xml, "title"));
    item["Overview"] = json!(nfo_element(&xml, "plot"));
    item["DateLastRefreshed"] = json!("after-fixture-refresh");
    match host.fault.lock().await.as_str() {
        "streams" => {
            item["MediaStreams"][1]["Title"] = json!("unexpected replacement");
            item["MediaSources"][0]["MediaStreams"] = item["MediaStreams"].clone();
        }
        "chapters" => item["Chapters"] = json!([]),
        "json" => {
            let json_path = path.with_file_name(format!(
                "{}-mediainfo.json",
                path.file_stem().unwrap().to_string_lossy()
            ));
            std::fs::write(
                json_path,
                br#"[{"MediaSourceInfo":{"Container":"mkv","MediaStreams":[]},"Chapters":[]}]"#,
            )
            .unwrap();
        }
        "chapter" => {
            let changed = jpeg([0, 0, 255]);
            *host.chapter_jpeg.lock().await = changed.clone();
            std::fs::write(&*host.chapter_path.lock().await, changed).unwrap();
        }
        "chapter-local" => {
            std::fs::write(&*host.chapter_path.lock().await, jpeg([0, 0, 255])).unwrap();
        }
        "bif" => {
            let changed = bif(&jpeg([0, 0, 255]));
            *host.bif.lock().await = changed.clone();
            std::fs::write(&*host.bif_path.lock().await, changed).unwrap();
        }
        "bif-local" => {
            std::fs::write(&*host.bif_path.lock().await, bif(&jpeg([0, 0, 255]))).unwrap();
        }
        _ => {}
    }
    drop(items);
    host.logs.lock().await.extend([
        format!("Info fixture: RefreshItem Start: {id} metadata"),
        format!("Info fixture: RefreshItem Complete: {id} metadata"),
    ]);
    StatusCode::NO_CONTENT
}

async fn forbidden_write(State(host): State<Host>, uri: axum::http::Uri) -> StatusCode {
    host.writes
        .lock()
        .await
        .push((uri.path().into(), HashMap::new()));
    StatusCode::BAD_REQUEST
}

async fn tmdb(State(host): State<Host>, Path(path): Path<String>) -> Response {
    host.tmdb_reads.lock().await.push(path.clone());
    if *host.fault.lock().await == "tmdb" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"fixture metadata unavailable"})),
        )
            .into_response();
    }
    let common = json!({"id":500,"original_language":"en","overview":"新的简介","genres":[],"credits":{"cast":[],"crew":[]},"images":{"posters":[]},"external_ids":{},"production_companies":[],"keywords":{"results":[]}});
    let mut data = common;
    match path.as_str() {
        "movie/500" => {
            data["title"] = json!("新电影标题");
            data["original_title"] = json!("Original Movie");
            data["release_date"] = json!("2026-01-01");
            data["runtime"] = json!(2);
        }
        "tv/500" => {
            data["name"] = json!("新电视剧标题");
            data["original_name"] = json!("Original Series");
            data["first_air_date"] = json!("2026-01-01");
            data["seasons"] = json!([{"id":5001,"season_number":1}]);
        }
        "tv/500/season/1" => {
            data["id"] = json!(5001);
            data["name"] = json!("新季标题");
            data["season_number"] = json!(1);
            data["episodes"] = json!([
                {"id":50011,"season_number":1,"episode_number":1,"name":"新分集标题","overview":"新的分集简介","air_date":"2026-01-01","runtime":2,"credits":{"cast":[],"crew":[]}},
                {"id":50012,"season_number":1,"episode_number":2,"name":"不应修改的第二集","overview":"不应修改","air_date":"2026-01-02","runtime":2,"credits":{"cast":[],"crew":[]}}
            ]);
        }
        _ => data = json!({"results":[],"credits":{"cast":[],"crew":[]}}),
    }
    Json(data).into_response()
}

fn jpeg(color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(4, 4, image::Rgb(color));
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode_image(&image)
        .unwrap();
    bytes
}

fn bif(image: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..8].copy_from_slice(b"\x89BIF\r\n\x1a\n");
    bytes[12..16].copy_from_slice(&1_u32.to_le_bytes());
    bytes[16..24].copy_from_slice(&1000_u64.to_le_bytes());
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend(80_u32.to_le_bytes());
    bytes.extend(u32::MAX.to_le_bytes());
    bytes.extend((80_u32 + image.len() as u32).to_le_bytes());
    bytes.extend(image);
    bytes
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    host: Host,
    target_path: PathBuf,
    json_path: PathBuf,
    original_json: Vec<u8>,
    protected_files: Vec<(PathBuf, Vec<u8>)>,
}

impl Fixture {
    async fn new(kind: &str) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("rescrape_{}", uuid::Uuid::new_v4().simple());
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
        let library = directory.path().join("library");
        let media = if kind == "Episode" {
            library.join("Series/Season 01")
        } else {
            library.join("Movie")
        };
        std::fs::create_dir_all(&media).unwrap();
        let filename = if kind == "Episode" {
            "Original.S01E01.strm"
        } else {
            "Original.strm"
        };
        let target_path = media.join(filename);
        let source = directory.path().join("original.mkv");
        std::fs::write(&source, vec![0_u8; 1000]).unwrap();
        std::fs::write(&target_path, source.to_string_lossy().as_bytes()).unwrap();
        let streams = json!([
            {"Index":0,"Type":"Video","Codec":"hevc","Width":1920,"Height":1080},
            {"Index":1,"Type":"Audio","Codec":"aac","Language":"jpn","Title":"用户保留的日语标题","Channels":2,"SampleRate":48000},
            {"Index":2,"Type":"Subtitle","Codec":"subrip","Language":"cht","Title":"用户保留的繁英字幕","IsExternal":false}
        ]);
        let chapters = json!([{"Name":"原章节","MarkerType":"Chapter","StartPositionTicks":0_u64,"ImageTag":"same-tag","ChapterIndex":0}]);
        let guid = uuid::Uuid::new_v4().simple().to_string();
        let mut item = json!({"Id":"101","Type":kind,"Name":"旧标题","Overview":"旧简介","Path":target_path,"Guid":guid,"PresentationUniqueKey":"unique-101","PartCount":1,"Size":1000,"RunTimeTicks":1_200_000_000_u64,"DateLastRefreshed":"before-fixture-refresh","ProviderIds":{"Tmdb":"500"},"People":[],"Genres":[],"Chapters":chapters,"MediaStreams":streams,"MediaSources":[{"Id":"mediasource_101","ItemId":"101","Path":source,"Size":1000,"Container":"mkv","RunTimeTicks":1_200_000_000_u64,"MediaStreams":streams}]});
        if kind == "Episode" {
            item["ProviderIds"]["Tmdb"] = json!("50011");
            item["ParentId"] = json!("302");
            item["SeriesId"] = json!("301");
            item["SeasonId"] = json!("302");
            item["ParentIndexNumber"] = json!(1);
            item["IndexNumber"] = json!(1);
        }
        let mut source_info = item["MediaSources"][0].clone();
        for key in ["Id", "ItemId"] {
            source_info.as_object_mut().unwrap().remove(key);
        }
        let info = json!([{"MediaSourceInfo":source_info,"Chapters":chapters,"UnknownEnvelope":{"preserve":"exact bytes"}}]);
        // Formatting and trailing whitespace are intentional: equality of parsed
        // JSON is insufficient evidence that metadata-only work did not rewrite it.
        let original_json =
            format!("{}\n \n", serde_json::to_string_pretty(&info).unwrap()).into_bytes();
        let json_path = target_path.with_file_name(format!(
            "{}-mediainfo.json",
            target_path.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::write(&json_path, &original_json).unwrap();
        let chapter_path = directory
            .path()
            .join("metadata/library")
            .join(&guid[..2])
            .join(&guid)
            .join("chapters/0.jpg");
        std::fs::create_dir_all(chapter_path.parent().unwrap()).unwrap();
        let original_image = jpeg([255, 0, 0]);
        std::fs::write(&chapter_path, &original_image).unwrap();
        let bif_path = target_path.with_file_name(format!(
            "{}-320-4.bif",
            target_path.file_stem().unwrap().to_string_lossy()
        ));
        let original_bif = bif(&original_image);
        std::fs::write(&bif_path, &original_bif).unwrap();
        let host = Host {
            configuration: Arc::new(Mutex::new(
                json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"}),
            )),
            chapter_jpeg: Arc::new(Mutex::new(original_image.clone())),
            bif: Arc::new(Mutex::new(original_bif.clone())),
            chapter_path: Arc::new(Mutex::new(chapter_path.clone())),
            bif_path: Arc::new(Mutex::new(bif_path.clone())),
            ..Host::default()
        };
        host.items.lock().await.insert("101".into(), item);
        let mut protected_files = vec![(chapter_path, original_image), (bif_path, original_bif)];
        if kind == "Episode" {
            let series_path = media.parent().unwrap().to_path_buf();
            host.items.lock().await.insert("301".into(),json!({"Id":"301","Type":"Series","Name":"旧电视剧","Path":series_path,"ProviderIds":{"Tmdb":"500"},"People":[],"Genres":[]}));
            host.items.lock().await.insert("302".into(),json!({"Id":"302","Type":"Season","Name":"旧季","Path":media,"ParentId":"301","SeriesId":"301","IndexNumber":1}));
            let other_path = media.join("Original.S01E02.strm");
            std::fs::write(&other_path, source.to_string_lossy().as_bytes()).unwrap();
            let mut other = host.items.lock().await["101"].clone();
            other["Id"] = json!("102");
            other["Path"] = json!(other_path);
            other["Guid"] = json!(uuid::Uuid::new_v4().simple().to_string());
            other["PresentationUniqueKey"] = json!("unique-102");
            other["ProviderIds"]["Tmdb"] = json!("50012");
            other["IndexNumber"] = json!(2);
            other["MediaSources"][0]["ItemId"] = json!("102");
            other["MediaSources"][0]["Id"] = json!("mediasource_102");
            other["Chapters"] = json!([]);
            host.items.lock().await.insert("102".into(), other);
            for (path, bytes) in [
                (
                    series_path.join("tvshow.nfo"),
                    b"original series nfo".to_vec(),
                ),
                (media.join("season.nfo"), b"original season nfo".to_vec()),
                (
                    other_path.with_extension("nfo"),
                    b"original other episode nfo".to_vec(),
                ),
                (
                    series_path.join("poster.jpg"),
                    b"original series poster".to_vec(),
                ),
                (
                    series_path.join("season01-poster.jpg"),
                    b"original season poster".to_vec(),
                ),
                (
                    media.join("Original.S01E02-thumb.jpg"),
                    b"original other thumbnail".to_vec(),
                ),
            ] {
                std::fs::write(&path, &bytes).unwrap();
                protected_files.push((path, bytes));
            }
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json) VALUES('500','Series','旧电视剧',NULL,NULL,NULL,true,'[\"301\"]','[]'),('5001','Season','旧季','500',1,NULL,true,'[\"302\"]','[]'),('50012','Episode','旧第二集','500',1,2,true,'[\"102\"]','[]')")
                .execute(&state.db).await.unwrap();
        }
        let provider = if kind == "Movie" { "500" } else { "50011" };
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,overview,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json,actors_json) VALUES($1,$2,'旧标题','旧简介',$3,$4,$5,true,$6,$7,$8,$9,'[]')")
            .bind(provider).bind(kind).bind(if kind=="Episode" {Some("500")} else {None})
            .bind(if kind=="Episode" {Some(1_i32)} else {None}).bind(if kind=="Episode" {Some(1_i32)} else {None})
            .bind(json!(["101"])).bind(json!([{"emby_item_id":"101","path":target_path,"size_bytes":1000}]))
            .bind(json!(["a".repeat(40)])).bind(json!(["pc101"])).execute(&state.db).await.unwrap();
        let location = library.to_string_lossy().into_owned();
        let router = Router::new()
            .route("/Users/user/Items/{id}",get(details))
            .route("/Items",get(items))
            .route("/Items/{id}/Refresh",post(refresh))
            .route("/Items/{id}/ClearMediaInfo",post(forbidden_write))
            .route("/Items/{id}/SyncMediaInfo",post(forbidden_write))
            .route("/ScheduledTasks/Running/{id}",post(forbidden_write))
            .route("/ScheduledTasks",get(||async {Json(json!([]))}))
            .route("/Users/user",get(||async {Json(json!({"Id":"user","Policy":{"IsAdministrator":true,"MaxParentalRating":null,"BlockUnratedItems":[],"BlockedTags":[],"IsTagBlockingModeInclusive":false,"AllowTagOrRating":false}}))}))
            .route("/Videos/{id}/AdditionalParts",get(||async {Json(json!({"Items":[],"TotalRecordCount":0}))}))
            .route("/Users/user/Items/{id}/SpecialFeatures",get(||async {Json(json!([]))}))
            .route("/Items/{id}/ThumbnailSet",get(||async {Json(json!({"Thumbnails":[{"PositionTicks":0}]}))}))
            .route("/Items/{id}/Images/Chapter/{index}",get(|State(host):State<Host>|async move {host.chapter_jpeg.lock().await.clone()}))
            .route("/Videos/{id}/index.bif",get(|State(host):State<Host>|async move {host.bif.lock().await.clone()}))
            .route("/Items/{id}/Ancestors",get(||async {Json(json!([]))}))
            .route("/Library/VirtualFolders",get(move || {let location=location.clone();async move {Json(json!([{"ItemId":"library","Guid":"fixture-library","Locations":[location]}]))}}))
            .route("/System/Logs/embyserver.txt/Lines",get(|State(host):State<Host>,Query(query):Query<HashMap<String,String>>|async move {let logs=host.logs.lock().await;Json(json!({"TotalRecordCount":logs.len(),"Items":if query.get("Limit").is_some_and(|limit|limit=="0") {vec![]} else {logs.iter().rev().cloned().collect::<Vec<_>>()}}))}))
            .route("/web/ConfigurationPages",get(||async {Json(json!([{"Name":"MediaInfoExtractPageView","Href":"configurationpage?PageId=fixture:MediaInfoExtractPageView"}]))}))
            .route("/UI/View",get(|State(host):State<Host>|async move {Json(json!({"EditObjectContainer":{"Object":host.configuration.lock().await.clone()}}))}))
            .route("/UI/Command",post(|State(host):State<Host>,Json(body):Json<Value>|async move {host.writes.lock().await.push(("configuration".into(),HashMap::new()));*host.configuration.lock().await=serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();Json(json!({}))}))
            .route("/tmdb/{*path}",get(tmdb))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":library,"emby_metadata_path":directory.path().join("metadata"),"tmdb_api_key":"fixture","tmdb_api_base_url":format!("{base}/tmdb"),"douban_enable_online_api":false,"ai_api_key":"","ai_translate_title":false,"ai_translate_overview":false,"ai_translate_episode_overview":false,"ai_translate_actor_role":false,"telegram_bot_token":"","telegram_channel_id":"","monitor_sha1_pc_search":false,"emby_webhook_ingest_enabled":false,"item_delay":0})).await.unwrap();
        Self {
            state,
            admin,
            schema,
            _directory: directory,
            server,
            host,
            target_path,
            json_path,
            original_json,
            protected_files,
        }
    }

    async fn run(&self) -> Value {
        self.run_scope("101", "rescrape").await
    }

    async fn run_scope(&self, target: &str, mode: &str) -> Value {
        let session = self.state.sessions.sign(&Session {
            user_id: "operator".into(),
            name: "Test Operator".into(),
            setup_mode: false,
            expires: chrono::Utc::now().timestamp() + 600,
        });
        let response = emby_toolkit::actions::router()
            .with_state(self.state.clone())
            .oneshot(
                Request::post("/api/actions/reorganize")
                    .header("content-type", "application/json")
                    .header("cookie", format!("etk_session={session}"))
                    .body(Body::from(
                        json!({"item_ids":[target],"mode":mode}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let accepted: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let id = accepted["queue_ids"][0].as_str().unwrap();
        let cancellation = CancellationToken::new();
        let worker = tokio::spawn(emby_toolkit::media_organize_queue::run(
            self.state.clone(),
            cancellation.clone(),
        ));
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let row: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1",
                )
                .bind(id)
                .fetch_one(&self.state.db)
                .await
                .unwrap();
                if !matches!(row["status"].as_str(), Some("queued" | "running")) {
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("metadata-only queue did not reach a verified terminal state");
        cancellation.cancel();
        worker.await.unwrap();
        result
    }

    async fn unchanged_resources(&self) {
        assert_eq!(std::fs::read(&self.json_path).unwrap(), self.original_json);
        for (path, bytes) in &self.protected_files {
            assert_eq!(std::fs::read(path).unwrap(), *bytes, "{}", path.display());
        }
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
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all upstreams are localhost"]
async fn movie_rescrape_updates_metadata_without_rebuilding_any_media_resource() {
    let f = Fixture::new("Movie").await;
    let before = f.host.items.lock().await["101"].clone();
    let row = f.run().await;
    assert_eq!(row["status"], "completed", "{row}");
    let after = f.host.items.lock().await["101"].clone();
    assert_eq!(after["Name"], "新电影标题");
    assert_eq!(after["Overview"], "新的简介");
    for field in ["MediaStreams", "MediaSources", "Chapters"] {
        assert_eq!(after[field], before[field], "{field}");
    }
    f.unchanged_resources().await;
    let writes = f.host.writes.lock().await.clone();
    let refreshes = writes
        .iter()
        .filter(|(action, _)| action.starts_with("refresh:"))
        .collect::<Vec<_>>();
    assert_eq!(refreshes.len(), 1, "{writes:?}");
    assert_eq!(refreshes[0].0, "refresh:101");
    assert_eq!(
        refreshes[0].1.get("Recursive").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        refreshes[0]
            .1
            .get("MetadataRefreshMode")
            .map(String::as_str),
        Some("FullRefresh")
    );
    for option in ["ReplaceAllMetadata", "ReplaceAllImages"] {
        assert_eq!(
            refreshes[0].1.get(option).map(String::as_str),
            Some("false")
        );
    }
    assert!(
        !writes
            .iter()
            .any(|(action, _)| action.contains("ClearMediaInfo")
                || action.contains("SyncMediaInfo")
                || action.contains("ScheduledTasks")),
        "{writes:?}"
    );
    assert_eq!(
        f.host.configuration.lock().await["ExclusiveControlFeatures"],
        "ExtractOnFileChange"
    );
    let cached: Value = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='500' AND item_type='Movie'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    assert_eq!(cached["title"], "新电影标题");
    assert_eq!(cached["file_sha1_json"], json!(["a".repeat(40)]));
    assert_eq!(cached["file_pickcode_json"], json!(["pc101"]));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all upstreams are localhost"]
async fn episode_rescrape_leaves_parent_and_other_episode_metadata_and_files_unchanged() {
    let f = Fixture::new("Episode").await;
    let cached: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id<>'50011' ORDER BY tmdb_id",
    )
    .fetch_all(&f.state.db)
    .await
    .unwrap();
    let row = f.run().await;
    assert_eq!(row["status"], "completed", "{row}");
    assert_eq!(f.host.items.lock().await["101"]["Name"], "新分集标题");
    assert!(
        std::fs::read_to_string(f.target_path.with_extension("nfo"))
            .unwrap()
            .contains("新分集标题")
    );
    f.unchanged_resources().await;
    let after: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id<>'50011' ORDER BY tmdb_id",
    )
    .fetch_all(&f.state.db)
    .await
    .unwrap();
    assert_eq!(after, cached);
    assert_eq!(f.host.items.lock().await["301"]["Name"], "旧电视剧");
    assert_eq!(f.host.items.lock().await["302"]["Name"], "旧季");
    assert_eq!(f.host.items.lock().await["102"]["Name"], "旧标题");
    let writes = f.host.writes.lock().await;
    assert_eq!(
        writes
            .iter()
            .filter(|(action, _)| action.starts_with("refresh:"))
            .map(|(action, _)| action.as_str())
            .collect::<Vec<_>>(),
        vec!["refresh:101"]
    );
    drop(writes);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all upstreams are localhost"]
async fn wash_pending_is_rejected_before_metadata_or_original_resources_are_changed() {
    let f = Fixture::new("Movie").await;
    sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,'{0,moviepilot_media_pending}','true') WHERE tmdb_id='500' AND item_type='Movie'").execute(&f.state.db).await.unwrap();
    let row = f.run().await;
    assert_eq!(row["status"], "failed", "{row}");
    assert!(row.to_string().contains("重新整理入库"), "{row}");
    assert!(!f.target_path.with_extension("nfo").exists());
    assert!(f.host.writes.lock().await.is_empty());
    assert!(f.host.tmdb_reads.lock().await.is_empty());
    f.unchanged_resources().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all upstreams are localhost"]
async fn changed_resources_after_refresh_never_report_success_and_keep_original_recovery_materials()
{
    for fault in [
        "streams",
        "chapters",
        "json",
        "chapter",
        "chapter-local",
        "bif",
        "bif-local",
    ] {
        let f = Fixture::new("Movie").await;
        *f.host.fault.lock().await = fault.into();
        let row = f.run().await;
        assert_eq!(row["status"], "failed", "fault {fault}: {row}");
        assert!(
            f.host
                .writes
                .lock()
                .await
                .iter()
                .any(|(operation, _)| operation == "refresh:101"),
            "fault {fault}: failed before the intended native mutation: {row}"
        );
        let root = f.state.data_dir.join("backups/media-info-reload");
        assert!(root.is_dir(), "fault {fault}: missing recovery root: {row}");
        let mut pending = vec![root];
        let mut original_json_found = false;
        let mut original_jpeg_found = false;
        let mut original_bif_found = false;
        while let Some(path) = pending.pop() {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                    continue;
                }
                let bytes = std::fs::read(entry.path()).unwrap();
                original_json_found |= bytes == f.original_json;
                original_jpeg_found |= bytes == f.protected_files[0].1;
                original_bif_found |= bytes == f.protected_files[1].1;
            }
        }
        assert!(
            original_json_found && original_jpeg_found && original_bif_found,
            "fault {fault}: incomplete original recovery materials: {row}"
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all upstreams are localhost"]
async fn metadata_failure_before_native_refresh_preserves_resources_and_allows_a_real_retry() {
    let f = Fixture::new("Movie").await;
    *f.host.fault.lock().await = "tmdb".into();
    let failed = f.run().await;
    assert_eq!(failed["status"], "failed", "{failed}");
    assert!(
        !f.host
            .writes
            .lock()
            .await
            .iter()
            .any(|(operation, _)| operation.starts_with("refresh:"))
    );
    f.unchanged_resources().await;
    assert_eq!(
        f.host.configuration.lock().await["ExclusiveControlFeatures"],
        "ExtractOnFileChange"
    );
    *f.host.fault.lock().await = String::new();
    let retried = f.run().await;
    assert_eq!(retried["status"], "completed", "{retried}");
    f.unchanged_resources().await;
    f.close().await;
}

// Use the existing metadata-only fixture unchanged, adding the verified native
// file-import contract only for these full reorganization scope regressions.
#[derive(Clone)]
struct ReprocessProxy {
    base: String,
    db: sqlx::PgPool,
    host: Host,
    selected: Arc<Mutex<Vec<String>>>,
    imports: Arc<Mutex<Vec<String>>>,
    binding_faults: Arc<Mutex<Vec<String>>>,
}

fn reprocess_sync_export(item: &Value, index: usize) -> Value {
    let mut source = item["MediaSources"][index].clone();
    for field in ["Id", "ItemId", "Path", "Name"] {
        source[field] = Value::Null;
    }
    json!([{"MediaSourceInfo":source,"Chapters":item["Chapters"]}])
}

async fn reprocess_proxy(
    State(proxy): State<ReprocessProxy>,
    request: axum::extract::Request,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    let query = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>();
    if method == axum::http::Method::GET {
        match path.as_str() {
            "/System/Info/Public" => {
                return Json(json!({"Id":"rescrape-scope-fixture"})).into_response();
            }
            "/Plugins" => {
                return Json(json!([{"Id":"63c322b7-a371-41a3-b11f-04f8418b37d8","Name":"Strm Assistant PRO","Version":"3.0.0.57"}])).into_response();
            }
            "/ScheduledTasks" => {
                return Json(json!([
                    {"Id":"info","Key":"MediaInfoExtractTask","State":"Idle"},
                    {"Id":"intro","Key":"IntroFingerprintExtractTask","State":"Idle"},
                    {"Id":"thumb","Key":"VideoThumbnailExtractTask","State":"Idle"}
                ]))
                .into_response();
            }
            "/swagger.json" => {
                let mut contract: Value =
                    serde_json::from_str(include_str!("fixtures/pro_sync_contract.json")).unwrap();
                contract["paths"]["/Items/{Id}/Refresh"] = json!({"post":{"parameters":[
                    {"name":"Id","in":"path"},{"name":"MetadataRefreshMode","in":"query"},
                    {"name":"ImageRefreshMode","in":"query"},{"name":"ReplaceAllMetadata","in":"query"},
                    {"name":"ReplaceAllImages","in":"query"}
                ]}});
                return Json(contract).into_response();
            }
            _ => {}
        }
        if proxy
            .selected
            .lock()
            .await
            .iter()
            .any(|id| path == format!("/Items/{id}/ThumbnailSet"))
        {
            return Json(json!({"Thumbnails":[]})).into_response();
        }
    }
    if method == axum::http::Method::POST && path == "/Items/SyncMediaInfo" {
        let id = query.get("Id").expect("Sync must select one native ItemId");
        assert!(!query.contains_key("Path") && !query.contains_key("Recursive"));
        assert!(proxy.selected.lock().await.contains(id));
        let body = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
        let info: Value = serde_json::from_slice(&body).unwrap();
        assert!(info.as_array().is_some_and(|bundles| bundles.len() == 1));
        assert!(emby_toolkit::media_info::recoverable(&info));
        let mut items = proxy.host.items.lock().await;
        let item = items.get_mut(id).expect("Sync target must exist");
        assert_eq!(item["Id"], *id);
        let item_path = PathBuf::from(item["Path"].as_str().unwrap());
        let formal = item_path.with_file_name(format!(
            "{}-mediainfo.json",
            item_path.file_stem().unwrap().to_string_lossy()
        ));
        let current: Value = serde_json::from_slice(&std::fs::read(&formal).unwrap()).unwrap();
        assert_eq!(
            info, current,
            "Sync must read the actual edited formal JSON"
        );
        let owned = item["MediaSources"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, source)| source["ItemId"] == *id)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(
            owned.len(),
            1,
            "Sync must resolve exactly the selected native source"
        );
        let index = owned[0];
        let has_media = !item["RunTimeTicks"].is_null()
            && item["Size"] != 0
            && item["MediaStreams"].as_array().is_some_and(|streams| {
                streams
                    .iter()
                    .any(|stream| matches!(stream["Type"].as_str(), Some("Video" | "Audio")))
            });
        if has_media {
            let exported = reprocess_sync_export(item, index);
            std::fs::write(formal, serde_json::to_vec(&exported).unwrap()).unwrap();
            proxy
                .host
                .writes
                .lock()
                .await
                .push((format!("sync-noop:{id}"), query.clone()));
            return Json(exported).into_response();
        }
        assert_eq!(
            item["Size"], 0,
            "only a cleared native target accepts edited information"
        );
        assert!(item["MediaStreams"].as_array().unwrap().is_empty());
        assert!(item["Chapters"].as_array().unwrap().is_empty());
        let mut source = info[0]["MediaSourceInfo"].clone();
        if !source["Path"].is_null() {
            assert_eq!(source["Path"], item["MediaSources"][index]["Path"]);
        }
        source["Id"] = item["MediaSources"][index]["Id"].clone();
        source["ItemId"] = json!(id);
        source["Path"] = item["MediaSources"][index]["Path"].clone();
        if let Some(name) = item["MediaSources"][index].get("Name") {
            source["Name"] = name.clone();
        } else {
            source.as_object_mut().unwrap().remove("Name");
        }
        if let Some(kind) = item["MediaSources"][index].get("Type") {
            source["Type"] = kind.clone();
        } else {
            source.as_object_mut().unwrap().remove("Type");
        }
        item["MediaStreams"] = source["MediaStreams"].clone();
        item["Size"] = source["Size"].clone();
        item["RunTimeTicks"] = source["RunTimeTicks"].clone();
        item["Chapters"] = info[0]["Chapters"].clone();
        item["Container"] = source["Container"].clone();
        item["TotalBitrate"] = source["Bitrate"].clone();
        if let Some(video) = source["MediaStreams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["Type"] == "Video")
        {
            item["Width"] = video["Width"].clone();
            item["Height"] = video["Height"].clone();
        }
        item["MediaSources"][index] = source;
        let exported = reprocess_sync_export(item, index);
        std::fs::write(formal, serde_json::to_vec(&exported).unwrap()).unwrap();
        proxy.imports.lock().await.push(id.clone());
        proxy
            .host
            .writes
            .lock()
            .await
            .push((format!("sync:{id}"), query.clone()));
        return Json(exported).into_response();
    }
    if method == axum::http::Method::POST
        && let Some(id) = path
            .strip_prefix("/Items/")
            .and_then(|path| path.strip_suffix("/ClearMediaInfo"))
    {
        assert!(
            proxy
                .selected
                .lock()
                .await
                .iter()
                .any(|selected| selected == id)
        );
        assert!(query.is_empty());
        let mut items = proxy.host.items.lock().await;
        let key = items[id]["PresentationUniqueKey"].clone();
        assert!(key.as_str().is_some_and(|key| !key.is_empty()));
        let related = items
            .values()
            .filter(|item| item["PresentationUniqueKey"] == key)
            .map(|item| item["Id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            related,
            vec![id.to_string()],
            "Clear was submitted without proving its native association scope"
        );
        let item = items.get_mut(id).unwrap();
        assert!(
            item["MediaSources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|source| source["ItemId"] == id)
        );
        let item_path = PathBuf::from(item["Path"].as_str().unwrap());
        let stem = item_path.file_stem().unwrap().to_string_lossy();
        let formal = item_path.with_file_name(format!("{stem}-mediainfo.json"));
        if formal.exists() {
            std::fs::remove_file(formal).unwrap();
        }
        let bif = item_path.with_file_name(format!("{stem}-320-4.bif"));
        if bif.exists() {
            std::fs::remove_file(bif).unwrap();
        }
        let original_chapter = proxy.host.chapter_path.lock().await.clone();
        let library = original_chapter.ancestors().nth(4).unwrap();
        let guid = item["Guid"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(guid).is_ok());
        let chapters = library.join(&guid[..2]).join(guid).join("chapters");
        if chapters.exists() {
            std::fs::remove_dir_all(chapters).unwrap();
        }
        item["MediaStreams"] = json!([]);
        item["Chapters"] = json!([]);
        item["Size"] = json!(0);
        item["RunTimeTicks"] = Value::Null;
        item["Container"] = Value::Null;
        item["Width"] = json!(0);
        item["Height"] = json!(0);
        item["TotalBitrate"] = json!(0);
        for source in item["MediaSources"].as_array_mut().unwrap() {
            source["MediaStreams"] = json!([]);
            source["Chapters"] = json!([]);
            source["Size"] = json!(0);
            source["RunTimeTicks"] = Value::Null;
            source["Container"] = Value::Null;
        }
        proxy
            .host
            .writes
            .lock()
            .await
            .push((format!("clear:{id}"), query.clone()));
        return StatusCode::NO_CONTENT.into_response();
    }
    if method == axum::http::Method::POST
        && let Some(id) = path
            .strip_prefix("/Items/")
            .and_then(|path| path.strip_suffix("/Refresh"))
    {
        proxy
            .host
            .writes
            .lock()
            .await
            .push((format!("refresh:{id}"), query.clone()));
        let mut items = proxy.host.items.lock().await;
        let item = items
            .get_mut(id)
            .expect("refresh target must exist in the native inventory");
        let item_path = PathBuf::from(item["Path"].as_str().unwrap());
        let importing = query
            .get("ImageRefreshMode")
            .is_some_and(|mode| mode == "ValidationOnly")
            && query
                .get("MetadataRefreshMode")
                .is_some_and(|mode| mode == "Default")
            && proxy.host.configuration.lock().await["PersistMediaInfoMode"] == "Restore";
        if importing {
            assert_eq!(query.get("Recursive").map(String::as_str), Some("false"));
            for option in [
                "ReplaceAllMetadata",
                "ReplaceAllImages",
                "ReplaceThumbnailImages",
            ] {
                assert_eq!(query.get(option).map(String::as_str), Some("false"));
            }
            let info_path = item_path.with_file_name(format!(
                "{}-mediainfo.json",
                item_path.file_stem().unwrap().to_string_lossy()
            ));
            let info: Value = serde_json::from_slice(&std::fs::read(info_path).unwrap()).unwrap();
            assert!(
                emby_toolkit::media_info::recoverable(&info),
                "formal JSON was not recoverable"
            );
            let mut source = info[0]["MediaSourceInfo"].clone();
            source["Id"] = item["MediaSources"][0]["Id"].clone();
            source["ItemId"] = json!(id);
            item["MediaStreams"] = source["MediaStreams"].clone();
            item["Size"] = source["Size"].clone();
            item["RunTimeTicks"] = source["RunTimeTicks"].clone();
            item["Chapters"] = info[0]["Chapters"].clone();
            item["MediaSources"] = json!([source]);
            proxy.imports.lock().await.push(id.to_string());
        } else {
            // Every metadata change is read from the actual NFO written by ETK.
            let nfo = match item["Type"].as_str() {
                Some("Series") => item_path.join("tvshow.nfo"),
                Some("Season") => item_path.join("season.nfo"),
                _ => item_path.with_extension("nfo"),
            };
            let xml = std::fs::read_to_string(&nfo).unwrap_or_else(|error| {
                panic!(
                    "metadata refresh requires a real NFO {}: {error}",
                    nfo.display()
                )
            });
            item["Name"] = json!(nfo_element(&xml, "title"));
            item["Overview"] = json!(nfo_element(&xml, "plot"));
            if let Ok(year) = nfo_element(&xml, "year").parse::<i64>() {
                item["ProductionYear"] = json!(year);
            }
            let premiered = nfo_element(&xml, "premiered");
            if !premiered.is_empty() {
                item["PremiereDate"] = json!(premiered);
            }
            let end = nfo_element(&xml, "enddate");
            item["EndDate"] = if end.is_empty() {
                Value::Null
            } else {
                json!(end)
            };
            if id == "101" && *proxy.host.fault.lock().await == "binding-sha" {
                sqlx::query("UPDATE media_metadata SET file_sha1_json=jsonb_set(file_sha1_json,'{0}',$1) WHERE tmdb_id='50011' AND item_type='Episode'")
                    .bind(json!("C".repeat(40))).execute(&proxy.db).await.unwrap();
                proxy.binding_faults.lock().await.push(id.to_string());
            }
        }
        item["DateLastRefreshed"] = json!(format!(
            "scope-refresh-{id}-{}",
            proxy.host.writes.lock().await.len()
        ));
        drop(items);
        proxy.host.logs.lock().await.extend([
            format!("Info fixture: RefreshItem Start: {id} scope"),
            format!("Info fixture: RefreshItem Complete: {id} scope"),
        ]);
        return StatusCode::NO_CONTENT.into_response();
    }
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 1024 * 1024).await.unwrap();
    let upstream = reqwest::Client::new()
        .request(method, format!("{}{}", proxy.base, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let mut response = Response::new(Body::from(upstream.bytes().await.unwrap()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

async fn actual_episode_source(directory: &std::path::Path, id: &str, color: &str) -> PathBuf {
    let path = directory.join(format!("reorganize-{id}.mkv"));
    let output = tokio::process::Command::new("ffmpeg")
        .kill_on_drop(true)
        .args([
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-filter_threads",
            "1",
            "-filter_complex_threads",
            "1",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("color=c={color}:s=16x16:r=10:d=0.3"))
        .args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-t",
            "0.3",
            "-threads",
            "1",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-metadata:s:a:0",
            "language=jpn",
            "-metadata:s:a:0",
            "title=日语",
            "-y",
        ])
        .arg(&path)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    path
}

async fn reprocess_fixture(
    ids: &[&str],
    alternate: bool,
) -> (Fixture, ReprocessProxy, tokio::task::JoinHandle<()>) {
    use sha1::{Digest, Sha1};
    let mut f = Fixture::new("Episode").await;
    *f.host.configuration.lock().await = json!({
        "ExclusiveControlFeatures":"ExtractOnFileChange", "PersistMediaInfoMode":"Default",
        "ExclusiveExtract":true, "IsModSupported":true
    });
    for (index, id) in ids.iter().enumerate() {
        let source = actual_episode_source(
            f._directory.path(),
            id,
            if index == 0 { "black" } else { "blue" },
        )
        .await;
        let bytes = std::fs::read(&source).unwrap();
        let sha = format!("{:X}", Sha1::digest(&bytes));
        let mut items = f.host.items.lock().await;
        let item = items.get_mut(*id).unwrap();
        let path = PathBuf::from(item["Path"].as_str().unwrap());
        std::fs::write(&path, source.to_string_lossy().as_bytes()).unwrap();
        item["Size"] = json!(bytes.len());
        item["MediaSources"][0]["Path"] = json!(source);
        item["MediaSources"][0]["Size"] = json!(bytes.len());
        item["Chapters"] = json!([]);
        let mut info_source = item["MediaSources"][0].clone();
        for field in ["Id", "ItemId"] {
            info_source.as_object_mut().unwrap().remove(field);
        }
        let old_info = json!([{"MediaSourceInfo":info_source,"Chapters":[],"UnknownEnvelope":{"preserve":"old raw streams"}}]);
        let info_path = path.with_file_name(format!(
            "{}-mediainfo.json",
            path.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::write(info_path, old_info.to_string()).unwrap();
        let provider = item["ProviderIds"]["Tmdb"].as_str().unwrap().to_string();
        drop(items);
        sqlx::query("UPDATE media_metadata SET asset_details_json=$1,file_sha1_json=$2,file_pickcode_json=$3 WHERE tmdb_id=$4 AND item_type='Episode'")
            .bind(json!([{"emby_item_id":id,"path":path,"size_bytes":bytes.len()}]))
            .bind(json!([sha])).bind(json!([format!("pc{id}")])).bind(provider)
            .execute(&f.state.db).await.unwrap();
    }
    // Initial selected media have no old native previews to retire; the tests
    // below protect the other episode/version's markers and sidecars instead.
    std::fs::remove_file(&*f.host.chapter_path.lock().await).unwrap();
    std::fs::remove_file(&*f.host.bif_path.lock().await).unwrap();
    if alternate {
        let mut items = f.host.items.lock().await;
        items.get_mut("102").unwrap()["Chapters"] = json!([
            {"MarkerType":"IntroStart","Name":"第二集片头","StartPositionTicks":10_000_000_u64},
            {"MarkerType":"IntroEnd","Name":"第二集片头结束","StartPositionTicks":200_000_000_u64},
            {"MarkerType":"CreditsStart","Name":"第二集片尾","StartPositionTicks":1_100_000_000_u64}
        ]);
        let mut other = items["102"].clone();
        other["Id"] = json!("103");
        other["Name"] = json!("另一版本原标题");
        other["IndexNumber"] = json!(1);
        other["ProviderIds"]["Tmdb"] = json!("50011");
        other["Guid"] = json!(uuid::Uuid::new_v4().simple().to_string());
        other["PresentationUniqueKey"] = json!("unique-103");
        let path = f
            .target_path
            .parent()
            .unwrap()
            .join("Original.Extended.S01E01.strm");
        other["Path"] = json!(path);
        other["MediaSources"][0]["Id"] = json!("mediasource_103");
        other["MediaSources"][0]["ItemId"] = json!("103");
        std::fs::write(&path, other["MediaSources"][0]["Path"].as_str().unwrap()).unwrap();
        let alternate_sha = format!(
            "{:X}",
            Sha1::digest(
                &std::fs::read(other["MediaSources"][0]["Path"].as_str().unwrap()).unwrap()
            )
        );
        items.insert("103".into(), other);
        for id in ["102", "103"] {
            let item = &items[id];
            let path = PathBuf::from(item["Path"].as_str().unwrap());
            let mut info_source = item["MediaSources"][0].clone();
            for field in ["Id", "ItemId"] {
                info_source.as_object_mut().unwrap().remove(field);
            }
            let info = json!([{"MediaSourceInfo":info_source,"Chapters":item["Chapters"],"UnknownEnvelope":{"preserve":"unselected bytes"}}]);
            let info_path = path.with_file_name(format!(
                "{}-mediainfo.json",
                path.file_stem().unwrap().to_string_lossy()
            ));
            let bytes =
                format!("{}\n \n", serde_json::to_string_pretty(&info).unwrap()).into_bytes();
            std::fs::write(&info_path, &bytes).unwrap();
            f.protected_files.push((info_path, bytes));
            if id == "103" {
                let nfo = path.with_extension("nfo");
                let bytes = b"original alternate episode NFO".to_vec();
                std::fs::write(&nfo, &bytes).unwrap();
                f.protected_files.push((nfo, bytes));
            }
        }
        drop(items);
        sqlx::query("UPDATE media_metadata SET emby_item_ids_json=emby_item_ids_json||'[\"103\"]'::jsonb,asset_details_json=asset_details_json||$1::jsonb,file_sha1_json=file_sha1_json||$2::jsonb,file_pickcode_json=file_pickcode_json||'[\"pc103\"]'::jsonb WHERE tmdb_id='50011' AND item_type='Episode'")
            .bind(json!([{"emby_item_id":"103","path":path,"size_bytes":1000,"user_field":"preserve version"}]))
            .bind(json!([alternate_sha])).execute(&f.state.db).await.unwrap();
    }
    let proxy = ReprocessProxy {
        base: f.state.config.read().await["emby_server_url"]
            .as_str()
            .unwrap()
            .to_string(),
        db: f.state.db.clone(),
        host: f.host.clone(),
        selected: Arc::new(Mutex::new(ids.iter().map(|id| id.to_string()).collect())),
        imports: Arc::default(),
        binding_faults: Arc::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let service = proxy.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(reprocess_proxy).with_state(service),
        )
        .await
        .unwrap();
    });
    f.state
        .save_config(
            &json!({"emby_server_url":base,"auto_extract_intro_fingerprint_on_ingest":false,
        "auto_extract_credits_on_ingest":false,"auto_extract_video_thumbnails_on_ingest":false,
        "auto_fill_language_titles_on_ingest":false,"delay_between_items_sec":0}),
        )
        .await
        .unwrap();
    (f, proxy, server)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real ffprobe and native imports are localhost"]
async fn single_episode_reprocess_rejects_asset_sha_drift_after_metadata_refresh() {
    let (f, proxy, server) = reprocess_fixture(&["101"], true).await;
    let other_native = {
        let items = f.host.items.lock().await;
        [items["102"].clone(), items["103"].clone()]
    };
    *f.host.fault.lock().await = "binding-sha".into();
    let row = f.run_scope("101", "reprocess").await;
    assert_eq!(row["status"], "failed", "{row}");
    assert!(
        row["message"]
            .as_str()
            .unwrap()
            .contains("媒体资产身份变化"),
        "{row}"
    );
    assert_eq!(*proxy.imports.lock().await, vec!["101"]);
    assert_eq!(
        *proxy.binding_faults.lock().await,
        vec!["101"],
        "identity drift must occur after the real metadata refresh"
    );
    for (id, before) in ["102", "103"].into_iter().zip(other_native) {
        assert_eq!(f.host.items.lock().await[id], before);
    }
    for (path, bytes) in f.protected_files.iter().skip(2).filter(|(path, _)| {
        !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("tvshow.nfo" | "season.nfo")
        )
    }) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            *bytes,
            "unselected resource {}",
            path.display()
        );
    }
    assert!(
        std::fs::read_dir(f.state.data_dir.join("backups/media-info-reload"))
            .unwrap()
            .next()
            .is_some()
    );
    server.abort();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real ffprobe and native imports are localhost"]
async fn single_episode_reprocess_preserves_other_episode_and_other_version() {
    let (f, proxy, server) = reprocess_fixture(&["101"], true).await;
    let other_native = {
        let items = f.host.items.lock().await;
        [items["102"].clone(), items["103"].clone()]
    };
    let other_row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='50012' AND item_type='Episode'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    let versions_before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='50011' AND item_type='Episode'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    let row = f.run_scope("101", "reprocess").await;
    assert_eq!(row["status"], "completed", "{row}");
    assert_eq!(
        *proxy.imports.lock().await,
        vec!["101"],
        "only the selected file may be imported"
    );
    assert_eq!(f.host.items.lock().await["101"]["Name"], "新分集标题");
    assert_eq!(
        f.host.items.lock().await["101"]["MediaStreams"][0]["Codec"],
        "mpeg4"
    );
    assert!(
        std::fs::read_to_string(f.target_path.with_extension("nfo"))
            .unwrap()
            .contains("新分集标题")
    );
    let series_path = f.target_path.parent().unwrap().parent().unwrap();
    assert!(
        std::fs::read_to_string(series_path.join("tvshow.nfo"))
            .unwrap()
            .contains("新电视剧标题")
    );
    assert!(
        std::fs::read_to_string(f.target_path.parent().unwrap().join("season.nfo"))
            .unwrap()
            .contains("新季标题")
    );
    for (id, before) in ["102", "103"].into_iter().zip(other_native) {
        assert_eq!(
            f.host.items.lock().await[id],
            before,
            "unselected native {id}"
        );
    }
    for (path, bytes) in f.protected_files.iter().skip(2).filter(|(path, _)| {
        !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("tvshow.nfo" | "season.nfo")
        )
    }) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            *bytes,
            "unselected file {}",
            path.display()
        );
    }
    let after_other: Value = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='50012' AND item_type='Episode'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    assert_eq!(after_other, other_row);
    let versions_after: Value = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='50011' AND item_type='Episode'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    for field in [
        "asset_details_json",
        "file_sha1_json",
        "file_pickcode_json",
        "emby_item_ids_json",
    ] {
        assert_eq!(
            versions_after[field][1], versions_before[field][1],
            "unselected version {field}"
        );
    }
    let writes = f.host.writes.lock().await.clone();
    assert!(
        writes.iter().any(|(action, _)| action == "refresh:301"),
        "{writes:?}"
    );
    for (action, query) in writes
        .iter()
        .filter(|(action, _)| action.starts_with("refresh:"))
    {
        assert!(
            matches!(
                action.as_str(),
                "refresh:101" | "refresh:301" | "refresh:302"
            ),
            "{writes:?}"
        );
        assert_eq!(
            query.get("Recursive").map(String::as_str),
            Some("false"),
            "{action}"
        );
    }
    assert!(
        !writes
            .iter()
            .any(|(action, _)| action.contains("ClearMediaInfo")
                || action.contains("SyncMediaInfo")
                || action.contains("ScheduledTasks")),
        "{writes:?}"
    );
    assert_eq!(
        f.host.configuration.lock().await["PersistMediaInfoMode"],
        "Default"
    );
    server.abort();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; actual media probing and native title imports stay localhost"]
async fn single_episode_reprocess_runs_configured_title_fill_and_preserves_unselected_versions() {
    let (f, proxy, server) = reprocess_fixture(&["101"], true).await;
    let unselected = {
        let items = f.host.items.lock().await;
        [items["102"].clone(), items["103"].clone()]
    };
    emby_toolkit::db::save_setting(
        &f.state.db,
        "language_mapping",
        &json!([{"label":"日语定制","value":"jpn","aliases":["日语"]}]),
    )
    .await
    .unwrap();
    f.state
        .save_config(&json!({"auto_fill_language_titles_on_ingest":true,
        "monitor_sha1_pc_search":false,"auto_repair_truehd_core_on_ingest":true}))
        .await
        .unwrap();
    let row = f.run_scope("101", "reprocess").await;
    assert_eq!(row["status"], "completed", "{row}");
    let finalization = &row["result"]["finalization"];
    assert_eq!(finalization["metadata_refresh"]["completed"], true, "{row}");
    assert_eq!(finalization["title_fill"]["completed"], true, "{row}");
    assert!(
        finalization["title_fill"]["changed_streams"]
            .as_u64()
            .unwrap()
            > 0,
        "{row}"
    );
    assert_eq!(finalization["audio_repair"]["scanned"], 1, "{row}");
    let items = f.host.items.lock().await;
    assert!(
        items["101"]["MediaStreams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stream| stream["Type"] == "Audio"
                && stream["Title"]
                    .as_str()
                    .is_some_and(|title| title.contains("日语定制"))),
        "{row}"
    );
    for (id, before) in ["102", "103"].into_iter().zip(unselected) {
        assert_eq!(items[id], before, "unselected native {id}");
    }
    drop(items);
    assert!(
        proxy.imports.lock().await.len() >= 2,
        "media import and the actual edited title import are both required"
    );
    assert!(proxy.imports.lock().await.iter().all(|id| id == "101"));
    let sync_stages = f
        .host
        .writes
        .lock()
        .await
        .iter()
        .filter(|(action, _)| matches!(action.as_str(), "sync-noop:101" | "clear:101" | "sync:101"))
        .map(|(action, _)| action.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        sync_stages,
        vec!["sync-noop:101", "clear:101", "sync:101"],
        "edited titles must come from the actual import into the proven empty target"
    );
    for (path, bytes) in f.protected_files.iter().skip(2).filter(|(path, _)| {
        !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("tvshow.nfo" | "season.nfo")
        )
    }) {
        assert_eq!(std::fs::read(path).unwrap(), *bytes, "{}", path.display());
    }
    assert!(
        row["result"]["automatic_extraction"].is_null(),
        "disabled automatic extraction must stay disabled: {row}"
    );
    server.abort();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; both linked movie versions use real ffprobe and localhost imports"]
async fn movie_reprocess_keeps_its_confirmed_linked_version_scope() {
    use sha1::{Digest, Sha1};
    let (f, proxy, server) = reprocess_fixture(&["101"], false).await;
    proxy.selected.lock().await.push("103".into());
    let source = actual_episode_source(f._directory.path(), "103", "blue").await;
    let bytes = std::fs::read(&source).unwrap();
    let sha = format!("{:X}", Sha1::digest(&bytes));
    let path = f.target_path.parent().unwrap().join("Movie.Extended.strm");
    std::fs::write(&path, source.to_string_lossy().as_bytes()).unwrap();
    let mut items = f.host.items.lock().await;
    let item = items.get_mut("101").unwrap();
    item["Type"] = json!("Movie");
    item["ProviderIds"] = json!({"Tmdb":"500"});
    let mut version = item.clone();
    version["Id"] = json!("103");
    version["Name"] = json!("电影关联版本");
    version["Path"] = json!(path);
    version["Guid"] = json!(uuid::Uuid::new_v4().simple().to_string());
    version["PresentationUniqueKey"] = json!("unique-103");
    version["Size"] = json!(bytes.len());
    version["MediaSources"][0]["Id"] = json!("mediasource_103");
    version["MediaSources"][0]["ItemId"] = json!("103");
    version["MediaSources"][0]["Path"] = json!(source);
    version["MediaSources"][0]["Size"] = json!(bytes.len());
    let mut info_source = version["MediaSources"][0].clone();
    for field in ["Id", "ItemId"] {
        info_source.as_object_mut().unwrap().remove(field);
    }
    std::fs::write(
        path.with_file_name("Movie.Extended-mediainfo.json"),
        json!([{"MediaSourceInfo":info_source,"Chapters":[]}]).to_string(),
    )
    .unwrap();
    items.insert("103".into(), version);
    drop(items);
    sqlx::query("UPDATE media_metadata SET tmdb_id='500',item_type='Movie',parent_series_tmdb_id=NULL,season_number=NULL,episode_number=NULL,emby_item_ids_json=emby_item_ids_json||'[\"103\"]'::jsonb,asset_details_json=asset_details_json||$1::jsonb,file_sha1_json=file_sha1_json||$2::jsonb,file_pickcode_json=file_pickcode_json||'[\"pc103\"]'::jsonb WHERE tmdb_id='50011' AND item_type='Episode'")
        .bind(json!([{"emby_item_id":"103","path":path,"size_bytes":bytes.len()}])).bind(json!([sha])).execute(&f.state.db).await.unwrap();
    let row = f.run_scope("101", "reprocess").await;
    assert_eq!(row["status"], "completed", "{row}");
    let imported = proxy.imports.lock().await.clone();
    assert!(
        imported.iter().any(|id| id == "101") && imported.iter().any(|id| id == "103"),
        "{imported:?}: {row}"
    );
    let marked: Value = sqlx::query_scalar(
        "SELECT asset_details_json FROM media_metadata WHERE tmdb_id='500' AND item_type='Movie'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    for id in ["101", "103"] {
        assert!(
            marked
                .as_array()
                .unwrap()
                .iter()
                .any(|asset| asset["emby_item_id"] == id
                    && asset["moviepilot_ingest_identity"]["id"] == id),
            "{marked}"
        );
    }
    let versions = row["result"]["finalization"]["versions"]
        .as_array()
        .unwrap();
    assert_eq!(versions.len(), 2, "{row}");
    for id in ["101", "103"] {
        assert!(
            versions.iter().any(|version| version["item_id"] == id
                && version["report"]["metadata_refresh"]["completed"] == true),
            "{row}"
        );
        assert!(
            f.host
                .writes
                .lock()
                .await
                .iter()
                .any(|(action, _)| action == &format!("refresh:{id}")),
            "{row}"
        );
        assert_eq!(
            f.host.items.lock().await[id]["Name"],
            "新电影标题",
            "each linked version must actually load the fresh NFO"
        );
    }
    server.abort();
    f.close().await;
}

async fn verify_failed_first_admission_reprocess(kind: &str) {
    let (f, proxy, server) = reprocess_fixture(&["101"], false).await;
    if kind == "Movie" {
        let mut items = f.host.items.lock().await;
        items.get_mut("101").unwrap()["Type"] = json!("Movie");
        items.get_mut("101").unwrap()["ProviderIds"] = json!({"Tmdb":"500"});
    }
    sqlx::query("DELETE FROM media_metadata WHERE emby_item_ids_json @> '[\"101\"]'::jsonb")
        .execute(&f.state.db)
        .await
        .unwrap();
    f.state
        .save_config(
            &json!({"monitor_sha1_pc_search":false,"auto_repair_truehd_core_on_ingest":false}),
        )
        .await
        .unwrap();
    let row = f.run_scope("101", "reprocess").await;
    assert_eq!(
        row["status"], "completed",
        "first-admission retry {kind}: {row}"
    );
    assert_eq!(
        *proxy.imports.lock().await,
        vec!["101"],
        "the verified media must only be imported once"
    );
    let bound: Value = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> '[\"101\"]'::jsonb AND item_type=$1")
        .bind(kind).fetch_one(&f.state.db).await.unwrap();
    assert_eq!(
        bound["asset_details_json"][0]["moviepilot_media_pending"], false,
        "{bound}"
    );
    assert_eq!(
        bound["asset_details_json"][0]["moviepilot_media_identity"]["id"], "101",
        "{bound}"
    );
    assert_eq!(
        bound["asset_details_json"][0]["moviepilot_ingest_identity"]["id"], "101",
        "{bound}"
    );
    assert_eq!(
        row["result"]["finalization"]["metadata_refresh"]["completed"], true,
        "{row}"
    );
    server.abort();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; first Movie admission retry probes actual media and imports only localhost"]
async fn failed_first_movie_admission_reprocess_creates_a_verified_local_binding() {
    verify_failed_first_admission_reprocess("Movie").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; first Episode admission retry probes actual media and imports only localhost"]
async fn failed_first_episode_admission_reprocess_creates_a_verified_local_binding() {
    verify_failed_first_admission_reprocess("Episode").await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real ffprobe and native imports are localhost"]
async fn explicit_season_and_series_scrape_keep_the_two_episode_scope() {
    for target in ["302", "301"] {
        let (f, proxy, server) = reprocess_fixture(&["101", "102"], false).await;
        // Explicit Series/Season reprocess continues to use its existing cloud
        // rebuild contract. Exercise the same public scope resolver and scrape
        // stage here rather than fabricating a completed 115 extraction.
        let work = f.state.clone();
        let parent = f.host.items.lock().await["301"].clone();
        let scope = target.to_string();
        let task = f
            .state
            .jobs
            .submit("显式季剧刮削范围回归", move |context| async move {
                let mut ids = emby_toolkit::revalidation::scope(&work, &scope, &context).await?;
                ids.sort();
                assert_eq!(ids, vec!["101", "102"], "explicit {scope}");
                Box::pin(emby_toolkit::ingest::scrape(
                    &work, &parent, &ids, None, &context,
                ))
                .await
            })
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let result = f.state.jobs.result(&task).await.unwrap();
                if result["is_running"] == false {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(result["state"], "completed", "target {target}: {result}");
        assert!(proxy.imports.lock().await.is_empty());
        for (id, title) in [("101", "新分集标题"), ("102", "不应修改的第二集")] {
            let item = f.host.items.lock().await[id].clone();
            let nfo = PathBuf::from(item["Path"].as_str().unwrap()).with_extension("nfo");
            assert!(
                std::fs::read_to_string(nfo).unwrap().contains(title),
                "target {target}, episode {id}"
            );
            assert_eq!(
                item["MediaStreams"][0]["Codec"], "hevc",
                "metadata-only explicit {target}, episode {id}"
            );
        }
        let row: Value = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='50012' AND item_type='Episode'")
            .fetch_one(&f.state.db).await.unwrap();
        assert_eq!(row["title"], "不应修改的第二集", "target {target}");
        assert_eq!(
            f.host.configuration.lock().await["PersistMediaInfoMode"],
            "Default"
        );
        server.abort();
        f.close().await;
    }
}
