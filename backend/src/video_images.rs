use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
    state::string,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub fn capture_options(info: &Value, filename: &str, episode: &Value) -> (f64, String) {
    let source = crate::media_info::container(info).unwrap_or(info);
    let duration = source["RunTimeTicks"]
        .as_f64()
        .filter(|n| *n > 0.0)
        .map(|n| n / 10_000_000.0)
        .or_else(|| {
            episode["runtime"]
                .as_f64()
                .filter(|n| *n > 0.0)
                .map(|n| n * 60.0)
        });
    let timestamp = duration.map_or(600.0, |seconds| {
        (seconds * 0.15)
            .floor()
            .max(60.0)
            .min((seconds - 10.0).floor())
            .max(0.0)
    });
    let video = list(&source["MediaStreams"])
        .into_iter()
        .find(|s| s["Type"] == "Video")
        .unwrap_or(Value::Null);
    let description = video.to_string().to_lowercase();
    let hdr = [
        "hdr",
        "hlg",
        "smpte2084",
        "bt2020",
        "dolbyvision",
        "dovi",
        "p5",
        "profile 5",
    ]
    .iter()
    .any(|term| description.contains(term))
        || regex::Regex::new(r"(?i)(hdr|hlg|dv|dovi|dolby[.\s_-]*vision)")
            .unwrap()
            .is_match(filename);
    let mut filters = "scale=640:360:force_original_aspect_ratio=increase,crop=640:360".to_string();
    if hdr {
        filters.push_str(",zscale=t=linear:npl=100,format=gbrpf32le,tonemap=tonemap=hable:desat=0,zscale=p=bt709:t=bt709:m=bt709:r=tv");
    }
    filters.push_str(",format=yuv420p");
    (timestamp, filters)
}

pub async fn missing(
    state: &AppState,
    files: &[PathBuf],
    aggregate: &Value,
    context: Option<&JobContext>,
) -> Result<usize> {
    missing_with_overwrite(state, files, aggregate, context, &[]).await
}
pub async fn missing_with_overwrite(
    state: &AppState,
    files: &[PathBuf],
    aggregate: &Value,
    context: Option<&JobContext>,
    force_overwrite_episodes: &[String],
) -> Result<usize> {
    if state.config.read().await["extract_thumb"] != true {
        return Ok(0);
    }
    let numbers = regex::Regex::new(r"(?i)s(\d{1,4})e(\d{1,4})").unwrap();
    let mut tasks = vec![];
    for file in files {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        if ![".strm", ".mkv", ".mp4", ".avi", ".ts", ".iso", ".rmvb"]
            .contains(&crate::media_paths::extension(file.to_str().unwrap_or("")).as_str())
        {
            continue;
        }
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let Some(captures) = numbers.captures(&name) else {
            continue;
        };
        let season: u32 = captures[1].parse().unwrap_or_default();
        let episode: u32 = captures[2].parse().unwrap_or_default();
        let overwrite = force_overwrite_episodes.contains(&format!("S{season}E{episode}"));
        let episodes = &aggregate["episodes_details"];
        let keyed = &episodes[format!("S{season}E{episode}")];
        let data = if keyed.is_object() {
            keyed.clone()
        } else {
            episodes
                .as_object()
                .map(|episodes| episodes.values().cloned().collect::<Vec<_>>())
                .unwrap_or_else(|| list(episodes))
                .into_iter()
                .find(|data| {
                    data["season_number"].as_u64() == Some(season.into())
                        && data["episode_number"].as_u64() == Some(episode.into())
                })
                .unwrap_or(Value::Null)
        };
        if !string(&data, "still_path").is_empty() {
            continue;
        }
        let target = file.with_file_name(format!(
            "{}-thumb.jpg",
            file.file_stem().unwrap_or_default().to_string_lossy()
        ));
        if !overwrite && target.is_file() {
            continue;
        }
        tasks.push((season, episode, file.clone(), target, data, overwrite));
    }
    if tasks.is_empty() {
        return Ok(0);
    }
    tasks.sort_by_key(|(season, episode, _, _, _, _)| (*season, *episode));
    use futures_util::{FutureExt, StreamExt, stream};
    let limit = crate::strm::thread_limit(state, "main").await;
    let pending = stream::iter(tasks.into_iter().enumerate())
        .map(|(index, (_, _, file, target, data, overwrite))| {
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            tracing::info!(file = %name, "[视频截图] 开始处理缺失图片");
            async move {
                let result = async {
                    if let Some(context) = context {
                        context.checkpoint().await?;
                    }
                    match capture(state, &file, &target, &data, context, overwrite).await {
                        Ok(done) => Ok((usize::from(done), None)),
                        Err(error) if context.is_some_and(JobContext::is_cancelled) => Err(error),
                        Err(error) => Ok((0, Some(error.1))),
                    }
                }
                .await;
                (index, name, result)
            }
            .boxed()
        })
        .buffer_unordered(limit);
    tokio::pin!(pending);
    let mut captured = 0;
    let mut cancelled = None;
    let mut finished = std::collections::BTreeMap::new();
    let mut next = 0;
    while let Some((index, name, result)) = pending.next().await {
        finished.insert(index, (name, result));
        while let Some((name, result)) = finished.remove(&next) {
            match result {
                Ok((done, error)) => {
                    captured += done;
                    if let Some(error) = error {
                        tracing::warn!(file = %name, error = %error, "[视频截图] 截取失败，保留现有图片");
                    } else if done > 0 {
                        tracing::info!(file = %name, "[视频截图] 分集图已生成");
                    }
                }
                Err(error) => cancelled = Some(error),
            }
            next += 1;
        }
    }
    if let Some(error) = cancelled {
        return Err(error);
    }
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    Ok(captured)
}

