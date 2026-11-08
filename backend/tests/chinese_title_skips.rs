use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    routing::get,
};
use emby_toolkit::{AppState, titles};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Host {
    item: Arc<Mutex<Value>>,
    unexpected: Arc<Mutex<Vec<String>>>,
}

fn streams(mixed: bool, unknown: bool) -> Vec<Value> {
    let mut streams = vec![json!({"Type":"Video","Codec":"hevc"})];
    for index in 0..9 {
        streams.push(json!({"Type":"Subtitle","Codec":"subrip","Language":"chi","Title":format!("简中特效{index}")}));
        if index == 0 && mixed {
            streams.push(json!({"Type":"Audio","Codec":"aac","Language":"jpn","Title":"日语"}));
            streams
                .push(json!({"Type":"Subtitle","Codec":"subrip","Language":"eng","Title":"英文"}));
        }
    }
    if unknown {
        streams
            .push(json!({"Type":"Subtitle","Codec":"subrip","Language":"und","Title":"保留原文"}));
    }
    streams
}

async fn run(state: &AppState) -> Value {
    let work = state.clone();
    let task = state
        .jobs
        .submit("入库语言标题", move |context| async move {
            titles::fill_in_job(&work, &["61616".into()], true, &context).await
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = state.jobs.result(&task).await.unwrap();
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
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn chinese_subtitle_skips_are_normal_and_do_not_trigger_title_restoration() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("chinese_titles_{}", uuid::Uuid::new_v4().simple());
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
    let path = directory.path().join("61616.strm");
    let playback = "/cd2/61616.mkv";
    tokio::fs::write(&path, playback).await.unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('chinese-title-skips','Movie',$1,$2,$3,$3)")
        .bind(json!(["61616"])).bind(json!([{"emby_item_id":"61616","path":path,"size_bytes":1000}])).bind(json!([null])).execute(&state.db).await.unwrap();
    let host = Host::default();
    let fake = Router::new()
        .route(
            "/Users/user/Items/61616",
            get(|State(host): State<Host>| async move { Json(host.item.lock().await.clone()) }),
        )
        .fallback(|State(host): State<Host>, request: Request| async move {
            host.unexpected.lock().await.push(format!(
                "{} {}",
                request.method(),
                request.uri().path()
            ));
            StatusCode::NOT_FOUND
        })
        .with_state(host.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":directory.path(),"monitor_sha1_pc_search":false})).await.unwrap();
    let backup = directory.path().join("61616-mediainfo.json");
    let mut evidence = vec![];
    for (mixed, unknown) in [(false, false), (true, false), (true, true)] {
        let original = streams(mixed, unknown);
        let mut live = original.clone();
        for stream in &mut live {
            if stream["Type"] == "Subtitle" && stream["Language"] == "chi" {
                stream["Title"] = json!("Emby现有中文字幕标题");
            }
        }
        *host.item.lock().await = json!({"Id":"61616","Type":"Movie","Name":"中文字幕跳过验证","Path":path,"Size":1000,"PartCount":1,"MediaStreams":live,"MediaSources":[{"Id":"mediasource_61616","ItemId":"61616","Path":playback,"Size":1000,"MediaStreams":live}]});
        let bytes = serde_json::to_vec(&json!([{"MediaSourceInfo":{"Path":playback,"Size":1000,"Container":"mkv","RunTimeTicks":600000000_u64,"MediaStreams":original},"Chapters":[]}])).unwrap();
        tokio::fs::write(&backup, &bytes).await.unwrap();
        let result = run(&state).await;
        evidence.push((
            mixed,
            unknown,
            result,
            tokio::fs::read(&backup).await.unwrap() == bytes,
        ));
    }
    let mut original = streams(true, false);
    original
        .iter_mut()
        .find(|stream| stream["Type"] == "Audio")
        .unwrap()["Title"] = json!("");
    let mut live = original.clone();
    live.iter_mut()
        .find(|stream| stream["Type"] == "Subtitle")
        .unwrap()["Codec"] = json!("ass");
    *host.item.lock().await = json!({"Id":"61616","Type":"Movie","Name":"中文字幕保护验证","Path":path,"Size":1000,"PartCount":1,"MediaStreams":live,"MediaSources":[{"Id":"mediasource_61616","ItemId":"61616","Path":playback,"Size":1000,"MediaStreams":live}]});
    let bytes = serde_json::to_vec(&json!([{"MediaSourceInfo":{"Path":playback,"Size":1000,"Container":"mkv","RunTimeTicks":600000000_u64,"MediaStreams":original},"Chapters":[]}])).unwrap();
    tokio::fs::write(&backup, &bytes).await.unwrap();
    let refused = run(&state).await;
    let refused_unchanged = tokio::fs::read(&backup).await.unwrap() == bytes;
    let unexpected = host.unexpected.lock().await.clone();
    state.jobs.shutdown().await;
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
    for (mixed, unknown, result, unchanged) in evidence {
        assert_eq!(result["state"], "completed", "{result}");
        let report = &result["result"];
        assert_eq!(report["completed"], !unknown, "{report}");
        assert_eq!(report["failed"], 0, "{report}");
        assert_eq!(report["changed_streams"], 0, "{report}");
        assert_eq!(report["changed_languages"], 0, "{report}");
        assert_eq!(
            report["verified_streams"],
            if mixed { 2 } else { 0 },
            "{report}"
        );
        assert_eq!(report["skipped_chinese_streams"], 9, "{report}");
        let pending = report["pending"].as_array().unwrap();
        assert_eq!(pending.len(), usize::from(unknown), "{report}");
        assert!(
            pending.iter().all(|entry| entry["language"] != "chi"),
            "{report}"
        );
        assert!(unchanged, "正常跳过不得修改任何本地中文字幕标题");
    }
    assert_eq!(refused["result"]["completed"], false, "{refused}");
    assert_eq!(refused["result"]["failed"], 1, "{refused}");
    assert!(
        refused["result"]["pending"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("中文字幕编码与本地记录不一致"),
        "{refused}"
    );
    assert!(refused_unchanged, "无法保护当前中文字幕时不得改写本地JSON");
    assert!(
        unexpected.is_empty(),
        "正常跳过及已一致轨道不得触发恢复：{unexpected:?}"
    );
}
