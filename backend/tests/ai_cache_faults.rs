use axum::{Json, Router, routing::post};
use emby_toolkit::{AppState, ai};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn translations_survive_database_cache_failures_and_reuse_memory_until_invalidated() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("ai_cache_faults_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&root)
        .await
        .unwrap();
    let search_path = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |conn, _| {
            let query = search_path.clone();
            Box::pin(async move {
                sqlx::query(&query).execute(conn).await?;
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
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let input: Value =
                    serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                let out: BTreeMap<_, _> = if let Some(values) = input.as_array() {
                    values
                        .iter()
                        .map(|text| {
                            (
                                text.as_str().unwrap().to_string(),
                                format!("中文{}", text.as_str().unwrap()),
                            )
                        })
                        .collect()
                } else {
                    input
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(id, text)| (id.clone(), format!("中文{}", text.as_str().unwrap())))
                        .collect()
                };
                Json(json!({"choices":[{"message":{"content":json!(out).to_string()}}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    state.save_config(&json!({"ai_provider":"openai","ai_api_key":"fixture","ai_base_url":format!("http://{address}/v1"),"ai_model_name":"fixture","ai_max_retries":0})).await.unwrap();
    sqlx::query("DROP TABLE translation_cache")
        .execute(&pool)
        .await
        .unwrap();
    let first = BTreeMap::from([("1".into(), "One".into())]);
    let value = ai::translate(
        &state,
        &first,
        "batch_title_translation",
        &json!({}),
        &Value::Null,
    )
    .await;
    assert!(
        value.is_ok(),
        "missing cache table must not abort translation: {value:?}"
    );
    assert_eq!(value.unwrap()["1"], "中文One");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        ai::translate(
            &state,
            &first,
            "batch_title_translation",
            &json!({}),
            &Value::Null
        )
        .await
        .unwrap()["1"],
        "中文One"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "failed database writes still retain process cache"
    );
    state.ai.cache_revision.fetch_add(1, Ordering::SeqCst);
    ai::translate(
        &state,
        &first,
        "batch_title_translation",
        &json!({}),
        &Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "invalidated memory must not be reused"
    );
    sqlx::raw_sql("CREATE TABLE translation_cache(original_text TEXT PRIMARY KEY,translated_text TEXT,engine_used TEXT,last_updated_at TIMESTAMPTZ DEFAULT NOW()); CREATE FUNCTION fail_cache() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture cache write failure'; END $$; CREATE TRIGGER fail_cache_write BEFORE INSERT ON translation_cache FOR EACH ROW EXECUTE FUNCTION fail_cache();").execute(&pool).await.unwrap();
    let second = BTreeMap::from([("2".into(), "Two".into())]);
    assert_eq!(
        ai::translate(
            &state,
            &second,
            "batch_title_translation",
            &json!({}),
            &Value::Null
        )
        .await
        .unwrap()["2"],
        "中文Two"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    ai::translate(
        &state,
        &second,
        "batch_title_translation",
        &json!({}),
        &Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM translation_cache")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    sqlx::query("DROP TRIGGER fail_cache_write ON translation_cache")
        .execute(&pool)
        .await
        .unwrap();
    state.ai.cache_revision.fetch_add(1, Ordering::SeqCst);
    ai::translate(
        &state,
        &second,
        "batch_title_translation",
        &json!({}),
        &Value::Null,
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM translation_cache")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    state
        .save_config(&json!({"ai_translate_actor_role":true,"ai_translation_mode":"fast"}))
        .await
        .unwrap();
    sqlx::query("INSERT INTO translation_cache(original_text,translated_text,engine_used) VALUES('Alice','bad english','old'),('Hero','still english','old')")
        .execute(&pool).await.unwrap();
    let cast = emby_toolkit::media_manual::translate_cast(
        &state,
        &json!({"cast":[{"name":"Alice","role":"Hero"}]}),
    )
    .await
    .unwrap();
    assert_eq!(cast[0]["name"], "中文Alice");
    assert_eq!(cast[0]["role"], "中文Hero");
    let legacy: String = sqlx::query_scalar(
        "SELECT translated_text FROM translation_cache WHERE original_text='Alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(legacy, "中文Alice");
    sqlx::query("DROP TABLE translation_cache")
        .execute(&pool)
        .await
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    let input = json!({"cast":[{"name":"Other","role":"Helper"}]});
    let cast = emby_toolkit::media_manual::translate_cast(&state, &input)
        .await
        .unwrap();
    assert_eq!(cast[0]["name"], "中文Other");
    emby_toolkit::media_manual::translate_cast(&state, &input)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), before + 1);
    let mut metadata = json!({"id":42,"title":"Movie","overview":"","credits":{"cast":[{"id":1,"name":"NewActor","character":"NewRole","profile_path":"/photo.jpg"}]}});
    state.save_config(&json!({"ai_translate_overview":false,"ai_translate_title":false,"ai_translate_episode_overview":false,"ai_translate_actor":true,"ai_translate_role":true})).await.unwrap();
    emby_toolkit::translation::metadata_tree(&state, "Movie", &mut metadata, "")
        .await
        .unwrap();
    assert_eq!(metadata["credits"]["cast"][0]["name"], "中文NewActor");
    sqlx::query("INSERT INTO emby_users(id,name,is_administrator) VALUES('admin','Admin',true)")
        .execute(&pool)
        .await
        .unwrap();
    let token = state.sessions.sign(&emby_toolkit::auth::Session {
        user_id: "admin".into(),
        name: "Admin".into(),
        setup_mode: false,
        expires: chrono::Utc::now().timestamp() + 60,
    });
    let mut lists = Vec::new();
    for context in ["manual", "%20MANUAL%20"] {
        let response = emby_toolkit::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/tasks/available?context={context}"))
                    .header("Cookie", format!("etk_session={token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        lists.push(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        lists[0], lists[1],
        "context whitespace and case must match the original normalization"
    );
    server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
