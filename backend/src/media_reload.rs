use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit, media_info,
    media_save::{SavedTarget, saved_details},
    state::string,
};
use axum::http::StatusCode;
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

async fn identity_details(state: &AppState, target: &SavedTarget) -> Result<Value> {
    if !Path::new(&target.path).is_absolute() {
        return Err(conflict("重载目标不是原生绝对文件路径，未清理媒体信息"));
    }
    if let Some(asset) = &target.asset {
        let current = media_edit::repair_target(state, &target.id).await?;
        if current.path != target.path || current.sha != target.sha || current.asset != *asset {
            return Err(conflict("重载期间媒体资产身份变化，已保留恢复备份"));
        }
    }
    let live = media_edit::details(state, &target.id,
        "Id,Path,Type,Guid,PresentationUniqueKey,ExtraType,PartCount,Size,Chapters,MediaSources,MediaStreams,ImageTags").await?;
    if live["Id"] != target.id
        || live["Path"] != target.path
        || crate::media_paths::strm_target(&target.path).await != target.source
    {
        return Err(conflict(
            "重载期间媒体 ID、路径或 STRM 来源变化，已保留恢复备份",
        ));
    }
    if !matches!(
        live["Type"].as_str(),
        Some("Movie" | "Episode" | "Video" | "MusicVideo" | "Audio")
    ) {
        return Err(conflict("重载目标不是单个文件媒体，未提交递归清理"));
    }
    Ok(live)
}

fn differences(target: &SavedTarget, live: &Value) -> Vec<String> {
    let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
    let mut reasons = media_info::stream_differences(&target.info, live, &paths);
    let expected = &media_info::object(&target.info).unwrap()["Chapters"];
    let empty = json!([]);
    let actual = if live["Type"] == "Audio" && live["Chapters"].is_null() {
        &empty
    } else {
        &live["Chapters"]
    };
    if !media_info::chapters_restored(expected, actual) {
        reasons.push("章节或片头片尾标记未一致".into());
    }
    reasons
}

fn group(item: &Value) -> Result<(String, bool)> {
    let key = string(item, "PresentationUniqueKey");
    if key.is_empty() {
        return Err(conflict(
            "Emby 未返回媒体演示键，无法确认清理影响范围，未清理媒体信息",
        ));
    }
    Ok((key, !string(item, "ExtraType").is_empty()))
}

