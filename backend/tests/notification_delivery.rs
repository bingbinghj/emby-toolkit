use axum::{Json, Router, extract::State, routing::post};
use chrono::{NaiveTime, Timelike};
use emby_toolkit::{AppState, notifications};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

#[test]
fn notification_controls_reject_invalid_scope_time_and_enable_values() {
    for config in [
        json!({"notification_time":{"start":"24:00","end":"10:00"}}),
        json!({"notification_time":{"start":"9:00","end":"10:00"}}),
        json!({"notification_time":{"start":" 1:00","end":"10:00"}}),
        json!({"notification_time":{"start":"12:30","end":null}}),
        json!({"notification_scope":{"recognize_fail":"everyone"}}),
        json!({"notification_scope":{"intercept_notify":"all"}}),
        json!({"telegram_enabled":"false"}),
    ] {
        assert!(notifications::validate_config(&config).is_err(), "{config}");
    }
    assert!(
        notifications::validate_config(&json!({
            "telegram_enabled":false,
            "notification_scope":{"library_new":"user,admin","recognize_fail":"all"},
            "notification_time":{"start":"22:30","end":"06:00"}
        }))
        .is_ok()
    );
}

#[test]
fn sending_window_includes_end_minute_and_supports_overnight_range() {
    let at = |text| NaiveTime::parse_from_str(text, "%H:%M:%S").unwrap();
    let all_day = json!({"notification_time":{"start":"00:00","end":"23:59"}});
    assert!(notifications::in_send_window(&all_day, at("23:59:59")));
    let night = json!({"notification_time":{"start":"22:30","end":"06:00"}});
    for time in ["22:30:00", "23:59:59", "00:00:00", "06:00:59"] {
        assert!(notifications::in_send_window(&night, at(time)), "{time}");
    }
    for time in ["06:01:00", "12:00:00", "22:29:59"] {
        assert!(!notifications::in_send_window(&night, at(time)), "{time}");
    }
    assert!(!notifications::in_send_window(
        &json!({"notification_time":{}}),
        at("12:00:00")
    ));
}

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    sent: Arc<Mutex<Vec<Value>>>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("notification_delivery_{}", uuid::Uuid::new_v4().simple());
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
        let mut state = AppState::new(pool, directory.path().into()).await.unwrap();
        for (id, admin, status, disabled, target) in [
            ("admin", true, "active", false, "100"),
            ("member", false, "active", false, "200"),
            ("disabled", false, "active", true, "300"),
            ("expired", false, "expired", false, "400"),
        ] {
            sqlx::query(
                "INSERT INTO emby_users(id,name,is_administrator,policy_json) VALUES($1,$1,$2,$3)",
            )
            .bind(id)
            .bind(admin)
            .bind(json!({"IsDisabled":disabled}))
            .execute(&state.db)
            .await
            .unwrap();
            sqlx::query("INSERT INTO emby_users_extended(emby_user_id,status,telegram_chat_id) VALUES($1,$2,$3)")
                .bind(id).bind(status).bind(target).execute(&state.db).await.unwrap();
        }
        let sent = Arc::new(Mutex::new(vec![]));
        let app = Router::new().route("/botfixture/sendMessage", post(
            |State(sent): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                sent.lock().await.push(body);
                Json(json!({"ok":true,"result":{"message_id":1}}))
            }
        )).with_state(sent.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state.notifications = Arc::new(notifications::Service::with_api_base(api_base));
        state.save_config(&json!({
            "telegram_bot_token":"fixture", "telegram_channel_id":"@channel", "telegram_enabled":true,
            "telegram_notify_types":["library_new","recognize_fail"],
            "notification_scope":{"library_new":"admin","recognize_fail":"admin"},
            "notification_time":{"start":"00:00","end":"23:59"}
        })).await.unwrap();
        Self {
            state,
            root,
            schema,
            _directory: directory,
            server,
            sent,
        }
    }
    async fn failure(&self) {
        notifications::failure(
            &self.state,
            &json!({"Id":"101","Name":"真实失败剧集 S01E02"}),
            "入库失败，token=must-not-leak，媒体未成功",
        )
        .await
        .unwrap();
    }
    async fn cleanup(self) {
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn failure_scope_follows_mp_admin_fallback_all_users_and_excludes_disabled_accounts() {
    let f = Fixture::new().await;
    for (scope, expected) in [
        ("admin", vec!["100", "@channel"]),
        ("user", vec!["100", "@channel"]),
        ("user,admin", vec!["100", "@channel"]),
        ("all", vec!["100", "200", "@channel"]),
    ] {
        f.state
            .save_config(&json!({"notification_scope":{"recognize_fail":scope}}))
            .await
            .unwrap();
        f.sent.lock().await.clear();
        f.failure().await;
        let sent = f.sent.lock().await;
        let targets = sent
            .iter()
            .map(|message| message["chat_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(targets, expected, "scope={scope}");
        for message in &*sent {
            let text = message["text"].as_str().unwrap();
            assert!(text.contains("整理失败") && text.contains("S01E02"));
            assert!(!text.contains("must-not-leak"));
            assert_eq!(message["parse_mode"], "MarkdownV2");
        }
    }
    for (scope, operator, expected) in [
        ("user", "member", vec!["200"]),
        ("user,admin", "member", vec!["100", "200", "@channel"]),
        ("admin", "member", vec!["100", "@channel"]),
        ("user", "unknown-user", vec!["100", "@channel"]),
        ("user", "disabled", vec![]),
    ] {
        f.state
            .save_config(&json!({"notification_scope":{"recognize_fail":scope}}))
            .await
            .unwrap();
        f.sent.lock().await.clear();
        notifications::with_operator(operator, f.failure()).await;
        let sent = f.sent.lock().await;
        assert_eq!(
            sent.iter()
                .map(|message| message["chat_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected,
            "scope={scope} actor={operator}"
        );
    }
    sqlx::query(
        "INSERT INTO emby_users(id,name,is_administrator) VALUES('unbound','Unbound',false)",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    f.state
        .save_config(&json!({"notification_scope":{"recognize_fail":"user"}}))
        .await
        .unwrap();
    f.sent.lock().await.clear();
    notifications::with_operator("unbound", f.failure()).await;
    assert!(
        f.sent.lock().await.is_empty(),
        "an existing user without a bound channel must not fall back to the global channel"
    );
    sqlx::query(
        "UPDATE emby_users_extended SET telegram_chat_id='100' WHERE emby_user_id='member'",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    f.state
        .save_config(&json!({"notification_scope":{"recognize_fail":"user,admin"}}))
        .await
        .unwrap();
    notifications::with_operator("member", f.failure()).await;
    assert_eq!(
        f.sent.lock().await.len(),
        2,
        "the actor/admin's identical chat target is delivered only once"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn outside_window_queues_then_sends_without_blocking_the_admission() {
    let f = Fixture::new().await;
    let now = chrono::Local::now().time();
    let other = NaiveTime::from_hms_opt((now.hour() + 12) % 24, now.minute(), 0)
        .unwrap()
        .format("%H:%M")
        .to_string();
    f.state
        .save_config(&json!({"notification_time":{"start":other,"end":other}}))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), f.failure())
        .await
        .unwrap();
    assert!(f.sent.lock().await.is_empty());
    assert_eq!(f.state.notifications.pending().await, 2);
    notifications::flush(&f.state).await;
    assert_eq!(f.state.notifications.pending().await, 2);
    f.state
        .save_config(&json!({"notification_time":{"start":"00:00","end":"23:59"}}))
        .await
        .unwrap();
    notifications::flush(&f.state).await;
    assert_eq!(f.sent.lock().await.len(), 2);
    assert_eq!(f.state.notifications.pending().await, 0);
    notifications::flush(&f.state).await;
    assert_eq!(f.sent.lock().await.len(), 2);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn disabled_channel_or_failure_type_does_not_send_and_worker_stops() {
    let f = Fixture::new().await;
    f.state
        .save_config(&json!({"telegram_enabled":false}))
        .await
        .unwrap();
    f.failure().await;
    f.state
        .save_config(&json!({"telegram_enabled":true,"telegram_notify_types":["library_new"]}))
        .await
        .unwrap();
    f.failure().await;
    assert!(f.sent.lock().await.is_empty());
    assert_eq!(f.state.notifications.pending().await, 0);
    let token = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(notifications::run(f.state.clone(), token.clone()));
    token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
async fn explicit_notification_test_checks_telegram_acknowledgement_and_bypasses_quiet_hours() {
    let app = Router::new().route(
        "/botfixture/sendMessage",
        post(|Json(body): Json<Value>| async move {
            let ok = body["chat_id"] != "reject";
            Json(json!({"ok":ok,"description":"Telegram rejected token=hidden-secret"}))
        }),
    );
    let app = app
        .route(
            "/botdraft/getMe",
            axum::routing::get(|| async {
                Json(json!({"ok":true,"result":{"username":"draft_bot"}}))
            }),
        )
        .route(
            "/botfixture/getMe",
            axum::routing::get(|| async {
                Json(json!({"ok":true,"result":{"username":"saved_bot"}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(tokio::sync::RwLock::new(
            json!({"telegram_bot_token":"fixture","telegram_enabled":false,"notification_time":{"start":"12:00","end":"12:00"}}),
        )),
        http: reqwest::Client::new(),
        sessions: Arc::new(emby_toolkit::auth::SessionSigner::load(directory.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: directory.path().into(),
        notifications: Arc::new(notifications::Service::with_api_base(api_base)),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: emby_toolkit::logs::LogStore::open(&directory.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    notifications::test_message(&state, "fixture", "accept")
        .await
        .unwrap();
    let error = notifications::test_message(&state, "fixture", "reject")
        .await
        .unwrap_err();
    assert!(error.1.contains("发送失败"));
    assert!(!error.1.contains("hidden-secret"));
    assert_eq!(state.notifications.pending().await, 0);
    use tower::ServiceExt;
    let response = emby_toolkit::system_ops::router()
        .with_state(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/telegram/test")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"token":"fixture","chat_id":"reject"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
    let payload: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(payload["success"], false);
    assert!(!payload["error"].as_str().unwrap().contains("hidden-secret"));
    let app = emby_toolkit::portal::router().with_state(state.clone());
    let draft_response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/portal/telegram-bot-info")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(json!({"token":"draft"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let draft: Value = serde_json::from_slice(
        &axum::body::to_bytes(draft_response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(draft["bot_username"], "draft_bot");
    assert_eq!(
        state.config.read().await["telegram_bot_token"],
        "fixture",
        "draft lookup must not save a token"
    );
    let saved_response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/portal/telegram-bot-info")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let saved: Value = serde_json::from_slice(
        &axum::body::to_bytes(saved_response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved["bot_username"], "saved_bot",
        "the original GET endpoint remains compatible"
    );
    server.abort();
}

#[tokio::test]
async fn operator_context_is_explicit_and_scoped_across_awaits() {
    assert_eq!(notifications::current_operator(), None);
    notifications::with_operator("member", async {
        tokio::task::yield_now().await;
        assert_eq!(notifications::current_operator().as_deref(), Some("member"));
        notifications::with_operator("admin", async {
            assert_eq!(notifications::current_operator().as_deref(), Some("admin"));
        })
        .await;
        assert_eq!(notifications::current_operator().as_deref(), Some("member"));
    })
    .await;
    assert_eq!(notifications::current_operator(), None);
}
