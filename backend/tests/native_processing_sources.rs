use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, audio_tasks, thumbnails, webhook};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[test]
fn thumbnail_repair_refuses_another_native_owner_even_with_one_source() {
    let mut item = json!({"Id":"102","Path":"/media/102.strm","MediaSources":[
        {"ItemId":"101","Path":"/media/102.strm"}
    ]});
    assert!(thumbnails::source(&item).is_err());
    item["MediaSources"] = json!([{"Id":"mediasource_101","Path":item["Path"]}]);
    assert!(
        thumbnails::source(&item).is_err(),
        "matching path cannot override another legacy native owner"
    );
}

#[test]
fn thumbnail_repair_uses_native_owner_and_supports_legacy_ids() {
    let mut item = json!({"Id":"102","Path":"/media/102.strm","MediaSources":[
        {"ItemId":"101","Path":"/cd2/101.mkv"},
        {"ItemId":"102","Id":"opaque-source","Path":"/cd2/102.mkv"}
    ]});
    assert_eq!(thumbnails::source(&item).unwrap()["Path"], "/cd2/102.mkv");
    item["MediaSources"] = json!([{"Id":"mediasource_102","Path":"/cd2/102.mkv"}]);
    assert_eq!(thumbnails::source(&item).unwrap()["Path"], "/cd2/102.mkv");
    item["MediaSources"] = json!([
        {"Id":"opaque-a","Path":"/cd2/a.mkv"},
        {"Id":"opaque-b","Path":"/cd2/b.mkv"}
    ]);
    assert!(thumbnails::source(&item).is_err());
}

#[derive(Clone)]
struct Host {
    items: Arc<Mutex<HashMap<String, Value>>>,
    reads: Arc<Mutex<Vec<String>>>,
    refreshes: Arc<Mutex<Vec<String>>>,
}

async fn run_audio(state: &AppState, ids: Option<Vec<String>>) -> Value {
    let work = state.clone();
    let task = state
        .jobs
        .submit("扫描高清音轨", move |context| async move {
            match ids {
                Some(ids) => audio_tasks::repair(&work, &ids, &context).await,
                None => audio_tasks::repair_all(&work, &context).await,
            }
        })
        .await
        .unwrap();
    completed(state, &task).await
}

