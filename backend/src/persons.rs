use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
    translation::chinese,
};
use serde_json::{Value, json};

pub async fn upsert(state: &AppState, person: &Value) -> Result<()> {
    let id = person["id"]
        .as_i64()
        .or_else(|| person["tmdb_id"].as_i64())
        .filter(|n| *n > 0 && *n <= i32::MAX as i64)
        .ok_or_else(|| ApiError::bad("演员缺少合法 TMDb ID"))? as i32;
    let name = string(person, "name");
    let name = if name.trim().is_empty() {
        "Unknown Actor"
    } else {
        name.trim()
    };
    let optional = |field| {
        person[field]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::trim)
    };
    sqlx::query("INSERT INTO person_metadata(tmdb_person_id,primary_name,imdb_id,douban_celebrity_id,last_updated_at) VALUES($1,$2,$3,$4,NOW()) ON CONFLICT(tmdb_person_id) DO UPDATE SET primary_name=EXCLUDED.primary_name,imdb_id=COALESCE(EXCLUDED.imdb_id,person_metadata.imdb_id),douban_celebrity_id=COALESCE(EXCLUDED.douban_celebrity_id,person_metadata.douban_celebrity_id),last_updated_at=NOW() WHERE person_metadata.primary_name IS DISTINCT FROM EXCLUDED.primary_name OR person_metadata.imdb_id IS DISTINCT FROM COALESCE(EXCLUDED.imdb_id,person_metadata.imdb_id) OR person_metadata.douban_celebrity_id IS DISTINCT FROM COALESCE(EXCLUDED.douban_celebrity_id,person_metadata.douban_celebrity_id)")
        .bind(id).bind(name).bind(optional("imdb_id")).bind(optional("douban_id")).execute(&state.db).await
        .inspect(|result| tracing::debug!("人员元数据库写入完成：TMDb {id}，名称 {name}，变更 {} 行", result.rows_affected()))
        .inspect_err(|_| tracing::error!("人员元数据库写入失败：TMDb {id}，名称 {name}"))?;
    if ["profile_path", "gender", "popularity"]
        .iter()
        .any(|k| person.get(*k).is_some())
    {
        sqlx::query("UPDATE person_metadata SET profile_path=$1,gender=$2,adult=$3,popularity=$4,original_name=$5,last_updated_at=NOW() WHERE tmdb_person_id=$6")
            .bind(person["profile_path"].as_str()).bind(person["gender"].as_i64().map(|n|n as i32)).bind(person["adult"].as_bool().unwrap_or(false)).bind(person["popularity"].as_f64().map(|n|n as f32)).bind(person["original_name"].as_str()).bind(id).execute(&state.db).await?;
    }
    Ok(())
}
pub fn score(cast: &[Value], animation: bool) -> f64 {
    if cast.is_empty() {
        return if animation { 7.0 } else { 0.0 };
    }
    let total = cast
        .iter()
        .map(|p| {
            let name = p["name"]
                .as_str()
                .or_else(|| p["Name"].as_str())
                .unwrap_or("");
            let role = p["character"]
                .as_str()
                .or_else(|| p["Role"].as_str())
                .unwrap_or("");
            let names = if chinese(name) {
                5.0
            } else if !name.is_empty() {
                1.0
            } else {
                0.0
            };
            let roles = if chinese(role) {
                if ["演员", "配音"].contains(&role) || role.ends_with("(配音)") {
                    2.5
                } else {
                    5.0
                }
            } else if !role.is_empty() {
                0.5
            } else {
                0.0
            };
            names + roles
        })
        .sum::<f64>()
        / cast.len() as f64;
    let score = if !animation && cast.len() < 10 {
        total * cast.len() as f64 / 10.0
    } else {
        total
    };
    (score * 10.0).round_ties_even() / 10.0
}
pub async fn process(state: &AppState, source: &[Value], item: &Value) -> Result<Vec<Value>> {
    process_inner(state, source, item, true).await
}
pub async fn preview(state: &AppState, source: &[Value], item: &Value) -> Result<Vec<Value>> {
    process_inner(state, source, item, false).await
}
async fn process_inner(
    state: &AppState,
    source: &[Value],
    item: &Value,
    persist: bool,
) -> Result<Vec<Value>> {
    tracing::info!(
        "演员表处理开始：Emby {}，名称 {}，输入 {} 位，缓存方式 {}",
        string(item, "Id"),
        string(item, "Name"),
        source.len(),
        if persist {
            "处理后写入缓存"
        } else {
            "仅生成预览"
        }
    );
    let config = state.config.read().await.clone();
    let mut raw = source.to_vec();
    raw.sort_by_key(|p| p["order"].as_i64().unwrap_or(999));
    let mut seen = std::collections::HashSet::new();
    raw.retain(|p| {
        !string(p, "name").trim().is_empty() && seen.insert(string(p, "name").trim().to_string())
    });
    for actor in &mut raw {
        actor["character"] = json!(crate::cast::clean_character(&string(actor, "character")));
    }
    let mut result = vec![];
    let mut used = std::collections::HashSet::new();
    for person in list(&item["People"])
        .iter()
        .filter(|p| p["Type"] == "Actor")
    {
        let id = string(&person["ProviderIds"], "Tmdb");
        let name = string(person, "Name");
        let translated:Option<String>=sqlx::query_scalar("SELECT original_text FROM translation_cache WHERE translated_text=$1 AND original_text NOT LIKE 'ai-result:%' LIMIT 1").bind(&name).fetch_optional(&state.db).await?;
        if let Some(actor) = raw.iter().find(|a| {
            a["id"].as_i64().map(|n| n.to_string()).as_deref() == Some(&id)
                || string(a, "name").trim().eq_ignore_ascii_case(name.trim())
                || translated
                    .as_ref()
                    .is_some_and(|name| string(a, "name").trim().eq_ignore_ascii_case(name.trim()))
        }) {
            let mut actor = actor.clone();
            used.insert(string(&actor, "name").trim().to_string());
            let douban_matched = actor["_douban_matched"] == true;
            if !douban_matched && actor["_douban_matched"] != "name" && chinese(&name) {
                actor["name"] = json!(name);
            }
            let role = crate::cast::clean_character(&string(person, "Role"));
            if douban_matched {
                actor["character"] = json!(crate::douban::best_role(
                    &role,
                    &string(&actor, "character")
                ));
            } else if chinese(&role) || string(&actor, "character").is_empty() {
                actor["character"] = json!(role);
            }
            actor["emby_person_id"] = person["Id"].clone();
            result.push(actor);
        }
    }
    for actor in raw {
        if !used.contains(string(&actor, "name").trim()) {
            let mut actor = actor;
            actor["emby_person_id"] = Value::Null;
            result.push(actor);
        }
    }
    result.sort_by_key(|a| a["order"].as_i64().unwrap_or(999));
    result.truncate(
        config["max_actors_to_process"]
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(30) as usize,
    );
    if config["remove_actors_without_avatars"] != false {
        result.retain(|p| p["profile_path"].as_str().is_some_and(|s| !s.is_empty()));
    }
    let mut result = crate::cast::format(&result, item, &config);
    for actor in &mut result {
        if let Some(actor) = actor.as_object_mut() {
            actor.remove("_douban_matched");
        }
        actor["provider_ids"] = json!({"Tmdb":actor["id"].as_i64().map(|n|n.to_string()),"Imdb":actor["imdb_id"],"Douban":actor["douban_id"]});
        if persist {
            upsert(state, actor).await?;
        }
    }
    tracing::info!(
        "演员表处理完成：Emby {}，名称 {}，保留 {} 位，过滤 {} 位",
        string(item, "Id"),
        string(item, "Name"),
        result.len(),
        source.len().saturating_sub(result.len())
    );
    Ok(result)
}

