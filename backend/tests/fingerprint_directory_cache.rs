use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use emby_toolkit::{AppState, db, p115, revalidation};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Cloud {
    requests: Arc<Mutex<Vec<(String, usize)>>>,
    parent: Arc<Mutex<String>>,
}

async fn run(state: &AppState, ids: Vec<String>) -> Value {
    let work = state.clone();
    let task = state
        .jobs
        .submit("指纹目录缓存验证", move |context| async move {
            revalidation::ids(&work, &ids, &context).await
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let result = state.jobs.result(&task).await.unwrap();
            if result["is_running"] == false {
                assert_eq!(result["state"], "completed", "{result}");
                assert_eq!(result["progress_known"], true, "{result}");
                assert_eq!(result["phase"], "指纹校验", "{result}");
                return result["result"].clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn batch_reuses_complete_parent_pages_but_reads_current_files_and_new_roots() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("fingerprint_directories_{}", uuid::Uuid::new_v4().simple());
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
    *cloud.parent.lock().await = "3".into();
    let fake = Router::new().route("/files", get(
        |State(cloud): State<Cloud>, Query(query): Query<HashMap<String, String>>| async move {
            let cid = query["cid"].clone();
            let offset = query["offset"].parse::<usize>().unwrap();
            cloud.requests.lock().await.push((cid.clone(), offset));
            let parent = cloud.parent.lock().await.clone();
            if matches!(cid.as_str(), "9" | "20") {
                // The real target folder is on page two; partial page caching would miss it.
                let all = (0..1000).map(|n| json!({"cid":format!("dir-{n}"),"name":format!("Other-{n}"),"fc":"0"}))
                    .chain(std::iter::once(json!({"cid":parent,"name":"Film","fc":"0"})))
                    .collect::<Vec<_>>();
                Json(json!({"state":true,"count":all.len(),"data":all.into_iter().skip(offset).take(1000).collect::<Vec<_>>()}))
            } else if cid == parent {
                let revision = cloud.requests.lock().await.iter().filter(|(id, _)| id == &cid).count();
                Json(json!({"state":true,"count":2,"data":[
                    {"id":"101","name":"First.mkv","pick_code":"101","sha1":format!("FIRST-{revision}"),"size":100,"fc":"1"},
                    {"id":"102","name":"Second.mkv","pick_code":"102","sha1":format!("SECOND-{revision}"),"size":100,"fc":"1"}
                ]}))
            } else {
                Json(json!({"state":false,"errno":20018}))
            }
        }
    )).with_state(cloud.clone());
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
    let emby_root = directory.path().to_string_lossy().to_string();
    let emby = Router::new().route("/Users/admin/Items/{id}", get(move |Path(id): Path<String>| {
        let root = emby_root.clone();
        async move { Json(json!({"Id":id,"Type":"Movie","Path":format!("{root}/Film/{id}.strm"),"Size":100})) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let emby_address = listener.local_addr().unwrap();
    let emby_server = tokio::spawn(async move { axum::serve(listener, emby).await.unwrap() });
    state.save_config(&json!({"cd2_media_root":"/cd2","local_strm_root":directory.path(),"p115_fingerprint_media_root_cid":"9","emby_user_id":"admin","emby_api_key":"fixture","emby_server_url":format!("http://{emby_address}")})).await.unwrap();
    tokio::fs::create_dir_all(directory.path().join("Film"))
        .await
        .unwrap();
    for stem in ["First", "Second"] {
        let path = directory.path().join(format!("Film/{stem}.strm"));
        tokio::fs::write(&path, format!("/cd2/Film/{stem}.mkv"))
            .await
            .unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Movie',true,$2,$3,$4,$5)")
            .bind(stem).bind(json!([stem])).bind(json!([{"emby_item_id":stem,"path":path,"size_bytes":100}])).bind(json!(["OLD-SHA"])).bind(json!(["OLD-PC"])).execute(&pool).await.unwrap();
    }
    let result = run(&state, vec!["First".into(), "Second".into()]).await;
    assert_eq!(result["updated"], 2, "{result}");
    let requests = cloud.requests.lock().await.clone();
    assert_eq!(
        requests
            .iter()
            .filter(|(id, _)| id == "9")
            .cloned()
            .collect::<Vec<_>>(),
        vec![("9".into(), 0), ("9".into(), 1000)]
    );
    assert_eq!(requests.iter().filter(|(id, _)| id == "3").count(), 2);
    let first_sha: Value =
        sqlx::query_scalar("SELECT file_sha1_json FROM media_metadata WHERE tmdb_id='First'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let second_sha: Value =
        sqlx::query_scalar("SELECT file_sha1_json FROM media_metadata WHERE tmdb_id='Second'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(matches!(
        (first_sha[0].as_str(), second_sha[0].as_str()),
        (Some("FIRST-1"), Some("SECOND-2")) | (Some("FIRST-2"), Some("SECOND-1"))
    ));
    // Each later batch starts from the current root and freshly discovers its child IDs.
    for parent in ["4", "5"] {
        state
            .save_config(&json!({"p115_fingerprint_media_root_cid":"20"}))
            .await
            .unwrap();
        *cloud.parent.lock().await = parent.into();
        cloud.requests.lock().await.clear();
        assert_eq!(run(&state, vec!["First".into()]).await["updated"], 1);
        assert_eq!(
            *cloud.requests.lock().await,
            vec![("20".into(), 0), ("20".into(), 1000), (parent.into(), 0)]
        );
    }
    state.jobs.shutdown().await;
    server.abort();
    emby_server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
