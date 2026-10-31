use crate::{
    AppState, emby,
    error::{ApiError, Result},
    media_edit, media_info, media_paths,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

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
async fn native_orders(items: &[Value], native: &[Value]) -> HashMap<(i64, i64), Vec<String>> {
    let native = native
        .iter()
        .map(|item| (string(item, "Id"), item))
        .collect::<HashMap<_, _>>();
    let mut groups = BTreeMap::<(i64, i64), Vec<usize>>::new();
    for (i, item) in items.iter().enumerate() {
        groups
            .entry((num(&item["season_number"]), num(&item["episode_number"])))
            .or_default()
            .push(i);
    }
    let mut orders = HashMap::new();
    for (key, group) in groups {
        if group.len() < 2 {
            continue;
        }
        let mut paths = HashMap::<usize, Vec<String>>::new();
        for i in &group {
            let item = native.get(&string(&items[*i], "emby_id"));
            for path in [
                string(&items[*i], "version_path"),
                item.map(|item| string(item, "Path")).unwrap_or_default(),
            ] {
                let path = media_paths::clean(&path);
                if !path.is_empty() {
                    paths.entry(*i).or_default().push(path.clone());
                    if let Some(target) = media_paths::strm_target(&path).await {
                        paths
                            .entry(*i)
                            .or_default()
                            .push(media_paths::clean(&target));
                    }
                }
            }
        }
        for i in &group {
            let Some(sources) = native
                .get(&string(&items[*i], "emby_id"))
                .and_then(|item| item["MediaSources"].as_array())
            else {
                continue;
            };
            let mut order = vec![];
            for source in sources {
                let matched = ["ItemId", "Id"]
                    .iter()
                    .find_map(|field| {
                        source[*field]
                            .as_str()
                            .and_then(|id| group.iter().find(|i| items[**i]["emby_id"] == id))
                    })
                    .or_else(|| {
                        let path = media_paths::clean(&string(source, "Path"));
                        let mut matches = group.iter().filter(|i| {
                            !path.is_empty()
                                && paths.get(i).is_some_and(|paths| paths.contains(&path))
                        });
                        let matched = matches.next();
                        matched.filter(|_| matches.next().is_none())
                    });
                if let Some(i) = matched {
                    let id = string(&items[*i], "emby_id");
                    if !order.contains(&id) {
                        order.push(id);
                    }
                }
            }
            if order.len() == group.len() {
                orders.insert(key, order);
                break;
            }
        }
    }
    orders
}
pub fn lanes(items: &mut [Value], orders: &HashMap<(i64, i64), Vec<String>>) {
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
        for (episode, group) in &mut episodes {
            let order = orders.get(&(season, *episode)).filter(|order| {
                order.len() == group.len()
                    && order.iter().collect::<HashSet<_>>().len() == group.len()
                    && group
                        .iter()
                        .all(|i| order.contains(&string(&items[*i], "emby_id")))
            });
            group.sort_by_key(|i| {
                order
                    .and_then(|order| order.iter().position(|id| items[*i]["emby_id"] == *id))
                    .map(|i| i as i64)
                    .unwrap_or_else(|| num(&items[*i]["_version_order"]))
            });
            for (lane, i) in group.iter().enumerate() {
                items[*i]["version_index"] = json!(lane + 1);
                items[*i]["version_count"] = json!(group.len());
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
    let mut seen = HashSet::new();
    for row in rows {
        let ids = media_edit::list(&row["emby_item_ids_json"]);
        let assets = media_edit::list(&row["asset_details_json"]);
        for (index, id) in ids.iter().enumerate() {
            let Some(id) = id.as_str() else {
                continue;
            };
            if id.is_empty() || !seen.insert(id.to_string()) {
                continue;
            }
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
    let ids = result
        .iter()
        .map(|item| string(item, "emby_id"))
        .collect::<Vec<_>>();
    let native = emby::by_ids(&state, &ids, "Path,MediaSources", None).await?;
    let orders = native_orders(&result, &native).await;
    lanes(&mut result, &orders);
    annotate(&mut result);
    Ok(Json(json!(result)))
}
