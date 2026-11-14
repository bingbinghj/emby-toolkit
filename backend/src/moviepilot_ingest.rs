//! A notification identifies a scope, not a media revision. Resolve the current
//! entity before skipping work and recheck it before publishing new media data.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    media_info, media_paths,
    media_save::SavedTarget,
    state::string,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Identity {
    pub id: String,
    pub path: String,
    pub source: Option<String>,
    pub entity: Value,
    pub cloud: Option<Value>,
    relative: Option<String>,
}
impl Identity {
    pub fn revision(&self) -> Value {
        json!({"id":self.id,"path":self.path,"source":self.source,"entity":self.entity})
    }
    fn content(&self) -> Value {
        let content = if let Some(cloud) = &self.cloud {
            json!({"sha1":string(cloud,"sha1").to_uppercase(),"size":cloud["size"]})
        } else {
            self.entity.clone()
        };
        json!({"id":self.id,"source":self.source,"content":content})
    }
}

pub(crate) async fn observe(
    state: &AppState,
    item: &Value,
    context: &JobContext,
) -> Result<Identity> {
    context.checkpoint().await?;
    let path = string(item, "Path");
    let source = media_paths::strm_target(&path).await;
    if media_paths::extension(&path) == ".strm" && source.is_none() {
        return Err(ApiError::bad(
            "STRM 来源为空或不可读取，无法确认实际媒体文件身份",
        ));
    }
    let actual = source.as_deref().unwrap_or(&path);
    let config = state.config.read().await.clone();
    if crate::media_save::protected(&config, &path) {
        return Err(ApiError::bad(
            "Emby 原生路径位于云端源目录，请使用本地 STRM 路径，已禁止写入媒体侧车",
        ));
    }
    let cloud = crate::fingerprints::pickcode(actual).is_some()
        || crate::media_save::protected(&config, actual);
    let (entity, cloud, relative) = if cloud {
        let (cloud, relative) = crate::revalidation::resolve_ingest(state, actual, &path, context)
            .await
            .map_err(|error| {
                ApiError(
                    error.0,
                    format!(
                        "无法确认当前云端文件身份：{}；请核对115凭证和媒体路径映射",
                        error.1
                    ),
                )
            })?;
        if string(&cloud, "sha1").len() != 40
            || !string(&cloud, "sha1")
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || crate::identity::size(&cloud["size"]).is_none_or(|size| size > i64::MAX as u64)
        {
            return Err(ApiError::bad("115 未返回有效的当前文件 SHA1 或精确大小"));
        }
        (cloud.clone(), Some(cloud), relative)
    } else if actual.starts_with("http://") || actual.starts_with("https://") {
        let response = state
            .http
            .head(actual)
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?;
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|etag| !etag.starts_with("W/") && !etag.is_empty())
            .ok_or_else(|| {
                ApiError::bad("远程媒体未提供可验证的内容标识，无法确认覆盖版本，请核对后重新提取")
            })?;
        (
            json!({"etag":etag,"size":response.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|value|value.to_str().ok()).and_then(|value|value.parse::<u64>().ok())}),
            None,
            None,
        )
    } else {
        let metadata = tokio::fs::metadata(actual)
            .await
            .map_err(|_| ApiError::bad("无法确认实际媒体文件身份，请核对 STRM 来源及挂载"))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(ApiError::bad("实际媒体来源不是有效文件"));
        }
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApiError::bad("实际媒体文件时间无效"))?
            .as_nanos()
            .to_string();
        let mut entity = json!({"size":metadata.len(),"modified":modified});
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            entity["device"] = json!(metadata.dev());
            entity["inode"] = json!(metadata.ino());
            entity["ctime"] = json!([metadata.ctime(), metadata.ctime_nsec()]);
        }
        (entity, None, None)
    };
    Ok(Identity {
        id: string(item, "Id"),
        path,
        source,
        entity,
        cloud,
        relative,
    })
}

