use crate::{
    AppState,
    error::{ApiError, Result},
    media_save::SavedTarget,
    state::string,
};
use axum::http::StatusCode;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const MAGIC: &[u8; 8] = b"\x89BIF\r\n\x1a\n";

pub(crate) struct Bridge {
    pub source: PathBuf,
    pub destination: PathBuf,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}

fn jpeg_width(bytes: &[u8]) -> Result<u32> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(|_| invalid("章节图片不是有效 JPEG，未清理媒体信息"))?;
    Ok(image.width())
}

// Emby 4.10.2 Video.CreateBif writes the Roku BIF v0 header and index.
// ImageService.GetThumbnailsFromBif uses the millisecond multiplier at byte
// 16 and (timestamp, byte offset) pairs beginning at byte 64.
fn validate(bytes: &[u8]) -> Result<u32> {
    let bad = || invalid("BIF 文件头、时间索引或图片内容无效，未清理媒体信息");
    if bytes.len() < 80 || bytes.get(..8) != Some(MAGIC.as_slice()) {
        return Err(bad());
    }
    let read = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let count = read(12) as usize;
    let multiplier = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let table_end = count
        .checked_add(1)
        .and_then(|n| n.checked_mul(8))
        .and_then(|n| n.checked_add(64))
        .ok_or_else(bad)?;
    if read(8) != 0
        || count == 0
        || table_end > bytes.len()
        || multiplier == 0
        || multiplier > i64::MAX as u64 / 10000
        || read(64 + count * 8) != u32::MAX
        || read(68 + count * 8) as usize != bytes.len()
    {
        return Err(bad());
    }
    let mut previous = None;
    let mut width = 0;
    for index in 0..count {
        let position = read(64 + index * 8);
        let begin = read(68 + index * 8) as usize;
        let end = read(76 + index * 8) as usize;
        if position > i32::MAX as u32
            || previous.is_some_and(|p| position < p)
            || begin < table_end
            || begin >= end
            || end > bytes.len()
            || (position as u64)
                .checked_mul(multiplier)
                .is_none_or(|ms| ms > i64::MAX as u64 / 10000)
        {
            return Err(bad());
        }
        let frame_width = jpeg_width(&bytes[begin..end])?;
        if index == 0 {
            width = frame_width;
        }
        previous = Some(position);
    }
    Ok(width)
}

fn encode(frames: &[(u64, Vec<u8>)]) -> Result<(Vec<u8>, u32)> {
    if frames.is_empty() {
        return Err(invalid("没有可保护的章节图片，未清理媒体信息"));
    }
    let count = u32::try_from(frames.len()).map_err(|_| invalid("章节图片数超出 BIF 格式范围"))?;
    let header = 64_usize
        .checked_add(
            (frames.len() + 1)
                .checked_mul(8)
                .ok_or_else(|| invalid("BIF 索引超出格式范围"))?,
        )
        .ok_or_else(|| invalid("BIF 索引超出格式范围"))?;
    let size = frames
        .iter()
        .try_fold(header, |size, (_, bytes)| size.checked_add(bytes.len()))
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| invalid("BIF 图片总大小超出格式范围"))?;
    let mut output = vec![0; header];
    output[..8].copy_from_slice(MAGIC);
    output[12..16].copy_from_slice(&count.to_le_bytes());
    output[16..20].copy_from_slice(&1_u32.to_le_bytes());
    let mut previous = None;
    for (index, (ticks, bytes)) in frames.iter().enumerate() {
        // Round upwards: Emby's image endpoint selects the first frame at or
        // after the chapter timestamp. Preserve sub-millisecond positions.
        let millis = ticks
            .checked_add(9999)
            .map(|ticks| ticks / 10000)
            .and_then(|ms| i32::try_from(ms).ok())
            .ok_or_else(|| invalid("章节时间超出 BIF 格式范围，未清理媒体信息"))?;
        if previous.is_some_and(|p| millis <= p) {
            return Err(invalid(
                "章节图片时间无法唯一表示为 BIF 毫秒索引，未清理媒体信息",
            ));
        }
        let offset = u32::try_from(output.len()).unwrap();
        output[64 + index * 8..68 + index * 8].copy_from_slice(&millis.to_le_bytes());
        output[68 + index * 8..72 + index * 8].copy_from_slice(&offset.to_le_bytes());
        output.extend_from_slice(bytes);
        previous = Some(millis);
    }
    output[64 + frames.len() * 8..68 + frames.len() * 8].copy_from_slice(&u32::MAX.to_le_bytes());
    output[68 + frames.len() * 8..72 + frames.len() * 8].copy_from_slice(&size.to_le_bytes());
    let width = validate(&output)?;
    Ok((output, width))
}

