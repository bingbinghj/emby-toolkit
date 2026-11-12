use crate::media_info;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::LazyLock};

static ISO: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/languages.json")).expect("bundled language data")
});
static CHINESE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)繁|正体|正體|简|簡|中文|国语|國語|粤|粵|\bchinese\b|\b(?:cht|chs)\b|zh[-_](?:tw|hk|mo|hant|cn|sg|hans)").unwrap()
});
static TECHNICAL_PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)^(?:hi[\s_-]*res(?:olution)?|l?pcm(?:[_-]s\d+(?:le|be)?)?|dts(?:[\s._-]*hd)?(?:[\s._-]*ma)?|truehd|e?ac[-_]?3|ddp?|(?:he[\s_-]*aac|aac[\s_-]*he)(?:[\s_-]*v2)?|aac|flac|opus|mp3|dolby(?:[\s._-]+(?:digital|plus|truehd|atmos))*|pgssub|pgs|subrip|srt|ass|ssa|\d+\.\d+(?:\s*ch)?|\d+[\s_-]*(?:kbps|khz|bit))(?:[\s._:;/|-]+|$)").unwrap()
});
static AAC_PROFILE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)(?:he[\s_-]*aac|aac[\s_-]*he)(?:[\s_-]*v2)?").unwrap()
});
static LANGUAGE_TAG: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b[a-z]{2,3}(?:[-_](?:[a-z]{4}|[a-z]{2}|[0-9]{3}))+\b").unwrap()
});
static NEUTRAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)原声|原生|原版|原音|原始|字幕|解说|解說|(?:^|\b)(?:sdh|original(?:\s+audio)?|audio(?:\s+track)?|subtitles?)(?:\b|$)").unwrap()
});
static COMMENTARY_TITLE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)(?:commentary|导评|導評)(?:\s*#?\s*(\d+))?").unwrap());
pub(crate) fn is_commentary_title(title: &str) -> bool {
    COMMENTARY_TITLE
        .find_iter(title)
        .any(|marker| boundary(title, marker.start(), marker.end(), marker.as_str()))
}
static REGION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\blatin[\s-]+america(?:n)?\b").unwrap());
static TAIWAN_DUB: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\p{Han}A-Za-z0-9]*(?:台配|台湾配音|臺灣配音)").unwrap());
static CANTONESE_SOURCE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)dvd|tvb").unwrap());
static DUB_SOURCE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\p{Han}]*(?:配音|译制|译製|配|译|譯)(?:版)?").unwrap());
static NON_DUB_PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| {
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let mut aliases =
        vec![r"cc|ad|forced|brazilian|latin[\s-]+america(?:n)?|拉美|拉丁美洲".to_string()];
    for entry in defaults["DEFAULT_COUNTRY_MAPPING"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for name in std::iter::once(entry["label"].as_str())
            .chain(
                entry["aliases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(Value::as_str),
            )
            .flatten()
        {
            aliases.push(regex::escape(name));
        }
    }
    regex::Regex::new(&format!("(?i){}", aliases.join("|"))).unwrap()
});
pub fn token(text: &str) -> String {
    text.trim().to_lowercase().replace('_', "-")
}

pub(crate) fn subtitle_script(title: &str) -> Option<&'static str> {
    let title = title
        .trim()
        .to_lowercase()
        .replace("簡體", "简体")
        .replace("繁體", "繁体")
        .replace("雙語", "双语");
    if title.starts_with("简繁") || title.starts_with("繁简") {
        return None;
    }
    let simplified = ["简体", "简中", "中简"].iter().any(|s| title.contains(s));
    let traditional = ["繁体", "繁中", "中繁"].iter().any(|s| title.contains(s));
    if simplified || traditional {
        return (simplified != traditional).then_some(if simplified { "chs" } else { "cht" });
    }
    let (language, rest) = if let Some(rest) = title.strip_prefix('简') {
        ("chs", rest)
    } else {
        ("cht", title.strip_prefix('繁')?)
    };
    let dual =
        rest.find("双语").is_some_and(|offset| offset > 0) && !rest.starts_with(['体', '中']);
    let compact = rest.chars().count() == 1 && "英日韩西德法葡俄泰越意".contains(rest);
    (dual || compact).then_some(language)
}