pub(crate) async fn unchanged(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    let live = details(state, &identity.id, "Id,Path,MediaSources,MediaStreams").await?;
    if live["Id"] != identity.id
        || observe(state, &live, context).await?.revision() != identity.revision()
    {
        return Err(ApiError::bad(
            "媒体文件在处理期间再次变化，已停止提交，请重试当前版本",
        ));
    }
    Ok(())
}

async fn slot(state: &AppState, id: &str) -> Result<Option<(Value, usize)>> {
    let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE item_type IN ('Movie','Episode') AND emby_item_ids_json @> $1::jsonb LIMIT 1")
        .bind(json!([id])).fetch_optional(&state.db).await?;
    Ok(row.and_then(|row| {
        list(&row["emby_item_ids_json"])
            .iter()
            .position(|value| value == id)
            .map(|index| (row, index))
    }))
}

pub(crate) async fn completed(state: &AppState, identity: &Identity) -> Result<bool> {
    let marked = slot(state, &identity.id)
        .await?
        .is_some_and(|(row, index)| {
            row["in_library"] == true
                && row["asset_details_json"][index]["moviepilot_ingest_identity"]
                    == identity.revision()
        });
    Ok(marked && media_ready(state, identity).await?)
}

fn valid_info(info: &Value, identity: &Identity) -> bool {
    let Some(source) = media_info::container(info) else {
        return false;
    };
    let video = source["MediaStreams"].as_array().is_some_and(|streams| {
        streams.iter().any(|stream| {
            stream["Type"] == "Video"
                && stream["Width"].as_f64().is_some_and(|v| v > 0.0)
                && stream["Height"].as_f64().is_some_and(|v| v > 0.0)
        })
    });
    video
        && media_info::recoverable(info)
        && (crate::identity::size(&identity.entity["size"]).is_none()
            || crate::identity::size(&source["Size"])
                == crate::identity::size(&identity.entity["size"]))
}

fn readback_matches(info: &Value, live: &Value, identity: &Identity) -> bool {
    let target = SavedTarget {
        id: identity.id.clone(),
        path: identity.path.clone(),
        source: identity.source.clone(),
        sha: None,
        asset: None,
        info: info.clone(),
        refresh_required: true,
    };
    if live["Id"] != identity.id
        || live["Path"] != identity.path
        || !crate::media_reload::differences(&target, live).is_empty()
    {
        return false;
    }
    let paths = [
        identity.path.as_str(),
        identity.source.as_deref().unwrap_or(""),
    ];
    let Some(actual) = media_info::item_source(live, &paths) else {
        return false;
    };
    let Some(expected) = media_info::container(info) else {
        return false;
    };
    for field in ["Size", "RunTimeTicks"] {
        if !expected[field].is_null()
            && crate::identity::size(&expected[field]) != crate::identity::size(&actual[field])
        {
            return false;
        }
    }
    for (expected, actual) in list(&expected["MediaStreams"])
        .iter()
        .zip(list(&actual["MediaStreams"]))
    {
        if !matches!(
            expected["Type"].as_str(),
            Some("Video" | "Audio" | "Subtitle")
        ) {
            for field in [
                "Type",
                "Title",
                "Codec",
                "Language",
                "Index",
                "AttachmentSize",
            ] {
                if expected[field] != actual[field] {
                    return false;
                }
            }
        }
    }
    let expected_chapters = &media_info::object(info).unwrap()["Chapters"];
    matches!((media_info::signature(expected_chapters),media_info::signature(&live["Chapters"])),(Ok(a),Ok(b)) if a==b)
}

async fn media_ready(state: &AppState, identity: &Identity) -> Result<bool> {
    let marked = slot(state, &identity.id)
        .await?
        .is_some_and(|(row, index)| {
            row["asset_details_json"][index]["moviepilot_media_pending"] != true
                && row["asset_details_json"][index]["moviepilot_media_identity"]
                    == identity.content()
        });
    if !marked {
        return Ok(false);
    }
    let Ok(bytes) = tokio::fs::read(sidecar(&identity.path)).await else {
        return Ok(false);
    };
    let Ok(info) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(false);
    };
    if !valid_info(&info, identity) {
        return Ok(false);
    }
    let live = details(
        state,
        &identity.id,
        "Id,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters",
    )
    .await?;
    Ok(readback_matches(&info, &live, identity))
}

