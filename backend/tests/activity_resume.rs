use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, activity, jobs::JobSpec};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const EMBY: &str = "emby-recorded";
const HELPER: &str = "helper-recorded";
const HELPER_NAME: &str = "etk-activity-clear-fixture";

#[derive(Clone)]
struct DockerHost {
    calls: Arc<Mutex<Vec<String>>>,
    running: Arc<Mutex<bool>>,
    helper: Arc<Mutex<Option<String>>>,
    helper_name: Arc<Mutex<String>>,
    result: Arc<Mutex<Value>>,
    database: PathBuf,
    pool: Arc<Mutex<Option<sqlx::PgPool>>>,
    at_stop: Arc<Mutex<Vec<Value>>>,
}
fn inspect(id: &str, running: bool, status: &str, image: &str) -> Value {
    json!({"Id":id,"Image":image,"State":{"Running":running,"Status":status,"ExitCode":0},
        "Config":{"Labels":{"etk.activity-job":HELPER_NAME}},"HostConfig":{"VolumesFrom":[EMBY]}})
}
fn cloud_result(result: Value) -> Value {
    let backup = std::path::Path::new(result["backup"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy();
    json!({"before":result["before"],"after":result["after"],"backup":format!("/config/data/{backup}")})
}
async fn docker_api(State(host): State<DockerHost>, request: Request<Body>) -> Response {
    let path = request.uri().path();
    let path = path
        .find("/containers")
        .map(|offset| &path[offset..])
        .unwrap_or(path)
        .to_string();
    let method = request.method().as_str().to_string();
    host.calls.lock().unwrap().push(format!("{method} {path}"));
    match (method.as_str(), path.as_str()) {
        ("GET", "/containers/json") => {
            Json(json!([{"Id":EMBY,"Image":"amilys/embyserver:fixture","State":"running"}]))
                .into_response()
        }
        ("GET", "/containers/emby-recorded/json") => Json(inspect(
            EMBY,
            *host.running.lock().unwrap(),
            "running",
            "emby-image",
        ))
        .into_response(),
        (
            "GET",
            "/containers/etk-activity-clear-fixture/json" | "/containers/helper-recorded/json",
        ) => {
            let helper = host.helper.lock().unwrap().clone();
            if let Some(status) = helper {
                let mut info = inspect(HELPER, status == "running", &status, "toolkit-image");
                info["Config"]["Labels"]["etk.activity-job"] =
                    json!(host.helper_name.lock().unwrap().clone());
                Json(info).into_response()
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"message":"No such container"})),
                )
                    .into_response()
            }
        }
        ("POST", "/containers/emby-recorded/stop") => {
            let pool = host.pool.lock().unwrap().clone().unwrap();
            let (id, mut checkpoint): (String, Value) = sqlx::query_as(
                "SELECT id,checkpoint_json FROM durable_jobs ORDER BY job_order DESC LIMIT 1",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let steps: Vec<(String, Value)> =
                sqlx::query_as("SELECT step_key,value_json FROM durable_job_steps WHERE job_id=$1")
                    .bind(id)
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            for (key, value) in steps {
                if let Some(key) = key.strip_prefix("plans:") {
                    checkpoint["plans"][key] = value;
                } else if let Some(key) = key.strip_prefix("completed:") {
                    checkpoint["completed"][key] = value;
                } else {
                    checkpoint["completed"][key] = value;
                }
            }
            host.at_stop.lock().unwrap().push(checkpoint);
            *host.running.lock().unwrap() = false;
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", "/containers/emby-recorded/start") => {
            *host.running.lock().unwrap() = true;
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", "/containers/helper-recorded/start") => {
            let result = cloud_result(activity::clear_database(&host.database).unwrap());
            *host.result.lock().unwrap() = result;
            *host.helper.lock().unwrap() = Some("exited".into());
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", "/containers/helper-recorded/wait") => {
            if host.helper.lock().unwrap().as_deref() == Some("running") {
                let result = cloud_result(activity::clear_database(&host.database).unwrap());
                *host.result.lock().unwrap() = result;
                *host.helper.lock().unwrap() = Some("exited".into());
            }
            Json(json!({"StatusCode":0})).into_response()
        }
        ("GET", "/containers/helper-recorded/logs") => {
            let bytes = format!("{}\n", host.result.lock().unwrap());
            let mut frame = vec![1, 0, 0, 0];
            frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            frame.extend_from_slice(bytes.as_bytes());
            (
                [("Content-Type", "application/vnd.docker.raw-stream")],
                frame,
            )
                .into_response()
        }
        ("DELETE", "/containers/helper-recorded") => {
            *host.helper.lock().unwrap() = None;
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", "/containers/create") => {
            let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["Image"], "toolkit-image");
            assert_eq!(body["HostConfig"]["VolumesFrom"], json!([EMBY]));
            assert_eq!(body["Entrypoint"], json!(["/app/emby-toolkit"]));
            assert_eq!(
                body["Cmd"],
                json!(["--clear-activity-log", "/config/data/activitylog.db"])
            );
            *host.helper_name.lock().unwrap() = body["Labels"]["etk.activity-job"]
                .as_str()
                .unwrap()
                .to_string();
            *host.helper.lock().unwrap() = Some("created".into());
            (
                StatusCode::CREATED,
                Json(json!({"Id":HELPER,"Warnings":[]})),
            )
                .into_response()
        }
        ("GET", path)
            if path == format!("/containers/{}/json", host.helper_name.lock().unwrap()) =>
        {
            let status = host.helper.lock().unwrap().clone().unwrap();
            let mut info = inspect(HELPER, status == "running", &status, "toolkit-image");
            info["Config"]["Labels"]["etk.activity-job"] =
                json!(host.helper_name.lock().unwrap().clone());
            Json(info).into_response()
        }
        ("GET", path)
            if path.starts_with("/containers/etk-activity-clear-") && path.ends_with("/json") =>
        {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"message":"No such container"})),
            )
                .into_response()
        }
        ("GET", path) if path.starts_with("/containers/") && path.ends_with("/json") => Json(
            inspect("toolkit-recorded", true, "running", "toolkit-image"),
        )
        .into_response(),
        ("GET", "/System/Info/Public") => Json(json!({"Id":"fixture"})).into_response(),
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"message":format!("unexpected fixture request {method} {path}")})),
        )
            .into_response(),
    }
}
struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    host: DockerHost,
    base: String,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(helper: Option<&str>, running: bool, already_cleared: bool) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("activity_resume_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
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
        let database = directory.path().join("activitylog.db");
        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection.execute_batch("CREATE TABLE ActivityLog(id INTEGER PRIMARY KEY,message TEXT);INSERT INTO ActivityLog(message) VALUES('old-a'),('old-b');").unwrap();
        }
        let result = if already_cleared {
            let result = cloud_result(activity::clear_database(&database).unwrap());
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO ActivityLog(message) VALUES('new-a'),('new-b'),('new-c');",
                )
                .unwrap();
            result
        } else {
            Value::Null
        };
        let host = DockerHost {
            calls: Arc::default(),
            running: Arc::new(Mutex::new(running)),
            helper: Arc::new(Mutex::new(helper.map(str::to_string))),
            helper_name: Arc::new(Mutex::new(HELPER_NAME.to_string())),
            result: Arc::new(Mutex::new(result)),
            database,
            pool: Arc::default(),
            at_stop: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(docker_api).with_state(host.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        state
            .save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture"}))
            .await
            .unwrap();
        *host.pool.lock().unwrap() = Some(state.db.clone());
        state.docker = Arc::new(emby_toolkit::docker_host::Service {
            docker: Some(
                bollard::Docker::connect_with_http(&base, 10, bollard::API_DEFAULT_VERSION)
                    .unwrap(),
            ),
        });
        Self {
            state,
            admin,
            schema,
            directory,
            host,
            base,
            server,
        }
    }
    async fn begin_resume(
        &mut self,
        cleared: bool,
        helper_created: bool,
        paused: bool,
        stopping: bool,
    ) -> tokio::task::JoinHandle<emby_toolkit::error::Result<()>> {
        let id = "original-activity-task";
        let plan = json!({"container_id":EMBY,"container_image":"emby-image","original_running":true,
            "toolkit_image":"toolkit-image","helper_name":HELPER_NAME});
        let mut completed = json!({});
        if cleared {
            completed["activity:cleared"] = self.host.result.lock().unwrap().clone();
        }
        if helper_created {
            completed["activity:helper-created"] = json!(HELPER);
        }
        let spec = JobSpec::new(
            "tasks",
            json!({"task":"clear-emby-activity-log","full":false,"batch_size":10,"unsupported":[]}),
        );
        sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json,checkpoint_json) VALUES($1,$2,$3,$4)")
            .bind(id).bind(serde_json::to_value(spec).unwrap())
            .bind(json!({"task_id":id,"state":if stopping {"stopping"} else if paused {"paused"} else {"running"},"stop_requested":stopping,"cleanup_pending":true,"pause_requested":paused,"is_running":true,"progress":30,"progress_known":true,"last_action":"清理活动日志","started_at":1}))
            .bind(json!({"plans":{"activity:plan":plan},"completed":completed}))
            .execute(&self.state.db).await.unwrap();
        // Reconstruct the whole state so recovery has no old JobContext or
        // in-memory plan. Docker and Emby are both this localhost substitute.
        self.reopen().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let state = self.state.clone();
        let jobs = state.jobs.clone();
        tokio::spawn(async move { jobs.resume_pending(state, cancel).await })
    }
    async fn reopen(&mut self) {
        self.state = AppState::new(self.state.db.clone(), self.directory.path().join("app"))
            .await
            .unwrap();
        self.state.docker = Arc::new(emby_toolkit::docker_host::Service {
            docker: Some(
                bollard::Docker::connect_with_http(&self.base, 10, bollard::API_DEFAULT_VERSION)
                    .unwrap(),
            ),
        });
    }
    async fn resume(&mut self, cleared: bool, helper_created: bool) -> Value {
        let runner = self
            .begin_resume(cleared, helper_created, false, false)
            .await;
        tokio::time::timeout(Duration::from_secs(30), runner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.state
            .jobs
            .result("original-activity-task")
            .await
            .unwrap()
    }
    fn count(&self, request: &str) -> usize {
        self.host
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == request)
            .count()
    }
    fn rows(&self) -> i64 {
        rusqlite::Connection::open(&self.host.database)
            .unwrap()
            .query_row("SELECT count(*) FROM ActivityLog", [], |row| row.get(0))
            .unwrap()
    }
    async fn cleanup(self) {
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[test]
fn activity_helper_backs_up_sqlite_before_clearing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("activitylog.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE ActivityLog(id INTEGER);INSERT INTO ActivityLog VALUES(1),(2);",
        )
        .unwrap();
    drop(connection);
    let result = activity::clear_database(&path).unwrap();
    assert_eq!(result["before"], 2);
    assert_eq!(result["after"], 0);
    let backup = rusqlite::Connection::open(result["backup"].as_str().unwrap()).unwrap();
    assert_eq!(
        backup
            .query_row("SELECT count(*) FROM ActivityLog", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn first_clear_saves_original_container_and_helper_before_stopping_emby() {
    let fixture = Fixture::new(None, true, false).await;
    let work = fixture.state.clone();
    let spec = JobSpec::new(
        "tasks",
        json!({"task":"clear-emby-activity-log","full":false,"batch_size":10,"unsupported":[]}),
    );
    let id = fixture
        .state
        .jobs
        .submit_persistent("清理活动日志", spec, move |context| async move {
            activity::clear(&work, &context).await
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let result = fixture.state.jobs.result(&id).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(fixture.rows(), 0);
    assert_eq!(fixture.count("POST /containers/create"), 1);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 1);
    let checkpoints = fixture.host.at_stop.lock().unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(
        checkpoints[0]["plans"]["activity:plan"]["container_id"],
        EMBY
    );
    assert_eq!(
        checkpoints[0]["plans"]["activity:plan"]["original_running"],
        true
    );
    assert_eq!(
        checkpoints[0]["completed"]["activity:helper-created"],
        HELPER
    );
    drop(checkpoints);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn restart_reads_finished_helper_without_clearing_new_logs_or_restarting_emby() {
    let mut fixture = Fixture::new(Some("exited"), true, true).await;
    let result = fixture.resume(false, true).await;
    assert_eq!(result["task_id"], "original-activity-task");
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["deleted"], 2);
    assert_eq!(fixture.rows(), 3);
    assert_eq!(fixture.count("POST /containers/emby-recorded/stop"), 0);
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 0);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 0);
    assert_eq!(fixture.count("GET /containers/json"), 0);
    assert_eq!(fixture.count("GET /containers/helper-recorded/logs"), 1);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn restart_after_cleared_checkpoint_only_restores_recorded_emby() {
    let mut fixture = Fixture::new(None, false, true).await;
    let result = fixture.resume(true, true).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(fixture.rows(), 3);
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/emby-recorded/stop"), 0);
    assert_eq!(fixture.count("GET /containers/helper-recorded/logs"), 0);
    assert_eq!(fixture.count("POST /containers/create"), 0);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn restart_continues_created_helper_once_and_preserves_sqlite_backup() {
    let mut fixture = Fixture::new(Some("created"), true, false).await;
    let result = fixture.resume(false, true).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["deleted"], 2);
    assert_eq!(fixture.rows(), 0);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/emby-recorded/stop"), 1);
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/create"), 0);
    let backup_name = std::path::Path::new(result["result"]["backup"].as_str().unwrap())
        .file_name()
        .unwrap();
    let backup = rusqlite::Connection::open(fixture.directory.path().join(backup_name)).unwrap();
    assert_eq!(
        backup
            .query_row("SELECT count(*) FROM ActivityLog", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(backup);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn lost_unverified_helper_restores_emby_and_refuses_another_clear() {
    let mut fixture = Fixture::new(None, false, false).await;
    let result = fixture.resume(false, true).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(result["message"].as_str().unwrap().contains("拒绝重复清空"));
    assert_eq!(fixture.rows(), 2);
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/create"), 0);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn failed_checkpoint_waits_for_existing_helper_before_restoring_emby() {
    let mut fixture = Fixture::new(Some("running"), false, false).await;
    sqlx::raw_sql("CREATE FUNCTION refuse_helper_checkpoint() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.step_key='completed:activity:helper-created' THEN RAISE EXCEPTION 'fixture checkpoint unavailable'; END IF; RETURN NEW; END $$; CREATE TRIGGER refuse_helper_checkpoint BEFORE INSERT OR UPDATE ON durable_job_steps FOR EACH ROW EXECUTE FUNCTION refuse_helper_checkpoint();")
        .execute(&fixture.state.db).await.unwrap();
    let result = fixture.resume(false, false).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(fixture.rows(), 0);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 0);
    assert_eq!(fixture.count("POST /containers/helper-recorded/wait"), 1);
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    let calls = fixture.host.calls.lock().unwrap();
    let waited = calls
        .iter()
        .position(|call| call == "POST /containers/helper-recorded/wait")
        .unwrap();
    let restarted = calls
        .iter()
        .position(|call| call == "POST /containers/emby-recorded/start")
        .unwrap();
    assert!(waited < restarted);
    drop(calls);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn recovering_paused_clear_restores_emby_before_waiting_for_operator() {
    let mut fixture = Fixture::new(Some("created"), false, false).await;
    let runner = fixture.begin_resume(false, true, true, false).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = fixture.state.jobs.status().await;
            if status["state"] == "paused"
                && status["resumed_at"].is_number()
                && *fixture.host.running.lock().unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(*fixture.host.running.lock().unwrap());
    assert_eq!(fixture.rows(), 2);
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 0);
    fixture
        .state
        .jobs
        .control(Some("original-activity-task"), "resume")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .state
            .jobs
            .result("original-activity-task")
            .await
            .unwrap()["state"],
        "completed"
    );
    assert_eq!(fixture.rows(), 0);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 1);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Docker and Emby are localhost substitutes"]
async fn stopped_task_only_restores_resources_and_never_replays_clearing() {
    let mut fixture = Fixture::new(Some("created"), false, false).await;
    let runner = fixture.begin_resume(false, true, false, true).await;
    tokio::time::timeout(Duration::from_secs(30), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let result = fixture
        .state
        .jobs
        .result("original-activity-task")
        .await
        .unwrap();
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(fixture.rows(), 2);
    assert!(*fixture.host.running.lock().unwrap());
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 0);
    // A crash after module cleanup but before the manager clears its flag must
    // not override a subsequent intentional stop of the media server.
    sqlx::query("UPDATE durable_jobs SET state_json=jsonb_set(state_json,'{cleanup_pending}','true'::jsonb) WHERE id='original-activity-task'")
        .execute(&fixture.state.db).await.unwrap();
    *fixture.host.running.lock().unwrap() = false;
    fixture.reopen().await;
    fixture
        .state
        .jobs
        .resume_pending(
            fixture.state.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .state
            .jobs
            .result("original-activity-task")
            .await
            .unwrap()["state"],
        "cancelled"
    );
    assert!(!*fixture.host.running.lock().unwrap());
    assert_eq!(fixture.count("POST /containers/emby-recorded/start"), 1);
    assert_eq!(fixture.count("POST /containers/helper-recorded/start"), 0);
    assert_eq!(fixture.rows(), 2);
    let pending: bool = sqlx::query_scalar("SELECT (state_json->>'cleanup_pending')::boolean FROM durable_jobs WHERE id='original-activity-task'")
        .fetch_one(&fixture.state.db).await.unwrap();
    assert!(!pending);
    fixture.cleanup().await;
}
