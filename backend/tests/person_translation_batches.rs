use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, emby, tasks};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
struct Host {
    people: Arc<Mutex<BTreeMap<String, Value>>>,
    empty_people: bool,
    mode: usize,
    ai_calls: Arc<AtomicUsize>,
    ai_inputs: Arc<Mutex<Vec<Vec<String>>>>,
    second_after_writes: Arc<AtomicUsize>,
    fallback_starts: Arc<Mutex<Vec<usize>>>,
    writes: Arc<Mutex<Vec<String>>>,
    first_batch_written: Arc<Notify>,
    release_write: Arc<Notify>,
}
async fn items(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let people = host.people.lock().await;
    let items = if query
        .get("IncludeItemTypes")
        .is_some_and(|kind| kind == "Person")
    {
        let start = query["StartIndex"].parse::<usize>().unwrap();
        assert_eq!(query["Limit"], "500");
        assert_eq!(query["Fields"], "ProviderIds,Name");
        host.fallback_starts.lock().await.push(start);
        let items = people
            .values()
            .skip(start)
            .take(500)
            .cloned()
            .collect::<Vec<_>>();
        return Json(json!({"Items":items,"TotalRecordCount":people.len()}));
    } else if let Some(ids) = query.get("Ids") {
        ids.split(',')
            .filter_map(|id| people.get(id).cloned())
            .collect::<Vec<_>>()
    } else {
        let associations = if host.empty_people {
            vec![]
        } else {
            people
                .values()
                .map(|person| json!({"Id":person["Id"],"Name":person["Name"]}))
                .collect::<Vec<_>>()
        };
        vec![json!({"Id":"movie","Type":"Movie","Path":"/fixture/movie.mkv","People":associations})]
    };
    Json(json!({"TotalRecordCount":items.len(),"Items":items}))
}
async fn person(State(host): State<Host>, Path(id): Path<String>) -> Json<Value> {
    Json(host.people.lock().await[&id].clone())
}
async fn save_person(
    State(host): State<Host>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    assert!(body["Name"].as_str().unwrap().starts_with("译名"));
    *host.people.lock().await.get_mut(&id).unwrap() = body;
    let count = {
        let mut writes = host.writes.lock().await;
        writes.push(id);
        writes.len()
    };
    if (host.mode == 2 && count == 50) || (host.mode == 4 && count == 1) {
        host.first_batch_written.notify_one();
        host.release_write.notified().await;
    }
    StatusCode::NO_CONTENT
}
async fn translate(State(host): State<Host>, Json(body): Json<Value>) -> Response {
    let call = host.ai_calls.fetch_add(1, Ordering::SeqCst) + 1;
    if call == 2 {
        host.second_after_writes
            .store(host.writes.lock().await.len(), Ordering::SeqCst);
        if host.mode == 1 {
            return StatusCode::BAD_GATEWAY.into_response();
        }
        if host.mode == 3 {
            host.first_batch_written.notify_one();
            host.release_write.notified().await;
        }
    }
    let input: Vec<String> =
        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    host.ai_inputs.lock().await.push(input.clone());
    let output = input
        .into_iter()
        .filter(|name| !(host.mode == 4 && call == 1 && name == "Actor000"))
        .map(|name| (name.clone(), format!("译名{name}")))
        .collect::<BTreeMap<_, _>>();
    Json(json!({"choices":[{"message":{"content":json!(output).to_string()}}]})).into_response()
}
struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    host: Host,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}
impl Fixture {
    async fn new(count: usize, empty_people: bool, mode: usize) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("person_batches_{}", uuid::Uuid::new_v4().simple());
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
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        let people = (0..count).map(|index| {
            let id = format!("p{index:03}");
            (id.clone(), json!({"Id":id,"Type":"Person","Name":format!("Actor{index:03}"),"ProviderIds":{}}))
        }).collect();
        let host = Host {
            people: Arc::new(Mutex::new(people)),
            empty_people,
            mode,
            ai_calls: Arc::default(),
            ai_inputs: Arc::default(),
            second_after_writes: Arc::default(),
            fallback_starts: Arc::default(),
            writes: Arc::default(),
            first_batch_written: Arc::default(),
            release_write: Arc::default(),
        };
        let router = Router::new()
            .route(
                "/Users/user/Views",
                get(|| async { Json(json!({"Items":[{"Id":"library"},{"Id":"ignored"}]})) }),
            )
            .route("/Items", get(items))
            .route("/Users/user/Items/{id}", get(person))
            .route("/Items/{id}", post(save_person))
            .route("/v1/chat/completions", post(translate))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.save_config(&json!({
            "emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user",
            "ai_base_url":format!("{base}/v1"),"ai_api_key":"fixture","ai_model_name":"fixture",
            "ai_provider":"openai","ai_translate_actor_role":true,"ai_translation_batch_size":50,
            "ai_max_retries":0,"ai_name_role_request_interval":0,"libraries_to_ignore":[]
        })).await.unwrap();
        Self {
            state,
            admin,
            schema,
            host,
            server,
            _directory: directory,
        }
    }
    async fn start_translation(&self) -> String {
        tasks::submit_task(&self.state, "actor-translation", false)
            .await
            .unwrap()["task_id"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn wait_job(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn scan(&self) -> Value {
        let state = self.state.clone();
        let id = self
            .state
            .jobs
            .submit("人物扫描兼容回退", move |context| async move {
                Ok(json!({"persons":emby::persons(&state,&context).await?}))
            })
            .await
            .unwrap();
        self.wait_job(&id).await
    }
    async fn restart(&mut self) {
        self.state = AppState::new(self.state.db.clone(), self.state.data_dir.clone())
            .await
            .unwrap();
        self.state
            .jobs
            .clone()
            .resume_pending(
                self.state.clone(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    async fn interrupt_second_batch(&self, task: &str, stop: bool) {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.host.first_batch_written.notified(),
        )
        .await
        .unwrap();
        assert_eq!(self.host.writes.lock().await.len(), 50);
        if stop {
            self.state.jobs.control(Some(task), "stop").await.unwrap();
            self.host.release_write.notify_one();
            return;
        }
        let manager = self.state.jobs.clone();
        let shutdown = tokio::spawn(async move { manager.shutdown().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.state.jobs.status().await["state"] == "interrupted" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        self.host.release_write.notify_one();
        shutdown.await.unwrap();
    }
    async fn close(self) {
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn later_translation_failure_keeps_first_batch_written_before_the_next_ai_request() {
    let f = Fixture::new(60, false, 1).await;
    let task = f.start_translation().await;
    let result = f.wait_job(&task).await;
    let second_after = f.host.second_after_writes.load(Ordering::SeqCst);
    let writes = f.host.writes.lock().await.clone();
    let names = f
        .host
        .people
        .lock()
        .await
        .values()
        .map(|person| person["Name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    f.close().await;
    assert_eq!(
        second_after, 50,
        "later AI requests must follow the first batch's Emby writes"
    );
    assert_eq!(result["result"]["updated"], 50, "{result}");
    assert!(
        result["result"]["warnings"].as_u64().unwrap() > 0,
        "{result}"
    );
    assert_eq!(writes.len(), 50);
    assert!(names[..50].iter().all(|name| name.starts_with("译名")));
    assert!(names[50..].iter().all(|name| name.starts_with("Actor")));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn stopping_after_first_batch_preserves_writes_without_requesting_later_translations() {
    let f = Fixture::new(60, false, 2).await;
    let task = f.start_translation().await;
    tokio::time::timeout(
        Duration::from_secs(15),
        f.host.first_batch_written.notified(),
    )
    .await
    .unwrap();
    f.state.jobs.control(Some(&task), "stop").await.unwrap();
    f.host.release_write.notify_one();
    let result = f.wait_job(&task).await;
    let calls = f.host.ai_calls.load(Ordering::SeqCst);
    let writes = f.host.writes.lock().await.len();
    f.close().await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(
        calls, 1,
        "stop must prevent the next outer translation batch"
    );
    assert_eq!(writes, 50);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn restarting_translation_keeps_its_id_and_completed_people_without_duplicate_writes() {
    let mut f = Fixture::new(60, false, 3).await;
    let task = f.start_translation().await;
    f.interrupt_second_batch(&task, false).await;
    let interrupted = f.wait_job(&task).await;
    assert_eq!(interrupted["state"], "interrupted", "{interrupted}");
    assert_eq!(interrupted["completed"], 50);
    assert_eq!(interrupted["stop_requested"], false);
    f.restart().await;
    let result = f.wait_job(&task).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["task_id"], task);
    assert_eq!(result["result"]["updated"], 60);
    let writes = f.host.writes.lock().await.clone();
    assert_eq!(writes.len(), 60);
    assert_eq!(
        writes
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        60
    );
    assert_eq!(f.host.ai_calls.load(Ordering::SeqCst), 2);
    f.state.jobs.shutdown().await;
    f.restart().await;
    assert_eq!(f.wait_job(&task).await["state"], "completed");
    assert_eq!(f.host.writes.lock().await.len(), 60);
    assert_eq!(f.host.ai_calls.load(Ordering::SeqCst), 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn restarting_partial_translation_requests_only_missing_valid_names() {
    let mut f = Fixture::new(3, false, 4).await;
    let task = f.start_translation().await;
    tokio::time::timeout(
        Duration::from_secs(15),
        f.host.first_batch_written.notified(),
    )
    .await
    .unwrap();
    let completed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_job_steps WHERE job_id=$1 AND step_key IN ('completed:tasks:actor-translation:person:p000','completed:tasks:actor-translation:batch:0'))")
        .bind(&task).fetch_one(&f.state.db).await.unwrap();
    assert!(
        !completed,
        "a missing translation must not complete its person or batch"
    );
    let manager = f.state.jobs.clone();
    let shutdown = tokio::spawn(async move { manager.shutdown().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.jobs.status().await["state"] != "interrupted" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.host.release_write.notify_one();
    shutdown.await.unwrap();
    f.restart().await;
    let result = f.wait_job(&task).await;
    assert!(
        matches!(
            result["state"].as_str(),
            Some("completed" | "completed_with_warnings")
        ),
        "{result}"
    );
    assert_eq!(
        f.host.ai_inputs.lock().await.clone(),
        vec![vec!["Actor000", "Actor001", "Actor002"], vec!["Actor000"]]
    );
    assert!(
        f.host
            .people
            .lock()
            .await
            .values()
            .all(|person| person["Name"].as_str().unwrap().starts_with("译名"))
    );
    let writes = f.host.writes.lock().await.clone();
    assert_eq!(writes.len(), 3);
    assert_eq!(
        writes
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn restarting_a_stopped_translation_never_revives_its_remaining_people() {
    let mut f = Fixture::new(60, false, 3).await;
    let task = f.start_translation().await;
    f.interrupt_second_batch(&task, true).await;
    assert_eq!(f.wait_job(&task).await["state"], "cancelled");
    f.state.jobs.shutdown().await;
    f.restart().await;
    assert_eq!(f.wait_job(&task).await["state"], "cancelled");
    assert_eq!(f.host.writes.lock().await.len(), 50);
    assert_eq!(f.host.ai_calls.load(Ordering::SeqCst), 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn restarting_a_paused_translation_keeps_pause_even_if_translation_was_disabled() {
    let mut f = Fixture::new(60, false, 3).await;
    let task = f.start_translation().await;
    tokio::time::timeout(
        Duration::from_secs(15),
        f.host.first_batch_written.notified(),
    )
    .await
    .unwrap();
    f.state.jobs.control(Some(&task), "pause").await.unwrap();
    f.host.release_write.notify_one();
    tokio::time::timeout(Duration::from_secs(15), async {
        while f.state.jobs.status().await["state"] != "paused" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.state
        .save_config(&json!({"ai_translate_actor_role":false}))
        .await
        .unwrap();
    f.state.jobs.shutdown().await;
    assert_eq!(f.wait_job(&task).await["pause_requested"], true);
    f.state = AppState::new(f.state.db.clone(), f.state.data_dir.clone())
        .await
        .unwrap();
    let work = f.state.clone();
    let manager = work.jobs.clone();
    let recovery = tokio::spawn(async move {
        manager
            .resume_pending(work, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.jobs.status().await["state"] != "paused" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let status = f.state.jobs.status().await;
    assert_eq!(status["state"], "paused", "{status}");
    assert_eq!(status["task_id"], task);
    assert_eq!(f.host.writes.lock().await.len(), 50);
    f.state.jobs.control(Some(&task), "resume").await.unwrap();
    recovery.await.unwrap();
    assert_eq!(f.wait_job(&task).await["state"], "completed");
    assert_eq!(f.host.writes.lock().await.len(), 50);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all media endpoints are localhost"]
async fn restarting_after_a_committed_cache_clear_keeps_new_cache_rows_and_original_result() {
    let mut f = Fixture::new(0, false, 0).await;
    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
        .bind("A".repeat(40))
        .bind(json!({"fixture":"before"}))
        .execute(&f.state.db)
        .await
        .unwrap();
    let task = tasks::submit_task(&f.state, "clear-mediainfo", false)
        .await
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(f.wait_job(&task).await["result"]["deleted"], 1);
    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2)")
        .bind("B".repeat(40))
        .bind(json!({"fixture":"after"}))
        .execute(&f.state.db)
        .await
        .unwrap();
    // Replay the crash window after the clear transaction commits, before the
    // job's terminal state is persisted. The committed step must win.
    sqlx::query(
        "UPDATE durable_jobs SET state_json=(state_json-'result'-'finished_at') || $2 WHERE id=$1",
    )
    .bind(&task)
    .bind(json!({"state":"running","is_running":true,"stop_requested":false}))
    .execute(&f.state.db)
    .await
    .unwrap();
    f.state.jobs.shutdown().await;
    f.restart().await;
    let result = f.wait_job(&task).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["deleted"], 1);
    let remaining: Vec<String> = sqlx::query_scalar("SELECT sha1 FROM p115_mediainfo_cache")
        .fetch_all(&f.state.db)
        .await
        .unwrap();
    assert_eq!(remaining, vec!["B".repeat(40)]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all media endpoints are localhost"]
async fn resumed_full_role_translation_does_not_clear_logs_written_after_its_reset() {
    let mut f = Fixture::new(0, false, 0).await;
    sqlx::query("INSERT INTO processed_log(item_id) VALUES('before-reset')")
        .execute(&f.state.db)
        .await
        .unwrap();
    let work = f.state.clone();
    let ready = Arc::new(Notify::new());
    let reached = ready.clone();
    let spec = emby_toolkit::jobs::JobSpec::new(
        "tasks",
        json!({"task":"role-translation","full":true,"batch_size":10,"unsupported":[]}),
    );
    // Arrange the persisted boundary immediately after a full run resets logs,
    // then restore through the real task dispatcher rather than this fixture.
    let task = f
        .state
        .jobs
        .submit_persistent("角色中文化", spec, move |context| async move {
            context
                .save_plan(
                    "tasks:role-translation:plan",
                    &json!({"media":[],"ignored_ids":[]}),
                )
                .await?;
            let mut transaction = work.db.begin().await?;
            sqlx::query("DELETE FROM processed_log")
                .execute(&mut *transaction)
                .await?;
            context
                .complete_in_tx(
                    &mut transaction,
                    "tasks:role-translation:reset",
                    &json!({"done":true}),
                )
                .await?;
            transaction.commit().await?;
            reached.notify_one();
            context.cancelled().await;
            context.checkpoint().await?;
            Ok(json!({}))
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), ready.notified())
        .await
        .unwrap();
    sqlx::query("INSERT INTO processed_log(item_id) VALUES('after-reset')")
        .execute(&f.state.db)
        .await
        .unwrap();
    f.state.jobs.shutdown().await;
    assert_eq!(f.wait_job(&task).await["state"], "interrupted");
    f.restart().await;
    assert_eq!(f.wait_job(&task).await["state"], "completed");
    let ids: Vec<String> = sqlx::query_scalar("SELECT item_id FROM processed_log")
        .fetch_all(&f.state.db)
        .await
        .unwrap();
    assert_eq!(ids, vec!["after-reset"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all media endpoints are localhost"]
async fn role_translation_keeps_each_movies_webhook_checkpoint_scope_separate() {
    let mut f = Fixture::new(0, false, 0).await;
    let items = [
        json!({"Id":"m1","Type":"Movie","Name":"电影一","Path":"/fixture/one.mkv","ProviderIds":{"Tmdb":"1"},"MediaSources":[]}),
        json!({"Id":"m2","Type":"Movie","Name":"电影二","Path":"/fixture/two.mkv","ProviderIds":{"Tmdb":"2"},"MediaSources":[]}),
    ];
    for item in &items {
        f.host
            .people
            .lock()
            .await
            .insert(item["Id"].as_str().unwrap().into(), item.clone());
    }
    let ready = Arc::new(Notify::new());
    let reached = ready.clone();
    let spec = emby_toolkit::jobs::JobSpec::new(
        "tasks",
        json!({"task":"role-translation","full":false,"batch_size":10,"unsupported":[]}),
    );
    let task = f
        .state
        .jobs
        .submit_persistent("角色中文化", spec, move |context| async move {
            context
                .save_plan(
                    "tasks:role-translation:plan",
                    &json!({"media":items,"ignored_ids":[]}),
                )
                .await?;
            context
                .complete(
                    "tasks:role-translation:cleanup",
                    &json!({"warnings":0,"warning_details":[]}),
                )
                .await?;
            let phases = [
                ("emby_feedback", json!({"updated":1,"warnings":0})),
                (
                    "emby_finalized",
                    json!({"title_fill":{"state":"disabled","pending":[]},"warnings":0}),
                ),
                ("emby_automatic", json!({"state":"skipped","failed":0})),
            ];
            for (key, report) in &phases {
                context.complete(key, report).await?;
            }
            for item in &items {
                let item_context = context.scoped(&format!(
                    "tasks:role-translation:work:{}",
                    item["Id"].as_str().unwrap()
                ));
                for (key, report) in &phases {
                    item_context.complete(key, report).await?;
                }
            }
            reached.notify_one();
            context.cancelled().await;
            context.checkpoint().await?;
            Ok(json!({}))
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), ready.notified())
        .await
        .unwrap();
    f.state.jobs.shutdown().await;
    f.restart().await;
    let result = f.wait_job(&task).await;
    assert_eq!(result["result"]["updated"], 2, "{result}");
    assert_eq!(result["result"]["failures"], json!([]), "{result}");
    let scopes: Vec<Value> = sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key LIKE 'plans:tasks:role-translation:work:%:emby_scope' ORDER BY step_key")
        .bind(&task).fetch_all(&f.state.db).await.unwrap();
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0]["id"], "m1");
    assert_eq!(scopes[1]["id"], "m2");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all media endpoints are localhost"]
async fn resumed_nfo_restore_rejects_rebound_paths_and_preserves_other_movies_files() {
    for update_cache in [false, true] {
        let mut f = Fixture::new(0, false, 0).await;
        let old_path = f.state.data_dir.join("A.mkv");
        let new_path = f.state.data_dir.join("B.mkv");
        tokio::fs::write(&old_path, b"a").await.unwrap();
        tokio::fs::write(&new_path, b"b").await.unwrap();
        let other_nfo = "<movie><title>另一部作品</title><plot>必须保留</plot></movie>".as_bytes();
        let nfo_path = old_path.with_extension("nfo");
        tokio::fs::write(&nfo_path, other_nfo).await.unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json) VALUES('42','Movie','旧缓存作品',true,$1,$2)")
            .bind(json!(["movie"])).bind(json!([{"emby_item_id":"movie","path":old_path}])).execute(&f.state.db).await.unwrap();
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id='42'")
                .fetch_one(&f.state.db)
                .await
                .unwrap();
        let ready = Arc::new(Notify::new());
        let reached = ready.clone();
        let spec = emby_toolkit::jobs::JobSpec::new(
            "tasks",
            json!({"task":"restore-nfo-and-images","full":false,"batch_size":10,"unsupported":[]}),
        );
        let task = f
            .state
            .jobs
            .submit_persistent("恢复NFO和封面", spec, move |context| async move {
                context
                    .save_plan("tasks:restore-nfo-and-images:plan", &json!([row]))
                    .await?;
                reached.notify_one();
                context.cancelled().await;
                context.checkpoint().await?;
                Ok(json!({}))
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), ready.notified())
            .await
            .unwrap();
        if update_cache {
            sqlx::query("UPDATE media_metadata SET asset_details_json=$1 WHERE tmdb_id='42'")
                .bind(json!([{"emby_item_id":"movie","path":new_path}]))
                .execute(&f.state.db)
                .await
                .unwrap();
        }
        f.host.people.lock().await.insert("movie".into(),json!({"Id":"movie","Type":"Movie","Name":"旧缓存作品","Path":new_path,"ProviderIds":{"Tmdb":"42"}}));
        f.state.jobs.shutdown().await;
        f.restart().await;
        let result = f.wait_job(&task).await;
        assert_eq!(result["state"], "failed", "{result}");
        assert!(
            result["result"]["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|failure| failure.as_str().unwrap().contains("绑定已变化")
                    || failure.as_str().unwrap().contains("路径")),
            "{result}"
        );
        assert_eq!(tokio::fs::read(&nfo_path).await.unwrap(), other_nfo);
        assert!(!new_path.with_extension("nfo").exists());
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn empty_people_associations_use_compatible_person_pagination_without_ignored_libraries() {
    let f = Fixture::new(503, true, 0).await;
    let result = f.scan().await;
    let starts = f.host.fallback_starts.lock().await.clone();
    f.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["persons"].as_array().unwrap().len(), 503);
    assert_eq!(starts, vec![0, 500]);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn person_fallback_never_bypasses_ignored_libraries_and_reports_unverifiable_scope() {
    let f = Fixture::new(2, true, 0).await;
    f.state
        .save_config(&json!({"libraries_to_ignore":["ignored"]}))
        .await
        .unwrap();
    let refused = f.scan().await;
    f.state
        .save_config(&json!({"libraries_to_ignore":["ignored","library"]}))
        .await
        .unwrap();
    let excluded = f.scan().await;
    let starts = f.host.fallback_starts.lock().await.clone();
    f.close().await;
    assert_eq!(refused["state"], "failed", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("无法确认人物处理范围")
    );
    assert_eq!(excluded["state"], "completed", "{excluded}");
    assert_eq!(excluded["result"]["persons"], json!([]));
    assert!(starts.is_empty());
}
