use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::{JobContext, JobSpec},
    media_edit::details,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/actions/reprocess_item/{id}", post(submit))
        .route("/api/actions/reprocess_all_review_items", post(all))
        .route("/api/actions/reorganize", post(reorganize))
}
async fn reorganize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let operator = state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?
        .user_id;
    let selected = body["item_ids"]
        .as_array()
        .filter(|ids| !ids.is_empty() && ids.len() <= 1000)
        .ok_or_else(|| ApiError::bad("请选择 1 至 1000 个明确的媒体项目"))?;
    let mut ids = vec![];
    for value in selected {
        let id = value
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            })
            .ok_or_else(|| ApiError::bad("媒体项目 ID 无效"))?;
        if !ids.iter().any(|selected: &String| selected == id) {
            ids.push(id.to_string());
        }
    }
    let mode = crate::media_organize_queue::mode(body.get("mode"))?;
    let history_ids = crate::media_organize_history::selected_history(&state, &body, &ids).await?;
    let queued =
        crate::media_organize_queue::enqueue(&state, &ids, &history_ids, mode, &operator).await?;
    Ok((StatusCode::ACCEPTED, Json(queued)))
}

pub(crate) async fn run_organize_item(
    state: &AppState,
    id: &str,
    mode: &str,
    context: &JobContext,
) -> Result<Value> {
    match mode {
        "reprocess" => Box::pin(reorganize_ingest(state, id, context)).await,
        "rescrape" => Box::pin(crate::media_rescrape::run(state, id, context)).await,
        _ => Err(ApiError::bad("整理模式无效")),
    }
}

async fn reorganize_ingest(state: &AppState, id: &str, context: &JobContext) -> Result<Value> {
    let requested = details(state, id, "Id,Type,Path,ProviderIds,SeriesId,ParentId").await?;
    let ids = crate::revalidation::scope(state, id, context).await?;
    let mut files = vec![];
    for id in ids {
        context.checkpoint().await?;
        let item = details(state, &id, "Id,Type,Path").await?;
        files.push(json!(string(&item, "Path")));
    }
    let item = series_parent(state, requested).await?;
    let event = json!({"source":"moviepilot","files":files,
        "tmdb_id":item["ProviderIds"]["Tmdb"],"episode_group":null,
        "media_type":if item["Type"]=="Series" {"series"} else {"movie"}});
    let mut report = crate::webhook::process_reorganize(state, &event, context).await?;
    report["finalization"] = report["finalizations"][0]["report"].clone();
    if crate::media_edit::list(&report["failures"]).is_empty() {
        let warnings = report["warnings"].as_u64().unwrap_or(0);
        report["message"] = json!(if warnings == 0 { "重新整理入库流程已完成".to_string() }
            else { format!("重新整理入库流程已完成，有 {warnings} 条提醒") });
    }
    Ok(report)
}

async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let operator = state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?
        .user_id;
    let payload = json!({"operation":"reprocess_item","id":id,"reason":string(&body,"reason")});
    let spec = JobSpec::new("actions", payload.clone()).operator(operator.clone());
    let work = state.clone();
    let task = state
        .jobs
        .submit_persistent("重新处理项目", spec, move |context| async move {
            crate::notifications::with_operator(operator, async move {
                Ok(execute_persisted(&work, &payload, &context).await?.1)
            })
            .await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"重新处理项目任务已提交"})),
    ))
}
async fn all(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let operator = state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?
        .user_id;
    let kind = string(&body, "media_type");
    if !["", "Movie", "Series"].contains(&kind.as_str()) {
        return Err(ApiError::bad("媒体类型仅支持电视剧或电影"));
    }
    let payload =
        json!({"operation":"review_items","media_type":kind,"reason":string(&body,"reason")});
    let spec = JobSpec::new("actions", payload.clone()).operator(operator.clone());
    let work = state.clone();
    let task = state
        .jobs
        .submit_persistent("重新处理待复核项", spec, move |context| async move {
            crate::notifications::with_operator(operator, async move {
                Ok(execute_persisted(&work, &payload, &context).await?.1)
            })
            .await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"批量重新处理任务已提交"})),
    ))
}