// Use Python's script-title rules only for a title edit. An explicit language
// edit takes priority, and untouched tracks retain their stored language.
pub fn normalize_subtitles_for_save(info: &mut Value, original: &Value) {
    normalize_subtitles_for_save_with_explicit(info, original, &[]);
}

pub(crate) fn normalize_subtitles_for_save_with_explicit(
    info: &mut Value,
    original: &Value,
    explicit_language_positions: &[usize],
) {
    let Some(original_streams) =
        media_info::container(original).and_then(|source| source["MediaStreams"].as_array())
    else {
        return;
    };
    let Some(streams) = media_info::streams_mut(info) else {
        return;
    };
    let subtitle_indices = streams
        .iter()
        .filter(|s| s["Type"] == "Subtitle")
        .map(|s| s["Index"].clone())
        .collect::<Vec<_>>();
    for (position, stream) in streams
        .iter_mut()
        .enumerate()
        .filter(|(_, stream)| stream["Type"] == "Subtitle")
    {
        if explicit_language_positions.contains(&position) {
            continue;
        }
        let mut matching = original_streams.iter().filter(|candidate| {
            candidate["Type"] == stream["Type"] && candidate["Index"] == stream["Index"]
        });
        let Some(before) = matching.next() else {
            continue;
        };
        if matching.next().is_some()
            || subtitle_indices
                .iter()
                .filter(|index| **index == stream["Index"])
                .count()
                != 1
            || before["Title"] == stream["Title"]
            || before["Language"] != stream["Language"]
        {
            continue;
        }
        let Some(language) = subtitle_script(stream["Title"].as_str().unwrap_or("")) else {
            continue;
        };
        let display = if language == "chs" {
            "Chinese Simplified"
        } else {
            "Chinese Traditional"
        };
        stream["Language"] = json!(language);
        if stream.get("DisplayLanguage").is_some() {
            stream["DisplayLanguage"] = json!(display);
        }
        if let Some(title) = stream["DisplayTitle"]
            .as_str()
            .and_then(|s| s.strip_prefix("Chinese"))
        {
            let suffix = title
                .strip_prefix(" Simplified")
                .or_else(|| title.strip_prefix(" Traditional"))
                .unwrap_or(title);
            stream["DisplayTitle"] = json!(format!("{display}{suffix}"));
        }
    }
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

pub(crate) fn code(entry: &Value) -> String {
    if ["国语", "普通话", "中文"].contains(&entry["label"].as_str().unwrap_or("")) {
        return "chi".into();
    }
    let value = token(entry["value"].as_str().unwrap_or(""));
    ISO.get(&value)
        .or_else(|| ISO.get(value.split('-').next().unwrap_or("")))
        .and_then(|entry| entry["value"].as_str())
        .map(token)
        .unwrap_or(value)
}
fn chinese(entry: &Value) -> bool {
    ["chi", "zho", "cmn", "yue", "chs", "cht"].contains(&code(entry).as_str())
}
fn title_for(kind: &str, entry: &Value) -> String {
    let label = entry["label"].as_str().unwrap_or("");
    if kind == "Audio" && ["简体", "繁体", "中文"].contains(&label) {
        "国语".into()
    } else if kind == "Subtitle" && ["国语", "中文", "普通话"].contains(&label) {
        "中文".into()
    } else if kind == "Subtitle" && label == "粤语" {
        "繁体".into()
    } else if kind == "Subtitle" {
        label
            .strip_suffix('语')
            .map(|s| format!("{s}文"))
            .unwrap_or(label.into())
    } else {
        label.into()
    }
}
fn boundary(text: &str, start: usize, end: usize, alias: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() && !('\u{4e00}'..='\u{9fff}').contains(&c);
    !(alias.starts_with(|c: char| c.is_ascii_alphanumeric())
        && text[..start].chars().next_back().is_some_and(word)
        || alias.ends_with(|c: char| c.is_ascii_alphanumeric())
            && text[end..].chars().next().is_some_and(word))
}
fn title_hits(title: &str, mappings: &Value) -> Vec<(usize, usize, Value)> {
    // ASCII folding keeps byte positions valid when preserving Chinese and source remarks.
    let text = REGION
        .replace_all(
            &title.to_ascii_lowercase(),
            |captures: &regex::Captures<'_>| " ".repeat(captures[0].len()),
        )
        .into_owned();
    let text = AAC_PROFILE
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            let marker = captures.get(0).unwrap();
            if boundary(&text, marker.start(), marker.end(), marker.as_str()) {
                " ".repeat(marker.as_str().len())
            } else {
                marker.as_str().to_string()
            }
        })
        .into_owned();
    let mut hits = vec![];
    let mut add = |alias: &str, entry: &Value| {
        let alias = alias.trim().to_ascii_lowercase();
        if alias.is_empty()
            || [
                "pcm", "lpcm", "dts", "aac", "ass", "ssa", "sdh", "cc", "ad", "ma", "hd", "bit",
                "khz", "kbps",
            ]
            .contains(&alias.as_str())
        {
            return;
        }
        for (start, _) in text.match_indices(&alias) {
            let end = start + alias.len();
            if boundary(&text, start, end, &alias) {
                let observed = &title[start..end];
                let explicit = alias.len() > 3
                    || !alias.chars().all(|c| c.is_ascii_alphabetic())
                    || observed.chars().all(|c| c.is_ascii_uppercase())
                    || title.trim().eq_ignore_ascii_case(&alias)
                    || start == 0 && alias.len() == 3 && alias == code(entry)
                    || title[..start].ends_with(['[', '(']) && title[end..].starts_with([']', ')']);
                hits.push((start, end, entry.clone(), explicit));
            }
        }
    };
    for entry in mappings.as_array().into_iter().flatten() {
        for field in ["value", "label"] {
            if let Some(alias) = entry[field].as_str() {
                add(alias, entry);
            }
        }
        if let Some(aliases) = entry["aliases"].as_array() {
            for alias in aliases.iter().filter_map(Value::as_str) {
                add(alias, entry);
            }
        } else if let Some(aliases) = entry["aliases"].as_str() {
            for alias in aliases.split(',') {
                add(alias, entry);
            }
        }
    }
    for (alias, entry) in ISO.as_object().unwrap() {
        add(alias, entry);
    }
    for tag in LANGUAGE_TAG.find_iter(&text) {
        if !TECHNICAL_PREFIX.is_match(tag.as_str())
            && let Some(entry) = lookup(tag.as_str(), mappings)
        {
            hits.push((tag.start(), tag.end(), entry, true));
        }
    }
    hits.sort_by_key(|(start, end, _, _)| std::cmp::Reverse(end - start));
    let mut selected: Vec<(usize, usize, Value, bool)> = vec![];
    for hit in hits {
        if selected
            .iter()
            .all(|(start, end, _, _)| hit.1 <= *start || hit.0 >= *end)
        {
            selected.push(hit);
        }
    }
    let explicit_codes = selected
        .iter()
        .filter(|hit| hit.3)
        .map(|hit| code(&hit.2))
        .collect::<BTreeSet<_>>();
    selected.retain(|hit| hit.3 || explicit_codes.contains(&code(&hit.2)));
    selected.sort_by_key(|hit| hit.0);
    selected
        .into_iter()
        .map(|(start, end, entry, _)| (start, end, entry))
        .collect()
}

