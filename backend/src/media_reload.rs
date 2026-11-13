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
use sha2::Sha256;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

mod assets;
pub(crate) mod chapter_images;
pub(crate) mod ingest_preservation;

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

type RecoveryJsonTargets = (Vec<SavedTarget>, BTreeMap<String, (PathBuf, PathBuf)>);

fn parse_recovery_json(bytes: &[u8], path: &Path) -> Result<Value> {
    // Reject duplicate keys: comparing a lossy map would hide a real value
    // change when another JSON consumer selects the first duplicate instead.
    struct CheckedJson(Value);
    impl<'de> serde::Deserialize<'de> for CheckedJson {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = CheckedJson;
                fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    formatter.write_str("JSON without duplicate keys")
                }
                fn visit_bool<E: serde::de::Error>(
                    self,
                    value: bool,
                ) -> std::result::Result<CheckedJson, E> {
                    Ok(CheckedJson(json!(value)))
                }
                fn visit_i64<E: serde::de::Error>(
                    self,
                    value: i64,
                ) -> std::result::Result<CheckedJson, E> {
                    Ok(CheckedJson(json!(value)))
                }
                fn visit_u64<E: serde::de::Error>(
                    self,
                    value: u64,
                ) -> std::result::Result<CheckedJson, E> {
                    Ok(CheckedJson(json!(value)))
                }
                fn visit_f64<E: serde::de::Error>(
                    self,
                    value: f64,
                ) -> std::result::Result<CheckedJson, E> {
                    serde_json::Number::from_f64(value)
                        .map(|value| CheckedJson(Value::Number(value)))
                        .ok_or_else(|| E::custom("nonfinite JSON number"))
                }
                fn visit_str<E: serde::de::Error>(
                    self,
                    value: &str,
                ) -> std::result::Result<CheckedJson, E> {
                    Ok(CheckedJson(json!(value)))
                }
                fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<CheckedJson, E> {
                    Ok(CheckedJson(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut sequence: A,
                ) -> std::result::Result<CheckedJson, A::Error> {
                    let mut values = vec![];
                    while let Some(value) = sequence.next_element::<CheckedJson>()? {
                        values.push(value.0);
                    }
                    Ok(CheckedJson(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut fields: A,
                ) -> std::result::Result<CheckedJson, A::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some((key, value)) = fields.next_entry::<String, CheckedJson>()? {
                        if values.insert(key, value.0).is_some() {
                            return Err(serde::de::Error::custom("duplicate JSON key"));
                        }
                    }
                    Ok(CheckedJson(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }
    serde_json::from_slice::<CheckedJson>(bytes)
        .map(|value| value.0)
        .map_err(|_| {
            conflict(format!(
                "媒体 JSON 无法严格解析，保留当前文件及备份：{}",
                path.display()
            ))
        })
}

fn json_equivalent(left: &[u8], right: &[u8], path: &Path) -> Result<bool> {
    Ok(parse_recovery_json(left, path)? == parse_recovery_json(right, path)?)
}

fn sync_json_matches(
    current: &[u8],
    submitted: &[u8],
    response: &[u8],
    path: &Path,
) -> Result<bool> {
    let mut current = parse_recovery_json(current, path)?;
    let submitted = parse_recovery_json(submitted, path)?;
    if current == submitted {
        return Ok(true);
    }
    let mut response = parse_recovery_json(response, path)?;
    if current == response {
        return Ok(true);
    }
    // PRO recalculates this envelope flag in the Sync DTO. A persisted value
    // is accepted only when it still equals the protected submitted value.
    let confidence_unchanged =
        current
            .as_array()
            .zip(submitted.as_array())
            .is_some_and(|(current, submitted)| {
                current.len() == 1
                    && submitted.len() == 1
                    && current[0].get("ZeroFingerprintConfidence")
                        == submitted[0].get("ZeroFingerprintConfidence")
            });
    // Native Sync responses and persisted exports can calculate these display
    // fields differently. All identity, editable and unknown fields stay exact.
    for value in [&mut current, &mut response] {
        for bundle in value.as_array_mut().into_iter().flatten() {
            if confidence_unchanged && let Some(bundle) = bundle.as_object_mut() {
                bundle.remove("ZeroFingerprintConfidence");
            }
            for stream in bundle
                .get_mut("MediaSourceInfo")
                .and_then(|source| source.get_mut("MediaStreams"))
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(stream) = stream.as_object_mut() {
                    stream.remove("DisplayTitle");
                    stream.remove("DisplayLanguage");
                }
            }
        }
    }
    Ok(current == response)
}

fn json_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn current_json(path: &Path) -> Result<Option<Vec<u8>>> {
    check_ancestors(path).await?;
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata.is_file() => {
            return Err(conflict(format!(
                "恢复 JSON 不是普通文件，备份已保留：{}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let bytes = tokio::fs::read(path).await?;
    parse_recovery_json(&bytes, path)?;
    Ok(Some(bytes))
}

fn json_preview(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return json!({"missing":true});
    };
    let text = value.to_string();
    if text.chars().count() > 256 {
        json!({"preview":text.chars().take(256).collect::<String>(),"bytes":text.len()})
    } else {
        value.clone()
    }
}

fn json_changes(
    expected: Option<&Value>,
    current: Option<&Value>,
    path: &str,
    changes: &mut Vec<Value>,
    count: &mut usize,
) {
    if expected == current {
        return;
    }
    match (expected, current) {
        (Some(Value::Object(left)), Some(Value::Object(right))) => {
            let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                let key_path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                json_changes(left.get(key), right.get(key), &key_path, changes, count);
            }
        }
        (Some(Value::Array(left)), Some(Value::Array(right))) => {
            for index in 0..left.len().max(right.len()) {
                json_changes(
                    left.get(index),
                    right.get(index),
                    &format!("{path}/{index}"),
                    changes,
                    count,
                );
            }
        }
        _ => {
            *count += 1;
            if changes.len() < 64 {
                changes.push(json!({"path":path,"expected":json_preview(expected),"current":json_preview(current)}));
            }
        }
    }
}

async fn inspect_recovery_json(
    recovery: &Path,
    record: &Value,
    requested: &BTreeMap<String, (PathBuf, PathBuf)>,
) -> Result<(Vec<Value>, BTreeMap<PathBuf, Option<Vec<u8>>>)> {
    let mut conflicts = vec![];
    let mut observed = BTreeMap::new();
    for (id, (path, backup)) in requested {
        let bytes = current_json(path).await?;
        observed.insert(path.clone(), bytes.clone());
        let Some(bytes) = bytes else {
            continue;
        };
        let expected_bytes = tokio::fs::read(backup).await?;
        if json_equivalent(&bytes, &expected_bytes, path)? {
            continue;
        }
        let canonical = tokio::fs::canonicalize(path).await?;
        let original = media_edit::list(&record["files"]).into_iter().find(|file| {
            file["path"]
                .as_str()
                .is_some_and(|file| Path::new(file) == canonical || Path::new(file) == path)
        });
        if let Some(original) = original {
            let original = recovery_backup(recovery, &original["backup"]).await?;
            if json_equivalent(&bytes, &tokio::fs::read(original).await?, path)? {
                continue;
            }
        }
        let audio = media_edit::list(&record["audio_complete"])
            .into_iter()
            .find(|audio| audio["item_id"] == *id);
        if let Some(audio) = audio {
            let complete = recovery_backup(recovery, &audio["complete"]).await?;
            if json_equivalent(&bytes, &tokio::fs::read(complete).await?, path)? {
                continue;
            }
        }
        let expected = parse_recovery_json(&expected_bytes, backup)?;
        let current = parse_recovery_json(&bytes, path)?;
        let mut changes = vec![];
        let mut change_count = 0;
        json_changes(
            Some(&expected),
            Some(&current),
            "",
            &mut changes,
            &mut change_count,
        );
        conflicts.push(json!({"item_id":id,"path":path,"current_sha256":json_hash(&bytes),"changes":changes,"change_count":change_count}));
    }
    Ok((conflicts, observed))
}

pub(crate) async fn recovery_json_conflicts(
    state: &AppState,
    recovery: &Path,
) -> Result<Vec<Value>> {
    let record = recovery_record(recovery).await?;
    let (_, requested) = prepare_recovery_json(state, recovery, &record).await?;
    Ok(inspect_recovery_json(recovery, &record, &requested)
        .await?
        .0)
}

pub(crate) async fn conflict_backup_directories(
    state: &AppState,
    recovery: &Path,
) -> Result<Vec<PathBuf>> {
    let index = recovery.join("conflict-backups.json");
    if !tokio::fs::try_exists(&index).await? {
        return Ok(vec![]);
    }
    let directories: Vec<PathBuf> =
        serde_json::from_value(crate::media_recovery::read_json(&index).await?)
            .map_err(|_| conflict("当前 JSON 留存记录无效，全部备份仍保留"))?;
    let root = state.data_dir.join("backups/media-info-conflicts");
    for directory in &directories {
        check_ancestors(&directory.join("unused")).await?;
        if !directory.is_absolute() || !directory.starts_with(&root) {
            return Err(conflict(
                "当前 JSON 留存目录不在独立备份根目录内，全部备份仍保留",
            ));
        }
    }
    Ok(directories)
}

fn validate_current_json_hashes(
    conflicts: &[Value],
    hashes: &BTreeMap<String, String>,
) -> Result<()> {
    if hashes.is_empty() && !conflicts.is_empty() {
        return Err(conflict(format!(
            "媒体 JSON 在失败后被修改，保留当前文件及备份；请核对差异并确认备份当前 JSON 后恢复：{}",
            conflicts
                .iter()
                .map(|value| format!("{}：{}", string(value, "item_id"), string(value, "path")))
                .collect::<Vec<_>>()
                .join("；")
        )));
    }
    let expected = conflicts
        .iter()
        .map(|value| (string(value, "item_id"), string(value, "current_sha256")))
        .collect::<BTreeMap<_, _>>();
    if *hashes != expected {
        return Err(conflict(
            "当前 JSON 已再次变化或确认目标不一致，请重新核对差异，未覆盖任何媒体 JSON",
        ));
    }
    Ok(())
}

async fn verify_current_json_snapshot(observed: &BTreeMap<PathBuf, Option<Vec<u8>>>) -> Result<()> {
    for (path, before) in observed {
        if current_json(path).await? != *before {
            return Err(conflict(format!(
                "当前 JSON 在恢复准备期间再次变化，未覆盖媒体 JSON：{}",
                path.display()
            )));
        }
    }
    Ok(())
}

async fn write_recovery_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}

async fn preserve_current_json(
    state: &AppState,
    recovery: &Path,
    conflicts: &[Value],
    observed: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    directories: &mut Vec<PathBuf>,
) -> Result<()> {
    if conflicts.is_empty() {
        return Ok(());
    }
    let root = state.data_dir.join("backups/media-info-conflicts");
    check_ancestors(&root.join("unused")).await?;
    tokio::fs::create_dir_all(&root).await?;
    let directory = root.join(uuid::Uuid::new_v4().simple().to_string());
    tokio::fs::create_dir(&directory).await?;
    #[cfg(unix)]
    tokio::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).await?;
    directories.push(directory.clone());
    // Persist the retained location before copying. An interrupted copy must
    // not lose the only pointer to its already created preservation directory.
    let index = recovery.join("conflict-backups.json");
    let staged = recovery.join(format!(
        ".conflict-index-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    write_recovery_file(&staged, &serde_json::to_vec(directories).unwrap()).await?;
    tokio::fs::rename(&staged, &index).await?;
    tokio::fs::File::open(recovery).await?.sync_all().await?;
    for conflict in conflicts {
        let id = string(conflict, "item_id");
        let path = PathBuf::from(string(conflict, "path"));
        let bytes = observed[&path].as_ref().unwrap();
        write_recovery_file(&directory.join(format!("current-{id}.json")), bytes).await?;
    }
    write_recovery_file(
        &directory.join("manifest.json"),
        &serde_json::to_vec(&json!({
            "recovery_directory":recovery,"files":conflicts
        }))
        .unwrap(),
    )
    .await?;
    tokio::fs::File::open(&directory).await?.sync_all().await?;
    tokio::fs::File::open(&root).await?.sync_all().await?;
    Ok(())
}

async fn recovery_record(recovery: &Path) -> Result<Value> {
    let marker = recovery.join("media-reload.json");
    let record = if tokio::fs::try_exists(&marker).await? {
        crate::media_recovery::read_json(&marker).await?
    } else {
        crate::media_recovery::read_json(&recovery.join("pending-scan.json")).await?["verification"]
            .clone()
    };
    Ok(record)
}

async fn prepare_recovery_json(
    state: &AppState,
    recovery: &Path,
    record: &Value,
) -> Result<RecoveryJsonTargets> {
    let ids = record["target_ids"]
        .as_array()
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| conflict("恢复记录没有有效目标媒体，备份已保留"))?;
    let mut targets = vec![];
    let mut requested = BTreeMap::new();
    for id in ids {
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
        let info = parse_recovery_json(&tokio::fs::read(&backup).await?, &backup)?;
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
        requested.insert(id.to_string(), (path, backup));
    }
    let owners = targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    if owners.len() != targets.len() {
        return Err(conflict("恢复记录包含重复媒体 ID"));
    }
    let mut shared = BTreeMap::<PathBuf, Value>::new();
    for (path, backup) in requested.values() {
        let canonical = match tokio::fs::canonicalize(path).await {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = path
                    .parent()
                    .ok_or_else(|| conflict("恢复 JSON 路径没有父目录"))?;
                tokio::fs::canonicalize(parent)
                    .await?
                    .join(path.file_name().unwrap())
            }
            Err(error) => return Err(error.into()),
        };
        let info = parse_recovery_json(&tokio::fs::read(backup).await?, backup)?;
        if shared
            .insert(canonical, info.clone())
            .is_some_and(|previous| previous != info)
        {
            return Err(conflict(
                "多个恢复目标共用同一 JSON 路径但备份内容不同，未覆盖任何媒体 JSON",
            ));
        }
    }
    Ok((targets, requested))
}

pub(crate) async fn recover_record(
    state: &AppState,
    recovery: &Path,
    context: &JobContext,
    current_hashes: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>> {
    context.checkpoint().await?;
    let record = recovery_record(recovery).await?;
    let (targets, requested) = prepare_recovery_json(state, recovery, &record).await?;
    let json_paths = requested
        .values()
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let owners = targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    let chapter_images = chapter_images::load(recovery, &record, &owners).await?;
    let mut files = BTreeMap::new();
    let mut thumbnails = BTreeMap::new();
    let mut native_bifs = BTreeMap::<String, Vec<BifEvidence>>::new();
    let mut image_times = BTreeMap::<String, BTreeSet<u64>>::new();
    let mut audio_complete = BTreeMap::new();
    for audio in media_edit::list(&record["audio_complete"]) {
        let id = string(&audio, "item_id");
        let target = targets
            .iter()
            .find(|target| target.id == id)
            .ok_or_else(|| conflict("完整音乐 JSON 的恢复目标不一致"))?;
        let complete = recovery_backup(recovery, &audio["complete"]).await?;
        let full = parse_recovery_json(&tokio::fs::read(&complete).await?, &complete)?;
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
        let saved = Snapshot {
            path: path.clone(),
            backup,
            owners: ids.clone(),
            sha256: snapshot_hash(&file)?,
        };
        // Validate every recorded original before writing any formal JSON or
        // resource. Reuse these bytes for the existing BIF/JPEG validation.
        let bytes = snapshot_bytes(&saved).await?;
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("bif"))
        {
            if file["unused_bif_sha256"].is_null() {
                let frames = recovery_bif_times(&bytes)?.as_array().unwrap().len();
                for id in &ids {
                    native_bifs
                        .entry(id.clone())
                        .or_default()
                        .push(BifEvidence {
                            path: path.clone(),
                            bytes: bytes.clone(),
                            frames,
                        });
                }
            } else if file["unused_bif_sha256"].as_str()
                != Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
                || recovery_bif_times(&bytes).is_ok()
            {
                return Err(conflict(
                    "未使用的 BIF 恢复备份与原记录不一致，已保留全部恢复材料",
                ));
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
            image::load_from_memory(&bytes).map_err(|_| conflict("原有章节图片恢复备份无效"))?;
            for id in &ids {
                if record["observations"][id].is_null() {
                    let target = targets.iter().find(|target| target.id == *id).unwrap();
                    let ticks = recovery_chapter_ticks(&path, &target.info)?;
                    image_times.entry(id.clone()).or_default().insert(ticks);
                }
            }
        }
        files.insert(path, saved);
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
        let saved = Snapshot {
            path: path.clone(),
            backup,
            owners: BTreeSet::from([id.clone()]),
            sha256: snapshot_hash(&bridge)?,
        };
        let bytes = snapshot_bytes(&saved).await?;
        let frames = recovery_bif_times(&bytes)?.as_array().unwrap().len();
        let evidence = native_bifs.entry(id.clone()).or_default();
        if !evidence.iter().any(|original| original.path == path) {
            evidence.push(BifEvidence {
                path: path.clone(),
                bytes,
                frames,
            });
        }
        files.entry(path).or_insert(saved);
    }
    let mut before = BTreeMap::new();
    for target in &targets {
        let live = identity_details(state, target).await?;
        let mut observed = record["observations"][&target.id]
            .as_object()
            .map(|_| record["observations"][&target.id].clone())
            .unwrap_or_else(|| live.clone());
        for field in METADATA_FIELDS {
            observed[*field] = live[*field].clone();
        }
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
    let (conflicts, observed) = inspect_recovery_json(recovery, &record, &requested).await?;
    validate_current_json_hashes(&conflicts, current_hashes)?;
    context.checkpoint().await?;
    let mut directories = conflict_backup_directories(state, recovery).await?;
    let outcome: Result<()> = async {
        verify_current_json_snapshot(&observed).await?;
        for target in &targets {
            identity_details(state, target).await?;
        }
        preserve_current_json(state, recovery, &conflicts, &observed, &mut directories).await?;
        verify_current_json_snapshot(&observed).await?;
        for target in &targets {
            identity_details(state, target).await?;
        }
        context.checkpoint().await?;
        let finish = JobContext::for_recovery(Some(context)).await;
        let mut written = BTreeSet::new();
        for target in &targets {
            let (path, backup) = &requested[&target.id];
            if written.insert(path.clone()) {
                identity_details(state, target).await?;
                if current_json(path).await? != observed[path] {
                    return Err(conflict(
                        "当前 JSON 在写入前再次变化，保留当前文件与全部备份",
                    ));
                }
                restore_json_guarded(path, backup, observed[path].as_deref()).await?;
            }
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
            &native_bifs,
            &chapter_images,
        )
        .await?;
        restore_files(&resources, &owners).await?;
        verify_restored_resources(&resources, &owners).await?;
        for target in &targets {
            if let Some((complete, full)) = audio_complete.get(&target.id) {
                let (path, import) = &requested[&target.id];
                identity_details(state, target).await?;
                let observed = current_json(path)
                    .await?
                    .ok_or_else(|| conflict("音乐 JSON 在恢复期间丢失，完整音乐备份仍保留"))?;
                if !json_equivalent(&observed, &tokio::fs::read(import).await?, path)? {
                    return Err(conflict("音乐 JSON 在恢复期间变化，完整音乐备份仍保留"));
                }
                restore_json_guarded(path, complete, Some(&observed)).await?;
                crate::media_save::cache_saved(state, target.sha.as_deref(), full).await?;
            }
        }
        Ok(())
    }
    .await;
    if let Err(mut error) = outcome {
        if !directories.is_empty() {
            error.1.push_str(&format!(
                "；当前 JSON 独立备份仍保留：{}",
                directories
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("；")
            ));
        }
        return Err(error);
    }
    Ok(directories)
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
    sha256: Option<String>,
}

fn snapshot_hash(record: &Value) -> Result<Option<String>> {
    match record.get("sha256") {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        None | Some(Value::Null) => Ok(None),
        _ => Err(conflict("原资源恢复备份校验值无效，已保留全部恢复材料")),
    }
}

async fn snapshot_bytes(saved: &Snapshot) -> Result<Vec<u8>> {
    let bytes = tokio::fs::read(&saved.backup).await?;
    // Old recovery records did not store hashes; preserve their existing
    // compatibility checks without inventing an original checksum.
    if saved
        .sha256
        .as_ref()
        .is_some_and(|expected| expected != &format!("{:x}", Sha256::digest(&bytes)))
    {
        return Err(conflict(format!(
            "原资源恢复备份与原记录校验值不一致，已保留全部恢复材料：{}",
            saved.path.display()
        )));
    }
    Ok(bytes)
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
    let sha256 = format!("{:x}", Sha256::digest(tokio::fs::read(&backup).await?));
    files.insert(
        path.clone(),
        Snapshot {
            path,
            backup,
            owners: BTreeSet::from([owner.into()]),
            sha256: Some(sha256),
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
            let original = snapshot_bytes(saved).await?;
            if tokio::fs::symlink_metadata(&saved.path)
                .await
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(conflict("恢复路径在重载期间变为符号链接，已保留备份"));
            }
            if tokio::fs::try_exists(&saved.path).await? {
                if tokio::fs::read(&saved.path).await? != original {
                    return Err(conflict(format!(
                        "恢复资源在重载期间被修改，保留新文件及备份：{}",
                        saved.path.display()
                    )));
                }
                return Ok(());
            }
            tokio::fs::create_dir_all(saved.path.parent().unwrap()).await?;
            assets::restore_bytes(&original, &saved.path).await?;
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

async fn verify_restored_resources(
    files: &BTreeMap<PathBuf, Snapshot>,
    ids: &BTreeSet<String>,
) -> Result<()> {
    for saved in files
        .values()
        .filter(|saved| !saved.owners.is_disjoint(ids))
    {
        check_ancestors(&saved.path).await?;
        let original = snapshot_bytes(saved).await?;
        let metadata = match tokio::fs::symlink_metadata(&saved.path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(conflict(format!(
                    "原有图片资源最终验收时文件缺失，保留恢复备份：{}",
                    saved.path.display()
                )));
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(conflict(format!(
                "原有图片资源最终验收不是普通文件，保留当前路径及备份：{}",
                saved.path.display()
            )));
        }
        let current = tokio::fs::read(&saved.path).await?;
        if current != original {
            return Err(conflict(format!(
                "原有图片资源最终验收字节未一致，保留当前文件及备份：{}（原 SHA256：{:x}；当前 SHA256：{:x}）",
                saved.path.display(),
                Sha256::digest(&original),
                Sha256::digest(&current),
            )));
        }
    }
    Ok(())
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
            continue;
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
    let expected = current_json(path).await?;
    restore_json_guarded(path, backup, expected.as_deref()).await
}

async fn restore_json_guarded(path: &Path, backup: &Path, expected: Option<&[u8]>) -> Result<()> {
    check_ancestors(path).await?;
    if tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(conflict("恢复 JSON 变为符号链接，已保留备份"));
    }
    crate::media_save::commit_json_stage_guarded(backup, path, expected).await
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
    let original = match files.get(&original_path) {
        Some(saved) => snapshot_bytes(saved).await?,
        None => tokio::fs::read(backup).await?,
    };
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        json_equivalent(&current, &original, path)
    } else {
        Ok(current == original)
    }
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

fn has_filtered_markers(chapters: &Value) -> bool {
    media_edit::list(chapters).iter().any(|chapter| {
        chapter["MarkerType"]
            .as_i64()
            .is_some_and(|value| matches!(value, 1..=3))
            || chapter["MarkerType"].as_str().is_some_and(|value| {
                ["IntroStart", "IntroEnd", "CreditsStart", "1", "2", "3"]
                    .iter()
                    .any(|marker| value.trim().eq_ignore_ascii_case(marker))
            })
    })
}

fn ensure_unfiltered(chapters: &Value, streams: Option<&Vec<Value>>, id: &str) -> Result<()> {
    if has_filtered_markers(chapters) {
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

pub(crate) fn ensure_json_streams_unfiltered(info: &Value, id: &str) -> Result<()> {
    if !needs_file_import(info, &Value::Null) {
        return ensure_json_unfiltered(info, id);
    }
    ensure_single_bundle(info)?;
    let source = media_info::container(info).ok_or_else(|| conflict("同步 JSON 缺少媒体源"))?;
    let streams = source["MediaStreams"]
        .as_array()
        .ok_or_else(|| conflict("目标 JSON 缺少有效媒体流，未提交媒体信息恢复"))?;
    ensure_unfiltered(&Value::Null, Some(streams), id)
}

fn needs_file_import(info: &Value, live: &Value) -> bool {
    media_info::object(info).is_some_and(|bundle| has_filtered_markers(&bundle["Chapters"]))
        || media_info::container(info)
            .is_some_and(|source| has_filtered_markers(&source["Chapters"]))
        || has_filtered_markers(&live["Chapters"])
}

fn ensure_file_import_controls(settings: &Value, id: &str, applied: bool) -> Result<()> {
    if settings["ExclusiveExtract"] != true {
        return Err(conflict(format!(
            "{id}：神医同步会移除片头片尾标记，独占提取未开启，无法安全使用原生文件恢复，已停止保存"
        )));
    }
    let controls = if applied {
        settings["ExclusiveControlFeatures"].clone()
    } else {
        crate::strm::temporary_features(settings, "restore_media_info")?
    };
    let controls = controls.as_str().unwrap_or("");
    if controls
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        != BTreeSet::from(["IgnoreFileChange"])
        || applied && settings["PersistMediaInfoMode"] != "Restore"
        || settings["IsModSupported"] != true
    {
        return Err(conflict(format!(
            "{id}：当前神医独占控制未通过文件恢复验证，已停止保存，未清理媒体信息"
        )));
    }
    Ok(())
}

async fn ensure_file_import_supported(
    state: &AppState,
    settings: &Value,
    id: &str,
    applied: bool,
) -> Result<()> {
    ensure_file_import_controls(settings, id, applied)?;
    let swagger = ensure_sync_supported(state, settings).await?;
    let operation = &swagger["paths"]["/Items/{Id}/Refresh"]["post"];
    let parameters = media_edit::list(&operation["parameters"]);
    if ![
        ("Id", "path"),
        ("MetadataRefreshMode", "query"),
        ("ImageRefreshMode", "query"),
        ("ReplaceAllMetadata", "query"),
        ("ReplaceAllImages", "query"),
    ]
    .iter()
    .all(|(name, location)| {
        parameters
            .iter()
            .any(|p| p["name"] == *name && p["in"] == *location)
    }) {
        return Err(conflict(
            "Emby 未提供已验证的定点文件恢复接口，未清理媒体信息",
        ));
    }
    Ok(())
}

pub(crate) async fn preflight_file_import(
    state: &AppState,
    target: &SavedTarget,
    context: &JobContext,
) -> Result<()> {
    preflight_file_import_prepared(state, target, context, &mut None).await
}

async fn preflight_file_import_prepared(
    state: &AppState,
    target: &SavedTarget,
    context: &JobContext,
    inventory: &mut Option<ClearScopeInventory>,
) -> Result<()> {
    context.checkpoint().await?;
    ensure_json_streams_unfiltered(&target.info, &target.id)?;
    if !needs_file_import(&target.info, &Value::Null) {
        return Ok(());
    }
    let settings = crate::strm::config(state).await?;
    ensure_file_import_supported(state, &settings, &target.id, false).await?;
    let live = identity_details(state, target).await?;
    ensure_target_reload_supported(target, &live)?;
    ensure_single_clear_scope_prepared(state, target, &live, context, inventory).await
}

pub(crate) async fn preflight_file_imports(
    state: &AppState,
    targets: &[SavedTarget],
    context: &JobContext,
) -> Result<()> {
    if let [target] = targets {
        return preflight_file_import(state, target, context).await;
    }
    // This snapshot belongs only to this read-only preparation batch.
    let mut inventory = None;
    for target in targets {
        preflight_file_import_prepared(state, target, context, &mut inventory).await?;
    }
    Ok(())
}

async fn refresh_from_file(
    state: &AppState,
    target: &SavedTarget,
    context: &JobContext,
) -> Result<Option<String>> {
    let live = identity_details(state, target).await?;
    ensure_single_clear_scope(state, target, &live, context).await?;
    ensure_file_import_controls(&crate::strm::config(state).await?, &target.id, true)?;
    context
        .activity(
            "从正式 JSON 恢复媒体信息",
            &format!(
                "媒体ID {}：保留原标记和图片，提交已保护的文件恢复",
                target.id
            ),
        )
        .await;
    // The native service expands media-source versions even when Recursive is
    // false. The strict native scope proof above is required before submission.
    // ExclusiveExtract's verified file path does not run Sync's marker filter.
    let restored = state
        .emby_once(
            Method::POST,
            &format!("Items/{}/Refresh", target.id),
            &[
                ("Recursive", "false".into()),
                // PRO's persistence hook distinguishes Default (2) from
                // FullRefresh (3). Only the former was verified to restore
                // complete streams, exact markers and original pictures.
                ("MetadataRefreshMode", "Default".into()),
                ("ImageRefreshMode", "ValidationOnly".into()),
                ("ReplaceAllMetadata", "false".into()),
                ("ReplaceAllImages", "false".into()),
                ("ReplaceThumbnailImages", "false".into()),
            ],
            None,
        )
        .await;
    Ok(restored.err().map(|error| {
        format!(
            "{}：原生文件恢复请求未正常返回，继续按实际媒体流、章节及图片回读验收：{}",
            target.id, error.1
        )
    }))
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

fn ensure_target_reload_supported(target: &SavedTarget, live: &Value) -> Result<bool> {
    let restoring = media_info::restore_json(&target.info)?;
    let paths = [&target.path[..], target.source.as_deref().unwrap_or("")];
    let file_import = needs_file_import(&restoring, live)
        || media_info::item_source(live, &paths)
            .is_some_and(|source| has_filtered_markers(&source["Chapters"]));
    if !file_import {
        ensure_target_unfiltered(target, live)?;
        return Ok(false);
    }
    ensure_json_streams_unfiltered(&restoring, &target.id)?;
    media_info::signature(&media_info::object(&restoring).unwrap()["Chapters"])?;
    // Only marker restoration is supported by this file-import contract.
    // Keep refusing local external streams, including unedited native ones.
    ensure_unfiltered(
        &Value::Null,
        media_info::item_streams(live, &paths),
        &target.id,
    )?;
    ensure_unfiltered(&Value::Null, live["MediaStreams"].as_array(), &target.id)?;
    Ok(true)
}

async fn ensure_sync_supported(state: &AppState, settings: &Value) -> Result<Value> {
    // PRO reads the first nonempty Bundle, but its importer can return early
    // for complete native information. Its response serializes current data.
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
    Ok(swagger)
}

async fn sync_one(
    state: &AppState,
    target: &SavedTarget,
    payload: Value,
    context: &JobContext,
    before: &Value,
) -> Result<Value> {
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
    Ok(restored)
}

struct ClearScopeInventory {
    items: Vec<Value>,
    connection: [String; 3],
}

async fn scope_connection(state: &AppState) -> [String; 3] {
    let config = state.config.read().await;
    [
        string(&config, "emby_server_url")
            .trim_end_matches('/')
            .into(),
        string(&config, "emby_api_key"),
        string(&config, "emby_user_id"),
    ]
}

impl ClearScopeInventory {
    async fn check_connection(&self, state: &AppState) -> Result<()> {
        if self.connection != scope_connection(state).await {
            return Err(conflict(
                "Emby 连接或用户身份在范围核对期间变化，未提交媒体信息恢复",
            ));
        }
        Ok(())
    }
}

async fn clear_scope_inventory(
    state: &AppState,
    context: &JobContext,
) -> Result<ClearScopeInventory> {
    // Pin every page to one connection, including a transient A -> B -> A
    // config change while the complete inventory is being read.
    let mut inventory_state = state.clone();
    inventory_state.config =
        std::sync::Arc::new(tokio::sync::RwLock::new(state.config.read().await.clone()));
    let connection = scope_connection(&inventory_state).await;
    // The public Items API has no PresentationUniqueKey query parameter.
    // Read an unfiltered, complete native inventory before trusting uniqueness.
    let items = crate::emby::query_items(
        &inventory_state,
        &[
            ("Recursive", "true".into()),
            (
                "IncludeItemTypes",
                "Movie,Episode,Video,MusicVideo,Audio".into(),
            ),
            ("Fields", "Path,PresentationUniqueKey,ExtraType".into()),
            ("EnableImages", "false".into()),
            ("EnableUserData", "false".into()),
            ("GroupItemsIntoCollections", "false".into()),
        ],
        context,
    )
    .await?;
    let inventory = ClearScopeInventory { items, connection };
    inventory.check_connection(state).await?;
    Ok(inventory)
}

pub(crate) async fn ensure_single_clear_scope(
    state: &AppState,
    target: &SavedTarget,
    live: &Value,
    context: &JobContext,
) -> Result<()> {
    // Native Clear and Refresh always use a new inventory; never reuse a
    // preparation snapshot across a native write.
    ensure_single_clear_scope_prepared(state, target, live, context, &mut None).await
}

async fn ensure_single_clear_scope_prepared(
    state: &AppState,
    target: &SavedTarget,
    live: &Value,
    context: &JobContext,
    inventory: &mut Option<ClearScopeInventory>,
) -> Result<()> {
    context.checkpoint().await?;
    let refuse = || conflict("神医未导入编辑值，关联清理范围不是唯一当前媒体，未清理任何项目");
    let key = string(live, "PresentationUniqueKey");
    if key.is_empty()
        || live["Id"] != target.id
        || live["Path"] != target.path
        || !Path::new(&target.path).is_absolute()
        || !matches!(live["Type"].as_str(), Some("Movie" | "Episode"))
        || !string(live, "ExtraType").is_empty()
        || live["PartCount"].as_u64() != Some(1)
    {
        return Err(refuse());
    }
    if inventory.is_none() {
        *inventory = Some(clear_scope_inventory(state, context).await?);
    }
    let inventory = inventory.as_ref().unwrap();
    inventory.check_connection(state).await?;
    let items = &inventory.items;
    if items
        .iter()
        .any(|item| string(item, "PresentationUniqueKey").is_empty())
    {
        return Err(refuse());
    }
    let members = items
        .iter()
        .filter(|item| item["PresentationUniqueKey"] == key && string(item, "ExtraType").is_empty())
        .collect::<Vec<_>>();
    if members.len() != 1 || members[0]["Id"] != target.id || members[0]["Path"] != target.path {
        return Err(refuse());
    }
    let sources = live["MediaSources"].as_array().ok_or_else(refuse)?;
    if sources.len() != 1
        || !media_info::item_source(
            live,
            &[&target.path, target.source.as_deref().unwrap_or("")],
        )
        .is_some_and(|source| source == &sources[0])
    {
        return Err(refuse());
    }
    // Inspect the unfiltered native response: the editor helper applies a
    // UserId and drops malformed rows, so it cannot prove an empty clear scope.
    let parts = state
        .emby(
            Method::GET,
            &format!("Videos/{}/AdditionalParts", target.id),
            &[],
            None,
        )
        .await?;
    if !parts["Items"].as_array().is_some_and(Vec::is_empty)
        || parts["TotalRecordCount"].as_u64() != Some(0)
    {
        return Err(refuse());
    }
    let user_id = string(&*state.config.read().await, "emby_user_id");
    let user = state
        .emby(Method::GET, &format!("Users/{user_id}"), &[], None)
        .await?;
    let policy = &user["Policy"];
    if user["Id"] != user_id
        || policy["IsAdministrator"] != true
        || !policy["MaxParentalRating"].is_null()
        || !policy["BlockUnratedItems"]
            .as_array()
            .is_some_and(Vec::is_empty)
        || !policy["BlockedTags"].as_array().is_some_and(Vec::is_empty)
        || policy["IsTagBlockingModeInclusive"] != false
        || policy["AllowTagOrRating"] != false
    {
        return Err(refuse());
    }
    let extras = state
        .emby(
            Method::GET,
            &format!("Users/{user_id}/Items/{}/SpecialFeatures", target.id),
            &[],
            None,
        )
        .await?;
    if !extras.as_array().is_some_and(Vec::is_empty) {
        return Err(refuse());
    }
    inventory.check_connection(state).await?;
    Ok(())
}

async fn restore_bridge(bridge: &crate::chapter_bif::Bridge) -> Result<()> {
    check_ancestors(&bridge.destination).await?;
    let bytes = tokio::fs::read(&bridge.source).await?;
    match assets::restore_file(&bridge.source, &bridge.destination).await {
        Ok(()) => Ok(()),
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

struct BifEvidence {
    path: PathBuf,
    bytes: Vec<u8>,
    frames: usize,
}

async fn local_bif_difference(evidence: &[BifEvidence]) -> Result<Option<String>> {
    for original in evidence {
        check_ancestors(&original.path).await?;
        if !tokio::fs::symlink_metadata(&original.path)
            .await
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            || tokio::fs::read(&original.path).await? != original.bytes
        {
            return Ok(Some(format!(
                "原有 BIF 资源未完整恢复：{}",
                original.path.display()
            )));
        }
    }
    Ok(None)
}

async fn native_bif_difference(
    state: &AppState,
    id: &str,
    evidence: &[BifEvidence],
) -> Result<Option<String>> {
    if let Some(difference) = local_bif_difference(evidence).await? {
        return Ok(Some(difference));
    }
    // RokuMetadata.BifService builds ThumbnailSet from the filename interval
    // and runtime; those synthetic positions are not the BIF frame index.
    // The binary endpoint exposes the actual index and JPEG bytes. Width=0
    // avoids its strict <= width filter returning an empty BIF for 1920px art.
    let config = state.config.read().await.clone();
    let response = crate::emby::send(
        state
            .http
            .get(format!(
                "{}/Videos/{id}/index.bif",
                string(&config, "emby_server_url").trim_end_matches('/')
            ))
            .header("X-Emby-Token", string(&config, "emby_api_key"))
            .query(&[("Width", "0")]),
    )
    .await?
    .error_for_status()?;
    let actual = response.bytes().await?;
    if evidence.iter().any(|original| original.bytes == actual) {
        // A second BIF may change while Emby sends a still-matching first one.
        return local_bif_difference(evidence).await;
    }
    let expected = evidence
        .iter()
        .map(|original| original.frames)
        .collect::<BTreeSet<_>>();
    // The whole-byte comparison above is the acceptance check. Do not decode
    // every original JPEG again merely to format a failed readback's count.
    let actual_count = actual
        .get(12..16)
        .filter(|_| actual.get(..8) == Some(b"\x89BIF\r\n\x1a\n"))
        .map(|count| u32::from_le_bytes(count.try_into().unwrap()).to_string())
        .unwrap_or_else(|| "空或无效".into());
    Ok(Some(format!(
        "原有 BIF 缩略图集合未恢复：原生时间索引或图片内容与备份不一致（备份帧数 {expected:?}，回读帧数 {actual_count}）"
    )))
}

async fn verify_reloaded(
    state: &AppState,
    targets: &[&SavedTarget],
    context: &JobContext,
    before: &BTreeMap<String, Value>,
    thumbnails: &BTreeMap<String, Value>,
    native_bifs: &BTreeMap<String, Vec<BifEvidence>>,
    chapter_images: &chapter_images::EvidenceMap,
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
            let live = match tokio::time::timeout_at(deadline, readback).await {
                Ok(result) => result?,
                Err(_) if !pending[&target.id].is_empty() => break,
                Err(_) => {
                    return Err(conflict(format!(
                        "{}：重载回读超时，JSON 和恢复备份已保留",
                        target.id
                    )));
                }
            };
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
            if let Some(evidence) = native_bifs.get(&target.id) {
                let readback = tokio::time::timeout_at(
                    deadline,
                    native_bif_difference(state, &target.id, evidence),
                )
                .await;
                let difference = match readback {
                    Ok(result) => result?,
                    Err(_) if !pending[&target.id].is_empty() => break,
                    Err(_) => return Err(conflict("原生 BIF 回读超时，恢复备份已保留")),
                };
                if let Some(difference) = difference {
                    reasons.push(difference);
                }
            }
            if let Some(evidence) = chapter_images.get(&target.id) {
                let difference = tokio::time::timeout_at(
                    deadline,
                    chapter_images::difference(state, &target.id, &live, evidence),
                )
                .await
                .map_err(|_| conflict("原有章节图片回读超时，恢复备份已保留"))??;
                if let Some(difference) = difference {
                    reasons.push(difference);
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

async fn verify_unchanged_resources(
    state: &AppState,
    target: &SavedTarget,
    live: &Value,
    context: &JobContext,
) -> Result<()> {
    if live["Type"] == "Audio" {
        return Ok(());
    }
    context.checkpoint().await?;
    let verification = async {
        let images = chapter_images::capture(state, &target.id, live, None).await?;
        let path = Path::new(&target.path);
        let parent = path
            .parent()
            .ok_or_else(|| conflict("媒体路径没有父目录"))?;
        let prefix = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let mut entries = match tokio::fs::read_dir(parent).await {
            Ok(entries) => Some(entries),
            // No-op saves previously required no local media mount. Continue
            // API image verification when this containing folder is unavailable.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                None
            }
            Err(error) => return Err(error.into()),
        };
        let mut bifs = vec![];
        while let Some(entry) = match &mut entries {
            Some(entries) => entries.next_entry().await?,
            None => None,
        } {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.ends_with(".bif") || !name.starts_with(&prefix) {
                continue;
            }
            check_ancestors(&entry.path()).await?;
            if !entry.file_type().await?.is_file() {
                return Err(conflict("原有 BIF 资源不是普通文件，未提交媒体信息恢复"));
            }
            let bytes = tokio::fs::read(entry.path()).await?;
            let frames = recovery_bif_times(&bytes)?.as_array().unwrap().len();
            bifs.push(BifEvidence {
                path: entry.path(),
                bytes,
                frames,
            });
        }
        let current = identity_details(state, target).await?;
        if !differences(target, &current).is_empty() {
            return Err(conflict(
                "无变化保存的媒体流或章节在图片验收期间变化，未提交媒体信息恢复",
            ));
        }
        if let Some(reason) =
            chapter_images::difference(state, &target.id, &current, &images).await?
        {
            return Err(conflict(reason));
        }
        if !bifs.is_empty()
            && let Some(reason) = native_bif_difference(state, &target.id, &bifs).await?
        {
            return Err(conflict(reason));
        }
        Ok(())
    };
    tokio::select! {
        _ = context.cancelled() => { context.checkpoint().await?; unreachable!() },
        result = tokio::time::timeout(Duration::from_secs(30), verification) =>
            result.map_err(|_| conflict("无变化保存的图片验收超时，原资源已保留"))?,
    }
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
    let mut file_imports = BTreeSet::new();
    for target in targets {
        context.checkpoint().await?;
        let live = saved_details(state, target).await?;
        if !differences(target, &live).is_empty() {
            let observed = identity_details(state, target).await?;
            if ensure_target_reload_supported(target, &observed)? {
                file_imports.insert(target.id.clone());
            }
            observations.insert(target.id.clone(), observed);
            affected.push(target);
        } else {
            verify_unchanged_resources(state, target, &live, context).await?;
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
    if let Some(id) = file_imports.first() {
        ensure_file_import_supported(state, &settings, id, true).await?;
    }
    let mut inventory = None;
    for target in affected
        .iter()
        .filter(|target| file_imports.contains(&target.id))
    {
        ensure_single_clear_scope_prepared(
            state,
            target,
            &observations[&target.id],
            context,
            &mut inventory,
        )
        .await?;
    }
    // Do not retain this read-only snapshot for the per-target native writes.
    drop(inventory);
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
    // A no-op Sync may require resetting a single verified native item. Protect
    // its whole metadata directory, including images without chapter thumbnails.
    let root = if affected
        .iter()
        .all(|target| observations[&target.id]["Type"] == "Audio")
    {
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
    let mut chapter_images = chapter_images::EvidenceMap::new();
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
            chapter_images.insert(
                target.id.clone(),
                tokio::select! {
                    _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
                    result=chapter_images::capture(state,&target.id,live,Some(recovery))=>result?,
                },
            );
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
    let mut native_bifs = BTreeMap::<String, Vec<BifEvidence>>::new();
    let mut unused_bifs = BTreeMap::new();
    for saved in files.values().filter(|saved| {
        saved
            .path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bif"))
    }) {
        let bytes = tokio::fs::read(&saved.backup).await?;
        // Native metadata may contain an empty, unused BIF. Preserve that
        // resource, but only populated BIFs can prove the native image index.
        let Ok(times) = recovery_bif_times(&bytes) else {
            unused_bifs.insert(saved.path.clone(), format!("{:x}", Sha256::digest(&bytes)));
            continue;
        };
        for id in &saved.owners {
            native_bifs
                .entry(id.clone())
                .or_default()
                .push(BifEvidence {
                    path: saved.path.clone(),
                    bytes: bytes.clone(),
                    frames: times.as_array().unwrap().len(),
                });
        }
    }
    for (id, bridge) in &bridges {
        let bytes = tokio::fs::read(&bridge.source).await?;
        let frames = recovery_bif_times(&bytes)?.as_array().unwrap().len();
        native_bifs
            .entry(id.clone())
            .or_default()
            .push(BifEvidence {
                path: bridge.destination.clone(),
                bytes,
                frames,
            });
    }
    for target in &affected {
        saved_details(state, target).await?;
        let current = identity_details(state, target).await?;
        let before = &observations[&target.id];
        if ensure_target_reload_supported(target, &current)? != file_imports.contains(&target.id) {
            return Err(conflict("媒体标记范围在准备期间变化，未提交媒体信息恢复"));
        }
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
        if tokio::fs::read(&saved.path).await? != snapshot_bytes(saved).await? {
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
        "chapter_images":chapter_images,
        "files":files.values().map(|f|json!({"path":f.path,"backup":f.backup,"owner_ids":f.owners,"sha256":f.sha256,"unused_bif_sha256":unused_bifs.get(&f.path)})).collect::<Vec<_>>(),
        "json":json_files.iter().map(|(id,path,backup)|json!({"item_id":id,"path":path,"backup":backup})).collect::<Vec<_>>(),
        "audio_complete":audio_json.iter().map(|(id,path,import,complete)|json!({"item_id":id,"path":path,"import":import,"complete":complete})).collect::<Vec<_>>(),
        "bridges":bridges.iter().map(|(id,b)|json!({"item_id":id,"source":b.source,"destination":b.destination,
            "sha256":format!("{:x}", Sha256::digest(&native_bifs[id].last().unwrap().bytes))})).collect::<Vec<_>>()
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
                restore_bridge(bridge).await
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
        let resource_files = files
            .into_iter()
            .filter(|(path, _)| !formal_paths.contains(path))
            .collect::<BTreeMap<_, _>>();
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
                let file_import = file_imports.contains(&target.id);
                let restored = if file_import {
                    // Sync's DTO filter destroys markers. Use the existing
                    // protected reset-and-recovery transaction, then let the
                    // native file importer read the complete formal JSON.
                    Value::Null
                } else {
                    sync_one(state, target, payload.clone(), &recovery_context,
                        &observations[&target.id]).await?
                };
                let mut reported = restored[0]["MediaSourceInfo"].clone();
                reported["Chapters"] = restored[0]["Chapters"].clone();
                if file_import || !differences(target, &reported).is_empty() {
                    let live = identity_details(state, target).await?;
                    if file_import || !differences(target, &live).is_empty() {
                        let current = identity_details(state, target).await?;
                        if METADATA_FIELDS
                            .iter()
                            .chain(
                                [
                                    "PresentationUniqueKey",
                                    "PartCount",
                                    "ExtraType",
                                    "MediaSources",
                                    "MediaStreams",
                                    "Chapters",
                                ]
                                .iter(),
                            )
                            .any(|field| current[*field] != observations[&target.id][*field])
                        {
                            return Err(conflict(
                                "媒体或关联范围在重导入准备期间变化，未清理媒体信息",
                            ));
                        }
                        let response_bytes = if file_import {
                            bytes.clone()
                        } else {
                            serde_json::to_vec(&restored).unwrap()
                        };
                        check_ancestors(path).await?;
                        let observed_json = current_json(path)
                            .await?
                            .ok_or_else(|| conflict("JSON 在同步返回后丢失，未清理媒体信息"))?;
                        let matches = if file_import {
                            json_equivalent(&observed_json, &response_bytes, path)?
                        } else {
                            sync_json_matches(&observed_json, &bytes, &response_bytes, path)?
                        };
                        if !matches {
                            return Err(conflict(
                                "JSON 在同步返回后被修改，保留用户新文件及恢复备份，未清理媒体信息",
                            ));
                        }
                        for resource in resource_files
                            .values()
                            .filter(|resource| resource.owners.contains(&target.id))
                        {
                            writable_parent(&resource.path).await?;
                            if !tokio::fs::symlink_metadata(&resource.path).await?.is_file()
                                || tokio::fs::read(&resource.path).await?
                                    != snapshot_bytes(resource).await?
                            {
                                return Err(conflict(
                                    "恢复资源在重导入准备期间变化，未清理媒体信息",
                                ));
                            }
                        }
                        recovery_context
                            .activity(
                                "重新导入当前媒体信息",
                                &format!(
                                    "媒体ID {}：已保护原资源，正在核对唯一目标并恢复完整媒体信息",
                                    target.id
                                ),
                            )
                            .await;
                        ensure_single_clear_scope(state, target, &current, &recovery_context).await?;
                        if file_import {
                            ensure_file_import_controls(&crate::strm::config(state).await?, &target.id, true)?;
                        }
                        // ClearMediaInfo expands by presentation key and extras.
                        // It is only allowed after proving that scope is this
                        // single protected file; no unrequested sibling is reset.
                        let cleared = state
                            .emby_once(
                                Method::POST,
                                &format!("Items/{}/ClearMediaInfo", target.id),
                                &[],
                                None,
                            )
                            .await;
                        // Once Clear was submitted, attempt every local recovery
                        // even when its HTTP result or a JSON guard fails.
                        let mut recovery_failures = vec![];
                        let json_restored = async {
                            let after_clear_json = current_json(path).await?;
                            if let Some(current) = &after_clear_json
                                && !json_equivalent(current, &observed_json, path)?
                            {
                                return Err(conflict("JSON 在当前媒体清理期间变化，已保留用户文件及恢复备份"));
                            }
                            restore_json_guarded(path, backup, after_clear_json.as_deref()).await
                        }.await;
                        if let Err(error) = json_restored {
                            recovery_failures.push(error.1);
                        }
                        if let Err(error) = restore_files(&resource_files, &BTreeSet::from([target.id.clone()])).await {
                            recovery_failures.push(error.1);
                        }
                        for (_, bridge) in bridges.iter().filter(|(id, _)| *id == target.id) {
                            if let Err(error) = restore_bridge(bridge).await {
                                recovery_failures.push(error.1);
                            }
                        }
                        if !recovery_failures.is_empty() {
                            if let Err(error) = &cleared { recovery_failures.push(error.1.clone()); }
                            return Err(conflict(recovery_failures.join("；")));
                        }
                        if let Err(error) = cleared {
                            cache_warnings.push(format!(
                                "{}：原生清理请求未正常返回，已保护资源并按实际身份继续补救同步：{}",
                                target.id, error.1
                            ));
                        }
                        let empty = identity_details(state, target).await?;
                        if file_import {
                            if let Some(warning) = refresh_from_file(state, target, &recovery_context).await? {
                                cache_warnings.push(warning);
                            }
                        } else {
                            sync_one(state, target, payload, &recovery_context, &empty).await?;
                        }
                    }
                }
                Ok(())
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
        if let Err(error) = restore_files(&resource_files, &synchronized).await {
            failures.push(error.1);
        }
        let verified = async {
            verify_reloaded(
                state,
                &ready,
                &recovery_context,
                &observations,
                &thumbnails,
                &native_bifs,
                &chapter_images,
            ).await?;
            // Native BIF restore may remove standalone images before its
            // chapters converge. Restore missing originals after that readback.
            restore_files(&resource_files, &synchronized).await?;
            verify_restored_resources(&resource_files, &synchronized).await
        }.await;
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
                let target = ready.iter().find(|target| target.id == *id).unwrap();
                identity_details(state, target).await?;
                let observed = current_json(path)
                    .await?
                    .ok_or_else(|| conflict("音乐 JSON 在同步期间丢失，完整音乐备份仍保留"))?;
                if !json_equivalent(&observed, &tokio::fs::read(import).await?, path)? {
                    return Err(conflict(
                        "音乐 JSON 在同步期间变化，保留用户文件及完整恢复备份",
                    ));
                }
                restore_json_guarded(path, complete, Some(&observed)).await
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
            tracing::warn!("[媒体信息重载] 原生媒体信息已确认，仍有处理提醒：{warning}");
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
mod scope_tests;

#[cfg(test)]
mod chapter_images_tests;

#[cfg(test)]
mod file_import_control_tests {
    use super::*;

    #[test]
    fn file_controls_predict_temporary_changes_but_require_actual_ignore_only() {
        let mut settings = json!({"ExclusiveExtract":true,"IsModSupported":true,"PersistMediaInfoMode":"Default","ExclusiveControlFeatures":"CatchAllAllow,ExtractOnFileChange"});
        assert!(ensure_file_import_controls(&settings, "61688", false).is_ok());
        assert!(ensure_file_import_controls(&settings, "61688", true).is_err());
        settings["PersistMediaInfoMode"] = json!("Restore");
        settings["ExclusiveControlFeatures"] = json!("IgnoreFileChange");
        assert!(ensure_file_import_controls(&settings, "61688", true).is_ok());
        settings["ExclusiveControlFeatures"] = json!("IgnoreFileChange,CatchAllBlock");
        assert!(ensure_file_import_controls(&settings, "61688", false).is_err());
        assert!(ensure_file_import_controls(&settings, "61688", true).is_err());
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn native_sync_json_retains_submitted_confidence_when_native_response_recalculates_it() {
        let path = Path::new("/media/item-mediainfo.json");
        let current = json!([{"ZeroFingerprintConfidence":true,"MediaSourceInfo":{
            "Size":42,"MediaStreams":[{"Type":"Subtitle","Title":"original","Language":"jpn"}]
        }}]);
        let mut submitted = current.clone();
        submitted[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("edited");
        let mut response = current.clone();
        response[0]["ZeroFingerprintConfidence"] = json!(false);
        let bytes = |value: &Value| serde_json::to_vec(value).unwrap();
        assert!(
            sync_json_matches(
                &bytes(&current),
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .unwrap()
        );
        assert!(!json_equivalent(&bytes(&current), &bytes(&response), path).unwrap());
        let mut changed = current.clone();
        changed[0]["ZeroFingerprintConfidence"] = json!("external change");
        assert!(
            !sync_json_matches(
                &bytes(&changed),
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .unwrap()
        );
        changed = current;
        changed[0]["UnknownFingerprintState"] = json!(true);
        assert!(
            !sync_json_matches(
                &bytes(&changed),
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .unwrap()
        );
    }

    #[test]
    fn native_sync_json_accepts_only_known_exports_and_calculated_stream_display_fields() {
        let path = Path::new("/media/item-mediainfo.json");
        let submitted = json!([{"MediaSourceInfo":{"Size":42,"MediaStreams":[{
            "Type":"Subtitle","Index":1,"Codec":"ass","Title":"edited","Language":"chs",
            "IsDefault":true,"IsForced":false,"IsHearingImpaired":false
        }]}}]);
        let mut response = submitted.clone();
        response[0]["MediaSourceInfo"]["MediaStreams"][0]["Title"] = json!("original");
        let mut current = response.clone();
        current[0]["MediaSourceInfo"]["MediaStreams"][0]["DisplayTitle"] =
            json!("Calculated title");
        current[0]["MediaSourceInfo"]["MediaStreams"][0]["DisplayLanguage"] = json!("Chinese");
        let bytes = |value: &Value| serde_json::to_vec(value).unwrap();
        assert!(
            sync_json_matches(
                &bytes(&submitted),
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .unwrap()
        );
        assert!(
            sync_json_matches(
                &bytes(&current),
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .unwrap()
        );
        assert!(!json_equivalent(&bytes(&current), &bytes(&response), path).unwrap());
        for (field, value) in [
            ("Title", json!("user change")),
            ("Language", json!("eng")),
            ("IsDefault", json!(false)),
            ("IsForced", json!(true)),
            ("IsHearingImpaired", json!(true)),
            ("Index", json!(2)),
            ("Codec", json!("srt")),
            ("Unknown", json!(true)),
            ("Extra", json!({"DisplayTitle":"user change"})),
        ] {
            let mut changed = current.clone();
            changed[0]["MediaSourceInfo"]["MediaStreams"][0][field] = value;
            assert!(
                !sync_json_matches(
                    &bytes(&changed),
                    &bytes(&submitted),
                    &bytes(&response),
                    path
                )
                .unwrap(),
                "{field}"
            );
        }
        current[0]["MediaSourceInfo"]["Size"] = json!(43);
        assert!(
            !sync_json_matches(
                &bytes(&current),
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .unwrap()
        );
        assert!(
            sync_json_matches(
                br#"[{"Title":"a","Title":"b"}]"#,
                &bytes(&submitted),
                &bytes(&response),
                path
            )
            .is_err()
        );
    }

    #[test]
    fn semantic_json_comparison_accepts_formatting_without_ignoring_real_changes() {
        let path = Path::new("/media/item-mediainfo.json");
        assert!(json_equivalent(br#"{"Size":42,"MediaStreams":[{"Title":"original","Language":"jpn"}]}"#,
            b"{ \n \"MediaStreams\": [{ \"Language\":\"jpn\", \"Title\":\"original\" }],\"Size\":42 }", path).unwrap());
        assert!(
            !json_equivalent(br#"{"Title":"original"}"#, br#"{"Title":"changed"}"#, path).unwrap()
        );
        assert!(!json_equivalent(br#"[1,2]"#, br#"[2,1]"#, path).unwrap());
        assert!(!json_equivalent(br#"{"Title":null}"#, br#"{}"#, path).unwrap());
    }

    #[test]
    fn ambiguous_duplicate_keys_and_invalid_json_cannot_be_treated_as_unchanged() {
        let path = Path::new("/media/item-mediainfo.json");
        for current in [
            br#"{"Title":"changed","Title":"original"}"#.as_slice(),
            br#"[{"Title":"changed","\u0054itle":"original"}]"#,
            br#"{"MediaStreams":[]} trailing"#,
            br#"{"Title":"original""#,
        ] {
            assert!(json_equivalent(current, br#"{"Title":"original"}"#, path).is_err());
        }
    }

    #[test]
    fn explicit_hash_confirmation_must_match_the_exact_actual_conflict_set() {
        let hash = "a".repeat(64);
        let conflicts =
            vec![json!({"item_id":"61616","path":"/media/item.json","current_sha256":hash})];
        assert!(validate_current_json_hashes(&conflicts, &BTreeMap::new()).is_err());
        assert!(
            validate_current_json_hashes(
                &conflicts,
                &BTreeMap::from([("61616".into(), hash.clone())])
            )
            .is_ok()
        );
        assert!(
            validate_current_json_hashes(
                &conflicts,
                &BTreeMap::from([("61616".into(), "b".repeat(64))])
            )
            .is_err()
        );
        assert!(
            validate_current_json_hashes(
                &conflicts,
                &BTreeMap::from([("61617".into(), hash.clone())])
            )
            .is_err()
        );
        assert!(
            validate_current_json_hashes(&[], &BTreeMap::from([("61616".into(), hash)])).is_err()
        );
    }

    #[test]
    fn json_diagnostics_count_all_changes_and_keep_missing_distinct_from_null() {
        let expected = json!({"Title":null,"tracks":vec![0;80]});
        let current = json!({"tracks":vec![1;80]});
        let mut changes = vec![];
        let mut count = 0;
        json_changes(
            Some(&expected),
            Some(&current),
            "",
            &mut changes,
            &mut count,
        );
        assert_eq!(count, 81);
        assert_eq!(changes.len(), 64);
        assert_eq!(changes[0]["path"], "/Title");
        assert_eq!(changes[0]["expected"], Value::Null);
        assert_eq!(changes[0]["current"]["missing"], true);
    }

    #[tokio::test]
    async fn json_guard_is_semantic_but_binary_resource_guard_stays_byte_exact() {
        let temporary = tempfile::tempdir().unwrap();
        let backup = temporary.path().join("asset-backup");
        tokio::fs::write(&backup, br#"{"Title":"original","Size":42}"#)
            .await
            .unwrap();
        let formatted = br#"{ "Size":42, "Title":"original" }"#;
        let json = temporary.path().join("item.json");
        let binary = temporary.path().join("item.bif");
        tokio::fs::write(&json, formatted).await.unwrap();
        tokio::fs::write(&binary, formatted).await.unwrap();
        assert!(
            json_unchanged(&json, &backup, &BTreeMap::new())
                .await
                .unwrap()
        );
        assert!(
            !json_unchanged(&binary, &backup, &BTreeMap::new())
                .await
                .unwrap()
        );
        let observed = BTreeMap::from([(json.clone(), Some(formatted.to_vec()))]);
        tokio::fs::write(&json, br#"{"Title":"new change","Size":42}"#)
            .await
            .unwrap();
        assert!(verify_current_json_snapshot(&observed).await.is_err());
        assert_eq!(
            tokio::fs::read(&json).await.unwrap(),
            br#"{"Title":"new change","Size":42}"#
        );
    }

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
