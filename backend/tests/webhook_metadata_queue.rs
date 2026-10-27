use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::get,
};
use emby_toolkit::{AppState, webhook};
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; Emby is a localhost mock"]
async fn queued_metadata_updates_preserve_each_episode_in_the_same_series() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("webhook_queue_{}", uuid::Uuid::new_v4().simple());
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
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, directory.path().into()).await.unwrap();
    let fake = Router::new()
        .route(
            "/Users/user/Items/{id}",
            get(|Path(id): Path<String>| async move {
                Json(json!({"Id":id,"Type":if id == "series" {"Series"} else {"Episode"},"Path":format!("/library/{id}.strm"),"SeriesId":"series","ProviderIds":{"Tmdb":id},"Name":format!("新标题-{id}")}))
            }),
        )
        .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    state
        .save_config(&json!({"emby_server_url":base,"emby_api_key":"mock","emby_user_id":"user","emby_webhook_secret":"secret"}))
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title) VALUES('episode1','Episode','旧标题'),('episode2','Episode','旧标题')")
        .execute(&state.db).await.unwrap();
    let blocker = state
        .jobs
        .submit("hold", |context| async move {
            context.cancelled().await;
            context.checkpoint().await?;
            Ok(Value::Null)
        })
        .await
        .unwrap();
    let app = webhook::router().with_state(state.clone());
    for id in ["episode1", "episode2"] {
        let response = app.clone().oneshot(Request::builder()
            .method("POST").uri("/webhook/emby?token=secret")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"Event":"metadata.update","Item":{"Id":id,"Type":"Episode","SeriesId":"series","Path":format!("/library/{id}.strm")}}).to_string()))
            .unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
    state.jobs.control(Some(&blocker), "stop").await.unwrap();
    let complete = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let rows: Vec<String> =
                sqlx::query_scalar("SELECT title FROM media_metadata ORDER BY tmdb_id")
                    .fetch_all(&state.db)
                    .await
                    .unwrap();
            if rows == ["新标题-episode1", "新标题-episode2"] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_ok();
    state.jobs.shutdown().await;
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
    assert!(complete, "同一剧的两个分集元数据更新都应回写数据库");
}
