use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use emby_toolkit::{AppState, auth::Session, database_admin, db, jobs::JobSpec};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

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
        let schema = format!("database_restore_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
            .await
            .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |connection, _| {
                let query = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&query).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        sqlx::query("INSERT INTO emby_users(id,name,is_administrator) VALUES('restore-admin','Restore fixture',true)")
            .execute(&state.db).await.unwrap();
        state
            .save_config(&json!({
                "emby_server_url":"http://127.0.0.1:1", "emby_api_key":"fixture",
                "delay_between_items_sec":11
            }))
            .await
            .unwrap();
        state.config.write().await["emby_server_id"] = json!("fixture-server");
        Self {
            state,
            root,
            schema,
            _directory: directory,
        }
    }

    fn cookie(&self) -> String {
        let session = Session {
            user_id: "restore-admin".into(),
            name: "Restore fixture".into(),
            setup_mode: false,
            expires: chrono::Utc::now().timestamp() + 60,
        };
        format!(
            "etk_session={}",
            self.state.sessions.sign_administrator(&session)
        )
    }

    async fn request(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = emby_toolkit::router(self.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn restore(&self, backup: Value) -> Value {
        let boundary = "database-restore-fixture";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"tables\"\r\n\r\napp_settings\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"backup.json\"\r\nContent-Type: application/json\r\n\r\n{backup}\r\n--{boundary}--\r\n"
        );
        let (status, submitted) = self
            .request(
                Request::post("/api/database/import")
                    .header(header::COOKIE, self.cookie())
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{submitted}");
        self.finish(submitted["task_id"].as_str().unwrap()).await
    }

    async fn finish(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("database restore should terminate")
    }

    async fn config(&self) -> Value {
        let (status, body) = self
            .request(
                Request::get("/api/config")
                    .header(header::COOKIE, self.cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn cleanup(self) {
        self.state.jobs.shutdown().await;
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

fn settings_backup(value: Value) -> Value {
    json!({"data":{"app_settings":[{"setting_key":"dynamic_app_config","value_json":value}]}})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn committed_restore_resumes_sequence_correction_without_reimporting_rows() {
    let f = Fixture::new().await;
    let upstream_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = upstream_calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let new_server = format!("http://{}", listener.local_addr().unwrap());
    let fake = axum::Router::new().fallback(move || {
        let calls = counted.clone();
        async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            axum::Json(json!({"Id":"another-server"}))
        }
    });
    let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
    let restored_config =
        json!({"emby_server_url":new_server,"emby_api_key":"fixture","delay_between_items_sec":23});
    sqlx::query("CREATE TABLE restore_sequence_rows(id BIGSERIAL PRIMARY KEY,value TEXT NOT NULL)")
        .execute(&f.state.db)
        .await
        .unwrap();
    let backup = json!({"data":{"restore_sequence_rows":[{"id":42,"value":"restored"}],"app_settings":[{"setting_key":"dynamic_app_config","value_json":restored_config}]}});
    let names = vec![
        "restore_sequence_rows".to_string(),
        "app_settings".to_string(),
    ];
    let work = f.state.clone();
    let committed = Arc::new(tokio::sync::Notify::new());
    let observed = committed.clone();
    let id = f.state.jobs.submit_persistent("数据库恢复提交断点",
        JobSpec::new("database_admin",json!({"backup":backup,"names":names})),
        move|context|async move {
            // Reproduce a process stopping after the import transaction commits,
            // before the existing post-commit sequence correction runs.
            let mut tx = work.db.begin().await?;
            sqlx::query("TRUNCATE restore_sequence_rows,app_settings RESTART IDENTITY").execute(&mut *tx).await?;
            sqlx::query("INSERT INTO restore_sequence_rows(id,value) VALUES(42,'restored')").execute(&mut *tx).await?;
            sqlx::query("INSERT INTO app_settings(setting_key,value_json) VALUES('dynamic_app_config',$1)").bind(&restored_config).execute(&mut *tx).await?;
            context.complete_in_tx(&mut tx,"database-import",&json!({"tables":["restore_sequence_rows","app_settings"]})).await?;
            tx.commit().await?;
            observed.notify_one();
            context.cancelled().await;
            context.checkpoint().await?;
            Ok(json!({"unexpectedly_finished":true}))
        }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), committed.notified())
        .await
        .unwrap();
    sqlx::query("INSERT INTO restore_sequence_rows(id,value) VALUES(77,'user row after commit')")
        .execute(&f.state.db)
        .await
        .unwrap();
    let sequence_before: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM restore_sequence_rows_id_seq")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    assert_eq!(sequence_before, (1, false));
    f.state.jobs.shutdown().await;
    // The record deliberately carries the old implementation's verified server
    // binding. New submissions naturally omit it and cannot prove legacy recovery.
    sqlx::query("UPDATE durable_jobs SET spec_json=spec_json||$2::jsonb WHERE id=$1")
        .bind(&id)
        .bind(json!({"server_id":"legacy-verified-server","server_url":"http://127.0.0.1:1"}))
        .execute(&f.state.db)
        .await
        .unwrap();
    let legacy: Value = sqlx::query_scalar("SELECT spec_json FROM durable_jobs WHERE id=$1")
        .bind(&id)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(legacy["server_id"], "legacy-verified-server");
    let restarted = AppState::new(f.state.db.clone(), f.state.data_dir.clone())
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    restarted
        .jobs
        .resume_pending(restarted.clone(), cancel.clone())
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let result = restarted.jobs.result(&id).await.unwrap();
            if result["is_running"] == false && result["state"] != "interrupted" {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("committed restore must finish sequence correction after restart");
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,value FROM restore_sequence_rows ORDER BY id")
            .fetch_all(&restarted.db)
            .await
            .unwrap();
    let next_id: i64 =
        sqlx::query_scalar("INSERT INTO restore_sequence_rows(value) VALUES('next') RETURNING id")
            .fetch_one(&restarted.db)
            .await
            .unwrap();
    let runtime = restarted.config.read().await.clone();
    cancel.cancel();
    restarted.jobs.shutdown().await;
    server.abort();
    f.cleanup().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(
        rows,
        vec![
            (42, "restored".into()),
            (77, "user row after commit".into())
        ]
    );
    assert_eq!(
        next_id, 78,
        "resume must advance the sequence without truncating later rows"
    );
    assert_eq!(runtime["emby_server_url"], new_server);
    assert_eq!(runtime["delay_between_items_sec"], 23);
    assert_eq!(result["result"]["config_reloaded"], true);
    assert_eq!(
        upstream_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "local restore must neither read nor write an Emby server"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn importing_business_rows_preserves_recovery_queues_and_reserved_sequence_values() {
    let f = Fixture::new().await;
    sqlx::query("CREATE TABLE restore_business_rows(id BIGSERIAL PRIMARY KEY,value TEXT)")
        .execute(&f.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,operator_id,status) VALUES('active-queue','video-1','reprocess','{}','restore-admin','running')")
        .execute(&f.state.db).await.unwrap();
    let queue_before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(q) FROM media_organize_queue q WHERE id='active-queue'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    for sequence in [
        "media_organize_queue_queue_order_seq",
        "durable_jobs_job_order_seq",
    ] {
        for _ in 0..7 {
            sqlx::query("SELECT nextval($1::regclass)")
                .bind(sequence)
                .execute(&f.state.db)
                .await
                .unwrap();
        }
    }
    let queue_sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM media_organize_queue_queue_order_seq")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let job_sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM durable_jobs_job_order_seq")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let forbidden = database_admin::import_data(&f.state.db,&json!({"data":{"media_organize_queue":[{"id":"old-backup-queue","item_id":"old-video","mode":"reprocess","item_json":{},"operator_id":"restore-admin","status":"queued"}]}}),&["media_organize_queue".into()],None).await;
    assert!(
        forbidden.is_err(),
        "an old backup must not revive queue execution intents"
    );
    assert!(
        !database_admin::tables(&f.state.db)
            .await
            .unwrap()
            .contains(&"media_organize_queue".to_string())
    );
    database_admin::import_data(
        &f.state.db,
        &json!({"data":{"restore_business_rows":[{"id":42,"value":"restored"}]}}),
        &["restore_business_rows".into()],
        None,
    )
    .await
    .unwrap();
    let queue_after: Value = sqlx::query_scalar(
        "SELECT to_jsonb(q) FROM media_organize_queue q WHERE id='active-queue'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    let queue_after_sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM media_organize_queue_queue_order_seq")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let job_after_sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM durable_jobs_job_order_seq")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let business_next: i64 =
        sqlx::query_scalar("INSERT INTO restore_business_rows(value) VALUES('next') RETURNING id")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_organize_queue")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    f.cleanup().await;
    assert_eq!(queue_after, queue_before);
    assert_eq!(count, 1);
    assert_eq!(
        queue_after_sequence, queue_sequence,
        "import must not roll back reserved queue order values"
    );
    assert_eq!(
        job_after_sequence, job_sequence,
        "import must not roll back independent job order values"
    );
    assert_eq!(business_next, 43);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn legacy_bound_local_restore_and_terminal_cleanup_never_contact_emby() {
    for original_state in ["interrupted", "failed", "cancelled"] {
        let f = Fixture::new().await;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let mock = axum::Router::new().fallback(move || {
            let calls = observed.clone();
            async move {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                StatusCode::SERVICE_UNAVAILABLE
            }
        });
        let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        f.state
            .save_config(&json!({"emby_server_url":endpoint}))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE restore_offline_rows(id BIGSERIAL PRIMARY KEY,value TEXT)")
            .execute(&f.state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO restore_offline_rows(id,value) VALUES(9,'existing user data')")
            .execute(&f.state.db)
            .await
            .unwrap();
        let id = format!("legacy-local-{original_state}");
        let spec = json!({"kind":"database_admin","payload":{"backup":{"data":{"restore_offline_rows":[{"id":42,"value":"restored"}]}},"names":["restore_offline_rows"]},"server_id":"legacy-verified-server","server_url":endpoint,"operator_id":null,"owned":false});
        let state = json!({"task_id":id,"state":original_state,"is_running":false,"cleanup_pending":true,"pause_requested":false,"started_at":chrono::Utc::now().timestamp(),"last_action":"数据库恢复"});
        sqlx::query(
            "INSERT INTO durable_jobs(id,spec_json,state_json,checkpoint_json) VALUES($1,$2,$3,$4)",
        )
        .bind(&id)
        .bind(spec)
        .bind(state)
        .bind(json!({"plans":{},"completed":{}}))
        .execute(&f.state.db)
        .await
        .unwrap();
        f.state.jobs.shutdown().await;
        let restarted = AppState::new(f.state.db.clone(), f.state.data_dir.clone())
            .await
            .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        restarted
            .jobs
            .resume_pending(restarted.clone(), cancel.clone())
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let result = restarted.jobs.result(&id).await.unwrap();
                if result["is_running"] == false && result["state"] != "interrupted" {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("legacy local DB restore/cleanup must not wait for Emby");
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT id,value FROM restore_offline_rows ORDER BY id")
                .fetch_all(&restarted.db)
                .await
                .unwrap();
        cancel.cancel();
        restarted.jobs.shutdown().await;
        server.abort();
        f.cleanup().await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "local DB job must not use its legacy Emby binding"
        );
        if original_state == "interrupted" {
            assert_eq!(result["state"], "completed", "{result}");
            assert_eq!(rows, vec![(42, "restored".into())]);
        } else {
            assert_eq!(
                result["state"], original_state,
                "cleanup must preserve the terminal state"
            );
            assert_eq!(
                rows,
                vec![(9, "existing user data".into())],
                "terminal cleanup must not import data again"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn cancelling_the_last_restore_insert_rolls_back_before_commit() {
    let f = Fixture::new().await;
    sqlx::query("CREATE TABLE restore_gate_rows (id BIGSERIAL PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&f.state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO restore_gate_rows(value) VALUES('before')")
        .execute(&f.state.db)
        .await
        .unwrap();
    let key = ((uuid::Uuid::new_v4().as_u128() & 0x7fff_ffff) + 1) as i64;
    sqlx::query(&format!("CREATE FUNCTION wait_restore_insert() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}::bigint); RETURN NEW; END $$"))
        .execute(&f.state.db).await.unwrap();
    sqlx::query("CREATE TRIGGER restore_insert_gate BEFORE INSERT ON restore_gate_rows FOR EACH ROW EXECUTE FUNCTION wait_restore_insert()")
        .execute(&f.state.db).await.unwrap();
    let mut blocker = f.state.db.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let work = f.state.clone();
    let committed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = committed.clone();
    let id = f
        .state
        .jobs
        .submit(
            "Restore final insert cancellation",
            move |context| async move {
                database_admin::import_data(
                    &work.db,
                    &json!({"data":{"restore_gate_rows":[{"id":9,"value":"after"}]}}),
                    &["restore_gate_rows".into()],
                    Some(&context),
                )
                .await?;
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({"restored":true}))
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND objid::bigint=$1 AND NOT granted)")
                .bind(key).fetch_one(&f.state.db).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the final INSERT must really wait on PostgreSQL");
    f.state.jobs.control(Some(&id), "stop").await.unwrap();
    blocker.rollback().await.unwrap();
    let result = f.finish(&id).await;
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,value FROM restore_gate_rows ORDER BY id")
            .fetch_all(&f.state.db)
            .await
            .unwrap();
    let next_id: i64 =
        sqlx::query_scalar("INSERT INTO restore_gate_rows(value) VALUES('next') RETURNING id")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    f.cleanup().await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert!(
        !committed.load(std::sync::atomic::Ordering::SeqCst),
        "cancelled restore must not commit"
    );
    assert_eq!(rows, vec![(1, "before".into())]);
    assert_eq!(next_id, 2, "RESTART IDENTITY must also roll back");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn restoring_app_settings_updates_runtime_and_config_api() {
    let f = Fixture::new().await;
    let result = f
        .restore(settings_backup(json!({
            "emby_server_url":"http://127.0.0.1:1", "emby_api_key":"fixture",
            "delay_between_items_sec":22, "min_score_for_review":100
        })))
        .await;
    let stored = db::setting(&f.state.db, "dynamic_app_config")
        .await
        .unwrap()
        .unwrap();
    let runtime = f.state.config.read().await.clone();
    let api = f.config().await;
    f.cleanup().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["data_restored"], true);
    assert_eq!(result["result"]["restored_tables"], json!(["app_settings"]));
    assert_eq!(result["result"]["config_reloaded"], true);
    assert_eq!(result["result"]["warnings"], 0);
    assert_eq!(stored["delay_between_items_sec"], 22);
    assert_eq!(runtime["delay_between_items_sec"], 22);
    assert_eq!(api["delay_between_items_sec"], 22);
    assert!(runtime.get("min_score_for_review").is_none());
    assert_eq!(runtime["emby_server_id"], "fixture-server");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn clearing_app_settings_updates_runtime_even_if_a_later_table_fails() {
    let defaults: Value =
        serde_json::from_str(include_str!("../assets/config_defaults.json")).unwrap();
    for fails_after_clear in [false, true] {
        let f = Fixture::new().await;
        let names = if fails_after_clear {
            json!(["app_settings", "missing_table"])
        } else {
            json!(["app_settings"])
        };
        let (status, response) = f
            .request(
                Request::post("/api/actions/clear_tables")
                    .header(header::COOKIE, f.cookie())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"tables":names}).to_string()))
                    .unwrap(),
            )
            .await;
        let stored = db::setting(&f.state.db, "dynamic_app_config")
            .await
            .unwrap();
        let runtime = f.state.config.read().await.clone();
        let api = f.config().await;
        f.cleanup().await;
        assert_eq!(status.is_success(), !fails_after_clear);
        assert_eq!(response["cleared_tables"], json!(["app_settings"]));
        assert_eq!(response["config_reloaded"], true);
        assert!(stored.is_none());
        assert_eq!(
            runtime["delay_between_items_sec"],
            defaults["delay_between_items_sec"]
        );
        assert_eq!(
            api["delay_between_items_sec"],
            defaults["delay_between_items_sec"]
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn invalid_restore_keeps_database_and_runtime_settings() {
    let f = Fixture::new().await;
    let before = db::setting(&f.state.db, "dynamic_app_config")
        .await
        .unwrap();
    let result = f.restore(json!({"data":{"app_settings":[{
        "setting_key":"dynamic_app_config","value_json":{"delay_between_items_sec":22},"missing_column":"invalid"
    }]}})).await;
    let after = db::setting(&f.state.db, "dynamic_app_config")
        .await
        .unwrap();
    let runtime = f.state.config.read().await.clone();
    let api = f.config().await;
    f.cleanup().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(result["result"]["data_restored"].is_null());
    assert_eq!(after, before);
    assert_eq!(runtime["delay_between_items_sec"], 11);
    assert_eq!(api["delay_between_items_sec"], 11);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn concurrent_reload_cannot_replace_a_newer_saved_runtime_config() {
    let f = Fixture::new().await;
    let url = std::env::var("DATABASE_URL").unwrap();
    let search_path = format!("SET search_path TO {}", f.schema);
    let reload_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _| {
            let query = search_path.clone();
            Box::pin(async move {
                sqlx::query(&query).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&reload_pool)
        .await
        .unwrap();
    let hold_runtime = f.state.config.read().await;
    let save_state = f.state.clone();
    let save = tokio::spawn(async move {
        save_state
            .save_config(&json!({"delay_between_items_sec":33}))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if tokio::time::timeout(Duration::from_millis(20), f.state.config.read())
                .await
                .is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("save must be queued for the runtime write lock");
    let mut reload_state = f.state.clone();
    reload_state.db = reload_pool.clone();
    let reload = tokio::spawn(async move { reload_state.reload_config().await });
    // If reload can read before taking the config lock, observe that real
    // PostgreSQL read before releasing the queued save. A serialized reload
    // cannot reach PostgreSQL yet; the bounded wait also releases that path.
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let query: Option<String> = sqlx::query_scalar(
                "SELECT query FROM pg_stat_activity WHERE pid=$1 AND state='idle'",
            )
            .bind(pid)
            .fetch_optional(&f.root)
            .await
            .unwrap();
            if query.is_some_and(|query| query.contains("SELECT value_json FROM app_settings")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    drop(hold_runtime);
    save.await.unwrap().unwrap();
    reload.await.unwrap().unwrap();
    let stored = db::setting(&f.state.db, "dynamic_app_config")
        .await
        .unwrap()
        .unwrap();
    let runtime = f.state.config.read().await.clone();
    let api = f.config().await;
    reload_pool.close().await;
    f.cleanup().await;
    assert_eq!(stored["delay_between_items_sec"], 33);
    assert_eq!(runtime["delay_between_items_sec"], 33);
    assert_eq!(api["delay_between_items_sec"], 33);
}
