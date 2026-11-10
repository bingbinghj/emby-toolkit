use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
    translation::chinese,
};
use serde_json::{Value, json};

fn tmdb_id(person: &Value) -> Result<String> {
    let mut found = "";
    if let Some(ids) = person["ProviderIds"].as_object() {
        for (key, value) in ids {
            if !key.eq_ignore_ascii_case("tmdb") {
                continue;
            }
            let Some(id) = value.as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            if !found.is_empty() && found != id {
                return Err(ApiError(
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "人物 {} 的 TMDb ProviderIds 大小写键值冲突，无法确认演员关联",
                        string(person, "Id")
                    ),
                ));
            }
            found = id;
        }
    }
    Ok(found.to_owned())
}

fn normalize_tmdb_id(person: &mut Value) -> Result<()> {
    let id = tmdb_id(person)?;
    if !id.is_empty() {
        person["ProviderIds"]["Tmdb"] = json!(id);
    }
    Ok(())
}

fn actors_match(expected: &Value, actual: &Value) -> Result<bool> {
    let (Some(expected), Some(actual)) = (expected.as_array(), actual.as_array()) else {
        return Ok(false);
    };
    let expected = expected
        .iter()
        .filter(|person| person["Type"] == "Actor")
        .collect::<Vec<_>>();
    let actual = actual
        .iter()
        .filter(|person| person["Type"] == "Actor")
        .collect::<Vec<_>>();
    if expected.len() != actual.len() {
        return Ok(false);
    }
    let actual_ids = actual
        .iter()
        .map(|person| tmdb_id(person))
        .collect::<Result<Vec<_>>>()?;
    let mut used = std::collections::HashSet::new();
    for person in expected {
        let tmdb = tmdb_id(person)?;
        let matched = actual.iter().enumerate().find(|(index, live)| {
            !used.contains(index)
                && live["Name"] == person["Name"]
                && string(live, "Role") == string(person, "Role")
                && person["Id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .is_none_or(|id| live["Id"] == id)
                && (tmdb.is_empty() || actual_ids[*index] == tmdb)
        });
        if let Some((index, _)) = matched {
            used.insert(index);
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) async fn verify_readback(
    state: &AppState,
    id: &str,
    expected: &Value,
    context: Option<&crate::jobs::JobContext>,
) -> Result<()> {
    use std::time::Duration;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut reason = None;
    let user = string(&*state.config.read().await, "emby_user_id");
    let path = format!("GET Users/{user}/Items/{id}");
    loop {
        if let Some(context) = context {
            let paused = tokio::time::Instant::now();
            context.checkpoint().await?;
            deadline += paused.elapsed();
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let read = tokio::time::timeout_at(deadline, async {
            let mut live = crate::media_edit::details(state, id, "Id,Name,People").await?;
            if expected.get("People").is_some() {
                live["People"] =
                    json!(complete_provider_ids(state, &list(&live["People"]), None).await?);
            }
            Ok::<_, ApiError>(live)
        });
        let live = if let Some(context) = context {
            tokio::select! { _=context.cancelled()=>{context.checkpoint().await?;unreachable!()}, value=read=>value }
        } else {
            read.await
        };
        match live {
            Ok(Ok(live)) => {
                if live["Id"] != id {
                    return Err(ApiError(
                        axum::http::StatusCode::CONFLICT,
                        format!("人物回读 ID 与目标 {id} 不一致，已停止保存"),
                    ));
                }
                let name_matches = expected
                    .get("Name")
                    .is_none_or(|name| live["Name"] == *name);
                let people_matches = match expected.get("People") {
                    Some(people) => actors_match(people, &live["People"])?,
                    None => true,
                };
                if name_matches && people_matches {
                    tracing::info!("[人物回读] 媒体/人物ID {id}：本次姓名和演员角色已通过验收");
                    return Ok(());
                }
                reason = Some(if !name_matches {
                    format!(
                        "姓名未一致（期望 {}，实际 {}）",
                        expected["Name"], live["Name"]
                    )
                } else {
                    format!(
                        "演员、角色或人物关联未一致（期望 {}，实际 {}）",
                        expected["People"], live["People"]
                    )
                });
            }
            Ok(Err(error)) => reason = Some(format!("回读失败：{}", error.1)),
            Err(_) => {
                reason.get_or_insert_with(|| format!("回读失败：{path} 请求耗尽30秒共享期限"));
                break;
            }
        }
        let delay = Duration::from_millis(500)
            .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let before = tokio::time::Instant::now();
        if let Some(context) = context {
            context.sleep(delay).await?;
            deadline += before.elapsed().saturating_sub(delay);
        } else {
            tokio::time::sleep(delay).await;
        }
    }
    let message = format!(
        "目标 {id} 的人物修改尚未通过30秒回读验收：{}",
        reason.unwrap_or_else(|| "尚未取得回读结果".into())
    );
    tracing::warn!("[人物回读] {message}");
    Err(ApiError(axum::http::StatusCode::CONFLICT, message))
}

pub(crate) async fn update_name(
    state: &AppState,
    id: &str,
    name: &str,
    context: Option<&crate::jobs::JobContext>,
) -> Result<bool> {
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    let read = crate::media_edit::details(state, id, "Id,ProviderIds,Name");
    let mut person = if let Some(context) = context {
        tokio::select! { _=context.cancelled()=>{context.checkpoint().await?;unreachable!()}, value=read=>value? }
    } else {
        read.await?
    };
    if person["Id"] != id {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            format!("人物 ID 与目标 {id} 不一致，未修改人物"),
        ));
    }
    tmdb_id(&person)?;
    if person["Name"] == name {
        return Ok(false);
    }
    person["Name"] = json!(name);
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    let path = format!("Items/{id}");
    let write = state.emby(reqwest::Method::POST, &path, &[], Some(person));
    if let Some(context) = context {
        tokio::select! { _=context.cancelled()=>{context.checkpoint().await?;unreachable!()}, result=write=>result? };
    } else {
        write.await?;
    }
    verify_readback(state, id, &json!({"Name":name}), context).await?;
    Ok(true)
}

pub async fn upsert(state: &AppState, person: &Value) -> Result<()> {
    let id = crate::cast::person_id(&person["id"])
        .or_else(|| crate::cast::person_id(&person["tmdb_id"]))
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
pub async fn process(state: &AppState, source: &[Value], item: &Value) -> Result<Vec<Value>> {
    process_inner(state, source, item, true).await
}
pub async fn preview(state: &AppState, source: &[Value], item: &Value) -> Result<Vec<Value>> {
    process_inner(state, source, item, false).await
}
fn cast_id(actor: &Value) -> String {
    match &actor["id"] {
        Value::Number(number) if number.as_f64() != Some(0.0) => number.to_string(),
        Value::String(id) if !id.is_empty() && id != "None" => id.clone(),
        _ => String::new(),
    }
}
pub async fn enrich_provider_ids(
    state: &AppState,
    people: &[Value],
    context: &crate::jobs::JobContext,
) -> Result<Vec<Value>> {
    let mut positions = std::collections::HashMap::new();
    let mut actors = vec![];
    let mut pending = vec![];
    for person in people.iter().filter(|person| person["Type"] == "Actor") {
        let mut person = person.clone();
        normalize_tmdb_id(&mut person)?;
        let id = string(&person, "Id");
        if id.is_empty() {
            continue;
        }
        let position = if let Some(position) = positions.get(&id) {
            *position
        } else {
            let position = actors.len();
            positions.insert(id.clone(), position);
            actors.push(person.clone());
            position
        };
        actors[position] = person.clone();
        if string(&person["ProviderIds"], "Tmdb").is_empty() && !chinese(&string(&person, "Name")) {
            pending.push(id);
        }
    }
    if !pending.is_empty() {
        tracing::info!(
            "[演员数据管家] 正在批量获取 {} 位演员的外部 ID...",
            pending.len()
        );
        let missing = actors
            .iter()
            .filter(|person| pending.contains(&string(person, "Id")))
            .cloned()
            .collect::<Vec<_>>();
        for person in complete_provider_ids(state, &missing, Some(context)).await? {
            if let Some(position) = positions.get(&string(&person, "Id")) {
                actors[*position]["ProviderIds"] = person["ProviderIds"].clone();
            }
        }
    }
    Ok(actors)
}

// Media People entries omit ProviderIds; resolve native Person records without
// changing names, roles, duplicate associations, or an already returned TMDb ID.
pub(crate) async fn complete_provider_ids(
    state: &AppState,
    people: &[Value],
    context: Option<&crate::jobs::JobContext>,
) -> Result<Vec<Value>> {
    let mut people = people.to_vec();
    for person in people.iter_mut().filter(|person| person["Type"] == "Actor") {
        normalize_tmdb_id(person)?;
    }
    let ids = people
        .iter()
        .filter(|person| {
            person["Type"] == "Actor" && string(&person["ProviderIds"], "Tmdb").is_empty()
        })
        .map(|person| string(person, "Id"))
        .filter(|id| !id.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for batch in ids.chunks(100) {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        let query = [
            ("Ids", batch.join(",")),
            ("Fields", "Id,Type,ProviderIds".into()),
            ("IncludeItemTypes", "Person".into()),
        ];
        let read = state.emby(reqwest::Method::GET, "Items", &query, None);
        let response = if let Some(context) = context {
            tokio::select! {
                _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
                result=read=>result,
            }
        } else {
            read.await
        }
        .map_err(|error| {
            ApiError(
                error.0,
                format!("演员 {} 的人物元数据回读失败：{}", batch.join(","), error.1),
            )
        })?;
        let returned = response["Items"]
            .as_array()
            .ok_or_else(|| ApiError::bad("Emby 未返回有效人物列表，无法确认演员关联"))?;
        for id in batch {
            let matching = returned
                .iter()
                .filter(|person| person["Id"] == *id && person["Type"] == "Person")
                .collect::<Vec<_>>();
            if matching.len() != 1 {
                return Err(ApiError::bad(format!(
                    "Emby 未返回演员 {id} 的唯一人物记录，无法确认演员关联"
                )));
            }
            let tmdb = tmdb_id(matching[0])?;
            if !tmdb.is_empty() {
                for person in people.iter_mut().filter(|person| {
                    person["Id"] == *id && string(&person["ProviderIds"], "Tmdb").is_empty()
                }) {
                    if person["ProviderIds"]
                        .as_object()
                        .is_none_or(|ids| ids.is_empty())
                    {
                        person["ProviderIds"] = matching[0]["ProviderIds"].clone();
                    }
                    person["ProviderIds"]["Tmdb"] = json!(tmdb);
                }
            }
        }
    }
    Ok(people)
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
        let id = tmdb_id(person)?;
        let name = string(person, "Name");
        let matched = raw
            .iter()
            .rev()
            .find(|a| !id.is_empty() && cast_id(a) == id)
            .or_else(|| {
                raw.iter()
                    .rev()
                    .find(|a| string(a, "name").trim().to_lowercase() == name.trim().to_lowercase())
            });
        let matched = if matched.is_some() {
            matched
        } else {
            let translated: Option<String> = sqlx::query_scalar("SELECT original_text FROM translation_cache WHERE translated_text=$1 AND original_text NOT LIKE 'ai-result:%' LIMIT 1")
                .bind(&name).fetch_optional(&state.db).await.unwrap_or_else(|error| {
                    tracing::warn!("反查演员翻译缓存失败，继续处理其他演员：{error}"); None
                });
            translated.as_ref().and_then(|name| {
                raw.iter()
                    .rev()
                    .find(|a| string(a, "name").trim().to_lowercase() == name.trim().to_lowercase())
            })
        };
        if let Some(actor) = matched {
            let mut actor = actor.clone();
            used.insert(cast_id(&actor));
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
        if !used.contains(&cast_id(&actor)) {
            let mut actor = actor;
            actor["emby_person_id"] = Value::Null;
            result.push(actor);
        }
    }
    let mut unique = vec![];
    let mut positions = std::collections::HashMap::new();
    for actor in result {
        let id = cast_id(&actor);
        if id.is_empty() {
            continue;
        }
        if let Some(position) = positions.get(&id) {
            unique[*position] = actor;
        } else {
            positions.insert(id, unique.len());
            unique.push(actor);
        }
    }
    let mut result = unique;
    result.sort_by_key(|a| a["order"].as_i64().unwrap_or(999));
    if config["remove_actors_without_avatars"] != false {
        result.retain(|p| p["profile_path"].as_str().is_some_and(|s| !s.is_empty()));
    }
    let mut result = crate::cast::format(&result, item, &config);
    for actor in &mut result {
        if let Some(actor) = actor.as_object_mut() {
            actor.remove("_douban_matched");
        }
        actor["provider_ids"] =
            json!({"Tmdb":cast_id(actor),"Imdb":actor["imdb_id"],"Douban":actor["douban_id"]});
        if persist && let Err(error) = upsert(state, actor).await {
            tracing::warn!(
                "演员 {} 数据库回写失败，继续处理其他演员：{}",
                string(actor, "name"),
                error.1
            );
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
    update_emby_inner(state, cast, data, item, None).await
}
pub async fn update_emby_in_job(
    state: &AppState,
    cast: &[Value],
    data: &Value,
    item: &Value,
    context: &crate::jobs::JobContext,
) -> Result<usize> {
    update_emby_inner(state, cast, data, item, Some(context)).await
}
async fn update_emby_inner(
    state: &AppState,
    cast: &[Value],
    data: &Value,
    item: &Value,
    context: Option<&crate::jobs::JobContext>,
) -> Result<usize> {
    tracing::info!(
        "Emby 人物名字同步开始：Emby {}，名称 {}",
        string(item, "Id"),
        string(item, "Name")
    );
    let mut names = std::collections::BTreeMap::<String, String>::new();
    let mut locations = std::collections::HashMap::new();
    let mut allowed = cast
        .iter()
        .filter_map(|p| p["id"].as_i64().map(|n| n.to_string()))
        .collect::<std::collections::HashSet<_>>();
    for person in list(&item["People"]) {
        allowed.insert(tmdb_id(&person)?);
    }
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
        let id = tmdb_id(&person)?;
        if names.contains_key(&id) && !string(&person, "Id").is_empty() {
            locations.insert(id, string(&person, "Id"));
        }
    }
    for id in names.keys() {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
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
        for person in list(&items["Items"]) {
            if tmdb_id(&person)? == *id {
                locations.insert(id.clone(), string(&person, "Id"));
                break;
            }
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
        let Some(name) = names.get(&id) else {
            continue;
        };
        if update_name(state, &location, name, context).await? {
            updated += 1;
            tracing::debug!(
                "Emby 人物名字更新完成：{}，人员 {location}，TMDb {id}",
                name
            );
        } else {
            tracing::debug!(
                "Emby 人物名字更新跳过：{name}，人员 {location}，TMDb {id}，名字无需更新"
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

#[cfg(test)]
#[path = "../tests/fixtures/person_provider_ids.rs"]
mod provider_id_tests;
