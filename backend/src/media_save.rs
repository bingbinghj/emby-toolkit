use crate::{
    AppState,
    error::{ApiError, Result},
    identity,
    jobs::{JobContext, JobSpec},
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
use serde::{Deserialize, Serialize};
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
        .submit_persistent_response(
            "保存媒体信息",
            JobSpec::new("media_save", json!({"id":id,"body":body})),
            move |context| async move { save(&state, &id, &body, &context).await },
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"保存媒体信息已提交，可在通知中心查看实时日志"})),
    ))
}
pub(crate) async fn execute_persisted(
    state: &AppState,
    payload: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    save(state, &string(payload, "id"), &payload["body"], context).await
}
#[derive(Serialize, Deserialize)]
struct Prepared {
    target: Target,
    info: Value,
    path: String,
    source: Option<String>,
    final_path: PathBuf,
    stage: Option<PathBuf>,
    expected_json: Option<Vec<u8>>,
    chapters: Value,
    multipart: bool,
    has_streams: bool,
    preserve_streams: bool,
    #[serde(default)]
    observation: Value,
}
#[derive(Serialize, Deserialize)]
struct SavePreparation {
    restore: BTreeMap<String, Prepared>,
    groups: BTreeSet<Vec<String>>,
    failures: Vec<String>,
    reports: Vec<Value>,
    saved: Vec<String>,
}
fn failed(message: &str, failures: &[String], status: u16) -> Result<(u16, Value)> {
    Ok((status, json!({"error":message,"failures":failures})))
}
pub(crate) fn protected(state_config: &Value, path: &str) -> bool {
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

fn validate_requested_chapters(info: &Value, original: &Value) -> Result<()> {
    let requested = media_info::restore_json(info)?;
    let original = media_info::restore_json(original)?;
    let original_root = media_info::object(&original).unwrap();
    let original_source = media_info::container(&original).unwrap();
    let empty = json!([]);
    let expected = if original_root["Chapters"].is_null() {
        &original_source["Chapters"]
    } else {
        &original_root["Chapters"]
    };
    let expected = media_info::signature(if expected.is_null() { &empty } else { expected })?;
    let requested_root = media_info::object(&requested).unwrap();
    let requested_source = media_info::container(&requested).unwrap();
    for chapters in [&requested_root["Chapters"], &requested_source["Chapters"]] {
        if chapters.is_null() {
            continue;
        }
        let actual = media_info::signature(chapters)?;
        // Empty chapters from an old editing page remain recoverable. A
        // nonempty change must never be silently overwritten as a success.
        if !actual.is_empty() && actual != expected {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "媒体信息编辑不支持编辑章节，章节内容与原记录不一致，已停止保存".into(),
            ));
        }
    }
    Ok(())
}
async fn prepare(
    state: &AppState,
    target: Target,
    mut info: Value,
    preserve_streams: bool,
) -> Result<Prepared> {
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
    let expected_json = match tokio::fs::read(&final_path).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let source = crate::media_paths::strm_target(&path).await;
    let paths = [&path[..], source.as_deref().unwrap_or("")];
    let has_streams = media_info::has_media_streams(&item, &paths);
    if preserve_streams {
        media_info::preserve_live_streams(&mut info, &item, &paths)?;
    }
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
        preserve_streams,
        target,
        info,
        path,
        final_path,
        stage: None,
        expected_json,
        chapters,
        multipart: item["PartCount"].as_u64().unwrap_or(0) > 1,
        observation: item,
    })
}
async fn stage_all(
    state: &AppState,
    prepared: &mut BTreeMap<String, Prepared>,
    order: &std::collections::HashMap<String, crate::media_order::Key>,
) -> Result<()> {
    // All identity, stream and chapter reads finish before any persistent write or cache clear.
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
            "Id,Path,Size,Type,ParentId,PartCount,ExtraType,MediaSources,MediaStreams,Chapters",
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
        if item.preserve_streams {
            media_info::preserve_live_streams(&mut item.info, &current, &paths)?;
        }
        item.chapters = media_info::recovery_chapters(&item.info, &current, &paths)?;
        media_info::preserve_chapters(&mut item.info, &item.chapters)?;
        identity::verify(&item.target.asset, &item.info)?;
        item.observation = current;
        let filename = item
            .final_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ApiError::bad("媒体信息目标文件名无效"))?;
        let directory = state
            .data_dir
            .join("backups/media-info-reload")
            .join(format!("staging-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&directory).await?;
        let path = directory.join(filename);
        item.stage = Some(path.clone());
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        file.write_all(
            &serde_json::to_vec_pretty(&item.info)
                .map_err(|_| ApiError::bad("媒体信息 JSON 无效"))?,
        )
        .await?;
        file.sync_all().await?;
    }
    Ok(())
}
pub(crate) async fn commit_json_stage(
    stage: &std::path::Path,
    destination: &std::path::Path,
) -> Result<()> {
    commit_json_stage_with_validation(stage, destination, || async { Ok(()) }).await
}

pub(crate) async fn commit_json_stage_guarded(
    stage: &std::path::Path,
    destination: &std::path::Path,
    expected: Option<&[u8]>,
) -> Result<()> {
    validate_json_destination(destination, expected).await?;
    commit_json_stage_with_validation(stage, destination, || {
        validate_json_destination(destination, expected)
    })
    .await
}

async fn validate_json_destination(
    destination: &std::path::Path,
    expected: Option<&[u8]>,
) -> Result<()> {
    let changed = || {
        ApiError(
            StatusCode::CONFLICT,
            format!(
                "媒体 JSON 在提交期间再次变化，保留当前文件及全部备份：{}",
                destination.display()
            ),
        )
    };
    for parent in destination
        .parent()
        .into_iter()
        .flat_map(std::path::Path::ancestors)
    {
        if !tokio::fs::symlink_metadata(parent)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return Err(changed());
        }
    }
    let current = match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) if metadata.is_file() => Some(tokio::fs::read(destination).await?),
        Ok(_) => return Err(changed()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if current.as_deref() != expected {
        return Err(changed());
    }
    if current.is_some()
        && !tokio::fs::symlink_metadata(destination)
            .await
            .is_ok_and(|metadata| metadata.is_file())
    {
        return Err(changed());
    }
    Ok(())
}

async fn commit_json_stage_with_validation<F, Fut>(
    stage: &std::path::Path,
    destination: &std::path::Path,
    validate: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let parent = destination
        .parent()
        .ok_or_else(|| ApiError::bad("媒体信息目标目录无效"))?;
    let commit = parent.join(format!(".etk-commit-{}", uuid::Uuid::new_v4().simple()));
    let outcome = async {
        tokio::fs::copy(stage, &commit).await?;
        tokio::fs::File::open(&commit).await?.sync_all().await?;
        validate().await?;
        tokio::fs::rename(&commit, destination).await?;
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(commit).await;
    outcome
}
pub(crate) async fn cache_saved(state: &AppState, sha: Option<&str>, info: &Value) -> Result<()> {
    if let Some(sha) = sha {
        let stored: Value = sqlx::query_scalar("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json RETURNING mediainfo_json")
            .bind(sha).bind(info).fetch_one(&state.db).await?;
        if stored != *info {
            return Err(ApiError::bad("媒体信息缓存同步后内容不一致"));
        }
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedTarget {
    pub id: String,
    pub path: String,
    pub source: Option<String>,
    pub sha: Option<String>,
    pub asset: Option<Value>,
    pub info: Value,
    pub refresh_required: bool,
}
pub(crate) async fn saved_details(state: &AppState, target: &SavedTarget) -> Result<Value> {
    if protected(&*state.config.read().await, &target.path) {
        return Err(ApiError::bad(
            "原保存目标现已位于 115 源目录，已停止恢复写入",
        ));
    }
    if let Some(asset) = &target.asset {
        let current = media_edit::repair_target(state, &target.id).await?;
        if current.path != target.path || current.sha != target.sha || current.asset != *asset {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "保存后媒体资产身份变化，已停止刷新验收".into(),
            ));
        }
    }
    let live = media_edit::details(
        state,
        &target.id,
        "Id,Path,Type,Size,Chapters,MediaSources,MediaStreams",
    )
    .await?;
    if live["Id"] != target.id
        || live["Path"] != target.path
        || crate::media_paths::strm_target(&target.path).await != target.source
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "保存后媒体路径或STRM来源变化，已停止刷新验收".into(),
        ));
    }
    identity::verify(&json!({"size_bytes":live["Size"]}), &target.info)
        .map_err(|error| ApiError(StatusCode::CONFLICT, error.1))?;
    Ok(live)
}