pub async fn update_emby(
    state: &AppState,
    cast: &[Value],
    data: &Value,
    item: &Value,
) -> Result<usize> {
    tracing::info!(
        "Emby 人物名字同步开始：Emby {}，名称 {}",
        string(item, "Id"),
        string(item, "Name")
    );
    let mut names = std::collections::BTreeMap::<String, String>::new();
    let mut locations = std::collections::HashMap::new();
    let allowed = cast
        .iter()
        .filter_map(|p| p["id"].as_i64().map(|n| n.to_string()))
        .chain(
            list(&item["People"])
                .iter()
                .map(|p| string(&p["ProviderIds"], "Tmdb")),
        )
        .collect::<std::collections::HashSet<_>>();
    for actor in cast {
        if chinese(&string(actor, "name"))
            && let Some(id) = actor["id"].as_i64()
        {
            names.insert(id.to_string(), string(actor, "name"));
            if !string(actor, "emby_person_id").is_empty() {
                locations.insert(id.to_string(), string(actor, "emby_person_id"));
            }
        }
    }
    let episodes = data["episodes_details"]
        .as_object()
        .map(|o| o.values().cloned().collect::<Vec<_>>())
        .unwrap_or_else(|| list(&data["episodes_details"]));
    for node in std::iter::once(data.clone())
        .chain(list(&data["seasons_details"]))
        .chain(episodes)
    {
        let credits = ["credits", "aggregate_credits", "casts"]
            .iter()
            .find_map(|k| {
                node.get(*k)
                    .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
            })
            .unwrap_or(&Value::Null);
        let people = list(&credits["crew"])
            .into_iter()
            .filter(|p| matches!(p["job"].as_str(), Some("Director" | "Series Director")))
            .chain(list(&node["created_by"]))
            .chain(
                list(&credits["cast"])
                    .into_iter()
                    .chain(list(&credits["guest_stars"]))
                    .filter(|p| {
                        p["id"]
                            .as_i64()
                            .is_some_and(|n| allowed.contains(&n.to_string()))
                    }),
            );
        for person in people {
            if let Some(id) = person["id"].as_i64()
                && chinese(&string(&person, "name"))
            {
                names.insert(id.to_string(), string(&person, "name"));
            }
        }
    }
    for person in list(&item["People"]) {
        let id = string(&person["ProviderIds"], "Tmdb");
        if names.contains_key(&id) && !string(&person, "Id").is_empty() {
            locations.insert(id, string(&person, "Id"));
        }
    }
    for id in names.keys() {
        if locations.contains_key(id) {
            continue;
        }
        let items = state
            .emby(
                reqwest::Method::GET,
                "Items",
                &[
                    ("IncludeItemTypes", "Person".into()),
                    ("Recursive", "true".into()),
                    ("AnyProviderIdEquals", format!("tmdb.{id}")),
                    ("Fields", "ProviderIds".into()),
                    ("Limit", "5".into()),
                ],
                None,
            )
            .await?;
        if let Some(person) = list(&items["Items"])
            .iter()
            .find(|p| string(&p["ProviderIds"], "Tmdb") == *id)
        {
            locations.insert(id.clone(), string(person, "Id"));
        }
    }
    tracing::info!(
        "Emby 人物名字同步范围确认：《{}》，Emby {}，待核对 {} 位",
        string(item, "Name"),
        string(item, "Id"),
        names.len()
    );
    let mut updated = 0;
    for (id, location) in locations {
        let mut person = crate::media_edit::details(state, &location, "Name").await?;
        if names.get(&id).is_some_and(|name| person["Name"] != *name) {
            tracing::debug!(
                "Emby 人物名字更新开始：{} → {}，人员 {location}，TMDb {id}",
                string(&person, "Name"),
                names[&id]
            );
            person["Name"] = json!(names[&id]);
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Items/{location}"),
                    &[],
                    Some(person),
                )
                .await?;
            updated += 1;
            tracing::debug!(
                "Emby 人物名字更新完成：{}，人员 {location}，TMDb {id}",
                names[&id]
            );
        } else {
            tracing::debug!(
                "Emby 人物名字更新跳过：{}，人员 {location}，TMDb {id}，名字无需更新",
                string(&person, "Name")
            );
        }
    }
    tracing::info!(
        "Emby 人物名字同步完成：《{}》，Emby {}，更新 {updated} 位",
        string(item, "Name"),
        string(item, "Id")
    );
    Ok(updated)
}
