use crate::{AppState, error::Result};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

pub(crate) type Key = (String, String, i64, i64, u64);

fn text<'a>(item: &'a Value, fields: &[&str]) -> &'a str {
    fields
        .iter()
        .find_map(|field| item[*field].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("")
}

fn number(item: &Value, fields: &[&str]) -> i64 {
    fields
        .iter()
        .find_map(|field| {
            item[*field]
                .as_i64()
                .or_else(|| item[*field].as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(-1)
}

pub(crate) fn key(item: &Value) -> Key {
    let kind = text(item, &["item_type", "Type"]);
    let child = matches!(kind, "Season" | "Episode");
    let parent = text(item, &["parent_series_tmdb_id", "SeriesId"]);
    let title = if child {
        text(item, &["series_title", "parent_series_title", "SeriesName"])
    } else {
        ""
    };
    let title = if title.is_empty() && child && !parent.is_empty() {
        parent
    } else if title.is_empty() {
        text(
            item,
            &[
                "title",
                "Name",
                "primary_name",
                "original_title",
                "path",
                "Path",
            ],
        )
    } else {
        title
    };
    let identity = if child && !parent.is_empty() {
        parent
    } else {
        text(item, &["tmdb_id", "Id", "item_id"])
    };
    let season = match kind {
        "Season" => number(item, &["season_number", "IndexNumber"]),
        "Episode" => number(item, &["season_number", "ParentIndexNumber"]),
        _ => -1,
    };
    let episode = if kind == "Episode" {
        number(item, &["episode_number", "IndexNumber"])
    } else {
        -1
    };
    (
        title.to_lowercase(),
        identity.into(),
        season,
        episode,
        item["_task_version"].as_u64().unwrap_or(0),
    )
}

pub(crate) fn sort(items: &mut [Value]) {
    items.sort_by_cached_key(key);
}

pub(crate) async fn sort_rows(state: &AppState, rows: &mut [Value]) -> Result<()> {
    let parents = rows
        .iter()
        .filter(|row| text(row, &["series_title", "parent_series_title"]).is_empty())
        .map(|row| text(row, &["parent_series_tmdb_id"]))
        .filter(|id| !id.is_empty())
        .collect::<BTreeSet<_>>();
    let names: HashMap<String, String> = if parents.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT tmdb_id,COALESCE(NULLIF(title,''),NULLIF(original_title,''),tmdb_id) FROM media_metadata WHERE item_type='Series' AND tmdb_id=ANY($1)",
        )
        .bind(parents.into_iter().collect::<Vec<_>>())
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|(id, name)| name.filter(|name| !name.is_empty()).map(|name| (id, name)))
        .collect()
    };
    for row in rows.iter_mut() {
        if let Some(name) = names.get(text(row, &["parent_series_tmdb_id"])) {
            row["series_title"] = Value::String(name.clone());
        }
    }
    sort(rows);
    Ok(())
}

pub(crate) async fn id_keys(state: &AppState, ids: &[String]) -> Result<HashMap<String, Key>> {
    let rows = crate::revalidation::fingerprint_rows(state, Some(ids)).await?;
    let mut keys = HashMap::new();
    for row in rows {
        let ids = crate::media_edit::list(&row["emby_item_ids_json"]);
        for (index, asset) in crate::media_edit::list(&row["asset_details_json"])
            .iter()
            .enumerate()
        {
            let mut key = key(&row);
            key.4 = index as u64;
            let id = asset["emby_item_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .or_else(|| ids.get(index).and_then(Value::as_str))
                .unwrap_or("");
            keys.entry(id.to_string()).or_insert(key);
        }
    }
    Ok(keys)
}

pub(crate) async fn sort_ids(state: &AppState, ids: &mut [String]) -> Result<()> {
    if ids.len() < 2 {
        return Ok(());
    }
    let keys = id_keys(state, ids).await?;
    ids.sort_by_cached_key(|id| keys.get(id).cloned());
    Ok(())
}

pub(crate) async fn sort_items(state: &AppState, items: &mut [Value]) -> Result<()> {
    if items.len() < 2 {
        return Ok(());
    }
    let ids = items
        .iter()
        .map(|item| text(item, &["Id"]).to_string())
        .collect::<Vec<_>>();
    let keys = id_keys(state, &ids).await?;
    items.sort_by_cached_key(|item| {
        keys.get(text(item, &["Id"]))
            .cloned()
            .unwrap_or_else(|| key(item))
    });
    Ok(())
}
