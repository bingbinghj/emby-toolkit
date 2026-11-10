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

async fn recovery_backup(recovery: &Path, value: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(value.as_str().ok_or_else(|| conflict("恢复材料路径缺失"))?);
    check_ancestors(&path).await?;
    if !tokio::fs::symlink_metadata(&path).await?.is_file()
        || !tokio::fs::canonicalize(&path)
            .await?
            .starts_with(tokio::fs::canonicalize(recovery).await?)
    {
        return Err(conflict("恢复材料不在该备份目录内或不是普通文件"));
    }
    Ok(path)
}

fn recovery_bif_times(bytes: &[u8]) -> Result<Value> {
    if bytes.len() < 80 || bytes.get(..8) != Some(b"\x89BIF\r\n\x1a\n") {
        return Err(conflict("原有 BIF 恢复备份无效"));
    }
    let read = |n| u32::from_le_bytes(bytes[n..n + 4].try_into().unwrap());
    let count = read(12) as usize;
    let end = count
        .checked_add(1)
        .and_then(|n| n.checked_mul(8))
        .and_then(|n| n.checked_add(64))
        .filter(|n| *n <= bytes.len())
        .ok_or_else(|| conflict("原有 BIF 索引无效"))?;
    let multiplier = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    if count == 0
        || multiplier == 0
        || read(8) != 0
        || read(64 + count * 8) != u32::MAX
        || read(68 + count * 8) as usize != bytes.len()
    {
        return Err(conflict("原有 BIF 恢复备份无效"));
    }
    let mut times = vec![];
    for i in 0..count {
        let begin = read(68 + i * 8) as usize;
        let finish = read(76 + i * 8) as usize;
        if begin < end
            || begin >= finish
            || finish > bytes.len()
            || image::load_from_memory(&bytes[begin..finish]).is_err()
        {
            return Err(conflict("原有 BIF 图片内容无效"));
        }
        let ticks = u64::from(read(64 + i * 8))
            .checked_mul(multiplier)
            .and_then(|n| n.checked_mul(10000))
            .ok_or_else(|| conflict("原有 BIF 时间索引溢出"))?;
        if times
            .last()
            .is_some_and(|previous: &Value| previous.as_u64().unwrap() > ticks)
        {
            return Err(conflict("原有 BIF 时间索引顺序无效"));
        }
        times.push(json!(ticks));
    }
    Ok(json!(times))
}

fn recovery_chapter_ticks(path: &Path, info: &Value) -> Result<u64> {
    let invalid = || conflict("旧记录无法确定原章节图片对应时间，备份已保留");
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    // Emby 4.10.2 ThumbnailGenerator.GetChapterImagePath names saved images
    // <DateModified Unix seconds>_<StartPositionTicks>.jpg. The suffix is the
    // original tick value, not an ordinal or an extraction time in milliseconds.
    let time = if let Some((modified, ticks)) = stem.split_once('_') {
        let unix_seconds = modified.parse::<i64>().map_err(|_| invalid())?;
        if modified != unix_seconds.to_string() {
            return Err(invalid());
        }
        ticks
    } else {
        stem
    };
    if time.is_empty() || !time.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let ticks = time
        .parse::<u64>()
        .ok()
        .filter(|ticks| *ticks <= i64::MAX as u64)
        .ok_or_else(invalid)?;
    if !media_info::object(info)
        .and_then(|bundle| bundle["Chapters"].as_array())
        .is_some_and(|chapters| {
            chapters
                .iter()
                .any(|chapter| chapter["StartPositionTicks"].as_u64() == Some(ticks))
        })
    {
        return Err(conflict("旧记录章节图片时间与恢复 JSON 不一致，备份已保留"));
    }
    Ok(ticks)
}

