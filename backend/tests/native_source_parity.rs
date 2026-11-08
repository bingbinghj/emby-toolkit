use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use emby_toolkit::{AppState, assets, media_edit, revalidation, tasks};
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

fn version_item() -> Value {
    json!({"Id":"102","Type":"Episode","Path":"/media/episode-102.strm","MediaSources":[
        {"Id":"opaque-101","ItemId":"101","Path":"/cd2/episode-101.mkv","Size":100,"Container":"mkv","MediaStreams":[{"Type":"Video","Width":1920,"Height":1080,"Codec":"h264"},{"Type":"Subtitle","Language":"eng","IsExternal":true}]},
        {"Id":"opaque-102","ItemId":"102","Path":"/cd2/episode-102.mkv","Size":200,"Container":"mkv","MediaStreams":[{"Type":"Video","Width":3840,"Height":2160,"Codec":"hevc"},{"Type":"Subtitle","Language":"chi","IsExternal":true}]}
    ]})
}

#[test]
fn asset_uses_current_native_version_when_other_source_is_first() {
    let item = version_item();
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert_eq!(asset["emby_item_id"], "102");
    assert_eq!(asset["path"], "/media/episode-102.strm");
    assert_eq!(asset["size_bytes"], 200);
    assert_eq!(asset["video_codec"], "hevc");
    assert_eq!(asset["width"], 3840);
    assert_eq!(asset["subtitles"].as_array().unwrap().len(), 1);
    assert_eq!(asset["subtitles"][0]["language"], "chi");
}

#[test]
fn asset_does_not_borrow_another_owner_or_an_ambiguous_source() {
    let mut item = version_item();
    item["MediaSources"].as_array_mut().unwrap().pop();
    item["MediaSources"][0]["Path"] = item["Path"].clone();
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert!(asset["size_bytes"].is_null());
    assert!(asset["video_codec"].is_null());
    assert!(asset["subtitles"].as_array().unwrap().is_empty());
    item["MediaSources"] = json!([
        {"Id":"opaque-a","Path":"/media/episode-102.strm","Size":100},
        {"Id":"opaque-b","Path":"/media/episode-102.strm","Size":200}
    ]);
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert!(asset["size_bytes"].is_null());
    item["MediaSources"] = json!([{"Id":"mediasource_102","Path":"/cd2/episode.mkv","Size":200}]);
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert_eq!(asset["size_bytes"], 200);
    item["Id"] = json!("");
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert!(asset["size_bytes"].is_null());
    item["Id"] = Value::Null;
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert!(asset["size_bytes"].is_null());
    item["Id"] = json!("102");
    item["MediaSources"][0]["ItemId"] = json!("");
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert_eq!(asset["size_bytes"], 200);
    item["MediaSources"][0]["ItemId"] = json!("101");
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert!(asset["size_bytes"].is_null());
}

#[test]
fn asset_rejects_conflicting_legacy_owner_even_if_the_path_matches() {
    let mut item = version_item();
    item["MediaSources"] = json!([{"Id":"mediasource_101","Path":item["Path"],"Size":100}]);
    for owner in [Value::Null, json!("")] {
        item["MediaSources"][0]["ItemId"] = owner;
        let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
        assert!(asset["size_bytes"].is_null());
    }
    item["MediaSources"][0]["ItemId"] = json!("102");
    let asset = assets::parse_with_cache(&item, &HashMap::new(), "", &json!([]), &Value::Null);
    assert_eq!(
        asset["size_bytes"], 100,
        "explicit native owner takes precedence over source ID"
    );
}

#[derive(Clone, Default)]
struct Host {
    reject_existing: Arc<AtomicBool>,
    queries: Arc<Mutex<Vec<Vec<String>>>>,
    parts_mode: Arc<AtomicUsize>,
    parts_sources: Arc<Mutex<Vec<String>>>,
    wrong_version: Arc<AtomicBool>,
}