pub(crate) async fn reload_filled_titles(
    state: &AppState,
    ids: &[String],
    skip_chinese: bool,
    context: &JobContext,
    confirmed: &std::sync::atomic::AtomicBool,
    recovery: &std::path::Path,
) -> Result<Vec<String>> {
    let mut pending = ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut visited = BTreeSet::new();
    let mut saved = BTreeMap::new();
    let mappings = crate::db::setting(&state.db, "language_mapping")
        .await?
        .unwrap_or(json!([]));
    while let Some(id) = pending.pop_first() {
        if !visited.insert(id.clone()) {
            continue;
        }
        context.checkpoint().await?;
        let target = media_edit::repair_target(state, &id).await?;
        if crate::emby::path_ignored(state, &target.path).await? {
            return Err(ApiError::bad("关联版本位于忽略媒体库，未清理媒体信息"));
        }
        let source = crate::media_paths::strm_target(&target.path).await;
        let live = media_edit::details(
            state,
            &id,
            "Id,Path,Type,Size,ExtraType,PartCount,Chapters,MediaSources,MediaStreams",
        )
        .await?;
        if ids.contains(&id)
            && (!string(&live, "ExtraType").is_empty()
                || live["PartCount"].as_u64().unwrap_or(0) > 1)
        {
            continue;
        }
        pending.extend(
            target
                .versions
                .iter()
                .map(|v| string(v, "emby_id"))
                .filter(|v| !v.is_empty()),
        );
        let mut info = media_info::restore_json(&media_edit::load(state, &target).await?)?;
        let paths = [target.path.as_str(), source.as_deref().unwrap_or("")];
        if ids.contains(&id) {
            let expected = crate::titles::expected_titles(
                &info,
                &crate::language::fill(&info, &mappings, skip_chinese).1,
            );
            if media_info::has_media_streams(&live, &paths) {
                let original = media_info::container(&info).unwrap()["MediaStreams"]
                    .as_array()
                    .unwrap();
                let current = media_info::item_streams(&live, &paths)
                    .ok_or_else(|| ApiError::bad("无法唯一确认当前媒体流，未清理媒体信息"))?;
                if ["Video", "Audio", "Subtitle"].iter().any(|kind| {
                    original.iter().filter(|s| s["Type"] == *kind).count()
                        != current.iter().filter(|s| s["Type"] == *kind).count()
                }) {
                    return Err(ApiError::bad("填充期间轨道范围变化，未清理媒体信息"));
                }
            }
            media_info::preserve_live_streams(&mut info, &live, &paths)?;
            for entry in expected {
                let stream = media_info::streams_mut(&mut info)
                    .unwrap()
                    .iter_mut()
                    .filter(|s| s["Type"] == entry["type"])
                    .nth(entry["ordinal"].as_u64().unwrap() as usize - 1)
                    .ok_or_else(|| ApiError::bad("填充期间轨道范围变化，未清理媒体信息"))?;
                if entry["codec"].as_str().is_some_and(|codec| {
                    !stream["Codec"]
                        .as_str()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(codec)
                }) {
                    return Err(ApiError::bad("填充期间轨道编码变化，未清理媒体信息"));
                }
                stream["Title"] = entry["title"].clone();
                if entry["check_language"] == true {
                    stream["Language"] = entry["language"].clone();
                }
            }
        } else {
            media_info::preserve_live_streams(&mut info, &live, &paths)?;
        }
        let chapters = media_info::recovery_chapters(&info, &live, &paths)?;
        media_info::preserve_chapters(&mut info, &chapters)?;
        if live["PartCount"].as_u64().unwrap_or(0) > 1 {
            pending.extend(
                media_edit::additional_parts(state, &id, &live)
                    .await?
                    .iter()
                    .map(|part| string(part, "Id"))
                    .filter(|v| !v.is_empty()),
            );
        }
        let prepared = SavedTarget {
            id: id.clone(), path: target.path, source, sha: target.sha,
            asset: (!target.kind.is_empty()).then_some(target.asset), info,
            refresh_required: true,
        };
        let paths = [prepared.path.as_str(), prepared.source.as_deref().unwrap_or("")];
        let unchanged = crate::media_reload::differences(&prepared,&live).is_empty()
            && media_info::container(&prepared.info).zip(media_info::item_source(&live,&paths))
                .is_some_and(|(expected,actual)| ["Size","RunTimeTicks"].iter().all(|field|
                    expected[*field].is_null() || crate::identity::size(&expected[*field])==crate::identity::size(&actual[*field])));
        if !unchanged { saved.insert(id,prepared); }
    }
    if saved.is_empty() { return Ok(vec![]); }
    crate::media_reload::reload(
        state,
        &saved.into_values().collect::<Vec<_>>(),
        context,
        confirmed,
        recovery,
    )
    .await
}
pub(crate) async fn refresh_saved<F, Fut>(
    state: &AppState,
    context: &JobContext,
    prepare: F,
) -> Result<Value>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<SavedTarget>>>,
{
    use std::sync::atomic::{AtomicBool, Ordering};
    let confirmed = AtomicBool::new(true);
    let root = state.data_dir.join("backups/media-info-reload");
    let recovery = if let Some(original) = context.plan("saved-refresh-directory").await? {
        let directory: PathBuf =
            serde_json::from_value(original).map_err(|_| ApiError::bad("原保存恢复目录无效"))?;
        if directory.parent() != Some(root.as_path())
            || !directory
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("saved-"))
        {
            return Err(ApiError::bad("原保存恢复目录不在固定备份根目录内"));
        }
        directory
    } else {
        let directory = root.join(format!("saved-{}", uuid::Uuid::new_v4()));
        context
            .save_plan("saved-refresh-directory", &json!(directory))
            .await?;
        directory
    };
    if let Some(previous) = context.completed("saved-refresh-report").await? {
        let mut report = previous["report"].clone();
        if previous["cleanup_allowed"] == true {
            let _ = cleanup_recovery(&recovery, &mut report).await;
        }
        return Ok(report);
    }
    let recovering = context.scoped("saved-refresh-recovery");
    let pending = tokio::fs::try_exists(recovery.join("media-reload.json")).await?
        || tokio::fs::try_exists(recovery.join("pending-scan.json")).await?
        || recovering
            .completed("media_recovery_verified")
            .await?
            .is_some();
    if pending {
        let id = format!("media:{}", recovery.file_name().unwrap().to_string_lossy());
        crate::media_recovery::execute_persisted(
            state,
            &json!({"recovery_id":id,"current_json_hashes":{}}),
            &recovering,
        )
        .await?;
        context
            .complete("saved-refresh-native-import", &json!([]))
            .await?;
    } else if tokio::fs::try_exists(recovery.join("original-settings.json")).await? {
        restore_saved_settings(state, &recovery).await?;
    }
    let imported = context.completed("saved-refresh-native-import").await?;
    let original_targets = context.plan("saved-refresh-targets").await?;
    let mut report = json!({"state":"not_requested","requested_ids":[],"verified_ids":[],"failed_ids":[],"failures":[],"warning_details":[]});
    let mut targets = vec![];
    let outcome = crate::strm::with_settings_guarded(state, "restore_media_info", || async {
        context.save_plan("saved-refresh-settings", &json!({
            "original":crate::media_recovery::read_json(&recovery.join("original-settings.json")).await?,
            "applied":crate::media_recovery::read_json(&recovery.join("applied-settings.json")).await?
        })).await?;
        targets = if let Some(previous) = original_targets {
            serde_json::from_value(previous).map_err(|_| ApiError::bad("原保存目标计划无效"))?
        } else {
            let targets = prepare().await?;
            context.save_plan("saved-refresh-targets", &json!(targets)).await?;
            targets
        };
        // Every JSON is already saved. Reload only the protected native versions.
        let mut reload = vec![];
        for target in &targets {
            context.checkpoint().await?;
            let live = saved_details(state, target).await?;
            if matches!(live["Type"].as_str(), Some("Series" | "Season")) {
                return Err(ApiError::bad("保存目标不是单个媒体版本，未提交递归刷新"));
            }
            if !target.refresh_required { continue; }
            report["requested_ids"].as_array_mut().unwrap().push(json!(target.id));
            context.save_plan(&format!("saved-formal-json:{}", target.id), &target.info).await?;
            reload.push(target.clone());
        }
        let warnings = if let Some(previous) = imported {
            previous
        } else {
            let warnings = json!(crate::media_reload::reload(state, &reload, context, &confirmed, &recovery).await?);
            context.complete("saved-refresh-native-import", &warnings).await?;
            warnings
        };
        report["warning_details"] = warnings;
        let mut pending = targets.iter().map(|target| (target.id.clone(), "尚未完成媒体流和章节回读".to_string())).collect::<BTreeMap<_, _>>();
        let mut deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !pending.is_empty() && tokio::time::Instant::now() < deadline {
            let paused = tokio::time::Instant::now();
            context.checkpoint().await?;
            deadline += paused.elapsed();
            for target in targets.iter().filter(|target| pending.contains_key(&target.id)).collect::<Vec<_>>() {
                let paused = tokio::time::Instant::now();
                context.checkpoint().await?;
                deadline += paused.elapsed();
                if tokio::time::Instant::now() >= deadline { break; }
                let readback = tokio::select! {
                    _ = context.cancelled() => { context.checkpoint().await?; unreachable!() },
                    result = tokio::time::timeout_at(deadline, saved_details(state, target)) => result,
                };
                let differences = match readback {
                    Ok(Ok(live)) => {
                        let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
                        let mut differences = media_info::stream_differences(&target.info, &live, &paths);
                        let chapters = &media_info::object(&target.info).unwrap()["Chapters"];
                        let actual = if live["Type"] == "Audio" && live["Chapters"].is_null() { json!([]) } else { live["Chapters"].clone() };
                        if !media_info::chapters_restored(chapters, &actual) {
                            differences.push("章节或片头片尾标记未一致".into());
                        }
                        differences
                    }
                    Ok(Err(error)) if error.0 == StatusCode::CONFLICT => return Err(error),
                    Ok(Err(error)) => vec![error.1],
                    Err(_) => vec!["30秒共享回读期限已耗尽，尚未确认媒体流和章节".into()],
                };
                if differences.is_empty() {
                    pending.remove(&target.id);
                    report["verified_ids"].as_array_mut().unwrap().push(json!(target.id));
                    tracing::info!("[媒体信息保存回读] 媒体ID {}：媒体流、标题和章节已通过验收", target.id);
                } else {
                    pending.insert(target.id.clone(), differences.join("；"));
                }
            }
            if !pending.is_empty() {
                let delay = Duration::from_secs(1).min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                let started = tokio::time::Instant::now();
                context.sleep(delay).await?;
                deadline += started.elapsed().saturating_sub(delay);
            }
        }
        report["state"] = json!(if pending.is_empty() { "verified" } else { "unverified" });
        for (id, reason) in pending {
            tracing::warn!("[媒体信息保存回读] 媒体ID {id}：未通过验收：{reason}");
            report["failed_ids"].as_array_mut().unwrap().push(json!(id));
            report["failures"].as_array_mut().unwrap().push(json!({"item_id":id,"reason":reason}));
        }
        Ok(())
    }, &confirmed, &recovery).await;
    let keep_recovery = outcome
        .as_ref()
        .is_err_and(|error| error.0 == StatusCode::CONFLICT && error.1 != "任务已取消");
    if let Err(error) = outcome {
        let ids = targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>()
            .join("、");
        tracing::warn!(
            "[媒体信息保存回读] 目标媒体ID [{ids}]：保存或刷新验收流程中断：{}",
            error.1
        );
        if context.is_cancelled() {
            if confirmed.load(Ordering::Acquire) && !keep_recovery {
                context
                    .complete("saved-refresh-settings-restored", &json!(true))
                    .await?;
            }
            if confirmed.load(Ordering::Acquire)
                && !keep_recovery
                && let Some(warning) = cleanup_recovery(&recovery, &mut report).await
            {
                return Err(ApiError(error.0, format!("{}；{warning}", error.1)));
            }
            return Err(error);
        }
        report["state"] = json!("failed");
        report["failed_ids"] = json!(
            targets
                .iter()
                .filter(|target| !media_edit::list(&report["verified_ids"])
                    .iter()
                    .any(|id| id == &target.id))
                .map(|target| target.id.clone())
                .collect::<Vec<_>>()
        );
        let failed_ids = media_edit::list(&report["failed_ids"]);
        if failed_ids.is_empty() {
            report["failures"]
                .as_array_mut()
                .unwrap()
                .push(json!({"reason":error.1}));
        } else {
            report["failures"].as_array_mut().unwrap().extend(
                failed_ids
                    .into_iter()
                    .map(|id| json!({"item_id":id,"reason":error.1})),
            );
        }
    }
    if confirmed.load(Ordering::Acquire) && !keep_recovery {
        context
            .complete(
                "saved-refresh-report",
                &json!({"report":report,"cleanup_allowed":true}),
            )
            .await?;
    }
    if confirmed.load(Ordering::Acquire) && !keep_recovery {
        let _ = cleanup_recovery(&recovery, &mut report).await;
    }
    Ok(report)
}
async fn restore_saved_settings(state: &AppState, recovery: &std::path::Path) -> Result<()> {
    let original =
        crate::media_recovery::read_json(&recovery.join("original-settings.json")).await?;
    let applied = crate::strm::recovery_applied_settings(recovery, &original).await?;
    crate::strm::finish_media_recovery(state, &original, applied.as_ref(), || async { Ok(()) })
        .await
}
fn legacy_identity(id: &str, item: &Prepared) -> Value {
    json!({"id":id,"path":item.path,"source":item.source,"sha":item.target.sha,"asset":item.target.asset})
}
async fn legacy_clear_candidates(
    restore: &BTreeMap<String, Prepared>,
    context: &JobContext,
) -> Result<BTreeSet<String>> {
    let allowed = restore.keys().cloned().collect::<BTreeSet<_>>();
    let mut started = BTreeSet::new();
    for (id, item) in restore {
        if let Some(proof) = context.plan(&format!("legacy-clear-intent:{id}")).await? {
            let affected: BTreeSet<String> = serde_json::from_value(proof["affected_ids"].clone())
                .map_err(|_| ApiError::bad("原清理目标范围无效"))?;
            if proof["target"] != legacy_identity(id, item)
                || affected.is_empty()
                || !affected.is_subset(&allowed)
            {
                return Err(ApiError::bad("原清理意图与保存范围不一致，未扩大恢复范围"));
            }
            started.extend(affected);
        }
    }
    Ok(started)
}
async fn legacy_current_empty(state: &AppState, item: &Prepared) -> Result<bool> {
    let target = SavedTarget {
        id: item.target.id.clone(),
        path: item.path.clone(),
        source: item.source.clone(),
        sha: item.target.sha.clone(),
        asset: (!item.target.kind.is_empty()).then(|| item.target.asset.clone()),
        info: item.info.clone(),
        refresh_required: true,
    };
    let live = saved_details(state, &target).await?;
    Ok(live["MediaStreams"].as_array().is_some_and(Vec::is_empty)
        && live["MediaSources"].as_array().is_some_and(|sources| {
            sources
                .iter()
                .all(|source| source["MediaStreams"].as_array().is_some_and(Vec::is_empty))
        }))
}
async fn legacy_current_restored(state: &AppState, item: &Prepared) -> Result<bool> {
    let target = SavedTarget {
        id: item.target.id.clone(),
        path: item.path.clone(),
        source: item.source.clone(),
        sha: item.target.sha.clone(),
        asset: (!item.target.kind.is_empty()).then(|| item.target.asset.clone()),
        info: item.info.clone(),
        refresh_required: true,
    };
    let live = saved_details(state, &target).await?;
    let paths = [item.path.as_str(), item.source.as_deref().unwrap_or("")];
    Ok(
        media_info::stream_differences(&item.info, &live, &paths).is_empty()
            && media_info::chapters_restored(&item.chapters, &live["Chapters"]),
    )
}
async fn validate_legacy_scope(
    state: &AppState,
    restore: &BTreeMap<String, Prepared>,
) -> Result<()> {
    let paths = restore
        .values()
        .flat_map(|item| std::iter::once(item.path.clone()).chain(item.source.clone()))
        .collect::<BTreeSet<_>>();
    for item in restore.values() {
        let target = SavedTarget {
            id: item.target.id.clone(),
            path: item.path.clone(),
            source: item.source.clone(),
            sha: item.target.sha.clone(),
            asset: (!item.target.kind.is_empty()).then(|| item.target.asset.clone()),
            info: item.info.clone(),
            refresh_required: true,
        };
        let live = saved_details(state, &target).await?;
        if media_edit::list(&live["MediaSources"])
            .iter()
            .any(|source| {
                source["Path"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty() && !paths.contains(path))
            })
        {
            return Err(ApiError::bad(
                "原保存范围之外出现新的关联版本，未清理任何媒体",
            ));
        }
    }
    Ok(())
}
pub(crate) async fn resume_cleanup(state: &AppState, context: &JobContext) -> Result<()> {
    let root = state.data_dir.join("backups/media-info-reload");
    for (key, value) in context.saved_plans().await? {
        if let Some(prefix) = key.strip_suffix("legacy-restore-settings") {
            let scoped = context.scoped(prefix.trim_end_matches(':'));
            if scoped.completed("media-save-result").await?.is_some() {
                continue;
            }
            scoped.ensure_server(state).await?;
            if let Some(plan) = scoped.plan("media-save-plan").await? {
                let preparation: SavePreparation = serde_json::from_value(plan)
                    .map_err(|_| ApiError::bad("原媒体恢复准备计划无效"))?;
                let candidates = legacy_clear_candidates(&preparation.restore, &scoped).await?;
                let mut cleared = vec![];
                for id in candidates {
                    if legacy_current_empty(state, &preparation.restore[&id]).await? {
                        cleared.push(id);
                    }
                }
                if !cleared.is_empty() {
                    validate_legacy_scope(state, &preparation.restore).await?;
                    crate::strm::with_changes(state, &value["applied"], || async {
                        for id in &cleared {
                            let item = &preparation.restore[id];
                            let expected = serde_json::to_vec_pretty(&item.info)
                                .map_err(|_| ApiError::bad("原恢复 JSON 无效"))?;
                            let current = tokio::fs::read(&item.final_path).await.ok();
                            if current.as_deref() != Some(expected.as_slice()) {
                                if current.is_some() && current != item.expected_json {
                                    return Err(ApiError(
                                        StatusCode::CONFLICT,
                                        "已清理目标的 JSON 在重启期间被修改，保留当前文件及原计划"
                                            .into(),
                                    ));
                                }
                                let stage = item
                                    .stage
                                    .as_ref()
                                    .ok_or_else(|| ApiError::bad("原恢复暂存路径缺失"))?;
                                if !tokio::fs::try_exists(stage).await? {
                                    tokio::fs::create_dir_all(
                                        stage
                                            .parent()
                                            .ok_or_else(|| ApiError::bad("原恢复暂存路径无效"))?,
                                    )
                                    .await?;
                                    tokio::fs::write(stage, &expected).await?;
                                    tokio::fs::File::open(stage).await?.sync_all().await?;
                                }
                                if tokio::fs::read(stage).await? != expected {
                                    return Err(ApiError(
                                        StatusCode::CONFLICT,
                                        "原恢复 JSON 暂存内容已变化，未覆盖当前文件".into(),
                                    ));
                                }
                                commit_json_stage_guarded(
                                    stage,
                                    &item.final_path,
                                    current.as_deref(),
                                )
                                .await?;
                            }
                            if let Err(error) =
                                cache_saved(state, item.target.sha.as_deref(), &item.info).await
                            {
                                tracing::warn!(
                                    "[重启恢复] 原 JSON 已恢复，缓存同步未完成：{}",
                                    error.1
                                );
                            }
                            scoped
                                .complete(
                                    &format!("legacy-json:{id}"),
                                    &json!({"path":item.final_path,"info":item.info}),
                                )
                                .await?;
                        }
                        for group in &preparation.groups {
                            let key = group.join(",");
                            if scoped
                                .completed(&format!("legacy-ungroup:{key}"))
                                .await?
                                .is_some()
                                && scoped
                                    .completed(&format!("legacy-merge:{key}"))
                                    .await?
                                    .is_none()
                            {
                                emby_post(state, "Videos/MergeVersions", &[("Ids", key.clone())])
                                    .await?;
                                scoped
                                    .complete(&format!("legacy-merge:{key}"), &json!(group))
                                    .await?;
                            }
                        }
                        let all_prepared = preparation
                            .restore
                            .keys()
                            .all(|id| preparation.restore[id].final_path.is_file());
                        let original_import = scoped.scoped("legacy-all-import");
                        let native_key = "native-task:MediaInfoExtractTask";
                        let original_started = original_import.plan(native_key).await?.is_some()
                            || original_import.completed(native_key).await?.is_some();
                        let phase = if original_started
                            || (all_prepared && cleared.len() == preparation.restore.len())
                        {
                            "legacy-all-import".to_string()
                        } else {
                            format!("legacy-cleared-import:{}", cleared.join(","))
                        };
                        crate::emby::scheduled(
                            state,
                            "MediaInfoExtractTask",
                            &scoped.scoped(&phase),
                        )
                        .await?;
                        for id in &cleared {
                            let item = &preparation.restore[id];
                            let target = SavedTarget {
                                id: id.clone(),
                                path: item.path.clone(),
                                source: item.source.clone(),
                                sha: item.target.sha.clone(),
                                asset: (!item.target.kind.is_empty())
                                    .then(|| item.target.asset.clone()),
                                info: item.info.clone(),
                                refresh_required: true,
                            };
                            let live = saved_details(state, &target).await?;
                            let paths = [item.path.as_str(), item.source.as_deref().unwrap_or("")];
                            if !media_info::stream_differences(&item.info, &live, &paths).is_empty()
                                || !media_info::chapters_restored(&item.chapters, &live["Chapters"])
                            {
                                return Err(ApiError::bad(
                                    "已清理子集的原媒体流或章节恢复尚未通过验收",
                                ));
                            }
                        }
                        Ok(())
                    })
                    .await?;
                }
            }
            crate::strm::restore_media_save_settings(state, &value["original"], &value["applied"])
                .await?;
            continue;
        }
        let Some(prefix) = key.strip_suffix("saved-refresh-directory") else {
            continue;
        };
        let scoped = context.scoped(prefix.trim_end_matches(':'));
        let recovery: PathBuf =
            serde_json::from_value(value).map_err(|_| ApiError::bad("原保存恢复目录无效"))?;
        if recovery.parent() != Some(root.as_path()) {
            return Err(ApiError::bad("原保存恢复目录不在固定备份根目录内"));
        }
        if let Some(completed) = scoped.completed("saved-refresh-report").await? {
            if completed["cleanup_allowed"] == true {
                let mut report = completed["report"].clone();
                let _ = cleanup_recovery(&recovery, &mut report).await;
            }
            continue;
        }
        let original_settings = recovery.join("original-settings.json");
        if tokio::fs::try_exists(&original_settings).await? {
            scoped.ensure_server(state).await?;
            let original = crate::media_recovery::read_json(&original_settings).await?;
            let applied = crate::strm::recovery_applied_settings(&recovery, &original).await?;
            let pending = tokio::fs::try_exists(recovery.join("media-reload.json")).await?
                || tokio::fs::try_exists(recovery.join("pending-scan.json")).await?;
            if pending {
                crate::strm::finish_media_recovery_rearmed(
                    state,
                    &original,
                    applied.as_ref(),
                    || async {
                        crate::media_reload::recover_started_record(state, &recovery, &scoped)
                            .await?;
                        Ok(())
                    },
                )
                .await?;
            } else {
                crate::strm::finish_media_recovery(state, &original, applied.as_ref(), || async {
                    Ok(())
                })
                .await?;
            }
        } else if scoped
            .completed("saved-refresh-settings-restored")
            .await?
            .is_none()
            && let Some(settings) = scoped.plan("saved-refresh-settings").await?
        {
            scoped.ensure_server(state).await?;
            crate::strm::finish_media_recovery(
                state,
                &settings["original"],
                Some(&settings["applied"]),
                || async { Ok(()) },
            )
            .await?;
        }
        scoped
            .complete("saved-refresh-settings-restored", &json!(true))
            .await?;
    }
    Ok(())
}
async fn cleanup_recovery(recovery: &std::path::Path, report: &mut Value) -> Option<String> {
    match tokio::fs::remove_dir_all(recovery).await {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            let warning = format!(
                "ETK 恢复暂存文件清理失败：{error}；仍保留：{}",
                recovery.display()
            );
            tracing::warn!("[媒体信息保存收尾] {warning}");
            report["warning_details"]
                .as_array_mut()
                .unwrap()
                .push(json!(warning));
            Some(warning)
        }
    }
}
async fn cleanup_stages(prepared: &BTreeMap<String, Prepared>) {
    for item in prepared.values() {
        if let Some(path) = &item.stage {
            let _ = tokio::fs::remove_file(path).await;
            if let Some(parent) = path.parent()
                && parent
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("staging-"))
            {
                let _ = tokio::fs::remove_dir(parent).await;
            }
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
pub(crate) async fn save_rebuilt_files(
    state: &AppState,
    id: &str,
    body: &Value,
    rebuilt: &BTreeMap<String, Value>,
    context: &JobContext,
) -> Result<(u16, Value)> {
    save_inner(state, id, body, rebuilt, Some(context), true).await
}
async fn save_inner(
    state: &AppState,
    id: &str,
    body: &Value,
    rebuilt: &BTreeMap<String, Value>,
    context: Option<&JobContext>,
    file_refresh: bool,
) -> Result<(u16, Value)> {
    let owned_context =
        context.map(|context| context.scoped(&format!("media-save:{id}:{file_refresh}")));
    let context = owned_context.as_ref();
    if let Some(context) = context
        && let Some(result) = context.completed("media-save-result").await?
    {
        return Ok((
            result["status"].as_u64().unwrap_or(500) as u16,
            result["result"].clone(),
        ));
    }
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
    let original_plan = if let Some(context) = context {
        context.plan("media-save-plan").await?
    } else {
        None
    };
    let resuming_preparation = original_plan.is_some();
    let SavePreparation {
        mut restore,
        groups,
        mut failures,
        reports,
        mut saved,
    } = if let Some(plan) = original_plan {
        serde_json::from_value(plan)
            .map_err(|_| ApiError::bad("原媒体保存计划无效，已保留现有媒体和恢复材料"))?
    } else {
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
            if file_refresh { crate::media_reload::ensure_single_bundle(&original)?; }
            if target_id==id {
                if let Some(sha)=body.get("sha1") && text(sha).to_uppercase()!=target.sha.clone().unwrap_or_default().to_uppercase() {return Err(ApiError::bad("当前文件指纹已变化，请重新打开编辑器"));}
                if body["media_path"].as_str().is_some_and(|p|!p.is_empty() && p!=target.path) {return Err(ApiError::bad("当前文件路径已变化，请重新打开编辑器"));}
                if let Some(source) = body["strm_source"].as_str() && crate::media_paths::strm_target(&target.path).await.unwrap_or_default() != source { return Err(ApiError::bad("STRM目标已变化，未清理Emby数据")); }
            }
            identity::verify(&target.asset,if target_id==id {&body["mediainfo"]} else {&original})?;
            let mut explicit_language_positions = vec![];
            let mut info=if let Some(info)=rebuilt.get(target_id){info.clone()}
                else if target_id==id {body["mediainfo"].clone()}
                else {
                    let (info,report)=media_info::apply_patches(&original,&body["stream_patches"]);
                    explicit_language_positions = report["explicit_language_positions"].as_array()
                        .into_iter().flatten().filter_map(Value::as_u64)
                        .map(|position| position as usize).collect();
                    reports.push(json!({"item_id":target_id,"path":target.path,"matched":report["matched"],"skipped":report["skipped"]}));
                    if report["matched"]==0 {return Ok(None);}
                    info
                };
            crate::language::normalize_subtitles_for_save_with_explicit(
                &mut info, &original, &explicit_language_positions,
            );
            validate_requested_chapters(&info, &original)?;
            if file_refresh { crate::media_reload::ensure_json_streams_unfiltered(&info,target_id)?; }
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
            let message = failures.first().map_or_else(
                || "没有可安全恢复的媒体信息，已停止保存，未清理 Emby 数据".into(),
                |reason| format!("{reason}；已停止保存，未清理 Emby 数据"),
            );
            return failed(&message, &failures, 409);
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
            let prepared = prepare(state, target, info, !saved.contains(&id)).await?;
            if prepared.multipart {
                let details =
                    media_edit::details(state, &id, "Id,Path,PartCount,MediaSources").await?;
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
                    restore.insert(part_id, prepare(state, target, info, true).await?);
                }
            }
            restore.insert(id, prepared);
        }
        SavePreparation {
            restore,
            groups,
            failures,
            reports,
            saved,
        }
    };
    if let Some(context) = context
        && !resuming_preparation
    {
        context.update(35, "暂存", "暂存媒体信息和当前章节").await;
    }
    if file_refresh {
        let context = context.ok_or_else(|| ApiError::bad("保存任务上下文缺失"))?;
        let mut file_targets = vec![];
        for (id, item) in &restore {
            crate::media_reload::ensure_single_bundle(&item.info)?;
            file_targets.push(SavedTarget {
                id: id.clone(),
                path: item.path.clone(),
                source: item.source.clone(),
                sha: item.target.sha.clone(),
                asset: (!item.target.kind.is_empty()).then(|| item.target.asset.clone()),
                info: item.info.clone(),
                refresh_required: true,
            });
            if let Ok(bytes) = tokio::fs::read(&item.final_path).await
                && let Ok(original) = serde_json::from_slice::<Value>(&bytes)
            {
                crate::media_reload::ensure_single_bundle(&original)?;
            }
        }
        if !resuming_preparation {
            crate::media_reload::preflight_file_imports(state, &file_targets, context).await?;
            let staged = stage_all(state, &mut restore, &order).await;
            if let Err(error) = staged {
                cleanup_stages(&restore).await;
                return Err(error);
            }
            context.save_plan("media-save-plan", &json!({"restore":restore,"saved":saved,"groups":groups,"failures":failures,"reports":reports})).await?;
        }
        let mut warning_details = vec![];
        let mut written_ids = vec![];
        let json_result = context.completed("saved-json-results").await?;
        let json_prepared = json_result.is_some();
        if let Some(previous) = json_result {
            written_ids = serde_json::from_value(previous["written_ids"].clone())
                .map_err(|_| ApiError::bad("原 JSON 保存结果无效"))?;
            warning_details = serde_json::from_value(previous["warning_details"].clone())
                .map_err(|_| ApiError::bad("原 JSON 保存提醒无效"))?;
            failures = serde_json::from_value(previous["failures"].clone())
                .map_err(|_| ApiError::bad("原 JSON 保存失败结果无效"))?;
            saved = serde_json::from_value(previous["saved"].clone())
                .map_err(|_| ApiError::bad("原 JSON 保存目标无效"))?;
        }
        for id in restore.keys() {
            if !json_prepared
                && context
                    .completed(&format!("saved-json:{id}"))
                    .await?
                    .is_some()
            {
                written_ids.push(id.clone());
            }
        }
        let refresh_outcome = refresh_saved(state, context, || async {
            let mut items = restore.iter().collect::<Vec<_>>();
            items.sort_by_cached_key(|(id, _)| order.get(*id).cloned());
            if !json_prepared {
            for (id, item) in items {
                if let Some(stage) = &item.stage {
                    let saved = SavedTarget {
                        id: id.clone(),
                        path: item.path.clone(),
                        source: item.source.clone(),
                        sha: item.target.sha.clone(),
                        asset: (!item.target.kind.is_empty()).then(|| item.target.asset.clone()),
                        info: item.info.clone(),
                        refresh_required: true,
                    };
                    saved_details(state, &saved).await?;
                    let expected = serde_json::to_vec_pretty(&item.info).map_err(|_| ApiError::bad("原保存 JSON 无效"))?;
                    let written = if tokio::fs::read(&item.final_path).await.ok().as_deref() == Some(expected.as_slice()) {
                        validate_json_destination(&item.final_path, Some(&expected)).await
                    } else if written_ids.contains(id) {
                        Err(ApiError(StatusCode::CONFLICT, "已保存 JSON 在重启期间变化，已保留用户文件和原恢复计划".into()))
                    } else {
                        if !tokio::fs::try_exists(stage).await? {
                            tokio::fs::create_dir_all(stage.parent().ok_or_else(|| ApiError::bad("原保存暂存路径无效"))?).await?;
                            tokio::fs::write(stage, &expected).await?;
                            tokio::fs::File::open(stage).await?.sync_all().await?;
                        }
                        if tokio::fs::read(stage).await? != expected {
                            return Err(ApiError(StatusCode::CONFLICT, "原 JSON 暂存内容已变化，保留当前文件及原计划".into()));
                        }
                        commit_json_stage_guarded(stage, &item.final_path, item.expected_json.as_deref()).await
                    };
                    if let Err(error) = written {
                        written_ids.retain(|written| written != id);
                        failures.push(format!("{id}：媒体信息 JSON 保存失败：{}", error.1));
                        continue;
                    }
                    context.complete(&format!("saved-json:{id}"), &json!({"path":item.final_path,"info":item.info})).await?;
                    if !written_ids.contains(id) { written_ids.push(id.clone()); }
                    if let Err(error) =
                        cache_saved(state, item.target.sha.as_deref(), &item.info).await
                    {
                        warning_details.push(format!(
                            "{id}：媒体信息 JSON 已保存，缓存更新失败：{}",
                            error.1
                        ));
                    }
                }
            }
            }
            if written_ids.is_empty() {
                return Err(ApiError::bad("媒体信息 JSON 未能保存，未通知 Emby 刷新"));
            }
            saved.retain(|id| written_ids.contains(id));
            if saved.is_empty() {
                return Err(ApiError::bad(
                    "编辑目标的媒体信息 JSON 未能保存，已保留关联版本文件",
                ));
            }
            context.complete("saved-json-results", &json!({"written_ids":written_ids,"saved":saved,"failures":failures,"warning_details":warning_details})).await?;
            context.checkpoint().await?;
            context
                .activity("通知Emby刷新", "媒体信息 JSON 已保存，通知 Emby 重新读取")
                .await;
            Ok(written_ids
                .iter()
                .map(|id| {
                    let item = &restore[id];
                    SavedTarget {
                        id: id.clone(),
                        path: item.path.clone(),
                        source: item.source.clone(),
                        sha: item.target.sha.clone(),
                        asset: (!item.target.kind.is_empty()).then(|| item.target.asset.clone()),
                        info: item.info.clone(),
                        refresh_required: true,
                    }
                })
                .collect::<Vec<_>>())
        })
        .await;
        cleanup_stages(&restore).await;
        let metadata_refresh = refresh_outcome?;
        saved.retain(|id| written_ids.contains(id));
        warning_details.extend(
            media_edit::list(&metadata_refresh["warning_details"])
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
        if written_ids.is_empty() || saved.is_empty() {
            failures.extend(
                media_edit::list(&metadata_refresh["failures"])
                    .iter()
                    .map(|failure| string(failure, "reason")),
            );
            return failed(
                "媒体信息 JSON 未能安全保存，未通知 Emby 刷新",
                &failures,
                500,
            );
        }
        for failure in media_edit::list(&metadata_refresh["failures"]) {
            warning_details.push(format!(
                "媒体信息 JSON 已保存，Emby 刷新或回读未确认：{}：{}",
                string(&failure, "item_id"),
                string(&failure, "reason")
            ));
        }
        let video_thumbnails = if let Some(previous) =
            context.completed("saved-video-thumbnails").await?
        {
            previous
        } else if metadata_refresh["state"] == "verified" {
            let report = thumbnails_after_save(state, &restore, &saved, context).await?;
            context.complete("saved-video-thumbnails", &report).await?;
            report
        } else {
            json!({"state":"not_requested","reason":"媒体流和章节尚未通过回读，未继续缩略图任务"})
        };
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
        let message = match metadata_refresh["state"].as_str() {
            Some("verified") => "媒体信息 JSON 已保存，Emby 媒体流、标题和章节已通过回读验收",
            _ => "媒体信息 JSON 已保存，Emby 刷新或回读尚未确认，请查看提醒",
        };
        let saved_mediainfo = restore
            .get(id)
            .filter(|_| saved.iter().any(|saved_id| saved_id == id))
            .map(|item| &item.info);
        let outcome = (
            if failures.is_empty() { 200 } else { 207 },
            json!({
                "message":message,"saved_item_ids":saved,"media_info_saved":true,"saved_mediainfo":saved_mediainfo,
                "failures":failures,"batch_reports":reports,"warnings":warnings,"warning_details":warning_details,
                "metadata_refresh":metadata_refresh,
                "video_thumbnails":video_thumbnails
            }),
        );
        context
            .complete(
                "media-save-result",
                &json!({"status":outcome.0,"result":outcome.1}),
            )
            .await?;
        return Ok(outcome);
    }
    let targets = restore
        .values()
        .map(|item| (item.target.id.clone(), item.path.clone()))
        .collect::<Vec<_>>();
    let legacy_settings = if let Some(context) = context {
        context.plan("legacy-restore-settings").await?
    } else {
        None
    };
    let settings = if let Some(previous) = legacy_settings {
        previous["applied"].clone()
    } else {
        let settings =
            restoration_settings(state, &targets, restore.values().any(|item| item.multipart))
                .await?;
        if let Some(context) = context {
            let current = crate::strm::config(state).await?;
            let original = settings
                .as_object()
                .unwrap()
                .keys()
                .map(|key| (key.clone(), current[key].clone()))
                .collect::<serde_json::Map<_, _>>();
            context
                .save_plan(
                    "legacy-restore-settings",
                    &json!({"original":original,"applied":settings}),
                )
                .await?;
        }
        settings
    };
    if !resuming_preparation {
        let result = stage_all(state, &mut restore, &order).await;
        if let Err(error) = result {
            cleanup_stages(&restore).await;
            return Err(error);
        }
        if let Some(context) = context {
            context.save_plan("media-save-plan", &json!({"restore":restore,"saved":saved,"groups":groups,"failures":failures,"reports":reports})).await?;
        }
    }
    if let Some(context) = context
        && let Err(error) = context.checkpoint().await
    {
        cleanup_stages(&restore).await;
        return Err(error);
    }
    let mut ordered_restore = restore.iter().collect::<Vec<_>>();
    ordered_restore.sort_by_cached_key(|(id, _)| order.get(*id).cloned());
    if resuming_preparation && let Some(context) = context {
        let started = legacy_clear_candidates(&restore, context).await?;
        for (id, item) in &ordered_restore {
            let current = tokio::fs::read(&item.final_path).await.ok();
            let requested_matches = current.as_ref().is_some_and(|bytes| {
                serde_json::from_slice::<Value>(bytes).is_ok_and(|actual| actual == item.info)
            });
            let completed = context
                .completed(&format!("legacy-json:{id}"))
                .await?
                .is_some();
            let known_missing = current.is_none()
                && started.contains(*id)
                && legacy_current_empty(state, item).await?;
            if !requested_matches
                && (completed || (current != item.expected_json && !known_missing))
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "原恢复 JSON 在暂停期间被修改，未重新清理或覆盖当前文件".into(),
                ));
            }
        }
    }
    let imported = if let Some(context) = context {
        context.completed("legacy-import").await?
    } else {
        None
    };
    let outcome = if let Some(previous) = imported {
        Ok(serde_json::from_value(previous).map_err(|_| ApiError::bad("原恢复任务结果无效"))?)
    } else {
        crate::strm::with_changes(state, &settings, || async {
    if let Some(context) = context {
        context.update(50, "清理缓存", "清理旧媒体信息缓存").await;
    }
    // Once clear starts, finish file/group restoration even if cancellation is requested.
    let mut ungrouped = vec![];
    let mut operation_failures = vec![];
    let mut clear_attempted = false;
    let mut started = if let Some(context) = context { legacy_clear_candidates(&restore, context).await? } else { BTreeSet::new() };
    validate_legacy_scope(state, &restore).await?;
    for group in &groups {
        let multipart = group
            .iter()
            .any(|id| restore.get(id).is_some_and(|p| p.multipart));
        if multipart && group.len() > 1 {
            let ungrouped_before = if let Some(context) = context { context.completed(&format!("legacy-ungroup:{}", group.join(","))).await?.is_some() } else { false };
            if !ungrouped_before && let Err(error) = emby_post(
                state,
                &format!("Videos/{}/AlternateSources/Delete", group[0]),
                &[],
            )
            .await
            {
                operation_failures.push(error.1);
                break;
            }
            if let Some(context) = context {
                context.complete(&format!("legacy-ungroup:{}", group.join(",")), &json!(group)).await?;
            }
            ungrouped.push(group.clone());
        }
        // ETK bindings may contain versions that Emby has not merged. Clear each
        // prepared item instead of assuming the first ID clears the entire group.
        for id in group {
            if started.contains(id) && (legacy_current_empty(state, &restore[id]).await?
                || legacy_current_restored(state, &restore[id]).await?) {
                continue;
            }
            if restore.get(id).is_some_and(|item| !item.has_streams) {
                tracing::info!(
                    "[媒体信息保存] 媒体ID {id}：当前没有音视频流，保留现有备份直接恢复，不重复清理"
                );
                continue;
            }
            let affected = group.iter().cloned().chain(restore.iter().filter_map(|(member, item)| {
                group.contains(&string(&item.observation, "ParentId")).then(|| member.clone())
            })).collect::<BTreeSet<_>>();
            if let Some(context) = context {
                context.save_plan(&format!("legacy-clear-intent:{id}"), &json!({"target":legacy_identity(id, &restore[id]),"affected_ids":affected})).await?;
            }
            started.extend(affected);
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
            let expected = serde_json::to_vec_pretty(&item.info).map_err(|_| ApiError::bad("原恢复 JSON 无效"))?;
            if !tokio::fs::try_exists(stage).await? {
                tokio::fs::create_dir_all(stage.parent().ok_or_else(|| ApiError::bad("原暂存恢复路径无效"))?).await?;
                tokio::fs::write(stage, &expected).await?;
                tokio::fs::File::open(stage).await?.sync_all().await?;
            }
            if tokio::fs::read(stage).await? != expected {
                return Err(ApiError(StatusCode::CONFLICT, "原恢复 JSON 暂存内容已变化，保留全部恢复材料".into()));
            }
            let current = tokio::fs::read(&item.final_path).await.ok();
            let already_saved = if let Some(context) = context { context.completed(&format!("legacy-json:{id}")).await?.is_some() } else { false };
            let requested_matches = current.as_ref().is_some_and(|bytes| serde_json::from_slice::<Value>(bytes).is_ok_and(|actual| actual == item.info));
            let restored = if requested_matches {
                validate_json_destination(&item.final_path, current.as_deref()).await
            } else if already_saved || (current != item.expected_json && (current.is_some() || !started.contains(*id))) {
                Err(ApiError(StatusCode::CONFLICT, "恢复 JSON 在任务中断后发生变化，保留用户新文件与原持久请求".into()))
            } else { commit_json_stage_guarded(stage, &item.final_path, current.as_deref()).await };
            if let Err(error) = restored {
                operation_failures.push(format!("{id}: 无法恢复 JSON: {}", error.1));
                continue;
            }
            if let Some(context) = context {
                context.complete(&format!("legacy-json:{id}"), &json!({"path":item.final_path,"info":item.info})).await?;
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
                    if let Err(error) = cache_saved(state, item.target.sha.as_deref(), &item.info).await {
                        operation_failures.push(format!("{id}：JSON 已恢复，缓存同步失败：{}", error.1));
                    }
                }
                _ => operation_failures.push(format!("{id}: 恢复的 JSON 无效")),
            }
        }
    }
    for group in ungrouped {
        let merged_before = if let Some(context) = context { context.completed(&format!("legacy-merge:{}", group.join(","))).await?.is_some() } else { false };
        if !merged_before && let Err(error) =
            emby_post(state, "Videos/MergeVersions", &[("Ids", group.join(","))]).await
        {
            operation_failures.push(format!("恢复版本组 {} 失败: {}", group.join(","), error.1));
        } else if let Some(context) = context {
            context.complete(&format!("legacy-merge:{}", group.join(",")), &json!(group)).await?;
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
    if let Some(context) = context {
        let recovery_context = context.for_resume_cleanup().await.scoped("legacy-all-import");
        crate::emby::scheduled(state, "MediaInfoExtractTask", &recovery_context).await?;
        context.complete("legacy-import", &json!(operation_failures)).await?;
    } else {
        crate::emby::scheduled(state, "MediaInfoExtractTask", &recovery_context).await?;
    }
    Ok(operation_failures)
    }).await
    };
    cleanup_stages(&restore).await;
    let operation_failures = outcome?;
    validate_legacy_scope(state, &restore).await?;
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
    let result = (
        if failures.is_empty() { 200 } else { 207 },
        json!({"message":format!("媒体信息已更新，已恢复 {} 个版本 JSON，Emby 媒体流、标题和章节已通过回读验收",restore.len()),"saved_mediainfo":restore.get(id).map(|item| &item.info),"failures":failures,"batch_reports":reports,"warnings":skipped_tracks,"warning_details":[],"metadata_refresh":{"state":"not_requested","requested_ids":[],"failed_ids":[]},"video_thumbnails":{"state":"not_requested"}}),
    );
    if let Some(context) = context {
        context
            .complete(
                "media-save-result",
                &json!({"status":result.0,"result":result.1}),
            )
            .await?;
    }
    Ok(result)
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

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[tokio::test]
    async fn verified_reload_keeps_verification_and_reports_real_cleanup_failure() {
        let directory = tempfile::tempdir().unwrap();
        let recovery = directory.path().join("saved-recovery");
        tokio::fs::write(&recovery, b"retained recovery record")
            .await
            .unwrap();
        let mut report =
            json!({"state":"verified","verified_ids":["61616"],"failures":[],"warning_details":[]});
        let warning = cleanup_recovery(&recovery, &mut report).await.unwrap();
        assert_eq!(report["state"], "verified");
        assert_eq!(report["verified_ids"], json!(["61616"]));
        assert_eq!(report["failures"], json!([]));
        assert_eq!(report["warning_details"], json!([warning]));
        assert!(warning.contains("清理失败") && warning.contains(recovery.to_str().unwrap()));
        assert_eq!(
            tokio::fs::read(&recovery).await.unwrap(),
            b"retained recovery record"
        );
    }

    #[tokio::test]
    async fn completed_recovery_is_removed_without_false_warning() {
        let directory = tempfile::tempdir().unwrap();
        let recovery = directory.path().join("saved-recovery");
        tokio::fs::create_dir(&recovery).await.unwrap();
        tokio::fs::write(recovery.join("original-settings.json"), b"{}")
            .await
            .unwrap();
        let mut report = json!({"state":"verified","warning_details":[]});
        assert!(cleanup_recovery(&recovery, &mut report).await.is_none());
        assert!(!recovery.exists());
        assert!(cleanup_recovery(&recovery, &mut report).await.is_none());
        assert_eq!(report["state"], "verified");
        assert_eq!(report["warning_details"], json!([]));
    }
}

