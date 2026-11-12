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

#[test]
fn telegram_channel_fields_validate_api_topic_and_principal_lists() {
    for config in [
        json!({"telegram_api_url":"ftp://example.invalid"}),
        json!({"telegram_api_url":"https://user:secret@example.invalid"}),
        json!({"telegram_api_url":"https://example.invalid?token=secret"}),
        json!({"telegram_api_url":"https://example.invalid#secret"}),
        json!({"telegram_api_url":123}),
        json!({"telegram_topic_id":"0"}),
        json!({"telegram_topic_id":"-123"}),
        json!({"telegram_topic_id":"9999999999999999999999999999"}),
        json!({"telegram_topic_id":"12.5"}),
        json!({"telegram_users":"100,,200"}),
        json!({"telegram_admins":"100,abc"}),
        json!({"telegram_admins":true}),
    ] {
        assert!(notifications::validate_config(&config).is_err(), "{config}");
    }
    let config = json!({"telegram_channel_name":"媒体通知","telegram_api_url":" https://example.invalid/api/ ","telegram_topic_id":"17","telegram_users":"100, 200,100","telegram_admins":"200"});
    notifications::validate_config(&config).unwrap();
    assert_eq!(
        notifications::telegram_api_base(&config, "http://localhost:1").unwrap(),
        "https://example.invalid/api"
    );
    assert_eq!(
        notifications::principal_ids(&config, "telegram_users")
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["100", "200"]
    );
    assert_eq!(
        notifications::telegram_api_base(&json!({"telegram_api_url":""}), "http://localhost:1/")
            .unwrap(),
        "http://localhost:1"
    );
}