async fn completed(state: &AppState, task: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let result = state.jobs.result(task).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn audio_scan_processes_each_legacy_native_version_without_modifying_files() {
    let admin = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let schema = format!("audio_sources_{}", uuid::Uuid::new_v4().simple());
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
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, directory.path().into()).await.unwrap();
    let mut items = HashMap::new();
    let mut assets = vec![];
    let mut sources = vec![];
    let mut originals = vec![];
    for id in ["101", "102"] {
        let path = directory.path().join(format!("{id}.strm"));
        let playback = format!("/cd2/{id}.mkv");
        tokio::fs::write(&path, &playback).await.unwrap();
        let streams = json!([{"Type":"Video","Codec":"hevc"},{"Type":"Audio","Codec":"aac"}]);
        let source = json!({"Id":format!("mediasource_{id}"),"Path":playback,"Size":1000,"MediaStreams":streams});
        items.insert(id.into(), json!({"Id":id,"Name":id,"Type":"Movie","Path":path,"Size":1000,"PartCount":1,"MediaStreams":streams,"DateLastRefreshed":"2026-11-08T00:00:00Z"}));
        assets.push(json!({"emby_item_id":id,"path":path,"size_bytes":1000}));
        sources.push(source.clone());
        let info = json!([{"MediaSourceInfo":{"Path":playback,"Size":1000,"Container":"mkv","RunTimeTicks":600000000_u64,"MediaStreams":streams},"Chapters":[]}]);
        let backup = directory.path().join(format!("{id}-mediainfo.json"));
        let bytes = serde_json::to_vec(&info).unwrap();
        tokio::fs::write(&backup, &bytes).await.unwrap();
        originals.push((backup, bytes));
    }
    for item in items.values_mut() {
        item["MediaSources"] = json!(sources);
    }
    let top = items["101"].clone();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('audio-sources','Movie',$1,$2,$3,$3)")
        .bind(json!(["101","102"])).bind(json!(assets)).bind(json!([null,null])).execute(&state.db).await.unwrap();
    let host = Host {
        items: Arc::new(Mutex::new(items)),
        reads: Arc::default(),
        refreshes: Arc::default(),
    };
    let mock = Router::new()
        .route(
            "/emby/Users/admin/Views",
            get(|| async { Json(json!({"Items":[{"Id":"library"}]})) }),
        )
        .route(
            "/Items",
            get(move || {
                let top = top.clone();
                async move { Json(json!({"Items":[top],"TotalRecordCount":1})) }
            }),
        )
        .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
        .route(
            "/ScheduledTasks",
            get(|| async {
                Json(json!([{"Id":"scan","Name":"Scan media library","State":"Idle"}]))
            }),
        )
        .route(
            "/Items/{id}/Refresh",
            post(
                |State(host): State<Host>, Path(id): Path<String>| async move {
                    host.refreshes.lock().await.push(id.clone());
                    host.items.lock().await.get_mut(&id).unwrap()["DateLastRefreshed"] =
                        json!("2026-11-08T00:00:01Z");
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .route(
            "/Users/admin/Items/{id}",
            get(
                |State(host): State<Host>, Path(id): Path<String>| async move {
                    host.reads.lock().await.push(id.clone());
                    Json(host.items.lock().await[&id].clone())
                },
            ),
        )
        .with_state(host.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","local_strm_root":directory.path(),"monitor_sha1_pc_search":false})).await.unwrap();
    let result = run_audio(&state, None).await;
    host.items.lock().await.get_mut("102").unwrap()["MediaSources"] = json!([
        {"ItemId":"101","Path":"/cd2/102.mkv"}
    ]);
    let ambiguous = run_audio(&state, Some(vec!["102".into()])).await;
    state.save_config(&json!({"auto_repair_truehd_core_on_ingest":true,"auto_fill_language_titles_on_ingest":false,"telegram_notify_types":[]})).await.unwrap();
    let work = state.clone();
    let task = state
        .jobs
        .submit("入库收尾", move |context| async move {
            webhook::finalize(&work, "102", &[], true, &context).await
        })
        .await
        .unwrap();
    let finalized = completed(&state, &task).await;
    let reads = host.reads.lock().await.clone();
    let mut unchanged = true;
    for (path, bytes) in originals {
        unchanged &= tokio::fs::read(path).await.unwrap() == bytes;
    }
    state.jobs.shutdown().await;
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["skipped"], 2, "{result}");
    assert_eq!(result["result"]["failed"], 0, "{result}");
    assert_eq!(ambiguous["state"], "completed_with_warnings", "{ambiguous}");
    assert_eq!(ambiguous["result"]["warnings"], 1, "{ambiguous}");
    assert!(
        ambiguous["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("无法唯一确认当前版本")
    );
    assert_eq!(finalized["state"], "completed_with_warnings", "{finalized}");
    assert_eq!(
        finalized["result"]["audio_repair"]["failed"], 0,
        "{finalized}"
    );
    assert_eq!(
        finalized["result"]["audio_repair"]["ambiguous"], 1,
        "{finalized}"
    );
    assert_eq!(finalized["result"]["warnings"], 1, "{finalized}");
    assert!(
        finalized["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("无法唯一确认当前版本")
    );
    assert_eq!(
        *host.refreshes.lock().await,
        ["102"],
        "入库收尾仍应仅刷新当前电影"
    );
    assert!(
        reads.contains(&"101".into()) && reads.contains(&"102".into()),
        "{reads:?}"
    );
    assert!(unchanged, "普通 AAC 音轨不得触发 JSON 修改");
}
