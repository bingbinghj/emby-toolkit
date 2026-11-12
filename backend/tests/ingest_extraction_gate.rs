use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, db, webhook};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Host {
    items: Arc<Mutex<HashMap<String, Value>>>,
    title_failures: Arc<Mutex<Vec<String>>>,
    refresh_failures: Arc<Mutex<Vec<String>>>,
    refresh_attempts: Arc<Mutex<Vec<(String, String)>>>,
    native_starts: Arc<Mutex<Vec<String>>>,
    chain_enabled: Arc<AtomicBool>,
    stage_order: Arc<Mutex<Vec<String>>>,
    intro_runs: Arc<Mutex<u64>>,
    year_readbacks: Arc<AtomicUsize>,
    info_runs: Arc<Mutex<u64>>,
    logs: Arc<Mutex<Vec<String>>>,
    configuration: Arc<Mutex<Value>>,
}
async fn details(
    State(host): State<Host>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
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
    }) && host.title_failures.lock().await.contains(&id)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match host.items.lock().await.get(&id) {
        Some(item) => Json(item.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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
            query.get("Path").is_some_and(|path| item["Path"] == *path)
                || query
                    .get("Ids")
                    .is_some_and(|ids| ids.split(',').any(|id| item["Id"] == id))
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
    if host.chain_enabled.load(Ordering::SeqCst) && id == "301" {
        host.stage_order.lock().await.push(
            if mode == "FullRefresh" {
                "metadata-refresh"
            } else {
                "nfo-refresh"
            }
            .into(),
        );
    }
    host.logs.lock().await.extend([
        format!("Info fixture: RefreshItem Start: {id} metadata"),
        format!("Info fixture: RefreshItem Complete: {id} metadata"),
    ]);
    StatusCode::NO_CONTENT
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
        return StatusCode::NO_CONTENT;
    }
    host.native_starts.lock().await.push(id.clone());
    if host.chain_enabled.load(Ordering::SeqCst) && id == "intro" {
        *host.intro_runs.lock().await += 1;
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
            let playback = format!("/cd2/{id}.mkv");
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
            let item = json!({"Id":id,"Type":"Movie","Name":format!("电影{id}"),"Path":path,"PartCount":1,"Size":1000,"Chapters":[],"ProviderIds":{"Tmdb":id},"People":[],"Genres":[],"MediaStreams":streams,"MediaSources":[{"Id":format!("mediasource_{id}"),"ItemId":id,"Path":playback,"Size":1000,"Container":"mkv","MediaStreams":streams}]});
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
            .route("/Users/user/Items/{id}", get(details))
            .route("/Items", get(items))
            .route("/Items/{id}/Refresh", post(refresh))
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

    async fn run_series(&self, moviepilot: bool) -> Value {
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

struct TelegramCapture {
    messages: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
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
async fn both_ingest_entries_allow_disabled_titles_after_actual_refresh_and_block_unfinished_enabled_titles()
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
                assert_eq!(result["automatic_extraction"]["state"], "skipped", "{job}");
                assert_eq!(result["automatic_extraction"]["warnings"], 0, "{job}");
                assert!(starts.is_empty(), "标题未完成不得启动原生提取：{starts:?}");
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
async fn moviepilot_requires_every_eligible_group_and_unrelated_cover_warnings_do_not_block_extraction()
 {
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
        assert_eq!(result["automatic_extraction"]["state"], "skipped", "{job}");
        assert!(
            f.host.native_starts.lock().await.is_empty(),
            "一个成功组不得放行另一个未完成组"
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
