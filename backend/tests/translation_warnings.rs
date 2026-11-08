use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use emby_toolkit::{
    AppState, ai, douban, ingest, media_extra, media_manual, sync, tasks, translation,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Host {
    mode: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}
async fn translate(State(host): State<Host>, Json(body): Json<Value>) -> Response {
    host.calls.fetch_add(1, Ordering::SeqCst);
    if host.mode.load(Ordering::SeqCst) == 1 {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let input: Value =
        serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    let terms = input.as_array().or_else(|| input["terms"].as_array());
    let inputs: BTreeMap<String, String> = if let Some(terms) = terms {
        terms
            .iter()
            .map(|term| (term.as_str().unwrap().into(), term.as_str().unwrap().into()))
            .collect()
    } else {
        input
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, term)| (id.clone(), term.as_str().unwrap().into()))
            .collect()
    };
    let output: BTreeMap<_, _> = inputs
        .into_iter()
        .filter(|(_, text)| text != "Missing")
        .map(|(id, text)| (id, format!("译文{text}")))
        .collect();
    Json(json!({"choices":[{"message":{"content":json!(output).to_string()}}]})).into_response()
}
struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    host: Host,
    item: Value,
}
impl Fixture {
    async fn new(mode: usize) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("translation_warnings_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
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
        let path = directory.path().join("Movie.strm");
        std::fs::write(&path, "https://fixture.invalid/video").unwrap();
        let item = json!({"Id":"movie","Type":"Movie","Name":"测试电影","Path":path,"ProviderIds":{"Tmdb":"7"},"Genres":[],"People":if mode==4 {json!([{"Id":"person","Name":"Missing"}])}else{json!([])}});
        let host = Host {
            mode: Arc::new(AtomicUsize::new(mode)),
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let movie = item.clone();
        let own = item.clone();
        let own_host = host.clone();
        let items_host = host.clone();
        let router = Router::new()
            .route("/v1/chat/completions",post(translate))
            .route("/Users/admin/Views",get(||async { Json(json!({"Items":[{"Id":"library"}]})) }))
            .route("/Items",get(move |Query(query): Query<HashMap<String,String>>|{
                let movie = movie.clone(); let host=items_host.clone(); async move {
                    if host.mode.load(Ordering::SeqCst)==5 && query.get("Ids").is_some_and(|ids| ids.split(',').any(|id|id=="movie")) {
                        return StatusCode::BAD_GATEWAY.into_response();
                    }
                    let items = if query.get("Ids").is_some_and(|ids| ids=="person") {
                        vec![json!({"Id":"person","Type":"Person","Name":"Missing"})]
                    } else if query.get("ParentId").is_some_and(|id| id == "library")
                        || query.get("Ids").is_some_and(|ids| ids.split(',').any(|id| id=="movie")) {
                        vec![movie]
                    } else { vec![] };
                    Json(json!({"Items":items,"TotalRecordCount":items.len()})).into_response()
                }
            }))
            .route("/Users/admin/Items/movie",get(move || { let item=own.clone(); async move {Json(item)} }))
            .route("/Items/movie/Ancestors",get(||async {Json(json!([]))}))
            .route("/Library/VirtualFolders",get(||async {Json(json!([]))}))
            .route("/movie/7",get(move ||{
                let host=own_host.clone(); async move {
                    if host.mode.load(Ordering::SeqCst)==2 { StatusCode::BAD_GATEWAY.into_response() }
                    else { Json(json!({"id":7,"title":"Missing","overview":"中文简介","tagline":"中文标语","credits":{"cast":if host.mode.load(Ordering::SeqCst)==3 {json!([{"id":1,"name":"Valid","character":"Missing","profile_path":"/photo.jpg"}])}else{json!([])}},"images":{"posters":[],"backdrops":[]}})).into_response() }
                }
            }))
            .route("/tv/7/season/1",get(||async {Json(json!({"id":71,"season_number":1,"name":"中文季","episodes":[{"id":711,"season_number":1,"episode_number":1,"name":"Missing","overview":"中文简介"}]}))}))
            .with_state(host.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","tmdb_api_base_url":base,"tmdb_api_key":"fixture","douban_enable_online_api":false,"ai_provider":"openai","ai_api_key":"fixture","ai_model_name":"fixture","ai_base_url":format!("{base}/v1"),"ai_max_retries":0,"ai_name_role_request_interval":0,"ai_translate_title":true,"ai_translate_overview":false,"ai_translate_episode_overview":false,"ai_translate_actor_role":false,"ai_translate_on_sync":false,"p115_fingerprint_media_root_cid":"","local_strm_root":directory.path()})).await.unwrap();
        Self {
            state,
            root,
            schema,
            _directory: directory,
            server,
            host,
            item,
        }
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.server.abort();
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
    async fn job(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = self.state.jobs.result(task).await.unwrap();
                if job["is_running"] == false {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("localhost job must finish")
    }
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn partial_translation_reports_missing_results_and_valid_cache_hits_are_not_warnings() {
    let f = Fixture::new(0).await;
    let inputs = BTreeMap::from([("1".into(), "Valid".into()), ("2".into(), "Missing".into())]);
    let (translated, warnings) =
        ai::translate_report(&f.state, &inputs, "fast_mode", &json!({}), &Value::Null)
            .await
            .unwrap();
    assert_eq!(translated["1"], "译文Valid");
    assert!(!translated.contains_key("2"));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("1 条"));
    let cached = BTreeMap::from([("1".into(), "Valid".into())]);
    let (_, warnings) =
        ai::translate_report(&f.state, &cached, "fast_mode", &json!({}), &Value::Null)
            .await
            .unwrap();
    assert!(warnings.is_empty());
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 1);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn native_request_failure_is_reported_but_preserves_source_metadata() {
    let f = Fixture::new(1).await;
    let mut data = json!({"id":7,"title":"Missing"});
    let report = translation::metadata_tree_report(&f.state, "Movie", &mut data, "测试电影")
        .await
        .unwrap();
    assert_eq!(data["title"], "Missing");
    assert_eq!(report["completed"], 0);
    assert_eq!(report["warnings"], 1);
    assert!(
        report["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("批次失败")
    );
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn chinese_fields_and_disabled_translation_do_not_create_false_warnings() {
    let f = Fixture::new(1).await;
    let mut data = json!({"id":7,"title":"已有中文"});
    let report = translation::metadata_tree_report(&f.state, "Movie", &mut data, "测试电影")
        .await
        .unwrap();
    assert_eq!(report["warnings"], 0);
    f.state
        .save_config(&json!({"ai_translate_title":false}))
        .await
        .unwrap();
    data["title"] = json!("Missing");
    let report = translation::metadata_tree_report(&f.state, "Movie", &mut data, "测试电影")
        .await
        .unwrap();
    assert_eq!(report["warnings"], 0);
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 0);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn manual_actor_translation_marks_only_incomplete_rows_and_keeps_the_array_contract() {
    let f = Fixture::new(0).await;
    f.state
        .save_config(&json!({"ai_translate_actor_role":true,"ai_translation_mode":"fast"}))
        .await
        .unwrap();
    let result = media_manual::translate_cast(
        &f.state,
        &json!({"cast":[{"name":"Valid","role":"Missing"},{"name":"中文人名","role":"中文角色","matchStatus":"翻译未完成","translationWarning":"旧提醒"}]}),
    )
    .await
    .unwrap();
    assert!(result.is_array());
    assert_eq!(result[0]["name"], "译文Valid");
    assert_eq!(result[0]["role"], "Missing");
    assert_eq!(result[0]["matchStatus"], "翻译未完成");
    assert!(!result[0]["translationWarning"].as_str().unwrap().is_empty());
    assert!(result[1]["translationWarning"].is_null());
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn cached_season_patch_reports_translation_warnings_but_complete_cache_skips_ai() {
    let f = Fixture::new(0).await;
    let mut payload =
        json!({"name":"测试剧","genres":[],"seasons_details":[],"episodes_details":{}});
    let report = ingest::patch_cached_series_report(&f.state, "7", &mut payload, &[(1, 1)])
        .await
        .unwrap();
    assert_eq!(report["patched"], true);
    assert_eq!(report["warnings"], 1);
    assert_eq!(payload["episodes_details"]["S1E1"]["name"], "Missing");
    let report = ingest::patch_cached_series_report(&f.state, "7", &mut payload, &[(1, 1)])
        .await
        .unwrap();
    assert_eq!(report["patched"], false);
    assert_eq!(report["warnings"], 0);
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 1);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn pending_ingest_task_keeps_translation_warnings_in_its_final_result() {
    let f = Fixture::new(0).await;
    let work = f.state.clone();
    let item = f.item.clone();
    let task = f
        .state
        .jobs
        .submit("入库翻译警告验证", move |context| async move {
            ingest::prepare_new(&work, &item, &[], None, &context).await
        })
        .await
        .unwrap();
    let job = f.job(&task).await;
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["updated"], 1);
    assert_eq!(job["result"]["warnings"], 1);
    assert!(
        job["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("AI 翻译未完成")
    );
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn synchronization_metadata_fetch_failure_is_a_task_warning_and_keeps_existing_metadata() {
    let f = Fixture::new(2).await;
    sqlx::query(
        "INSERT INTO media_metadata(tmdb_id,item_type,title) VALUES('7','Movie','原有中文标题')",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    let work = f.state.clone();
    let task = f
        .state
        .jobs
        .submit("同步抓取警告验证", move |context| async move {
            sync::cache_with_batch(&work, true, 10, &context).await
        })
        .await
        .unwrap();
    let job = f.job(&task).await;
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["updated"], 1);
    assert_eq!(job["result"]["failures"], json!([]));
    assert_eq!(job["result"]["warnings"], 1);
    assert!(
        job["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("读取元数据失败")
    );
    let title: String = sqlx::query_scalar(
        "SELECT title FROM media_metadata WHERE tmdb_id='7' AND item_type='Movie'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    assert_eq!(title, "原有中文标题");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn synchronization_observed_detail_failure_warns_without_overwriting_cached_metadata() {
    let f = Fixture::new(5).await;
    sqlx::query(
        "INSERT INTO media_metadata(tmdb_id,item_type,title) VALUES('7','Movie','原有中文标题')",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    let work = f.state.clone();
    let task = f
        .state
        .jobs
        .submit("同步详情警告验证", move |context| async move {
            sync::cache_with_batch(&work, true, 10, &context).await
        })
        .await
        .unwrap();
    let job = f.job(&task).await;
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["updated"], 0);
    assert_eq!(job["result"]["warnings"], 1);
    assert!(
        job["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("媒体详情回读不完整")
    );
    let title: String = sqlx::query_scalar(
        "SELECT title FROM media_metadata WHERE tmdb_id='7' AND item_type='Movie'",
    )
    .fetch_one(&f.state.db)
    .await
    .unwrap();
    assert_eq!(title, "原有中文标题");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn synchronization_confirmed_offline_id_is_not_an_incomplete_detail_warning() {
    let f = Fixture::new(0).await;
    sqlx::query(
        "INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json) VALUES('7','Movie','原有中文标题',true,'[\"old\"]'::jsonb)",
    )
    .execute(&f.state.db)
    .await
    .unwrap();
    let work = f.state.clone();
    let task = f
        .state
        .jobs
        .submit("同步离线正常跳过验证", move |context| async move {
            sync::cache_with_batch(&work, true, 10, &context).await
        })
        .await
        .unwrap();
    let job = f.job(&task).await;
    assert_eq!(job["state"], "completed", "{job}");
    assert_eq!(job["result"]["updated"], 1);
    assert_eq!(job["result"]["offline"], 1);
    assert_eq!(job["result"]["warnings"], 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn person_translation_task_reports_missing_translation_without_writing_original_name() {
    let f = Fixture::new(4).await;
    f.state
        .save_config(&json!({"ai_translate_actor_role":true}))
        .await
        .unwrap();
    let submitted = tasks::submit_task(&f.state, "actor-translation", false)
        .await
        .unwrap();
    let job = f.job(submitted["task_id"].as_str().unwrap()).await;
    assert_eq!(job["state"], "completed_with_warnings", "{job}");
    assert_eq!(job["result"]["updated"], 0);
    assert_eq!(job["result"]["failures"], json!([]));
    assert_eq!(job["result"]["warnings"], 1);
    assert!(
        job["result"]["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("AI 翻译未完成")
    );
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn douban_failure_keeps_actor_source_and_reports_reason_while_disabled_douban_skips() {
    let mut f = Fixture::new(0).await;
    let mut service = douban::Service::default();
    let base = f.state.config.read().await["tmdb_api_base_url"]
        .as_str()
        .unwrap()
        .to_string();
    service.api_base = base.clone();
    service.imdb_base = base;
    f.state.douban = Arc::new(service);
    f.state
        .save_config(&json!({"douban_enable_online_api":true}))
        .await
        .unwrap();
    let mut data = json!({"credits":{"cast":[{"id":1,"name":"Valid","character":"Missing","profile_path":"/photo.jpg"}]}});
    let original = data.clone();
    let report = douban::premerge_online_report(&f.state, &f.item, &mut data)
        .await
        .unwrap();
    assert_eq!(data, original);
    assert_eq!(report["warnings"], 1);
    assert!(
        report["warning_details"][0]
            .as_str()
            .unwrap()
            .contains("豆瓣演员补充失败")
    );
    f.state
        .save_config(&json!({"douban_enable_online_api":false}))
        .await
        .unwrap();
    let report = douban::premerge_online_report(&f.state, &f.item, &mut data)
        .await
        .unwrap();
    assert_eq!(report["warnings"], 0);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn actor_preview_keeps_array_and_marks_incomplete_translation_without_persisting_people() {
    let f = Fixture::new(3).await;
    f.state
        .save_config(&json!({"ai_translate_actor_role":true,"ai_translation_mode":"fast"}))
        .await
        .unwrap();
    let work = f.state.clone();
    let task = f
        .state
        .jobs
        .submit("演员预览警告验证", move |context| async move {
            media_extra::preview_cast(&work, "movie", &context).await
        })
        .await
        .unwrap();
    let job = f.job(&task).await;
    let actors = &job["result"];
    assert!(actors.is_array(), "{job}");
    assert_eq!(actors[0]["name"], "译文Valid");
    assert_eq!(actors[0]["role"], "Missing");
    assert_eq!(actors[0]["matchStatus"], "翻译未完成");
    assert!(!actors[0]["translationWarning"].as_str().unwrap().is_empty());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM person_metadata")
        .fetch_one(&f.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.close().await;
}
