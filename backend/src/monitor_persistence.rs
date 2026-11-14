//! Durable file events remain linked to their fixed batch across the handoff to a job.
use crate::{
    AppState,
    error::{ApiError, Result},
    monitor::Settings,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub(crate) async fn record(state: &AppState, path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| ApiError::bad("实时监控路径不是有效 UTF-8"))?;
    sqlx::query("INSERT INTO monitor_pending_events(id,path) VALUES($1,$2) ON CONFLICT(path) WHERE batch_id IS NULL DO UPDATE SET updated_at=NOW()")
        .bind(uuid::Uuid::new_v4().to_string()).bind(path).execute(&state.db).await?;
    Ok(())
}

pub(crate) async fn retain(state: &AppState, settings: &Settings) -> Result<()> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT p.id,p.path FROM monitor_pending_events p WHERE p.batch_id IS NULL OR NOT EXISTS(SELECT 1 FROM durable_jobs j WHERE j.lane='monitor' AND j.spec_json->>'kind'='monitor' AND j.spec_json->'payload'->>'batch_id'=p.batch_id)")
            .fetch_all(&state.db)
            .await?;
    let mut removed = vec![];
    for (id, path) in rows {
        if !settings.enabled || settings.accepts(Path::new(&path)).await.is_none() {
            removed.push(id);
        }
    }
    sqlx::query("DELETE FROM monitor_pending_events p WHERE p.id=ANY($1) AND (p.batch_id IS NULL OR NOT EXISTS(SELECT 1 FROM durable_jobs j WHERE j.lane='monitor' AND j.spec_json->>'kind'='monitor' AND j.spec_json->'payload'->>'batch_id'=p.batch_id))")
        .bind(removed)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub(crate) async fn next(state: &AppState, settings: &Settings) -> Result<Option<Value>> {
    let mut transaction = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('emby-toolkit:monitor-events'))")
        .execute(&mut *transaction)
        .await?;
    // A crash after creating the durable job must not turn its retained events into another job.
    sqlx::query("DELETE FROM monitor_pending_events p USING durable_jobs j WHERE j.lane='monitor' AND j.spec_json->>'kind'='monitor' AND p.batch_id=j.spec_json->'payload'->>'batch_id' AND j.state_json->>'state' IN ('completed','failed','cancelled')")
        .execute(&mut *transaction).await?;
    let batch: Option<String> = sqlx::query_scalar("SELECT p.batch_id FROM monitor_pending_events p WHERE p.batch_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM durable_jobs j WHERE j.lane='monitor' AND j.spec_json->>'kind'='monitor' AND j.spec_json->'payload'->>'batch_id'=p.batch_id) GROUP BY p.batch_id ORDER BY MIN(p.created_at),p.batch_id LIMIT 1")
        .fetch_optional(&mut *transaction).await?;
    let (batch, rows, snapshot) = if let Some(batch) = batch {
        let rows: Vec<(String, String, Option<Value>)> = sqlx::query_as(
            "SELECT id,path,settings_json FROM monitor_pending_events WHERE batch_id=$1 ORDER BY path,id",
        )
        .bind(&batch).fetch_all(&mut *transaction).await?;
        let snapshot = rows
            .first()
            .and_then(|row| row.2.clone())
            .unwrap_or_else(|| json!(settings));
        (
            batch,
            rows.into_iter()
                .map(|(id, path, _)| (id, path))
                .collect::<Vec<_>>(),
            snapshot,
        )
    } else {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id,path FROM monitor_pending_events WHERE batch_id IS NULL ORDER BY created_at,id FOR UPDATE",
        )
        .fetch_all(&mut *transaction).await?;
        if rows.is_empty() {
            transaction.commit().await?;
            return Ok(None);
        }
        let batch = uuid::Uuid::new_v4().to_string();
        let snapshot = json!(settings);
        let ids = rows.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        sqlx::query("UPDATE monitor_pending_events SET batch_id=$1,settings_json=$2 WHERE id=ANY($3) AND batch_id IS NULL")
            .bind(&batch).bind(&snapshot).bind(ids).execute(&mut *transaction).await?;
        (batch, rows, snapshot)
    };
    let payload = json!({"batch_id":batch,"pending_ids":rows.iter().map(|row|&row.0).collect::<Vec<_>>(),
        "paths":rows.iter().map(|row|&row.1).collect::<Vec<_>>(),"settings":snapshot});
    transaction.commit().await?;
    Ok(Some(payload))
}

pub(crate) async fn revision(paths: &[PathBuf]) -> Result<Value> {
    let mut revision = vec![];
    for path in paths {
        let metadata = tokio::fs::metadata(path).await?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApiError::bad("实时监控文件时间无效"))?
            .as_nanos()
            .to_string();
        let mut entry = json!({"path":path,"size":metadata.len(),"modified":modified});
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            entry["device"] = json!(metadata.dev());
            entry["inode"] = json!(metadata.ino());
            entry["ctime"] = json!([metadata.ctime(), metadata.ctime_nsec()]);
        }
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("strm"))
        {
            entry["source_hash"] = json!(format!(
                "{:x}",
                Sha256::digest(tokio::fs::read(path).await?)
            ));
        }
        revision.push(entry);
    }
    Ok(json!(revision))
}
