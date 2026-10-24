use crate::{
    AppState, emby,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    metadata,
    metadata_records::OnlineContext,
    state::string,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub async fn feedback(
    state: &AppState,
    id: &str,
    episode_ids: &[String],
    media_info_only: bool,
    context: &JobContext,
) -> Result<Value> {
    context.checkpoint().await?;
    let fields = "Id,Type,Name,Path,Size,MediaSources,MediaStreams,ProviderIds,DateCreated,Container,RunTimeTicks,SeriesId,ParentIndexNumber,IndexNumber,ParentId";
    let mut item = details(state, id, fields).await?;
    let library = emby::library_for_path(state, &string(&item, "Path")).await?;
    if let Some(library) = &library {
        item["_SourceLibraryId"] = library["ItemId"].clone();
    }
    if emby::path_ignored(state, &string(&item, "Path")).await? {
        return Ok(json!({"updated":0,"status":"ignored_library"}));
    }
    tracing::info!(
        "缓存回流开始：Emby {id}，目标分集 {} 个，仅媒体信息 {media_info_only}",
        episode_ids.len()
    );
    let kind = string(&item, "Type");
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    if !matches!(kind.as_str(), "Movie" | "Series" | "Season" | "Episode") {
        return Err(ApiError::bad("仅支持影视缓存回流"));
    }
    let bound:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1::jsonb AND item_type=$2 LIMIT 1").bind(json!([id])).bind(&kind).fetch_optional(&state.db).await?;
    let tmdb = if tmdb.is_empty() {
        bound
            .as_ref()
            .map(|r| string(r, "tmdb_id"))
            .unwrap_or_default()
    } else {
        tmdb
    };
    let Some((payload, cast)) =
        metadata::reconstruct_full(state, &tmdb, &kind, media_info_only).await?
    else {
        tracing::info!("缓存回流未命中：Emby {id}，类型 {kind}，TMDb {tmdb}");
        return Err(ApiError::bad(
            "未命中可复用元数据/演员缓存，无法执行缓存回流",
        ));
    };
    let children = if kind == "Series" {
        emby::items(state, id, fields, context).await?
    } else {
        vec![]
    };
    let seasons = children
        .iter()
        .filter(|i| i["Type"] == "Season")
        .cloned()
        .collect::<Vec<_>>();
    let episodes = children
        .iter()
        .filter(|i| i["Type"] == "Episode")
        .cloned()
        .collect::<Vec<_>>();
    tracing::info!(
        "缓存回流命中：Emby {id}，TMDb {tmdb}，演员 {} 位，Emby 季 {} 个，分集 {} 个",
        cast.len(),
        seasons.len(),
        episodes.len()
    );
    if kind == "Series" && !episode_ids.is_empty() && !media_info_only {
        let targets = episodes
            .iter()
            .filter(|e| episode_ids.contains(&string(e, "Id")))
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Err(ApiError::bad("无法解析目标分集所属季集号"));
        }
        let group = string(&payload, "episode_group_id");
        for episode in targets {
            let season = episode["ParentIndexNumber"]
                .as_i64()
                .ok_or_else(|| ApiError::bad("目标分集缺少季号"))?;
            let number = episode["IndexNumber"]
                .as_i64()
                .ok_or_else(|| ApiError::bad("目标分集缺少集号"))?;
            let valid = list(&payload["seasons_details"]).iter().any(|s| {
                if s["season_number"].as_i64() != Some(season) {
                    return false;
                }
                let id = match &s["id"] {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                };
                if group.is_empty() {
                    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
                } else {
                    id == format!("{tmdb}-G{group}-S{season}")
                }
            });
            if !valid
                || payload["episodes_details"]
                    .get(format!("S{season}E{number}"))
                    .is_none()
            {
                tracing::warn!(
                    "缓存回流范围检查失败：Emby {id}，TMDb {tmdb}，S{season}E{number} 缓存不完整"
                );
                return Err(ApiError::bad("目标季集缓存不完整，请先完成后置增强"));
            }
        }
    }
    let parents = children
        .iter()
        .filter(|i| !string(i, "ParentId").is_empty())
        .map(|i| (string(i, "Id"), string(i, "ParentId")))
        .collect::<HashMap<_, _>>();
    let context_data = OnlineContext {
        seasons: &seasons,
        episodes: &episodes,
        parents: &parents,
        library_guid: library
            .as_ref()
            .and_then(|l| l["Guid"].as_str())
            .unwrap_or(""),
        episode_ids,
        skip_series_record: false,
    };
    let mut records = if matches!(kind.as_str(), "Movie" | "Series") {
        crate::metadata_records::online(state, &kind, &payload, &cast, &item, &context_data).await?
    } else {
        let mut record = bound.ok_or_else(|| ApiError::bad("子项缺少本地元数据绑定"))?;
        record["asset_details_json"] =
            json!([crate::assets::parse(&item, &HashMap::new(), "", &json!([])).await]);
        record["emby_item_ids_json"] = json!([id]);
        record["file_sha1_json"] = json!([null]);
        record["file_pickcode_json"] = json!([null]);
        for key in ["created_at", "last_updated_at", "last_synced_at"] {
            record.as_object_mut().unwrap().remove(key);
        }
        vec![record]
    };
    // Refreshing identities must not mark other directories offline.
    metadata::ingest(state, &mut records, false).await?;
    let score = review(state, &item, &cast, media_info_only).await?;
    let incoming = records
        .iter()
        .flat_map(|r| {
            list(&r["emby_item_ids_json"])
                .into_iter()
                .filter_map(|id| id.as_str().map(str::to_string))
        })
        .collect::<HashSet<_>>();
    tracing::info!(
        "缓存回流完成：Emby {id}，TMDb {tmdb}，数据库记录 {} 条，评分 {score:.2}",
        records.len()
    );
    Ok(
        json!({"updated":records.len(),"ids":incoming,"cache_hit":true,"score":score,"message":"缓存回流完成，已更新 Emby ID 和媒体资产"}),
    )
}

