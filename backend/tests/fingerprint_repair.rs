use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use emby_toolkit::{AppState, db, p115, tasks};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Requests {
    cloud: Arc<Mutex<Vec<String>>>,
    emby: Arc<Mutex<Vec<String>>>,
    root: String,
    mutation: Arc<Mutex<Option<sqlx::PgPool>>>,
}

async fn run(state: &AppState) -> Value {
    let task = tasks::submit_task(state, "repair-p115-fingerprints", false)
        .await
        .unwrap();
    finish(state, task["task_id"].as_str().unwrap()).await
}

async fn finish(state: &AppState, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let result = state.jobs.result(id).await.unwrap();
            if result["is_running"] == false {
                assert_eq!(result["progress_known"], true, "{result}");
                assert_eq!(result["completed"], result["total"], "{result}");
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn media(pool: &sqlx::PgPool, path: &std::path::Path, pc: Value, sha: Value, pending: bool) {
    sqlx::query("DELETE FROM media_metadata")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM p115_filesystem_cache")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json,file_pickcode_json,file_sha1_json) VALUES('film','Movie','缓存验证电影',true,$1,$2,$3,$4)")
        .bind(json!(["101"]))
        .bind(json!([{"emby_item_id":"101","path":path,"size_bytes":100,"fingerprint_pending":pending,"fingerprint_revision":7}]))
        .bind(json!([pc])).bind(json!([sha])).execute(pool).await.unwrap();
}

async fn cache(pool: &sqlx::PgPool, parent: Option<&str>, sha: Option<&str>) {
    sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,pick_code,sha1,local_path,size,preid) VALUES('101',$1,'First.mkv','101',$2,'Film/First.mkv',100,'PRESERVE-PREID')")
        .bind(parent.unwrap_or_default()).bind(sha).execute(pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn repair_uses_cache_and_only_queries_cloud_for_missing_cache_fields() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("fingerprint_repair_{}", uuid::Uuid::new_v4().simple());
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
    let mut state = AppState::new(pool.clone(), directory.path().into())
        .await
        .unwrap();
    let requests = Requests {
        root: directory.path().to_string_lossy().into(),
        ..Default::default()
    };
    let fake = Router::new()
        .route("/files/file", get(|State(requests): State<Requests>, Query(query): Query<HashMap<String,String>>| async move {
            requests.cloud.lock().await.push(query["file_id"].clone());
            if let Some(pool) = requests.mutation.lock().await.take() {
                sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(jsonb_set(asset_details_json,'{0,fingerprint_pending}','true'),'{0,fingerprint_revision}','8') WHERE tmdb_id='film'").execute(&pool).await.unwrap();
            }
            Json(json!({"state":true,"data":{"id":query["file_id"],"parent_id":"3","name":if query["file_id"] == "102" { "Second.mkv" } else { "First.mkv" },"pick_code":query["file_id"],"sha1":"CACHE-SHA","size":100}}))
        }))
        .route("/Users/admin/Items/{id}", get(|State(requests): State<Requests>, Path(id): Path<String>| async move {
            requests.emby.lock().await.push(id.clone());
            Json(json!({"Id":id,"Type":"Movie","Name":"缓存验证电影","Path":format!("{}/Film/First.strm",requests.root),"Size":100}))
        })).with_state(requests.clone());
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
    state.save_config(&json!({"cd2_media_root":"/cd2","local_strm_root":directory.path(),"emby_user_id":"admin","emby_api_key":"fixture","emby_server_url":format!("http://{address}")})).await.unwrap();
    tokio::fs::create_dir_all(directory.path().join("Film"))
        .await
        .unwrap();
    let path = directory.path().join("Film/First.strm");
    tokio::fs::write(&path, "http://fixture/d/101")
        .await
        .unwrap();

    // A complete media fingerprint and complete cache must not revalidate either service.
    media(&pool, &path, json!("101"), json!("CACHE-SHA"), false).await;
    cache(&pool, Some("3"), Some("CACHE-SHA")).await;
    let result = run(&state).await;
    assert!(
        requests.cloud.lock().await.is_empty(),
        "complete cache unexpectedly queried 115: {result}"
    );
    assert!(
        requests.emby.lock().await.is_empty(),
        "repair unexpectedly queried Emby: {result}"
    );
    assert_eq!(result["result"]["skipped"], 1, "{result}");

    // Complete media fingerprints are preserved even when the same PC's cached SHA1 differs.
    sqlx::query("UPDATE media_metadata SET file_sha1_json=$1 WHERE tmdb_id='film'")
        .bind(json!(["ORIGINAL-SHA"]))
        .execute(&pool)
        .await
        .unwrap();
    let result = run(&state).await;
    assert_eq!(result["result"]["skipped"], 1, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");
    let hashes: Value =
        sqlx::query_scalar("SELECT file_sha1_json FROM media_metadata WHERE tmdb_id='film'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(hashes, json!(["ORIGINAL-SHA"]));

    // Legacy JSON strings use the same populated slots as native JSON arrays.
    sqlx::query(
        "UPDATE media_metadata SET file_pickcode_json=$1,file_sha1_json=$2 WHERE tmdb_id='film'",
    )
    .bind(json!("[\"101\"]"))
    .bind(json!("[\"CACHE-SHA\"]"))
    .execute(&pool)
    .await
    .unwrap();
    let result = run(&state).await;
    assert_eq!(result["result"]["skipped"], 1, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");

    // Python also treats the asset's known size and real filename as populated cache values.
    media(
        &pool,
        &directory.path().join("Film/First.mkv"),
        json!("101"),
        json!("CACHE-SHA"),
        false,
    )
    .await;
    cache(&pool, Some("3"), Some("CACHE-SHA")).await;
    sqlx::query("UPDATE p115_filesystem_cache SET name='',size=0 WHERE id='101'")
        .execute(&pool)
        .await
        .unwrap();
    let result = run(&state).await;
    assert_eq!(result["result"]["skipped"], 1, "{result}");
    assert_eq!(result["result"]["cache_updates"], 0, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");

    // Cache-only API writes leave legacy snapshots unchanged for later versions in the same row.
    media(&pool, &path, json!("101"), json!("CACHE-SHA"), false).await;
    let second = directory.path().join("Film/Second.strm");
    tokio::fs::write(&second, "http://fixture/d/102")
        .await
        .unwrap();
    let assets = json!([{"emby_item_id":"101","path":path,"size_bytes":100},{"emby_item_id":"102","path":second,"size_bytes":100}]);
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_pickcode_json=$3,file_sha1_json=$4 WHERE tmdb_id='film'")
        .bind(json!(["101","102"])).bind(json!(assets.to_string())).bind(json!("[\"101\",\"102\"]")).bind(json!("[\"CACHE-SHA\",\"CACHE-SHA\"]"))
        .execute(&pool).await.unwrap();
    cache(&pool, None, Some("CACHE-SHA")).await;
    sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,pick_code,sha1,local_path,size) VALUES('102','','Second.mkv','102','CACHE-SHA','Film/Second.mkv',100)")
        .execute(&pool).await.unwrap();
    let result = run(&state).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["cache_updates"], 2, "{result}");
    assert_eq!(result["result"]["updated_rows"], 0, "{result}");
    requests.cloud.lock().await.clear();

    // Webhook repairs only the requested new version and retains sibling slots.
    sqlx::query("UPDATE media_metadata SET file_pickcode_json='[null,null]',file_sha1_json='[null,null]' WHERE tmdb_id='film'")
        .execute(&pool).await.unwrap();
    let work = state.clone();
    let task = state
        .jobs
        .submit("Webhook新集指纹补齐", move |context| async move {
            emby_toolkit::revalidation::repair_ids(&work, &["101".into()], &context).await
        })
        .await
        .unwrap();
    let result = finish(&state, &task).await;
    assert_eq!(result["total"], 1, "{result}");
    assert_eq!(result["result"]["cache_fixed_assets"], 1, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");
    let pair: Value = sqlx::query_scalar("SELECT jsonb_build_array(file_pickcode_json,file_sha1_json) FROM media_metadata WHERE tmdb_id='film'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(pair[0], json!(["101", null]));
    assert_eq!(pair[1], json!(["CACHE-SHA", null]));

    // Missing PC means obtain the whole pair from one selected cache row, replacing orphan SHA1.
    media(&pool, &path, Value::Null, json!("ORPHAN-SHA"), false).await;
    cache(&pool, Some("3"), Some("CACHE-SHA")).await;
    let result = run(&state).await;
    assert_eq!(result["result"]["fixed_assets"], 1, "{result}");
    assert_eq!(result["result"]["cache_fixed_assets"], 1, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");
    let pair: Value = sqlx::query_scalar("SELECT jsonb_build_array(file_pickcode_json,file_sha1_json,asset_details_json) FROM media_metadata WHERE tmdb_id='film'").fetch_one(&pool).await.unwrap();
    assert_eq!(pair[0], json!(["101"]));
    assert_eq!(pair[1], json!(["CACHE-SHA"]));
    assert_eq!(pair[2][0]["fingerprint_revision"], 7);

    // An existing PC and a missing SHA1 also use that exact cache identity without cloud calls.
    media(&pool, &path, json!("101"), Value::Null, false).await;
    cache(&pool, Some("3"), Some("CACHE-SHA")).await;
    let result = run(&state).await;
    assert_eq!(result["result"]["cache_fixed_assets"], 1, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");

    // Complete media fingerprints still repair a cache with missing parent fields using one file-info request.
    media(&pool, &path, json!("101"), json!("CACHE-SHA"), false).await;
    cache(&pool, None, Some("CACHE-SHA")).await;
    let result = run(&state).await;
    assert_eq!(*requests.cloud.lock().await, vec!["101"]);
    assert!(requests.emby.lock().await.is_empty(), "{result}");
    assert_eq!(result["result"]["cache_updates"], 1, "{result}");
    let cached: Value =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_filesystem_cache c WHERE id='101'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cached["parent_id"], "3");
    assert_eq!(cached["local_path"], "Film/First.mkv");
    assert_eq!(cached["preid"], "PRESERVE-PREID");
    let revision: Value =
        sqlx::query_scalar("SELECT asset_details_json FROM media_metadata WHERE tmdb_id='film'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revision[0]["fingerprint_revision"], 7);

    // Python merges populated media fields before deciding whether the cache needs an API lookup.
    requests.cloud.lock().await.clear();
    media(&pool, &path, json!("101"), json!("ORIGINAL-SHA"), false).await;
    cache(&pool, Some("3"), None).await;
    let result = run(&state).await;
    assert!(requests.cloud.lock().await.is_empty(), "{result}");
    assert_eq!(result["result"]["fixed_assets"], 0, "{result}");
    assert_eq!(result["result"]["updated_rows"], 0, "{result}");
    assert_eq!(result["result"]["cache_updates"], 0, "{result}");
    let hashes: Value =
        sqlx::query_scalar("SELECT file_sha1_json FROM media_metadata WHERE tmdb_id='film'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(hashes, json!(["ORIGINAL-SHA"]));

    // Pending identity must never be filled from old cache or silently cleared.
    requests.cloud.lock().await.clear();
    media(&pool, &path, Value::Null, Value::Null, true).await;
    cache(&pool, Some("3"), Some("CACHE-SHA")).await;
    let result = run(&state).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["failed"], 1, "{result}");
    assert!(requests.cloud.lock().await.is_empty(), "{result}");
    let pending: Value = sqlx::query_scalar("SELECT jsonb_build_array(file_pickcode_json,file_sha1_json,asset_details_json) FROM media_metadata WHERE tmdb_id='film'").fetch_one(&pool).await.unwrap();
    assert_eq!(pending[0], json!([null]));
    assert_eq!(pending[2][0]["fingerprint_pending"], true);

    // A concurrent identity revision during a cache API refresh must stop the write.
    media(&pool, &path, json!("101"), json!("ORIGINAL-SHA"), false).await;
    cache(&pool, None, Some("CACHE-SHA")).await;
    *requests.mutation.lock().await = Some(pool.clone());
    let result = run(&state).await;
    assert_eq!(result["state"], "failed", "{result}");
    let pending: Value = sqlx::query_scalar("SELECT jsonb_build_array(file_sha1_json,asset_details_json) FROM media_metadata WHERE tmdb_id='film'").fetch_one(&pool).await.unwrap();
    assert_eq!(pending[0], json!(["ORIGINAL-SHA"]));
    assert_eq!(pending[1][0]["fingerprint_pending"], true);
    assert_eq!(pending[1][0]["fingerprint_revision"], 8);

    // Cache-only fills use the same revision guard when a concurrent media edit owns the row lock.
    media(&pool, &path, Value::Null, Value::Null, false).await;
    cache(&pool, Some("3"), Some("CACHE-SHA")).await;
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("SELECT tmdb_id FROM media_metadata WHERE tmdb_id='film' FOR UPDATE")
        .execute(&mut *transaction)
        .await
        .unwrap();
    let work = state.clone();
    let task = tokio::spawn(async move { run(&work).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if state.jobs.status().await["phase"] == "缓存补齐指纹" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(jsonb_set(asset_details_json,'{0,fingerprint_pending}','true'),'{0,fingerprint_revision}','8') WHERE tmdb_id='film'")
        .execute(&mut *transaction).await.unwrap();
    transaction.commit().await.unwrap();
    let result = task.await.unwrap();
    assert_eq!(result["state"], "failed", "{result}");
    let pending: Value = sqlx::query_scalar("SELECT jsonb_build_array(file_pickcode_json,asset_details_json) FROM media_metadata WHERE tmdb_id='film'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(pending[0], json!([null]));
    assert_eq!(pending[1][0]["fingerprint_pending"], true);
    assert_eq!(pending[1][0]["fingerprint_revision"], 8);

    state.jobs.shutdown().await;
    server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
