//! Metadata-only processing with proof that existing media resources survive.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    state::string,
};
use axum::{extract::State, http::StatusCode};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) async fn run(state: &AppState, id: &str, context: &JobContext) -> Result<Value> {
    context.checkpoint().await?;
    Box::pin(recover_previous(state, id, context)).await?;
    let target = details(state, id, "Id,Type,Name,Path,People,ProviderIds,Genres,MediaSources,SeriesId,ParentId,ParentIndexNumber,IndexNumber").await?;
    if target["Id"] != id || !matches!(target["Type"].as_str(), Some("Movie" | "Episode")) {
        return Err(ApiError::bad("重新刮削仅支持明确的电影或分集目标"));
    }
    let binding = crate::media_edit::repair_target(state, id).await?;
    let path = string(&target, "Path");
    if binding.path != path || !Path::new(&path).is_absolute() {
        return Err(ApiError::bad("重新刮削目标的原生路径未确认"));
    }
    if crate::media_save::protected(&*state.config.read().await, &path) {
        return Err(ApiError::bad(
            "重新刮削仅写入本地媒体目录，请使用本地 STRM 路径",
        ));
    }
    let pending = crate::media_recovery::status(State(state.clone())).await?.0;
    if list(&pending["records"]).iter().any(|record| record["kind"] != "wash-pending") {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "存在相关媒体或原配置尚未恢复的记录，请先完成恢复或重新整理入库".into(),
        ));
    }
    let item = if target["Type"] == "Episode" {
        let series = string(&target, "SeriesId");
        let item = details(
            state,
            &series,
            "Id,Type,Name,Path,People,ProviderIds,Genres,MediaSources",
        )
        .await?;
        if series.is_empty() || item["Id"] != series || item["Type"] != "Series" {
            return Err(ApiError::bad("重新刮削未能确认分集所属电视剧"));
        }
        item
    } else {
        target.clone()
    };
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    if tmdb.is_empty() {
        return Err(ApiError::bad("重新刮削缺少影视 TMDb 绑定"));
    }
    // A cached official-season binding is explicit too; do not rematch groups.
    let group = if item["Type"] == "Series" {
        crate::metadata::row(state, &tmdb, "Series")
            .await?
            .map(|row| string(&row, "episode_group_id"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let recovery = state
        .data_dir
        .join("backups/media-info-reload")
        .join(uuid::Uuid::new_v4().simple().to_string());
    context
        .save_plan("rescrape-recovery", &json!(recovery))
        .await?;
    let guard = match crate::media_reload::ingest_preservation::capture_strict(
        state, &target, context, &recovery,
    )
    .await
    {
        Ok(guard) => guard,
        Err(error) => {
            // Preparation did not change a media file or native configuration.
            let _ = tokio::fs::remove_dir_all(&recovery).await;
            return Err(error);
        }
    };
    guard.check(state, context).await?;
    guard.publish().await?;
    let confirmed = AtomicBool::new(true);
    let result = Box::pin(crate::strm::with_settings_guarded(
        state,
        "preserve",
        || async {
            guard.check(state, context).await?;
            context
                .activity("重新刮削", "更新目标元数据、NFO 和图片，保留原媒体信息")
                .await;
            let mut report = Box::pin(crate::ingest::rescrape_metadata(
                state,
                &item,
                &target,
                Some(&group),
                context,
            ))
            .await?;
            let expected = report
                .as_object_mut()
                .and_then(|report| report.remove("_rescrape_expected"))
                .filter(Value::is_object)
                .ok_or_else(|| ApiError::bad("重新刮削未生成可验收的目标元数据"))?;
            if !Path::new(&path).with_extension("nfo").is_file() {
                return Err(ApiError::bad("重新刮削目标 NFO 未生成"));
            }
            guard.check(state, context).await?;
            let position = crate::media_extra::log_position(state).await?;
            crate::emby::prepare_refresh_guard(Some((&confirmed, &recovery)), id, Some(position))
                .await?;
            state
                .emby(
                    Method::POST,
                    &format!("Items/{id}/Refresh"),
                    &[
                        ("Recursive", "false".into()),
                        ("MetadataRefreshMode", "FullRefresh".into()),
                        ("ImageRefreshMode", "FullRefresh".into()),
                        ("ReplaceAllMetadata", "false".into()),
                        ("ReplaceAllImages", "false".into()),
                    ],
                    None,
                )
                .await?;
            // An accepted native refresh must finish before temporary settings are
            // restored, including when the operator stops the outer queue job.
            let finishing = JobContext::for_recovery(Some(context)).await;
            crate::emby::wait_item_refresh(
                state,
                id,
                None,
                Some(crate::media_extra::RefreshTrace::new(position)),
                &finishing,
                "重新刮削刷新验收",
                &string(&target, "Name"),
            )
            .await?;
            confirmed.store(true, Ordering::Release);
            guard.verify(state, &finishing).await?;
            guard.verify_unchanged_json().await?;
            let current = details(state, id, "Id,Type,Path,Name,Overview").await?;
            if ["Id", "Type", "Path"]
                .iter()
                .any(|field| current[*field] != target[*field])
                || ["Name", "Overview"]
                    .iter()
                    .any(|field| current[*field] != expected[*field])
            {
                return Err(ApiError::bad(
                    "重新刮削后的目标身份、标题或简介未通过回读验收",
                ));
            }
            report["media_preserved"] = json!(true);
            report["message"] = json!("重新刮削完成，原媒体信息、音轨、章节及预览资源已验收保留");
            Ok(report)
        },
        &confirmed,
        &recovery,
    ))
    .await;
    match result {
        Ok(report) => {
            tokio::fs::remove_dir_all(&recovery)
                .await
                .map_err(|error| {
                    ApiError::bad(format!(
                        "重新刮削已验收，恢复材料清理失败：{error}；目录：{}",
                        recovery.display()
                    ))
                })?;
            Ok(report)
        }
        Err(error) => {
            if !tokio::fs::try_exists(recovery.join("original-settings.json")).await? {
                // Discovering/applying the guarded configuration never reached
                // a native mutation; these are read-only duplicate snapshots.
                if tokio::fs::remove_dir_all(&recovery).await.is_ok() {
                    return Err(error);
                }
            } else if confirmed.load(Ordering::Acquire) {
                let finishing = JobContext::for_recovery(Some(context)).await;
                if crate::media_reload::ingest_preservation::finish_saved_guard(
                    state, &recovery, &finishing,
                )
                .await
                .is_ok()
                {
                    // Preserve the real scrape failure, while allowing a retry
                    // when all original resources/configuration still verify.
                    return Err(error);
                }
            }
            Err(ApiError(
                error.0,
                format!("{}；恢复材料：{}", error.1, recovery.display()),
            ))
        }
    }
}

async fn recover_previous(state: &AppState, id: &str, context: &JobContext) -> Result<()> {
    if !context.is_resuming() {
        return Ok(());
    }
    let Some(saved) = context.plan("rescrape-recovery").await? else {
        return Ok(());
    };
    let directory = recovery_directory(state, saved)?;
    if !tokio::fs::try_exists(&directory).await? {
        return Ok(());
    }
    if !tokio::fs::try_exists(directory.join("original-settings.json")).await? {
        // Settings are written durably before their native mutation. This
        // unfinished read-only preparation contains only duplicate backups.
        tokio::fs::remove_dir_all(directory).await?;
        return Ok(());
    }
    let pending = crate::media_recovery::status(State(state.clone())).await?.0;
    let record = list(&pending["records"])
        .into_iter()
        .find(|record| record["directory"] == json!(directory));
    let Some(record) = record else {
        return Ok(());
    };
    if record["can_recover"] != true
        || !list(&record["json_conflicts"]).is_empty()
        || !list(&record["target_ids"]).contains(&json!(id))
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "本次重新刮削的恢复材料或当前 JSON 存在未确认变化，已保留材料并停止自动覆盖".into(),
        ));
    }
    context
        .activity(
            "恢复重新刮削",
            "先验收本任务原媒体资源和原配置，再继续未完成的元数据处理",
        )
        .await;
    let finishing = JobContext::for_recovery(Some(context)).await;
    Box::pin(crate::media_reload::ingest_preservation::finish_saved_guard(
        state,
        &directory,
        &finishing),
    )
    .await?;
    Ok(())
}

