use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};

pub fn clean(raw: &str) -> String {
    let raw = raw.trim();
    let parsed = reqwest::Url::parse(raw).ok();
    let value = match parsed.as_ref() {
        Some(url) if matches!(url.scheme(), "http" | "https") => {
            let mut url = url.clone();
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Some(url) if url.scheme() == "file" => url.path().to_string(),
        _ => raw.into(),
    };
    percent_decode_str(&value)
        .decode_utf8_lossy()
        .replace('\\', "/")
}
pub fn extension(raw: &str) -> String {
    let parsed = reqwest::Url::parse(raw).ok();
    let value = parsed
        .as_ref()
        .filter(|v| matches!(v.scheme(), "http" | "https"))
        .map_or(raw, |v| v.path());
    let decoded = percent_decode_str(value).decode_utf8_lossy();
    let ext = Path::new(decoded.as_ref())
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if [
        "mkv", "mp4", "ts", "avi", "rmvb", "wmv", "mov", "m2ts", "flv", "mpg", "mpeg", "iso",
        "webm", "strm",
    ]
    .contains(&ext.as_str())
    {
        format!(".{ext}")
    } else {
        String::new()
    }
}
fn add(paths: &mut Vec<String>, value: String) {
    if !value.is_empty() && !paths.contains(&value) {
        paths.push(value);
    }
}
fn local(paths: &mut Vec<String>, raw: &str) {
    let value = clean(raw);
    if !value.starts_with("http://") && !value.starts_with("https://") {
        add(paths, value);
    }
}
pub async fn strm_target(path: &str) -> Option<String> {
    if !path.to_lowercase().ends_with(".strm") {
        return None;
    }
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .map(|s| s.trim().replace('\\', "/"))
        .filter(|s| !s.is_empty())
}
pub fn local_candidates(path: &str, target: &str, roots: &[&str]) -> Vec<String> {
    let mut candidates = vec![];
    let mut inputs = vec![path.to_string(), target.to_string()];
    let path = clean(path);
    let target = clean(target);
    if path.to_lowercase().ends_with(".strm") {
        let stem = Path::new(&path)
            .with_extension("")
            .to_string_lossy()
            .to_string();
        let ext = extension(&target);
        if !ext.is_empty() && ext != ".strm" {
            inputs.push(format!("{stem}{ext}"));
        } else if target.is_empty() {
            for ext in [
                ".avi", ".flv", ".iso", ".m2ts", ".mkv", ".mov", ".mp4", ".mpg", ".rmvb", ".ts",
                ".wmv",
            ] {
                inputs.push(format!("{stem}{ext}"));
            }
        }
    }
    for input in inputs {
        let cleaned = clean(&input);
        if cleaned.is_empty() || cleaned.starts_with("http://") || cleaned.starts_with("https://") {
            continue;
        }
        let mut variants = vec![cleaned.clone()];
        let parts: Vec<_> = cleaned.trim_matches('/').split('/').collect();
        for (index, part) in parts.iter().enumerate().take(parts.len().saturating_sub(1)) {
            if ["115open", "115", "strm", "媒体库"].contains(&part.trim().to_lowercase().as_str())
            {
                variants.push(parts[index + 1..].join("/"));
            }
        }
        for root in roots {
            let root = clean(root).trim_end_matches('/').to_string();
            if !root.is_empty()
                && let Some(suffix) = cleaned.strip_prefix(&format!("{root}/"))
            {
                variants.push(suffix.into());
            }
        }
        for value in variants {
            add(
                &mut candidates,
                value
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
    candidates
}
pub async fn info_candidates(
    path: &str,
    local_root: &str,
    cd2_root: &str,
    cache_path: &str,
) -> Vec<PathBuf> {
    let path = clean(path);
    let target = strm_target(&path).await.unwrap_or_default();
    let mut media = vec![];
    local(&mut media, &path);
    local(&mut media, &target);
    let mut paths = local_candidates(&path, &target, &[local_root, cd2_root]);
    paths.push(cache_path.into());
    for path in paths {
        let path = clean(&path);
        local(&mut media, &path);
        let windows_absolute =
            path.as_bytes().get(1) == Some(&b':') && path.as_bytes().get(2) == Some(&b'/');
        if !Path::new(&path).is_absolute() && !windows_absolute && !path.is_empty() {
            for root in [local_root, cd2_root].iter().filter(|s| !s.is_empty()) {
                local(&mut media, &Path::new(root).join(&path).to_string_lossy());
            }
        }
    }
    let mut result = vec![];
    for path in media {
        let candidates = if path.to_lowercase().ends_with("-mediainfo.json") {
            vec![path.clone()]
        } else {
            vec![
                format!("{path}-mediainfo.json"),
                format!(
                    "{}-mediainfo.json",
                    Path::new(&path).with_extension("").to_string_lossy()
                ),
            ]
        };
        for candidate in candidates {
            let candidate = PathBuf::from(candidate);
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
    }
    result
}
pub fn matches_container(info: &serde_json::Value, path: &str) -> bool {
    let Some(source) = crate::media_info::container(info) else {
        return true;
    };
    let container = source["Container"]
        .as_str()
        .or_else(|| source["container"].as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if container.is_empty() {
        return true;
    }
    let expected: &[&str] = match extension(path).as_str() {
        ".iso" => &["iso"],
        ".mkv" => &["mkv", "matroska"],
        ".webm" => &["webm", "matroska"],
        ".mp4" => &["mp4"],
        ".mov" => &["mov", "mp4", "quicktime"],
        ".ts" => &["ts", "mpegts", "mpeg-ts"],
        ".m2ts" => &["m2ts", "ts", "mpegts", "mpeg-ts"],
        ".avi" => &["avi"],
        ".wmv" => &["wmv", "asf"],
        ".flv" => &["flv"],
        ".mpg" | ".mpeg" => &["mpg", "mpeg", "mpegvideo"],
        ".rmvb" => &["rmvb", "rm", "realmedia"],
        _ => return true,
    };
    container
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .any(|s| expected.contains(&s))
}
