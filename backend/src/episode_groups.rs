use crate::{AppState, error::Result, media_edit::list, state::string};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn normalize(raw: &str) -> String {
    regex::Regex::new(r"[^\w\u4e00-\u9fff]+")
        .unwrap()
        .replace_all(raw, "")
        .to_lowercase()
}
fn integer(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
pub fn select(
    signals: &[Value],
    groups: &BTreeMap<String, Value>,
    official: &Value,
    preferred: &str,
) -> Option<String> {
    let signals = signals
        .iter()
        .filter(|s| {
            integer(&s["season_number"]).is_some() && integer(&s["episode_number"]).is_some()
        })
        .collect::<Vec<_>>();
    if signals.is_empty() {
        return None;
    }
    let regex = regex::Regex::new(r"(?i)(?:^|[;\[])g=([0-9a-f]{8,})(?:;|\]|$)").unwrap();
    let explicit = signals
        .iter()
        .filter_map(|s| regex.captures(&string(s, "path")).map(|c| c[1].to_string()))
        .collect::<BTreeSet<_>>();
    let candidates = groups
        .iter()
        .filter(|(id, _)| explicit.is_empty() || explicit.contains(*id))
        .collect::<Vec<_>>();
    if !explicit.is_empty() && candidates.len() != 1 {
        return None;
    }
    let identities = signals
        .iter()
        .filter(|s| {
            !string(s, "tmdb_episode_id").is_empty()
                || !string(s, "air_date").is_empty()
                || !normalize(&string(s, "title")).is_empty()
        })
        .count();
    let mut counts = BTreeMap::<i64, BTreeSet<i64>>::new();
    for signal in &signals {
        counts
            .entry(integer(&signal["season_number"]).unwrap())
            .or_default()
            .insert(integer(&signal["episode_number"]).unwrap());
    }
    let complete = counts
        .into_iter()
        .filter_map(|(season, episodes)| {
            let max = *episodes.last()?;
            (max > 0 && episodes == (1..=max).collect::<BTreeSet<_>>()).then_some((season, max))
        })
        .collect::<BTreeMap<_, _>>();
    let matches = candidates
        .into_iter()
        .filter(|(_, details)| {
            let mut positions = BTreeMap::new();
            for group in list(&details["groups"]) {
                let Some(season) = integer(&group["order"]) else {
                    continue;
                };
                for (index, episode) in list(&group["episodes"]).into_iter().enumerate() {
                    positions.insert((season, index as i64 + 1), episode);
                }
            }
            signals.iter().all(|s| {
                let Some(episode) = positions.get(&(
                    integer(&s["season_number"]).unwrap(),
                    integer(&s["episode_number"]).unwrap(),
                )) else {
                    return false;
                };
                let id = string(s, "tmdb_episode_id");
                let date = string(s, "air_date");
                let title = normalize(&string(s, "title"));
                if !id.is_empty() {
                    return episode["id"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or(episode["id"].to_string())
                        == id;
                }
                if !date.is_empty() {
                    return string(episode, "air_date")
                        .chars()
                        .take(10)
                        .collect::<String>()
                        == date.chars().take(10).collect::<String>();
                }
                title.is_empty() || normalize(&string(episode, "name")) == title
            })
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return None;
    }
    if !explicit.is_empty() && matches.len() == 1 {
        return Some(matches[0].0.clone());
    }
    let official_counts = list(&official["seasons"])
        .iter()
        .filter_map(|s| {
            Some((
                integer(&s["season_number"])?,
                integer(&s["episode_count"]).unwrap_or(0),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    if !complete.is_empty()
        && identities == 0
        && !official_counts.is_empty()
        && complete
            .iter()
            .all(|(s, n)| official_counts.get(s) == Some(n))
    {
        return None;
    }
    let counts_match = |details: &Value| {
        let counts = list(&details["groups"])
            .iter()
            .filter_map(|g| Some((integer(&g["order"])?, list(&g["episodes"]).len() as i64)))
            .collect::<BTreeMap<_, _>>();
        complete.iter().all(|(s, n)| counts.get(s) == Some(n))
    };
    if let Some((id, _)) = matches
        .iter()
        .find(|(id, details)| id.as_str() == preferred && counts_match(details))
    {
        return Some((*id).clone());
    }
    if matches.len() == 1 && identities == signals.len() {
        return Some(matches[0].0.clone());
    }
    let exact = matches
        .iter()
        .filter(|(_, details)| !complete.is_empty() && counts_match(details))
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return Some(exact[0].0.clone());
    }
    if matches.len() == 1 && identities > 0 {
        return Some(matches[0].0.clone());
    }
    None
}
pub async fn match_group(
    state: &AppState,
    id: &str,
    signals: &[Value],
    preferred: &str,
) -> Result<Option<String>> {
    if signals.is_empty() {
        return Ok(None);
    }
    let first = crate::tmdb::get(state, &format!("/tv/{id}/episode_groups"), &[]).await?;
    let mut summaries = list(&first["results"]);
    for page in 2..=first["total_pages"].as_u64().unwrap_or(1) {
        let value = crate::tmdb::get(
            state,
            &format!("/tv/{id}/episode_groups"),
            &[("page", page.to_string())],
        )
        .await?;
        summaries.extend(list(&value["results"]));
    }
    let mut groups = BTreeMap::new();
    for summary in summaries {
        let group = string(&summary, "id");
        if !group.is_empty()
            && let Ok(details) =
                crate::tmdb::get(state, &format!("/tv/episode_group/{group}"), &[]).await
        {
            groups.insert(group, details);
        }
    }
    let official = if !signals.iter().any(|s| {
        !string(s, "tmdb_episode_id").is_empty()
            || !string(s, "air_date").is_empty()
            || !normalize(&string(s, "title")).is_empty()
    }) {
        crate::tmdb::get(
            state,
            &format!("/tv/{id}"),
            &[("append_to_response", "".into())],
        )
        .await
        .unwrap_or(json!({}))
    } else {
        json!({})
    };
    Ok(select(signals, &groups, &official, preferred))
}
pub fn signals(episodes: &[Value]) -> Vec<Value> {
    episodes.iter().filter_map(|e|{
    let season=integer(&e["ParentIndexNumber"])?;let number=integer(&e["IndexNumber"])?;
    let name=string(e,"Name");let filename=std::path::Path::new(e["Path"].as_str().unwrap_or("")).file_stem().unwrap_or_default().to_string_lossy();
    let generic=name.is_empty()||name==filename||regex::Regex::new(r"(?i)^(?:第\s*\d+\s*集|episode\s*\d+)$").unwrap().is_match(&name);
    Some(json!({"season_number":season,"episode_number":number,"tmdb_episode_id":e["ProviderIds"]["Tmdb"].as_str().unwrap_or(""),"air_date":e["PremiereDate"].as_str().unwrap_or(""),"title":if generic{""}else{&name},"path":e["Path"]}))
}).collect()
}