async fn reusable_content(state: &AppState, identity: &Identity) -> Result<Option<Value>> {
    let Some(cloud) = &identity.cloud else {
        return Ok(None);
    };
    let Some((row, index)) = slot(state, &identity.id).await? else {
        return Ok(None);
    };
    let marker = &row["asset_details_json"][index]["moviepilot_media_identity"];
    let content = json!({"sha1":string(cloud,"sha1").to_uppercase(),"size":cloud["size"]});
    if marker["id"] != identity.id || marker["content"] != content {
        return Ok(None);
    }
    let info = match tokio::fs::read(sidecar(&identity.path)).await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            sqlx::query_scalar::<_, Option<Value>>(
                "SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1",
            )
            .bind(string(cloud, "sha1").to_uppercase())
            .fetch_optional(&state.db)
            .await?
            .flatten()
        }
        Err(error) => return Err(error.into()),
    };
    let Some(info) = info else { return Ok(None) };
    let live = details(
        state,
        &identity.id,
        "Id,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters",
    )
    .await?;
    let mut before = identity.clone();
    before.source = marker["source"].as_str().map(str::to_string);
    if valid_info(&info, identity) && readback_matches(&info, &live, &before) {
        Ok(Some(info))
    } else {
        Ok(None)
    }
}

async fn write_identity(
    state: &AppState,
    identity: &Identity,
    media: bool,
    complete: bool,
) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    if let (Some(cloud), Some(relative)) = (&identity.cloud, &identity.relative) {
        sqlx::query(
            "DELETE FROM p115_filesystem_cache WHERE id=$1 AND (parent_id<>$2 OR name<>$3)",
        )
        .bind(string(cloud, "id"))
        .bind(string(cloud, "parent_id"))
        .bind(string(cloud, "name"))
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,pick_code,local_path,size) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(parent_id,name) DO UPDATE SET id=EXCLUDED.id,sha1=EXCLUDED.sha1,pick_code=EXCLUDED.pick_code,local_path=EXCLUDED.local_path,size=EXCLUDED.size,updated_at=NOW()")
            .bind(string(cloud,"id")).bind(string(cloud,"parent_id")).bind(string(cloud,"name")).bind(string(cloud,"sha1").to_uppercase()).bind(string(cloud,"pick_code")).bind(relative).bind(crate::identity::size(&cloud["size"]).unwrap() as i64).execute(&mut *transaction).await?;
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE item_type IN ('Movie','Episode') AND emby_item_ids_json @> $1::jsonb FOR UPDATE")
        .bind(json!([identity.id])).fetch_all(&mut *transaction).await?;
    for row in rows {
        let Some(index) = list(&row["emby_item_ids_json"])
            .iter()
            .position(|id| id == &identity.id)
        else {
            continue;
        };
        let mut assets = list(&row["asset_details_json"]);
        let mut pcs = list(&row["file_pickcode_json"]);
        let mut shas = list(&row["file_sha1_json"]);
        assets.resize(list(&row["emby_item_ids_json"]).len(), json!({}));
        pcs.resize(assets.len(), Value::Null);
        shas.resize(assets.len(), Value::Null);
        if string(&assets[index], "path") != identity.path {
            return Err(ApiError::bad("当前媒体绑定路径已变化，停止记录入库完成"));
        }
        if let Some(cloud) = &identity.cloud
            && (media || complete)
        {
            let (asset, pc, sha) = crate::identity::merge(
                &assets[index],
                &assets[index],
                &pcs[index],
                &shas[index],
                &cloud["pick_code"],
                &cloud["sha1"],
            );
            assets[index] = asset;
            pcs[index] = pc;
            shas[index] = sha;
            assets[index]["size_bytes"] = cloud["size"].clone();
            assets[index]["fingerprint_pending"] = json!(false);
        }
        if media {
            assets[index]["moviepilot_media_identity"] = identity.content();
            assets[index]["moviepilot_media_pending"] = json!(false);
        } else if !complete {
            assets[index]["moviepilot_media_pending"] = json!(true);
        }
        if complete {
            assets[index]["moviepilot_ingest_identity"] = identity.revision();
        }
        sqlx::query("UPDATE media_metadata SET asset_details_json=$1,file_pickcode_json=$2,file_sha1_json=$3,last_updated_at=NOW() WHERE tmdb_id=$4 AND item_type=$5")
            .bind(json!(assets)).bind(json!(pcs)).bind(json!(shas)).bind(string(&row,"tmdb_id")).bind(string(&row,"item_type")).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn finish(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    unchanged(state, identity, context).await?;
    let destination = sidecar(&identity.path);
    let original = tokio::fs::read(&destination).await?;
    let mut info: Value = serde_json::from_slice(&original)
        .map_err(|_| ApiError::bad("收尾媒体信息 JSON 无效，未记录入库完成"))?;
    let live = details(
        state,
        &identity.id,
        "Id,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters",
    )
    .await?;
    let chapters = media_info::clean_chapters(&live["Chapters"])?;
    // Intro/credits tasks add markers only after the new entity was imported.
    // Keep these verified markers in its sidecar without accepting other stale
    // streams or technical fields from the final metadata refresh.
    for bundle in info.as_array_mut().into_iter().flatten() {
        bundle["Chapters"] = chapters.clone();
        if bundle["MediaSourceInfo"].get("Chapters").is_some() {
            bundle["MediaSourceInfo"]["Chapters"] = chapters.clone();
        }
    }
    if !valid_info(&info, identity) || !readback_matches(&info, &live, identity) {
        return Err(ApiError::bad(
            "收尾后当前媒体流尚未完整验收，未记录入库完成",
        ));
    }
    let bytes = serde_json::to_vec(&info).unwrap();
    if bytes != original {
        let stage =
            destination.with_file_name(format!(".etk-finish-{}", uuid::Uuid::new_v4().simple()));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stage)
                .await?;
            file.write_all(&bytes).await?;
            file.sync_all().await?;
            drop(file);
            unchanged(state, identity, context).await?;
            crate::media_save::commit_json_stage_guarded(&stage, &destination, Some(&original))
                .await
        }
        .await;
        let _ = tokio::fs::remove_file(stage).await;
        result?;
    }
    if let Some(cloud) = &identity.cloud {
        crate::media_save::cache_saved(state, Some(&string(cloud, "sha1").to_uppercase()), &info)
            .await?;
    }
    unchanged(state, identity, context).await?;
    write_identity(state, identity, true, true).await
}

