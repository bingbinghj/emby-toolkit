//! Pre-library file processing: identity and scope come from new files, never from an Emby scan.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
    state::string,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};
use tokio_util::sync::CancellationToken;

fn rx(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("static monitor expression")
}
pub fn season(text: &str) -> Option<i64> {
    let text = text.replace('\\', "/");
    static SPECIAL: LazyLock<regex::Regex> = LazyLock::new(|| {
        rx(
            r"(?i)^(specials?|sp|ova|oad|extras?|特别篇|特別篇|番外(?:篇)?|外传|外傳|第\s*0+\s*季|season\s*0+|s0+)$",
        )
    });
    if text.split('/').any(|p| SPECIAL.is_match(p.trim())) {
        return Some(0);
    }
    static SEASON: LazyLock<regex::Regex> = LazyLock::new(|| {
        rx(r"(?i)(?:^|[/\s.\-_\[(])(?:Season\s*|S|第)\s*(\d{1,4})(?:季)?(?:$|[/\s.\-_\])])")
    });
    SEASON.captures(&text).and_then(|c| c[1].parse().ok())
}
pub fn episode(name: &str) -> Option<i64> {
    let name = Path::new(name).file_stem()?.to_str()?;
    static DIRECT: LazyLock<regex::Regex> = LazyLock::new(|| rx(r"(?i)s\d{1,4}[ep](\d{1,4})"));
    if let Some(c) = DIRECT.captures(name) {
        return c[1].parse().ok();
    }
    static EPISODE: LazyLock<regex::Regex> = LazyLock::new(|| {
        rx(
            r"(?i)(?:^|[ .\-_\[(])(?:ep|episode)[ .\-]*?(\d{1,4})\b|(?:^|[ .\-_\[(])e(\d{1,4})\b|第\s*(\d{1,4})\s*[集话話回]|(?:^|[ .\-_\[(])(\d{1,4})[集话話回]",
        )
    });
    if let Some(c) = EPISODE.captures(name) {
        return c
            .iter()
            .skip(1)
            .flatten()
            .find_map(|n| n.as_str().parse().ok());
    }
    if name.bytes().all(|b| b.is_ascii_digit()) {
        return name.parse().ok();
    }
    static TECHNICAL: LazyLock<regex::Regex> = LazyLock::new(|| {
        rx(r"(?i)(19|20)\d{2}|1080p?|2160p?|720p?|480p?|4k|264|265|10bit|8bit|5\.1|7\.1|2\.0")
    });
    let clean = TECHNICAL.replace_all(name, "");
    static ANIME: LazyLock<regex::Regex> =
        LazyLock::new(|| rx(r"(?:\s-\s+)(\d{1,4})(?:\s|$)|\[(\d{1,4})\]|【(\d{1,4})】"));
    if let Some(c) = ANIME.captures(&clean) {
        return c
            .iter()
            .skip(1)
            .flatten()
            .find_map(|n| n.as_str().parse().ok());
    }
    static TAIL: LazyLock<regex::Regex> =
        LazyLock::new(|| rx(r"(?:^|[ .\-_\[(])(\d{1,4})(?:[\])]|\s*)$"));
    TAIL.captures(&clean).and_then(|c| c[1].parse().ok())
}
pub fn numbers(path: &Path) -> (Option<i64>, Option<i64>) {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    static DIRECT: LazyLock<regex::Regex> = LazyLock::new(|| rx(r"(?i)s(\d{1,4})[ep](\d{1,4})"));
    if let Some(c) = DIRECT.captures(name) {
        return (c[1].parse().ok(), c[2].parse().ok());
    }
    (
        season(name).or_else(|| {
            season(
                path.parent()
                    .and_then(Path::file_name)
                    .and_then(|s| s.to_str())
                    .unwrap_or(""),
            )
        }),
        episode(name),
    )
}
pub fn identity(paths: &[PathBuf]) -> Value {
    let Some(first) = paths.first() else {
        return Value::Null;
    };
    let name = first.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let parent = first.parent().unwrap_or(Path::new(""));
    let folder = parent.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let grand = parent
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let scope = paths
        .iter()
        .filter_map(|p| {
            let (s, e) = numbers(p);
            Some(json!({"season_number":s?,"episode_number":e?,"path":p}))
        })
        .collect::<Vec<_>>();
    let kind = if scope.is_empty() { "Movie" } else { "Series" };
    static TMDB: LazyLock<regex::Regex> = LazyLock::new(|| rx(r"(?i)(?:tmdb|tmdbid)[-_=\s]*(\d+)"));
    let id = [folder, grand, name]
        .into_iter()
        .find_map(|text| TMDB.captures(text).map(|c| c[1].to_string()))
        .filter(|s| s.bytes().any(|b| b != b'0'));
    let text = if season(folder).is_some() {
        grand
    } else {
        folder
    };
    let (query, year) = title_year(text);
    let (query, year) = if query.is_empty() {
        title_year(first.file_stem().and_then(|s| s.to_str()).unwrap_or(""))
    } else {
        (query, year)
    };
    json!({"tmdb_id":id,"kind":kind,"query":query,"year":year,"scope":scope})
}
fn title_year(text: &str) -> (String, Option<String>) {
    static YEAR: LazyLock<regex::Regex> = LazyLock::new(|| rx(r"\b(?:19|20)\d{2}\b"));
    static EP: LazyLock<regex::Regex> = LazyLock::new(|| rx(r"(?i)s\d{1,2}e\d{1,2}"));
    let year = YEAR.find_iter(text).last();
    let end = year
        .map(|m| m.start())
        .or_else(|| EP.find(text).map(|m| m.start()))
        .unwrap_or(text.len());
    (
        text[..end]
            .replace(['.', '_'], " ")
            .trim_matches([' ', '-', '[', ']', '(', ')'])
            .to_string(),
        year.map(|m| m.as_str().into()),
    )
}
async fn ensure(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    let resolved = tokio::fs::canonicalize(path).await?;
    if !roots.iter().any(|root| resolved.starts_with(root)) {
        return Err(ApiError::bad("实时监控路径超出配置目录"));
    }
    Ok(resolved)
}
async fn before_write(path: &Path, roots: &[PathBuf]) -> Result<()> {
    ensure(
        path.parent().ok_or_else(|| ApiError::bad("媒体目录无效"))?,
        roots,
    )
    .await?;
    if tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(ApiError::bad("监控元数据目标不能为符号链接"));
    }
    Ok(())
}
async fn check_active(
    state: &AppState,
    paths: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
    scrape: bool,
) -> Result<crate::monitor::Settings> {
    tokio::select! {
        _ = cancel.cancelled() => return Err(ApiError::bad("实时监控服务已停止")),
        result = context.checkpoint() => result?,
    }
    if cancel.is_cancelled() {
        return Err(ApiError::bad("实时监控服务已停止"));
    }
    let config = state.config.read().await.clone();
    let settings = crate::monitor::Settings::read(&config).await;
    if !settings.enabled {
        return Err(ApiError::bad("实时监控已关闭，停止处理"));
    }
    for path in paths {
        if settings.accepts(path).await.is_none() || (scrape && settings.excluded(path)) {
            return Err(ApiError::bad("实时监控配置已变化，停止处理文件"));
        }
    }
    Ok(settings)
}
fn media_root<'a>(kind: &str, paths: &'a [PathBuf]) -> &'a Path {
    let parent = paths[0].parent().unwrap();
    if kind == "Series"
        && season(parent.file_name().and_then(|s| s.to_str()).unwrap_or("")).is_some()
    {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    }
}
fn normalize_title(kind: &str, data: &mut Value) {
    let key = if kind == "Movie" { "title" } else { "name" };
    let clean = |text: &str| {
        text.replace(['\u{a0}', '\u{3000}'], " ")
            .chars()
            .filter(|c| {
                !('\u{200b}'..='\u{200f}').contains(c)
                    && !('\u{202a}'..='\u{202e}').contains(c)
                    && !('\u{2060}'..='\u{206f}').contains(c)
                    && *c != '\u{feff}'
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let spam = |text: &str| {
        [
            "看黄",
            "片网",
            "色网",
            "澳门",
            "赌场",
            "真人发牌",
            "加微",
            "微信",
            "网址",
            "在线观看",
            "免费看",
            "AV",
        ]
        .iter()
        .any(|word| text.contains(word))
            || rx(r"(?i)[a-zA-Z0-9-]+\.(com|net|org|xyz|cc|tv|vip|top|me)\b|\d{6,}").is_match(text)
    };
    let current = clean(&string(data, key));
    if crate::translation::chinese(&current) && !spam(&current) {
        data[key] = json!(current);
        return;
    }
    let mut alternatives = list(&data["alternative_titles"]["titles"]);
    if alternatives.is_empty() {
        alternatives = list(&data["alternative_titles"]["results"]);
    }
    alternatives.sort_by_key(|a| match string(a, "iso_3166_1").as_str() {
        "CN" => 1,
        "SG" => 2,
        "TW" => 3,
        "HK" => 4,
        _ => 5,
    });
    data[key] = json!(
        alternatives
            .iter()
            .map(|a| clean(&string(a, "title")))
            .find(|s| crate::translation::chinese(s) && !spam(s))
            .unwrap_or_else(|| clean(&string(
                data,
                if kind == "Movie" {
                    "original_title"
                } else {
                    "original_name"
                }
            )))
    );
}
pub async fn prepare_batch(
    state: &AppState,
    groups: &[Vec<PathBuf>],
    roots: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<Value> {
    let mut result = json!({"prepared":0,"failed":0,"failed_groups":[],"cache_hits":0,"nfo_written":0,"images_downloaded":0});
    let mut notify = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            state.item_delay(context).await?;
        }
        let mut ignored = false;
        for path in group {
            if crate::emby::path_ignored(state, path.to_str().unwrap_or("")).await? {
                ignored = true;
                break;
            }
        }
        if ignored {
            continue;
        }
        check_active(state, group, context, cancel, true).await?;
        context
            .update(
                (index * 90 / groups.len().max(1)) as u64,
                "预入库刮削",
                &format!("处理第 {}/{} 组文件", index + 1, groups.len()),
            )
            .await;
        match prepare_group(state, group, roots, context, cancel).await {
            Ok(data) => {
                for path in data.notify {
                    notify.insert(path);
                }
                for path in data.metadata_dirs {
                    directories.insert(path);
                }
                result["prepared"] = json!(result["prepared"].as_u64().unwrap() + 1);
                result["cache_hits"] =
                    json!(result["cache_hits"].as_u64().unwrap() + u64::from(data.cache_hit));
                result["nfo_written"] =
                    json!(result["nfo_written"].as_u64().unwrap() + data.nfo as u64);
                result["images_downloaded"] =
                    json!(result["images_downloaded"].as_u64().unwrap() + data.images as u64);
            }
            Err(error) => {
                check_active(state, group, context, cancel, true).await?;
                result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                result["failed_groups"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"files":group,"reason":error.1}));
            }
        }
    }
    if !notify.is_empty() {
        refresh_files(
            state,
            &notify.into_iter().collect::<Vec<_>>(),
            roots,
            false,
            context,
            cancel,
        )
        .await?;
    }
    if !directories.is_empty() {
        refresh_files(
            state,
            &directories.into_iter().collect::<Vec<_>>(),
            roots,
            true,
            context,
            cancel,
        )
        .await?;
    }
    Ok(result)
}
struct Prepared {
    notify: Vec<PathBuf>,
    metadata_dirs: Vec<PathBuf>,
    cache_hit: bool,
    nfo: usize,
    images: usize,
}
fn scope_covers(payload: &Value, scope: &[(i64, i64)], id: &str) -> bool {
    scope.iter().all(|(season, ep)| {
        list(&payload["seasons_details"]).iter().any(|s| {
            if s["season_number"].as_i64() != Some(*season) {
                return false;
            }
            let group = string(payload, "episode_group_id");
            let season_id = match &s["id"] {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => String::new(),
            };
            if group.is_empty() {
                !season_id.is_empty() && season_id.bytes().all(|n| n.is_ascii_digit())
            } else {
                season_id == format!("{id}-G{group}-S{season}")
            }
        }) && payload["episodes_details"]
            .get(format!("S{season}E{ep}"))
            .is_some_and(|episode| match &episode["id"] {
                Value::Number(id) => id.as_u64().is_some_and(|id| id > 0),
                Value::String(id) => !id.is_empty() && id != "0",
                _ => false,
            })
    })
}
async fn prepare_group(
    state: &AppState,
    group: &[PathBuf],
    roots: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<Prepared> {
    if group.is_empty() {
        return Err(ApiError::bad("监控文件组为空"));
    }
    let mut paths = vec![];
    for path in group {
        paths.push(ensure(path, roots).await?);
    }
    let identity = identity(&paths);
    let kind = string(&identity, "kind");
    let mut id = string(&identity, "tmdb_id");
    if id.is_empty() {
        let query = string(&identity, "query");
        if query.is_empty() {
            return Err(ApiError::bad("无法从路径识别媒体名称"));
        }
        let mut parameters = vec![("query", query)];
        if let Some(year) = identity["year"].as_str() {
            parameters.push((
                if kind == "Series" {
                    "first_air_date_year"
                } else {
                    "year"
                },
                year.into(),
            ));
        }
        let search = crate::tmdb::get(
            state,
            if kind == "Series" {
                "/search/tv"
            } else {
                "/search/movie"
            },
            &parameters,
        )
        .await?;
        id = list(&search["results"])
            .first()
            .and_then(|v| v["id"].as_u64())
            .filter(|n| *n > 0)
            .map(|n| n.to_string())
            .ok_or_else(|| ApiError::bad("TMDb 搜索没有可用匹配"))?;
    }
    let mut item = json!({"Id":"pending","Type":kind,"ProviderIds":{"Tmdb":id},"Path":paths[0],"Name":identity["query"]});
    let known:Vec<Value>=sqlx::query_scalar("SELECT asset_details_json FROM media_metadata WHERE in_library=true AND ((item_type='Movie' AND tmdb_id=$1) OR (item_type='Episode' AND parent_series_tmdb_id=$1))").bind(&id).fetch_all(&state.db).await?;
    let known_stems = known
        .iter()
        .flat_map(list)
        .filter_map(|asset| {
            Path::new(&string(&asset, "path"))
                .file_stem()
                .map(|name| name.to_string_lossy().trim().to_lowercase())
        })
        .collect::<BTreeSet<_>>();
    let all_paths = paths.clone();
    paths.retain(|path| {
        !known_stems.contains(
            &path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    });
    if paths.is_empty() {
        return Ok(Prepared {
            notify: all_paths,
            metadata_dirs: vec![],
            cache_hit: true,
            nfo: 0,
            images: 0,
        });
    }
    item["Path"] = json!(paths[0]);
    let scope = paths
        .iter()
        .filter_map(|p| {
            let (s, e) = numbers(p);
            Some((s?, e?))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let seasons = scope
        .iter()
        .map(|s| s.0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cached = crate::metadata::reconstruct_full(state, &id, &kind, false).await?;
    let mut cache_hit = false;
    let mut patch_records = vec![];
    let (mut payload, cast) = if let Some((mut payload, cast)) = cached.filter(|(p, _)| {
        kind == "Movie" || string(p, "episode_group_id").is_empty() || scope_covers(p, &scope, &id)
    }) {
        cache_hit = true;
        if kind == "Series" && string(&payload, "episode_group_id").is_empty() {
            let old_seasons = list(&payload["seasons_details"])
                .iter()
                .filter_map(|s| s["season_number"].as_i64())
                .collect::<BTreeSet<_>>();
            let old_episodes = payload["episodes_details"]
                .as_object()
                .map(|o| o.keys().cloned().collect::<BTreeSet<_>>())
                .unwrap_or_default();
            if crate::ingest::patch_cached_series(state, &id, &mut payload, &scope).await? {
                let mut additions = payload.clone();
                additions["seasons_details"] = json!(
                    list(&payload["seasons_details"])
                        .into_iter()
                        .filter(
                            |s| !old_seasons.contains(&s["season_number"].as_i64().unwrap_or(-1))
                        )
                        .collect::<Vec<_>>()
                );
                additions["episodes_details"] = json!(
                    payload["episodes_details"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(key, _)| !old_episodes.contains(*key))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<serde_json::Map<_, _>>()
                );
                patch_records =
                    crate::metadata_records::pending("Series", &additions, &cast, &item)?;
                patch_records.retain(|r| r["item_type"] != "Series");
            }
        }
        (payload, cast)
    } else {
        context.checkpoint().await?;
        let mut data = if kind == "Movie" {
            crate::tmdb::details(state, "Movie", &id).await?
        } else {
            let signals = list(&identity["scope"]);
            let preferred = crate::metadata::row(state, &id, "Series")
                .await?
                .map(|r| string(&r, "episode_group_id"))
                .unwrap_or_default();
            let group =
                crate::episode_groups::match_group(state, &id, &signals, &preferred).await?;
            if let Some(group) = group {
                crate::tmdb::aggregate_group(state, &id, &group, &seasons).await?
            } else {
                crate::tmdb::aggregate(state, &id, &seasons).await?
            }
        };
        let top = if kind == "Series" {
            &mut data["series_details"]
        } else {
            &mut data
        };
        normalize_title(&kind, top);
        let mut dummy = item.clone();
        dummy["Name"] = top[if kind == "Movie" { "title" } else { "name" }].clone();
        dummy["OriginalTitle"] = top[if kind == "Movie" {
            "original_title"
        } else {
            "original_name"
        }]
        .clone();
        dummy["ProductionYear"] = json!(
            string(
                top,
                if kind == "Movie" {
                    "release_date"
                } else {
                    "first_air_date"
                }
            )
            .chars()
            .take(4)
            .collect::<String>()
        );
        dummy["Genres"] = top["genres"].clone();
        crate::douban::premerge_in_job(state, &dummy, top, context).await?;
        crate::translation::metadata_tree(state, &kind, &mut data, &string(&dummy, "Name")).await?;
        crate::translation::overview_fallback(state, &kind, &mut data, &scope).await?;
        let top = if kind == "Series" {
            &data["series_details"]
        } else {
            &data
        };
        let cast = crate::persons::process(state, &list(&top["credits"]["cast"]), &dummy).await?;
        for director in crate::nfo::directors(top) {
            crate::persons::upsert(state, &director).await?;
        }
        let defaults: Value =
            serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
        let ratings = crate::db::setting(&state.db, "rating_mapping")
            .await?
            .unwrap_or(defaults["DEFAULT_RATING_MAPPING"].clone());
        let priority = crate::db::setting(&state.db, "rating_priority")
            .await?
            .unwrap_or(defaults["DEFAULT_RATING_PRIORITY"].clone());
        let mut payload =
            crate::metadata_payload::build(&kind, top, Some(&data), None, &ratings, &priority);
        if kind == "Series" {
            payload["episode_group_id"] = data["episode_group_id"].clone();
            payload["credits"]["crew"] = json!(crate::nfo::directors(top));
        }
        if kind == "Series" && !scope_covers(&payload, &scope, &id) {
            return Err(ApiError::bad("TMDb 未提供本批完整目标季集元数据"));
        }
        check_active(state, &paths, context, cancel, true).await?;
        let mut records = crate::metadata_records::pending(&kind, &payload, &cast, &item)?;
        crate::metadata::ingest(state, &mut records, false).await?;
        (payload, cast)
    };
    if kind == "Series" && !scope_covers(&payload, &scope, &id) {
        return Err(ApiError::bad("本批目标季集缓存不完整"));
    }
    check_active(state, &paths, context, cancel, true).await?;
    if !patch_records.is_empty() {
        crate::metadata::ingest(state, &mut patch_records, false).await?;
    }
    let config = state.config.read().await.clone();
    let defaults: Value = serde_json::from_str(include_str!("../assets/defaults.json")).unwrap();
    let studio = crate::db::setting(&state.db, "studio_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_STUDIO_MAPPING"].clone());
    let keywords = crate::db::setting(&state.db, "keyword_mapping")
        .await?
        .unwrap_or(defaults["DEFAULT_KEYWORD_MAPPING"].clone());
    let options = crate::nfo_files::Options {
        config: &config,
        studio_mapping: &studio,
        keyword_mapping: &keywords,
        ..Default::default()
    };
    let nfo = write_nfo(
        state,
        &kind,
        &paths,
        &mut payload,
        &cast,
        &item,
        &options,
        roots,
        context,
        cancel,
    )
    .await?;
    let images = if cache_hit && media_root(&kind, &paths).join("poster.jpg").is_file() {
        0
    } else {
        match download_images(state, &kind, &id, &payload, &paths, roots, context, cancel).await {
            Ok(count) => count,
            Err(error) => {
                check_active(state, &paths, context, cancel, true).await?;
                tracing::warn!(reason=%error.1, "实时监控图片下载失败，保留 NFO 并继续入库通知");
                0
            }
        }
    };
    let notify = if kind == "Movie" {
        movie_files(&paths[0]).await?
    } else {
        let mut parents = BTreeSet::new();
        paths
            .iter()
            .filter(|p| parents.insert(p.parent().unwrap().to_path_buf()))
            .cloned()
            .collect()
    };
    Ok(Prepared {
        notify,
        metadata_dirs: if kind == "Series" {
            paths
                .iter()
                .filter_map(|p| p.parent().map(Path::to_path_buf))
                .collect()
        } else {
            vec![]
        },
        cache_hit,
        nfo,
        images,
    })
}
async fn movie_files(media_path: &Path) -> Result<Vec<PathBuf>> {
    let directory = media_path
        .parent()
        .ok_or_else(|| ApiError::bad("媒体目录无效"))?;
    static SKIP: LazyLock<regex::Regex> = LazyLock::new(|| {
        rx(
            r"(?i)(^|[\s._\-\[\]()])(sample|trailer|trailers|preview|teaser|extra|extras|behind|featurette|花絮|预告|样片|片花|彩蛋|特辑|访谈)($|[\s._\-\[\]()])",
        )
    });
    let mut files = vec![];
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.')
            && !SKIP.is_match(&name)
            && !rx(r"(?i)s\d{1,4}e\d{1,4}").is_match(&name)
            && [
                ".mp4", ".mkv", ".avi", ".ts", ".iso", ".rmvb", ".strm", ".mov", ".m2ts", ".webm",
            ]
            .contains(&crate::media_paths::extension(&name).as_str())
        {
            files.push(path);
        }
    }
    if media_path.is_file() && !files.iter().any(|p| p == media_path) {
        files.push(media_path.into());
    }
    files.sort();
    Ok(files)
}
#[allow(clippy::too_many_arguments)]
async fn write_nfo(
    state: &AppState,
    kind: &str,
    paths: &[PathBuf],
    payload: &mut Value,
    cast: &[Value],
    item: &Value,
    options: &crate::nfo_files::Options<'_>,
    roots: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<usize> {
    let config = options.config;
    crate::nfo_files::prepare(kind, payload, options);
    let mut plan = BTreeMap::new();
    if kind == "Movie" {
        for path in movie_files(&paths[0]).await? {
            plan.insert(
                path.with_extension("nfo"),
                crate::nfo::build(kind, payload, cast),
            );
        }
    } else {
        let root = media_root(kind, paths);
        ensure(root, roots).await?;
        plan.insert(
            root.join("tvshow.nfo"),
            crate::nfo::build(kind, payload, cast),
        );
        for path in paths {
            let (Some(s), Some(e)) = numbers(path) else {
                continue;
            };
            let season = list(&payload["seasons_details"])
                .into_iter()
                .find(|v| v["season_number"].as_i64() == Some(s));
            if let Some(season) = &season {
                plan.insert(
                    path.parent().unwrap().join("season.nfo"),
                    crate::nfo::build("Season", season, &[]),
                );
            }
            let mut episode = payload["episodes_details"]
                .get(format!("S{s}E{e}"))
                .cloned()
                .ok_or_else(|| ApiError::bad(format!("本次目标 S{s}E{e} 缺少元数据")))?;
            let limit = config["max_episode_actors_to_process"]
                .as_u64()
                .unwrap_or(0) as usize;
            let mut raw = list(&episode["credits"]["cast"])
                .into_iter()
                .chain(list(&episode["credits"]["guest_stars"]))
                .take(limit)
                .filter(|p| {
                    config["remove_actors_without_avatars"] == false
                        || !string(p, "profile_path").is_empty()
                })
                .collect::<Vec<_>>();
            if raw.is_empty() {
                raw = season
                    .as_ref()
                    .map(|season| list(&season["credits"]["cast"]))
                    .unwrap_or_default();
                raw.retain(|person| {
                    config["remove_actors_without_avatars"] == false
                        || !string(person, "profile_path").is_empty()
                });
            }
            if list(&episode["crew"]).is_empty()
                && list(&episode["credits"]["crew"]).is_empty()
                && let Some(season) = &season
            {
                episode["credits"]["crew"] = season["credits"]["crew"].clone();
            }
            let actors = if raw.is_empty() {
                cast.to_vec()
            } else {
                crate::cast::format(&raw, item, config)
            };
            plan.insert(
                path.with_extension("nfo"),
                crate::nfo::build(
                    "Episode",
                    &episode,
                    if actors.is_empty() { cast } else { &actors },
                ),
            );
        }
    }
    for path in plan.keys() {
        before_write(path, roots).await?;
    }
    let mut count = 0;
    for (path, xml) in plan {
        let current = check_active(state, paths, context, cancel, true).await?;
        before_write(&path, &current.roots).await?;
        before_write(&path, roots).await?;
        if tokio::fs::read_to_string(&path).await.ok().as_deref() != Some(&xml) {
            tokio::fs::write(path, xml).await?;
            count += 1;
        }
    }
    Ok(count)
}
#[allow(clippy::too_many_arguments)]
async fn download_images(
    state: &AppState,
    kind: &str,
    id: &str,
    payload: &Value,
    paths: &[PathBuf],
    roots: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<usize> {
    check_active(state, paths, context, cancel, true).await?;
    let root = media_root(kind, paths);
    ensure(root, roots).await?;
    let config = state.config.read().await.clone();
    let endpoint = format!("/{}/{id}", if kind == "Movie" { "movie" } else { "tv" });
    let base = crate::tmdb::get(state, &endpoint, &[("append_to_response", "".into())])
        .await
        .unwrap_or_default();
    let original = base["original_language"].as_str().unwrap_or("en");
    let mut images = json!({});
    for languages in
        crate::images::strategies(original, config["tmdb_image_language_preference"] == "zh")
    {
        check_active(state, paths, context, cancel, true).await?;
        let response = crate::tmdb::get(
            state,
            &endpoint,
            &[
                (
                    "append_to_response",
                    if kind == "Movie" {
                        "images"
                    } else {
                        "images,seasons"
                    }
                    .into(),
                ),
                ("include_image_language", languages),
            ],
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(reason=%error.1, "实时监控图片语言候选查询失败，继续下一候选");
                continue;
            }
        };
        if !list(&response["images"]["posters"]).is_empty() {
            images = response;
            break;
        }
    }
    let mut count = 0;
    let files = if kind == "Movie" {
        movie_files(&paths[0]).await?
    } else {
        paths.to_vec()
    };
    for (path, image) in image_plan(kind, root, &images, payload, &files) {
        check_active(state, paths, context, cancel, true).await?;
        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
        {
            continue;
        }
        before_write(&path, roots).await?;
        let bytes = match crate::images::fetch(state, &image, context).await {
            Ok(bytes) => bytes,
            Err(error) => {
                check_active(state, paths, context, cancel, true).await?;
                tracing::warn!(file=%path.display(), reason=%error.1, "实时监控图片下载失败，继续处理其它图片");
                continue;
            }
        };
        let current = check_active(state, paths, context, cancel, true).await?;
        before_write(&path, &current.roots).await?;
        before_write(&path, roots).await?;
        match tokio::fs::write(&path, bytes).await {
            Ok(()) => count += 1,
            Err(error) => {
                tracing::warn!(file=%path.display(), %error, "实时监控图片写入失败，继续处理其它图片")
            }
        }
    }
    if kind == "Series" {
        check_active(state, paths, context, cancel, true).await?;
        count += crate::video_images::missing(state, &files, payload, Some(context)).await?;
    } else if kind == "Movie" {
        check_active(state, paths, context, cancel, true).await?;
        if images["runtime"].is_null() {
            images["runtime"] = payload["runtime"].clone();
        }
        count += crate::video_images::movie_missing(state, &paths[0], root, &images, Some(context))
            .await?;
    }
    Ok(count)
}
pub fn image_plan(
    kind: &str,
    root: &Path,
    images: &Value,
    payload: &Value,
    files: &[PathBuf],
) -> BTreeMap<PathBuf, String> {
    let mut plan = crate::images::plan(kind, root, images, payload, files);
    if kind == "Series" {
        for file in files {
            let (Some(s), Some(e)) = numbers(file) else {
                continue;
            };
            if let Some(still) = payload["episodes_details"][format!("S{s}E{e}")]["still_path"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                plan.insert(
                    file.with_file_name(format!(
                        "{}-thumb.jpg",
                        file.file_stem().unwrap().to_string_lossy()
                    )),
                    still.into(),
                );
            }
        }
    }
    plan
}
pub async fn refresh_files(
    state: &AppState,
    paths: &[PathBuf],
    roots: &[PathBuf],
    metadata: bool,
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let current = check_active(state, &[], context, cancel, false).await?;
    let config = state.config.read().await.clone();
    if string(&config, "emby_server_url").is_empty() || string(&config, "emby_api_key").is_empty() {
        return Ok(());
    }
    let mut directories = BTreeSet::new();
    for path in paths {
        let directory = if metadata {
            path.clone()
        } else {
            path.parent()
                .ok_or_else(|| ApiError::bad("媒体目录无效"))?
                .into()
        };
        ensure(&directory, &current.roots).await?;
        directories.insert(ensure(&directory, roots).await?);
    }
    crate::strm::with_optional_preserve(state, || async {
        let mut failure = None;
        for mut directory in directories {
            let mut directory_failure = None;
            for _ in 0..if metadata { 1 } else { 4 } {
                if !roots.iter().any(|root| directory.starts_with(root)) { break; }
                let current = check_active(state, &[], context, cancel, false).await?;
                ensure(&directory, &current.roots).await?;
                let path = map_emby(&directory, &config);
                let response = state.emby(reqwest::Method::GET, "Items", &[
                    ("Recursive", "true".into()),
                    ("Path", path.clone()),
                    ("Fields", if metadata { "Id,Path,Name,Type" } else { "Id,Path,Name" }.into()),
                ], None).await;
                match response {
                    Ok(response) => {
                        let items = list(&response["Items"]);
                        let item = if metadata {
                            items.iter().find(|v| crate::monitor::normalized_path(Path::new(&string(v, "Path"))) == crate::monitor::normalized_path(Path::new(&path)))
                        } else {
                            items.first()
                        };
                        if let Some(item) = item {
                            let current = check_active(state, &[], context, cancel, false).await?;
                            ensure(&directory, &current.roots).await?;
                            let response = state.http.post(format!("{}/Items/{}/Refresh", string(&config, "emby_server_url").trim_end_matches('/'), string(item, "Id")))
                                .header("X-Emby-Token", string(&config, "emby_api_key"))
                                .query(&[
                                    ("Recursive", "true"),
                                    ("MetadataRefreshMode", if metadata { "FullRefresh" } else { "Default" }),
                                    ("ImageRefreshMode", "Default"),
                                    ("ReplaceAllImages", "false"),
                                    ("ReplaceAllMetadata", "false"),
                                ]).send().await.and_then(reqwest::Response::error_for_status);
                            match response {
                                Ok(_) => { directory_failure = None; break; },
                                Err(error) => {
                                    tracing::warn!(path=%path, error=%error.without_url(), "Emby 目录刷新失败");
                                    directory_failure = Some(ApiError::bad("Emby 目录刷新失败"));
                                }
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(path=%path, reason=%error.1, "Emby 目录查询失败，继续处理");
                        directory_failure = Some(error);
                    }
                }
                if metadata || !directory.pop() { break; }
            }
            if directory_failure.is_some() { failure = directory_failure; }
        }
        if let Some(error) = failure { Err(error) } else { Ok(()) }
    }).await
}
pub fn map_emby(path: &Path, config: &Value) -> String {
    let value = path.to_string_lossy().trim().replace('\\', "/");
    let local = string(config, "local_strm_root")
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    let cd2 = string(config, "cd2_media_root")
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    if !local.is_empty() && !cd2.is_empty() {
        if value == cd2 {
            return local;
        }
        if let Some(relative) = value.strip_prefix(&format!("{cd2}/")) {
            return format!("{local}/{relative}");
        }
    }
    value
}
