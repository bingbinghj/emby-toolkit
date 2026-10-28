use crate::{AppState, ai, error::Result, media_edit::list, metadata, state::string};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub async fn overview_fallback(
    state: &AppState,
    kind: &str,
    data: &mut Value,
    scope: &[(i64, i64)],
) -> Result<BTreeSet<String>> {
    let config = state.config.read().await.clone();
    if config["ai_joke_fallback"] != true
        || string(&config, "ai_api_key").trim().is_empty()
        || string(&config, "ai_model_name").trim().is_empty()
        || !matches!(
            string(&config, "ai_provider").to_lowercase().as_str(),
            "openai" | "zhipuai" | "gemini"
        )
        || !matches!(kind, "Movie" | "Series")
    {
        return Ok(BTreeSet::new());
    }
    let top = data.get("series_details").unwrap_or(data);
    let series_id = top["id"]
        .as_u64()
        .map(|id| id.to_string())
        .unwrap_or_else(|| string(top, "id"));
    let title = string(top, if kind == "Movie" { "title" } else { "name" });
    let mut missing = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut filled = BTreeSet::new();
    for (path, node_kind) in nodes(kind, data) {
        let node = data.pointer_mut(&path).unwrap();
        if node_kind == "Season" || !string(node, "overview").is_empty() {
            continue;
        }
        let key = if node_kind == "Episode" {
            let (Some(season), Some(number)) = (
                node["season_number"].as_i64(),
                node["episode_number"].as_i64(),
            ) else {
                continue;
            };
            if !scope.contains(&(season, number)) {
                continue;
            }
            format!("S{season}E{number}")
        } else {
            "main".into()
        };
        let cached: Option<String> = if node_kind == "Episode" {
            sqlx::query_scalar("SELECT overview FROM media_metadata WHERE item_type='Episode' AND parent_series_tmdb_id=$1 AND season_number=$2 AND episode_number=$3 AND overview LIKE '%【老六占位简介】%' LIMIT 1")
                .bind(&series_id).bind(node["season_number"].as_i64().map(|n| n as i32)).bind(node["episode_number"].as_i64().map(|n| n as i32)).fetch_optional(&state.db).await?
        } else {
            metadata::row(state, &series_id, kind)
                .await?
                .map(|row| string(&row, "overview"))
        };
        if let Some(cached) = cached.filter(|text| text.contains("【老六占位简介】")) {
            node["overview"] = json!(cached);
            tracing::debug!("占位简介缓存继承：《{title}》，TMDb {series_id}，目标 {key}");
            filled.insert(key);
        } else {
            missing.insert(
                key.clone(),
                if key == "main" {
                    title.clone()
                } else {
                    format!("{title} {key}")
                },
            );
            paths.insert(key, path);
        }
    }
    if !missing.is_empty() {
        tracing::info!(
            "占位简介生成开始：《{title}》，TMDb {series_id}，缺失简介 {} 处",
            missing.len()
        );
        let generated = ai::translate(
            state,
            &missing,
            "batch_joke_fallback",
            &Value::Null,
            &Value::Null,
        )
        .await?;
        for (key, text) in generated {
            data.pointer_mut(&paths[&key]).unwrap()["overview"] = json!(text);
            filled.insert(key);
        }
        tracing::info!(
            "占位简介处理结束：《{title}》，TMDb {series_id}，填充或继承 {} 处",
            filled.len()
        );
    }
    Ok(filled)
}

