use axum::{
    Json, Router,
    extract::{Path, Query},
    routing::get,
};
use emby_toolkit::{AppState, metadata, sync};
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};

struct Observed {
    result: Value,
    before: HashMap<String, Value>,
    after: HashMap<String, Value>,
    moving_owners: Vec<String>,
}

async fn changed_provider(
    full: bool,
    reject_destination: bool,
    keep_old_version: bool,
    conflicting_cache: bool,
) -> Observed {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("provider_rebind_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search_path = format!("SET search_path TO {schema}");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(move |connection, _| {
            let query = search_path.clone();
            Box::pin(async move {
                sqlx::query(&query).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let state = AppState::new(pool, temp.path().into()).await.unwrap();
    let mut items = vec![
        json!({"Id":"moving","Type":"Movie","ProviderIds":{"Tmdb":"456"},"Name":"Moved movie","Path":"/media/moving.mkv","Size":1000,"RunTimeTicks":600000000_u64}),
        json!({"Id":"kept","Type":"Movie","ProviderIds":{"Tmdb":"123"},"Name":"Kept movie","Path":"/media/kept.mkv","Size":1000,"RunTimeTicks":600000000_u64}),
        json!({"Id":"other","Type":"Movie","ProviderIds":{"Tmdb":"456"},"Name":"Other edition","Path":"/media/other.mkv","Size":1000,"RunTimeTicks":600000000_u64}),
    ];
    if !keep_old_version {
        items.retain(|item| item["Id"] != "kept");
    }
    let native = items.clone();
    let upstream = Router::new()
        .route(
            "/Users/admin/Views",
            get(|| async { Json(json!({"Items":[{"Id":"library"}]})) }),
        )
        .route(
            "/Items",
            get(move |Query(query): Query<HashMap<String, String>>| {
                let items = native
                    .iter()
                    .filter(|item| {
                        query
                            .get("Ids")
                            .is_none_or(|ids| ids.split(',').any(|id| item["Id"] == id))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                async move { Json(json!({"Items":items,"TotalRecordCount":items.len()})) }
            }),
        )
        .route(
            "/Library/VirtualFolders",
            get(|| async { Json(json!([])) }),
        )
        .route(
            "/movie/{id}",
            get(move |Path(id): Path<String>| async move {
                Json(json!({"id":id.parse::<i64>().unwrap(),"title":format!("Movie {id}"),"original_language":"en","runtime":if reject_destination && id=="456" {json!("invalid-runtime")} else {json!(90)}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    state.save_config(&json!({"emby_server_url":base,"emby_api_key":"fixture","emby_user_id":"admin","tmdb_api_base_url":base,"tmdb_api_key":"fixture","ai_translate_on_sync":false,"monitor_sha1_pc_search":false,"p115_fingerprint_media_root_cid":"","libraries_to_ignore":[]})).await.unwrap();
    for (tmdb, ids) in [
        (
            "123",
            if keep_old_version {
                vec!["moving", "kept"]
            } else {
                vec!["moving"]
            },
        ),
        (
            "456",
            if conflicting_cache {
                vec!["moving", "other"]
            } else {
                vec!["other"]
            },
        ),
    ] {
        let assets = ids.iter().map(|id| json!({"emby_item_id":id,"path":format!("/media/{id}.mkv"),"size_bytes":1000,"fingerprint_pending":false,"fingerprint_revision":7,"fingerprint_target":format!("/original/{id}.mkv")})).collect::<Vec<_>>();
        let shas = ids
            .iter()
            .map(|id| {
                json!(match *id {
                    "moving" if conflicting_cache && tmdb == "456" => "d".repeat(40),
                    "moving" => "a".repeat(40),
                    "kept" => "b".repeat(40),
                    _ => "c".repeat(40),
                })
            })
            .collect::<Vec<_>>();
        let pcs = ids
            .iter()
            .map(|id| {
                json!(if conflicting_cache && tmdb == "456" && *id == "moving" {
                    "conflicting-pc-moving".into()
                } else {
                    format!("pc-{id}")
                })
            })
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json,runtime_minutes) VALUES($1,'Movie',$2,true,$3,$4,$5,$6,15)")
            .bind(tmdb).bind(format!("Cached {tmdb}")).bind(json!(ids)).bind(json!(assets)).bind(json!(shas)).bind(json!(pcs)).execute(&state.db).await.unwrap();
    }
    let mut before = HashMap::new();
    for id in ["123", "456"] {
        before.insert(
            id.into(),
            metadata::row(&state, id, "Movie").await.unwrap().unwrap(),
        );
    }
    let work = state.clone();
    let job = state
        .jobs
        .submit("Provider binding check", move |context| async move {
            sync::cache(&work, full, &context).await
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let result = state.jobs.result(&job).await.unwrap();
            if result["is_running"] == false {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut after = HashMap::new();
    for id in ["123", "456"] {
        after.insert(
            id.into(),
            metadata::row(&state, id, "Movie").await.unwrap().unwrap(),
        );
    }
    let moving_owners = sqlx::query_scalar::<_, String>("SELECT tmdb_id FROM media_metadata WHERE item_type='Movie' AND emby_item_ids_json ? 'moving' ORDER BY tmdb_id")
        .fetch_all(&state.db).await.unwrap();
    state.jobs.shutdown().await;
    server.abort();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    Observed {
        result,
        before,
        after,
        moving_owners,
    }
}

fn fingerprint(row: &Value, id: &str) -> Value {
    let index = row["emby_item_ids_json"]
        .as_array()
        .unwrap()
        .iter()
        .position(|value| value == id)
        .unwrap();
    let asset = &row["asset_details_json"][index];
    json!({"path":asset["path"],"size_bytes":asset["size_bytes"],"pending":asset["fingerprint_pending"],"revision":asset["fingerprint_revision"],"target":asset["fingerprint_target"],"sha":row["file_sha1_json"][index],"pc":row["file_pickcode_json"][index]})
}

fn migrated(observed: Observed) {
    assert_eq!(observed.result["state"], "completed", "{}", observed.result);
    assert_eq!(
        observed.moving_owners,
        vec!["456"],
        "same native ID and path must follow the confirmed provider: {}",
        observed.result
    );
    assert_eq!(observed.after["123"]["emby_item_ids_json"], json!(["kept"]));
    for (before, after, native) in [
        ("123", "123", "kept"),
        ("456", "456", "other"),
        ("123", "456", "moving"),
    ] {
        assert_eq!(
            fingerprint(&observed.after[after], native),
            fingerprint(&observed.before[before], native),
            "fingerprint of {native} must be preserved"
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstreams are localhost"]
async fn incremental_provider_change_moves_only_the_confirmed_native_version() {
    migrated(changed_provider(false, false, true, false).await);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstreams are localhost"]
async fn full_provider_change_does_not_keep_the_native_version_on_its_old_work() {
    migrated(changed_provider(true, false, true, false).await);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstreams are localhost"]
async fn failed_provider_destination_write_keeps_original_versions_and_fingerprints() {
    let observed = changed_provider(true, true, true, false).await;
    assert_eq!(observed.result["state"], "failed", "{}", observed.result);
    assert_eq!(observed.moving_owners, vec!["123"]);
    assert_eq!(observed.after["456"], observed.before["456"]);
    for native in ["moving", "kept"] {
        assert_eq!(
            fingerprint(&observed.after["123"], native),
            fingerprint(&observed.before["123"], native)
        );
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstreams are localhost"]
async fn moved_last_native_version_keeps_the_old_work_offline_with_its_metadata() {
    let observed = changed_provider(false, false, false, false).await;
    assert_eq!(observed.result["state"], "completed", "{}", observed.result);
    assert_eq!(observed.moving_owners, vec!["456"]);
    assert_eq!(observed.after["123"]["in_library"], false);
    assert_eq!(observed.after["123"]["emby_item_ids_json"], json!([]));
    assert_eq!(
        observed.after["123"]["title"],
        observed.before["123"]["title"]
    );
    assert_eq!(
        fingerprint(&observed.after["456"], "moving"),
        fingerprint(&observed.before["123"], "moving")
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; upstreams are localhost"]
async fn conflicting_cached_fingerprints_are_not_guessed_or_removed_during_rebinding() {
    let observed = changed_provider(false, false, true, true).await;
    assert_eq!(observed.result["state"], "failed", "{}", observed.result);
    assert_eq!(observed.moving_owners, vec!["123", "456"]);
    assert_eq!(observed.after["123"], observed.before["123"]);
    assert_eq!(observed.after["456"], observed.before["456"]);
    assert!(
        observed.result["result"]["failures"]
            .to_string()
            .contains("指纹缓存冲突")
    );
}