fn sidecar(path: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}-mediainfo.json",
        Path::new(path).with_extension("").to_string_lossy()
    ))
}

async fn local_probe(state: &AppState, identity: &Identity, context: &JobContext) -> Result<Value> {
    let _slot = crate::strm::slot(state, "main", Some(context)).await?;
    let mut command = tokio::process::Command::new("ffprobe");
    command.kill_on_drop(true).args([
        "-v",
        "error",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-of",
        "json",
        identity.source.as_deref().unwrap_or(&identity.path),
    ]);
    let output = tokio::select! {
        _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
        output=tokio::time::timeout(Duration::from_secs(120),command.output())=>output.map_err(|_|ApiError::bad("本地媒体信息提取超时"))??,
    };
    if !output.status.success() {
        return Err(ApiError::bad("本地媒体信息提取失败，旧媒体信息备份已保留"));
    }
    let raw: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| ApiError::bad("本地提取返回无效JSON"))?;
    let mappings = crate::db::setting(&state.db, "language_mapping")
        .await?
        .unwrap_or(json!([]));
    crate::probe::build(
        &raw,
        &json!({"fn":Path::new(identity.source.as_deref().unwrap_or(&identity.path)).file_name()}),
        &mappings,
    )
    .filter(media_info::recoverable)
    .ok_or_else(|| ApiError::bad("本地提取未返回有效媒体流"))
}

