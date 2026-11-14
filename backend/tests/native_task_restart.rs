use axum::{Json, Router, routing::get};
use emby_toolkit::{AppState, emby};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const KEY: &str = "MediaInfoExtractTask";

fn task(state: &str, run: &str, status: &str) -> Value {
    json!({"Id":"extract","Key":KEY,"Name":"Extract MediaInfo","State":state,
        "LastExecutionResult":{"StartTimeUtc":run,"Status":status}})
}

#[derive(Clone)]
struct Host {
    task: Arc<Mutex<Value>>,
    reads: Arc<AtomicUsize>,
    starts: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
}

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    host: Host,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(initial: &str) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("native_restart_{}", uuid::Uuid::new_v4().simple());
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
        let path = std::env::temp_dir();
        std::fs::create_dir_all(&path).unwrap();
        let directory = tempfile::tempdir_in(&path).unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        let host = Host {
            task: Arc::new(Mutex::new(task(initial, "old", "Completed"))),
            reads: Arc::default(),
            starts: Arc::default(),
            stops: Arc::default(),
        };
        let reads = host.clone();
        let starts = host.clone();
        let stops = host.clone();
        let app = Router::new()
            .route(
                "/ScheduledTasks",
                get(move || {
                    let host = reads.clone();
                    async move {
                        let current = host.task.lock().await.clone();
                        host.reads.fetch_add(1, Ordering::SeqCst);
                        Json(json!([current]))
                    }
                }),
            )
            .route(
                "/ScheduledTasks/Running/extract",
                axum::routing::post(move || {
                    starts.starts.fetch_add(1, Ordering::SeqCst);
                    async { axum::http::StatusCode::NO_CONTENT }
                })
                .delete(move || {
                    let host = stops.clone();
                    async move {
                        host.stops.fetch_add(1, Ordering::SeqCst);
                        *host.task.lock().await = task("Idle", "cancelled-run", "Aborted");
                        axum::http::StatusCode::NO_CONTENT
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state
            .save_config(
                &json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin"}),
            )
            .await
            .unwrap();
        Self {
            state,
            root,
            schema,
            directory,
            host,
            server,
        }
    }

    async fn submit(&self, restored_id: Option<&str>) -> String {
        let work = self.state.clone();
        // Model an owner restoring the same context, as the organize queue does.
        self.state
            .jobs
            .submit_owned(
                "原生提取重启验收",
                json!({"kind":"native_restart_probe","key":KEY}),
                restored_id,
                move |context| async move {
                    emby::scheduled(&work, KEY, &context).await?;
                    Ok(json!({"message":"本次原生提取已完成"}))
                },
            )
            .await
            .unwrap()
    }

    async fn wait_plan(&self, id: &str, key: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let value: Option<Value> = sqlx::query_scalar(
                    "SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key=$2",
                )
                .bind(id)
                .bind(format!("plans:native-task:{KEY}"))
                .fetch_optional(&self.state.db)
                .await
                .unwrap();
                if value.is_some_and(|value| value[key] == true) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("native intent must be durable before shutdown");
    }

    async fn shutdown(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.state.jobs.shutdown())
            .await
            .expect("shutdown must leave native execution running");
    }

    async fn restart(&mut self) {
        self.state = AppState::new(self.state.db.clone(), self.directory.path().into())
            .await
            .unwrap();
    }

    async fn wait_read(&self, after: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.host.reads.load(Ordering::SeqCst) <= after {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recovery must read the original native task");
    }

    async fn finish(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if result["is_running"] == false {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the original native execution must settle")
    }

    async fn close(self) {
        self.state.jobs.shutdown().await;
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
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; native API is localhost"]
async fn accepted_native_start_with_delayed_readback_is_not_submitted_again_after_restart() {
    let mut fixture = Fixture::new("Idle").await;
    let id = fixture.submit(None).await;
    fixture.wait_plan(&id, "accepted").await;
    fixture.shutdown().await;
    assert_eq!(fixture.host.stops.load(Ordering::SeqCst), 0);
    fixture.restart().await;
    assert_eq!(fixture.submit(Some(&id)).await, id);
    let host = fixture.host.clone();
    let completion = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(3500)).await;
        *host.task.lock().await = task("Idle", "accepted-run", "Completed");
    });
    let result = fixture.finish(&id).await;
    completion.await.unwrap();
    let starts = fixture.host.starts.load(Ordering::SeqCst);
    let stops = fixture.host.stops.load(Ordering::SeqCst);
    fixture.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(
        starts, 1,
        "a delayed original result cannot authorize another POST"
    );
    assert_eq!(stops, 0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; native API is localhost"]
async fn borrowed_native_running_task_remains_borrowed_across_restart() {
    let mut fixture = Fixture::new("Running").await;
    let id = fixture.submit(None).await;
    fixture.wait_plan(&id, "initial_running").await;
    fixture.shutdown().await;
    let reads = fixture.host.reads.load(Ordering::SeqCst);
    fixture.restart().await;
    assert_eq!(fixture.submit(Some(&id)).await, id);
    fixture.wait_read(reads).await;
    *fixture.host.task.lock().await = task("Idle", "borrowed-run", "Completed");
    let result = fixture.finish(&id).await;
    let starts = fixture.host.starts.load(Ordering::SeqCst);
    let stops = fixture.host.stops.load(Ordering::SeqCst);
    fixture.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(starts, 0, "recovery attaches to the running task");
    assert_eq!(stops, 0, "shutdown cannot stop an externally owned task");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; native API is localhost"]
async fn user_stop_after_owned_native_restart_stops_once_and_remains_terminal() {
    let mut fixture = Fixture::new("Idle").await;
    let id = fixture.submit(None).await;
    fixture.wait_plan(&id, "accepted").await;
    *fixture.host.task.lock().await = task("Running", "old", "Completed");
    fixture.shutdown().await;
    assert_eq!(fixture.host.stops.load(Ordering::SeqCst), 0);
    fixture.restart().await;
    assert_eq!(fixture.submit(Some(&id)).await, id);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.state.jobs.status().await["message"]
            .as_str()
            .is_some_and(|message| message.contains("已有任务运行"))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture.state.jobs.control(Some(&id), "stop").await.unwrap();
    let result = fixture.finish(&id).await;
    assert_eq!(result["state"], "cancelled", "{result}");
    fixture.restart().await;
    fixture
        .state
        .jobs
        .resume_pending(fixture.state.clone(), CancellationToken::new())
        .await
        .unwrap();
    let result = fixture.state.jobs.result(&id).await.unwrap();
    let starts = fixture.host.starts.load(Ordering::SeqCst);
    let stops = fixture.host.stops.load(Ordering::SeqCst);
    fixture.close().await;
    assert_eq!(result["state"], "cancelled");
    assert_eq!(starts, 1, "the original accepted task cannot be restarted");
    assert_eq!(
        stops, 1,
        "only a user stop terminates the owned native task"
    );
}
