use emby_toolkit::{
    AppState,
    jobs::JobSpec,
    monitor::{Service, Settings},
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("monitor_retry_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let application_name = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |connection, _| {
                let search = search.clone();
                let application_name = application_name.clone();
                Box::pin(async move {
                    sqlx::query(&search).execute(&mut *connection).await?;
                    sqlx::query("SELECT set_config('application_name',$1,false)")
                        .bind(application_name)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let media = directory.path().join("media");
        tokio::fs::create_dir(&media).await.unwrap();
        let path = media.join("pending.strm");
        tokio::fs::write(&path, "http://127.0.0.1:9/unused-test-source")
            .await
            .unwrap();
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        state.save_config(&json!({"monitor_enabled":true,"monitor_paths":[media],
            "monitor_extensions":[".strm"],"monitor_exclude_dirs":[],"monitor_exclude_refresh_delay":0,
            "emby_server_url":"","emby_api_key":"","extract_thumb":false})).await.unwrap();
        sqlx::query(
            "INSERT INTO monitor_pending_events(id,path) VALUES('pending-before-restart',$1)",
        )
        .bind(path.to_string_lossy().as_ref())
        .execute(&state.db)
        .await
        .unwrap();
        Self {
            state,
            admin,
            schema,
            _directory: directory,
            path,
        }
    }

    async fn pending(&self) -> Vec<Value> {
        sqlx::query_scalar("SELECT to_jsonb(p) FROM monitor_pending_events p ORDER BY id")
            .fetch_all(&self.state.db)
            .await
            .unwrap()
    }

    async fn dispatched(&self) -> Option<Value> {
        sqlx::query_scalar("SELECT spec_json->'payload' FROM durable_jobs WHERE lane='monitor' AND spec_json->'payload'->'pending_ids' ? 'pending-before-restart'")
            .fetch_optional(&self.state.db).await.unwrap()
    }

    async fn wait_dispatched(&self) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(payload) = self.dispatched().await {
                    return payload;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the retained event must dispatch after database recovery")
    }

    async fn wait_failed_open(&self) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                // The sole fixture connection last ran ALTER TABLE before
                // starting the service. Only the service can run this missing
                // JOIN; the separate control pool cannot satisfy this probe.
                let failed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND state='idle' AND query LIKE '%FROM durable_jobs j LEFT JOIN durable_job_steps%')")
                    .bind(&self.schema).fetch_one(&self.admin).await.unwrap();
                if failed { return; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the lane must encounter the real missing-table error");
    }

    async fn install_resume_fault(&self) -> Value {
        let settings = Settings::read(&self.state.config.read().await.clone()).await;
        let payload =
            json!({"batch_id":"original-batch","pending_ids":[],"paths":[],"settings":settings});
        let spec = JobSpec::new("monitor", payload);
        let progress = json!({"task_id":"original-monitor-job","state":"interrupted","is_running":false,
            "last_action":"原监控任务","current_action":"原监控任务","message":"等待恢复","stop_requested":false});
        sqlx::query("INSERT INTO durable_jobs(id,lane,spec_json,state_json) VALUES('original-monitor-job','monitor',$1,$2)")
            .bind(json!(spec)).bind(&progress).execute(&self.state.db).await.unwrap();
        sqlx::query("CREATE TABLE recovery_gate(allowed BOOLEAN NOT NULL)")
            .execute(&self.state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO recovery_gate VALUES(false)")
            .execute(&self.state.db)
            .await
            .unwrap();
        sqlx::query("CREATE SEQUENCE recovery_attempts")
            .execute(&self.state.db)
            .await
            .unwrap();
        sqlx::query("CREATE FUNCTION reject_monitor_preparation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='original-monitor-job' AND NEW.state_json->>'cleanup_pending'='false' THEN PERFORM nextval('recovery_attempts'); IF NOT (SELECT allowed FROM recovery_gate) THEN RAISE EXCEPTION 'temporary preparation database failure'; END IF; END IF; RETURN NEW; END $$")
            .execute(&self.state.db).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_monitor_preparation BEFORE UPDATE ON durable_jobs FOR EACH ROW EXECUTE FUNCTION reject_monitor_preparation()")
            .execute(&self.state.db).await.unwrap();
        progress
    }

    async fn wait_failed_preparation(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let called: bool = sqlx::query_scalar("SELECT is_called FROM recovery_attempts")
                    .fetch_one(&self.state.db)
                    .await
                    .unwrap();
                if called {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the real recovery update must encounter the temporary database error");
    }

    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream writes"]
async fn failed_lane_open_retries_and_dispatches_the_original_pending_event() {
    let f = Fixture::new().await;
    sqlx::query("ALTER TABLE durable_job_steps RENAME TO temporarily_unavailable_steps")
        .execute(&f.state.db)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let service = Service::new(f.state.clone());
    let run = tokio::spawn(service.run(cancel.clone()));
    f.wait_failed_open().await;
    assert!(
        !run.is_finished(),
        "temporary lane read failure must not terminate monitoring"
    );
    let before = f.pending().await;
    sqlx::query("ALTER TABLE temporarily_unavailable_steps RENAME TO durable_job_steps")
        .execute(&f.state.db)
        .await
        .unwrap();
    let payload = f.wait_dispatched().await;
    assert_eq!(payload["pending_ids"], json!(["pending-before-restart"]));
    assert_eq!(payload["paths"], json!([f.path]));
    assert_eq!(before.len(), 1);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_jobs WHERE lane='monitor' AND spec_json->'payload'->'pending_ids' ? 'pending-before-restart'")
        .fetch_one(&f.state.db).await.unwrap();
    assert_eq!(
        count, 1,
        "opening retry must not create duplicate event jobs"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream writes"]
async fn cancelling_failed_lane_open_keeps_pending_events_and_exits_promptly() {
    let f = Fixture::new().await;
    let before = f.pending().await;
    sqlx::query("ALTER TABLE durable_job_steps RENAME TO temporarily_unavailable_steps")
        .execute(&f.state.db)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let run = tokio::spawn(Service::new(f.state.clone()).run(cancel.clone()));
    f.wait_failed_open().await;
    assert!(!run.is_finished());
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(500), run)
        .await
        .expect("cancellation must interrupt retry backoff")
        .unwrap();
    sqlx::query("ALTER TABLE temporarily_unavailable_steps RENAME TO durable_job_steps")
        .execute(&f.state.db)
        .await
        .unwrap();
    assert_eq!(f.pending().await, before);
    assert!(f.dispatched().await.is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream writes"]
async fn failed_lane_preparation_retries_the_same_job_before_dispatching_pending_events() {
    let f = Fixture::new().await;
    f.install_resume_fault().await;
    let before = f.pending().await;
    let cancel = CancellationToken::new();
    let run = tokio::spawn(Service::new(f.state.clone()).run(cancel.clone()));
    f.wait_failed_preparation().await;
    assert_eq!(f.pending().await, before);
    assert!(
        f.dispatched().await.is_none(),
        "dispatch must wait for original resource preparation"
    );
    sqlx::query("UPDATE recovery_gate SET allowed=true")
        .execute(&f.state.db)
        .await
        .unwrap();
    let payload = f.wait_dispatched().await;
    assert_eq!(payload["pending_ids"], json!(["pending-before-restart"]));
    let original: Value =
        sqlx::query_scalar("SELECT state_json FROM durable_jobs WHERE id='original-monitor-job'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(original["task_id"], "original-monitor-job");
    assert_eq!(original["state"], "completed");
    assert_eq!(original["result"]["prepared"], 0);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_jobs WHERE lane='monitor' AND spec_json->'payload'->>'batch_id'='original-batch'")
        .fetch_one(&f.state.db).await.unwrap();
    assert_eq!(
        count, 1,
        "the old job must keep its original ID rather than being replayed as a new job"
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream writes"]
async fn cancelling_failed_lane_preparation_keeps_original_job_and_pending_events() {
    let f = Fixture::new().await;
    let progress = f.install_resume_fault().await;
    let before = f.pending().await;
    let cancel = CancellationToken::new();
    let run = tokio::spawn(Service::new(f.state.clone()).run(cancel.clone()));
    f.wait_failed_preparation().await;
    assert!(f.dispatched().await.is_none());
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(500), run)
        .await
        .expect("cancellation must interrupt recovery backoff")
        .unwrap();
    assert_eq!(f.pending().await, before);
    let retained: Value =
        sqlx::query_scalar("SELECT state_json FROM durable_jobs WHERE id='original-monitor-job'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(retained, progress);
    assert!(f.dispatched().await.is_none());
    f.close().await;
}
