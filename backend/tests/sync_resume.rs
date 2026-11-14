use axum::{Json, Router, extract::Query, routing::get};
use emby_toolkit::{AppState, jobs::JobSpec, metadata, sync};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq)]
enum Gate {
    None,
    Series,
    Unbind,
}
struct Source {
    items: Mutex<Vec<Value>>,
    libraries: Mutex<Vec<String>>,
    series_details: Mutex<Value>,
    season_details: Mutex<Value>,
    gate: Gate,
    blocked: AtomicBool,
    reached: Mutex<Option<oneshot::Sender<()>>>,
    release: Notify,
    removal_checks: AtomicUsize,
    movie_reads: AtomicUsize,
    overlap_changed: AtomicBool,
}
impl Source {
    async fn wait(&self) {
        if self.blocked.load(Ordering::Acquire)
            && let Some(reached) = self.reached.lock().await.take()
        {
            reached.send(()).unwrap();
            self.release.notified().await;
        }
    }
}
struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    source: Arc<Source>,
    reached: Option<oneshot::Receiver<()>>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(gate: Gate) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("sync_resume_{}", uuid::Uuid::new_v4().simple());
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
        let root = std::env::temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(&root).unwrap();
        let state = AppState::new(pool, directory.path().into()).await.unwrap();
        let (reached, receiver) = oneshot::channel();
        let source = Arc::new(Source {
            items: Mutex::new(vec![
                json!({"Id":"movie","Type":"Movie","Name":"A movie","Path":"/media/movie.mkv","ProviderIds":{"Tmdb":"123"}}),
                json!({"Id":"series","Type":"Series","Name":"Z series","Path":"/media/series","ProviderIds":{"Tmdb":"456"}}),
                json!({"Id":"season","Type":"Season","Name":"Season 1","Path":"/media/series/Season 1","SeriesId":"series","ParentId":"series","IndexNumber":1,"ProviderIds":{"Tmdb":"789"}}),
            ]),
            libraries: Mutex::new(vec!["library".into()]),
            series_details: Mutex::new(
                json!({"id":456,"name":"Series","original_language":"en","overview":"Current series overview","seasons":[{"id":789,"season_number":1,"name":"Season 1"}]}),
            ),
            season_details: Mutex::new(
                json!({"id":789,"season_number":1,"name":"Season 1","episodes":[{"id":900,"season_number":1,"episode_number":1,"name":"Episode 1"}]}),
            ),
            gate,
            blocked: AtomicBool::new(gate != Gate::None),
            reached: Mutex::new(Some(reached)),
            release: Notify::new(),
            removal_checks: AtomicUsize::new(0),
            movie_reads: AtomicUsize::new(0),
            overlap_changed: AtomicBool::new(false),
        });
        let items_source = source.clone();
        let movie_source = source.clone();
        let series_source = source.clone();
        let season_source = source.clone();
        let library_source = source.clone();
        let router = Router::new()
            .route("/Users/admin/Views", get(move || {
                let source = library_source.clone();
                async move { Json(json!({"Items":source.libraries.lock().await.iter().map(|id| json!({"Id":id})).collect::<Vec<_>>()})) }
            }))
            .route("/Items", get(move |Query(query): Query<HashMap<String, String>>| {
                let source = items_source.clone();
                async move {
                    if query.get("Ids").is_some_and(|ids| ids.split(',').any(|id| id == "removed")) {
                        let count = source.removal_checks.fetch_add(1, Ordering::AcqRel);
                        if source.gate == Gate::Unbind && count == 1 { source.wait().await; }
                    }
                    let libraries = source.libraries.lock().await.clone();
                    let mut items = source.items.lock().await.iter().filter(|item| {
                        if let Some(ids) = query.get("Ids") {
                            ids.split(',').any(|id| item["Id"] == id)
                        } else if let Some(parent) = query.get("ParentId").filter(|id| !libraries.contains(*id)) {
                            matches!(item["Type"].as_str(), Some("Season" | "Episode")) && item["SeriesId"] == parent.as_str()
                        } else { query.get("ParentId").is_none_or(|id| item["_Libraries"].as_array().map_or_else(|| item["_Library"].as_str().unwrap_or("library") == id.as_str(), |libraries| libraries.iter().any(|library| library == id.as_str()))) }
                    }).cloned().collect::<Vec<_>>();
                    if source.overlap_changed.load(Ordering::Acquire) && query.get("ParentId").is_some_and(|id| id == "library-overlap") {
                        for item in &mut items { if item["Id"] == "series" { item["Path"] = json!("/different/series"); } }
                    }
                    Json(json!({"TotalRecordCount":items.len(),"Items":items}))
                }
            }))
            .route("/Library/VirtualFolders", get(|| async { Json(json!([])) }))
            .route("/movie/123", get(move || {
                let source = movie_source.clone();
                async move {
                    source.movie_reads.fetch_add(1, Ordering::AcqRel);
                    Json(json!({"id":123,"title":"Movie","original_language":"en","runtime":90}))
                }
            }))
            .route("/tv/456", get(move || {
                let source = series_source.clone();
                async move {
                    if source.gate == Gate::Series { source.wait().await; }
                    Json(source.series_details.lock().await.clone())
                }
            }))
            .route("/tv/456/season/1", get(move || {
                let source = season_source.clone();
                async move { Json(source.season_details.lock().await.clone()) }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.save_config(&json!({
            "emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin",
            "tmdb_api_base_url":base,"tmdb_api_key":"fixture","ai_translate_on_sync":false,
            "monitor_sha1_pc_search":false,"p115_fingerprint_media_root_cid":"","libraries_to_ignore":[]
        })).await.unwrap();
        for (id, kind, native, parent, path) in [
            ("456", "Series", "series", None, "/media/series"),
            (
                "789",
                "Season",
                "season",
                Some("456"),
                "/media/series/Season 1",
            ),
            (
                "900",
                "Episode",
                "removed",
                Some("456"),
                "/media/series/Season 1/old.mkv",
            ),
        ] {
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,overview,in_library,emby_item_ids_json,asset_details_json,parent_series_tmdb_id,season_number) VALUES($1,$2,'Cached title','Cached overview',true,$3,$4,$5,1)")
                .bind(id).bind(kind).bind(json!([native]))
                .bind(json!([{"emby_item_id":native,"path":path}])).bind(parent)
                .execute(&state.db).await.unwrap();
        }
        Self {
            state,
            admin,
            schema,
            directory,
            source,
            reached: Some(receiver),
            server,
        }
    }
    async fn submit(&self) -> String {
        let work = self.state.clone();
        self.state.jobs.submit_persistent("同步媒体数据",
            JobSpec::new("tasks", json!({"task":"populate-metadata","full":false,"batch_size":1,"unsupported":[]})),
            move |context| async move { sync::cache_with_batch(&work, false, 1, &context).await })
            .await.unwrap()
    }
    async fn interrupt(&mut self) {
        tokio::time::timeout(Duration::from_secs(20), self.reached.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        let jobs = self.state.jobs.clone();
        let closing = tokio::spawn(async move { jobs.shutdown().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.jobs.status().await["state"] != "interrupted" {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        self.source.blocked.store(false, Ordering::Release);
        self.source.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), closing)
            .await
            .unwrap()
            .unwrap();
    }
    async fn restart(&mut self) {
        self.state = AppState::new(self.state.db.clone(), self.directory.path().into())
            .await
            .unwrap();
        self.state
            .jobs
            .resume_pending(self.state.clone(), CancellationToken::new())
            .await
            .unwrap();
    }
    async fn interrupt_at_step(&self, blocked_step: &str, committed_step: &str) -> String {
        let lock_key = format!("{}-work-completion", self.schema);
        let mut locked = self.state.db.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
            .bind(&lock_key)
            .execute(&mut *locked)
            .await
            .unwrap();
        sqlx::raw_sql(&format!("CREATE FUNCTION interrupt_sync_work() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
            IF NEW.step_key='{blocked_step}' THEN
                PERFORM pg_advisory_xact_lock(hashtext('{lock_key}'));
                RAISE EXCEPTION 'fixture interruption after media commit';
            END IF;
            RETURN NEW;
            END $$;
            CREATE TRIGGER interrupt_sync_work BEFORE INSERT ON durable_job_steps FOR EACH ROW EXECUTE FUNCTION interrupt_sync_work();"))
            .execute(&self.state.db).await.unwrap();
        let job = self.submit().await;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let committed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_job_steps WHERE job_id=$1 AND step_key=$2)")
                    .bind(&job).bind(committed_step).fetch_one(&self.state.db).await.unwrap();
                if committed { break; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        let jobs = self.state.jobs.clone();
        let closing = tokio::spawn(async move { jobs.shutdown().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.jobs.status().await["state"] != "interrupted" {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
            .bind(&lock_key)
            .execute(&mut *locked)
            .await
            .unwrap();
        drop(locked);
        tokio::time::timeout(Duration::from_secs(10), closing)
            .await
            .unwrap()
            .unwrap();
        let finished: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM durable_job_steps WHERE job_id=$1 AND step_key=$2)",
        )
        .bind(&job)
        .bind(blocked_step)
        .fetch_one(&self.state.db)
        .await
        .unwrap();
        assert!(
            !finished,
            "the interrupted completion point must not commit"
        );
        sqlx::raw_sql("DROP TRIGGER interrupt_sync_work ON durable_job_steps; DROP FUNCTION interrupt_sync_work();")
            .execute(&self.state.db).await.unwrap();
        job
    }
    async fn result(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(20), async {
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
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_keeps_original_incremental_work_and_does_not_rewrite_completed_movies() {
    let mut fixture = Fixture::new(Gate::Series).await;
    fixture.source.items.lock().await.push(json!({"Id":"series-new","Type":"Series","Name":"Z series","Path":"/media/series-new","ProviderIds":{"Tmdb":"456"}}));
    let job = fixture.submit().await;
    fixture.interrupt().await;
    let movie = metadata::row(&fixture.state, "123", "Movie")
        .await
        .unwrap()
        .unwrap();
    let series = metadata::row(&fixture.state, "456", "Series")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(series["overview"], "Cached overview");
    assert_eq!(
        metadata::row(&fixture.state, "900", "Episode")
            .await
            .unwrap()
            .unwrap()["in_library"],
        false
    );
    let plan: Value = sqlx::query_scalar(
        "SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key='plans:sync-cache'",
    )
    .bind(&job)
    .fetch_one(&fixture.state.db)
    .await
    .unwrap();
    assert!(
        plan["works"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["work"] == json!(["456", "Series"])),
        "the removed episode no longer appears in a freshly computed incremental diff"
    );
    assert_eq!(plan["quick_bindings"], json!([["series-new", "456"]]));
    assert_eq!(
        series["emby_item_ids_json"],
        json!(["series", "series-new"])
    );
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["task_id"], job);
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(result["result"]["offline"], 1, "{result}");
    assert_eq!(
        metadata::row(&fixture.state, "456", "Series")
            .await
            .unwrap()
            .unwrap()["overview"],
        "Current series overview"
    );
    assert_eq!(
        metadata::row(&fixture.state, "123", "Movie")
            .await
            .unwrap()
            .unwrap()["last_synced_at"],
        movie["last_synced_at"]
    );
    assert_eq!(fixture.source.movie_reads.load(Ordering::Acquire), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM durable_jobs")
            .fetch_one(&fixture.state.db)
            .await
            .unwrap(),
        1
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_rechecks_original_unbind_candidates_and_keeps_reappeared_versions() {
    let mut fixture = Fixture::new(Gate::Unbind).await;
    let job = fixture.submit().await;
    fixture.interrupt().await;
    assert_eq!(
        metadata::row(&fixture.state, "900", "Episode")
            .await
            .unwrap()
            .unwrap()["in_library"],
        true
    );
    fixture.source.items.lock().await.push(json!({
        "Id":"removed","Type":"Episode","Name":"Episode 1","Path":"/media/series/Season 1/old.mkv",
        "SeriesId":"series","ParentId":"season","ParentIndexNumber":1,"IndexNumber":1,"ProviderIds":{"Tmdb":"900"}
    }));
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["offline"], 0, "{result}");
    let episode = metadata::row(&fixture.state, "900", "Episode")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(episode["in_library"], true);
    assert_eq!(episode["emby_item_ids_json"], json!(["removed"]));
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn a_planned_version_disappearing_during_restart_is_reported_without_overwriting_its_cache() {
    let mut fixture = Fixture::new(Gate::Series).await;
    let job = fixture.submit().await;
    fixture.interrupt().await;
    let before = metadata::row(&fixture.state, "456", "Series")
        .await
        .unwrap()
        .unwrap();
    fixture
        .source
        .items
        .lock()
        .await
        .retain(|item| item["Type"] == "Movie");
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["updated"], 1, "{result}");
    assert_eq!(
        result["result"]["warning_details"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        metadata::row(&fixture.state, "456", "Series")
            .await
            .unwrap()
            .unwrap(),
        before
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_refuses_a_planned_top_with_a_different_tmdb_binding() {
    let mut fixture = Fixture::new(Gate::Series).await;
    let job = fixture.submit().await;
    fixture.interrupt().await;
    let before = metadata::row(&fixture.state, "456", "Series")
        .await
        .unwrap()
        .unwrap();
    for item in fixture.source.items.lock().await.iter_mut() {
        if item["Id"] == "series" {
            item["ProviderIds"]["Tmdb"] = json!("999");
        }
    }
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["updated"], 1, "{result}");
    assert!(
        result["result"]["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure.as_str().unwrap().contains("TMDb"))
    );
    assert_eq!(
        metadata::row(&fixture.state, "456", "Series")
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(
        metadata::row(&fixture.state, "999", "Series")
            .await
            .unwrap()
            .is_none()
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_does_not_follow_a_planned_media_into_a_new_library() {
    let mut fixture = Fixture::new(Gate::Series).await;
    let job = fixture.submit().await;
    fixture.interrupt().await;
    let before = metadata::row(&fixture.state, "456", "Series")
        .await
        .unwrap()
        .unwrap();
    fixture
        .source
        .libraries
        .lock()
        .await
        .push("library-new".into());
    for item in fixture.source.items.lock().await.iter_mut() {
        if item["Type"] != "Movie" {
            item["_Library"] = json!("library-new");
        }
    }
    fixture.source.items.lock().await.push(json!({"Id":"not-planned","Type":"Movie","Name":"New library movie","ProviderIds":{"Tmdb":"999"},"_Library":"library-new"}));
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["updated"], 1, "{result}");
    assert!(
        result["result"]["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure.as_str().unwrap().contains("媒体库"))
    );
    assert_eq!(
        metadata::row(&fixture.state, "456", "Series")
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(
        metadata::row(&fixture.state, "999", "Movie")
            .await
            .unwrap()
            .is_none()
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_uses_frozen_child_keys_after_tmdb_details_become_available() {
    let mut fixture = Fixture::new(Gate::None).await;
    fixture.source.items.lock().await.push(json!({
        "Id":"new-episode","Type":"Episode","Name":"Episode 1","Path":"/media/series/Season 1/new.mkv",
        "SeriesId":"series","ParentId":"season","ParentIndexNumber":1,"IndexNumber":1,"ProviderIds":{"Tmdb":"900"}
    }));
    *fixture.source.season_details.lock().await =
        json!({"id":789,"season_number":1,"name":"Season 1","episodes":[]});
    let job = fixture
        .interrupt_at_step(
            "completed:sync-children:Series:456",
            "completed:sync-record:Series:456",
        )
        .await;
    let episode = metadata::row(&fixture.state, "456-S1E1", "Episode")
        .await
        .unwrap()
        .unwrap();
    let season = metadata::row(&fixture.state, "789", "Season")
        .await
        .unwrap()
        .unwrap();
    *fixture.source.series_details.lock().await = json!({"id":456,"name":"Series","original_language":"en","seasons":[{"id":790,"season_number":1,"name":"New season"}]});
    *fixture.source.season_details.lock().await = json!({"id":790,"season_number":1,"name":"New season","episodes":[{"id":900,"season_number":1,"episode_number":1,"name":"New episode"}]});
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 4, "{result}");
    assert_eq!(
        metadata::row(&fixture.state, "456-S1E1", "Episode")
            .await
            .unwrap()
            .unwrap(),
        episode
    );
    assert_eq!(
        metadata::row(&fixture.state, "789", "Season")
            .await
            .unwrap()
            .unwrap(),
        season
    );
    assert!(
        metadata::row(&fixture.state, "790", "Season")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        metadata::row(&fixture.state, "900", "Episode")
            .await
            .unwrap()
            .unwrap()["emby_item_ids_json"],
        json!([])
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_retains_committed_child_cleanup_counts_when_the_top_disappears() {
    let mut fixture = Fixture::new(Gate::None).await;
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,season_number,in_library,emby_item_ids_json) VALUES('901','Episode','456',1,true,'[]')")
        .execute(&fixture.state.db).await.unwrap();
    let job = fixture
        .interrupt_at_step(
            "completed:sync-work:Series:456",
            "completed:sync-children:Series:456",
        )
        .await;
    fixture
        .source
        .items
        .lock()
        .await
        .retain(|item| item["Type"] == "Movie");
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(result["result"]["offline"], 2, "{result}");
    assert_eq!(
        metadata::row(&fixture.state, "901", "Episode")
            .await
            .unwrap()
            .unwrap()["in_library"],
        false
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn overlapping_libraries_keep_the_existing_source_choice_and_reject_conflicting_items() {
    for conflicting in [false, true] {
        let mut fixture = Fixture::new(Gate::None).await;
        fixture
            .source
            .libraries
            .lock()
            .await
            .push("library-overlap".into());
        for item in fixture.source.items.lock().await.iter_mut() {
            item["_Libraries"] = json!(["library", "library-overlap"]);
        }
        fixture
            .source
            .overlap_changed
            .store(conflicting, Ordering::Release);
        if conflicting {
            let job = fixture.submit().await;
            let result = fixture.result(&job).await;
            assert_eq!(result["state"], "failed", "{result}");
            assert_eq!(
                metadata::row(&fixture.state, "900", "Episode")
                    .await
                    .unwrap()
                    .unwrap()["in_library"],
                true
            );
            let published: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_job_steps WHERE job_id=$1 AND step_key='plans:sync-cache')")
                .bind(&job).fetch_one(&fixture.state.db).await.unwrap();
            assert!(
                !published,
                "conflicting duplicate IDs cannot publish a plan or unbind media"
            );
        } else {
            let job = fixture
                .interrupt_at_step(
                    "completed:sync-children:Series:456",
                    "completed:sync-record:Series:456",
                )
                .await;
            let before = metadata::row(&fixture.state, "456", "Series")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                before["asset_details_json"][0]["source_library_id"],
                "library-overlap"
            );
            fixture.restart().await;
            let result = fixture.result(&job).await;
            assert_eq!(result["state"], "completed", "{result}");
            assert_eq!(result["result"]["updated"], 3, "{result}");
            assert_eq!(
                metadata::row(&fixture.state, "456", "Series")
                    .await
                    .unwrap()
                    .unwrap(),
                before
            );
        }
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn failed_transaction_commit_rolls_back_media_and_success_checkpoints_together() {
    let fixture = Fixture::new(Gate::None).await;
    sqlx::query("DELETE FROM media_metadata")
        .execute(&fixture.state.db)
        .await
        .unwrap();
    fixture.source.items.lock().await.truncate(1);
    sqlx::raw_sql("CREATE FUNCTION reject_sync_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture deferred failure'; END $$;
        CREATE CONSTRAINT TRIGGER reject_sync_commit AFTER INSERT OR UPDATE ON media_metadata DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_sync_commit();")
        .execute(&fixture.state.db).await.unwrap();
    let job = fixture.submit().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert!(
        metadata::row(&fixture.state, "123", "Movie")
            .await
            .unwrap()
            .is_none()
    );
    let steps: Vec<String> =
        sqlx::query_scalar("SELECT step_key FROM durable_job_steps WHERE job_id=$1")
            .bind(&job)
            .fetch_all(&fixture.state.db)
            .await
            .unwrap();
    assert!(
        !steps
            .iter()
            .any(|key| key == "completed:sync-record:Movie:123"
                || key == "completed:sync-written:Movie:123"),
        "a deferred commit failure must roll back success records as well as media"
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn restart_after_media_commit_skips_rows_even_when_work_completion_was_interrupted() {
    let mut fixture = Fixture::new(Gate::None).await;
    let job = fixture
        .interrupt_at_step(
            "completed:sync-children:Series:456",
            "completed:sync-record:Series:456",
        )
        .await;
    let series = metadata::row(&fixture.state, "456", "Series")
        .await
        .unwrap()
        .unwrap();
    let season = metadata::row(&fixture.state, "789", "Season")
        .await
        .unwrap()
        .unwrap();
    fixture.restart().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["updated"], 3, "{result}");
    assert_eq!(result["result"]["offline"], 1, "{result}");
    assert_eq!(
        metadata::row(&fixture.state, "456", "Series")
            .await
            .unwrap()
            .unwrap()["last_synced_at"],
        series["last_synced_at"]
    );
    assert_eq!(
        metadata::row(&fixture.state, "789", "Season")
            .await
            .unwrap()
            .unwrap()["last_synced_at"],
        season["last_synced_at"]
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all upstreams are localhost"]
async fn incomplete_scan_cannot_publish_a_plan_or_remove_original_bindings() {
    let fixture = Fixture::new(Gate::None).await;
    let movie = fixture.source.items.lock().await[0].clone();
    *fixture.source.items.lock().await = vec![movie.clone(), movie];
    let job = fixture.submit().await;
    let result = fixture.result(&job).await;
    assert_eq!(result["state"], "failed", "{result}");
    let episode = metadata::row(&fixture.state, "900", "Episode")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(episode["in_library"], true);
    assert_eq!(episode["emby_item_ids_json"], json!(["removed"]));
    let published: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_job_steps WHERE job_id=$1 AND step_key='plans:sync-cache')")
            .bind(&job)
            .fetch_one(&fixture.state.db)
            .await
            .unwrap();
    assert!(!published);
    fixture.close().await;
}
