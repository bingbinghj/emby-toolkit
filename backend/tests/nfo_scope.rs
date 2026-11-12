use axum::{Json, Router, routing::get};
use emby_toolkit::{AppState, nfo_files, tasks};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tower::ServiceExt;

fn package() -> Value {
    json!({"id":123,"title":"Target Movie","name":"Target Series","overview":"Target plot",
        "seasons_details":[{"id":124,"name":"Season 1","season_number":1}],
        "episodes_details":{"S1E1":{"id":125,"name":"Target Episode","season_number":1,"episode_number":1}}})
}

#[tokio::test]
async fn dotted_series_directory_never_writes_library_root_or_another_series() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Series.Name.2020");
    let other = dir.path().join("Other Series");
    for root in [&target, &other] {
        tokio::fs::create_dir_all(root.join("Season 01"))
            .await
            .unwrap();
        tokio::fs::write(root.join("Season 01/S01E01.strm"), "playback")
            .await
            .unwrap();
    }
    let untouched = [
        dir.path().join("tvshow.nfo"),
        other.join("tvshow.nfo"),
        other.join("Season 01/season.nfo"),
        other.join("Season 01/S01E01.nfo"),
    ];
    for path in &untouched {
        tokio::fs::write(path, "keep unrelated NFO bytes")
            .await
            .unwrap();
    }
    let config = json!({});
    let options = nfo_files::Options {
        force: true,
        config: &config,
        ..Default::default()
    };
    let item = json!({"Id":"series","Type":"Series","Name":"Target Series","Path":target});
    let count = nfo_files::sync("Series", &item, &package(), &[], &options)
        .await
        .unwrap();
    for path in untouched {
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            b"keep unrelated NFO bytes",
            "{}",
            path.display()
        );
    }
    assert_eq!(count, 3);
    assert!(
        tokio::fs::read_to_string(target.join("tvshow.nfo"))
            .await
            .unwrap()
            .contains("Target Series")
    );
    assert!(
        tokio::fs::read_to_string(target.join("Season 01/S01E01.nfo"))
            .await
            .unwrap()
            .contains("Target Episode")
    );
}

#[tokio::test]
async fn selected_flat_movie_never_overwrites_or_creates_another_movie_nfo() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("A.strm");
    let other = dir.path().join("B.strm");
    let without_nfo = dir.path().join("C.strm");
    for path in [&target, &other, &without_nfo] {
        tokio::fs::write(path, "playback").await.unwrap();
    }
    tokio::fs::write(other.with_extension("nfo"), "keep B NFO bytes")
        .await
        .unwrap();
    let config = json!({});
    let options = nfo_files::Options {
        force: true,
        config: &config,
        ..Default::default()
    };
    let item = json!({"Id":"movie","Type":"Movie","Name":"Target Movie","Path":target});
    let count = nfo_files::sync("Movie", &item, &package(), &[], &options)
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::read(other.with_extension("nfo")).await.unwrap(),
        b"keep B NFO bytes"
    );
    assert!(!without_nfo.with_extension("nfo").exists());
    assert_eq!(count, 1);
    assert!(
        tokio::fs::read_to_string(target.with_extension("nfo"))
            .await
            .unwrap()
            .contains("Target Movie")
    );
}

