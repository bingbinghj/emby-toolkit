use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
    translation::chinese,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha1::Sha1;
use std::{collections::HashSet, time::Duration};
use tokio::sync::Mutex;

pub struct Service {
    pub api_base: String,
    pub imdb_base: String,
    gate: Mutex<Option<tokio::time::Instant>>,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            api_base: "https://frodo.douban.com/api/v2".into(),
            imdb_base: "https://api.douban.com/v2".into(),
            gate: Mutex::new(None),
        }
    }
}
pub fn signature(path: &str, date: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(b"bf7dddc7c9cfe6f7").unwrap();
    let encoded =
        percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC).to_string();
    mac.update(format!("GET&{encoded}&{date}").as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}
async fn request(
    state: &AppState,
    path: &str,
    post: bool,
    query: &[(&str, String)],
) -> Result<Value> {
    let config = state.config.read().await.clone();
    let interval = config["api_douban_default_cooldown_seconds"]
        .as_f64()
        .filter(|n| *n > 0.0)
        .unwrap_or(1.5);
    let mut gate = state.douban.gate.lock().await;
    if let Some(before) = *gate {
        let ready = before + Duration::from_secs_f64(interval);
        if ready > tokio::time::Instant::now() {
            tokio::time::sleep_until(ready).await;
        }
    }
    *gate = Some(tokio::time::Instant::now());
    drop(gate);
    let mut request = if post {
        state.http.post(format!("{}{path}", state.douban.imdb_base))
    } else {
        state.http.get(format!("{}{path}", state.douban.api_base))
    };
    request=request.header("User-Agent","api-client/1 com.douban.frodo/7.22.0.beta9(231) Android/23 product/Mate 40 vendor/HUAWEI model/Mate 40 brand/HUAWEI rom/android network/wifi platform/AndroidPad");
    let cookie = string(&config, "douban_cookie");
    if !cookie.is_empty() {
        request = request.header("Cookie", cookie);
    }
    let mut params = query.to_vec();
    if post {
        params.push(("apikey", "0ab215a8b1977939201640fa14c66bab".into()));
        request = request.form(&params);
    } else {
        let date = chrono::Local::now().format("%Y%m%d").to_string();
        params.extend([
            ("apiKey", "0dad551ec0f84ed02907ff5c42e8ec70".into()),
            ("os_rom", "android".into()),
            ("_sig", signature(&format!("/api/v2{path}"), &date)),
            ("_ts", date),
        ]);
        request = request.query(&params);
    }
    let response: Value = request
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if response["code"] == 1080 || response["error"].as_str().is_some_and(|s| !s.is_empty()) {
        return Err(ApiError::bad("豆瓣API返回错误"));
    }
    Ok(response)
}
pub fn match_search(response: &Value, name: &str, kind: &str, year: &str) -> Option<Value> {
    let mut candidates = vec![];
    for item in list(&response["items"]) {
        let target = &item["target"];
        let title = string(target, "title");
        let id = target["id"].as_str().map(str::to_string).unwrap_or(
            target["id"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_default(),
        );
        let actual_year = target["year"].as_str().map(str::to_string).unwrap_or(
            target["year"]
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_default(),
        );
        if !["movie", "tv"].contains(&item["target_type"].as_str().unwrap_or(""))
            || (!kind.is_empty() && item["target_type"] != kind)
            || title.trim().is_empty()
            || id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        if !year.is_empty()
            && !matches!((year.parse::<i64>(),actual_year.parse::<i64>()),(Ok(a),Ok(b)) if (a-b).abs()<=1)
        {
            continue;
        }
        let candidate = json!({"id":id,"title":title,"original_title":target["original_title"],"year":actual_year,"type":item["target_type"],"source":"name_search_candidate"});
        if title.trim().eq_ignore_ascii_case(name.trim())
            && (year.is_empty() || actual_year == year)
        {
            let mut exact = candidate;
            exact["source"] = json!("name_search_exact");
            return Some(exact);
        }
        candidates.push(candidate);
    }
    candidates
        .iter()
        .find(|c| !year.is_empty() && c["year"] == year)
        .cloned()
        .or_else(|| candidates.into_iter().next())
}
pub async fn cast(state: &AppState, item: &Value, data: &Value) -> Result<Vec<Value>> {
    let kind = if item["Type"] == "Series" {
        "tv"
    } else {
        "movie"
    };
    let name = string(item, "Name");
    let imdb = data["external_ids"]["imdb_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| item["ProviderIds"]["Imdb"].as_str())
        .unwrap_or("");
    let date = data["release_date"]
        .as_str()
        .or_else(|| data["first_air_date"].as_str())
        .unwrap_or("");
    let year = date.get(..4).map(str::to_string).unwrap_or(
        item["ProductionYear"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_default(),
    );
    let mut id = None;
    if imdb.starts_with("tt")
        && let Ok(result) = request(state, &format!("/movie/imdb/{imdb}"), true, &[]).await
    {
        let value = string(&result, "id");
        let regex = regex::Regex::new(r"/(?:movie|tv)/(\d+)/?$").unwrap();
        id = regex.captures(&value).map(|c| c[1].to_string());
    }
    if id.is_none() {
        let query = if !year.is_empty() && !name.contains(&format!("({year})")) {
            format!("{name} {year}")
        } else {
            name.clone()
        };
        let response = request(
            state,
            "/search/weixin",
            false,
            &[("q", query), ("start", "0".into()), ("count", "20".into())],
        )
        .await?;
        id = match_search(&response, &name, kind, &year).map(|m| string(&m, "id"));
    }
    let Some(id) = id else {
        return Ok(vec![]);
    };
    let response = request(state, &format!("/{kind}/{id}/celebrities"), false, &[]).await?;
    Ok(list(response.get("celebrities").unwrap_or(&response["actors"])).iter().enumerate().map(|(i,a)|{
        let role=a["character"].as_str().filter(|s|!s.is_empty()).map(str::to_string).unwrap_or_else(||list(&a["attrs"]["role"]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" / "));
        let image=a.get("avatar").unwrap_or(&a["cover_url"]);let image=if image.is_object(){image.get("large").unwrap_or(&image["normal"]).clone()}else{image.clone()};
        json!({"id":a["id"].as_i64().or_else(||a["id"].as_str().and_then(|s|s.parse::<i64>().ok())),"name":a["name"],"character":crate::cast::clean_character(&role),"original_name":a.get("latin_name").unwrap_or(&a["name_en"]),"profile_path":image,"order":a.get("rank").cloned().unwrap_or(json!(i))})
    }).collect())
}
pub fn should_fetch(cast: &[Value], config: &Value) -> bool {
    if config["douban_enable_online_api"] != true || cast.is_empty() {
        return false;
    }
    let limit = config["max_actors_to_process"]
        .as_u64()
        .filter(|n| *n > 0)
        .unwrap_or(30) as usize;
    if cast.len() < limit {
        return true;
    }
    let mut actors = cast.to_vec();
    actors.sort_by_key(|a| a["order"].as_i64().filter(|n| *n >= 0).unwrap_or(999));
    actors.iter().take(limit).any(|a| {
        let name = a["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| a["original_name"].as_str())
            .unwrap_or("");
        let role = crate::cast::clean_character(&string(a, "character"));
        (!name.trim().is_empty() && !chinese(name)) || (!role.is_empty() && !chinese(&role))
    })
}
pub fn best_role(current: &str, candidate: &str) -> String {
    let clean = |s: &str| {
        let r = regex::Regex::new(r"^\s*(?:配音|配)\s*[:：]?\s*")
            .unwrap()
            .replace(s.trim(), "")
            .to_string();
        if regex::Regex::new(r"[·•]{2,}").unwrap().is_match(&r) {
            String::new()
        } else {
            r
        }
    };
    let current = clean(current);
    let candidate = clean(candidate);
    let placeholder =
        |s: &str| ["actor", "actress", "演员", "配音"].contains(&s.to_lowercase().as_str());
    if chinese(&candidate) && !placeholder(&candidate) {
        candidate
    } else if chinese(&current) && !placeholder(&current) && !chinese(&candidate) {
        current
    } else if !candidate.is_empty() && !placeholder(&candidate) {
        candidate
    } else if !current.is_empty() && !placeholder(&current) {
        current
    } else if !candidate.is_empty() {
        candidate
    } else {
        current
    }
}
pub fn premerge(raw: &[Value], douban: &[Value]) -> Vec<Value> {
    premerge_cast(raw, douban, false)
}
fn premerge_cast(raw: &[Value], douban: &[Value], marked: bool) -> Vec<Value> {
    let mut actors = raw.to_vec();
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    for candidate in douban {
        let name = string(candidate, "name").trim().to_lowercase();
        if name.is_empty() || !names.insert(name.clone()) {
            continue;
        }
        let id = candidate["id"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| string(candidate, "id"));
        if !id.is_empty() && !seen.insert(id.clone()) {
            continue;
        }
        let english = string(candidate, "latin_name").trim().to_lowercase();
        if let Some(actor) = actors.iter_mut().find(|a| {
            [
                string(a, "name").trim().to_lowercase(),
                string(a, "original_name").trim().to_lowercase(),
            ]
            .iter()
            .any(|a| a == &name || (!english.is_empty() && a == &english))
        }) {
            if chinese(&string(candidate, "name")) {
                actor["name"] = candidate["name"].clone();
            }
            if string(actor, "original_name").is_empty() && !english.is_empty() {
                actor["original_name"] = candidate["latin_name"].clone();
            }
            actor["character"] = json!(best_role(
                &crate::cast::clean_character(&string(actor, "character")),
                &crate::cast::clean_character(&string(candidate, "character"))
            ));
            if !id.is_empty() {
                actor["douban_id"] = json!(id);
            }
            if marked {
                actor["_douban_matched"] = json!(true);
            }
        }
    }
    actors
}
fn actor_id(actor: &Value) -> Option<i32> {
    actor["id"]
        .as_i64()
        .or_else(|| actor["id"].as_str().and_then(|id| id.parse().ok()))
        .filter(|id| *id > 0 && *id <= i32::MAX as i64)
        .map(|id| id as i32)
}
fn actor_count(actors: &[Value]) -> usize {
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    actors
        .iter()
        .filter(|actor| {
            let name = string(actor, "name").trim().to_string();
            !name.is_empty()
                && names.insert(name)
                && actor_id(actor).is_some_and(|id| ids.insert(id))
        })
        .count()
}
async fn mapped_person(state: &AppState, douban: &str, imdb: &str) -> Result<Option<Value>> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(p) FROM person_metadata p WHERE ($1 <> '' AND douban_celebrity_id=$1) OR ($2 <> '' AND imdb_id=$2) LIMIT 1")
        .bind(douban).bind(imdb).fetch_optional(&state.db).await?)
}
fn add_actor(actors: &mut Vec<Value>, candidate: &Value, id: i32, metadata: &Value, imdb: &str) {
    if actors.iter().any(|actor| actor_id(actor) == Some(id)) {
        return;
    }
    let mut actor = json!({"_douban_matched":true,"id":id,"name":candidate["name"],"character":candidate["character"],"order":999,"imdb_id":if imdb.is_empty(){None}else{Some(imdb)},"douban_id":candidate["id"].as_i64().map(|id|id.to_string()).unwrap_or_else(||string(candidate,"id")),"emby_person_id":null});
    for field in [
        "original_name",
        "profile_path",
        "gender",
        "adult",
        "popularity",
    ] {
        if let Some(value) = metadata.get(field) {
            actor[field] = value.clone();
        }
    }
    if string(&actor, "original_name").is_empty() {
        actor["original_name"] = candidate["latin_name"].clone();
    }
    tracing::debug!(tmdb_id = id, name = %string(&actor,"name"), "豆瓣补充演员身份匹配完成");
    actors.push(actor);
}
pub async fn supplement(state: &AppState, raw: &[Value], douban: &[Value]) -> Result<Vec<Value>> {
    let mut actors = premerge_cast(raw, douban, true);
    let config = state.config.read().await.clone();
    let limit = config["max_actors_to_process"]
        .as_u64()
        .filter(|limit| *limit > 0)
        .unwrap_or(30) as usize;
    if douban.is_empty() || actor_count(&actors) >= limit {
        return Ok(actors);
    }
    let original_ids = raw.iter().filter_map(actor_id).collect::<HashSet<_>>();
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let mut unmatched = vec![];
    // Resolve all cached Douban identities before spending online lookups on the remaining actors.
    for candidate in douban {
        let name = string(candidate, "name").trim().to_lowercase();
        let id = candidate["id"]
            .as_i64()
            .map(|id| id.to_string())
            .unwrap_or_else(|| string(candidate, "id"));
        if name.is_empty()
            || !names.insert(name)
            || id.is_empty()
            || !ids.insert(id.clone())
            || actors.iter().any(|actor| string(actor, "douban_id") == id)
        {
            continue;
        }
        if let Some(metadata) = mapped_person(state, &id, "").await?
            && let Some(id) = metadata["tmdb_person_id"]
                .as_i64()
                .and_then(|id| i32::try_from(id).ok())
        {
            add_actor(
                &mut actors,
                candidate,
                id,
                &metadata,
                &string(&metadata, "imdb_id"),
            );
            continue;
        }
        unmatched.push(candidate);
    }
    if config["douban_enable_online_api"] == true && !string(&config, "tmdb_api_key").is_empty() {
        for candidate in unmatched {
            if actor_count(&actors) >= limit {
                break;
            }
            let id = candidate["id"]
                .as_i64()
                .map(|id| id.to_string())
                .unwrap_or_else(|| string(candidate, "id"));
            if !id.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let details = request(state, &format!("/celebrity/{id}"), false, &[]).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let details = match details {
                Ok(details) => details,
                Err(error) => {
                    tracing::warn!(douban_id = %id, "豆瓣演员详情查询失败：{}，{}", string(candidate, "name"), error.1);
                    continue;
                }
            };
            let Some(imdb) = list(&details["extra"]["info"]).iter().find_map(|info| {
                (info.as_array().is_some_and(|info| info.len() == 2) && info[0] == "IMDb编号")
                    .then(|| info[1].as_str())
                    .flatten()
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_string)
            }) else {
                continue;
            };
            if let Some(metadata) = mapped_person(state, "", &imdb).await?
                && let Some(id) = metadata["tmdb_person_id"]
                    .as_i64()
                    .and_then(|id| i32::try_from(id).ok())
            {
                add_actor(&mut actors, candidate, id, &metadata, &imdb);
                continue;
            }
            let Ok(found) = crate::tmdb::get(
                state,
                &format!("/find/{imdb}"),
                &[
                    ("external_source", "imdb_id".into()),
                    ("language", "en-US".into()),
                ],
            )
            .await
            else {
                continue;
            };
            let Some(id) = list(&found["person_results"]).first().and_then(actor_id) else {
                continue;
            };
            let metadata: Option<Value> = sqlx::query_scalar(
                "SELECT to_jsonb(p) FROM person_metadata p WHERE tmdb_person_id=$1",
            )
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
            if metadata.is_none()
                && let Some(existing) = actors.iter_mut().find(|actor| actor_id(actor) == Some(id))
            {
                if chinese(&string(candidate, "name")) {
                    existing["name"] = candidate["name"].clone();
                    if existing["_douban_matched"] != true {
                        existing["_douban_matched"] = json!("name");
                    }
                }
            } else {
                add_actor(
                    &mut actors,
                    candidate,
                    id,
                    &metadata.unwrap_or(Value::Null),
                    &imdb,
                );
            }
        }
    }
    let added = actors.len().saturating_sub(raw.len());
    let mut photos = 0;
    for actor in actors
        .iter_mut()
        .filter(|actor| actor_id(actor).is_some_and(|id| !original_ids.contains(&id)))
    {
        if string(actor, "profile_path").is_empty()
            && !string(&config, "tmdb_api_key").is_empty()
            && let Ok(details) = crate::tmdb::person(state, actor_id(actor).unwrap()).await
        {
            actor["profile_path"] = details["profile_path"].clone();
        }
        if !string(actor, "profile_path").is_empty() {
            photos += 1;
        }
    }
    if added > 0 {
        tracing::info!(added, photos, "[演员资料] 豆瓣补充演员身份与头像检查完成");
    }
    Ok(actors)
}
pub async fn premerge_online(state: &AppState, item: &Value, data: &mut Value) -> Result<()> {
    let media_name = string(item, "Name");
    let raw = list(&data["credits"]["cast"]);
    if !should_fetch(&raw, &*state.config.read().await) {
        return Ok(());
    }
    tracing::info!(item_id = %string(item, "Id"), "[演员资料] 开始匹配《{media_name}》豆瓣演员与角色");
    match cast(state, item, data).await {
        Ok(cast) => {
            tracing::info!(
                matched = cast.len(),
                "[演员资料] 《{media_name}》豆瓣演员匹配完成"
            );
            // Keep Douban's foreign names available for matching before the shared translation pass.
            let cast = cast
                .into_iter()
                .map(|mut a| {
                    a["latin_name"] = a["original_name"].clone();
                    a
                })
                .collect::<Vec<_>>();
            data["credits"]["cast"] = json!(supplement(state, &raw, &cast).await?);
        }
        Err(error) => tracing::warn!("豆瓣演员补充失败：《{media_name}》，{}", error.1),
    }
    Ok(())
}

pub async fn premerge_in_job(
    state: &AppState,
    item: &Value,
    data: &mut Value,
    context: &crate::jobs::JobContext,
) -> Result<()> {
    context.checkpoint().await?;
    tokio::select! {
        _ = context.cancelled() => context.checkpoint().await,
        result = premerge_online(state, item, data) => result,
    }
}
