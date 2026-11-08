use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use emby_toolkit::{AppState, images};
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
    mode: Arc<AtomicUsize>,
    bytes: Arc<Vec<u8>>,
}
async fn movie(State(host): State<Host>, Query(query): Query<HashMap<String, String>>) -> Response {
    let mode = host.mode.load(Ordering::SeqCst);
    if mode == 0
        || mode == 2
            && query
                .get("include_image_language")
                .is_some_and(|value| value != "en,null")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    Json(json!({"original_language":"en","images":{"posters":if mode == 1 {json!([])}else{json!([{"file_path":"/poster.png"}])}}})).into_response()
}
async fn poster(State(host): State<Host>) -> Response {
    if host.mode.load(Ordering::SeqCst) == 3 {
        StatusCode::BAD_GATEWAY.into_response()
    } else {
        ([("Content-Type", "image/png")], host.bytes.as_ref().clone()).into_response()
    }
}
async fn run(state: &AppState, item: &Value) -> Value {
    let work = state.clone();
    let item = item.clone();
    let id = state
        .jobs
        .submit("图片处理警告验证", move |context| async move {
            images::download_report(&work, "Movie", "7", &item, &json!({}), &context).await
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = state.jobs.result(&id).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; TMDb and images are localhost"]
fn image_request_and_write_failures_warn_but_no_posters_existing_files_and_successful_fallback_do_not()
 {
    // Set the image fixture URL before starting any runtime threads, then restore it after shutdown.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let previous = std::env::var_os("ETK_TMDB_IMAGE_BASE");
    unsafe {
        std::env::set_var("ETK_TMDB_IMAGE_BASE", &base);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("image_warnings_{}",uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&admin).await.unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new().after_connect(move |connection,_| {
            let search = search.clone();
            Box::pin(async move { sqlx::query(&search).execute(connection).await?; Ok(()) })
        }).connect(&url).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool,directory.path().join("app")).await.unwrap();
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(16,24,image::Rgb([80,120,160])))
            .write_to(&mut image,image::ImageFormat::Png).unwrap();
        let host = Host { mode: Arc::default(), bytes: Arc::new(image.into_inner()) };
        let router = Router::new().route("/movie/7",get(movie)).route("/poster.png",get(poster)).with_state(host.clone());
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
        state.save_config(&json!({"tmdb_api_base_url":base,"tmdb_api_key":"fixture","tmdb_image_language_preference":"zh","extract_thumb":false})).await.unwrap();
        let path = directory.path().join("movie.strm");
        std::fs::write(&path,"fixture").unwrap();
        let item = json!({"Id":"movie","Type":"Movie","Name":"测试图片","Path":path});
        let poster_path = directory.path().join("poster.jpg");

        let all_failed = run(&state,&item).await;
        assert_eq!(all_failed["state"],"completed_with_warnings","{all_failed}");
        assert_eq!(all_failed["result"]["warnings"],1);
        assert!(all_failed["result"]["warning_details"][0].as_str().unwrap().contains("所有语言策略"));

        host.mode.store(1,Ordering::SeqCst);
        let no_posters = run(&state,&item).await;
        assert_eq!(no_posters["state"],"completed","{no_posters}");
        assert_eq!(no_posters["result"]["warnings"],0);
        assert_eq!(no_posters["result"]["downloaded"],0);

        std::fs::write(&poster_path,"keep-existing").unwrap();
        host.mode.store(2,Ordering::SeqCst);
        let fallback = run(&state,&item).await;
        assert_eq!(fallback["state"],"completed","{fallback}");
        assert_eq!(fallback["result"]["warnings"],0);
        assert_eq!(std::fs::read_to_string(&poster_path).unwrap(),"keep-existing");

        std::fs::remove_file(&poster_path).unwrap();
        host.mode.store(3,Ordering::SeqCst);
        let failed_download = run(&state,&item).await;
        assert_eq!(failed_download["state"],"completed_with_warnings","{failed_download}");
        assert_eq!(failed_download["result"]["warnings"],1);
        assert!(failed_download["result"]["warning_details"][0].as_str().unwrap().contains("poster.jpg"));

        std::fs::create_dir(&poster_path).unwrap();
        host.mode.store(4,Ordering::SeqCst);
        let failed_write = run(&state,&item).await;
        assert_eq!(failed_write["state"],"completed_with_warnings","{failed_write}");
        assert_eq!(failed_write["result"]["warnings"],1);
        std::fs::remove_dir(&poster_path).unwrap();
        host.mode.store(5,Ordering::SeqCst);
        let success = run(&state,&item).await;
        assert_eq!(success["state"],"completed","{success}");
        assert_eq!(success["result"]["downloaded"],1);
        assert_eq!(success["result"]["warnings"],0);
        assert_eq!(std::fs::read(&poster_path).unwrap(),host.bytes.as_ref().clone());

        state.jobs.shutdown().await;
        server.abort();
        state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(&admin).await.unwrap();
        admin.close().await;
    });
    drop(runtime);
    unsafe {
        match previous {
            Some(value) => std::env::set_var("ETK_TMDB_IMAGE_BASE", value),
            None => std::env::remove_var("ETK_TMDB_IMAGE_BASE"),
        };
    }
}
