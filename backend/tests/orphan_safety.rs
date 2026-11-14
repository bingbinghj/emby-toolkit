use axum::{
    Json, Router,
    extract::{Form, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, db, jobs::JobSpec, orphans, p115};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Cloud {
    mode: Arc<Mutex<u8>>,
    moves: Arc<Mutex<Vec<HashMap<String, String>>>>,
    files: Arc<Mutex<Vec<Value>>>,
    partial: Arc<AtomicBool>,
    partial_sidecar: Arc<AtomicBool>,
    hold: Arc<AtomicBool>,
    moved: Arc<Notify>,
    released: Arc<Notify>,
    plan_before_move: Arc<AtomicBool>,
    conflict_on_last_info: Arc<AtomicBool>,
    target: Arc<Mutex<String>>,
    db: sqlx::PgPool,
}

fn hits() -> Vec<Value> {
    vec![
        json!({"id":"101","name":"First.mkv","parent_id":"99","fc":"1"}),
        json!({"id":"102","name":"First.srt","parent_id":"99","fc":"1"}),
        json!({"id":"103","name":"First.zh.ass","parent_id":"99","fc":"1"}),
        json!({"id":"104","name":"First2.mkv","parent_id":"99","fc":"1"}),
        json!({"id":"105","name":"FirstExtra.srt","parent_id":"99","fc":"1"}),
        json!({"id":"106","name":"First.mp4","parent_id":"99","fc":"1"}),
        json!({"id":"107","name":"First.nfo","parent_id":"99","fc":"0"}),
        json!({"id":"108","name":"First.jpg","parent_id":"98","fc":"1"}),
        json!({"id":"109","name":"First.png","parent_id":"99","fc":"1"}),
        json!({"id":"110","name":"First-mediainfo.json","parent_id":"99","fc":"1"}),
        json!({"id":"111","name":"First2-mediainfo.json","parent_id":"99","fc":"1"}),
    ]
    .into_iter()
    .map(|mut file| {
        let id = file["id"].as_str().unwrap().to_string();
        file["sha1"] = json!(format!("SHA-{id}"));
        file["size"] = json!(100);
        file["pick_code"] = json!(format!("pc-{id}"));
        file
    })
    .collect()
}

struct Fixture {
    state: AppState,
    cloud: Cloud,
    root: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let root = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("orphan_safety_{}", uuid::Uuid::new_v4().simple());
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
        let path = std::env::temp_dir();
        std::fs::create_dir_all(&path).unwrap();
        let directory = tempfile::tempdir_in(&path).unwrap();
        let mut state = AppState::new(pool.clone(), directory.path().into())
            .await
            .unwrap();
        let cloud = Cloud {
            mode: Arc::new(Mutex::new(1)),
            moves: Arc::default(),
            files: Arc::new(Mutex::new(hits())),
            partial: Arc::default(),
            partial_sidecar: Arc::default(),
            hold: Arc::default(),
            moved: Arc::default(),
            released: Arc::default(),
            plan_before_move: Arc::default(),
            conflict_on_last_info: Arc::default(),
            target: Arc::new(Mutex::new("3".into())),
            db: pool.clone(),
        };
        let fake = Router::new()
            .route("/files", get(|State(cloud): State<Cloud>, Query(query): Query<HashMap<String, String>>| async move {
                let data = if query["cid"] == "9" {
                    vec![json!({"id":cloud.target.lock().await.clone(),"name":"Film","fc":"0"})]
                } else {
                    cloud.files.lock().await.iter().filter(|file| file["parent_id"] == query["cid"])
                        .cloned().collect()
                };
                Json(json!({"state":true,"count":data.len(),"data":data}))
            }))
            .route("/files/search", get(|State(cloud): State<Cloud>| async move {
                let data = cloud.files.lock().await.clone();
                Json(json!({"state":true,"count":if *cloud.mode.lock().await == 2 {200}else{data.len()},"data":data}))
            }))
            .route("/files/file", get(|State(cloud): State<Cloud>, Query(query): Query<HashMap<String, String>>| async move {
                if query["file_id"] != "99" {
                    if query["file_id"] == "110" && cloud.conflict_on_last_info.swap(false,Ordering::SeqCst) {
                        cloud.files.lock().await.push(json!({"id":"201","name":"First.srt","parent_id":"3","fc":"1"}));
                    }
                    let file = cloud.files.lock().await.iter().find(|file| file["id"] == query["file_id"]).cloned();
                    return (StatusCode::OK, Json(file.map_or(json!({"state":false,"errno":20018}), |file| json!({"state":true,"data":file}))));
                }
                match *cloud.mode.lock().await {
                    0 => (StatusCode::OK, Json(json!({"state":false,"errno":401,"message":"凭证无效"}))),
                    3 => (StatusCode::OK, Json(json!({"state":false,"message":"暂时无法查询目录"}))),
                    4 => (StatusCode::BAD_GATEWAY, Json(json!({}))),
                    5 => (StatusCode::OK, Json(json!({"state":true,"data":{"id":"99"}}))),
                    _ => (StatusCode::OK, Json(json!({"state":false,"errno":20018,"message":"文件不存在或已删除"}))),
                }
            }))
            .route("/files/move", post(|State(cloud): State<Cloud>, Form(query): Form<HashMap<String, String>>| async move {
                let planned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_job_steps WHERE step_key LIKE 'plans:orphan-move:%' AND jsonb_array_length(value_json->'files')=5)")
                    .fetch_one(&cloud.db).await.unwrap();
                cloud.plan_before_move.store(planned, Ordering::SeqCst);
                let ids = query.iter().filter(|(key,_)| key.starts_with("fid[")).map(|(_,id)|id.clone()).collect::<BTreeSet<_>>();
                for file in cloud.files.lock().await.iter_mut() {
                    if ids.contains(file["id"].as_str().unwrap()) && (!cloud.partial.load(Ordering::SeqCst)
                        || file["id"] == "101" || (cloud.partial_sidecar.load(Ordering::SeqCst) && file["id"] == "102")) {
                        file["parent_id"] = json!(query["pid"]);
                    }
                }
                cloud.moves.lock().await.push(query);
                if cloud.hold.load(Ordering::SeqCst) {
                    cloud.moved.notify_one();
                    cloud.released.notified().await;
                }
                Json(json!({"state":true}))
            })).with_state(cloud.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
        let base = format!("http://{address}");
        state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            webapi: base.clone(),
            ..Default::default()
        }));
        db::save_setting(
            &pool,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
        )
        .await
        .unwrap();
        state
            .save_config(
                &json!({"local_strm_root":directory.path(),"p115_fingerprint_media_root_cid":"9",
                    "emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin"}),
            )
            .await
            .unwrap();
        let path = directory.path().join("Film/First.strm");
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('First','Movie',true,$1,$2,$3,$4)")
            .bind(json!(["First"])).bind(json!([{"emby_item_id":"First","path":path}])).bind(json!(["SHA"])).bind(json!(["101"])).execute(&pool).await.unwrap();
        Self {
            state,
            cloud,
            root,
            schema,
            directory,
            server,
        }
    }
    async fn submit(&self) -> String {
        let work = self.state.clone();
        self.state
            .jobs
            .submit_persistent(
                "孤儿修复验证",
                JobSpec::new("orphans", json!({"id":"First"})),
                move |context| async move { orphans::repair(&work, "First", &context).await },
            )
            .await
            .unwrap()
    }
    async fn finish(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let result = self.state.jobs.result(id).await.unwrap();
                if matches!(
                    result["state"].as_str(),
                    Some("completed" | "completed_with_warnings" | "failed" | "cancelled")
                ) {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn run(&self) -> Value {
        let id = self.submit().await;
        self.finish(&id).await["result"].clone()
    }
    async fn interrupt_partial(&self) -> String {
        self.cloud.partial.store(true, Ordering::SeqCst);
        self.cloud.hold.store(true, Ordering::SeqCst);
        let id = self.submit().await;
        tokio::time::timeout(Duration::from_secs(30), self.cloud.moved.notified())
            .await
            .unwrap();
        let jobs = self.state.jobs.clone();
        let closing = tokio::spawn(async move { jobs.shutdown().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.jobs.status().await["state"] != "interrupted" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        self.cloud.released.notify_one();
        tokio::time::timeout(Duration::from_secs(15), closing)
            .await
            .unwrap()
            .unwrap();
        self.cloud.hold.store(false, Ordering::SeqCst);
        self.cloud.partial.store(false, Ordering::SeqCst);
        id
    }
    async fn restart(&mut self, id: &str) -> Value {
        let api = self.state.p115.clone();
        self.state = AppState::new(self.state.db.clone(), self.directory.path().into())
            .await
            .unwrap();
        self.state.p115 = api;
        let state = self.state.clone();
        let jobs = self.state.jobs.clone();
        let recovery =
            tokio::spawn(async move { jobs.resume_pending(state, CancellationToken::new()).await });
        let result = self.finish(id).await;
        recovery.await.unwrap().unwrap();
        result
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
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_repair_requires_confirmed_missing_parent_and_only_moves_exact_sidecars() {
    let f = Fixture::new().await;
    for mode in [0, 2, 3, 4, 5, 1] {
        *f.cloud.mode.lock().await = mode;
        f.cloud.moves.lock().await.clear();
        *f.cloud.files.lock().await = hits();
        let result = f.run().await;
        let moves = f.cloud.moves.lock().await;
        if mode == 5 {
            assert_eq!(result["skipped_mismatch"], 1, "{result}");
            assert!(
                moves.is_empty(),
                "an existing parent must never move: {moves:?}"
            );
        } else if mode != 1 {
            assert_eq!(result["failed"], 1, "{result}");
            assert!(
                moves.is_empty(),
                "unconfirmed parent or incomplete hits must never move: {moves:?}"
            );
        } else {
            assert_eq!(result["repaired_videos"], 1, "{result}");
            assert_eq!(result["repaired_sidecars"], 4, "{result}");
            assert_eq!(moves.len(), 1);
            assert_eq!(
                moves[0]
                    .iter()
                    .filter(|(key, _)| key.starts_with("fid["))
                    .map(|(_, id)| id.as_str())
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["101", "102", "103", "109", "110"])
            );
            assert_eq!(moves[0]["pid"], "3");
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_partial_move_response_cannot_claim_all_sidecars_completed() {
    let f = Fixture::new().await;
    f.cloud.partial.store(true, Ordering::SeqCst);
    let result = f.run().await;
    let video = f
        .cloud
        .files
        .lock()
        .await
        .iter()
        .find(|file| file["id"] == "101")
        .unwrap()["parent_id"]
        .clone();
    f.close().await;
    assert_eq!(video, "3");
    assert_eq!(
        result["failed"], 1,
        "a moved video does not prove its sidecars moved: {result}"
    );
    assert_eq!(result["repaired_videos"], 0);
    assert_eq!(result["repaired_sidecars"], 0);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_new_destination_conflict_during_file_readback_stops_before_moving() {
    let f = Fixture::new().await;
    f.cloud.conflict_on_last_info.store(true, Ordering::SeqCst);
    let result = f.run().await;
    let moves = f.cloud.moves.lock().await.len();
    f.close().await;
    assert_eq!(result["failed"], 1, "{result}");
    assert_eq!(
        moves, 0,
        "a target conflict discovered during preflight must not allow any move"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_restart_continues_original_sidecars_without_moving_video_twice() {
    for sidecar_already_moved in [false, true] {
        let mut f = Fixture::new().await;
        f.cloud
            .partial_sidecar
            .store(sidecar_already_moved, Ordering::SeqCst);
        let id = f.interrupt_partial().await;
        let planned = f.cloud.plan_before_move.load(Ordering::SeqCst);
        let result = f.restart(&id).await;
        let moves = f.cloud.moves.lock().await.clone();
        let restored: Vec<_> = f
            .cloud
            .files
            .lock()
            .await
            .iter()
            .filter(|file| file["parent_id"] == "3")
            .map(|file| file["id"].as_str().unwrap().to_string())
            .collect();
        f.close().await;
        assert!(
            planned,
            "the entire move intent must exist before the first cloud mutation"
        );
        assert_eq!(result["state"], "completed", "{result}");
        assert_eq!(result["result"]["repaired_videos"], 1);
        assert_eq!(result["result"]["repaired_sidecars"], 4);
        assert_eq!(moves.len(), 2);
        assert_eq!(
            moves[1]
                .iter()
                .filter(|(key, _)| key.starts_with("fid["))
                .map(|(_, id)| id.as_str())
                .collect::<BTreeSet<_>>(),
            if sidecar_already_moved {
                BTreeSet::from(["103", "109", "110"])
            } else {
                BTreeSet::from(["102", "103", "109", "110"])
            }
        );
        assert_eq!(
            restored.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "101".into(),
                "102".into(),
                "103".into(),
                "109".into(),
                "110".into()
            ])
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_restart_rejects_new_name_conflicts_and_changed_file_identities() {
    for mode in ["name", "sha", "conflict"] {
        let mut f = Fixture::new().await;
        let id = f.interrupt_partial().await;
        {
            let mut files = f.cloud.files.lock().await;
            match mode {
                "name" => {
                    files.iter_mut().find(|file| file["id"] == "102").unwrap()["name"] =
                        json!("Renamed.srt")
                }
                "sha" => {
                    files.iter_mut().find(|file| file["id"] == "102").unwrap()["sha1"] =
                        json!("DIFFERENT")
                }
                _ => files.push(json!({"id":"201","name":"First.srt","parent_id":"3","fc":"1"})),
            }
        }
        let result = f.restart(&id).await;
        let moves = f.cloud.moves.lock().await.len();
        f.close().await;
        assert_eq!(result["state"], "failed", "{mode}: {result}");
        assert_eq!(
            moves, 1,
            "{mode}: recovery must not move any more files after identity changes"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_explicit_new_job_reuses_only_the_original_unfinished_move_plan() {
    let f = Fixture::new().await;
    f.cloud.partial.store(true, Ordering::SeqCst);
    f.cloud.partial_sidecar.store(true, Ordering::SeqCst);
    let old = f.submit().await;
    assert_eq!(f.finish(&old).await["state"], "failed");
    f.cloud.partial.store(false, Ordering::SeqCst);
    let new = f.submit().await;
    let result = f.finish(&new).await;
    let again = f.submit().await;
    let unchanged = f.finish(&again).await;
    let old_state = f.state.jobs.result(&old).await.unwrap()["state"].clone();
    let moves = f.cloud.moves.lock().await.clone();
    f.close().await;
    assert_ne!(old, new);
    assert_eq!(
        old_state, "failed",
        "the original terminal job is not revived"
    );
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["result"]["repaired_sidecars"], 4);
    assert_eq!(
        unchanged["result"]["already_correct"], 1,
        "a completed continuation resolves the old data-only plan"
    );
    assert_eq!(unchanged["result"]["repaired_sidecars"], 0);
    assert_eq!(moves.len(), 2);
    assert_eq!(
        moves[1]
            .iter()
            .filter(|(key, _)| key.starts_with("fid["))
            .map(|(_, id)| id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["103", "109", "110"])
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL in DATABASE_URL"]
async fn orphan_new_job_rejects_changed_connection_scope_identity_and_ambiguous_plans() {
    let f = Fixture::new().await;
    f.cloud.partial.store(true, Ordering::SeqCst);
    let old = f.submit().await;
    assert_eq!(f.finish(&old).await["state"], "failed");
    f.cloud.partial.store(false, Ordering::SeqCst);
    let originals = f.cloud.files.lock().await.clone();
    for mode in ["connection", "root", "target", "name", "sha", "ambiguous"] {
        *f.cloud.files.lock().await = originals.clone();
        *f.cloud.target.lock().await = "3".into();
        f.state
            .save_config(&json!({"p115_fingerprint_media_root_cid":"9"}))
            .await
            .unwrap();
        db::save_setting(
            &f.state.db,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1_fixture; CID=fixture; SEID=fixture"}),
        )
        .await
        .unwrap();
        match mode {
            "connection" => db::save_setting(
                &f.state.db,
                "p115_auth_tokens",
                &json!({"cookie":"UID=2_A1_fixture; CID=fixture; SEID=fixture"}),
            )
            .await
            .unwrap(),
            "root" => f
                .state
                .save_config(&json!({"p115_fingerprint_media_root_cid":"10"}))
                .await
                .unwrap(),
            "target" => *f.cloud.target.lock().await = "4".into(),
            "name" => {
                f.cloud
                    .files
                    .lock()
                    .await
                    .iter_mut()
                    .find(|file| file["id"] == "101")
                    .unwrap()["name"] = json!("Renamed.mkv")
            }
            "sha" => {
                f.cloud
                    .files
                    .lock()
                    .await
                    .iter_mut()
                    .find(|file| file["id"] == "101")
                    .unwrap()["sha1"] = json!("DIFFERENT")
            }
            _ => {
                let mut plan: Value=sqlx::query_scalar("SELECT value_json FROM durable_job_steps WHERE job_id=$1 AND step_key LIKE 'plans:orphan-move:%'").bind(&old).fetch_one(&f.state.db).await.unwrap();
                plan["source_parent"] = json!("98");
                sqlx::query("INSERT INTO durable_job_steps(job_id,step_key,value_json) VALUES($1,'plans:ambiguous-orphan-move',$2)").bind(&old).bind(plan).execute(&f.state.db).await.unwrap();
            }
        }
        let id = f.submit().await;
        let result = f.finish(&id).await;
        assert_eq!(result["state"], "failed", "{mode}: {result}");
        assert_eq!(
            f.cloud.moves.lock().await.len(),
            1,
            "{mode}: no additional FID may move"
        );
        assert_eq!(f.state.jobs.result(&old).await.unwrap()["state"], "failed");
    }
    f.close().await;
}
