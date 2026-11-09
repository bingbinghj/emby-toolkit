use crate::{
    AppState,
    error::{ApiError, Result},
    identity,
    jobs::JobContext,
    language,
    media_edit::{self, list},
    media_info,
    state::string,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
};

fn protected(path: &Path) -> bool {
    path.components().any(|part| {
        ["cd2", "115", "115open"]
            .contains(&part.as_os_str().to_string_lossy().to_lowercase().as_str())
    })
}
async fn atomic(path: &Path, data: &Value) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let stage = path.with_file_name(format!(
        ".{}.language-title-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)
            .await?;
        file.write_all(&serde_json::to_vec_pretty(data).unwrap())
            .await?;
        file.sync_all().await?;
        tokio::fs::rename(&stage, path).await?;
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(stage).await;
    result
}

async fn verify_saved_target(state: &AppState, id: &str, snapshot: &Value) -> Result<Value> {
    let target = media_edit::repair_target(state, id).await?;
    if target.path != snapshot["media_path"]
        || json!(target.sha) != snapshot["sha1"]
        || target.asset != snapshot["asset"]
        || json!(target.versions) != snapshot["versions"]
        || target.top_id != snapshot["top_id"]
        || json!(crate::media_paths::strm_target(&target.path).await) != snapshot["source"]
        || tokio::fs::canonicalize(string(snapshot, "candidate")).await?
            != Path::new(&string(snapshot, "json_path"))
    {
        return Err(ApiError::bad(
            "媒体文件身份、版本、JSON路径或STRM来源在标题保存期间变化，已停止通知Emby",
        ));
    }
    let live = media_edit::details(state, id, "Id,Path,Size,MediaSources,MediaStreams").await?;
    if live["Id"] != id || live["Path"] != snapshot["media_path"] {
        return Err(ApiError::bad(
            "Emby媒体身份或路径在标题保存期间变化，已停止通知Emby",
        ));
    }
    identity::verify(&json!({"size_bytes":live["Size"]}), &snapshot["info"])?;
    let paths = [
        snapshot["media_path"].as_str().unwrap_or(""),
        snapshot["source"].as_str().unwrap_or(""),
    ];
    if media_info::item_streams(&live, &paths).is_none() {
        return Err(ApiError::bad("Emby未返回唯一的目标媒体源，已停止通知Emby"));
    }
    Ok(live)
}

async fn title_readback_before_deadline<T>(
    deadline: tokio::time::Instant,
    read: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let expired = || ApiError::bad("共享30秒标题回读期限已耗尽，未继续请求Emby");
    if tokio::time::Instant::now() >= deadline {
        return Err(expired());
    }
    tokio::time::timeout_at(deadline, read)
        .await
        .map_err(|_| expired())?
}

// Type-local ordinal avoids comparing Emby stream Index with ffprobe/local JSON Index.
fn expected_titles(info: &Value, report: &Value) -> Vec<Value> {
    let pending = list(&report["pending"]);
    let skipped = list(&report["skipped_tracks"]);
    let mut expected = vec![];
    let mut ordinals = [0_u64; 2];
    for stream in media_info::container(info)
        .and_then(|s| s["MediaStreams"].as_array())
        .into_iter()
        .flatten()
    {
        let slot = match stream["Type"].as_str() {
            Some("Audio") => 0,
            Some("Subtitle") => 1,
            _ => continue,
        };
        ordinals[slot] += 1;
        let ordinal = ordinals[slot];
        let unresolved = pending
            .iter()
            .find(|entry| entry["type"] == stream["Type"] && entry["ordinal"] == ordinal);
        if unresolved.is_some_and(|entry| entry["title_ready"] != true)
            || skipped
                .iter()
                .any(|entry| entry["type"] == stream["Type"] && entry["ordinal"] == ordinal)
        {
            continue;
        }
        expected.push(json!({"type":stream["Type"],"ordinal":ordinal,"language":stream["Language"],"check_language":unresolved.is_none(),"codec":stream["Codec"],"title":stream["Title"]}));
    }
    expected
}
fn same_language(expected: &Value, actual: &Value, mappings: &Value) -> bool {
    let canonical = |value: &Value| {
        let raw = value.as_str().unwrap_or("");
        language::lookup(raw, mappings)
            .map(|entry| language::code(&entry))
            .unwrap_or_else(|| language::token(raw))
    };
    canonical(expected) == canonical(actual)
}

fn preserve_skipped_titles(
    info: &mut Value,
    item: &Value,
    paths: &[&str],
    skipped: &[Value],
) -> Result<()> {
    if skipped.is_empty() {
        return Ok(());
    }
    let live = media_info::item_streams(item, paths)
        .ok_or_else(|| ApiError::bad("无法确认当前Emby中文字幕，已停止自动保存，未覆盖原标题"))?;
    let streams = media_info::streams_mut(info)
        .ok_or_else(|| ApiError::bad("本地媒体信息缺少字幕流，无法保护当前中文字幕标题"))?;
    let original_subtitles = streams
        .iter()
        .filter(|stream| stream["Type"] == "Subtitle")
        .collect::<Vec<_>>();
    let current_subtitles = live
        .iter()
        .filter(|stream| stream["Type"] == "Subtitle")
        .collect::<Vec<_>>();
    if original_subtitles.len() != current_subtitles.len() {
        return Err(ApiError::bad(format!(
            "Emby字幕范围变化（本地 {} 条，当前 {} 条），已停止自动保存，未覆盖原标题",
            original_subtitles.len(),
            current_subtitles.len()
        )));
    }
    if original_subtitles
        .iter()
        .zip(&current_subtitles)
        .any(|(original, current)| {
            original["Codec"]
                .as_str()
                .filter(|codec| !codec.is_empty())
                .is_none_or(|codec| {
                    current["Codec"]
                        .as_str()
                        .is_none_or(|actual| !actual.eq_ignore_ascii_case(codec))
                })
                || original["IsExternal"].as_bool().unwrap_or(false)
                    != current["IsExternal"].as_bool().unwrap_or(false)
        })
    {
        return Err(ApiError::bad(
            "Emby中文字幕编码与本地记录不一致，或字幕内外置身份变化，已停止自动保存，未覆盖原标题",
        ));
    }
    for entry in skipped {
        if entry["type"] != "Subtitle" {
            return Err(ApiError::bad("中文字幕类型无效，已停止自动保存"));
        }
        let index = entry["ordinal"]
            .as_u64()
            .and_then(|n| n.checked_sub(1))
            .ok_or_else(|| ApiError::bad("中文字幕序号无效，已停止自动保存"))?
            as usize;
        let original = streams
            .iter_mut()
            .filter(|s| s["Type"] == entry["type"])
            .nth(index)
            .ok_or_else(|| ApiError::bad("本地中文字幕范围变化，已停止自动保存"))?;
        let current = live
            .iter()
            .filter(|s| s["Type"] == entry["type"])
            .nth(index)
            .ok_or_else(|| ApiError::bad("Emby中文字幕范围变化，已停止自动保存"))?;
        for field in ["Title", "Language"] {
            if let Some(value) = current.get(field) {
                original[field] = value.clone();
            } else {
                original.as_object_mut().unwrap().remove(field);
            }
        }
    }
    Ok(())
}

fn mismatches(expected: &[Value], item: &Value, paths: &[&str], mappings: &Value) -> Vec<Value> {
    let streams = media_info::item_streams(item, paths);
    expected
        .iter()
        .filter_map(|entry| {
            let stream = streams.and_then(|streams| {
                streams
                    .iter()
                    .filter(|s| s["Type"] == entry["type"])
                    .nth(entry["ordinal"].as_u64().unwrap() as usize - 1)
            });
            let reason = match stream {
                None => Some("Emby 未返回目标版本的对应轨道".to_string()),
                Some(stream) if stream["Title"] != entry["title"] => Some(format!(
                    "标题不一致（期望 {}，实际 {}）",
                    entry["title"], stream["Title"]
                )),
                Some(stream)
                    if entry["codec"].as_str().is_some_and(|codec| {
                        stream["Codec"]
                            .as_str()
                            .is_none_or(|actual| !actual.eq_ignore_ascii_case(codec))
                    }) =>
                {
                    Some(format!(
                        "轨道编码不一致（期望 {}，实际 {}）",
                        entry["codec"], stream["Codec"]
                    ))
                }
                Some(stream)
                    if entry["check_language"] == true
                        && !same_language(&entry["language"], &stream["Language"], mappings) =>
                {
                    Some(format!(
                        "语言不一致（期望 {}，实际 {}）",
                        entry["language"], stream["Language"]
                    ))
                }
                _ => None,
            };
            reason.map(|reason| {
                let mut entry = entry.clone();
                entry["reason"] = json!(reason);
                entry["actual_title"] =
                    stream.map_or(Value::Null, |stream| stream["Title"].clone());
                entry["actual_codec"] =
                    stream.map_or(Value::Null, |stream| stream["Codec"].clone());
                entry["actual_language"] =
                    stream.map_or(Value::Null, |stream| stream["Language"].clone());
                entry
            })
        })
        .collect()
}

async fn checkpoint(context: Option<&JobContext>) -> Result<()> {
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    Ok(())
}
fn media_label(
    name: &str,
    season: Option<i64>,
    episode: Option<i64>,
    file: &str,
    id: &str,
) -> String {
    let mut parts = vec![];
    if !name.is_empty() {
        parts.push(format!("《{name}》"));
    }
    if let Some(season) = season {
        parts.push(format!("第{season}季"));
    }
    if let Some(episode) = episode {
        parts.push(format!("第{episode}集"));
    }
    let file = file.split(['?', '#']).next().unwrap_or("");
    if !file.is_empty() {
        parts.push(file.into());
    }
    parts.push(format!("媒体ID：{id}"));
    parts.join(" · ")
}
fn track_label(entry: &Value) -> String {
    let kind = match entry["type"].as_str() {
        Some("Audio") => "音轨",
        Some("Subtitle") => "字幕",
        _ => return "媒体项目".into(),
    };
    format!(
        "{kind}#{}，语言 {}",
        entry["ordinal"].as_u64().unwrap_or(0),
        entry["language"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or("未标注")
    )
}

pub async fn fill(state: &AppState, ids: &[String], skip_chinese: bool) -> Value {
    fill_inner(
        state,
        ids,
        skip_chinese,
        None,
        None::<std::future::Ready<Result<()>>>,
    )
    .await
    .unwrap_or_else(|error| json!({"completed":false,"failed":1,"pending":[{"reason":error.1}]}))
}
pub async fn fill_in_job(
    state: &AppState,
    ids: &[String],
    skip_chinese: bool,
    context: &JobContext,
) -> Result<Value> {
    fill_inner(
        state,
        ids,
        skip_chinese,
        Some(context),
        None::<std::future::Ready<Result<()>>>,
    )
    .await
}
pub async fn fill_in_job_with_refresh(
    state: &AppState,
    ids: &[String],
    skip_chinese: bool,
    context: &JobContext,
    refresh: impl std::future::Future<Output = Result<()>>,
) -> Result<Value> {
    fill_inner(state, ids, skip_chinese, Some(context), Some(refresh)).await
}
async fn fill_inner(
    state: &AppState,
    ids: &[String],
    skip_chinese: bool,
    context: Option<&JobContext>,
    refresh: Option<impl std::future::Future<Output = Result<()>>>,
) -> Result<Value> {
    let final_refresh = refresh.is_some();
    let config = state.config.read().await.clone();
    let root = tokio::fs::canonicalize(string(&config, "local_strm_root"))
        .await
        .ok()
        .filter(|p| p.is_dir() && !protected(p));
    let mut result = json!({"completed":false,"scanned":0,"changed_items":0,"changed_streams":0,"changed_languages":0,"verified_items":0,"verified_streams":0,"skipped":0,"skipped_chinese_streams":0,"failed":0,"pending":[],"refresh_state":"not_requested"});
    let Some(root) = root else {
        result["skipped"] = json!(ids.len());
        result["pending"] = json!(
            ids.iter()
                .map(|id| json!({"item_id":id,"reason":"媒体信息目录不可读写"}))
                .collect::<Vec<_>>()
        );
        if let Some(refresh) = refresh {
            refresh.await?;
        }
        log_report(&result);
        return Ok(result);
    };
    let mappings = match crate::db::setting(&state.db, "language_mapping").await {
        Ok(mappings) => mappings.unwrap_or(json!([])),
        Err(error) => {
            if let Some(refresh) = refresh {
                result["failed"] = json!(1);
                result["pending"] = json!([{"reason":error.1}]);
                refresh.await?;
                result["refresh_state"] = json!("submitted");
                log_report(&result);
                return Ok(result);
            }
            return Err(error);
        }
    };
    let mut processed = HashSet::new();
    let mut queued = BTreeMap::new();
    let mut snapshots = BTreeMap::new();
    let mut safety_error = None;
    for id in ids.iter().collect::<BTreeSet<_>>() {
        checkpoint(context).await?;
        if let Some(context) = context {
            context
                .activity("读取语言和标题", &format!("正在读取媒体ID {id} 的媒体信息"))
                .await;
        }
        result["scanned"] = json!(result["scanned"].as_u64().unwrap() + 1);
        let mut pending = vec![];
        let mut changed = 0;
        let mut changed_languages = 0;
        let mut unsafe_target = false;
        let mut label = format!("媒体ID：{id}");
        let outcome = async {
            let item = media_edit::details(state, id, "Id,Name,Path,Size,MediaSources,MediaStreams,PartCount,ExtraType,IndexNumber,ParentIndexNumber").await?;
            let clean_path = crate::media_paths::clean(&string(&item, "Path"));
            let file = Path::new(&clean_path).file_name().unwrap_or_default().to_string_lossy();
            label = media_label(&string(&item, "Name"), item["ParentIndexNumber"].as_i64(), item["IndexNumber"].as_i64(), &file, id);
            if let Some(context) = context {
                context.activity("填充语言和标题", &format!("正在检查 {label} 的音轨和字幕语言及标题")).await;
            }
            let annotate = |mut entry: Value| {
                entry["item_id"] = json!(id);
                entry["name"] = json!(item["Name"].as_str().unwrap_or(id));
                entry["season"] = item["ParentIndexNumber"].clone();
                entry["episode"] = item["IndexNumber"].clone();
                entry
            };
            if !string(&item, "ExtraType").is_empty() || item["PartCount"].as_u64().unwrap_or(0) > 1 {
                pending.push(annotate(json!({"reason":"分段或附加内容需手动填充"})));
                return Ok((true, 0));
            }
            let target = media_edit::repair_target(state, id).await?;
            if target.asset["fingerprint_pending"] == true { unsafe_target = true; return Err(ApiError::bad("文件身份待校验，请先重新校验指纹")); }
            let item_path = string(&item, "Path");
            if item["Id"] != *id || item_path != target.path { unsafe_target = true; return Err(ApiError::bad("Emby身份或路径与当前媒体资产不一致，已停止填充标题")); }
            let source = crate::media_paths::strm_target(&target.path).await;
            let strm_source = source.clone().unwrap_or_default();
            let source_paths = [&target.path[..], &item_path[..], &strm_source[..]];
            let mut expected = None;
            for candidate in &target.candidates {
                let Ok(path) = tokio::fs::canonicalize(candidate).await else { continue; };
                if !path.starts_with(&root) || protected(&path) || processed.contains(&path) || !path.is_file() { continue; }
                let original: Value = serde_json::from_slice(&tokio::fs::read(&path).await?).map_err(|_| ApiError::bad("媒体信息 JSON 无效"))?;
                identity::verify(&target.asset, &original).inspect_err(|_| unsafe_target = true)?;
                identity::verify(&json!({"size_bytes":item["Size"]}), &original).inspect_err(|_| unsafe_target = true)?;
                if !media_info::recoverable(&original) || !crate::media_paths::matches_container(&original, &target.path) {
                    unsafe_target = true;
                    return Err(ApiError::bad("本地媒体信息不可恢复或与当前媒体文件格式不一致，未清理 Emby 数据"));
                }
                let (mut updated, report) = language::fill(&original, &mappings, skip_chinese);
                let skipped = list(&report["skipped_tracks"]);
                result["skipped_chinese_streams"] = json!(result["skipped_chinese_streams"].as_u64().unwrap() + skipped.len() as u64);
                if !skipped.is_empty() {
                    tracing::info!("[语言标题] {label} 正常跳过 {} 条中文字幕，保留当前语言和标题", skipped.len());
                }
                pending.extend(list(&report["pending"]).into_iter().map(|mut entry| {
                    entry["file"] = json!(Path::new(&target.path).file_name().unwrap_or_default().to_string_lossy());
                    annotate(entry)
                }));
                processed.insert(path.clone());
                let count = report["changed"].as_u64().unwrap_or(0);
                let titles = expected_titles(&updated, &report);
                let snapshot = json!({"media_path":target.path,"sha1":target.sha,"asset":target.asset,"versions":target.versions,"top_id":target.top_id,"source":source,"candidate":candidate,"json_path":path,"info":original,"skipped":skipped});
                snapshots.insert(id.clone(), snapshot.clone());
                if !titles.is_empty() && !mismatches(&titles, &item, &source_paths, &mappings).is_empty() {
                    preserve_skipped_titles(&mut updated, &item, &source_paths, &skipped).inspect_err(|_| unsafe_target = true)?;
                }
                if updated != original {
                    if let Some(context) = context {
                        context.activity("保存语言和标题", &format!("正在保存 {label} 的本地媒体信息，本次规范化 {count} 条轨道语言和标题")).await;
                    }
                    verify_saved_target(state, id, &snapshot).await.inspect_err(|_| unsafe_target = true)?;
                    atomic(&path, &updated).await?;
                    changed = count;
                    changed_languages = report["changed_languages"].as_u64().unwrap_or(0);
                    if let Some(sha) = &target.sha
                        && let Err(error) = sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                            .bind(sha.to_uppercase()).bind(&updated).execute(&state.db).await {
                            tracing::warn!("[语言标题] {label} 本地 JSON 已保存，缓存同步失败：{error}");
                    }
                }
                expected = Some(titles);
                break;
            }
            let Some(expected) = expected else {
                pending.push(annotate(json!({"reason":"未找到本地媒体信息 JSON"})));
                return Ok((true, 0));
            };
            if expected.is_empty() { return Ok((false, 0)); }
            if !final_refresh && mismatches(&expected, &item, &source_paths, &mappings).is_empty() {
                if let Some(context) = context {
                    context.activity("语言和标题已一致", &format!("{label} 的 {} 条待处理轨道已通过语言和标题核对，无需提交文件刷新", expected.len())).await;
                }
                return Ok((false, expected.len()));
            }
            queued.insert(id.clone(), json!({"media_path":target.path,"strm_source":strm_source,"expected":expected.into_iter().map(annotate).collect::<Vec<_>>(),"label":label}));
            Ok((false, 0))
        }.await;
        result["changed_items"] =
            json!(result["changed_items"].as_u64().unwrap() + u64::from(changed > 0));
        result["changed_streams"] = json!(result["changed_streams"].as_u64().unwrap() + changed);
        result["changed_languages"] =
            json!(result["changed_languages"].as_u64().unwrap() + changed_languages);
        match outcome {
            Ok((skipped, verified)) => {
                result["skipped"] = json!(result["skipped"].as_u64().unwrap() + u64::from(skipped));
                result["verified_items"] =
                    json!(result["verified_items"].as_u64().unwrap() + u64::from(verified > 0));
                result["verified_streams"] =
                    json!(result["verified_streams"].as_u64().unwrap() + verified as u64);
            }
            Err(error) => {
                tracing::warn!("[语言标题] {label} 语言和标题填充或回读失败：{}", error.1);
                result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                if !pending
                    .iter()
                    .any(|p| string(p, "reason").contains("Emby 回读标题未一致"))
                {
                    pending
                        .push(json!({"item_id":id,"reason":format!("标题填充失败: {}",error.1)}));
                }
                if unsafe_target {
                    safety_error = Some(error);
                }
            }
        }
        result["pending"].as_array_mut().unwrap().extend(pending);
        checkpoint(context).await?;
    }
    if final_refresh && let Some(error) = safety_error {
        log_report(&result);
        return Err(error);
    }
    if !queued.is_empty() || final_refresh {
        checkpoint(context).await?;
        if let Some(context) = context {
            context
                .activity(
                    "提交媒体信息文件刷新",
                    &format!(
                        "汇总 {} 个媒体项目的文件变更并通知 Emby 重新读取",
                        queued.len()
                    ),
                )
                .await;
        }
        let paths = queued
            .values()
            .map(|input| string(input, "media_path"))
            .collect::<Vec<_>>();
        let directories = paths
            .iter()
            .filter_map(|path| Path::new(path).parent())
            .collect::<BTreeSet<_>>();
        let notified = async {
            if !final_refresh && result["failed"].as_u64().unwrap_or(0) > 0 {
                return Err(ApiError::bad("批次存在未能安全保存的目标，已停止通知Emby"));
            }
            for (id, snapshot) in &snapshots {
                checkpoint(context).await?;
                let live = verify_saved_target(state, id, snapshot).await?;
                if !final_refresh && !Path::new(snapshot["media_path"].as_str().unwrap_or("")).parent().is_some_and(|parent| directories.contains(parent)) {
                    continue;
                }
                let skipped = list(&snapshot["skipped"]);
                if skipped.is_empty() { continue; }
                let path = Path::new(snapshot["json_path"].as_str().unwrap_or(""));
                let original: Value = serde_json::from_slice(&tokio::fs::read(path).await?).map_err(|_| ApiError::bad("媒体信息 JSON 无效"))?;
                identity::verify(&snapshot["asset"], &original)?;
                let mut updated = original.clone();
                let source_paths = [snapshot["media_path"].as_str().unwrap_or(""), snapshot["source"].as_str().unwrap_or("")];
                preserve_skipped_titles(&mut updated, &live, &source_paths, &skipped)?;
                if updated != original {
                    verify_saved_target(state, id, snapshot).await?;
                    atomic(path, &updated).await?;
                    if let Some(sha) = snapshot["sha1"].as_str()
                        && let Err(error) = sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                            .bind(sha.to_uppercase()).bind(&updated).execute(&state.db).await {
                        tracing::warn!("[语言标题] 本地 JSON 已保存，缓存同步失败：{error}");
                    }
                }
            }
            // A later target can change an earlier target while the batch is prepared.
            for (id, snapshot) in &snapshots {
                checkpoint(context).await?;
                verify_saved_target(state, id, snapshot).await?;
            }
            // The ingest refresh must read the already updated JSON, just like
            // the Python workflow. Confirm live titles only after it completes.
            if let Some(refresh) = refresh {
                refresh.await
            } else {
                crate::media_save::refresh(state, &paths).await
            }
        }.await;
        let notified = match notified {
            Err(error) if final_refresh => return Err(error),
            outcome => outcome,
        };
        checkpoint(context).await?;
        let refresh_error = match notified {
            Ok(()) => {
                result["refresh_state"] = json!("submitted");
                None
            }
            Err(error) => {
                result["refresh_state"] = json!("failed");
                Some(error.1)
            }
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut remaining = queued.keys().cloned().collect::<BTreeSet<_>>();
        let mut outcomes = BTreeMap::new();
        if refresh_error.is_none() {
            while !remaining.is_empty() && tokio::time::Instant::now() < deadline {
                for id in remaining.clone() {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    let input = &queued[&id];
                    let expected = list(&input["expected"]);
                    let source_paths = [
                        input["media_path"].as_str().unwrap_or(""),
                        input["strm_source"].as_str().unwrap_or(""),
                    ];
                    if let Some(context) = context {
                        context
                            .activity(
                                "语言和标题回读验收",
                                &format!(
                                    "正在核对 {} 的实际音轨和字幕语言及标题",
                                    string(input, "label")
                                ),
                            )
                            .await;
                    }
                    checkpoint(context).await?;
                    let current = title_readback_before_deadline(
                        deadline,
                        verify_saved_target(state, &id, &snapshots[&id]),
                    )
                    .await
                    .map(|live| mismatches(&expected, &live, &source_paths, &mappings));
                    if current.as_ref().map_or(true, Vec::is_empty) {
                        remaining.remove(&id);
                    }
                    outcomes.insert(id, current);
                }
                if remaining.is_empty() {
                    break;
                }
                let delay = std::time::Duration::from_secs(1)
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                if let Some(context) = context {
                    context.sleep(delay).await?;
                } else {
                    tokio::time::sleep(delay).await;
                }
            }
        }
        for (id, input) in &queued {
            let expected = list(&input["expected"]);
            let readback = outcomes.remove(id).unwrap_or_else(|| {
                Err(ApiError::bad(refresh_error.clone().unwrap_or_else(|| {
                    "共享30秒标题回读期限已耗尽，该目标未完成回读，未继续请求Emby".into()
                })))
            });
            match readback {
                Ok(missing) if missing.is_empty() && refresh_error.is_none() => {
                    result["verified_items"] =
                        json!(result["verified_items"].as_u64().unwrap() + 1);
                    result["verified_streams"] =
                        json!(result["verified_streams"].as_u64().unwrap() + expected.len() as u64);
                    if let Some(context) = context {
                        context
                            .activity(
                                "语言和标题回读完成",
                                &format!(
                                    "{} 的 {} 条音轨和字幕已通过 Emby 语言和标题回读验收",
                                    string(input, "label"),
                                    expected.len()
                                ),
                            )
                            .await;
                    }
                }
                outcome => {
                    result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                    let reason = match outcome {
                        Ok(missing) if !missing.is_empty() => {
                            result["pending"].as_array_mut().unwrap().extend(
                                missing.into_iter().map(|mut entry| {
                                    entry["reason"] = json!(format!(
                                        "本地标题已保存，{}Emby 回读标题未一致：{}",
                                        refresh_error
                                            .as_ref()
                                            .map(|error| format!(
                                                "通知 Emby 重新读取失败（{error}）；"
                                            ))
                                            .unwrap_or_default(),
                                        string(&entry, "reason")
                                    ));
                                    entry
                                }),
                            );
                            None
                        }
                        Err(error) => Some(format!(
                            "本地标题已保存，文件刷新后回读验收失败：{}",
                            error.1
                        )),
                        _ => refresh_error.clone().map(|error| {
                            format!("本地标题已保存，但通知 Emby 重新读取失败：{error}")
                        }),
                    };
                    if let Some(reason) = reason {
                        result["pending"].as_array_mut().unwrap().push(json!({"item_id":id,"name":expected[0]["name"],"season":expected[0]["season"],"episode":expected[0]["episode"],"reason":reason}));
                    }
                }
            }
        }
    }
    result["completed"] = json!(
        result["scanned"].as_u64().unwrap_or(0) > 0
            && result["failed"].as_u64().unwrap_or(0) == 0
            && list(&result["pending"]).is_empty()
    );
    log_report(&result);
    Ok(result)
}
fn log_report(report: &Value) {
    for entry in list(&report["pending"]) {
        tracing::warn!(
            "[语言标题] {} · {}：{}",
            media_label(
                &string(&entry, "name"),
                entry["season"].as_i64(),
                entry["episode"].as_i64(),
                &string(&entry, "file"),
                &string(&entry, "item_id")
            ),
            track_label(&entry),
            string(&entry, "reason")
        );
    }
    tracing::info!(
        "[语言标题] 检查 {} 个目标媒体，本地更新 {} 条轨道的语言和标题，其中语言变化 {} 条，Emby 回读确认 {} 条轨道，正常跳过 {} 条中文字幕，失败 {} 个，待手动处理 {} 项",
        report["scanned"],
        report["changed_streams"],
        report["changed_languages"],
        report["verified_streams"],
        report["skipped_chinese_streams"],
        report["failed"],
        list(&report["pending"]).len()
    );
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[tokio::test]
async fn slow_title_readback_obeys_deadline_and_expired_reads_are_not_started() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                axum::Json(json!({}))
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(50);
    let read = async {
        reqwest::Client::new()
            .get(format!("http://{address}/"))
            .send()
            .await?;
        Ok(json!({}))
    };
    let outcome = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        title_readback_before_deadline(deadline, read),
    )
    .await;
    server.abort();
    assert!(
        outcome
            .expect("the shared deadline must cancel a slow HTTP read")
            .is_err()
    );
    let started = std::sync::atomic::AtomicBool::new(false);
    let late_read = async {
        started.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(json!({}))
    };
    assert!(
        title_readback_before_deadline(deadline, late_read)
            .await
            .is_err()
    );
    assert!(!started.load(std::sync::atomic::Ordering::SeqCst));
}
