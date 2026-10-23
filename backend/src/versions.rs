use crate::{
    AppState,
    error::{ApiError, Result},
    identity,
    media_edit::list,
    state::string,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeSet, HashMap, HashSet};

fn ids(value: &Value) -> Vec<String> {
    list(value)
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}
pub fn merge(old: &Value, new: &mut Value) {
    let old_ids = ids(&old["emby_item_ids_json"]);
    let new_ids = ids(&new["emby_item_ids_json"]);
    if old_ids.is_empty() {
        return;
    }
    let mut output = new_ids.clone();
    for id in &old_ids {
        if !output.contains(id) {
            output.push(id.clone());
        }
    }
    let mut slots = HashMap::new();
    for (index, id) in old_ids.iter().enumerate() {
        slots.insert(
            id.clone(),
            (
                list(&old["asset_details_json"])
                    .get(index)
                    .cloned()
                    .unwrap_or(Value::Null),
                list(&old["file_pickcode_json"])
                    .get(index)
                    .cloned()
                    .unwrap_or(Value::Null),
                list(&old["file_sha1_json"])
                    .get(index)
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
        );
    }
    let assets = list(&new["asset_details_json"]);
    let pc = list(&new["file_pickcode_json"]);
    let sha = list(&new["file_sha1_json"]);
    for (index, id) in new_ids.iter().enumerate() {
        let asset = assets.get(index).cloned().unwrap_or(Value::Null);
        let pc = pc.get(index).cloned().unwrap_or(Value::Null);
        let sha = sha.get(index).cloned().unwrap_or(Value::Null);
        let (asset, pc, sha) = if let Some((before, before_pc, before_sha)) = slots.get(id) {
            identity::merge(before, &asset, before_pc, before_sha, &pc, &sha)
        } else {
            (asset, pc, sha)
        };
        slots.insert(id.clone(), (asset, pc, sha));
    }
    new["emby_item_ids_json"] = json!(output);
    new["asset_details_json"] = json!(
        output
            .iter()
            .map(|id| slots[id].0.clone())
            .collect::<Vec<_>>()
    );
    new["file_pickcode_json"] = json!(
        output
            .iter()
            .map(|id| slots[id].1.clone())
            .collect::<Vec<_>>()
    );
    new["file_sha1_json"] = json!(
        output
            .iter()
            .map(|id| slots[id].2.clone())
            .collect::<Vec<_>>()
    );
    new["in_library"] = json!(true);
}
pub async fn lock(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &str,
    tmdb: &str,
    parent: &str,
) -> Result<()> {
    let (kind, work) = if matches!(kind, "Season" | "Episode") && !parent.is_empty() {
        ("Series", parent)
    } else {
        (kind, tmdb)
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("emby-toolkit:versions:{kind}:{work}"))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}
pub async fn unbind(state: &AppState, removed: &[String]) -> Result<u64> {
    if removed.is_empty() {
        return Ok(0);
    }
    let mut transaction = state.db.begin().await?;
    let rows=sqlx::query("SELECT tmdb_id,item_type,parent_series_tmdb_id FROM media_metadata WHERE emby_item_ids_json ?| $1 ORDER BY tmdb_id,item_type").bind(removed).fetch_all(&mut *transaction).await?;
    let mut works = BTreeSet::new();
    for row in &rows {
        let kind: String = row.try_get("item_type")?;
        let id: String = row.try_get("tmdb_id")?;
        let parent: Option<String> = row.try_get("parent_series_tmdb_id")?;
        works.insert(match (kind.as_str(), parent) {
            ("Season" | "Episode", Some(parent)) => ("Series".to_string(), parent),
            _ => (kind, id),
        });
    }
    for (kind, id) in works {
        lock(&mut transaction, &kind, &id, "").await?;
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json ?| $1 ORDER BY tmdb_id,item_type FOR UPDATE").bind(removed).fetch_all(&mut *transaction).await?;
    let removed: HashSet<_> = removed.iter().collect();
    let mut count = 0;
    for row in rows {
        let old_ids = ids(&row["emby_item_ids_json"]);
        let retained = old_ids
            .iter()
            .enumerate()
            .filter(|(_, id)| !removed.contains(id))
            .collect::<Vec<_>>();
        count += old_ids.len() - retained.len();
        let keep = |key: &str| {
            let values = list(&row[key]);
            retained
                .iter()
                .filter_map(|(index, _)| values.get(*index).cloned())
                .collect::<Vec<_>>()
        };
        let assets = keep("asset_details_json");
        sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,file_sha1_json=$3,file_pickcode_json=$4,in_library=$5,last_updated_at=NOW() WHERE tmdb_id=$6 AND item_type=$7")
            .bind(json!(retained.iter().map(|(_,id)|id).collect::<Vec<_>>())).bind(if assets.is_empty(){Value::Null}else{json!(assets)})
            .bind(json!(keep("file_sha1_json"))).bind(json!(keep("file_pickcode_json"))).bind(!retained.is_empty()).bind(string(&row,"tmdb_id")).bind(string(&row,"item_type")).execute(&mut *transaction).await?;
    }
    let removed = removed.into_iter().cloned().collect::<Vec<_>>();
    for table in ["failed_log", "processed_log"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE item_id=ANY($1)"))
            .bind(&removed)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(count as u64)
}
pub async fn existing(state: &AppState, ids: &[String]) -> Result<HashSet<String>> {
    let mut result = HashSet::new();
    // Query each ID through the server endpoint: only an explicit 404 proves deletion.
    for id in ids {
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(ApiError::bad("无效的 Emby ID"));
        }
        let config = state.config.read().await.clone();
        let base = string(&config, "emby_server_url");
        let key = string(&config, "emby_api_key");
        let response = state
            .http
            .get(format!("{}/Items/{id}", base.trim_end_matches('/')))
            .header("X-Emby-Token", key)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        let response: Value = response.error_for_status()?.json().await?;
        if response["Id"].as_str() != Some(id.as_str()) {
            return Err(ApiError::bad("Emby ID 核验响应不一致，已保留绑定"));
        }
        result.insert(id.clone());
    }
    Ok(result)
}
pub async fn reconcile(state: &AppState, current: &[String]) -> Result<u64> {
    if current.is_empty() {
        return Ok(0);
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT emby_item_ids_json FROM media_metadata WHERE item_type IN ('Movie','Episode') AND emby_item_ids_json ?| $1").bind(current).fetch_all(&state.db).await?;
    let mut related: HashSet<_> = rows.iter().flat_map(ids).collect();
    related.extend(current.iter().cloned());
    if related.len() <= current.len() {
        return Ok(0);
    }
    let all = related.iter().cloned().collect::<Vec<_>>();
    let existing = existing(state, &all).await?;
    if current.iter().any(|id| !existing.contains(id)) {
        return Err(ApiError::bad("无法确认本批当前版本状态，已保留旧版本绑定"));
    }
    unbind(
        state,
        &related.difference(&existing).cloned().collect::<Vec<_>>(),
    )
    .await
}