async fn retire_previews(
    state: &AppState,
    identity: &Identity,
    target: &SavedTarget,
    directory: &Path,
    context: &JobContext,
) -> Result<()> {
    let live = details(
        state,
        &identity.id,
        "Id,Path,Chapters,MediaSources,MediaStreams",
    )
    .await?;
    let mut indices = list(&live["Chapters"])
        .iter()
        .enumerate()
        .filter(|(_, chapter)| !string(chapter, "ImageTag").is_empty())
        .map(|(index, chapter)| chapter["ChapterIndex"].as_u64().unwrap_or(index as u64))
        .collect::<Vec<_>>();
    let path = Path::new(&identity.path);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut bifs = Vec::new();
    let mut entries = tokio::fs::read_dir(
        path.parent()
            .ok_or_else(|| ApiError::bad("媒体文件缺少父目录"))?,
    )
    .await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(suffix) = name
            .strip_prefix(&format!("{stem}-"))
            .and_then(|name| name.strip_suffix(".bif"))
            && suffix
                .split('-')
                .all(|piece| !piece.is_empty() && piece.bytes().all(|b| b.is_ascii_digit()))
        {
            if !entry.file_type().await?.is_file() {
                return Err(ApiError::bad("旧预览资源不是普通文件，已停止洗版资源清理"));
            }
            bifs.push(entry.path());
        }
    }
    if indices.is_empty() && bifs.is_empty() {
        return Ok(());
    }
    let previous = slot(state, &identity.id).await?;
    let known = previous
        .as_ref()
        .map(|(row, index)| row["asset_details_json"][*index]["moviepilot_media_identity"].clone())
        .unwrap_or(Value::Null);
    let old_sha = previous
        .as_ref()
        .map(|(row, index)| string(&json!({"sha":row["file_sha1_json"][*index]}), "sha"))
        .unwrap_or_default();
    let changed = if let Some(cloud) = &identity.cloud {
        if old_sha.len() != 40 || !old_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ApiError::bad(
                "旧预览资源缺少可验证的文件归属，请核对并清理旧生成资源后重试",
            ));
        }
        old_sha.to_uppercase() != string(cloud, "sha1").to_uppercase()
    } else {
        if known.is_null() {
            return Err(ApiError::bad(
                "旧预览资源缺少可验证的文件归属，请核对并清理旧生成资源后重试",
            ));
        }
        known != identity.content()
    };
    if !changed {
        return Ok(());
    }
    let images =
        crate::media_reload::chapter_images::capture(state, &identity.id, &live, Some(directory))
            .await?;
    tokio::fs::write(
        directory.join("chapter-images.json"),
        serde_json::to_vec(&images).unwrap(),
    )
    .await?;
    let mut captured = Vec::new();
    for path in bifs {
        let bytes = tokio::fs::read(&path).await?;
        let backup = directory.join(path.file_name().unwrap());
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        captured.push((path, bytes));
    }
    unchanged(state, identity, context).await?;
    crate::media_reload::preflight_ingest_import(state, target, context).await?;
    indices.sort_unstable();
    indices.dedup();
    for index in indices.into_iter().rev() {
        unchanged(state, identity, context).await?;
        state
            .emby_once(
                reqwest::Method::DELETE,
                &format!("Items/{}/Images/Chapter/{index}", identity.id),
                &[],
                None,
            )
            .await?;
    }
    for (path, bytes) in captured {
        unchanged(state, identity, context).await?;
        if tokio::fs::read(&path).await? != bytes {
            return Err(ApiError::bad("预览文件在清理前变化，保留当前资源及备份"));
        }
        tokio::fs::remove_file(path).await?;
    }
    unchanged(state, identity, context).await
}

