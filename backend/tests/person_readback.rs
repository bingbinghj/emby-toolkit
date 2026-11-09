use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, media_manual, persons, tasks};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Host {
    people: Arc<Mutex<BTreeMap<String, Value>>>,
    media: Arc<Mutex<Value>>,
    mode: usize,
    writes: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
    refresh_logs: Arc<Mutex<Vec<String>>>,
}
async fn read(State(host): State<Host>, Path(id): Path<String>) -> Response {
    if host.mode == 3 && host.writes.load(Ordering::SeqCst) > 0 {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    if id == "movie" {
        let mut media = host.media.lock().await.clone();
        let people = host.people.lock().await;
        for person in media["People"].as_array_mut().unwrap() {
            if let Some(live) = people.get(person["Id"].as_str().unwrap_or("")) {
                person["Name"] = live["Name"].clone();
            }
        }
        return Json(media).into_response();
    }
    let mut person = host.people.lock().await[&id].clone();
    if host.mode == 7 {
        person["Id"] = json!("other-person");
    }
    Json(person).into_response()
}
async fn write(
    State(host): State<Host>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    host.writes.fetch_add(1, Ordering::SeqCst);
    if host.mode == 1 || host.mode == 5 && id == "p2" {
        return StatusCode::NO_CONTENT;
    }
    if host.mode == 2 {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            host.people.lock().await.insert(id, body);
        });
    } else {
        host.people.lock().await.insert(id, body);
    }
    StatusCode::NO_CONTENT
}
async fn items(
    State(host): State<Host>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let items = if query
        .get("IncludeItemTypes")
        .is_some_and(|kind| kind == "Person")
    {
        host.people
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>()
    } else if let Some(ids) = query.get("Ids") {
        let people = host.people.lock().await;
        ids.split(',')
            .filter_map(|id| people.get(id).cloned())
            .collect()
    } else {
        vec![host.media.lock().await.clone()]
    };
    Json(json!({"TotalRecordCount":items.len(),"Items":items}))
}
async fn refresh(State(host): State<Host>) -> StatusCode {
    host.refreshes.fetch_add(1, Ordering::SeqCst);
    host.refresh_logs
        .lock()
        .await
        .push("2026-11-09 00:00:01.000 RefreshItem Start: movie fixture".into());
    if host.mode != 4 {
        host.media.lock().await["People"] = json!([{"Id":"p1","Type":"Actor","Name":"新演员","Role":"新角色","ProviderIds":{"Tmdb":"1"}}]);
    }
    if host.mode == 8 {
        host.media.lock().await["People"][0]["Id"] = json!("other-person");
        host.media.lock().await["People"][0]["ProviderIds"]["Tmdb"] = json!("999");
    }
    host.refresh_logs
        .lock()
        .await
        .push("2026-11-09 00:00:02.000 RefreshItem Complete: movie fixture".into());
    StatusCode::NO_CONTENT
}
async fn translate(Json(body): Json<Value>) -> Json<Value> {
    let names: Vec<String> =
        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    let output = names
        .into_iter()
        .map(|name| (name.clone(), format!("译名{name}")))
        .collect::<BTreeMap<_, _>>();
    Json(json!({"choices":[{"message":{"content":json!(output).to_string()}}]}))
}
struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    host: Host,
    server: tokio::task::JoinHandle<()>,
    directory: tempfile::TempDir,
}
impl Fixture {
    async fn new(mode: usize) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("person_readback_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::query(&path).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let media_path = directory.path().join("movie.strm");
        tokio::fs::write(&media_path, "/fixture/movie.mkv")
            .await
            .unwrap();
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        let people = BTreeMap::from([
            (
                "p1".into(),
                json!({"Id":"p1","Type":"Person","Name":"Alice","ProviderIds":{"Tmdb":"1"}}),
            ),
            (
                "p2".into(),
                json!({"Id":"p2","Type":"Person","Name":"Bob","ProviderIds":{"Tmdb":"2"}}),
            ),
        ]);
        let host = Host {
            people: Arc::new(Mutex::new(people)),
            media: Arc::new(Mutex::new(
                json!({"Id":"movie","Type":"Movie","Path":media_path,"Name":"影片","ProviderIds":{"Tmdb":"42"},"Genres":[],"People":[{"Id":"p1","Type":"Actor","Name":"Alice","Role":"旧角色","ProviderIds":{"Tmdb":"1"}},{"Id":"p2","Type":"Actor","Name":"Bob","Role":"旁人","ProviderIds":{"Tmdb":"2"}}]}),
            )),
            mode,
            writes: Arc::default(),
            refreshes: Arc::default(),
            refresh_logs: Arc::default(),
        };
        let app = Router::new()
            .route("/Users/user/Items/{id}", get(read))
            .route("/Items/{id}", post(write))
            .route("/Items/movie/Refresh", post(refresh))
            .route("/Items", get(items))
            .route("/System/Logs/embyserver.txt/Lines", get(|State(host):State<Host>| async move {
                let logs=host.refresh_logs.lock().await.clone();
                Json(json!({"Items":logs.iter().rev().collect::<Vec<_>>(),"TotalRecordCount":logs.len()}))
            }))
            .route(
                "/Users/user/Views",
                get(|| async { Json(json!({"Items":[{"Id":"library"}]})) }),
            )
            .route("/v1/chat/completions", post(translate))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user","libraries_to_ignore":[],"tmdb_api_key":"","remove_actors_without_avatars":false,"actor_role_add_prefix":false,"ai_translate_actor_role":true,"ai_base_url":format!("{base}/v1"),"ai_api_key":"fixture","ai_model_name":"fixture","ai_provider":"openai","ai_max_retries":0,"ai_name_role_request_interval":0})).await.unwrap();
        sqlx::query("INSERT INTO person_metadata(tmdb_person_id,primary_name,profile_path,original_name) VALUES(1,'旧演员','/fixture.jpg','Alice')").execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,emby_item_ids_json,actors_json) VALUES('42','Movie','影片',$1,$2)").bind(json!(["movie"])).bind(json!([{"tmdb_id":1,"character":"旧角色","order":0}])).execute(&state.db).await.unwrap();
        Self {
            state,
            admin,
            schema,
            host,
            server,
            directory,
        }
    }
    async fn person_update(&self) -> emby_toolkit::error::Result<usize> {
        let item = self.host.media.lock().await.clone();
        persons::update_emby(
            &self.state,
            &[json!({"id":1,"name":"新演员","emby_person_id":"p1"})],
            &json!({}),
            &item,
        )
        .await
    }
    async fn manual(&self) -> Value {
        let state = self.state.clone();
        let id = state
            .jobs
            .clone()
            .submit("人物回读验证", move |context| async move {
                media_manual::save(
                    &state,
                    "movie",
                    &json!({"cast":[{"tmdbId":1,"name":"新演员","role":"新角色"}]}),
                    &context,
                )
                .await
            })
            .await
            .unwrap();
        self.wait(&id).await
    }
    async fn wait(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(75), async {
            loop {
                let r = self.state.jobs.result(id).await.unwrap();
                if r["is_running"] == false {
                    return r;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
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
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn person_update_refuses_acknowledged_but_ignored_name() {
    let f = Fixture::new(1).await;
    let result = f.person_update().await;
    let name = f.host.people.lock().await["p1"]["Name"].clone();
    f.close().await;
    assert!(
        result.is_err(),
        "an accepted request is not a persisted name: {result:?}"
    );
    assert_eq!(name, "Alice");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn person_update_waits_for_delayed_name_before_counting() {
    let f = Fixture::new(2).await;
    let result = f.person_update().await;
    let name = f.host.people.lock().await["p1"]["Name"].clone();
    f.close().await;
    assert_eq!(result.unwrap(), 1);
    assert_eq!(name, "新演员");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn person_readback_errors_include_target_and_do_not_count_success() {
    let f = Fixture::new(3).await;
    let result = f.person_update().await;
    f.close().await;
    let error = result.unwrap_err().1;
    assert!(
        error.contains("GET Users/user/Items/p1") && error.contains("回读失败"),
        "{error}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn wrong_person_identity_stops_before_sending_changes() {
    let f = Fixture::new(7).await;
    let result = f.person_update().await;
    let writes = f.host.writes.load(Ordering::SeqCst);
    f.close().await;
    assert!(result.unwrap_err().1.contains("ID"));
    assert_eq!(writes, 0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn manual_actor_roles_must_be_read_back_before_database_success() {
    let f = Fixture::new(4).await;
    let result = f.manual().await;
    let processed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM processed_log WHERE item_id='movie'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let actors: Value =
        sqlx::query_scalar("SELECT actors_json FROM media_metadata WHERE tmdb_id='42'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let primary: String =
        sqlx::query_scalar("SELECT primary_name FROM person_metadata WHERE tmdb_person_id=1")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let nfo = f.directory.path().join("movie.nfo").is_file();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(processed, 0);
    assert_eq!(actors[0]["character"], "旧角色");
    assert_eq!(primary, "旧演员");
    assert!(nfo, "the saved NFO remains available for a retry");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn manual_actor_save_records_confirmed_names_and_roles() {
    let f = Fixture::new(0).await;
    let result = f.manual().await;
    let processed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM processed_log WHERE item_id='movie'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(processed, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn equal_actor_name_and_role_with_wrong_identity_never_record_success() {
    let f = Fixture::new(8).await;
    let result = f.manual().await;
    let processed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM processed_log WHERE item_id='movie'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("人物关联未一致"),
        "{result}"
    );
    assert_eq!(processed, 0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn person_translation_counts_only_persisted_names_and_reports_partial_failure() {
    let f = Fixture::new(5).await;
    let id = tasks::submit_task(&f.state, "actor-translation", false)
        .await
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let result = f.wait(&id).await;
    f.close().await;
    assert_eq!(result["result"]["updated"], 1, "{result}");
    assert_eq!(
        result["result"]["failures"].as_array().unwrap().len(),
        1,
        "{result}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn cancelling_person_readback_stops_polling_without_claiming_success() {
    let f = Fixture::new(1).await;
    let id = tasks::submit_task(&f.state, "actor-translation", false)
        .await
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.host.writes.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.state.jobs.control(Some(&id), "stop").await.unwrap();
    let result = f.wait(&id).await;
    f.close().await;
    assert_eq!(result["state"], "cancelled", "{result}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby and AI are localhost"]
async fn pausing_person_readback_does_not_start_the_next_person() {
    let f = Fixture::new(2).await;
    let id = tasks::submit_task(&f.state, "actor-translation", false)
        .await
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.host.writes.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.state.jobs.control(Some(&id), "pause").await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.state.jobs.result(&id).await.unwrap()["state"] != "paused" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let writes = f.host.writes.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(f.host.writes.load(Ordering::SeqCst), writes);
    f.state.jobs.control(Some(&id), "resume").await.unwrap();
    let result = f.wait(&id).await;
    f.close().await;
    assert_eq!(writes, 1);
    assert_eq!(result["result"]["updated"], 2, "{result}");
}
