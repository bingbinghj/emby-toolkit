use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request as HostRequest, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, auth::Session};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use tokio::sync::Mutex;
use tower::ServiceExt;

type Call = (String, String, String, String);
#[derive(Clone, Default)]
struct Host {
    mode: Arc<AtomicU8>,
    calls: Arc<Mutex<Vec<Call>>>,
    redirect: Arc<Mutex<String>>,
}
async fn upstream(State(host): State<Host>, request: HostRequest) -> Response {
    let path = request.uri().path().to_string();
    let token = request
        .headers()
        .get("X-Emby-Token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    host.calls.lock().await.push((
        request.method().to_string(),
        path.clone(),
        token.clone(),
        request.uri().query().unwrap_or_default().to_string(),
    ));
    if path == "/draft/System/Info" && token != "draft-secret-key" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match path.as_str() {
        "/stored/web/ConfigurationPages" => Json(json!([{"Name":"MediaInfoExtractPageView","Href":"?PageId=fixture:MediaInfoExtractPageView"}])).into_response(),
        "/stored/UI/View" => Json(json!({"EditObjectContainer":{"Object":{"GeneralOptions":{"MaxConcurrentCount":1,"Tier2MaxConcurrentCount":2,"CooldownDurationSeconds":1}}}})).into_response(),
        "/draft/System/Info" => match host.mode.load(Ordering::SeqCst) {
            1 => (StatusCode::UNAUTHORIZED, "upstream leaked draft-secret-key").into_response(),
            2 => (StatusCode::OK, "<html>draft-secret-key</html>").into_response(),
            3 => Json(json!({"Id":"fixture-server"})).into_response(),
            4 => StatusCode::NOT_FOUND.into_response(),
            5 => (StatusCode::FOUND, [(header::LOCATION, host.redirect.lock().await.clone())]).into_response(),
            6 => {
                let prefix = futures_util::stream::iter([Ok::<_, std::io::Error>(
                    axum::body::Bytes::from_static(b"{\"Id\":\"fixture\","),
                )]);
                let delayed = futures_util::stream::once(async {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                        b"\"ServerName\":\"Draft Emby\",\"Version\":\"4.10.2.0\"}",
                    ))
                });
                Response::builder().status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from_stream(prefix.chain(delayed))).unwrap()
            },
            _ => Json(json!({"Id":"fixture-server","ServerName":"Draft Emby","Version":"4.10.2.0"})).into_response(),
        },
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Fixture {
    state: AppState,
    app: Router,
    host: Host,
    draft_url: String,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("emby_check_{}", uuid::Uuid::new_v4().simple());
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
        sqlx::query("INSERT INTO emby_users(id,name,is_administrator) VALUES('admin','Fixture',true),('user','Fixture',false)")
            .execute(&state.db).await.unwrap();
        let host = Host::default();
        let router = Router::new().fallback(upstream).with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.save_config(&json!({"emby_server_url":format!("{base}/stored"),"emby_api_key":"stored-secret-key","emby_api_timeout":60,"tmdb_api_key":"other-private-setting"})).await.unwrap();
        let app = emby_toolkit::router(state.clone());
        Self {
            state,
            app,
            host,
            draft_url: format!("{base}/draft"),
            root,
            schema,
            _directory: directory,
            server,
        }
    }
    fn body(&self) -> Value {
        json!({"emby_server_url":self.draft_url,"emby_api_key":"draft-secret-key"})
    }
    fn cookie(&self, user: &str, setup_mode: bool) -> String {
        format!(
            "etk_session={}",
            self.state.sessions.sign(&Session {
                user_id: user.into(),
                name: "Fixture".into(),
                setup_mode,
                expires: chrono::Utc::now().timestamp() + 60,
            })
        )
    }
    async fn request(&self, cookie: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post("/api/config/emby_check")
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn settings(&self) -> Vec<(String, Value)> {
        sqlx::query_as("SELECT setting_key,value_json FROM app_settings ORDER BY setting_key")
            .fetch_all(&self.state.db)
            .await
            .unwrap()
    }
    async fn cleanup(self) {
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn draft_check_authenticates_system_info_without_saving_or_starting_tasks() {
    let f = Fixture::new().await;
    let before_config = f.state.config.read().await.clone();
    let before_settings = f.settings().await;
    let before_jobs = f.state.jobs.status().await;
    let (status, result) = f.request(&f.cookie("admin", false), f.body()).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["success"], true);
    assert_eq!(
        result["data"],
        json!({"name":"Draft Emby","version":"4.10.2.0"})
    );
    let calls = f.host.calls.lock().await.clone();
    let checks = calls
        .iter()
        .filter(|call| call.1 == "/draft/System/Info")
        .collect::<Vec<_>>();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].0, "GET");
    assert_eq!(checks[0].2, "draft-secret-key");
    assert!(
        checks[0].3.is_empty(),
        "API key must not be placed in the URL"
    );
    assert!(calls.iter().all(|call| call.0 == "GET"));
    assert!(calls.iter().all(|call| !call.1.contains("Public")));
    assert_eq!(*f.state.config.read().await, before_config);
    assert_eq!(f.settings().await, before_settings);
    assert_eq!(f.state.jobs.status().await, before_jobs);
    assert!(!result.to_string().contains("draft-secret-key"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn connection_check_requires_current_admin_not_anonymous_user_or_recovery() {
    let f = Fixture::new().await;
    for cookie in [String::new(), f.cookie("user", false), f.cookie("", true)] {
        assert_eq!(f.request(&cookie, f.body()).await.0, StatusCode::FORBIDDEN);
    }
    assert!(f.host.calls.lock().await.is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn invalid_connection_drafts_fail_before_any_upstream_request() {
    let f = Fixture::new().await;
    let admin = f.cookie("admin", false);
    let mut invalid = vec![json!({}), json!([])];
    for url in [
        "",
        "not a url",
        "ftp://example.test",
        "http://user:password@example.test",
        "http://example.test?api_key=secret",
        "http://example.test/#fragment",
    ] {
        let mut body = f.body();
        body["emby_server_url"] = json!(url);
        invalid.push(body);
    }
    for key in ["", "\ninvalid-header"] {
        let mut body = f.body();
        body["emby_api_key"] = json!(key);
        invalid.push(body);
    }
    for timeout in [
        json!(0),
        json!(-1),
        json!(601),
        json!(1.5),
        json!("60"),
        Value::Null,
    ] {
        let mut body = f.body();
        body["emby_api_timeout"] = timeout;
        invalid.push(body);
    }
    for body in invalid {
        let (status, result) = f.request(&admin, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{result}");
        assert_eq!(result["success"], false);
    }
    assert!(f.host.calls.lock().await.is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn rejected_key_is_business_failure_without_exposing_key_or_logging_out() {
    let f = Fixture::new().await;
    f.host.mode.store(1, Ordering::SeqCst);
    let before = f.state.config.read().await.clone();
    let settings = f.settings().await;
    let (status, result) = f.request(&f.cookie("admin", false), f.body()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(result["success"], false);
    assert!(result["message"].as_str().unwrap().contains("API Key 无效"));
    assert!(!result.to_string().contains("draft-secret-key"));
    assert_eq!(*f.state.config.read().await, before);
    assert_eq!(f.settings().await, settings);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn invalid_system_info_and_missing_endpoint_cannot_report_success() {
    let f = Fixture::new().await;
    let admin = f.cookie("admin", false);
    for mode in [2, 3, 4] {
        f.host.mode.store(mode, Ordering::SeqCst);
        let (status, result) = f.request(&admin, f.body()).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{result}");
        assert_eq!(result["success"], false);
        assert!(!result.to_string().contains("draft-secret-key"));
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn connection_probe_waits_for_existing_main_slot_and_cooldown() {
    let f = Fixture::new().await;
    let primary = emby_toolkit::strm::slot(&f.state, "main", None)
        .await
        .unwrap();
    {
        let cookie = f.cookie("admin", false);
        let request = f.request(&cookie, f.body());
        tokio::pin!(request);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut request)
                .await
                .is_err()
        );
        assert!(
            !f.host
                .calls
                .lock()
                .await
                .iter()
                .any(|call| call.1 == "/draft/System/Info")
        );
        let released = tokio::time::Instant::now();
        drop(primary);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(700), &mut request)
                .await
                .is_err()
        );
        assert!(
            !f.host
                .calls
                .lock()
                .await
                .iter()
                .any(|call| call.1 == "/draft/System/Info")
        );
        let (status, result) =
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut request)
                .await
                .unwrap();
        assert_eq!(status, StatusCode::OK, "{result}");
        assert!(released.elapsed() >= std::time::Duration::from_millis(900));
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn draft_probe_still_runs_when_current_plugin_settings_cannot_be_read() {
    let f = Fixture::new().await;
    let stored = f.state.config.read().await["emby_server_url"]
        .as_str()
        .unwrap()
        .replace("/stored", "/missing");
    f.state.config.write().await["emby_server_url"] = json!(stored);
    let before = f.state.config.read().await.clone();
    let (status, result) = f.request(&f.cookie("admin", false), f.body()).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["success"], true);
    assert_eq!(*f.state.config.read().await, before);
    let calls = f.host.calls.lock().await.clone();
    assert!(calls.iter().any(|call| call.1.starts_with("/missing/")));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.1 == "/draft/System/Info")
            .count(),
        1
    );
    assert!(calls.iter().all(|call| call.0 == "GET"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn connection_check_rejects_redirects_without_forwarding_key_or_using_public_info() {
    let f = Fixture::new().await;
    let redirected_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let received = redirected_calls.clone();
    let target = Router::new().fallback(move || {
        let received = received.clone();
        async move {
            received.fetch_add(1, Ordering::SeqCst);
            Json(json!({"Id":"other","ServerName":"Public server","Version":"4.10.2.0"}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let external = format!(
        "http://{}/System/Info/Public",
        listener.local_addr().unwrap()
    );
    let target_server = tokio::spawn(async move { axum::serve(listener, target).await.unwrap() });
    f.host.mode.store(5, Ordering::SeqCst);
    for redirect in [external, format!("{}/System/Info/Public", f.draft_url)] {
        *f.host.redirect.lock().await = redirect;
        let (status, result) = f.request(&f.cookie("admin", false), f.body()).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{result}");
        assert_eq!(result["success"], false);
        assert!(!result.to_string().contains("draft-secret-key"));
    }
    assert_eq!(redirected_calls.load(Ordering::SeqCst), 0);
    assert!(
        !f.host
            .calls
            .lock()
            .await
            .iter()
            .any(|call| call.1.contains("Public"))
    );
    target_server.abort();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn explicit_timeout_also_limits_response_body_reading() {
    let f = Fixture::new().await;
    f.host.mode.store(6, Ordering::SeqCst);
    let mut body = f.body();
    body["emby_api_timeout"] = json!(1);
    let (status, result) = f.request(&f.cookie("admin", false), body).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{result}");
    assert_eq!(result["success"], false);
    assert!(result["message"].as_str().unwrap().contains("请求超时"));
    assert_eq!(
        f.host
            .calls
            .lock()
            .await
            .iter()
            .filter(|call| call.1 == "/draft/System/Info")
            .count(),
        1
    );
    f.cleanup().await;
}