async fn resume_media(state: &AppState, identity: &Identity, context: &JobContext) -> Result<bool> {
    let root = state.data_dir.join("backups/media-info-wash");
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut matching = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let marker = entry.path().join("pending-scan.json");
        if !tokio::fs::try_exists(&marker).await? {
            continue;
        }
        let pending = crate::media_recovery::read_json(&marker).await?;
        if pending["verification"]["current_entity"] == identity.revision() {
            matching.push((entry.path(), pending));
        }
    }
    if matching.len() > 1 {
        return Err(ApiError::bad(
            "当前版本存在多份未确认洗版记录，已保留恢复材料",
        ));
    }
    let Some((directory, pending)) = matching.pop() else {
        return Ok(false);
    };
    let targets = list(&pending["verification"]["targets"]);
    if targets.len() != 1 || targets[0]["id"] != identity.id || targets[0]["path"] != identity.path
    {
        return Err(ApiError::bad("洗版恢复的原目标范围不一致，未重建媒体信息"));
    }
    unchanged(state, identity, context).await?;
    let position = pending["position"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("洗版恢复缺少本次刷新记录"))?;
    let before = string(&pending, "before_date_last_refreshed");
    crate::emby::wait_item_refresh(
        state,
        &identity.id,
        (!before.is_empty()).then_some(before.as_str()),
        Some(crate::media_extra::RefreshTrace::new(position)),
        context,
        "继续新版媒体信息验收",
        &identity.id,
    )
    .await?;
    crate::emby::idle(state, context).await?;
    let info: Value = serde_json::from_slice(&tokio::fs::read(sidecar(&identity.path)).await?)
        .map_err(|_| ApiError::bad("洗版恢复的当前JSON无效，未覆盖恢复材料"))?;
    if info != targets[0]["info"] || !valid_info(&info, identity) {
        return Err(ApiError::bad(
            "洗版恢复的当前JSON与已提交新版不一致，未覆盖文件",
        ));
    }
    let original =
        crate::media_recovery::read_json(&directory.join("original-settings.json")).await?;
    let applied = crate::strm::recovery_applied_settings(&directory, &original).await?;
    crate::strm::finish_media_recovery(state, &original, applied.as_ref(), || async {
        unchanged(state, identity, context).await?;
        let live = details(
            state,
            &identity.id,
            "Id,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters",
        )
        .await?;
        if !readback_matches(&info, &live, identity) {
            return Err(ApiError::bad(
                "重启后的新版媒体流尚未通过回读验收，未恢复旧版JSON",
            ));
        }
        write_identity(state, identity, true, false).await
    })
    .await?;
    match tokio::fs::remove_file(directory.join("pending-scan.json")).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(cloud) = &identity.cloud {
        crate::media_save::cache_saved(state, Some(&string(cloud, "sha1").to_uppercase()), &info)
            .await?;
    }
    unchanged(state, identity, context).await?;
    Ok(true)
}

