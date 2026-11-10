use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
    routing::get,
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
        let schema = format!("user_management_{}", uuid::Uuid::new_v4().simple());
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
            "INSERT INTO emby_users(id,name,is_administrator) VALUES('admin','Fixture',true)",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let native_user = |id: &str, name: &str, admin: bool, disabled: bool| {
            json!({
                "Id":id,"Name":name,"PrimaryImageTag":"avatar-tag",
                "Policy":{"IsAdministrator":admin,"IsDisabled":disabled,"UnneededSecret":"hidden-policy"},
                "AccessToken":"must-not-return-token", "LastActivityDate":"2026-11-10T10:00:00Z"
            })
        };
        let admin = native_user("admin", "Fixture", true, false);
        let member = native_user("member", "Member", false, true);
        let all = json!([admin, member]);
        let host = Router::new()
            .route(
                "/Users",
                get(move || {
                    let all = all.clone();
                    async move { Json(all) }
                }),
            )
            .route(
                "/Users/member",
                get(move || {
                    let member = member.clone();
                    async move { Json(member) }
                }),
            )
            .route(
                "/Users/wrong",
                get(|| async { Json(json!({"Id":"member","Name":"Wrong target"})) }),
            );
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
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn users_are_read_from_emby_with_only_management_fields_and_local_details() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO emby_users_extended(emby_user_id,notes,telegram_chat_id) VALUES('admin','Local note','123')")
        .execute(&f.state.db).await.unwrap();
    let (status, _, body) = f
        .request("/api/portal/users", &f.cookie(true, false), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0]["notes"], "Local note");
    assert_eq!(users[0]["telegram_chat_id"], "123");
    assert_eq!(users[0]["is_administrator"], true);
    assert_eq!(users[1]["is_administrator"], false);
    assert_eq!(users[1]["is_disabled"], true);
    assert_eq!(users[1]["profile_image_tag"], "avatar-tag");
    assert!(!body.to_string().contains("must-not-return-token"));
    assert!(!body.to_string().contains("hidden-policy"));
    assert!(
        users
            .iter()
            .all(|user| user.get("Policy").is_none() && user.get("AccessToken").is_none())
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn editing_user_notification_details_preserves_native_role_and_checks_identity() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    let (status, _, body) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(json!({"notes":" New note ","telegram_chat_id":" 456 "})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], "member");
    let details:Value=sqlx::query_scalar("SELECT jsonb_build_object('is_administrator',u.is_administrator,'notes',e.notes,'chat',e.telegram_chat_id) FROM emby_users u JOIN emby_users_extended e ON e.emby_user_id=u.id WHERE u.id='member'")
        .fetch_one(&f.state.db).await.unwrap();
    assert_eq!(
        details,
        json!({"is_administrator":false,"notes":"New note","chat":"456"})
    );
    let (status, _, _) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(json!({"notes":"overwrite","telegram_chat_id":"","is_administrator":true})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = f
        .request(
            "/api/portal/users/wrong",
            &cookie,
            Some(json!({"notes":"wrong","telegram_chat_id":"wrong"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let notes: String =
        sqlx::query_scalar("SELECT notes FROM emby_users_extended WHERE emby_user_id='member'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(notes, "New note");
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn user_management_requires_current_administrator() {
    let f = Fixture::new().await;
    for path in ["/api/portal/users", "/api/portal/users/member"] {
        let body = path
            .ends_with("member")
            .then(|| json!({"notes":"","telegram_chat_id":""}));
        let (status, _, _) = f.request(path, "", body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        sqlx::query("UPDATE emby_users SET is_administrator=false WHERE id='admin'")
            .execute(&f.state.db)
            .await
            .unwrap();
        let (status, _, _) = f.request(path, &f.cookie(true, false), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        sqlx::query("UPDATE emby_users SET is_administrator=true WHERE id='admin'")
            .execute(&f.state.db)
            .await
            .unwrap();
    }
    f.cleanup().await;
}
