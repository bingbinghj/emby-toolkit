use crate::{
    AppState, assets, emby,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
    metadata, metadata_payload,
    state::string,
    tmdb, versions,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(serde::Serialize, serde::Deserialize)]
struct SyncWork {
    work: (String, String),
    ids: Vec<String>,
    #[serde(default)]
    observed_ids: Vec<String>,
    #[serde(default)]
    child_ids: Vec<String>,
    numbers: Vec<i64>,
    title: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncPayload {
    records: Vec<Value>,
    identities: Vec<Value>,
    warning_details: Vec<String>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncPlan {
    libraries: Vec<String>,
    works: Vec<SyncWork>,
    removed: Vec<String>,
    quick_bindings: Vec<(String, String)>,
    fingerprint_ids: Vec<String>,
    pending_revisions: BTreeMap<String, u64>,
    warning_details: Vec<String>,
}
fn work_checkpoint(work: &(String, String)) -> String {
    format!("sync-work:{}:{}", work.1, work.0)
}
fn payload_key(work: &(String, String)) -> String {
    format!("sync-payload:{}:{}", work.1, work.0)
}
fn source_identity(item: &Value) -> Value {
    let mut identity = json!({"Tmdb":item["ProviderIds"]["Tmdb"]});
    for field in [
        "Id",
        "Type",
        "Path",
        "SeriesId",
        "ParentId",
        "ParentIndexNumber",
        "IndexNumber",
        "Size",
        "Container",
        "MediaStreams",
        "_SourceLibraryId",
    ] {
        identity[field] = item[field].clone();
    }
    identity
}
async fn committed_work(context: &JobContext, work: &(String, String)) -> Result<(usize, u64)> {
    let written = context
        .completed(&format!("sync-written:{}:{}", work.1, work.0))
        .await?
        .and_then(|result| result["updated"].as_u64())
        .unwrap_or(0) as usize;
    let offline = context
        .completed(&format!("sync-children:{}:{}", work.1, work.0))
        .await?
        .and_then(|result| result["offline"].as_u64())
        .unwrap_or(0);
    Ok((written, offline))
}

const FIELDS: &str = "ProviderIds,Type,DateCreated,Name,OriginalTitle,PremiereDate,Genres,Studios,Tags,TagItems,DateModified,OfficialRating,CustomRating,ProductionYear,Path,PrimaryImageAspectRatio,Overview,MediaStreams,Container,Size,SeriesId,ParentIndexNumber,IndexNumber,ParentId,RunTimeTicks,_SourceLibraryId";
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
    let saved_plan = context
        .plan("sync-cache")
        .await?
        .map(|plan| {
            serde_json::from_value::<SyncPlan>(plan)
                .map_err(|_| ApiError::bad("已保存的同步计划无效"))
        })
        .transpose()?;
    let resuming = saved_plan.is_some();
    let config = state.config.read().await.clone();
    let available_libraries = emby::processing_libraries(state).await?;
    let libraries = if let Some(plan) = &saved_plan {
        if plan
            .libraries
            .iter()
            .any(|id| !available_libraries.contains(id))
        {
            return Err(ApiError::bad(
                "同步计划中的处理媒体库已变化，请核对处理范围后重试",
            ));
        }
        plan.libraries.clone()
    } else {
        available_libraries
    };
    if libraries.is_empty() {
        return Ok(json!({"updated":0,"message":"没有需要处理的媒体库"}));
    }
    tracing::info!(
        "[同步媒体] 开始{}同步，扫描 {} 个处理媒体库",
        if full { "全量" } else { "增量" },
        libraries.len()
    );
    if resuming {
        context
            .activity("阶段1/3：建立差异基准", "重新核对在线版本和作品绑定")
            .await;
    } else {
        context
            .update(0, "阶段1/3：建立差异基准", "读取在线版本和作品绑定")
            .await;
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m")
        .fetch_all(&state.db)
        .await?;
    let mut series = HashMap::new();
    let mut online = HashSet::new();
    let mut top_online = HashSet::new();
    let mut top_ids: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut native_works: HashMap<String, BTreeSet<(String, String)>> = HashMap::new();
    let mut work_titles = HashMap::new();
    let mut asset_paths: HashMap<String, HashSet<String>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        if index % 128 == 0 {
            context.checkpoint().await?;
            tokio::task::yield_now().await;
        }
        let ids = list(&row["emby_item_ids_json"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let work = (string(row, "tmdb_id"), string(row, "item_type"));
        for asset in list(&row["asset_details_json"]) {
            if let Some(id) = asset["emby_item_id"].as_str() {
                asset_paths
                    .entry(id.to_string())
                    .or_default()
                    .insert(string(&asset, "path"));
            }
        }
        if row["in_library"] == true {
            online.extend(ids.clone());
            if matches!(work.1.as_str(), "Movie" | "Series") {
                top_online.insert(work.clone());
            }
        }
        if matches!(work.1.as_str(), "Movie" | "Series") {
            for id in &ids {
                native_works
                    .entry(id.clone())
                    .or_default()
                    .insert(work.clone());
            }
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
    if resuming {
        context
            .activity("阶段2/3：扫描Emby", "重新核对 Emby，保留已完成的同步进度")
            .await;
    } else {
        context
            .update(10, "阶段2/3：扫描Emby", "正在扫描 Emby 并计算媒体差异")
            .await;
    }
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
    if resuming {
        context
            .activity(
                "阶段2/3：计算媒体差异",
                "Emby 核对完成，继续已保存的同步计划",
            )
            .await;
    } else {
        context
            .update(
                20,
                "阶段2/3：计算媒体差异",
                "Emby 扫描完成，核对版本和路径变更",
            )
            .await;
    }
    let mut scanned_by_id = HashMap::new();
    for (index, item) in scanned.iter().enumerate() {
        if index % 128 == 0 {
            context.checkpoint().await?;
            tokio::task::yield_now().await;
        }
        if let Some(id) = item["Id"].as_str() {
            if let Some(previous) = scanned_by_id.insert(id, item) {
                let mut previous = (*previous).clone();
                let mut current = item.clone();
                for value in [&mut previous, &mut current] {
                    if let Some(object) = value.as_object_mut() {
                        object.remove("_SourceLibraryId");
                    }
                }
                if previous != current {
                    return Err(ApiError::bad(format!(
                        "Emby 在不同媒体库返回原生 ID {id} 的不同内容，已停止同步并保留缓存"
                    )));
                }
            }
        }
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
    let mut pending_revisions = BTreeMap::new();
    if !string(&config, "p115_fingerprint_media_root_cid").is_empty()
        && config["monitor_sha1_pc_search"] != false
    {
        for snapshot in rows
            .iter()
            .filter(|r| {
                r["in_library"] == true
                    && matches!(r["item_type"].as_str(), Some("Movie" | "Episode"))
            })
            .cloned()
        {
            context.checkpoint().await?;
            for (index, asset) in list(&snapshot["asset_details_json"]).iter().enumerate() {
                if index % 128 == 0 {
                    context.checkpoint().await?;
                    tokio::task::yield_now().await;
                }
                if emby::library_ignored(&config, &string(asset, "source_library_id")) {
                    continue;
                }
                let id = string(asset, "emby_item_id");
                let Some(&item) = scanned_by_id.get(id.as_str()) else {
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
                    pending_revisions.insert(
                        format!(
                            "{}:{}:{id}",
                            string(&snapshot, "item_type"),
                            string(&snapshot, "tmdb_id")
                        ),
                        asset["fingerprint_revision"].as_u64().unwrap_or(0),
                    );
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
    let mut removed = vec![];
    let mut quick_bindings = vec![];
    let mut warning_details = vec![];
    if saved_plan.is_none() && !missing.is_empty() {
        match emby::existing_ids(state, &missing, Some(context)).await {
            Ok(existing) => {
                removed = missing
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
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                let warning = format!(
                    "Emby 离线核对失败，保留 {} 个未扫描到的绑定和指纹：{}",
                    missing.len(),
                    error.1
                );
                tracing::warn!("[同步媒体] {warning}");
                warning_details.push(warning);
            }
        }
    }
    for (index, item) in scanned.iter().enumerate() {
        if index % 128 == 0 {
            context.checkpoint().await?;
            tokio::task::yield_now().await;
        }
        let id = string(item, "Id");
        if !full
            && item["Type"] == "Series"
            && let Some(work) = key(item)
            && !online.contains(&id)
            && top_online.contains(&work)
        {
            quick_bindings.push((id, work.0));
            continue;
        }
        let path = item["Path"].as_str().unwrap_or("");
        let changed = !path.is_empty()
            && asset_paths
                .get(&id)
                .is_some_and(|paths| paths.len() > 1 || !paths.contains(path));
        let provider_changed = key(item).is_some_and(|work| {
            native_works
                .get(&id)
                .is_some_and(|owners| owners.len() != 1 || !owners.contains(&work))
        });
        if !full && online.contains(&id) && !changed && !provider_changed {
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
    if batch_size == 0 {
        return Err(crate::error::ApiError(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "分批大小不能为 0".into(),
        ));
    }
    let mut dirty = dirty.into_iter().collect::<Vec<_>>();
    dirty.sort_by_cached_key(|work| {
        (
            work_titles
                .get(work)
                .map(|name| name.to_lowercase())
                .unwrap_or_default(),
            work.clone(),
        )
    });
    let plan = if let Some(plan) = saved_plan {
        plan
    } else {
        let works = dirty
            .into_iter()
            .map(|work| {
                let ids = top_ids.get(&work).cloned().unwrap_or_default();
                let numbers = season_scope.get(&work).cloned().unwrap_or_else(|| {
                    ids.iter()
                        .flat_map(|id| all_seasons.get(id).cloned().unwrap_or_default())
                        .collect()
                });
                let title = work_titles
                    .get(&work)
                    .cloned()
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| format!("未命名媒体（TMDb {}）", work.0));
                let observed_ids = ids
                    .iter()
                    .filter(|id| {
                        scanned_by_id
                            .get(id.as_str())
                            .is_some_and(|item| key(item) == Some(work.clone()))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let child_ids = if work.1 == "Series" {
                    scanned
                        .iter()
                        .filter(|item| {
                            child_scope(item).is_some_and(|(parent, number)| {
                                observed_ids.contains(&parent) && numbers.contains(&number)
                            })
                        })
                        .map(|item| string(item, "Id"))
                        .collect()
                } else {
                    vec![]
                };
                SyncWork {
                    work,
                    ids: ids.into_iter().collect(),
                    observed_ids,
                    child_ids,
                    numbers: numbers.into_iter().collect(),
                    title,
                }
            })
            .collect();
        let plan = SyncPlan {
            libraries,
            works,
            removed,
            quick_bindings,
            fingerprint_ids: fingerprint_ids.into_iter().collect(),
            pending_revisions,
            warning_details,
        };
        context
            .save_plan(
                "sync-cache",
                &serde_json::to_value(&plan).map_err(|_| ApiError::bad("无法保存同步计划"))?,
            )
            .await?;
        plan
    };
    // The original plan must survive every binding or fingerprint mutation below.
    warning_details = plan.warning_details.clone();
    let fingerprint_result = context.completed("sync-fingerprints").await?;
    if fingerprint_result.is_none() {
        for mut snapshot in rows.iter().filter(|row| row["in_library"] == true).cloned() {
            for (index, asset) in list(&snapshot["asset_details_json"]).iter().enumerate() {
                let mark = format!(
                    "{}:{}:{}",
                    string(&snapshot, "item_type"),
                    string(&snapshot, "tmdb_id"),
                    string(asset, "emby_item_id")
                );
                let Some(revision) = plan.pending_revisions.get(&mark) else {
                    continue;
                };
                // A higher durable revision proves this mark (or its validation) already committed.
                if asset["fingerprint_revision"].as_u64().unwrap_or(0) > *revision {
                    continue;
                }
                context.checkpoint().await?;
                crate::revalidation::mark_pending(state, &mut snapshot, index, None).await?;
            }
        }
    }
    let mut offline = if let Some(result) = context.completed("sync-unbind").await? {
        warning_details.extend(
            list(&result["warning_details"])
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string)),
        );
        result["offline"].as_u64().unwrap_or(0)
    } else {
        context.checkpoint().await?;
        let candidates = plan
            .removed
            .iter()
            .filter(|id| !observed.contains(*id) && !ignored_ids.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        match emby::existing_ids(state, &candidates, Some(context)).await {
            Ok(existing) => {
                context.checkpoint().await?;
                versions::unbind_with_checkpoint(
                    state,
                    &candidates
                        .into_iter()
                        .filter(|id| !existing.contains(id))
                        .collect::<Vec<_>>(),
                    Some(context),
                )
                .await?
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                let warning = format!(
                    "Emby 离线核对失败，保留 {} 个未扫描到的绑定和指纹：{}",
                    candidates.len(),
                    error.1
                );
                warning_details.push(warning.clone());
                context
                    .complete(
                        "sync-unbind",
                        &json!({"offline":0,"warning_details":[warning]}),
                    )
                    .await?;
                0
            }
        }
    };
    for (id, tmdb) in &plan.quick_bindings {
        let checkpoint = format!("sync-binding:{id}");
        if context.completed(&checkpoint).await?.is_some() {
            continue;
        }
        context.checkpoint().await?;
        if scanned_by_id
            .get(id.as_str())
            .is_none_or(|item| key(item) != Some((tmdb.clone(), "Series".into())))
        {
            continue;
        }
        let mut transaction = state.db.begin().await?;
        sqlx::query("UPDATE media_metadata SET emby_item_ids_json=CASE WHEN emby_item_ids_json @> $1 THEN emby_item_ids_json ELSE emby_item_ids_json || $1 END,last_synced_at=NOW() WHERE tmdb_id=$2 AND item_type='Series' AND in_library=true")
            .bind(json!([id])).bind(tmdb).execute(&mut *transaction).await?;
        context
            .complete_in_tx(&mut transaction, &checkpoint, &json!({"bound":true}))
            .await?;
        transaction.commit().await?;
    }
    let total = plan.works.len();
    let mut completed_works = BTreeMap::new();
    if resuming {
        for planned in &plan.works {
            context.checkpoint().await?;
            if let Some(result) = context.completed(&work_checkpoint(&planned.work)).await? {
                completed_works.insert(planned.work.clone(), result);
            }
        }
    }
    let completed_base = completed_works.len();
    let mut updated = 0;
    let mut failures = vec![];
    let mut work_warnings = BTreeMap::new();
    let chunks = if batch_size < 0 {
        vec![]
    } else {
        plan.works.chunks(batch_size as usize).collect::<Vec<_>>()
    };
    for (batch_index, batch) in chunks.into_iter().enumerate() {
        context.checkpoint().await?;
        let mut groups = vec![];
        let mut payloads = BTreeMap::new();
        let mut failed_scans = BTreeSet::new();
        for (offset, planned) in batch.iter().enumerate() {
            context.checkpoint().await?;
            let work = &planned.work;
            if let Some(result) = completed_works.get(work) {
                updated += result["updated"].as_u64().unwrap_or(0) as usize;
                offline += result["offline"].as_u64().unwrap_or(0);
                warning_details.extend(
                    list(&result["warning_details"])
                        .into_iter()
                        .filter_map(|v| v.as_str().map(str::to_string)),
                );
                continue;
            }
            let index = batch_index * batch_size as usize + offset;
            let title = planned.title.clone();
            context
                .update(
                    20 + (index.max(completed_base) as u64 * 80 / total.max(1) as u64),
                    "阶段3/3：同步元数据",
                    &format!("正在同步《{title}》 · {}/{}", index + 1, total),
                )
                .await;
            let numbers = planned.numbers.clone();
            let mut tops = vec![];
            let mut children = vec![];
            let ids = planned.ids.clone();
            let warning_start = warning_details.len();
            let details = emby::by_ids(state, &ids, FIELDS, Some(context)).await?;
            let drift = details.iter().filter(|item| planned.observed_ids.contains(&string(item, "Id")))
                .filter(|item| !observed.contains(&string(item, "Id")) || key(item) != Some(work.clone()))
                .map(|item| format!("《{title}》原生 ID {} 的 TMDb 绑定或原媒体库范围已变化，已保留缓存并停止该作品同步", string(item, "Id")))
                .collect::<Vec<_>>();
            if !drift.is_empty() {
                failures.extend(drift);
                let (written, child_offline) = committed_work(context, work).await?;
                updated += written;
                offline += child_offline;
                continue;
            }
            let fetched = details
                .iter()
                .filter(|item| {
                    item["Type"] == work.1
                        && observed.contains(&string(item, "Id"))
                        && key(item).is_none_or(|actual| actual == *work)
                })
                .map(|item| string(item, "Id"))
                .collect::<HashSet<_>>();
            let missing = planned
                .observed_ids
                .iter()
                .filter(|id| !fetched.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                let reason = format!(
                    "《{title}》（TMDb {}）本次已扫描到的媒体详情回读不完整，原生 ID {} 未取得匹配详情，已保留现有记录并跳过这些条目",
                    work.0,
                    missing.join("、")
                );
                tracing::warn!("[同步媒体] {reason}");
                warning_details.push(reason);
            }
            if let Some(saved) = context.plan(&payload_key(work)).await? {
                let payload: SyncPayload = serde_json::from_value(saved)
                    .map_err(|_| ApiError::bad("已解析的同步媒体记录无效"))?;
                let changed = payload.identities.iter().any(|identity| {
                    scanned_by_id
                        .get(string(identity, "Id").as_str())
                        .is_none_or(|item| source_identity(item) != *identity)
                });
                if changed {
                    failures.push(format!("《{title}》原计划中的媒体、路径或分集身份已变化，已保留已完成缓存并停止该作品同步"));
                    let (written, child_offline) = committed_work(context, work).await?;
                    updated += written;
                    offline += child_offline;
                    continue;
                }
                warning_details.extend(payload.warning_details.clone());
                work_warnings.insert(work.clone(), payload.warning_details.clone());
                payloads.insert(work.clone(), payload);
                groups.push((work.clone(), vec![], vec![], planned.numbers.clone(), title));
                continue;
            }
            for mut top in details {
                let id = string(&top, "Id");
                if !ids.contains(&id)
                    || !observed.contains(&id)
                    || top["Type"] != work.1
                    || key(&top).is_some_and(|actual| actual != *work)
                {
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
                                if planned.child_ids.contains(&string(&child, "Id"))
                                    && child_scope(&child)
                                        .is_some_and(|(_, n)| numbers.contains(&n))
                                {
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
                            failed_scans.insert(work.clone());
                            continue;
                        }
                    }
                }
                tops.push(top);
            }
            let warnings = warning_details[warning_start..].to_vec();
            work_warnings.insert(work.clone(), warnings.clone());
            let missing_children = planned
                .child_ids
                .iter()
                .filter(|id| !children.iter().any(|child| child["Id"] == id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !missing_children.is_empty() {
                failures.push(format!(
                    "《{title}》原计划分集详情不完整，已保留缓存：{}",
                    missing_children.join("、")
                ));
                failed_scans.insert(work.clone());
            }
            if tops.is_empty() || failed_scans.contains(work) {
                tracing::info!("[同步媒体] 跳过《{title}》：未取得可处理的在库版本");
                let (written, child_offline) = committed_work(context, work).await?;
                updated += written;
                offline += child_offline;
                if !failed_scans.contains(work) {
                    context
                        .complete(
                            &work_checkpoint(work),
                            &json!({"updated":written,"offline":child_offline,"warning_details":warnings}),
                        )
                        .await?;
                }
                continue;
            }
            if work.1 == "Series" && !numbers.is_empty() {
                tracing::info!("[同步按季刮削] 《{title}》本次仅聚合第 {:?} 季", numbers);
            }
            groups.push((work.clone(), tops, children, numbers, title));
        }
        use futures_util::{FutureExt, StreamExt, stream};
        let limit = crate::strm::thread_limit(state, "secondary").await;
        let pending_groups = groups
            .iter()
            .filter(|(work, _, _, _, _)| !payloads.contains_key(work))
            .cloned()
            .collect::<Vec<_>>();
        let pending = stream::iter(pending_groups).map(|(work, _, _, numbers, _)| async move {
            context.checkpoint().await?;
        let data = if work.1 == "Movie" {
            tmdb::details(state, "Movie", &work.0).await
        } else if numbers.is_empty() {
            tmdb::details(state,"Series",&work.0).await.map(|series|json!({"series_details":series,"seasons_details":[],"episodes_details":{}}))
        } else {
            tmdb::aggregate(state, &work.0, &numbers).await
        };
            Ok::<_, crate::error::ApiError>((work.clone(), data))
        }.boxed()).buffer_unordered(limit);
        tokio::pin!(pending);
        let mut metadata = BTreeMap::new();
        let mut metadata_errors = BTreeMap::new();
        while let Some(result) = pending.next().await {
            let (work, data) = result?;
            let data = data.unwrap_or_else(|error| {
                metadata_errors.insert(work.clone(), error.1);
                json!({})
            });
            metadata.insert(work, data);
        }
        for (work, _, _, _, title) in &groups {
            if payloads.contains_key(work) {
                continue;
            }
            context.checkpoint().await?;
            let warning_start = warning_details.len();
            if let Some(error) = metadata_errors.remove(work) {
                let reason = format!(
                    "《{title}》（类型 {}，TMDb {}）读取元数据失败，使用现有媒体信息：{}",
                    work.1,
                    work.0,
                    crate::logs::redact(&error)
                );
                tracing::warn!("[同步媒体] {reason}");
                warning_details.push(reason);
            }
            let data = metadata.get_mut(work).unwrap();
            if config["ai_translate_on_sync"] == true {
                let mut pruned = data.clone();
                crate::translation::prune(&work.1, &mut pruned);
                let report =
                    crate::translation::metadata_tree_report(state, &work.1, &mut pruned, "")
                        .await?;
                warning_details.extend(list(&report["warning_details"]).into_iter().filter_map(
                    |reason| {
                        reason
                            .as_str()
                            .map(|reason| format!("《{title}》（TMDb {}）{reason}", work.0))
                    },
                ));
                merge_translations(data, pruned);
            }
            work_warnings
                .entry(work.clone())
                .or_default()
                .extend_from_slice(&warning_details[warning_start..]);
        }
        let mut batch_records = vec![];
        for (work, tops, children, _, title) in &groups {
            context.checkpoint().await?;
            if !payloads.contains_key(work) {
                let parsed = records(
                    work,
                    tops,
                    children,
                    &metadata[work],
                    &parents,
                    &guids,
                    [&mapping, &rating_mapping, &rating_priority],
                )
                .await;
                let payload = SyncPayload {
                    records: metadata::prepare_sync_records(state, &parsed).await?,
                    identities: tops
                        .iter()
                        .chain(children.iter())
                        .map(source_identity)
                        .collect(),
                    warning_details: work_warnings.get(work).cloned().unwrap_or_default(),
                };
                context
                    .save_plan(
                        &payload_key(work),
                        &serde_json::to_value(&payload)
                            .map_err(|_| ApiError::bad("无法保存已解析的同步媒体记录"))?,
                    )
                    .await?;
                payloads.insert(work.clone(), payload);
            }
            let mut records = payloads[work].records.clone();
            tracing::info!("[同步媒体] 《{title}》准备写入 {} 个媒体项", records.len());
            batch_records.append(&mut records);
        }
        let active_children = batch_records
            .iter()
            .filter(|row| matches!(row["item_type"].as_str(), Some("Season" | "Episode")))
            .map(|row| string(row, "tmdb_id"))
            .collect::<Vec<_>>();
        let mut written_works = BTreeMap::new();
        let mut failed_works = failed_scans;
        if !batch_records.is_empty() {
            match metadata::sync_upsert_with_context(state, &mut batch_records, context, resuming)
                .await
            {
                Ok(report) => {
                    updated += report.written;
                    failures.extend(report.failures);
                    written_works = report.written_works;
                    failed_works.extend(report.failed_works);
                }
                Err(error) => {
                    tracing::warn!("[同步媒体] 本批写入失败：{}", error.1);
                    failures.push(error.1);
                    for (work, _, _, _, _) in &groups {
                        updated += context
                            .completed(&format!("sync-written:{}:{}", work.1, work.0))
                            .await?
                            .and_then(|result| result["updated"].as_u64())
                            .unwrap_or(0) as usize;
                    }
                    failed_works.extend(groups.iter().map(|(work, _, _, _, _)| work.clone()));
                }
            }
        }
        for (work, _, _, numbers, _) in &groups {
            let cleanup_key = format!("sync-children:{}:{}", work.1, work.0);
            let child_offline = if work.1 != "Series" || numbers.is_empty() {
                0
            } else if let Some(result) = context.completed(&cleanup_key).await? {
                result["offline"].as_u64().unwrap_or(0)
            } else {
                let mut transaction = state.db.begin().await?;
                let count = sqlx::query("UPDATE media_metadata SET in_library=false,asset_details_json=NULL,file_sha1_json='[]'::jsonb,file_pickcode_json='[]'::jsonb WHERE parent_series_tmdb_id=$1 AND season_number=ANY($2) AND item_type IN ('Season','Episode') AND in_library=true AND COALESCE(emby_item_ids_json,'[]'::jsonb)='[]'::jsonb AND NOT(tmdb_id=ANY($3))")
                    .bind(&work.0).bind(numbers).bind(&active_children).execute(&mut *transaction).await?.rows_affected();
                context
                    .complete_in_tx(&mut transaction, &cleanup_key, &json!({"offline":count}))
                    .await?;
                transaction.commit().await?;
                count
            };
            offline += child_offline;
            if !failed_works.contains(work) {
                context.complete(&work_checkpoint(work), &json!({"updated":written_works.get(work).copied().unwrap_or(0),"offline":child_offline,"warning_details":work_warnings.get(work).cloned().unwrap_or_default()})).await?;
            }
        }
        let processed = ((batch_index + 1) * batch_size as usize)
            .min(total)
            .max(completed_base);
        context
            .update(
                20 + processed as u64 * 80 / total.max(1) as u64,
                "阶段3/3：同步元数据",
                &format!("处理进度 {processed}/{total}..."),
            )
            .await;
    }
    context.checkpoint().await?;
    if context.completed("sync-cleanup").await?.is_none() {
        let mut transaction = state.db.begin().await?;
        sqlx::query("DELETE FROM media_metadata WHERE in_library=false AND tmdb_id LIKE '%-%'")
            .execute(&mut *transaction)
            .await?;
        context
            .complete_in_tx(&mut transaction, "sync-cleanup", &json!({"complete":true}))
            .await?;
        transaction.commit().await?;
    }
    let fingerprints = if let Some(result) = fingerprint_result {
        result
    } else if plan.fingerprint_ids.is_empty() {
        Value::Null
    } else {
        let result = crate::revalidation::ids(state, &plan.fingerprint_ids, context).await?;
        context.complete("sync-fingerprints", &result).await?;
        result
    };
    let mut message = format!(
        "同步完成！新增/更新 {updated} 个媒体项，标记离线 {offline} 个，元数据处理失败 {} 项，指纹未完成 {} 项，处理警告 {} 项。",
        failures.len(),
        fingerprints["failed"].as_u64().unwrap_or(0),
        warning_details.len()
    );
    if let Some(reason) = warning_details.first() {
        message.push_str(&format!(
            "提醒：{}；完整明细见任务结果",
            crate::logs::redact(reason)
        ));
    }
    tracing::info!("[同步媒体] {message}");
    Ok(
        json!({"updated":updated,"offline":offline,"failures":failures,"failed":fingerprints["failed"].as_u64().unwrap_or(0),"fingerprints":fingerprints,"message":message,"warnings":warning_details.len(),"warning_details":warning_details}),
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
        "date_added":item["DateCreated"],
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
    if details.as_object().is_none_or(|v| v.is_empty()) {
        // A failed TMDb read cannot confirm empty collections or country ratings.
        for field in [
            "overview",
            "production_companies_json",
            "networks_json",
            "directors_json",
            "countries_json",
            "keywords_json",
            "official_rating_json",
        ] {
            top.as_object_mut().unwrap().remove(field);
        }
    }
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
            "overview":season["overview"],"poster_path":season.get("poster_path").filter(|v|v.as_str().is_some_and(|s|!s.is_empty())).unwrap_or(&details["poster_path"])});
        if info.is_none() {
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
