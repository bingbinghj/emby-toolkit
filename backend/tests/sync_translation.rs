use axum::{
    Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, metadata, sync};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstreams are localhost mocks"]
async fn synchronization_persists_translated_movie_and_episode_fields_and_keeps_excluded_metadata()
{
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("sync_translation_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&root)
        .await
        .unwrap();
    let query = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let query = query.clone();
            Box::pin(async move {
                sqlx::query(&query).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new(pool.clone(), directory.path().into())
        .await
        .unwrap();
    let movie = json!({"Id":"movie","Type":"Movie","ProviderIds":{"Tmdb":"123"},"Name":"Movie name","Path":"/media/movie.mkv"});
    let series = json!({"Id":"series","Type":"Series","ProviderIds":{"Tmdb":"456"},"Name":"Series name","Path":"/media/show"});
    let season = json!({"Id":"season","Type":"Season","SeriesId":"series","ParentId":"series","IndexNumber":1,"Name":"Season name"});
    let episode = json!({"Id":"episode","Type":"Episode","SeriesId":"series","ParentId":"season","ParentIndexNumber":1,"IndexNumber":1,"Name":"Episode name","Path":"/media/show/Season 01/E01.mkv"});
    let all = vec![movie.clone(), series.clone(), season, episode];
    let requested = Arc::new(Mutex::new(Vec::<String>::new()));
    let requests = requested.clone();
    let upstream = Router::new()
        .route("/Users/admin/Views", get(|| async { Json(json!({"Items":[{"Id":"library"}]})) }))
        .route("/Items", get(move |Query(query): Query<HashMap<String, String>>| {
            let all = all.clone();
            async move {
                let items = if query["ParentId"] == "series" { all[2..].to_vec() } else { all };
                Json(json!({"TotalRecordCount":items.len(),"Items":items}))
            }
        }))
        .route("/Users/admin/Items/{id}", get(move |Path(id): Path<String>| {
            let movie = movie.clone();
            let series = series.clone();
            async move { Json(if id == "movie" { movie } else { series }) }
        }))
        .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
        .route("/movie/123", get(|| async {
            Json(json!({"id":123,"title":"Movie name","original_title":"Movie name","original_language":"en","overview":"Movie overview","credits":{"crew":[{"id":700,"job":"Director","name":"Director name"}]}}))
        }))
        .route("/tv/456", get(|| async {
            Json(json!({"id":456,"name":"Series name","original_name":"Series name","original_language":"en","overview":"Series overview","created_by":[{"id":800,"name":"Creator name"}],"seasons":[{"id":400,"season_number":1,"name":"Season name"}]}))
        }))
        .route("/tv/456/season/1", get(|| async {
            Json(json!({"id":400,"season_number":1,"name":"Season name","episodes":[{"id":789,"season_number":1,"episode_number":1,"name":"Episode name","overview":"Episode overview"}]}))
        }))
        .route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
            let requests = requests.clone();
            async move {
                let input: HashMap<String, String> = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                let translations = HashMap::from([
                    ("Movie overview", "电影中文简介"),
                    ("Series overview", "电视剧中文简介"),
                    ("Episode overview", "分集中文简介"),
                    ("Episode name", "分集中文标题"),
                    ("Season name", "季中文标题"),
                ]);
                requests.lock().await.extend(input.values().cloned());
                let output: HashMap<_, _> = input.into_iter().map(|(key, text)| (key, translations[text.as_str()])).collect();
                (StatusCode::OK, Json(json!({"choices":[{"message":{"content":serde_json::to_string(&output).unwrap()}}]})))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    state
        .save_config(&json!({
            "emby_server_url":base,"emby_api_key":"test","emby_user_id":"admin",
            "tmdb_api_base_url":base,"tmdb_api_key":"test","ai_provider":"openai",
            "ai_base_url":format!("{base}/v1"),"ai_api_key":"test","ai_model_name":"test",
            "ai_translate_on_sync":true,"ai_translate_title":true,"ai_translate_overview":true,
            "ai_translate_episode_overview":true,"ai_translate_actor_role":true,
            "p115_fingerprint_media_root_cid":""
        }))
        .await
        .unwrap();
    for (id, kind, title) in [
        ("123", "Movie", "手工电影名"),
        ("456", "Series", "手工电视剧名"),
    ] {
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title) VALUES($1,$2,$3)")
            .bind(id)
            .bind(kind)
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
    }
    let work = state.clone();
    let job = state
        .jobs
        .submit("翻译同步验证", move |context| async move {
            sync::cache(&work, true, &context).await
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = state.jobs.result(&job).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let movie = metadata::row(&state, "123", "Movie")
        .await
        .unwrap()
        .unwrap();
    let series = metadata::row(&state, "456", "Series")
        .await
        .unwrap()
        .unwrap();
    let episode = metadata::row(&state, "789", "Episode")
        .await
        .unwrap()
        .unwrap();
    server.abort();
    state.jobs.shutdown().await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;

    assert_eq!(result["state"], "completed", "{result}");
    let requested = requested.lock().await;
    assert!(requested.contains(&"Movie overview".into()));
    assert!(requested.contains(&"Series overview".into()));
    assert!(requested.contains(&"Episode name".into()));
    assert!(requested.contains(&"Episode overview".into()));
    assert!(!requested.iter().any(|text| {
        ["Movie name", "Series name", "Director name", "Creator name"].contains(&text.as_str())
    }));
    assert_eq!(movie["title"], "手工电影名");
    assert_eq!(series["title"], "手工电视剧名");
    assert_eq!(
        movie["directors_json"],
        json!([{"id":700,"name":"Director name"}])
    );
    assert_eq!(
        series["directors_json"],
        json!([{"id":800,"name":"Creator name"}])
    );
    assert_eq!(movie["overview"], "电影中文简介");
    assert_eq!(series["overview"], "电视剧中文简介");
    assert_eq!(episode["title"], "分集中文标题");
    assert_eq!(episode["overview"], "分集中文简介");
}
