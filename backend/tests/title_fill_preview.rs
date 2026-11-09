use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use emby_toolkit::{AppState, auth::Session, db};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

async fn preview(app: &Router, cookie: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/media_info/fill_titles")
                .header("Content-Type", "application/json")
                .header("Cookie", cookie)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is localhost"]
async fn title_fill_preview_uses_draft_sources_and_saved_mappings_without_saving() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("title_preview_{}", uuid::Uuid::new_v4().simple());
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
    sqlx::query("INSERT INTO emby_users(id,name,is_administrator) VALUES('admin','Fixture',true),('user','Fixture',false)")
        .execute(&state.db).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let fake = Router::new().fallback(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            StatusCode::NOT_FOUND
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    state.config.write().await["emby_server_url"] =
        json!(format!("http://{}", listener.local_addr().unwrap()));
    state.config.write().await["emby_api_key"] = json!("fixture");
    state.config.write().await["emby_user_id"] = json!("user");
    let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    let app = emby_toolkit::router(state.clone());
    let cookie = |id: &str| {
        format!(
            "etk_session={}",
            state.sessions.sign(&Session {
                user_id: id.into(),
                name: "Fixture".into(),
                setup_mode: false,
                expires: chrono::Utc::now().timestamp() + 60,
            })
        )
    };
    let admin = cookie("admin");
    let original_file = directory.path().join("preview-mediainfo.json");
    tokio::fs::write(&original_file, b"original backup")
        .await
        .unwrap();
    let before_jobs = state.jobs.status().await;
    for unauthorised in ["".to_string(), cookie("user")] {
        assert_eq!(
            preview(&app, &unauthorised, json!({"streams":[]})).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let streams = json!([
        {"Type":"Audio","Language":"chi","Title":"台","Codec":"ac3","Index":1},
        {"Type":"Audio","Language":"chi","Title":"东和兴"},
        {"Type":"Audio","Language":"chi","Title":"福建文艺"},
        {"Type":"Audio","Language":"ace","Title":"DVD粤"},
        {"Type":"Audio","Language":"chi","Title":"TVB粤"},
        {"Type":"Subtitle","Language":"ace","Title":"TVB粤对","Codec":"subrip","Index":8,"IsDefault":true,"DisplayTitle":"original"},
        {"Type":"Subtitle","Language":"chi","Title":"台对","Codec":"ass"},
        {"Type":"Subtitle","Language":"jpn","Title":"日对"},
        {"Type":"Subtitle","Language":"eng","Title":"英文字幕"},
        {"Type":"Audio","Language":"und","Title":"未标记音轨"}
    ]);
    let (status, result) = preview(&app, &admin, json!({"streams":streams})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    for (index, title) in [
        "台配国语",
        "东和兴国语",
        "福建文艺国语",
        "DVD粤语",
        "TVB粤语",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(result["streams"][index]["Title"], title);
    }
    assert_eq!(result["streams"][0]["Codec"], "ac3");
    assert_eq!(result["streams"][0]["Index"], 1);
    assert_eq!(result["streams"][3]["Language"], "yue");
    assert_eq!(result["streams"][4]["Language"], "yue");
    for index in [5, 6, 9] {
        assert_eq!(result["streams"][index], streams[index]);
    }
    assert_eq!(result["streams"][7]["Title"], "日文（日对）");
    assert_eq!(result["streams"][8]["Title"], "英文");
    assert_eq!(
        result["report"]["skipped_tracks"].as_array().unwrap().len(),
        2
    );
    assert_eq!(result["report"]["pending"].as_array().unwrap().len(), 1);
    for invalid in [
        json!({}),
        json!([]),
        json!({"streams":{}}),
        json!({"streams":[null]}),
        json!({"streams":[1]}),
        json!({"streams":["Audio"]}),
    ] {
        assert_eq!(
            preview(&app, &admin, invalid).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mapping = json!([{"label":"定制克林贡语","value":"tlh","aliases":["战士母语"]}]);
    db::save_setting(&state.db, "language_mapping", &mapping)
        .await
        .unwrap();
    let (status, custom) = preview(
        &app,
        &admin,
        json!({"streams":[{"Type":"Audio","Language":"und","Title":"战士母语"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{custom}");
    assert_eq!(custom["streams"][0]["Language"], "tlh");
    assert_eq!(custom["streams"][0]["Title"], "定制克林贡语");
    assert_eq!(
        db::setting(&state.db, "language_mapping").await.unwrap(),
        Some(mapping)
    );
    assert_eq!(state.jobs.status().await, before_jobs);
    assert_eq!(
        tokio::fs::read(&original_file).await.unwrap(),
        b"original backup".to_vec()
    );
    let cached: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM p115_mediainfo_cache")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(cached, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    state.jobs.shutdown().await;
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
