use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use emby_toolkit::{AppState, auth::Session, database_admin, db};
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
