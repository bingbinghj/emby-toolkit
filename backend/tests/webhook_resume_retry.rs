use emby_toolkit::{AppState, webhook};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("webhook_retry_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let schema = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {schema}"))
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SELECT set_config('application_name',$1,false)")
                        .bind(schema)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        Self {
            state,
            root,
            schema,
            _directory: directory,
        }
    }

    async fn seed_pending(&self) -> Value {
        // An already-finalized prepared event is a valid database-only replay;
        // no upstream connection, media operation or notification is needed.
        let event = json!({
            "source":"emby", "id":"completed-media", "event":"item.add",
            "history_ids":[], "_resume_prepared":"completed-fixture",
            "_pending_id":"original-pending-id", "_queue_key":"emby:completed-media:item.add"
        });
        sqlx::query(&format!(
            "INSERT INTO {}.webhook_pending(event_key,event_json) VALUES($1,$2)",
            self.schema
        ))
        .bind("emby:completed-media:item.add")
        .bind(&event)
        .execute(&self.root)
        .await
        .unwrap();
        event
    }

    async fn hide(&self, table: &str) {
        sqlx::query(&format!(
            "ALTER TABLE {}.{table} RENAME TO unavailable_{table}",
            self.schema
        ))
        .execute(&self.root)
        .await
        .unwrap();
    }

    async fn reveal(&self, table: &str) {
        sqlx::query(&format!(
            "ALTER TABLE {}.unavailable_{table} RENAME TO {table}",
            self.schema
        ))
        .execute(&self.root)
        .await
        .unwrap();
    }

    async fn wait_failed_read(&self, table: &str) {
        // Observe the actual failed statement on this fixture's connections.
        // The control pool has a different application_name and cannot create
        // a false observation by executing the assertions themselves.
        let statement = match table {
            "webhook_pending" => "SELECT event_key,event_json FROM webhook_pending%",
            "webhook_prepared" => "SELECT p.top_id,p.item_id,p.path,%FROM webhook_prepared%",
            _ => panic!("unexpected retry table"),
        };
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let observed: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND state='idle' AND query LIKE $2)",
                )
                .bind(&self.schema)
                .bind(statement)
                .fetch_one(&self.root)
                .await
                .unwrap();
                if observed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the missing table must cause a real completed database read failure");
    }

    async fn registrations(&self) -> Vec<Value> {
        sqlx::query_scalar(&format!(
            "SELECT jsonb_build_object('id',id,'event',spec_json#>'{{payload,event}}','state',state_json) FROM {}.durable_jobs WHERE spec_json->>'kind'='webhook' AND spec_json#>>'{{payload,event,_pending_id}}'='original-pending-id' ORDER BY job_order",
            self.schema
        ))
        .fetch_all(&self.root)
        .await
        .unwrap()
    }

    async fn wait_registration(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.registrations().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the original persisted pending event must become a durable job");
    }

    async fn wait_finished(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let rows = self.registrations().await;
                if rows.len() == 1
                    && rows[0]["state"]["is_running"] == false
                    && self.pending_count().await == 0
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("database-only replay must finish");
    }

    async fn pending_count(&self) -> i64 {
        sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {}.webhook_pending",
            self.schema
        ))
        .fetch_one(&self.root)
        .await
        .unwrap()
    }

    async fn pending_row(&self, table: &str) -> Value {
        sqlx::query_scalar(&format!(
            "SELECT jsonb_build_object('event_key',event_key,'event_json',event_json,'updated_at',updated_at) FROM {}.{table}",
            self.schema
        ))
        .fetch_one(&self.root)
        .await
        .unwrap()
    }

    async fn assert_finished_once(&self, original: &Value) {
        self.wait_registration().await;
        self.wait_finished().await;
        let rows = self.registrations().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(&rows[0]["event"], original);
        assert_eq!(rows[0]["state"]["state"], "completed");
        assert_eq!(rows[0]["state"]["result"]["status"], "already_completed");
        assert_eq!(self.pending_count().await, 0);
    }

    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream connection or media writes"]
async fn pending_read_failure_retries_and_registers_the_original_event_without_restart() {
    let fixture = Fixture::new().await;
    let original = fixture.seed_pending().await;
    fixture.hide("webhook_pending").await;
    let state = fixture.state.clone();
    let cancel = CancellationToken::new();
    let running_cancel = cancel.clone();
    let recovery = tokio::spawn(async move {
        webhook::resume_pending_with_retry(&state, &running_cancel).await;
    });
    fixture.wait_failed_read("webhook_pending").await;
    assert!(
        !recovery.is_finished(),
        "a temporary query error must not abandon recovery"
    );
    assert!(fixture.registrations().await.is_empty());
    assert_eq!(
        fixture.pending_row("unavailable_webhook_pending").await["event_json"],
        original
    );
    fixture.reveal("webhook_pending").await;
    tokio::time::timeout(Duration::from_secs(5), recovery)
        .await
        .expect("recovery must retry after the table becomes available")
        .unwrap();
    fixture.assert_finished_once(&original).await;
    cancel.cancel();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream connection or media writes"]
async fn prepared_read_failure_retries_after_loading_pending_without_duplicate_registration() {
    let fixture = Fixture::new().await;
    let original = fixture.seed_pending().await;
    fixture.hide("webhook_prepared").await;
    let state = fixture.state.clone();
    let cancel = CancellationToken::new();
    let running_cancel = cancel.clone();
    let recovery = tokio::spawn(async move {
        webhook::resume_pending_with_retry(&state, &running_cancel).await;
    });
    fixture.wait_failed_read("webhook_prepared").await;
    fixture.wait_registration().await;
    assert!(
        !recovery.is_finished(),
        "the prepared-event read still needs a retry"
    );
    fixture.reveal("webhook_prepared").await;
    tokio::time::timeout(Duration::from_secs(8), recovery)
        .await
        .expect("prepared recovery must finish after the temporary failure")
        .unwrap();
    webhook::resume_pending(&fixture.state).await.unwrap();
    webhook::resume_pending(&fixture.state).await.unwrap();
    fixture.assert_finished_once(&original).await;
    cancel.cancel();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL; no upstream connection or media writes"]
async fn cancelling_a_failed_pending_read_preserves_the_event_and_stops_backoff() {
    let fixture = Fixture::new().await;
    fixture.seed_pending().await;
    let before = fixture.pending_row("webhook_pending").await;
    fixture.hide("webhook_pending").await;
    let state = fixture.state.clone();
    let cancel = CancellationToken::new();
    let running_cancel = cancel.clone();
    let recovery = tokio::spawn(async move {
        webhook::resume_pending_with_retry(&state, &running_cancel).await;
    });
    fixture.wait_failed_read("webhook_pending").await;
    assert!(!recovery.is_finished());
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(500), recovery)
        .await
        .expect("cancellation must interrupt the one-second retry backoff")
        .unwrap();
    assert!(fixture.registrations().await.is_empty());
    fixture.reveal("webhook_pending").await;
    assert_eq!(fixture.pending_row("webhook_pending").await, before);
    fixture.close().await;
}
