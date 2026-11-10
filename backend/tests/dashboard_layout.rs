use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
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
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("dashboard_layout_{}", uuid::Uuid::new_v4().simple());
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
        let app = emby_toolkit::router(state.clone());
        Self {
            state,
            app,
            root,
            schema,
            _directory: directory,
        }
    }

    fn cookie(&self, user: &str, setup: bool, expires: i64) -> String {
        format!(
            "etk_session={}",
            self.state.sessions.sign(&Session {
                user_id: user.into(),
                name: user.into(),
                setup_mode: setup,
                expires: chrono::Utc::now().timestamp() + expires,
            })
        )
    }

    async fn request(
        &self,
        profile: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let path = format!("/api/dashboard/layout/{profile}");
        let request = if body.is_some() {
            Request::post(path)
        } else {
            Request::get(path)
        }
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            body.map(|body| Body::from(body.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn cleanup(self) {
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

fn layout() -> Value {
    json!({
        "order": ["storage", "statistics", "recent", "libraries"],
        "hidden": ["recent"],
        "items": {"storage": {"x":0,"y":4,"w":4,"h":8,"manualHeight":true}},
        "updatedAt": 1794300000000_u64,
    })
}

#[tokio::test]
async fn signed_non_admin_layouts_are_isolated_by_user_and_device_and_survive_new_clients() {
    let fixture = Fixture::new().await;
    let first = fixture.cookie("viewer-one", false, 60);
    let second = fixture.cookie("viewer-two", false, 60);
    assert_eq!(
        fixture.request("desktop", &first, None).await,
        (StatusCode::OK, json!({"layout":null}))
    );
    let submitted = json!({"layout":layout()});
    assert_eq!(
        fixture
            .request("desktop", &first, Some(submitted.clone()))
            .await,
        (StatusCode::OK, submitted.clone())
    );
    assert_eq!(fixture.request("desktop", &first, None).await.1, submitted);
    assert_eq!(
        fixture.request("desktop", &second, None).await.1,
        json!({"layout":null})
    );
    assert_eq!(
        fixture.request("tablet", &first, None).await.1,
        json!({"layout":null})
    );
    let new_app = emby_toolkit::router(fixture.state.clone());
    let response = new_app
        .oneshot(
            Request::get("/api/dashboard/layout/desktop")
                .header(header::COOKIE, &first)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let saved: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(saved, submitted);
    fixture.cleanup().await;
}

#[tokio::test]
async fn malformed_layouts_cannot_replace_a_successful_layout_or_select_another_user() {
    let fixture = Fixture::new().await;
    let cookie = fixture.cookie("viewer", false, 60);
    let saved = json!({"layout":layout()});
    assert_eq!(
        fixture
            .request("desktop", &cookie, Some(saved.clone()))
            .await
            .0,
        StatusCode::OK
    );
    let mut invalid = vec![];
    for (field, value) in [
        (
            "order",
            json!(["storage", "storage", "recent", "libraries"]),
        ),
        (
            "order",
            json!(["storage", "statistics", "recent", "unknown"]),
        ),
        ("hidden", json!(["storage", "storage"])),
        ("updatedAt", json!(-1)),
        ("updatedAt", json!(9_007_199_254_740_992_u64)),
        ("user_id", json!("someone-else")),
    ] {
        let mut candidate = layout();
        candidate[field] = value;
        invalid.push(candidate);
    }
    for (field, value) in [
        ("x", json!(9)),
        ("y", json!(10001)),
        ("w", json!(0)),
        ("h", json!(1001)),
        ("h", json!(1.5)),
        ("manualHeight", json!("true")),
    ] {
        let mut candidate = layout();
        candidate["items"]["storage"][field] = value;
        invalid.push(candidate);
    }
    let mut unknown = layout();
    unknown["items"]["unknown"] = json!({"x":0,"y":0,"w":1,"h":1});
    invalid.push(unknown);
    for candidate in invalid {
        assert_eq!(
            fixture
                .request("desktop", &cookie, Some(json!({"layout":candidate})))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        fixture
            .request(
                "desktop",
                &cookie,
                Some(json!({"layout":layout(),"user_id":"other"}))
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request("mobile", &cookie, Some(json!({"layout":layout()})))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture.request("unknown", &cookie, None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fixture.request("desktop", &cookie, None).await.1, saved);
    fixture.cleanup().await;
}

#[tokio::test]
async fn authentication_and_body_limits_apply_to_layout_reads_and_writes() {
    let fixture = Fixture::new().await;
    for cookie in [
        String::new(),
        fixture.cookie("viewer", true, 60),
        fixture.cookie("viewer", false, -60),
        fixture.cookie("", false, 60),
        "etk_session=forged.token".into(),
    ] {
        assert_eq!(
            fixture.request("desktop", &cookie, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            fixture
                .request("desktop", &cookie, Some(json!({"layout":layout()})))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let cookie = fixture.cookie("viewer", false, 60);
    assert_eq!(
        fixture
            .request(
                "desktop",
                &cookie,
                Some(json!({"layout": "x".repeat(17000)}))
            )
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        fixture.request("desktop", &cookie, None).await.1,
        json!({"layout":null})
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn empty_widget_layouts_and_missing_optional_times_are_valid_but_corrupt_saved_data_is_explicit()
 {
    let fixture = Fixture::new().await;
    let cookie = fixture.cookie("viewer", false, 60);
    let mut value = layout();
    value["items"] = json!({});
    value.as_object_mut().unwrap().remove("updatedAt");
    let submitted = json!({"layout":value});
    assert_eq!(
        fixture
            .request("mobile", &cookie, Some(submitted.clone()))
            .await,
        (StatusCode::OK, submitted)
    );
    emby_toolkit::db::save_setting(
        &fixture.state.db,
        "dashboard-layout-v2:viewer:desktop",
        &json!({"broken":true}),
    )
    .await
    .unwrap();
    let (status, body) = fixture.request("desktop", &cookie, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());
    assert!(body.get("layout").is_none());
    fixture.cleanup().await;
}