fn ready_commentary_title(stream: &mut Value, report: &mut Value, kind: &str, title: Option<&str>) {
    let Some(title) = title else {
        return;
    };
    if stream["Title"] == title {
        report["existing_title"] = json!(report["existing_title"].as_u64().unwrap() + 1);
        return;
    }
    stream["Title"] = json!(title);
    for field in [
        "changed",
        "changed_titles",
        if kind == "Audio" { "audio" } else { "subtitle" },
    ] {
        report[field] = json!(report[field].as_u64().unwrap() + 1);
    }
}

fn display_title(cleaned: &str, hits: &[(usize, usize, Value)], label: &str) -> String {
    let mut title = String::new();
    let mut position = 0;
    for (start, end, _) in hits {
        title.push_str(&cleaned[position..*start]);
        title.push_str(label);
        position = *end;
    }
    title.push_str(&cleaned[position..]);
    title = COMMENTARY_TITLE
        .replace_all(&title, |captures: &regex::Captures<'_>| {
            let marker = captures.get(0).unwrap();
            if boundary(&title, marker.start(), marker.end(), marker.as_str()) {
                format!(
                    "导评{}",
                    captures.get(1).map_or("", |number| number.as_str())
                )
            } else {
                marker.as_str().to_string()
            }
        })
        .into_owned();
    title = NEUTRAL
        .replace_all(&title, "")
        .trim_matches(|c: char| c.is_whitespace() || ".:_-/".contains(c))
        .to_string();
    if !label.is_empty() && hits.len() > 1 {
        let duplicate =
            regex::Regex::new(&format!(r"(?:{}[\s/|+._-]*){{2,}}", regex::escape(label))).unwrap();
        title = duplicate.replace_all(&title, label).into_owned();
    }
    title
        .replace("（）", "")
        .replace("()", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn short_dubbing_title(cleaned: &str, mappings: &Value) -> Option<&'static str> {
    let source = cleaned
        .split_once('（')
        .filter(|(label, _)| lookup(label, mappings).is_some())
        .and_then(|(_, source)| source.strip_suffix('）'))
        .unwrap_or(cleaned);
    match source.trim().to_ascii_lowercase().as_str() {
        "台" => Some("台配国语"),
        "东和兴" => Some("东和兴国语"),
        "福建文艺" => Some("福建文艺国语"),
        "dvd粤" => Some("DVD粤语"),
        "tvb粤" => Some("TVB粤语"),
        _ => None,
    }
}

