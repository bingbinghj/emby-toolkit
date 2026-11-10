use super::{row, sync_upsert};
use crate::{AppState, sync};
use serde_json::{Value, json};
use std::collections::HashMap;

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    _temp: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_URL").unwrap();
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("sync_bindings_{}", uuid::Uuid::new_v4().simple());
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
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(pool, temp.path().into()).await.unwrap();
        Self {
            state,
            admin,
            schema,
            _temp: temp,
        }
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
    async fn keys(&self, kind: &str, native: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT tmdb_id FROM media_metadata WHERE item_type=$1 AND emby_item_ids_json ? $2 ORDER BY tmdb_id")
            .bind(kind).bind(native).fetch_all(&self.state.db).await.unwrap()
    }
}

async fn records(season: Option<u64>, episode: Option<u64>) -> Vec<Value> {
    let data = season.map(|id| json!({"series_details":{"name":"Series","seasons":[{"id":id,"season_number":1,"name":"Season 1"}]},"episodes_details":{"S1E1":{"id":episode,"name":"Episode 1"}}})).unwrap_or(json!({}));
    sync::records(
        &("456".into(), "Series".into()),
        &[json!({"Id":"series","Type":"Series","Name":"Series","ProviderIds":{"Tmdb":"456"}})],
        &[
            json!({"Id":"season","Type":"Season","Name":"Season 1","SeriesId":"series","IndexNumber":1}),
            json!({"Id":"episode","Type":"Episode","Name":"Episode 1","SeriesId":"series","ParentIndexNumber":1,"IndexNumber":1,"Path":"/media/episode.mkv"}),
        ],
        &data,
        &HashMap::new(), &HashMap::new(), [&json!([]), &json!([]), &json!([])],
    ).await
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn failure_preserves_real_keys_repairs_legacy_duplicates_and_keeps_other_versions() {
    let f = Fixture::new().await;
    let mut real = records(Some(400), Some(789)).await;
    let result = sync_upsert(&f.state, &mut real).await.unwrap();
    assert_eq!(result.written, 3);
    assert!(result.failures.is_empty());
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=emby_item_ids_json || '[\"other\"]'::jsonb,asset_details_json='[{\"emby_item_id\":\"episode\",\"path\":\"/media/episode.mkv\",\"fingerprint_revision\":5,\"fingerprint_pending\":false},{\"emby_item_id\":\"other\",\"path\":\"/other/episode.mkv\"}]',file_sha1_json='[\"original-sha\",\"other-sha\"]',file_pickcode_json='[\"original-pc\",\"other-pc\"]' WHERE item_type='Episode' AND tmdb_id='789'")
        .execute(&f.state.db).await.unwrap();
    let mut fallback = records(None, None).await;
    let report = sync_upsert(&f.state, &mut fallback).await.unwrap();
    assert_eq!(report.written, 3);
    assert!(report.failures.is_empty());
    assert_eq!(f.keys("Season", "season").await, ["400"]);
    assert_eq!(f.keys("Episode", "episode").await, ["789"]);

    // Reproduce the duplicate rows left by an earlier version, then synchronize again.
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,season_number,episode_number,in_library,emby_item_ids_json) VALUES('456-S1','Season','456',1,NULL,true,'[\"season\"]'),('456-S1E1','Episode','456',1,1,true,'[\"episode\"]')")
        .execute(&f.state.db).await.unwrap();
    sqlx::query("UPDATE media_metadata SET asset_details_json='[{\"emby_item_id\":\"episode\",\"path\":\"/old/episode.mkv\",\"fingerprint_revision\":0,\"fingerprint_pending\":true}]',file_sha1_json='[\"stale-sha\"]',file_pickcode_json='[\"stale-pc\"]' WHERE tmdb_id='456-S1E1' AND item_type='Episode'")
        .execute(&f.state.db).await.unwrap();
    sync_upsert(&f.state, &mut fallback).await.unwrap();
    assert_eq!(f.keys("Season", "season").await, ["400"]);
    assert_eq!(f.keys("Episode", "episode").await, ["789"]);
    let repaired = row(&f.state, "789", "Episode").await.unwrap().unwrap();
    assert_eq!(
        repaired["asset_details_json"][0]["path"],
        "/media/episode.mkv"
    );
    assert_eq!(repaired["asset_details_json"][0]["fingerprint_revision"], 5);
    assert_eq!(
        repaired["asset_details_json"][0]["fingerprint_pending"],
        false
    );
    assert_eq!(repaired["file_sha1_json"][0], "original-sha");
    assert_eq!(repaired["file_pickcode_json"][0], "original-pc");

    let mut restored = records(Some(401), Some(790)).await;
    sync_upsert(&f.state, &mut restored).await.unwrap();
    assert_eq!(f.keys("Season", "season").await, ["401"]);
    assert_eq!(f.keys("Episode", "episode").await, ["790"]);
    assert_eq!(f.keys("Episode", "other").await, ["789"]);
    let current = row(&f.state, "790", "Episode").await.unwrap().unwrap();
    assert_eq!(current["file_sha1_json"], json!(["original-sha"]));
    assert_eq!(current["file_pickcode_json"], json!(["original-pc"]));
    assert_eq!(current["asset_details_json"][0]["fingerprint_revision"], 5);
    assert_eq!(
        current["asset_details_json"][0]["fingerprint_pending"],
        false
    );
    let other = row(&f.state, "789", "Episode").await.unwrap().unwrap();
    assert_eq!(other["emby_item_ids_json"], json!(["other"]));
    assert_eq!(other["asset_details_json"][0]["path"], "/other/episode.mkv");
    assert_eq!(other["file_sha1_json"], json!(["other-sha"]));
    assert_eq!(other["file_pickcode_json"], json!(["other-pc"]));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn unknown_children_use_synthetic_keys_and_failed_migration_restores_old_binding() {
    let f = Fixture::new().await;
    let mut fallback = records(None, None).await;
    sync_upsert(&f.state, &mut fallback).await.unwrap();
    assert_eq!(f.keys("Season", "season").await, ["456-S1"]);
    assert_eq!(f.keys("Episode", "episode").await, ["456-S1E1"]);
    let before = row(&f.state, "456-S1E1", "Episode").await.unwrap().unwrap();
    let mut real = records(Some(400), Some(789)).await;
    real.iter_mut()
        .find(|r| r["item_type"] == "Episode")
        .unwrap()["runtime_minutes"] = json!("invalid-integer");
    let report = sync_upsert(&f.state, &mut real).await.unwrap();
    assert_eq!(report.written, 2);
    assert_eq!(report.failures.len(), 1);
    assert!(report.failures[0].contains("22P02"));
    assert_eq!(f.keys("Season", "season").await, ["400"]);
    assert_eq!(f.keys("Episode", "episode").await, ["456-S1E1"]);
    assert_eq!(
        row(&f.state, "456-S1E1", "Episode").await.unwrap().unwrap(),
        before
    );
    assert!(row(&f.state, "789", "Episode").await.unwrap().is_none());
    let mut recovered = records(Some(400), Some(789)).await;
    sync_upsert(&f.state, &mut recovered).await.unwrap();
    assert_eq!(f.keys("Episode", "episode").await, ["789"]);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn fallback_preserves_distinct_known_keys_for_native_versions_of_the_same_episode() {
    let f = Fixture::new().await;
    let mut first = records(Some(400), Some(789)).await;
    sync_upsert(&f.state, &mut first).await.unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,season_number,episode_number,episode_group_id,in_library,emby_item_ids_json,file_sha1_json) VALUES('890','Episode','456',2,3,'alternate-group',true,'[\"episode-alt\"]','[\"alt-sha\"]')")
        .execute(&f.state.db).await.unwrap();
    let mut fallback = records(None, None).await;
    let episode = fallback
        .iter_mut()
        .find(|r| r["item_type"] == "Episode")
        .unwrap();
    episode["emby_item_ids_json"] = json!(["episode", "episode-alt"]);
    episode["asset_details_json"] =
        json!([{"emby_item_id":"episode","path":"/media/episode.mkv"}, null]);
    let report = sync_upsert(&f.state, &mut fallback).await.unwrap();
    assert_eq!(report.written, 4);
    assert!(report.failures.is_empty());
    assert_eq!(f.keys("Episode", "episode").await, ["789"]);
    assert_eq!(f.keys("Episode", "episode-alt").await, ["890"]);
    let alternate = row(&f.state, "890", "Episode").await.unwrap().unwrap();
    assert_eq!(alternate["season_number"], 2);
    assert_eq!(alternate["episode_number"], 3);
    assert_eq!(alternate["episode_group_id"], "alternate-group");
    assert_eq!(alternate["file_sha1_json"], json!(["alt-sha"]));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn fallback_does_not_guess_when_one_native_version_has_conflicting_real_keys() {
    let f = Fixture::new().await;
    let mut real = records(Some(400), Some(789)).await;
    sync_upsert(&f.state, &mut real).await.unwrap();
    sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,parent_series_tmdb_id,in_library,emby_item_ids_json) VALUES('890','Episode','456',true,'[\"episode\"]')")
        .execute(&f.state.db).await.unwrap();
    let before = row(&f.state, "789", "Episode").await.unwrap().unwrap();
    let mut fallback = records(None, None).await;
    let error = sync_upsert(&f.state, &mut fallback).await.err().unwrap();
    assert!(error.1.contains("多个真实 TMDb 绑定"));
    assert_eq!(f.keys("Episode", "episode").await, ["789", "890"]);
    assert_eq!(
        row(&f.state, "789", "Episode").await.unwrap().unwrap(),
        before
    );
    assert!(
        row(&f.state, "456-S1E1", "Episode")
            .await
            .unwrap()
            .is_none()
    );
    f.close().await;
}
