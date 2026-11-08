use crate::error::{ApiError, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn object(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(_) => Some(value),
        Value::Array(items) => items.iter().find(|v| v.is_object()),
        _ => None,
    }
}
pub fn container(info: &Value) -> Option<&Value> {
    let root = object(info)?;
    for key in ["MediaSourceInfo", "MediaSources"] {
        if let Some(source) = object(&root[key]) {
            return Some(source);
        }
    }
    root["MediaStreams"].is_array().then_some(root)
}
pub(crate) fn restore_json(info: &Value) -> Result<Value> {
    let mut entries = info
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![info.clone()]);
    if entries.is_empty() {
        return Err(ApiError::bad("媒体信息恢复备份为空，未清理 Emby 数据"));
    }
    for entry in &mut entries {
        if !entry["MediaSourceInfo"].is_object()
            && entry["MediaSources"]
                .as_array()
                .is_some_and(|sources| sources.len() > 1)
        {
            return Err(ApiError::bad(
                "媒体信息包含无法确定身份的多个媒体源，未清理 Emby 数据",
            ));
        }
        let mut source = container(entry)
            .cloned()
            .ok_or_else(|| ApiError::bad("媒体信息缺少可恢复的媒体源，未清理 Emby 数据"))?;
        let ticks = crate::identity::size(&source["RunTimeTicks"])
            .filter(|ticks| *ticks <= i64::MAX as u64)
            .ok_or_else(|| {
                ApiError::bad("媒体信息备份缺少有效时长，无法供神医恢复，未清理 Emby 数据")
            })?;
        source["RunTimeTicks"] = serde_json::json!(ticks);
        entry["MediaSourceInfo"] = source;
    }
    Ok(Value::Array(entries))
}
pub(crate) fn has_media_streams(item: &Value, paths: &[&str]) -> bool {
    item_streams(item, paths).is_some_and(|streams| {
        streams
            .iter()
            .any(|stream| matches!(stream["Type"].as_str(), Some("Video" | "Audio")))
    })
}
pub(crate) fn readback_summary(item: &Value) -> String {
    let counts = |streams: Option<&Vec<Value>>| {
        let count = |kind| {
            streams.map_or(0, |streams| {
                streams.iter().filter(|s| s["Type"] == kind).count()
            })
        };
        format!(
            "视频 {}，音轨 {}，字幕 {}",
            count("Video"),
            count("Audio"),
            count("Subtitle")
        )
    };
    let sources = item["MediaSources"]
        .as_array()
        .map(|sources| {
            sources
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    format!(
                        "源#{}（{}）",
                        index + 1,
                        counts(source["MediaStreams"].as_array())
                    )
                })
                .collect::<Vec<_>>()
                .join("；")
        })
        .unwrap_or_else(|| "媒体源未返回".into());
    let chapters = item["Chapters"]
        .as_array()
        .map(|chapters| format!("{} 个", chapters.len()))
        .unwrap_or_else(|| "未返回或无效".into());
    format!(
        "顶层流：{}；{sources}；章节：{chapters}",
        counts(item["MediaStreams"].as_array())
    )
}
pub(crate) fn recovery_chapters(info: &Value, item: &Value, paths: &[&str]) -> Result<Value> {
    if let Ok(chapters) = clean_chapters(&item["Chapters"])
        && !chapters.as_array().unwrap().is_empty()
    {
        return Ok(chapters);
    }
    if !has_media_streams(item, paths) {
        // Empty media info after a failed clear/restore is not evidence that the
        // user's original chapters were removed. Keep the verified JSON backup.
        return clean_chapters(
            &object(info).ok_or_else(|| ApiError::bad("媒体信息备份无效"))?["Chapters"],
        );
    }
    if item["Type"] == "Audio" && item["Chapters"].is_null() {
        return Ok(serde_json::json!([]));
    }
    clean_chapters(&item["Chapters"])
}
pub(crate) fn item_streams<'a>(item: &'a Value, paths: &[&str]) -> Option<&'a Vec<Value>> {
    let sources = item["MediaSources"].as_array().filter(|v| !v.is_empty());
    if let Some(sources) = sources {
        let owner_matches = |source: &Value| {
            if let Some(owner) = source["ItemId"].as_str().filter(|owner| !owner.is_empty()) {
                return item["Id"] == owner;
            }
            (source["ItemId"].is_null() || source["ItemId"] == "")
                && source["Id"]
                    .as_str()
                    .and_then(|source_id| source_id.strip_prefix("mediasource_"))
                    .is_none_or(|owner| item["Id"] == owner)
        };
        let select = |normalized: bool| {
            let matches = sources
                .iter()
                .filter(|source| {
                    if !owner_matches(source) {
                        return false;
                    }
                    source["Path"].as_str().is_some_and(|path| {
                        !path.is_empty()
                            && paths.iter().any(|target| {
                                !target.is_empty()
                                    && if normalized {
                                        let http = path.starts_with("http://")
                                            || path.starts_with("https://")
                                            || target.starts_with("http://")
                                            || target.starts_with("https://");
                                        (!http
                                            || item["Id"]
                                                .as_str()
                                                .is_some_and(|id| source["ItemId"] == id))
                                            && crate::media_paths::clean(path)
                                                == crate::media_paths::clean(target)
                                    } else {
                                        path == *target
                                    }
                            })
                    })
                })
                .collect::<Vec<_>>();
            let mut owned = matches
                .iter()
                .copied()
                .filter(|source| item["Id"].as_str().is_some_and(|id| source["ItemId"] == id));
            let owner = owned.next();
            if owned.next().is_some() {
                return None;
            }
            owner.or_else(|| (matches.len() == 1).then(|| matches[0]))
        };
        // Query parameters can identify different versions. Prefer the exact URL;
        // accept a normalized alias only when its target version is unambiguous.
        let source = select(false).or_else(|| select(true)).or_else(|| {
            (sources.len() == 1 && sources[0]["Path"].as_str().is_none_or(str::is_empty))
                .then(|| &sources[0])
                .filter(|source| owner_matches(source))
        })?;
        // The matched source's cache may lag behind the requested item's streams.
        source["MediaStreams"]
            .as_array()
            .filter(|streams| !streams.is_empty())
            .or_else(|| item["MediaStreams"].as_array())
            .or_else(|| source["MediaStreams"].as_array())
    } else {
        item["MediaStreams"].as_array()
    }
}

