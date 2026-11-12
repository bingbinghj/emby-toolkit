use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Query, State},
    http::{Method, Request, StatusCode, header},
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::Session, covers, db, media_preflight};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;

#[derive(Clone)]
struct Host {
    uploads: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    primary_downloads: Arc<AtomicUsize>,
    backdrop_downloads: Arc<AtomicUsize>,
    image: Arc<Vec<u8>>,
}
struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    host: Host,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("cover_warnings_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&search).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            96,
            image::Rgb([80, 120, 160]),
        ))
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
        let host = Host {
            uploads: Arc::default(),
            requests: Arc::default(),
            primary_downloads: Arc::default(),
            backdrop_downloads: Arc::default(),
            image: Arc::new(image.into_inner()),
        };
        let router = Router::new()
            .route("/emby/Users/admin/Views", get(|| async { Json(json!({"Items":[{"Id":"library","Name":"测试库","CollectionType":"movies"}]})) }))
            .route("/Users/admin/Items", get(|State(host): State<Host>, Query(query): Query<HashMap<String,String>>| async move {
                host.requests.fetch_add(1, Ordering::SeqCst);
                Json(if query.get("Limit").is_some_and(|limit| limit=="0") { json!({"Items":[],"TotalRecordCount":2}) }
                    else { json!({"Items":[{"Id":"bad","Name":"失败图片","ImageTags":{"Primary":"one"}},{"Id":"good","Name":"正常图片","ImageTags":{"Primary":"two"},"BackdropImageTags":["backdrop"]}],"TotalRecordCount":2}) })
            }))
            .route("/Items/bad/Images/Primary", get(|| async { StatusCode::BAD_GATEWAY }))
            .route("/Items/good/Images/Primary", get(|State(host): State<Host>| async move { host.primary_downloads.fetch_add(1, Ordering::SeqCst); ([("Content-Type","image/png")], host.image.as_ref().clone()) }))
            .route("/Items/good/Images/Backdrop/0", get(|State(host): State<Host>| async move { host.backdrop_downloads.fetch_add(1, Ordering::SeqCst); ([("Content-Type","image/png")], host.image.as_ref().clone()) }))
            .route("/Items/library/Images/Primary", post(|State(host): State<Host>| async move { host.uploads.fetch_add(1, Ordering::SeqCst); StatusCode::NO_CONTENT }))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","monitor_sha1_pc_search":false})).await.unwrap();
        Self {
            state,
            admin,
            schema,
            directory,
            host,
            server,
        }
    }
    async fn result(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn generate(&self, config: Value) -> Value {
        db::save_setting(&self.state.db, "cover_generator_config", &config)
            .await
            .unwrap();
        self.generate_saved().await
    }
    async fn generate_saved(&self) -> Value {
        let work = self.state.clone();
        let id = self
            .state
            .jobs
            .submit("封面警告验证", move |context| async move {
                covers::generate(&work, None, &context).await
            })
            .await
            .unwrap();
        self.result(&id).await
    }
    async fn enable_config_clients(&self) {
        sqlx::query("INSERT INTO emby_users(id,name,is_administrator) VALUES('cover-a','Cover A',true),('cover-b','Cover B',true)")
            .execute(&self.state.db).await.unwrap();
    }
    async fn config_request(
        &self,
        user: Option<&str>,
        method: Method,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri("/api/config/cover_generator")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(user) = user {
            let token = self.state.sessions.sign_administrator(&Session {
                user_id: user.into(),
                name: user.into(),
                setup_mode: false,
                expires: chrono::Utc::now().timestamp() + 300,
            });
            request = request.header(header::COOKIE, format!("etk_session={token}"));
        }
        let response = emby_toolkit::router(self.state.clone())
            .oneshot(
                request
                    .body(
                        body.map(|value| Body::from(value.to_string()))
                            .unwrap_or_else(Body::empty),
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or(json!({"raw":String::from_utf8_lossy(&bytes)})),
        )
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; HTTP clients are localhost"]
async fn partial_cover_clients_saving_different_sections_preserve_prior_saves() {
    let f = Fixture::new().await;
    f.enable_config_clients().await;
    db::save_setting(
        &f.state.db,
        "cover_generator_config",
        &json!({"future_option":{"keep":true}}),
    )
    .await
    .unwrap();
    let (first_status, first) = f.config_request(Some("cover-a"), Method::GET, None).await;
    let (second_status, second) = f.config_request(Some("cover-b"), Method::GET, None).await;
    assert_eq!(first["enabled"], false);
    assert_eq!(second["enabled"], false);
    let saved_first = f
        .config_request(
            Some("cover-a"),
            Method::PATCH,
            Some(json!({"enabled":true})),
        )
        .await;
    let saved_second = f
        .config_request(
            Some("cover-b"),
            Method::PATCH,
            Some(json!({"cover_style":"multi_1"})),
        )
        .await;
    let (status, observed) = f.config_request(Some("cover-a"), Method::GET, None).await;
    f.close().await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(saved_first.0, StatusCode::OK, "{}", saved_first.1);
    assert_eq!(saved_second.0, StatusCode::OK, "{}", saved_second.1);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        observed["enabled"], true,
        "other section was overwritten: {observed}"
    );
    assert_eq!(observed["cover_style"], "multi_1");
    assert_eq!(observed["future_option"], json!({"keep":true}));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; HTTP clients are localhost"]
async fn concurrent_cover_patches_preserve_both_cards_on_the_first_insert() {
    let f = Fixture::new().await;
    f.enable_config_clients().await;
    assert!(
        db::setting(&f.state.db, "cover_generator_config")
            .await
            .unwrap()
            .is_none()
    );
    let (first, second) = tokio::join!(
        f.config_request(
            Some("cover-a"),
            Method::PATCH,
            Some(json!({"enabled":true}))
        ),
        f.config_request(
            Some("cover-b"),
            Method::PATCH,
            Some(json!({"cover_style":"multi_1"}))
        )
    );
    let (_, observed) = f.config_request(Some("cover-a"), Method::GET, None).await;
    f.close().await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    assert_eq!(second.0, StatusCode::OK, "{}", second.1);
    assert_eq!(observed["enabled"], true);
    assert_eq!(observed["cover_style"], "multi_1");
    assert_eq!(observed["multi_1_use_primary"], true);
    assert_eq!(observed["zh_font_size"], 1.0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; HTTP clients are localhost"]
async fn cover_patch_keeps_explicit_clears_and_legacy_post_still_replaces_everything() {
    let f = Fixture::new().await;
    f.enable_config_clients().await;
    db::save_setting(&f.state.db, "cover_generator_config", &json!({"enabled":true,"cover_style":"multi_1","title_config":"电影: [旧标题, Old]","exclude_libraries":["old-library"],"future_option":{"keep":true}})).await.unwrap();
    let cleared = f
        .config_request(
            Some("cover-a"),
            Method::PATCH,
            Some(json!({"title_config":"","exclude_libraries":[],"en_font_url":null})),
        )
        .await;
    let (_, patched) = f.config_request(Some("cover-a"), Method::GET, None).await;
    let replaced = f
        .config_request(
            Some("cover-b"),
            Method::POST,
            Some(json!({"enabled":false})),
        )
        .await;
    let raw = db::setting(&f.state.db, "cover_generator_config")
        .await
        .unwrap();
    let (_, reset) = f.config_request(Some("cover-a"), Method::GET, None).await;
    f.close().await;
    assert_eq!(cleared.0, StatusCode::OK);
    assert_eq!(patched["enabled"], true);
    assert_eq!(patched["cover_style"], "multi_1");
    assert_eq!(patched["title_config"], "");
    assert_eq!(patched["exclude_libraries"], json!([]));
    assert!(patched["en_font_url"].is_null());
    assert_eq!(patched["future_option"], json!({"keep":true}));
    assert_eq!(replaced.0, StatusCode::OK);
    assert_eq!(raw, Some(json!({"enabled":false})));
    assert_eq!(reset["cover_style"], covers::defaults()["cover_style"]);
    assert_eq!(reset["title_config"], covers::defaults()["title_config"]);
    assert!(reset.get("future_option").is_none());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; HTTP clients are localhost"]
async fn invalid_or_unauthorized_cover_patches_do_not_create_configuration() {
    let f = Fixture::new().await;
    f.enable_config_clients().await;
    for user in [None, Some("unknown-user")] {
        let result = f
            .config_request(user, Method::PATCH, Some(json!({"enabled":true})))
            .await;
        assert_eq!(result.0, StatusCode::FORBIDDEN, "{}", result.1);
    }
    for invalid in [json!(null), json!([]), json!(1), json!("invalid")] {
        let result = f
            .config_request(Some("cover-a"), Method::PATCH, Some(invalid))
            .await;
        assert_eq!(result.0, StatusCode::BAD_REQUEST, "{}", result.1);
    }
    let observed = db::setting(&f.state.db, "cover_generator_config")
        .await
        .unwrap();
    f.close().await;
    assert!(observed.is_none());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; HTTP clients are localhost"]
async fn a_failed_cover_sql_patch_leaves_the_saved_configuration_intact() {
    let f = Fixture::new().await;
    f.enable_config_clients().await;
    let original = json!({"enabled":true,"future_option":{"keep":true}});
    db::save_setting(&f.state.db, "cover_generator_config", &original)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION reject_cover_patch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.setting_key='cover_generator_config' AND NEW.value_json ? 'reject_patch' THEN RAISE EXCEPTION 'fixture cover write rejected'; END IF; RETURN NEW; END $$; CREATE TRIGGER cover_write_failure BEFORE INSERT OR UPDATE ON app_settings FOR EACH ROW EXECUTE FUNCTION reject_cover_patch();")
        .execute(&f.state.db).await.unwrap();
    let failed = f
        .config_request(
            Some("cover-a"),
            Method::PATCH,
            Some(json!({"enabled":false,"reject_patch":true})),
        )
        .await;
    let observed = db::setting(&f.state.db, "cover_generator_config")
        .await
        .unwrap();
    f.close().await;
    assert!(failed.0.is_server_error(), "{}", failed.1);
    assert_eq!(observed, Some(original));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn cover_generation_uses_the_complete_defaults_after_a_partial_first_save() {
    let f = Fixture::new().await;
    f.enable_config_clients().await;
    let saved = f
        .config_request(
            Some("cover-a"),
            Method::PATCH,
            Some(json!({"enabled":true,"cover_style":"multi_1","multi_1_use_main_font":true})),
        )
        .await;
    assert_eq!(saved.0, StatusCode::OK, "{}", saved.1);
    let raw = db::setting(&f.state.db, "cover_generator_config")
        .await
        .unwrap()
        .unwrap();
    assert!(raw.get("multi_1_use_primary").is_none());
    let (_, view) = f.config_request(Some("cover-a"), Method::GET, None).await;
    assert_eq!(view["multi_1_use_primary"], true);
    let job = f.generate_saved().await;
    let primary = f.host.primary_downloads.load(Ordering::SeqCst);
    let backdrop = f.host.backdrop_downloads.load(Ordering::SeqCst);
    let uploads = f.host.uploads.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["updated"], 1);
    assert_eq!(
        primary, 1,
        "generation must use the primary-image default shown by GET"
    );
    assert_eq!(backdrop, 0);
    assert_eq!(uploads, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn successful_cover_upload_still_reports_one_failed_image_download() {
    let f = Fixture::new().await;
    let mut config = covers::defaults();
    config["enabled"] = json!(true);
    config["cover_style"] = json!("multi_1");
    config["multi_1_use_main_font"] = json!(true);
    let job = f.generate(config).await;
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["updated"], 1);
    assert_eq!(job["result"]["warnings"], 1);
    assert!(
        job["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("502")
    );
    assert_eq!(f.host.uploads.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn failed_configured_cover_copy_remains_visible_after_upload_and_unset_output_is_not_a_warning()
 {
    let f = Fixture::new().await;
    let input = f.directory.path().join("input").join("测试库");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(input.join("poster.png"), f.host.image.as_ref()).unwrap();
    let blocked = f.directory.path().join("not-a-directory");
    std::fs::write(&blocked, "fixture").unwrap();
    let mut config = covers::defaults();
    config["enabled"] = json!(true);
    config["covers_input"] = json!(input.parent().unwrap());
    config["covers_output"] = json!(blocked);
    let failed_copy = f.generate(config.clone()).await;
    assert_eq!(
        failed_copy["state"], "completed_with_warnings",
        "{failed_copy}"
    );
    assert_eq!(failed_copy["result"]["updated"], 1);
    assert_eq!(failed_copy["result"]["warnings"], 1);
    assert!(
        failed_copy["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("另存封面失败")
    );
    config["covers_output"] = json!("");
    let uploaded = f.generate(config).await;
    assert_eq!(uploaded["state"], "completed", "{uploaded}");
    assert_eq!(uploaded["result"]["warnings"], 0);
    assert_eq!(f.host.uploads.load(Ordering::SeqCst), 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn missing_native_path_stops_preflight_before_considering_a_playback_path() {
    let f = Fixture::new().await;
    let work = f.state.clone();
    let item = json!({"Id":"102","Type":"Movie","Name":"缺路径","MediaSources":[{"ItemId":"102","Path":"/cd2/102.mkv","MediaStreams":[{"Type":"Video","Width":1920,"Height":1080}]}]});
    let id = f
        .state
        .jobs
        .submit("预检路径验证", move |context| async move {
            media_preflight::run(&work, &item, &context).await
        })
        .await
        .unwrap();
    let job = f.result(&id).await;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["message"].as_str().unwrap().contains("原生路径"));
    assert_eq!(f.host.requests.load(Ordering::SeqCst), 0);
    assert_eq!(f.host.uploads.load(Ordering::SeqCst), 0);
    f.close().await;
}
