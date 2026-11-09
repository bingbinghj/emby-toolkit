use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit, media_info,
    media_save::{SavedTarget, saved_details},
    state::string,
};
use axum::http::StatusCode;
use md5::{Digest, Md5};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

const METADATA_FIELDS: &[&str] = &[
    "Name",
    "Overview",
    "Genres",
    "People",
    "ProviderIds",
    "ImageTags",
    "Album",
    "Artists",
    "AlbumArtists",
    "Composers",
    "IndexNumber",
    "ParentIndexNumber",
];

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

async fn identity_details(state: &AppState, target: &SavedTarget) -> Result<Value> {
    if !Path::new(&target.path).is_absolute() {
        return Err(conflict("重载目标不是原生绝对文件路径，未提交媒体信息同步"));
    }
    if let Some(asset) = &target.asset {
        let current = media_edit::repair_target(state, &target.id).await?;
        if current.path != target.path || current.sha != target.sha || current.asset != *asset {
            return Err(conflict("重载期间媒体资产身份变化，已保留恢复备份"));
        }
    }
    let live = media_edit::details(state, &target.id,
        "Id,Name,Overview,Genres,People,ProviderIds,Album,Artists,AlbumArtists,Composers,IndexNumber,ParentIndexNumber,Path,Type,Guid,PresentationUniqueKey,ExtraType,PartCount,Size,RunTimeTicks,Chapters,MediaSources,MediaStreams,ImageTags").await?;
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
        return Err(conflict("重载目标不是单个文件媒体，未提交媒体信息同步"));
    }
    Ok(live)
}

fn differences(target: &SavedTarget, live: &Value) -> Vec<String> {
    let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
    let mut reasons = media_info::stream_differences(&target.info, live, &paths);
    if let (Some(expected), Some(actual)) = (
        media_info::container(&target.info).and_then(|source| source["MediaStreams"].as_array()),
        media_info::item_streams(live, &paths),
    ) {
        if expected.len() != actual.len() {
            reasons.push(format!(
                "全部媒体流数量不一致（期望 {}，实际 {}）",
                expected.len(),
                actual.len()
            ));
        }
        for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
            if expected["Type"] != actual["Type"] {
                reasons.push(format!("媒体流#{} 类型或顺序不一致", index + 1));
            }
            for field in [
                "Index",
                "Profile",
                "Level",
                "BitRate",
                "BitDepth",
                "SampleRate",
                "ChannelLayout",
                "Width",
                "Height",
                "AspectRatio",
                "AverageFrameRate",
                "RealFrameRate",
                "PixelFormat",
                "TimeBase",
                "NalLengthSize",
                "AttachmentSize",
                "VideoRange",
                "ExtendedVideoType",
                "ExtendedVideoSubType",
                "IsAnamorphic",
                "IsInterlaced",
                "IsHearingImpaired",
            ] {
                let equal = if matches!(field, "AverageFrameRate" | "RealFrameRate") {
                    expected[field].as_f64().map(|v| v as f32)
                        == actual[field].as_f64().map(|v| v as f32)
                } else {
                    expected[field] == actual[field]
                };
                if !expected[field].is_null() && !equal {
                    reasons.push(format!("媒体流#{} {field} 未一致", index + 1));
                }
            }
            if expected["IsExternal"].as_bool().unwrap_or(false)
                != actual["IsExternal"].as_bool().unwrap_or(false)
            {
                reasons.push(format!("媒体流#{} 外挂标记未恢复", index + 1));
            }
            if expected["IsExternal"] == true {
                if expected["Path"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty())
                    && expected["Path"] != actual["Path"]
                {
                    reasons.push(format!("媒体流#{} 外挂路径未恢复", index + 1));
                }
                if !expected["Protocol"].is_null() && expected["Protocol"] != actual["Protocol"] {
                    reasons.push(format!("媒体流#{} 外挂协议未恢复", index + 1));
                }
            }
        }
    }
    let expected = &media_info::object(&target.info).unwrap()["Chapters"];
    let empty = json!([]);
    let actual = if live["Type"] == "Audio" && live["Chapters"].is_null() {
        &empty
    } else {
        &live["Chapters"]
    };
    if !matches!((media_info::signature(expected), media_info::signature(actual)),
        (Ok(expected), Ok(actual)) if expected == actual)
    {
        reasons.push("章节或片头片尾标记未一致".into());
    }
    reasons
}