/// The old sidecar is never evidence for a replacement. Generate from the
/// currently verified content, publish one file, and prove native readback.
pub(crate) async fn ensure_media(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    if resume_media(state, identity, context).await? {
        return Ok(());
    }
    if media_ready(state, identity).await? {
        unchanged(state, identity, context).await?;
        return Ok(());
    }
    let reusable = reusable_content(state, identity).await?;
    write_identity(state, identity, false, false).await?;
    let mut info = if let Some(info) = reusable {
        info
    } else if let Some(cloud) = &identity.cloud {
        let info = crate::media_restore::rebuild_info(
            state,
            &string(cloud, "sha1").to_uppercase(),
            &string(cloud, "pick_code"),
            &string(cloud, "name"),
            context,
        )
        .await?;
        crate::identity::verify(&json!({"size_bytes":cloud["size"]}), &info)?;
        info
    } else {
        local_probe(state, identity, context).await?
    };
    info = media_info::restore_json(&info)?;
    if !valid_info(&info, identity) {
        return Err(ApiError::bad(
            "当前媒体提取缺少有效视频流或文件大小与当前实体不一致",
        ));
    }
    if info.as_array().is_none_or(|items| items.len() != 1) {
        return Err(ApiError::bad("当前媒体提取包含多个不明确媒体源"));
    }
    info[0]["MediaSourceInfo"]["Path"] =
        json!(identity.source.as_deref().unwrap_or(&identity.path));
    if info[0]["Chapters"].is_null() {
        info[0]["Chapters"] = json!([]);
    }
    unchanged(state, identity, context).await?;
    let destination = sidecar(&identity.path);
    let expected = match tokio::fs::read(&destination).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let directory = state
        .data_dir
        .join("backups/media-info-wash")
        .join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&directory).await?;
    tokio::fs::write(directory.join("identity.json"),serde_json::to_vec(&json!({"current":identity.revision(),"previous":slot(state,&identity.id).await?.map(|(row,index)|json!({"asset":row["asset_details_json"][index],"sha1":row["file_sha1_json"][index]}))})).unwrap()).await?;
    if let Some(bytes) = &expected {
        tokio::fs::write(directory.join("previous-mediainfo.json"), bytes).await?;
    }
    let stage = directory.join("current-mediainfo.json");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)
        .await?;
    file.write_all(&serde_json::to_vec(&info).unwrap()).await?;
    file.sync_all().await?;
    drop(file);
    let target = SavedTarget {
        id: identity.id.clone(),
        path: identity.path.clone(),
        source: identity.source.clone(),
        sha: None,
        asset: None,
        info: info.clone(),
        refresh_required: true,
    };
    crate::media_reload::preflight_ingest_import(state, &target, context).await?;
    let confirmed = std::sync::atomic::AtomicBool::new(true);
    crate::strm::with_settings_guarded(
        state,
        "restore_media_info",
        || async {
            unchanged(state, identity, context).await?;
            retire_previews(state, identity, &target, &directory, context).await?;
            crate::media_save::commit_json_stage_guarded(&stage, &destination, expected.as_deref())
                .await?;
            let position = crate::media_extra::log_position(state).await?;
            let before = details(state, &identity.id, "DateLastRefreshed").await?;
            tokio::fs::write(
                directory.join("pending-scan.json"),
                serde_json::to_vec(&json!({"position":position,"before_date_last_refreshed":before["DateLastRefreshed"],"ids":[identity.id],"verification":{"targets":[{"id":identity.id,"path":identity.path,"source":identity.source,"sha":identity.cloud.as_ref().map(|cloud|string(cloud,"sha1").to_uppercase()),"asset":null,"info":info}],"current_entity":identity.revision()}})).unwrap(),
            )
            .await?;
            confirmed.store(false, std::sync::atomic::Ordering::Release);
            if let Some(warning) =
                crate::media_reload::refresh_from_file(state, &target, context).await?
            {
                tracing::warn!("{warning}");
            }
            crate::emby::wait_item_refresh(
                state,
                &identity.id,
                Some(&string(&before, "DateLastRefreshed")),
                Some(crate::media_extra::RefreshTrace::new(position)),
                context,
                "新版媒体信息导入",
                &identity.id,
            )
            .await?;
            confirmed.store(true, std::sync::atomic::Ordering::Release);
            for attempt in 0..30 {
                context.checkpoint().await?;
                let live = details(
                    state,
                    &identity.id,
                    "Id,Path,Size,MediaSources,MediaStreams,Chapters",
                )
                .await?;
                if readback_matches(&info, &live, identity) {
                    unchanged(state, identity, context).await?;
                    if let Some(cloud) = &identity.cloud {
                        crate::media_save::cache_saved(
                            state,
                            Some(&string(cloud, "sha1").to_uppercase()),
                            &info,
                        )
                        .await?;
                    }
                    write_identity(state, identity, true, false).await?;
                    confirmed.store(true, std::sync::atomic::Ordering::Release);
                    return Ok(());
                }
                if attempt < 29 {
                    context.sleep(Duration::from_secs(1)).await?;
                }
            }
            Err(ApiError::bad(
                "新版媒体流或章节尚未通过 Emby 回读验收，恢复材料已保留",
            ))
        },
        &confirmed,
        &directory,
    )
    .await
}

pub(crate) async fn remember_initial(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    unchanged(state, identity, context).await?;
    let bytes = tokio::fs::read(sidecar(&identity.path)).await?;
    let info: Value =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("新文件媒体信息不是有效JSON"))?;
    let live = details(
        state,
        &identity.id,
        "Id,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters",
    )
    .await?;
    if !valid_info(&info, identity) || !readback_matches(&info, &live, identity) {
        return Err(ApiError::bad("新文件媒体信息尚未通过完整回读验收"));
    }
    write_identity(state, identity, true, false).await
}