fn mandarin_source(cleaned: &str) -> Option<String> {
    TAIWAN_DUB
        .find(cleaned)
        .map(|source| {
            source
                .as_str()
                .replace("台湾配音", "台配")
                .replace("臺灣配音", "台配")
        })
        .or_else(|| {
            ["东和兴", "福建文艺"]
                .into_iter()
                .find(|source| {
                    cleaned
                        .strip_prefix(source)
                        .is_some_and(|title| title.starts_with("国语"))
                })
                .map(str::to_string)
        })
}
fn dubbing_title(
    cleaned: &str,
    hits: &[(usize, usize, Value)],
    entry: &Value,
    label: &str,
    fallback: String,
) -> String {
    let source = match code(entry).as_str() {
        "chi" => mandarin_source(cleaned),
        "yue" => hits.first().and_then(|(start, _, _)| {
            let prefix = &cleaned[..*start];
            CANTONESE_SOURCE
                .find_iter(prefix)
                .find(|word| boundary(prefix, word.start(), word.end(), word.as_str()))
                .map(|word| word.as_str().to_ascii_uppercase())
        }),
        _ => None,
    };
    let Some((start, _, _)) = hits.first() else {
        if let Some(source) = source {
            let rest = DUB_SOURCE.replace_all(cleaned, "");
            let rest = display_title(&rest, &[], "");
            return format!("{source}{label} {rest}").trim().to_string();
        }
        if DUB_SOURCE.is_match(cleaned) {
            let rest = DUB_SOURCE.replace_all(cleaned, "");
            let rest = display_title(&rest, &[], "");
            return format!("{label} {rest}").trim().to_string();
        }
        return fallback;
    };
    let end = hits.last().unwrap().1;
    let suffix = DUB_SOURCE.replace_all(&cleaned[end..], "");
    let clipped = format!("{}{}", &cleaned[*start..end], suffix);
    let local_hits = hits
        .iter()
        .map(|(begin, end, entry)| (begin - start, end - start, entry.clone()))
        .collect::<Vec<_>>();
    let mut title = display_title(&clipped, &local_hits, label);
    if let Some(source) = source {
        title = format!("{source}{title}");
    }
    let prefix = &cleaned[..*start];
    let retained = NON_DUB_PREFIX
        .find_iter(prefix)
        .filter(|word| {
            boundary(prefix, word.start(), word.end(), word.as_str())
                && (word.as_str().len() > 3
                    || !word.as_str().is_ascii()
                    || word.as_str().chars().all(|c| c.is_ascii_uppercase())
                    || ["cc", "ad"].contains(&word.as_str().to_ascii_lowercase().as_str()))
        })
        .map(|word| word.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    format!("{retained} {title}").trim().to_string()
}

pub fn fill(info: &Value, mappings: &Value, skip_chinese: bool) -> (Value, Value) {
    let mut output = info.clone();
    let mut report = json!({"changed":0,"changed_titles":0,"changed_languages":0,"audio":0,"subtitle":0,"existing_title":0,"unknown_language":0,"pending":[],"skipped_tracks":[],"language_changes":[]});
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
        let original_title = stream["Title"].as_str().unwrap_or("");
        let original_entry = lookup(&raw, mappings);
        let mut cleaned = original_title.trim().to_string();
        while TECHNICAL_PREFIX.is_match(&cleaned) {
            cleaned = TECHNICAL_PREFIX.replace(&cleaned, "").trim().to_string();
        }
        if kind == "Audio"
            && let Some(title) = short_dubbing_title(&cleaned, mappings)
        {
            cleaned = title.into();
        }
        let hits = title_hits(&cleaned, mappings);
        if skip_chinese
            && kind == "Subtitle"
            && (original_entry.as_ref().is_some_and(chinese)
                || CHINESE.is_match(original_title)
                || hits.iter().any(|(_, _, entry)| chinese(entry)))
        {
            report["skipped_tracks"].as_array_mut().unwrap().push(json!({
                "type":kind,"ordinal":ordinal,"language":raw,"reason":"中文字幕按策略跳过，保留原标题和语言"
            }));
            continue;
        }
        let commentary = is_commentary_title(&cleaned).then(|| display_title(&cleaned, &hits, ""));
        let ready_title = commentary.clone().or_else(|| {
            (kind == "Audio" && hits.is_empty())
                .then(|| mandarin_source(&cleaned))
                .flatten()
        });
        let codes = hits
            .iter()
            .map(|(_, _, entry)| code(entry))
            .collect::<BTreeSet<_>>();
        if codes.len() > 1 {
            ready_commentary_title(stream, &mut report, &kind, ready_title.as_deref());
            report["pending"].as_array_mut().unwrap().push(json!({
                "type":kind,"ordinal":ordinal,"language":raw,"reason":"原标题包含多种语言，需手动确认","title_ready":ready_title.is_some()
            }));
            continue;
        }
        let inferred = hits.first().map(|(_, _, entry)| entry.clone());
        let entry = inferred
            .as_ref()
            .or(original_entry.as_ref())
            .filter(|entry| {
                !entry["label"].as_str().unwrap_or("").is_empty()
                    && ![
                        "und",
                        "unk",
                        "xx",
                        "zxx",
                        "unknown",
                        "undefined",
                        "none",
                        "null",
                    ]
                    .contains(&code(entry).as_str())
            });
        let Some(entry) = entry else {
            ready_commentary_title(stream, &mut report, &kind, ready_title.as_deref());
            report["unknown_language"] = json!(report["unknown_language"].as_u64().unwrap() + 1);
            report["pending"].as_array_mut().unwrap().push(json!({
                "type":kind,"ordinal":ordinal,"language":raw,"reason":"语言为空或未知","title_ready":ready_title.is_some()
            }));
            continue;
        };
        let label = title_for(&kind, entry);
        let mut title = display_title(&cleaned, &hits, &label);
        if title.is_empty() {
            title = label.clone();
        } else if hits.is_empty() {
            title = format!("{label}（{title}）");
        }
        if let Some(commentary) = commentary {
            title = commentary;
        } else if kind == "Audio" {
            title = dubbing_title(&cleaned, &hits, entry, &label, title);
        }
        let title_changed = stream["Title"] != title;
        let language_changed = inferred.as_ref().is_some_and(|entry| {
            original_entry
                .as_ref()
                .is_none_or(|original| code(original) != code(entry))
        });
        if !title_changed {
            report["existing_title"] = json!(report["existing_title"].as_u64().unwrap() + 1);
        } else {
            stream["Title"] = json!(title);
            report["changed_titles"] = json!(report["changed_titles"].as_u64().unwrap() + 1);
        }
        if language_changed {
            let to = code(inferred.as_ref().unwrap());
            stream["Language"] = json!(to);
            report["changed_languages"] = json!(report["changed_languages"].as_u64().unwrap() + 1);
            report["language_changes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":kind,"ordinal":ordinal,"from":raw,"to":to}));
        }
        if !title_changed && !language_changed {
            continue;
        }
        for field in [
            "changed",
            if kind == "Audio" { "audio" } else { "subtitle" },
        ] {
            report[field] = json!(report[field].as_u64().unwrap() + 1);
        }
    }
    (output, report)
}