pub(crate) async fn validate_targets_scope(
    state: &AppState,
    targets: &[SavedTarget],
) -> Result<()> {
    for target in targets {
        identity_details(state, target).await?;
    }
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
            "恢复资源不是普通文件：{}，未提交媒体信息同步",
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
            "工具没有恢复资源父目录的写入权限：{}，未提交媒体信息同步",
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
                "目标元数据包含符号链接：{}，未提交媒体信息同步",
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
            let mut source = tokio::fs::File::open(&saved.backup).await?;
            let mut destination = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&saved.path)
                .await?;
            tokio::io::copy(&mut source, &mut destination).await?;
            destination.sync_all().await?;
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
            .map_err(|_| conflict("无法确定神医 JSON 原生绝对路径，未提交媒体信息同步"))?;
        Path::new(&root).join(relative)
    };
    Ok(parent.join(format!(
        "{}-mediainfo.json",
        path.file_stem().unwrap_or_default().to_string_lossy()
    )))
}

async fn metadata_directory(state: &AppState, root: &Path, item: &Value) -> Result<PathBuf> {
    // 4.10.2 only exposes Guid for collection types. GetNewItemId hashes
    // Type.FullName + Path as UTF-16LE, with optional case folding. A hash is
    // only a candidate: resolve it through Emby before trusting its directory.
    let type_name = match item["Type"].as_str() {
        Some("Movie") => "MediaBrowser.Controller.Entities.Movies.Movie",
        Some("Episode") => "MediaBrowser.Controller.Entities.TV.Episode",
        Some("Video") => "MediaBrowser.Controller.Entities.Video",
        Some("MusicVideo") => "MediaBrowser.Controller.Entities.MusicVideo",
        Some("Audio") => "MediaBrowser.Controller.Entities.Audio.Audio",
        _ => return Err(conflict("无法确认目标媒体类型，未提交媒体信息同步")),
    };
    let mut candidates = vec![];
    if let Ok(guid) = uuid::Uuid::parse_str(&string(item, "Guid")) {
        candidates.push(guid);
    }
    let path = string(item, "Path");
    for path in [path.clone(), path.to_lowercase()] {
        let key = format!("{type_name}{path}");
        let bytes = key
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let guid = uuid::Uuid::from_bytes_le(Md5::digest(bytes).into());
        if !candidates.contains(&guid) {
            candidates.push(guid);
        }
    }
    let user = string(&*state.config.read().await, "emby_user_id");
    for guid in candidates {
        let guid = guid.simple().to_string();
        let response = state
            .emby(
                Method::GET,
                "Items",
                &[
                    ("Ids", guid.clone()),
                    ("UserId", user.clone()),
                    ("Fields", "Path".into()),
                ],
                None,
            )
            .await?;
        let items = response["Items"]
            .as_array()
            .ok_or_else(|| conflict("Emby 未返回有效 GUID 查找结果，未提交媒体信息同步"))?;
        if items.is_empty() {
            continue;
        }
        if items.len() != 1
            || ["Id", "Path", "Type"]
                .iter()
                .any(|key| items[0][*key] != item[*key])
        {
            return Err(conflict(
                "目标 GUID 回读的媒体 ID、路径或类型不一致，未提交媒体信息同步",
            ));
        }
        tracing::debug!(
            "[媒体信息重载] 媒体ID {} 的元数据 GUID 已通过原生回读核对",
            item["Id"]
        );
        return Ok(root.join("library").join(&guid[..2]).join(guid));
    }
    Err(conflict(
        "目标 GUID 未能通过原生回读核对，无法确认元数据目录，未提交媒体信息同步",
    ))
}

async fn restore_json(path: &Path, backup: &Path) -> Result<()> {
    check_ancestors(path).await?;
    if tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(conflict("恢复 JSON 变为符号链接，已保留备份"));
    }
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

