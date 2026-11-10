use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    routing::get,
};
use emby_toolkit::{AppState, auth::Session, error::ApiError, media_organize_history as history};
use serde_json::{Value, json};
use std::time::Duration;
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
fn episode(id: &str) -> Value {
    json!({"Id":id,"Type":"Episode","Name":format!("本集-{id}"),"SeriesId":"series","SeriesName":"入库测试剧","ParentIndexNumber":1,"IndexNumber":2,"ImageTags":{"Primary":"frame"},"SeriesPrimaryImageTag":"series-poster","Path":format!("/media/{id}.strm"),"AccessToken":"must-not-store","ProviderIds":{"Tmdb":"private-extra"}})
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
    assert!(row["message"].as_str().unwrap().contains("重启"));
    assert_eq!(
        history::selected_history(&f.state, &json!({"history_ids":ids}), &["ep1".into()])
            .await
            .unwrap(),
        ids
    );
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
