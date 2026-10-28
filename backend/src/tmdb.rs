use crate::media_edit::list;
use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use serde_json::{Value, json};
use std::time::Duration;

pub async fn get(state: &AppState, path: &str, query: &[(&str, String)]) -> Result<Value> {
    let config = state.config.read().await.clone();
    let key = string(&config, "tmdb_api_key");
    if key.is_empty() {
        tracing::warn!("TMDb 请求跳过：API Key 未配置，资源 {path}");
        return Err(ApiError::bad("TMDb API Key 未配置"));
    }
    let base = string(&config, "tmdb_api_base_url");
    let mut parameters =
        std::collections::BTreeMap::from([("api_key", key), ("language", "zh-CN".into())]);
    parameters.extend(query.iter().map(|(k, v)| (*k, v.clone())));
    let _slot = crate::strm::slot(state, "secondary", None).await?;
    tracing::debug!("TMDb 请求开始：资源 {path}");
    for attempt in 0..4 {
        let response = state
            .http
            .get(format!("{}{path}", base.trim_end_matches('/')))
            .query(&parameters)
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        let retry_after = response
            .as_ref()
            .ok()
            .and_then(|response| crate::emby::retry_after(response.headers()));
        match response {
            Ok(response)
                if (matches!(response.status().as_u16(), 429 | 500 | 502 | 503 | 504)
                    || (response.status().as_u16() == 413
                        && response.headers().contains_key("Retry-After")))
                    && attempt < 3 =>
            {
                tracing::warn!(
                    "TMDb 请求暂时失败：资源 {path}，HTTP {}，准备重试 {}/3",
                    response.status(),
                    attempt + 1
                );
            }
            Err(error) if (error.is_connect() || error.is_timeout()) && attempt < 3 => {
                tracing::warn!(
                    "TMDb 请求暂时失败：资源 {path}，连接或超时错误，准备重试 {}/3",
                    attempt + 1
                );
            }
            Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                tracing::warn!("TMDb 请求未找到资源：{path}");
                return Err(ApiError(
                    axum::http::StatusCode::NOT_FOUND,
                    "TMDb 资源不存在".into(),
                ));
            }
            response => {
                let data = response
                    .inspect_err(|_| tracing::error!("TMDb 请求失败：资源 {path}，网络错误"))?
                    .error_for_status()
                    .inspect_err(|e| {
                        tracing::error!("TMDb 请求失败：资源 {path}，HTTP {:?}", e.status())
                    })?
                    .json()
                    .await
                    .inspect_err(|_| {
                        tracing::error!("TMDb 请求失败：资源 {path}，响应 JSON 无效")
                    })?;
                tracing::debug!("TMDb 请求完成：资源 {path}，尝试 {} 次", attempt + 1);
                return Ok(data);
            }
        }
        tokio::time::sleep(retry_after.unwrap_or_else(|| {
            Duration::from_secs(if attempt == 0 {
                0
            } else {
                2_u64.pow(attempt + 1)
            })
        }))
        .await;
    }
    unreachable!()
}
pub async fn details(state: &AppState, kind: &str, id: &str) -> Result<Value> {
    details_with_append(state, kind, id, None).await
}
async fn details_with_append(
    state: &AppState,
    kind: &str,
    id: &str,
    append: Option<&str>,
) -> Result<Value> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        tracing::warn!("TMDb 详情获取拒绝：类型 {kind}，ID 无效");
        return Err(ApiError::bad("TMDb ID 无效"));
    }
    let movie = kind == "Movie";
    tracing::info!("TMDb 详情获取开始：类型 {kind}，TMDb {id}");
    let path = format!("/{}/{id}", if movie { "movie" } else { "tv" });
    let append = append.unwrap_or(if movie {
        "credits,videos,images,keywords,external_ids,translations,release_dates,alternative_titles"
    } else {
        "credits,videos,images,keywords,external_ids,translations,content_ratings,alternative_titles"
    });
    let mut data = get(
        state,
        &path,
        &[
            ("append_to_response", append.into()),
            (
                "include_image_language",
                "zh-CN,zh-TW,zh,en,null,ja,ko".into(),
            ),
        ],
    )
    .await?;
    let title_field = if movie { "title" } else { "name" };
    let media_name = string(&data, title_field);
    let english = if movie {
        "english_title"
    } else {
        "english_name"
    };
    if data["original_language"] == "en" {
        data[english] = data[if movie {
            "original_title"
        } else {
            "original_name"
        }]
        .clone();
        tracing::info!(
            "TMDb 详情获取完成：类型 {kind}，TMDb {id}，名称 {}",
            string(&data, title_field)
        );
        return Ok(data);
    }
    if let Some(title) = data["translations"]["translations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["iso_639_1"] == "en")
        .find_map(|v| v["data"][title_field].as_str().filter(|s| !s.is_empty()))
    {
        data[english] = json!(title);
    }
    let config = state.config.read().await.clone();
    let title = flag(&config["ai_translate_title"]);
    let overview = flag(&config["ai_translate_overview"]);
    let need_title = data[english].as_str().is_none_or(str::is_empty) && (movie || title);
    let need_overview = !movie
        && overview
        && data["overview"]
            .as_str()
            .is_none_or(|s| s.chars().count() < 2);
    if need_title || need_overview {
        tracing::info!(
            "TMDb 英文补抓开始：《{media_name}》，类型 {kind}，TMDb {id}，缺少标题源 {}，缺少简介源 {}",
            if need_title { "是" } else { "否" },
            if need_overview { "是" } else { "否" }
        );
        let fallback = match get(state, &path, &[("language", "en-US".into())]).await {
            Ok(value) => value,
            Err(_) => {
                tracing::warn!(
                    "TMDb 英文补抓失败，保留原数据：《{media_name}》，类型 {kind}，TMDb {id}"
                );
                return Ok(data);
            }
        };
        if need_title && !string(&fallback, title_field).is_empty() {
            data[english] = fallback[title_field].clone();
        }
        if need_overview && !string(&fallback, "overview").is_empty() {
            data["overview"] = fallback["overview"].clone();
        }
    }
    tracing::info!(
        "TMDb 详情获取完成：类型 {kind}，TMDb {id}，名称 {}",
        string(&data, title_field)
    );
    Ok(data)
}
fn flag(value: &Value) -> bool {
    match value {
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_i64().is_some_and(|n| n != 0),
        Value::String(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "enabled" | "开启" | "启用"
        ),
        _ => false,
    }
}
pub async fn aggregate(state: &AppState, id: &str, numbers: &[i64]) -> Result<Value> {
    tracing::info!("TMDb 按季聚合开始：TMDb {id}，目标季 {numbers:?}");
    let mut series = details_with_append(state, "Series", id,
        Some("credits,aggregate_credits,keywords,external_ids,content_ratings,alternative_titles,translations")).await?;
    let media_name = string(&series, "name");
    let cast = series["aggregate_credits"]["cast"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|actor| {
            let mut actor = actor.clone();
            if let Some(role) = actor["roles"][0].get("character") {
                actor["character"] = role.clone();
            }
            actor
        })
        .collect::<Vec<_>>();
    if !cast.is_empty() {
        series["credits"]["cast"] = json!(cast);
    }
    let config = state.config.read().await.clone();
    let original = series["original_language"].as_str().unwrap_or("en");
    let images = if config["tmdb_image_language_preference"]
        .as_str()
        .unwrap_or("zh")
        == "zh"
    {
        "zh-CN,zh-TW,zh,en,null".into()
    } else if original != "en" {
        format!("{original},en,null,zh-CN,zh-TW,zh")
    } else {
        "en,null,zh-CN,zh-TW,zh".into()
    };
    let title = flag(&config["ai_translate_title"]);
    let overview = flag(&config["ai_translate_episode_overview"]);
    let mut seasons = vec![];
    let mut episodes = json!({});
    let mut numbers = series["seasons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["season_number"].as_i64())
        .filter(|n| *n >= 0 && (numbers.is_empty() || numbers.contains(n)))
        .collect::<Vec<_>>();
    numbers.sort_unstable();
    numbers.dedup();
    tracing::info!("TMDb 按季范围确认：《{media_name}》，TMDb {id}，本次季 {numbers:?}");
    use futures_util::{FutureExt, StreamExt, stream};
    let limit = crate::strm::thread_limit(state, "secondary").await;
    let pending = stream::iter(numbers).map(|number| {
        let images = images.clone();
        let media_name = media_name.clone();
        async move {
        let mut episodes = json!({});
        tracing::info!("TMDb 季详情获取开始：《{media_name}》，TMDb {id}，第 {number} 季");
        let mut details = match get(
            state,
            &format!("/tv/{id}/season/{number}"),
            &[
                ("append_to_response", "credits,images".into()),
                ("include_image_language", images.clone()),
            ],
        )
        .await
        {
            Ok(data) => data,
            Err(error) => {
                tracing::warn!(
                    "TMDb 季详情获取失败，跳过该季：《{media_name}》，TMDb {id}，第 {number} 季，{}",
                    error.1
                );
                return None;
            }
        };
        if let Some(poster) = details["images"]["posters"][0]["file_path"].as_str() {
            details["poster_path"] = json!(poster);
        }
        let needs_fallback = |episode: &Value| {
            (title && weak_title(&string(episode, "name")))
                || (overview && string(episode, "overview").chars().count() < 2)
        };
        let mut season_episodes = crate::media_edit::list(&details["episodes"]);
        if season_episodes.iter().any(needs_fallback)
            && let Ok(english) = get(
                state,
                &format!("/tv/{id}/season/{number}"),
                &[
                    ("language", "en-US".into()),
                    ("append_to_response", "credits".into()),
                ],
            )
            .await
        {
            for episode in season_episodes.iter_mut().filter(|e| needs_fallback(e)) {
                if let Some(source) = english["episodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|e| e["episode_number"] == episode["episode_number"])
                {
                    let english_title = string(source, "name");
                    if title
                        && weak_title(&string(episode, "name"))
                        && !english_title.is_empty()
                        && !generic_title(&english_title, false)
                    {
                        episode["name"] = json!(english_title);
                    }
                    if overview
                        && string(episode, "overview").chars().count() < 2
                        && !string(source, "overview").is_empty()
                    {
                        episode["overview"] = source["overview"].clone();
                    }
                }
            }
        }
        for episode in &mut season_episodes {
            if let Some(index) = episode["episode_number"].as_i64() {
                if episode.get("credits").is_none() {
                    episode["credits"] = json!({"cast":episode.get("cast").cloned().unwrap_or(json!([])),
                        "guest_stars":episode.get("guest_stars").cloned().unwrap_or(json!([])),
                        "crew":episode.get("crew").cloned().unwrap_or(json!([]))});
                }
                episodes[format!("S{number}E{index}")] = episode.clone();
            }
        }
        details["episodes"] = json!(season_episodes);
        tracing::info!(
            "TMDb 季详情获取完成：《{media_name}》，TMDb {id}，第 {number} 季，分集 {} 个",
            list(&details["episodes"]).len()
        );
        Some((details, episodes))
        }.boxed()
    }).buffer_unordered(limit);
    tokio::pin!(pending);
    while let Some(result) = pending.next().await {
        if let Some((details, children)) = result {
            seasons.push(details);
            episodes
                .as_object_mut()
                .unwrap()
                .extend(children.as_object().unwrap().clone());
        }
    }
    seasons.sort_by_key(|season| season["season_number"].as_i64());
    tracing::info!(
        "TMDb 按季聚合完成：《{media_name}》，TMDb {id}，季 {} 个，分集 {} 个",
        seasons.len(),
        episodes.as_object().map_or(0, |o| o.len())
    );
    Ok(json!({"series_details":series,"seasons_details":seasons,"episodes_details":episodes}))
}
fn generic_title(title: &str, chinese: bool) -> bool {
    let expression = if chinese {
        r"(?i)^(第\s*\d+\s*集|Episode\s*\d+)$"
    } else {
        r"(?i)^Episode\s*\d+$"
    };
    regex::Regex::new(expression)
        .expect("title regex")
        .is_match(title)
}
fn weak_title(title: &str) -> bool {
    title.is_empty()
        || generic_title(title, true)
        || !title
            .chars()
            .any(|c| matches!(c as u32, 0x4e00..=0x9fff | 0x3400..=0x4dbf | 0xf900..=0xfaff))
}
pub async fn aggregate_group(
    state: &AppState,
    id: &str,
    group: &str,
    numbers: &[i64],
) -> Result<Value> {
    if group.is_empty()
        || !group
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        tracing::warn!("TMDb 剧集组聚合拒绝：TMDb {id}，剧集组 ID 无效");
        return Err(ApiError::bad("剧集组 ID 无效"));
    }
    tracing::info!("TMDb 剧集组聚合开始：TMDb {id}，剧集组 {group}，目标季 {numbers:?}");
    let mut series = details_with_append(state,"Series",id,
        Some("credits,aggregate_credits,keywords,external_ids,content_ratings,alternative_titles,translations")).await?;
    let media_name = string(&series, "name");
    let data = get(state, &format!("/tv/episode_group/{group}"), &[]).await?;
    let mut seasons = vec![];
    let mut episodes = serde_json::Map::new();
    for season in data["groups"].as_array().into_iter().flatten() {
        let Some(number) = season["order"]
            .as_i64()
            .or_else(|| season["order"].as_str().and_then(|s| s.parse().ok()))
        else {
            continue;
        };
        if !numbers.is_empty() && !numbers.contains(&number) {
            tracing::debug!(
                "TMDb 剧集组季跳过：《{media_name}》，TMDb {id}，第 {number} 季不在本次范围"
            );
            continue;
        }
        let mut normalized = vec![];
        for (index, source) in season["episodes"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let mut episode = source.clone();
            episode["season_number"] = json!(number);
            episode["episode_number"] = json!(index + 1);
            if episode.get("credits").is_none() {
                episode["credits"] = json!({"cast":[],"guest_stars":[],"crew":[]});
            }
            episodes.insert(format!("S{number}E{}", index + 1), episode.clone());
            normalized.push(episode);
        }
        if normalized.is_empty() {
            tracing::info!(
                "TMDb 剧集组季跳过：《{media_name}》，TMDb {id}，第 {number} 季没有分集"
            );
            continue;
        }
        tracing::info!(
            "TMDb 剧集组季处理完成：《{media_name}》，TMDb {id}，第 {number} 季，分集 {} 个",
            normalized.len()
        );
        seasons.push(json!({"id":format!("{id}-G{group}-S{number}"),
            "name":season["name"].as_str().filter(|s|!s.is_empty()).map(str::to_string).unwrap_or(format!("第 {number} 季")),
            "overview":"","season_number":number,"air_date":normalized[0]["air_date"],"poster_path":null,
            "episodes":normalized,"credits":{"cast":[],"crew":[]}}));
    }
    seasons.sort_by_key(|s| s["season_number"].as_i64().unwrap_or(0));
    series["episode_group_id"] = json!(group);
    series["number_of_episodes"] = json!(
        data["episode_count"]
            .as_u64()
            .filter(|v| *v > 0)
            .unwrap_or(episodes.len() as u64)
    );
    tracing::info!(
        "TMDb 剧集组聚合完成：《{media_name}》，TMDb {id}，剧集组 {group}，季 {} 个，分集 {} 个",
        seasons.len(),
        episodes.len()
    );
    Ok(
        json!({"series_details":series,"seasons_details":seasons,"episodes_details":episodes,"episode_group_id":group}),
    )
}

pub async fn person(state: &AppState, id: i32) -> Result<Value> {
    let mut details = get(
        state,
        &format!("/person/{id}"),
        &[("append_to_response", "external_ids,translations".into())],
    )
    .await?;
    if details["name"] != details["original_name"] {
        if let Some(name) = list(&details["translations"]["translations"])
            .iter()
            .filter(|t| t["iso_639_1"] == "en")
            .find_map(|t| t["data"]["name"].as_str().filter(|s| !s.is_empty()))
        {
            details["english_name_from_translations"] = json!(name);
        }
        if let Some(name) = details["original_name"]
            .as_str()
            .filter(|s| !s.is_empty() && !crate::translation::chinese(s))
        {
            details["foreign_name_from_original"] = json!(name);
        }
    }
    Ok(details)
}