pub(crate) async fn execute_persisted(
    state: &AppState,
    payload: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    if payload["operation"] == "reprocess_item" {
        if let Some(report) = context.completed("item").await? {
            return Ok((200, report));
        }
        let item_context = context.scoped("item");
        let report = reprocess(
            state,
            &string(payload, "id"),
            &string(payload, "reason"),
            &item_context,
        )
        .await?;
        context.complete("item", &report).await?;
        return Ok((200, report));
    }
    if payload["operation"] != "review_items" {
        return Err(ApiError::bad("未知重新处理任务"));
    }
    let rows = if let Some(rows) = context.plan("review_rows").await? {
        rows
    } else {
        let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(f) FROM failed_log f WHERE ($1='' OR reason LIKE $1||'%') AND ($2='' OR item_type=$2 OR ($2='Series' AND item_type IN ('Season','Episode'))) ORDER BY item_id")
            .bind(string(payload,"reason")).bind(string(payload,"media_type")).fetch_all(&state.db).await?;
        let rows = json!(rows);
        context.save_plan("review_rows", &rows).await?;
        rows
    };
    let mut updated = 0;
    let mut failures = vec![];
    let mut warnings = 0_u64;
    let mut warning_details = vec![];
    let rows = crate::media_edit::list(&rows);
    for (index, row) in rows.iter().enumerate() {
        context.checkpoint().await?;
        let id = string(row, "item_id");
        let key = format!("review:{index}:{id}");
        context
            .update_counted(
                index as u64,
                rows.len() as u64,
                "重新处理待复核项",
                &format!("重新处理媒体 {id}"),
            )
            .await;
        let report = if let Some(report) = context.completed(&key).await? {
            report
        } else {
            let item_context = context.scoped(&key);
            let report = match reprocess(state, &id, &string(row, "reason"), &item_context).await {
                Ok(report) => report,
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => json!({"error":crate::logs::redact(&error.1)}),
            };
            context.complete(&key, &report).await?;
            report
        };
        if let Some(error) = report["error"].as_str() {
            failures.push(format!("{id}: {error}"));
        } else {
            updated += 1;
            warnings += report["warnings"].as_u64().unwrap_or(0);
            warning_details.extend(crate::media_edit::list(&report["warning_details"]));
        }
    }
    Ok((
        200,
        json!({"updated":updated,"failures":failures,"warnings":warnings,"warning_details":warning_details,
        "message":format!("待复核项目重新处理完成：处理 {updated} 项，失败 {} 项，提醒 {warnings} 条",failures.len())}),
    ))
}

