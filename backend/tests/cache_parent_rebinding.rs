use emby_toolkit::{AppState, metadata};
use serde_json::{Value, json};

async fn reparent(reject_write: bool) -> (Value, Value, Value, Value, bool) {
    let url = std::env::var("DATABASE_URL").unwrap();
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("parent_rebind_{}", uuid::Uuid::new_v4().simple());
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
    for (tmdb, parent, ids) in [
        ("12311", "123", vec!["moving", "kept"]),
        ("45611", "456", vec!["other"]),
    ] {
        let assets=ids.iter().map(|id|json!({"emby_item_id":id,"path":format!("/media/{id}.mkv"),"fingerprint_pending":false,"fingerprint_revision":7,"size_bytes":1000})).collect::<Vec<_>>();
        let shas = ids
            .iter()
            .map(|id| {
                json!(match *id {
                    "moving" => "a".repeat(40),
                    "kept" => "b".repeat(40),
                    _ => "c".repeat(40),
                })
            })
            .collect::<Vec<_>>();
        let pcs = ids
            .iter()
            .map(|id| json!(format!("pc-{id}")))
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,title,season_number,episode_number,in_library,emby_item_ids_json,asset_details_json,file_sha1_json,file_pickcode_json) VALUES($1,'Episode',$2,'Cached episode',1,1,true,$3,$4,$5,$6)")
            .bind(tmdb).bind(parent).bind(json!(ids)).bind(json!(assets)).bind(json!(shas)).bind(json!(pcs)).execute(&state.db).await.unwrap();
    }
    let before_old = metadata::row(&state, "12311", "Episode")
        .await
        .unwrap()
        .unwrap();
    let before_new = metadata::row(&state, "45611", "Episode")
        .await
        .unwrap()
        .unwrap();
    let mut incoming = vec![
        json!({"tmdb_id":"45611","item_type":"Episode","parent_series_tmdb_id":"456","title":"Confirmed destination episode","season_number":1,"episode_number":1,"in_library":true,"emby_item_ids_json":["moving"],"asset_details_json":[{"emby_item_id":"moving","path":"/media/moving.mkv","size_bytes":1000}],"file_sha1_json":[null],"file_pickcode_json":[null],"runtime_minutes":if reject_write {json!("invalid-runtime")} else {json!(40)}}),
    ];
    let written = metadata::ingest(&state, &mut incoming, true).await.is_ok();
    let after_old = metadata::row(&state, "12311", "Episode")
        .await
        .unwrap()
        .unwrap();
    let after_new = metadata::row(&state, "45611", "Episode")
        .await
        .unwrap()
        .unwrap();
    state.db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    (before_old, before_new, after_old, after_new, written)
}

fn version(row: &Value, id: &str) -> Value {
    let index = row["emby_item_ids_json"]
        .as_array()
        .unwrap()
        .iter()
        .position(|value| value == id)
        .unwrap();
    json!({"id":row["emby_item_ids_json"][index],"asset":row["asset_details_json"][index],"sha":row["file_sha1_json"][index],"pc":row["file_pickcode_json"][index]})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn reparented_episode_moves_only_its_native_version_between_series() {
    let (before_old, before_new, after_old, after_new, written) = reparent(false).await;
    assert!(written);
    assert_eq!(after_old["emby_item_ids_json"], json!(["kept"]));
    assert_eq!(version(&after_old, "kept"), version(&before_old, "kept"));
    assert_eq!(version(&after_new, "other"), version(&before_new, "other"));
    let moved = version(&after_new, "moving");
    let original = version(&before_old, "moving");
    assert_eq!(moved["sha"], original["sha"]);
    assert_eq!(moved["pc"], original["pc"]);
    assert_eq!(
        moved["asset"]["fingerprint_revision"],
        original["asset"]["fingerprint_revision"]
    );
    assert_eq!(moved["asset"]["fingerprint_pending"], false);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn failed_reparent_keeps_both_series_original_versions() {
    let (before_old, before_new, after_old, after_new, written) = reparent(true).await;
    assert!(!written);
    assert_eq!(after_old, before_old);
    assert_eq!(after_new, before_new);
}