// PRO 3.0.0.57 ClearMediaInfo expands PresentationUniqueKey and additional
// parts. The REST item list has no PresentationUniqueKey filter: match it
// locally, including every page, before allowing the first clear request.
async fn check_scope(
    state: &AppState,
    targets: &[&SavedTarget],
    groups: &BTreeSet<(String, bool)>,
) -> Result<BTreeMap<(String, bool), BTreeSet<String>>> {
    let ids = targets
        .iter()
        .map(|t| t.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut members = BTreeMap::<(String, bool), BTreeSet<String>>::new();
    let mut seen = BTreeSet::new();
    let user = string(&*state.config.read().await, "emby_user_id");
    let mut start = 0;
    loop {
        let response = state
            .emby(
                Method::GET,
                "Items",
                &[
                    ("Recursive", "true".into()),
                    (
                        "IncludeItemTypes",
                        "Movie,Episode,Video,MusicVideo,Audio".into(),
                    ),
                    ("GroupItemsIntoCollections", "false".into()),
                    (
                        "Fields",
                        "Id,Path,Type,Guid,PresentationUniqueKey,ExtraType,PartCount,MediaSources"
                            .into(),
                    ),
                    ("StartIndex", start.to_string()),
                    ("Limit", "500".into()),
                ],
                None,
            )
            .await?;
        let items = response["Items"]
            .as_array()
            .ok_or_else(|| conflict("Emby 未返回完整媒体列表，未清理媒体信息"))?;
        for item in items {
            let item_id = string(item, "Id");
            if item_id.is_empty() || !seen.insert(item_id) {
                return Err(conflict(
                    "Emby 分页媒体列表缺少 ID 或返回重复项目，未清理媒体信息",
                ));
            }
            let key = (
                string(item, "PresentationUniqueKey"),
                !string(item, "ExtraType").is_empty(),
            );
            if !groups.contains(&key) {
                continue;
            }
            let id = string(item, "Id");
            if !ids.contains(id.as_str()) {
                return Err(conflict(format!(
                    "清理还会影响未准备的关联版本 {id}，未清理媒体信息"
                )));
            }
            members.entry(key.clone()).or_default().insert(id.clone());
            for source in media_edit::list(&item["MediaSources"]) {
                let member = string(&source, "ItemId");
                if !member.is_empty() && !ids.contains(member.as_str()) {
                    return Err(conflict(format!(
                        "清理还会影响未准备的媒体源 {member}，未清理媒体信息"
                    )));
                }
                if !member.is_empty() {
                    members.entry(key.clone()).or_default().insert(member);
                }
            }
            let multipart = item["PartCount"].as_u64().unwrap_or(0) > 1
                || media_edit::list(&item["MediaSources"])
                    .iter()
                    .any(|s| s["PartCount"].as_u64().unwrap_or(0) > 1);
            for part in if multipart {
                media_edit::additional_parts(state, &id, item).await?
            } else {
                vec![]
            } {
                let member = string(&part, "Id");
                if member.is_empty() || !ids.contains(member.as_str()) {
                    return Err(conflict(format!(
                        "清理还会影响未准备的额外分段 {member}，未清理媒体信息"
                    )));
                }
                members.entry(key.clone()).or_default().insert(member);
            }
            if string(item, "ExtraType").is_empty() && item["Type"] != "Audio" {
                let extras = state
                    .emby(
                        Method::GET,
                        &format!("Users/{user}/Items/{id}/SpecialFeatures"),
                        &[],
                        None,
                    )
                    .await?;
                let extras = extras
                    .as_array()
                    .ok_or_else(|| conflict("Emby 未返回有效附加内容列表，未清理媒体信息"))?;
                for extra in extras {
                    let kind = string(extra, "ExtraType");
                    if ["Sample", "ThemeSong", "ThemeVideo", "Trailer"].contains(&kind.as_str()) {
                        continue;
                    }
                    if ![
                        "Clip",
                        "BehindTheScenes",
                        "DeletedScene",
                        "Interview",
                        "Scene",
                        "AdditionalPart",
                    ]
                    .contains(&kind.as_str())
                    {
                        return Err(conflict("Emby 返回无法确认类型的附加内容，未清理媒体信息"));
                    }
                    let member = string(extra, "Id");
                    if member.is_empty() || !ids.contains(member.as_str()) {
                        return Err(conflict(format!(
                            "清理还会影响未准备的附加内容 {member}，未清理媒体信息"
                        )));
                    }
                    members.entry(key.clone()).or_default().insert(member);
                }
            }
        }
        start += items.len();
        if let Some(total) = response["TotalRecordCount"].as_u64() {
            if start as u64 >= total {
                break;
            }
            if items.is_empty() {
                return Err(conflict("Emby 分页提前结束，尚未完整核对清理范围"));
            }
        } else if items.len() < 500 {
            break;
        }
    }
    if groups.iter().any(|key| !members.contains_key(key)) {
        return Err(conflict(
            "Emby 媒体列表未完整返回目标关联范围，未清理媒体信息",
        ));
    }
    Ok(members)
}

pub(crate) async fn validate_targets_scope(
    state: &AppState,
    targets: &[SavedTarget],
) -> Result<()> {
    let mut groups = BTreeSet::new();
    for target in targets {
        groups.insert(group(&identity_details(state, target).await?)?);
    }
    check_scope(state, &targets.iter().collect::<Vec<_>>(), &groups).await?;
    Ok(())
}

struct Snapshot {
    path: PathBuf,
    backup: PathBuf,
    owners: BTreeSet<String>,
}

async fn snapshot_file(
    path: &Path,
    recovery: &Path,
    files: &mut BTreeMap<PathBuf, Snapshot>,
    owner: &str,
) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(conflict(format!(
            "恢复资源不是普通文件：{}，未清理媒体信息",
            path.display()
        )));
    }
    let path = tokio::fs::canonicalize(path).await?;
    if let Some(saved) = files.get_mut(&path) {
        saved.owners.insert(owner.into());
        return Ok(());
    }
    let backup = recovery.join(format!("asset-{}", files.len()));
    tokio::fs::copy(&path, &backup).await?;
    tokio::fs::File::open(&backup).await?.sync_all().await?;
    files.insert(
        path.clone(),
        Snapshot {
            path,
            backup,
            owners: BTreeSet::from([owner.into()]),
        },
    );
    Ok(())
}

