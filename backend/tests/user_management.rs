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
    fail_password: Arc<AtomicBool>,
    fail_create: Arc<AtomicBool>,
    avatar_success: Arc<AtomicBool>,
    ignore_avatar_delete: Arc<AtomicBool>,
    ignore_user_delete: Arc<AtomicBool>,
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
    if mock.fail_password.load(Ordering::SeqCst) {
        return StatusCode::BAD_REQUEST;
    }
    mock.users
        .lock()
        .await
        .iter_mut()
        .find(|user| user["Id"] == id)
        .unwrap()["HasPassword"] = json!(true);
    StatusCode::NO_CONTENT
}
async fn native_create(State(mock): State<EmbyMock>, Json(body): Json<Value>) -> Response {
    mock.writes
        .lock()
        .await
        .push(("Users/New".into(), body.clone()));
    if mock.fail_create.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut created = mock.users.lock().await[1].clone();
    created["Id"] = json!("created-member");
    created["Name"] = body["Name"].clone();
    created["HasPassword"] = json!(false);
    created["Policy"]["IsDisabled"] = json!(false);
    mock.users.lock().await.push(created.clone());
    Json(created).into_response()
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
                "Policy":{"IsAdministrator":admin,"IsDisabled":disabled,"UnneededSecret":"hidden-policy","EnableAllFolders":false,"EnabledFolders":["movies"],"EnableRemoteAccess":true,"EnableMediaPlayback":true,"EnableContentDownloading":true},
                "Configuration":{"OrderedViews":["movies"],"AudioLanguagePreference":"chi","SubtitleMode":"Smart","PlayDefaultAudioTrack":true},
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
            fail_password: Arc::new(AtomicBool::new(false)),
            fail_create: Arc::new(AtomicBool::new(false)),
            avatar_success: Arc::new(AtomicBool::new(false)),
            ignore_avatar_delete: Arc::new(AtomicBool::new(false)),
            ignore_user_delete: Arc::new(AtomicBool::new(false)),
        };
        let host =
            Router::new()
                .route(
                    "/Users",
                    get(|State(mock): State<EmbyMock>| async move {
                        Json(mock.users.lock().await.clone())
                    }),
                )
                .route("/Users/New",post(native_create))
                .route("/Library/VirtualFolders",get(||async { Json(json!([{"ItemId":"movies","Name":"Movies","Locations":["private-path"]},{"ItemId":"tv","Name":"TV"}])) }))
                .route("/Users/{id}", get(native_get).post(native_profile).delete(|State(mock):State<EmbyMock>,Path(id):Path<String>|async move {
                    mock.writes.lock().await.push((format!("DELETE Users/{id}"),Value::Null));
                    if !mock.ignore_user_delete.load(Ordering::SeqCst) { mock.users.lock().await.retain(|user|user["Id"] != id); }
                    StatusCode::NO_CONTENT
                }))
                .route("/Users/{id}/Configuration",post(|State(mock):State<EmbyMock>,Path(id):Path<String>,Json(body):Json<Value>|async move {
                    mock.writes.lock().await.push((format!("Users/{id}/Configuration"),body.clone()));
                    if !mock.ignore_profile.load(Ordering::SeqCst) { mock.users.lock().await.iter_mut().find(|user|user["Id"] == id).unwrap()["Configuration"] = body; }
                    StatusCode::NO_CONTENT
                }))
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
                            if mock.avatar_success.load(Ordering::SeqCst) {
                                mock.users.lock().await.iter_mut().find(|user|user["Id"] == id).unwrap()["PrimaryImageTag"] = json!("updated-avatar");
                                StatusCode::NO_CONTENT
                            } else { StatusCode::SERVICE_UNAVAILABLE }
                        },
                    )
                    .delete(
                        |State(mock): State<EmbyMock>, Path(id): Path<String>| async move {
                            mock.writes
                                .lock()
                                .await
                                .push((format!("DELETE Users/{id}/Images/Primary"), Value::Null));
                            if !mock.ignore_avatar_delete.load(Ordering::SeqCst) { mock.users
                                .lock()
                                .await
                                .iter_mut()
                                .find(|user| user["Id"] == id)
                                .unwrap()["PrimaryImageTag"] = Value::Null; }
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
        let reason = body["error"].as_str().unwrap();
        assert!(
            reason.contains("当前登录用户") || reason.contains("不能禁用管理员"),
            "{body}"
        );
        assert!(f.native.writes.lock().await.is_empty());
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
    let request = avatar_request("/api/portal/upload-avatar", &f.cookie(true, false));
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

fn avatar_request(path: &str, cookie: &str) -> Request<Body> {
    let mut image = std::io::Cursor::new(vec![]);
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let mut body=b"--avatar-fixture\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"avatar.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
    body.extend_from_slice(image.get_ref());
    body.extend_from_slice(b"\r\n--avatar-fixture--\r\n");
    Request::post(path)
        .header(header::COOKIE, cookie)
        .header(
            header::CONTENT_TYPE,
            "multipart/form-data; boundary=avatar-fixture",
        )
        .body(Body::from(body))
        .unwrap()
}
async fn delete_request(f: &Fixture, path: &str, cookie: &str) -> (StatusCode, Value) {
    let request = Request::delete(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn details_show_confirmed_permissions_libraries_and_no_native_secrets() {
    let f = Fixture::new().await;
    let (status, _, body) = f
        .request("/api/portal/users/member", &f.cookie(true, false), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["policy"]["EnableRemoteAccess"], true);
    assert_eq!(body["user"]["configuration"]["SubtitleMode"], "Smart");
    assert_eq!(
        body["libraries"]["items"],
        json!([{"id":"movies","name":"Movies"},{"id":"tv","name":"TV"}])
    );
    for secret in ["hidden-policy", "must-not-return-token", "private-path"] {
        assert!(!body.to_string().contains(secret));
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn permission_and_configuration_patches_preserve_untouched_native_fields() {
    let f = Fixture::new().await;
    let mut body = details("Member", false, true);
    body["policy"] = json!({"EnableRemoteAccess":false,"EnabledFolders":["tv"]});
    body["configuration"] = json!({"SubtitleMode":"None"});
    let (status, _, saved) = f
        .request(
            "/api/portal/users/member",
            &f.cookie(true, false),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let user = f.native.users.lock().await[1].clone();
    assert_eq!(user["Policy"]["UnneededSecret"], "hidden-policy");
    assert_eq!(user["Policy"]["EnableContentDownloading"], true);
    assert_eq!(user["Configuration"]["OrderedViews"], json!(["movies"]));
    assert_eq!(user["Configuration"]["AudioLanguagePreference"], "chi");
    assert_eq!(user["Policy"]["EnableRemoteAccess"], false);
    assert_eq!(user["Policy"]["EnabledFolders"], json!(["tv"]));
    assert_eq!(user["Configuration"]["SubtitleMode"], "None");
    assert_eq!(
        f.native.writes.lock().await[0].0,
        "Users/member/Configuration"
    );
    assert_eq!(f.native.writes.lock().await[1].0, "Users/member/Policy");
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn unsupported_permission_fields_and_unconfirmed_configuration_cannot_write() {
    let f = Fixture::new().await;
    for patch in [
        json!({"IsAdministrator":true}),
        json!({"AuthenticationProviderId":"other"}),
        json!({"EnableRemoteAccess":1}),
        json!({"EnabledFolders":["../path"]}),
        json!({"Unknown":true}),
    ] {
        let mut body = details("Member", false, true);
        body["policy"] = patch;
        assert_eq!(
            f.request(
                "/api/portal/users/member",
                &f.cookie(true, false),
                Some(body)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(f.native.writes.lock().await.is_empty());
    f.native.ignore_profile.store(true, Ordering::SeqCst);
    let mut body = details("Member", false, true);
    body["configuration"] = json!({"SubtitleMode":"None"});
    let (status, _, result) = f
        .request(
            "/api/portal/users/member",
            &f.cookie(true, false),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(result["error"].as_str().unwrap().contains("未完整生效"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn creates_real_user_password_and_role_and_prevents_duplicate_creation() {
    let f = Fixture::new().await;
    let body = json!({"name":"New user","new_password":"test-secret","is_administrator":false,"is_disabled":true});
    let (status, _, result) = f
        .request(
            "/api/portal/users",
            &f.cookie(true, false),
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["id"], "created-member");
    assert_eq!(result["user"]["is_disabled"], true);
    assert!(!result.to_string().contains("test-secret"));
    assert_eq!(
        f.native
            .writes
            .lock()
            .await
            .iter()
            .filter(|(path, _)| path == "Users/New")
            .count(),
        1
    );
    assert_eq!(
        f.request("/api/portal/users", &f.cookie(true, false), Some(body))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.native
            .writes
            .lock()
            .await
            .iter()
            .filter(|(path, _)| path == "Users/New")
            .count(),
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn failed_creation_is_not_replayed_and_partial_creation_has_precise_id_receipt() {
    let f = Fixture::new().await;
    let body = json!({"name":"New user","new_password":"test-secret","is_administrator":false,"is_disabled":false});
    f.native.fail_create.store(true, Ordering::SeqCst);
    let (status, _, result) = f
        .request(
            "/api/portal/users",
            &f.cookie(true, false),
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(result.get("created").is_none());
    assert_eq!(f.native.writes.lock().await.len(), 1);
    f.native.fail_create.store(false, Ordering::SeqCst);
    f.native.fail_password.store(true, Ordering::SeqCst);
    let (status, _, result) = f
        .request("/api/portal/users", &f.cookie(true, false), Some(body))
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(result["created"], true);
    assert_eq!(result["id"], "created-member");
    assert_eq!(result["success"], false);
    assert_eq!(result["user"]["has_password"], false);
    assert!(!result.to_string().contains("test-secret"));
    assert_eq!(f.native.users.lock().await.len(), 3);
    let cached: String =
        sqlx::query_scalar("SELECT name FROM emby_users WHERE id='created-member'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(cached, "New user");
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn partial_creation_password_retry_is_read_back_before_enabling_the_known_user() {
    let f = Fixture::new().await;
    let cookie = f.cookie(true, false);
    f.native.fail_password.store(true, Ordering::SeqCst);
    let (status, _, created) = f.request("/api/portal/users", &cookie,
        Some(json!({"name":"New user","new_password":"fixture-only","is_administrator":false,"is_disabled":false}))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(created["created"], true);
    assert_eq!(created["id"], "created-member");
    assert_eq!(created["user"]["has_password"], false);
    assert_eq!(created["user"]["is_disabled"], true);
    let writes_before = f.native.writes.lock().await.len();
    f.native.fail_password.store(false, Ordering::SeqCst);
    let (status, _, password) = f
        .request(
            "/api/portal/users/created-member/password",
            &cookie,
            Some(json!({"new_password":"fixture-only"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{password}");
    assert_eq!(password["id"], "created-member");
    let (status, _, confirmed) = f
        .request("/api/portal/users/created-member", &cookie, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["user"]["has_password"], true);
    assert_eq!(confirmed["user"]["is_disabled"], true);
    let (status, _, enabled) = f
        .request(
            "/api/portal/users/created-member",
            &cookie,
            Some(details("New user", false, false)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert_eq!(enabled["user"]["has_password"], true);
    assert_eq!(enabled["user"]["is_disabled"], false);
    let writes = f.native.writes.lock().await;
    assert_eq!(writes[writes_before].0, "Users/created-member/Password");
    assert_eq!(writes[writes_before + 1].0, "Users/created-member/Policy");
    assert_eq!(writes.len(), writes_before + 2);
    assert_eq!(
        writes.iter().filter(|write| write.0 == "Users/New").count(),
        1
    );
    drop(writes);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn any_user_avatar_updates_and_reset_require_native_readback() {
    let f = Fixture::new().await;
    f.native.avatar_success.store(true, Ordering::SeqCst);
    let response = f
        .app
        .clone()
        .oneshot(avatar_request(
            "/api/portal/users/member/avatar",
            &f.cookie(true, false),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["id"], "member");
    assert_eq!(body["new_tag"], "updated-avatar");
    assert!(
        f.native
            .writes
            .lock()
            .await
            .iter()
            .all(|(path, _)| path == "POST Users/member/Images/Primary")
    );
    f.native.ignore_avatar_delete.store(true, Ordering::SeqCst);
    let (status, body) = delete_request(
        &f,
        "/api/portal/users/member/avatar",
        &f.cookie(true, false),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("尚未确认"));
    f.native.ignore_avatar_delete.store(false, Ordering::SeqCst);
    let (status, body) = delete_request(
        &f,
        "/api/portal/users/member/avatar",
        &f.cookie(true, false),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["new_tag"], Value::Null);
    assert_eq!(
        f.native.users.lock().await[0]["PrimaryImageTag"],
        "avatar-tag"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn user_delete_protects_self_last_admin_and_unconfirmed_deletion() {
    let f = Fixture::new().await;
    assert_eq!(
        delete_request(&f, "/api/portal/users/admin", &f.cookie(true, false))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.native.users.lock().await[0]["Policy"]["IsAdministrator"] = json!(false);
    f.native.users.lock().await[1]["Policy"]["IsAdministrator"] = json!(true);
    f.native.users.lock().await[1]["Policy"]["IsDisabled"] = json!(false);
    assert_eq!(
        delete_request(&f, "/api/portal/users/member", &f.cookie(true, false))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert!(f.native.writes.lock().await.is_empty());
    f.native.users.lock().await[1]["Policy"]["IsAdministrator"] = json!(false);
    f.native.ignore_user_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        delete_request(&f, "/api/portal/users/member", &f.cookie(true, false))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.native.ignore_user_delete.store(false, Ordering::SeqCst);
    assert_eq!(
        delete_request(&f, "/api/portal/users/member", &f.cookie(true, false))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(f.native.users.lock().await.len(), 1);
    f.cleanup().await;
}
