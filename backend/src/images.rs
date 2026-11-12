use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
    tmdb,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(test)]
#[path = "independent_switch_tests.rs"]
mod independent_switch_tests;

pub fn strategies(original: &str, chinese: bool) -> Vec<String> {
    let mut result = vec![];
    if chinese {
        result.extend(["zh-CN", "zh-TW,zh,zh-HK,zh-SG", "en,null"].map(str::to_string));
        if !["zh", "cn", "tw", "hk", "en"].contains(&original) {
            result.push(original.into());
        }
    } else {
        if ["zh", "cn", "tw", "hk"].contains(&original) {
            result.push("zh-CN,zh-HK,zh-TW,zh,cn".into());
        } else if original != "en" {
            result.push(original.into());
        }
        result.push("en,null".into());
        if !["zh", "cn", "tw", "hk"].contains(&original) {
            result.push("zh-CN,zh-HK,zh-TW,zh".into());
        }
    }
    result
}
pub fn plan(
    kind: &str,
    root: &Path,
    data: &Value,
    aggregate: &Value,
    files: &[PathBuf],
) -> BTreeMap<PathBuf, String> {
    let mut result = BTreeMap::new();
    let images = &data["images"];
    for (field, name) in [("posters", "poster.jpg"), ("logos", "clearlogo.png")] {
        if let Some(path) = images[field][0]["file_path"].as_str() {
            result.insert(root.join(name), path.into());
        }
    }
    if let Some(path) = images["backdrops"][0]["file_path"]
        .as_str()
        .or_else(|| data["backdrop_path"].as_str())
        .filter(|s| !s.is_empty())
    {
        for name in ["fanart.jpg", "landscape.jpg"] {
            result.insert(root.join(name), path.into());
        }
    }
    if kind == "Series" {
        let seasons = aggregate
            .get("seasons_details")
            .map(list)
            .unwrap_or_else(|| list(&data["seasons"]));
        for season in seasons {
            if let (Some(number), Some(poster)) = (
                season["season_number"].as_i64(),
                season["poster_path"].as_str().filter(|s| !s.is_empty()),
            ) {
                result.insert(
                    root.join(format!("season{number:02}-poster.jpg")),
                    poster.into(),
                );
            }
        }
        let episodes = aggregate["episodes_details"]
            .as_object()
            .map(|o| o.values().cloned().collect::<Vec<_>>())
            .unwrap_or_else(|| list(&aggregate["episodes_details"]));
        let pattern = regex::Regex::new(r"(?i)s(\d{1,4})e(\d{1,4})").unwrap();
        for file in files {
            let extension = file
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !["mp4", "mkv", "avi", "ts", "iso", "rmvb", "strm"].contains(&extension.as_str()) {
                continue;
            }
            let Some(c) = pattern.captures(file.file_name().and_then(|s| s.to_str()).unwrap_or(""))
            else {
                continue;
            };
            let (Some(season), Some(number)) = (c[1].parse::<i64>().ok(), c[2].parse::<i64>().ok())
            else {
                continue;
            };
            if let Some(poster) = episodes
                .iter()
                .find(|e| {
                    e["season_number"].as_i64() == Some(season)
                        && e["episode_number"].as_i64() == Some(number)
                })
                .and_then(|e| e["still_path"].as_str())
                .filter(|s| !s.is_empty())
            {
                let filename = format!("{}-thumb.jpg", file.file_stem().unwrap().to_string_lossy());
                result.insert(file.with_file_name(filename), poster.into());
            }
        }
    }
    result
}
pub(crate) async fn fetch(
    state: &AppState,
    image: &str,
    context: &crate::jobs::JobContext,
) -> Result<Vec<u8>> {
    let _slot = crate::strm::slot(state, "secondary", Some(context)).await?;
    let base = std::env::var("ETK_TMDB_IMAGE_BASE")
        .unwrap_or_else(|_| "https://image.tmdb.org/t/p/original".into());
    tokio::select! {
        _ = context.cancelled() => Err(ApiError(axum::http::StatusCode::CONFLICT, "任务已取消".into())),
        result = async {
            Ok(state.http.get(format!("{}{image}", base.trim_end_matches('/')))
                .timeout(std::time::Duration::from_secs(15))
                .send().await?.error_for_status()?.bytes().await?.to_vec())
        } => result,
    }
}

