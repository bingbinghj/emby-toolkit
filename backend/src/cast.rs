use crate::{media_edit::list, state::string};
use serde_json::{Value, json};
use std::sync::LazyLock;

fn chinese(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}
pub fn clean_character(raw: &str) -> String {
    static RULES: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
        [
            r"\(.*?\)|\[.*?\]|（.*?）|【.*?】",
            r"(?i)^(as\s+)",
            r"(?i)^((?:饰演|饰|扮演|扮|配音|配|as\b)\s*)+",
            r"(\s*(?:饰演|饰|配音|配))+$",
            r"^[a-zA-Z][a-zA-Z\s._-]*",
            r"[a-zA-Z][a-zA-Z\s._-]*$",
        ]
        .iter()
        .map(|p| regex::Regex::new(p).unwrap())
        .collect()
    });
    let mut name = raw.trim().to_string();
    for rule in &RULES[..4] {
        name = rule.replace_all(&name, "").trim().to_string();
    }
    if chinese(&name) && name.chars().any(|c| c.is_ascii_alphabetic()) {
        if let Some(part) = name.split(['/', '|']).find(|s| chinese(s)) {
            name = part.trim().to_string();
        }
        if let Some(found) = RULES[4].find(&name)
            && name[found.end()..]
                .chars()
                .next()
                .is_some_and(|c| ('\u{4e00}'..='\u{9fa5}').contains(&c))
        {
            name = name[found.end()..].trim().to_string();
        }
        if let Some(found) = RULES[5].find(&name)
            && name[..found.start()]
                .trim_end()
                .chars()
                .next_back()
                .is_some_and(|c| ('\u{4e00}'..='\u{9fa5}').contains(&c))
        {
            name = name[..found.start()].trim().to_string();
        }
        name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    name
}
pub fn format(raw: &[Value], item: &Value, config: &Value) -> Vec<Value> {
    let animation = list(&item["Genres"]).iter().any(|g| {
        let name = g
            .as_str()
            .unwrap_or_else(|| g["name"].as_str().unwrap_or(""));
        ["Animation", "动画", "Documentary", "纪录", "记录"].contains(&name)
    });
    let mut cast = raw.to_vec();
    for actor in &mut cast {
        let mut role = clean_character(&string(actor, "character"));
        if chinese(&role) {
            role = role.replace([' ', '　'], "");
        }
        if role.is_empty() {
            role = if animation { "配音" } else { "演员" }.into();
        } else if config["actor_role_add_prefix"] == true
            && !["演员", "配音"].contains(&role.as_str())
        {
            role = format!("{} {role}", if animation { "配" } else { "饰" });
        }
        actor["character"] = json!(role);
    }
    cast.sort_by_key(|p| p["order"].as_i64().unwrap_or(999));
    for (index, actor) in cast.iter_mut().enumerate() {
        actor["order"] = json!(index);
    }
    cast
}
pub fn relations(cast: &[Value]) -> Value {
    json!(cast.iter().filter(|p|p["id"].as_i64().is_some_and(|id|id!=0))
        .map(|p|json!({"tmdb_id":p["id"],"character":p["character"],"order":p.get("order").cloned().unwrap_or(json!(999))})).collect::<Vec<_>>())
}
pub fn manual(raw: &[Value], item: &Value, config: &Value) -> Vec<Value> {
    let mut actors = raw.to_vec();
    for (i, actor) in actors.iter_mut().enumerate() {
        actor["order"] = json!(i);
    }
    let mut actors = format(&actors, item, config);
    actors.sort_by_key(|a| usize::from(matches!(a["character"].as_str(), Some("演员" | "配音"))));
    for (i, actor) in actors.iter_mut().enumerate() {
        actor["order"] = json!(i);
    }
    actors
}
pub fn episode(
    package: &Value,
    episode: &Value,
    cast: &[Value],
    item: &Value,
    config: &Value,
    fallback: bool,
) -> Value {
    let details = package.get("series_details").unwrap_or(package);
    let seasons = package
        .get("seasons_details")
        .map(list)
        .unwrap_or_else(|| list(&details["seasons"]));
    let season = seasons
        .iter()
        .find(|s| s["season_number"] == episode["season_number"]);
    let credits = season
        .map(|s| {
            s.get("credits")
                .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
                .unwrap_or(&s["aggregate_credits"])
        })
        .unwrap_or(&Value::Null);
    let season_cast = list(&credits["cast"]);
    let limit = config["max_episode_actors_to_process"]
        .as_u64()
        .unwrap_or(0) as usize;
    let raw: Vec<Value> = if fallback || limit == 0 {
        vec![]
    } else {
        list(&episode["credits"]["cast"])
            .into_iter()
            .chain(list(&episode["credits"]["guest_stars"]))
            .take(limit)
            .collect()
    };
    let filtered = |actors: &[Value]| {
        actors
            .iter()
            .filter(|p| {
                p["id"].as_i64().is_some_and(|id| id != 0)
                    && (config["remove_actors_without_avatars"] == false
                        || p["profile_path"].as_str().is_some_and(|s| !s.is_empty()))
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    if !raw.is_empty() {
        relations(&format(&filtered(&raw), item, config))
    } else if !season_cast.is_empty() {
        let filtered = filtered(&season_cast);
        if fallback {
            relations(&filtered)
        } else if filtered.is_empty() {
            relations(cast)
        } else {
            relations(&format(&filtered, item, config))
        }
    } else {
        relations(cast)
    }
}