fn sync_contract_supported(swagger: &Value) -> bool {
    let operation = &swagger["paths"]["/Items/SyncMediaInfo"]["post"];
    let parameters = media_edit::list(&operation["parameters"]);
    let bundle_array = |schema: &Value| {
        schema["type"] == "array"
            && schema["items"]["$ref"] == "#/definitions/StrmAssistant.Dto.MediaInfoBundle"
    };
    parameters
        .iter()
        .any(|p| p["in"] == "query" && p["name"] == "Id" && p["type"] == "string")
        && parameters
            .iter()
            .any(|p| p["in"] == "body" && bundle_array(&p["schema"]))
        && !parameters
            .iter()
            .any(|p| p["name"] == "Path" && p["required"] == true)
        && bundle_array(&operation["responses"]["200"]["schema"])
}

pub(crate) fn ensure_single_bundle(info: &Value) -> Result<()> {
    if info.as_array().is_some_and(|bundles| bundles.len() != 1) {
        return Err(conflict(
            "神医定点恢复只读取首个 Bundle，未同步多 Bundle 媒体信息",
        ));
    }
    Ok(())
}

fn ensure_unfiltered(chapters: &Value, streams: Option<&Vec<Value>>, id: &str) -> Result<()> {
    if media_edit::list(chapters).iter().any(|chapter| {
        chapter["MarkerType"]
            .as_i64()
            .is_some_and(|value| matches!(value, 1..=3))
            || chapter["MarkerType"].as_str().is_some_and(|value| {
                ["IntroStart", "IntroEnd", "CreditsStart", "1", "2", "3"]
                    .iter()
                    .any(|marker| value.trim().eq_ignore_ascii_case(marker))
            })
    }) {
        return Err(conflict(format!(
            "{id}：神医同步会移除片头片尾标记，未提交媒体信息同步"
        )));
    }
    if streams.into_iter().flatten().any(|stream| {
        let local = stream["Protocol"].is_null()
            || stream["Protocol"] == 0
            || stream["Protocol"]
                .as_str()
                .is_some_and(|p| p == "0" || p.eq_ignore_ascii_case("File"));
        stream["IsExternal"] == true
            && matches!(stream["Type"].as_str(), Some("Audio" | "Subtitle"))
            && local
    }) {
        return Err(conflict(format!(
            "{id}：神医同步会移除本地外挂音轨或字幕，未提交媒体信息同步"
        )));
    }
    Ok(())
}

pub(crate) fn ensure_json_unfiltered(info: &Value, id: &str) -> Result<()> {
    ensure_single_bundle(info)?;
    let bundle = media_info::object(info).ok_or_else(|| conflict("同步 JSON 无效"))?;
    let source = media_info::container(info).ok_or_else(|| conflict("同步 JSON 缺少媒体源"))?;
    ensure_unfiltered(&bundle["Chapters"], source["MediaStreams"].as_array(), id)?;
    ensure_unfiltered(&source["Chapters"], None, id)
}

fn ensure_target_unfiltered(target: &SavedTarget, live: &Value) -> Result<()> {
    let restoring = media_info::restore_json(&target.info)?;
    ensure_single_bundle(&restoring)?;
    let bundle = media_info::object(&restoring).unwrap();
    let source = media_info::container(&restoring).unwrap();
    media_info::signature(&bundle["Chapters"])?;
    let streams = source["MediaStreams"]
        .as_array()
        .ok_or_else(|| conflict("目标 JSON 缺少有效媒体流，未提交媒体信息同步"))?;
    ensure_unfiltered(&bundle["Chapters"], Some(streams), &target.id)?;
    ensure_unfiltered(&source["Chapters"], None, &target.id)?;
    let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
    ensure_unfiltered(
        &live["Chapters"],
        media_info::item_streams(live, &paths),
        &target.id,
    )?;
    if let Some(source) = media_info::item_source(live, &paths) {
        ensure_unfiltered(&source["Chapters"], None, &target.id)?;
    }
    ensure_unfiltered(&Value::Null, live["MediaStreams"].as_array(), &target.id)
}

