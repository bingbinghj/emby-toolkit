use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use emby_toolkit::{AppState, db, p115, webhook};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Host {
    items: Arc<Mutex<BTreeMap<String, Value>>>,
    deleted: Arc<Mutex<Vec<String>>>,
    id_reads: Arc<AtomicUsize>,
    path: PathBuf,
    mode: &'static str,
}

async fn api(State(host): State<Host>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    if request.method() == axum::http::Method::GET && path == "/Items" {
        if let Some(ids) = query.get("Ids") {
            let read = host.id_reads.fetch_add(1, Ordering::SeqCst) + 1;
            if host.mode == "ids_error" {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if host.mode == "ids_malformed" {
                return Json(json!({"TotalRecordCount":0})).into_response();
            }
            if host.mode == "reappears_before_delete" && read >= 2 {
                host.items.lock().await.insert(
                    "old".into(),
                    json!({"Id":"old","Type":"Movie","Path":host.path}),
                );
            }
            let items = host
                .items
                .lock()
                .await
                .values()
                .filter(|item| ids.split(',').any(|id| item["Id"] == id))
                .cloned()
                .collect::<Vec<_>>();
            return Json(json!({"TotalRecordCount":items.len(),"Items":items})).into_response();
        }
        if let Some(path) = query.get("Path") {
            if host.mode == "paths_incomplete" {
                return Json(json!({"TotalRecordCount":2,"Items":[]})).into_response();
            }
            let items = host
                .items
                .lock()
                .await
                .values()
                .filter(|item| item["Path"] == *path)
                .cloned()
                .collect::<Vec<_>>();
            return Json(json!({"TotalRecordCount":items.len(),"Items":items})).into_response();
        }
    }
    if request.method() == axum::http::Method::POST && path == "/rb/delete" {
        let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
        let ids = url::form_urlencoded::parse(&bytes)
            .filter(|(key, _)| key.starts_with("fid"))
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>();
        assert!(!ids.is_empty(), "cloud delete must contain exact file IDs");
        host.deleted.lock().await.extend(ids);
        return Json(json!({"state":true})).into_response();
    }
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
    async fn new(mode: &'static str) -> Self {
        let database_url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&database_url).await.unwrap();
        let schema = format!("deep_delete_wash_{}", uuid::Uuid::new_v4().simple());
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
            .connect(&database_url)
            .await
            .unwrap();
        let directory = tempfile::Builder::new()
            .prefix("deep-delete-wash-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root = directory.path().join("media");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let path = root.join("same.strm");
        let peer_path = root.join("peer.strm");
        tokio::fs::write(&peer_path, "/cd2/peer.mkv").await.unwrap();
        let host = Host {
            items: Arc::new(Mutex::new(BTreeMap::from([(
                "peer".into(),
                json!({"Id":"peer","Type":"Movie","Path":peer_path}),
            )]))),
            deleted: Arc::default(),
            id_reads: Arc::default(),
            path: path.clone(),
            mode,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(api).with_state(host.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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
        state.save_config(&json!({"emby_server_url":endpoint,"emby_api_key":"fixture","emby_user_id":"user","local_strm_root":root,"p115_api_priority":"cookie","p115_enable_sync_delete":true})).await.unwrap();
        db::save_setting(
            &state.db,
            "p115_auth_tokens",
            &json!({"cookie":"UID=1_A1;CID=fixture;SEID=fixture"}),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES('101','Movie',true,$1,$2,$3,$4)")
            .bind(json!(["old","peer"]))
            .bind(json!([{"emby_item_id":"old","path":path,"size_bytes":1000},{"emby_item_id":"peer","path":peer_path,"size_bytes":1000}]))
            .bind(json!(["A".repeat(40),"C".repeat(40)]))
            .bind(json!(["pc-old","pc-peer"])).execute(&state.db).await.unwrap();
        for (id, name, pc, sha) in [
            ("10001", "same.mkv", "pc-old", "A"),
            ("20002", "new.mkv", "pc-new", "B"),
            ("30003", "peer.mkv", "pc-peer", "C"),
        ] {
            sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,local_path,pick_code,sha1,size) VALUES($1,'9',$2,$2,$3,$4,1000)")
                .bind(id).bind(name).bind(pc).bind(sha.repeat(40)).execute(&state.db).await.unwrap();
        }
        Self {
            state,
            host,
            _directory: directory,
            admin,
            schema,
            server,
        }
    }

    async fn run(&self) -> emby_toolkit::error::Result<Value> {
        webhook::deep_delete(
            &self.state,
            &json!({"Id":"old","Type":"Movie","Name":"洗版测试","Path":self.host.path}),
        )
        .await
    }

    async fn ids(&self) -> Value {
        sqlx::query_scalar("SELECT emby_item_ids_json FROM media_metadata WHERE tmdb_id='101' AND item_type='Movie'")
            .fetch_one(&self.state.db).await.unwrap()
    }

    async fn assert_preserved_cloud(&self) {
        assert!(self.host.deleted.lock().await.is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM p115_filesystem_cache")
            .fetch_one(&self.state.db)
            .await
            .unwrap();
        assert_eq!(count, 3);
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
async fn old_delete_notification_preserves_a_current_online_item() {
    let fixture = Fixture::new("normal").await;
    fixture.host.items.lock().await.insert(
        "old".into(),
        json!({"Id":"old","Type":"Movie","Path":fixture.host.path}),
    );
    fixture.run().await.unwrap();
    fixture.assert_preserved_cloud().await;
    assert_eq!(fixture.ids().await, json!(["old", "peer"]));
    fixture.close().await;
}

#[tokio::test]
async fn old_delete_notification_keeps_same_path_with_a_new_emby_id() {
    let fixture = Fixture::new("normal").await;
    fixture.host.items.lock().await.insert(
        "new".into(),
        json!({"Id":"new","Type":"Movie","Path":fixture.host.path}),
    );
    fixture.run().await.unwrap();
    fixture.assert_preserved_cloud().await;
    assert_eq!(fixture.ids().await, json!(["peer"]));
    fixture.close().await;
}

#[tokio::test]
async fn old_delete_notification_does_not_delete_a_migrated_new_pickcode_slot() {
    let fixture = Fixture::new("normal").await;
    sqlx::query("UPDATE media_metadata SET file_pickcode_json=$1,file_sha1_json=$2 WHERE tmdb_id='101' AND item_type='Movie'")
        .bind(json!(["pc-new","pc-peer"])).bind(json!(["B".repeat(40),"C".repeat(40)])).execute(&fixture.state.db).await.unwrap();
    fixture.host.items.lock().await.insert(
        "new".into(),
        json!({"Id":"new","Type":"Movie","Path":fixture.host.path}),
    );
    fixture.run().await.unwrap();
    fixture.assert_preserved_cloud().await;
    assert_eq!(fixture.ids().await, json!(["peer"]));
    fixture.close().await;
}

#[tokio::test]
async fn old_delete_notification_preserves_a_reappeared_strm_before_native_rescan() {
    let fixture = Fixture::new("normal").await;
    tokio::fs::write(&fixture.host.path, "/cd2/new.mkv")
        .await
        .unwrap();
    fixture.run().await.unwrap();
    fixture.assert_preserved_cloud().await;
    assert_eq!(
        tokio::fs::read_to_string(&fixture.host.path).await.unwrap(),
        "/cd2/new.mkv"
    );
    fixture.close().await;
}

#[tokio::test]
async fn old_delete_notification_rechecks_native_presence_before_cloud_and_database_removal() {
    let fixture = Fixture::new("reappears_before_delete").await;
    fixture.run().await.unwrap();
    fixture.assert_preserved_cloud().await;
    assert_eq!(fixture.ids().await, json!(["old", "peer"]));
    fixture.close().await;
}

#[tokio::test]
async fn completed_real_delete_removes_only_the_absent_original_cloud_file_and_slot() {
    let fixture = Fixture::new("normal").await;
    fixture.run().await.unwrap();
    assert_eq!(*fixture.host.deleted.lock().await, vec!["10001"]);
    assert_eq!(fixture.ids().await, json!(["peer"]));
    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT id FROM p115_filesystem_cache ORDER BY id")
            .fetch_all(&fixture.state.db)
            .await
            .unwrap();
    assert_eq!(remaining, vec!["20002", "30003"]);
    fixture.close().await;
}

#[tokio::test]
async fn old_delete_without_pickcode_cannot_use_a_replaced_path_cache() {
    let fixture = Fixture::new("normal").await;
    sqlx::query(
        "UPDATE media_metadata SET file_pickcode_json=$1 WHERE tmdb_id='101' AND item_type='Movie'",
    )
    .bind(json!([null, "pc-peer"]))
    .execute(&fixture.state.db)
    .await
    .unwrap();
    // Only the replacement is at the final path; its STRM has not reappeared
    // and its new native Item has not yet been created.
    sqlx::query("UPDATE p115_filesystem_cache SET local_path='old/same.mkv' WHERE id='10001'")
        .execute(&fixture.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE p115_filesystem_cache SET local_path='same.strm' WHERE id='20002'")
        .execute(&fixture.state.db)
        .await
        .unwrap();
    fixture.run().await.unwrap();
    fixture.assert_preserved_cloud().await;
    assert_eq!(fixture.ids().await, json!(["peer"]));
    fixture.close().await;
}

#[tokio::test]
async fn old_delete_cannot_authorize_a_pending_wash_slot() {
    let fixture = Fixture::new("normal").await;
    sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,'{0,moviepilot_media_pending}','true') WHERE tmdb_id='101' AND item_type='Movie'")
        .execute(&fixture.state.db).await.unwrap();
    assert!(fixture.run().await.is_err());
    fixture.assert_preserved_cloud().await;
    assert_eq!(fixture.ids().await, json!(["old", "peer"]));
    fixture.close().await;
}

#[tokio::test]
async fn native_lookup_failure_or_malformed_response_cannot_authorize_deletion() {
    for mode in ["ids_error", "ids_malformed", "paths_incomplete"] {
        let fixture = Fixture::new(mode).await;
        assert!(fixture.run().await.is_err(), "{mode}");
        fixture.assert_preserved_cloud().await;
        assert_eq!(fixture.ids().await, json!(["old", "peer"]), "{mode}");
        fixture.close().await;
    }
}
