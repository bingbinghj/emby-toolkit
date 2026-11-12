use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::get,
};
use emby_toolkit::{AppState, media_edit, media_manual};
use serde_json::json;
use tower::ServiceExt;

async fn read_conflicting_bindings() -> (bool, StatusCode, bool, StatusCode) {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("binding_readers_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
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
    let temp = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, temp.path().into()).await.unwrap();
    let host=Router::new().route("/Users/admin/Items/moving",get(||async {
        Json(json!({"Id":"moving","Type":"Movie","Name":"Moving movie","Path":"/media/moving.mkv","ProviderIds":{"Tmdb":"123"},"People":[]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","monitor_sha1_pc_search":false,"tmdb_api_key":""})).await.unwrap();
    let mut healthy_target = false;
    let mut healthy_actor = StatusCode::INTERNAL_SERVER_ERROR;
    let mut ambiguous_target = false;
    let mut ambiguous_actor = StatusCode::INTERNAL_SERVER_ERROR;
    for (index, id) in ["123", "456"].iter().enumerate() {
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Movie',$2,true,'[\"moving\"]'::jsonb,$3,$4,$5)")
            .bind(id).bind(format!("Cached {id}"))
            .bind(json!([{"emby_item_id":"moving","path":"/media/moving.mkv","size_bytes":1000,"fingerprint_pending":false}]))
            .bind(json!([if index==0 {"a".repeat(40)} else {"b".repeat(40)}]))
            .bind(json!([format!("pc-{id}")])).execute(&state.db).await.unwrap();
        let target = media_edit::target(&state, "moving").await.is_ok();
        let response = media_manual::router()
            .with_state(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/media_for_editing/moving")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(!body.is_empty());
        if index == 0 {
            healthy_target = target;
            healthy_actor = status;
        } else {
            ambiguous_target = target;
            ambiguous_actor = status;
        }
    }
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    (
        healthy_target,
        healthy_actor,
        ambiguous_target,
        ambiguous_actor,
    )
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn media_info_reader_rejects_conflicting_metadata_bindings() {
    let (healthy, actor_ok, ambiguous, actor_conflict) = read_conflicting_bindings().await;
    assert!(healthy);
    assert_eq!(actor_ok, StatusCode::OK);
    assert!(
        !ambiguous,
        "ambiguous target accepted={ambiguous}, actor status={actor_conflict}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn actor_page_rejects_conflicting_metadata_bindings() {
    let (healthy, actor_ok, ambiguous, actor_conflict) = read_conflicting_bindings().await;
    assert!(healthy);
    assert_eq!(actor_ok, StatusCode::OK);
    assert_eq!(
        actor_conflict,
        StatusCode::CONFLICT,
        "ambiguous target accepted={ambiguous}"
    );
}
