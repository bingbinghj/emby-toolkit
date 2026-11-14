use axum::{Json, Router, routing::post};
use emby_toolkit::{AppState, ai, db};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};

#[derive(Clone)]
struct Host {
    requests: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    block: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
}

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    host: Host,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("ai_batch_reads_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&search).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        let host = Host {
            requests: Arc::default(),
            block: Arc::default(),
            entered: Arc::default(),
            release: Arc::new(Semaphore::new(0)),
        };
        let observed = host.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let host = observed.clone();
                async move {
                    let inputs: BTreeMap<String, String> =
                        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap())
                            .unwrap();
                    host.requests.lock().await.push(inputs.clone());
                    if host.block.load(Ordering::SeqCst) {
                        host.entered.notify_one();
                        host.release.acquire().await.unwrap().forget();
                    }
                    let output: BTreeMap<_, _> = inputs
                        .into_iter()
                        .map(|(id, source)| (id, format!("中文{source}")))
                        .collect();
                    Json(json!({"choices":[{"message":{"content":json!(output).to_string()}}]}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state
            .save_config(&json!({"ai_provider":"openai","ai_api_key":"fixture",
                "ai_base_url":format!("http://{address}/v1"),"ai_model_name":"fixture",
                "ai_max_retries":0,"ai_name_role_request_interval":0}))
            .await
            .unwrap();
        db::save_setting(
            &state.db,
            "ai_user_prompts",
            &json!({"batch_title_translation":"fixture prompt"}),
        )
        .await
        .unwrap();
        Self {
            state,
            root,
            schema,
            _directory: directory,
            host,
            server,
        }
    }

    async fn seed(&self, source: &str, translated: Option<&str>) {
        let config = self.state.config.read().await.clone();
        let key = ai::cache_key(
            &config,
            "batch_title_translation",
            "fixture prompt",
            &json!(source),
            &json!({}),
            &Value::Null,
        );
        sqlx::query("INSERT INTO translation_cache(original_text,translated_text) VALUES($1,$2) ON CONFLICT(original_text) DO UPDATE SET translated_text=EXCLUDED.translated_text")
            .bind(key).bind(translated).execute(&self.state.db).await.unwrap();
    }

    async fn translate(&self, inputs: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        ai::translate(
            &self.state,
            inputs,
            "batch_title_translation",
            &json!({}),
            &Value::Null,
        )
        .await
        .unwrap()
    }

    async fn close(self) {
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn cold_memory_reads_fifty_database_hits_with_one_cache_query() {
    let fixture = Fixture::new().await;
    let mut inputs = BTreeMap::new();
    let mut expected = BTreeMap::new();
    for index in 0..50 {
        let source = format!("Source{index}");
        let translated = format!("数据库译文{index}");
        fixture.seed(&source, Some(&translated)).await;
        inputs.insert(index.to_string(), source);
        expected.insert(index.to_string(), translated);
    }
    // The view adds one fixed delay per SELECT, representing a database round trip.
    sqlx::raw_sql(
        "ALTER TABLE translation_cache RENAME TO translation_cache_storage;
        CREATE TABLE cache_reads(calls BIGINT NOT NULL); INSERT INTO cache_reads VALUES(0);
        CREATE FUNCTION record_cache_read() RETURNS integer LANGUAGE plpgsql VOLATILE AS $$
        BEGIN UPDATE cache_reads SET calls=calls+1; PERFORM pg_sleep(0.01); RETURN 0; END $$;
        CREATE VIEW translation_cache AS
        WITH probe AS MATERIALIZED (SELECT record_cache_read() AS marker)
        SELECT cache.* FROM translation_cache_storage cache CROSS JOIN probe WHERE probe.marker=0;",
    )
    .execute(&fixture.state.db)
    .await
    .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(fixture.translate(&inputs).await, expected);
    let elapsed = started.elapsed();
    let queries: i64 = sqlx::query_scalar("SELECT calls FROM cache_reads")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    eprintln!(
        "cold database cache: {queries} SELECTs, {} ms",
        elapsed.as_millis()
    );
    assert!(fixture.host.requests.lock().await.is_empty());
    fixture.close().await;
    assert_eq!(
        queries, 1,
        "one input batch must use one database cache read"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn partial_hits_keep_shared_keys_and_invalid_rows_and_preview_semantics() {
    let fixture = Fixture::new().await;
    for (source, translated) in [
        ("Alpha", Some("缓存Alpha")),
        ("Empty", Some("")),
        ("Null", None),
        ("Shared", Some("共享译文")),
    ] {
        fixture.seed(source, translated).await;
    }
    let inputs = BTreeMap::from([
        ("cached".into(), "Alpha".into()),
        ("empty".into(), "Empty".into()),
        ("missing".into(), "Missing".into()),
        ("null".into(), "Null".into()),
        ("same1".into(), "Shared".into()),
        ("same2".into(), "Shared".into()),
    ]);
    let output = fixture.translate(&inputs).await;
    assert_eq!(output["cached"], "缓存Alpha");
    assert_eq!(output["same1"], "共享译文");
    assert_eq!(output["same2"], "共享译文");
    assert_eq!(output["empty"], "中文Empty");
    assert_eq!(output["null"], "中文Null");
    assert_eq!(output["missing"], "中文Missing");
    assert_eq!(
        fixture.host.requests.lock().await.as_slice(),
        &[BTreeMap::from([
            ("empty".into(), "Empty".into()),
            ("missing".into(), "Missing".into()),
            ("null".into(), "Null".into()),
        ])]
    );
    assert_eq!(fixture.translate(&inputs).await, output);
    fixture.seed("Alpha", Some("新缓存Alpha")).await;
    fixture
        .state
        .ai
        .cache_revision
        .fetch_add(1, Ordering::SeqCst);
    assert_eq!(fixture.translate(&inputs).await["cached"], "新缓存Alpha");
    assert_eq!(fixture.host.requests.lock().await.len(), 1);
    let preview = BTreeMap::from([("preview".into(), "Preview".into())]);
    for _ in 0..2 {
        let (output, warnings) = ai::translate_preview_report(
            &fixture.state,
            &preview,
            "batch_title_translation",
            &json!({}),
            &Value::Null,
        )
        .await
        .unwrap();
        assert_eq!(output["preview"], "中文Preview");
        assert!(warnings.is_empty());
    }
    assert_eq!(fixture.host.requests.lock().await.len(), 3);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translation_cache")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 5, "preview must not populate either cache");
    sqlx::query("DROP TABLE translation_cache")
        .execute(&fixture.state.db)
        .await
        .unwrap();
    let mixed = BTreeMap::from([
        ("cached".into(), "Alpha".into()),
        ("new".into(), "Fresh".into()),
    ]);
    let output = fixture.translate(&mixed).await;
    assert_eq!(output["cached"], "新缓存Alpha");
    assert_eq!(output["new"], "中文Fresh");
    assert_eq!(
        fixture.host.requests.lock().await.last(),
        Some(&BTreeMap::from([("new".into(), "Fresh".into())])),
        "a database read failure must retain memory hits",
    );
    assert_eq!(fixture.translate(&mixed).await, output);
    assert_eq!(fixture.host.requests.lock().await.len(), 4);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn cache_restore_gate_does_not_repopulate_with_an_inflight_old_revision() {
    let fixture = Fixture::new().await;
    fixture.host.block.store(true, Ordering::SeqCst);
    let inputs = BTreeMap::from([("one".into(), "One".into())]);
    let work = fixture.state.clone();
    let input = inputs.clone();
    let mut request = tokio::spawn(async move {
        ai::translate(
            &work,
            &input,
            "batch_title_translation",
            &json!({}),
            &Value::Null,
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(2), fixture.host.entered.notified())
        .await
        .unwrap();
    let gate = fixture.state.ai.cache_gate.write().await;
    fixture
        .state
        .ai
        .cache_revision
        .fetch_add(1, Ordering::SeqCst);
    sqlx::query("DELETE FROM translation_cache")
        .execute(&fixture.state.db)
        .await
        .unwrap();
    fixture.host.release.add_permits(1);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut request)
            .await
            .is_err()
    );
    drop(gate);
    assert_eq!(request.await.unwrap()["one"], "中文One");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translation_cache")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    assert_eq!(
        rows, 0,
        "pre-restore results must not overwrite restored cache"
    );
    fixture.host.block.store(false, Ordering::SeqCst);
    assert_eq!(fixture.translate(&inputs).await["one"], "中文One");
    assert_eq!(fixture.host.requests.lock().await.len(), 2);
    fixture.close().await;
}
