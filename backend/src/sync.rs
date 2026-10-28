use crate::{
    AppState, assets, emby, error::Result, jobs::JobContext, media_edit::list, metadata,
    metadata_payload, state::string, tmdb, versions,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const FIELDS: &str = "ProviderIds,Type,DateCreated,Name,OriginalTitle,PremiereDate,CommunityRating,Genres,Studios,Tags,TagItems,DateModified,OfficialRating,ProductionYear,Path,PrimaryImageAspectRatio,Overview,MediaStreams,Container,Size,SeriesId,ParentIndexNumber,IndexNumber,ParentId,RunTimeTicks,_SourceLibraryId";
fn key(item: &Value) -> Option<(String, String)> {
    let id = item["ProviderIds"]["Tmdb"].as_str()?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && id.bytes().any(|b| b != b'0'))
        .then(|| (id.to_string(), string(item, "Type")))
}
fn child_scope(item: &Value) -> Option<(String, i64)> {
    match item["Type"].as_str() {
        Some("Season") => Some((
            if string(item, "SeriesId").is_empty() {
                string(item, "ParentId")
            } else {
                string(item, "SeriesId")
            },
            item["IndexNumber"].as_i64()?,
        )),
        Some("Episode") => Some((
            string(item, "SeriesId"),
            item["ParentIndexNumber"].as_i64()?,
        )),
        _ => None,
    }
}
fn tags(item: &Value) -> Value {
    json!(
        list(&item["Tags"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .chain(
                list(&item["TagItems"])
                    .iter()
                    .filter_map(|t| t.as_str().or_else(|| t["Name"].as_str()))
                    .map(str::to_string)
            )
            .filter(|s| !s.is_empty())
            .collect::<BTreeSet<_>>()
    )
}
pub async fn cache(state: &AppState, full: bool, context: &JobContext) -> Result<Value> {
    cache_with_batch(state, full, 10, context).await
}
pub async fn cache_with_batch(
    state: &AppState,
    full: bool,
    batch_size: i64,
    context: &JobContext,
) -> Result<Value> {
    let config = state.config.read().await.clone();
    let libraries = emby::processing_libraries(state).await?;
    if libraries.is_empty() {
        return Ok(json!({"updated":0,"message":"没有需要处理的媒体库"}));
    }
    tracing::info!(
        "[同步媒体] 开始{}同步，扫描 {} 个处理媒体库",
        if full { "全量" } else { "增量" },
        libraries.len()
    );
    context
        .update(0, "阶段1/3：建立差异基准", "读取在线版本和作品绑定")
        .await;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m")
        .fetch_all(&state.db)
        .await?;
    let mut series = HashMap::new();
    let mut online = HashSet::new();
    let mut top_online = HashSet::new();
    let mut top_ids: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut work_titles = HashMap::new();
    for row in &rows {
        let ids = list(&row["emby_item_ids_json"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let work = (string(row, "tmdb_id"), string(row, "item_type"));
        if row["in_library"] == true {
            online.extend(ids.clone());
            if matches!(work.1.as_str(), "Movie" | "Series") {
                top_online.insert(work.clone());
            }
        }
        if matches!(work.1.as_str(), "Movie" | "Series") {
            work_titles.insert(work.clone(), string(row, "title"));
            top_ids.entry(work.clone()).or_default().extend(ids.clone());
        }
        if work.1 == "Series" {
            for id in ids {
                series.insert(id, work.0.clone());
            }
        }
    }
    let mut scanned = vec![];
    let mut parents = HashMap::new();
    let mut source_libraries = HashMap::new();
    context
        .update(10, "阶段2/3：扫描Emby", "正在扫描 Emby 并计算媒体差异")
        .await;
    for library in &libraries {
        context.checkpoint().await?;
        let items = emby::items(state, library, FIELDS, context).await?;
        tracing::info!(
            "[同步媒体] 媒体库 {library} 扫描完成，读取 {} 个项目",
            items.len()
        );
        for item in &items {
            let id = string(item, "Id");
            if !string(item, "ParentId").is_empty() {
                parents.insert(id.clone(), string(item, "ParentId"));
            }
            source_libraries.insert(id.clone(), library.clone());
            if let Some(work) = key(item)
                && matches!(work.1.as_str(), "Movie" | "Series")
            {
                if !string(item, "Name").is_empty() {
                    work_titles.insert(work.clone(), string(item, "Name"));
                }
                top_ids.entry(work.clone()).or_default().insert(id.clone());
                if work.1 == "Series" {
                    series.insert(id, work.0);
                }
            }
        }
        scanned.extend(items);
    }
    let observed = scanned
        .iter()
        .filter(|i| {
            matches!(
                i["Type"].as_str(),
                Some("Movie" | "Series" | "Season" | "Episode")
            )
        })
        .map(|i| string(i, "Id"))
        .collect::<HashSet<_>>();
    let mut fingerprint_ids = BTreeSet::new();
    if !string(&config, "p115_fingerprint_media_root_cid").is_empty()
        && config["monitor_sha1_pc_search"] != false
    {
        for mut snapshot in rows
            .iter()
            .filter(|r| {
                r["in_library"] == true
                    && matches!(r["item_type"].as_str(), Some("Movie" | "Episode"))
            })
            .cloned()
        {
            for (index, asset) in list(&snapshot["asset_details_json"]).iter().enumerate() {
                if emby::library_ignored(&config, &string(asset, "source_library_id")) {
                    continue;
                }
                let id = string(asset, "emby_item_id");
                let Some(item) = scanned.iter().find(|i| i["Id"] == id) else {
                    continue;
                };
                let (path, target, size) = crate::revalidation::observation(state, item).await;
                let changed = asset["fingerprint_pending"] == true
                    || (!path.is_empty()
                        && !string(asset, "path").is_empty()
                        && path != string(asset, "path"))
                    || (!target.is_empty()
                        && !string(asset, "fingerprint_target").is_empty()
                        && target != string(asset, "fingerprint_target"))
                    || matches!((size,crate::identity::size(&asset["size_bytes"])),(Some(a),Some(b)) if a!=b)
                    || crate::fingerprints::pickcode(&target).is_some_and(|pc| {
                        list(&snapshot["file_pickcode_json"])
                            .get(index)
                            .is_some_and(|old| {
                                old.as_str().is_some_and(|old| !old.is_empty() && pc != old)
                            })
                    });
                if changed {
                    crate::revalidation::mark_pending(state, &mut snapshot, index, None).await?;
                    fingerprint_ids.insert(id);
                }
            }
        }
    }
    let mut dirty = BTreeSet::new();
    let mut season_scope: HashMap<(String, String), BTreeSet<i64>> = HashMap::new();
    let mut all_seasons: HashMap<String, BTreeSet<i64>> = HashMap::new();
    for item in &scanned {
        if let Some((parent, number)) = child_scope(item) {
            all_seasons.entry(parent).or_default().insert(number);
        }
    }
    let ignored_ids = rows
        .iter()
        .flat_map(|row| list(&row["asset_details_json"]))
        .filter(|asset| emby::library_ignored(&config, &string(asset, "source_library_id")))
        .map(|asset| string(&asset, "emby_item_id"))
        .collect::<HashSet<_>>();
    let missing = online
        .difference(&observed)
        .filter(|id| !ignored_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    let mut offline = 0;
    if !missing.is_empty() {
        match emby::existing_ids(state, &missing, Some(context)).await {
            Ok(existing) => {
                let removed = missing
                    .iter()
                    .filter(|id| !existing.contains(*id))
                    .cloned()
                    .collect::<Vec<_>>();
                for row in &rows {
                    if list(&row["emby_item_ids_json"]).iter().any(|id| {
                        id.as_str()
                            .is_some_and(|id| removed.iter().any(|v| v == id))
                    }) && !string(row, "parent_series_tmdb_id").is_empty()
                    {
                        dirty.insert((string(row, "parent_series_tmdb_id"), "Series".into()));
                    }
                }
                offline = versions::unbind(state, &removed).await?;
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(_) => tracing::warn!("Emby 离线核对失败，保留未扫描到的绑定和指纹"),
        }
    }
    for item in &scanned {
        let id = string(item, "Id");
        if !full
            && item["Type"] == "Series"
            && let Some(work) = key(item)
            && !online.contains(&id)
            && top_online.contains(&work)
        {
            sqlx::query("UPDATE media_metadata SET emby_item_ids_json=CASE WHEN emby_item_ids_json @> $1 THEN emby_item_ids_json ELSE emby_item_ids_json || $1 END,last_synced_at=NOW() WHERE tmdb_id=$2 AND item_type='Series' AND in_library=true")
                .bind(json!([id])).bind(work.0).execute(&state.db).await?;
            continue;
        }
        let changed = rows
            .iter()
            .flat_map(|r| list(&r["asset_details_json"]))
            .any(|a| {
                a["emby_item_id"] == id
                    && !string(item, "Path").is_empty()
                    && a["path"] != item["Path"]
            });
        if !full && online.contains(&id) && !changed {
            continue;
        }
        if let Some(work) = key(item)
            && matches!(work.1.as_str(), "Movie" | "Series")
        {
            dirty.insert(work);
        } else if let Some((parent, number)) = child_scope(item)
            && let Some(tmdb) = series.get(&parent)
        {
            let work = (tmdb.clone(), "Series".into());
            dirty.insert(work.clone());
            season_scope.entry(work).or_default().insert(number);
        }
    }
    let defaults: Value =
        serde_json::from_str(include_str!("../assets/defaults.json")).expect("defaults");
    let mapping = crate::db::setting(&state.db, "language_mapping")
        .await?
        .unwrap_or(json!([]));
    let rating_mapping = crate::db::setting(&state.db, "rating_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_RATING_MAPPING"].clone());
    let rating_priority = crate::db::setting(&state.db, "rating_priority")
        .await?
        .unwrap_or(defaults["DEFAULT_RATING_PRIORITY"].clone());
    let virtual_folders = state
        .emby(Method::GET, "Library/VirtualFolders", &[], None)
        .await
        .unwrap_or(json!([]));
    let guids: HashMap<String, String> = list(&virtual_folders)
        .iter()
        .map(|l| (string(l, "ItemId"), string(l, "Guid")))
        .collect();
    let total = dirty.len();
    let mut updated = 0;
    let mut failures = vec![];
    if batch_size == 0 {
        return Err(crate::error::ApiError(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "分批大小不能为 0".into(),
        ));
    }
    let dirty = dirty.into_iter().collect::<Vec<_>>();
    let chunks = if batch_size < 0 {
        vec![]
    } else {
        dirty.chunks(batch_size as usize).collect::<Vec<_>>()
    };
    for (batch_index, batch) in chunks.into_iter().enumerate() {
        context.checkpoint().await?;
        let mut groups = vec![];
        for (offset, work) in batch.iter().enumerate() {
            context.checkpoint().await?;
            let index = batch_index * batch_size as usize + offset;
            let title = work_titles
                .get(work)
                .cloned()
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("未命名媒体（TMDb {}）", work.0));
            context
                .update(
                    20 + (index as u64 * 80 / total.max(1) as u64),
                    "阶段3/3：同步元数据",
                    &format!("正在同步《{title}》 · {}/{}", index + 1, total),
                )
                .await;
            let ids = top_ids.get(work).cloned().unwrap_or_default();
            let numbers = season_scope
                .get(work)
                .cloned()
                .unwrap_or_else(|| {
                    ids.iter()
                        .flat_map(|id| all_seasons.get(id).cloned().unwrap_or_default())
                        .collect()
                })
                .into_iter()
                .collect::<Vec<_>>();
            let mut tops = vec![];
            let mut children = vec![];
            let ids = ids.into_iter().collect::<Vec<_>>();
            for mut top in emby::by_ids(state, &ids, FIELDS, Some(context)).await? {
                let id = string(&top, "Id");
                if top["Type"] != work.1 {
                    continue;
                }
                if emby::path_ignored(state, &string(&top, "Path")).await? {
                    continue;
                }
                let library = source_libraries.get(&id).cloned().unwrap_or_default();
                top["_SourceLibraryId"] = json!(library);
                if work.1 == "Series" {
                    match emby::items(state, &id, FIELDS, context).await {
                        Ok(items) => {
                            for mut child in items {
                                if child_scope(&child).is_some_and(|(_, n)| numbers.contains(&n)) {
                                    child["_SourceLibraryId"] = json!(library);
                                    children.push(child);
                                }
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                "[同步媒体] 《{title}》读取分集失败（Item {id}）：{}",
                                error.1
                            );
                            failures.push(format!("{id}: {}", error.1));
                            continue;
                        }
                    }
                }
                tops.push(top);
            }
            if tops.is_empty() {
                tracing::info!("[同步媒体] 跳过《{title}》：未取得可处理的在库版本");
                continue;
            }
            if work.1 == "Series" && !numbers.is_empty() {
                tracing::info!("[同步按季刮削] 《{title}》本次仅聚合第 {:?} 季", numbers);
            }
            groups.push((work.clone(), tops, children, numbers, title));
        }
        use futures_util::{FutureExt, StreamExt, stream};
        let limit = crate::strm::thread_limit(state, "secondary").await;
        let pending = stream::iter(groups.clone()).map(|(work, _, _, numbers, title)| async move {
            context.checkpoint().await?;
        let data = if work.1 == "Movie" {
            tmdb::details(state, "Movie", &work.0).await
        } else if numbers.is_empty() {
            tmdb::details(state,"Series",&work.0).await.map(|series|json!({"series_details":series,"seasons_details":[],"episodes_details":{}}))
        } else {
            tmdb::aggregate(state, &work.0, &numbers).await
        };
        let data = data.unwrap_or_else(|error| {
            tracing::warn!(
                "[同步媒体] 《{title}》读取 TMDb 元数据失败，使用现有媒体信息：{}",
                error.1
            );
            json!({})
        });
            Ok::<_, crate::error::ApiError>((work.clone(), data))
        }.boxed()).buffer_unordered(limit);
        tokio::pin!(pending);
        let mut metadata = BTreeMap::new();
        while let Some(result) = pending.next().await {
            let (work, data) = result?;
            metadata.insert(work, data);
        }
        for (work, _, _, _, _) in &groups {
            context.checkpoint().await?;
            let data = metadata.get_mut(work).unwrap();
            if config["ai_translate_on_sync"] == true {
                let mut pruned = data.clone();
                crate::translation::prune(&work.1, &mut pruned);
                crate::translation::metadata_tree(state, &work.1, &mut pruned, "").await?;
                merge_translations(data, pruned);
            }
        }
        let mut batch_records = vec![];
        for (work, tops, children, _, title) in &groups {
            context.checkpoint().await?;
            let mut records = records(
                work,
                tops,
                children,
                &metadata[work],
                &parents,
                &guids,
                [&mapping, &rating_mapping, &rating_priority],
            )
            .await;
            tracing::info!("[同步媒体] 《{title}》准备写入 {} 个媒体项", records.len());
            batch_records.append(&mut records);
        }
        let active_children = batch_records
            .iter()
            .filter(|row| matches!(row["item_type"].as_str(), Some("Season" | "Episode")))
            .map(|row| string(row, "tmdb_id"))
            .collect::<Vec<_>>();
        if !batch_records.is_empty() {
            match metadata::upsert(state, &mut batch_records, true).await {
                Ok(()) => updated += batch_records.len(),
                Err(error) => {
                    tracing::warn!("[同步媒体] 本批写入失败：{}", error.1);
                    failures.push(error.1);
                }
            }
        }
        for (work, _, _, numbers, _) in &groups {
            if work.1 == "Series" && !numbers.is_empty() {
                offline += sqlx::query("UPDATE media_metadata SET in_library=false,asset_details_json=NULL,file_sha1_json='[]'::jsonb,file_pickcode_json='[]'::jsonb WHERE parent_series_tmdb_id=$1 AND season_number=ANY($2) AND item_type IN ('Season','Episode') AND in_library=true AND COALESCE(emby_item_ids_json,'[]'::jsonb)='[]'::jsonb AND NOT(tmdb_id=ANY($3))")
                    .bind(&work.0).bind(numbers).bind(&active_children).execute(&state.db).await?.rows_affected();
            }
        }
        let processed = ((batch_index + 1) * batch_size as usize).min(total);
        context
            .update(
                20 + processed as u64 * 80 / total.max(1) as u64,
                "阶段3/3：同步元数据",
                &format!("处理进度 {processed}/{total}..."),
            )
            .await;
    }
    sqlx::query("DELETE FROM media_metadata WHERE in_library=false AND tmdb_id LIKE '%-%'")
        .execute(&state.db)
        .await?;
    let fingerprints = if fingerprint_ids.is_empty() {
        Value::Null
    } else {
        crate::revalidation::ids(
            state,
            &fingerprint_ids.into_iter().collect::<Vec<_>>(),
            context,
        )
        .await?
    };
    let message = format!(
        "同步完成！新增/更新 {updated} 个媒体项，标记离线 {offline} 个，元数据失败 {} 项，指纹未完成 {} 项。",
        failures.len(),
        fingerprints["failed"].as_u64().unwrap_or(0)
    );
    tracing::info!("[同步媒体] {message}");
    Ok(
        json!({"updated":updated,"offline":offline,"failures":failures,"failed":fingerprints["failed"].as_u64().unwrap_or(0),"fingerprints":fingerprints,"message":message}),
    )
}
fn merge_translations(data: &mut Value, translated: Value) {
    match (data, translated) {
        (Value::Object(data), Value::Object(translated)) => {
            for (key, value) in translated {
                merge_translations(data.entry(key).or_insert(Value::Null), value);
            }
        }
        (Value::Array(data), Value::Array(translated)) => {
            for (data, value) in data.iter_mut().zip(translated) {
                merge_translations(data, value);
            }
        }
        (data, translated) => *data = translated,
    }
}
pub async fn records(
    work: &(String, String),
    tops: &[Value],
    children: &[Value],
    data: &Value,
    parents: &HashMap<String, String>,
    guids: &HashMap<String, String>,
    mappings: [&Value; 3],
) -> Vec<Value> {
    let [mapping, rating_mapping, rating_priority] = mappings;
    let item = &tops[0];
    let movie = work.1 == "Movie";
    let details = if movie { data } else { &data["series_details"] };
    let mut assets = vec![];
    for top in tops {
        let library = top["_SourceLibraryId"]
            .as_str()
            .unwrap_or("None")
            .to_string();
        let mut asset = assets::parse(
            top,
            parents,
            guids.get(&library).map(String::as_str).unwrap_or(""),
            mapping,
        )
        .await;
        asset["source_library_id"] = json!(library);
        assets.push(asset);
    }
    let date = if !string(item, "PremiereDate").is_empty() {
        item["PremiereDate"].clone()
    } else {
        details[if movie {
            "release_date"
        } else {
            "first_air_date"
        }]
        .clone()
    };
    let mut ratings =
        metadata_payload::sync_ratings(&work.1, details, rating_mapping, rating_priority);
    if ratings.as_object().is_some_and(|o| o.is_empty())
        && !string(item, "OfficialRating").is_empty()
    {
        ratings["US"] = item["OfficialRating"].clone();
    }
    let genres = if !list(&details["genres"]).is_empty() {
        list(&details["genres"])
    } else {
        list(&item["Genres"])
            .iter()
            .map(|g| json!({"id":0,"name":g}))
            .collect()
    };
    let genres = genres
        .iter()
        .map(|genre| {
            let name = genre.get("name").unwrap_or(genre);
            let name = match name.as_str() {
                Some("Sci-Fi & Fantasy") => json!("科幻奇幻"),
                Some("War & Politics") => json!("战争政治"),
                _ => name.clone(),
            };
            json!({"id":genre.get("id").cloned().unwrap_or(json!(0)),"name":name})
        })
        .collect::<Vec<_>>();
    let mut top = json!({"tmdb_id":work.0,"item_type":work.1,"release_year":item["ProductionYear"],"in_library":true,
        "emby_item_ids_json":tops.iter().map(|i|i["Id"].clone()).collect::<Vec<_>>(),"asset_details_json":assets,
        "imdb_id":item["ProviderIds"].get("Imdb").filter(|v|v.as_str().is_some_and(|s|!s.is_empty())).unwrap_or(&details["imdb_id"]),"original_language":details["original_language"],
        "rating":details.get("vote_average").filter(|v|!v.is_null()).unwrap_or(&item["CommunityRating"]),"date_added":item["DateCreated"],
        "release_date":date,"last_air_date":details["last_air_date"],"genres_json":genres,"tags_json":tags(item),"official_rating_json":ratings,
        "custom_rating":item["CustomRating"],"runtime_minutes":if movie{details["runtime"].clone()}else{Value::Null},"tagline":details["tagline"],
        "poster_path":details["poster_path"],"backdrop_path":details["backdrop_path"],"homepage":details["homepage"],"overview":details["overview"]});
    for (source, target) in [
        ("production_companies", "production_companies_json"),
        ("networks", "networks_json"),
    ] {
        top[target] = json!(
            list(&details[source])
                .iter()
                .filter(|i| !string(i, "name").is_empty())
                .map(|i| json!({"id":i["id"],"name":i["name"]}))
                .collect::<Vec<_>>()
        );
    }
    if details.as_object().is_none_or(|v| v.is_empty()) {
        top.as_object_mut().unwrap().remove("overview");
    }
    if !movie && details.as_object().is_some_and(|v| !v.is_empty()) {
        top["total_episodes"] = details
            .get("number_of_episodes")
            .cloned()
            .unwrap_or(json!(0));
    }
    let directors = if movie {
        let credits = details
            .get("credits")
            .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
            .unwrap_or(&details["casts"]);
        list(&credits["crew"])
            .into_iter()
            .filter(|d| d["job"] == "Director")
            .collect()
    } else {
        list(&details["created_by"])
    };
    top["directors_json"] = json!(
        directors
            .iter()
            .map(|d| json!({"id":d["id"],"name":d["name"]}))
            .collect::<Vec<_>>()
    );
    top["countries_json"] = if movie {
        json!(
            list(&details["production_countries"])
                .iter()
                .filter_map(|c| c["iso_3166_1"].as_str())
                .collect::<Vec<_>>()
        )
    } else {
        json!(list(&details["origin_country"]))
    };
    top["keywords_json"] = json!(
        list(&details["keywords"][if movie { "keywords" } else { "results" }])
            .iter()
            .filter(|i| !string(i, "name").is_empty())
            .map(|i| json!({"id":i["id"],"name":i["name"]}))
            .collect::<Vec<_>>()
    );
    for key in ["release_date", "last_air_date"] {
        if top[key] == "" {
            top[key] = Value::Null;
        }
    }
    let mut records = vec![top];
    if movie {
        return records;
    }
    let mut seasons: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
    let mut episodes: BTreeMap<(i64, i64), Vec<Value>> = BTreeMap::new();
    for child in children {
        if child["Type"] == "Season"
            && let Some(number) = child["IndexNumber"].as_i64()
        {
            seasons.entry(number).or_default().push(child.clone());
        }
        if child["Type"] == "Episode"
            && let (Some(season), Some(episode)) = (
                child["ParentIndexNumber"].as_i64(),
                child["IndexNumber"].as_i64(),
            )
        {
            episodes
                .entry((season, episode))
                .or_default()
                .push(child.clone());
        }
    }
    let mut known_seasons = HashSet::new();
    for (number, versions) in seasons {
        let info = list(&details["seasons"])
            .into_iter()
            .find(|s| s["season_number"].as_i64() == Some(number));
        let empty = json!({});
        let season = info.as_ref().unwrap_or(&empty);
        if info.is_some() {
            known_seasons.insert(number);
        }
        let date = season
            .get("air_date")
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            .or_else(|| {
                versions[0]
                    .get("PremiereDate")
                    .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            })
            .cloned()
            .or_else(|| {
                episodes
                    .iter()
                    .filter(|((s, _), _)| *s == number)
                    .flat_map(|(_, v)| v.iter())
                    .filter_map(|v| v["PremiereDate"].as_str().filter(|s| !s.is_empty()))
                    .min()
                    .map(|s| json!(s))
            })
            .unwrap_or(Value::Null);
        let mut record = json!({"tmdb_id":season["id"].as_u64().map(|n|n.to_string()).unwrap_or(format!("{}-S{number}",work.0)),
            "item_type":"Season","parent_series_tmdb_id":work.0,"season_number":number,"in_library":true,"release_date":date,
            "emby_item_ids_json":versions.iter().map(|v|v["Id"].clone()).collect::<Vec<_>>(),"tags_json":tags(&versions[0]),
            "imdb_id":versions[0]["ProviderIds"]["Imdb"],"title":season.get("name").unwrap_or(&versions[0]["Name"]),
            "overview":season["overview"],"poster_path":season.get("poster_path").filter(|v|v.as_str().is_some_and(|s|!s.is_empty())).unwrap_or(&details["poster_path"]),
            "rating":season["vote_average"]});
        if info.is_none() {
            record.as_object_mut().unwrap().remove("rating");
            record["title"] = versions[0]["Name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| json!(s))
                .unwrap_or(json!(format!("Season {number}")));
        }
        if info.is_some() {
            record["total_episodes"] = season.get("episode_count").cloned().unwrap_or(json!(0));
        }
        records.push(record);
    }
    for ((season, number), versions) in episodes {
        let info = if known_seasons.contains(&season) {
            &data["episodes_details"][format!("S{season}E{number}")]
        } else {
            &Value::Null
        };
        let first = &versions[0];
        let mut assets = vec![];
        for version in &versions {
            assets.push(assets::parse(version, &HashMap::new(), "", mapping).await);
        }
        let mut record = json!({"tmdb_id":info["id"].as_u64().filter(|n|*n>0).map(|n|n.to_string()).unwrap_or(format!("{}-S{season}E{number}",work.0)),
            "item_type":"Episode","parent_series_tmdb_id":work.0,"season_number":season,"episode_number":number,"in_library":true,
            "imdb_id":first["ProviderIds"]["Imdb"],"release_date":first.get("PremiereDate").filter(|v|v.as_str().is_some_and(|s|!s.is_empty())).unwrap_or(&info["air_date"]),
            "rating":info.get("vote_average").filter(|v|!v.is_null()).unwrap_or(&first["CommunityRating"]),
            "title":info.get("name").unwrap_or(&first["Name"]),"overview":info.get("overview").unwrap_or(&first["Overview"]),
            "runtime_minutes":info["runtime"],"poster_path":info["still_path"],"backdrop_path":info["still_path"],
            "emby_item_ids_json":versions.iter().map(|v|v["Id"].clone()).collect::<Vec<_>>(),"asset_details_json":assets,"tags_json":tags(first)});
        if record["release_date"] == "" {
            record["release_date"] = Value::Null;
        }
        records.push(record);
    }
    records
}
