use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_paths,
    state::string,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// Retry a failed mount read without changing the saved reader preference.
pub(crate) async fn cd2_file(
    state: &AppState,
    path: &str,
    pc: Option<&str>,
    context: Option<&JobContext>,
) -> Result<Option<PathBuf>> {
    let config = state.config.read().await.clone();
    if config["p115_media_read_mode"] != "cd2" {
        return Ok(None);
    }
    for attempt in 0..=3 {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            cd2_file_once(state, &config, path, pc),
        );
        let outcome = if let Some(context) = context {
            tokio::select! {
                _ = context.cancelled() => return Err(ApiError::bad("媒体读取已取消")),
                result = read => result,
            }
        } else {
            read.await
        };
        let error = match outcome {
            Ok(Ok(file)) => return Ok(Some(file)),
            Ok(Err(error)) if error.0 != axum::http::StatusCode::SERVICE_UNAVAILABLE => {
                return Err(error);
            }
            Ok(Err(error)) => error,
            Err(_) => unavailable("CD2 挂载读取超时"),
        };
        if attempt == 3 {
            tracing::warn!("[媒体读取] CD2 重试 3 次仍不可用，回退 ETK：{}", error.1);
            return Ok(None);
        }
        tracing::info!(
            "[媒体读取] CD2 不可用，准备第 {} 次重试：{}",
            attempt + 1,
            error.1
        );
        if let Some(context) = context {
            context.sleep(std::time::Duration::from_secs(1)).await?;
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    unreachable!()
}

fn unavailable(message: &str) -> ApiError {
    ApiError(axum::http::StatusCode::SERVICE_UNAVAILABLE, message.into())
}

async fn cd2_file_once(
    state: &AppState,
    config: &serde_json::Value,
    path: &str,
    pc: Option<&str>,
) -> Result<PathBuf> {
    let root = string(&config, "cd2_media_root");
    if root.trim().is_empty() {
        return Err(unavailable("CD2 媒体库本地根目录尚未配置"));
    }
    let canonical_root = tokio::fs::canonicalize(&root)
        .await
        .map_err(|_| unavailable("CD2 媒体库目录不可访问，请检查挂载"))?;
    let source = if media_paths::extension(path) == ".strm" {
        crate::rename::stream_target(Path::new(path))
            .await
            .unwrap_or_else(|| path.into())
    } else {
        path.into()
    };
    let pc = pc
        .filter(|pc| !pc.is_empty())
        .map(str::to_string)
        .or_else(|| crate::fingerprints::pickcode(&source));
    let cached = if pc.is_some() {
        crate::fingerprints::cached(state, pc.as_deref(), &[]).await?
    } else {
        None
    };
    let mut candidates = vec![PathBuf::from(&source)];
    if let Some(cached) = &cached {
        let path = crate::rename::configured_cloud(&string(cached, "local_path"), &config);
        if !path.is_empty() {
            candidates.push(path.clone().into());
            candidates.push(
                crate::rename::configured_cloud(path.trim_start_matches('/'), &config).into(),
            );
        }
    }
    for relative in
        media_paths::local_candidates(path, &source, &[&string(&config, "local_strm_root"), &root])
    {
        candidates.push(Path::new(&root).join(relative));
    }
    let mut files = BTreeSet::new();
    for candidate in candidates {
        let extension = media_paths::extension(&candidate.to_string_lossy());
        if extension.is_empty() || extension == ".strm" {
            continue;
        }
        let Ok(candidate) = tokio::fs::canonicalize(candidate).await else {
            continue;
        };
        if !candidate.starts_with(&canonical_root) {
            continue;
        }
        let metadata = tokio::fs::metadata(&candidate)
            .await
            .map_err(|_| unavailable("CD2 文件状态读取失败"))?;
        if metadata.is_file() {
            files.insert((candidate, metadata.len()));
        }
    }
    if files.is_empty() {
        return Err(unavailable(
            "CD2 挂载目录中未找到当前媒体文件，请检查目录映射和 STRM 路径",
        ));
    }
    if files.len() > 1 {
        return Err(ApiError::bad("CD2 路径对应多个媒体文件，请检查目录映射"));
    }
    let (file, size) = files.into_iter().next().unwrap();
    if cached
        .as_ref()
        .and_then(|cached| crate::identity::size(&cached["size"]))
        .is_some_and(|expected| expected > 0 && expected != size)
    {
        return Err(ApiError::bad(
            "CD2 文件大小与当前媒体缓存不一致，请检查挂载文件是否已更新",
        ));
    }
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut input = tokio::fs::File::open(&file)
        .await
        .map_err(|_| unavailable("CD2 文件无法打开"))?;
    let mut byte = [0u8; 1];
    input
        .read_exact(&mut byte)
        .await
        .map_err(|_| unavailable("CD2 文件读取失败"))?;
    input
        .seek(std::io::SeekFrom::End(-1))
        .await
        .map_err(|_| unavailable("CD2 文件定位失败"))?;
    input
        .read_exact(&mut byte)
        .await
        .map_err(|_| unavailable("CD2 文件尾部读取失败"))?;
    tracing::debug!(path=%file.display(), "[媒体读取] 使用 CD2 挂载文件");
    Ok(file)
}
