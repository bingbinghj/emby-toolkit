use crate::{AppState, error::Result, media_paths, state::string};
use serde_json::Value;
use sqlx::Row;
use std::{path::Path, sync::LazyLock};

pub fn pickcode(url: &str) -> Option<String> {
    static PATTERNS: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
        [
            r"(?i)pick_?code=([a-zA-Z0-9]+)",
            r"/d/([a-zA-Z0-9]+)[.?/]",
            r"/d/([a-zA-Z0-9]+)$",
            r"(?i)fileid=([a-zA-Z0-9]+)",
        ]
        .iter()
        .map(|p| regex::Regex::new(p).unwrap())
        .collect()
    });
    PATTERNS
        .iter()
        .find_map(|p| p.captures(url).map(|c| c[1].to_string()))
}
pub async fn lookup(
    state: &AppState,
    path: &str,
) -> Result<(Option<String>, Option<String>, String)> {
    let config = state.config.read().await.clone();
    if config["monitor_sha1_pc_search"] == false || path.is_empty() {
        return Ok((None, None, String::new()));
    }
    let target = media_paths::strm_target(path)
        .await
        .unwrap_or_else(|| path.into());
    let pc = pickcode(&target);
    let candidates = media_paths::local_candidates(
        path,
        &target,
        &[
            &string(&config, "local_strm_root"),
            &string(&config, "cd2_media_root"),
        ],
    );
    let row = sqlx::query("SELECT pick_code,sha1,local_path FROM p115_filesystem_cache WHERE ($1::text IS NOT NULL AND pick_code=$1) OR ($1::text IS NULL AND trim(both '/' from replace(local_path,chr(92),'/'))=ANY($2)) ORDER BY array_position($2,trim(both '/' from replace(local_path,chr(92),'/'))) NULLS LAST LIMIT 1")
        .bind(&pc).bind(&candidates).fetch_optional(&state.db).await?;
    if let Some(row) = row {
        return Ok((
            pc.or(row.try_get("pick_code")?),
            row.try_get("sha1")?,
            row.try_get::<Option<String>, _>("local_path")?
                .unwrap_or_default(),
        ));
    }
    let sha = if let Some(pc) = pc.as_deref()
        && let Some(id) = file_id(pc)
    {
        crate::p115::file_info(state, id).await.ok().and_then(|v| {
            v["sha1"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    } else {
        None
    };
    Ok((pc, sha, String::new()))
}
pub async fn asset_path(
    state: &AppState,
    raw: &str,
    pc: Option<&str>,
    cache_path: &str,
    item: &Value,
    id: &str,
) -> String {
    if !raw.starts_with("http") {
        return raw.into();
    }
    let main = string(item, "Path");
    if id == string(item, "Id") {
        return main;
    }
    let base = Path::new(&main).parent().unwrap_or(Path::new(""));
    if main.to_lowercase().ends_with(".strm")
        && let Ok(mut entries) = tokio::fs::read_dir(base).await
    {
        let real_stem = Path::new(cache_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let mut named = None;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let candidate = entry.path();
            if !candidate
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("strm"))
                || !entry.file_type().await.is_ok_and(|s| s.is_file())
            {
                continue;
            }
            if let Some(pc) = pc
                && tokio::fs::read_to_string(&candidate)
                    .await
                    .is_ok_and(|s| s.chars().take(8192).collect::<String>().contains(pc))
            {
                return candidate.to_string_lossy().into_owned();
            }
            if !real_stem.is_empty()
                && candidate
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase()
                    == real_stem
            {
                named = Some(candidate);
            }
        }
        if let Some(named) = named {
            return named.to_string_lossy().into_owned();
        }
    }
    if cache_path.is_empty() {
        return String::new();
    }
    let root = string(&*state.config.read().await, "local_strm_root");
    let cache_path = cache_path.replace('\\', "/");
    if !root.is_empty() {
        Path::new(&root)
            .join(cache_path.trim_start_matches('/'))
            .to_string_lossy()
            .into_owned()
    } else if !main.is_empty() {
        base.join(Path::new(&cache_path).file_name().unwrap_or_default())
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    }
}

// 115 pickcode substitution tables: p115pickcode (ChenyangGao), GPLv3.
pub fn file_id(pc: &str) -> Option<u64> {
    if pc.bytes().all(|b| b.is_ascii_digit()) {
        return pc.parse().ok();
    }
    let tables = [
        "fuln1ytpj3smg8d5a094qh7cxkbi62zvewro",
        "sk721n9a0emlfpcrzbqdw3gjh6ty5xui48vo",
        "ywcz3hite6f1j0guoakvdb2ns7p8qr9ml5x4",
        "rq2vl5o7wsken9u8tp4jg3zbyc6xmhifd01a",
        "ljm9eqbcfhw7ktv3x1dgp5ua8y6s4znr2io0",
    ];
    let prefix = pc.as_bytes().first()?.checked_sub(b'a')? as usize;
    let table = tables.get(prefix)?.as_bytes();
    let cipher = pc.get(1..pc.len().checked_sub(4)?)?;
    if cipher.is_empty() {
        return None;
    }
    cipher.bytes().try_fold(0_u64, |value, b| {
        let position = if b.is_ascii_digit() {
            b - b'0'
        } else if b.is_ascii_lowercase() {
            b - b'a' + 10
        } else {
            return None;
        } as usize;
        let decoded = *table.get(position)?;
        let digit = if decoded.is_ascii_digit() {
            decoded - b'0'
        } else {
            decoded - b'a' + 10
        };
        value.checked_mul(36)?.checked_add(digit as u64)
    })
}
