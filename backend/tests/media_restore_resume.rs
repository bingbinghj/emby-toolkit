use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use emby_toolkit::{AppState, jobs::JobSpec};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicUsize, Ordering}}};

#[derive(Clone)]
struct Host {
    item: Value,
    unexpected: Arc<AtomicUsize>,
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn restart_keeps_the_original_restore_file_scope_and_already_verified_json() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("restore_resume_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&admin).await.unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new().after_connect(move |connection, _| {
        let search = search.clone();
        Box::pin(async move { sqlx::query(&search).execute(connection).await?; Ok(()) })
    }).connect(&url).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("media");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("original.strm");
    let source = "http://fixture.invalid/原媒体.mkv";
    std::fs::write(&path, source).unwrap();
    // This file arrived after the interrupted task captured its original scope.
    // Its empty source would fail if a restart silently rescanned new work.
    std::fs::write(root.join("later.strm"), "").unwrap();
    let streams = json!([{"Type":"Video","Index":0,"Codec":"h264","Width":1920,"Height":1080},
        {"Type":"Audio","Index":1,"Codec":"aac","Title":"原始备注","Language":"jpn","Channels":2}]);
    let info = json!({"MediaSourceInfo":{"Path":source,"Container":"mkv","Size":1024,
        "RunTimeTicks":300000000,"MediaStreams":streams},"Chapters":[]});
    let json_path = path.with_file_name("original-mediainfo.json");
    let original = serde_json::to_vec_pretty(&info).unwrap();
    std::fs::write(&json_path, &original).unwrap();
    let before = std::fs::metadata(&json_path).unwrap().modified().unwrap();
    let host = Host { item: json!({"Id":"101","Type":"Movie","Path":path,"Size":1024,
        "MediaStreams":streams,"MediaSources":[{"ItemId":"101","Path":path,"Container":"mkv","MediaStreams":streams}],"Chapters":[]}), unexpected: Arc::default() };
    let api = Router::new()
        .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
        .route("/Users/admin/Items/101", get(|State(host): State<Host>| async move { Json(host.item) }))
        .fallback(|State(host): State<Host>| async move { host.unexpected.fetch_add(1, Ordering::SeqCst); StatusCode::BAD_GATEWAY })
        .with_state(host.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap(); });
    let data_dir: PathBuf = directory.path().join("app");
    let state = AppState::new(pool.clone(), data_dir.clone()).await.unwrap();
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","local_strm_root":root})).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let asset = json!({"path":path,"size_bytes":1024});
    let mut labels = serde_json::Map::new();
    labels.insert(path.to_string_lossy().to_string(), json!("原媒体"));
    sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json,checkpoint_json) VALUES($1,$2,$3,$4)")
        .bind(&id).bind(json!(JobSpec::new("tasks", json!({"task":"restore_mediainfo","full":false,"batch_size":10,"unsupported":[]}))))
        .bind(json!({"task_id":id,"state":"running","is_running":true,"last_action":"还原媒体信息","progress":80}))
        .bind(json!({"plans":{"media-restore-files":{"assets":[[path,[["101",asset]]]],"labels":labels}},
            "completed":{"media-restore-import:101":{"path":path,"source":source}}}))
        .execute(&pool).await.unwrap();
    let restarted = AppState::new(pool.clone(), data_dir).await.unwrap();
    restarted.jobs.resume_pending(restarted.clone(), tokio_util::sync::CancellationToken::new()).await.unwrap();
    let result = restarted.jobs.result(&id).await.unwrap();
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["verified_ids"], json!(["101"]));
    assert_eq!(result["result"]["failures"], json!([]));
    assert_eq!(std::fs::read(&json_path).unwrap(), original);
    assert_eq!(std::fs::metadata(&json_path).unwrap().modified().unwrap(), before);
    assert_eq!(host.unexpected.load(Ordering::SeqCst), 0);
    // A verified full-version save has no media work left. Losing configuration
    // while restarting must not turn its final status update into a new probe.
    let completed_id = uuid::Uuid::new_v4().to_string();
    let mut completed_report = result["result"].clone();
    completed_report["updated"] = json!(1);
    completed_report["full_restore"] = json!({"verified":1});
    completed_report["message"] = json!("媒体信息还原结束：本地恢复 1 个文件，Emby 回读确认 1 个目标，未绑定待导入 0 个，失败 0 项");
    sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json,checkpoint_json) VALUES($1,$2,$3,$4)")
        .bind(&completed_id).bind(json!(JobSpec::new("tasks", json!({"task":"restore_mediainfo","full":true,"batch_size":10,"unsupported":[]}))))
        .bind(json!({"task_id":completed_id,"state":"interrupted","is_running":false,"last_action":"还原媒体信息","progress":99}))
        .bind(json!({"plans":{},"completed":{"media-restore-full-save":{"status":200,"report":{"verified":1},"ids":["101"],"task_result":completed_report}}}))
        .execute(&pool).await.unwrap();
    restarted.save_config(&json!({"emby_server_url":"","local_strm_root":""})).await.unwrap();
    let finalized = AppState::new(pool.clone(), restarted.data_dir.clone()).await.unwrap();
    finalized.jobs.resume_pending(finalized.clone(), tokio_util::sync::CancellationToken::new()).await.unwrap();
    let finalized_result = finalized.jobs.result(&completed_id).await.unwrap();
    assert_eq!(finalized_result["state"], "completed", "{finalized_result}");
    assert_eq!(finalized_result["result"], completed_report);
    assert_eq!(host.unexpected.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&json_path).unwrap(), original);
    finalized.jobs.shutdown().await;
    restarted.jobs.shutdown().await;
    state.jobs.shutdown().await;
    server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(&admin).await.unwrap();
    admin.close().await;
}
