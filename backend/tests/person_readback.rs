use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{AppState, media_extra, media_manual, persons, tasks};
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
use tower::ServiceExt;

#[derive(Clone)]
struct Host {
    people: Arc<Mutex<BTreeMap<String, Value>>>,
    media: Arc<Mutex<Value>>,
    mode: usize,
    writes: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
    refresh_logs: Arc<Mutex<Vec<String>>>,
    provider_reads: Arc<AtomicUsize>,
    provider_queries: Arc<Mutex<Vec<String>>>,
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
            if host.mode >= 9 {
                person.as_object_mut().unwrap().remove("ProviderIds");
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
async fn items(State(host): State<Host>, Query(query): Query<HashMap<String, String>>) -> Response {
    if query.contains_key("Ids") {
        host.provider_queries
            .lock()
            .await
            .push(query["Ids"].clone());
        if query
            .get("Fields")
            .is_some_and(|fields| fields.contains("ProviderIds"))
        {
            host.provider_reads.fetch_add(1, Ordering::SeqCst);
        }
        if host.mode == 11 && host.refreshes.load(Ordering::SeqCst) > 0 {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let items = if query
        .get("IncludeItemTypes")
        .is_some_and(|kind| kind == "Person")
    {
        host.people
            .lock()
            .await
            .values()
            .filter(|person| {
                query
                    .get("Ids")
                    .is_none_or(|ids| ids.split(',').any(|id| person["Id"] == id))
            })
            .cloned()
            .collect::<Vec<_>>()
    } else if let Some(ids) = query.get("Ids") {
        let people = host.people.lock().await;
        ids.split(',')
            .filter(|_| host.mode < 9)
            .filter_map(|id| people.get(id).cloned())
            .collect()
    } else {
        vec![host.media.lock().await.clone()]
    };
    Json(json!({"TotalRecordCount":items.len(),"Items":items})).into_response()
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
    if host.mode == 10 {
        host.people.lock().await.get_mut("p1").unwrap()["ProviderIds"]["Tmdb"] = json!("999");
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
                json!({"Id":"p1","Type":"Person","Name":if mode>=9 {"旧演员"}else{"Alice"},"ProviderIds":{"Tmdb":"1"}}),
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
            provider_reads: Arc::default(),
            provider_queries: Arc::default(),
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
async fn manual_actor_save_enriches_omitted_provider_ids_for_chinese_names() {
    let f = Fixture::new(9).await;
    let result = f.manual().await;
    let processed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM processed_log WHERE item_id='movie'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let provider_reads = f.host.provider_reads.load(Ordering::SeqCst);
    let name = f.host.people.lock().await["p1"]["Name"].clone();
    f.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(processed, 1);
    assert_eq!(name, "新演员");
    assert!(
        provider_reads >= 2,
        "both initial and refreshed Chinese cast must resolve native person IDs"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn actor_editor_resolves_native_people_without_cached_cast_or_provider_ids() {
    let f = Fixture::new(9).await;
    sqlx::query("UPDATE media_metadata SET actors_json='[]'::jsonb WHERE tmdb_id='42'")
        .execute(&f.state.db)
        .await
        .unwrap();
    let response = media_manual::router()
        .with_state(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/media_for_editing/movie")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let result: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let provider_reads = f.host.provider_reads.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let actors = result["current_emby_cast"].as_array().unwrap();
    assert_eq!(actors.len(), 2, "{result}");
    assert_eq!(actors[0]["tmdbId"], 1);
    assert_eq!(actors[0]["emby_person_id"], "p1");
    assert_eq!(actors[1]["tmdbId"], 2);
    assert_eq!(actors[1]["emby_person_id"], "p2");
    assert_eq!(provider_reads, 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn actor_preview_falls_back_to_native_people_without_persisting_changes() {
    let f = Fixture::new(9).await;
    sqlx::query("UPDATE media_metadata SET actors_json='[]'::jsonb WHERE tmdb_id='42'")
        .execute(&f.state.db)
        .await
        .unwrap();
    f.state
        .save_config(&json!({"ai_translate_actor_role":false}))
        .await
        .unwrap();
    let snapshot = "SELECT jsonb_build_object('media',(SELECT jsonb_agg(to_jsonb(m)) FROM media_metadata m),'people',(SELECT jsonb_agg(to_jsonb(p)) FROM person_metadata p),'translations',(SELECT jsonb_agg(to_jsonb(t)) FROM translation_cache t),'processed',(SELECT jsonb_agg(to_jsonb(p)) FROM processed_log p))";
    let before: Value = sqlx::query_scalar(snapshot)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    let state = f.state.clone();
    let task = f
        .state
        .jobs
        .submit("演员预览验证", move |context| async move {
            media_extra::preview_cast(&state, "movie", &context).await
        })
        .await
        .unwrap();
    let result = f.wait(&task).await;
    let after: Value = sqlx::query_scalar(snapshot)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    let writes = f.host.writes.load(Ordering::SeqCst);
    let refreshes = f.host.refreshes.load(Ordering::SeqCst);
    let provider_reads = f.host.provider_reads.load(Ordering::SeqCst);
    let nfo = f.directory.path().join("movie.nfo").exists();
    f.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    let actors = result["result"].as_array().unwrap();
    assert_eq!(actors.len(), 2, "{result}");
    assert_eq!(actors[0]["tmdbId"], 1);
    assert_eq!(actors[0]["embyPersonId"], "p1");
    assert_eq!(actors[1]["tmdbId"], 2);
    assert_eq!(actors[1]["embyPersonId"], "p2");
    assert_eq!(
        before, after,
        "preview must preserve media, people and translation caches"
    );
    assert_eq!((writes, refreshes, provider_reads), (0, 0, 1));
    assert!(!nfo);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn cached_foreign_cast_matches_native_chinese_people_without_reverse_translation_cache() {
    let f = Fixture::new(9).await;
    f.host.people.lock().await.get_mut("p1").unwrap()["Name"] = json!("原生中文名");
    sqlx::query("UPDATE person_metadata SET primary_name='Foreign Alice',original_name='Foreign Alice' WHERE tmdb_person_id=1")
        .execute(&f.state.db).await.unwrap();
    f.state
        .save_config(&json!({"ai_translate_actor_role":false}))
        .await
        .unwrap();
    let translation_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translation_cache")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    let state = f.state.clone();
    let task = f
        .state
        .jobs
        .submit("缓存演员预览验证", move |context| async move {
            media_extra::preview_cast(&state, "movie", &context).await
        })
        .await
        .unwrap();
    let result = f.wait(&task).await;
    let provider_reads = f.host.provider_reads.load(Ordering::SeqCst);
    let writes = f.host.writes.load(Ordering::SeqCst);
    let cached_name: String =
        sqlx::query_scalar("SELECT primary_name FROM person_metadata WHERE tmdb_person_id=1")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(translation_count, 0);
    assert_eq!(result["state"], "completed", "{result}");
    let actors = result["result"].as_array().unwrap();
    assert_eq!(actors.len(), 1, "{result}");
    assert_eq!(actors[0]["tmdbId"], 1);
    assert_eq!(actors[0]["embyPersonId"], "p1");
    assert_eq!(actors[0]["name"], "原生中文名");
    assert_eq!(cached_name, "Foreign Alice");
    assert_eq!((provider_reads, writes), (1, 0));
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn ingest_provider_enrichment_preserves_chinese_skip_and_propagates_lookup_failure() {
    for mode in [9, 11] {
        let f = Fixture::new(mode).await;
        f.host.people.lock().await.get_mut("p2").unwrap()["ProviderIds"]["Imdb"] = json!("nm2");
        if mode == 11 {
            f.host.refreshes.store(1, Ordering::SeqCst);
        }
        let people = vec![
            json!({"Id":"p1","Type":"Actor","Name":"中文演员","Role":"原角色"}),
            json!({"Id":"p2","Type":"Actor","Name":"Bob","Role":"先前角色"}),
            json!({"Id":"p2","Type":"Actor","Name":"Bob","Role":"最后角色"}),
            json!({"Id":"known","Type":"Actor","Name":"Known Actor","ProviderIds":{"Tmdb":"777"}}),
            json!({"Id":"director","Type":"Director","Name":"Director"}),
            json!({"Type":"Actor","Name":"No ID"}),
        ];
        let state = f.state.clone();
        let task = f
            .state
            .jobs
            .submit("入库人物ID补齐验证", move |context| async move {
                Ok(json!(
                    persons::enrich_provider_ids(&state, &people, &context).await?
                ))
            })
            .await
            .unwrap();
        let result = f.wait(&task).await;
        let queries = f.host.provider_queries.lock().await.clone();
        let writes = f.host.writes.load(Ordering::SeqCst);
        f.close().await;
        assert_eq!(
            queries,
            vec!["p2"],
            "only the original pending foreign actor subset may be queried"
        );
        assert_eq!(writes, 0);
        if mode == 11 {
            assert_eq!(result["state"], "failed", "{result}");
            let message = result["message"].as_str().unwrap();
            assert!(
                message.contains("p2") && message.contains("GET /Items") && message.contains("403"),
                "{result}"
            );
        } else {
            assert_eq!(result["state"], "completed", "{result}");
            let actors = result["result"].as_array().unwrap();
            assert_eq!(actors.len(), 3);
            assert_eq!(actors[0]["ProviderIds"], Value::Null);
            assert_eq!(actors[0]["Role"], "原角色");
            assert_eq!(actors[1]["ProviderIds"], json!({"Tmdb":"2","Imdb":"nm2"}));
            assert_eq!(actors[1]["Role"], "最后角色");
            assert_eq!(actors[2]["ProviderIds"]["Tmdb"], "777");
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn enriched_actor_with_wrong_tmdb_never_records_success() {
    let f = Fixture::new(10).await;
    let result = f.manual().await;
    let processed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM processed_log WHERE item_id='movie'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    let provider_reads = f.host.provider_reads.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(processed, 0);
    assert!(provider_reads >= 2);
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("人物关联未一致"),
        "{result}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn actor_provider_lookup_failure_reports_upstream_and_never_records_success() {
    let f = Fixture::new(11).await;
    let result = f.manual().await;
    let processed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM processed_log WHERE item_id='movie'")
            .fetch_one(&f.state.db)
            .await
            .unwrap();
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(processed, 0);
    let message = result["message"].as_str().unwrap();
    assert!(
        message.contains("movie") && message.contains("GET /Items") && message.contains("403"),
        "{result}"
    );
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
