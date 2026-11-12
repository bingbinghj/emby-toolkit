//! Real ffmpeg/Chromaprint extraction against local HTTP audio and a disposable
//! database. No request reaches a cloud drive or a real Emby instance.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, credits, db, p115};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    path::Path as FsPath,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify};
use tower_http::services::ServeDir;

const RUNTIME: u64 = 1_200_000_000;

#[derive(Clone, Default)]
struct Host {
    items: Arc<Mutex<HashMap<String, Value>>>,
    downloads: Arc<Mutex<Vec<Instant>>>,
    writes: Arc<Mutex<Vec<String>>>,
    deny_readback: bool,
    base: String,
    pause_plugins: Arc<AtomicBool>,
    plugins_seen: Arc<Notify>,
    plugins_release: Arc<Notify>,
    failed_season: Arc<Mutex<Option<String>>>,
    item_queries: Arc<Mutex<Vec<HashMap<String, String>>>>,
    items_seen: Arc<Notify>,
    page_limit: Arc<Mutex<Option<usize>>>,
}

async fn details(State(host): State<Host>, Path(id): Path<String>) -> Response {
    match host.items.lock().await.get(&id) {
        Some(item) => Json(item.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn items(State(host): State<Host>, Query(query): Query<HashMap<String, String>>) -> Response {
    host.items_seen.notify_one();
    host.item_queries.lock().await.push(query.clone());
    let failed_season = host.failed_season.lock().await.clone();
    if failed_season.is_some() && query.get("ParentId") == failed_season.as_ref() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut items = host
        .items
        .lock()
        .await
        .values()
        .filter(|item| {
            query
                .get("IncludeItemTypes")
                .is_none_or(|types| types.split(',').any(|kind| item["Type"] == kind))
                && query
                    .get("ParentId")
                    .is_none_or(|parent| item["SeasonId"] == *parent || item["SeriesId"] == *parent)
        })
        .cloned()
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item["IndexNumber"].as_u64().unwrap_or(0));
    let total = items.len();
    let start = query
        .get("StartIndex")
        .and_then(|index| index.parse().ok())
        .unwrap_or(0);
    let limit = query
        .get("Limit")
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(total);
    let limit = host.page_limit.lock().await.unwrap_or(limit).min(limit);
    let page = items
        .into_iter()
        .skip(start)
        .take(limit)
        .collect::<Vec<_>>();
    Json(json!({"TotalRecordCount":total,"Items":page})).into_response()
}

async fn plugins(State(host): State<Host>) -> Json<Value> {
    if host.pause_plugins.swap(false, Ordering::SeqCst) {
        host.plugins_seen.notify_one();
        host.plugins_release.notified().await;
    }
    Json(json!([{"Name":"Chapter API","Version":"1.4.0.0"}]))
}

async fn download(State(host): State<Host>) -> Json<Value> {
    let mut calls = host.downloads.lock().await;
    let episode = if calls.is_empty() { "101" } else { "102" };
    calls.push(Instant::now());
    Json(json!({"state":true,"data":{"url":format!("{}/media/{episode}.mkv",host.base)}}))
}

fn ticks(time: &str) -> u64 {
    let components = time.split(':').collect::<Vec<_>>();
    assert_eq!(components.len(), 3);
    let seconds =
        components[0].parse::<u64>().unwrap() * 3600 + components[1].parse::<u64>().unwrap() * 60;
    let (seconds_last, fraction) = components[2].split_once('.').unwrap();
    (seconds + seconds_last.parse::<u64>().unwrap()) * 10_000_000 + fraction.parse::<u64>().unwrap()
}

async fn add_chapter(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(query.get("action").map(String::as_str), Some("add"));
    assert_eq!(query.get("type").map(String::as_str), Some("credits_start"));
    let id = query.get("id").unwrap();
    host.writes.lock().await.push(id.clone());
    if !host.deny_readback {
        host.items.lock().await.get_mut(id).unwrap()["Chapters"]
            .as_array_mut()
            .unwrap()
            .push(json!({"MarkerType":"CreditsStart","Name":"片尾","StartPositionTicks":ticks(query.get("time").unwrap())}));
    }
    // A successful HTTP status alone must never count as a successful write.
    Json(json!(["Chapter Added"]))
}

// Each episode has unrelated opening music, followed by exactly the same
// changing melody. A stationary sine wave would prove only repeated tones.
fn pcm_episode(variant: u64) -> Vec<u8> {
    let rate = 16_000_u32;
    let frames = rate * 120;
    let mut pcm = Vec::with_capacity((frames * 2 + 44) as usize);
    pcm.extend_from_slice(b"RIFF");
    pcm.extend_from_slice(&(frames * 2 + 36).to_le_bytes());
    pcm.extend_from_slice(b"WAVEfmt ");
    pcm.extend_from_slice(&16_u32.to_le_bytes());
    pcm.extend_from_slice(&1_u16.to_le_bytes());
    pcm.extend_from_slice(&1_u16.to_le_bytes());
    pcm.extend_from_slice(&rate.to_le_bytes());
    pcm.extend_from_slice(&(rate * 2).to_le_bytes());
    pcm.extend_from_slice(&2_u16.to_le_bytes());
    pcm.extend_from_slice(&16_u16.to_le_bytes());
    pcm.extend_from_slice(b"data");
    pcm.extend_from_slice(&(frames * 2).to_le_bytes());
    let notes = [220.0, 246.94, 277.18, 293.66, 329.63, 369.99, 415.30, 440.0];
    for frame in 0..frames {
        let time = frame as f64 / rate as f64;
        let (phase, seed) = if time >= 75.0 {
            (time - 75.0, 17_u64)
        } else {
            (time, variant * 37)
        };
        let beat = (phase * 3.0).floor() as u64;
        let note = notes[((beat * 5 + seed + beat / 3) % 8) as usize];
        let envelope = 0.5 + 0.5 * (-((phase * 3.0).fract()) * 4.0).exp();
        let angle = 2.0 * std::f64::consts::PI * note * phase;
        let music = envelope
            * (0.43 * angle.sin()
                + 0.22 * (angle * 1.5).sin()
                + 0.11 * (angle * 2.0).sin()
                + 0.06 * (angle * 3.0).sin());
        pcm.extend_from_slice(&((music * 25_000.0) as i16).to_le_bytes());
    }
    pcm
}

fn media_file(root: &FsPath, id: &str, variant: u64) -> Vec<u8> {
    let wav = root.join(format!("{id}.wav"));
    let subtitles = root.join(format!("{id}.srt"));
    let media = root.join(format!("{id}.mkv"));
    std::fs::write(&wav, pcm_episode(variant)).unwrap();
    std::fs::write(&subtitles, "1\n00:00:02,000 --> 00:00:05,000\n原始中文字幕\n\n2\n00:01:50,000 --> 00:01:58,000\n片尾字幕\n").unwrap();
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=32x32:r=1:d=120",
            "-i",
        ])
        .arg(&wav)
        .arg("-i")
        .arg(&subtitles)
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:s",
            "-c:v",
            "mpeg4",
            "-q:v",
            "31",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "srt",
            "-t",
            "120",
            "-y",
        ])
        .arg(&media)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(media).unwrap()
}

