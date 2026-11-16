use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{Request, StatusCode},
    routing::get,
};
use emby_toolkit::AppState;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    root: Option<sqlx::PgPool>,
    schema: Option<String>,
    requests: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
    worker: Option<tokio::task::JoinHandle<()>>,
    cancel: CancellationToken,
    _directory: tempfile::TempDir,
    waiting: Arc<Notify>,
    release: Arc<Notify>,
}

#[derive(Clone)]
struct Host {
    requests: Arc<Mutex<Vec<String>>>,
    waiting: Arc<Notify>,
    release: Arc<Notify>,
}

impl Fixture {
    async fn new(database: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let waiting = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let upstream = Router::new()
            .route(
                "/Users/user/Items/{id}",
                get(|State(host): State<Host>, Path(id): Path<String>| async move {
                    let count = {
                        let mut requests = host.requests.lock().await;
                        requests.push(id.clone());
                        requests.iter().filter(|requested| **requested == id).count()
                    };
                    if id == "wrong" {
                        return (StatusCode::OK, Json(json!({"Id":"202","Type":"Movie","Name":"another movie"})));
                    }
                    if id == "remapped" {
                        return (StatusCode::OK, Json(json!({"Id":id,"Type":"Movie","Name":"another work","ProviderIds":{"Tmdb":"999"}})));
                    }
                    if id == "wait" {
                        if count == 1 {
                            return (StatusCode::OK, Json(json!({"Id":id,"Type":"Movie","Name":"queued movie","Path":"/media/wait.mkv","ProviderIds":{"Tmdb":"700"}})));
                        }
                        host.waiting.notify_one();
                        host.release.notified().await;
                    }
                    (StatusCode::NOT_FOUND, Json(json!({"error":"fixture item disappeared"})))
                }),
            )
            .with_state(Host { requests: requests.clone(), waiting: waiting.clone(), release: release.clone() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let (pool, root, schema) = if database {
            let url = std::env::var("DATABASE_URL").expect("disposable DATABASE_URL is required");
            let root = sqlx::PgPool::connect(&url).await.unwrap();
            let schema = format!("organize_queue_{}", uuid::Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&root)
                .await
                .unwrap();
            let path = format!("SET search_path TO {schema}");
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(8)
                .after_connect(move |connection, _| {
                    let path = path.clone();
                    Box::pin(async move {
                        sqlx::query(&path).execute(connection).await?;
                        Ok(())
                    })
                })
                .connect(&url)
                .await
                .unwrap();
            (pool, Some(root), Some(schema))
        } else {
            (
                sqlx::postgres::PgPoolOptions::new()
                    .acquire_timeout(Duration::from_millis(10))
                    .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
                    .unwrap(),
                None,
                None,
            )
        };
        let state = if database {
            AppState::new(pool, directory.path().into()).await.unwrap()
        } else {
            AppState {
                db: pool,
                config: Arc::new(RwLock::new(json!({}))),
                http: reqwest::Client::new(),
                sessions: Arc::new(
                    emby_toolkit::auth::SessionSigner::load(directory.path()).unwrap(),
                ),
                recovery_tokens: Arc::default(),
                data_dir: directory.path().into(),
                notifications: Arc::default(),
                webhooks: Arc::default(),
                ai: Arc::default(),
                jobs: Arc::default(),
                logs: emby_toolkit::logs::LogStore::open(&directory.path().join("logs")).unwrap(),
                p115: Arc::default(),
                strm: Arc::default(),
                douban: Arc::default(),
                docker: Arc::default(),
            }
        };
        state.config.write().await.as_object_mut().unwrap().extend(
            json!({"emby_server_url":base,"emby_user_id":"user","emby_api_key":"fixture","telegram_bot_token":""})
                .as_object().unwrap().clone(),
        );
        Self {
            state,
            root,
            schema,
            requests,
            server,
            worker: None,
            cancel: CancellationToken::new(),
            _directory: directory,
            waiting,
            release,
        }
    }

    fn start(&mut self) {
        self.cancel = CancellationToken::new();
        self.worker = Some(tokio::spawn(emby_toolkit::media_organize_queue::run(
            self.state.clone(),
            self.cancel.clone(),
        )));
    }

    async fn stop_worker(&mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            tokio::time::timeout(Duration::from_secs(5), worker)
                .await
                .unwrap()
                .unwrap();
        }
    }

    async fn request(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let token = self.state.sessions.sign(&emby_toolkit::auth::Session {
            user_id: "fixture-operator".into(),
            name: "Fixture Operator".into(),
            setup_mode: false,
            expires: chrono::Utc::now().timestamp() + 120,
        });
        let response = emby_toolkit::actions::router()
            .merge(emby_toolkit::media_organize_queue::router())
            .with_state(self.state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("cookie", format!("etk_session={token}"))
                    .body(if body.is_null() {
                        Body::empty()
                    } else {
                        Body::from(body.to_string())
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn submit(&self, body: Value) -> (StatusCode, Value) {
        self.request("POST", "/api/actions/reorganize", body).await
    }

    async fn row(&self, id: &str) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(q) FROM media_organize_queue q WHERE id=$1")
            .bind(id)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    async fn wait_status(&self, id: &str, status: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let row = self.row(id).await;
                if row["status"] == status {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|error| panic!("queue entry {id} did not reach status {status}: {error}"))
    }

    async fn history(&self, item: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_id,item_json,status) VALUES($1,'emby',$2,$3,$4,'success')")
            .bind(&id).bind(format!("/media/{item}.mkv")).bind(item)
            .bind(json!({"item_id":item,"item_type":"Movie","name":"queued movie","media_source":"tmdb","media_id":"700"}))
            .execute(&self.state.db).await.unwrap();
        id
    }

    async fn close(mut self) {
        self.release.notify_one();
        self.stop_worker().await;
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        if let Some(root) = self.root.take() {
            sqlx::query(&format!(
                "DROP SCHEMA {} CASCADE",
                self.schema.take().unwrap()
            ))
            .execute(&root)
            .await
            .unwrap();
            root.close().await;
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}

#[tokio::test]
async fn an_explicit_valid_selection_and_mode_are_required_before_starting_any_work() {
    let f = Fixture::new(false).await;
    for body in [
        json!({}),
        json!({"item_ids":[]}),
        json!({"item_ids":"all"}),
        json!({"item_ids":["101",null]}),
        json!({"item_ids":["../101"]}),
        json!({"item_ids":[""]}),
        json!({"item_ids":vec!["101";1001]}),
        json!({"item_ids":["101"],"mode":"unknown"}),
        json!({"item_ids":["101"],"mode":null}),
        json!({"item_ids":["101"],"mode":false}),
    ] {
        let (status, result) = f.submit(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{result}");
        assert_eq!(f.state.jobs.status().await["is_running"], false);
    }
    assert!(f.requests.lock().await.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn selections_are_persisted_and_deduplicated_per_media_and_operation_without_starting_work() {
    let f = Fixture::new(true).await;
    let (status, first) = f.submit(json!({"item_ids":["101","101","202"]})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    assert_eq!(first["queued_count"], 2);
    assert_eq!(first["duplicate_count"], 0);
    assert_eq!(first["mode"], "reprocess");
    let (_, duplicate) = f.submit(json!({"item_ids":["202","101"]})).await;
    assert_eq!(duplicate["queued_count"], 0);
    assert_eq!(duplicate["duplicate_count"], 2);
    assert_eq!(duplicate["queue_ids"][0], first["queue_ids"][1]);
    assert_eq!(duplicate["queue_ids"][1], first["queue_ids"][0]);
    let (_, scrape) = f
        .submit(json!({"item_ids":["101"],"mode":"rescrape"}))
        .await;
    assert_eq!(scrape["queued_count"], 1);
    assert_ne!(scrape["queue_ids"][0], first["queue_ids"][0]);
    let (_, queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    assert_eq!(queue["total"], 3);
    assert_eq!(queue["queued_count"], 3);
    assert_eq!(queue["running_count"], 0);
    assert_eq!(
        queue["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["item_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["101", "202", "101"]
    );
    assert!(
        queue["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("operator_id").is_none() && row.get("history_ids").is_none())
    );
    assert_eq!(f.state.jobs.status().await["is_running"], false);
    assert!(f.requests.lock().await.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no media operations"]
async fn one_full_page_of_one_thousand_distinct_media_is_persisted_without_truncation() {
    let f = Fixture::new(true).await;
    let ids = (0..1000)
        .map(|index| format!("media{index}"))
        .collect::<Vec<_>>();
    let (status, accepted) = f.submit(json!({"item_ids":ids,"mode":"rescrape"})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["queued_count"], 1000);
    assert_eq!(accepted["duplicate_count"], 0);
    assert_eq!(accepted["queue_ids"].as_array().unwrap().len(), 1000);
    let queued: Vec<String> =
        sqlx::query_scalar("SELECT item_id FROM media_organize_queue ORDER BY queue_order")
            .fetch_all(&f.state.db)
            .await
            .unwrap();
    assert_eq!(queued, ids);
    assert!(f.requests.lock().await.is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no media operations"]
async fn concurrent_reverse_order_batches_do_not_deadlock_or_create_duplicate_active_entries() {
    let f = Fixture::new(true).await;
    let (a, b) = tokio::join!(f.state.db.acquire(), f.state.db.acquire());
    drop((a.unwrap(), b.unwrap()));
    // Ordinary insert latency gives both opposing batches time to overlap;
    // this reproduces the old lock-order deadlock instead of relying on luck.
    sqlx::raw_sql(
        "CREATE FUNCTION delay_queue_insert() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_sleep(0.01); RETURN NEW; END $$;
         CREATE TRIGGER delay_queue_insert BEFORE INSERT ON media_organize_queue
         FOR EACH ROW EXECUTE FUNCTION delay_queue_insert()",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    let ids = (1..=32).map(|id| id.to_string()).collect::<Vec<_>>();
    let reverse = ids.iter().rev().cloned().collect::<Vec<_>>();
    let ((a_status, a), (b_status, b)) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(
            f.submit(json!({"item_ids":ids})),
            f.submit(json!({"item_ids":reverse}))
        )
    })
    .await
    .expect("concurrent enqueue did not finish");
    assert_eq!(a_status, StatusCode::ACCEPTED, "{a}");
    assert_eq!(b_status, StatusCode::ACCEPTED, "{b}");
    assert_eq!(
        a["queued_count"].as_u64().unwrap() + b["queued_count"].as_u64().unwrap(),
        32
    );
    assert_eq!(
        a["duplicate_count"].as_u64().unwrap() + b["duplicate_count"].as_u64().unwrap(),
        32
    );
    let (_, queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    assert_eq!(queue["total"], 32);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn an_existing_background_job_delays_the_queue_and_cancelled_waiting_entries_never_execute() {
    let mut f = Fixture::new(true).await;
    let release = Arc::new(Notify::new());
    let started = Arc::new(Notify::new());
    let done = release.clone();
    let ready = started.clone();
    f.state
        .jobs
        .submit("fixture busy job", move |_| async move {
            ready.notify_one();
            done.notified().await;
            Ok(json!({"message":"busy job finished"}))
        })
        .await
        .unwrap();
    started.notified().await;
    let (_, accepted) = f.submit(json!({"item_ids":["101","202"]})).await;
    let first = accepted["queue_ids"][0].as_str().unwrap();
    let cancelled = accepted["queue_ids"][1].as_str().unwrap();
    f.start();
    let (status, response) = f
        .request(
            "DELETE",
            &format!("/api/media_organize/queue/{cancelled}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let (_, queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    assert_eq!(queue["queued_count"], 1);
    assert_eq!(queue["running_count"], 0);
    assert!(
        queue["items"][0]["message"]
            .as_str()
            .unwrap()
            .contains("等待当前后台任务")
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(f.requests.lock().await.is_empty());
    release.notify_one();
    f.wait_status(first, "failed").await;
    assert_eq!(f.row(cancelled).await["status"], "cancelled");
    assert_eq!(*f.requests.lock().await, ["101"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn running_entries_cannot_be_removed_and_repeated_history_selection_deduplicates() {
    let mut f = Fixture::new(true).await;
    let history = f.history("wait").await;
    let body = json!({"item_ids":["wait"],"history_ids":[history]});
    let (_, accepted) = f.submit(body.clone()).await;
    let id = accepted["queue_ids"][0].as_str().unwrap();
    f.start();
    tokio::time::timeout(Duration::from_secs(5), f.waiting.notified())
        .await
        .unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM media_organize_history WHERE id=$1")
            .bind(&history)
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(status, "processing");
    let (status, _) = f
        .request(
            "DELETE",
            &format!("/api/media_organize/queue/{id}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = f
        .request(
            "POST",
            &format!("/api/media_organize/queue/{id}/retry"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, duplicate) = f.submit(body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{duplicate}");
    assert_eq!(duplicate["duplicate_count"], 1);
    assert_eq!(duplicate["queue_ids"][0], id);
    let running = f.row(id).await;
    let task = running["task_id"].as_str().unwrap();
    let (read_status, read_queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    assert_eq!(read_status, StatusCode::OK);
    let read_item = read_queue["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap();
    assert_eq!(read_item["status"], "running");
    assert_eq!(
        read_item["job_state"],
        f.state.jobs.result(task).await.unwrap()["state"]
    );
    f.state.jobs.control(Some(task), "pause").await.unwrap();
    let (_, pausing_queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    let pausing_item = pausing_queue["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap();
    assert_eq!(pausing_item["status"], "running");
    assert_eq!(
        pausing_item["job_state"],
        f.state.jobs.result(task).await.unwrap()["state"]
    );
    assert_eq!(pausing_item["pause_requested"], true);
    f.state.jobs.control(Some(task), "stop").await.unwrap();
    f.release.notify_one();
    f.wait_status(id, "cancelled").await;
    f.stop_worker().await;
    let (status, retried) = f
        .request(
            "POST",
            &format!("/api/media_organize/queue/{id}/retry"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
    assert_ne!(retried["queue_ids"][0], id);
    assert_eq!(retried["queued_count"], 1);
    assert_eq!(f.row(id).await["status"], "cancelled");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn failure_does_not_stop_later_entries_and_retry_creates_a_fresh_attempt() {
    let mut f = Fixture::new(true).await;
    let (_, accepted) = f.submit(json!({"item_ids":["101","101","202"]})).await;
    let first = accepted["queue_ids"][0].as_str().unwrap();
    let second = accepted["queue_ids"][1].as_str().unwrap();
    f.start();
    f.wait_status(first, "failed").await;
    let failed = f.wait_status(second, "failed").await;
    assert!(
        failed["result"]["error"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    assert_eq!(*f.requests.lock().await, ["101", "202"]);
    let (_, idle_queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    assert_eq!(
        idle_queue["items"],
        json!([]),
        "terminal failures must leave the active queue"
    );
    assert_eq!(idle_queue["total"], 0);
    assert_eq!(idle_queue["queued_count"], 0);
    assert_eq!(idle_queue["running_count"], 0);
    f.stop_worker().await;
    let (status, retried) = f
        .request(
            "POST",
            &format!("/api/media_organize/queue/{first}/retry"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
    let retry = retried["queue_ids"][0].as_str().unwrap();
    assert_ne!(retry, first);
    let (_, retry_queue) = f
        .request("GET", "/api/media_organize/queue", Value::Null)
        .await;
    assert_eq!(retry_queue["total"], 1);
    assert_eq!(retry_queue["items"][0]["id"], retry);
    f.start();
    f.wait_status(retry, "failed").await;
    assert_eq!(f.row(first).await["status"], "failed");
    assert_eq!(*f.requests.lock().await, ["101", "202", "101"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; terminal queue history remains durable and read-only"]
async fn five_previous_failures_do_not_occupy_active_queue_pages_or_erase_their_results() {
    let f = Fixture::new(true).await;
    for _ in 0..5 {
        let (status, accepted) = f.submit(json!({"item_ids":["101"]})).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = accepted["queue_ids"][0].as_str().unwrap();
        sqlx::query("UPDATE media_organize_queue SET status='failed',message='完整入库未完成',result='{\"error\":\"完整入库未完成\"}',completed_at=NOW() WHERE id=$1")
            .bind(id).execute(&f.state.db).await.unwrap();
    }
    let before: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(q) ORDER BY queue_order) FROM media_organize_queue q",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    let (_, idle) = f
        .request(
            "GET",
            "/api/media_organize/queue?page=1&page_size=1",
            Value::Null,
        )
        .await;
    assert_eq!(idle["items"], json!([]));
    assert_eq!(idle["total"], 0);
    let after: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(q) ORDER BY queue_order) FROM media_organize_queue q",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "hiding finished attempts must not delete recovery or failure evidence"
    );
    let (_, accepted) = f.submit(json!({"item_ids":["101"]})).await;
    assert_eq!(accepted["queued_count"], 1);
    assert_eq!(accepted["duplicate_count"], 0);
    let (_, current) = f
        .request(
            "GET",
            "/api/media_organize/queue?page=1&page_size=1",
            Value::Null,
        )
        .await;
    assert_eq!(current["total"], 1);
    assert_eq!(current["queued_count"], 1);
    assert_eq!(current["items"][0]["id"], accepted["queue_ids"][0]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn changed_native_identity_or_work_binding_cannot_expand_the_selected_scope() {
    let mut f = Fixture::new(true).await;
    let wrong = f.history("wrong").await;
    let remapped = f.history("remapped").await;
    let (_, accepted) = f
        .submit(json!({"item_ids":["wrong","remapped"],"history_ids":[wrong,remapped]}))
        .await;
    f.start();
    let wrong = f
        .wait_status(accepted["queue_ids"][0].as_str().unwrap(), "failed")
        .await;
    let remapped = f
        .wait_status(accepted["queue_ids"][1].as_str().unwrap(), "failed")
        .await;
    assert!(wrong["result"]["error"].as_str().unwrap().contains("身份"));
    assert!(
        remapped["result"]["error"]
            .as_str()
            .unwrap()
            .contains("作品绑定")
    );
    assert_eq!(*f.requests.lock().await, ["wrong", "remapped"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn restart_requeues_unfinished_entries_and_rereads_native_identity_before_execution() {
    let mut f = Fixture::new(true).await;
    let (_, accepted) = f.submit(json!({"item_ids":["101","202"]})).await;
    let running = accepted["queue_ids"][0].as_str().unwrap();
    let queued = accepted["queue_ids"][1].as_str().unwrap();
    sqlx::query("UPDATE media_organize_queue SET status='running',task_id='previous-process-task',started_at=NOW() WHERE id=$1")
        .bind(running).execute(&f.state.db).await.unwrap();
    let config = f.state.config.read().await.clone();
    let restored = AppState::new(f.state.db.clone(), f.state.data_dir.clone())
        .await
        .unwrap();
    *restored.config.write().await = config;
    f.state = restored;
    assert_eq!(f.row(running).await["status"], "queued");
    assert_eq!(f.row(queued).await["status"], "queued");
    assert!(f.requests.lock().await.is_empty());
    f.start();
    f.wait_status(running, "failed").await;
    f.wait_status(queued, "failed").await;
    assert_eq!(*f.requests.lock().await, ["101", "202"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no media operations"]
async fn legacy_interrupted_entries_remain_retryable_without_modifying_old_results() {
    let f = Fixture::new(true).await;
    let (_, accepted) = f
        .submit(json!({"item_ids":["101"],"mode":"rescrape"}))
        .await;
    let interrupted = accepted["queue_ids"][0].as_str().unwrap();
    sqlx::query("UPDATE media_organize_queue SET status='interrupted',message='previous process interrupted',completed_at=NOW() WHERE id=$1")
        .bind(interrupted).execute(&f.state.db).await.unwrap();
    let (status, retried) = f
        .request(
            "POST",
            &format!("/api/media_organize/queue/{interrupted}/retry"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
    assert_eq!(retried["mode"], "rescrape");
    assert_ne!(retried["queue_ids"][0], interrupted);
    let old = f.row(interrupted).await;
    assert_eq!(old["status"], "interrupted");
    assert_eq!(old["message"], "previous process interrupted");
    // Restarting after a retry must neither violate active uniqueness nor
    // revive the superseded interrupted attempt alongside its queued retry.
    emby_toolkit::media_organize_queue::recover_unfinished(&f.state.db)
        .await
        .unwrap();
    assert_eq!(f.row(interrupted).await["status"], "interrupted");
    assert_eq!(
        f.row(retried["queue_ids"][0].as_str().unwrap()).await["status"],
        "queued"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; upstream is localhost"]
async fn service_shutdown_keeps_running_queue_work_and_history_available_for_automatic_resume() {
    let mut f = Fixture::new(true).await;
    let history = f.history("wait").await;
    let (_, accepted) = f
        .submit(json!({"item_ids":["wait"],"history_ids":[history]}))
        .await;
    let id = accepted["queue_ids"][0].as_str().unwrap();
    f.start();
    tokio::time::timeout(Duration::from_secs(5), f.waiting.notified())
        .await
        .unwrap();
    let running = f.row(id).await;
    let jobs = f.state.jobs.clone();
    let closing = tokio::spawn(async move { jobs.shutdown().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.jobs.status().await["state"] != "interrupted" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap();
    let pending = f.row(id).await;
    assert_eq!(pending["status"], "queued", "{pending}");
    assert!(pending["completed_at"].is_null());
    assert_eq!(pending["task_id"], running["task_id"]);
    assert_eq!(pending["started_at"], running["started_at"]);
    let history_status: String =
        sqlx::query_scalar("SELECT status FROM media_organize_history WHERE id=$1")
            .bind(&history)
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(history_status, "processing");
    assert_eq!(
        f.state
            .jobs
            .result(pending["task_id"].as_str().unwrap())
            .await
            .unwrap()["state"],
        "interrupted"
    );
    let (status, cancelled) = f
        .request(
            "DELETE",
            &format!("/api/media_organize/queue/{id}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    let history_status: String =
        sqlx::query_scalar("SELECT status FROM media_organize_history WHERE id=$1")
            .bind(&history)
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(history_status, "failed");
    assert_eq!(f.row(id).await["status"], "cancelled");
    f.close().await;
}
