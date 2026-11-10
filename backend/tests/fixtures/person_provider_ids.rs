use super::*;
use axum::{Json, Router, extract::State, routing::get};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    reads: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}
impl Fixture {
    async fn new(native: Value) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("person_ids_{}", uuid::Uuid::new_v4().simple());
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
        let app = Router::new()
            .route(
                "/Items",
                get(
                    |State((native, reads)): State<(Value, Arc<AtomicUsize>)>| async move {
                        reads.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"Items":[native],"TotalRecordCount":1}))
                    },
                ),
            )
            .with_state((native, reads.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state
            .save_config(
                &json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user"}),
            )
            .await
            .unwrap();
        Self {
            state,
            admin,
            schema,
            reads,
            server,
            _directory: directory,
        }
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
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
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn missing_actor_ids_use_lowercase_native_provider_without_losing_associations() {
    let f = Fixture::new(
        json!({"Id":"p1","Type":"Person","ProviderIds":{"tmdb":"123","Imdb":"native-imdb"}}),
    )
    .await;
    let people = vec![
        json!({"Id":"p1","Type":"Actor","Name":"演员","Role":"角色一","ProviderIds":{"Imdb":"existing-imdb"}}),
        json!({"Id":"p1","Type":"Actor","Name":"演员","Role":"角色二"}),
    ];
    let result = complete_provider_ids(&f.state, &people, None)
        .await
        .unwrap();
    let reads = f.reads.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(reads, 1);
    assert_eq!(result.len(), 2);
    for (index, person) in result.iter().enumerate() {
        assert_eq!(person["Id"], people[index]["Id"]);
        assert_eq!(person["Name"], people[index]["Name"]);
        assert_eq!(person["Role"], people[index]["Role"]);
        assert_eq!(person["ProviderIds"]["Tmdb"], "123");
    }
    assert_eq!(result[0]["ProviderIds"]["Imdb"], "existing-imdb");
    assert_eq!(result[1]["ProviderIds"]["Imdb"], "native-imdb");
    assert_eq!(result[1]["ProviderIds"]["tmdb"], "123");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn existing_mixed_case_provider_is_preserved_without_refetching_native_identity() {
    let f = Fixture::new(json!({"Id":"p1","Type":"Person","ProviderIds":{"Tmdb":"999"}})).await;
    let people = vec![
        json!({"Id":"p1","Type":"Actor","Name":"演员","Role":"角色","ProviderIds":{"TMDB":"123","Imdb":"old"}}),
    ];
    let result = complete_provider_ids(&f.state, &people, None)
        .await
        .unwrap();
    let reads = f.reads.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(reads, 0);
    let mut expected = people.clone();
    expected[0]["ProviderIds"]["Tmdb"] = json!("123");
    assert_eq!(result, expected);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn native_provider_case_conflict_returns_explicit_identity_error() {
    let f =
        Fixture::new(json!({"Id":"p1","Type":"Person","ProviderIds":{"tmdb":"123","Tmdb":"999"}}))
            .await;
    let people = vec![
        json!({"Id":"p1","Type":"Actor","Name":"演员","Role":"角色","ProviderIds":{"Imdb":"old"}}),
    ];
    let error = complete_provider_ids(&f.state, &people, None)
        .await
        .unwrap_err();
    f.close().await;
    assert!(
        error.1.contains("p1") && error.1.contains("TMDb") && error.1.contains("冲突"),
        "{}",
        error.1
    );
    assert_eq!(people[0]["ProviderIds"], json!({"Imdb":"old"}));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn existing_provider_case_conflict_is_rejected_before_native_read() {
    let f = Fixture::new(json!({"Id":"p1","Type":"Person","ProviderIds":{"Tmdb":"999"}})).await;
    let people = vec![
        json!({"Id":"p1","Type":"Actor","Name":"演员","ProviderIds":{"tmdb":"123","Tmdb":"999"}}),
    ];
    let error = complete_provider_ids(&f.state, &people, None)
        .await
        .unwrap_err();
    let reads = f.reads.load(Ordering::SeqCst);
    f.close().await;
    assert!(error.1.contains("冲突"), "{}", error.1);
    assert_eq!(reads, 0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn equal_provider_case_variants_are_accepted_without_overwriting_existing_values() {
    let f = Fixture::new(json!({"Id":"p1","Type":"Person","ProviderIds":{"Tmdb":"999"}})).await;
    let people = vec![
        json!({"Id":"p1","Type":"Actor","Name":"演员","Role":"角色","ProviderIds":{"tmdb":"123","Tmdb":"123","TMDB":"123","Imdb":"old"}}),
    ];
    let result = complete_provider_ids(&f.state, &people, None)
        .await
        .unwrap();
    let reads = f.reads.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(reads, 0);
    assert_eq!(result, people);
}