async fn candidates(directory: &Path, prefix: Option<&str>) -> Result<Vec<PathBuf>> {
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let prefix = prefix.map(str::to_lowercase);
    let mut paths = vec![];
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".bif")
            && prefix
                .as_ref()
                .is_none_or(|prefix| name.starts_with(prefix))
        {
            if !entry.file_type().await?.is_file() {
                return Err(invalid("BIF 资源不是普通文件，未清理媒体信息"));
            }
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) async fn prepare(
    state: &AppState,
    target: &SavedTarget,
    live: &Value,
    metadata_dir: &Path,
    recovery: &Path,
) -> Result<Option<Bridge>> {
    let chapters = live["Chapters"].as_array();
    let tagged = chapters
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, chapter)| !string(chapter, "ImageTag").is_empty())
        .collect::<Vec<_>>();
    let path = Path::new(&target.path);
    let parent = path.parent().ok_or_else(|| invalid("媒体路径没有父目录"))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("媒体文件名无效"))?;
    let local = candidates(parent, Some(stem)).await?;
    if !local.is_empty() {
        // The native provider selects from all matching files. One valid BIF
        // cannot make a second corrupt matching file safe to register.
        for file in local {
            validate(&tokio::fs::read(file).await?)?;
        }
        return Ok(None);
    }
    let mut stored = candidates(metadata_dir, None).await?;
    stored.extend(candidates(&metadata_dir.join("thumbnailcache"), Some(stem)).await?);
    let mut bytes = None;
    for file in stored {
        let content = tokio::fs::read(file).await?;
        if let Ok(width) = validate(&content) {
            bytes = Some((content, width));
            break;
        }
    }
    let (bytes, width) = if let Some(bytes) = bytes {
        bytes
    } else {
        if tagged.is_empty() {
            return Ok(None);
        }
        let config = state.config.read().await.clone();
        let url = string(&config, "emby_server_url");
        let key = string(&config, "emby_api_key");
        let mut frames = std::collections::BTreeMap::<u64, Vec<u8>>::new();
        for (ordinal, chapter) in tagged {
            let ticks = chapter["StartPositionTicks"]
                .as_u64()
                .ok_or_else(|| invalid("章节图片时间无效，未清理媒体信息"))?;
            let index = chapter["ChapterIndex"].as_u64().unwrap_or(ordinal as u64);
            let response = crate::emby::send(
                state
                    .http
                    .get(format!(
                        "{}/Items/{}/Images/Chapter/{index}",
                        url.trim_end_matches('/'),
                        target.id
                    ))
                    .header("X-Emby-Token", &key)
                    .query(&[("Format", "jpg")]),
            )
            .await?
            .error_for_status()?;
            let content = response.bytes().await?.to_vec();
            if let Some(previous) = frames.insert(ticks, content.clone())
                && previous != content
            {
                return Err(invalid("同一时间的章节图片内容不一致，未清理媒体信息"));
            }
        }
        encode(&frames.into_iter().collect::<Vec<_>>())?
    };
    let current = crate::media_edit::details(state, &target.id, "Id,Path,Chapters").await?;
    if current["Id"] != target.id
        || current["Path"] != target.path
        || current["Chapters"] != live["Chapters"]
    {
        return Err(invalid("保护期间媒体身份或章节图片已变化，未清理媒体信息"));
    }
    let name = format!("{stem}-{width}-1.bif");
    if name.len() > 255 {
        return Err(invalid("章节图片恢复旁车文件名过长，未清理媒体信息"));
    }
    tokio::fs::create_dir_all(recovery).await?;
    let source = recovery.join(format!(
        "chapter-bif-{}-{}.bif",
        target.id,
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&source)
        .await?;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    Ok(Some(Bridge {
        source,
        destination: parent.join(name),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(color: u8) -> Vec<u8> {
        let mut bytes = vec![];
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(&[color; 12], 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    #[test]
    fn native_bif_header_and_index_preserve_each_existing_jpeg() {
        let a = jpeg(12);
        let b = jpeg(200);
        let (bytes, _) = encode(&[(0, a.clone()), (100_000_001, b.clone())]).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(
            &bytes[8..24],
            &[0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        // Independent reading follows Emby's published/decompiled byte layout.
        let read = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(read(64), 0);
        assert_eq!(read(72), 10001);
        assert_eq!(read(80), u32::MAX);
        assert_eq!(read(68), 88);
        assert_eq!(&bytes[read(68) as usize..read(76) as usize], a);
        assert_eq!(&bytes[read(76) as usize..read(84) as usize], b);
        assert_eq!(validate(&bytes).unwrap(), 2);
        if let Ok(path) = std::env::var("ETK_BIF_NATIVE_PROBE_OUTPUT") {
            std::fs::write(path, &bytes).unwrap();
        }
    }

    #[test]
    fn rejects_incomplete_files_bad_offsets_and_non_jpeg_payloads() {
        let (original, _) = encode(&[(0, jpeg(55))]).unwrap();
        assert!(validate(&original[..original.len() - 1]).is_err());
        let mut bad = original.clone();
        bad[68..72].copy_from_slice(&1_u32.to_le_bytes());
        assert!(validate(&bad).is_err());
        let mut bad = original;
        bad[80] = 0;
        assert!(validate(&bad).is_err());
        assert!(encode(&[(0, b"arbitrary data".to_vec())]).is_err());
    }

    #[test]
    fn refuses_distinct_frames_that_cannot_share_a_bif_millisecond() {
        assert!(encode(&[(1, jpeg(1)), (2, jpeg(2))]).is_err());
    }
}