#[test]
fn telegram_topic_attaches_only_to_the_configured_channel_not_personal_chats() {
    let config = json!({"telegram_channel_id":"https://t.me/mediacenter","telegram_topic_id":"17"});
    assert_eq!(
        notifications::topic_payload(&config, &json!({"chat_id":"@mediacenter"})).unwrap()["message_thread_id"],
        17
    );
    assert!(notifications::topic_payload(&config, &json!({"chat_id":"100"})).unwrap()["message_thread_id"].is_null());
    let config = json!({"telegram_channel_id":"-10012345","telegram_topic_id":"17"});
    assert_eq!(
        notifications::topic_payload(&config, &json!({"chat_id":-10012345})).unwrap()["message_thread_id"],
        17
    );
    assert!(
        notifications::topic_payload(
            &json!({"telegram_channel_id":"-10012345","telegram_topic_id":""}),
            &json!({"chat_id":"-10012345"})
        )
        .unwrap()["message_thread_id"]
            .is_null()
    );
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
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn telegram_allowed_users_cannot_execute_and_group_members_cannot_borrow_admin_chat_identity()
{
    let f = Fixture::new().await;
    f.state
        .save_config(&json!({"telegram_users":"200","telegram_admins":"900"}))
        .await
        .unwrap();
    let update =
        |from, chat, text| json!({"message":{"from":{"id":from},"chat":{"id":chat},"text":text}});
    emby_toolkit::telegram_bot::handle_update(&f.state, "fixture", &update(200, 200, "/all_tasks"))
        .await
        .unwrap();
    assert!(
        f.sent.lock().await[0]["text"]
            .as_str()
            .unwrap()
            .contains("所有可用任务")
    );
    assert!(
        f.sent.lock().await[0]["reply_markup"].is_null(),
        "allowed users see names without executable buttons"
    );
    f.sent.lock().await.clear();
    emby_toolkit::telegram_bot::handle_update(
        &f.state,
        "fixture",
        &update(200, 200, "/clear_mediainfo"),
    )
    .await
    .unwrap();
    assert!(
        f.sent.lock().await[0]["text"]
            .as_str()
            .unwrap()
            .contains("Emby 管理员")
    );
    f.sent.lock().await.clear();
    emby_toolkit::telegram_bot::handle_update(
        &f.state,
        "fixture",
        &update(900, 900, "/clear_mediainfo"),
    )
    .await
    .unwrap();
    assert!(
        f.sent.lock().await[0]["text"]
            .as_str()
            .unwrap()
            .contains("Emby 管理员"),
        "Telegram-only admins do not gain Emby maintenance privileges"
    );
    f.sent.lock().await.clear();
    emby_toolkit::telegram_bot::handle_update(
        &f.state,
        "fixture",
        &update(100, 100, "/clear_mediainfo"),
    )
    .await
    .unwrap();
    assert!(
        f.sent.lock().await[0]["text"]
            .as_str()
            .unwrap()
            .contains("Emby 管理员"),
        "an explicit TG admin list limits even a bound Emby admin"
    );
    f.sent.lock().await.clear();
    f.state
        .save_config(&json!({"telegram_admins":"","telegram_users":""}))
        .await
        .unwrap();
    emby_toolkit::telegram_bot::handle_update(
        &f.state,
        "fixture",
        &update(200, 100, "/clear_mediainfo"),
    )
    .await
    .unwrap();
    assert!(
        f.sent.lock().await.is_empty(),
        "a group chat that matches a bound admin ID does not authorize another sender"
    );
    emby_toolkit::telegram_bot::handle_update(&f.state, "fixture", &json!({"callback_query":{"id":"callback","from":{"id":200},"message":{"chat":{"id":100}},"data":"run_task_clear-mediainfo"}})).await.unwrap();
    assert!(
        f.sent.lock().await.is_empty(),
        "callbacks have the same sender permission boundary"
    );
    f.failure().await;
    assert!(
        f.sent
            .lock()
            .await
            .iter()
            .all(|message| message["chat_id"] != "900")
    );
    f.sent.lock().await.clear();
    f.state
        .save_config(&json!({"telegram_admins":"900","telegram_topic_id":"17"}))
        .await
        .unwrap();
    f.failure().await;
    let sent = f.sent.lock().await;
    assert_eq!(
        sent.iter()
            .map(|message| message["chat_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["100", "900", "@channel"]
    );
    for message in &*sent {
        assert_eq!(
            message["message_thread_id"],
            if message["chat_id"] == "@channel" {
                json!(17)
            } else {
                Value::Null
            }
        );
    }
    drop(sent);
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

fn localhost_state(directory: &tempfile::TempDir, api_base: String, config: Value) -> AppState {
    AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(tokio::sync::RwLock::new(config)),
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
    }
}

#[tokio::test]
async fn telegram_draft_api_url_topic_and_get_me_are_real_requests_without_saving_drafts() {
    use tower::ServiceExt;
    let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
    let app = Router::new()
        .route(
            "/proxy/botdraft/sendMessage",
            post(
                |State(sent): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    sent.lock().await.push(body);
                    Json(json!({"ok":true}))
                },
            ),
        )
        .route(
            "/proxy/botdraft/getMe",
            axum::routing::get(|| async {
                Json(json!({"ok":true,"result":{"username":"draft_bot"}}))
            }),
        )
        .route(
            "/proxy/botsaved/getMe",
            axum::routing::get(|| async {
                Json(json!({"ok":true,"result":{"username":"saved_bot"}}))
            }),
        )
        .with_state(sent.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_url = format!("http://{}/proxy", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let saved = json!({"telegram_bot_token":"saved","telegram_api_url":"http://127.0.0.1:9","telegram_channel_id":"@saved","telegram_topic_id":"12"});
    let state = localhost_state(&directory, "http://127.0.0.1:9".into(), saved.clone());
    let request = |uri: &str, body: Value| {
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    };
    let response = emby_toolkit::system_ops::router()
        .with_state(state.clone())
        .oneshot(request(
            "/api/telegram/test",
            json!({"token":"draft","chat_id":"@draft","api_url":api_url,"topic_id":"41"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(sent.lock().await[0]["message_thread_id"], 41);
    assert_eq!(sent.lock().await[0]["chat_id"], "@draft");
    let app = emby_toolkit::portal::router().with_state(state.clone());
    let response = app
        .clone()
        .oneshot(request(
            "/api/portal/telegram-bot-info",
            json!({"token":"draft","api_url":api_url}),
        ))
        .await
        .unwrap();
    let result: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["bot_username"], "draft_bot");
    assert_eq!(
        *state.config.read().await,
        saved,
        "draft token, API URL, channel and topic are never persisted by testing or getMe"
    );
    state.config.write().await["telegram_api_url"] = json!(api_url);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/portal/telegram-bot-info")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let result: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        result["bot_username"], "saved_bot",
        "saved getMe also uses the configured API URL"
    );
    for draft in [
        json!({"api_url":7}),
        json!({"api_url":"https://user:secret@example.invalid"}),
        json!({"topic_id":"-1"}),
    ] {
        assert!(
            notifications::test_message_draft(&state, "draft", "@draft", &draft)
                .await
                .is_err()
        );
    }
    assert_eq!(
        sent.lock().await.len(),
        1,
        "invalid draft controls never send a Telegram request"
    );
    server.abort();
}

#[tokio::test]
async fn telegram_polling_switches_api_base_and_stops_when_the_channel_is_disabled() {
    let first_poll = Arc::new(tokio::sync::Notify::new());
    let first_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/botsaved/setMyCommands",
            post(|| async { Json(json!({"ok":true})) }),
        )
        .route(
            "/botsaved/getUpdates",
            axum::routing::get({
                let first_poll = first_poll.clone();
                let first_count = first_count.clone();
                move || {
                    let first_poll = first_poll.clone();
                    let first_count = first_count.clone();
                    async move {
                        first_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        first_poll.notify_one();
                        tokio::time::sleep(std::time::Duration::from_secs(35)).await;
                        Json(json!({"ok":true,"result":[]}))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_url = format!("http://{}", listener.local_addr().unwrap());
    let first_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let second_poll = Arc::new(tokio::sync::Notify::new());
    let second_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let second_menu_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/api/botsaved/setMyCommands",
            post({
                let count = second_menu_count.clone();
                move || {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({"ok":true}))
                    }
                }
            }),
        )
        .route(
            "/api/botsaved/getUpdates",
            axum::routing::get({
                let count = second_count.clone();
                let poll = second_poll.clone();
                move || {
                    let count = count.clone();
                    let poll = poll.clone();
                    async move {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        poll.notify_one();
                        Json(json!({"ok":true,"result":[]}))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_url = format!("http://{}/api", listener.local_addr().unwrap());
    let second_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = localhost_state(
        &directory,
        "http://127.0.0.1:9".into(),
        json!({"telegram_bot_token":"saved","telegram_api_url":first_url,"telegram_enabled":true}),
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(emby_toolkit::telegram_bot::run(
        state.clone(),
        cancel.clone(),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(3), first_poll.notified())
        .await
        .unwrap();
    state.config.write().await["telegram_api_url"] = json!(second_url);
    tokio::time::timeout(std::time::Duration::from_secs(4), second_poll.notified())
        .await
        .unwrap();
    assert_eq!(first_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        second_menu_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the new API instance registers the task menu"
    );
    state.config.write().await["telegram_enabled"] = json!(false);
    let count = second_count.load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(
        second_count.load(std::sync::atomic::Ordering::SeqCst),
        count
    );
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap();
    first_server.abort();
    second_server.abort();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn library_template_delivers_only_this_admission_scope_with_real_cached_sizes() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,emby_item_ids_json,genres_json) VALUES('4426','Series','盖亚奥特曼',$1,$2)")
        .bind(json!(["series"])).bind(json!(["动作","家庭"])).execute(&f.state.db).await.unwrap();
    for (id, episode, size, container, letter) in [
        ("ep1", 1, 1073741824u64, "mkv", "a"),
        ("ep2", 2, 2147483648u64, "mp4", "b"),
        ("old", 99, 1073741824000u64, "mkv", "c"),
    ] {
        let sha = letter.repeat(40);
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,season_number,episode_number,emby_item_ids_json,asset_details_json,file_sha1_json) VALUES($1,'Episode','4426',1,$2,$3,$4,$5)")
            .bind(id).bind(episode).bind(json!([id])).bind(json!([{"emby_item_id":id,"path":format!("/media/Series.S01E{episode:02}.strm"),"size_bytes":50,"container":"strm"}]))
            .bind(json!([sha])).execute(&f.state.db).await.unwrap();
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
            .bind(&sha).bind(json!([{"MediaSourceInfo":{"Size":size,"Container":container,"MediaStreams":[{"Type":"Video","Codec":"H264"}]}}]))
            .execute(&f.state.db).await.unwrap();
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,size) VALUES($1,'0',$2,$3,$4)")
            .bind(id).bind(format!("Series.S01E{episode:02}.{container}")).bind(sha.to_uppercase()).bind(size as i64)
            .execute(&f.state.db).await.unwrap();
    }
    notifications::media(&f.state,
        &json!({"Id":"series","Type":"Series","Name":"盖亚奥特曼","ProductionYear":1998,"ProviderIds":{"Tmdb":"4426"},"Overview":"S01E01-E99"}),
        true, &["ep2".into(),"ep1".into(),"ep1".into()], &json!({"pending":[]}), &[], None).await.unwrap();
    let sent = f.sent.lock().await.clone();
    assert_eq!(sent.len(), 2);
    for message in sent {
        let text = message["text"].as_str().unwrap();
        assert!(
            text.starts_with(
                "📂 已入库2个文件\n[盖亚奥特曼 \\(1998\\)](https://www.themoviedb.org/tv/4426)"
            ),
            "{text}"
        );
        assert!(text.contains("📺 集数：S01E01\\-E02"));
        assert!(text.ends_with("💾 大小：3\\.0G"));
        assert!(
            text.contains("MKV") && text.contains("MP4"),
            "actual original containers: {text}"
        );
        assert!(!text.contains("STRM") && !text.contains("50B"));
        assert!(!text.contains("E99"));
        assert_eq!(message["parse_mode"], "MarkdownV2");
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn long_library_template_uses_message_instead_of_exceeding_photo_caption_limit() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,poster_path) VALUES('42','Movie',$1,'/poster.jpg')")
        .bind(json!(["101"])).execute(&f.state.db).await.unwrap();
    for raw in ["_*\\[".repeat(1200), "😀".repeat(1200)] {
        f.sent.lock().await.clear();
        notifications::media(&f.state,&json!({"Id":"101","Type":"Movie","Name":raw,"Genres":[raw],"ProviderIds":{"Tmdb":"42"}}),
            false,&[],&json!({"pending":[]}), &[], None).await.unwrap();
        let sent = f.sent.lock().await.clone();
        assert_eq!(
            sent.len(),
            2,
            "mock only accepts sendMessage; an oversized photo must not be posted"
        );
        for message in sent {
            let text = message["text"].as_str().unwrap();
            assert!(text.encode_utf16().count() > 1000 && text.encode_utf16().count() < 4096);
            assert!(message["caption"].is_null());
        }
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and Telegram are localhost"]
async fn native_episode_size_and_format_use_existing_gets_with_main_slot_and_cooldown() {
    let f = Fixture::new().await;
    let calls = Arc::new(Mutex::new(Vec::<(String, std::time::Instant)>::new()));
    let captures = calls.clone();
    let app = Router::new()
        .route("/web/ConfigurationPages",axum::routing::get(|| async {
            Json(json!([{"Name":"MediaInfoExtractPageView","Href":"?PageId=fixture:MediaInfoExtractPageView"}]))
        }))
        .route("/UI/View",axum::routing::get(|| async {
            Json(json!({"EditObjectContainer":{"Object":{"GeneralOptions":{"MaxConcurrentCount":1,"Tier2MaxConcurrentCount":1,"CooldownDurationSeconds":1}}}}))
        }))
        .route("/Users/service/Items/{id}",axum::routing::get(
            move |axum::extract::Path(id):axum::extract::Path<String>,axum::extract::Query(fields):axum::extract::Query<std::collections::HashMap<String,String>>| {
                let calls=captures.clone();
                async move {
                    assert!(fields["Fields"].contains("MediaSources"));
                    calls.lock().await.push((id.clone(),std::time::Instant::now()));
                    let episode = if id == "ep1" {1} else {2};
                    Json(json!({"Id":id,"Type":"Episode","Path":format!("/media/{id}.strm"),"Size":50,"ParentIndexNumber":1,"IndexNumber":episode,
                        "MediaSources":[{"ItemId":id,"Path":format!("/original/{id}.{}",if episode==1 {"mkv"} else {"mp4"}),"Container":if episode==1 {"mkv"} else {"mp4"},"Size":1073741824u64*episode,"MediaStreams":[{"Type":"Video","Codec":"H264"}]}]}))
                }
            })) ;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    f.state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"service","emby_api_timeout":5})).await.unwrap();
    for (id, episode, letter) in [("ep1", 1, "a"), ("ep2", 2, "b"), ("old", 99, "c")] {
        let hash = letter.repeat(40);
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,season_number,episode_number,emby_item_ids_json,asset_details_json,file_sha1_json) VALUES($1,'Episode','4426',1,$2,$3,$4,$5)")
            .bind(id).bind(episode).bind(json!([id])).bind(json!([{"emby_item_id":id,"path":format!("/media/{id}.strm"),"container":"strm","size_bytes":50}]))
            .bind(json!([hash])).execute(&f.state.db).await.unwrap();
        // A still-bound old source must not overwrite the current native video.
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
            .bind(&hash).bind(json!([{"MediaSourceInfo":{"Size":1048576,"Container":"mkv","MediaStreams":[{"Type":"Video","Codec":"H264"}]}}]))
            .execute(&f.state.db).await.unwrap();
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,size) VALUES($1,'0',$2,$3,1048576)")
            .bind(id).bind(format!("Old.{id}.mkv")).bind(&hash).execute(&f.state.db).await.unwrap();
    }
    let item = json!({"Id":"series","Type":"Series","Name":"盖亚奥特曼"});
    let ids = ["ep2".into(), "ep1".into(), "ep1".into()];
    let report = json!({"pending":[]});
    {
        let primary = emby_toolkit::strm::slot(&f.state, "main", None)
            .await
            .unwrap();
        let sending = notifications::media(&f.state, &item, true, &ids, &report, &[], None);
        tokio::pin!(sending);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(75), &mut sending)
                .await
                .is_err()
        );
        assert!(calls.lock().await.is_empty());
        let released = std::time::Instant::now();
        drop(primary);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(700), &mut sending)
                .await
                .is_err()
        );
        assert!(calls.lock().await.is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(4), &mut sending)
            .await
            .unwrap()
            .unwrap();
        assert!(released.elapsed() >= std::time::Duration::from_millis(1900));
        let observed = calls.lock().await.clone();
        assert_eq!(
            observed
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["ep1", "ep2"]
        );
        assert!(
            observed[1].1.duration_since(observed[0].1) >= std::time::Duration::from_millis(900)
        );
        let sent = f.sent.lock().await.clone();
        assert_eq!(sent.len(), 2);
        for message in sent {
            let text = message["text"].as_str().unwrap();
            assert!(text.starts_with("📂 已入库2个文件"));
            assert!(text.contains("MKV") && text.contains("MP4") && !text.contains("STRM"));
            assert!(text.ends_with("💾 大小：3\\.0G"));
        }
    }
    server.abort();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Telegram is localhost"]
async fn missing_native_size_does_not_replace_current_media_with_stale_cache() {
    let f = Fixture::new().await;
    let hash = "a".repeat(40);
    let path = "/original/Film.2160p.BLURAY.HEVC.AAC-CURRENT.mkv";
    let streams = json!([
        {"Type":"Video","Codec":"HEVC","Width":3840,"Height":2160},
        {"Type":"Audio","Codec":"AAC","IsDefault":true}
    ]);
    let item = json!({"Id":"101","Type":"Movie","Name":"电影","Path":"/media/film.strm",
        "MediaSources":[{"ItemId":"101","Path":path,"Container":"mkv","MediaStreams":streams}]});
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,emby_item_ids_json,asset_details_json,file_sha1_json) VALUES('42','Movie',$1,$2,$3)")
        .bind(json!(["101"]))
        .bind(json!([{"emby_item_id":"101","path":"/media/film.strm","container":"strm","size_bytes":50}]))
        .bind(json!([hash])).execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,size) VALUES('101','0',$1,$2,1073741824)")
        .bind("Film.720p.WEB-DL.H264.AC3-OLD.mkv").bind(&hash).execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
        .bind(&hash).bind(json!([{"MediaSourceInfo":{"Size":1073741824,"Container":"mkv",
            "MediaStreams":[{"Type":"Video","Codec":"H264","Width":1280,"Height":720},{"Type":"Audio","Codec":"AC3"}]}}]))
        .execute(&f.state.db).await.unwrap();
    for name in [
        "Film.720p.WEB-DL.H264.AC3-OLD.mkv",
        path.rsplit('/').next().unwrap(),
    ] {
        sqlx::query("UPDATE p115_filesystem_cache SET name=$1 WHERE id='101'")
            .bind(name)
            .execute(&f.state.db)
            .await
            .unwrap();
        notifications::media(
            &f.state,
            &item,
            false,
            &[],
            &json!({"pending":[]}),
            &[],
            None,
        )
        .await
        .unwrap();
        assert_eq!(f.sent.lock().await.len(), 2);
        for message in f.sent.lock().await.iter() {
            let text = message["text"].as_str().unwrap();
            assert!(
                text.contains("2160p")
                    && text.contains("HEVC")
                    && text.contains("AAC")
                    && text.contains("CURRENT"),
                "{text}"
            );
            assert!(
                !text.contains("720p")
                    && !text.contains("H264")
                    && !text.contains("AC3")
                    && !text.contains("OLD"),
                "{text}"
            );
            assert!(
                text.ends_with("💾 大小：未知"),
                "conflicting cache must not supply size: {text}"
            );
        }
        f.sent.lock().await.clear();
    }
    sqlx::query("UPDATE media_metadata SET asset_details_json=$1 WHERE tmdb_id='42' AND item_type='Movie'")
        .bind(json!([{"emby_item_id":"101","path":"/original/Film.720p.WEB-DL.H264.AC3-OLD.mkv","container":"mkv","size_bytes":1073741824}]))
        .execute(&f.state.db).await.unwrap();
    let mut local_item = item.clone();
    local_item["Path"] = json!(path);
    notifications::media(
        &f.state,
        &local_item,
        false,
        &[],
        &json!({"pending":[]}),
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(f.sent.lock().await.len(), 2);
    for message in f.sent.lock().await.iter() {
        let text = message["text"].as_str().unwrap();
        assert!(
            text.contains("2160p") && text.contains("HEVC") && text.contains("CURRENT"),
            "{text}"
        );
        assert!(
            text.ends_with("💾 大小：未知"),
            "rejected cache must not reenter through stale asset size: {text}"
        );
    }
    f.sent.lock().await.clear();
    let partial_path = "/original/Film-CURRENT.mkv";
    let mut partial_item = item.clone();
    partial_item["Path"] = json!(partial_path);
    partial_item["MediaSources"][0]["Path"] = json!(partial_path);
    partial_item["MediaSources"][0]["MediaStreams"] = json!([{"Type":"Video","Codec":"HEVC"}]);
    sqlx::query(
        "UPDATE media_metadata SET asset_details_json=$1 WHERE tmdb_id='42' AND item_type='Movie'",
    )
    .bind(
        json!([{"emby_item_id":"101","path":partial_path,"container":"mkv","size_bytes":1073741824,
            "quality_display":"WEB","resolution_display":"720p","effect_display":"SDR",
            "audio_tracks":[{"codec":"AC3","is_default":true}]}]),
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE p115_filesystem_cache SET name='Film-CURRENT.mkv' WHERE id='101'")
        .execute(&f.state.db)
        .await
        .unwrap();
    notifications::media(
        &f.state,
        &partial_item,
        false,
        &[],
        &json!({"pending":[]}),
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(f.sent.lock().await.len(), 2);
    for message in f.sent.lock().await.iter() {
        let text = message["text"].as_str().unwrap();
        assert!(text.contains("HEVC") && text.contains("CURRENT"), "{text}");
        assert!(
            !text.contains("720p")
                && !text.contains("SDR")
                && !text.contains("AC3")
                && !text.contains("WEB"),
            "rejected stale asset fields must not be mixed with current source: {text}"
        );
        assert!(text.ends_with("💾 大小：未知"), "{text}");
    }
    f.sent.lock().await.clear();
    sqlx::query("UPDATE p115_filesystem_cache SET name=$1 WHERE id='101'")
        .bind(path.rsplit('/').next().unwrap())
        .execute(&f.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2")
        .bind(
            json!([{"MediaSourceInfo":{"Path":path,"Size":1073741824,"Container":"matroska",
            "MediaStreams":[{"Type":"Video","Codec":"hevc"},{"Type":"Audio","Codec":"aac"}]}}]),
        )
        .bind(&hash)
        .execute(&f.state.db)
        .await
        .unwrap();
    notifications::media(
        &f.state,
        &item,
        false,
        &[],
        &json!({"pending":[]}),
        &[],
        None,
    )
    .await
    .unwrap();
    assert_eq!(f.sent.lock().await.len(), 2);
    for message in f.sent.lock().await.iter() {
        let text = message["text"].as_str().unwrap();
        assert!(
            text.contains("2160p") && text.contains("HEVC") && text.contains("CURRENT"),
            "{text}"
        );
        assert!(
            text.ends_with("💾 大小：1\\.0G"),
            "matching cache may fill only missing size: {text}"
        );
    }
    f.cleanup().await;
}
