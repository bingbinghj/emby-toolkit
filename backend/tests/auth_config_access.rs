use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::Session};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    app: Router,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("auth_config_{}", uuid::Uuid::new_v4().simple());
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
        sqlx::query(
            "INSERT INTO emby_users(id,name,is_administrator) VALUES('admin','Fixture',false)",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let host = Router::new()
            .route("/System/Info", get(|| async { Json(json!({"Id":"fixture-server"})) }))
            .route("/emby/System/Info", get(|| async { Json(json!({"Id":"fixture-server"})) }))
            .route("/Users/AuthenticateByName", post(|| async {
                Json(json!({"AccessToken":"fixture-token","User":{"Id":"admin","Name":"Fixture","Policy":{"IsAdministrator":true}}}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });
        state.config.write().await["emby_server_url"] = json!(base);
        state.config.write().await["emby_api_key"] = json!("fixture-key");
        state.config.write().await["tmdb_api_key"] = json!("private-tmdb-sentinel");
        let app = emby_toolkit::router(state.clone());
        Self {
            state,
            app,
            root,
            schema,
            _directory: directory,
            server,
        }
    }
    fn cookie(&self, claim: bool, setup_mode: bool) -> String {
        let session = Session {
            user_id: if setup_mode {
                String::new()
            } else {
                "admin".into()
            },
            name: "Fixture".into(),
            setup_mode,
            expires: chrono::Utc::now().timestamp() + 60,
        };
        let token = if claim {
            self.state.sessions.sign_administrator(&session)
        } else {
            self.state.sessions.sign(&session)
        };
        format!("etk_session={token}")
    }
    async fn request(
        &self,
        path: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let request = if body.is_some() {
            Request::post(path)
        } else {
            Request::get(path)
        }
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            body.map(|value| Body::from(value.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, headers, serde_json::from_slice(&body).unwrap())
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
async fn status_config_and_protected_routes_follow_current_admin_mirror() {
    let f = Fixture::new().await;
    let cookies = [f.cookie(true, false), f.cookie(false, false)];
    for is_admin in [false, true, false] {
        sqlx::query("UPDATE emby_users SET is_administrator=$1 WHERE id='admin'")
            .bind(is_admin)
            .execute(&f.state.db)
            .await
            .unwrap();
        for cookie in &cookies {
            let (_, _, status) = f.request("/api/auth/check_status", cookie, None).await;
            assert_eq!(
                status["status"],
                if is_admin {
                    "logged_in"
                } else {
                    "login_required"
                }
            );
            let (config_status, _, config) = f.request("/api/config", cookie, None).await;
            let (protected_status, _, _) = f.request("/api/status", cookie, None).await;
            let expected = if is_admin {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            };
            assert_eq!(config_status, expected);
            assert_eq!(protected_status, expected);
            if is_admin {
                assert_eq!(config["tmdb_api_key"], "private-tmdb-sentinel");
            } else {
                assert!(!config.to_string().contains("private-tmdb-sentinel"));
            }
        }
    }
    sqlx::query("DELETE FROM emby_users WHERE id='admin'")
        .execute(&f.state.db)
        .await
        .unwrap();
    assert_eq!(
        f.request("/api/auth/check_status", &cookies[0], None)
            .await
            .2["status"],
        "login_required"
    );
    assert_eq!(
        f.request("/api/status", &cookies[0], None).await.0,
        StatusCode::FORBIDDEN
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn unconfigured_config_hides_residual_secrets_and_preserves_initial_setup() {
    let f = Fixture::new().await;
    f.state.config.write().await["emby_api_key"] = json!("");
    f.state.config.write().await["p115_cookie"] = json!("private-cookie-sentinel");
    for cookie in [String::new(), f.cookie(true, false)] {
        let (status, _, config) = f.request("/api/config", &cookie, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(config, json!({}));
    }
    assert_eq!(
        f.request("/api/auth/check_status", "", None).await.2["status"],
        "setup_required"
    );
    let base = f.state.config.read().await["emby_server_url"].clone();
    let (status, _, result) = f
        .request(
            "/api/auth/setup",
            "",
            Some(json!({"url":base,"api_key":"new-fixture-key"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(f.state.configured().await);
    assert_eq!(
        f.state.config.read().await["tmdb_api_key"],
        "private-tmdb-sentinel"
    );
    assert_eq!(
        f.request("/api/config", "", None).await.0,
        StatusCode::FORBIDDEN
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn recovery_session_can_read_and_save_config_without_business_admin_access() {
    let f = Fixture::new().await;
    let recovery = f.cookie(false, true);
    let (status, _, config) = f.request("/api/config", &recovery, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["tmdb_api_key"], "private-tmdb-sentinel");
    let (status, _, result) = f
        .request("/api/config", &recovery, Some(json!({"item_delay":7})))
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(f.state.config.read().await["item_delay"], 7);
    assert_eq!(
        f.request("/api/status", &recovery, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request("/api/auth/check_status", &recovery, None).await.2["status"],
        "login_required"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn login_synchronizes_admin_mirror_before_issuing_session() {
    let f = Fixture::new().await;
    let (status, headers, result) = f
        .request(
            "/api/auth/login",
            "",
            Some(json!({"username":"Fixture","password":"fixture"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let cookie = headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(
        f.request("/api/auth/check_status", cookie, None).await.2["status"],
        "logged_in"
    );
    assert_eq!(
        f.request("/api/config", cookie, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("/api/status", cookie, None).await.0,
        StatusCode::OK
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn login_does_not_issue_successful_session_when_admin_mirror_write_fails() {
    let f = Fixture::new().await;
    sqlx::query("DROP TABLE emby_users CASCADE")
        .execute(&f.state.db)
        .await
        .unwrap();
    let (status, headers, result) = f
        .request(
            "/api/auth/login",
            "",
            Some(json!({"username":"Fixture","password":"fixture"})),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{result}");
    assert!(!headers.contains_key(header::SET_COOKIE));
    assert_ne!(result["status"], "ok");
    f.cleanup().await;
}
