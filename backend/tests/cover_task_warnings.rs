use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, covers, db, media_preflight};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Host {
    uploads: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
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
            image: Arc::new(image.into_inner()),
        };
        let router = Router::new()
            .route("/emby/Users/admin/Views", get(|| async { Json(json!({"Items":[{"Id":"library","Name":"测试库","CollectionType":"movies"}]})) }))
            .route("/Users/admin/Items", get(|State(host): State<Host>, Query(query): Query<HashMap<String,String>>| async move {
                host.requests.fetch_add(1, Ordering::SeqCst);
                Json(if query.get("Limit").is_some_and(|limit| limit=="0") { json!({"Items":[],"TotalRecordCount":2}) }
                    else { json!({"Items":[{"Id":"bad","Name":"失败图片","ImageTags":{"Primary":"one"}},{"Id":"good","Name":"正常图片","ImageTags":{"Primary":"two"}}],"TotalRecordCount":2}) })
            }))
            .route("/Items/bad/Images/Primary", get(|| async { StatusCode::BAD_GATEWAY }))
            .route("/Items/good/Images/Primary", get(|State(host): State<Host>| async move { ([("Content-Type","image/png")], host.image.as_ref().clone()) }))
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
