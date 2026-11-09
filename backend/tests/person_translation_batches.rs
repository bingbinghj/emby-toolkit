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
    if host.mode == 2 && count == 50 {
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
    }
    let input: Vec<String> =
        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    let output = input
        .into_iter()
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
