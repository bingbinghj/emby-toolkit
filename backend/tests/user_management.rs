use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::Session};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;
use tower::ServiceExt;

#[derive(Clone)]
struct EmbyMock {
    users: Arc<Mutex<Vec<Value>>>,
    writes: Arc<Mutex<Vec<(String, Value)>>>,
    fail_policy: Arc<AtomicBool>,
    ignore_profile: Arc<AtomicBool>,
}
async fn native_get(State(mock): State<EmbyMock>, Path(id): Path<String>) -> Response {
    let users = mock.users.lock().await;
    let target = if id == "wrong" { "member" } else { &id };
    users
        .iter()
        .find(|user| user["Id"] == target)
        .cloned()
        .map(|user| Json(user).into_response())
        .unwrap_or_else(|| {
            (StatusCode::NOT_FOUND, Json(json!({"error":"not found"}))).into_response()
        })
}
async fn native_profile(
    State(mock): State<EmbyMock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    mock.writes
        .lock()
        .await
        .push((format!("Users/{id}"), body.clone()));
    if !mock.ignore_profile.load(Ordering::SeqCst) {
        let mut users = mock.users.lock().await;
        let user = users.iter_mut().find(|user| user["Id"] == id).unwrap();
        user["Name"] = body["Name"].clone();
        // Matches Emby 4.10.2: the endpoint also replaces Configuration when only the name changed.
        user["Configuration"] = body["Configuration"].clone();
    }
    StatusCode::NO_CONTENT
}
async fn native_policy(
    State(mock): State<EmbyMock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    mock.writes
        .lock()
        .await
        .push((format!("Users/{id}/Policy"), body.clone()));
    if mock.fail_policy.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let mut users = mock.users.lock().await;
    let user = users.iter_mut().find(|user| user["Id"] == id).unwrap();
    user["Policy"] = body;
    StatusCode::NO_CONTENT
}
async fn native_password(
    State(mock): State<EmbyMock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    mock.writes
        .lock()
        .await
        .push((format!("Users/{id}/Password"), body));
    StatusCode::NO_CONTENT
}
struct Fixture {
    state: AppState,
    app: Router,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    native: EmbyMock,
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
                "Id":id,"Name":name,"PrimaryImageTag":"avatar-tag","HasPassword":true,
                "Policy":{"IsAdministrator":admin,"IsDisabled":disabled,"UnneededSecret":"hidden-policy","EnableAllFolders":false,"EnabledFolders":["movies"]},
                "Configuration":{"OrderedViews":["movies"],"AudioLanguagePreference":"chi"},
                "AccessToken":"must-not-return-token", "LastActivityDate":"2026-11-10T10:00:00Z"
            })
        };
        let native = EmbyMock {
            users: Arc::new(Mutex::new(vec![
                native_user("admin", "Fixture", true, false),
                native_user("member", "Member", false, true),
            ])),
            writes: Arc::new(Mutex::new(vec![])),
            fail_policy: Arc::new(AtomicBool::new(false)),
            ignore_profile: Arc::new(AtomicBool::new(false)),
        };
        let host =
            Router::new()
                .route(
                    "/Users",
                    get(|State(mock): State<EmbyMock>| async move {
                        Json(mock.users.lock().await.clone())
                    }),
                )
                .route("/Users/{id}", get(native_get).post(native_profile))
                .route("/Users/{id}/Policy", post(native_policy))
                .route("/Users/{id}/Password", post(native_password))
                .route(
                    "/Users/{id}/Images/Primary",
                    post(
                        |State(mock): State<EmbyMock>, Path(id): Path<String>| async move {
                            mock.writes
                                .lock()
                                .await
                                .push((format!("POST Users/{id}/Images/Primary"), Value::Null));
                            StatusCode::SERVICE_UNAVAILABLE
                        },
                    )
                    .delete(
                        |State(mock): State<EmbyMock>, Path(id): Path<String>| async move {
                            mock.writes
                                .lock()
                                .await
                                .push((format!("DELETE Users/{id}/Images/Primary"), Value::Null));
                            mock.users
                                .lock()
                                .await
                                .iter_mut()
                                .find(|user| user["Id"] == id)
                                .unwrap()["PrimaryImageTag"] = Value::Null;
                            StatusCode::NO_CONTENT
                        },
                    ),
                )
                .with_state(native.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });
        state.config.write().await["emby_server_url"] = json!(base);
        state.config.write().await["emby_api_key"] = json!("fixture-key");
        let app = emby_toolkit::router(state.clone());
        Self {
            state,
            app,
            root,
            schema,
            _directory: directory,
            server,
            native,
        }
    }
    fn cookie(&self, administrator: bool, setup_mode: bool) -> String {
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
        let token = if administrator {
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
fn details(name: &str, administrator: bool, disabled: bool) -> Value {
    json!({"name":name,"is_administrator":administrator,"is_disabled":disabled})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn users_are_read_from_emby_without_credentials_or_notification_contacts() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO emby_users_extended(emby_user_id,notes,telegram_chat_id) VALUES('admin','Local note','private-contact')").execute(&f.state.db).await.unwrap();
    let (status, _, body) = f
        .request("/api/portal/users", &f.cookie(true, false), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0]["is_administrator"], true);
    assert_eq!(users[1]["is_disabled"], true);
    assert_eq!(users[1]["profile_image_tag"], "avatar-tag");
    for secret in ["must-not-return-token", "hidden-policy", "private-contact"] {
        assert!(!body.to_string().contains(secret));
    }
    assert!(users.iter().all(|user| user.get("Policy").is_none()
        && user.get("AccessToken").is_none()
        && user.get("telegram_chat_id").is_none()));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn edits_native_profile_and_policy_with_readback_and_preserves_user_configuration() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    let before = f.native.users.lock().await[1].clone();
    let (status, _, body) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(details(" New Member ", true, false)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["name"], "New Member");
    assert_eq!(body["user"]["is_administrator"], true);
    assert_eq!(body["user"]["is_disabled"], false);
    let writes = f.native.writes.lock().await;
    assert_eq!(writes[0].0, "Users/member");
    assert_eq!(writes[0].1["Configuration"], before["Configuration"]);
    assert_eq!(writes[1].0, "Users/member/Policy");
    assert_eq!(
        writes[1].1["EnabledFolders"],
        before["Policy"]["EnabledFolders"]
    );
    assert_eq!(writes[1].1["UnneededSecret"], "hidden-policy");
    drop(writes);
    let cached:Value=sqlx::query_scalar("SELECT jsonb_build_object('name',name,'administrator',is_administrator,'policy',policy_json) FROM emby_users WHERE id='member'").fetch_one(&f.state.db).await.unwrap();
    assert_eq!(cached["name"], "New Member");
    assert_eq!(cached["administrator"], true);
    assert_eq!(cached["policy"]["EnabledFolders"], json!(["movies"]));
    assert!(!body.to_string().contains("must-not-return-token"));
    assert!(!body.to_string().contains("hidden-policy"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn profile_failures_do_not_claim_success_and_reconcile_native_changes() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    f.native.fail_policy.store(true, Ordering::SeqCst);
    let (status, _, body) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(details("Renamed", false, false)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("未完整保存"));
    let cached:Value=sqlx::query_scalar("SELECT jsonb_build_object('name',name,'disabled',policy_json->'IsDisabled') FROM emby_users WHERE id='member'").fetch_one(&f.state.db).await.unwrap();
    assert_eq!(cached, json!({"name":"Renamed","disabled":true}));
    f.native.fail_policy.store(false, Ordering::SeqCst);
    f.native.ignore_profile.store(true, Ordering::SeqCst);
    let (status, _, body) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(details("Not Applied", false, true)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("未完整生效"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn rejects_unknown_fields_wrong_identity_and_missing_configuration_before_writing() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    for (path, body) in [
        (
            "/api/portal/users/member",
            json!({"notes":"old fields","telegram_chat_id":"123"}),
        ),
        (
            "/api/portal/users/member",
            json!({"name":"Name","is_administrator":"true","is_disabled":false}),
        ),
        ("/api/portal/users/member", details(" ", false, true)),
        ("/api/portal/users/wrong", details("Wrong", false, false)),
    ] {
        let (status, _, _) = f.request(path, &cookie, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    f.native.users.lock().await[1]["Configuration"] = Value::Null;
    let (status, _, body) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(details("Changed", false, true)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("配置尚未确认"));
    assert!(f.native.writes.lock().await.is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn protects_current_administrator_and_keeps_login_name_after_rename() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    for body in [
        details("Fixture", false, false),
        details("Fixture", true, true),
    ] {
        let (status, _, body) = f
            .request("/api/portal/users/admin", &cookie, Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("当前登录用户"));
    }
    f.native.users.lock().await[1]["Policy"]["IsAdministrator"] = json!(true);
    let (status, _, body) = f
        .request(
            "/api/portal/users/member",
            &cookie,
            Some(details("Member", false, true)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("先保存"));
    assert!(f.native.writes.lock().await.is_empty());
    let (status, headers, body) = f
        .request(
            "/api/portal/users/admin",
            &cookie,
            Some(details("Renamed Admin", true, false)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut updated = HeaderMap::new();
    updated.insert(header::COOKIE, headers[header::SET_COOKIE].clone());
    assert_eq!(
        f.state.sessions.from_headers(&updated).unwrap().name,
        "Renamed Admin"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn password_change_uses_native_contract_without_clearing_password_or_echoing_it() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    let (status, _, body) = f
        .request(
            "/api/portal/users/member/password",
            &cookie,
            Some(json!({"new_password":"fixture-password"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], "member");
    assert!(!body.to_string().contains("fixture-password"));
    assert_eq!(
        *f.native.writes.lock().await,
        vec![(
            "Users/member/Password".into(),
            json!({"Id":"member","NewPw":"fixture-password","ResetPassword":false})
        )]
    );
    for body in [
        json!({"new_password":""}),
        json!({"new_password":"x","ResetPassword":true}),
        json!({"new_password":123}),
    ] {
        let (status, _, _) = f
            .request("/api/portal/users/member/password", &cookie, Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _, _) = f
        .request(
            "/api/portal/users/wrong/password",
            &cookie,
            Some(json!({"new_password":"x"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(f.native.writes.lock().await.len(), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn user_management_requires_current_administrator_for_profiles_and_passwords() {
    let f = Fixture::new().await;
    for (path, body) in [
        ("/api/portal/users", None),
        (
            "/api/portal/users/member",
            Some(details("Member", false, true)),
        ),
        (
            "/api/portal/users/member/password",
            Some(json!({"new_password":"x"})),
        ),
    ] {
        let (status, _, _) = f.request(path, "", body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = f.request(path, &f.cookie(true, true), body.clone()).await;
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
    assert!(f.native.writes.lock().await.is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn failed_avatar_upload_does_not_delete_the_existing_emby_avatar() {
    let f = Fixture::new().await;
    let request = Request::post("/api/portal/upload-avatar")
        .header(header::COOKIE,f.cookie(true,false))
        .header(header::CONTENT_TYPE,"multipart/form-data; boundary=avatar-fixture")
        .body(Body::from("--avatar-fixture\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"avatar.png\"\r\nContent-Type: image/png\r\n\r\nfixture-image\r\n--avatar-fixture--\r\n")).unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let writes = f.native.writes.lock().await;
    assert!(!writes.is_empty());
    assert!(
        writes
            .iter()
            .all(|(path, _)| path == "POST Users/admin/Images/Primary")
    );
    drop(writes);
    assert_eq!(
        f.native.users.lock().await[0]["PrimaryImageTag"],
        "avatar-tag"
    );
    f.cleanup().await;
}
