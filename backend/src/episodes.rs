use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit, media_info,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/media_info/series/{id}/episodes", get(get_episodes))
}
fn num(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
pub fn structure(info: &Value) -> Option<Value> {
    let source = media_info::container(info)?;
    let streams = source["MediaStreams"].as_array()?;
    let audio=streams.iter().filter(|s|s["Type"]=="Audio").map(|s|json!({"codec":s["Codec"].as_str().filter(|s|!s.is_empty()).unwrap_or("未知编码").trim().to_lowercase(),"channels":s["Channels"].as_i64().map(|n|n.to_string()).unwrap_or("未知声道".into()),"language":string(s,"Language").trim().to_lowercase(),"title":string(s,"Title").trim()})).collect::<Vec<_>>();
    let subtitles=streams.iter().filter(|s|s["Type"]=="Subtitle").map(|s|json!({"codec":s["Codec"].as_str().filter(|s|!s.is_empty()).unwrap_or("未知编码").trim().to_lowercase(),"external":s["IsExternal"]==true,"language":string(s,"Language").trim().to_lowercase(),"title":string(s,"Title").trim()})).collect::<Vec<_>>();
    Some(json!({"audio":audio,"subtitles":subtitles}))
}
pub fn asset_structure(asset: &Value) -> Option<Value> {
    let audio=asset["audio_tracks"].as_array()?.iter().map(|t|json!({"codec":t["codec"].as_str().filter(|s|!s.is_empty()).unwrap_or("未知编码").trim().to_lowercase(),"channels":t["channels"].as_i64().map(|n|n.to_string()).unwrap_or("未知声道".into()),"language":string(t,"language").trim().to_lowercase(),"title":string(t,"display_title").trim()})).collect::<Vec<_>>();
    let subtitles=asset["subtitles"].as_array()?.iter().map(|t|json!({"codec":t["format"].as_str().or_else(||t["codec"].as_str()).filter(|s|!s.is_empty()).unwrap_or("未知编码").trim().to_lowercase(),"external":null,"language":string(t,"language").trim().to_lowercase(),"title":string(t,"display_title").trim()})).collect::<Vec<_>>();
    Some(json!({"audio":audio,"subtitles":subtitles}))
}
pub fn lanes(items: &mut [Value], orders: &HashMap<i64, Vec<String>>) {
    let mut seasons = BTreeMap::<i64, Vec<usize>>::new();
    for (i, item) in items.iter().enumerate() {
        seasons
            .entry(num(&item["season_number"]))
            .or_default()
            .push(i);
    }
    for (season, positions) in seasons {
        let mut episodes = BTreeMap::<i64, Vec<usize>>::new();
        for i in &positions {
            episodes
                .entry(num(&items[*i]["episode_number"]))
                .or_default()
                .push(*i);
        }
        let count = episodes.values().map(Vec::len).max().unwrap_or(1);
        let native = orders.get(&season).cloned().unwrap_or_default();
        let numeric = positions
            .iter()
            .map(|i| string(&items[*i], "emby_id").parse::<u64>().ok())
            .collect::<Option<Vec<_>>>();
        let mut assigned = false;
        if let Some(mut ids) = numeric
            && native.len() >= count
            && count > 1
        {
            ids.sort_unstable();
            ids.dedup();
            let mut gaps = ids
                .windows(2)
                .enumerate()
                .map(|(i, p)| (p[1] - p[0], i))
                .collect::<Vec<_>>();
            gaps.sort_by(|a, b| b.cmp(a));
            gaps.truncate(count - 1);
            if gaps.len() == count - 1 && gaps.iter().all(|(gap, _)| *gap > 1) {
                let mut splits = gaps.iter().map(|(_, i)| *i).collect::<Vec<_>>();
                splits.sort_unstable();
                let block = |id: u64| splits.iter().filter(|split| id > ids[**split]).count();
                let mapping = native
                    .iter()
                    .take(count)
                    .enumerate()
                    .filter_map(|(i, id)| Some((block(id.parse().ok()?), i + 1)))
                    .collect::<HashMap<_, _>>();
                if mapping.len() == count {
                    for i in &positions {
                        items[*i]["version_index"] =
                            json!(mapping[&block(string(&items[*i], "emby_id").parse().unwrap())]);
                    }
                    assigned = true;
                }
            }
        }
        if !assigned {
            for group in episodes.values_mut() {
                group.sort_by_key(|i| num(&items[*i]["_version_order"]));
                for (lane, i) in group.iter().enumerate() {
                    items[*i]["version_index"] = json!(lane + 1);
                }
            }
        }
        let mut summaries = HashMap::<i64, Vec<(String, usize)>>::new();
        for i in &positions {
            let lane = num(&items[*i]["version_index"]).max(1);
            let summary = string(&items[*i], "version_summary");
            if summary.is_empty() {
                continue;
            }
            let list = summaries.entry(lane).or_default();
            if let Some((_, count)) = list.iter_mut().find(|(s, _)| *s == summary) {
                *count += 1;
            } else {
                list.push((summary, 1));
            }
        }
        for i in positions {
            let lane = num(&items[i]["version_index"]).max(1);
            let mut summary = string(&items[i], "version_summary");
            if summary.is_empty() {
                summary = summaries
                    .get(&lane)
                    .and_then(|values| {
                        values
                            .iter()
                            .enumerate()
                            .max_by_key(|(i, (_, count))| (*count, std::cmp::Reverse(*i)))
                    })
                    .map(|(_, (s, _))| s.clone())
                    .unwrap_or(format!("版本 {lane}"));
            }
            items[i]["season_version_count"] = json!(count);
            items[i]["version_label"] = json!(if count > 1 {
                format!("版本 {lane} · {summary}")
            } else if summary.is_empty() {
                "版本 1".into()
            } else {
                summary
            });
            items[i].as_object_mut().unwrap().remove("_version_order");
        }
    }
}
fn describe(track: &Value) -> String {
    let codec = track["codec"].as_str().unwrap_or("未知编码").to_uppercase();
    let channels = track["channels"].as_str().unwrap_or("未知声道");
    format!(
        "{codec} {} Language={} Title={}",
        if channels == "未知声道" {
            channels.into()
        } else {
            format!("{channels}ch")
        },
        track["language"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("<空>"),
        track["title"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("<空>")
    )
}
pub fn annotate(items: &mut [Value]) {
    let mut groups = BTreeMap::<(i64, i64), Vec<usize>>::new();
    for (i, item) in items.iter().enumerate() {
        groups
            .entry((
                num(&item["season_number"]),
                num(&item["version_index"]).max(1),
            ))
            .or_default()
            .push(i);
    }
    for positions in groups.values() {
        let mut counts = Vec::<(usize, usize, Value)>::new();
        for i in positions {
            let value = items[*i]
                .as_object_mut()
                .unwrap()
                .remove("_track_structure")
                .unwrap_or(Value::Null);
            if value.is_null() {
                items[*i]["track_status"] = json!("unavailable");
                items[*i]["track_difference"] = json!("");
                continue;
            }
            let count = value["audio"].as_array().map(Vec::len).unwrap_or(0);
            if let Some((_, total, old)) = counts.iter_mut().find(|(n, _, _)| *n == count) {
                *total += 1;
                *old = value.clone();
            } else {
                counts.push((count, 1, value.clone()));
            }
            items[*i]["track_structure"] = value;
        }
        let baseline = counts
            .iter()
            .enumerate()
            .max_by_key(|(i, (_, count, _))| (*count, std::cmp::Reverse(*i)))
            .map(|(_, (_, _, s))| s.clone());
        for i in positions {
            let structure = items[*i]
                .as_object_mut()
                .unwrap()
                .remove("track_structure")
                .unwrap_or(Value::Null);
            if structure.is_null() {
                continue;
            }
            let expected = baseline.as_ref().unwrap();
            let before = expected["audio"].as_array().unwrap();
            let after = structure["audio"].as_array().unwrap();
            if before.len() == after.len() {
                items[*i]["track_status"] = json!("consistent");
                items[*i]["track_difference"] = json!("");
            } else {
                items[*i]["track_status"] = json!("inconsistent");
                let missing = after.len() < before.len();
                let (larger, smaller) = if missing {
                    (before, after)
                } else {
                    (after, before)
                };
                let mut remaining = smaller.clone();
                let mut difference = vec![];
                for (index, track) in larger.iter().enumerate() {
                    if let Some(i) = remaining.iter().position(|s| s == track) {
                        remaining.remove(i);
                    } else {
                        difference.push(format!("音轨{} {}", index + 1, describe(track)));
                    }
                }
                items[*i]["track_difference"] = json!(if remaining.is_empty() {
                    format!(
                        "音轨数量 {}→{}（{} {}）",
                        before.len(),
                        after.len(),
                        if missing { "缺少" } else { "多出" },
                        difference.join("、")
                    )
                } else {
                    format!(
                        "音轨数量 {}→{}（轨道属性同时变化，无法可靠定位增减音轨）",
                        before.len(),
                        after.len()
                    )
                });
            }
        }
    }
}
async fn get_episodes(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let parent:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1 AND item_type IN ('Series','Season') LIMIT 1").bind(json!([id])).fetch_optional(&state.db).await?;
    let parent = parent.ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "未在数据库中找到该剧集或季信息".into(),
        )
    })?;
    if parent["item_type"] == "Season"
        && (string(&parent, "parent_series_tmdb_id").is_empty()
            || parent["season_number"].is_null())
    {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "该季缺少父剧集或季号信息".into(),
        ));
    }
    let (series, season) = if parent["item_type"] == "Season" {
        (
            string(&parent, "parent_series_tmdb_id"),
            parent["season_number"].as_i64(),
        )
    } else {
        (string(&parent, "tmdb_id"), None)
    };
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE parent_series_tmdb_id=$1 AND item_type='Episode' AND in_library=true AND ($2::integer IS NULL OR season_number=$2) ORDER BY season_number,episode_number").bind(series).bind(season.map(|n|n as i32)).fetch_all(&state.db).await?;
    let mut result = vec![];
    for row in rows {
        let ids = media_edit::list(&row["emby_item_ids_json"]);
        let assets = media_edit::list(&row["asset_details_json"]);
        for (index, id) in ids.iter().filter_map(Value::as_str).enumerate() {
            let target = media_edit::target(&state, id).await?;
            let Some(version) = target.versions.iter().find(|v| v["emby_id"] == id) else {
                continue;
            };
            let info = media_edit::load(&state, &target).await.ok();
            let tracks = info
                .as_ref()
                .and_then(structure)
                .or_else(|| assets.get(index).and_then(asset_structure));
            result.push(json!({"emby_id":id,"season_number":row["season_number"],"episode_number":row["episode_number"],"title":row["title"],"version_count":target.versions.len(),"version_summary":version["base_label"],"version_path":version["path"],"_version_order":index,"_track_structure":tracks}));
        }
    }
    let mut orders = HashMap::new();
    let mut groups = BTreeMap::<i64, BTreeMap<i64, Vec<Value>>>::new();
    for item in &result {
        groups
            .entry(num(&item["season_number"]))
            .or_default()
            .entry(num(&item["episode_number"]))
            .or_default()
            .push(item.clone());
    }
    for (season, episodes) in groups {
        if let Some(anchor) = episodes
            .values()
            .max_by_key(|v| v.len())
            .filter(|v| v.len() > 1)
        {
            let item =
                media_edit::details(&state, &string(&anchor[0], "emby_id"), "MediaSources").await?;
            let order = media_edit::list(&item["MediaSources"])
                .iter()
                .map(|s| {
                    s["ItemId"]
                        .as_str()
                        .or_else(|| s["Id"].as_str())
                        .unwrap_or("")
                        .replace("mediasource_", "")
                })
                .filter(|id| anchor.iter().any(|a| a["emby_id"] == *id))
                .collect::<Vec<_>>();
            if order.len() == anchor.len() {
                orders.insert(season, order);
            }
        }
    }
    lanes(&mut result, &orders);
    annotate(&mut result);
    Ok(Json(json!(result)))
}
