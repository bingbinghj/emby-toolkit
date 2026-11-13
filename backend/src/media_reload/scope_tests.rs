use super::*;
use crate::{auth::SessionSigner, jobs::JobManager, logs::LogStore};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicUsize},
};
use tokio::sync::RwLock;

#[derive(Clone)]
struct Fixture {
    inventory: Arc<RwLock<Vec<Value>>>,
    inventory_reads: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    policy: Arc<RwLock<Value>>,
    parts: Arc<RwLock<Value>>,
    extras: Arc<RwLock<Value>>,
    config: Arc<RwLock<Value>>,
    switch_config_between_pages: Arc<AtomicBool>,
    page_tokens: Arc<RwLock<Vec<String>>>,
}

fn live(id: &str) -> Value {
    json!({
        "Id":id,"Path":format!("/fixture/{id}.mkv"),"Type":"Episode",
        "PresentationUniqueKey":format!("episode-{id}"),"PartCount":1,"ExtraType":null,
        "Chapters":[{"StartPositionTicks":0,"MarkerType":"IntroStart"}],
        "MediaStreams":[{"Type":"Video","Index":0}],
        "MediaSources":[{"Id":id,"Path":format!("/fixture/{id}.mkv"),
            "RunTimeTicks":10_000_000,"MediaStreams":[{"Type":"Video","Index":0}]}]
    })
}

fn target(id: &str) -> SavedTarget {
    let item = live(id);
    SavedTarget {
        id: id.into(),
        path: string(&item, "Path"),
        source: None,
        sha: None,
        asset: None,
        info: json!([{"Chapters":item["Chapters"],"MediaSourceInfo":item["MediaSources"][0]}]),
        refresh_required: true,
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    AppState,
    Fixture,
    tokio::task::JoinHandle<()>,
) {
    let fixture = Fixture {
        inventory: Arc::new(RwLock::new(vec![live("1"), live("2")])),
        inventory_reads: Arc::new(AtomicUsize::new(0)),
        writes: Arc::new(AtomicUsize::new(0)),
        policy: Arc::new(RwLock::new(
            json!({"IsAdministrator":true,"MaxParentalRating":null,
            "BlockUnratedItems":[],"BlockedTags":[],"IsTagBlockingModeInclusive":false,
            "AllowTagOrRating":false}),
        )),
        parts: Arc::new(RwLock::new(json!({"Items":[],"TotalRecordCount":0}))),
        extras: Arc::new(RwLock::new(json!([]))),
        config: Arc::new(RwLock::new(Value::Null)),
        switch_config_between_pages: Arc::new(AtomicBool::new(false)),
        page_tokens: Arc::new(RwLock::new(vec![])),
    };
    let bundle =
        json!({"type":"array","items":{"$ref":"#/definitions/StrmAssistant.Dto.MediaInfoBundle"}});
    let swagger = json!({"paths":{
        "/Items/SyncMediaInfo":{"post":{"parameters":[
            {"name":"Id","in":"query","type":"string"},
            {"name":"Body","in":"body","schema":bundle}],
            "responses":{"200":{"schema":bundle}}}},
        "/Items/{Id}/Refresh":{"post":{"parameters":[
            {"name":"Id","in":"path"},{"name":"MetadataRefreshMode","in":"query"},
            {"name":"ImageRefreshMode","in":"query"},{"name":"ReplaceAllMetadata","in":"query"},
            {"name":"ReplaceAllImages","in":"query"}]}}
    }});
    let app = Router::new()
        .route("/Items",get(|State(f):State<Fixture>,Query(q):Query<HashMap<String,String>>,headers:HeaderMap|async move {
            f.inventory_reads.fetch_add(1,Ordering::SeqCst);
            f.page_tokens.write().await.push(headers["X-Emby-Token"].to_str().unwrap().into());
            let items=f.inventory.read().await;
            let start=q["StartIndex"].parse::<usize>().unwrap();
            if f.switch_config_between_pages.load(Ordering::SeqCst) {
                f.config.write().await["emby_api_key"]=json!(if start==0 {"other-admin"}else{"fixture"});
            }
            Json(json!({"Items":items.iter().skip(start).take(500).collect::<Vec<_>>(),"TotalRecordCount":items.len()}))
        }))
        .route("/Users/{user}/Items/{id}",get(|State(f):State<Fixture>,AxumPath((_user,id)):AxumPath<(String,String)>|async move {
            let item=f.inventory.read().await.iter().find(|item|item["Id"]==id).cloned().unwrap_or_else(||live(&id));
            Json(item)
        }))
        .route("/Videos/{id}/AdditionalParts",get(|State(f):State<Fixture>|async move {Json(f.parts.read().await.clone())}))
        .route("/Users/{user}",get(|State(f):State<Fixture>,AxumPath(user):AxumPath<String>|async move {Json(json!({"Id":user,"Policy":f.policy.read().await.clone()}))}))
        .route("/Users/{user}/Items/{id}/SpecialFeatures",get(|State(f):State<Fixture>|async move {Json(f.extras.read().await.clone())}))
        .route("/web/ConfigurationPages",get(||async {Json(json!([{"Name":"MediaInfoExtractPageView","Href":"UI/View?PageId=fixture"}]))}))
        .route("/UI/View",get(||async {Json(json!({"EditObjectContainer":{"Object":{
            "ExclusiveControlFeatures":"IgnoreFileChange","ExclusiveExtract":true,
            "IsModSupported":true,"PersistMediaInfoMode":"Restore"}}}))}))
        .route("/Plugins",get(||async {Json(json!([{"Id":"63c322b7-a371-41a3-b11f-04f8418b37d8","Version":"3.0.0.57"}]))}))
        .route("/swagger.json",get(move || {let swagger=swagger.clone();async move {Json(swagger)}}))
        .route("/Items/{id}/Refresh",post(|State(f):State<Fixture>|async move {
            f.writes.fetch_add(1,Ordering::SeqCst);Json(json!({"success":true}))
        }))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    *fixture.config.write().await =
        json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin"});
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: fixture.config.clone(),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root.path()).unwrap()),
        recovery_tokens: Arc::new(RwLock::new(HashMap::new())),
        data_dir: root.path().into(),
        notifications: Arc::default(),
        webhooks: Arc::default(),
        ai: Arc::default(),
        jobs: Arc::new(JobManager::default()),
        logs: LogStore::open(&root.path().join("logs")).unwrap(),
        p115: Arc::default(),
        strm: Arc::default(),
        douban: Arc::default(),
        docker: Arc::default(),
    };
    (root, state, fixture, server)
}

