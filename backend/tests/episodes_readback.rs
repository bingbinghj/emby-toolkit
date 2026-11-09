use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use emby_toolkit::{AppState, episodes};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;

#[derive(Clone)]
struct Host {
    mode: &'static str,
    items: Vec<Value>,
    reads: Arc<AtomicUsize>,
}

async fn native_items(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    host.reads.fetch_add(1, Ordering::SeqCst);
    assert_eq!(query["Ids"], "a1,b1,a2,b2");
    assert_eq!(query["Fields"], "Path,MediaSources");
    match host.mode {
        "http500" => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("Retry-After", "1")],
            Json(json!({"error":"native read failed"})),
        )
            .into_response(),
        "invalid" => Json(json!({"Items":{"Id":"a1"}})).into_response(),
        "missing" => Json(json!({"Items":host.items[..3]})).into_response(),
        _ => Json(json!({"Items":host.items})).into_response(),
    }
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    host: Host,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(mode: &'static str) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("episodes_readback_{}", uuid::Uuid::new_v4().simple());
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
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        sqlx::query("INSERT INTO media_metadata (tmdb_id,item_type,title,emby_item_ids_json,in_library) VALUES ('series','Series','回读验证','[\"series\"]',true)")
            .execute(&state.db).await.unwrap();
        let mut native = vec![];
        for episode in 1..=2 {
            let ids = [format!("a{episode}"), format!("b{episode}")];
            let assets = ids
                .iter()
                .map(|id| {
                    json!({
                        "emby_item_id":id,"path":format!("/media/{id}.mkv"),
                        "audio_tracks":[],"subtitles":[]
                    })
                })
                .collect::<Vec<_>>();
            sqlx::query("INSERT INTO media_metadata (tmdb_id,item_type,title,parent_series_tmdb_id,season_number,episode_number,emby_item_ids_json,asset_details_json,in_library) VALUES ($1,'Episode',$2,'series',1,$3,$4,$5,true)")
                .bind(format!("episode{episode}")).bind(format!("分集{episode}"))
                .bind(episode).bind(json!(ids)).bind(json!(assets))
                .execute(&state.db).await.unwrap();
            let sources = ids.iter().rev().map(|id| json!({
                "Id":format!("mediasource_{id}"),"ItemId":id,"Path":format!("/media/{id}.mkv")
            })).collect::<Vec<_>>();
            for id in ids {
                native.push(json!({"Id":id,"Type":"Episode","Path":format!("/media/{id}.mkv"),"MediaSources":sources}));
            }
        }
        let host = Host {
            mode,
            items: native,
            reads: Arc::new(AtomicUsize::new(0)),
        };
        let fake = Router::new()
            .route("/Items", get(native_items))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
        state
            .save_config(&json!({
                "emby_user_id":"user","emby_api_key":"fixture",
                "emby_server_url":format!("http://{address}")
            }))
            .await
            .unwrap();
        Self {
            state,
            admin,
            schema,
            host,
            server,
            _directory: directory,
        }
    }

    async fn request(&self) -> (StatusCode, Value) {
        let response = episodes::router()
            .with_state(self.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/media_info/series/series/episodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn close(self) {
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
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn last_native_order_http500_is_reported_instead_of_database_order_success() {
    let fixture = Fixture::new("http500").await;
    let (status, body) = fixture.request().await;
    let reads = fixture.host.reads.load(Ordering::SeqCst);
    fixture.close().await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(reads > 0);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("分集版本顺序回读失败"),
        "{body}"
    );
    assert!(
        body["message"].as_str().unwrap().contains("HTTP 500"),
        "{body}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn invalid_last_native_order_items_is_reported_instead_of_database_order_success() {
    let fixture = Fixture::new("invalid").await;
    let (status, body) = fixture.request().await;
    fixture.close().await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("Items"),
        "{body}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn incomplete_last_native_order_readback_is_reported() {
    let fixture = Fixture::new("missing").await;
    let (status, body) = fixture.request().await;
    fixture.close().await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(body["message"].as_str().unwrap().contains("b2"), "{body}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn real_media_source_order_remains_consistent_across_episode_versions() {
    let fixture = Fixture::new("normal").await;
    let (status, body) = fixture.request().await;
    let reads = fixture.host.reads.load(Ordering::SeqCst);
    fixture.close().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(reads, 1);
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 4);
    for item in items {
        let expected = if item["emby_id"].as_str().unwrap().starts_with('b') {
            1
        } else {
            2
        };
        assert_eq!(item["version_index"], expected, "{item}");
        assert_eq!(item["season_version_count"], 2, "{item}");
    }
}