pub(crate) async fn recover_record(
    state: &AppState,
    recovery: &Path,
    context: &JobContext,
) -> Result<()> {
    let marker = recovery.join("media-reload.json");
    let record = if tokio::fs::try_exists(&marker).await? {
        crate::media_recovery::read_json(&marker).await?
    } else {
        crate::media_recovery::read_json(&recovery.join("pending-scan.json")).await?["verification"]
            .clone()
    };
    let ids = record["target_ids"]
        .as_array()
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| conflict("恢复记录没有有效目标媒体，备份已保留"))?;
    let mut targets = vec![];
    let mut json_paths = BTreeSet::new();
    let mut requested = BTreeMap::new();
    for id in ids {
        context.checkpoint().await?;
        let id = id
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| conflict("恢复记录媒体 ID 无效"))?;
        let entries = media_edit::list(&record["json"]);
        let entries = entries
            .iter()
            .filter(|entry| entry["item_id"] == id)
            .collect::<Vec<_>>();
        if entries.len() != 1 {
            return Err(conflict(format!("{id}：恢复 JSON 记录缺失或重复")));
        }
        let entry = entries[0];
        let backup = recovery_backup(recovery, &entry["backup"]).await?;
        let info = crate::media_recovery::read_json(&backup).await?;
        ensure_single_bundle(&info)?;
        let path = PathBuf::from(string(entry, "path"));
        let current = media_edit::repair_target(state, id).await?;
        if !current.candidates.contains(&path) {
            return Err(conflict(format!(
                "{id}：备份 JSON 路径与当前媒体身份不一致"
            )));
        }
        crate::identity::verify(&current.asset, &info)?;
        let source = crate::media_paths::strm_target(&current.path).await;
        if let Some(saved) = media_edit::list(&record["targets"])
            .iter()
            .find(|entry| entry["id"] == id)
            && (saved["path"] != current.path
                || saved["source"] != json!(source)
                || saved["sha"] != json!(current.sha)
                || (saved["asset"].is_object() && saved["asset"] != json!(current.asset)))
        {
            return Err(conflict(format!("{id}：备份后的媒体文件或 STRM 来源变化")));
        }
        targets.push(SavedTarget {
            id: id.into(),
            path: current.path,
            source,
            sha: current.sha,
            asset: (!current.kind.is_empty()).then_some(current.asset),
            info: media_info::restore_json(&info)?,
            refresh_required: true,
        });
        json_paths.insert(path.clone());
        requested.insert(id.to_string(), (path, backup));
    }
    let owners = targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    if owners.len() != targets.len() {
        return Err(conflict("恢复记录包含重复媒体 ID"));
    }
    let mut files = BTreeMap::new();
    let mut thumbnails = BTreeMap::new();
    let mut image_times = BTreeMap::<String, BTreeSet<u64>>::new();
    let mut audio_complete = BTreeMap::new();
    for audio in media_edit::list(&record["audio_complete"]) {
        let id = string(&audio, "item_id");
        let target = targets
            .iter()
            .find(|target| target.id == id)
            .ok_or_else(|| conflict("完整音乐 JSON 的恢复目标不一致"))?;
        let complete = recovery_backup(recovery, &audio["complete"]).await?;
        let full = crate::media_recovery::read_json(&complete).await?;
        ensure_single_bundle(&full)?;
        let mut import = full.clone();
        for bundle in import.as_array_mut().unwrap() {
            bundle.as_object_mut().unwrap().remove("EmbeddedInfo");
            bundle.as_object_mut().unwrap().remove("EmbeddedImage");
        }
        if import != target.info || audio["path"] != json!(requested[&id].0) {
            return Err(conflict(
                "完整音乐 JSON 与导入备份不一致，已保留全部恢复材料",
            ));
        }
        audio_complete.insert(id, (complete, full));
    }
    for file in media_edit::list(&record["files"]) {
        let path = PathBuf::from(string(&file, "path"));
        if !path.is_absolute() {
            return Err(conflict("原资源恢复路径不是绝对路径"));
        }
        let backup = recovery_backup(recovery, &file["backup"]).await?;
        let ids = media_edit::list(&file["owner_ids"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        if ids.is_empty() || !ids.is_subset(&owners) {
            return Err(conflict("原资源恢复记录缺少有效所属媒体"));
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("bif"))
        {
            let times = recovery_bif_times(&tokio::fs::read(&backup).await?)?;
            for id in &ids {
                if thumbnails.get(id).is_none_or(|old: &Value| {
                    old.as_array().unwrap().len() < times.as_array().unwrap().len()
                }) {
                    thumbnails.insert(id.clone(), times.clone());
                }
            }
        }
        if path.components().any(|part| part.as_os_str() == "chapters")
            && path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp"
                )
            })
        {
            image::load_from_memory(&tokio::fs::read(&backup).await?)
                .map_err(|_| conflict("原有章节图片恢复备份无效"))?;
            for id in &ids {
                if record["observations"][id].is_null() {
                    let target = targets.iter().find(|target| target.id == *id).unwrap();
                    let ticks = recovery_chapter_ticks(&path, &target.info)?;
                    image_times.entry(id.clone()).or_default().insert(ticks);
                }
            }
        }
        files.insert(
            path.clone(),
            Snapshot {
                path,
                backup,
                owners: ids,
            },
        );
    }
    // A generated bridge is also original recovery evidence, not a fresh
    // snapshot of the currently missing pictures.
    for bridge in media_edit::list(&record["bridges"]) {
        let id = string(&bridge, "item_id");
        if !owners.contains(&id) {
            return Err(conflict("BIF 桥接恢复目标不一致"));
        }
        let backup = recovery_backup(recovery, &bridge["source"]).await?;
        let path = PathBuf::from(string(&bridge, "destination"));
        let target = targets.iter().find(|target| target.id == id).unwrap();
        if path.parent() != Path::new(&target.path).parent()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("bif"))
        {
            return Err(conflict("BIF 桥接位置与当前媒体文件不一致"));
        }
        let times = recovery_bif_times(&tokio::fs::read(&backup).await?)?;
        thumbnails.entry(id.clone()).or_insert(times);
        files.entry(path.clone()).or_insert(Snapshot {
            path,
            backup,
            owners: BTreeSet::from([id]),
        });
    }
    let mut before = BTreeMap::new();
    for target in &targets {
        let live = identity_details(state, target).await?;
        let mut observed = record["observations"][&target.id]
            .as_object()
            .map(|_| record["observations"][&target.id].clone())
            .unwrap_or(live);
        if record["observations"][&target.id].is_null() && image_times.contains_key(&target.id) {
            observed["Chapters"] = media_info::object(&target.info).unwrap()["Chapters"].clone();
            for chapter in observed["Chapters"].as_array_mut().unwrap() {
                if chapter["StartPositionTicks"]
                    .as_u64()
                    .is_some_and(|ticks| image_times[&target.id].contains(&ticks))
                {
                    chapter["ImageTag"] = json!("retained-original-image");
                }
            }
        }
        if let Some(times) = record["thumbnail_times"].get(&target.id) {
            thumbnails.insert(target.id.clone(), times.clone());
        }
        before.insert(target.id.clone(), observed);
    }
    for target in &targets {
        let (path, backup) = &requested[&target.id];
        check_ancestors(path).await?;
        if tokio::fs::symlink_metadata(path)
            .await
            .is_ok_and(|metadata| !metadata.is_file())
        {
            return Err(conflict("恢复 JSON 不是普通文件，备份已保留"));
        }
        let already_imported =
            tokio::fs::read(path).await.ok() == Some(tokio::fs::read(backup).await?);
        let full_audio = if let Some((full, _)) = audio_complete.get(&target.id) {
            tokio::fs::read(path).await.ok() == Some(tokio::fs::read(full).await?)
        } else {
            false
        };
        if !already_imported && !full_audio && !json_unchanged(path, backup, &files).await? {
            return Err(conflict("媒体 JSON 在失败后被修改，保留当前文件及备份"));
        }
    }
    context.checkpoint().await?;
    let finish = JobContext::for_recovery(Some(context)).await;
    for target in &targets {
        let (path, backup) = &requested[&target.id];
        restore_json(path, backup).await?;
        crate::media_save::cache_saved(state, target.sha.as_deref(), &target.info).await?;
    }
    let resources = files
        .into_iter()
        .filter(|(path, _)| !json_paths.contains(path))
        .collect::<BTreeMap<_, _>>();
    restore_files(&resources, &owners).await?;
    let confirmed = AtomicBool::new(false);
    let retry = recovery.join(format!("retry-{}", uuid::Uuid::new_v4().simple()));
    reload(state, &targets, &finish, &confirmed, &retry).await?;
    verify_reloaded(
        state,
        &targets.iter().collect::<Vec<_>>(),
        &finish,
        &before,
        &thumbnails,
    )
    .await?;
    for target in &targets {
        if let Some((complete, full)) = audio_complete.get(&target.id) {
            let (path, import) = &requested[&target.id];
            if tokio::fs::read(path).await? != tokio::fs::read(import).await? {
                return Err(conflict("音乐 JSON 在恢复期间变化，完整音乐备份仍保留"));
            }
            restore_json(path, complete).await?;
            crate::media_save::cache_saved(state, target.sha.as_deref(), full).await?;
        }
    }
    Ok(())
}

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
        "targets":affected.iter().map(|t|json!({"id":t.id,"path":t.path,"source":t.source,"sha":t.sha,"asset":t.asset})).collect::<Vec<_>>(),
        "observations":observations,
        "thumbnail_times":thumbnails,
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
                "verification":crate::media_recovery::read_json(&marker).await?,
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