async fn check_ancestors(path: &Path) -> Result<()> {
    for parent in path.parent().into_iter().flat_map(Path::ancestors) {
        if tokio::fs::symlink_metadata(parent)
            .await
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(conflict(format!(
                "恢复资源的父目录变为符号链接，已保留备份：{}",
                parent.display()
            )));
        }
    }
    Ok(())
}

async fn writable_parent(path: &Path) -> Result<()> {
    check_ancestors(path).await?;
    let mut parent = path
        .parent()
        .ok_or_else(|| conflict("恢复资源没有父目录"))?;
    while !tokio::fs::try_exists(parent).await? {
        parent = parent
            .parent()
            .ok_or_else(|| conflict("恢复资源没有可访问的父目录"))?;
    }
    let access = tokio::process::Command::new("sh")
        .args([
            "-c",
            "test -w \"$1\" && test -x \"$1\"",
            "etk-reload-access",
        ])
        .arg(parent)
        .status()
        .await?;
    if !access.success() {
        return Err(conflict(format!(
            "工具没有恢复资源父目录的写入权限：{}，未清理媒体信息",
            parent.display()
        )));
    }
    Ok(())
}

async fn snapshot_directory(
    root: &Path,
    recovery: &Path,
    files: &mut BTreeMap<PathBuf, Snapshot>,
    owner: &str,
) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            return Err(conflict(format!(
                "目标元数据包含符号链接：{}，未清理媒体信息",
                path.display()
            )));
        }
        if metadata.is_dir() {
            let mut entries = tokio::fs::read_dir(path).await?;
            while let Some(entry) = entries.next_entry().await? {
                pending.push(entry.path());
            }
        } else {
            snapshot_file(&path, recovery, files, owner).await?;
        }
    }
    Ok(())
}

async fn snapshot_local_bif(
    target: &SavedTarget,
    recovery: &Path,
    files: &mut BTreeMap<PathBuf, Snapshot>,
) -> Result<bool> {
    let path = Path::new(&target.path);
    let parent = path
        .parent()
        .ok_or_else(|| conflict("媒体路径没有父目录"))?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let prefix = stem.to_lowercase();
    let mut entries = tokio::fs::read_dir(parent).await?;
    let mut found = false;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let name = name.to_lowercase();
        if name.ends_with(".bif") && name.starts_with(&prefix) {
            snapshot_file(&entry.path(), recovery, files, &target.id).await?;
            found = true;
        }
    }
    Ok(found)
}

async fn restore_files(files: &BTreeMap<PathBuf, Snapshot>, ids: &BTreeSet<String>) -> Result<()> {
    let mut failures = vec![];
    for saved in files
        .values()
        .filter(|saved| !saved.owners.is_disjoint(ids))
    {
        let restored = async {
            check_ancestors(&saved.path).await?;
            if tokio::fs::symlink_metadata(&saved.path)
                .await
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(conflict("恢复路径在重载期间变为符号链接，已保留备份"));
            }
            if tokio::fs::try_exists(&saved.path).await? {
                if tokio::fs::read(&saved.path).await? != tokio::fs::read(&saved.backup).await? {
                    return Err(conflict(format!(
                        "恢复资源在重载期间被修改，保留新文件及备份：{}",
                        saved.path.display()
                    )));
                }
                return Ok(());
            }
            tokio::fs::create_dir_all(saved.path.parent().unwrap()).await?;
            tokio::fs::copy(&saved.backup, &saved.path).await?;
            tokio::fs::File::open(&saved.path).await?.sync_all().await?;
            Ok::<_, ApiError>(())
        }
        .await;
        if let Err(error) = restored {
            failures.push(error.1);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(conflict(failures.join("；")))
    }
}

