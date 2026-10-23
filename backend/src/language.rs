use crate::media_info;
use serde_json::{Value, json};
use std::sync::LazyLock;

static ISO: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/languages.json")).expect("bundled language data")
});
static CHINESE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)繁|正体|正體|简|簡|traditional\s+chinese|chinese\s+traditional|simplified\s+chinese|chinese\s+simplified|\b(?:cht|chs)\b|zh[-_](?:tw|hk|mo|hant|cn|sg|hans)").unwrap()
});
pub fn token(text: &str) -> String {
    text.trim().to_lowercase().replace('_', "-")
}
pub fn lookup(language: &str, mappings: &Value) -> Option<Value> {
    let key = token(language);
    let match_custom = |key: &str| {
        for item in mappings.as_array().into_iter().flatten() {
            let mut aliases = vec![
                item["value"].as_str().unwrap_or(""),
                item["label"].as_str().unwrap_or(""),
            ];
            if let Some(values) = item["aliases"].as_array() {
                aliases.extend(values.iter().filter_map(Value::as_str));
            } else if let Some(values) = item["aliases"].as_str() {
                aliases.extend(values.split(',').map(str::trim));
            }
            if aliases.iter().any(|v| token(v) == key) {
                return Some(item.clone());
            }
        }
        None
    };
    if let Some(entry) = match_custom(&key) {
        return Some(entry);
    }
    let key = if matches!(key.as_str(), "tai" | "tai languages") {
        "tha".into()
    } else {
        key
    };
    if let Some(entry) = match_custom(&key) {
        return Some(entry);
    }
    ISO.get(&key)
        .or_else(|| ISO.get(key.split('-').next().unwrap_or("")))
        .cloned()
}
pub fn fill(info: &Value, mappings: &Value, skip_chinese: bool) -> (Value, Value) {
    let mut output = info.clone();
    let mut report = json!({"changed":0,"audio":0,"subtitle":0,"existing_title":0,"unknown_language":0,"pending":[]});
    let Some(streams) = media_info::streams_mut(&mut output) else {
        return (output, report);
    };
    let (mut audio, mut subtitle) = (0, 0);
    for stream in streams {
        let kind = stream["Type"].as_str().unwrap_or("").to_string();
        let ordinal = match kind.as_str() {
            "Audio" => {
                audio += 1;
                audio
            }
            "Subtitle" => {
                subtitle += 1;
                subtitle
            }
            _ => continue,
        };
        let raw = stream["Language"].as_str().unwrap_or("").to_string();
        let key = token(&raw);
        let entry = lookup(&raw, mappings);
        let label = entry
            .as_ref()
            .and_then(|v| v["label"].as_str())
            .unwrap_or("");
        let chinese = ["chi", "zho", "zh", "cmn", "yue", "chs", "cht"].contains(&key.as_str())
            || ["国语", "粤语", "简体", "繁体", "中文"].contains(&label)
            || key.starts_with("zh-")
            || key.starts_with("yue-")
            || key.starts_with("cmn-");
        let reason = if skip_chinese
            && kind == "Subtitle"
            && (chinese || CHINESE.is_match(stream["Title"].as_str().unwrap_or("")))
        {
            Some("中文字幕需手动填充")
        } else if matches!(
            key.as_str(),
            "" | "und" | "unk" | "xx" | "zxx" | "unknown" | "undefined" | "none" | "null"
        ) || label.is_empty()
        {
            report["unknown_language"] = json!(report["unknown_language"].as_u64().unwrap() + 1);
            Some("语言为空或未知")
        } else {
            None
        };
        if let Some(reason) = reason {
            report["pending"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":kind,"ordinal":ordinal,"language":raw,"reason":reason}));
            continue;
        }
        let title = if kind == "Audio" && ["简体", "繁体", "中文"].contains(&label) {
            "国语".into()
        } else if kind == "Subtitle" && ["国语", "中文", "普通话"].contains(&label) {
            "中文".into()
        } else if kind == "Subtitle" && label == "粤语" {
            "繁体".into()
        } else if kind == "Subtitle" {
            label
                .strip_suffix('语')
                .map(|v| format!("{v}文"))
                .unwrap_or(label.to_string())
        } else {
            label.to_string()
        };
        if stream["Title"] == title {
            report["existing_title"] = json!(report["existing_title"].as_u64().unwrap() + 1);
            continue;
        }
        stream["Title"] = json!(title);
        for field in [
            "changed",
            if kind == "Audio" { "audio" } else { "subtitle" },
        ] {
            report[field] = json!(report[field].as_u64().unwrap() + 1);
        }
    }
    (output, report)
}
