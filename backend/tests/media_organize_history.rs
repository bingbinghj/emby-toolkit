use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use chrono::TimeZone;
use emby_toolkit::{AppState, auth::Session, error::ApiError, media_organize_history as history};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Notify};
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let root = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("admission_history_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
            .await
            .unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::query(&path).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        sqlx::query(
            "INSERT INTO emby_users(id,name,is_administrator) VALUES('admin','History Test',true)",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let host = Router::new()
            .route("/System/Info/Public", get(|| async { Json(json!({"Id":"history-fixture"})) }))
            .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
            .route(
                "/Users/user/Items/{id}",
                get(|| async {
                    (
                        StatusCode::NOT_FOUND,
                        Json(json!({"error":"fixture media unavailable"})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });
        state.config.write().await.as_object_mut().unwrap().extend(json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","emby_webhook_secret":"fixture-shared","emby_webhook_ingest_enabled":true}).as_object().unwrap().clone());
        Self {
            state,
            root,
            schema,
            _directory: directory,
            server,
        }
    }
    fn cookie(&self) -> String {
        format!(
            "etk_session={}",
            self.state.sessions.sign(&Session {
                user_id: "admin".into(),
                name: "History Test".into(),
                setup_mode: false,
                expires: chrono::Utc::now().timestamp() + 60
            })
        )
    }
    async fn request(&self, query: &str, cookie: &str) -> (StatusCode, Value) {
        let response = emby_toolkit::router(self.state.clone())
            .oneshot(
                Request::get(format!("/api/media_organize/history{query}"))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn seed_history(&self, item: Option<&str>, status: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_id,item_json,status) VALUES($1,'emby',$1,$2,$3,$4)")
            .bind(&id).bind(item).bind(json!({"item_type":"Episode","name":"删除范围测试","file_name":"ep200.strm"})).bind(status)
            .execute(&self.state.db).await.unwrap();
        id
    }
    async fn rows(&self) -> Vec<Value> {
        sqlx::query_scalar("SELECT item_json || jsonb_build_object('id',id,'status',status,'message',message,'created_at',created_at,'completed_at',completed_at) FROM media_organize_history ORDER BY id").fetch_all(&self.state.db).await.unwrap()
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

async fn mutate_history_fixture(
    state: &AppState,
    method: &str,
    path: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Value) {
    let response = emby_toolkit::router(state.clone())
        .oneshot(Request::builder().method(method).uri(path)
            .header(header::COOKIE, cookie).header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())).unwrap())
        .await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
}

async fn wait_for_database_blocker(pool: &sqlx::PgPool, blocker: i32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker).fetch_one(pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("request must reach the intended database lock");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; deletion and media sentinels stay local"]
async fn history_delete_removes_only_selected_records_and_preserves_media_state() {
    let f = Fixture::new().await;
    let first = f.seed_history(Some("ep200"), "success").await;
    let second = f.seed_history(Some("ep200"), "warning").await;
    let unbound = f.seed_history(None, "failed").await;
    let retained = f.seed_history(Some("ep201"), "failed").await;
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json) VALUES('200','Episode','[\"ep200\"]','[{\"path\":\"/media/ep200.strm\"}]')")
        .execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO processed_log(item_id,item_name) VALUES('ep200','已有媒体')")
        .execute(&f.state.db).await.unwrap();
    let before: Value = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='200'")
        .fetch_one(&f.state.db).await.unwrap();
    let files = ["ep200.strm", "ep200-mediainfo.json", "ep200.nfo", "ep200-320-10.bif"];
    for name in files { std::fs::write(f._directory.path().join(name), name).unwrap(); }
    let (status, result) = mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history",
        json!({"history_ids":[first,second,unbound,first]}), &f.cookie()).await;
    if status != StatusCode::OK {
        f.close().await;
        panic!("history deletion must return 200, got {status}: {result}");
    }
    assert_eq!(result["deleted_count"], 3);
    let deleted = result["deleted_ids"].as_array().unwrap().iter().map(|id| id.as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(deleted, [&first[..], &second[..], &unbound[..]].into_iter().collect());
    let rows = f.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], retained);
    assert_eq!(sqlx::query_scalar::<_, Value>("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='200'").fetch_one(&f.state.db).await.unwrap(), before);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM processed_log WHERE item_id='ep200'").fetch_one(&f.state.db).await.unwrap(), 1);
    for name in files { assert_eq!(std::fs::read(f._directory.path().join(name)).unwrap(), name.as_bytes()); }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all invalid delete requests remain local"]
async fn history_delete_rejects_nonadmin_invalid_and_missing_ids_without_partial_deletion() {
    let f = Fixture::new().await;
    let id = f.seed_history(Some("ep200"), "failed").await;
    let body = json!({"history_ids":[id]});
    assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", body.clone(), "").await.0, StatusCode::FORBIDDEN);
    sqlx::query("UPDATE emby_users SET is_administrator=false WHERE id='admin'").execute(&f.state.db).await.unwrap();
    assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", body, &f.cookie()).await.0, StatusCode::FORBIDDEN);
    sqlx::query("UPDATE emby_users SET is_administrator=true WHERE id='admin'").execute(&f.state.db).await.unwrap();
    for body in [json!({}), json!({"history_ids":[]}), json!({"history_ids":[""]}), json!({"history_ids":[12]}), json!({"history_ids":["not-a-uuid"]}), json!({"history_ids":vec![id.clone();1001]})] {
        assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", body, &f.cookie()).await.0, StatusCode::BAD_REQUEST);
    }
    let (status, _) = mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history",
        json!({"history_ids":[id,uuid::Uuid::new_v4().to_string()]}), &f.cookie()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(f.rows().await.len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; unfinished queue protection remains local"]
async fn history_delete_protects_processing_and_resumable_queue_references_atomically() {
    let f = Fixture::new().await;
    let safe = f.seed_history(Some("ep199"), "success").await;
    let protected = f.seed_history(Some("ep200"), "processing").await;
    let body = json!({"history_ids":[safe,protected]});
    assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", body.clone(), &f.cookie()).await.0, StatusCode::CONFLICT);
    sqlx::query("UPDATE media_organize_history SET status='failed' WHERE id=$1").bind(&protected).execute(&f.state.db).await.unwrap();
    let queue = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,history_ids,operator_id) VALUES($1,'ep200','reprocess','{}',$2,'admin')")
        .bind(&queue).bind(json!([protected])).execute(&f.state.db).await.unwrap();
    let before = f.rows().await;
    for state in ["queued", "running", "interrupted"] {
        sqlx::query("UPDATE media_organize_queue SET status=$1 WHERE id=$2").bind(state).bind(&queue).execute(&f.state.db).await.unwrap();
        assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", body.clone(), &f.cookie()).await.0, StatusCode::CONFLICT);
        assert_eq!(f.rows().await, before);
    }
    sqlx::query("UPDATE media_organize_queue SET status='failed' WHERE id=$1").bind(&queue).execute(&f.state.db).await.unwrap();
    assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", body, &f.cookie()).await.0, StatusCode::OK);
    let (status, _) = mutate_history_fixture(&f.state, "POST", &format!("/api/media_organize/queue/{queue}/retry"), json!({}), &f.cookie()).await;
    assert_eq!(status, StatusCode::CONFLICT, "retry must not discard deleted history IDs");
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media_organize_queue").fetch_one(&f.state.db).await.unwrap(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; obsolete attempts and retained results stay local"]
async fn history_delete_allows_an_interrupted_attempt_superseded_by_a_completed_retry() {
    let f = Fixture::new().await;
    let id = f.seed_history(Some("ep200"), "success").await;
    for status in ["interrupted", "completed"] {
        sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,history_ids,operator_id,status,result) VALUES($1,'ep200','reprocess','{}',$2,'admin',$3,$4)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(json!([id])).bind(status).bind(json!({"original_status":status}))
            .execute(&f.state.db).await.unwrap();
    }
    let before: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q ORDER BY queue_order").fetch_all(&f.state.db).await.unwrap();
    let (status, result) = mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", json!({"history_ids":[id]}), &f.cookie()).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["deleted_count"], 1);
    assert_eq!(result["deleted_ids"], json!([id]));
    assert!(f.rows().await.is_empty());
    assert_eq!(sqlx::query_scalar::<_, Value>("SELECT to_jsonb(q) FROM media_organize_queue q ORDER BY queue_order").fetch_all(&f.state.db).await.unwrap(), before);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; deterministic delete/enqueue race stays local"]
async fn history_deleted_after_selection_is_rejected_inside_the_enqueue_transaction() {
    let f = Fixture::new().await;
    let item = format!("ep_{}", uuid::Uuid::new_v4().simple());
    let id = f.seed_history(Some(&item), "failed").await;
    let mut gate = f.state.db.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *gate).await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(format!("emby-toolkit:organize-queue:{item}:reprocess")).execute(&mut *gate).await.unwrap();
    let state = f.state.clone();
    let cookie = f.cookie();
    let body = json!({"item_ids":[item],"history_ids":[id]});
    let request = tokio::spawn(async move { mutate_history_fixture(&state, "POST", "/api/actions/reorganize", body, &cookie).await });
    wait_for_database_blocker(&f.state.db, pid).await;
    assert_eq!(mutate_history_fixture(&f.state, "DELETE", "/api/media_organize/history", json!({"history_ids":[id]}), &f.cookie()).await.0, StatusCode::OK);
    gate.commit().await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(3), request).await.unwrap().unwrap().0, StatusCode::CONFLICT);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media_organize_queue").fetch_one(&f.state.db).await.unwrap(), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; deterministic enqueue/delete race stays local"]
async fn history_delete_waits_for_an_enqueue_transaction_then_preserves_its_active_reference() {
    let f = Fixture::new().await;
    let item = format!("ep_{}", uuid::Uuid::new_v4().simple());
    let id = f.seed_history(Some(&item), "failed").await;
    let label = format!("history-delete-insert-{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE FUNCTION hold_queue_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('{label}')); RETURN NEW; END $$"))
        .execute(&f.state.db).await.unwrap();
    sqlx::query("CREATE TRIGGER hold_queue_insert BEFORE INSERT ON media_organize_queue FOR EACH ROW EXECUTE FUNCTION hold_queue_insert()")
        .execute(&f.state.db).await.unwrap();
    let mut gate = f.state.db.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *gate).await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(label).execute(&mut *gate).await.unwrap();
    let state = f.state.clone();
    let cookie = f.cookie();
    let body = json!({"item_ids":[item],"history_ids":[id]});
    let enqueue = tokio::spawn(async move { mutate_history_fixture(&state, "POST", "/api/actions/reorganize", body, &cookie).await });
    wait_for_database_blocker(&f.state.db, pid).await;
    let enqueue_pid: i32 = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'INSERT INTO media_organize_queue%' LIMIT 1")
        .bind(pid).fetch_one(&f.state.db).await.unwrap();
    let state = f.state.clone();
    let cookie = f.cookie();
    let deletion = tokio::spawn(async move { mutate_history_fixture(&state, "DELETE", "/api/media_organize/history", json!({"history_ids":[id]}), &cookie).await });
    wait_for_database_blocker(&f.state.db, enqueue_pid).await;
    gate.commit().await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(3), enqueue).await.unwrap().unwrap().0, StatusCode::ACCEPTED);
    assert_eq!(tokio::time::timeout(Duration::from_secs(3), deletion).await.unwrap().unwrap().0, StatusCode::CONFLICT);
    assert_eq!(f.rows().await.len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; queue cancellation lock ordering stays local"]
async fn history_enqueue_keeps_queue_then_history_lock_order_when_cancellation_runs() {
    let f = Fixture::new().await;
    let item = format!("ep_{}", uuid::Uuid::new_v4().simple());
    let id = f.seed_history(Some(&item), "failed").await;
    let queue = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,history_ids,operator_id) VALUES($1,$2,'reprocess','{}',$3,'admin')")
        .bind(&queue).bind(&item).bind(json!([id])).execute(&f.state.db).await.unwrap();
    let mut cancel = f.state.db.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *cancel).await.unwrap();
    sqlx::query("UPDATE media_organize_queue SET status='cancelled' WHERE id=$1").bind(queue).execute(&mut *cancel).await.unwrap();
    let state = f.state.clone();
    let cookie = f.cookie();
    let body = json!({"item_ids":[item],"history_ids":[id]});
    let enqueue = tokio::spawn(async move { mutate_history_fixture(&state, "POST", "/api/actions/reorganize", body, &cookie).await });
    wait_for_database_blocker(&f.state.db, pid).await;
    tokio::time::timeout(Duration::from_secs(2), sqlx::query("UPDATE media_organize_history SET message='取消已完成' WHERE id=$1").bind(id).execute(&mut *cancel))
        .await.expect("enqueue must not lock history before the existing queue").unwrap();
    cancel.commit().await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(3), enqueue).await.unwrap().unwrap().0, StatusCode::ACCEPTED);
    f.close().await;
}
fn episode(id: &str) -> Value {
    json!({"Id":id,"Type":"Episode","Name":format!("本集-{id}"),"SeriesId":"series","SeriesName":"入库测试剧","ParentIndexNumber":1,"IndexNumber":2,"ImageTags":{"Primary":"frame"},"SeriesPrimaryImageTag":"series-poster","Path":format!("/media/{id}.strm"),"AccessToken":"must-not-store","ProviderIds":{"Tmdb":"private-extra"}})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn strm_names_come_from_the_admitted_source_and_legacy_job_snapshot() {
    let f = Fixture::new().await;
    let mut item = episode("ep200");
    let path = "/media/ep200.strm";
    item["MediaSources"] = json!([
        {"ItemId":"another","Path":"/media/wrong.mkv"},
        {"ItemId":"ep200","Path":"https://media.test/watch/ep200.mp4?token=private"}
    ]);
    let ids = history::begin_emby(&f.state, "item.add", &item, path, None).await.unwrap();
    let mut event = json!({"files":[path],"media_type":"tv"});
    let mp = history::begin_moviepilot(&f.state, &event).await.unwrap();
    event["history_ids"] = json!(mp);
    history::bind_item(&f.state, &event, path, &item).await.unwrap();
    assert!(f.rows().await.iter().all(|row| row["file_name"] == "ep200.mp4"));

    // Re-create the two older receipts. Only the original job names one of them.
    sqlx::query("UPDATE media_organize_history SET item_json=jsonb_set(item_json,'{file_name}',to_jsonb('ep200.strm'::text))")
        .execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json) VALUES('job',$1,'{}')")
        .bind(json!({"kind":"webhook","payload":{"event":event}})).execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO durable_job_steps(job_id,step_key,value_json) VALUES('job','plans:moviepilot_file:sample',$1)")
        .bind(json!({"identity":{"id":"ep200","path":path,"source":"https://media.test/watch/old%20copy.mkv?token=private"}}))
        .execute(&f.state.db).await.unwrap();
    sqlx::query("DELETE FROM app_settings WHERE setting_key='history_source_filenames_v1'").execute(&f.state.db).await.unwrap();
    emby_toolkit::db::initialize(&f.state.db).await.unwrap();
    let rows = f.rows().await;
    assert!(rows.iter().any(|r| r["id"] == ids[0] && r["file_name"] == "ep200.strm"));
    assert!(rows.iter().any(|r| r["id"] == mp[0] && r["file_name"] == "old copy.mkv" && r["source_path"] == path));
    assert!(!rows.iter().any(|r| r.to_string().contains("private")));
    f.close().await;
}