fn json_path(target: &SavedTarget, settings: &Value) -> Result<PathBuf> {
    let path = Path::new(&target.path);
    let parent = path
        .parent()
        .ok_or_else(|| conflict("媒体路径没有父目录"))?;
    let root = string(settings, "MediaInfoJsonRootFolder");
    // Matching PRO GetMediaInfoJsonPath makes the absolute containing folder
    // relative to its filesystem root before joining MediaInfoJsonRootFolder.
    let parent = if root.is_empty() {
        parent.to_path_buf()
    } else {
        let relative = parent
            .strip_prefix("/")
            .map_err(|_| conflict("无法确定神医 JSON 原生绝对路径，未清理媒体信息"))?;
        Path::new(&root).join(relative)
    };
    Ok(parent.join(format!(
        "{}-mediainfo.json",
        path.file_stem().unwrap_or_default().to_string_lossy()
    )))
}

// Emby 4.10.2.0 BaseItem.GetInternalMetadataPath uses IdString = Guid.ToString("N").
fn metadata_directory(root: &Path, item: &Value) -> Result<PathBuf> {
    let guid = uuid::Uuid::parse_str(&string(item, "Guid"))
        .map_err(|_| conflict("Emby 未返回有效 GUID，无法确认目标元数据目录，未清理媒体信息"))?
        .simple()
        .to_string();
    Ok(root.join("library").join(&guid[..2]).join(guid))
}

async fn restore_json(path: &Path, backup: &Path) -> Result<()> {
    check_ancestors(path).await?;
    crate::media_save::commit_json_stage(backup, path).await
}

async fn json_unchanged(
    path: &Path,
    backup: &Path,
    files: &BTreeMap<PathBuf, Snapshot>,
) -> Result<bool> {
    check_ancestors(path).await?;
    if !tokio::fs::try_exists(path).await? {
        return Ok(true);
    }
    if tokio::fs::symlink_metadata(path)
        .await?
        .file_type()
        .is_symlink()
    {
        return Err(conflict("恢复 JSON 变为符号链接，已保留备份"));
    }
    let current = tokio::fs::read(path).await?;
    let original_path = tokio::fs::canonicalize(path).await?;
    let original = files.get(&original_path);
    Ok(current == tokio::fs::read(original.map(|f| f.backup.as_path()).unwrap_or(backup)).await?)
}

async fn wait_cleared(
    state: &AppState,
    targets: &[&SavedTarget],
    context: &JobContext,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let mut pending = vec![];
        for target in targets {
            let live = tokio::time::timeout_at(deadline, identity_details(state, target))
                .await
                .map_err(|_| conflict("清理结果回读超时，未提前覆盖尚未确认的媒体信息文件"))??;
            let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
            let empty_chapters = live["Chapters"].as_array().is_some_and(Vec::is_empty)
                || live["Type"] == "Audio" && live["Chapters"].is_null();
            if live["Size"].as_u64() != Some(0)
                || !empty_chapters
                || media_info::has_media_streams(&live, &paths)
            {
                pending.push(target.id.as_str());
            }
        }
        if pending.is_empty() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(conflict(format!(
                "目标 {} 的清理结果未确认，未提前覆盖 JSON，恢复备份已保留",
                pending.join(",")
            )));
        }
        context
            .activity(
                "确认目标清理",
                &format!("等待目标 {} 的媒体流、章节和大小归零", pending.join(",")),
            )
            .await;
        tokio::time::sleep(
            Duration::from_secs(1)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
    }
}