pub async fn movie_missing(
    state: &AppState,
    file: &Path,
    root: &Path,
    data: &Value,
    context: Option<&JobContext>,
) -> Result<usize> {
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    let landscape = root.join("landscape.jpg");
    let fanart = root.join("fanart.jpg");
    if state.config.read().await["extract_thumb"] != true
        || !file.is_file()
        || landscape.exists()
        || fanart.exists()
        || !string(data, "backdrop_path").is_empty()
        || list(&data["images"]["backdrops"])
            .iter()
            .any(|image| !string(image, "file_path").is_empty())
    {
        return Ok(0);
    }
    match capture(state, file, &landscape, data, context, false).await {
        Ok(true) => {
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            if !fanart.exists()
                && let Err(error) = tokio::fs::copy(&landscape, &fanart).await
            {
                tracing::warn!(error = %error, "[视频截图] 电影备用背景图复制失败：{}", fanart.display());
            }
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            tracing::info!("[视频截图] 电影背景图已生成：{}", landscape.display());
            Ok(1)
        }
        Ok(false) => Ok(0),
        Err(error) if context.is_some_and(JobContext::is_cancelled) => Err(error),
        Err(error) => {
            tracing::warn!(error = %error.1, "[视频截图] 电影截取失败，保留现有图片：{}", file.display());
            Ok(0)
        }
    }
}

async fn capture(
    state: &AppState,
    file: &Path,
    target: &Path,
    data: &Value,
    context: Option<&JobContext>,
    overwrite: bool,
) -> Result<bool> {
    let stage = target.with_file_name(format!(".etk-thumb-{}.jpg", uuid::Uuid::new_v4().simple()));
    let result = {
        let operation = async {
            let (pc, sha, _) =
                crate::fingerprints::lookup(state, file.to_str().unwrap_or("")).await?;
            let pc = pc.ok_or_else(|| ApiError::bad("视频截图缺少可验证115 PC"))?;
            let info: Option<Value> = sqlx::query_scalar(
                "SELECT mediainfo_json FROM p115_mediainfo_cache WHERE UPPER(sha1)=UPPER($1)",
            )
            .bind(sha)
            .fetch_optional(&state.db)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("读取截图媒体信息缓存失败，使用默认截图位置：{error}");
                None
            })
            .flatten();
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            let (timestamp, filter) = capture_options(&info.unwrap_or(json!({})), &name, data);
            let url = crate::p115::download_url(state, &pc).await?;
            tracing::debug!(file = %name, timestamp, "[视频截图] 开始截取缺失图片");
            let mut command = tokio::process::Command::new("ffmpeg");
            command
                .kill_on_drop(true)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-user_agent",
                    "Mozilla/5.0",
                    "-rw_timeout",
                    "15000000",
                    "-fflags",
                    "+fastseek",
                    "-ss",
                    &timestamp.to_string(),
                    "-i",
                    &url,
                    "-map",
                    "0:v:0",
                    "-an",
                    "-sn",
                    "-dn",
                    "-vf",
                    &filter,
                    "-frames:v",
                    "1",
                    "-q:v",
                    "5",
                    "-y",
                ])
                .arg(&stage);
            let _slot = crate::strm::slot(state, "main", context).await?;
            if overwrite && target.exists() {
                tokio::fs::remove_file(target).await?;
            }
            let output = tokio::time::timeout(Duration::from_secs(30), command.output())
                .await
                .map_err(|_| ApiError::bad("视频截图超时"))??;
            drop(_slot);
            if !output.status.success()
                || !tokio::fs::metadata(&stage)
                    .await
                    .is_ok_and(|meta| meta.len() > 0)
            {
                return Err(ApiError::bad("视频截图未生成有效图片"));
            }
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            if !overwrite && target.exists() {
                return Ok(false);
            }
            tokio::fs::rename(&stage, target).await?;
            Ok(true)
        };
        tokio::pin!(operation);
        if let Some(context) = context {
            loop {
                tokio::select! {
                    result = &mut operation => break result,
                    tick = context.sleep(Duration::from_millis(100)) => {
                        if let Err(error) = tick {
                            break Err(error);
                        }
                    }
                }
            }
        } else {
            operation.await
        }
    };
    let _ = tokio::fs::remove_file(stage).await;
    result
}
