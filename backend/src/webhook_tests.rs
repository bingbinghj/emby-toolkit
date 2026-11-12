use super::*;

#[path = "webhook_metadata_control_tests.rs"]
mod metadata_control;

struct Fixture {
    admin: sqlx::PgPool,
    schema: String,
    state: AppState,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let admin = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("webhook_retry_{}", uuid::Uuid::new_v4().simple());
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
            .connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let directory =
            tempfile::tempdir_in("/root/emby-toolkit-nov12-followup3-verification").unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        Self {
            admin,
            schema,
            state,
            _directory: directory,
        }
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
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn failed_submit_preserves_newer_pending_episode_and_history_ids() {
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    // Control the dequeue/submit ordering without racing the background timer.
    state
        .webhooks
        .draining
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let key = "emby:series:item.add".to_string();
    enqueue(
        state,
        key.clone(),
        json!({"source":"emby","id":"series","event":"item.add",
            "episode_ids":["episode1"],"history_ids":["history1"]}),
    )
    .await;
    let (key, event) = state.webhooks.pending.lock().await.pop_first().unwrap();
    state
        .jobs
        .submit("抢占任务槽", |context| async move {
            context.cancelled().await;
            context.checkpoint().await?;
            Ok(Value::Null)
        })
        .await
        .unwrap();
    enqueue(
        state,
        key.clone(),
        json!({"source":"emby","id":"series","event":"item.add",
            "episode_ids":["episode2"],"history_ids":["history2"]}),
    )
    .await;
    submit_queued_event(state, key.clone(), event).await;
    let pending = state.webhooks.pending.lock().await[&key].clone();
    fixture.close().await;
    assert_eq!(pending["episode_ids"], json!(["episode1", "episode2"]));
    assert_eq!(list(&pending["history_ids"]).len(), 2);
    assert!(list(&pending["history_ids"]).contains(&json!("history1")));
    assert!(list(&pending["history_ids"]).contains(&json!("history2")));
}
