use crate::{AppState, error::Result, jobs::JobContext, media_edit::list, state::string};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::HashMap;

pub(super) async fn server_key(state: &AppState) -> String {
    let config = state.config.read().await;
    let id = string(&config, "emby_server_id");
    if id.is_empty() {
        crate::jobs::server_url(&config).unwrap_or_default()
    } else {
        id
    }
}

pub(super) async fn identity(state: &AppState, item: &Value) -> Result<Value> {
    // SHA1 distinguishes a replacement at the same path. Native source details
    // also invalidate failures when extraction inputs or audio selection change.
    let hashes: Option<Value> = sqlx::query_scalar(
        "SELECT file_sha1_json FROM media_metadata WHERE emby_item_ids_json @> $1::jsonb LIMIT 1",
    )
    .bind(json!([item["Id"]]))
    .fetch_optional(&state.db)
    .await?;
    let sources: Vec<_> = list(&item["MediaSources"])
        .iter()
        .map(|source| {
            json!({
                "path":source["Path"], "size":source["Size"], "runtime":source["RunTimeTicks"],
                "audio":super::fingerprint_audio_identity(source),
            })
        })
        .collect();
    Ok(
        json!({"path":item["Path"],"runtime":item["RunTimeTicks"],"sha1":hashes,
        "sources":sources,"audio":super::fingerprint_audio_identity(item)}),
    )
}

pub(super) async fn load(state: &AppState, server: &str) -> Result<HashMap<String, Value>> {
    let rows =
        sqlx::query("SELECT item_id,file_identity FROM credits_failures WHERE server_key=$1")
            .bind(server)
            .fetch_all(&state.db)
            .await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("item_id")?, row.try_get("file_identity")?)))
        .collect()
}

pub(super) async fn record(
    state: &AppState,
    server: &str,
    item: &Value,
    report: &Value,
    context: &JobContext,
) -> Result<()> {
    if report["status"] == "failed" {
        let identity = identity(state, item).await?;
        let name = format!(
            "{} S{:02}E{:02} {}",
            string(item, "SeriesName"),
            item["ParentIndexNumber"].as_u64().unwrap_or(0),
            item["IndexNumber"].as_u64().unwrap_or(0),
            string(item, "Name")
        );
        let mut transaction = state.db.begin().await?;
        sqlx::query("INSERT INTO credits_failures(server_key,item_id,item_name,file_identity,reason) VALUES($1,$2,$3,$4,$5) ON CONFLICT(server_key,item_id) DO UPDATE SET item_name=EXCLUDED.item_name,file_identity=EXCLUDED.file_identity,reason=EXCLUDED.reason,failed_at=NOW()")
            .bind(server).bind(string(item,"Id")).bind(name).bind(identity)
            .bind(string(report,"error")).execute(&mut *transaction).await?;
        context.complete_in_tx(&mut transaction, &format!("credits:{}", string(item, "Id")), report).await?;
        transaction.commit().await?;
    } else if report["status"] == "completed" || report["chapter"].is_object() {
        clear(state, server, &string(item, "Id")).await?;
    }
    Ok(())
}

pub(super) async fn clear(state: &AppState, server: &str, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM credits_failures WHERE server_key=$1 AND item_id=$2")
        .bind(server).bind(id).execute(&state.db).await?;
    Ok(())
}
