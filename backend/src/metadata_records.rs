use crate::{
    error::{ApiError, Result},
    media_edit::list,
    nfo,
    state::string,
};
use serde_json::{Value, json};

fn empty_record() -> Value {
    let columns = "tmdb_id item_type title original_title overview release_date release_year last_air_date backdrop_path homepage original_language poster_path rating actors_json parent_series_tmdb_id season_number episode_number in_library episode_group_id emby_item_ids_json file_sha1_json file_pickcode_json date_added official_rating_json genres_json directors_json production_companies_json networks_json countries_json keywords_json asset_details_json runtime_minutes total_episodes imdb_id tagline";
    let mut record = Value::Object(
        columns
            .split_whitespace()
            .map(|s| (s.into(), Value::Null))
            .collect(),
    );
    record["in_library"] = json!(false);
    for key in ["emby_item_ids_json", "file_sha1_json", "file_pickcode_json"] {
        record[key] = json!([]);
    }
    record
}
fn runtime(value: &Value) -> Value {
    json!(
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            .map(f64::round_ties_even)
            .filter(|n| *n > 0.0 && *n <= i32::MAX as f64)
            .map(|n| n as i32)
    )
}
pub fn pending(kind: &str, package: &Value, cast: &[Value], item: &Value) -> Result<Vec<Value>> {
    tracing::debug!(
        "元数据记录构建开始：类型 {kind}，Emby {}，演员 {} 位",
        string(item, "Id"),
        cast.len()
    );
    if !matches!(kind, "Movie" | "Series") {
        return Err(ApiError::bad("待入库类型无效"));
    }
    let details = if kind == "Series" {
        package.get("series_details").unwrap_or(package)
    } else {
        package
    };
    let provider = details["id"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| ApiError::bad("TMDb ID 无效"))?;
    let mut record = empty_record();
    record["tmdb_id"] = json!(provider.to_string());
    record["item_type"] = json!(kind);
    record["in_library"] = json!(false);
    for key in [
        "emby_item_ids_json",
        "file_sha1_json",
        "file_pickcode_json",
        "asset_details_json",
    ] {
        record[key] = json!([]);
    }
    for key in [
        "overview",
        "last_air_date",
        "backdrop_path",
        "homepage",
        "original_language",
        "poster_path",
        "tagline",
    ] {
        record[key] = details[key].clone();
    }
    record["title"] = details[if kind == "Movie" { "title" } else { "name" }].clone();
    record["original_title"] = details[if kind == "Movie" {
        "original_title"
    } else {
        "original_name"
    }]
    .clone();
    record["rating"] = details["vote_average"].clone();
    record["release_date"] = details[if kind == "Movie" {
        "release_date"
    } else {
        "first_air_date"
    }]
    .clone();
    record["release_year"] = json!(
        record["release_date"]
            .as_str()
            .and_then(|s| s.get(..4))
            .and_then(|s| s.parse::<i32>().ok())
    );
    record["date_added"] = item
        .get("DateCreated")
        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        .cloned()
        .unwrap_or(json!(chrono::Utc::now().to_rfc3339()));
    record["actors_json"] = json!(
        cast.iter()
            .filter(|p| p["id"].as_i64().is_some_and(|id| id != 0))
            .map(|p| json!({"tmdb_id":p["id"],"character":p["character"],"order":p["order"]}))
            .collect::<Vec<_>>()
    );
    record["official_rating_json"] = package
        .get("_official_rating_map")
        .cloned()
        .unwrap_or(json!({}));
    record["genres_json"] = json!(
        list(&details["genres"])
            .iter()
            .map(|g| {
                let name = g.get("name").unwrap_or(g);
                let name = match name.as_str() {
                    Some("Sci-Fi & Fantasy") => json!("科幻奇幻"),
                    Some("War & Politics") => json!("战争政治"),
                    _ => name.clone(),
                };
                json!({"id":g.get("id").cloned().unwrap_or(json!(0)),"name":name})
            })
            .collect::<Vec<_>>()
    );
    for (source, target) in [
        ("production_companies", "production_companies_json"),
        ("networks", "networks_json"),
    ] {
        record[target] = json!(
            list(&details[source])
                .iter()
                .filter(|v| !string(v, "name").is_empty())
                .map(|v| json!({"id":v["id"],"name":v["name"]}))
                .collect::<Vec<_>>()
        );
    }
    let keywords = details
        .get("keywords")
        .filter(|v| !v.is_null())
        .or_else(|| details.get("tags"))
        .unwrap_or(&Value::Null);
    let keywords = if keywords.is_object() {
        let primary = if kind == "Series" {
            "results"
        } else {
            "keywords"
        };
        [primary, "results", "keywords"]
            .iter()
            .map(|k| list(&keywords[*k]))
            .find(|v| !v.is_empty())
            .unwrap_or_default()
    } else {
        list(keywords)
    };
    record["keywords_json"] = json!(
        keywords
            .iter()
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()) || !string(v, "name").is_empty())
            .map(|v| json!({"id":v["id"],"name":v.get("name").unwrap_or(v)}))
            .collect::<Vec<_>>()
    );
    let countries = ["production_countries", "origin_country"]
        .iter()
        .map(|k| list(&details[*k]))
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    record["countries_json"] = json!(
        countries
            .iter()
            .filter_map(|v| v.as_str().or_else(|| v["iso_3166_1"].as_str()))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    );
    if kind == "Movie" {
        for key in [
            "season_number",
            "episode_number",
            "total_episodes",
            "imdb_id",
        ] {
            record[key] = details[key].clone();
        }
        record["runtime_minutes"] = runtime(&details["runtime"]);
        let credits = details
            .get("credits")
            .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
            .unwrap_or(&details["casts"]);
        record["directors_json"] = json!(
            list(&credits["crew"])
                .iter()
                .filter(|p| p["job"] == "Director")
                .map(|p| json!({"id":p["id"],"name":p["name"]}))
                .collect::<Vec<_>>()
        );
        for release in list(&details["releases"]["countries"]) {
            if let Some(country) = release["iso_3166_1"].as_str().filter(|s| !s.is_empty())
                && let Some(certification) =
                    release["certification"].as_str().filter(|s| !s.is_empty())
            {
                record["official_rating_json"][country] = json!(certification);
            }
        }
    } else {
        record["total_episodes"] = details
            .get("number_of_episodes")
            .cloned()
            .unwrap_or(json!(0));
        record["episode_group_id"] = package
            .get("episode_group_id")
            .filter(|v| !v.is_null() && **v != "")
            .unwrap_or(&details["episode_group_id"])
            .clone();
        record["imdb_id"] = package["imdb_id"].clone();
        record["directors_json"] = json!(
            nfo::directors(details)
                .iter()
                .map(|d| json!({"id":d["id"],"name":d["name"]}))
                .collect::<Vec<_>>()
        );
        if record["original_language"].is_null() {
            record["original_language"] = details["languages"][0].clone();
        }
    }
    let mut records = vec![record];
    if kind == "Series" {
        let seasons = package
            .get("seasons_details")
            .map(list)
            .unwrap_or_else(|| list(&details["seasons"]));
        for season in &seasons {
            let Some(number) = season["season_number"].as_i64() else {
                continue;
            };
            let Some(id) = season["id"]
                .as_u64()
                .filter(|n| *n > 0)
                .map(|n| n.to_string())
                .or_else(|| {
                    season["id"]
                        .as_str()
                        .filter(|s| s.contains('-'))
                        .map(str::to_string)
                })
            else {
                continue;
            };
            let mut child = empty_record();
            child["tmdb_id"] = json!(id);
            child["item_type"] = json!("Season");
            child["parent_series_tmdb_id"] = json!(provider.to_string());
            child["season_number"] = json!(number);
            child["title"] = season["name"].clone();
            child["overview"] = season["overview"].clone();
            child["release_date"] = season["air_date"].clone();
            child["poster_path"] = season
                .get("poster_path")
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                .unwrap_or(&details["poster_path"])
                .clone();
            child["total_episodes"] = season.get("episode_count").cloned().unwrap_or(json!(0));
            records.push(child);
        }
        let episodes = if let Some(map) = package["episodes_details"].as_object() {
            map.values().cloned().collect()
        } else {
            list(&package["episodes_details"])
        };
        for episode in episodes {
            let (Some(season), Some(number)) = (
                episode["season_number"].as_i64(),
                episode["episode_number"].as_i64(),
            ) else {
                continue;
            };
            let mut child = empty_record();
            child["tmdb_id"] = json!(
                episode["id"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .map(|n| n.to_string())
                    .unwrap_or(format!("{provider}-S{season}E{number}"))
            );
            child["item_type"] = json!("Episode");
            child["parent_series_tmdb_id"] = json!(provider.to_string());
            child["season_number"] = json!(season);
            child["episode_number"] = json!(number);
            child["title"] = episode["name"].clone();
            child["overview"] = episode["overview"].clone();
            child["release_date"] = episode["air_date"].clone();
            child["poster_path"] = episode["still_path"].clone();
            child["backdrop_path"] = episode["still_path"].clone();
            child["runtime_minutes"] = runtime(&episode["runtime"]);
            child["asset_details_json"] = json!([]);
            child["actors_json"] = json!(cast.iter().filter(|p|p["id"].as_i64().is_some_and(|id|id!=0))
                .map(|p|json!({"tmdb_id":p["id"],"character":p["character"],"order":p.get("order").cloned().unwrap_or(json!(999))})).collect::<Vec<_>>());
            let crew = [
                list(&episode["crew"]),
                list(&episode["credits"]["crew"]),
                seasons
                    .iter()
                    .find(|s| s["season_number"].as_i64() == Some(season))
                    .map(|s| list(&s["credits"]["crew"]))
                    .unwrap_or_default(),
            ]
            .into_iter()
            .find(|c| !c.is_empty())
            .unwrap_or_default();
            child["directors_json"] = json!(
                crew.iter()
                    .filter(|p| p["job"] == "Director")
                    .map(|p| json!({"id":p["id"],"name":p["name"]}))
                    .collect::<Vec<_>>()
            );
            records.push(child);
        }
    }
    for record in &mut records {
        if record["release_date"] == "" {
            record["release_date"] = Value::Null;
        }
        if record["last_air_date"] == "" {
            record["last_air_date"] = Value::Null;
        }
        if let Some(year) = record["release_date"]
            .as_str()
            .and_then(|s| s.get(..4))
            .and_then(|s| s.parse::<i32>().ok())
        {
            record["release_year"] = json!(year);
        }
        if record["imdb_id"].is_null() || record["imdb_id"] == "" {
            record["imdb_id"] = package["imdb_id"].clone();
        }
    }
    tracing::debug!(
        "元数据记录构建完成：类型 {kind}，TMDb {provider}，记录 {} 条",
        records.len()
    );
    Ok(records)
}