pub async fn reprocess(
    state: &AppState,
    id: &str,
    reason: &str,
    context: &JobContext,
) -> Result<Value> {
    Box::pin(reprocess_inner(state, id, reason, context)).await
}
async fn reprocess_inner(
    state: &AppState,
    id: &str,
    reason: &str,
    context: &JobContext,
) -> Result<Value> {
    context.checkpoint().await?;
    let requested = details(
        state,
        id,
        "Id,Name,Type,Path,People,ProviderIds,Genres,MediaSources,SeriesId,ParentId",
    )
    .await?;
    if requested["Id"] != id {
        return Err(ApiError::bad(
            "Emby 返回的媒体 ID 与整理目标不一致，未继续处理",
        ));
    }
    let missing = reason.contains("缺失媒体信息");
    let mut ids = crate::revalidation::scope(state, id, context).await?;
    if matches!(requested["Type"].as_str(), Some("Series" | "Season")) && ids.is_empty() {
        return Err(ApiError::bad("无法确定在库季集范围，禁止全季处理"));
    }
    if requested["Type"] == "Movie" {
        let target = crate::media_edit::target(state, id).await?;
        for version in target.versions {
            let version = string(&version, "emby_id");
            if !version.is_empty() && !ids.contains(&version) {
                ids.push(version);
            }
        }
    }
    let mut rebuilt = std::collections::BTreeMap::new();
    for asset in &ids {
            context.checkpoint().await?;
            let target = crate::media_edit::target(state, asset).await?;
            let final_path = sidecar(state, &target).await?;
            if missing
                && let Ok(bytes) = tokio::fs::read(&final_path).await
                && let Ok(info) = serde_json::from_slice::<Value>(&bytes)
                && !has_strm_container(&info)
            {
                continue;
            }
            let (pc, sha, _) = crate::fingerprints::lookup(state, &target.path).await?;
            let pc = pc.ok_or_else(|| ApiError::bad("缺失115 PC"))?;
            let sha = sha
                .or(target.sha.clone())
                .ok_or_else(|| ApiError::bad("缺失SHA1"))?;
            let source = crate::media_paths::strm_target(&target.path)
                .await
                .unwrap_or(target.path.clone());
            let ext = crate::media_paths::extension(&source);
            let filename = format!(
                "{}{}",
                std::path::Path::new(&target.path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
                if ext == ".strm" { "" } else { &ext }
            );
            let mut info = if missing {
                crate::media_restore::info(state, &sha, &pc, &filename, false, context).await?
            } else {
                crate::media_restore::rebuild_info(state, &sha, &pc, &filename, context).await?
            };
            crate::identity::verify(&target.asset, &info)?;
            if missing {
                let item = details(state, asset, "Path,Size,MediaSources,Chapters").await?;
                let before = crate::revalidation::observation(state, &item).await;
                crate::identity::verify(&json!({"size_bytes":item["Size"]}), &info)?;
                let chapters = crate::media_info::clean_chapters(&item["Chapters"])?;
                crate::media_info::preserve_chapters(&mut info, &chapters)?;
                let stage = final_path
                    .with_file_name(format!(".etk-info-{}", uuid::Uuid::new_v4().simple()));
                let result = async {
                use tokio::io::AsyncWriteExt;
                let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&stage).await?;
                file.write_all(&serde_json::to_vec(&info).unwrap()).await?;
                file.sync_all().await?;
                drop(file);
                context.checkpoint().await?;
                let current = crate::media_edit::target(state, asset).await?;
                if current.path != target.path || current.sha != target.sha || current.asset != target.asset
                    || crate::media_paths::strm_target(&target.path).await.unwrap_or(target.path.clone()) != source
                    || crate::revalidation::observation(state, &details(state, asset, "Path,Size,MediaSources").await?).await != before {
                    return Err(ApiError::bad("文件身份在提取期间变化"));
                }
                tokio::fs::rename(&stage, &final_path).await?;
                sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json").bind(&sha).bind(&info).execute(&state.db).await?;
                Ok::<_,ApiError>(())
            }.await;
                let _ = tokio::fs::remove_file(stage).await;
                result?;
            } else {
                rebuilt.insert(asset.clone(), info);
            }
        }
    if missing {
        return crate::ingest::feedback(state, id, &ids, true, context).await;
    }
    let mut rebuild_warnings = 0_u64;
    let mut rebuild_warning_details = vec![];
    if let Some(first) = rebuilt.keys().next() {
        let body = json!({"mediainfo":rebuilt[first],"target_item_ids":ids,"stream_patches":[]});
        let (status, result) = Box::pin(crate::media_save::save_with_rebuilt(
                state, first, &body, &rebuilt, context,
            ))
            .await?;
        ensure_rebuild_completed(status, &result)?;
        rebuild_warnings = result["warnings"].as_u64().unwrap_or(0);
        rebuild_warning_details = crate::media_edit::list(&result["warning_details"]);
    }
    let single_episode = requested["Type"] == "Episode";
    let item = series_parent(state, requested).await?;
    let episodes = if item["Type"] == "Series" {
        ids
    } else {
        vec![]
    };
    let mut result = if single_episode {
        Box::pin(scrape_episode_scope(
            state,
            &item,
            &episodes,
            context, true))
        .await?
    } else {
        Box::pin(crate::ingest::scrape(
            state, &item, &episodes, None, context,
        ))
        .await?
    };
    if rebuild_warnings > 0 || !rebuild_warning_details.is_empty() {
        result["warnings"] = json!(result["warnings"].as_u64().unwrap_or(0) + rebuild_warnings);
        let mut details = crate::media_edit::list(&result["warning_details"]);
        details.extend(rebuild_warning_details);
        result["warning_details"] = json!(details);
        result["message"] = json!(format!(
            "{}；重建媒体信息有提醒，请查看处理详情",
            string(&result, "message")
        ));
    }
    Ok(result)
}