#[tokio::test]
async fn marker_batch_preflight_reads_complete_inventory_once() {
    let (_root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    preflight_file_imports(&state, &[target("1"), target("2")], &context)
        .await
        .unwrap();
    assert_eq!(
        f.inventory_reads.load(Ordering::SeqCst),
        1,
        "a read-only preparation batch must not rescan the whole library for every marker target"
    );
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn refresh_rechecks_new_siblings_after_preflight_and_does_not_write() {
    let (_root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    let target = target("1");
    preflight_file_import(&state, &target, &context)
        .await
        .unwrap();
    let mut sibling = live("3");
    sibling["PresentationUniqueKey"] = json!("episode-1");
    f.inventory.write().await.push(sibling);
    assert!(refresh_from_file(&state, &target, &context).await.is_err());
    assert_eq!(f.inventory_reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.writes.load(Ordering::SeqCst),
        0,
        "a newly added unselected sibling must prevent the native refresh submission"
    );
    server.abort();
}

#[tokio::test]
async fn marker_batch_preflight_keeps_all_inventory_pages() {
    let (_root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    *f.inventory.write().await = (1..=501).map(|id| live(&id.to_string())).collect();
    preflight_file_imports(&state, &[target("1"), target("2")], &context)
        .await
        .unwrap();
    assert_eq!(f.inventory_reads.load(Ordering::SeqCst), 2);
    // A sibling on the second page must still prevent both preparation and writes.
    f.inventory.write().await[500]["PresentationUniqueKey"] = json!("episode-1");
    assert!(
        preflight_file_imports(&state, &[target("1"), target("2")], &context)
            .await
            .is_err()
    );
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn inventory_pages_pin_connection_during_transient_config_switch() {
    let (_root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    *f.inventory.write().await = (1..=501).map(|id| live(&id.to_string())).collect();
    f.switch_config_between_pages.store(true, Ordering::SeqCst);
    let inventory = clear_scope_inventory(&state, &context).await.unwrap();
    assert_eq!(inventory.items.len(), 501);
    assert_eq!(
        *f.page_tokens.read().await,
        vec!["fixture", "fixture"],
        "A -> B -> A must not mix authentication contexts across inventory pages"
    );
    assert_eq!(f.inventory_reads.load(Ordering::SeqCst), 2);
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn prepared_scope_rechecks_policy_parts_and_extras_for_each_target() {
    let context = JobContext::for_recovery(None).await;
    for field in ["policy", "parts", "parts_count", "extras", "sources"] {
        let (_root, state, f, server) = fixture().await;
        let mut inventory = None;
        preflight_file_import_prepared(&state, &target("1"), &context, &mut inventory)
            .await
            .unwrap();
        match field {
            "policy" => f.policy.write().await["BlockedTags"] = json!(["restricted"]),
            "parts" => {
                *f.parts.write().await = json!({"Items":[{"Id":"part"}],"TotalRecordCount":1})
            }
            "parts_count" => f.parts.write().await["TotalRecordCount"] = json!(1),
            "extras" => *f.extras.write().await = json!([{"Id":"extra"}]),
            _ => f.inventory.write().await[1]["MediaSources"] = json!([]),
        }
        assert!(
            preflight_file_import_prepared(&state, &target("2"), &context, &mut inventory)
                .await
                .is_err(),
            "{field} cannot be trusted from an earlier target"
        );
        assert_eq!(f.inventory_reads.load(Ordering::SeqCst), 1);
        assert_eq!(f.writes.load(Ordering::SeqCst), 0);
        server.abort();
    }
}

#[tokio::test]
async fn prepared_scope_rejects_changed_connection_and_two_admin_identities() {
    let context = JobContext::for_recovery(None).await;
    for field in ["emby_server_url", "emby_api_key", "emby_user_id"] {
        let (_root, state, f, server) = fixture().await;
        let mut inventory = None;
        preflight_file_import_prepared(&state, &target("1"), &context, &mut inventory)
            .await
            .unwrap();
        state.config.write().await[field] = json!(match field {
            "emby_server_url" => "http://another-server.invalid",
            "emby_api_key" => "second-admin-key",
            _ => "second-admin",
        });
        let error = if field == "emby_server_url" {
            ensure_single_clear_scope_prepared(
                &state,
                &target("2"),
                &live("2"),
                &context,
                &mut inventory,
            )
            .await
        } else {
            // The fixture permits both administrator IDs and both keys; the
            // refusal must come from snapshot identity, not an HTTP failure.
            preflight_file_import_prepared(&state, &target("2"), &context, &mut inventory).await
        }
        .unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert!(error.1.contains("连接或用户身份"), "{field}: {}", error.1);
        assert_eq!(f.inventory_reads.load(Ordering::SeqCst), 1);
        assert_eq!(f.writes.load(Ordering::SeqCst), 0);
        server.abort();
    }
}

#[tokio::test]
async fn prepared_scope_rejects_malformed_inventory_and_live_scope() {
    let (_root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    for field in ["PresentationUniqueKey", "Id", "Path", "duplicate"] {
        let mut items = vec![live("1"), live("2")];
        match field {
            "PresentationUniqueKey" => items[1][field] = json!(""),
            "Id" => items[1][field] = json!(""),
            "Path" => items[0][field] = json!("/fixture/another.mkv"),
            _ => items[1]["Id"] = json!("1"),
        }
        *f.inventory.write().await = items;
        assert!(
            preflight_file_imports(&state, &[target("1"), target("2")], &context)
                .await
                .is_err(),
            "{field} must fail closed"
        );
    }
    *f.inventory.write().await = vec![live("1"), live("2")];
    let mut inventory = Some(clear_scope_inventory(&state, &context).await.unwrap());
    for (field, value) in [
        ("PartCount", json!(2)),
        ("ExtraType", json!("Trailer")),
        ("MediaSources", json!([])),
        ("Type", json!("Season")),
        ("Id", json!("another")),
        ("Path", json!("/fixture/another.mkv")),
    ] {
        let mut item = live("1");
        item[field] = value;
        assert!(
            ensure_single_clear_scope_prepared(
                &state,
                &target("1"),
                &item,
                &context,
                &mut inventory
            )
            .await
            .is_err(),
            "{field} must fail closed"
        );
    }
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn unmarked_preflight_does_not_read_full_inventory() {
    let (_root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    let mut targets = vec![target("1"), target("2")];
    for target in &mut targets {
        target.info[0]["Chapters"] = json!([]);
    }
    preflight_file_imports(&state, &targets, &context)
        .await
        .unwrap();
    assert_eq!(f.inventory_reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn final_resource_verification_refuses_changed_file_without_overwriting_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chapter.jpg");
    let backup = root.path().join("original.jpg");
    tokio::fs::write(&path, b"original image").await.unwrap();
    tokio::fs::write(&backup, b"original image").await.unwrap();
    let owners = BTreeSet::from(["1".into()]);
    let files = BTreeMap::from([(
        path.clone(),
        Snapshot {
            path: path.clone(),
            backup: backup.clone(),
            owners: owners.clone(),
            sha256: Some(format!("{:x}", Sha256::digest(b"original image"))),
        },
    )]);
    verify_restored_resources(&files, &owners).await.unwrap();
    tokio::fs::write(&path, b"concurrent user image")
        .await
        .unwrap();
    assert!(verify_restored_resources(&files, &owners).await.is_err());
    assert_eq!(
        tokio::fs::read(&path).await.unwrap(),
        b"concurrent user image"
    );
    assert_eq!(tokio::fs::read(&backup).await.unwrap(), b"original image");
}

#[tokio::test]
async fn noop_without_original_images_does_not_require_a_visible_media_parent() {
    let (root, state, f, server) = fixture().await;
    let context = JobContext::for_recovery(None).await;
    let path = root.path().join("unmounted-media").join("episode.mkv");
    let mut target = target("1");
    target.path = path.to_string_lossy().into();
    target.info[0]["MediaSourceInfo"]["Path"] = json!(target.path);
    {
        let mut inventory = f.inventory.write().await;
        inventory[0]["Path"] = json!(target.path);
        inventory[0]["MediaSources"][0]["Path"] = json!(target.path);
    }
    assert!(!path.parent().unwrap().exists());
    reload(
        &state,
        &[target],
        &context,
        &AtomicBool::new(true),
        &root.path().join("recovery"),
    )
    .await
    .unwrap();
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    assert!(!path.parent().unwrap().exists());
    server.abort();
}
