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
    final_path: PathBuf,
    stage: Option<PathBuf>,
    chapters: Value,
    multipart: bool,
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
    let item = media_edit::details(state, &target.id, "Path,PartCount,Chapters,Type").await?;
    let path = string(&item, "Path");
    if path.is_empty() {
        return Err(ApiError::bad(format!(
            "{}: 无法读取 Emby 原生 Path",
            target.id
        )));
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
    let empty = json!([]);
    let chapters = if item["Type"] == "Audio" && item["Chapters"].is_null() {
        &empty
    } else {
        &item["Chapters"]
    };
    let chapters = media_info::clean_chapters(chapters).map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            format!("{}: 无法保护当前章节和片头片尾标记", target.id),
        )
    })?;
    media_info::preserve_chapters(&mut info, &chapters)?;
    Ok(Prepared {
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
        let current =
            media_edit::details(state, &item.target.id, "Path,Size,MediaSources,Chapters").await?;
        if string(&current, "Path") != item.path {
            return Err(ApiError::bad("Emby路径在准备保存期间变化，未清理Emby数据"));
        }
        identity::verify(
            &serde_json::json!({"size_bytes":current["Size"]}),
            &item.info,
        )?;
        let empty = json!([]);
        item.chapters = media_info::clean_chapters(
            if current["Type"] == "Audio" && current["Chapters"].is_null() {
                &empty
            } else {
                &current["Chapters"]
            },
        )?;
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
        if let Some(sha) = &item.target.sha {
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
    for mut directory in directories {
        let mut refreshed = false;
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
                    return Ok(false);
                }
                let response: Value = response.json().await?;
                if let Some(item) = response["Items"].as_array().and_then(|items| items.first()) {
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
                    .await?;
                    tracing::info!(
                        "[精准扫描] 已通知 Emby 对 {} 立即扫描！",
                        item["Name"].as_str().unwrap_or(&path)
                    );
                    return Ok(true);
                }
                Ok(false)
            }
            .await;
            if matches!(attempt, Ok(true)) {
                refreshed = true;
                break;
            }
            if !directory.pop() {
                break;
            }
        }
        if !refreshed {
            tracing::warn!(
                "[精准扫描] 未能在 Emby 中找到有效父目录，将等待自动扫描；媒体信息文件已保留。"
            );
        }
    }
    Ok(())
}
pub async fn save(
    state: &AppState,
    id: &str,
    body: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    save_with_rebuilt(state, id, body, &BTreeMap::new(), context).await
}
pub async fn save_with_rebuilt(
    state: &AppState,
    id: &str,
    body: &Value,
    rebuilt: &BTreeMap<String, Value>,
    context: &JobContext,
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
        context.checkpoint().await?;
        context
            .update(
                5 + 20 * index as u64 / ids.len() as u64,
                "核对媒体信息",
                &format!("核对媒体信息 {}/{}", index + 1, ids.len()),
            )
            .await;
        let result=async {
            let target=if rebuilt.is_empty(){media_edit::target(state,target_id).await?}else{media_edit::repair_target(state,target_id).await?};
            let original=if let Some(info)=rebuilt.get(target_id){info.clone()}else{media_edit::load(state,&target).await?};
            if target_id==id {
                if let Some(sha)=body.get("sha1") && text(sha).to_uppercase()!=target.sha.clone().unwrap_or_default().to_uppercase() {return Err(ApiError::bad("当前文件指纹已变化，请重新打开编辑器"));}
                if body["media_path"].as_str().is_some_and(|p|!p.is_empty() && p!=target.path) {return Err(ApiError::bad("当前文件路径已变化，请重新打开编辑器"));}
            }
            identity::verify(&target.asset,if target_id==id {&body["mediainfo"]} else {&original})?;
            let info=if let Some(info)=rebuilt.get(target_id){info.clone()}
                else if target_id==id {body["mediainfo"].clone()}
                else {
                    let (info,report)=media_info::apply_patches(&original,&body["stream_patches"]);
                    reports.push(json!({"item_id":target_id,"path":target.path,"matched":report["matched"],"skipped":report["skipped"]}));
                    if report["matched"]==0 {return Ok(None);}
                    info
                };
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
        context.checkpoint().await?;
        let prepared = prepare(state, target, info).await?;
        if prepared.multipart {
            let details = media_edit::details(state, &id, "MediaSources").await?;
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
                let source = part["MediaSources"][0]["Path"]
                    .as_str()
                    .or_else(|| part["Path"].as_str())
                    .unwrap_or("");
                let target = media_edit::build(state, &part_id, source, None, None).await?;
                let info = media_edit::load(state, &target).await?;
                restore.insert(part_id, prepare(state, target, info).await?);
            }
        }
        restore.insert(id, prepared);
    }
    context.update(35, "暂存", "暂存媒体信息和当前章节").await;
    let result = stage_all(state, &mut restore, &order).await;
    if let Err(error) = result {
        cleanup_stages(&restore).await;
        return Err(error);
    }
    if let Err(error) = context.checkpoint().await {
        cleanup_stages(&restore).await;
        return Err(error);
    }
    context.update(50, "清理缓存", "清理旧媒体信息缓存").await;
    // Once clear starts, finish file/group restoration even if cancellation is requested.
    let mut ungrouped = vec![];
    let mut operation_failures = vec![];
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
        let clear = if multipart {
            group.as_slice()
        } else {
            &group[..1]
        };
        for id in clear {
            if let Err(error) = emby_post(state, &format!("Items/{id}/ClearMediaInfo"), &[]).await {
                operation_failures.push(format!("{id}: {}", error.1));
                break;
            }
        }
        if !operation_failures.is_empty() {
            break;
        }
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    context
        .update(65, "恢复媒体信息", "恢复全部受影响版本和分段 JSON")
        .await;
    let mut ordered_restore = restore.iter().collect::<Vec<_>>();
    ordered_restore.sort_by_cached_key(|(id, _)| order.get(*id).cloned());
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
                        .is_some_and(|v| media_info::recoverable(&v)) => {}
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
    let paths = restore.values().map(|p| p.path.clone()).collect::<Vec<_>>();
    refresh(state, &paths).await?;
    context
        .update(90, "验收", "等待 Emby 回读媒体流和章节标记")
        .await;
    let mut verification_failures = vec![];
    for attempt in 0..3 {
        verification_failures.clear();
        for (id, item) in &ordered_restore {
            match media_edit::details(state, id, "Type,Chapters,MediaSources,MediaStreams").await {
                Ok(details) => {
                    let has_streams = details["MediaStreams"]
                        .as_array()
                        .is_some_and(|s| !s.is_empty())
                        || details["MediaSources"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|s| s["MediaStreams"].as_array().is_some_and(|s| !s.is_empty()));
                    if !has_streams {
                        verification_failures.push(format!("{id}: Emby 媒体流为空"));
                    }
                    let empty = json!([]);
                    let chapters = if details["Type"] == "Audio" && details["Chapters"].is_null() {
                        &empty
                    } else {
                        &details["Chapters"]
                    };
                    if !media_info::chapters_restored(&item.chapters, chapters) {
                        verification_failures
                            .push(format!("{id}: 章节或片头片尾标记缺失、变化或无法读取"));
                    }
                }
                Err(error) => verification_failures.push(format!("{id}: 回读失败: {}", error.1)),
            }
        }
        if verification_failures.is_empty() {
            break;
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            refresh(state, &paths).await?;
        }
    }
    if !verification_failures.is_empty() {
        return failed(
            "媒体信息文件已恢复，但 Emby 媒体流或章节标记未完整恢复",
            &verification_failures,
            500,
        );
    }
    if !operation_failures.is_empty() {
        return failed(
            "媒体信息文件已恢复，但 Emby 版本操作未完整成功",
            &operation_failures,
            500,
        );
    }
    let warnings: usize = reports
        .iter()
        .map(|v| v["skipped"].as_array().map_or(0, Vec::len))
        .sum();
    tracing::info!(
        item_id = id,
        restored = restore.len(),
        failures = failures.len(),
        skipped_tracks = warnings,
        "[媒体信息保存] JSON、媒体流和章节回读验收结束"
    );
    Ok((
        if failures.is_empty() { 200 } else { 207 },
        json!({"message":format!("媒体信息已更新，已恢复 {} 个版本 JSON，Emby 正在重新扫描",restore.len()),"failures":failures,"batch_reports":reports,"warnings":warnings}),
    ))
}
fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}
