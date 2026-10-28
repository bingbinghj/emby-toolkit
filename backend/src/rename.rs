use crate::{
    error::{ApiError, Result},
    media_paths,
    state::string,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn safe_path(raw: &str) -> Result<String> {
    let raw = raw.trim().replace('\\', "/");
    if raw.is_empty()
        || raw.starts_with('/')
        || raw.as_bytes().get(1) == Some(&b':')
        || raw.chars().any(|c| c.is_control())
    {
        return Err(ApiError::bad("命名必须是有效相对路径"));
    }
    let mut parts = vec![];
    for part in raw.split('/').filter(|s| !s.is_empty()) {
        if [".", ".."].contains(&part) || part.ends_with([' ', '.']) {
            return Err(ApiError::bad("命名不能访问上级目录或以空格句点结尾"));
        }
        let mut part = part
            .chars()
            .map(|c| match c {
                '<' => '＜',
                '>' => '＞',
                ':' => '：',
                '"' => '＂',
                '|' => '｜',
                '?' => '？',
                '*' => '＊',
                _ => c,
            })
            .collect::<String>();
        let base = part.split('.').next().unwrap_or("").to_uppercase();
        if ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
            || (base.is_ascii()
                && base.len() == 4
                && ["COM", "LPT"].contains(&&base[..3])
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            part.insert(0, '_');
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}
pub fn extension(raw: &str, ext: &str) -> Result<String> {
    let safe = safe_path(raw)?;
    if ext.is_empty() {
        return Ok(safe);
    }
    let path = Path::new(&safe);
    let name = path.file_name().unwrap().to_string_lossy();
    let suffix = media_paths::extension(&name);
    let stem = if suffix.is_empty() {
        name.as_ref()
    } else {
        &name[..name.len() - suffix.len()]
    };
    if stem.is_empty() {
        return Err(ApiError::bad("目标文件名不能为空"));
    }
    Ok(path
        .with_file_name(format!("{stem}{}", ext.to_lowercase()))
        .to_string_lossy()
        .into())
}
pub fn render(template: &str, fields: &Value, ext: &str) -> Result<String> {
    if template.trim().is_empty() {
        return Err(ApiError::bad("重命名格式不能为空"));
    }
    let mut environment = minijinja::Environment::new();
    environment.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    minijinja_contrib::add_to_environment(&mut environment);
    let rendered = environment
        .render_str(template, minijinja::value::Serde(fields))
        .map_err(|e| ApiError::bad(format!("Jinja2格式错误: {e}")))?;
    extension(&rendered, ext)
}
fn captures(pattern: &str, text: &str, index: usize) -> Option<String> {
    fancy_regex::Regex::new(pattern)
        .unwrap()
        .captures(text)
        .ok()
        .flatten()
        .map(|c| c[index].to_string())
}
pub fn pt(raw: &str) -> Value {
    let ext = media_paths::extension(raw);
    let stem = if ext.is_empty() {
        raw
    } else {
        &raw[..raw.len() - ext.len()]
    };
    let mut out = json!({});
    let years = fancy_regex::Regex::new(r"(?<!\d)(19\d{2}|20\d{2})(?!\d)")
        .unwrap()
        .find_iter(stem)
        .filter_map(|m| m.ok().map(|m| m.as_str()))
        .collect::<Vec<_>>();
    if let Some(year) = years.last() {
        out["year"] = json!(year);
    }
    if let Some(token) = captures(
        r"(?i)(S\d{1,2}E\d{1,4}(?:[-_]S?\d{1,2}E?\d{1,4})?)",
        stem,
        1,
    ) {
        let token = token.to_uppercase().replace('_', "");
        out["season"] = json!(captures(r"S(\d{1,2})", &token, 1));
        out["episode"] = json!(captures(r"E(\d{1,4})", &token, 1));
        out["season_episode"] = json!(token);
    }
    if let Some(token) = captures(r"(?i)(?<![A-Z0-9])(8K|4K|\d{3,4}P)(?![A-Z0-9])", stem, 1) {
        out["videoFormat"] = json!(match token.to_uppercase().as_str() {
            "4K" => "2160p".into(),
            "8K" => "4320p".into(),
            _ => token.to_lowercase(),
        });
    }
    if let Some(source) = captures(
        r"(?i)(ULTRAHD\s+BLURAY|UHD\s+BLURAY|BLURAY|WEB-DL|WEBDL|WEBRIP|HDTV|REMUX|DVD)",
        stem,
        1,
    ) {
        let source = source.split_whitespace().collect::<Vec<_>>().join(" ");
        let web = ["WEB-DL", "WEBDL"].contains(&source.to_uppercase().as_str());
        out["edition"] = json!(source);
        out["resourceType"] = json!(if web { "WEB-DL" } else { &source });
        if web {
            out["webSource"] = json!("WEB-DL");
        }
    }
    if let Some(effect) = captures(
        r"(?i)(?<![A-Z0-9])(DOVI|DOLBY\s+VISION|HDR10\+|HDR10|HDR|HLG)(?![A-Z0-9])",
        stem,
        1,
    ) {
        let effect = effect
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_uppercase();
        out["effect"] = json!(if ["DOVI", "DOLBY VISION"].contains(&effect.as_str()) {
            "DoVi"
        } else {
            &effect
        });
    }
    if let Some(codec) = captures(
        r"(?i)(?<![A-Z0-9])(HEVC|H\.265|H265|X265|AVC|H\.264|H264|X264|AV1)(?![A-Z0-9])",
        stem,
        1,
    ) {
        let codec = codec.to_uppercase();
        out["videoCodec"] = json!(match codec.as_str() {
            "HEVC" | "H.265" | "X265" => "H265",
            "AVC" | "H.264" | "X264" => "H264",
            _ => &codec,
        });
    }
    if let Some(audio) = captures(
        r"(?i)(?<![A-Z0-9])((?:ATMOS[ ._-]+)?(?:DTS(?:-HD)?(?:\s+MA)?|TRUEHD|EAC3|DDP|DD\+?|AC3|AAC|FLAC|OPUS)(?:[ ._-]?ATMOS)?(?:[ ._-]?\d\.\d)?)(?![A-Z0-9])",
        stem,
        1,
    ) {
        let channel = regex::Regex::new(r"(\d)[ ._-]?(\d)$")
            .unwrap()
            .captures(&audio)
            .map(|c| (c.get(0).unwrap().start(), format!("{}.{}", &c[1], &c[2])));
        let value = if let Some((start, _)) = &channel {
            &audio[..*start]
        } else {
            &audio
        };
        let atmos = audio.to_lowercase().contains("atmos");
        let value = regex::Regex::new("(?i)ATMOS")
            .unwrap()
            .replace_all(value, "")
            .replace(['.', '_'], " ");
        let value = value
            .trim_matches([' ', '.', '_', '-'])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let value = regex::Regex::new("(?i)^TRUEHD")
            .unwrap()
            .replace(&value, "Dolby TrueHD")
            .to_string();
        let mut value = regex::Regex::new("(?i)^EAC3")
            .unwrap()
            .replace(&value, "EAC3")
            .to_string();
        if atmos {
            value.push_str(" Atmos");
        }
        if let Some((_, channel)) = channel {
            value.push(' ');
            value.push_str(&channel);
        }
        out["audioCodec"] = json!(value);
    }
    if let Some(group) = captures(r"@([A-Za-z0-9][A-Za-z0-9._-]*)$", stem, 1)
        .or_else(|| captures(r"-([A-Za-z0-9][A-Za-z0-9_]*)$", stem, 1))
    {
        out["releaseGroup"] = json!(group);
    }
    out
}
pub fn probe_fields(raw: &Value) -> Value {
    let streams = crate::media_edit::list(&raw["streams"]);
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .unwrap_or(&Value::Null);
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio" && s["disposition"]["default"] == 1)
        .or_else(|| streams.iter().find(|s| s["codec_type"] == "audio"))
        .unwrap_or(&Value::Null);
    let number = |v: &Value| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0.0)
    };
    let mut output = json!({});
    let width = number(&video["width"]) as i64;
    let height = number(&video["height"]) as i64;
    if let Some((_, label)) = [
        (7680, "4320p"),
        (3840, "2160p"),
        (2560, "1440p"),
        (1920, "1080p"),
        (1280, "720p"),
        (854, "480p"),
        (640, "360p"),
    ]
    .iter()
    .find(|(min, _)| width >= *min)
    {
        output["videoFormat"] = json!(label);
    } else if height > 0 {
        let targets = [
            (4320, 48),
            (2880, 48),
            (2160, 48),
            (1920, 40),
            (1800, 40),
            (1600, 40),
            (1536, 40),
            (1440, 40),
            (1366, 32),
            (1280, 32),
            (1200, 32),
            (1152, 32),
            (1080, 40),
            (1050, 32),
            (1024, 32),
            (960, 32),
            (900, 28),
            (864, 28),
            (854, 24),
            (800, 24),
            (768, 24),
            (720, 32),
            (704, 24),
            (640, 24),
            (600, 20),
            (576, 20),
            (540, 20),
            (528, 20),
            (512, 20),
            (506, 20),
            (480, 24),
            (468, 20),
            (456, 20),
            (432, 20),
            (408, 16),
            (400, 16),
            (360, 16),
            (320, 16),
            (288, 16),
            (272, 16),
            (240, 16),
            (228, 12),
            (180, 12),
            (168, 12),
            (144, 12),
            (120, 12),
        ];
        let value = targets
            .iter()
            .find(|(t, d)| (height - t).abs() <= *d || height >= *t)
            .map_or(height, |(t, _)| *t);
        output["videoFormat"] = json!(format!("{value}p"));
    }
    let codec = string(video, "codec_name").to_lowercase();
    let codec = match codec.as_str() {
        "h264" | "avc" => "H264".into(),
        "hevc" | "h265" => "H265".into(),
        "mpeg2video" => "MPEG2".into(),
        _ => codec.to_uppercase(),
    };
    if !codec.is_empty() {
        output["videoCodec"] = json!(codec);
    }
    let pixel = string(video, "pix_fmt");
    let bit = string(video, "bits_per_raw_sample")
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .or_else(|| captures(r"(\d+)(?:le|be)", &pixel, 1))
        .or_else(|| captures(r"(?i)(?:main|high)\s*(\d+)", &string(video, "profile"), 1))
        .or_else(|| (!pixel.is_empty()).then_some("8".into()));
    if let Some(bit) = bit {
        output["videoBit"] = json!(format!("{bit}bit"));
    }
    for field in ["avg_frame_rate", "r_frame_rate"] {
        let raw = string(video, field);
        let rate = if let Some((a, b)) = raw.split_once('/') {
            a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(0.0)
        } else {
            raw.parse::<f64>().unwrap_or(0.0)
        };
        if rate.is_finite() && rate > 0.0 {
            output["fps"] = json!((rate.round_ties_even() as i64).to_string());
            break;
        }
    }
    let codec = string(audio, "codec_name");
    if !codec.is_empty() {
        let mut label = if codec == "truehd" {
            "Dolby TrueHD".into()
        } else if codec == "vorbis" {
            "Vorbis".into()
        } else {
            codec.to_uppercase()
        };
        let tags = audio["tags"]
            .as_object()
            .map(|o| {
                o.values()
                    .map(|v| v.as_str().unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let meta = format!(
            "{} {} {} {}",
            string(audio, "profile"),
            string(audio, "codec_tag_string"),
            string(audio, "codec_long_name"),
            tags
        )
        .to_lowercase();
        if meta.contains("atmos") || tags.contains("全景声") {
            label.push_str(" Dolby Atmos");
        }
        let layout = string(audio, "channel_layout")
            .split('(')
            .next()
            .unwrap_or("")
            .replace(' ', "")
            .to_lowercase();
        let channels: String = match layout.as_str() {
            "mono" => "1.0".into(),
            "stereo" => "2.0".into(),
            "quad" => "4.0".into(),
            s if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.') => s.into(),
            _ => match number(&audio["channels"]) as i64 {
                1 => "1.0",
                2 => "2.0",
                3 => "2.1",
                4 => "4.0",
                5 => "5.0",
                6 => "5.1",
                7 => "6.1",
                8 => "7.1",
                10 => "7.1.2",
                12 => "7.1.4",
                _ => "",
            }
            .into(),
        };
        if !channels.is_empty() {
            label.push(' ');
            label.push_str(&channels);
        }
        output["audioCodec"] = json!(label);
    }
    let side = crate::media_edit::list(&video["side_data_list"])
        .iter()
        .map(|s| string(s, "side_data_type").to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let mut effects = vec![];
    if ["dvh1", "dvhe", "dva1", "dvav"]
        .contains(&string(video, "codec_tag_string").to_lowercase().as_str())
        || side.contains("dovi")
        || side.contains("dolby vision")
    {
        effects.push("DoVi");
    }
    if side.contains("2094-40") {
        effects.push("HDR10+");
    }
    let transfer = string(video, "color_transfer");
    let primaries = string(video, "color_primaries");
    let space = string(video, "color_space");
    if effects.is_empty() {
        if transfer == "smpte2084" {
            effects.push("HDR10");
        } else if transfer.contains("arib-std-b67") {
            effects.push("HLG");
        }
    }
    if effects.is_empty()
        && (primaries.is_empty() || primaries == "bt709")
        && (transfer == "bt709" || (transfer.is_empty() && space == "bt709"))
    {
        effects.push("SDR");
    }
    if !effects.is_empty() {
        output["effect"] = json!(effects.join(" "));
    }
    output
}
pub fn supplement(name: &str, fields: &Value, ext: &str) -> Result<String> {
    let name = extension(name, ext)?;
    let path = Path::new(&name);
    let filename = path.file_name().unwrap().to_string_lossy();
    let stem = filename.strip_suffix(ext).unwrap_or(&filename);
    let normal = |s: &str| {
        regex::Regex::new(r"[\s._-]+")
            .unwrap()
            .replace_all(s, " ")
            .to_lowercase()
    };
    let mut normalized = normal(stem);
    let mut additions = vec![];
    for key in [
        "videoFormat",
        "effect",
        "videoCodec",
        "videoBit",
        "fps",
        "audioCodec",
    ] {
        let value = string(fields, key);
        if value.is_empty() {
            continue;
        }
        let normalized_value = normal(&value);
        let mut aliases = vec![normalized_value.clone()];
        if key == "videoCodec" {
            match normalized_value.as_str() {
                "h265" => aliases.push("hevc".into()),
                "hevc" => aliases.push("h265".into()),
                "h264" => aliases.push("avc".into()),
                "avc" => aliases.push("h264".into()),
                _ => (),
            }
        } else if key == "audioCodec" {
            aliases.push(normalized_value.replace("dolby ", ""));
            aliases.push(normalized_value.replace("dolby atmos ", "atmos "));
        }
        if !aliases
            .iter()
            .any(|a| !a.is_empty() && normalized.contains(a))
        {
            additions.push(if key == "fps" {
                format!("{value}fps")
            } else {
                value
            });
            normalized.push(' ');
            normalized.push_str(&normalized_value);
        }
    }
    let filename = format!(
        "{stem}{}{ext}",
        if additions.is_empty() {
            String::new()
        } else {
            format!(".{}", additions.join("."))
        }
    );
    Ok(path.with_file_name(filename).to_string_lossy().into())
}
pub fn english(raw: &str) -> bool {
    raw.chars().any(|c| c.is_ascii_alphabetic())
        && raw
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(|c| c.is_ascii_alphabetic() || matches!(c as u32,0xc0..=0x24f|0x1e00..=0x1eff))
}
pub fn context(item: &Value, source: &str, extra: &Value) -> Value {
    let episode = item["Type"] == "Episode";
    let season = if episode {
        item["ParentIndexNumber"].as_i64()
    } else {
        item["IndexNumber"].as_i64()
    };
    let number = episode.then(|| item["IndexNumber"].as_i64()).flatten();
    let token = match (season, number) {
        (Some(s), Some(e)) => format!("S{s:02}E{e:02}"),
        _ => String::new(),
    };
    let stem = Path::new(source)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let marker = if !token.is_empty() {
        token.clone()
    } else {
        item["ProductionYear"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_default()
    };
    let mut english_title = String::new();
    if !marker.is_empty() {
        let pattern =
            regex::Regex::new(&format!(r"(?i)\.{}(?:\.|$)", regex::escape(&marker))).unwrap();
        if let Some(found) = pattern.find_iter(&stem).last()
            && let Some((_, name)) = stem[..found.start()].split_once('.')
            && english(name)
        {
            english_title = name.trim_matches([' ', '.']).into();
        }
    }
    if english_title.is_empty() && english(&string(item, "OriginalTitle")) {
        english_title = string(item, "OriginalTitle");
    }
    let mut out = json!({"title":item["Name"].as_str().unwrap_or(""),"en_title":english_title,"year":item["ProductionYear"].as_i64().map(|n|json!(n)).unwrap_or(json!("")),"tmdbid":item["ProviderIds"]["Tmdb"].as_str().unwrap_or(""),"season":season.unwrap_or(0),"episode":number.unwrap_or(0),"season_episode":token,"fileExt":media_paths::extension(source)});
    for field in [
        "webSource",
        "edition",
        "videoFormat",
        "videoCodec",
        "audioCodec",
        "part",
        "customization",
        "releaseGroup",
        "resourceType",
        "effect",
    ] {
        out[field] = json!("");
    }
    if let Some(extra) = extra.as_object() {
        out.as_object_mut().unwrap().extend(extra.clone());
    }
    out
}
pub fn replace(path: &str, directories: &[String], filename: &str) -> Result<String> {
    let raw = path.trim().replace('\\', "/");
    let parsed = reqwest::Url::parse(&raw)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"));
    if let Some(url) = &parsed
        && ["", ".strm"].contains(&media_paths::extension(url.path()).as_str())
    {
        return Ok(raw);
    }
    let path = parsed
        .as_ref()
        .map(|u| {
            percent_encoding::percent_decode_str(u.path())
                .decode_utf8_lossy()
                .into_owned()
        })
        .unwrap_or(raw.clone());
    let mut parts = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.len() < directories.len() + 1 {
        return Err(ApiError::bad("原始路径层级不足"));
    }
    parts.truncate(parts.len() - directories.len() - 1);
    for directory in directories {
        parts.push(safe_path(directory)?.rsplit('/').next().unwrap().into());
    }
    parts.push(safe_path(filename)?.rsplit('/').next().unwrap().into());
    let path = format!(
        "{}{}",
        if path.starts_with('/') { "/" } else { "" },
        parts.join("/")
    );
    if let Some(mut url) = parsed {
        url.set_path(&path);
        Ok(url.to_string())
    } else {
        Ok(path)
    }
}
pub fn local(old: &str, directories: &[String], filename: &str) -> Result<PathBuf> {
    let ext = media_paths::extension(old);
    let media_ext = media_paths::extension(filename);
    let stem = Path::new(filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let name = if ext == ".strm" {
        format!(
            "{}.strm",
            if media_ext == ".iso" { filename } else { &stem }
        )
    } else {
        format!("{stem}{}", if ext.is_empty() { media_ext } else { ext })
    };
    Ok(PathBuf::from(replace(old, directories, &name)?))
}

pub fn short_name(name: &str) -> bool {
    regex::Regex::new(r"(?i)^[A-Za-z0-9]{1,8}~[0-9A-Z](?:\.[A-Za-z0-9]{1,3})?$")
        .unwrap()
        .is_match(name)
}
pub fn protected_mount(path: &str, config: &Value) -> bool {
    let path = path.trim().replace('\\', "/").to_lowercase();
    let root = string(config, "cd2_media_root")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase();
    (!root.is_empty() && (path == root || path.starts_with(&format!("{root}/"))))
        || path.starts_with("/cd2/")
        || path.contains("/115open/")
        || path.contains("/115/")
}
fn relative_cache(path: &str, config: &Value) -> String {
    let path = path.trim().replace('\\', "/");
    let cd2 = string(config, "cd2_media_root")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    if !cd2.is_empty()
        && let Some(relative) = path.strip_prefix(&format!("{cd2}/"))
    {
        return relative.into();
    }
    let path = path.trim_start_matches('/');
    for root in [
        string(config, "p115_fingerprint_media_root_path"),
        Path::new(&cd2)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    ] {
        let root = root.trim_matches('/');
        if !root.is_empty()
            && let Some(relative) = path.strip_prefix(&format!("{root}/"))
        {
            return relative.into();
        }
    }
    path.into()
}
pub fn configured_cloud(path: &str, config: &Value) -> String {
    let path = path.trim().replace('\\', "/");
    let root = string(config, "cd2_media_root");
    if path.is_empty() || path.starts_with('/') || root.is_empty() {
        path
    } else {
        Path::new(&root)
            .join(relative_cache(&path, config))
            .to_string_lossy()
            .into_owned()
    }
}
pub async fn exact_entry(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let Ok(mut entries) = tokio::fs::read_dir(path.parent().unwrap_or(Path::new("."))).await else {
        return false;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name() == name {
            return true;
        }
    }
    false
}
pub async fn stream_target(path: &Path) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.replace('\\', "/"))
}
async fn add_local(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_file() && exact_entry(&path).await && !paths.contains(&path) {
        paths.push(path);
    }
}
fn stream_names(path: &Path) -> Vec<PathBuf> {
    let text = path.to_string_lossy();
    if text.to_lowercase().ends_with(".strm") {
        return vec![path.into()];
    }
    let mut names = vec![];
    if !["", ".iso"].contains(&media_paths::extension(&text).as_str()) {
        names.push(path.with_extension("strm"));
    }
    names.push(PathBuf::from(format!("{text}.strm")));
    names
}
pub async fn local_candidates(
    media: &str,
    cloud: &str,
    cache: &Value,
    config: &Value,
) -> Vec<PathBuf> {
    let mut paths = vec![];
    let media = media.trim().replace('\\', "/");
    if media.to_lowercase().ends_with(".strm") {
        add_local(&mut paths, PathBuf::from(&media)).await;
    }
    let cache_path = string(cache, "local_path").replace('\\', "/");
    let root = string(config, "local_strm_root");
    let mut legacy_targets = vec![];
    for raw in [&cache_path, cloud] {
        if raw.is_empty() {
            continue;
        }
        let parsed = reqwest::Url::parse(raw).ok();
        let decoded = parsed
            .as_ref()
            .map(|url| {
                percent_encoding::percent_decode_str(url.path())
                    .decode_utf8_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| raw.to_string());
        if parsed.is_none() {
            for path in stream_names(Path::new(&decoded)) {
                add_local(&mut paths, path).await;
            }
            if !protected_mount(&decoded, config) {
                add_local(&mut paths, PathBuf::from(&decoded)).await;
            }
        }
        if !root.is_empty() {
            let relative = relative_cache(&decoded, config);
            if !Path::new(&relative)
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            {
                let local = Path::new(&root).join(relative);
                for path in stream_names(&local) {
                    add_local(&mut paths, path).await;
                }
                if parsed.is_none() && !protected_mount(&local.to_string_lossy(), config) {
                    add_local(&mut paths, local).await;
                }
            }
        }
        legacy_targets.push(configured_cloud(&decoded, config));
    }
    if paths.is_empty() && !root.is_empty() {
        for expected in legacy_targets {
            let cd2 = string(config, "cd2_media_root");
            let Some(relative) = expected.strip_prefix(&format!("{}/", cd2.trim_end_matches('/')))
            else {
                continue;
            };
            let directory =
                Path::new(&root).join(Path::new(relative).parent().unwrap_or(Path::new("")));
            let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
                continue;
            };
            let mut matches = vec![];
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                let short = short_name(&name);
                if !name.to_lowercase().ends_with(".strm") && !short {
                    continue;
                }
                let Some(target) = stream_target(&entry.path()).await else {
                    continue;
                };
                let parsed = reqwest::Url::parse(&target).ok();
                let target_path = parsed.as_ref().map_or_else(
                    || media_paths::clean(&target),
                    |url| {
                        percent_encoding::percent_decode_str(url.path())
                            .decode_utf8_lossy()
                            .into_owned()
                    },
                );
                let target_path = configured_cloud(&target_path, config);
                if Path::new(&target_path).parent() == Path::new(&expected).parent()
                    && media_paths::extension(&target_path) == media_paths::extension(&expected)
                    && (!short
                        || Path::new(&target_path).file_name() == Path::new(&expected).file_name())
                {
                    matches.push((entry.path(), target));
                }
            }
            if matches.len() > 1 {
                let pc = string(cache, "pick_code");
                matches.retain(|(_, target)| {
                    !pc.is_empty()
                        && crate::fingerprints::pickcode(target).as_deref() == Some(pc.as_str())
                });
            }
            if matches.len() == 1 {
                add_local(&mut paths, matches.remove(0).0).await;
                break;
            }
        }
    }
    if paths.is_empty() && !protected_mount(&media, config) && reqwest::Url::parse(&media).is_err()
    {
        add_local(&mut paths, PathBuf::from(media)).await;
    }
    paths
}

fn nfo_has_tmdb(text: &str, tmdb: &str) -> bool {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut selected = vec![];
    let mut found = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = String::from_utf8_lossy(element.name().as_ref()).to_lowercase();
                let is_tmdb = name == "tmdbid"
                    || (name == "uniqueid"
                        && element.attributes().flatten().any(|a| {
                            a.key.as_ref() == b"type" && a.value.eq_ignore_ascii_case(b"tmdb")
                        }));
                selected.push(is_tmdb);
            }
            Ok(Event::Text(value)) if selected.last() == Some(&true) => {
                found |= value.decode().is_ok_and(|value| value.trim() == tmdb);
            }
            Ok(Event::End(_)) => {
                selected.pop();
            }
            Ok(Event::Eof) => return found,
            Err(_) => return false,
            _ => (),
        }
    }
}
pub async fn sidecar_moves(
    media: &Path,
    target: &Path,
    files: &[PathBuf],
    expected: &str,
    tmdb: &str,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let old_anchor = media.file_stem().unwrap_or_default().to_string_lossy();
    let new_anchor = target.file_stem().unwrap_or_default().to_string_lossy();
    let siblings = files
        .iter()
        .filter(|p| p.parent() == media.parent() && p.as_path() != media)
        .collect::<Vec<_>>();
    let anchors = files
        .iter()
        .filter(|p| {
            p.parent() == media.parent()
                && p.extension()
                    .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("strm"))
        })
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy()))
        .collect::<Vec<_>>();
    let mut moves = vec![];
    for file in &siblings {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let Some(suffix) = name
            .strip_prefix(old_anchor.as_ref())
            .filter(|s| s.starts_with(['.', '-']))
        else {
            continue;
        };
        let longest = anchors
            .iter()
            .filter(|anchor| {
                name.strip_prefix(anchor.as_ref())
                    .is_some_and(|s| s.starts_with(['.', '-']))
            })
            .map(|anchor| anchor.len())
            .max()
            .unwrap_or(0);
        if longest <= old_anchor.len() {
            moves.push((
                (*file).clone(),
                target.with_file_name(format!("{new_anchor}{suffix}")),
            ));
        }
    }
    let mut infos = vec![];
    let mut nfos = vec![];
    for file in siblings {
        if moves.iter().any(|(old, _)| old == file)
            || !short_name(&file.file_name().unwrap_or_default().to_string_lossy())
        {
            continue;
        }
        let bytes = tokio::fs::read(file).await?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{feff}');
        if let Ok(value) = serde_json::from_str::<Value>(text)
            && crate::media_info::container(&value).is_some_and(|source| {
                source["MediaStreams"].as_array().is_some_and(|streams| {
                    streams
                        .iter()
                        .any(|s| matches!(s["Type"].as_str(), Some("Video" | "Audio" | "Subtitle")))
                })
            })
            && media_paths::matches_container(&value, expected)
        {
            infos.push(file.clone());
        } else if !tmdb.is_empty() && nfo_has_tmdb(text, tmdb) {
            nfos.push((file.clone(), bytes));
        }
    }
    if infos.len() > 1 {
        return Err(ApiError::bad("短文件名媒体信息匹配不唯一"));
    }
    if let Some(info) = infos.pop() {
        let destination = target.with_file_name(format!("{new_anchor}-mediainfo.json"));
        if moves.iter().any(|(_, new)| new == &destination) {
            return Err(ApiError::bad("短文件名媒体信息与已有旁车冲突"));
        }
        moves.push((info, destination));
    }
    let destination = target.with_file_name(format!("{new_anchor}.nfo"));
    if !moves.iter().any(|(_, new)| new == &destination) && !nfos.is_empty() {
        if nfos.iter().all(|(_, bytes)| bytes == &nfos[0].1) {
            nfos.sort_by_key(|(path, _)| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
            });
            moves.push((nfos.remove(0).0, destination));
        } else {
            tracing::warn!("短文件名NFO内容不同，跳过自动恢复：{}", target.display());
        }
    }
    Ok(moves)
}
