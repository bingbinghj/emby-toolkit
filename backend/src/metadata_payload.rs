use crate::media_edit::list;
use serde_json::{Value, json};

pub fn ratings(kind: &str, data: &Value, mapping: &Value, priority: &Value) -> Value {
    map_ratings(kind, data, mapping, priority, false)
}
pub fn sync_ratings(kind: &str, data: &Value, mapping: &Value, priority: &Value) -> Value {
    map_ratings(kind, data, mapping, priority, true)
}
fn map_ratings(kind: &str, data: &Value, mapping: &Value, priority: &Value, sync: bool) -> Value {
    let origin = if kind == "Movie" {
        &data["production_countries"][0]["iso_3166_1"]
    } else {
        &data["origin_country"][0]
    };
    let mut available = serde_json::Map::new();
    let source = if kind == "Movie" {
        &data["release_dates"]["results"]
    } else {
        &data["content_ratings"]["results"]
    };
    for entry in list(source) {
        let Some(country) = entry["iso_3166_1"].as_str() else {
            continue;
        };
        let rating = if kind == "Movie" {
            list(&entry["release_dates"])
                .iter()
                .find_map(|r| {
                    r["certification"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .map(Value::String)
        } else {
            entry
                .get("rating")
                .filter(|v| !sync || v.as_str().is_some_and(|s| !s.is_empty()))
                .cloned()
        };
        if let Some(rating) = rating {
            available.insert(country.into(), rating);
        }
    }
    if data["adult"] == true {
        available.insert("US".into(), json!("XXX"));
    } else if sync || !available.contains_key("US") {
        for country in list(priority) {
            let country = if country == "ORIGIN" {
                origin
            } else {
                &country
            };
            let Some(country) = country.as_str() else {
                continue;
            };
            let Some(rating) = available.get(country) else {
                continue;
            };
            let normalize = |value: &Value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or(value.to_string())
                    .trim()
                    .to_uppercase()
            };
            let to_number = |value: &Value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
            };
            let level = list(&mapping[country])
                .iter()
                .find(|r| normalize(&r["code"]) == normalize(rating))
                .and_then(|r| to_number(&r["emby_value"]));
            if let Some(level) = level {
                let rules = list(&mapping["US"]);
                let rules = rules
                    .iter()
                    .filter(|r| {
                        let code = r["code"].as_str().unwrap_or("");
                        if sync {
                            (kind == "Series") == code.to_uppercase().starts_with("TV-")
                        } else if kind == "Movie" {
                            !code.starts_with("TV-")
                        } else {
                            !matches!(code, "G" | "PG" | "PG-13" | "R" | "NC-17")
                        }
                    })
                    .collect::<Vec<_>>();
                let target = [level, level + 1].iter().find_map(|level| {
                    rules
                        .iter()
                        .find(|r| to_number(&r["emby_value"]) == Some(*level))
                        .map(|r| r["code"].clone())
                });
                if let Some(target) = target {
                    available.insert("US".into(), target);
                    break;
                }
            }
        }
    }
    json!(available)
}
pub fn build(
    kind: &str,
    data: &Value,
    aggregate: Option<&Value>,
    emby: Option<&Value>,
    mapping: &Value,
    priority: &Value,
) -> Value {
    if data.as_object().is_none_or(|v| v.is_empty()) {
        return json!({});
    }
    let excludes = [
        "casts",
        "releases",
        "release_dates",
        "keywords",
        "trailers",
        "content_ratings",
        "videos",
        "credits",
        "genres",
        "episodes_details",
        "seasons_details",
        "created_by",
        "networks",
        "production_companies",
    ];
    let mut output = Value::Object(
        data.as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| !excludes.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let emby = emby.unwrap_or(&Value::Null);
    let genres = list(&data["genres"]);
    if !genres.is_empty() {
        output["genres"] = json!(genres);
    } else if !list(&emby["Genres"]).is_empty() {
        output["genres"] = json!(
            list(&emby["Genres"])
                .iter()
                .map(|g| json!({"id":0,"name":g}))
                .collect::<Vec<_>>()
        );
    }
    if let Some(keywords) = data.get("keywords") {
        output["keywords"] = if keywords.is_object() {
            json!(list(
                &keywords[if kind == "Movie" {
                    "keywords"
                } else {
                    "results"
                }]
            ))
        } else {
            json!(list(keywords))
        };
    }
    for key in ["production_companies", "networks"] {
        if let Some(value) = data.get(key) {
            output[key] = value.clone();
        }
    }
    if kind == "Series" {
        if let Some(created) = data.get("created_by") {
            output["created_by"] = created.clone();
        }
        if let Some(aggregate) = aggregate {
            output["seasons_details"] = aggregate
                .get("seasons_details")
                .cloned()
                .unwrap_or(json!([]));
            let mut episodes = serde_json::Map::new();
            for (key, episode) in aggregate["episodes_details"]
                .as_object()
                .into_iter()
                .flatten()
            {
                let mut episode_out = json!({});
                for field in [
                    "id",
                    "season_number",
                    "episode_number",
                    "name",
                    "overview",
                    "air_date",
                    "vote_average",
                    "still_path",
                ] {
                    episode_out[field] = episode[field].clone();
                }
                episode_out["credits"] = json!({});
                for field in ["cast", "guest_stars", "crew"] {
                    episode_out["credits"][field] =
                        episode["credits"].get(field).cloned().unwrap_or(json!([]));
                }
                episodes.insert(key.clone(), episode_out);
            }
            output["episodes_details"] = json!(episodes);
        }
    }
    if matches!(kind, "Movie" | "Series") {
        let mapped = ratings(kind, data, mapping, priority);
        if let Some(rating) = mapped["US"].as_str().filter(|s| !s.is_empty()) {
            output["mpaa"] = json!(rating);
            output["certification"] = json!(rating);
        }
        output["_official_rating_map"] = mapped;
    }
    if let Some(id) = emby["ProviderIds"].get("Imdb") {
        output["imdb_id"] = id.clone();
    }
    if let Some(id) = data["external_ids"]["imdb_id"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        output["imdb_id"] = json!(id);
    }
    if let Some(date) = emby["DateCreated"].as_str().filter(|s| !s.is_empty()) {
        output["date_added"] = json!(date);
    }
    let credits = data
        .get("credits")
        .filter(|v| v.as_object().is_some_and(|v| !v.is_empty()))
        .unwrap_or(&data["casts"]);
    let directors = list(&credits["crew"])
        .into_iter()
        .filter(|p| p["job"] == "Director")
        .collect::<Vec<_>>();
    if !directors.is_empty() {
        output["casts"] = json!({"crew":directors});
    }
    output
}
