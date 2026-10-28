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
    time::Duration,
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
        if pending
            .iter()
            .any(|p| p["type"] == stream["Type"] && p["ordinal"] == ordinal)
        {
            continue;
        }
        expected.push(json!({"type":stream["Type"],"ordinal":ordinal,"language":stream["Language"],"codec":stream["Codec"],"title":stream["Title"]}));
    }
    expected
}
fn mismatches(expected: &[Value], item: &Value, paths: &[&str]) -> Vec<Value> {
    let sources = list(&item["MediaSources"]);
    let streams = sources
        .iter()
        .find(|s| s["Path"].as_str().is_some_and(|path| paths.contains(&path)))
        .or_else(|| {
            (sources.len() == 1 && string(&sources[0], "Path").is_empty()).then(|| &sources[0])
        })
        .and_then(|s| s["MediaStreams"].as_array())
        .or_else(|| {
            if sources.is_empty() {
                item["MediaStreams"].as_array()
            } else {
                None
            }
        });
    expected
        .iter()
        .filter(|entry| {
            let stream = streams.and_then(|s| {
                s.iter()
                    .filter(|v| v["Type"] == entry["type"])
                    .nth(entry["ordinal"].as_u64().unwrap() as usize - 1)
            });
            stream.is_none_or(|s| {
                s["Title"] != entry["title"]
                    || entry["codec"].as_str().is_some_and(|c| {
                        s["Codec"]
                            .as_str()
                            .is_none_or(|v| !v.eq_ignore_ascii_case(c))
                    })
            })
        })
        .cloned()
        .collect()
}
async fn checkpoint(context: Option<&JobContext>) -> Result<()> {
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    Ok(())
}

pub async fn fill(state: &AppState, ids: &[String], skip_chinese: bool) -> Value {
    fill_inner(state, ids, skip_chinese, None)
        .await
        .unwrap_or_else(|error| json!({"failed":1,"pending":[{"reason":error.1}]}))
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
    let mut result = json!({"scanned":0,"changed_items":0,"changed_streams":0,"verified_items":0,"verified_streams":0,"skipped":0,"failed":0,"pending":[]});
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
        .await
        .ok()
        .flatten()
        .unwrap_or(json!([]));
    let mut processed = HashSet::new();
    for id in ids.iter().collect::<BTreeSet<_>>() {
        checkpoint(context).await?;
        result["scanned"] = json!(result["scanned"].as_u64().unwrap() + 1);
        let mut pending = vec![];
        let mut changed = 0;
        let outcome = async {
            let item = media_edit::details(state, id, "Id,Name,Path,MediaSources,MediaStreams,PartCount,ExtraType,IndexNumber,ParentIndexNumber").await?;
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
                let (updated, report) = language::fill(&original, &mappings, skip_chinese);
                pending.extend(list(&report["pending"]).into_iter().map(|mut entry| {
                    entry["file"] = json!(Path::new(&target.path).file_name().unwrap_or_default().to_string_lossy());
                    annotate(entry)
                }));
                processed.insert(path.clone());
                let count = report["changed"].as_u64().unwrap_or(0);
                if count > 0 {
                    atomic(&path, &updated).await?;
                    changed = count;
                    if let Some(sha) = &target.sha
                        && let Err(error) = sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                            .bind(sha.to_uppercase()).bind(&updated).execute(&state.db).await {
                            tracing::warn!("[语言标题] Item {id} 本地 JSON 已保存，缓存同步失败：{error}");
                    }
                }
                expected = Some(expected_titles(&updated, &report));
                break;
            }
            let Some(expected) = expected else {
                pending.push(annotate(json!({"reason":"未找到本地媒体信息 JSON"})));
                return Ok((true, 0));
            };
            if expected.is_empty() { return Ok((false, 0)); }
            if changed == 0 && mismatches(&expected, &item, &source_paths).is_empty() { return Ok((false, expected.len())); }
            checkpoint(context).await?;
            let readback = crate::strm::with_settings(state, "preserve", || async {
                crate::media_save::refresh(state, &[string(&item, "Path")]).await?;
                let mut missing = expected.clone();
                for attempt in 0..3 {
                    let live = media_edit::details(state, id, "MediaSources,MediaStreams").await?;
                    missing = mismatches(&expected, &live, &source_paths);
                    if missing.is_empty() { break; }
                    if attempt < 2 { tokio::time::sleep(Duration::from_secs(1)).await; }
                }
                Ok(missing)
            }).await;
            match readback {
                Ok(missing) if missing.is_empty() => Ok((false, expected.len())),
                Ok(missing) => {
                    pending.extend(missing.into_iter().map(|mut entry| {
                        entry["reason"] = json!(format!("本地标题已保存，但 Emby 回读标题未一致（期望：{}），请重新读取后检查", string(&entry, "title")));
                        annotate(entry)
                    }));
                    Err(ApiError::bad("Emby 回读标题未一致"))
                }
                Err(error) => Err(ApiError::bad(format!("本地标题已保存，通知 Emby 重新读取或回读失败：{}", error.1))),
            }
        }.await;
        result["changed_items"] =
            json!(result["changed_items"].as_u64().unwrap() + u64::from(changed > 0));
        result["changed_streams"] = json!(result["changed_streams"].as_u64().unwrap() + changed);
        match outcome {
            Ok((skipped, verified)) => {
                result["skipped"] = json!(result["skipped"].as_u64().unwrap() + u64::from(skipped));
                result["verified_items"] =
                    json!(result["verified_items"].as_u64().unwrap() + u64::from(verified > 0));
                result["verified_streams"] =
                    json!(result["verified_streams"].as_u64().unwrap() + verified as u64);
            }
            Err(error) => {
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
    log_report(&result);
    Ok(result)
}
fn log_report(report: &Value) {
    for entry in list(&report["pending"]) {
        tracing::warn!(
            "[语言标题] Item {} {}#{}，语言 {}：{}",
            string(&entry, "item_id"),
            match entry["type"].as_str() {
                Some("Audio") => "音轨",
                Some("Subtitle") => "字幕",
                _ => "媒体流",
            },
            entry["ordinal"].as_u64().unwrap_or(0),
            string(&entry, "language"),
            string(&entry, "reason")
        );
    }
    tracing::info!(
        "[语言标题] 检查 {} 个当前入库媒体，本地填充 {} 条标题，Emby 回读确认 {} 条，失败 {} 个，待手动处理 {} 项",
        report["scanned"],
        report["changed_streams"],
        report["verified_streams"],
        report["failed"],
        list(&report["pending"]).len()
    );
}