#[cfg(test)]
mod guarded_commit_tests {
    use super::*;

    #[tokio::test]
    async fn guarded_commit_accepts_matching_bytes_and_expected_missing_file() {
        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join("stage.json");
        let destination = directory.path().join("item.json");
        let old = br#"{"Title":"observed"}"#;
        let requested = br#"{"Title":"requested"}"#;
        tokio::fs::write(&stage, requested).await.unwrap();
        tokio::fs::write(&destination, old).await.unwrap();
        commit_json_stage_guarded(&stage, &destination, Some(old))
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), requested);
        tokio::fs::remove_file(&destination).await.unwrap();
        commit_json_stage_guarded(&stage, &destination, None)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), requested);
        // The ordinary save entry remains an unconditional staged commit.
        tokio::fs::write(&destination, b"ordinary unsnapshotted value")
            .await
            .unwrap();
        commit_json_stage(&stage, &destination).await.unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), requested);
    }

    #[tokio::test]
    async fn guarded_commit_keeps_new_created_or_removed_destination_state() {
        for (current, expected) in [
            (Some(b"new bytes".as_slice()), Some(b"old bytes".as_slice())),
            (Some(b"new bytes".as_slice()), None),
            (None, Some(b"old bytes".as_slice())),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let stage = directory.path().join("stage.json");
            let destination = directory.path().join("item.json");
            tokio::fs::write(&stage, b"original recovery bytes")
                .await
                .unwrap();
            if let Some(bytes) = current {
                tokio::fs::write(&destination, bytes).await.unwrap();
            }
            let error = commit_json_stage_guarded(&stage, &destination, expected)
                .await
                .unwrap_err();
            assert_eq!(error.0, StatusCode::CONFLICT);
            assert_eq!(tokio::fs::read(&destination).await.ok().as_deref(), current);
            assert_eq!(
                tokio::fs::read(&stage).await.unwrap(),
                b"original recovery bytes"
            );
        }
    }

    #[tokio::test]
    async fn mutation_after_staging_is_not_overwritten_and_temporary_commit_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join("stage.json");
        let destination = directory.path().join("item.json");
        let expected = b"observed current JSON";
        let requested = b"original recovery JSON";
        let changed = b"JSON changed while the staged file was copied and synchronized";
        tokio::fs::write(&stage, requested).await.unwrap();
        tokio::fs::write(&destination, expected).await.unwrap();
        let error = commit_json_stage_with_validation(&stage, &destination, || async {
            let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
            let mut staged = vec![];
            while let Some(entry) = entries.next_entry().await.unwrap() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".etk-commit-")
                {
                    staged.push(entry.path());
                }
            }
            assert_eq!(staged.len(), 1);
            assert_eq!(tokio::fs::read(&staged[0]).await.unwrap(), requested);
            tokio::fs::write(&destination, changed).await.unwrap();
            validate_json_destination(&destination, Some(expected)).await
        })
        .await
        .unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), changed);
        assert_eq!(tokio::fs::read(&stage).await.unwrap(), requested);
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".etk-commit-")
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn guarded_commit_refuses_nonfiles_and_symlinked_destinations_or_parents() {
        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join("stage.json");
        let outside = directory.path().join("outside");
        tokio::fs::create_dir(&outside).await.unwrap();
        let kept = outside.join("keep.json");
        tokio::fs::write(&kept, b"keep external bytes")
            .await
            .unwrap();
        tokio::fs::write(&stage, b"recovery bytes").await.unwrap();
        let symlink = directory.path().join("item.json");
        std::os::unix::fs::symlink(&kept, &symlink).unwrap();
        let parent_link = directory.path().join("parent-link");
        std::os::unix::fs::symlink(&outside, &parent_link).unwrap();
        let linked_destination = parent_link.join("keep.json");
        for destination in [&symlink, &linked_destination, &outside] {
            let error =
                commit_json_stage_guarded(&stage, destination, Some(b"keep external bytes"))
                    .await
                    .unwrap_err();
            assert_eq!(error.0, StatusCode::CONFLICT);
            assert_eq!(
                tokio::fs::read(&kept).await.unwrap(),
                b"keep external bytes"
            );
        }
        assert!(
            tokio::fs::symlink_metadata(&symlink)
                .await
                .unwrap()
                .is_symlink()
        );
        assert!(outside.is_dir());
    }
}
