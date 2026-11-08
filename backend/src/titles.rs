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
    collections::{BTreeSet, HashSet},
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
    fill_inner(state, ids, skip_chinese, None)
        .await
        .unwrap_or_else(
            |error| json!({"completed":false,"failed":1,"pending":[{"reason":error.1}]}),
        )
}
pub async fn fill_in_job(
    state: &AppState,
    ids: &[String],
    skip_chinese: bool,
    context: &JobContext,
) -> Result<Value> {
    fill_inner(state, ids, skip_chinese, Some(context)).await
}
async fn fill_inner(
    state: &AppState,
    ids: &[String],
    skip_chinese: bool,
    context: Option<&JobContext>,
) -> Result<Value> {
    let config = state.config.read().await.clone();
    let root = tokio::fs::canonicalize(string(&config, "local_strm_root"))
        .await
        .ok()
        .filter(|p| p.is_dir() && !protected(p));
    let mut result = json!({"completed":false,"scanned":0,"changed_items":0,"changed_streams":0,"changed_languages":0,"verified_items":0,"verified_streams":0,"skipped":0,"skipped_chinese_streams":0,"failed":0,"pending":[]});
    let Some(root) = root else {
        result["skipped"] = json!(ids.len());
        result["pending"] = json!(
            ids.iter()
                .map(|id| json!({"item_id":id,"reason":"媒体信息目录不可读写"}))
                .collect::<Vec<_>>()
        );
        log_report(&result);
        return Ok(result);
    };
    let mappings = crate::db::setting(&state.db, "language_mapping")
        .await?
        .unwrap_or(json!([]));
    let mut processed = HashSet::new();
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
        let mut label = format!("媒体ID：{id}");
        let outcome = async {
            let item = media_edit::details(state, id, "Id,Name,Path,MediaSources,MediaStreams,PartCount,ExtraType,IndexNumber,ParentIndexNumber").await?;
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
            let target = media_edit::target(state, id).await?;
            if target.asset["fingerprint_pending"] == true { return Err(ApiError::bad("文件身份待校验，请先重新校验指纹")); }
            let item_path = string(&item, "Path");
            if item_path != target.path { return Err(ApiError::bad("Emby 路径与当前媒体资产不一致，已停止填充标题")); }
            let strm_source = crate::media_paths::strm_target(&target.path).await.unwrap_or_default();
            let source_paths = [&target.path[..], &item_path[..], &strm_source[..]];
            let mut expected = None;
            for candidate in &target.candidates {
                let Ok(path) = tokio::fs::canonicalize(candidate).await else { continue; };
                if !path.starts_with(&root) || protected(&path) || processed.contains(&path) || !path.is_file() { continue; }
                let original: Value = serde_json::from_slice(&tokio::fs::read(&path).await?).map_err(|_| ApiError::bad("媒体信息 JSON 无效"))?;
                identity::verify(&target.asset, &original)?;
                if !media_info::recoverable(&original) || !crate::media_paths::matches_container(&original, &target.path) {
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
                if !titles.is_empty() && (count > 0 || !mismatches(&titles, &item, &source_paths, &mappings).is_empty()) {
                    preserve_skipped_titles(&mut updated, &item, &source_paths, &skipped)?;
                }
                if count > 0 {
                    if let Some(context) = context {
                        context.activity("保存语言和标题", &format!("正在保存 {label} 的 {count} 条轨道语言和标题")).await;
                    }
                    atomic(&path, &updated).await?;
                    changed = count;
                    changed_languages = report["changed_languages"].as_u64().unwrap_or(0);
                    if let Some(sha) = &target.sha
                        && let Err(error) = sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                            .bind(sha.to_uppercase()).bind(&updated).execute(&state.db).await {
                            tracing::warn!("[语言标题] {label} 本地 JSON 已保存，缓存同步失败：{error}");
                    }
                }
                expected = Some((updated, titles));
                break;
            }
            let Some((updated, expected)) = expected else {
                pending.push(annotate(json!({"reason":"未找到本地媒体信息 JSON"})));
                return Ok((true, 0));
            };
            if expected.is_empty() { return Ok((false, 0)); }
            if changed == 0 && mismatches(&expected, &item, &source_paths, &mappings).is_empty() {
                if let Some(context) = context {
                    context.activity("语言和标题已一致", &format!("{label} 的 {} 条待处理轨道已通过语言和标题核对，无需重新扫描", expected.len())).await;
                }
                return Ok((false, expected.len()));
            }
            checkpoint(context).await?;
            let readback = async {
                if let Some(context) = context {
                    context.activity("恢复语言和标题到 Emby", &format!("正在通过受控媒体信息保存恢复 {label} 的 {} 条轨道语言和标题", expected.len())).await;
                }
                let body = json!({"mediainfo":updated,"media_path":target.path,"sha1":target.sha,"strm_source":strm_source});
                let (status, report) = crate::media_save::save_titles(state, id, &body, context).await?;
                if status >= 400 {
                    return Err(ApiError::bad(format!("{}；{}", string(&report, "error"), list(&report["failures"]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join("；"))));
                }
                checkpoint(context).await?;
                if let Some(context) = context {
                    context.activity("语言和标题回读验收", &format!("正在核对 {label} 的实际音轨和字幕语言及标题")).await;
                }
                let live = media_edit::details(state, id, "MediaSources,MediaStreams").await?;
                Ok::<_, ApiError>(mismatches(&expected, &live, &source_paths, &mappings))
            }.await;
            match readback {
                Ok(missing) if missing.is_empty() => {
                    if let Some(context) = context {
                        context.activity("语言和标题回读完成", &format!("{label} 的 {} 条音轨和字幕已通过 Emby 语言和标题回读验收", expected.len())).await;
                    }
                    Ok((false, expected.len()))
                }
                Ok(missing) => {
                    pending.extend(missing.into_iter().map(|mut entry| {
                        entry["reason"] = json!(format!("本地标题已保存，但 Emby 回读标题未一致：{}", string(&entry, "reason")));
                        annotate(entry)
                    }));
                    Err(ApiError::bad("Emby 回读标题未一致"))
                }
                Err(error) => Err(ApiError::bad(format!("本地标题已保存，恢复到 Emby 或回读验收失败：{}", error.1))),
            }
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
            }
        }
        result["pending"].as_array_mut().unwrap().extend(pending);
        checkpoint(context).await?;
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