// MediaSource.Id identifies a source, while ItemId identifies its native item.
// Prefer an unambiguous native owner and never borrow another version's source.
pub(crate) fn item_source<'a>(item: &'a Value, paths: &[&str]) -> Option<&'a Value> {
    let id = item["Id"].as_str().filter(|id| !id.is_empty())?;
    let sources = item["MediaSources"].as_array()?;
    let matches = sources
        .iter()
        .filter(|source| {
            let owner = source["ItemId"].as_str().filter(|owner| !owner.is_empty());
            if let Some(owner) = owner {
                return owner == id;
            }
            (source["ItemId"].is_null() || source["ItemId"] == "")
                && source["Id"].as_str().is_some_and(|source_id| {
                    source_id.strip_prefix("mediasource_").unwrap_or(source_id) == id
                })
        })
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Some(matches[0]);
    }
    let candidates = if matches.is_empty() {
        sources.iter().collect::<Vec<_>>()
    } else {
        matches
    };
    let mut matched = candidates.into_iter().filter(|source| {
        (source["ItemId"] == id
            || ((source["ItemId"].is_null() || source["ItemId"] == "")
                && source["Id"]
                    .as_str()
                    .and_then(|source_id| source_id.strip_prefix("mediasource_"))
                    .is_none_or(|owner| owner == id)))
            && source["Path"].as_str().is_some_and(|path| {
                !path.is_empty()
                    && paths
                        .iter()
                        .any(|target| !target.is_empty() && path == *target)
            })
    });
    let source = matched.next()?;
    matched.next().is_none().then_some(source)
}
pub(crate) fn stream_differences(info: &Value, item: &Value, paths: &[&str]) -> Vec<String> {
    let Some(expected) = container(info).and_then(|source| source["MediaStreams"].as_array())
    else {
        return vec!["保存数据缺少媒体流".into()];
    };
    let Some(actual) = item_streams(item, paths) else {
        return vec!["Emby 未返回目标版本的媒体流".into()];
    };
    let mut differences = vec![];
    for (kind, label) in [("Video", "视频"), ("Audio", "音轨"), ("Subtitle", "字幕")] {
        let expected = expected
            .iter()
            .filter(|s| s["Type"] == kind)
            .collect::<Vec<_>>();
        let actual = actual
            .iter()
            .filter(|s| s["Type"] == kind)
            .collect::<Vec<_>>();
        if expected.len() != actual.len() {
            differences.push(format!(
                "{label}数量不一致（期望 {}，实际 {}）",
                expected.len(),
                actual.len()
            ));
        }
        for (index, (expected, actual)) in expected.iter().zip(&actual).enumerate() {
            for (field, name) in [
                ("Title", "标题"),
                ("Codec", "编码"),
                ("Language", "语言"),
                ("Channels", "声道数"),
                ("IsDefault", "默认标记"),
                ("IsForced", "强制标记"),
            ] {
                if field == "Language" && expected.get(field).is_none() {
                    continue;
                }
                let value = &expected[field];
                if value.is_null() && !matches!(field, "Title" | "Language") {
                    continue;
                }
                let matches = match field {
                    "Title" => value.as_str().unwrap_or("") == actual[field].as_str().unwrap_or(""),
                    "Codec" | "Language" => value
                        .as_str()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(actual[field].as_str().unwrap_or("")),
                    "IsDefault" | "IsForced" => {
                        value.as_bool().unwrap_or(false) == actual[field].as_bool().unwrap_or(false)
                    }
                    _ => value == &actual[field],
                };
                if !matches {
                    differences.push(format!(
                        "{label}#{} {name}不一致（期望 {value}，实际 {}）",
                        index + 1,
                        actual[field]
                    ));
                }
            }
        }
    }
    differences
}
pub fn streams_mut(info: &mut Value) -> Option<&mut Vec<Value>> {
    let root = if info.is_array() {
        info.as_array_mut()?.iter_mut().find(|v| v.is_object())?
    } else {
        info
    };
    let source_key = ["MediaSourceInfo", "MediaSources"]
        .into_iter()
        .find(|key| root[*key].is_object() || root[*key].is_array());
    let source = if let Some(key) = source_key {
        let source = root.get_mut(key)?;
        if source.is_array() {
            source.as_array_mut()?.iter_mut().find(|v| v.is_object())?
        } else {
            source
        }
    } else {
        root
    };
    source.get_mut("MediaStreams")?.as_array_mut()
}
pub fn recoverable(info: &Value) -> bool {
    let Some(source) = container(info) else {
        return false;
    };
    if source.get("ItemId").is_some_and(|v| !v.is_null())
        || source["Id"]
            .as_str()
            .is_some_and(|v| v.starts_with("mediasource_"))
    {
        return false;
    }
    source["MediaStreams"].as_array().is_some_and(|streams| {
        streams
            .iter()
            .any(|s| matches!(s["Type"].as_str(), Some("Audio" | "Video" | "Subtitle")))
    })
}
pub fn preserve_chapters(info: &mut Value, chapters: &Value) -> Result<()> {
    let chapters = clean_chapters(chapters)?;
    match info {
        Value::Array(items) => {
            for entry in items {
                if !entry.is_object() {
                    return Err(ApiError::bad("无效媒体信息"));
                }
                entry["Chapters"] = chapters.clone();
            }
        }
        Value::Object(_) => info["Chapters"] = chapters,
        _ => return Err(ApiError::bad("无效媒体信息")),
    }
    Ok(())
}
pub type ChapterSignature = BTreeMap<(String, u64, String), usize>;
pub fn clean_chapters(chapters: &Value) -> Result<Value> {
    signature(chapters)?;
    Ok(Value::Array(
        chapters
            .as_array()
            .unwrap()
            .iter()
            .map(|chapter| {
                let mut output = serde_json::Map::new();
                for key in ["Name", "StartPositionTicks", "MarkerType"] {
                    if let Some(value) = chapter.get(key) {
                        output.insert(key.into(), value.clone());
                    }
                }
                Value::Object(output)
            })
            .collect(),
    ))
}
pub fn signature(chapters: &Value) -> Result<ChapterSignature> {
    let chapters = chapters
        .as_array()
        .ok_or_else(|| ApiError::bad("章节数据缺失或无效"))?;
    let mut result = BTreeMap::new();
    for chapter in chapters {
        for key in ["Name", "MarkerType"] {
            if chapter
                .get(key)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(ApiError::bad("章节名称或标记类型无效"));
            }
        }
        let ticks = chapter["StartPositionTicks"]
            .as_u64()
            .ok_or_else(|| ApiError::bad("章节时间点无效"))?;
        let key = (
            chapter["Name"].as_str().unwrap_or("").to_string(),
            ticks,
            chapter["MarkerType"]
                .as_str()
                .unwrap_or("Chapter")
                .to_string(),
        );
        *result.entry(key).or_default() += 1;
    }
    Ok(result)
}