#[tokio::test]
async fn explicit_movie_versions_and_parts_are_written_without_directory_guessing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        dir.path().join("Movie.strm"),
        dir.path().join("Movie.part2.strm"),
        dir.path().join("Other location/Related.mkv"),
    ];
    tokio::fs::create_dir_all(paths[2].parent().unwrap())
        .await
        .unwrap();
    for path in &paths {
        tokio::fs::write(path, "playback").await.unwrap();
    }
    let other = dir.path().join("Unrelated.strm");
    tokio::fs::write(&other, "unrelated playback")
        .await
        .unwrap();
    tokio::fs::write(other.with_extension("nfo"), "keep unrelated bytes")
        .await
        .unwrap();
    let config = json!({});
    let options = nfo_files::Options {
        force: true,
        movie_paths: &paths,
        config: &config,
        ..Default::default()
    };
    let item = json!({"Id":"movie","Type":"Movie","Path":paths[0]});
    let count = nfo_files::sync("Movie", &item, &package(), &[], &options)
        .await
        .unwrap();
    assert_eq!(count, 3);
    for path in paths {
        assert!(
            tokio::fs::read_to_string(path.with_extension("nfo"))
                .await
                .unwrap()
                .contains("Target Movie")
        );
    }
    assert_eq!(
        tokio::fs::read(other.with_extension("nfo")).await.unwrap(),
        b"keep unrelated bytes"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn manual_nfo_save_rejects_a_stale_cached_movie_template_before_any_file_write() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("nfo_template_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
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
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, dir.path().into()).await.unwrap();
    let target = dir.path().join("Movie.strm");
    let original = "<movie><title>Original NFO</title></movie>";
    tokio::fs::write(&target, "playback").await.unwrap();
    tokio::fs::write(target.with_extension("nfo"), original)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json,actors_json) VALUES('123','Movie','Cached A',true,$1,$2,'[]'::jsonb)")
        .bind(json!(["movie"])).bind(json!([{"emby_item_id":"movie","path":target}]))
        .execute(&state.db).await.unwrap();
    let live = json!({"Id":"movie","Type":"Movie","Name":"Live B","Path":target,
        "People":[],"ProviderIds":{"Tmdb":"456"},"PartCount":1,
        "MediaSources":[{"Id":"mediasource_movie","ItemId":"movie","Path":target}]});
    let mutations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let written = mutations.clone();
    let router = Router::new()
        .route(
            "/Users/user/Items/movie",
            get(move || {
                let live = live.clone();
                async move { Json(live) }
            }),
        )
        .fallback(move |request: axum::extract::Request| {
            let written = written.clone();
            async move {
                if request.method() == axum::http::Method::POST {
                    written.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
            }
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    state
        .save_config(
            &json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user"}),
        )
        .await
        .unwrap();
    let get = emby_toolkit::media_manual::router()
        .with_state(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/media_for_editing/movie")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), axum::http::StatusCode::CONFLICT);
    let work = state.clone();
    let id = state
        .jobs
        .submit("stale NFO template", move |context| async move {
            emby_toolkit::media_manual::save(&work, "movie", &json!({"cast":[]}), &context).await
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = state.jobs.result(&id).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result_status"], 409, "{result}");
    assert!(
        result["result"]["error"].as_str().unwrap().contains("TMDb"),
        "{result}"
    );
    assert_eq!(
        tokio::fs::read_to_string(target.with_extension("nfo"))
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        mutations.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no native People POST or refresh is allowed for the wrong cached template"
    );
}

#[tokio::test]
async fn nfo_write_failure_is_an_error_instead_of_an_unchanged_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Movie.strm");
    tokio::fs::write(&target, "playback").await.unwrap();
    tokio::fs::create_dir(target.with_extension("nfo"))
        .await
        .unwrap();
    let config = json!({});
    let options = nfo_files::Options {
        force: true,
        config: &config,
        ..Default::default()
    };
    let item = json!({"Id":"movie","Type":"Movie","Path":target});
    assert!(
        nfo_files::sync("Movie", &item, &package(), &[], &options)
            .await
            .is_err()
    );
    assert!(target.with_extension("nfo").is_dir());
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn restore_task_reports_nfo_io_failure_without_counting_the_item_as_restored() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("nfo_scope_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
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
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, dir.path().into()).await.unwrap();
    let target = dir.path().join("Movie.strm");
    tokio::fs::write(&target, "playback").await.unwrap();
    tokio::fs::create_dir(target.with_extension("nfo"))
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json,actors_json) VALUES('123','Movie','Target Movie',true,$1,$2,'[]'::jsonb)")
        .bind(json!(["movie"]))
        .bind(json!([{"emby_item_id":"movie","path":target}]))
        .execute(&state.db).await.unwrap();
    let router = Router::new()
        .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
        .fallback(|| async { Json(json!({"images":{"posters":[],"backdrops":[],"logos":[]}})) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","tmdb_api_base_url":base,"tmdb_api_key":"fixture"})).await.unwrap();
    let accepted = tasks::submit_task(&state, "restore-nfo-and-images", false)
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let result = state
                .jobs
                .result(accepted["task_id"].as_str().unwrap())
                .await
                .unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["updated"], 0, "{result}");
    assert_eq!(
        result["result"]["failures"].as_array().unwrap().len(),
        1,
        "{result}"
    );
    assert!(Path::new(&target).with_extension("nfo").is_dir());
}

// Append to backend/tests/nfo_scope.rs only after root releases the compile freeze.
struct ImageBaseRestore(Option<std::ffi::OsString>);
impl Drop for ImageBaseRestore {
    fn drop(&mut self) {
        // These regression binaries are run with --test-threads=1.
        unsafe {
            if let Some(value) = self.0.take() {
                std::env::set_var("ETK_TMDB_IMAGE_BASE", value);
            } else {
                std::env::remove_var("ETK_TMDB_IMAGE_BASE");
            }
        }
    }
}
fn image_bytes(color: [u8; 3]) -> Vec<u8> {
    let image =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb(color)));
    let mut output = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Jpeg)
        .unwrap();
    output.into_inner()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all services are localhost"]
async fn dotted_series_image_download_keeps_library_and_neighbour_images_unchanged() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("image_scope_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
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
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, dir.path().into()).await.unwrap();
    let target = dir.path().join("Series.Name.2020");
    let other = dir.path().join("Other Series");
    for root in [&target, &other] {
        tokio::fs::create_dir_all(root.join("Season 01"))
            .await
            .unwrap();
        tokio::fs::write(root.join("Season 01/S01E01.strm"), "playback")
            .await
            .unwrap();
    }
    let originals = [
        dir.path().join("poster.jpg"),
        dir.path().join("season01-poster.jpg"),
        other.join("poster.jpg"),
        other.join("Season 01/S01E01-thumb.jpg"),
    ];
    let original = image_bytes([255, 0, 0]);
    let replacement = image_bytes([0, 255, 0]);
    for path in &originals {
        tokio::fs::write(path, &original).await.unwrap();
    }
    let jpeg = replacement.clone();
    let router = Router::new().route("/tv/123", get(|| async { Json(json!({
        "id":123,"name":"Target Series","original_language":"en",
        "images":{"posters":[{"file_path":"/poster.jpg","iso_639_1":"en"}],"backdrops":[],"logos":[]}
    })) })).route("/image/{name}", get(move || {
        let jpeg = jpeg.clone();
        async move { ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], jpeg) }
    })).fallback(|| async { Json(json!([])) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    state
        .save_config(
            &json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user",
        "tmdb_api_base_url":base,"tmdb_api_key":"fixture","extract_thumb":false}),
        )
        .await
        .unwrap();
    let restore_env = ImageBaseRestore(std::env::var_os("ETK_TMDB_IMAGE_BASE"));
    unsafe {
        std::env::set_var("ETK_TMDB_IMAGE_BASE", format!("{base}/image"));
    }
    let work = state.clone();
    let selected = target.clone();
    let id = state.jobs.submit("dotted Series image scope", move |context| async move {
        emby_toolkit::images::download_with_overwrite_report(&work, "Series", "123",
            &json!({"Id":"series","Type":"Series","Name":"Target Series","Path":selected}),
            &json!({"seasons_details":[{"season_number":1,"poster_path":"/season.jpg"}],
                "episodes_details":{"S1E1":{"season_number":1,"episode_number":1,"still_path":"/episode.jpg"}}}),
            &context, &["S1E1".into()]).await
    }).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let result = state.jobs.result(&id).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(restore_env);
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    for path in originals {
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            original,
            "{}",
            path.display()
        );
    }
    assert_eq!(
        tokio::fs::read(target.join("poster.jpg")).await.unwrap(),
        replacement
    );
    assert_eq!(
        tokio::fs::read(target.join("season01-poster.jpg"))
            .await
            .unwrap(),
        replacement
    );
    assert_eq!(
        tokio::fs::read(target.join("Season 01/S01E01-thumb.jpg"))
            .await
            .unwrap(),
        replacement
    );
    assert_eq!(result["result"]["downloaded"], 3);
}