fn recovery_directory(state: &AppState, saved: Value) -> Result<PathBuf> {
    let directory: PathBuf =
        serde_json::from_value(saved).map_err(|_| ApiError::bad("重新刮削恢复目录记录无效"))?;
    if directory.parent() != Some(state.data_dir.join("backups/media-info-reload").as_path())
        || !directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
    {
        return Err(ApiError::bad("重新刮削恢复目录不属于本任务"));
    }
    Ok(directory)
}

/// A paused job may release its submitted native refresh guard without doing
/// any new scrape, media import, or replacement of an original media file.
pub(crate) async fn resume_cleanup(state: &AppState, context: &JobContext) -> Result<()> {
    for (key, saved) in context.saved_plans().await? {
        if key.rsplit(':').next() != Some("rescrape-recovery") {
            continue;
        }
        let directory = recovery_directory(state, saved)?;
        let marker = directory.with_file_name(format!(
            "{}.cleanup.json",
            directory.file_name().unwrap().to_string_lossy()
        ));
        if tokio::fs::try_exists(&marker).await? {
            crate::media_recovery::cleanup_verified(
                &directory,
                &marker,
                &state.data_dir.join("backups/media-info-conflicts"),
            )
            .await?;
        } else if tokio::fs::try_exists(&directory).await? {
            if !tokio::fs::try_exists(directory.join("original-settings.json")).await? {
                tokio::fs::remove_dir_all(&directory).await?;
            } else {
                crate::media_reload::ingest_preservation::finish_saved_guard(
                    state, &directory, context,
                )
                .await?;
            }
        }
    }
    Ok(())
}
