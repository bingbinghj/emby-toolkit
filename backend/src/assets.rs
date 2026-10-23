use crate::{media_edit::list, state::string};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::LazyLock,
};

static RULES: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/asset_rules.json")).expect("asset rules")
});
fn nonempty(value: &Value) -> bool {
    !value.is_null()
        && *value != 0
        && *value != ""
        && *value != false
        && value.as_array().is_none_or(|v| !v.is_empty())
}
fn aliases(value: &Value) -> Vec<String> {
    if let Some(value) = value.as_str() {
        value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        list(value)
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }
}
pub fn normalize(raw: &str, mappings: &Value) -> String {
    let raw = raw.to_lowercase().trim().to_string();
    for (code, words) in [
        ("tha", &["tai", "tai languages"][..]),
        (
            "chi",
            &[
                "chi",
                "zho",
                "zh",
                "guo",
                "guoyu",
                "cmn",
                "zh-cmn",
                "mandarin",
                "国语",
                "普通话",
                "中文",
            ][..],
        ),
        (
            "yue",
            &[
                "yue",
                "zh-yue",
                "yue-hk",
                "yue-hant-hk",
                "cantonese",
                "粤语",
                "粤配",
                "粤英双语",
                "港配",
                "粤语配音",
                "广东话",
                "粤文",
            ][..],
        ),
        (
            "chs",
            &[
                "chs",
                "zhs",
                "sc",
                "gb",
                "zh-cn",
                "zh-sg",
                "zh-hans",
                "simplified",
                "简体",
                "简中",
                "简体中文",
            ][..],
        ),
        (
            "cht",
            &[
                "cht",
                "zht",
                "tc",
                "big5",
                "zh-hk",
                "zh-tw",
                "zh-mo",
                "zh-hant",
                "traditional",
                "繁体",
                "繁中",
                "繁体中文",
            ][..],
        ),
    ] {
        if words.contains(&raw.as_str()) {
            return code.into();
        }
    }
    let mappings = if list(mappings).is_empty() {
        &RULES["defaults"]
    } else {
        mappings
    };
    for entry in list(mappings) {
        let words = aliases(&entry["aliases"]);
        if raw == string(&entry, "value").to_lowercase()
            || raw == string(&entry, "label").to_lowercase()
            || words.iter().any(|s| s.to_lowercase() == raw)
        {
            return words
                .iter()
                .find(|s| s.len() == 3 && s.chars().all(char::is_alphabetic))
                .cloned()
                .unwrap_or(string(&entry, "value"))
                .to_lowercase();
        }
    }
    raw
}
fn cache_source(value: &Value) -> Option<&Value> {
    match value {
        Value::Array(values) => values.iter().find_map(cache_source),
        Value::Object(_) => {
            if let Some(value) = value.get("MediaSourceInfo") {
                cache_source(value)
            } else if let Some(value) = value.get("MediaSources") {
                cache_source(value)
            } else if value["MediaStreams"].is_array() {
                Some(value)
            } else {
                None
            }
        }
        _ => None,
    }
}
fn word_matches(text: &str, keyword: &str) -> bool {
    let keyword = keyword.to_lowercase();
    if keyword.is_empty() {
        return false;
    }
    if keyword
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        return text.contains(&keyword);
    }
    text.match_indices(&keyword).any(|(start, _)| {
        let boundary = |c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit();
        text[..start].chars().next_back().is_none_or(boundary)
            && text[start + keyword.len()..]
                .chars()
                .next()
                .is_none_or(boundary)
    })
}
fn languages(streams: &[Value], kind: &str, mappings: &Value) -> BTreeSet<String> {
    let mut keywords = RULES["keywords"].as_object().unwrap().clone();
    for mapping in list(mappings) {
        if [
            "国语",
            "粤语",
            "简体",
            "繁体",
            "英语",
            "日语",
            "韩语",
            "无语言",
        ]
        .contains(&string(&mapping, "label").as_str())
        {
            continue;
        }
        let words = aliases(&mapping["aliases"]);
        let code = words
            .iter()
            .find(|s| s.len() == 3 && s.chars().all(char::is_alphabetic))
            .cloned()
            .unwrap_or(string(&mapping, "value"))
            .to_lowercase();
        if !code.is_empty() {
            keywords.insert(code.clone(), json!(words));
            keywords.insert(format!("sub_{code}"), json!(words));
        }
    }
    let mut result = BTreeSet::new();
    for stream in streams.iter().filter(|s| s["Type"] == kind) {
        let title = format!(
            "{} {}",
            string(stream, "Title"),
            string(stream, "DisplayTitle")
        )
        .to_lowercase();
        let mut found = BTreeSet::new();
        for (code, words) in &keywords {
            if code.starts_with("sub_") != (kind == "Subtitle") {
                continue;
            }
            if aliases(words).iter().any(|word| word_matches(&title, word)) {
                found.insert(code.strip_prefix("sub_").unwrap_or(code).to_string());
            }
        }
        let raw = string(stream, "Language").to_lowercase();
        if !raw.is_empty() {
            found.insert(normalize(&raw, mappings));
        }
        if kind == "Audio"
            && found.contains("chi")
            && found.contains("yue")
            && !["guo", "国", "mandarin"].iter().any(|s| title.contains(s))
        {
            found.remove("chi");
        }
        if kind == "Subtitle" {
            let original = string(stream, "Title").to_lowercase();
            let cantonese = ["粤语", "粵語", "广东话", "廣東話", "cantonese", "yue"]
                .iter()
                .any(|s| word_matches(&original, s));
            let traditional = ["繁", "traditional", "cht"]
                .iter()
                .any(|s| word_matches(&title, s));
            let simplified = ["简", "簡", "simplified", "chs"]
                .iter()
                .any(|s| word_matches(&title, s));
            let chosen = if cantonese {
                Some("yue")
            } else if traditional {
                Some("cht")
            } else if simplified {
                Some("chs")
            } else {
                None
            };
            if let Some(code) = chosen {
                for code in ["chi", "yue", "chs", "cht"] {
                    found.remove(code);
                }
                found.insert(code.into());
            } else if raw == "yue" || raw.starts_with("yue-") {
                found.remove("yue");
                found.insert("cht".into());
            }
        }
        result.extend(found);
    }
    result
}
fn effect(path: &str, video: &Value) -> &'static str {
    let subtype = string(video, "ExtendedVideoSubType").to_lowercase();
    for (names, result) in [
        (
            &["doviprofile81", "doviprofile8", "dvhe.08", "dvh1.08"][..],
            "DoVi_P8",
        ),
        (
            &["doviprofile76", "doviprofile7", "dvhe.07", "dvh1.07"][..],
            "DoVi_P7",
        ),
        (&["doviprofile5", "dvhe.05", "dvh1.05"][..], "DoVi_P5"),
    ] {
        if names.contains(&subtype.as_str()) {
            return result;
        }
    }
    let combined = [
        "ExtendedVideoSubType",
        "ExtendedVideoSubTypeDescription",
        "ExtendedVideoType",
        "VideoRange",
        "DisplayTitle",
        "Profile",
        "Codec",
        "ColorTransfer",
    ]
    .iter()
    .map(|k| string(video, k).to_lowercase())
    .collect::<Vec<_>>()
    .join(" ");
    if combined.contains("profile 8.1") || combined.contains("hdr10 compatible") {
        return "DoVi_P8";
    }
    if combined.contains("profile 7") {
        return "DoVi_P7";
    }
    if combined.contains("profile 5") {
        return "DoVi_P5";
    }
    let dv = ["dovi", "dolbyvision", "dolby vision"]
        .iter()
        .any(|s| combined.contains(s));
    let plus = combined.contains("hdr10+") || combined.contains("hdr10plus");
    let hdr = plus || combined.contains("hdr") || combined.contains("smpte2084");
    if dv && hdr {
        return "DoVi_P8";
    }
    if dv {
        return "DoVi";
    }
    if plus {
        return "HDR10+";
    }
    if hdr {
        return "HDR";
    }
    if (path.contains("dovi") || path.contains("dolbyvision") || path.contains("dv"))
        && path.contains("hdr")
    {
        return "DoVi_P8";
    }
    for (words, result) in [
        (
            &["dovi p7", "dovi.p7", "dv.p7", "profile 7", "profile7"][..],
            "DoVi_P7",
        ),
        (
            &["dovi p5", "dovi.p5", "dv.p5", "profile 5", "profile5"][..],
            "DoVi_P5",
        ),
    ] {
        if words.iter().any(|s| path.contains(s)) {
            return result;
        }
    }
    if path.contains("dovi") || path.contains("dolbyvision") {
        "DoVi"
    } else if path.contains("hdr10+") || path.contains("hdr10plus") {
        "HDR10+"
    } else if path.contains("hdr") {
        "HDR"
    } else {
        "SDR"
    }
}
fn display(item: &Value, streams: &[Value], mappings: &Value) -> Value {
    let path = string(item, "Path");
    let filename = Path::new(&path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let path = filename.to_lowercase();
    let video = streams
        .iter()
        .find(|s| s["Type"] == "Video")
        .unwrap_or(&Value::Null);
    let width = video["Width"].as_i64().unwrap_or(0);
    let height = video["Height"].as_i64().unwrap_or(0);
    let mut resolution = if width == 0 {
        "未知"
    } else if width >= 3800 || height >= 2100 {
        "4k"
    } else if width >= 1800 || height >= 1000 {
        "1080p"
    } else if width >= 1200 || height >= 700 {
        "720p"
    } else if width >= 700 || height >= 480 {
        "480p"
    } else {
        "未知"
    };
    if resolution == "未知" {
        for (token, label) in [
            ("2160p", "4k"),
            ("1080p", "1080p"),
            ("720p", "720p"),
            ("480p", "480p"),
        ] {
            if path.contains(token) {
                resolution = label;
                break;
            }
        }
    }
    let quality = [
        ("remux", "Remux"),
        ("bluray", "BluRay"),
        ("blu-ray", "BluRay"),
        ("web-dl", "WEB-DL"),
        ("webdl", "WEB-DL"),
        ("webrip", "WEBrip"),
        ("hdtv", "HDTV"),
        ("dvdrip", "DVDrip"),
    ]
    .iter()
    .find(|(token, _)| path.contains(token))
    .map(|(_, label)| *label)
    .unwrap_or("未知");
    let codecs = [
        ("hevc", "HEVC"),
        ("h265", "HEVC"),
        ("x265", "HEVC"),
        ("h264", "H.264"),
        ("avc", "H.264"),
        ("x264", "H.264"),
        ("vp9", "VP9"),
        ("av1", "AV1"),
    ];
    let raw = string(video, "Codec").to_lowercase();
    let codec = if raw.is_empty() {
        codecs
            .iter()
            .find(|(token, _)| path.contains(token))
            .map(|(_, label)| label.to_string())
            .unwrap_or("未知".into())
    } else {
        codecs
            .iter()
            .find(|(token, _)| *token == raw)
            .map(|(_, label)| label.to_string())
            .unwrap_or(raw.to_uppercase())
    };
    let audio = languages(streams, "Audio", mappings);
    let mut subtitle = languages(streams, "Subtitle", mappings);
    if !["chi", "yue", "chs", "cht"]
        .iter()
        .any(|s| subtitle.contains(*s))
        && streams
            .iter()
            .any(|s| s["Type"] == "Subtitle" && s["IsExternal"] == true)
    {
        subtitle.insert("chs".into());
    }
    let language_display = |codes: &BTreeSet<String>, kind: &str| {
        let mut values = codes
            .iter()
            .map(|code| {
                list(mappings)
                    .iter()
                    .find(|m| normalize(&string(m, "value"), mappings) == *code)
                    .map(|m| {
                        let label = string(m, "label");
                        if kind == "subtitle"
                            && !["国语", "粤语", "简体", "繁体", "英语", "日语", "韩语"]
                                .contains(&label.as_str())
                        {
                            label
                                .strip_suffix('语')
                                .map(|s| format!("{s}文"))
                                .unwrap_or(label)
                        } else if kind == "subtitle" {
                            RULES[kind][code].as_str().unwrap_or(&label).to_string()
                        } else {
                            label
                        }
                    })
                    .unwrap_or_else(|| RULES[kind][code].as_str().unwrap_or(code).to_string())
            })
            .collect::<Vec<_>>();
        values.sort();
        values.join(", ")
    };
    let mut audio_text = language_display(&audio, "audio");
    if audio_text.is_empty() {
        audio_text = [
            ("truehd", "TrueHD"),
            ("atmos", "Atmos"),
            ("dts-hd", "DTS-HD"),
            ("dts", "DTS"),
            ("ac3", "AC3"),
            ("eac3", "EAC3"),
            ("dd+", "Dolby Digital+"),
            ("aac", "AAC"),
            ("flac", "FLAC"),
        ]
        .iter()
        .filter(|(token, _)| path.contains(token))
        .map(|(_, label)| *label)
        .collect::<Vec<_>>()
        .join(" | ");
    }
    let subtitle_text = language_display(&subtitle, "subtitle");
    let mut groups = vec![];
    for entry in list(&RULES["groups"]) {
        let group = entry[0].as_str().unwrap_or("");
        if aliases(&entry[1]).iter().any(|pattern| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .is_ok_and(|r| r.is_match(filename))
        }) || filename.to_uppercase().contains(&group.to_uppercase())
        {
            groups.push(group.to_string());
            break;
        }
    }
    json!({"resolution_display":resolution,"quality_display":quality,"effect_display":effect(&path,video),"codec_display":codec,
        "audio_display":if audio_text.is_empty(){"无"}else{&audio_text},"subtitle_display":if subtitle_text.is_empty(){"无"}else{&subtitle_text},
        "audio_languages_raw":audio,"subtitle_languages_raw":subtitle,"release_group_raw":groups})
}
pub async fn parse(
    item: &Value,
    parents: &HashMap<String, String>,
    guid: &str,
    mappings: &Value,
) -> Value {
    let path = string(item, "Path");
    let local = Path::new(&path)
        .with_extension("")
        .to_string_lossy()
        .to_string()
        + "-mediainfo.json";
    let cached = tokio::fs::read(local)
        .await
        .ok()
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
        .unwrap_or(Value::Null);
    parse_with_cache(item, parents, guid, mappings, &cached)
}
pub fn parse_with_cache(
    item: &Value,
    parents: &HashMap<String, String>,
    guid: &str,
    mappings: &Value,
    cached: &Value,
) -> Value {
    let emby_source = item["MediaSources"]
        .as_array()
        .and_then(|v| v.first())
        .unwrap_or(&Value::Null);
    let native = if !list(&emby_source["MediaStreams"]).is_empty() {
        list(&emby_source["MediaStreams"])
    } else {
        list(&item["MediaStreams"])
    };
    let source = cache_source(cached).unwrap_or(emby_source);
    let mut streams = if cache_source(cached).is_some() {
        list(&source["MediaStreams"])
    } else {
        native.clone()
    };
    for subtitle in native
        .iter()
        .filter(|s| s["Type"] == "Subtitle" && s["IsExternal"] == true)
    {
        if !streams.iter().any(|s| {
            ["Index", "Codec", "DisplayTitle", "Title", "IsExternal"]
                .iter()
                .all(|key| s[*key] == subtitle[*key])
        }) {
            streams.push(subtitle.clone());
        }
    }
    let mut ancestors = BTreeSet::new();
    let mut current = string(item, "Id");
    while let Some(parent) = parents.get(&current) {
        if parent == "1" || !ancestors.insert(parent.clone()) {
            break;
        }
        if !guid.is_empty() && guid.to_lowercase() != "none" {
            ancestors.insert(format!("{guid}_{parent}"));
        }
        current = parent.clone();
    }
    if !parents.is_empty() && !guid.is_empty() && guid.to_lowercase() != "none" {
        ancestors.insert(guid.into());
    }
    let runtime = |v: &Value| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .filter(|v| *v != 0.0)
            .map(|v| (v / 600000000.0).round_ties_even() as i64)
    };
    let mut out = json!({"emby_item_id":item["Id"],"path":item["Path"].as_str().unwrap_or(""),
        "size_bytes":([&item["Size"],&emby_source["Size"],&source["Size"]].into_iter().find(|v|nonempty(v)).cloned().unwrap_or(Value::Null)),
        "container":if nonempty(&source["Container"]){&source["Container"]}else{&item["Container"]},
        "video_codec":null,"video_bitrate_mbps":null,"bit_depth":null,"frame_rate":null,"audio_tracks":[],"subtitles":[],
        "date_added_to_library":item["DateCreated"],"ancestor_ids":ancestors,
        "runtime_minutes":runtime(&source["RunTimeTicks"]).or_else(||runtime(&item["RunTimeTicks"]))});
    for stream in &streams {
        match stream["Type"].as_str() {
            Some("Video") => {
                for (source,target) in [("Codec","video_codec"),("Width","width"),("Height","height"),("BitDepth","bit_depth")] {out[target]=stream[source].clone();}
                out["frame_rate"]=if nonempty(&stream["AverageFrameRate"]){stream["AverageFrameRate"].clone()}else{stream["RealFrameRate"].clone()};
                if let Some(bitrate)=stream["BitRate"].as_f64().filter(|v|*v!=0.0) {out["video_bitrate_mbps"]=json!((bitrate/100000.0).round_ties_even()/10.0);}
            }
            Some("Audio") => out["audio_tracks"].as_array_mut().unwrap().push(json!({"language":stream["Language"],"codec":stream["Codec"],"channels":stream["Channels"],"display_title":stream["DisplayTitle"],"is_default":stream["IsDefault"]})),
            Some("Subtitle") => out["subtitles"].as_array_mut().unwrap().push(json!({"language":stream["Language"],"display_title":stream["DisplayTitle"],"is_forced":stream["IsForced"],"format":stream["Codec"]})),
            _=>{}
        }
    }
    if let Some(video) = streams.iter().find(|s| s["Type"] == "Video") {
        let title = string(video, "Title").to_lowercase();
        let language = if title.contains("简中")
            || title == "chs"
            || title.contains("简英")
            || title.contains("chs&eng")
        {
            Some("chs")
        } else if title.contains("繁中")
            || title == "cht"
            || title.contains("繁英")
            || title.contains("cht&eng")
        {
            Some("cht")
        } else {
            None
        };
        if let Some(language) = language {
            streams.push(json!({"Type":"Subtitle","Language":language,"Title":if language=="chs"{"简中"}else{"繁中"}}));
        }
    }
    out.as_object_mut().unwrap().extend(
        display(item, &streams, mappings)
            .as_object()
            .unwrap()
            .clone(),
    );
    out
}
