use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

static RESOLUTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^A-Z0-9])(8K|4K|\d{3,4}P)(?:[^A-Z0-9]|$)").unwrap());
static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(ULTRAHD\s+BLURAY|UHD\s+BLURAY|BLURAY|WEB-DL|WEBDL|WEBRIP|HDTV|REMUX|DVD)")
        .unwrap()
});
static EFFECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Z0-9])(DOVI|DOLBY\s+VISION|HDR10\+|HDR10|HDR|HLG)(?:[^A-Z0-9]|$)")
        .unwrap()
});
static CODEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^A-Z0-9])(HEVC|H\.265|H265|X265|AVC|H\.264|H264|X264|AV1)(?:[^A-Z0-9]|$)",
    )
    .unwrap()
});
pub fn fields(filename: &str) -> Value {
    let mut out = json!({});
    if let Some(c) = RESOLUTION.captures(filename) {
        let token = c[1].to_lowercase();
        out["resolution_display"] = json!(match token.as_str() {
            "2160p" | "4k" => "4k",
            "4320p" | "8k" => "8k",
            _ => &token,
        });
    }
    if let Some(c) = SOURCE.captures(filename) {
        let token = c[1].to_lowercase();
        let quality = [
            ("remux", "Remux"),
            ("bluray", "BluRay"),
            ("blu-ray", "BluRay"),
            ("web-dl", "WEB-DL"),
            ("webdl", "WEB-DL"),
            ("webrip", "WEBrip"),
            ("hdtv", "HDTV"),
            ("dvd", "DVDrip"),
        ]
        .into_iter()
        .find(|(s, _)| token.contains(s));
        if let Some((_, display)) = quality {
            out["quality_display"] = json!(display);
        }
    }
    if let Some(c) = EFFECT.captures(filename) {
        let token = c[1]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_uppercase();
        out["effect_display"] = json!(if matches!(token.as_str(), "DOVI" | "DOLBY VISION") {
            "DoVi"
        } else {
            &token
        });
    }
    if let Some(c) = CODEC.captures(filename) {
        out["codec_display"] = json!(match c[1].to_uppercase().as_str() {
            "HEVC" | "H.265" | "H265" | "X265" => "HEVC",
            "AVC" | "H.264" | "H264" | "X264" => "H.264",
            _ => "AV1",
        });
    }
    out
}
pub fn version_detail(filename: &str) -> String {
    let value = filename.to_lowercase();
    let mut details = vec![];
    for (names, label) in [
        (&["剧场版", "院线版", "theatrical cut"][..], "剧场版（TC）"),
        (
            &[
                "导演剪辑版",
                "导演剪辑",
                "director's cut",
                "directors cut",
                "director cut",
            ][..],
            "导演剪辑版（DC）",
        ),
        (&["加长版", "extended cut"][..], "加长版（EC）"),
        (
            &[
                "未分级版",
                "未分级",
                "unrated cut",
                "unrated version",
                "unrated",
            ][..],
            "未分级版（UC/UNRATED）",
        ),
        (&["终极剪辑版", "ultimate cut"][..], "终极剪辑版（UC）"),
        (&["特别版"][..], "特别版"),
    ] {
        if names.iter().any(|s| value.contains(s)) {
            details.push(label);
        }
    }
    for token in value.split(|c: char| c.is_ascii() && !c.is_ascii_alphanumeric()) {
        let (label, prefixes): (&str, &[&str]) = match token {
            "tc" => ("剧场版（TC）", &["剧场版"]),
            "dc" => ("导演剪辑版（DC）", &["导演剪辑版"]),
            "ec" => ("加长版（EC）", &["加长版"]),
            "uc" => ("未分级版（UC）", &["未分级版", "终极剪辑版"]),
            "unrated" => ("未分级版（UNRATED）", &["未分级版"]),
            _ => continue,
        };
        if !details
            .iter()
            .any(|s| prefixes.iter().any(|prefix| s.starts_with(prefix)))
        {
            details.push(label);
        }
    }
    details.join("、")
}