async fn ensure_sync_supported(state: &AppState, settings: &Value) -> Result<()> {
    // PRO 3.0.0.57 synchronously applies only the first bundle from a nonempty
    // request body. An empty array does not import the target's JSON file.
    if settings["IsModSupported"] != true {
        return Err(conflict("神医当前未支持原生章节恢复，未提交媒体信息同步"));
    }
    let plugins = state.emby(Method::GET, "Plugins", &[], None).await?;
    if !media_edit::list(&plugins)
        .iter()
        .any(|p| p["Id"] == "63c322b7-a371-41a3-b11f-04f8418b37d8" && p["Version"] == "3.0.0.57")
    {
        return Err(conflict(
            "当前神医版本尚未验证媒体信息同步协议，未提交媒体信息同步",
        ));
    }
    let swagger = state.emby(Method::GET, "swagger.json", &[], None).await?;
    if !sync_contract_supported(&swagger) {
        return Err(conflict(
            "Emby 未提供已验证的媒体信息同步接口，未提交媒体信息同步",
        ));
    }
    Ok(())
}

async fn sync_one(
    state: &AppState,
    target: &SavedTarget,
    payload: Value,
    context: &JobContext,
    before: &Value,
) -> Result<()> {
    let live = identity_details(state, target).await?;
    ensure_target_unfiltered(target, &live)?;
    if METADATA_FIELDS
        .iter()
        .chain(["Chapters", "MediaStreams", "MediaSources"].iter())
        .any(|key| live[*key] != before[*key])
    {
        return Err(conflict(
            "媒体信息或元数据在同步前变化，已保留文件和恢复备份",
        ));
    }
    ensure_single_bundle(&payload)?;
    if !payload
        .as_array()
        .is_some_and(|bundles| bundles[0]["MediaSourceInfo"].is_object())
    {
        return Err(conflict("同步请求缺少完整 Bundle，未提交媒体信息同步"));
    }
    context
        .activity(
            "从 JSON 恢复目标媒体信息",
            &format!("媒体ID {}：提交原生同步，等待实际恢复返回", target.id),
        )
        .await;
    let restored = state
        .emby(
            Method::POST,
            "Items/SyncMediaInfo",
            &[("Id", target.id.clone())],
            Some(payload),
        )
        .await?;
    if !restored
        .as_array()
        .is_some_and(|bundles| bundles.len() == 1 && bundles[0]["MediaSourceInfo"].is_object())
    {
        return Err(conflict("原生目标同步未返回有效恢复结果，恢复备份已保留"));
    }
    tracing::info!(
        "[媒体信息重载] 媒体ID {} 原生 JSON 同步已返回，继续验收媒体流、章节和图片",
        target.id
    );
    Ok(())
}

fn thumbnail_times(response: &Value) -> Result<Value> {
    let items = response["Thumbnails"]
        .as_array()
        .ok_or_else(|| conflict("Emby 未返回有效 BIF 缩略图集合，恢复备份已保留"))?;
    if items
        .iter()
        .any(|frame| frame["PositionTicks"].as_u64().is_none())
    {
        return Err(conflict("Emby 未返回有效 BIF 时间索引，恢复备份已保留"));
    }
    Ok(Value::Array(
        items
            .iter()
            .map(|frame| frame["PositionTicks"].clone())
            .collect(),
    ))
}

