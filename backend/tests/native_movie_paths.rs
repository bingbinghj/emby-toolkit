use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::StatusCode,
    routing::get,
};
use emby_toolkit::{AppState, metadata, metadata_records, titles};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct NativeItems {
    items: Arc<Mutex<HashMap<String, Value>>>,
    reads: Arc<Mutex<Vec<String>>>,
    unexpected: Arc<Mutex<Vec<String>>>,
}

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    host: NativeItems,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("movie_paths_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
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
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        let host = NativeItems::default();
        let fake = Router::new()
            .route(
                "/Users/user/Items/{id}",
                get(
                    |State(host): State<NativeItems>, Path(id): Path<String>| async move {
                        host.reads.lock().await.push(id.clone());
                        match host.items.lock().await.get(&id) {
                            Some(item) => (StatusCode::OK, Json(item.clone())),
                            None => (StatusCode::NOT_FOUND, Json(json!({}))),
                        }
                    },
                ),
            )
            .fallback(
                |State(host): State<NativeItems>, request: Request| async move {
                    host.unexpected.lock().await.push(format!(
                        "{} {}",
                        request.method(),
                        request.uri().path()
                    ));
                    StatusCode::NOT_FOUND
                },
            )
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
        state.save_config(&json!({"local_strm_root":directory.path(),"cd2_media_root":"/cd2/115open/媒体库","monitor_sha1_pc_search":false,"emby_user_id":"user","emby_api_key":"fixture","emby_server_url":format!("http://{address}")})).await.unwrap();
        Self {
            state,
            root,
            schema,
            directory,
            host,
            server,
        }
    }

    async fn item(&self, id: &str, source_id: &str, explicit_id: bool, source: &str) -> Value {
        let path = self.directory.path().join(format!("{id}.strm"));
        tokio::fs::write(&path, source).await.unwrap();
        let streams = json!([
            {"Type":"Video","Codec":"hevc","Width":3840,"Height":2160},
            {"Type":"Audio","Codec":"aac","Language":"jpn","Title":"日语"}
        ]);
        let mut media_source = json!({"Id":source_id,"Path":source,"Size":1000,"Container":"mkv","MediaStreams":streams});
        if explicit_id {
            media_source["ItemId"] = json!(id);
        }
        let item = json!({"Id":id,"Type":"Movie","Name":"原生路径验证","Path":path,"Size":1000,"Container":"mkv","PartCount":1,"MediaStreams":streams,"MediaSources":[media_source]});
        let backup = json!([{"MediaSourceInfo":{"Path":source,"Size":1000,"Container":"mkv","RunTimeTicks":600000000_u64,"MediaStreams":streams},"Chapters":[]}]);
        tokio::fs::write(
            self.directory.path().join(format!("{id}-mediainfo.json")),
            serde_json::to_vec(&backup).unwrap(),
        )
        .await
        .unwrap();
        self.host.items.lock().await.insert(id.into(), item.clone());
        item
    }

    async fn bind(&self, item: &Value) -> emby_toolkit::error::Result<Vec<Value>> {
        metadata_records::online(
            &self.state,
            "Movie",
            &json!({"id":634429,"title":"原生路径验证"}),
            &[],
            item,
            &metadata_records::OnlineContext {
                parents: &HashMap::new(),
                library_guid: "",
                seasons: &[],
                episodes: &[],
                episode_ids: &[],
                skip_series_record: false,
            },
        )
        .await
    }

    async fn close(self) {
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn movie_local_playback_path_keeps_native_strm_and_titles_pass_readback() {
    let f = Fixture::new().await;
    let item = f
        .item(
            "61588",
            "mediasource_61588",
            false,
            "/cd2/115open/媒体库/外语电影/新·奥特曼.mkv",
        )
        .await;
    let mut records = f.bind(&item).await.unwrap();
    assert_eq!(records[0]["asset_details_json"][0]["path"], item["Path"]);
    metadata::upsert(&f.state, &mut records, false)
        .await
        .unwrap();
    let result = titles::fill(&f.state, &["61588".into()], false).await;
    assert_eq!(result["failed"], 0, "{result}");
    assert_eq!(result["verified_streams"], 1, "{result}");
    assert_eq!(result["pending"], json!([]), "{result}");
    assert!(
        f.host.unexpected.lock().await.is_empty(),
        "matching titles require no plugin requests"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn title_readback_accepts_empty_owner_but_never_another_legacy_version() {
    let f = Fixture::new().await;
    let mut item = f
        .item("61588", "mediasource_61588", false, "/cd2/61588.mkv")
        .await;
    let mut records = f.bind(&item).await.unwrap();
    metadata::upsert(&f.state, &mut records, false)
        .await
        .unwrap();
    item["MediaSources"][0]["ItemId"] = json!("");
    f.host
        .items
        .lock()
        .await
        .insert("61588".into(), item.clone());
    let accepted = titles::fill(&f.state, &["61588".into()], false).await;
    assert_eq!(accepted["verified_streams"], 1, "{accepted}");
    assert_eq!(accepted["failed"], 0, "{accepted}");
    item["MediaSources"][0]["MediaStreams"] = json!([]);
    f.host
        .items
        .lock()
        .await
        .insert("61588".into(), item.clone());
    let fallback = titles::fill(&f.state, &["61588".into()], false).await;
    assert_eq!(
        fallback["verified_streams"], 1,
        "current native top-level streams remain valid: {fallback}"
    );
    for path in [json!("/cd2/61588.mkv"), Value::Null] {
        item["MediaSources"][0]["Id"] = json!("mediasource_61589");
        item["MediaSources"][0]["Path"] = path;
        for owner in [Value::Null, json!("")] {
            item["MediaSources"][0]["ItemId"] = owner;
            f.host
                .items
                .lock()
                .await
                .insert("61588".into(), item.clone());
            let refused = titles::fill(&f.state, &["61588".into()], false).await;
            assert_eq!(
                refused["verified_streams"], 0,
                "other version cannot pass title readback: {refused}"
            );
            assert_eq!(refused["failed"], 1, "{refused}");
        }
    }
    assert!(
        f.host.unexpected.lock().await.is_empty(),
        "matching or unsafe native sources require no plugin requests"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn movie_versions_follow_item_id_and_resolve_each_native_path() {
    let f = Fixture::new().await;
    let mut first = f
        .item(
            "first",
            "opaque-first-source",
            true,
            "/cd2/115open/媒体库/First.mkv",
        )
        .await;
    let second = f
        .item(
            "second",
            "opaque-second-source",
            true,
            "https://stream.invalid/Second.mkv",
        )
        .await;
    first["MediaSources"] = json!([second["MediaSources"][0], first["MediaSources"][0]]);
    let records = f.bind(&first).await.unwrap();
    assert_eq!(records[0]["emby_item_ids_json"], json!(["second", "first"]));
    assert_eq!(records[0]["asset_details_json"][0]["path"], second["Path"]);
    assert_eq!(records[0]["asset_details_json"][1]["path"], first["Path"]);
    assert_eq!(*f.host.reads.lock().await, vec!["second"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn movie_versions_without_item_id_use_verified_legacy_ids() {
    let f = Fixture::new().await;
    let mut first = f
        .item(
            "first",
            "mediasource_first",
            false,
            "/cd2/115open/媒体库/First.mkv",
        )
        .await;
    let second = f
        .item(
            "second",
            "mediasource_second",
            false,
            "/cd2/115open/媒体库/Second.mkv",
        )
        .await;
    first["MediaSources"] = json!([first["MediaSources"][0], second["MediaSources"][0]]);
    let records = f.bind(&first).await.unwrap();
    assert_eq!(records[0]["emby_item_ids_json"], json!(["first", "second"]));
    assert_eq!(records[0]["asset_details_json"][1]["path"], second["Path"]);
    assert_eq!(*f.host.reads.lock().await, vec!["second"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn movie_unknown_or_mismatched_native_version_cannot_bind_playback_path() {
    let f = Fixture::new().await;
    let mut first = f
        .item(
            "first",
            "mediasource_first",
            false,
            "/cd2/115open/媒体库/First.mkv",
        )
        .await;
    first["MediaSources"][0]["ItemId"] = json!("missing");
    assert!(
        f.bind(&first).await.is_err(),
        "unknown native ID must not fall back to a guessed path"
    );
    f.host.items.lock().await.insert(
        "missing".into(),
        json!({"Id":"other","Type":"Movie","Path":"/media/other.strm"}),
    );
    assert!(
        f.bind(&first).await.is_err(),
        "mismatched native ID must not bind another file"
    );
    f.host
        .items
        .lock()
        .await
        .insert("missing".into(), json!({"Id":"missing","Type":"Movie"}));
    assert!(
        f.bind(&first).await.is_err(),
        "missing native Path must not fall back to playback URL"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn movie_missing_source_ids_only_allow_the_single_native_item() {
    let f = Fixture::new().await;
    let mut item = f
        .item("first", "", false, "/cd2/115open/媒体库/First.mkv")
        .await;
    let records = f.bind(&item).await.unwrap();
    assert_eq!(records[0]["emby_item_ids_json"], json!(["first"]));
    assert_eq!(records[0]["asset_details_json"][0]["path"], item["Path"]);
    item["MediaSources"] = json!([item["MediaSources"][0], item["MediaSources"][0]]);
    assert!(
        f.bind(&item).await.is_err(),
        "multiple sources without IDs cannot be assigned to native files"
    );
    item["MediaSources"] = json!([{"Id":"opaque-unknown-source","Path":"/cd2/unknown.mkv"}]);
    assert!(
        f.bind(&item).await.is_err(),
        "opaque source ID cannot be silently treated as the primary Item ID"
    );
    item["MediaSources"] = json!([]);
    item.as_object_mut().unwrap().remove("Path");
    assert!(
        f.bind(&item).await.is_err(),
        "native Item without Path cannot bind even when MediaSources is empty"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn movie_native_rebinding_replaces_legacy_path_and_preserves_pending_identity() {
    let f = Fixture::new().await;
    let item = f
        .item(
            "61588",
            "mediasource_61588",
            false,
            "/cd2/115open/媒体库/新·奥特曼.mkv",
        )
        .await;
    let mut old =
        metadata_records::pending("Movie", &json!({"id":634429,"title":"旧路径"}), &[], &item)
            .unwrap();
    old[0]["in_library"] = json!(true);
    old[0]["emby_item_ids_json"] = json!(["61588"]);
    old[0]["asset_details_json"] =
        json!([{"emby_item_id":"61588","path":item["MediaSources"][0]["Path"],"size_bytes":1000}]);
    old[0]["file_sha1_json"] = json!(["ORIGINAL-SHA"]);
    old[0]["file_pickcode_json"] = json!(["ORIGINAL-PC"]);
    metadata::upsert(&f.state, &mut old, false).await.unwrap();
    let mut records = f.bind(&item).await.unwrap();
    metadata::upsert(&f.state, &mut records, false)
        .await
        .unwrap();
    let row = metadata::row(&f.state, "634429", "Movie")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["asset_details_json"][0]["path"], item["Path"]);
    assert_eq!(row["asset_details_json"][0]["fingerprint_pending"], true);
    assert_eq!(row["file_sha1_json"], json!(["ORIGINAL-SHA"]));
    assert_eq!(row["file_pickcode_json"], json!(["ORIGINAL-PC"]));
    f.close().await;
}