fn maintenance_item(id: &str) -> Value {
    let mut item =
        json!({"Id":id,"Type":"Movie","Path":format!("/media/movie-{id}.strm"),"PartCount":3});
    if id == "101" {
        item["MediaSources"] = json!([
            {"Id":"opaque-source-secondary","ItemId":"102","Path":"/cd2/movie-102.mkv"},
            {"Id":"mediasource_101","Path":"/cd2/movie-101.mkv"}
        ]);
    }
    item
}
async fn details(State(host): State<Host>, Path(id): Path<String>) -> Json<Value> {
    let mut item = maintenance_item(&id);
    if id == "102" && host.wrong_version.load(Ordering::SeqCst) {
        item["Id"] = json!("replacement");
    }
    Json(item)
}
async fn parts(State(host): State<Host>, Query(query): Query<HashMap<String, String>>) -> Response {
    host.parts_sources
        .lock()
        .await
        .push(query["mediaSourceId"].clone());
    let mode = host.parts_mode.load(Ordering::SeqCst);
    if mode == 1 {
        return StatusCode::FORBIDDEN.into_response();
    }
    if mode == 3 {
        return Json(json!({"Items":null})).into_response();
    }
    let mut items = vec![json!({"Id":"302","ParentId":"101","Path":"/media/movie.part2.strm"})];
    if mode != 2 {
        items.push(json!({"Id":"303","ParentId":"101","Path":"/media/movie.part3.strm"}));
    }
    if mode == 4 {
        items[1]["ParentId"] = json!("102");
    }
    if mode == 5 {
        items.push(items[0].clone());
    }
    Json(json!({"Items":items})).into_response()
}
async fn items(State(host): State<Host>, Query(query): Query<HashMap<String, String>>) -> Response {
    if let Some(ids) = query.get("Ids") {
        let ids = ids.split(',').map(str::to_owned).collect::<Vec<_>>();
        host.queries.lock().await.push(ids.clone());
        if host.reject_existing.load(Ordering::SeqCst) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let alive = ids
            .into_iter()
            .filter(|id| id == "102" || id == "103")
            .map(|id| json!({"Id":id}))
            .collect::<Vec<_>>();
        return Json(json!({"Items":alive,"TotalRecordCount":alive.len()})).into_response();
    }
    Json(
        json!({"Items":[{"Id":"101","Type":"Movie","Name":"已处理电影","MediaSources":[
        {"Id":"opaque-source-main","ItemId":"101"},
        {"Id":"opaque-source-secondary","ItemId":"102"}
    ]}],"TotalRecordCount":1}),
    )
    .into_response()
}
async fn run(state: &AppState) -> Value {
    let task = tasks::submit_task(state, "role-translation", false)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = state
                .jobs
                .result(task["task_id"].as_str().unwrap())
                .await
                .unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn logs(pool: &sqlx::PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT item_id FROM processed_log ORDER BY item_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstream is localhost"]
async fn native_identity_preserves_observation_and_live_role_records() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("native_source_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let query = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let query = query.clone();
            Box::pin(async move {
                sqlx::query(&query).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new(pool.clone(), directory.path().into())
        .await
        .unwrap();
    let observed = revalidation::observation(&state, &version_item()).await;
    assert_eq!(
        observed,
        (
            "/media/episode-102.strm".into(),
            "/cd2/episode-102.mkv".into(),
            Some(200)
        )
    );
    let host = Host::default();
    let mock = Router::new()
        .route(
            "/Users/admin/Views",
            get(|| async { Json(json!({"Items":[{"Id":"library"}]})) }),
        )
        .route("/Items", get(items))
        .route("/Users/admin/Items/{id}", get(details))
        .route("/Videos/101/AdditionalParts", get(parts))
        .with_state(host.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","ai_translate_actor_role":true})).await.unwrap();
    let target = media_edit::repair_target(&state, "101").await.unwrap();
    assert_eq!(target.path, "/media/movie-101.strm");
    assert_eq!(
        target
            .versions
            .iter()
            .find(|v| v["emby_id"] == "102")
            .unwrap()["path"],
        "/media/movie-102.strm"
    );
    assert_eq!(
        target
            .versions
            .iter()
            .find(|v| v["emby_id"] == "101")
            .unwrap()["path"],
        "/media/movie-101.strm"
    );
    host.wrong_version.store(true, Ordering::SeqCst);
    assert!(media_edit::repair_target(&state, "101").await.is_err());
    host.wrong_version.store(false, Ordering::SeqCst);
    let info = maintenance_item("101");
    let segments = media_edit::additional_parts(&state, "101", &info)
        .await
        .unwrap();
    assert_eq!(segments.len(), 2);
    assert_eq!(*host.parts_sources.lock().await, ["mediasource_101"]);
    let mut wrong_source = info.clone();
    wrong_source["MediaSources"] = json!([{"Id":"mediasource_102","Path":info["Path"]}]);
    for owner in [Value::Null, json!("")] {
        wrong_source["MediaSources"][0]["ItemId"] = owner;
        assert!(
            media_edit::additional_parts(&state, "101", &wrong_source)
                .await
                .is_err()
        );
    }
    assert_eq!(
        *host.parts_sources.lock().await,
        ["mediasource_101"],
        "conflicting source must be rejected before querying parts"
    );
    for mode in [1, 2, 3, 4] {
        host.parts_mode.store(mode, Ordering::SeqCst);
        assert!(
            media_edit::additional_parts(&state, "101", &info)
                .await
                .is_err(),
            "mode {mode} must not return a partial segment range"
        );
    }
    host.parts_mode.store(5, Ordering::SeqCst);
    assert_eq!(
        media_edit::additional_parts(&state, "101", &info)
            .await
            .unwrap()
            .len(),
        2
    );
    for id in ["101", "102", "103", "104"] {
        sqlx::query("INSERT INTO processed_log(item_id,item_name) VALUES($1,$1)")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let result = run(&state).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(logs(&pool).await, ["101", "102", "103"]);
    assert!(
        host.queries
            .lock()
            .await
            .iter()
            .all(|ids| !ids.contains(&"102".into())),
        "native ItemId version should already be known alive"
    );
    sqlx::query("INSERT INTO processed_log(item_id,item_name) VALUES('104','104')")
        .execute(&pool)
        .await
        .unwrap();
    host.reject_existing.store(true, Ordering::SeqCst);
    let result = run(&state).await;
    assert_eq!(result["state"], "completed_with_warnings", "{result}");
    assert_eq!(result["result"]["warnings"], 1);
    assert!(
        result["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("已保留处理和复核记录")
    );
    assert_eq!(logs(&pool).await, ["101", "102", "103", "104"]);
    server.abort();
    state.jobs.shutdown().await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
