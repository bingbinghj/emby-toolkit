use emby_toolkit::{AppState, video_images};
use serde_json::{Value, json};
use std::time::Duration;

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("video_image_warnings_{}", uuid::Uuid::new_v4().simple());
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
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, root.path().join("app")).await.unwrap();
        state
            .save_config(&json!({"extract_thumb":true,"local_strm_root":root.path()}))
            .await
            .unwrap();
        Self {
            state,
            admin,
            schema,
            root,
        }
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; no Emby or cloud requests"]
async fn missing_pickcode_reports_episode_and_movie_capture_failures_without_touching_sources() {
    let f = Fixture::new().await;
    let source = "https://fixture.invalid/no-cloud-binding";
    let episode = f.root.path().join("Series.S01E01.strm");
    let movie = f.root.path().join("Movie.strm");
    std::fs::write(&episode, source).unwrap();
    std::fs::write(&movie, source).unwrap();
    let report =
        video_images::missing_report(&f.state, std::slice::from_ref(&episode), &json!({}), None)
            .await
            .unwrap();
    assert_eq!(report["captured"], 0);
    assert_eq!(report["warnings"], 1);
    assert!(
        report["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("视频截图缺少可验证115 PC")
    );
    assert_eq!(std::fs::read_to_string(&episode).unwrap(), source);
    assert!(!f.root.path().join("Series.S01E01-thumb.jpg").exists());
    let state = f.state.clone();
    let file = movie.clone();
    let root = f.root.path().to_path_buf();
    let task = f
        .state
        .jobs
        .submit("视频补图警告验证", move |context| async move {
            video_images::movie_missing_report(&state, &file, &root, &json!({}), Some(&context))
                .await
        })
        .await
        .unwrap();
    let job = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let job = f.state.jobs.result(&task).await.unwrap();
            if job["is_running"] == false {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["captured"], 0);
    assert_eq!(job["result"]["warnings"], 1);
    assert_eq!(std::fs::read_to_string(movie).unwrap(), source);
    assert!(!f.root.path().join("landscape.jpg").exists());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; no Emby or cloud requests"]
async fn disabled_capture_existing_images_and_available_remote_stills_do_not_warn() {
    let f = Fixture::new().await;
    let episode = f.root.path().join("Series.S01E01.strm");
    let movie = f.root.path().join("Movie.strm");
    std::fs::write(&episode, "https://fixture.invalid/no-cloud-binding").unwrap();
    std::fs::write(&movie, "https://fixture.invalid/no-cloud-binding").unwrap();
    f.state
        .save_config(&json!({"extract_thumb":false}))
        .await
        .unwrap();
    let report =
        video_images::missing_report(&f.state, std::slice::from_ref(&episode), &Value::Null, None)
            .await
            .unwrap();
    assert_eq!(report["warnings"], 0);
    assert_eq!(
        video_images::movie_missing(&f.state, &movie, f.root.path(), &json!({}), None)
            .await
            .unwrap(),
        0
    );
    f.state
        .save_config(&json!({"extract_thumb":true}))
        .await
        .unwrap();
    std::fs::write(
        f.root.path().join("Series.S01E01-thumb.jpg"),
        "existing image",
    )
    .unwrap();
    let report =
        video_images::missing_report(&f.state, std::slice::from_ref(&episode), &Value::Null, None)
            .await
            .unwrap();
    assert_eq!(report["warnings"], 0);
    std::fs::remove_file(f.root.path().join("Series.S01E01-thumb.jpg")).unwrap();
    let report = video_images::missing_report(
        &f.state,
        &[episode],
        &json!({"episodes_details":{"S1E1":{"still_path":"/remote.jpg"}}}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(report["warnings"], 0);
    std::fs::write(f.root.path().join("landscape.jpg"), "existing image").unwrap();
    let report =
        video_images::movie_missing_report(&f.state, &movie, f.root.path(), &json!({}), None)
            .await
            .unwrap();
    assert_eq!(report["warnings"], 0);
    f.close().await;
}
