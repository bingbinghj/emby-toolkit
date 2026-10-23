use crate::{cast, error::Result, media_edit::list, nfo, state::string};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

#[derive(Default)]
pub struct Options<'a> {
    pub force: bool,
    pub series_refresh: bool,
    pub seasons: &'a [i64],
    pub episodes: &'a [(i64, i64)],
    pub config: &'a Value,
    pub studio_mapping: &'a Value,
    pub keyword_mapping: &'a Value,
}
fn element(xml: &str, tag: &str) -> String {
    regex::Regex::new(&format!(r"(?is)<{tag}[^>]*>(.*?)</{tag}>"))
        .unwrap()
        .captures(xml)
        .map(|c| c[1].trim().to_string())
        .unwrap_or_default()
}
async fn write(path: &Path, content: &str, force: bool) -> Result<bool> {
    if let Ok(old) = tokio::fs::read_to_string(path).await
        && (old == content
            || (!force
                && ["title", "plot"]
                    .iter()
                    .all(|tag| element(&old, tag) == element(content, tag))))
    {
        return Ok(false);
    }
    tokio::fs::write(path, content).await?;
    Ok(true)
}
fn season_dir(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    static SEASON: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(?:^|[/\s.\-_\[(])(?:Season\s*|S|第)\s*\d{1,4}(?:季)?(?:$|[/\s.\-_\])])",
        )
        .unwrap()
    });
    static SPECIAL: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(specials?|sp|ova|oad|extras?|特别篇|特別篇|番外(?:篇)?|外传|外傳|第\s*0+\s*季|season\s*0+|s0+)$").unwrap()
    });
    SPECIAL.is_match(&name) || SEASON.is_match(&name)
}
fn episode_number(path: &Path) -> Option<(i64, i64)> {
    static EPISODE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)s(\d{1,4})e(\d{1,4})").unwrap());
    let captures = EPISODE.captures(path.file_name()?.to_str()?)?;
    Some((captures[1].parse().ok()?, captures[2].parse().ok()?))
}
async fn files(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut result = vec![];
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            if kind.is_file() {
                result.push(entry.path());
            }
            if recursive && kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    result.sort();
    Ok(result)
}
pub(crate) fn prepare(kind: &str, data: &mut Value, options: &Options<'_>) {
    if options.config["studio_to_chinese"] == true {
        for (field, ids) in [
            ("production_companies", "company_ids"),
            ("networks", "network_ids"),
        ] {
            if kind != "Series" && (kind != "Movie" || field != "production_companies") {
                continue;
            }
            let studios = list(&data[field])
                .into_iter()
                .filter_map(|mut studio| {
                    let name = string(&studio, "name").trim().to_lowercase();
                    let mapping = list(options.studio_mapping).into_iter().find(|m| {
                        !string(m, "label").is_empty()
                            && (list(&m[ids]).contains(&studio["id"])
                                || list(&m["en"]).iter().any(|v| {
                                    v.as_str().is_some_and(|s| s.trim().to_lowercase() == name)
                                }))
                    });
                    studio["name"] = mapping?["label"].clone();
                    Some(studio)
                })
                .collect::<Vec<_>>();
            data[field] = json!(studios);
        }
    }
    if kind == "Series" {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        let networks = list(&data["networks"])
            .into_iter()
            .chain(list(&data["production_companies"]))
            .filter(|n| {
                let id = n["id"].as_i64().filter(|n| *n != 0);
                let name = string(n, "name");
                let duplicate = id.is_some_and(|id| !ids.insert(id))
                    | (!name.is_empty() && !names.insert(name.clone()));
                !duplicate && (id.is_some() || !name.is_empty())
            })
            .collect::<Vec<_>>();
        data["networks"] = json!(networks);
        data.as_object_mut().unwrap().remove("production_companies");
    }
    if options.config["keyword_to_tags"] == true {
        let keywords = if data["keywords"].is_object() {
            let primary = list(&data["keywords"]["keywords"]);
            if primary.is_empty() {
                list(&data["keywords"]["results"])
            } else {
                primary
            }
        } else {
            list(&data["keywords"])
        };
        let tags = list(options.keyword_mapping)
            .iter()
            .filter(|m| keywords.iter().any(|k| list(&m["ids"]).contains(&k["id"])))
            .filter_map(|m| m["label"].as_str().map(str::to_string))
            .collect::<BTreeSet<_>>();
        data["_mapped_chinese_tags"] = json!(tags);
    }
}
pub async fn sync(
    kind: &str,
    item: &Value,
    package: &Value,
    cast: &[Value],
    options: &Options<'_>,
) -> Result<usize> {
    let raw = string(item, "Path");
    if raw.is_empty() || raw.starts_with("http") {
        return Ok(0);
    }
    let path = Path::new(&raw);
    let directory = if path.is_file() || path.extension().is_some() {
        path.parent().unwrap_or(Path::new(""))
    } else {
        path
    };
    let root = if season_dir(directory) {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };
    if !root.is_dir() {
        return Ok(0);
    }
    let mut data = package.clone();
    prepare(kind, &mut data, options);
    let mut written = 0;
    match kind {
        "Movie" => {
            static SKIP: LazyLock<regex::Regex> = LazyLock::new(|| {
                regex::Regex::new(r"(?i)(^|[\s._\-\[\]()])(sample|trailer|trailers|preview|teaser|extra|extras|behind|featurette|花絮|预告|样片|片花|彩蛋|特辑|访谈)($|[\s._\-\[\]()])").unwrap()
            });
            let mut movies = files(directory, false)
                .await?
                .into_iter()
                .filter(|p| {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    let extension = p
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    [
                        "mp4", "mkv", "avi", "ts", "iso", "rmvb", "strm", "mov", "m2ts", "webm",
                    ]
                    .contains(&extension.as_str())
                        && !name.starts_with('.')
                        && episode_number(p).is_none()
                        && !SKIP.is_match(name)
                })
                .collect::<Vec<_>>();
            if path.is_file() && !movies.contains(&path.to_path_buf()) {
                movies.push(path.into());
            }
            if movies.is_empty() {
                movies.push(path.into());
            }
            let xml = nfo::build("Movie", &data, cast);
            for movie in movies {
                written +=
                    usize::from(write(&movie.with_extension("nfo"), &xml, options.force).await?);
            }
        }
        "Series" => {
            let top = root.join("tvshow.nfo");
            let mut cast = cast.to_vec();
            if options.series_refresh
                && let Ok(old) = tokio::fs::read_to_string(&top).await
            {
                for (tag, key) in [
                    ("title", "name"),
                    ("originaltitle", "original_name"),
                    ("sorttitle", "sorttitle"),
                ] {
                    let value = element(&old, tag);
                    if !value.is_empty() {
                        data[key] = json!(
                            quick_xml::escape::unescape(&value)
                                .unwrap_or(value.as_str().into())
                                .into_owned()
                        );
                    }
                }
                if cast.is_empty() {
                    let pattern = regex::Regex::new(r"(?is)<actor>(.*?)</actor>").unwrap();
                    cast=pattern.captures_iter(&old).map(|c|json!({"name":element(&c[1],"name"),"character":element(&c[1],"role"),"order":element(&c[1],"order"),"profile_path":element(&c[1],"thumb"),"tmdb_id":element(&c[1],"tmdbid")})).collect();
                }
            }
            written +=
                usize::from(write(&top, &nfo::build("Series", &data, &cast), options.force).await?);
            let episodes = if let Some(map) = data["episodes_details"].as_object() {
                map.values().cloned().collect::<Vec<_>>()
            } else {
                list(&data["episodes_details"])
            };
            if episodes.is_empty() {
                return Ok(written);
            }
            let seasons = list(&data["seasons_details"]);
            let mut directories = HashSet::new();
            for file in files(root, true).await? {
                let extension = file
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !["mp4", "mkv", "avi", "ts", "iso", "rmvb", "strm"].contains(&extension.as_str())
                {
                    continue;
                }
                let Some(key) = episode_number(&file) else {
                    continue;
                };
                if (!options.seasons.is_empty() && !options.seasons.contains(&key.0))
                    || (!options.episodes.is_empty() && !options.episodes.contains(&key))
                {
                    continue;
                }
                let directory = file.parent().unwrap();
                let season = seasons
                    .iter()
                    .find(|s| s["season_number"].as_i64() == Some(key.0));
                if directories.insert(directory.to_path_buf())
                    && let Some(season) = season
                {
                    written += usize::from(
                        write(
                            &directory.join("season.nfo"),
                            &nfo::build("Season", season, &[]),
                            options.force,
                        )
                        .await?,
                    );
                }
                let mut episode=episodes.iter().find(|e|e["season_number"].as_i64()==Some(key.0) && e["episode_number"].as_i64()==Some(key.1)).cloned().unwrap_or(json!({"season_number":key.0,"episode_number":key.1,"name":format!("第 {} 集",key.1),"overview":"","id":""}));
                let credits = season
                    .map(|s| {
                        s.get("credits")
                            .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
                            .unwrap_or(&s["aggregate_credits"])
                    })
                    .unwrap_or(&Value::Null);
                if list(&episode["crew"]).is_empty() && list(&episode["credits"]["crew"]).is_empty()
                {
                    episode["credits"]["crew"] = credits["crew"].clone();
                }
                let limit = options.config["max_episode_actors_to_process"]
                    .as_u64()
                    .unwrap_or(0) as usize;
                let filtered = |actors: Vec<Value>| {
                    actors
                        .into_iter()
                        .filter(|p| {
                            options.config["remove_actors_without_avatars"] == false
                                || p["profile_path"].as_str().is_some_and(|s| !s.is_empty())
                        })
                        .collect::<Vec<_>>()
                };
                let actors = filtered(
                    list(&episode["credits"]["cast"])
                        .into_iter()
                        .chain(list(&episode["credits"]["guest_stars"]))
                        .take(limit)
                        .collect(),
                );
                let actors = if !actors.is_empty() {
                    cast::format(&actors, item, options.config)
                } else {
                    let season_cast = filtered(list(&credits["cast"]));
                    if season_cast.is_empty() {
                        cast.clone()
                    } else {
                        cast::format(&season_cast, item, options.config)
                    }
                };
                written += usize::from(
                    write(
                        &file.with_extension("nfo"),
                        &nfo::build("Episode", &episode, &actors),
                        options.force,
                    )
                    .await?,
                );
            }
        }
        "Season" => {
            written += usize::from(
                write(
                    &directory.join("season.nfo"),
                    &nfo::build("Season", &data, cast),
                    options.force,
                )
                .await?,
            )
        }
        "Episode" => {
            written += usize::from(
                write(
                    &path.with_extension("nfo"),
                    &nfo::build("Episode", &data, cast),
                    options.force,
                )
                .await?,
            )
        }
        _ => (),
    }
    Ok(written)
}