async fn verify_reloaded(
    state: &AppState,
    targets: &[&SavedTarget],
    context: &JobContext,
    before: &BTreeMap<String, Value>,
    thumbnails: &BTreeMap<String, Value>,
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
            let readback = async {
                saved_details(state, target).await?;
                identity_details(state, target).await
            };
            let live = tokio::time::timeout_at(deadline, readback)
                .await
                .map_err(|_| conflict("重载回读超时，JSON 和恢复备份已保留"))??;
            let mut reasons = differences(target, &live);
            for field in METADATA_FIELDS {
                if live[*field] != before[&target.id][*field] {
                    reasons.push(format!("当前元数据字段 {field} 在同步期间被修改"));
                }
            }
            let expected = media_info::container(&target.info).unwrap();
            let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
            let source = media_info::item_source(&live, &paths).unwrap_or(&live);
            for field in ["Size", "RunTimeTicks"] {
                let actual = if source[field].is_null() {
                    &live[field]
                } else {
                    &source[field]
                };
                if !expected[field].is_null()
                    && crate::identity::size(&expected[field]) != crate::identity::size(actual)
                {
                    reasons.push(format!("媒体 {field} 未一致"));
                }
            }
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
            if let Some(expected) = thumbnails.get(&target.id) {
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
                if thumbnail_times(&readback)? != *expected {
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
    let mut affected = vec![];
    for target in targets {
        context.checkpoint().await?;
        let live = saved_details(state, target).await?;
        if !differences(target, &live).is_empty() {
            let observed = identity_details(state, target).await?;
            ensure_target_unfiltered(target, &observed)?;
            observations.insert(target.id.clone(), observed);
            affected.push(target);
        }
    }
    if affected.is_empty() {
        tracing::info!(
            "[媒体信息重载] 所有 {} 个目标已一致，无需同步",
            targets.len()
        );
        return Ok(vec![]);
    }
    let settings = crate::strm::config(state).await?;
    if settings["PersistMediaInfoMode"] != "Restore" {
        return Err(conflict("神医未处于媒体信息恢复模式，未提交媒体信息同步"));
    }
    ensure_sync_supported(state, &settings).await?;
    let mut thumbnails = BTreeMap::new();
    for target in affected
        .iter()
        .filter(|target| observations[&target.id]["Type"] != "Audio")
    {
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
            .ok_or_else(|| conflict("Emby 未返回有效缩略图集合，未提交媒体信息同步"))?;
        if !items.is_empty() {
            thumbnails.insert(target.id.clone(), thumbnail_times(&before)?);
        }
    }
    let root = if thumbnails.is_empty()
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
            return Err(conflict(
                "未配置可访问的 Emby 元数据根目录，未提交媒体信息同步",
            ));
        }
        let root = tokio::fs::canonicalize(&root).await.map_err(|_| {
            conflict(format!(
                "Emby 元数据根目录未挂载或不可访问：{}，未提交媒体信息同步",
                root.display()
            ))
        })?;
        if !root.is_dir() {
            return Err(conflict("Emby 元数据根目录不是目录，未提交媒体信息同步"));
        }
        Some(root)
    };
    crate::emby::idle(state, context).await?;
    tokio::fs::create_dir_all(recovery).await?;
    let mut files = BTreeMap::new();
    let mut json_files = vec![];
    let mut formal_paths = BTreeSet::new();
    let mut audio_json = vec![];
    let mut bridges = vec![];
    for target in &affected {
        context.checkpoint().await?;
        let live = &observations[&target.id];
        let directory = if let Some(root) = root.as_ref().filter(|_| live["Type"] != "Audio") {
            Some(metadata_directory(state, root, live).await?)
        } else {
            None
        };
        if live["Type"] != "Audio" {
            snapshot_local_bif(target, recovery, &mut files).await?;
        }
        if let Some(directory) = &directory {
            if recovery.starts_with(directory) {
                return Err(conflict(
                    "恢复备份目录位于目标元数据目录内，未提交媒体信息同步",
                ));
            }
            snapshot_directory(directory, recovery, &mut files, &target.id).await?;
            let bridge = tokio::select! {
                _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
                result=crate::chapter_bif::prepare(state,target,live,directory,recovery)=>result?,
            };
            if let Some(bridge) = bridge {
                writable_parent(&bridge.destination).await?;
                snapshot_file(&bridge.destination, recovery, &mut files, &target.id).await?;
                bridges.push((target.id.clone(), bridge));
            }
        }
        let path = json_path(target, &settings)?;
        writable_parent(&path).await?;
        if let Ok(bytes) = tokio::fs::read(&path).await {
            let original: Value = serde_json::from_slice(&bytes)
                .map_err(|_| conflict("正式媒体信息 JSON 无效，未提交媒体信息同步"))?;
            ensure_single_bundle(&original)?;
            if !media_info::recoverable(&original)
                || !crate::media_paths::matches_container(
                    &original,
                    target.source.as_deref().unwrap_or(&target.path),
                )
            {
                return Err(conflict(
                    "正式媒体信息 JSON 无法验证当前媒体格式，未提交媒体信息同步",
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
        if let Ok(path) = tokio::fs::canonicalize(&path).await {
            formal_paths.insert(path);
        }
        let backup = recovery.join(format!("requested-{}.json", target.id));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .await?;
        let mut restoring = media_info::restore_json(&target.info)?;
        let complete_info = restoring.clone();
        if live["Type"] == "Audio" {
            for entry in restoring.as_array_mut().unwrap() {
                let entry = entry.as_object_mut().unwrap();
                // Audio apply can overwrite current tags and artwork. Isolate
                // these fields only for this import, preserving the full JSON.
                entry.remove("EmbeddedInfo");
                entry.remove("EmbeddedImage");
            }
            if restoring != complete_info {
                let complete = recovery.join(format!("complete-audio-{}.json", target.id));
                let mut full = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&complete)
                    .await?;
                full.write_all(&serde_json::to_vec(&complete_info).unwrap())
                    .await?;
                full.sync_all().await?;
                audio_json.push((target.id.clone(), path.clone(), backup.clone(), complete));
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
        ensure_target_unfiltered(target, &current)?;
        if METADATA_FIELDS
            .iter()
            .chain(
                [
                    "Guid",
                    "PresentationUniqueKey",
                    "ExtraType",
                    "Chapters",
                    "MediaStreams",
                    "MediaSources",
                ]
                .iter(),
            )
            .any(|key| current[*key] != before[*key])
        {
            return Err(conflict(
                "媒体信息或元数据在准备同步期间变化，未提交媒体信息同步",
            ));
        }
    }
    for saved in files.values() {
        writable_parent(&saved.path).await?;
        if tokio::fs::read(&saved.path).await? != tokio::fs::read(&saved.backup).await? {
            return Err(conflict("目标恢复资源在准备期间变化，未提交媒体信息同步"));
        }
    }
    context.checkpoint().await?;
    let marker = recovery.join("media-reload.json");
    tokio::fs::write(&marker, serde_json::to_vec(&json!({
        "target_ids":affected.iter().map(|t|&t.id).collect::<Vec<_>>(),
        "files":files.values().map(|f|json!({"path":f.path,"backup":f.backup,"owner_ids":f.owners})).collect::<Vec<_>>(),
        "json":json_files.iter().map(|(id,path,backup)|json!({"item_id":id,"path":path,"backup":backup})).collect::<Vec<_>>(),
        "audio_complete":audio_json.iter().map(|(id,path,import,complete)|json!({"item_id":id,"path":path,"import":import,"complete":complete})).collect::<Vec<_>>(),
        "bridges":bridges.iter().map(|(id,b)|json!({"item_id":id,"source":b.source,"destination":b.destination})).collect::<Vec<_>>()
    })).unwrap()).await?;
    tokio::fs::File::open(&marker).await?.sync_all().await?;
    tokio::fs::File::open(recovery).await?.sync_all().await?;
    confirmed.store(false, Ordering::Release);
    let recovery_context = JobContext::for_recovery(Some(context)).await;
    let outcome = async {
        let mut failures = vec![];
        let mut cache_warnings = vec![];
        let mut blocked = BTreeSet::new();
        for (id, bridge) in &bridges {
            let restored = async {
                if !json_unchanged(&bridge.destination, &bridge.source, &files).await? {
                    return Err(conflict("BIF 旁车在同步前被修改，保留用户新文件及恢复备份"));
                }
                check_ancestors(&bridge.destination).await?;
                let bytes = tokio::fs::read(&bridge.source).await?;
                match tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&bridge.destination)
                    .await
                {
                    Ok(mut file) => {
                        file.write_all(&bytes).await?;
                        file.sync_all().await?;
                        Ok(())
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        if tokio::fs::read(&bridge.destination).await? == bytes {
                            Ok(())
                        } else {
                            Err(conflict("BIF 旁车已由其它操作创建，保留用户文件及恢复备份"))
                        }
                    }
                    Err(error) => Err(error.into()),
                }
            }
            .await;
            if let Err(error) = restored {
                blocked.insert(id.clone());
                failures.push(format!("{id}：原章节图片的 BIF 桥接准备失败：{}", error.1));
            }
        }
        let mut ready = BTreeSet::new();
        for (id, path, backup) in json_files.iter().filter(|(id, _, _)| !blocked.contains(id)) {
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
        let mut synchronized = BTreeSet::new();
        for target in &ready {
            let synced = async {
                let (_, path, backup) = json_files
                    .iter()
                    .find(|(id, _, _)| *id == target.id)
                    .unwrap();
                check_ancestors(path).await?;
                let bytes = tokio::fs::read(backup).await?;
                if tokio::fs::read(path).await? != bytes {
                    return Err(conflict("JSON 在同步前被修改，保留用户新文件及恢复备份"));
                }
                for (_, bridge) in bridges.iter().filter(|(id, _)| *id == target.id) {
                    if tokio::fs::read(&bridge.destination).await?
                        != tokio::fs::read(&bridge.source).await?
                    {
                        return Err(conflict("BIF 桥接在同步前变化，保留用户新文件及恢复备份"));
                    }
                }
                let payload: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| conflict("已保护的同步 JSON 无效，恢复备份已保留"))?;
                sync_one(
                    state,
                    target,
                    payload,
                    &recovery_context,
                    &observations[&target.id],
                )
                .await
            }
            .await;
            if let Err(error) = synced {
                failures.push(format!(
                    "{}：原生目标 JSON 同步未确认：{}",
                    target.id, error.1
                ));
            } else {
                synchronized.insert(target.id.clone());
            }
        }
        let resource_files = files
            .into_iter()
            .filter(|(path, _)| !formal_paths.contains(path))
            .collect::<BTreeMap<_, _>>();
        if let Err(error) = restore_files(&resource_files, &synchronized).await {
            failures.push(error.1);
        }
        let verified =
            verify_reloaded(state, &ready, &recovery_context, &observations, &thumbnails).await;
        let verified_ok = verified.is_ok();
        if let Err(error) = verified {
            failures.push(error.1);
        }
        for (id, path, import, complete) in audio_json
            .iter()
            .filter(|(id, _, _, _)| ready.iter().any(|target| target.id == *id))
        {
            if !verified_ok
                || !synchronized.contains(id)
                || json_files
                    .iter()
                    .any(|(owner, shared, _)| shared == path && !synchronized.contains(owner))
            {
                failures.push(format!(
                    "{id}：目标同步或回读尚未确认，暂不恢复音乐嵌入字段；完整 JSON 备份：{}",
                    complete.display()
                ));
                continue;
            }
            let restored = async {
                if tokio::fs::read(path).await? != tokio::fs::read(import).await? {
                    return Err(conflict(
                        "音乐 JSON 在同步期间变化，保留用户文件及完整恢复备份",
                    ));
                }
                restore_json(path, complete).await
            }
            .await;
            if let Err(error) = restored {
                failures.push(format!("{id}：完整音乐 JSON 恢复失败：{}", error.1));
            }
        }
        if ready.len() != affected.len() {
            failures.push("部分目标尚未完成同步，原资源备份已保留".into());
        }
        if !failures.is_empty() {
            failures.extend(cache_warnings);
            return Err(conflict(failures.join("；")));
        }
        // Preserve the completed synchronous operation across a later failure
        // to restore plugin settings. This is not a pending scanner request.
        tokio::fs::write(
            recovery.join("pending-scan.json"),
            serde_json::to_vec(&json!({
                "kind":"native-sync", "verified":true,
                "ids":ready.iter().map(|target| &target.id).collect::<Vec<_>>()
            }))
            .unwrap(),
        )
        .await?;
        tokio::fs::File::open(recovery.join("pending-scan.json"))
            .await?
            .sync_all()
            .await?;
        tokio::fs::remove_file(&marker).await?;
        tokio::fs::File::open(recovery).await?.sync_all().await?;
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