async fn scrape_episode_scope(
    state: &AppState,
    series: &Value,
    episode_ids: &[String],
    context: &JobContext,
    refresh_metadata: bool,
) -> Result<Value> {
    if episode_ids.is_empty() {
        return Err(ApiError::bad("目标分集范围为空，未扩大到整剧"));
    }
    let context = context.scoped("organize-metadata");
    if context.is_resuming() {
        Box::pin(crate::media_rescrape::resume_cleanup(state, &context)).await?;
    }
    let recovery = state
        .data_dir
        .join("backups/media-info-reload")
        .join(uuid::Uuid::new_v4().simple().to_string());
    context
        .save_plan("rescrape-recovery", &json!(recovery))
        .await?;
    let protection = Box::pin(crate::media_reload::ingest_preservation::capture_targets(
        state,
        series,
        episode_ids,
        &context,
        &recovery,
    ))
    .await;
    let protection = match protection {
        Ok(protection) => protection,
        Err(error) if !refresh_metadata && crate::emby::requires_metadata_only(&error) => {
            // 此阶段仅写所选元数据/NFO/图片，定点原生更新由入库收尾负责。
            tokio::fs::remove_dir_all(&recovery).await?;
            return Box::pin(crate::ingest::scrape_for_ingest(
                state,
                series,
                episode_ids,
                None,
                &context,
            ))
            .await;
        }
        Err(error) => return Err(error),
    };
    protection.check(state, &context).await?;
    protection.publish().await?;
    let confirmed = std::sync::atomic::AtomicBool::new(true);
    let result = Box::pin(crate::strm::with_settings_guarded(
        state,
        "preserve",
        || async {
            protection.check(state, &context).await?;
            let report = Box::pin(crate::ingest::scrape_for_ingest(
                state,
                series,
                episode_ids,
                None,
                &context,
            ))
            .await?;
            if refresh_metadata && report["status"] != "ignored_library" {
                crate::emby::finalize_refresh_targets_guarded(
                    state,
                    series,
                    episode_ids,
                    &context,
                    &confirmed,
                    &recovery,
                )
                .await?;
            }
            let finishing = JobContext::for_recovery(Some(&context)).await;
            protection.verify(state, &finishing).await?;
            protection.verify_unchanged_json().await?;
            Ok(report)
        },
        &confirmed,
        &recovery,
    ))
    .await;
    if result.is_ok() {
        let marker = crate::media_recovery::mark_verified(&recovery, &json!(episode_ids)).await?;
        crate::media_recovery::cleanup_verified(
            &recovery,
            &marker,
            &state.data_dir.join("backups/media-info-conflicts"),
        )
        .await?;
    }
    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            if !tokio::fs::try_exists(recovery.join("original-settings.json")).await? {
                let _ = tokio::fs::remove_dir_all(&recovery).await;
            } else if confirmed.load(std::sync::atomic::Ordering::Acquire) {
                let finishing = JobContext::for_recovery(Some(&context)).await;
                let _ = Box::pin(
                    crate::media_reload::ingest_preservation::finish_saved_guard(
                        state, &recovery, &finishing,
                    ),
                )
                .await;
            }
            if !tokio::fs::try_exists(&recovery).await? {
                return Err(error);
            }
            Err(ApiError(
                error.0,
                format!("{}；恢复材料：{}", error.1, recovery.display()),
            ))
        }
    }
}