pub async fn download(
    state: &AppState,
    kind: &str,
    id: &str,
    item: &Value,
    aggregate: &Value,
    context: &crate::jobs::JobContext,
) -> Result<usize> {
    Ok(
        download_report(state, kind, id, item, aggregate, context).await?["downloaded"]
            .as_u64()
            .unwrap_or(0) as usize,
    )
}
pub async fn download_report(
    state: &AppState,
    kind: &str,
    id: &str,
    item: &Value,
    aggregate: &Value,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    download_with_overwrite_report(state, kind, id, item, aggregate, context, &[]).await
}
pub async fn download_with_overwrite_report(
    state: &AppState,
    kind: &str,
    id: &str,
    item: &Value,
    aggregate: &Value,
    context: &crate::jobs::JobContext,
    force_overwrite_episodes: &[String],
) -> Result<Value> {
    let media_name = string(item, "Name");
    tracing::info!(
        "图片资产下载开始：类型 {kind}，TMDb {id}，名称 {}",
        string(item, "Name")
    );
    let raw = string(item, "Path");
    if raw.is_empty() || raw.starts_with("http") {
        tracing::info!("图片资产下载跳过：《{media_name}》，TMDb {id}，缺少本地路径");
        return Ok(json!({"downloaded":0,"warnings":0,"warning_details":[]}));
    }
    let path = Path::new(&raw);
    let directory = if path.is_dir() {
        path
    } else if path.is_file() || path.extension().is_some() {
        path.parent().unwrap_or(Path::new(""))
    } else {
        path
    };
    let season=regex::Regex::new(r"(?i)^(?:season\s*\d+|s\d+|specials?|sp|ova|oad|extras?|特别篇|特別篇|番外(?:篇)?|外传|外傳|第\s*\d+\s*季)$").unwrap();
    let root = if season.is_match(directory.file_name().and_then(|s| s.to_str()).unwrap_or("")) {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };
    if !root.is_dir() {
        tracing::info!("图片资产下载跳过：《{media_name}》，TMDb {id}，本地目录不存在");
        return Ok(json!({"downloaded":0,"warnings":0,"warning_details":[]}));
    }
    let config = state.config.read().await.clone();
    let endpoint = format!("/{}/{id}", if kind == "Movie" { "movie" } else { "tv" });
    let base = tmdb::get(state, &endpoint, &[("append_to_response", "".into())])
        .await
        .unwrap_or_default();
    let original = base["original_language"].as_str().unwrap_or("en");
    let mut data = None;
    let mut query_succeeded = false;
    let mut query_errors = vec![];
    let mut warning_details = vec![];
    for languages in strategies(
        original,
        config["tmdb_image_language_preference"]
            .as_str()
            .unwrap_or("zh")
            == "zh",
    ) {
        tracing::debug!("图片语言策略查询：《{media_name}》，TMDb {id}，语言 {languages}");
        match tmdb::get(
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
        .await
        {
            Ok(response) => {
                query_succeeded = true;
                if !list(&response["images"]["posters"]).is_empty() {
                    data = Some(response);
                    break;
                }
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => query_errors.push(crate::logs::redact(&error.1)),
        }
    }
    let Some(data) = data else {
        if !query_succeeded && !query_errors.is_empty() {
            let reason = format!(
                "《{media_name}》（TMDb {id}）图片查询失败：所有语言策略均未成功，{}",
                query_errors[0]
            );
            tracing::warn!("{reason}");
            warning_details.push(reason);
        }
        tracing::info!(
            "图片资产下载跳过：《{media_name}》，TMDb {id}，未取得可用海报，未生成下载计划"
        );
        return Ok(
            json!({"downloaded":0,"warnings":warning_details.len(),"warning_details":warning_details}),
        );
    };
    let mut pending = vec![root.to_path_buf()];
    let mut files = vec![];
    if kind == "Series" {
        while let Some(directory) = pending.pop() {
            let mut entries = tokio::fs::read_dir(directory).await?;
            while let Some(entry) = entries.next_entry().await? {
                let kind = entry.file_type().await?;
                if kind.is_file() {
                    files.push(entry.path());
                } else if kind.is_dir() {
                    pending.push(entry.path());
                }
            }
        }
    }
    let mut count = 0;
    let plan = plan(kind, root, &data, aggregate, &files);
    let planned = plan.len();
    tracing::info!(
        "图片资产下载计划：《{media_name}》，TMDb {id}，目标 {} 个",
        plan.len()
    );
    use futures_util::{FutureExt, StreamExt, stream};
    let episode = regex::Regex::new(r"(?i)s(\d{1,4})e(\d{1,4})").unwrap();
    let limit = crate::strm::thread_limit(state, "secondary").await;
    let pending = stream::iter(plan)
        .map(|(path, image)| {
            let overwrite = episode
                .captures(
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(""),
                )
                .is_some_and(|numbers| {
                    force_overwrite_episodes.contains(&format!(
                        "S{}E{}",
                        numbers[1].parse::<i64>().unwrap(),
                        numbers[2].parse::<i64>().unwrap()
                    ))
                });
            let media_name = media_name.clone();
            async move {
        context.checkpoint().await?;
        if !overwrite && tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
        {
            tracing::debug!(
                "图片下载跳过：《{media_name}》，TMDb {id}，文件 {} 已存在",
                path.display()
            );
            return Ok((0, 1, None));
        }
        tracing::debug!(
            "图片下载开始：《{media_name}》，TMDb {id}，文件 {}",
            path.display()
        );
        let result = async {
            let bytes = fetch(state, &image, context).await?;
            context.checkpoint().await?;
            tokio::fs::write(&path, &bytes).await?;
            Ok::<_, ApiError>(bytes.len())
        }
        .await;
        match result {
            Ok(size) => {
                tracing::debug!(
                    "图片下载完成：《{media_name}》，TMDb {id}，文件 {}，{size} 字节",
                    path.display()
                );
                Ok((1, 0, None))
            }
            Err(error) if context.is_cancelled() => Err(error),
            Err(error) => {
                let reason = format!("《{media_name}》（TMDb {id}）图片下载失败：文件 {}，{}，继续处理其它图片", path.display(), crate::logs::redact(&error.1));
                tracing::warn!("{reason}");
                Ok((0, 0, Some(reason)))
            },
        }
        }.boxed()
        })
        .buffer_unordered(limit);
    tokio::pin!(pending);
    let mut cancelled = None;
    let mut skipped = 0_usize;
    let mut failed = 0_usize;
    while let Some(result) = pending.next().await {
        match result {
            Ok((done, existing, error)) => {
                count += done;
                skipped += existing;
                if let Some(reason) = error {
                    failed += 1;
                    warning_details.push(reason);
                }
            }
            Err(error) => cancelled = Some(error),
        }
    }
    if let Some(error) = cancelled {
        return Err(error);
    }
    let downloaded = count;
    let video_report = if kind == "Series" {
        crate::video_images::missing_with_overwrite_report(
            state,
            &files,
            aggregate,
            Some(context),
            force_overwrite_episodes,
        )
        .await?
    } else if kind == "Movie" {
        let mut movie_data = data.clone();
        if movie_data["runtime"].is_null() {
            movie_data["runtime"] = aggregate["runtime"].clone();
        }
        crate::video_images::movie_missing_report(state, path, root, &movie_data, Some(context))
            .await?
    } else {
        json!({})
    };
    count += video_report["captured"].as_u64().unwrap_or(0) as usize;
    warning_details.extend(
        list(&video_report["warning_details"])
            .into_iter()
            .filter_map(|reason| reason.as_str().map(str::to_owned)),
    );
    tracing::info!(
        "图片资产处理结束：《{media_name}》，TMDb {id}，下载计划 {planned} 个，下载成功 {downloaded} 个，已存在跳过 {skipped} 个，下载失败 {failed} 个，视频补图 {} 个，共新增 {count} 个",
        count - downloaded
    );
    Ok(
        json!({"downloaded":count,"warnings":warning_details.len(),"warning_details":warning_details}),
    )
}