pub struct OnlineContext<'a> {
    pub seasons: &'a [Value],
    pub episodes: &'a [Value],
    pub parents: &'a std::collections::HashMap<String, String>,
    pub library_guid: &'a str,
    pub episode_ids: &'a [String],
    pub skip_series_record: bool,
}

pub async fn online(
    state: &crate::AppState,
    kind: &str,
    package: &Value,
    cast: &[Value],
    item: &Value,
    context: &OnlineContext<'_>,
) -> Result<Vec<Value>> {
    let id = string(item, "Id");
    tracing::info!(
        "在线媒体资产绑定开始：类型 {kind}，Emby {id}，目标分集 {} 个",
        context.episode_ids.len()
    );
    if id.is_empty() || id == "pending" {
        return Err(ApiError::bad("在线入库缺少 Emby ID"));
    }
    let mut records = pending(kind, package, cast, item)?;
    if kind == "Movie" {
        let sources = list(&item["MediaSources"]);
        let mut versions = vec![];
        if sources.is_empty() {
            versions.push(item.clone());
        } else {
            for source in sources {
                if string(&source, "Path").is_empty() {
                    continue;
                }
                let mut version = item.clone();
                version["Id"] = json!(
                    source["Id"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&id)
                        .replace("mediasource_", "")
                );
                version["Path"] = source["Path"].clone();
                for key in ["Container", "Size", "RunTimeTicks", "MediaStreams"] {
                    if let Some(value) = source.get(key) {
                        version[key] = value.clone();
                    }
                }
                version.as_object_mut().unwrap().remove("Width");
                version.as_object_mut().unwrap().remove("Height");
                version["MediaSources"] = json!([source]);
                versions.push(version);
            }
        }
        online_assets(state, &mut records[0], &versions, item, context, true).await?;
        tracing::info!(
            "在线电影资产绑定完成：Emby {id}，版本 {} 个，记录 {} 条",
            versions.len(),
            records.len()
        );
        return Ok(records);
    }
    records[0]["in_library"] = json!(true);
    records[0]["emby_item_ids_json"] = json!([id]);
    records[0]["asset_details_json"] = if string(item, "Path").is_empty() {
        json!([])
    } else {
        let parsed = crate::assets::parse_with_cache(
            item,
            context.parents,
            context.library_guid,
            &json!([]),
            &Value::Null,
        );
        json!([{"path":item["Path"],"source_library_id":string(item,"_SourceLibraryId"),"ancestor_ids":parsed["ancestor_ids"]}])
    };
    for record in records.iter_mut().filter(|r| r["item_type"] == "Season") {
        let ids = context
            .seasons
            .iter()
            .filter(|s| integer(&s["IndexNumber"]) == integer(&record["season_number"]))
            .map(|s| s["Id"].clone())
            .collect::<Vec<_>>();
        record["in_library"] = json!(!ids.is_empty());
        record["emby_item_ids_json"] = json!(ids);
    }
    let mut groups = std::collections::BTreeMap::<(i64, i64), Vec<Value>>::new();
    for episode in context.episodes {
        if let (Some(season), Some(number)) = (
            integer(&episode["ParentIndexNumber"]),
            integer(&episode["IndexNumber"]),
        ) {
            groups
                .entry((season, number))
                .or_default()
                .push(episode.clone());
        }
    }
    let config = state.config.read().await.clone();
    let raw_episodes = package
        .get("episodes_details")
        .and_then(Value::as_object)
        .map(|v| v.values().cloned().collect::<Vec<_>>())
        .unwrap_or_else(|| list(&package["episodes_details"]));
    let mut processed = std::collections::HashSet::new();
    for record in records.iter_mut().filter(|r| r["item_type"] == "Episode") {
        let key = (
            integer(&record["season_number"]).unwrap(),
            integer(&record["episode_number"]).unwrap(),
        );
        if !context.episode_ids.is_empty()
            && groups.get(&key).is_none_or(|versions| {
                !versions
                    .iter()
                    .any(|v| context.episode_ids.contains(&string(v, "Id")))
            })
        {
            continue;
        }
        if let Some(episode) = raw_episodes.iter().find(|e| {
            integer(&e["season_number"]) == Some(key.0)
                && integer(&e["episode_number"]) == Some(key.1)
        }) {
            record["actors_json"] =
                crate::cast::episode(package, episode, cast, item, &config, false);
        }
        if let Some(versions) = groups.get(&key) {
            processed.insert(key);
            online_assets(state, record, versions, item, context, false).await?;
        }
    }
    for (key, versions) in &groups {
        if processed.contains(key)
            || (!context.episode_ids.is_empty()
                && !versions
                    .iter()
                    .any(|v| context.episode_ids.contains(&string(v, "Id"))))
        {
            continue;
        }
        let first = &versions[0];
        let mut fallback = package.clone();
        fallback["episodes_details"] = json!([{"season_number":key.0,"episode_number":key.1,
            "name":first["Name"].as_str().filter(|s|!s.is_empty()).map(str::to_string).unwrap_or(format!("Episode {}",key.1)),
            "overview":first["Overview"],"air_date":first["PremiereDate"]}]);
        let mut child = pending("Series", &fallback, cast, item)?.pop().unwrap();
        tracing::debug!(
            "分集资产使用内部元数据 ID：Emby {id}，S{}E{}，版本 {} 个",
            key.0,
            key.1,
            versions.len()
        );
        child["actors_json"] = crate::cast::episode(
            package,
            &serde_json::json!({"season_number":key.0}),
            cast,
            item,
            &config,
            true,
        );
        online_assets(state, &mut child, versions, item, context, false).await?;
        records.push(child);
    }
    if !context.episode_ids.is_empty() {
        records.retain(|r| {
            r["item_type"] != "Episode"
                || list(&r["emby_item_ids_json"]).iter().any(|id| {
                    id.as_str()
                        .is_some_and(|id| context.episode_ids.iter().any(|v| v == id))
                })
        });
    }
    if context.skip_series_record {
        records.retain(|r| r["item_type"] != "Series");
    }
    tracing::info!(
        "在线媒体资产绑定完成：类型 {kind}，Emby {id}，记录 {} 条",
        records.len()
    );
    Ok(records)
}
fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}
async fn online_assets(
    state: &crate::AppState,
    record: &mut Value,
    versions: &[Value],
    item: &Value,
    context: &OnlineContext<'_>,
    movie: bool,
) -> Result<()> {
    let mut ids = vec![];
    let mut assets = vec![];
    let mut pcs = vec![];
    let mut shas = vec![];
    for version in versions {
        let raw = string(version, "Path");
        let (pc, sha, cache_path) = crate::fingerprints::lookup(state, &raw).await?;
        let mut version = version.clone();
        let id = string(&version, "Id").replace("mediasource_", "");
        let path =
            crate::fingerprints::asset_path(state, &raw, pc.as_deref(), &cache_path, item, &id)
                .await;
        version["Path"] = json!(path);
        let empty = std::collections::HashMap::new();
        let mut asset = crate::assets::parse(
            &version,
            if movie { context.parents } else { &empty },
            if movie { context.library_guid } else { "" },
            &json!([]),
        )
        .await;
        asset["source_library_id"] = if movie {
            json!(string(item, "_SourceLibraryId"))
        } else {
            item["_SourceLibraryId"].clone()
        };
        ids.push(id);
        assets.push(asset);
        pcs.push(pc);
        shas.push(sha);
    }
    record["in_library"] = json!(true);
    record["emby_item_ids_json"] = json!(ids);
    record["asset_details_json"] = json!(assets);
    record["file_pickcode_json"] = json!(pcs);
    record["file_sha1_json"] = json!(shas);
    Ok(())
}