async fn finish_moviepilot_with_plans(
    f: &Fixture,
    event: &Value,
    plans: Value,
    result: emby_toolkit::error::Result<Value>,
    persistent: bool,
) {
    let state = f.state.clone();
    let event = event.clone();
    let payload = json!({"event":event});
    let execute = move |context: emby_toolkit::jobs::JobContext| async move {
        for (key, plan) in plans.as_object().unwrap() {
            context.save_plan(key, plan).await?;
        }
        history::finish_moviepilot(&state, &event, &context, &result).await?;
        Ok(json!({"message":"历史范围验证完成"}))
    };
    let id = if persistent {
        f.state.jobs.submit_persistent("历史范围验证", emby_toolkit::jobs::JobSpec::new("webhook", payload), execute).await.unwrap()
    } else {
        f.state.jobs.submit("历史范围验证", execute).await.unwrap()
    };
    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let task = f.state.jobs.result(&id).await.unwrap();
            if task["is_running"] == false { return task; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    assert_eq!(finished["state"], "completed", "{finished}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_expanded_notice_records_only_the_handled_episode_200() {
    let f = Fixture::new().await;
    let files = (1..=203).map(|number| format!("/media/ep{number}.strm")).collect::<Vec<_>>();
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":files});
    let ids = history::begin_moviepilot(&f.state, &event).await.unwrap();
    event["history_ids"] = json!(ids);
    let mut item = episode("ep200");
    item["IndexNumber"] = json!(200);
    history::bind_item(&f.state, &event, "/media/ep200.strm", &item).await.unwrap();
    let mut plans = json!({"moviepilot_file:200":{"identity":{"path":"/media/ep200.strm"}}});
    for number in 1..=203 {
        plans[format!("moviepilot_attempt:{number}")] = json!({"path":format!("/media/ep{number}.strm"),"skipped":number!=200});
    }
    finish_moviepilot_with_plans(&f, &event, plans, Ok(json!({"updated":3,"awaiting_emby":true})), false).await;
    let rows = f.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["item_id"], "ep200");
    assert_eq!(rows[0]["episode_number"], 200);
    assert_eq!(rows[0]["status"], "processing");
    let (_, page) = f.request("", &f.cookie()).await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["episode_number"], 200);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_pure_noop_adds_no_history_and_keeps_previous_finished_admissions() {
    let f = Fixture::new().await;
    let item = episode("ep200");
    let previous = history::begin_emby(&f.state, "item.add", &item, "/media/ep200.strm", None).await.unwrap();
    history::finish(&f.state, &json!({"source":"emby","history_ids":previous}), &Ok(json!({"updated":1}))).await.unwrap();
    let before = f.rows().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep200.strm","/media/ep1.strm"]});
    event["history_ids"] = json!(history::begin_moviepilot(&f.state, &event).await.unwrap());
    let plans = json!({"moviepilot_attempt:200":{"path":"/media/ep200.strm","skipped":true},
        "moviepilot_attempt:1":{"path":"/media/ep1.strm","skipped":true}});
    finish_moviepilot_with_plans(&f, &event, plans, Ok(json!({"updated":0})), false).await;
    assert_eq!(f.rows().await, before);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_failure_does_not_fail_noop_or_unvisited_notice_paths() {
    let f = Fixture::new().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep1.strm","/media/ep200.strm","/media/ep201.strm","/media/ep202.strm"]});
    event["history_ids"] = json!(history::begin_moviepilot(&f.state, &event).await.unwrap());
    history::bind_item(&f.state, &event, "/media/ep200.strm", &episode("ep200")).await.unwrap();
    let plans = json!({"moviepilot_attempt:1":{"path":"/media/ep1.strm","skipped":true},
        "moviepilot_file:200":{"identity":{"path":"/media/ep200.strm"}},
        "moviepilot_attempt:201":{"path":"/media/ep201.strm","skipped":false}});
    finish_moviepilot_with_plans(&f, &event, plans, Err(ApiError::bad("入库项目查找失败")), false).await;
    let rows = f.rows().await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["status"] == "failed" && row["message"] == "入库项目查找失败"));
    let names = rows.iter().map(|row| row["file_name"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names, ["ep200.strm", "ep201.strm"].into_iter().collect());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_persisted_original_file_plan_keeps_the_same_admission_without_new_attempts() {
    let f = Fixture::new().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep200.strm","/media/ep1.strm"]});
    let ids = history::begin_moviepilot(&f.state, &event).await.unwrap();
    event["history_ids"] = json!(ids);
    history::bind_item(&f.state, &event, "/media/ep200.strm", &episode("ep200")).await.unwrap();
    let plans = json!({"moviepilot_file:200":{"identity":{"path":"/media/ep200.strm"}},
        "moviepilot_attempt:1":{"path":"/media/ep1.strm","skipped":true}});
    finish_moviepilot_with_plans(&f, &event, plans, Ok(json!({"updated":1,"awaiting_emby":false})), true).await;
    let rows = f.rows().await;
    assert_eq!(rows.len(), 1);
    assert!(ids.contains(&rows[0]["id"].as_str().unwrap().to_string()));
    assert_eq!(rows[0]["item_id"], "ep200");
    assert_eq!(rows[0]["status"], "success");
    let durable: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_job_steps WHERE step_key='plans:moviepilot_file:200'").fetch_one(&f.state.db).await.unwrap();
    assert_eq!(durable, 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_scope_pruning_rolls_back_with_a_failed_history_finish() {
    let f = Fixture::new().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep200.strm","/media/ep1.strm"]});
    event["history_ids"] = json!(history::begin_moviepilot(&f.state, &event).await.unwrap());
    let before = f.rows().await;
    sqlx::query("CREATE FUNCTION reject_scoped_finish() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture history finish rejected'; END $$")
        .execute(&f.state.db).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_scoped_finish BEFORE UPDATE ON media_organize_history FOR EACH ROW EXECUTE FUNCTION reject_scoped_finish()")
        .execute(&f.state.db).await.unwrap();
    let state = f.state.clone();
    let task = f.state.jobs.submit("历史事务验证", move |context| async move {
        context.save_plan("moviepilot_file:200", &json!({"identity":{"path":"/media/ep200.strm"}})).await?;
        let result = Ok(json!({"updated":1,"awaiting_emby":false}));
        assert!(history::finish_moviepilot(&state, &event, &context, &result).await.is_err());
        let after: Vec<Value> = sqlx::query_scalar("SELECT item_json || jsonb_build_object('id',id,'status',status,'message',message,'created_at',created_at,'completed_at',completed_at) FROM media_organize_history ORDER BY id")
            .fetch_all(&state.db).await?;
        assert_eq!(after, before, "scope pruning must roll back with the final status update");
        sqlx::query("DROP TRIGGER reject_scoped_finish ON media_organize_history").execute(&state.db).await?;
        history::finish_moviepilot(&state, &event, &context, &result).await?;
        Ok(json!({"message":"事务回滚及重试验证完成"}))
    }).await.unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = f.state.jobs.result(&task).await.unwrap();
            if result["is_running"] == false { return result; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    assert_eq!(finished["state"], "completed", "{finished}");
    let rows = f.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "success");
    assert_eq!(rows[0]["file_name"], "ep200.strm");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_scoped_finish_preserves_a_pending_emby_admission_even_if_ids_are_mixed() {
    let f = Fixture::new().await;
    let foreign = history::begin_emby(&f.state, "item.add", &episode("ep200"), "/media/ep200.strm", None).await.unwrap();
    let before = f.rows().await[0].clone();
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep200.strm"]});
    event["history_ids"] = json!(history::begin_moviepilot(&f.state, &event).await.unwrap());
    event["history_ids"].as_array_mut().unwrap().push(json!(foreign[0]));
    let plans = json!({"moviepilot_file:200":{"identity":{"path":"/media/ep200.strm"}}});
    finish_moviepilot_with_plans(&f, &event, plans, Ok(json!({"updated":1,"awaiting_emby":false})), false).await;
    let rows = f.rows().await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().find(|row| row["id"] == foreign[0]).unwrap(), &before);
    assert_eq!(rows.iter().filter(|row| row["status"] == "success").count(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn admissions_keep_exact_episodes_and_exclude_metadata_updates_and_parent_rows() {
    let f = Fixture::new().await;
    let item = episode("ep1");
    let ids = history::begin_emby(
        &f.state,
        "item.add",
        &item,
        "/media/ep1.strm",
        Some(&json!({"Name":"电视库"})),
    )
    .await
    .unwrap();
    assert_eq!(ids.len(), 1);
    assert!(
        history::begin_emby(&f.state, "metadata.update", &item, "/media/ep1.strm", None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        history::begin_emby(
            &f.state,
            "library.new",
            &json!({"Id":"series","Type":"Series"}),
            "/media",
            None
        )
        .await
        .unwrap()
        .is_empty()
    );
    let (_, value) = f.request("", &f.cookie()).await;
    assert_eq!(value["total"], 1);
    assert_eq!(value["items"][0]["item_id"], "ep1");
    assert_eq!(value["items"][0]["series_id"], "series");
    assert_eq!(value["items"][0]["library_name"], "电视库");
    assert!(!value.to_string().contains("must-not-store"));
    assert!(!value.to_string().contains("private-extra"));
    assert!(value["items"][0].get("source_identity").is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no real Emby writes"]
async fn monthly_admissions_follow_mp_success_identity_and_calendar_rules_without_double_counting_webhook_sources()
 {
    let f = Fixture::new().await;
    let at = |year, month, day, hour| {
        chrono::Local
            .with_ymd_and_hms(year, month, day, hour, 0, 0)
            .earliest()
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let now = at(2026, 11, 15, 12).with_timezone(&chrono::Local);
    let empty = history::monthly_statistics(&f.state, now).await.unwrap();
    assert_eq!(empty["movie_count"], 0);
    assert_eq!(empty["episode_count"], 0);
    assert_eq!(empty["month"]["state"], "recording");
    assert_eq!(empty["month"]["source"], "etk-history");
    let movie = |name| json!({"item_type":"Movie","name":name,"production_year":2026,"media_source":"tmdb","media_id":"700"});
    let episode = |id, name, year| json!({"item_type":"Episode","series_id":id,"series_name":name,"production_year":year});
    for (path, source, status, item, time) in [
        (
            "movie-path-a",
            "emby",
            "success",
            movie("同一电影"),
            at(2026, 11, 1, 0),
        ),
        (
            "movie-path-b",
            "emby",
            "success",
            movie("同一电影"),
            at(2026, 11, 5, 0),
        ),
        (
            "movie-path-c",
            "emby",
            "warning",
            movie("另一电影"),
            at(2026, 11, 5, 0),
        ),
        (
            "episode-path-a",
            "moviepilot",
            "success",
            episode("series-a", "同一剧", 2026),
            at(2026, 11, 1, 0),
        ),
        (
            "episode-path-a",
            "emby",
            "success",
            episode("series-a", "同一剧", 2026),
            at(2026, 11, 1, 1),
        ),
        (
            "episode-path-b",
            "emby",
            "warning",
            episode("series-a", "同一剧", 2027),
            at(2026, 11, 5, 0),
        ),
        (
            "episode-path-c",
            "emby",
            "success",
            episode("", "另一剧", 2026),
            at(2026, 11, 5, 0),
        ),
        (
            "failed-path",
            "emby",
            "failed",
            movie("失败电影"),
            at(2026, 11, 5, 0),
        ),
        (
            "pending-path",
            "emby",
            "processing",
            episode("series-pending", "处理中剧", 2026),
            at(2026, 11, 5, 0),
        ),
        (
            "prior-month",
            "emby",
            "success",
            movie("上月电影"),
            at(2026, 10, 31, 23),
        ),
        (
            "prior-year",
            "emby",
            "success",
            movie("去年同月电影"),
            at(2025, 11, 5, 0),
        ),
        (
            "next-month",
            "emby",
            "success",
            movie("下月电影"),
            at(2026, 12, 1, 0),
        ),
        (
            "track-a",
            "emby",
            "success",
            json!({"item_type":"Audio"}),
            at(2026, 11, 5, 0),
        ),
        (
            "track-b",
            "emby",
            "success",
            json!({"item_type":"Audio"}),
            at(2026, 11, 5, 0),
        ),
    ] {
        sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_json,status,created_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(source).bind(path).bind(item).bind(status).bind(time).execute(&f.state.db).await.unwrap();
    }
    sqlx::query("UPDATE app_settings SET value_json=$1 WHERE setting_key='media_organize_history_started_at'")
        .bind(json!(at(2026, 11, 2, 0).to_rfc3339())).execute(&f.state.db).await.unwrap();
    let monthly = history::monthly_statistics(&f.state, now).await.unwrap();
    assert_eq!(monthly["movie_count"], 2);
    assert_eq!(
        monthly["tv_count"], 2,
        "episode years must not split an already identified series"
    );
    assert_eq!(
        monthly["episode_count"], 3,
        "MP preparation and Emby finalization of the same file count once"
    );
    assert!(
        monthly.get("music_count").is_none(),
        "music admissions are outside ETK dashboard statistics"
    );
    assert_eq!(monthly["month"]["period"], "2026-11");
    assert_eq!(monthly["month"]["partial"], true);
    sqlx::query("UPDATE app_settings SET value_json=$1 WHERE setting_key='media_organize_history_started_at'")
        .bind(json!(at(2026, 10, 1, 0).to_rfc3339())).execute(&f.state.db).await.unwrap();
    assert_eq!(
        history::monthly_statistics(&f.state, now).await.unwrap()["month"]["partial"],
        false
    );
    f.state.db.close().await;
    assert!(
        history::monthly_statistics(&f.state, now).await.is_err(),
        "an unavailable history database cannot be interpreted as no monthly admissions"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn history_pages_and_filters_real_records_without_querying_the_library() {
    let f = Fixture::new().await;
    for (index, status) in ["success", "warning", "failed"].iter().enumerate() {
        let item = episode(&format!("ep{index}"));
        let ids = history::begin_emby(
            &f.state,
            "item.add",
            &item,
            &format!("/media/ep{index}.strm"),
            None,
        )
        .await
        .unwrap();
        let result = match *status {
            "failed" => Err(ApiError::bad("验收失败 password=should-hide")),
            "warning" => Ok(json!({"warnings":1,"message":"完成有提醒"})),
            _ => Ok(json!({"updated":1,"message":"完成"})),
        };
        history::finish(
            &f.state,
            &json!({"source":"emby","history_ids":ids}),
            &result,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE media_organize_history SET created_at=TIMESTAMPTZ '2026-11-01 00:00:00Z' + ($1 * INTERVAL '1 day') WHERE id=ANY($2)").bind(index as i32).bind(ids).execute(&f.state.db).await.unwrap();
    }
    let (_, page) = f.request("?page=2&page_size=1", &f.cookie()).await;
    assert_eq!(page["total"], 3);
    assert_eq!(page["items"][0]["item_id"], "ep1");
    let (_, success) = f.request("?status=success", &f.cookie()).await;
    assert_eq!(success["total"], 1);
    assert_eq!(success["items"][0]["status"], "success");
    let (_, failed) = f.request("?status=failed&search=ep2", &f.cookie()).await;
    assert_eq!(failed["total"], 1);
    assert!(!failed.to_string().contains("should-hide"));
    assert!(failed["tracking_since"].as_str().is_some());
    assert_eq!(failed["legacy_records_available"], false);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn history_supports_mp_page_sizes_without_losing_the_last_records() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_id,item_json,status,created_at) SELECT '00000000-0000-4000-8000-' || lpad(n::text,12,'0'),'emby','scope-' || n,'ep-' || n,jsonb_build_object('name','分页记录 ' || n),'success',TIMESTAMPTZ '2026-11-01 00:00:00Z' + n * INTERVAL '1 second' FROM generate_series(1,1003) AS numbers(n)")
        .execute(&f.state.db).await.unwrap();
    let cookie = f.cookie();
    let (status, default) = f.request("", &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(default["page_size"], 50);
    assert_eq!(default["items"].as_array().unwrap().len(), 50);
    for size in [25, 50, 100, 500, 1000] {
        let (status, page) = f.request(&format!("?page_size={size}"), &cookie).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 1003);
        assert_eq!(page["page"], 1);
        assert_eq!(page["page_size"], size);
        let rows = page["items"].as_array().unwrap();
        assert_eq!(rows.len(), size as usize);
        assert_eq!(rows.first().unwrap()["item_id"], "ep-1003");
        assert_eq!(
            rows.last().unwrap()["item_id"],
            format!("ep-{}", 1004 - size)
        );
    }
    let (_, first) = f.request("?page_size=1000", &cookie).await;
    let (_, last) = f.request("?page=2&page_size=1000", &cookie).await;
    assert_eq!(last["total"], 1003);
    assert_eq!(last["items"].as_array().unwrap().len(), 3);
    assert_eq!(last["items"][0]["item_id"], "ep-3");
    assert_eq!(last["items"][2]["item_id"], "ep-1");
    let ids = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(last["items"].as_array().unwrap())
        .map(|row| row["id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 1003);
    let record_ids = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let media_ids = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["item_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let selected = json!({"history_ids":record_ids});
    assert_eq!(
        history::selected_history(&f.state, &selected, &media_ids)
            .await
            .unwrap(),
        record_ids
    );
    let mut too_many = record_ids.clone();
    too_many.push(last["items"][0]["id"].as_str().unwrap().to_owned());
    assert!(
        history::selected_history(&f.state, &json!({"history_ids":too_many}), &media_ids)
            .await
            .is_err()
    );
    let (_, beyond) = f.request("?page=3&page_size=1000", &cookie).await;
    assert_eq!(beyond["total"], 1003);
    assert!(beyond["items"].as_array().unwrap().is_empty());
    let (_, bounded) = f.request("?page_size=1001", &cookie).await;
    assert_eq!(bounded["page_size"], 1000);
    assert_eq!(bounded["items"].as_array().unwrap().len(), 1000);
    assert_eq!(
        f.request("?page=9223372036854775807&page_size=1000", &cookie)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn moviepilot_records_bind_only_the_explicit_path_and_keep_urls_private() {
    let f = Fixture::new().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","title":"新剧","files":["/media/a.strm","/media/b.strm","https://user:pass@example.invalid/c.strm?private_signed_value=secret#fragment"]});
    let ids = history::begin_moviepilot(&f.state, &event).await.unwrap();
    event["history_ids"] = json!(ids);
    history::bind_item(&f.state, &event, "/media/another.strm", &episode("wrong"))
        .await
        .unwrap();
    assert!(f.rows().await.iter().all(|row| row["item_id"].is_null()));
    history::bind_item(&f.state, &event, "/media/a.strm", &episode("ep1"))
        .await
        .unwrap();
    let (_, value) = f.request("", &f.cookie()).await;
    assert_eq!(value["total"], 3);
    assert_eq!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["item_id"] == "ep1")
            .count(),
        1
    );
    assert!(!value.to_string().contains("private_signed_value"));
    assert!(!value.to_string().contains("user:pass"));
    history::finish(&f.state,&event,&Err(ApiError::bad("失败源 https://url-user:url-pass@example.invalid/c.strm?sig=private-signature#fragment；请核对"))).await.unwrap();
    let (_, failed) = f.request("", &f.cookie()).await;
    assert!(!failed.to_string().contains("url-pass"));
    assert!(!failed.to_string().contains("private-signature"));
    assert!(
        failed["items"][0]["message"]
            .as_str()
            .unwrap()
            .contains("https://example.invalid/c.strm")
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn deferred_moviepilot_success_waits_for_the_exact_episode_finalization() {
    let f = Fixture::new().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep1.strm","/media/ep2.strm"]});
    let ids = history::begin_moviepilot(&f.state, &event).await.unwrap();
    event["history_ids"] = json!(ids);
    for id in ["ep1", "ep2"] {
        history::bind_item(&f.state, &event, &format!("/media/{id}.strm"), &episode(id))
            .await
            .unwrap();
    }
    history::finish(
        &f.state,
        &event,
        &Ok(json!({"updated":2,"message":"预增强完成"})),
    )
    .await
    .unwrap();
    assert!(
        f.rows()
            .await
            .iter()
            .all(|row| row["status"] == "processing" && row["completed_at"].is_null())
    );
    let emby_ids = history::begin_emby(
        &f.state,
        "library.new",
        &episode("ep1"),
        "/media/ep1.strm",
        None,
    )
    .await
    .unwrap();
    history::finish(
        &f.state,
        &json!({"source":"emby","history_ids":emby_ids}),
        &Ok(json!({"updated":1})),
    )
    .await
    .unwrap();
    let rows = f.rows().await;
    assert!(
        rows.iter()
            .filter(|row| row["item_id"] == "ep1")
            .all(|row| row["status"] == "success")
    );
    assert!(
        rows.iter()
            .filter(|row| row["item_id"] == "ep2")
            .all(|row| row["status"] == "processing")
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn preparation_warnings_survive_a_successful_emby_finalization() {
    let f = Fixture::new().await;
    let mut event = json!({"source":"moviepilot","media_type":"tv","files":["/media/ep1.strm"]});
    let ids = history::begin_moviepilot(&f.state, &event).await.unwrap();
    event["history_ids"] = json!(ids);
    history::bind_item(&f.state, &event, "/media/ep1.strm", &episode("ep1"))
        .await
        .unwrap();
    history::finish(&f.state,&event,&Ok(json!({"updated":1,"warning_details":["预增强字幕有提醒"],"message":"预增强字幕有提醒"}))).await.unwrap();
    assert_eq!(f.rows().await[0]["status"], "processing");
    let emby_ids = history::begin_emby(
        &f.state,
        "item.add",
        &episode("ep1"),
        "/media/ep1.strm",
        None,
    )
    .await
    .unwrap();
    history::finish(
        &f.state,
        &json!({"source":"emby","history_ids":emby_ids}),
        &Ok(json!({"updated":1,"message":"入库验收完成"})),
    )
    .await
    .unwrap();
    let (_, value) = f.request("?status=warning", &f.cookie()).await;
    assert_eq!(value["total"], 1);
    assert!(
        value["items"][0]["message"]
            .as_str()
            .unwrap()
            .contains("预增强字幕有提醒")
    );
    assert!(value["items"][0].get("_preparation_warning").is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn history_requires_admin_and_rejects_invalid_filters_without_backfill() {
    let f = Fixture::new().await;
    assert_eq!(f.request("", "").await.0, StatusCode::FORBIDDEN);
    for query in [
        "?status=missing",
        "?page=not-a-number",
        "?page=9223372036854775807&page_size=100",
    ] {
        assert_eq!(
            f.request(query, &f.cookie()).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    sqlx::query("INSERT INTO processed_log(item_id,item_name) VALUES('old','旧处理缓存')")
        .execute(&f.state.db)
        .await
        .unwrap();
    let (_, empty) = f.request("", &f.cookie()).await;
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["items"], json!([]));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn reorganizing_history_checks_scope_and_preserves_admission_time() {
    let f = Fixture::new().await;
    let ids = history::begin_emby(
        &f.state,
        "item.add",
        &episode("ep1"),
        "/media/ep1.strm",
        None,
    )
    .await
    .unwrap();
    history::finish(
        &f.state,
        &json!({"source":"emby","history_ids":ids}),
        &Err(ApiError::bad("旧失败")),
    )
    .await
    .unwrap();
    let original = f.rows().await[0]["created_at"].clone();
    let body = json!({"history_ids":ids});
    assert!(
        history::selected_history(&f.state, &body, &["another".into()])
            .await
            .is_err()
    );
    assert!(
        history::selected_history(&f.state, &body, &["ep1".into(), "another".into()])
            .await
            .is_err()
    );
    assert_eq!(
        history::selected_history(&f.state, &body, &["ep1".into()])
            .await
            .unwrap(),
        ids
    );
    history::begin_reorganize(&f.state, &ids, "ep1")
        .await
        .unwrap();
    assert!(
        history::selected_history(&f.state, &body, &["ep1".into()])
            .await
            .is_err()
    );
    history::finish_reorganize(
        &f.state,
        &ids,
        "ep1",
        &Ok(json!({"warnings":1,"message":"重新整理完成有提醒"})),
    )
    .await
    .unwrap();
    let row = &f.rows().await[0];
    assert_eq!(row["created_at"], original);
    assert_eq!(row["status"], "warning");
    assert!(row["completed_at"].is_string());
    assert!(
        history::selected_history(&f.state, &json!({}), &["ep1".into()])
            .await
            .unwrap()
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn service_restart_marks_only_unfinished_admissions_for_retry_and_keeps_their_original_time()
{
    let f = Fixture::new().await;
    let ids = history::begin_emby(
        &f.state,
        "item.add",
        &episode("ep1"),
        "/media/ep1.strm",
        None,
    )
    .await
    .unwrap();
    let original = f.rows().await[0]["created_at"].clone();
    history::interrupt_unfinished(&f.state.db).await.unwrap();
    let row = &f.rows().await[0];
    assert_eq!(row["status"], "failed");
    assert_eq!(row["created_at"], original);
    let message = row["message"].as_str().unwrap();
    assert!(!message.trim().is_empty() && message.contains("核对") && message.contains("重新整理"));
    assert_eq!(
        history::selected_history(&f.state, &json!({"history_ids":ids}), &["ep1".into()])
            .await
            .unwrap(),
        ids
    );
    history::begin_reorganize(&f.state, &ids, "ep1")
        .await
        .unwrap();
    history::finish_reorganize(
        &f.state,
        &ids,
        "ep1",
        &Ok(json!({"warning_details":["旧章节有提醒"],"message":"完成有提醒"})),
    )
    .await
    .unwrap();
    assert_eq!(f.rows().await[0]["status"], "warning");
    history::interrupt_unfinished(&f.state.db).await.unwrap();
    assert_eq!(f.rows().await[0]["status"], "warning");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn authenticated_episode_events_preserve_both_history_rows_through_queue_merge_and_failure() {
    let f = Fixture::new().await;
    let blocker = f
        .state
        .jobs
        .submit("hold", |context| async move {
            context.cancelled().await;
            context.checkpoint().await?;
            Ok(Value::Null)
        })
        .await
        .unwrap();
    let app = emby_toolkit::webhook::router().with_state(f.state.clone());
    for id in ["ep1", "ep2"] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/webhook/emby?token=fixture-shared")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"Event":"item.add","Item":episode(id)}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
    assert_eq!(f.rows().await.len(), 2);
    f.state.jobs.control(Some(&blocker), "stop").await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if f.rows().await.iter().all(|row| row["status"] == "failed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let (_, value) = f.request("", &f.cookie()).await;
    assert_eq!(value["total"], 2);
    assert!(
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["item_id"] == "ep1" || row["item_id"] == "ep2")
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; notification server is localhost"]
async fn failed_admissions_keep_real_reasons_and_notify_once_without_blocking_completion() {
    let mut f = Fixture::new().await;
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let messages = received.clone();
    let request_started = started.clone();
    let response_release = release.clone();
    let sender = Router::new().route(
        "/botfixture/sendMessage",
        post(move |Json(body): Json<Value>| {
            let messages = messages.clone();
            let request_started = request_started.clone();
            let response_release = response_release.clone();
            async move {
                messages.lock().await.push(body);
                request_started.notify_one();
                response_release.notified().await;
                Json(json!({"ok":true}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.state.notifications = Arc::new(emby_toolkit::notifications::Service::with_api_base(
        format!("http://{}", listener.local_addr().unwrap()),
    ));
    let server = tokio::spawn(async move { axum::serve(listener, sender).await.unwrap() });
    f.state.config.write().await.as_object_mut().unwrap().extend(json!({
        "telegram_enabled":true,"notification_time":{"start":"00:00","end":"23:59"},"telegram_bot_token":"fixture", "telegram_channel_id":"-1000", "telegram_notify_types":["recognize_fail"]
    }).as_object().unwrap().clone());
    let ids = history::begin_emby(
        &f.state,
        "item.add",
        &episode("ep-failure"),
        "/media/ep-failure.strm",
        None,
    )
    .await
    .unwrap();
    let event = json!({"source":"emby","history_ids":ids});
    let failed = Ok(
        json!({"message":"本批已处理","failed":1,"failures":[{"item_id":"ep-failure","reason":"JSON 回读验收失败 password=private"}]}),
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        history::finish(&f.state, &event, &failed),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(f.state.notifications.pending().await, 1);
    let delivery_state = f.state.clone();
    let delivery = tokio::spawn(async move {
        emby_toolkit::notifications::flush(&delivery_state).await;
    });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    let original = f.rows().await[0].clone();
    assert_eq!(original["status"], "failed");
    assert!(
        original["message"]
            .as_str()
            .unwrap()
            .contains("ep-failure：JSON 回读验收失败")
    );
    assert!(!original["message"].as_str().unwrap().contains("private"));
    assert!(!original["message"].as_str().unwrap().contains("本批已处理"));
    history::finish(
        &f.state,
        &event,
        &Ok(json!({"updated":1,"message":"过期成功响应"})),
    )
    .await
    .unwrap();
    assert_eq!(
        f.rows().await[0],
        original,
        "a repeated terminal event must not overwrite a real failure"
    );
    assert_eq!(received.lock().await.len(), 1);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), delivery)
        .await
        .unwrap()
        .unwrap();
    history::begin_reorganize(&f.state, &ids, "ep-failure")
        .await
        .unwrap();
    history::finish_reorganize(
        &f.state,
        &ids,
        "ep-failure",
        &Ok(json!({"updated":1,"message":"重新整理成功"})),
    )
    .await
    .unwrap();
    let retried = f.rows().await[0].clone();
    assert_eq!(retried["status"], "success");
    assert_eq!(retried["created_at"], original["created_at"]);
    history::finish_reorganize(
        &f.state,
        &ids,
        "ep-failure",
        &Err(ApiError::bad("过期失败响应")),
    )
    .await
    .unwrap();
    assert_eq!(f.rows().await[0], retried);
    assert_eq!(
        received.lock().await.len(),
        1,
        "a repeated finish must not notify again"
    );
    history::begin_reorganize(&f.state, &ids, "ep-failure")
        .await
        .unwrap();
    history::finish_reorganize(
        &f.state,
        &ids,
        "ep-failure",
        &Err(ApiError::bad("任务已取消")),
    )
    .await
    .unwrap();
    assert_eq!(
        f.state.notifications.pending().await,
        0,
        "an explicit stop must not enqueue a recognition failure"
    );
    server.abort();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn repeated_completed_events_cannot_finalize_a_later_admission_of_the_same_file() {
    let f = Fixture::new().await;
    let item = episode("ep1");
    let old_ids = history::begin_emby(&f.state, "item.add", &item, "/media/ep1.strm", None)
        .await
        .unwrap();
    let old_event = json!({"source":"emby","history_ids":old_ids});
    history::finish(&f.state, &old_event, &Err(ApiError::bad("上次真实失败")))
        .await
        .unwrap();
    let mut later = json!({"source":"moviepilot","media_type":"tv","title":"后来再次入库","files":["/media/ep1.strm"]});
    let later_ids = history::begin_moviepilot(&f.state, &later).await.unwrap();
    later["history_ids"] = json!(later_ids);
    history::bind_item(&f.state, &later, "/media/ep1.strm", &item)
        .await
        .unwrap();
    history::finish(
        &f.state,
        &old_event,
        &Ok(json!({"updated":1,"message":"过期响应"})),
    )
    .await
    .unwrap();
    let rows = f.rows().await;
    let old = rows.iter().find(|row| row["id"] == old_ids[0]).unwrap();
    let new = rows.iter().find(|row| row["id"] == later_ids[0]).unwrap();
    assert_eq!(old["status"], "failed");
    assert_eq!(old["message"], "上次真实失败");
    assert_eq!(
        new["status"], "processing",
        "a stale event is no proof that a new admission completed"
    );
    assert!(new["completed_at"].is_null());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn stopping_full_role_translation_during_read_scan_preserves_processing_records() {
    let f = Fixture::new().await;
    f.state.config.write().await["ai_translate_actor_role"] = json!(true);
    f.state.config.write().await["libraries_to_ignore"] = json!(["ignored-library"]);
    sqlx::query(
        "INSERT INTO processed_log(item_id,item_name) VALUES('kept-item','Previous processing')",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    let before: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(t) FROM processed_log t")
        .fetch_all(&f.state.db)
        .await
        .unwrap();
    let mut blocker = f.state.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE media_metadata IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let submitted = emby_toolkit::tasks::submit_task(&f.state, "role-translation", true)
        .await
        .unwrap();
    let id = submitted["task_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation=to_regclass($1) AND mode='AccessShareLock' AND NOT granted)")
                .bind(format!("{}.media_metadata", f.schema))
                .fetch_one(&f.state.db).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the task must really wait on its read-only scan");
    let response = emby_toolkit::router(f.state.clone())
        .oneshot(
            Request::post("/api/trigger_stop_task")
                .header(header::COOKIE, f.cookie())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"task_id":id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let stop_status = response.status();
    blocker.rollback().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = f.state.jobs.result(id).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("role translation should finish after the read is released");
    let after: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(t) FROM processed_log t")
        .fetch_all(&f.state.db)
        .await
        .unwrap();
    f.close().await;
    assert_eq!(stop_status, StatusCode::OK);
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(
        after, before,
        "a stop accepted during read-only scanning must not reset processing records"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no real Emby writes"]
async fn linked_history_completion_is_atomic_and_retryable_after_sql_failure() {
    let f = Fixture::new().await;
    let item = episode("ep1");
    let path = "/media/ep1.strm";
    let mut mp_event =
        json!({"source":"moviepilot","media_type":"tv","title":"Atomic history","files":[path]});
    let mp_ids = history::begin_moviepilot(&f.state, &mp_event)
        .await
        .unwrap();
    mp_event["history_ids"] = json!(mp_ids);
    history::bind_item(&f.state, &mp_event, path, &item)
        .await
        .unwrap();
    let emby_ids = history::begin_emby(&f.state, "item.add", &item, path, None)
        .await
        .unwrap();
    let event = json!({"source":"emby","history_ids":emby_ids});
    sqlx::query("CREATE FUNCTION reject_linked_history_completion() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture linked history write failure'; END $$")
        .execute(&f.state.db).await.unwrap();
    sqlx::query("CREATE TRIGGER linked_history_completion_gate BEFORE UPDATE ON media_organize_history FOR EACH ROW WHEN (OLD.source='moviepilot' AND NEW.status='success') EXECUTE FUNCTION reject_linked_history_completion()")
        .execute(&f.state.db).await.unwrap();
    let result = Ok(json!({"updated":1,"message":"Metadata completed"}));
    let failed = history::finish(&f.state, &event, &result).await.is_err();
    let after_failure = f.rows().await;
    sqlx::query("DROP TRIGGER linked_history_completion_gate ON media_organize_history")
        .execute(&f.state.db)
        .await
        .unwrap();
    let retried = history::finish(&f.state, &event, &result).await;
    let after_retry = f.rows().await;
    f.close().await;
    assert!(
        failed,
        "the PostgreSQL fault must reach linked MoviePilot completion"
    );
    for id in [emby_ids[0].as_str(), mp_ids[0].as_str()] {
        let row = after_failure.iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(row["status"], "processing", "{row}");
        assert!(row["completed_at"].is_null(), "{row}");
    }
    assert!(retried.is_ok(), "{retried:?}");
    for id in [emby_ids[0].as_str(), mp_ids[0].as_str()] {
        let row = after_retry.iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(row["status"], "success", "{row}");
        assert!(!row["completed_at"].is_null(), "{row}");
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn warning_receipts_restore_episode_details_only_from_the_linked_run() {
    let f = Fixture::new().await;
    let first = f.seed_history(Some("101"), "warning").await;
    let second = f.seed_history(Some("102"), "warning").await;
    let report = json!({
        "message":"本批入库有提醒", "warning_details":["片尾未全部提取"],
        "automatic_extraction":{"credits":{"items":[{"item_id":"102","status":"failed","error":"声纹不匹配"}]}}
    });
    sqlx::query("UPDATE media_organize_history SET message=$1").bind("本批入库有提醒").execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json) VALUES('webhook',$1,$2)")
        .bind(json!({"kind":"webhook","payload":{"event":{"history_ids":[first,second]}}}))
        .bind(json!({"result":report})).execute(&f.state.db).await.unwrap();
    sqlx::query("DELETE FROM app_settings WHERE setting_key='history_warning_receipts_v1'").execute(&f.state.db).await.unwrap();
    emby_toolkit::db::initialize(&f.state.db).await.unwrap();
    for row in f.rows().await {
        assert_eq!(row["batch_warnings"],json!(["片尾未全部提取"]));
        assert_eq!(row["item_warnings"],if row["id"] == first {json!([])} else {json!(["片尾提取：声纹不匹配"])});
    }
    f.close().await;
}
