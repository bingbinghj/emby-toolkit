use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use emby_toolkit::{AppState, db, fingerprint_audit, p115};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone)]
struct Mock {
    directories: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    requests: Arc<Mutex<Vec<(String, usize)>>>,
    emby_reads: Arc<Mutex<Vec<String>>>,
    hold_emby_read: Arc<Mutex<Option<usize>>>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    sizes: Arc<Mutex<HashMap<String, u64>>>,
    source_mode: Arc<AtomicUsize>,
    searches: Arc<Mutex<usize>>,
    request_times: Arc<Mutex<Vec<tokio::time::Instant>>>,
    missing_roots: Arc<AtomicUsize>,
}

struct Fixture {
    state: AppState,
    root: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    mock: Mock,
    servers: Vec<tokio::task::JoinHandle<()>>,
}

impl Fixture {
    async fn new(stems: &[&str], paginated: bool) -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("fingerprint_audit_cache_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&root)
            .await
            .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |conn, _| {
                let query = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&query).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut state = AppState::new(pool.clone(), directory.path().into())
            .await
            .unwrap();
        let mut parents = vec![];
        let mut files = vec![];
        if paginated {
            for n in 0..1000 {
                parents
                    .push(json!({"cid":format!("dir-{n}"),"name":format!("Other-{n}"),"fc":"0"}));
                files.push(
                    json!({"id":format!("file-{n}"),"name":format!("Other-{n}.mkv"),"fc":"1"}),
                );
            }
        }
        parents.push(json!({"cid":"3","name":"Season","fc":"0"}));
        for (index, stem) in stems.iter().enumerate() {
            let id = (101 + index).to_string();
            files.push(json!({"id":id,"name":format!("{stem}.mkv"),"pick_code":id,"sha1":format!("{stem}-SHA"),"size":100,"fc":"1"}));
        }
        let mock = Mock {
            directories: Arc::new(Mutex::new(HashMap::from([
                ("9".into(), parents),
                ("3".into(), files),
            ]))),
            requests: Arc::new(Mutex::new(vec![])),
            emby_reads: Arc::new(Mutex::new(vec![])),
            hold_emby_read: Arc::new(Mutex::new(None)),
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            sizes: Arc::new(Mutex::new(
                stems.iter().map(|stem| (stem.to_string(), 100)).collect(),
            )),
            source_mode: Arc::new(AtomicUsize::new(0)),
            searches: Arc::new(Mutex::new(0)),
            request_times: Arc::new(Mutex::new(vec![])),
            missing_roots: Arc::new(AtomicUsize::new(0)),
        };
        let cloud = Router::new()
            .route("/files", get(|State(mock): State<Mock>, Query(query): Query<HashMap<String, String>>| async move {
                let cid = query["cid"].clone();
                let offset = query["offset"].parse::<usize>().unwrap();
                mock.requests.lock().await.push((cid.clone(), offset));
                mock.request_times.lock().await.push(tokio::time::Instant::now());
                if cid == "9" && mock.missing_roots.compare_exchange(1,0,Ordering::SeqCst,Ordering::SeqCst).is_ok() {
                    return Json(json!({"state":false,"errno":20018}));
                }
                let all = mock.directories.lock().await[&cid].clone();
                Json(json!({"state":true,"count":all.len(),"data":all.into_iter().skip(offset).take(1000).collect::<Vec<_>>()}))
            }))
            .route("/files/search", get(|State(mock): State<Mock>| async move {
                *mock.searches.lock().await += 1;
                Json(json!({"state":true,"count":0,"data":[]}))
            }))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cloud_address = listener.local_addr().unwrap();
        let cloud_server = tokio::spawn(async move { axum::serve(listener, cloud).await.unwrap() });
        state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            webapi: format!("http://{cloud_address}"),
            ..Default::default()
        }));
        db::save_setting(
            &pool,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
        )
        .await
        .unwrap();
        let emby_root = directory.path().to_path_buf();
        let emby = Router::new()
            .route("/Users/admin/Items/{id}", get(move |State(mock): State<Mock>, Path(id): Path<String>| {
                let root = emby_root.clone();
                async move {
                    let read = {
                        let mut reads = mock.emby_reads.lock().await;
                        reads.push(id.clone());
                        reads.len()
                    };
                    if *mock.hold_emby_read.lock().await == Some(read) {
                        mock.entered.add_permits(1);
                        mock.release.acquire().await.unwrap().forget();
                    }
                    let mut item = json!({"Id":id,"Type":"Episode","Path":root.join(format!("Season/{id}.strm")),"Size":17});
                    let mode = mock.source_mode.load(Ordering::SeqCst);
                    if mode != 1 {
                        let mut source = json!({"ItemId":id,"Path":format!("/cd2/Season/{id}.mkv"),"Container":"mkv","Size":mock.sizes.lock().await[&id]});
                        if mode == 2 {
                            source["Path"] = item["Path"].clone();
                            source["Container"] = json!("strm");
                            source["Size"] = json!(19);
                        }
                        item["MediaSources"] = json!([source]);
                    }
                    Json(item)
                }
            }))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let emby_address = listener.local_addr().unwrap();
        let emby_server = tokio::spawn(async move { axum::serve(listener, emby).await.unwrap() });
        state.save_config(&json!({"cd2_media_root":"/cd2","local_strm_root":directory.path(),"p115_fingerprint_media_root_cid":"9","emby_user_id":"admin","emby_api_key":"fixture","emby_server_url":format!("http://{emby_address}")})).await.unwrap();
        tokio::fs::create_dir_all(directory.path().join("Season"))
            .await
            .unwrap();
        for (index, stem) in stems.iter().enumerate() {
            let id = (101 + index).to_string();
            let path = directory.path().join(format!("Season/{stem}.strm"));
            tokio::fs::write(&path, format!("/cd2/Season/{stem}.mkv"))
                .await
                .unwrap();
            sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Episode',true,$2,$3,$4,$5)")
                .bind(stem).bind(json!([stem])).bind(json!([{"emby_item_id":stem,"path":path,"size_bytes":100,"fingerprint_pending":false}])).bind(json!([format!("{stem}-SHA")])).bind(json!([id])).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,pick_code,sha1,size,local_path) VALUES($1,'3',$2,$1,$3,100,$4)")
                .bind(&id).bind(format!("{stem}.mkv")).bind(format!("{stem}-SHA")).bind(format!("Season/{stem}.mkv")).execute(&pool).await.unwrap();
        }
        Self {
            state,
            root,
            schema,
            directory,
            mock,
            servers: vec![cloud_server, emby_server],
        }
    }

    async fn start(&self) -> String {
        let work = self.state.clone();
        self.state
            .jobs
            .submit("全库审计目录缓存验证", move |context| async move {
                fingerprint_audit::scan(&work, "start", None, &context).await
            })
            .await
            .unwrap()
    }

    async fn finish(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let result = self.state.jobs.result(task).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    async fn held(&self) {
        tokio::time::timeout(Duration::from_secs(60), self.mock.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    async fn metadata(&self) -> Vec<Value> {
        sqlx::query_scalar("SELECT jsonb_build_object('id',tmdb_id,'sha1',file_sha1_json,'pc',file_pickcode_json,'assets',asset_details_json) FROM media_metadata ORDER BY tmdb_id")
            .fetch_all(&self.state.db).await.unwrap()
    }

    async fn cleanup(self) {
        self.state.jobs.shutdown().await;
        for server in self.servers {
            server.abort();
        }
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn reports_unknown_emby_size_without_an_actual_media_source() {
    for mode in [1, 2] {
        let fixture = Fixture::new(&["First"], false).await;
        fixture.mock.source_mode.store(mode, Ordering::SeqCst);
        sqlx::query("UPDATE media_metadata SET file_sha1_json='[\"OLD-SHA\"]'")
            .execute(&fixture.state.db)
            .await
            .unwrap();
        let task = fixture.start().await;
        let result = fixture.finish(&task).await;
        let report: Value = serde_json::from_slice(
            &tokio::fs::read(
                fixture
                    .directory
                    .path()
                    .join("fingerprint-scan-latest.json"),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        fixture.cleanup().await;
        assert_eq!(
            result["result"]["counts"],
            json!({"consistent":0,"repaired":1,"unknown":0}),
            "mode={mode}: {result}"
        );
        assert!(
            report["entries"][0]["emby_size"].is_null(),
            "mode={mode}: {report}"
        );
        assert_eq!(report["entries"][0]["new"]["size"], 100);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn full_library_audit_reuses_all_directory_pages_across_episodes() {
    let fixture = Fixture::new(&["First", "Second"], true).await;
    let task = fixture.start().await;
    let result = fixture.finish(&task).await;
    let requests = fixture.mock.requests.lock().await.clone();
    let report: Value = serde_json::from_slice(
        &tokio::fs::read(
            fixture
                .directory
                .path()
                .join("fingerprint-scan-latest.json"),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    fixture.cleanup().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(
        result["result"]["counts"],
        json!({"consistent":2,"repaired":0,"unknown":0}),
        "{result}"
    );
    assert_eq!(report["remaining"], 0, "{report}");
    assert_eq!(
        requests,
        vec![
            ("9".into(), 0),
            ("9".into(), 1000),
            ("3".into(), 0),
            ("3".into(), 1000)
        ],
        "each complete directory listing should serve both episodes"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn repairs_recheck_live_cloud_for_stale_sha_and_pending_assets() {
    for pending in [false, true] {
        let fixture = Fixture::new(&["First"], false).await;
        if pending {
            sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,'{0,fingerprint_pending}','true')")
                .execute(&fixture.state.db).await.unwrap();
        } else {
            sqlx::query("UPDATE media_metadata SET file_sha1_json='[\"OLD-SHA\"]'")
                .execute(&fixture.state.db)
                .await
                .unwrap();
        }
        let before = fixture.metadata().await;
        *fixture.mock.hold_emby_read.lock().await = Some(2);
        let task = fixture.start().await;
        fixture.held().await;
        // The first listing is already cached; the write verifier must see this replacement.
        fixture.mock.directories.lock().await.get_mut("3").unwrap()[0]["sha1"] =
            json!("REPLACED-SHA");
        fixture.mock.release.add_permits(1);
        let result = fixture.finish(&task).await;
        let after = fixture.metadata().await;
        let requests = fixture.mock.requests.lock().await.clone();
        let progress = db::setting(&fixture.state.db, "fingerprint_scan_progress")
            .await
            .unwrap()
            .unwrap();
        let cache_sha: String =
            sqlx::query_scalar("SELECT sha1 FROM p115_filesystem_cache WHERE id='101'")
                .fetch_one(&fixture.state.db)
                .await
                .unwrap();
        fixture.cleanup().await;
        assert_eq!(
            result["state"], "completed_with_warnings",
            "pending={pending}: {result}"
        );
        assert_eq!(
            result["result"]["counts"],
            json!({"consistent":0,"repaired":0,"unknown":1}),
            "pending={pending}: {result}"
        );
        assert_eq!(
            requests,
            vec![
                ("9".into(), 0),
                ("3".into(), 0),
                ("9".into(), 0),
                ("3".into(), 0)
            ]
        );
        assert_eq!(
            after, before,
            "a cloud replacement must not overwrite fingerprints"
        );
        assert_eq!(cache_sha, "First-SHA");
        assert!(
            progress["entries"]["0"]["error"]
                .as_str()
                .unwrap()
                .contains("云端文件在检查后变化"),
            "{progress}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn pause_resume_refreshes_cached_directories_and_child_ids() {
    let fixture = Fixture::new(&["First", "Second"], false).await;
    *fixture.mock.hold_emby_read.lock().await = Some(3);
    let task = fixture.start().await;
    fixture.held().await;
    let second = fixture.mock.emby_reads.lock().await.last().unwrap().clone();
    fixture
        .state
        .jobs
        .control(Some(&task), "pause")
        .await
        .unwrap();
    fixture.mock.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if fixture.state.jobs.result(&task).await.unwrap()["state"] == "paused" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    {
        let mut directories = fixture.mock.directories.lock().await;
        directories.get_mut("9").unwrap()[0]["cid"] = json!("4");
        let mut files = directories["3"].clone();
        files
            .iter_mut()
            .find(|file| file["name"] == format!("{second}.mkv"))
            .unwrap()["sha1"] = json!("AFTER-PAUSE-SHA");
        directories.insert("4".into(), files);
    }
    fixture
        .state
        .jobs
        .control(Some(&task), "resume")
        .await
        .unwrap();
    let result = fixture.finish(&task).await;
    let requests = fixture.mock.requests.lock().await.clone();
    let rows = fixture.metadata().await;
    fixture.cleanup().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(
        result["result"]["counts"],
        json!({"consistent":1,"repaired":1,"unknown":0}),
        "{result}"
    );
    assert_eq!(
        requests,
        vec![
            ("9".into(), 0),
            ("3".into(), 0),
            ("9".into(), 0),
            ("4".into(), 0),
            ("9".into(), 0),
            ("4".into(), 0)
        ]
    );
    assert_eq!(
        rows.iter().find(|row| row["id"] == second).unwrap()["sha1"],
        json!(["AFTER-PAUSE-SHA"])
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn repairs_preserve_fingerprints_when_emby_changes_during_either_recheck() {
    for read in [2, 3] {
        let fixture = Fixture::new(&["First"], false).await;
        sqlx::query("UPDATE media_metadata SET file_sha1_json='[\"OLD-SHA\"]'")
            .execute(&fixture.state.db)
            .await
            .unwrap();
        let before = fixture.metadata().await;
        *fixture.mock.hold_emby_read.lock().await = Some(read);
        let task = fixture.start().await;
        fixture.held().await;
        fixture.mock.sizes.lock().await.insert("First".into(), 200);
        fixture.mock.release.add_permits(1);
        let result = fixture.finish(&task).await;
        let after = fixture.metadata().await;
        let requests = fixture.mock.requests.lock().await.clone();
        let progress = db::setting(&fixture.state.db, "fingerprint_scan_progress")
            .await
            .unwrap()
            .unwrap();
        fixture.cleanup().await;
        assert_eq!(
            result["state"], "completed_with_warnings",
            "read={read}: {result}"
        );
        assert_eq!(result["result"]["counts"]["unknown"], 1, "{result}");
        assert_eq!(
            after, before,
            "Emby changes during recheck {read} must prevent writes"
        );
        assert_eq!(requests.len(), if read == 2 { 2 } else { 4 });
        assert!(
            progress["entries"]["0"]["error"]
                .as_str()
                .unwrap()
                .contains("Emby媒体信息在检查期间变化"),
            "{progress}"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn cancelling_with_primed_cache_keeps_next_episode_pending_without_search() {
    let fixture = Fixture::new(&["First", "Second"], false).await;
    let before = fixture.metadata().await;
    *fixture.mock.hold_emby_read.lock().await = Some(3);
    let task = fixture.start().await;
    fixture.held().await;
    fixture
        .state
        .jobs
        .control(Some(&task), "stop")
        .await
        .unwrap();
    fixture.mock.release.add_permits(1);
    let result = fixture.finish(&task).await;
    let after = fixture.metadata().await;
    let requests = fixture.mock.requests.lock().await.clone();
    let searches = *fixture.mock.searches.lock().await;
    let progress = db::setting(&fixture.state.db, "fingerprint_scan_progress")
        .await
        .unwrap()
        .unwrap();
    fixture.cleanup().await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(progress["state"], "interrupted", "{progress}");
    assert_eq!(progress["pending"], json!([1]), "{progress}");
    assert_eq!(progress["counts"]["consistent"], 1, "{progress}");
    assert_eq!(requests, vec![("9".into(), 0), ("3".into(), 0)]);
    assert_eq!(searches, 0);
    assert_eq!(after, before);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn audit_and_explicit_revalidation_do_not_search_for_a_missing_configured_file() {
    for audit in [true, false] {
        let fixture = Fixture::new(&["First"], false).await;
        let before = fixture.metadata().await;
        fixture
            .mock
            .directories
            .lock()
            .await
            .get_mut("3")
            .unwrap()
            .clear();
        let task = if audit {
            fixture.start().await
        } else {
            let work = fixture.state.clone();
            fixture
                .state
                .jobs
                .submit("重新校验指纹", move |context| async move {
                    emby_toolkit::revalidation::ids(&work, &["First".into()], &context).await
                })
                .await
                .unwrap()
        };
        let result = fixture.finish(&task).await;
        let after = fixture.metadata().await;
        let requests = fixture.mock.requests.lock().await.clone();
        let searches = *fixture.mock.searches.lock().await;
        fixture.cleanup().await;
        assert_eq!(
            searches, 0,
            "a missing configured path must not search other files: {result}"
        );
        assert_eq!(requests, vec![("9".into(), 0), ("3".into(), 0)]);
        if audit {
            assert_eq!(result["result"]["counts"]["unknown"], 1, "{result}");
            assert_eq!(after, before);
        } else {
            assert_eq!(result["result"]["failed"], 1, "{result}");
            assert_eq!(after[0]["sha1"], before[0]["sha1"]);
            assert_eq!(after[0]["pc"], before[0]["pc"]);
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn failed_directory_reads_are_not_cached_for_the_next_episode() {
    let fixture = Fixture::new(&["First", "Second"], false).await;
    fixture.mock.missing_roots.store(1, Ordering::SeqCst);
    let task = fixture.start().await;
    let result = fixture.finish(&task).await;
    let requests = fixture.mock.requests.lock().await.clone();
    fixture.cleanup().await;
    assert_eq!(
        result["result"]["counts"],
        json!({"consistent":1,"repaired":0,"unknown":1}),
        "{result}"
    );
    assert_eq!(
        requests,
        vec![("9".into(), 0), ("9".into(), 0), ("3".into(), 0)]
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn audit_request_interval_is_shared_across_successive_runs() {
    let fixture = Fixture::new(&["First"], false).await;
    for _ in 0..2 {
        let task = fixture.start().await;
        assert_eq!(
            fixture.finish(&task).await["result"]["counts"]["consistent"],
            1
        );
    }
    let times = fixture.mock.request_times.lock().await.clone();
    fixture.cleanup().await;
    assert_eq!(times.len(), 4);
    assert!(
        times
            .windows(2)
            .all(|v| v[1].duration_since(v[0]) >= Duration::from_secs(5)),
        "scan retries and new runs must retain the original global request interval"
    );
}