async fn refresh_one(
    state: &AppState,
    target: &SavedTarget,
    context: &JobContext,
    confirmed: &AtomicBool,
    recovery: &Path,
) -> Result<()> {
    identity_details(state, target).await?;
    let position = crate::media_extra::log_position(state).await?;
    crate::emby::prepare_refresh_guard(Some((confirmed, recovery)), &target.id, Some(position))
        .await?;
    state
        .emby(
            Method::POST,
            &format!("Items/{}/Refresh", target.id),
            &[
                ("Recursive", "false".into()),
                ("MetadataRefreshMode", "FullRefresh".into()),
                ("ImageRefreshMode", "ValidationOnly".into()),
                ("ReplaceAllMetadata", "false".into()),
                ("ReplaceAllImages", "false".into()),
            ],
            None,
        )
        .await?;
    crate::emby::wait_item_refresh(
        state,
        &target.id,
        None,
        Some(crate::media_extra::RefreshTrace::new(position)),
        context,
        "从 JSON 重载媒体信息",
        &format!("媒体ID {}", target.id),
    )
    .await
}

async fn verify_reloaded(
    state: &AppState,
    targets: &[&SavedTarget],
    context: &JobContext,
    before: &BTreeMap<String, Value>,
    thumbnails: &BTreeSet<String>,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut pending = targets
        .iter()
        .map(|t| (t.id.clone(), String::new()))
        .collect::<BTreeMap<_, _>>();
    while !pending.is_empty() && tokio::time::Instant::now() < deadline {
        for target in targets
            .iter()
            .filter(|t| pending.contains_key(&t.id))
            .copied()
            .collect::<Vec<_>>()
        {
            let live = tokio::time::timeout_at(deadline, saved_details(state, target))
                .await
                .map_err(|_| conflict("重载回读超时，JSON 和恢复备份已保留"))??;
            let mut reasons = differences(target, &live);
            for chapter in media_edit::list(&before[&target.id]["Chapters"])
                .iter()
                .filter(|c| !string(c, "ImageTag").is_empty())
            {
                if !media_edit::list(&live["Chapters"]).iter().any(|actual| {
                    actual["StartPositionTicks"] == chapter["StartPositionTicks"]
                        && actual["MarkerType"].as_str().unwrap_or("Chapter")
                            == chapter["MarkerType"].as_str().unwrap_or("Chapter")
                        && !string(actual, "ImageTag").is_empty()
                }) {
                    reasons.push(format!(
                        "原有章节图片未恢复：{}",
                        chapter["StartPositionTicks"]
                    ));
                }
            }
            if thumbnails.contains(&target.id) {
                let readback = tokio::time::timeout_at(
                    deadline,
                    state.emby(
                        Method::GET,
                        &format!("Items/{}/ThumbnailSet", target.id),
                        &[("Width", "320".into())],
                        None,
                    ),
                )
                .await
                .map_err(|_| conflict("BIF 缩略图回读超时，恢复备份已保留"))??;
                if !readback["Thumbnails"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
                {
                    reasons.push("原有 BIF 缩略图集合未恢复".into());
                }
            }
            if reasons.is_empty() {
                pending.remove(&target.id);
            } else {
                pending.insert(target.id.clone(), reasons.join("；"));
            }
        }
        if !pending.is_empty() {
            context
                .activity(
                    "验收目标重载",
                    &format!(
                        "已恢复文件，等待 {} 个目标的媒体流、标题和章节回读",
                        pending.len()
                    ),
                )
                .await;
            tokio::time::sleep(
                Duration::from_secs(1)
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            )
            .await;
        }
    }
    if !pending.is_empty() {
        return Err(conflict(format!(
            "目标重载未通过回读：{}；原资源和恢复备份已保留",
            pending
                .iter()
                .map(|(id, reason)| format!("{id}：{reason}"))
                .collect::<Vec<_>>()
                .join("；")
        )));
    }
    Ok(())
}

pub(crate) async fn reload(
    state: &AppState,
    targets: &[SavedTarget],
    context: &JobContext,
    confirmed: &AtomicBool,
    recovery: &Path,
) -> Result<Vec<String>> {
    let mut observations = BTreeMap::new();
    let mut groups = BTreeSet::new();
    let mut leaders = BTreeMap::new();
    let mut import_only = BTreeSet::new();
    for target in targets {
        context.checkpoint().await?;
        let live = saved_details(state, target).await?;
        if !differences(target, &live).is_empty() {
            let observed = identity_details(state, target).await?;
            let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
            if media_info::has_media_streams(&live, &paths) {
                let key = group(&observed)?;
                leaders.entry(key.clone()).or_insert(target.id.clone());
                groups.insert(key);
            } else {
                import_only.insert(target.id.clone());
            }
            observations.insert(target.id.clone(), observed);
        }
    }
    if groups.is_empty() && import_only.is_empty() {
        tracing::info!(
            "[媒体信息重载] 所有 {} 个目标已一致，无需清理或重载",
            targets.len()
        );
        return Ok(vec![]);
    }
    let settings = crate::strm::config(state).await?;
    if settings["PersistMediaInfoMode"] != "Restore" {
        return Err(conflict("神医未处于媒体信息恢复模式，未清理媒体信息"));
    }
    let all = targets.iter().collect::<Vec<_>>();
    let scope = if groups.is_empty() {
        BTreeMap::new()
    } else {
        check_scope(state, &all, &groups).await?
    };
    let clear_ids = scope.values().flatten().cloned().collect::<BTreeSet<_>>();
    let affected_ids = clear_ids
        .union(&import_only)
        .cloned()
        .collect::<BTreeSet<_>>();
    let affected = targets
        .iter()
        .filter(|t| affected_ids.contains(&t.id))
        .collect::<Vec<_>>();
    for target in &affected {
        let live = identity_details(state, target).await?;
        observations.insert(target.id.clone(), live);
    }
    let root = if groups.is_empty()
        && !affected.iter().any(|target| {
            media_edit::list(&observations[&target.id]["Chapters"])
                .iter()
                .any(|c| !string(c, "ImageTag").is_empty())
        }) {
        None
    } else {
        let configured = string(&*state.config.read().await, "emby_metadata_path");
        let root = if configured.is_empty() {
            let system = state.emby(Method::GET, "System/Info", &[], None).await?;
            PathBuf::from(string(&system, "InternalMetadataPath"))
        } else {
            PathBuf::from(configured)
        };
        if !root.is_absolute() {
            return Err(conflict("未配置可访问的 Emby 元数据根目录，未清理媒体信息"));
        }
        let root = tokio::fs::canonicalize(&root).await.map_err(|_| {
            conflict(format!(
                "Emby 元数据根目录未挂载或不可访问：{}，未清理媒体信息",
                root.display()
            ))
        })?;
        if !root.is_dir() {
            return Err(conflict("Emby 元数据根目录不是目录，未清理媒体信息"));
        }
        Some(root)
    };
    crate::emby::idle(state, context).await?;
    tokio::fs::create_dir_all(recovery).await?;
    let mut files = BTreeMap::new();
    let mut json_files = vec![];
    let mut bridges = vec![];
    let mut thumbnails = BTreeSet::new();
    for target in &affected {
        context.checkpoint().await?;
        let live = &observations[&target.id];
        if clear_ids.contains(&target.id) {
            let directory = metadata_directory(root.as_ref().unwrap(), live)?;
            if recovery.starts_with(&directory) {
                return Err(conflict(
                    "恢复备份目录位于被清理的目标元数据目录内，未清理媒体信息",
                ));
            }
            snapshot_directory(&directory, recovery, &mut files, &target.id).await?;
            writable_parent(&directory.join(".etk-restore-check")).await?;
            snapshot_local_bif(target, recovery, &mut files).await?;
        }
        if live["Type"] != "Audio"
            && let Some(root) = &root
        {
            let directory = metadata_directory(root, live)?;
            let bridge = tokio::select! {
                _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
                result=crate::chapter_bif::prepare(state,target,live,&directory,recovery)=>result?,
            };
            if let Some(bridge) = bridge {
                writable_parent(&bridge.destination).await?;
                snapshot_file(&bridge.destination, recovery, &mut files, &target.id).await?;
                bridges.push((target.id.clone(), bridge));
            }
            let before = state
                .emby(
                    Method::GET,
                    &format!("Items/{}/ThumbnailSet", target.id),
                    &[("Width", "320".into())],
                    None,
                )
                .await?;
            let items = before["Thumbnails"]
                .as_array()
                .ok_or_else(|| conflict("Emby 未返回有效缩略图集合，未清理媒体信息"))?;
            if !items.is_empty() {
                thumbnails.insert(target.id.clone());
            }
        }
        let path = json_path(target, &settings)?;
        writable_parent(&path).await?;
        if clear_ids.contains(&target.id) {
            let bytes = tokio::fs::read(&path).await.map_err(|_| {
                conflict(format!(
                    "{}：正式媒体信息 JSON 不存在或尚未共享到工具，未清理媒体信息",
                    path.display()
                ))
            })?;
            let original: Value = serde_json::from_slice(&bytes)
                .map_err(|_| conflict("正式媒体信息 JSON 无效，未清理媒体信息"))?;
            if !media_info::recoverable(&original)
                || !crate::media_paths::matches_container(
                    &original,
                    target.source.as_deref().unwrap_or(&target.path),
                )
            {
                return Err(conflict(
                    "正式媒体信息 JSON 无法验证当前媒体格式，未清理媒体信息",
                ));
            }
            crate::identity::verify(
                &json!({"size_bytes":media_info::container(&target.info).unwrap()["Size"]}),
                &original,
            )?;
            if let Some(asset) = &target.asset {
                crate::identity::verify(asset, &original)?;
            }
        }
        snapshot_file(&path, recovery, &mut files, &target.id).await?;
        let backup = recovery.join(format!("requested-{}.json", target.id));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .await?;
        let mut restoring = media_info::restore_json(&target.info)?;
        if live["Type"] == "Audio" && !string(&live["ImageTags"], "Primary").is_empty() {
            for entry in restoring.as_array_mut().unwrap() {
                entry.as_object_mut().unwrap().remove("EmbeddedImage");
            }
        }
        file.write_all(&serde_json::to_vec(&restoring).unwrap())
            .await?;
        file.sync_all().await?;
        json_files.push((target.id.clone(), path, backup));
    }
    for target in &affected {
        saved_details(state, target).await?;
        let current = identity_details(state, target).await?;
        let before = &observations[&target.id];
        if [
            "Guid",
            "PresentationUniqueKey",
            "ExtraType",
            "Chapters",
            "MediaStreams",
            "MediaSources",
            "ImageTags",
        ]
        .iter()
        .any(|key| current[*key] != before[*key])
        {
            return Err(conflict("媒体关联范围在准备重载期间变化，未清理媒体信息"));
        }
    }
    if !groups.is_empty() && check_scope(state, &all, &groups).await? != scope {
        return Err(conflict("媒体关联范围在准备重载期间变化，未清理媒体信息"));
    }
    for saved in files.values() {
        writable_parent(&saved.path).await?;
        if tokio::fs::read(&saved.path).await? != tokio::fs::read(&saved.backup).await? {
            return Err(conflict("目标恢复资源在准备期间变化，未清理媒体信息"));
        }
    }
    context.checkpoint().await?;
    let marker = recovery.join("media-reload.json");
    tokio::fs::write(&marker, serde_json::to_vec(&json!({
        "target_ids":affected.iter().map(|t|&t.id).collect::<Vec<_>>(),
        "files":files.values().map(|f|json!({"path":f.path,"backup":f.backup,"owner_ids":f.owners})).collect::<Vec<_>>(),
        "json":json_files.iter().map(|(id,path,backup)|json!({"item_id":id,"path":path,"backup":backup})).collect::<Vec<_>>(),
        "bridges":bridges.iter().map(|(id,b)|json!({"item_id":id,"source":b.source,"destination":b.destination})).collect::<Vec<_>>()
    })).unwrap()).await?;
    confirmed.store(false, Ordering::Release);
    let recovery_context = JobContext::for_recovery(Some(context)).await;
    let outcome = async {
        let mut failures = vec![];
        let mut cache_warnings = vec![];
        let mut attempted = vec![];
        for (key, id) in &leaders {
            recovery_context
                .activity(
                    "清理目标旧媒体信息",
                    &format!("已备份关联资源，仅清理目标组 {id} 后从 JSON 重载"),
                )
                .await;
            attempted.push(key);
            if let Err(error) = state
                .emby(
                    Method::POST,
                    &format!("Items/{id}/ClearMediaInfo"),
                    &[],
                    None,
                )
                .await
            {
                failures.push(format!("{id}：清理请求未完整成功：{}", error.1));
                break;
            }
        }
        // The matching PRO API is synchronous. Require the persisted empty
        // state for every expanded member before replacing any deleted file.
        let mut restore_ids = import_only.clone();
        for key in attempted {
            let members = affected
                .iter()
                .filter(|t| scope[key].contains(&t.id))
                .copied()
                .collect::<Vec<_>>();
            match wait_cleared(state, &members, &recovery_context).await {
                Ok(()) => restore_ids.extend(scope[key].iter().cloned()),
                Err(error) => failures.push(error.1),
            }
        }
        if let Err(error) = restore_files(&files, &restore_ids).await {
            failures.push(error.1);
        }
        for (id, bridge) in bridges.iter().filter(|(id, _)| restore_ids.contains(id)) {
            let restored = async {
                if !json_unchanged(&bridge.destination, &bridge.source, &files).await? {
                    return Err(conflict(
                        "BIF 旁车在重载期间被修改，保留用户新文件及恢复备份",
                    ));
                }
                check_ancestors(&bridge.destination).await?;
                crate::media_save::commit_json_stage(&bridge.source, &bridge.destination).await
            }
            .await;
            if let Err(error) = restored {
                failures.push(format!("{id}：原章节图片的 BIF 桥接恢复失败：{}", error.1));
            }
        }
        let mut ready = BTreeSet::new();
        for (id, path, backup) in json_files
            .iter()
            .filter(|(id, _, _)| restore_ids.contains(id))
        {
            match json_unchanged(path, backup, &files).await {
                Ok(true) => (),
                Ok(false) => {
                    failures.push(format!(
                        "{id}：JSON 在重载期间被修改，保留用户新文件及恢复备份"
                    ));
                    continue;
                }
                Err(error) => {
                    failures.push(format!("{id}：JSON 恢复前检查失败：{}", error.1));
                    continue;
                }
            }
            match restore_json(path, backup).await {
                Ok(()) => {
                    ready.insert(id.clone());
                    let target = affected.iter().find(|t| t.id == *id).unwrap();
                    if let Err(error) =
                        crate::media_save::cache_saved(state, target.sha.as_deref(), &target.info)
                            .await
                    {
                        cache_warnings
                            .push(format!("{id}：JSON 已恢复，缓存同步失败：{}", error.1));
                    }
                }
                Err(error) => failures.push(format!("{id}：期望 JSON 恢复失败：{}", error.1)),
            }
        }
        let ready = affected
            .iter()
            .filter(|t| ready.contains(&t.id))
            .copied()
            .collect::<Vec<_>>();
        for target in &ready {
            if let Err(error) =
                refresh_one(state, target, &recovery_context, confirmed, recovery).await
            {
                failures.push(format!(
                    "{}：从 JSON 重载请求或刷新未确认：{}",
                    target.id, error.1
                ));
            }
        }
        if let Err(error) =
            verify_reloaded(state, &ready, &recovery_context, &observations, &thumbnails).await
        {
            failures.push(error.1);
        }
        if ready.len() != affected.len() {
            failures.push("部分目标尚未完成清理或重载，原资源备份已保留".into());
        }
        if !failures.is_empty() {
            failures.extend(cache_warnings);
            return Err(conflict(failures.join("；")));
        }
        tokio::fs::remove_file(&marker).await?;
        Ok(cache_warnings)
    }
    .await;
    confirmed.store(outcome.is_ok(), Ordering::Release);
    if let Ok(warnings) = &outcome {
        tracing::info!(
            "[媒体信息重载] {} 个目标已从 JSON 重载并通过媒体流、标题和章节验收",
            affected.len()
        );
        for warning in warnings {
            tracing::warn!("[媒体信息重载] 原生媒体信息已确认，但缓存未全部同步：{warning}");
        }
    } else if let Err(error) = &outcome {
        tracing::warn!(
            "[媒体信息重载] 目标 {} 重载未完整通过：{}；恢复备份：{}",
            affected
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
                .join(","),
            error.1,
            recovery.display()
        );
    }
    outcome
}
