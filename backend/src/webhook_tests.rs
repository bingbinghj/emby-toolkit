use super::*;

#[path = "webhook_metadata_control_tests.rs"]
mod metadata_control;

#[path = "webhook_retry_cleanup_tests.rs"]
mod retry_cleanup;

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
            tempfile::tempdir_in(std::env::temp_dir()).unwrap();
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
    .await
    .unwrap();
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
    .await
    .unwrap();
    assert!(submit_queued_event(state, &key, &event).await.is_err());
    let pending = state.webhooks.pending.lock().await[&key].clone();
    fixture.close().await;
    assert_eq!(pending["episode_ids"], json!(["episode1", "episode2"]));
    assert_eq!(list(&pending["history_ids"]).len(), 2);
    assert!(list(&pending["history_ids"]).contains(&json!("history1")));
    assert!(list(&pending["history_ids"]).contains(&json!("history2")));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn accepted_pending_event_restores_its_merged_scope_after_service_restart() {
    let mut fixture = Fixture::new().await;
    fixture
        .state
        .webhooks
        .draining
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let key = "emby:series:item.add";
    for (episode, history) in [("episode1", "history1"), ("episode2", "history2")] {
        enqueue(
            &fixture.state,
            key.into(),
            json!({"source":"emby","id":"series","event":"item.add",
            "episode_ids":[episode],"history_ids":[history]}),
        )
        .await
        .unwrap();
    }
    let before = fixture.state.webhooks.pending.lock().await[key].clone();
    fixture.state.webhooks = std::sync::Arc::default();
    fixture
        .state
        .webhooks
        .draining
        .store(true, std::sync::atomic::Ordering::SeqCst);
    resume_pending(&fixture.state).await.unwrap();
    let restored = fixture.state.webhooks.pending.lock().await[key].clone();
    fixture.close().await;
    assert_eq!(restored, before);
    assert_eq!(restored["episode_ids"], json!(["episode1", "episode2"]));
    assert_eq!(list(&restored["history_ids"]).len(), 2);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn task_registered_before_pending_cleanup_is_not_submitted_twice() {
    let fixture = Fixture::new().await;
    fixture
        .state
        .webhooks
        .draining
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let key = "registered-crash-window";
    enqueue(
        &fixture.state,
        key.into(),
        json!({"source":"moviepilot","files":[],"history_ids":[]}),
    )
    .await
    .unwrap();
    let event = fixture.state.webhooks.pending.lock().await[key].clone();
    let task = fixture
        .state
        .jobs
        .submit_persistent(
            "已登记旧事件",
            crate::jobs::JobSpec::new("webhook", json!({"event":event})),
            |_| async { Ok(json!({"updated":0})) },
        )
        .await
        .unwrap();
    fixture.state.jobs.wait_idle().await.unwrap();
    submit_queued_event(&fixture.state, key, &event)
        .await
        .unwrap();
    let jobs: Vec<String> = sqlx::query_scalar("SELECT id FROM durable_jobs")
        .fetch_all(&fixture.state.db)
        .await
        .unwrap();
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_pending")
        .fetch_one(&fixture.state.db)
        .await
        .unwrap();
    fixture.close().await;
    assert_eq!(jobs, vec![task]);
    assert_eq!(pending, 0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn new_member_after_registration_keeps_a_separate_durable_event_scope() {
    let fixture = Fixture::new().await;
    fixture
        .state
        .webhooks
        .draining
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let key = "emby:series:item.add";
    enqueue(
        &fixture.state,
        key.into(),
        json!({"source":"emby","id":"series","event":"item.add",
        "episode_ids":["episode1"],"history_ids":["history1"]}),
    )
    .await
    .unwrap();
    let old = fixture.state.webhooks.pending.lock().await[key].clone();
    fixture
        .state
        .jobs
        .submit_persistent(
            "已登记事件",
            crate::jobs::JobSpec::new("webhook", json!({"event":old})),
            |_| async { Ok(json!({"updated":0})) },
        )
        .await
        .unwrap();
    fixture.state.jobs.wait_idle().await.unwrap();
    enqueue(
        &fixture.state,
        key.into(),
        json!({"source":"emby","id":"series","event":"item.add",
        "episode_ids":["episode2"],"history_ids":["history2"]}),
    )
    .await
    .unwrap();
    let pending = fixture.state.webhooks.pending.lock().await[key].clone();
    let accepted: Value =
        sqlx::query_scalar("SELECT spec_json#>'{payload,event}' FROM durable_jobs")
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    fixture.close().await;
    assert_ne!(pending["_pending_id"], old["_pending_id"]);
    assert_eq!(pending["episode_ids"], json!(["episode2"]));
    assert_eq!(pending["history_ids"], json!(["history2"]));
    assert_eq!(accepted, old);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn startup_keeps_executable_history_and_closes_failed_or_stopped_orphans() {
    let fixture = Fixture::new().await;
    for id in ["pending", "running", "failed", "stopped"] {
        sqlx::query("INSERT INTO media_organize_history(id,source,source_identity,item_id,item_json,status,message) VALUES($1,'emby',$1,'episode','{}','processing','等待')")
            .bind(id).execute(&fixture.state.db).await.unwrap();
    }
    sqlx::query("INSERT INTO webhook_pending(event_key,event_json) VALUES('pending',$1)")
        .bind(json!({"history_ids":["pending"]}))
        .execute(&fixture.state.db)
        .await
        .unwrap();
    for (id, status, stopped) in [
        ("running", "interrupted", false),
        ("failed", "failed", false),
        ("stopped", "stopping", true),
    ] {
        sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json) VALUES($1,$2,$3)")
            .bind(id)
            .bind(json!({"kind":"webhook","payload":{"event":{"history_ids":[id]}}}))
            .bind(json!({"state":status,"stop_requested":stopped}))
            .execute(&fixture.state.db)
            .await
            .unwrap();
    }
    crate::media_organize_history::interrupt_unfinished(&fixture.state.db)
        .await
        .unwrap();
    let statuses: BTreeMap<String, String> =
        sqlx::query_as("SELECT id,status FROM media_organize_history")
            .fetch_all(&fixture.state.db)
            .await
            .unwrap()
            .into_iter()
            .collect();
    fixture.close().await;
    assert_eq!(statuses["pending"], "processing");
    assert_eq!(statuses["running"], "processing");
    assert_eq!(statuses["failed"], "failed");
    assert_eq!(statuses["stopped"], "failed");
}
