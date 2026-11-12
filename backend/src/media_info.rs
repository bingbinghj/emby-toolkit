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
// A linked version's JSON may lag behind edits that exist only in Emby.
pub(crate) fn preserve_live_streams(info: &mut Value, item: &Value, paths: &[&str]) -> Result<()> {
    let has_av = |streams: &Vec<Value>| {
        streams
            .iter()
            .any(|stream| matches!(stream["Type"].as_str(), Some("Video" | "Audio")))
    };
    let Some(streams) = item_streams(item, paths) else {
        let visible_av = item["MediaStreams"].as_array().is_some_and(has_av)
            || item["MediaSources"].as_array().is_some_and(|sources| {
                sources
                    .iter()
                    .any(|source| source["MediaStreams"].as_array().is_some_and(has_av))
            });
        return if visible_av {
            Err(ApiError::bad(
                "无法唯一确定当前版本的媒体流，未覆盖关联版本备份",
            ))
        } else {
            Ok(())
        };
    };
    if has_av(streams) {
        let saved =
            streams_mut(info).ok_or_else(|| ApiError::bad("关联版本备份缺少可恢复的媒体流"))?;
        *saved = streams.clone();
    }
    Ok(())
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

fn codec_identity(value: &Value) -> String {
    match identity_text(value).as_str() {
        "srt" | "subrip" => "subrip".into(),
        "pgs" | "pgssub" | "hdmv_pgs_subtitle" => "pgssub".into(),
        "vtt" | "webvtt" => "vtt".into(),
        "dvdsub" | "dvd_subtitle" => "dvdsub".into(),
        _ => identity_text(value),
    }
}
fn channel_identity(value: &Value) -> String {
    let number = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok());
    if let Some(number) = number.filter(|number| number.is_finite() && number.fract() == 0.0) {
        format!("{number:.0}")
    } else {
        identity_text(value)
    }
}
fn language_role(language: &Value, title: &Value, kind: &str) -> String {
    let raw = identity_text(language).replace('_', "-");
    match raw.as_str() {
        "chs" | "zh-hans" | "zh-cn" => "chs".into(),
        "cht" | "zh-hant" | "zh-tw" => "cht".into(),
        "chi" | "zho" | "zh" | "" | "und" => {
            if kind == "Subtitle"
                && let Some(script) = crate::language::subtitle_script(title.as_str().unwrap_or(""))
            {
                return script.into();
            }
            if matches!(raw.as_str(), "" | "und") {
                String::new()
            } else {
                "chi".into()
            }
        }
        "en" => "eng".into(),
        "ja" => "jpn".into(),
        "ko" => "kor".into(),
        _ => raw,
    }
}
fn compatible_roles(stream: &Value, identity: &Value) -> bool {
    let kind = identity["type"].as_str().unwrap_or("");
    let source = language_role(&identity["language"], &identity["title"], kind);
    let target = language_role(&stream["Language"], &stream["Title"], kind);
    if kind == "Video" && (source.is_empty() || target.is_empty()) {
        return true;
    }
    if source.is_empty() || target.is_empty() {
        return same_original_title(stream, identity)
            && !identity_text(&identity["codec"]).is_empty()
            && !identity_text(&stream["Codec"]).is_empty()
            && codec_identity(&identity["codec"]) == codec_identity(&stream["Codec"]);
    }
    source == target
        || (source == "chi" && matches!(target.as_str(), "chs" | "cht"))
        || (target == "chi" && matches!(source.as_str(), "chs" | "cht"))
}
fn codec_matches(stream: &Value, identity: &Value) -> bool {
    identity.get("codec").is_none_or(|value| {
        value.is_null()
            || identity_text(value).is_empty()
            || codec_identity(value) == codec_identity(&stream["Codec"])
    })
}
fn stable_role_matches(stream: &Value, identity: &Value) -> bool {
    (identity["type"] != "Audio"
        || crate::language::is_commentary_title(identity["title"].as_str().unwrap_or(""))
            == crate::language::is_commentary_title(stream["Title"].as_str().unwrap_or("")))
        && compatible_roles(stream, identity)
        && identity
            .get("channels")
            .filter(|value| !value.is_null())
            .is_none_or(|value| channel_identity(value) == channel_identity(&stream["Channels"]))
        && identity
            .get("is_external")
            .filter(|value| !value.is_null())
            .is_none_or(|value| {
                value.as_bool().unwrap_or(false) == stream["IsExternal"].as_bool().unwrap_or(false)
            })
}
fn same_original_title(stream: &Value, identity: &Value) -> bool {
    let title = identity_text(&identity["title"]);
    !title.is_empty() && title == identity_text(&stream["Title"])
}
fn track_description(streams: &[Value], index: usize) -> String {
    let stream = &streams[index];
    let ordinal = streams[..=index]
        .iter()
        .filter(|other| other["Type"] == stream["Type"])
        .count();
    format!(
        "第{ordinal}轨 Codec={} Channels={} Language={}（角色{}） Title={} IsExternal={}",
        stream["Codec"],
        stream["Channels"],
        stream["Language"],
        language_role(
            &stream["Language"],
            &stream["Title"],
            stream["Type"].as_str().unwrap_or("")
        ),
        stream["Title"],
        stream["IsExternal"]
    )
}
fn unique_track(
    candidates: &[usize],
    streams: &[Value],
    identity: &Value,
    ordinal: Option<usize>,
) -> Result<usize> {
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    let reason = if candidates.is_empty() {
        let observed = ordinal
            .map(|index| track_description(streams, index))
            .unwrap_or_else(|| "原同类序号不存在".into());
        format!(
            "没有相容的目标轨道：源 Codec={} Channels={} Language={} Title={} IsExternal={}；{observed}",
            identity["codec"],
            identity["channels"],
            identity["language"],
            identity["title"],
            identity["is_external"]
        )
    } else {
        format!(
            "存在 {} 条相容轨道，无法唯一匹配，未修改：{}",
            candidates.len(),
            candidates
                .iter()
                .take(4)
                .map(|index| track_description(streams, *index))
                .collect::<Vec<_>>()
                .join("；")
        )
    };
    Err(ApiError::bad(reason))
}
fn patch_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "Video" => &["Title"],
        "Audio" => &["Title", "Language", "IsDefault"],
        "Subtitle" => &["Title", "Language", "IsDefault", "IsForced"],
        _ => &[],
    }
}
fn resolve_patch(streams: &[Value], patch: &Value) -> Result<usize> {
    let identity = &patch["identity"];
    let kind = identity["type"].as_str().unwrap_or("");
    let fields = patch_fields(kind);
    if fields.is_empty()
        || patch["delete"] != true
            && !patch["changes"]
                .as_object()
                .is_some_and(|changes| changes.keys().any(|field| fields.contains(&field.as_str())))
    {
        return Err(ApiError::bad("没有可应用的字段"));
    }
    let ordinal = identity["ordinal"]
        .as_u64()
        .or_else(|| identity["ordinal"].as_str()?.parse().ok())
        .filter(|ordinal| *ordinal > 0)
        .ok_or_else(|| ApiError::bad("补丁缺少有效的类型内轨道序号"))?;
    let same_type = streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| stream["Type"] == kind)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let observed = same_type.get((ordinal - 1) as usize).copied();
    if patch["delete"] == true {
        if identity_text(&identity["codec"]).is_empty()
            || identity.get("language").is_none()
            || identity.get("title").is_none()
            || kind == "Subtitle" && !identity["is_external"].is_boolean()
        {
            return Err(ApiError::bad("删除补丁缺少完整的原轨道身份，未修改"));
        }
        if observed.is_none() {
            return Err(ApiError::bad("删除目标缺少对应的同类轨道，未修改"));
        }
        let candidates = same_type
            .into_iter()
            .filter(|index| identity_matches(&streams[*index], identity))
            .collect::<Vec<_>>();
        return unique_track(&candidates, streams, identity, observed);
    }
    if let Some(index) = observed
        && identity_matches(&streams[index], identity)
        && identity.get("language").is_some()
        && identity.get("title").is_some()
        && (compatible_roles(&streams[index], identity)
            || language_role(&identity["language"], &identity["title"], kind).is_empty()
                && language_role(&streams[index]["Language"], &streams[index]["Title"], kind)
                    .is_empty())
        && (kind != "Video"
            || same_type
                .iter()
                .filter(|other| identity_matches(&streams[**other], identity))
                .count()
                == 1)
    {
        return Ok(index);
    }
    let candidates = same_type
        .into_iter()
        .filter(|index| stable_role_matches(&streams[*index], identity))
        .collect::<Vec<_>>();
    if kind == "Video" && candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    let strong = candidates
        .iter()
        .copied()
        .filter(|index| {
            same_original_title(&streams[*index], identity)
                && (kind == "Video"
                    || !language_role(&identity["language"], &identity["title"], kind).is_empty()
                        && !language_role(
                            &streams[*index]["Language"],
                            &streams[*index]["Title"],
                            kind,
                        )
                        .is_empty())
        })
        .collect::<Vec<_>>();
    if !strong.is_empty() {
        let encoding = strong
            .iter()
            .copied()
            .filter(|index| codec_matches(&streams[*index], identity))
            .collect::<Vec<_>>();
        return unique_track(
            if encoding.is_empty() {
                &strong
            } else {
                &encoding
            },
            streams,
            identity,
            observed,
        );
    }
    let encoding = candidates
        .into_iter()
        .filter(|index| codec_matches(&streams[*index], identity))
        .collect::<Vec<_>>();
    let titled = encoding
        .iter()
        .copied()
        .filter(|index| same_original_title(&streams[*index], identity))
        .collect::<Vec<_>>();
    unique_track(
        if titled.is_empty() {
            &encoding
        } else {
            &titled
        },
        streams,
        identity,
        observed,
    )
}

