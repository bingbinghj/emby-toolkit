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
