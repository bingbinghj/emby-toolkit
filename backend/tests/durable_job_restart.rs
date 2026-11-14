use emby_toolkit::{
    AppState,
    error::{ApiError, Result},
    jobs::{JobContext, JobManager, JobSpec},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

struct Fixture {
    admin: PgPool,
    pool: PgPool,
    state: AppState,
    schema: String,
    directory: tempfile::TempDir,
}
async fn pool(schema: &str) -> PgPool {
    let search = format!("SET search_path TO {schema}");
    sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap()
}
impl Fixture {
    async fn new() -> Self {
        let admin = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("job_restart_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let pool = pool(&schema).await;
        let directory = tempfile::Builder::new()
            .prefix("job-restart-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let state = AppState::new(pool.clone(), directory.path().join("app"))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE resume_effects(item TEXT PRIMARY KEY, writes INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        Self {
            admin,
            pool,
            state,
            schema,
            directory,
        }
    }
    async fn cold(&self) -> AppState {
        AppState::new(self.pool.clone(), self.directory.path().join("app"))
            .await
            .unwrap()
    }
    async fn finish(self) {
        self.state.jobs.shutdown().await;
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}
async fn ended(manager: &Arc<JobManager>, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let value = manager.result(id).await.unwrap();
            if value["is_running"] != true {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn effect(pool: &PgPool, item: &str) {
    sqlx::query("INSERT INTO resume_effects(item,writes) VALUES($1,1) ON CONFLICT(item) DO UPDATE SET writes=resume_effects.writes+1")
        .bind(item).execute(pool).await.unwrap();
}
async fn two_items(pool: PgPool, context: JobContext) -> Result<Value> {
    let plan = context.plan("files").await?.unwrap();
    assert_eq!(plan, json!(["first", "second"]));
    for (index, item) in ["first", "second"].iter().enumerate() {
        if context.completed(item).await?.is_none() {
            effect(&pool, item).await;
            context.complete(item, &json!({"item":item})).await?;
        }
        context
            .update_counted(index as u64 + 1, 2, "续原任务", item)
            .await;
    }
    Ok(json!({"updated":2}))
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn hard_kill_keeps_the_original_id_plan_and_completed_item() {
    if let Ok(schema) = std::env::var("ETK_JOB_CRASH_SCHEMA") {
        let pool = pool(&schema).await;
        let state = AppState::new(
            pool.clone(),
            Path::new(&std::env::var("ETK_JOB_CRASH_DIR").unwrap()).join("app"),
        )
        .await
        .unwrap();
        state
            .jobs
            .submit_owned(
                "断电任务",
                json!({"kind":"restart_test"}),
                None,
                move |context| async move {
                    context
                        .save_plan("files", &json!(["first", "second"]))
                        .await?;
                    effect(&pool, "first").await;
                    context.complete("first", &json!({"item":"first"})).await?;
                    context.update_counted(1, 2, "首项已验收", "等待断电").await;
                    std::future::pending::<Result<Value>>().await
                },
            )
            .await
            .unwrap();
        std::future::pending::<()>().await;
        return;
    }
    let f = Fixture::new().await;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "hard_kill_keeps_the_original_id_plan_and_completed_item",
            "--include-ignored",
            "--test-threads=1",
        ])
        .env("ETK_JOB_CRASH_SCHEMA", &f.schema)
        .env("ETK_JOB_CRASH_DIR", f.directory.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let id = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before checkpoint"
            );
            let row: Option<(String, Value)> = sqlx::query_as(
                "SELECT id,state_json FROM durable_jobs WHERE state_json->>'phase'='首项已验收'",
            )
            .fetch_optional(&f.pool)
            .await
            .unwrap();
            if let Some((id, state)) = row {
                assert_eq!(state["completed"], 1);
                return id;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    let work = f.pool.clone();
    let resumed = cold
        .jobs
        .submit_owned(
            "断电任务",
            json!({"kind":"restart_test"}),
            Some(&id),
            move |context| async move {
                assert!(context.is_resuming());
                two_items(work, context).await
            },
        )
        .await
        .unwrap();
    assert_eq!(resumed, id);
    let result = ended(&cold.jobs, &id).await;
    assert_eq!(result["state"], "completed");
    assert_eq!(result["completed"], 2);
    assert_eq!(result["result"]["updated"], 2);
    let rows: Vec<(String, i32)> =
        sqlx::query_as("SELECT item,writes FROM resume_effects ORDER BY item")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![("first".into(), 1), ("second".into(), 1)]);
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn transaction_completion_is_invisible_until_commit_and_does_not_lock_progress() {
    let f = Fixture::new().await;
    let work = f.pool.clone();
    let id = f
        .state
        .jobs
        .submit_persistent(
            "事务恢复",
            JobSpec::new("tasks", json!({"task":"clear-mediainfo"})),
            move |context| async move {
                let mut transaction = work.begin().await?;
                context
                    .complete_in_tx(&mut transaction, "rolled-back", &json!({"done":true}))
                    .await?;
                tokio::time::timeout(
                    Duration::from_secs(1),
                    context.update(20, "事务内进度", "不会自锁"),
                )
                .await
                .unwrap();
                assert!(context.completed("rolled-back").await?.is_none());
                transaction.rollback().await?;
                assert!(context.completed("rolled-back").await?.is_none());
                let mut transaction = work.begin().await?;
                context
                    .complete_in_tx(&mut transaction, "committed", &json!({"count":1}))
                    .await?;
                transaction.commit().await?;
                assert_eq!(
                    context.completed("committed").await?,
                    Some(json!({"count":1}))
                );
                Ok(json!({"updated":1}))
            },
        )
        .await
        .unwrap();
    assert_eq!(ended(&f.state.jobs, &id).await["state"], "completed");
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(cold.jobs.result(&id).await.unwrap()["state"], "completed");
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn user_stop_then_service_shutdown_remains_cancelled_after_restart() {
    let f = Fixture::new().await;
    let (ready, received) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let id = f
        .state
        .jobs
        .submit_persistent(
            "停止优先",
            JobSpec::new("tasks", json!({"task":"clear-mediainfo"})),
            move |context| async move {
                let _ = ready.send(());
                context.cancelled().await;
                let _ = released.await;
                context.checkpoint().await?;
                Ok(json!({"updated":0}))
            },
        )
        .await
        .unwrap();
    received.await.unwrap();
    f.state.jobs.control(Some(&id), "stop").await.unwrap();
    let manager = f.state.jobs.clone();
    let shutdown = tokio::spawn(async move { manager.shutdown().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.send(()).unwrap();
    shutdown.await.unwrap();
    assert_eq!(
        f.state.jobs.result(&id).await.unwrap()["state"],
        "cancelled"
    );
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(cold.jobs.result(&id).await.unwrap()["state"], "cancelled");
    assert_eq!(cold.jobs.status().await["is_running"], false);
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn accepted_user_stop_after_shutdown_still_wins_over_restart_recovery() {
    let f = Fixture::new().await;
    let (ready, received) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let id = f
        .state
        .jobs
        .submit_persistent(
            "停止优先反序",
            JobSpec::new("tasks", json!({"task":"clear-mediainfo"})),
            move |context| async move {
                let _ = ready.send(());
                context.cancelled().await;
                let _ = released.await;
                context.checkpoint().await?;
                Ok(json!({"updated":0}))
            },
        )
        .await
        .unwrap();
    received.await.unwrap();
    let manager = f.state.jobs.clone();
    let shutdown = tokio::spawn(async move { manager.shutdown().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.state.jobs.result(&id).await.unwrap()["state"] == "interrupted" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.state.jobs.control(Some(&id), "stop").await.unwrap();
    release.send(()).unwrap();
    shutdown.await.unwrap();
    assert_eq!(
        f.state.jobs.result(&id).await.unwrap()["state"],
        "cancelled"
    );
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(cold.jobs.result(&id).await.unwrap()["state"], "cancelled");
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn a_paused_task_retains_pause_before_any_business_execution() {
    let f = Fixture::new().await;
    let (ready, received) = oneshot::channel();
    let id = f
        .state
        .jobs
        .submit_owned(
            "保留暂停",
            json!({"kind":"restart_test"}),
            None,
            move |context| async move {
                context
                    .save_plan("files", &json!(["first", "second"]))
                    .await?;
                let _ = ready.send(());
                loop {
                    context.sleep(Duration::from_millis(20)).await?;
                }
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
        )
        .await
        .unwrap();
    received.await.unwrap();
    f.state.jobs.control(Some(&id), "pause").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.state.jobs.status().await["state"] != "paused" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.state.jobs.shutdown().await;
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    let executed = Arc::new(AtomicUsize::new(0));
    let count = executed.clone();
    let resumed = cold
        .jobs
        .submit_owned(
            "保留暂停",
            json!({"kind":"restart_test"}),
            Some(&id),
            move |context| async move {
                count.fetch_add(1, Ordering::SeqCst);
                assert!(context.is_resuming());
                Ok(json!({"updated":1}))
            },
        )
        .await
        .unwrap();
    assert_eq!(resumed, id);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(executed.load(Ordering::SeqCst), 0);
    assert_eq!(cold.jobs.status().await["state"], "paused");
    cold.jobs.control(Some(&id), "resume").await.unwrap();
    assert_eq!(ended(&cold.jobs, &id).await["state"], "completed");
    assert_eq!(executed.load(Ordering::SeqCst), 1);
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn terminal_write_failure_publishes_interrupted_and_recovers_durable_result_proof() {
    let f = Fixture::new().await;
    sqlx::raw_sql("CREATE FUNCTION reject_job_finish() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state_json->>'state'='completed' THEN RAISE EXCEPTION 'injected terminal write failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_job_finish BEFORE UPDATE ON durable_jobs FOR EACH ROW EXECUTE FUNCTION reject_job_finish();")
        .execute(&f.pool).await.unwrap();
    let work = f.pool.clone();
    let id = f
        .state
        .jobs
        .submit_persistent(
            "终态故障",
            JobSpec::new("tasks", json!({"task":"clear-mediainfo"})),
            move |context| async move {
                effect(&work, "once").await;
                context.complete("effect", &json!({"done":true})).await?;
                Ok(json!({"updated":1}))
            },
        )
        .await
        .unwrap();
    assert_eq!(ended(&f.state.jobs, &id).await["state"], "interrupted");
    let proof: Value = sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key='completed:__job_result__'")
        .bind(&id).fetch_one(&f.pool).await.unwrap();
    assert_eq!(proof["state"], "completed");
    sqlx::query("DROP TRIGGER reject_job_finish ON durable_jobs")
        .execute(&f.pool)
        .await
        .unwrap();
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(cold.jobs.result(&id).await.unwrap()["state"], "completed");
    let count: i32 = sqlx::query_scalar("SELECT writes FROM resume_effects WHERE item='once'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn generic_and_owned_recovery_runs_in_original_registration_order() {
    let f = Fixture::new().await;
    sqlx::raw_sql("CREATE TABLE execution_order(position BIGSERIAL,task_id TEXT); CREATE FUNCTION observe_job_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state_json->>'state'='running' AND OLD.state_json->>'state'<>'running' THEN INSERT INTO execution_order(task_id) VALUES(NEW.id); END IF; RETURN NEW; END $$; CREATE TRIGGER observe_job_start BEFORE UPDATE ON durable_jobs FOR EACH ROW EXECUTE FUNCTION observe_job_start();")
        .execute(&f.pool).await.unwrap();
    let ids = ["first-generic", "owned-middle", "last-generic"];
    for (index, id) in ids.iter().enumerate() {
        let spec = if index == 1 {
            json!({"kind":"media_organize_queue","payload":{"kind":"media_organize_queue","queue_id":"queue-middle"},"owned":true})
        } else {
            json!({"kind":"tasks","payload":{"task":"clear-mediainfo","full":false,"batch_size":10,"unsupported":[]}})
        };
        sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json) VALUES($1,$2,$3)")
            .bind(id).bind(spec).bind(json!({"task_id":id,"state":"interrupted","is_running":false,"last_action":id,"started_at":1,"pause_requested":false})).execute(&f.pool).await.unwrap();
    }
    sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,operator_id,task_id,status) VALUES('queue-middle','missing-item','rescrape',$1,'test-user','owned-middle','queued')")
        .bind(json!({"item_id":"missing-item","item_type":"Movie","name":"隔离缺失目标"})).execute(&f.pool).await.unwrap();
    let cold = f.cold().await;
    tokio::time::timeout(
        Duration::from_secs(15),
        cold.jobs
            .resume_pending(cold.clone(), CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    let order: Vec<String> =
        sqlx::query_scalar("SELECT task_id FROM execution_order ORDER BY position")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert_eq!(order, ids.map(str::to_string).to_vec());
    assert_eq!(
        cold.jobs.result("first-generic").await.unwrap()["state"],
        "completed"
    );
    assert_eq!(
        cold.jobs.result("owned-middle").await.unwrap()["state"],
        "failed"
    );
    assert_eq!(
        cold.jobs.result("last-generic").await.unwrap()["state"],
        "completed"
    );
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn an_owned_user_stop_is_reconciled_with_the_queue_before_restart_dispatch() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,operator_id,task_id,status) VALUES('cancelled-queue','missing-item','rescrape','{}','test-user','cancelled-job','running')")
        .execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json) VALUES('cancelled-job',$1,$2)")
        .bind(json!({"kind":"media_organize_queue","payload":{"kind":"media_organize_queue","queue_id":"cancelled-queue"},"owned":true}))
        .bind(json!({"task_id":"cancelled-job","state":"stopping","is_running":true,"last_action":"停止任务","stop_requested":true}))
        .execute(&f.pool).await.unwrap();
    let cold = f.cold().await;
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        cold.jobs.result("cancelled-job").await.unwrap()["state"],
        "cancelled"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM media_organize_queue WHERE id='cancelled-queue'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(status, "cancelled");
    cold.jobs.shutdown().await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn monitor_only_recovery_blocks_new_main_tasks_until_common_resource_preparation() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO durable_jobs(id,lane,spec_json,state_json) VALUES('monitor-startup','monitor',$1,$2)")
        .bind(json!({"kind":"restart_test","payload":{},"owned":true}))
        .bind(json!({"task_id":"monitor-startup","state":"paused","is_running":false,"pause_requested":true,"cleanup_pending":true,"last_action":"监控恢复"}))
        .execute(&f.pool).await.unwrap();
    let cold = f.cold().await;
    let rejected = cold
        .jobs
        .submit_persistent(
            "新任务",
            JobSpec::new("tasks", json!({"task":"clear-mediainfo"})),
            |_| async { Ok(json!({})) },
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.0, axum::http::StatusCode::CONFLICT);
    assert_eq!(cold.jobs.status().await["state"], "interrupted");
    cold.jobs
        .resume_pending(cold.clone(), CancellationToken::new())
        .await
        .unwrap();
    cold.jobs.wait_recovery_ready().await.unwrap();
    let id = cold
        .jobs
        .submit_persistent(
            "准备后新任务",
            JobSpec::new("tasks", json!({"task":"clear-mediainfo"})),
            |_| async { Ok(json!({"updated":0})) },
        )
        .await
        .unwrap();
    assert_eq!(ended(&cold.jobs, &id).await["state"], "completed");
    let monitor: Value =
        sqlx::query_scalar("SELECT state_json FROM durable_jobs WHERE id='monitor-startup'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(monitor["pause_requested"], true);
    assert_eq!(monitor["cleanup_pending"], false);
    cold.jobs.shutdown().await;
    f.finish().await;
}