pub fn chinese(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c as u32,0x4e00..=0x9fff|0x3400..=0x4dbf|0xf900..=0xfaff))
}
pub fn needs_translation(text: &str) -> bool {
    static FOREIGN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"[A-Za-z]{2,}|[\u0370-\u052f\u0590-\u06ff\u0900-\u097f\u0e00-\u0e7f\u1100-\u11ff\u3040-\u30ff\u31f0-\u31ff\uac00-\ud7af]").unwrap()
    });
    let text = text.trim();
    !text.is_empty() && (!chinese(text) || ai::simplified(text) != text || FOREIGN.is_match(text))
}
pub fn prune(kind: &str, data: &mut Value) {
    let top = if kind == "Series" && data["series_details"].is_object() {
        &mut data["series_details"]
    } else {
        &mut *data
    };
    if let Some(top) = top.as_object_mut() {
        for key in if kind == "Movie" {
            ["title", "original_title"]
        } else {
            ["name", "original_name"]
        } {
            top.remove(key);
        }
    }
    fn people(data: &mut Value) {
        match data {
            Value::Object(o) => {
                for key in [
                    "cast",
                    "crew",
                    "guest_stars",
                    "created_by",
                    "credits",
                    "casts",
                    "aggregate_credits",
                ] {
                    o.remove(key);
                }
                for v in o.values_mut() {
                    people(v);
                }
            }
            Value::Array(a) => {
                for v in a {
                    people(v)
                }
            }
            _ => (),
        }
    }
    people(data);
}
fn nodes(kind: &str, data: &Value) -> Vec<(String, String)> {
    let mut nodes = vec![(
        if kind == "Series" && data["series_details"].is_object() {
            "/series_details".into()
        } else {
            "".into()
        },
        kind.into(),
    )];
    if kind == "Series" {
        for (index, _) in list(&data["seasons_details"]).iter().enumerate() {
            nodes.push((format!("/seasons_details/{index}"), "Season".into()));
        }
        if let Some(episodes) = data["episodes_details"].as_object() {
            for key in episodes.keys() {
                nodes.push((
                    format!(
                        "/episodes_details/{}",
                        key.replace('~', "~0").replace('/', "~1")
                    ),
                    "Episode".into(),
                ));
            }
        } else {
            for (index, _) in list(&data["episodes_details"]).iter().enumerate() {
                nodes.push((format!("/episodes_details/{index}"), "Episode".into()));
            }
        }
    }
    nodes
}
pub async fn metadata_tree(
    state: &AppState,
    kind: &str,
    data: &mut Value,
    item_name: &str,
) -> Result<usize> {
    let config = state.config.read().await.clone();
    if string(&config, "ai_api_key").is_empty() {
        return Ok(0);
    }
    let mut fields = BTreeMap::<String, BTreeMap<String, (String, String)>>::new();
    let mut persons = BTreeMap::new();
    let mut roles = BTreeMap::new();
    let mut role_actors = serde_json::Map::new();
    let nodes = nodes(kind, data);
    let top = data.get("series_details").unwrap_or(data);
    let animation = list(&top["genres"]).iter().any(|g| {
        ["animation", "动画", "documentary", "纪录", "记录"].contains(
            &g.as_str()
                .unwrap_or_else(|| g["name"].as_str().unwrap_or(""))
                .to_lowercase()
                .as_str(),
        )
    });
    for (path, kind) in &nodes {
        let Some(node) = data.pointer_mut(path) else {
            continue;
        };
        let id = match &node["id"] {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => continue,
        };
        if id.is_empty() || id == "0" {
            continue;
        }
        let local = metadata::row(state, &id, kind).await?.unwrap_or(json!({}));
        for (field, enabled, purpose) in [
            (
                if kind == "Movie" { "title" } else { "name" },
                config["ai_translate_title"] == true,
                "title",
            ),
            (
                "overview",
                if kind == "Episode" {
                    config["ai_translate_episode_overview"] == true
                } else {
                    config["ai_translate_overview"] == true
                },
                "overview",
            ),
            (
                "tagline",
                config["ai_translate_title"] == true && matches!(kind.as_str(), "Movie" | "Series"),
                "tagline",
            ),
        ] {
            let field_name = match field {
                "title" | "name" => "标题",
                "overview" => "简介",
                "tagline" => "标语",
                _ => field,
            };
            if !enabled {
                continue;
            }
            let current = string(node, field);
            if chinese(&current) || (purpose == "title" && current.is_empty()) {
                tracing::debug!(
                    "元数据字段翻译跳过：作品 {item_name}，类型 {kind}，TMDb {id}，字段 {field_name} 已有中文或空标题"
                );
                continue;
            }
            let stored = string(&local, if purpose == "title" { "title" } else { field });
            if chinese(&stored) {
                node[field] = json!(stored);
                tracing::debug!(
                    "元数据字段翻译缓存命中：作品 {item_name}，类型 {kind}，TMDb {id}，字段 {field_name}"
                );
                continue;
            }
            if current.is_empty()
                && matches!(kind.as_str(), "Movie" | "Series")
                && purpose != "title"
            {
                tracing::info!(
                    "元数据翻译英文源补抓开始：作品 {item_name}，类型 {kind}，TMDb {id}，字段 {field_name}"
                );
                let endpoint = format!("/{}/{id}", if kind == "Movie" { "movie" } else { "tv" });
                if let Ok(english) =
                    crate::tmdb::get(state, &endpoint, &[("language", "en-US".into())]).await
                {
                    node[field] = english[field].clone();
                }
            }
            let source = string(node, field);
            if !source.is_empty() {
                fields
                    .entry(purpose.into())
                    .or_default()
                    .insert(id.clone(), (format!("{path}/{field}"), source));
            }
        }
        if config["ai_translate_actor_role"] != true {
            continue;
        }
        let key = ["credits", "aggregate_credits", "casts"]
            .into_iter()
            .find(|key| node[*key].as_object().is_some_and(|o| !o.is_empty()))
            .unwrap_or("credits");
        if let Some(credits) = node.get_mut(key).filter(|v| v.is_object()) {
            let limit = config["max_actors_to_process"]
                .as_u64()
                .filter(|n| *n > 0)
                .unwrap_or(30) as usize;
            let ep_limit = config["max_episode_actors_to_process"]
                .as_u64()
                .unwrap_or(0) as usize;
            for (field, maximum) in [
                ("cast", if kind == "Episode" { ep_limit } else { limit }),
                ("guest_stars", if kind == "Episode" { ep_limit } else { 10 }),
            ] {
                if credits.get(field).is_none() {
                    continue;
                }
                let mut actors = list(&credits[field])
                    .into_iter()
                    .filter(|a| {
                        config["remove_actors_without_avatars"] == false
                            || a["profile_path"].as_str().is_some_and(|s| !s.is_empty())
                    })
                    .collect::<Vec<_>>();
                actors.sort_by_key(|a| a["order"].as_i64().unwrap_or(999));
                actors.truncate(maximum);
                credits[field] = json!(actors);
            }
            for (field, _) in [("cast", true), ("guest_stars", true), ("crew", false)] {
                if let Some(actors) = credits.get_mut(field).and_then(Value::as_array_mut) {
                    for actor in actors {
                        if field == "crew"
                            && !matches!(
                                actor["job"].as_str(),
                                Some("Director" | "Series Director")
                            )
                        {
                            continue;
                        }
                        let name = string(actor, "name");
                        if !name.is_empty() && (!chinese(&name) || ai::simplified(&name) != name) {
                            persons.insert(name.clone(), name.clone());
                        }
                        if field == "crew" {
                            continue;
                        }
                        let role = string(actor, "character");
                        let normalized = role
                            .chars()
                            .filter(|c| !c.is_whitespace() && !"()（）[]【】._-".contains(*c))
                            .collect::<String>()
                            .to_lowercase();
                        let placeholder = if ["voice", "voiceactor", "voiceactress", "配音", "声演"]
                            .contains(&normalized.as_str())
                        {
                            Some("配音")
                        } else if ["actor", "actress", "演员", "出演"]
                            .contains(&normalized.as_str())
                        {
                            Some("演员")
                        } else if normalized.is_empty() {
                            Some(if animation { "配音" } else { "演员" })
                        } else {
                            None
                        };
                        if let Some(role) = placeholder {
                            actor["character"] = json!(role);
                        } else {
                            let cleaned = crate::cast::clean_character(&role);
                            if needs_translation(&cleaned) {
                                roles.insert(cleaned.clone(), cleaned.clone());
                                role_actors.entry(cleaned).or_insert(json!(name));
                            }
                        }
                    }
                }
            }
        }
    }
    if !fields.is_empty() || !persons.is_empty() || !roles.is_empty() {
        tracing::info!(
            "元数据翻译开始：类型 {kind}，名称 {item_name}，元数据节点 {} 个，文本字段 {} 个，人名 {} 个，角色 {} 个",
            nodes.len(),
            fields.values().map(BTreeMap::len).sum::<usize>(),
            persons.len(),
            roles.len()
        );
    }
    let mut translated = 0;
    for purpose in ["overview", "tagline", "title"] {
        let Some(fields) = fields.get(purpose) else {
            continue;
        };
        let purpose_name = ai::purpose_label(purpose);
        tracing::info!(
            "元数据字段翻译开始：名称 {item_name}，用途 {purpose_name}，条目 {} 个",
            fields.len()
        );
        let inputs = fields
            .iter()
            .map(|(id, (_, source))| (id.clone(), source.clone()))
            .collect();
        let context = if purpose == "title" {
            json!({"media_type":"Episode"})
        } else {
            json!({"title":item_name})
        };
        let output = ai::translate(
            state,
            &inputs,
            if purpose == "title" {
                "batch_title_translation"
            } else {
                "batch_overview_translation"
            },
            &context,
            &Value::Null,
        )
        .await?;
        tracing::info!(
            "元数据字段翻译返回：名称 {item_name}，用途 {purpose_name}，可用结果 {} 个",
            output.len()
        );
        for (id, value) in output {
            if let Some((path, _)) = fields.get(&id)
                && let Some(node) = data.pointer_mut(path)
            {
                *node = json!(value);
                translated += 1;
            }
        }
    }
    async fn terms(
        state: &AppState,
        inputs: &BTreeMap<String, String>,
        purpose: &str,
        context: &Value,
        actors: &Value,
    ) -> Result<BTreeMap<String, String>> {
        let purpose_name = ai::purpose_label(purpose);
        let mut output = BTreeMap::new();
        let mut missing = BTreeMap::new();
        for (id, source) in inputs {
            let cached: Option<String> = if purpose == "quality_mode" {
                None
            } else {
                sqlx::query_scalar(
                    "SELECT translated_text FROM translation_cache WHERE original_text=$1",
                )
                .bind(source)
                .fetch_optional(&state.db)
                .await?
                .flatten()
            };
            if let Some(value) = cached.filter(|v| !v.is_empty()) {
                output.insert(id.clone(), value);
            } else {
                missing.insert(id.clone(), source.clone());
            }
        }
        if !inputs.is_empty() {
            tracing::info!(
                "人物翻译缓存检查完成：用途 {purpose_name}，命中 {} 个，待请求 {} 个",
                output.len(),
                missing.len()
            );
        }
        let revision = state
            .ai
            .cache_revision
            .load(std::sync::atomic::Ordering::SeqCst);
        let translated = ai::translate(state, &missing, purpose, context, actors).await?;
        let _gate = state.ai.cache_gate.read().await;
        if purpose != "quality_mode"
            && revision
                == state
                    .ai
                    .cache_revision
                    .load(std::sync::atomic::Ordering::SeqCst)
        {
            for (id, value) in &translated {
                sqlx::query("INSERT INTO translation_cache(original_text,translated_text,engine_used) VALUES($1,$2,$3) ON CONFLICT(original_text) DO UPDATE SET translated_text=EXCLUDED.translated_text,last_updated_at=NOW()")
            .bind(&inputs[id]).bind(value).bind(string(&*state.config.read().await,"ai_provider")).execute(&state.db).await?;
            }
            if !translated.is_empty() {
                tracing::info!(
                    "人物翻译缓存写入完成：用途 {purpose_name}，条目 {} 个",
                    translated.len()
                );
            }
        } else if !translated.is_empty() {
            tracing::debug!("人物翻译缓存写入跳过：用途 {purpose_name}，质量模式或缓存版本已变化");
        }
        output.extend(translated);
        Ok(output)
    }
    let people = terms(
        state,
        &persons,
        "transliterate_mode",
        &json!({}),
        &Value::Null,
    )
    .await?;
    for actor in role_actors.values_mut() {
        if let Some(value) = people.get(actor.as_str().unwrap_or("")) {
            *actor = json!(value);
        }
    }
    let purpose = match config["ai_translation_mode"].as_str() {
        Some("quality") => "quality_mode",
        Some("transliterate") => "transliterate_mode",
        _ => "fast_mode",
    };
    let top = data.get("series_details").unwrap_or(data);
    let context = if purpose == "quality_mode" {
        json!({"title":top.get("title").or_else(||top.get("name")).cloned().unwrap_or(json!(item_name)),"year":top.get("release_date").or_else(||top.get("first_air_date")).and_then(Value::as_str).and_then(|s|s.get(..4)),"media_type":kind})
    } else {
        json!({})
    };
    let roles = terms(state, &roles, purpose, &context, &json!(role_actors)).await?;
    for (path, _) in nodes {
        let Some(node) = data.pointer_mut(&path) else {
            continue;
        };
        let key = ["credits", "aggregate_credits", "casts"]
            .into_iter()
            .find(|key| node[*key].as_object().is_some_and(|o| !o.is_empty()))
            .unwrap_or("credits");
        for field in ["cast", "guest_stars", "crew"] {
            if let Some(actors) = node
                .get_mut(key)
                .and_then(|v| v.get_mut(field))
                .and_then(Value::as_array_mut)
            {
                for actor in actors {
                    if field == "crew"
                        && !matches!(actor["job"].as_str(), Some("Director" | "Series Director"))
                    {
                        continue;
                    }
                    let name = string(actor, "name");
                    if let Some(value) = people.get(&name) {
                        actor["original_name"] = json!(name);
                        actor["name"] = json!(value);
                    }
                    let role = crate::cast::clean_character(&string(actor, "character"));
                    if let Some(value) = roles.get(&role) {
                        actor["character"] = json!(crate::cast::clean_character(value));
                    }
                }
            }
        }
        if let Some(creators) = node.get_mut("created_by").and_then(Value::as_array_mut) {
            for creator in creators {
                let name = string(creator, "name");
                if let Some(value) = people.get(&name) {
                    creator["original_name"] = json!(name);
                    creator["name"] = json!(value);
                }
            }
        }
    }
    translated += people.len() + roles.len();
    if !fields.is_empty() || !people.is_empty() || !roles.is_empty() {
        tracing::info!(
            "元数据翻译完成：类型 {kind}，名称 {item_name}，字段和人物结果 {translated} 个"
        );
    }
    Ok(translated)
}