#[cfg(test)]
mod recovery_tests {
    use super::*;

    fn original_chapters() -> Value {
        json!([{"Chapters":[{"StartPositionTicks":0_u64},{"StartPositionTicks":897560000_u64},
            {"StartPositionTicks":4649640000_u64}]}])
    }

    #[test]
    fn emby_native_chapter_names_preserve_exact_ticks_in_legacy_recovery() {
        let info = original_chapters();
        for ticks in [0_u64, 897560000, 4649640000] {
            let path = PathBuf::from(format!("/metadata/chapters/1762645565_{ticks}.jpg"));
            assert_eq!(recovery_chapter_ticks(&path, &info).unwrap(), ticks);
        }
    }

    #[test]
    fn historical_plain_tick_names_remain_recoverable() {
        let path = Path::new("/metadata/chapters/897560000.jpg");
        assert_eq!(
            recovery_chapter_ticks(path, &original_chapters()).unwrap(),
            897560000
        );
    }

    #[test]
    fn unknown_or_ambiguous_chapter_names_never_supply_a_guessed_time() {
        for name in [
            "chapter_0.jpg",
            "1762645565_0_extra.jpg",
            "1762645565_.jpg",
            "+1762645565_0.jpg",
            "1762645565_-1.jpg",
            "1762645565_9223372036854775808.jpg",
        ] {
            assert!(
                recovery_chapter_ticks(&PathBuf::from(name), &original_chapters()).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn chapter_ordinals_and_unrecorded_ticks_cannot_bypass_recovery_verification() {
        for name in ["1.jpg", "1762645565_1.jpg", "1762645565_89756.jpg"] {
            let error =
                recovery_chapter_ticks(&PathBuf::from(name), &original_chapters()).unwrap_err();
            assert!(
                error.1.contains("与恢复 JSON 不一致"),
                "{name}: {}",
                error.1
            );
        }
    }
}
