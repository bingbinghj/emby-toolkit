use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use emby_toolkit::{AppState, episode_groups};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    reads: Arc<AtomicUsize>,
}

impl Fixture {
    async fn new(all_fail: bool) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("episode_group_reads_{}", uuid::Uuid::new_v4().simple());
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
        let reads = Arc::new(AtomicUsize::new(0));
        let a = reads.clone();
        let b = reads.clone();
        let router = Router::new()
            .route("/tv/123/episode_groups", get(|| async {
                Json(json!({"results":[{"id":"group-a"},{"id":"group-b"}],"total_pages":1}))
            }))
            .route("/tv/episode_group/group-a", get(move || {
                a.fetch_add(1, Ordering::SeqCst);
                async { (StatusCode::FORBIDDEN, Json(json!({"error":"candidate A cannot be read"}))) }
            }))
            .route("/tv/episode_group/group-b", get(move || {
                b.fetch_add(1, Ordering::SeqCst);
                async move {
                    if all_fail {
                        (StatusCode::FORBIDDEN, Json(json!({"error":"candidate B cannot be read"}))).into_response()
                    } else {
                        Json(json!({"groups":[{"order":1,"episodes":[{"id":101,"name":"First Episode"}]}]})).into_response()
                    }
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state
            .save_config(&json!({"tmdb_api_base_url":base,"tmdb_api_key":"fixture"}))
            .await
            .unwrap();
        Self {
            state,
            admin,
            schema,
            directory,
            server,
            reads,
        }
    }

    async fn close(self) {
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
        drop(self.directory);
    }
}

fn signals() -> Vec<Value> {
    vec![
        json!({"season_number":1,"episode_number":1,"tmdb_episode_id":"101","path":"/media/S01E01.strm"}),
    ]
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; every upstream request is localhost"]
async fn incomplete_group_details_cannot_make_the_only_readable_group_a_unique_match() {
    let f = Fixture::new(false).await;
    let result = episode_groups::match_group(&f.state, "123", &signals(), "").await;
    let reads = f.reads.load(Ordering::SeqCst);
    f.close().await;
    assert!(reads >= 1, "the candidate API was actually queried");
    assert!(
        result.is_err(),
        "candidate A is unknown; selecting B would pretend uniqueness: {result:?}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; every upstream request is localhost"]
async fn unavailable_group_details_are_not_a_confirmed_official_or_cache_mismatch() {
    let f = Fixture::new(true).await;
    let result = episode_groups::match_group(&f.state, "123", &signals(), "group-a").await;
    let reads = f.reads.load(Ordering::SeqCst);
    f.close().await;
    assert!(reads >= 1, "the cached group API was actually queried");
    assert!(
        result.is_err(),
        "Ok(None) makes cache reuse treat unavailable details as a mismatch: {result:?}"
    );
}