pub async fn scrape(
    state: &AppState,
    item: &Value,
    episode_ids: &[String],
    group: Option<&str>,
    context: &JobContext,
) -> Result<Value> {
    process(state, item, episode_ids, group, context, false)
        .await
        .inspect_err(|error| {
            tracing::error!(
                "完整刮削失败：Emby {}，名称 {}，{}",
                string(item, "Id"),
                string(item, "Name"),
                error.1
            )
        })
}
pub async fn prepare_new(
    state: &AppState,
    item: &Value,
    episode_ids: &[String],
    group: Option<&str>,
    context: &JobContext,
) -> Result<Value> {
    process(state, item, episode_ids, group, context, true)
        .await
        .inspect_err(|error| {
            tracing::error!(
                "入库预处理失败：Emby {}，名称 {}，{}",
                string(item, "Id"),
                string(item, "Name"),
                error.1
            )
        })
}
async fn process(
    state: &AppState,
    item: &Value,
    episode_ids: &[String],
    group: Option<&str>,
    context: &JobContext,
    pending: bool,
) -> Result<Value> {
    context.checkpoint().await?;
    if emby::path_ignored(state, &string(item, "Path")).await? {
        return Ok(json!({"updated":0,"status":"ignored_library"}));
    }
    tracing::info!(
        "完整刮削开始：Emby {}，名称 {}，目标分集 {} 个，待回流 {pending}",
        string(item, "Id"),
        string(item, "Name"),
        episode_ids.len()
    );
    let kind = string(item, "Type");
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    if !matches!(kind.as_str(), "Movie" | "Series") || tmdb.is_empty() {
        tracing::warn!(
            "完整刮削拒绝：Emby {}，类型 {kind}，缺少有效的影视 TMDb 绑定",
            string(item, "Id")
        );
        return Err(ApiError::bad("完整处理仅支持有 TMDb ID 的电影或剧集"));
    }
    let children = if kind == "Series" {
        emby::items(state,&string(item,"Id"),"Id,Type,Name,Path,Size,MediaSources,MediaStreams,ProviderIds,DateCreated,PremiereDate,Container,RunTimeTicks,ParentId,SeriesId,ParentIndexNumber,IndexNumber",context).await?
    } else {
        vec![]
    };
    let episodes = children
        .iter()
        .filter(|e| e["Type"] == "Episode")
        .cloned()
        .collect::<Vec<_>>();
    let seasons = children
        .iter()
        .filter(|e| e["Type"] == "Season")
        .cloned()
        .collect::<Vec<_>>();
    let target_numbers = episodes
        .iter()
        .filter(|e| episode_ids.is_empty() || episode_ids.contains(&string(e, "Id")))
        .filter_map(|e| e["ParentIndexNumber"].as_i64())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let auto_group = if kind == "Series" && group.is_none() {
        tracing::info!("剧集组自动匹配开始：TMDb {tmdb}，目标季 {target_numbers:?}");
        let selected = episodes
            .iter()
            .filter(|e| episode_ids.is_empty() || episode_ids.contains(&string(e, "Id")))
            .cloned()
            .collect::<Vec<_>>();
        let preferred = metadata::row(state, &tmdb, "Series")
            .await?
            .map(|r| string(&r, "episode_group_id"))
            .unwrap_or_default();
        match crate::episode_groups::match_group(
            state,
            &tmdb,
            &crate::episode_groups::signals(&selected),
            &preferred,
        )
        .await
        {
            Ok(group) => group,
            Err(error) => {
                tracing::warn!("剧集组自动匹配失败，沿用官方季：{}", error.1);
                None
            }
        }
    } else {
        None
    };
    let group = group.or(auto_group.as_deref());
    tracing::info!(
        "刮削范围确认：TMDb {tmdb}，类型 {kind}，目标季 {target_numbers:?}，剧集组 {}",
        group.filter(|g| !g.is_empty()).unwrap_or("官方季")
    );
    let mut data = if kind == "Movie" {
        crate::tmdb::details(state, "Movie", &tmdb).await?
    } else if let Some(group) = group.filter(|s| !s.is_empty()) {
        crate::tmdb::aggregate_group(state, &tmdb, group, &target_numbers).await?
    } else {
        crate::tmdb::aggregate(state, &tmdb, &target_numbers).await?
    };
    tracing::info!("TMDb 元数据已获取：TMDb {tmdb}，类型 {kind}");
    if kind == "Series" {
        crate::douban::premerge_in_job(state, item, &mut data["series_details"], context).await?;
    } else {
        crate::douban::premerge_in_job(state, item, &mut data, context).await?;
    }
    crate::translation::metadata_tree(state, &kind, &mut data, &string(item, "Name")).await?;
    let target_scope = episodes
        .iter()
        .filter(|e| episode_ids.is_empty() || episode_ids.contains(&string(e, "Id")))
        .filter_map(|e| Some((e["ParentIndexNumber"].as_i64()?, e["IndexNumber"].as_i64()?)))
        .collect::<Vec<_>>();
    crate::translation::overview_fallback(state, &kind, &mut data, &target_scope).await?;
    let source = if kind == "Series" {
        &data["series_details"]
    } else {
        &data
    };
    let raw = list(&source["credits"]["cast"]);
    let cast = crate::persons::process(state, &raw, item).await?;
    for director in crate::nfo::directors(source) {
        crate::persons::upsert(state, &director).await?;
    }
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let rating_mapping = crate::db::setting(&state.db, "rating_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_RATING_MAPPING"].clone());
    let priority = crate::db::setting(&state.db, "rating_priority")
        .await?
        .unwrap_or(defaults["DEFAULT_RATING_PRIORITY"].clone());
    let mut payload = crate::metadata_payload::build(
        &kind,
        source,
        Some(&data),
        Some(item),
        &rating_mapping,
        &priority,
    );
    tracing::info!(
        "元数据骨架格式化完成：TMDb {tmdb}，类型 {kind}，演员 {} 位",
        cast.len()
    );
    if kind == "Series" {
        payload["credits"]["crew"] = json!(crate::nfo::directors(source));
        payload["episode_group_id"] = data["episode_group_id"].clone();
    }
    let config = state.config.read().await.clone();
    let studio = crate::db::setting(&state.db, "studio_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_STUDIO_MAPPING"].clone());
    let keywords = crate::db::setting(&state.db, "keyword_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_KEYWORD_MAPPING"].clone());
    let options = crate::nfo_files::Options {
        force: true,
        seasons: &target_numbers,
        episodes: &target_scope,
        config: &config,
        studio_mapping: &studio,
        keyword_mapping: &keywords,
        ..Default::default()
    };
    let written = crate::nfo_files::sync(&kind, item, &payload, &cast, &options).await?;
    let images = crate::images::download(state, &kind, &tmdb, item, &payload, context).await?;
    if state.configured().await && !pending {
        tracing::info!(
            "Emby 元数据刷新开始：Emby {}，TMDb {tmdb}",
            string(item, "Id")
        );
        crate::persons::update_emby(state, &cast, &payload, item).await?;
        crate::strm::with_settings(state, "preserve", || async {
            crate::emby::refresh_item(state, &string(item, "Id"), kind == "Series", context).await
        })
        .await?;
        tracing::info!(
            "Emby 元数据刷新完成：Emby {}，TMDb {tmdb}",
            string(item, "Id")
        );
    }
    let parents = HashMap::new();
    let input = OnlineContext {
        seasons: &seasons,
        episodes: &episodes,
        parents: &parents,
        library_guid: "",
        episode_ids,
        skip_series_record: false,
    };
    let mut records = if pending {
        crate::metadata_records::pending(&kind, &payload, &cast, &json!({"Id":"pending"}))?
    } else {
        crate::metadata_records::online(state, &kind, &payload, &cast, item, &input).await?
    };
    metadata::ingest(state, &mut records, !pending && group.is_some()).await?;
    if pending {
        tracing::info!(
            "入库预处理完成：TMDb {tmdb}，数据库记录 {} 条，NFO 写入 {written} 个，图片下载 {images} 个，等待回流",
            records.len()
        );
        return Ok(
            json!({"updated":records.len(),"nfo_written":written,"images_downloaded":images,"message":"入库预处理完成，等待轻量回流绑定资产"}),
        );
    }
    let score = review(state, item, &cast, pending).await?;
    tracing::info!(
        "完整刮削完成：Emby {}，TMDb {tmdb}，数据库记录 {} 条，NFO 写入 {written} 个，图片下载 {images} 个，评分 {score:.2}",
        string(item, "Id"),
        records.len()
    );
    Ok(
        json!({"updated":records.len(),"nfo_written":written,"images_downloaded":images,"score":score,"message":"完整刮削已完成，元数据已写入本地文件与数据库"}),
    )
}