/// Plan against the original target; each logical role may be edited only once.
pub fn apply_patches(info: &Value, patches: &Value) -> (Value, Value) {
    let mut output = info.clone();
    let mut report = serde_json::json!({"matched":0,"skipped":[],"explicit_language_positions":[]});
    let Some(streams) = streams_mut(&mut output) else {
        report["skipped"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"reason":"目标媒体信息缺少MediaStreams"}));
        return (output, report);
    };
    let plans = patches
        .as_array()
        .into_iter()
        .flatten()
        .map(|patch| (patch, resolve_patch(streams, patch)))
        .collect::<Vec<_>>();
    let mut counts = BTreeMap::<usize, usize>::new();
    for (_, result) in &plans {
        if let Ok(index) = result {
            *counts.entry(*index).or_default() += 1;
        }
    }
    let mut deleted = std::collections::BTreeSet::new();
    let mut explicit_languages = vec![];
    for (patch, result) in plans {
        let identity = &patch["identity"];
        let kind = identity["type"].as_str().unwrap_or("");
        let result = result.and_then(|index| {
            if counts[&index] == 1 {
                Ok(index)
            } else {
                Err(ApiError::bad("多个补丁竞争同一目标轨道，全部保持原值"))
            }
        });
        let index = match result {
            Ok(index) => index,
            Err(error) => {
                report["skipped"].as_array_mut().unwrap().push(serde_json::json!({"type":kind,"index":identity["index"],"ordinal":identity["ordinal"],"reason":error.1}));
                continue;
            }
        };
        if patch["delete"] == true {
            deleted.insert(index);
        } else {
            for (field, value) in patch["changes"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(field, _)| patch_fields(kind).contains(&field.as_str()))
            {
                streams[index][field] = value.clone();
                if field == "Language" {
                    explicit_languages.push(index);
                }
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
    report["explicit_language_positions"] = serde_json::json!(
        explicit_languages
            .iter()
            .map(|position| position - deleted.range(..*position).count())
            .collect::<Vec<_>>()
    );
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
