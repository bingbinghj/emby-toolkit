use crate::{
    AppState,
    error::{ApiError, Result},
    identity,
    jobs::JobContext,
    media_display,
    media_edit::{self, Target},
    media_info,
    state::string,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};
use tokio::io::AsyncWriteExt;

pub async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    if !media_info::recoverable(&body["mediainfo"]) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "当前编辑数据不可安全恢复，已停止保存".into(),
        ));
    }
    if !body["target_item_ids"].is_null() && !body["target_item_ids"].is_array() {
        return Err(ApiError::bad("target_item_ids 必须是数组"));
    }
    if body["target_item_ids"]
        .as_array()
        .is_some_and(|v| !v.is_empty())
        && !body["stream_patches"].is_array()
    {
        return Err(ApiError::bad("批量编辑必须提供 stream_patches"));
    }
    let manager = state.jobs.clone();
    let task = manager
        .submit_response("保存媒体信息", move |context| async move {
            save(&state, &id, &body, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"保存媒体信息已提交，可在通知中心查看实时日志"})),
    ))
}
struct Prepared {
    target: Target,
    info: Value,
    path: String,
    source: Option<String>,
    final_path: PathBuf,
    stage: Option<PathBuf>,
    chapters: Value,
    multipart: bool,
    has_streams: bool,
}
fn failed(message: &str, failures: &[String], status: u16) -> Result<(u16, Value)> {
    Ok((status, json!({"error":message,"failures":failures})))
}
fn protected(state_config: &Value, path: &str) -> bool {
    let normalized = path.trim().replace('\\', "/").to_lowercase();
    let root = string(state_config, "cd2_media_root")
        .trim_end_matches('/')
        .replace('\\', "/")
        .to_lowercase();
    (!root.is_empty() && (normalized == root || normalized.starts_with(&format!("{root}/"))))
        || normalized.starts_with("/cd2/")
        || normalized.contains("/115open/")
        || normalized.contains("/115/")
}
async fn prepare(state: &AppState, target: Target, mut info: Value) -> Result<Prepared> {
    let item = media_edit::details(
        state,
        &target.id,
        "Id,Path,PartCount,Chapters,Type,MediaStreams,MediaSources",
    )
    .await?;
    info = media_info::restore_json(&info)?;
    let path = string(&item, "Path");
    if path.is_empty() {
        return Err(ApiError::bad(format!(
            "{}: 无法读取 Emby 原生 Path",
            target.id
        )));
    }
    if item["Id"] != target.id || path != target.path {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Emby媒体身份或路径与缓存不一致，请先同步媒体数据，已停止保存".into(),
        ));
    }
    let config = state.config.read().await;
    if protected(&config, &path) {
        return Err(ApiError::bad("Emby 原生路径位于 115 源目录，已禁止写入"));
    }
    drop(config);
    let final_path = PathBuf::from(format!(
        "{}-mediainfo.json",
        std::path::Path::new(&path)
            .with_extension("")
            .to_string_lossy()
    ));
    let source = crate::media_paths::strm_target(&path).await;
    let paths = [&path[..], source.as_deref().unwrap_or("")];
    let has_streams = media_info::has_media_streams(&item, &paths);
    let chapters = media_info::recovery_chapters(&info, &item, &paths).map_err(|error| {
        ApiError(
            StatusCode::CONFLICT,
            format!("{}：无法保护当前章节或恢复备份标记：{}", target.id, error.1),
        )
    })?;
    media_info::preserve_chapters(&mut info, &chapters)?;
    Ok(Prepared {
        source,
        has_streams,
        target,
        info,
        path,
        final_path,
        stage: None,
        chapters,
        multipart: item["PartCount"].as_u64().unwrap_or(0) > 1,
    })
}
async fn stage_all(
    state: &AppState,
    prepared: &mut BTreeMap<String, Prepared>,
    order: &std::collections::HashMap<String, crate::media_order::Key>,
    cache: bool,
) -> Result<()> {
    // All identity and chapter reads finish before any persistent write or cache clear.
    let mut transaction = state.db.begin().await?;
    let mut ids = prepared.keys().cloned().collect::<Vec<_>>();
    ids.sort_by_cached_key(|id| order.get(id).cloned());
    for id in ids {
        let item = prepared.get_mut(&id).unwrap();
        if !item.target.kind.is_empty() {
            let current = media_edit::target(state, &item.target.id).await?;
            if current.path != item.target.path
                || current.sha != item.target.sha
                || current.asset != item.target.asset
            {
                return Err(ApiError::bad("媒体资产在准备保存期间变化，未清理Emby数据"));
            }
        }
        let current = media_edit::details(
            state,
            &item.target.id,
            "Id,Path,Size,MediaSources,MediaStreams,Chapters",
        )
        .await?;
        if current["Id"] != item.target.id || string(&current, "Path") != item.path {
            return Err(ApiError::bad(
                "Emby媒体身份或路径在准备保存期间变化，未清理Emby数据",
            ));
        }
        if crate::media_paths::strm_target(&item.path).await != item.source {
            return Err(ApiError::bad("STRM目标在准备保存期间变化，未清理Emby数据"));
        }
        identity::verify(
            &serde_json::json!({"size_bytes":current["Size"]}),
            &item.info,
        )?;
        let paths = [&item.path[..], item.source.as_deref().unwrap_or("")];
        item.has_streams = media_info::has_media_streams(&current, &paths);
        item.chapters = media_info::recovery_chapters(&item.info, &current, &paths)?;
        media_info::preserve_chapters(&mut item.info, &item.chapters)?;
        identity::verify(&item.target.asset, &item.info)?;
        let directory = item
            .final_path
            .parent()
            .ok_or_else(|| ApiError::bad("媒体信息目标目录无效"))?;
        let filename = item
            .final_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ApiError::bad("媒体信息目标文件名无效"))?;
        let path = directory.join(format!(
            ".{filename}.etk-stage-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        item.stage = Some(path);
        file.write_all(
            &serde_json::to_vec_pretty(&item.info)
                .map_err(|_| ApiError::bad("媒体信息 JSON 无效"))?,
        )
        .await?;
        file.sync_all().await?;
        if cache && let Some(sha) = &item.target.sha {
            sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                .bind(sha).bind(&item.info).execute(&mut *transaction).await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}
async fn cleanup_stages(prepared: &BTreeMap<String, Prepared>) {
    for item in prepared.values() {
        if let Some(path) = &item.stage {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}
async fn emby_post(state: &AppState, path: &str, query: &[(&str, String)]) -> Result<()> {
    state.emby(Method::POST, path, query, None).await?;
    Ok(())
}
pub async fn refresh(state: &AppState, paths: &[String]) -> Result<()> {
    refresh_with_ids(state, paths).await.map(|_| ())
}
pub(crate) async fn refresh_with_ids(state: &AppState, paths: &[String]) -> Result<Vec<String>> {
    let (submitted, failures) = refresh_report(state, paths).await;
    if failures.is_empty() {
        Ok(submitted)
    } else {
        Err(ApiError::bad(format!(
            "Emby目录扫描请求未成功：{}",
            failures
                .iter()
                .map(|failure| format!(
                    "{}：{}",
                    string(failure, "path"),
                    string(failure, "reason")
                ))
                .collect::<Vec<_>>()
                .join("；")
        )))
    }
}
async fn refresh_report(state: &AppState, paths: &[String]) -> (Vec<String>, Vec<Value>) {
    let config = state.config.read().await.clone();
    let local = string(&config, "local_strm_root").replace('\\', "/");
    let cd2 = string(&config, "cd2_media_root").replace('\\', "/");
    let cd2 = cd2.trim_end_matches('/');
    let directories: BTreeSet<_> = paths
        .iter()
        .map(|path| {
            let path = path.replace('\\', "/");
            if !local.is_empty() && !cd2.is_empty() {
                if path == cd2 {
                    return local.clone();
                }
                if let Some(suffix) = path.strip_prefix(&format!("{cd2}/")) {
                    return format!("{}/{suffix}", local.trim_end_matches('/'));
                }
            }
            path
        })
        .filter_map(|p| std::path::Path::new(&p).parent().map(|p| p.to_path_buf()))
        .collect();
    let mut failures = vec![];
    let mut submitted = vec![];
    for mut directory in directories {
        let original_directory = directory.clone();
        let mut refreshed = false;
        let mut last_error = "Emby未返回匹配目录".to_string();
        for _ in 0..4 {
            if directory.as_os_str().is_empty() || directory == std::path::Path::new("/") {
                break;
            }
            if !directory.exists() {
                directory.pop();
                continue;
            }
            let path = directory.to_string_lossy().to_string();
            let attempt: Result<bool> = async {
                let base = string(&config, "emby_server_url");
                let key = string(&config, "emby_api_key");
                let response = crate::emby::send(
                    state
                        .http
                        .get(format!("{}/Items", base.trim_end_matches('/')))
                        .header("X-Emby-Token", &key)
                        .query(&[
                            ("Recursive", "true"),
                            ("Path", path.as_str()),
                            ("Fields", "Id,Path,Name"),
                        ]),
                )
                .await?;
                if response.status() != StatusCode::OK {
                    return Err(ApiError::bad(format!("目录查询返回HTTP {}", response.status().as_u16())));
                }
                let response: Value = response.json().await?;
                if let Some(item) = response["Items"].as_array().and_then(|items| items.first()) {
                    let target_id = string(item, "Id");
                    if target_id.is_empty() {
                        return Err(ApiError::bad("Emby返回的目录缺少媒体ID"));
                    }
                    if submitted.contains(&target_id) { return Ok(true); }
                    crate::emby::send(
                        state
                            .http
                            .post(format!(
                                "{}/Items/{}/Refresh",
                                base.trim_end_matches('/'),
                                string(item, "Id")
                            ))
                            .header("X-Emby-Token", &key)
                            .query(&[
                                ("Recursive", "true"),
                                ("MetadataRefreshMode", "Default"),
                                ("ImageRefreshMode", "Default"),
                                ("ReplaceAllImages", "false"),
                                ("ReplaceAllMetadata", "false"),
                            ]),
                    )
                    .await?.error_for_status()?;
                    submitted.push(target_id.clone());
                    tracing::debug!(
                        "[精准扫描] Emby已接收 {} 的目录扫描请求（媒体ID：{target_id}，路径：{path}，递归是，默认刷新，全部替换否）",
                        item["Name"].as_str().unwrap_or(&path)
                    );
                    tracing::info!("[精准扫描] Emby已接收《{}》的扫描请求", item["Name"].as_str().unwrap_or(&path));
                    return Ok(true);
                }
                Ok(false)
            }
            .await;
            if matches!(attempt, Ok(true)) {
                refreshed = true;
                break;
            }
            if let Err(error) = attempt {
                last_error = error.1;
                tracing::warn!(
                    "[精准扫描] 目录 {path} 查询或提交失败：{last_error}，继续核对父目录"
                );
            }
            if !directory.pop() {
                break;
            }
        }
        if !refreshed {
            let reason = format!("{}：{last_error}", original_directory.display());
            tracing::warn!("[精准扫描] {reason}；未提交有效扫描请求，媒体信息文件已保留");
            failures.push(json!({"path":original_directory,"reason":last_error}));
        }
    }
    (submitted, failures)
}
pub(crate) async fn restoration_settings(
    state: &AppState,
    targets: &[(String, String)],
    include_extra: bool,
) -> Result<Value> {
    if targets.is_empty() {
        return Err(ApiError::bad("恢复目标为空，未清理 Emby 数据"));
    }
    let settings = crate::strm::config(state).await?;
    if !matches!(
        settings["PersistMediaInfoMode"].as_str(),
        Some("None" | "Default" | "Restore")
    ) || !settings["LibraryScope"].is_string()
    {
        return Err(ApiError::bad(
            "神医恢复模式或媒体库范围配置无效，未清理 Emby 数据",
        ));
    }
    if !string(&settings, "MediaInfoJsonRootFolder").is_empty() {
        return Err(ApiError::bad(
            "神医 JSON 根目录与当前恢复文件位置不一致，已停止保存，未清理 Emby 数据",
        ));
    }
    let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
    tasks
        .as_array()
        .and_then(|tasks| {
            tasks.iter().find(|task| {
                task["Key"] == "MediaInfoExtractTask" && !string(task, "Id").is_empty()
            })
        })
        .ok_or_else(|| ApiError::bad("未找到神医提取媒体信息任务，已停止保存，未清理 Emby 数据"))?;
    if !crate::strm::media_tasks_idle(&tasks) {
        return Err(ApiError::bad(
            "神医媒体信息、声纹或缩略图提取任务尚未结束，请结束后重试保存，未清理 Emby 数据",
        ));
    }
    let folders = state
        .emby(Method::GET, "Library/VirtualFolders", &[], None)
        .await?;
    let folders = media_edit::list(&folders);
    let mut libraries = BTreeSet::new();
    for (id, path) in targets {
        let library = crate::emby::library_for_path_in(&folders, path)
            .ok_or_else(|| ApiError::bad(format!("{id}：无法确定恢复媒体库，未清理 Emby 数据")))?;
        let id = string(library, "ItemId");
        if id.is_empty() {
            return Err(ApiError::bad("恢复媒体库缺少 ID，未清理 Emby 数据"));
        }
        libraries.insert(id);
    }
    let mut changes = json!({"PersistMediaInfoMode":"Restore","LibraryScope":libraries.into_iter().collect::<Vec<_>>().join(",")});
    if include_extra {
        if !settings["IncludeExtra"].is_boolean() {
            return Err(ApiError::bad(
                "神医额外媒体配置无效，无法恢复分段电影，未清理 Emby 数据",
            ));
        }
        changes["IncludeExtra"] = json!(true);
    }
    Ok(changes)
}
pub async fn save(
    state: &AppState,
    id: &str,
    body: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    save_inner(state, id, body, &BTreeMap::new(), Some(context), true).await
}
pub async fn save_with_rebuilt(
    state: &AppState,
    id: &str,
    body: &Value,
    rebuilt: &BTreeMap<String, Value>,
    context: &JobContext,
) -> Result<(u16, Value)> {
    save_inner(state, id, body, rebuilt, Some(context), false).await
}
async fn save_inner(
    state: &AppState,
    id: &str,
    body: &Value,
    rebuilt: &BTreeMap<String, Value>,
    context: Option<&JobContext>,
    file_refresh: bool,
) -> Result<(u16, Value)> {
    let mut ids = vec![id.to_string()];
    for value in body["target_item_ids"].as_array().into_iter().flatten() {
        if let Some(value) = value.as_str().filter(|s| !s.trim().is_empty())
            && !ids.iter().any(|id| id == value.trim())
        {
            ids.push(value.trim().into());
        }
    }
    let order = crate::media_order::id_keys(state, &ids).await?;
    // The edited item remains the first identity/track validation gate.
    ids[1..].sort_by_cached_key(|id| order.get(id).cloned());
    tracing::info!(
        item_id = id,
        targets = ids.len(),
        "[媒体信息保存] 开始核对文件身份、轨道与关联版本"
    );
    let mut prepared = BTreeMap::new();
    let mut groups = BTreeSet::new();
    let mut failures = vec![];
    let mut reports = vec![];
    let mut saved = vec![];
    for (index, target_id) in ids.iter().enumerate() {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        if let Some(context) = context {
            context
                .update(
                    5 + 20 * index as u64 / ids.len() as u64,
                    "核对媒体信息",
                    &format!("核对媒体信息 {}/{}", index + 1, ids.len()),
                )
                .await;
        }
        let result=async {
            let target=if rebuilt.is_empty(){media_edit::target(state,target_id).await?}else{media_edit::repair_target(state,target_id).await?};
            let original=if let Some(info)=rebuilt.get(target_id){info.clone()}else{media_edit::load(state,&target).await?};
            if target_id==id {
                if let Some(sha)=body.get("sha1") && text(sha).to_uppercase()!=target.sha.clone().unwrap_or_default().to_uppercase() {return Err(ApiError::bad("当前文件指纹已变化，请重新打开编辑器"));}
                if body["media_path"].as_str().is_some_and(|p|!p.is_empty() && p!=target.path) {return Err(ApiError::bad("当前文件路径已变化，请重新打开编辑器"));}
                if let Some(source) = body["strm_source"].as_str() && crate::media_paths::strm_target(&target.path).await.unwrap_or_default() != source { return Err(ApiError::bad("STRM目标已变化，未清理Emby数据")); }
            }
            identity::verify(&target.asset,if target_id==id {&body["mediainfo"]} else {&original})?;
            let mut info=if let Some(info)=rebuilt.get(target_id){info.clone()}
                else if target_id==id {body["mediainfo"].clone()}
                else {
                    let (info,report)=media_info::apply_patches(&original,&body["stream_patches"]);
                    reports.push(json!({"item_id":target_id,"path":target.path,"matched":report["matched"],"skipped":report["skipped"]}));
                    if report["matched"]==0 {return Ok(None);}
                    info
                };
            // A stale editing page may still contain empty chapters after a failed restore.
            if let Some(chapters) = media_info::object(&original).and_then(|root| root["Chapters"].as_array()) {
                media_info::preserve_chapters(&mut info, &json!(chapters))?;
            }
            let mut group=BTreeSet::from([target.top_id.clone()]);
            group.extend(target.versions.iter().map(|v|string(v,"emby_id")).filter(|s|!s.is_empty()));
            groups.insert(group.into_iter().collect::<Vec<_>>());
            Ok::<_,ApiError>(Some((target,info)))
        }.await;
        match result {
            Ok(Some((target, info))) => {
                saved.push(target_id.clone());
                prepared.insert(target_id.clone(), (target, info));
            }
            Ok(None) => {}
            Err(error) => {
                failures.push(format!("{target_id}: {}", error.1));
                if target_id == id {
                    break;
                }
            }
        }
    }
    if saved.is_empty() {
        return failed(
            "没有可安全恢复的媒体信息，已停止保存，未清理 Emby 数据",
            &failures,
            409,
        );
    }
    for group in &groups {
        for linked in group {
            if prepared.contains_key(linked) {
                continue;
            }
            let result = async {
                let target = if rebuilt.is_empty() {
                    media_edit::target(state, linked).await?
                } else {
                    media_edit::repair_target(state, linked).await?
                };
                let info = if let Some(info) = rebuilt.get(linked) {
                    info.clone()
                } else {
                    media_edit::load(state, &target).await?
                };
                identity::verify(&target.asset, &info)?;
                Ok::<_, ApiError>((target, info))
            }
            .await;
            match result {
                Ok(item) => {
                    prepared.insert(linked.clone(), item);
                }
                Err(error) => {
                    failures.push(format!("{linked}: {}", error.1));
                    return failed(
                        "关联版本媒体信息不完整，已停止保存，未清理 Emby 数据",
                        &failures,
                        409,
                    );
                }
            }
        }
    }
    let mut restore = BTreeMap::new();
    let mut prepared = prepared.into_iter().collect::<Vec<_>>();
    prepared.sort_by_cached_key(|(id, _)| order.get(id).cloned());
    for (id, (target, info)) in prepared {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        let prepared = prepare(state, target, info).await?;
        if prepared.multipart {
            let details = media_edit::details(state, &id, "Id,Path,PartCount,MediaSources").await?;
            let parts = media_edit::additional_parts(state, &id, &details).await?;
            if parts.is_empty() {
                return failed(
                    "分段电影额外分段不可读取，已停止保存，未清理 Emby 数据",
                    &failures,
                    409,
                );
            }
            for part in parts {
                let part_id = string(&part, "Id");
                if part_id.is_empty() || string(&part, "Path").is_empty() {
                    return failed("额外分段缺少可恢复路径，已停止保存", &failures, 409);
                }
                if restore.contains_key(&part_id) {
                    continue;
                }
                let source = part["Path"].as_str().unwrap_or("");
                let target = media_edit::build(state, &part_id, source, None, None).await?;
                let info = media_edit::load(state, &target).await?;
                restore.insert(part_id, prepare(state, target, info).await?);
            }
        }
        restore.insert(id, prepared);
    }
    if let Some(context) = context {
        context.update(35, "暂存", "暂存媒体信息和当前章节").await;
    }
    if file_refresh {
        let staged = stage_all(state, &mut restore, &order, false).await;
        if let Err(error) = staged {
            cleanup_stages(&restore).await;
            return Err(error);
        }
        let mut warning_details = vec![];
        let mut written_ids = vec![];
        let mut items = restore.iter().collect::<Vec<_>>();
        items.sort_by_cached_key(|(id, _)| order.get(*id).cloned());
        for (id, item) in items {
            if let Some(stage) = &item.stage {
                if let Err(error) = tokio::fs::rename(stage, &item.final_path).await {
                    failures.push(format!("{id}：媒体信息 JSON 保存失败：{error}"));
                    continue;
                }
                written_ids.push(id.clone());
                if let Some(sha) = &item.target.sha
                    && let Err(error) = sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                        .bind(sha).bind(&item.info).execute(&state.db).await {
                    warning_details.push(format!("{id}：媒体信息 JSON 已保存，缓存更新失败：{error}"));
                }
            }
        }
        cleanup_stages(&restore).await;
        if written_ids.is_empty() {
            return failed("媒体信息 JSON 未能保存，未通知 Emby 刷新", &failures, 500);
        }
        saved.retain(|id| written_ids.contains(id));
        if saved.is_empty() {
            return failed(
                "编辑目标的媒体信息 JSON 未能保存，已保留关联版本文件",
                &failures,
                500,
            );
        }
        let context = context.ok_or_else(|| ApiError::bad("保存任务上下文缺失"))?;
        context.checkpoint().await?;
        context
            .activity("通知Emby刷新", "媒体信息 JSON 已保存，通知 Emby 重新读取")
            .await;
        let paths = written_ids
            .iter()
            .map(|id| restore[id].path.clone())
            .collect::<Vec<_>>();
        let (requested_ids, refresh_failures) = refresh_report(state, &paths).await;
        let metadata_state = if refresh_failures.is_empty() {
            "submitted"
        } else if requested_ids.is_empty() {
            "failed"
        } else {
            "partial"
        };
        let failed_ids = saved
            .iter()
            .filter(|id| {
                refresh_failures.iter().any(|failure| {
                    std::path::Path::new(&restore[*id].path)
                        .parent()
                        .is_some_and(|parent| failure["path"] == parent.to_string_lossy().as_ref())
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        for failure in &refresh_failures {
            warning_details.push(format!(
                "媒体信息 JSON 已保存，通知 Emby 刷新失败：{}：{}",
                string(failure, "path"),
                string(failure, "reason")
            ));
        }
        let video_thumbnails = thumbnails_after_save(state, &restore, &saved, context).await?;
        if video_thumbnails["failed"].as_u64().unwrap_or(0) > 0 {
            let reason = video_thumbnails["reason"]
                .as_str()
                .or_else(|| video_thumbnails["message"].as_str())
                .unwrap_or("仍有目标未通过图片验收");
            warning_details.push(format!(
                "媒体信息 JSON 已保存，视频缩略图处理未完成：{reason}"
            ));
        }
        let skipped_tracks: usize = reports
            .iter()
            .map(|v| v["skipped"].as_array().map_or(0, Vec::len))
            .sum();
        let warnings = skipped_tracks + warning_details.len();
        let message = match metadata_state {
            "submitted" => "媒体信息 JSON 已保存，已通知 Emby 在后台重新读取",
            "partial" => "媒体信息 JSON 已保存，部分 Emby 刷新通知失败，请查看提醒",
            _ => "媒体信息 JSON 已保存，Emby 刷新通知失败，请查看提醒",
        };
        return Ok((
            if failures.is_empty() { 200 } else { 207 },
            json!({
                "message":message,"saved_item_ids":saved,"media_info_saved":true,
                "failures":failures,"batch_reports":reports,"warnings":warnings,"warning_details":warning_details,
                "metadata_refresh":{"state":metadata_state,"requested_ids":requested_ids,"failed_ids":failed_ids,"failures":refresh_failures},
                "video_thumbnails":video_thumbnails
            }),
        ));
    }
    let targets = restore
        .values()
        .map(|item| (item.target.id.clone(), item.path.clone()))
        .collect::<Vec<_>>();
    let settings =
        restoration_settings(state, &targets, restore.values().any(|item| item.multipart)).await?;
    let result = stage_all(state, &mut restore, &order, true).await;
    if let Err(error) = result {
        cleanup_stages(&restore).await;
        return Err(error);
    }
    if let Some(context) = context
        && let Err(error) = context.checkpoint().await
    {
        cleanup_stages(&restore).await;
        return Err(error);
    }
    let mut ordered_restore = restore.iter().collect::<Vec<_>>();
    ordered_restore.sort_by_cached_key(|(id, _)| order.get(*id).cloned());
    let outcome = crate::strm::with_changes(state, &settings, || async {
    if let Some(context) = context {
        context.update(50, "清理缓存", "清理旧媒体信息缓存").await;
    }
    // Once clear starts, finish file/group restoration even if cancellation is requested.
    let mut ungrouped = vec![];
    let mut operation_failures = vec![];
    let mut clear_attempted = false;
    for group in &groups {
        let multipart = group
            .iter()
            .any(|id| restore.get(id).is_some_and(|p| p.multipart));
        if multipart && group.len() > 1 {
            if let Err(error) = emby_post(
                state,
                &format!("Videos/{}/AlternateSources/Delete", group[0]),
                &[],
            )
            .await
            {
                operation_failures.push(error.1);
                break;
            }
            ungrouped.push(group.clone());
        }
        // ETK bindings may contain versions that Emby has not merged. Clear each
        // prepared item instead of assuming the first ID clears the entire group.
        for id in group {
            if restore.get(id).is_some_and(|item| !item.has_streams) {
                tracing::info!(
                    "[媒体信息保存] 媒体ID {id}：当前没有音视频流，保留现有备份直接恢复，不重复清理"
                );
                continue;
            }
            clear_attempted = true;
            if let Err(error) = emby_post(state, &format!("Items/{id}/ClearMediaInfo"), &[]).await {
                operation_failures.push(format!("{id}: {}", error.1));
                break;
            }
        }
        if !operation_failures.is_empty() {
            break;
        }
    }
    if clear_attempted {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    if let Some(context) = context {
        context
            .update(65, "恢复媒体信息", "恢复全部受影响版本和分段 JSON")
            .await;
    }
    for (id, item) in &ordered_restore {
        if let Some(stage) = &item.stage {
            if let Err(error) = tokio::fs::rename(stage, &item.final_path).await {
                operation_failures.push(format!("{id}: 无法恢复 JSON: {error}"));
                continue;
            }
            match tokio::fs::read(&item.final_path).await {
                Ok(bytes)
                    if serde_json::from_slice::<Value>(&bytes)
                        .ok()
                        .is_some_and(|v| media_info::recoverable(&v)) =>
                {
                    tracing::info!(
                        "[媒体信息保存] 媒体ID {id}：JSON 已恢复到 {}，文件大小 {} 字节，时长 {}，保护章节 {} 个",
                        item.final_path.display(),
                        bytes.len(),
                        media_info::container(&item.info).unwrap()["RunTimeTicks"],
                        item.chapters.as_array().unwrap().len()
                    );
                }
                _ => operation_failures.push(format!("{id}: 恢复的 JSON 无效")),
            }
        }
    }
    for group in ungrouped {
        if let Err(error) =
            emby_post(state, "Videos/MergeVersions", &[("Ids", group.join(","))]).await
        {
            operation_failures.push(format!("恢复版本组 {} 失败: {}", group.join(","), error.1));
        }
    }
    for (_, item) in &ordered_restore {
        let fields = media_display::fields(&item.target.path);
        if let Err(error)=sqlx::query("UPDATE media_metadata SET asset_details_json=(SELECT jsonb_agg(CASE WHEN a->>'path'=$1 THEN a||$2::jsonb ELSE a END ORDER BY idx) FROM jsonb_array_elements(asset_details_json) WITH ORDINALITY entries(a,idx)),last_updated_at=NOW() WHERE jsonb_typeof(asset_details_json)='array' AND asset_details_json @> $3::jsonb")
            .bind(&item.target.path).bind(&fields).bind(json!([{"path":item.target.path}])).execute(&state.db).await {
            operation_failures.push(format!("更新 {} 展示字段失败: {error}",item.target.id));
        }
    }
    // The plugin's restore task imports streams, titles and chapters from JSON.
    // A recursive metadata refresh and SyncMediaInfo do not provide this contract.
    tracing::info!("[媒体信息恢复] JSON 已就绪，使用神医恢复模式，媒体库范围 {}，目标媒体 {} 个", settings["LibraryScope"], ordered_restore.len());
    if let Some(context) = context {
        context.update(75, "恢复媒体信息", "等待神医原生任务从 JSON 恢复媒体流、标题和章节").await;
    }
    let recovery_context = JobContext::for_recovery(context).await;
    crate::emby::scheduled(state, "MediaInfoExtractTask", &recovery_context).await?;
    Ok(operation_failures)
    }).await;
    cleanup_stages(&restore).await;
    let operation_failures = outcome?;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    if let Some(context) = context {
        context
            .update(90, "验收", "等待 Emby 回读实际媒体流、标题和章节标记")
            .await;
    }
    let mut verification_failures = vec![];
    let mut heartbeat = tokio::time::Instant::now();
    let mut diagnostics_due = true;
    loop {
        if let Some(context) = context {
            let pause = tokio::time::Instant::now();
            context.checkpoint().await?;
            deadline += pause.elapsed();
        }
        let probe = async {
            let mut failures = vec![];
            for (id, item) in &ordered_restore {
                match media_edit::details(
                    state,
                    id,
                    "Id,Path,Type,Chapters,MediaSources,MediaStreams",
                )
                .await
                {
                    Ok(details) => {
                        if diagnostics_due {
                            let file = tokio::fs::metadata(&item.final_path)
                                .await
                                .map(|metadata| format!("{} 字节", metadata.len()))
                                .unwrap_or_else(|_| "文件缺失或不可读".into());
                            tracing::info!(
                                "[媒体信息恢复回读] 媒体ID {id}：{}；恢复 JSON：{file}",
                                media_info::readback_summary(&details)
                            );
                        }
                        let source = crate::media_paths::strm_target(&item.path)
                            .await
                            .unwrap_or_default();
                        let paths = [&item.path[..], &source[..]];
                        if details["Id"] != item.target.id || string(&details, "Path") != item.path
                        {
                            failures.push(format!("{id}: Emby媒体身份或路径在恢复期间变化"));
                        }
                        failures.extend(
                            media_info::stream_differences(&item.info, &details, &paths)
                                .into_iter()
                                .map(|reason| format!("{id}: {reason}")),
                        );
                        let empty = json!([]);
                        let chapters =
                            if details["Type"] == "Audio" && details["Chapters"].is_null() {
                                &empty
                            } else {
                                &details["Chapters"]
                            };
                        if !media_info::chapters_restored(&item.chapters, chapters) {
                            failures.push(format!("{id}: 章节或片头片尾标记缺失、变化或无法读取"));
                        }
                    }
                    Err(error) => failures.push(format!("{id}: 回读失败：{}", error.1)),
                }
            }
            failures
        };
        let probe = tokio::time::timeout_at(deadline, probe);
        let outcome = if let Some(context) = context {
            tokio::select! {
                _ = context.cancelled() => { context.checkpoint().await?; unreachable!() },
                result = probe => result,
            }
        } else {
            probe.await
        };
        if let Some(context) = context {
            let pause = tokio::time::Instant::now();
            context.checkpoint().await?;
            deadline += pause.elapsed();
        }
        diagnostics_due = false;
        match outcome {
            Ok(failures) => verification_failures = failures,
            Err(_) => {
                if verification_failures.is_empty() {
                    verification_failures.push("Emby 媒体信息回读请求超时，尚未确认恢复".into());
                }
                break;
            }
        }
        if verification_failures.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if heartbeat.elapsed() >= Duration::from_secs(15) {
            tracing::info!(
                "[媒体信息保存] 仍在等待 Emby 恢复：{}",
                verification_failures.join("；")
            );
            heartbeat = tokio::time::Instant::now();
            diagnostics_due = true;
        }
        let delay = std::cmp::min(
            Duration::from_secs(1),
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        );
        if let Some(context) = context {
            tokio::select! {
                _ = context.cancelled() => { context.checkpoint().await?; },
                _ = tokio::time::sleep(delay) => {},
            }
        } else {
            tokio::time::sleep(delay).await;
        }
    }
    if !verification_failures.is_empty() {
        return failed(
            "媒体信息文件已恢复，但 120 秒内未确认 Emby 媒体流、标题和章节标记一致",
            &verification_failures,
            500,
        );
    }
    if !operation_failures.is_empty() {
        return failed(
            "媒体信息文件已恢复，但清理、版本恢复或刷新收尾未完整成功",
            &operation_failures,
            500,
        );
    }
    let skipped_tracks: usize = reports
        .iter()
        .map(|v| v["skipped"].as_array().map_or(0, Vec::len))
        .sum();
    tracing::info!(
        item_id = id,
        restored = restore.len(),
        failures = failures.len(),
        skipped_tracks,
        "[媒体信息保存] JSON、媒体流和章节回读验收结束"
    );
    Ok((
        if failures.is_empty() { 200 } else { 207 },
        json!({"message":format!("媒体信息已更新，已恢复 {} 个版本 JSON，Emby 媒体流、标题和章节已通过回读验收",restore.len()),"failures":failures,"batch_reports":reports,"warnings":skipped_tracks,"warning_details":[],"metadata_refresh":{"state":"not_requested","requested_ids":[],"failed_ids":[]},"video_thumbnails":{"state":"not_requested"}}),
    ))
}

async fn thumbnails_after_save(
    state: &AppState,
    restore: &BTreeMap<String, Prepared>,
    saved: &[String],
    context: &JobContext,
) -> Result<Value> {
    if state.config.read().await["extract_video_thumbnails_after_save"] != true {
        return Ok(json!({"state":"disabled"}));
    }
    let saved_videos = saved
        .iter()
        .filter_map(|id| {
            let item = restore.get(id)?;
            media_info::container(&item.info)?["MediaStreams"]
                .as_array()?
                .iter()
                .any(|stream| stream["Type"] == "Video")
                .then(|| (id.clone(), item.path.clone()))
        })
        .collect::<Vec<_>>();
    if saved_videos.is_empty() {
        return Ok(json!({"state":"not_requested"}));
    }
    context.checkpoint().await?;
    context
        .activity(
            "视频缩略图提取",
            "媒体信息 JSON 已保存，运行神医视频缩略图提取和验收流程",
        )
        .await;
    match crate::thumbnails::repair_saved(state, &saved_videos, context).await {
        Ok(report) => Ok(report),
        Err(error) if context.is_cancelled() => Err(error),
        Err(error) => Ok(json!({"state":"failed","failed":1,"reason":error.1})),
    }
}
fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}
