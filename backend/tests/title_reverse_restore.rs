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
    item: Arc<Mutex<Value>>,
    settings: Arc<Mutex<Value>>,
    starts: Arc<AtomicUsize>,
    clears: Arc<AtomicUsize>,
    wrong_final_language: Arc<AtomicBool>,
}

async fn details(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let mut item = host.item.lock().await.clone();
    let fields = query.get("Fields").map(String::as_str).unwrap_or("");
    if host.wrong_final_language.load(Ordering::SeqCst)
        && host.starts.load(Ordering::SeqCst) > 0
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
    let mut item = host.item.lock().await;
    item["MediaStreams"] = json!([]);
    item["MediaSources"][0]["MediaStreams"] = json!([]);
    item["Chapters"] = json!([]);
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
    host.starts.fetch_add(1, Ordering::SeqCst);
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
            item: Arc::new(Mutex::new(
                json!({"Id":"61616","Type":"Movie","Name":"迪迦奥特曼：最终圣战","Path":path,"Size":original[0]["MediaSourceInfo"]["Size"],"PartCount":1,"MediaStreams":streams,"MediaSources":[{"Id":"mediasource_61616","ItemId":"61616","Path":"/cd2/61616.mkv","MediaStreams":streams}],"Chapters":original[0]["Chapters"]}),
            )),
            settings: Arc::new(Mutex::new(original_settings.clone())),
            starts: Arc::default(),
            clears: Arc::default(),
            wrong_final_language: Arc::new(AtomicBool::new(wrong_final_language)),
        };
        let api = Router::new()
            .route("/Users/user/Items/61616", get(details))
            .route("/Items/61616/ClearMediaInfo", post(clear))
            .route("/ScheduledTasks", get(|State(host): State<Host>| async move {
                Json(json!([{"Id":"media","Key":"MediaInfoExtractTask","State":"Idle","LastExecutionResult":{"StartTimeUtc":host.starts.load(Ordering::SeqCst).to_string(),"Status":"Completed"}}]))
            }))
            .route("/ScheduledTasks/Running/media", post(native_start))
            .route("/Library/VirtualFolders", get(|State(host): State<Host>| async move {
                Json(json!([{"ItemId":"library","Locations":[host.directory]}]))
            }))
            .route("/System/Logs/embyserver.txt/Lines", get(|| async {Json(json!({"Items":[],"TotalRecordCount":0}))}))
            .route("/web/ConfigurationPages", get(||async {Json(json!([{"Name":"MediaInfoExtractPageView","Href":"configurationpage?PageId=plugin:MediaInfoExtractPageView"}]))}))
            .route("/UI/View", get(|State(host): State<Host>|async move {Json(json!({"EditObjectContainer":{"Object":host.settings.lock().await.clone()}}))}))
            .route("/UI/Command", post(|State(host): State<Host>,Json(body):Json<Value>|async move {*host.settings.lock().await=serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();StatusCode::NO_CONTENT}))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":directory.path(),"monitor_sha1_pc_search":false})).await.unwrap();
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
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let result = self.state.jobs.result(&task).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
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
    assert_eq!(f.host.clears.load(Ordering::SeqCst), 1);
    assert_eq!(f.host.starts.load(Ordering::SeqCst), 1);
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
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn local_language_corrections_are_not_forgotten_when_title_restoration_is_retried() {
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
    assert_eq!(f.host.starts.load(Ordering::SeqCst), 1, "{result}");
    assert_eq!(*f.host.settings.lock().await, f.original_settings);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn equivalent_language_aliases_do_not_trigger_unnecessary_restoration() {
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
        assert_eq!(f.host.starts.load(Ordering::SeqCst), 0, "{result}");
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
    f.close().await;
}