struct Fixture {
    state: AppState,
    host: Host,
    admin: sqlx::PgPool,
    schema: String,
    sha: HashMap<String, String>,
    before: HashMap<String, Value>,
    _root: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(deny_readback: bool) -> Self {
        let database = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&database).await.unwrap();
        let schema = format!("credits_{}", uuid::Uuid::new_v4().simple());
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
            .connect(&database)
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let assets = root.path().join("media");
        std::fs::create_dir(&assets).unwrap();
        let mut state = AppState::new(pool, root.path().join("app")).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let host = Host {
            deny_readback,
            base: base.clone(),
            ..Host::default()
        };
        let mut sha = HashMap::new();
        let mut before = HashMap::new();
        for (episode, id) in [(1, "101"), (2, "102")] {
            let bytes = media_file(&assets, id, episode);
            let digest = format!("{:X}", Sha1::digest(&bytes));
            sha.insert(id.into(), digest.clone());
            let pc = format!("pc{id}");
            let strm = root.path().join(format!("Series.S01E{episode:02}.strm"));
            let original = format!("/cd2/{id}.mkv");
            std::fs::write(&strm, &original).unwrap();
            let streams = json!([
                {"Type":"Video","Index":0,"Codec":"mpeg4","Width":32,"Height":32},
                {"Type":"Audio","Index":1,"Codec":"pcm_s16le","Language":"jpn","IsDefault":true,"Title":"原始音轨"},
                {"Type":"Subtitle","Index":2,"Codec":"subrip","Language":"chi","Title":"原始字幕"}
            ]);
            let chapters = json!([
                {"Name":"第一章","MarkerType":"Chapter","StartPositionTicks":0,"ImageTag":"original-chapter-image","ChapterIndex":0},
                {"Name":"片头","MarkerType":"IntroStart","StartPositionTicks":10_000_000,"ChapterIndex":1},
                {"Name":"片头结束","MarkerType":"IntroEnd","StartPositionTicks":200_000_000,"ChapterIndex":2}
            ]);
            // Item.Size is the STRM shell; MediaSources.Size is the actual MKV.
            let item = json!({
                "Id":id,"Type":"Episode","Name":format!("分集{episode}"),
                "SeriesName":"测试电视剧","SeriesId":"series","SeasonId":"season","ParentId":"season",
                "ParentIndexNumber":1,"IndexNumber":episode,"Path":strm,
                "Size":50,"RunTimeTicks":RUNTIME,"Chapters":chapters,"MediaStreams":streams,
                "MediaSources":[{"Id":format!("mediasource_{id}"),"ItemId":id,"Path":original,"Size":bytes.len(),"Container":"mkv","RunTimeTicks":RUNTIME,"MediaStreams":streams}]
            });
            let info = json!([{"MediaSourceInfo":{"Path":original,"Size":bytes.len(),"Container":"mkv","RunTimeTicks":RUNTIME,"MediaStreams":streams},"Chapters":chapters}]);
            std::fs::write(
                strm.with_file_name(format!("Series.S01E{episode:02}-mediainfo.json")),
                serde_json::to_vec(&info).unwrap(),
            )
            .unwrap();
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Episode',$2,'series',1,$3,true,$4,$5,$6,$7)")
                .bind(id).bind(format!("分集{episode}")).bind(episode as i32)
                .bind(json!([id])).bind(json!([{"emby_item_id":id,"path":strm,"size_bytes":bytes.len()}]))
                .bind(json!([digest])).bind(json!([pc])).execute(&state.db).await.unwrap();
            sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
                .bind(&digest)
                .bind(info)
                .execute(&state.db)
                .await
                .unwrap();
            host.items.lock().await.insert(id.into(), item.clone());
            before.insert(id.into(), item);
        }
        host.items.lock().await.insert(
            "201".into(),
            json!({"Id":"201","Type":"Movie","Name":"不提取的电影","Path":root.path().join("Movie.strm")}),
        );
        let router = Router::new()
            .route("/Users/user/Items/{id}", get(details))
            .route("/Items", get(items))
            .route("/Plugins", get(plugins))
            .route("/chapter_api/update_chapters", get(add_chapter))
            .route("/app/chrome/downurl", post(download))
            .route("/web/ConfigurationPages", get(|| async { Json(json!([{"Name":"MediaInfoExtractPageView","Href":"configurationpage?PageId=fixture:MediaInfoExtractPageView"}])) }))
            .route("/UI/View", get(|| async { Json(json!({"EditObjectContainer":{"Object":{"GeneralOptions":{"MaxConcurrentCount":1,"Tier2MaxConcurrentCount":2,"CooldownDurationSeconds":1}}}})) }))
            .nest_service("/media", ServeDir::new(&assets))
            .with_state(host.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            openapi: base.clone(),
            ..p115::Endpoints::default()
        }));
        state
            .save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":root.path(),"libraries_to_ignore":[],"libraries_to_process":[],"ai_api_key":"","telegram_bot_token":""}))
            .await
            .unwrap();
        db::save_setting(&state.db, "p115_auth_tokens", &json!({"cookie":"fixture"}))
            .await
            .unwrap();
        Self {
            state,
            host,
            admin,
            schema,
            sha,
            before,
            _root: root,
            server,
        }
    }

    async fn run(&self, ids: Option<Vec<String>>) -> Value {
        let work = self.state.clone();
        let task = self
            .state
            .jobs
            .submit("真实片尾提取验收", move |context| async move {
                match ids {
                    Some(ids) => credits::extract_ids(&work, &ids, &context).await,
                    None => credits::extract_all(&work, &context).await,
                }
            })
            .await
            .unwrap();
        self.wait(&task).await
    }

    async fn wait(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let job = self.state.jobs.result(task).await.unwrap();
                if job["is_running"] == false {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    async fn cache(&self, id: &str) -> Value {
        sqlx::query_scalar("SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1")
            .bind(&self.sha[id])
            .fetch_one(&self.state.db)
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

fn start(chapters: &Value) -> Option<u64> {
    chapters
        .as_array()
        .unwrap()
        .iter()
        .find(|chapter| chapter["MarkerType"] == "CreditsStart")
        .and_then(|chapter| chapter["StartPositionTicks"].as_u64())
}

async fn begin_endpoint(fixture: &Fixture, id: &str) -> String {
    use tower::ServiceExt;
    let response = credits::router()
        .with_state(fixture.state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/api/media/{id}/extract_credits"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    body["task_id"].as_str().unwrap().to_string()
}

async fn submit_endpoint(fixture: &Fixture, id: &str) -> Value {
    fixture.wait(&begin_endpoint(fixture, id).await).await
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all HTTP stays local"]
async fn series_and_season_endpoints_page_only_the_selected_scope() {
    for (id, kind, expected, action) in [
        ("series", "Series", vec!["101", "102"], "提取当前剧片尾"),
        ("season", "Season", vec!["101"], "提取当前季片尾"),
    ] {
        let fixture = Fixture::new(false).await;
        *fixture.host.page_limit.lock().await = Some(1);
        {
            let mut items = fixture.host.items.lock().await;
            items.insert(id.into(), json!({"Id":id,"Type":kind}));
            items.get_mut("102").unwrap()["SeasonId"] = json!("season2");
            items.get_mut("102").unwrap()["ParentId"] = json!("season2");
            items.get_mut("102").unwrap()["ParentIndexNumber"] = json!(2);
            for episode in ["101", "102"] {
                items.get_mut(episode).unwrap()["Chapters"].as_array_mut().unwrap()
                    .push(json!({"Name":"片尾","MarkerType":"CreditsStart","StartPositionTicks":750_000_000}));
            }
            items.insert("301".into(), json!({"Id":"301","Type":"Episode","SeriesId":"unrelated","SeasonId":"unrelated-season"}));
        }
        let job = submit_endpoint(&fixture, id).await;
        assert_eq!(job["last_action"], action, "{job}");
        assert_eq!(job["result"]["failed"], 0, "{job}");
        let selected = job["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["item_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(selected, expected, "{job}");
        assert_eq!(
            start(&fixture.cache("101").await[0]["Chapters"]),
            Some(750_000_000)
        );
        assert_eq!(
            start(&fixture.cache("102").await[0]["Chapters"]),
            (kind == "Series").then_some(750_000_000)
        );
        let queries = fixture.host.item_queries.lock().await;
        assert!(
            queries
                .iter()
                .all(|query| query["IncludeItemTypes"] == "Episode")
        );
        if kind == "Series" {
            assert!(
                queries
                    .iter()
                    .any(|query| query["ParentId"] == id && query["StartIndex"] == "1")
            );
        }
        drop(queries);
        assert!(fixture.host.writes.lock().await.is_empty());
        assert!(fixture.host.downloads.lock().await.is_empty());
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all HTTP stays local"]
async fn selected_series_list_failure_is_terminal_before_extraction() {
    let fixture = Fixture::new(false).await;
    fixture
        .host
        .items
        .lock()
        .await
        .insert("series".into(), json!({"Id":"series","Type":"Series"}));
    *fixture.host.failed_season.lock().await = Some("series".into());
    let job = submit_endpoint(&fixture, "series").await;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(fixture.host.writes.lock().await.is_empty());
    assert!(fixture.host.downloads.lock().await.is_empty());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all HTTP stays local"]
async fn stopping_series_scope_read_never_extracts_or_updates_episode_cache() {
    let fixture = Fixture::new(false).await;
    fixture
        .host
        .items
        .lock()
        .await
        .insert("series".into(), json!({"Id":"series","Type":"Series"}));
    let reads = fixture.host.item_queries.lock().await;
    let task = begin_endpoint(&fixture, "series").await;
    tokio::time::timeout(Duration::from_secs(10), fixture.host.items_seen.notified())
        .await
        .unwrap();
    fixture
        .state
        .jobs
        .control(Some(&task), "stop")
        .await
        .unwrap();
    drop(reads);
    let job = fixture.wait(&task).await;
    assert_eq!(job["state"], "cancelled", "{job}");
    assert!(fixture.host.writes.lock().await.is_empty());
    assert!(fixture.host.downloads.lock().await.is_empty());
    for id in ["101", "102"] {
        assert_eq!(start(&fixture.cache(id).await[0]["Chapters"]), None);
    }
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all HTTP stays local"]
async fn single_episode_endpoint_rejects_movies_and_identifies_its_own_task_scope() {
    use tower::ServiceExt;
    let fixture = Fixture::new(false).await;
    fixture.host.items.lock().await.get_mut("101").unwrap()["Type"] = json!("Movie");
    let request = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/media/101/extract_credits")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let response = credits::router()
        .with_state(fixture.state.clone())
        .oneshot(request())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.state.jobs.status().await["is_running"], false);

    {
        let mut items = fixture.host.items.lock().await;
        items.get_mut("101").unwrap()["Type"] = json!("Episode");
        items.get_mut("101").unwrap()["Chapters"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "Name":"片尾", "MarkerType":"CreditsStart", "StartPositionTicks":750_000_000
            }));
    }
    let response = credits::router()
        .with_state(fixture.state.clone())
        .oneshot(request())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let id = body["task_id"].as_str().unwrap();
    let job = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let job = fixture.state.jobs.result(id).await.unwrap();
            if job["is_running"] == false {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job["last_action"], "提取当前集片尾");
    assert_eq!(job["result"]["skipped"], 1, "{job}");
    assert_eq!(job["result"]["credits_start_ticks"], 750_000_000);
    assert!(fixture.host.writes.lock().await.is_empty());
    assert!(fixture.host.downloads.lock().await.is_empty());
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn real_tail_fingerprint_updates_only_current_episode_and_reuses_verified_result() {
    let fixture = Fixture::new(false).await;
    let job = fixture.run(Some(vec!["101".into()])).await;
    assert_eq!(job["result"]["updated"], 1, "{job}");
    assert_eq!(job["result"]["failed"], 0, "{job}");
    let marker = job["result"]["credits_start_ticks"].as_u64().unwrap();
    assert!(
        (68.0..=82.0).contains(&(marker as f64 / 10_000_000.0)),
        "unexpected credits start {marker}"
    );
    assert_eq!(*fixture.host.writes.lock().await, vec!["101"]);
    let downloads = fixture.host.downloads.lock().await.clone();
    assert_eq!(
        downloads.len(),
        2,
        "two different real media files must be sampled"
    );
    assert!(
        downloads[1].duration_since(downloads[0]) >= Duration::from_secs(1),
        "main-thread cooldown was bypassed"
    );
    let items = fixture.host.items.lock().await.clone();
    assert_eq!(
        items["101"]["MediaSources"],
        fixture.before["101"]["MediaSources"]
    );
    assert_eq!(
        items["101"]["MediaStreams"],
        fixture.before["101"]["MediaStreams"]
    );
    assert_eq!(
        items["101"]["Chapters"][0],
        fixture.before["101"]["Chapters"][0]
    );
    assert_eq!(items["102"], fixture.before["102"]);
    assert_eq!(
        start(&fixture.cache("101").await[0]["Chapters"]),
        Some(marker)
    );
    assert_eq!(start(&fixture.cache("102").await[0]["Chapters"]), None);
    let repeated = fixture.run(Some(vec!["101".into()])).await;
    assert_eq!(repeated["result"]["updated"], 0, "{repeated}");
    assert_eq!(repeated["result"]["skipped"], 1, "{repeated}");
    assert_eq!(fixture.host.downloads.lock().await.len(), downloads.len());
    assert_eq!(fixture.host.writes.lock().await.len(), 1);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn full_library_credits_task_extracts_episodes_and_never_movies() {
    let fixture = Fixture::new(false).await;
    let job = fixture.run(None).await;
    assert_eq!(job["result"]["updated"], 2, "{job}");
    assert_eq!(job["result"]["failed"], 0, "{job}");
    let writes = fixture.host.writes.lock().await.clone();
    assert_eq!(writes, vec!["101", "102"]);
    assert!(!writes.contains(&"201".into()));
    assert_eq!(fixture.host.downloads.lock().await.len(), 2);
    let movie = fixture.run(Some(vec!["201".into()])).await;
    assert_eq!(movie["state"], "failed", "{movie}");
    assert_eq!(fixture.host.writes.lock().await.len(), 2);
    assert_eq!(fixture.host.downloads.lock().await.len(), 2);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn successful_http_without_credits_readback_is_failed_and_never_caches_a_marker() {
    let fixture = Fixture::new(true).await;
    let job = fixture.run(Some(vec!["101".into()])).await;
    assert_eq!(job["result"]["updated"], 0, "{job}");
    assert_eq!(job["result"]["failed"], 1, "{job}");
    assert!(
        job["result"]["failures"][0]
            .as_str()
            .unwrap()
            .contains("Emby 未回读到本次片尾标记"),
        "{job}"
    );
    assert_eq!(start(&fixture.cache("101").await[0]["Chapters"]), None);
    assert_eq!(
        fixture.host.items.lock().await["101"],
        fixture.before["101"]
    );
    assert_eq!(
        *fixture.host.writes.lock().await,
        vec!["101"],
        "legacy GET mutation must not be retried"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn changed_media_during_main_slot_wait_is_rejected_before_any_chapter_write() {
    let fixture = Fixture::new(false).await;
    fixture.host.pause_plugins.store(true, Ordering::SeqCst);
    let changed_path = fixture._root.path().join("changed-after-fingerprint.strm");
    let run = fixture.run(Some(vec!["101".into()]));
    let steer = async {
        tokio::time::timeout(
            Duration::from_secs(60),
            fixture.host.plugins_seen.notified(),
        )
        .await
        .unwrap();
        // The writer is paused before it obtains its main permit. Reserve that
        // permit, release its plugin response, then change the native identity
        // while it must still wait for the shared main thread.
        let permit = emby_toolkit::strm::slot(&fixture.state, "main", None)
            .await
            .unwrap();
        fixture.host.plugins_release.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(fixture.host.writes.lock().await.is_empty());
        fixture.host.items.lock().await.get_mut("101").unwrap()["Path"] = json!(changed_path);
        drop(permit);
    };
    let (job, ()) = tokio::join!(run, steer);
    assert_eq!(job["result"]["updated"], 0, "{job}");
    assert_eq!(job["result"]["failed"], 1, "{job}");
    assert!(
        job["result"]["failures"][0]
            .as_str()
            .unwrap()
            .contains("身份"),
        "{job}"
    );
    assert!(
        fixture.host.writes.lock().await.is_empty(),
        "a stale identity must be rejected before the mutating GET"
    );
    assert_eq!(
        fixture.host.items.lock().await["101"]["Path"],
        json!(changed_path)
    );
    assert_eq!(start(&fixture.cache("101").await[0]["Chapters"]), None);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn later_season_read_failure_keeps_previously_written_episode_results() {
    let fixture = Fixture::new(false).await;
    let mut later = fixture.before["101"].clone();
    later["Id"] = json!("301");
    later["Name"] = json!("后续季分集");
    later["SeasonId"] = json!("zzz-season");
    later["ParentId"] = json!("zzz-season");
    later["ParentIndexNumber"] = json!(2);
    // Full-library discovery succeeds, then this second season's own paginated
    // sibling query fails after the first season was already written to Emby.
    fixture
        .host
        .items
        .lock()
        .await
        .insert("301".into(), later.clone());
    *fixture.host.failed_season.lock().await = Some("zzz-season".into());
    let job = fixture.run(None).await;
    assert_eq!(job["result"]["updated"], 2, "{job}");
    assert_eq!(job["result"]["failed"], 1, "{job}");
    let items = job["result"]["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["item_id"] == "101" && item["status"] == "completed"),
        "{job}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["item_id"] == "102" && item["status"] == "completed"),
        "{job}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["item_id"] == "301" && item["status"] == "failed"),
        "{job}"
    );
    assert!(
        job["result"]["failures"][0]
            .as_str()
            .unwrap()
            .contains("301"),
        "{job}"
    );
    assert_eq!(*fixture.host.writes.lock().await, vec!["101", "102"]);
    assert!(start(&fixture.cache("101").await[0]["Chapters"]).is_some());
    assert!(start(&fixture.cache("102").await[0]["Chapters"]).is_some());
    assert_eq!(
        fixture.host.items.lock().await["301"],
        later,
        "failed season must remain untouched"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn lowercase_sha_cache_is_updated_in_place_without_an_uppercase_duplicate() {
    let mut fixture = Fixture::new(false).await;
    let uppercase = fixture.sha["101"].clone();
    let lowercase = uppercase.to_ascii_lowercase();
    sqlx::query("UPDATE p115_mediainfo_cache SET sha1=$1,mediainfo_json=jsonb_set(mediainfo_json,'{0,OriginalCacheField}','\"lower-key-metadata\"'::jsonb) WHERE sha1=$2")
        .bind(&lowercase).bind(&uppercase).execute(&fixture.state.db).await.unwrap();
    sqlx::query(
        "UPDATE media_metadata SET file_sha1_json=$1 WHERE emby_item_ids_json @> $2::jsonb",
    )
    .bind(json!([lowercase]))
    .bind(json!(["101"]))
    .execute(&fixture.state.db)
    .await
    .unwrap();
    fixture.sha.insert("101".into(), lowercase.clone());
    let job = fixture.run(Some(vec!["101".into()])).await;
    assert_eq!(job["result"]["updated"], 1, "{job}");
    assert_eq!(job["result"]["failed"], 0, "{job}");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM p115_mediainfo_cache WHERE UPPER(sha1)=UPPER($1)")
            .bind(&uppercase)
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "writing a marker must not duplicate an old lowercase SHA key"
    );
    let upper_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM p115_mediainfo_cache WHERE sha1=$1)")
            .bind(&uppercase)
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    assert!(
        !upper_exists,
        "the original lowercase key must remain authoritative"
    );
    let cached = fixture.cache("101").await;
    let marker = job["result"]["credits_start_ticks"].as_u64().unwrap();
    assert_eq!(start(&cached[0]["Chapters"]), Some(marker));
    assert_eq!(cached[0]["OriginalCacheField"], "lower-key-metadata");
    // Force the real editor loader down its exact-SHA cache path instead of its
    // local sidecar path. The legacy lowercase asset binding must still work.
    std::fs::remove_file(fixture._root.path().join("Series.S01E01-mediainfo.json")).unwrap();
    let target = emby_toolkit::media_edit::target(&fixture.state, "101")
        .await
        .unwrap();
    assert_eq!(target.sha.as_deref(), Some(lowercase.as_str()));
    let loaded = emby_toolkit::media_edit::load(&fixture.state, &target)
        .await
        .unwrap();
    assert_eq!(start(&loaded[0]["Chapters"]), Some(marker));
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn stale_json_runtime_is_rejected_before_115_audio_download_or_marker_write() {
    let fixture = Fixture::new(false).await;
    let mut stale = fixture.cache("101").await;
    stale[0]["MediaSourceInfo"]["RunTimeTicks"] = json!(13_000_000_000_u64);
    std::fs::write(
        fixture._root.path().join("Series.S01E01-mediainfo.json"),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();
    sqlx::query("UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2")
        .bind(&stale)
        .bind(&fixture.sha["101"])
        .execute(&fixture.state.db)
        .await
        .unwrap();
    // The cached 1,300-second duration describes a different file. The real
    // HTTP media and both native runtime fields still describe this 120s item.
    let job = fixture.run(Some(vec!["101".into()])).await;
    assert_eq!(job["result"]["updated"], 0, "{job}");
    assert_eq!(job["result"]["failed"], 1, "{job}");
    assert!(
        job["result"]["failures"][0]
            .as_str()
            .unwrap()
            .contains("时长"),
        "{job}"
    );
    assert!(
        fixture.host.downloads.lock().await.is_empty(),
        "a stale duration must not produce an unsafe 115 tail seek"
    );
    assert!(
        fixture.host.writes.lock().await.is_empty(),
        "a stale duration must not write a native credits marker"
    );
    assert_eq!(
        fixture.host.items.lock().await["101"],
        fixture.before["101"]
    );
    assert_eq!(
        fixture.cache("101").await,
        stale,
        "failed duration validation must not rewrite the existing cache"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and ffmpeg with chromaprint; all HTTP stays local"]
async fn native_source_size_mismatch_is_rejected_before_audio_download_or_marker_write() {
    let fixture = Fixture::new(false).await;
    {
        let mut items = fixture.host.items.lock().await;
        let size = items["101"]["MediaSources"][0]["Size"].as_u64().unwrap();
        items.get_mut("101").unwrap()["MediaSources"][0]["Size"] = json!(size + 1);
    }
    let job = fixture.run(Some(vec!["101".into()])).await;
    assert_eq!(job["result"]["updated"], 0, "{job}");
    assert_eq!(job["result"]["failed"], 1, "{job}");
    assert!(
        job["result"]["failures"][0]
            .as_str()
            .unwrap()
            .contains("大小"),
        "{job}"
    );
    assert!(fixture.host.downloads.lock().await.is_empty());
    assert!(fixture.host.writes.lock().await.is_empty());
    assert_eq!(start(&fixture.cache("101").await[0]["Chapters"]), None);
    fixture.close().await;
}