fn ensure_rebuild_completed(status: u16, result: &Value) -> Result<()> {
    let mut failures = crate::media_edit::list(&result["failures"]);
    let unconfirmed = matches!(
        result["metadata_refresh"]["state"].as_str(),
        Some("failed" | "unverified")
    );
    if status >= 400 || status == 207 || !failures.is_empty() || unconfirmed {
        failures.extend(crate::media_edit::list(&result["warning_details"]));
        failures.extend(crate::media_edit::list(
            &result["metadata_refresh"]["failures"],
        ));
        let reason = if failures.is_empty() {
            result["error"]
                .as_str()
                .or_else(|| result["message"].as_str())
                .unwrap_or("请查看媒体信息恢复结果")
                .to_string()
        } else {
            failures
                .iter()
                .map(|failure| {
                    failure
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| failure.to_string())
                })
                .collect::<Vec<_>>()
                .join("；")
        };
        return Err(ApiError::bad(format!(
            "重建媒体信息未全部完成（HTTP {status}）：{reason}"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/fixtures/action_reprocess_result.rs"]
mod reprocess_regression;

async fn series_parent(state: &AppState, mut item: Value) -> Result<Value> {
    for _ in 0..2 {
        if !matches!(item["Type"].as_str(), Some("Season" | "Episode")) {
            return Ok(item);
        }
        let parent = item["SeriesId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| item["ParentId"].as_str().filter(|s| !s.is_empty()))
            .ok_or_else(|| ApiError::bad("无法定位父电视剧"))?;
        item = details(
            state,
            parent,
            "Id,Name,Type,Path,People,ProviderIds,Genres,MediaSources,SeriesId,ParentId",
        )
        .await?;
    }
    if item["Type"] != "Series" {
        return Err(ApiError::bad("无法定位父电视剧"));
    }
    Ok(item)
}
async fn sidecar(
    state: &AppState,
    target: &crate::media_edit::Target,
) -> Result<std::path::PathBuf> {
    let config = state.config.read().await.clone();
    let root = string(&config, "local_strm_root");
    if root.is_empty() {
        return Err(ApiError::bad("未配置本地媒体目录"));
    }
    let root = tokio::fs::canonicalize(root).await?;
    let path = std::path::Path::new(&target.path);
    let final_path = if target.path.starts_with("http") {
        target
            .candidates
            .iter()
            .find(|p| {
                p.is_absolute() && p.starts_with(&root) && p.parent().is_some_and(|p| p.is_dir())
            })
            .cloned()
            .ok_or_else(|| ApiError::bad("无法映射本地媒体信息路径"))?
    } else {
        path.with_file_name(format!(
            "{}-mediainfo.json",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ))
    };
    let directory = tokio::fs::canonicalize(
        final_path
            .parent()
            .ok_or_else(|| ApiError::bad("媒体信息路径无效"))?,
    )
    .await?;
    if !directory.starts_with(&root)
        || tokio::fs::symlink_metadata(&final_path)
            .await
            .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(ApiError::bad("媒体信息路径越过配置目录"));
    }
    Ok(final_path)
}
pub fn has_strm_container(info: &Value) -> bool {
    let entries = info
        .as_array()
        .map_or_else(|| vec![info], |v| v.iter().collect());
    entries.into_iter().any(|entry| {
        let source = if entry["MediaSourceInfo"].is_object() {
            &entry["MediaSourceInfo"]
        } else {
            entry
        };
        source["Container"]
            .as_str()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("strm"))
    })
}
