use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode},
    routing::get,
};
use emby_toolkit::{AppState, db};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Host {
    id: &'static str,
    online: Arc<AtomicBool>,
    delay: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    business: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
}
impl Host {
    fn new(id: &'static str) -> Self {
        Self {
            id,
            online: Arc::new(AtomicBool::new(true)),
            delay: Arc::default(),
            entered: Arc::default(),
            release: Arc::default(),
            business: Arc::default(),
            writes: Arc::default(),
        }
    }
}
async fn serve(host: Host) -> (String, tokio::task::JoinHandle<()>) {
    let identity = get(|State(host): State<Host>| async move {
        if host.delay.load(Ordering::SeqCst) {
            host.entered.notify_one();
            host.release.notified().await;
        }
        if host.online.load(Ordering::SeqCst) {
            (StatusCode::OK, Json(json!({"Id":host.id})))
        } else {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"fixture offline"})),
            )
        }
    });
    let router = Router::new()
        .route("/System/Info", identity.clone())
        .route("/System/Info/Public", identity)
        .fallback(|State(host): State<Host>, request: Request| async move {
            host.business.fetch_add(1, Ordering::SeqCst);
            if request.method() != Method::GET && request.method() != Method::HEAD {
                host.writes.fetch_add(1, Ordering::SeqCst);
            }
            StatusCode::BAD_GATEWAY
        })
        .with_state(host);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}
struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    directory: tempfile::TempDir,
    a: Host,
    b: Host,
    a_url: String,
    b_url: String,
    servers: Vec<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("server_identity_{}", uuid::Uuid::new_v4().simple());
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
        let directory = tempfile::Builder::new()
            .prefix("identity-cache-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let state = AppState::new(pool, directory.path().join("app"))
            .await
            .unwrap();
        let a = Host::new("server-A");
        let b = Host::new("server-B");
        let (a_url, a_server) = serve(a.clone()).await;
        let (b_url, b_server) = serve(b.clone()).await;
        state.save_config(&json!({"emby_server_url":a_url,"emby_api_key":"fixture-key","emby_user_id":"admin"})).await.unwrap();
        Self {
            state,
            admin,
            schema,
            directory,
            a,
            b,
            a_url,
            b_url,
            servers: vec![a_server, b_server],
        }
    }
    async fn cold(&self) -> AppState {
        AppState::new(self.state.db.clone(), self.directory.path().join("app"))
            .await
            .unwrap()
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        for server in self.servers {
            server.abort();
        }
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; both Emby services are localhost"]
async fn another_server_503_cannot_reuse_a_cached_identity_or_replay_a_job() {
    let f = Fixture::new().await;
    f.state.refresh_server_identity().await;
    assert_eq!(f.state.config.read().await["emby_server_id"], "server-A");
    assert_eq!(f.state.config.read().await["emby_server_id_verified"], true);
    assert_eq!(
        db::setting(&f.state.db, "emby_server_id_cache")
            .await
            .unwrap(),
        Some(json!({"id":"server-A","server_url":f.a_url}))
    );
    f.b.online.store(false, Ordering::SeqCst);
    f.state.save_config(&json!({"emby_server_url":f.b_url,"emby_server_id_verified":true,"emby_server_id":"server-A"})).await.unwrap();
    assert_eq!(
        f.state.config.read().await["emby_server_id_verified"],
        false
    );
    sqlx::query("INSERT INTO media_organize_queue(id,item_id,mode,item_json,operator_id,task_id,status) VALUES('original-queue','old-item','rescrape',$1,'fixture-user','original-job','queued')")
        .bind(json!({"item_id":"old-item","item_type":"Movie","name":"原服务器媒体"})).execute(&f.state.db).await.unwrap();
    sqlx::query("INSERT INTO durable_jobs(id,spec_json,state_json) VALUES('original-job',$1,$2)")
        .bind(json!({"kind":"media_organize_queue","payload":{"kind":"media_organize_queue","queue_id":"original-queue"},"owned":true,"server_id":"server-A","server_url":f.a_url}))
        .bind(json!({"task_id":"original-job","state":"interrupted","is_running":false,"last_action":"原服务器任务","cleanup_pending":true}))
        .execute(&f.state.db).await.unwrap();
    let cold = f.cold().await;
    assert_eq!(cold.config.read().await["emby_server_id_verified"], false);
    cold.refresh_server_identity().await;
    assert!(cold.config.read().await["emby_server_id"].is_null());
    assert_eq!(cold.config.read().await["emby_server_id_verified"], false);
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        cold.jobs
            .resume_pending(cold.clone(), CancellationToken::new()),
    )
    .await;
    assert_eq!(f.b.business.load(Ordering::SeqCst), 0);
    assert_eq!(f.b.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        db::setting(&f.state.db, "emby_server_id_cache")
            .await
            .unwrap(),
        Some(json!({"id":"server-A","server_url":f.a_url}))
    );
    cold.jobs.shutdown().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; both Emby services are localhost"]
async fn same_address_fallback_is_unverified_and_a_live_response_migrates_legacy_cache() {
    let f = Fixture::new().await;
    db::save_setting(&f.state.db, "emby_server_id_cache", &json!("server-A"))
        .await
        .unwrap();
    f.a.online.store(false, Ordering::SeqCst);
    f.state.refresh_server_identity().await;
    assert!(f.state.config.read().await["emby_server_id"].is_null());
    assert_eq!(
        f.state.config.read().await["emby_server_id_verified"],
        false
    );
    f.a.online.store(true, Ordering::SeqCst);
    f.state.refresh_server_identity().await;
    assert_eq!(f.state.config.read().await["emby_server_id_verified"], true);
    f.a.online.store(false, Ordering::SeqCst);
    f.state.refresh_server_identity().await;
    assert_eq!(f.state.config.read().await["emby_server_id"], "server-A");
    assert_eq!(
        f.state.config.read().await["emby_server_id_verified"],
        false
    );
    f.state
        .save_config(&json!({"emby_server_id_verified":true,"emby_api_key":"********"}))
        .await
        .unwrap();
    assert_eq!(
        f.state.config.read().await["emby_server_id_verified"],
        false
    );
    let persisted = db::setting(&f.state.db, "dynamic_app_config")
        .await
        .unwrap()
        .unwrap();
    assert!(persisted.get("emby_server_id_verified").is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; both Emby services are localhost"]
async fn api_key_changes_and_config_reloads_cannot_forge_live_verification() {
    let f = Fixture::new().await;
    f.state.refresh_server_identity().await;
    f.state
        .save_config(
            &json!({"emby_api_key":"different-fixture-key","emby_server_id_verified":true}),
        )
        .await
        .unwrap();
    assert_eq!(
        f.state.config.read().await["emby_server_id_verified"],
        false
    );
    assert!(f.state.config.read().await["emby_server_id"].is_null());
    let mut stored = db::setting(&f.state.db, "dynamic_app_config")
        .await
        .unwrap()
        .unwrap();
    stored["emby_server_id_verified"] = json!(true);
    db::save_setting(&f.state.db, "dynamic_app_config", &stored)
        .await
        .unwrap();
    f.state.reload_config().await.unwrap();
    assert_eq!(
        f.state.config.read().await["emby_server_id_verified"],
        false
    );
    let cold = f.cold().await;
    assert_eq!(cold.config.read().await["emby_server_id_verified"], false);
    cold.jobs.shutdown().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; both Emby services are localhost"]
async fn an_old_identity_response_cannot_validate_a_new_server_configuration() {
    let f = Fixture::new().await;
    f.a.delay.store(true, Ordering::SeqCst);
    let request_state = f.state.clone();
    let request = tokio::spawn(async move {
        request_state.refresh_server_identity().await;
    });
    tokio::time::timeout(Duration::from_secs(3), f.a.entered.notified())
        .await
        .unwrap();
    f.state
        .save_config(&json!({"emby_server_url":f.b_url}))
        .await
        .unwrap();
    f.state.refresh_server_identity().await;
    f.a.release.notify_one();
    request.await.unwrap();
    assert_eq!(f.state.config.read().await["emby_server_id"], "server-B");
    assert_eq!(f.state.config.read().await["emby_server_id_verified"], true);
    assert_eq!(
        db::setting(&f.state.db, "emby_server_id_cache")
            .await
            .unwrap(),
        Some(json!({"id":"server-B","server_url":f.b_url}))
    );
    assert_eq!(f.b.writes.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; both Emby services are localhost"]
async fn identity_cache_uses_the_normalized_address_without_url_credentials_or_fragments() {
    let f = Fixture::new().await;
    let raw = format!(
        "{}/?discard=yes#ignored",
        f.a_url.replace("http://", "http://fixture:ignored@")
    );
    f.state
        .save_config(&json!({"emby_server_url":raw}))
        .await
        .unwrap();
    f.state.refresh_server_identity().await;
    assert_eq!(f.state.config.read().await["emby_server_id"], "server-A");
    assert_eq!(f.state.config.read().await["emby_server_id_verified"], true);
    assert_eq!(
        db::setting(&f.state.db, "emby_server_id_cache")
            .await
            .unwrap(),
        Some(json!({"id":"server-A","server_url":f.a_url}))
    );
    assert_eq!(f.a.business.load(Ordering::SeqCst), 0);
    f.close().await;
}