fn identity_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Null => String::new(),
        other => other.to_string().to_lowercase(),
    }
}
fn identity_matches(stream: &Value, identity: &Value) -> bool {
    for (source, target) in [
        ("codec", "Codec"),
        ("channels", "Channels"),
        ("language", "Language"),
        ("title", "Title"),
    ] {
        if let Some(value) = identity.get(source) {
            let optional = matches!(source, "codec" | "channels");
            if optional && (value.is_null() || source == "codec" && identity_text(value).is_empty())
            {
                continue;
            }
            if identity_text(value) != identity_text(&stream[target]) {
                return false;
            }
        }
    }
    identity
        .get("is_external")
        .filter(|v| !v.is_null())
        .is_none_or(|v| {
            v.as_bool().unwrap_or(false) == stream["IsExternal"].as_bool().unwrap_or(false)
        })
}

/// Match by ordinal and observed identity; never copy another episode's container or size.
pub fn apply_patches(info: &Value, patches: &Value) -> (Value, Value) {
    let mut output = info.clone();
    let mut report = serde_json::json!({"matched":0,"skipped":[]});
    let Some(streams) = streams_mut(&mut output) else {
        report["skipped"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"reason":"目标媒体信息缺少MediaStreams"}));
        return (output, report);
    };
    let mut used = std::collections::HashSet::new();
    let mut deleted = std::collections::BTreeSet::new();
    for patch in patches.as_array().into_iter().flatten() {
        let identity = &patch["identity"];
        let kind = identity["type"].as_str().unwrap_or("");
        let allowed: &[&str] = match kind {
            "Video" => &["Title"],
            "Audio" => &["Title", "Language", "IsDefault"],
            "Subtitle" => &["Title", "Language", "IsDefault", "IsForced"],
            _ => &[],
        };
        let changes = patch["changes"]
            .as_object()
            .map(|v| {
                v.iter()
                    .filter(|(k, _)| allowed.contains(&k.as_str()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let ordinal = identity["ordinal"]
            .as_u64()
            .or_else(|| identity["ordinal"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0);
        let index = ordinal.checked_sub(1).and_then(|i| {
            streams
                .iter()
                .enumerate()
                .filter(|(_, s)| s["Type"] == kind)
                .nth(i as usize)
                .map(|(i, _)| i)
        });
        let reason = if allowed.is_empty() || changes.is_empty() && patch["delete"] != true {
            Some("没有可应用的字段")
        } else if ordinal == 0 {
            Some("补丁缺少有效的类型内轨道序号")
        } else {
            match index {
                None => Some("缺少对应的同类轨道"),
                Some(index) if used.contains(&index) => Some("同一目标轨道被重复匹配"),
                Some(index) if !identity_matches(&streams[index], identity) => {
                    Some("同类轨道身份不一致")
                }
                Some(_) => None,
            }
        };
        if let Some(reason) = reason {
            report["skipped"].as_array_mut().unwrap().push(serde_json::json!({"type":kind,"index":identity["index"],"ordinal":ordinal,"reason":reason}));
            continue;
        }
        let index = index.unwrap();
        used.insert(index);
        if patch["delete"] == true {
            deleted.insert(index);
        } else {
            for (field, value) in changes {
                streams[index][field] = value.clone();
            }
            if patch["changes"]["IsDefault"] == true && matches!(kind, "Audio" | "Subtitle") {
                for (other, stream) in streams.iter_mut().enumerate() {
                    if other != index && stream["Type"] == kind {
                        stream["IsDefault"] = Value::Bool(false);
                    }
                }
            }
        }
        report["matched"] = serde_json::json!(report["matched"].as_u64().unwrap() + 1);
    }
    for index in deleted.into_iter().rev() {
        streams.remove(index);
    }
    (output, report)
}
pub fn chapters_restored(expected: &Value, actual: &Value) -> bool {
    let (Ok(expected), Ok(actual)) = (signature(expected), signature(actual)) else {
        return false;
    };
    expected
        .iter()
        .all(|(key, count)| actual.get(key).unwrap_or(&0) >= count)
}

#[cfg(test)]
mod tests;
