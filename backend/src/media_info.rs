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
