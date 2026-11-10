use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::details,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use futures_util::FutureExt;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/actions/reprocess_item/{id}", post(submit))
        .route("/api/actions/reprocess_all_review_items", post(all))
        .route("/api/actions/reorganize", post(reorganize))
}
async fn reorganize(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let selected = body["item_ids"]
        .as_array()
        .filter(|ids| !ids.is_empty() && ids.len() <= 500)
        .ok_or_else(|| ApiError::bad("请选择 1 至 500 个明确的媒体项目"))?;
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
    let history_ids = crate::media_organize_history::selected_history(&state, &body, &ids).await?;
    let work = state.clone();
    let task = state.jobs.submit_response("媒体重新整理", move |context| async move {
        let total = ids.len();
        let mut items = vec![];
        let mut failures = vec![];
        let mut warning_details = vec![];
        let mut warnings = 0_u64;
        let mut updated = 0;
        for (index, id) in ids.iter().enumerate() {
            if let Err(error) = context.checkpoint().await {
                if context.is_cancelled() { break; }
                return Err(error);
            }
            context.update_counted(index as u64, total as u64, "媒体重新整理", &format!("重新执行 ETK 流程：媒体 {id}")).await;
            crate::media_organize_history::begin_reorganize(&work,&history_ids,id).await?;
            // The empty reason runs the complete rebuilding and scrape flow, rather
            // than the missing-media-info shortcut used by review retries.
            let report = std::panic::AssertUnwindSafe(reprocess_inner(&work, id, "", &context, true)).catch_unwind().await
                .unwrap_or_else(|_|Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR,"任务执行异常".into())));
            if let Err(error) = crate::media_organize_history::finish_reorganize(&work,&history_ids,id,&report).await {
                warnings += 1;
                warning_details.push(json!(format!("{id}：入库处理记录未能保存，{}",crate::logs::redact(&error.1))));
            }
            match report {
                Ok(report) => {
                    let mut errors = crate::media_edit::list(&report["failures"]);
                    if let Some(error) = report["error"].as_str().filter(|error| !error.is_empty()) {
                        errors.push(json!(error));
                    }
                    let failed = !errors.is_empty() || report["success"] == false
                        || report["failed"].as_u64().unwrap_or(0) > 0;
                    let details = crate::media_edit::list(&report["warning_details"]);
                    warnings += report["warnings"].as_u64().unwrap_or(0);
                    warning_details.extend(details.iter().map(|detail| json!(format!("{id}：{}", detail.as_str().unwrap_or("处理有提醒")))));
                    if failed {
                        failures.push(format!("{id}：{}", if errors.is_empty() { string(&report, "message") } else { errors.iter().map(|error| error.as_str().map(str::to_string).unwrap_or_else(||error.to_string())).collect::<Vec<_>>().join("；") }));
                    } else {
                        updated += 1;
                    }
                    items.push(json!({"item_id":id,"status":if failed {"failed"} else {"completed"},"report":report}));
                }
                Err(error) if context.is_cancelled() => {
                    items.push(json!({"item_id":id,"status":"cancelled","error":error.1}));
                    break;
                }
                Err(error) => {
                    failures.push(format!("{id}：{}", error.1));
                    items.push(json!({"item_id":id,"status":"failed","error":error.1}));
                }
            }
        }
        let failed = failures.len();
        let remaining = &ids[items.len()..];
        let cancelled = context.is_cancelled();
        if cancelled {
            warning_details.push(json!(format!("任务已取消，已保留处理结果，仍有 {} 项未开始整理", remaining.len())));
        }
        Ok((if failed > 0 || warnings > 0 || !warning_details.is_empty() {207} else {200}, json!({
            "selected_count":total,"updated":updated,"failed":failed,"items":items,"remaining_ids":remaining,"cancelled":cancelled,
            "failures":failures,"warnings":warnings,"warning_details":warning_details,
            "message":format!("媒体重新整理{}：处理 {updated} 项，失败 {failed} 项", if cancelled {"已取消"} else {"完成"})
        })))
    }).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"媒体重新整理任务已提交"})),
    ))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task = state
        .jobs
        .submit("重新处理项目", move |context| async move {
            reprocess(&work, &id, body["reason"].as_str().unwrap_or(""), &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"重新处理项目任务已提交"})),
    ))
}
async fn all(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let kind = string(&body, "media_type");
    if !["", "Movie", "Series"].contains(&kind.as_str()) {
        return Err(ApiError::bad("媒体类型仅支持电视剧或电影"));
    }
    let reason = string(&body, "reason");
    let work = state.clone();
    let task=state.jobs.submit("重新处理待复核项",move|context|async move{
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(f) FROM failed_log f WHERE ($1='' OR reason LIKE $1||'%') AND ($2='' OR item_type=$2 OR ($2='Series' AND item_type IN ('Season','Episode')))").bind(reason).bind(kind).fetch_all(&work.db).await?;
        let mut updated=0;let mut failures=vec![];let mut warnings=0_u64;let mut warning_details=vec![];
        for row in rows{
            context.checkpoint().await?;let id=string(&row,"item_id");
            match reprocess(&work,&id,&string(&row,"reason"),&context).await{
                Ok(report)=>{
                    updated+=1;
                    warnings+=report["warnings"].as_u64().unwrap_or(0);
                    warning_details.extend(crate::media_edit::list(&report["warning_details"]));
                },
                Err(error) if context.is_cancelled()=>return Err(error),
                Err(error)=>failures.push(format!("{id}: {}",error.1))
            }
        }
        Ok(json!({"updated":updated,"failures":failures,"warnings":warnings,"warning_details":warning_details,"message":format!("待复核项目重新处理完成：处理 {updated} 项，失败 {} 项，提醒 {warnings} 条",failures.len())}))
    }).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"批量重新处理任务已提交"})),
    ))
}
pub async fn reprocess(
    state: &AppState,
    id: &str,
    reason: &str,
    context: &JobContext,
) -> Result<Value> {
    reprocess_inner(state, id, reason, context, false).await
}
async fn reprocess_inner(
    state: &AppState,
    id: &str,
    reason: &str,
    context: &JobContext,
    file_refresh: bool,
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
            let stage =
                final_path.with_file_name(format!(".etk-info-{}", uuid::Uuid::new_v4().simple()));
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
        let (status, result) = if file_refresh {
            crate::media_save::save_rebuilt_files(state, first, &body, &rebuilt, context).await?
        } else {
            crate::media_save::save_with_rebuilt(state, first, &body, &rebuilt, context).await?
        };
        ensure_rebuild_completed(status, &result)?;
        rebuild_warnings = result["warnings"].as_u64().unwrap_or(0);
        rebuild_warning_details = crate::media_edit::list(&result["warning_details"]);
    }
    let item = series_parent(state, requested).await?;
    let episodes = if item["Type"] == "Series" {
        ids
    } else {
        vec![]
    };
    let mut result = crate::ingest::scrape(state, &item, &episodes, None, context).await?;
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
