use super::*;
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Default)]
struct Cloud {
    files: Arc<Mutex<Vec<Value>>>,
    requests: Arc<Mutex<Vec<(String, usize)>>>,
    incomplete: Arc<std::sync::atomic::AtomicBool>,
}

async fn files(
    State(cloud): State<Cloud>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let parent = query["cid"].clone();
    let offset = query["offset"].parse::<usize>().unwrap();
    cloud.requests.lock().await.push((parent.clone(), offset));
    let entries = match parent.as_str() {
        "0" | "1" => vec![json!({"cid":"10","name":"剧集","fc":"0"})],
        "10" => vec![json!({"cid":"20","name":"测试剧","fc":"0"})],
        "20" => vec![json!({"cid":"30","name":"Season 01","fc":"0"})],
        "30" => cloud.files.lock().await.clone(),
        _ => vec![],
    };
    let count = entries.len();
    let data = if offset > 0 && cloud.incomplete.load(std::sync::atomic::Ordering::SeqCst) {
        vec![]
    } else {
        entries
            .into_iter()
            .skip(offset)
            .take(1000)
            .collect::<Vec<_>>()
    };
    Json(json!({"state":true,"count":count,"data":data}))
}

fn path(episode: usize) -> String {
    format!("/cd2/剧集/测试剧/Season 01/episode-{episode:04}.mkv")
}

struct Fixture {
    state: AppState,
    cloud: Cloud,
    admin: sqlx::PgPool,
    schema: String,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(total: usize) -> Self {
        let url = std::env::var("DATABASE_URL").expect("requires disposable PostgreSQL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("ingest_directories_{}", uuid::Uuid::new_v4().simple());
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
        let mut state = AppState::new(pool, directory.path().into()).await.unwrap();
        let cloud = Cloud::default();
        *cloud.files.lock().await = (1..=total).map(|episode| json!({
            "id":episode.to_string(),"parent_id":"30","name":format!("episode-{episode:04}.mkv"),
            "pick_code":episode.to_string(),"sha1":"A".repeat(40),"size":1000,"fc":"1"
        })).collect();
        let router = Router::new()
            .route("/files", get(files))
            .route(
                "/Users/user/Items/1",
                get(|| async { Json(json!({"Id":"1","Type":"Episode","Path":path(1)})) }),
            )
            .with_state(cloud.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            qrcode: base.clone(),
            passport: base.clone(),
            openapi: base.clone(),
            webapi: base.clone(),
            profile: base.clone(),
        }));
        crate::db::save_setting(
            &state.db,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
        )
        .await
        .unwrap();
        state
            .save_config(
                &json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"user",
            "monitor_sha1_pc_search":true,"cd2_media_root":"/cd2","local_strm_root":"/strm",
            "p115_fingerprint_media_root_cid":"0","p115_api_priority":"cookie"}),
            )
            .await
            .unwrap();
        Self {
            state,
            cloud,
            admin,
            schema,
            server,
            _directory: directory,
        }
    }

    async fn resolve(&self, episode: usize, context: &JobContext) -> Value {
        resolve_ingest(&self.state, &path(episode), &path(episode), context)
            .await
            .unwrap()
            .0
    }

    async fn replace(&self) {
        self.cloud.files.lock().await[0]["sha1"] = json!("B".repeat(40));
    }

    async fn close(self) {
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
#[ignore = "requires disposable PostgreSQL; all 115 and Emby requests stay localhost"]
async fn ingestion_reuses_complete_directory_pages_for_siblings() {
    let fixture = Fixture::new(1001).await;
    let context = JobContext::for_recovery(None).await;
    assert_eq!(fixture.resolve(1, &context).await["id"], "1");
    assert_eq!(
        fixture.resolve(1001, &context.scoped("same-batch")).await["id"],
        "1001"
    );
    let requests = fixture.cloud.requests.lock().await.clone();
    assert_eq!(
        requests,
        vec![
            ("0".into(), 0),
            ("10".into(), 0),
            ("20".into(), 0),
            ("30".into(), 0),
            ("30".into(), 1000)
        ]
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all 115 and Emby requests stay localhost"]
async fn ingestion_verification_refreshes_reused_snapshot_before_publication() {
    let fixture = Fixture::new(2).await;
    let context = JobContext::for_recovery(None).await;
    let item = json!({"Id":"1","Type":"Episode","Path":path(1)});
    let original = crate::moviepilot_ingest::observe(&fixture.state, &item, &context)
        .await
        .unwrap();
    fixture.replace().await;
    assert_eq!(fixture.resolve(1, &context).await["sha1"], "A".repeat(40));
    let error = crate::moviepilot_ingest::unchanged_many(&fixture.state, &[original], None, &context)
        .await
        .unwrap_err();
    assert!(error.1.contains("再次变化"), "{}", error.1);
    assert_eq!(fixture.cloud.requests.lock().await.len(), 8);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all 115 and Emby requests stay localhost"]
async fn ingestion_incomplete_pages_are_not_reused() {
    let fixture = Fixture::new(1001).await;
    let context = JobContext::for_recovery(None).await;
    fixture
        .cloud
        .incomplete
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = resolve_ingest(&fixture.state, &path(1), &path(1), &context)
        .await
        .unwrap_err();
    assert_eq!(error.1, "115 目录结果不完整");
    fixture
        .cloud
        .incomplete
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(fixture.resolve(1001, &context).await["id"], "1001");
    assert_eq!(fixture.resolve(1, &context).await["id"], "1");
    assert_eq!(fixture.cloud.requests.lock().await.len(), 10);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all 115 and Emby requests stay localhost"]
async fn ingestion_new_tasks_and_changed_roots_get_current_directory_results() {
    let fixture = Fixture::new(2).await;
    let context = JobContext::for_recovery(None).await;
    assert_eq!(fixture.resolve(1, &context).await["sha1"], "A".repeat(40));
    fixture.replace().await;
    let new_task = JobContext::for_recovery(None).await;
    assert_eq!(fixture.resolve(1, &new_task).await["sha1"], "B".repeat(40));
    fixture.state.config.write().await["p115_fingerprint_media_root_cid"] = json!("1");
    assert_eq!(fixture.resolve(1, &context).await["sha1"], "B".repeat(40));
    assert_eq!(fixture.cloud.requests.lock().await.len(), 12);
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; all 115 and Emby requests stay localhost"]
async fn ingestion_pause_resume_discards_directory_snapshot() {
    let fixture = Fixture::new(2).await;
    let ready = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (work, reached, released) = (fixture.state.clone(), ready.clone(), release.clone());
    let task = fixture
        .state
        .jobs
        .submit("测试入库目录暂停恢复", move |context| async move {
            let first = resolve_ingest(&work, &path(1), &path(1), &context).await?.0;
            reached.notify_one();
            released.notified().await;
            let second = resolve_ingest(&work, &path(1), &path(1), &context).await?.0;
            Ok(json!({"before":first["sha1"],"after":second["sha1"]}))
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), ready.notified())
        .await
        .unwrap();
    fixture
        .state
        .jobs
        .control(Some(&task), "pause")
        .await
        .unwrap();
    fixture.replace().await;
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fixture.state.jobs.result(&task).await.unwrap()["state"] != "paused" {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture
        .state
        .jobs
        .control(Some(&task), "resume")
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let result = fixture.state.jobs.result(&task).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["before"], "A".repeat(40));
    assert_eq!(result["result"]["after"], "B".repeat(40));
    assert_eq!(fixture.cloud.requests.lock().await.len(), 8);
    fixture.close().await;
}
