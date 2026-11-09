use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, db, media_extra, p115, strm};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
struct Host {
    root: PathBuf,
    items: Arc<Mutex<BTreeMap<String, Value>>>,
    configuration: Arc<Mutex<Value>>,
    writes: Arc<Mutex<Vec<Value>>>,
    events: Arc<Mutex<Vec<String>>>,
    log: Arc<Mutex<Vec<String>>>,
    accepted: Arc<AtomicBool>,
    complete: Arc<AtomicBool>,
    release: Arc<Notify>,
    mode: Arc<Mutex<String>>,
}
async fn finish(h: Host) {
    h.release.notified().await;
    for name in ["v2.strm", "v2-mediainfo.json"] {
        tokio::fs::remove_file(h.root.join(name)).await.unwrap();
    }
    h.log
        .lock()
        .await
        .push("Info ProviderManager: RefreshItem Complete: folder item".into());
    h.complete.store(true, Ordering::SeqCst);
}
async fn api(State(h): State<Host>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if request.method() == axum::http::Method::GET {
        if let Some(id) = path.strip_prefix("/Users/user/Items/") {
            return h
                .items
                .lock()
                .await
                .get(id)
                .cloned()
                .map(Json)
                .map(IntoResponse::into_response)
                .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response());
        }
        if path == "/Items" {
            let items = if query
                .get("Path")
                .is_some_and(|p| p == h.root.to_str().unwrap())
            {
                vec![json!({"Id":"folder","Path":h.root,"IsFolder":true})]
            } else {
                h.items
                    .lock()
                    .await
                    .values()
                    .filter(|item| {
                        query
                            .get("Ids")
                            .is_some_and(|ids| ids.split(',').any(|id| item["Id"] == id))
                    })
                    .cloned()
                    .collect()
            };
            return Json(json!({"TotalRecordCount":items.len(),"Items":items})).into_response();
        }
        if path == "/web/ConfigurationPages" {
            return Json(json!([{"Name":"MediaInfoExtractPageView","Href":"/UI/View?PageId=fixture:MediaInfoExtractPageView"}])).into_response();
        }
        if path == "/UI/View" {
            return Json(
                json!({"EditObjectContainer":{"Object":h.configuration.lock().await.clone()}}),
            )
            .into_response();
        }
        if path == "/System/Logs/embyserver.txt/Lines" {
            if h.accepted.load(Ordering::SeqCst) && *h.mode.lock().await == "log-error" {
                return StatusCode::BAD_REQUEST.into_response();
            }
            let lines = h.log.lock().await.clone();
            return Json(json!({"TotalRecordCount":lines.len(),"Items":lines.iter().rev().cloned().collect::<Vec<_>>()})).into_response();
        }
        if path == "/files/file" {
            return Json(json!({"state":true,"data":[{"id":"100","sha1":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","pick_code":"pc0000000001","size":1000,"is_dir":false,"name":"v1.mkv"}]})).into_response();
        }
    } else if request.method() == axum::http::Method::POST {
        if path == "/UI/Command" {
            let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let config: Value = serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
            h.writes.lock().await.push(config.clone());
            *h.configuration.lock().await = config;
            return Json(json!({})).into_response();
        }
        if path == "/Videos/v1/AlternateSources/Delete" {
            h.events.lock().await.push("ungroup".into());
            return StatusCode::NO_CONTENT.into_response();
        }
        if path == "/rb/delete" {
            h.events.lock().await.push("cloud-delete".into());
            return Json(json!({"state":true})).into_response();
        }
        if path == "/Items/folder/Refresh" {
            assert_eq!(query["MetadataRefreshMode"], "Default");
            assert_eq!(query["Recursive"], "true");
            h.events.lock().await.push("scan".into());
            h.log
                .lock()
                .await
                .push("Info ProviderManager: RefreshItem Start: folder item".into());
            h.accepted.store(true, Ordering::SeqCst);
            tokio::spawn(finish(h));
            return StatusCode::NO_CONTENT.into_response();
        }
    } else if request.method() == axum::http::Method::DELETE && path == "/Items/v1" {
        h.events.lock().await.push("emby-delete".into());
        h.items.lock().await.remove("v1");
        for entry in std::fs::read_dir(&h.root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                tokio::fs::remove_file(path).await.unwrap();
            }
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    h.events
        .lock()
        .await
        .push(format!("unexpected:{}:{path}", request.method()));
    StatusCode::NOT_FOUND.into_response()
}
struct Fixture {
    state: AppState,
    host: Host,
    _directory: tempfile::TempDir,
    admin: sqlx::PgPool,
    schema: String,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let admin = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("delete_scan_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |conn, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&search).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("media");
        tokio::fs::create_dir(&root).await.unwrap();
        let host = Host {
            root: root.clone(),
            items: Arc::default(),
            configuration: Arc::new(Mutex::new(
                json!({"ExclusiveControlFeatures":"CatchAllAllow,ExtractOnFileChange,CustomFlag","PersistMediaInfoMode":"Default","LibraryScope":"original","MediaInfoJsonRootFolder":""}),
            )),
            writes: Arc::default(),
            events: Arc::default(),
            log: Arc::default(),
            accepted: Arc::default(),
            complete: Arc::default(),
            release: Arc::default(),
            mode: Arc::new(Mutex::new(mode.into())),
        };
        for id in ["v1", "v2"] {
            let path = root.join(format!("{id}.strm"));
            tokio::fs::write(&path, format!("/media/Films/{id}.mkv"))
                .await
                .unwrap();
            tokio::fs::write(
                root.join(format!("{id}-mediainfo.json")),
                format!("{id}-original-json"),
            )
            .await
            .unwrap();
            host.items.lock().await.insert(
                id.into(),
                json!({"Id":id,"Type":"Movie","Name":"Fixture","Path":path,"PartCount":1}),
            );
        }
        tokio::fs::write(root.join("v2.nfo"), "original nfo")
            .await
            .unwrap();
        tokio::fs::write(root.join("poster.jpg"), b"shared image")
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(api).with_state(host.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        state.p115 = Arc::new(p115::Service::with_endpoints(p115::Endpoints {
            webapi: endpoint.clone(),
            openapi: endpoint.clone(),
            qrcode: endpoint.clone(),
            passport: endpoint.clone(),
            profile: endpoint.clone(),
        }));
        state.save_config(&json!({"emby_server_url":endpoint,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":root,"p115_api_priority":"cookie"})).await.unwrap();
        db::save_setting(
            &state.db,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1;CID=fixture;SEID=fixture"}),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('fixture','Movie',true,$1,$2,$3,$4)")
            .bind(json!(["v1","v2"]))
            .bind(json!([{"emby_item_id":"v1","path":root.join("v1.strm"),"size_bytes":1000},{"emby_item_id":"v2","path":root.join("v2.strm"),"size_bytes":1000}]))
            .bind(json!(["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"]))
            .bind(json!(["pc0000000001","pc0000000002"])).execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,local_path,pick_code,sha1,size) VALUES('100','1','v1.mkv',$1,'pc0000000001','AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',1000)")
            .bind(root.join("v1.strm").to_string_lossy().to_string()).execute(&state.db).await.unwrap();
        Self {
            state,
            host,
            _directory: directory,
            admin,
            schema,
            server,
        }
    }
    async fn start(&self) -> String {
        let state = self.state.clone();
        self.state
            .jobs
            .submit("delete scan fixture", move |context| async move {
                media_extra::delete_version(&state, "v1", &context, &[Duration::from_millis(1)])
                    .await
            })
            .await
            .unwrap()
    }
    async fn wait_accepted(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.host.accepted.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn wait_job(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let value = self.state.jobs.result(id).await.unwrap();
                if value["is_running"] == false {
                    return value;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn wait_scan(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.host.complete.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn check_files(&self) {
        assert!(!self.host.root.join("v1.strm").exists());
        assert!(!self.host.root.join("v1-mediainfo.json").exists());
        assert_eq!(
            tokio::fs::read_to_string(self.host.root.join("v2.strm"))
                .await
                .unwrap(),
            "/media/Films/v2.mkv"
        );
        assert_eq!(
            tokio::fs::read_to_string(self.host.root.join("v2-mediainfo.json"))
                .await
                .unwrap(),
            "v2-original-json"
        );
        assert_eq!(
            tokio::fs::read(self.host.root.join("poster.jpg"))
                .await
                .unwrap(),
            b"shared image"
        );
        let events = self.host.events.lock().await.clone();
        assert!(
            !events.iter().any(|e| e.starts_with("unexpected:")),
            "{events:?}"
        );
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
#[ignore = "isolated PostgreSQL and localhost substitutes only"]
async fn delete_keeps_original_allow_flags_and_waits_for_actual_scan_completion() {
    let f = Fixture::new("hold").await;
    let task = f.start().await;
    f.wait_accepted().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        f.state.jobs.result(&task).await.unwrap()["is_running"],
        true,
        "a one-millisecond protection delay must not substitute for scan completion"
    );
    assert_eq!(
        f.host.configuration.lock().await["ExclusiveControlFeatures"],
        "CatchAllAllow,CustomFlag,IgnoreFileChange"
    );
    assert_eq!(f.host.writes.lock().await.len(), 1);
    f.host.release.notify_one();
    let result = f.wait_job(&task).await;
    assert_eq!(result["state"], "completed", "{result}");
    f.check_files().await;
    assert_eq!(
        f.host.configuration.lock().await["ExclusiveControlFeatures"],
        "CatchAllAllow,ExtractOnFileChange,CustomFlag"
    );
    assert_eq!(
        f.host
            .events
            .lock()
            .await
            .iter()
            .filter(|e| *e == "scan")
            .count(),
        1
    );
    assert!(
        !std::fs::read_dir(f.state.data_dir.join("rename-recovery"))
            .unwrap()
            .any(|e| e.unwrap().path().join("pending-scan.json").exists())
    );
    f.close().await;
}

async fn interrupted_delete(mode: &str, cancel: bool) {
    let f = Fixture::new(mode).await;
    let task = f.start().await;
    f.wait_accepted().await;
    if cancel {
        f.state.jobs.control(Some(&task), "stop").await.unwrap();
    }
    let result = f.wait_job(&task).await;
    assert_ne!(result["state"], "completed", "{result}");
    assert_eq!(f.host.writes.lock().await.len(), 1);
    assert_eq!(
        f.host.configuration.lock().await["ExclusiveControlFeatures"],
        "CatchAllAllow,CustomFlag,IgnoreFileChange"
    );
    let recovery = std::fs::read_dir(f.state.data_dir.join("rename-recovery"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(recovery.join("pending-scan.json").exists());
    assert!(recovery.join("version-delete-snapshot.json").exists());
    let blocked = strm::with_settings(&f.state, "capture", || async { Ok(()) }).await;
    assert!(blocked.is_err());
    assert_eq!(f.host.writes.lock().await.len(), 1);
    *f.host.mode.lock().await = "hold".into();
    tokio::fs::write(f.host.root.join("v2.nfo"), "user edit after cancellation")
        .await
        .unwrap();
    f.host.release.notify_one();
    f.wait_scan().await;
    assert!(
        !f.host.root.join("v2.strm").exists(),
        "late external deletion must really be simulated"
    );
    assert!(
        strm::with_settings(&f.state, "ignore_file_change", || async { Ok(()) })
            .await
            .is_ok()
    );
    f.check_files().await;
    assert_eq!(
        tokio::fs::read_to_string(f.host.root.join("v2.nfo"))
            .await
            .unwrap(),
        "user edit after cancellation"
    );
    assert!(!recovery.join("pending-scan.json").exists());
    assert_eq!(
        f.host.configuration.lock().await["ExclusiveControlFeatures"],
        "CatchAllAllow,ExtractOnFileChange,CustomFlag"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "isolated PostgreSQL and localhost substitutes only"]
async fn accepted_delete_scan_cancellation_keeps_snapshot_and_settings_until_verified() {
    interrupted_delete("hold", true).await;
}

#[tokio::test]
#[ignore = "isolated PostgreSQL and localhost substitutes only"]
async fn accepted_delete_scan_log_failure_keeps_snapshot_and_recovers_missing_files() {
    interrupted_delete("log-error", false).await;
}
