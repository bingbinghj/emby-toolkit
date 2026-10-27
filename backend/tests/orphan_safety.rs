use axum::{
    Json, Router,
    extract::{Form, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, db, orphans, p115};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Cloud {
    mode: Arc<Mutex<u8>>,
    moves: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

fn hits() -> Vec<Value> {
    vec![
        json!({"id":"101","name":"First.mkv","parent_id":"99","fc":"1"}),
        json!({"id":"102","name":"First.srt","parent_id":"99","fc":"1"}),
        json!({"id":"103","name":"First.zh.ass","parent_id":"99","fc":"1"}),
        json!({"id":"104","name":"First2.mkv","parent_id":"99","fc":"1"}),
        json!({"id":"105","name":"FirstExtra.srt","parent_id":"99","fc":"1"}),
        json!({"id":"106","name":"First.mp4","parent_id":"99","fc":"1"}),
        json!({"id":"107","name":"First.nfo","parent_id":"99","fc":"0"}),
        json!({"id":"108","name":"First.jpg","parent_id":"98","fc":"1"}),
        json!({"id":"109","name":"First.png","parent_id":"99","fc":"1"}),
        json!({"id":"110","name":"First-mediainfo.json","parent_id":"99","fc":"1"}),
        json!({"id":"111","name":"First2-mediainfo.json","parent_id":"99","fc":"1"}),
    ]
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_repair_requires_confirmed_missing_parent_and_only_moves_exact_sidecars() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("orphan_safety_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&root)
        .await
        .unwrap();
    let search_path = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |conn, _| {
            let query = search_path.clone();
            Box::pin(async move {
                sqlx::query(&query).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut state = AppState::new(pool.clone(), directory.path().into())
        .await
        .unwrap();
    let cloud = Cloud::default();
    let fake = Router::new()
        .route("/files", get(|State(cloud): State<Cloud>, Query(query): Query<HashMap<String, String>>| async move {
            let data = if query["cid"] == "9" {
                vec![json!({"id":"3","name":"Film","fc":"0"})]
            } else if cloud.moves.lock().await.is_empty() {
                vec![]
            } else {
                vec![json!({"id":"101","name":"First.mkv","parent_id":"3","fc":"1"})]
            };
            Json(json!({"state":true,"count":data.len(),"data":data}))
        }))
        .route("/files/search", get(|State(cloud): State<Cloud>| async move {
            let data = hits();
            Json(json!({"state":true,"count":if *cloud.mode.lock().await == 2 {200}else{data.len()},"data":data}))
        }))
        .route("/files/file", get(|State(cloud): State<Cloud>, Query(query): Query<HashMap<String, String>>| async move {
            assert_eq!(query["file_id"], "99");
            match *cloud.mode.lock().await {
                0 => (StatusCode::OK, Json(json!({"state":false,"errno":401,"message":"凭证无效"}))),
                3 => (StatusCode::OK, Json(json!({"state":false,"message":"暂时无法查询目录"}))),
                4 => (StatusCode::BAD_GATEWAY, Json(json!({}))),
                5 => (StatusCode::OK, Json(json!({"state":true,"data":{"id":"99"}}))),
                _ => (StatusCode::OK, Json(json!({"state":false,"errno":20018,"message":"文件不存在或已删除"}))),
            }
        }))
        .route("/files/move", post(|State(cloud): State<Cloud>, Form(query): Form<HashMap<String, String>>| async move {
            cloud.moves.lock().await.push(query);
            Json(json!({"state":true}))
        })).with_state(cloud.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
        webapi: format!("http://{address}"),
        ..Default::default()
    }));
    db::save_setting(
        &pool,
        "p115_auth_tokens",
        &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
    )
    .await
    .unwrap();
    state
        .save_config(
            &json!({"local_strm_root":directory.path(),"p115_fingerprint_media_root_cid":"9"}),
        )
        .await
        .unwrap();
    let path = directory.path().join("Film/First.strm");
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('First','Movie',true,$1,$2,$3,$4)")
        .bind(json!(["First"])).bind(json!([{"emby_item_id":"First","path":path}])).bind(json!(["SHA"])).bind(json!(["101"])).execute(&pool).await.unwrap();
    for mode in [0, 2, 3, 4, 5, 1] {
        *cloud.mode.lock().await = mode;
        cloud.moves.lock().await.clear();
        let work = state.clone();
        let task = state
            .jobs
            .submit("孤儿修复验证", move |context| async move {
                orphans::repair(&work, "First", &context).await
            })
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let result = state.jobs.result(&task).await.unwrap();
                if result["is_running"] == false {
                    return result["result"].clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let moves = cloud.moves.lock().await;
        if mode == 5 {
            assert_eq!(result["skipped_mismatch"], 1, "{result}");
            assert!(
                moves.is_empty(),
                "an existing parent must never move: {moves:?}"
            );
        } else if mode != 1 {
            assert_eq!(result["failed"], 1, "{result}");
            assert!(
                moves.is_empty(),
                "unconfirmed parent or incomplete hits must never move: {moves:?}"
            );
        } else {
            assert_eq!(result["repaired_videos"], 1, "{result}");
            assert_eq!(moves.len(), 1);
            let moved = moves[0]
                .iter()
                .filter(|(key, _)| key.starts_with("fid["))
                .map(|(_, id)| id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                moved,
                std::collections::BTreeSet::from(["101", "102", "103", "109", "110"])
            );
            assert_eq!(moves[0]["pid"], "3");
        }
    }
    state.jobs.shutdown().await;
    server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
