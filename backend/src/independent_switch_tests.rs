use crate::{AppState, auth::SessionSigner, jobs::JobContext, logs::LogStore};
use axum::{Json, Router, extract::Query, routing::get};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, sync::Arc};
use tokio::sync::RwLock;

fn state(root: &Path, db: sqlx::PgPool, config: Value) -> AppState {
    AppState {
        db,
        config: Arc::new(RwLock::new(config)),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root).unwrap()),
        recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
        data_dir: root.into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::default(),
        logs: LogStore::open(&root.join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    }
}

#[tokio::test]
async fn image_language_preference_controls_tmdb_requests_with_filesystem_monitor_disabled() {
    let requests = Arc::new(RwLock::new(Vec::<String>::new()));
    let received = requests.clone();
    let router = Router::new().route(
        "/movie/1",
        get(move |Query(query): Query<HashMap<String, String>>| {
            let received = received.clone();
            async move {
                if let Some(language) = query.get("include_image_language") {
                    received.write().await.push(language.clone());
                }
                // No poster candidates: exercise every fallback without any image download.
                Json(json!({"original_language":"ja","images":{"posters":[]}}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = state(
        directory.path(),
        sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        json!({"monitor_enabled":false,"tmdb_api_key":"fixture","tmdb_api_base_url":base}),
    );
    let context = JobContext::for_recovery(None).await;
    let item = json!({"Name":"测试电影","Path":directory.path().join("movie.mkv")});
    for (preference, expected) in [
        ("zh", vec!["zh-CN", "zh-TW,zh,zh-HK,zh-SG", "en,null", "ja"]),
        ("original", vec!["ja", "en,null", "zh-CN,zh-HK,zh-TW,zh"]),
    ] {
        state.config.write().await["tmdb_image_language_preference"] = json!(preference);
        requests.write().await.clear();
        let report =
            crate::images::download_report(&state, "Movie", "1", &item, &json!({}), &context)
                .await
                .unwrap();
        assert_eq!(report["downloaded"], 0);
        assert_eq!(report["warnings"], 0);
        assert_eq!(*requests.read().await, expected);
        assert_eq!(state.config.read().await["monitor_enabled"], false);
    }
    server.abort();
}

async fn assert_image_query_cancels(block_initial: bool) {
    let requested = Arc::new(tokio::sync::Notify::new());
    let received = requested.clone();
    let router = Router::new().route(
        "/movie/1",
        get(move |Query(query): Query<HashMap<String, String>>| {
            let received = received.clone();
            async move {
                if query.contains_key("include_image_language") != block_initial {
                    received.notify_one();
                    axum::response::IntoResponse::into_response((
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        [("Retry-After", "3600")],
                    ))
                } else {
                    axum::response::IntoResponse::into_response(Json(json!({
                        "original_language":"ja","images":{"posters":[]}
                    })))
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let state = state(
        directory.path(),
        sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        json!({"tmdb_api_key":"fixture","tmdb_api_base_url":base}),
    );
    let context = JobContext::for_recovery(None).await;
    let item = json!({"Name":"测试电影","Path":directory.path().join("movie.mkv")});
    let existing = directory.path().join("poster.jpg");
    tokio::fs::write(&existing, b"preserve-existing-image")
        .await
        .unwrap();
    let work = state.clone();
    let job = context.clone();
    let mut worker = tokio::spawn(async move {
        crate::images::download_report(&work, "Movie", "1", &item, &json!({}), &job).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), requested.notified())
        .await
        .expect("blocked query must start");
    context.request_stop();
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(1), &mut worker).await;
    worker.abort();
    server.abort();
    let error = stopped
        .expect("image query cancellation must not wait for Retry-After")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.0, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.1, "任务已取消");
    // The interrupted query must release the shared secondary slot for subsequent work.
    let _slot = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        crate::strm::slot(&state, "secondary", None),
    )
    .await
    .expect("cancelled image query must release its slot")
    .unwrap();
    assert_eq!(
        tokio::fs::read(existing).await.unwrap(),
        b"preserve-existing-image"
    );
}

#[tokio::test]
async fn image_original_language_query_cancels_during_long_retry_after() {
    assert_image_query_cancels(true).await;
}

#[tokio::test]
async fn image_language_strategy_query_cancels_during_long_retry_after() {
    assert_image_query_cancels(false).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn sha1_lookup_uses_its_own_switch_with_filesystem_monitor_disabled() {
    let url = std::env::var("DATABASE_URL").expect("disposable DATABASE_URL");
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("independent_sha1_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&root)
        .await
        .unwrap();
    let search_path = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let search_path = search_path.clone();
            Box::pin(async move {
                sqlx::query(&search_path).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE p115_filesystem_cache(pick_code TEXT,sha1 TEXT,local_path TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO p115_filesystem_cache VALUES('fixturepc','ABC123','media/movie.mkv')")
        .execute(&pool)
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let state = state(
        directory.path(),
        pool,
        json!({"monitor_enabled":false,"monitor_sha1_pc_search":true}),
    );
    let enabled = crate::fingerprints::lookup(&state, "/media/movie.mkv").await;
    // A closed pool proves the disabled branch does not consult SQL or the 115 API.
    state.db.close().await;
    state.config.write().await["monitor_sha1_pc_search"] = json!(false);
    let disabled = crate::fingerprints::lookup(&state, "/media/movie.mkv").await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
    assert_eq!(
        enabled.unwrap(),
        (
            Some("fixturepc".into()),
            Some("ABC123".into()),
            "media/movie.mkv".into()
        )
    );
    assert_eq!(disabled.unwrap(), (None, None, String::new()));
}