pub async fn patch_cached_series(
    state: &AppState,
    id: &str,
    payload: &mut Value,
    scope: &[(i64, i64)],
) -> Result<bool> {
    tracing::info!("追更缓存补齐开始：TMDb {id}，目标季集 {scope:?}");
    let config = state.config.read().await.clone();
    let mut seasons = list(&payload["seasons_details"])
        .into_iter()
        .filter_map(|s| Some((s["season_number"].as_i64()?, s)))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut episodes = payload["episodes_details"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut patch = json!({"series_details":{"name":payload["name"],"genres":payload["genres"],"first_air_date":payload["first_air_date"]},"seasons_details":[],"episodes_details":{}});
    let mut fetched = std::collections::BTreeMap::new();
    for &(season, number) in scope {
        let key = format!("S{season}E{number}");
        let existing = episodes.get(&key);
        let needs_credits = config["max_episode_actors_to_process"]
            .as_u64()
            .unwrap_or(0)
            > 0
            && existing.is_none_or(|e| {
                list(&e["credits"]["cast"]).is_empty()
                    && list(&e["credits"]["guest_stars"]).is_empty()
            });
        if seasons.contains_key(&season) && existing.is_some() && !needs_credits {
            tracing::debug!("追更缓存补齐跳过：TMDb {id}，{key} 缓存完整");
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = fetched.entry(season) {
            tracing::info!("追更季元数据补抓开始：TMDb {id}，第 {season} 季");
            let data = crate::tmdb::get(state, &format!("/tv/{id}/season/{season}"), &[]).await?;
            if data["id"].as_u64().is_none_or(|id| id == 0) {
                tracing::warn!("追更季元数据补抓失败：TMDb {id}，第 {season} 季没有有效元数据");
                return Err(ApiError::bad(format!("未能获取第 {season} 季元数据")));
            }
            entry.insert(data);
        }
        let data = &fetched[&season];
        if let std::collections::btree_map::Entry::Vacant(entry) = seasons.entry(season) {
            let mut new = data.clone();
            for key in ["episodes", "credits", "aggregate_credits"] {
                new.as_object_mut().unwrap().remove(key);
            }
            new["season_number"] = json!(season);
            entry.insert(new.clone());
            patch["seasons_details"].as_array_mut().unwrap().push(new);
        }
        if existing.is_some() && !needs_credits {
            continue;
        }
        let mut episode = list(&data["episodes"])
            .into_iter()
            .find(|e| {
                e["episode_number"].as_i64() == Some(number)
                    && e["id"].as_u64().is_some_and(|id| id > 0)
            })
            .ok_or_else(|| ApiError::bad(format!("TMDb 尚未提供 {key} 元数据")))?;
        episode["season_number"] = json!(season);
        let guest = list(&episode["credits"]["guest_stars"]);
        let guest = if guest.is_empty() {
            list(&episode["guest_stars"])
        } else {
            guest
        };
        let crew = list(&episode["credits"]["crew"]);
        let crew = if crew.is_empty() {
            list(&episode["crew"])
        } else {
            crew
        };
        let credits =
            json!({"cast":list(&episode["credits"]["cast"]),"guest_stars":guest,"crew":crew});
        episode["credits"] = credits.clone();
        if let Some(existing) = existing {
            episode = json!({"id":existing["id"],"season_number":season,"episode_number":number,"credits":credits});
        }
        patch["episodes_details"][&key] = episode;
        tracing::debug!("追更分集缓存已补齐：TMDb {id}，{key}，补抓演员表 {needs_credits}");
    }
    let patched = !list(&patch["seasons_details"]).is_empty()
        || patch["episodes_details"]
            .as_object()
            .is_some_and(|o| !o.is_empty());
    if patched {
        tracing::info!(
            "追更缓存补齐数据已就绪：TMDb {id}，新增季 {} 个，补齐分集 {} 个",
            list(&patch["seasons_details"]).len(),
            patch["episodes_details"].as_object().map_or(0, |o| o.len())
        );
        crate::translation::metadata_tree(state, "Series", &mut patch, &string(payload, "name"))
            .await?;
        for season in list(&patch["seasons_details"]) {
            seasons.insert(season["season_number"].as_i64().unwrap(), season);
        }
        for (key, episode) in patch["episodes_details"].as_object().unwrap() {
            let old = episodes.entry(key.clone()).or_insert(json!({}));
            old.as_object_mut()
                .unwrap()
                .extend(episode.as_object().unwrap().clone());
        }
    }
    payload["seasons_details"] = json!(seasons.into_values().collect::<Vec<_>>());
    payload["episodes_details"] = json!(episodes);
    tracing::info!("追更缓存补齐完成：TMDb {id}，缓存发生变化 {patched}");
    Ok(patched)
}
pub async fn prepare_cached(
    state: &AppState,
    item: &Value,
    episode_ids: &[String],
    requested_group: Option<&str>,
    context: &JobContext,
) -> Result<bool> {
    context.checkpoint().await?;
    if emby::path_ignored(state, &string(item, "Path")).await? {
        return Ok(true);
    }
    tracing::info!(
        "缓存预处理开始：Emby {}，名称 {}，目标分集 {} 个",
        string(item, "Id"),
        string(item, "Name"),
        episode_ids.len()
    );
    let id = string(&item["ProviderIds"], "Tmdb");
    let kind = string(item, "Type");
    let Some((mut payload, cast)) = metadata::reconstruct_full(state, &id, &kind, false).await?
    else {
        tracing::info!("缓存预处理未命中：类型 {kind}，TMDb {id}");
        return Ok(false);
    };
    if kind == "Series"
        && requested_group.is_some_and(|group| group != string(&payload, "episode_group_id"))
    {
        tracing::info!("缓存预处理未命中：TMDb {id}，请求剧集组与缓存不一致");
        return Ok(false);
    }
    let children = if kind == "Series" {
        emby::items(
            state,
            &string(item, "Id"),
            "Id,Type,ParentIndexNumber,IndexNumber,Path,ProviderIds,PremiereDate,Name",
            context,
        )
        .await?
    } else {
        vec![]
    };
    let scope = children
        .iter()
        .filter(|e| {
            e["Type"] == "Episode"
                && (episode_ids.is_empty() || episode_ids.contains(&string(e, "Id")))
        })
        .filter_map(|e| Some((e["ParentIndexNumber"].as_i64()?, e["IndexNumber"].as_i64()?)))
        .collect::<Vec<_>>();
    if kind == "Series" && !episode_ids.is_empty() && scope.is_empty() {
        return Err(ApiError::bad("无法确定本批目标季集"));
    }
    let scope = scope
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cached_group = string(&payload, "episode_group_id");
    tracing::info!(
        "缓存预处理范围确认：TMDb {id}，目标季集 {scope:?}，缓存剧集组 {}",
        if cached_group.is_empty() {
            "官方季"
        } else {
            &cached_group
        }
    );
    if kind == "Series" && requested_group.is_none() && !cached_group.is_empty() {
        let selected = children
            .iter()
            .filter(|e| {
                e["Type"] == "Episode"
                    && (episode_ids.is_empty() || episode_ids.contains(&string(e, "Id")))
            })
            .cloned()
            .collect::<Vec<_>>();
        if let Ok(group) = crate::episode_groups::match_group(
            state,
            &id,
            &crate::episode_groups::signals(&selected),
            &cached_group,
        )
        .await
            && group.as_deref() != Some(&cached_group)
        {
            tracing::info!("缓存预处理未命中：TMDb {id}，当前分集与缓存剧集组不匹配");
            return Ok(false);
        }
    }
    if kind == "Series"
        && !cached_group.is_empty()
        && scope.iter().any(|(s, e)| {
            !list(&payload["seasons_details"]).iter().any(|season| {
                season["season_number"].as_i64() == Some(*s)
                    && season["id"] == format!("{id}-G{cached_group}-S{s}")
            }) || payload["episodes_details"]
                .get(format!("S{s}E{e}"))
                .is_none()
        })
    {
        tracing::info!("缓存预处理未命中：TMDb {id}，目标季集的剧集组缓存不完整");
        return Ok(false);
    }
    let old_seasons = list(&payload["seasons_details"])
        .iter()
        .filter_map(|s| s["season_number"].as_i64())
        .collect::<std::collections::BTreeSet<_>>();
    let old_episodes = payload["episodes_details"]
        .as_object()
        .map(|m| m.keys().cloned().collect::<std::collections::BTreeSet<_>>())
        .unwrap_or_default();
    let patched = if kind == "Series" && string(&payload, "episode_group_id").is_empty() {
        patch_cached_series(state, &id, &mut payload, &scope).await?
    } else {
        false
    };
    let fallback =
        crate::translation::overview_fallback(state, &kind, &mut payload, &scope).await?;
    let config = state.config.read().await.clone();
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let studios = crate::db::setting(&state.db, "studio_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_STUDIO_MAPPING"].clone());
    let keywords = crate::db::setting(&state.db, "keyword_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_KEYWORD_MAPPING"].clone());
    let seasons = scope
        .iter()
        .map(|(s, _)| *s)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let options = crate::nfo_files::Options {
        seasons: &seasons,
        episodes: &scope,
        config: &config,
        studio_mapping: &studios,
        keyword_mapping: &keywords,
        ..Default::default()
    };
    crate::nfo_files::sync(&kind, item, &payload, &cast, &options).await?;
    if patched || !fallback.is_empty() {
        let mut new = payload.clone();
        new["seasons_details"] = json!(
            list(&payload["seasons_details"])
                .into_iter()
                .filter(|s| s["season_number"]
                    .as_i64()
                    .is_some_and(|n| !old_seasons.contains(&n)))
                .collect::<Vec<_>>()
        );
        new["episodes_details"] = json!(
            payload["episodes_details"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(k, _)| !old_episodes.contains(*k) || fallback.contains(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>()
        );
        let mut pending =
            crate::metadata_records::pending(&kind, &new, &cast, &json!({"Id":"pending"}))?;
        pending.retain(|r| r["item_type"] != "Series" || fallback.contains("main"));
        metadata::ingest(state, &mut pending, false).await?;
    }
    let path = std::path::Path::new(item["Path"].as_str().unwrap_or(""));
    let root = if kind == "Series" {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    if !root.join("poster.jpg").is_file() {
        tracing::info!("缓存恢复图片补抓开始：TMDb {id}，本地海报缺失");
        crate::images::download(state, &kind, &id, item, &payload, context).await?;
    } else {
        tracing::info!("缓存恢复图片补抓跳过：TMDb {id}，本地海报已存在");
    }
    tracing::info!(
        "缓存预处理完成：TMDb {id}，复用演员 {} 位，目标分集 {} 个，缓存已补齐 {patched}",
        cast.len(),
        scope.len()
    );
    Ok(true)
}

async fn review(
    state: &AppState,
    item: &Value,
    cast: &[Value],
    media_info_only: bool,
) -> Result<f64> {
    let config = state.config.read().await.clone();
    let threshold = config["min_score_for_review"].as_f64().unwrap_or(7.0);
    let animation = list(&item["Genres"]).iter().any(|g| {
        ["Animation", "动画", "Documentary", "纪录", "记录"].contains(
            &g.as_str()
                .unwrap_or_else(|| g["name"].as_str().unwrap_or("")),
        )
    });
    let mut score = if media_info_only {
        10.0_f64.max(threshold)
    } else {
        crate::persons::score(cast, animation)
    };
    let mut missing = None;
    if matches!(item["Type"].as_str(), Some("Movie" | "Episode")) {
        let sidecar = std::path::Path::new(item["Path"].as_str().unwrap_or(""))
            .with_extension("")
            .to_string_lossy()
            .to_string()
            + "-mediainfo.json";
        let video = list(&item["MediaSources"])
            .iter()
            .flat_map(|s| list(&s["MediaStreams"]))
            .any(|s| {
                s["Type"] == "Video"
                    && s["Width"].as_u64().is_some_and(|n| n > 0)
                    && s["Height"].as_u64().is_some_and(|n| n > 0)
            });
        if !std::path::Path::new(&sidecar).is_file() && !video {
            missing = Some("缺失媒体信息: ".to_string());
        }
    } else if item["Type"] == "Series" {
        let tmdb = string(&item["ProviderIds"], "Tmdb");
        let episodes:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE item_type='Episode' AND in_library=true AND parent_series_tmdb_id=$1 ORDER BY season_number,episode_number").bind(tmdb).fetch_all(&state.db).await?;
        for episode in episodes {
            let assets = list(&episode["asset_details_json"]);
            let path = assets
                .first()
                .map(|a| string(a, "path"))
                .unwrap_or_default();
            let sidecar = std::path::Path::new(&path)
                .with_extension("")
                .to_string_lossy()
                .to_string()
                + "-mediainfo.json";
            let video = assets
                .iter()
                .flat_map(|a| list(&a["video_streams"]))
                .any(|s| {
                    s["width"].as_u64().is_some_and(|n| n > 0)
                        && s["height"].as_u64().is_some_and(|n| n > 0)
                });
            if path.is_empty() || (!std::path::Path::new(&sidecar).is_file() && !video) {
                missing = Some(format!(
                    "缺失媒体信息: [S{}E{}]",
                    episode["season_number"], episode["episode_number"]
                ));
                break;
            }
        }
    }
    if missing.is_some() {
        score = 0.0;
    }
    let reason = missing.or_else(|| {
        (score < threshold).then(|| format!("处理评分 ({score:.2}) 低于阈值 ({threshold:.1})。"))
    });
    let id = if item["Type"] == "Episode" && !string(item, "SeriesId").is_empty() {
        string(item, "SeriesId")
    } else {
        string(item, "Id")
    };
    let name = string(item, "Name");
    let kind = if item["Type"] == "Episode" {
        "Series"
    } else {
        item["Type"].as_str().unwrap_or("")
    };
    let mut transaction = state.db.begin().await?;
    tracing::info!(
        "处理评分写入开始：Emby {id}，名称 {name}，评分 {score:.2}，待复核 {}",
        reason.is_some()
    );
    sqlx::query("INSERT INTO processed_log(item_id,item_name,score) VALUES($1,$2,$3) ON CONFLICT(item_id) DO UPDATE SET item_name=EXCLUDED.item_name,score=EXCLUDED.score,processed_at=NOW()")
        .bind(&id).bind(&name).bind(score as f32).execute(&mut *transaction).await?;
    if let Some(reason) = reason {
        sqlx::query("INSERT INTO failed_log(item_id,item_name,item_type,reason,score) VALUES($1,$2,$3,$4,$5) ON CONFLICT(item_id) DO UPDATE SET item_name=EXCLUDED.item_name,item_type=EXCLUDED.item_type,reason=EXCLUDED.reason,score=EXCLUDED.score,failed_at=NOW()")
        .bind(&id).bind(&name).bind(kind).bind(reason).bind(score as f32).execute(&mut *transaction).await?;
    } else {
        sqlx::query("DELETE FROM failed_log WHERE item_id=$1")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    tracing::info!(
        "处理评分写入完成：Emby {}，评分 {score:.2}",
        string(item, "Id")
    );
    Ok(score)
}
