use super::*;
use serde::{Deserialize, Serialize};

pub(crate) type EvidenceMap = BTreeMap<String, Vec<Evidence>>;

#[derive(Serialize, Deserialize)]
pub(crate) struct Evidence {
    name: String,
    ticks: u64,
    marker: String,
    occurrence: usize,
    backup: Option<PathBuf>,
    sha256: String,
    #[serde(skip)]
    bytes: Vec<u8>,
}

fn key(chapter: &Value) -> Result<(String, u64, String)> {
    Ok((
        string(chapter, "Name"),
        chapter["StartPositionTicks"]
            .as_u64()
            .ok_or_else(|| conflict("原有章节图片时间无效，恢复备份已保留"))?,
        chapter["MarkerType"].as_str().unwrap_or("Chapter").into(),
    ))
}

async fn read(state: &AppState, id: &str, index: u64) -> Result<Vec<u8>> {
    let config = state.config.read().await;
    let url = string(&config, "emby_server_url");
    let token = string(&config, "emby_api_key");
    drop(config);
    let response = crate::emby::send(
        state
            .http
            .get(format!(
                "{}/Items/{id}/Images/Chapter/{index}",
                url.trim_end_matches('/')
            ))
            .header("X-Emby-Token", token)
            .query(&[("Format", "jpg")]),
    )
    .await?;
    if !response.status().is_success() {
        return Err(conflict(format!(
            "原有章节图片无法回读：{id}，索引 {index}，恢复备份已保留"
        )));
    }
    Ok(response.bytes().await?.to_vec())
}

pub(crate) async fn capture(
    state: &AppState,
    id: &str,
    live: &Value,
    recovery: Option<&Path>,
) -> Result<Vec<Evidence>> {
    if !live["Chapters"].is_null() {
        media_info::signature(&live["Chapters"])?;
    }
    let mut evidence = vec![];
    let mut occurrences = BTreeMap::new();
    let mut cached = BTreeMap::new();
    for (ordinal, chapter) in media_edit::list(&live["Chapters"]).iter().enumerate() {
        let (name, ticks, marker) = key(chapter)?;
        let occurrence = occurrences
            .entry((name.clone(), ticks, marker.clone()))
            .or_insert(0);
        let current_occurrence = *occurrence;
        *occurrence += 1;
        if string(chapter, "ImageTag").is_empty() {
            continue;
        }
        let index = chapter["ChapterIndex"].as_u64().unwrap_or(ordinal as u64);
        if let std::collections::btree_map::Entry::Vacant(entry) = cached.entry(index) {
            let bytes = read(state, id, index).await?;
            image::load_from_memory(&bytes)
                .map_err(|_| conflict("原有章节图片内容无效，未提交媒体信息同步"))?;
            entry.insert(bytes);
        }
        let bytes = cached[&index].clone();
        let backup = if let Some(recovery) = recovery {
            tokio::fs::create_dir_all(recovery).await?;
            let path = recovery.join(format!(
                "chapter-image-{}.jpg",
                uuid::Uuid::new_v4().simple()
            ));
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await?;
            file.write_all(&bytes).await?;
            file.sync_all().await?;
            Some(path)
        } else {
            None
        };
        evidence.push(Evidence {
            name,
            ticks,
            marker,
            occurrence: current_occurrence,
            backup,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        });
    }
    Ok(evidence)
}

pub(crate) async fn load(
    recovery: &Path,
    record: &Value,
    owners: &BTreeSet<String>,
) -> Result<EvidenceMap> {
    if record["chapter_images"].is_null() {
        return Ok(BTreeMap::new());
    }
    let mut evidence: EvidenceMap = serde_json::from_value(record["chapter_images"].clone())
        .map_err(|_| conflict("原有章节图片恢复记录无效，全部恢复材料已保留"))?;
    for (id, images) in &mut evidence {
        if !owners.contains(id) {
            return Err(conflict("原有章节图片所属媒体与恢复目标不一致"));
        }
        for original in images {
            let backup = recovery_backup(recovery, &json!(original.backup)).await?;
            original.bytes = tokio::fs::read(backup).await?;
            if original.sha256 != format!("{:x}", Sha256::digest(&original.bytes)) {
                return Err(conflict(
                    "原有章节图片恢复备份与原记录不一致，全部恢复材料已保留",
                ));
            }
            image::load_from_memory(&original.bytes)
                .map_err(|_| conflict("原有章节图片恢复备份无效，全部恢复材料已保留"))?;
        }
    }
    Ok(evidence)
}

pub(crate) async fn difference(
    state: &AppState,
    id: &str,
    live: &Value,
    evidence: &[Evidence],
) -> Result<Option<String>> {
    if !live["Chapters"].is_null() {
        media_info::signature(&live["Chapters"])?;
    }
    let chapters = media_edit::list(&live["Chapters"]);
    let mut cached = BTreeMap::new();
    for original in evidence {
        let mut matching = chapters.iter().enumerate().filter(|(_, chapter)| {
            key(chapter).is_ok_and(|key| {
                key == (
                    original.name.clone(),
                    original.ticks,
                    original.marker.clone(),
                )
            })
        });
        let Some((ordinal, chapter)) = matching.nth(original.occurrence) else {
            return Ok(Some(format!(
                "原有章节图片对应章节未恢复：{}",
                original.ticks
            )));
        };
        if string(chapter, "ImageTag").is_empty() {
            return Ok(Some(format!("原有章节图片未恢复：{}", original.ticks)));
        }
        let index = chapter["ChapterIndex"].as_u64().unwrap_or(ordinal as u64);
        if let std::collections::btree_map::Entry::Vacant(entry) = cached.entry(index) {
            entry.insert(read(state, id, index).await?);
        }
        if cached[&index] != original.bytes {
            return Err(conflict(format!(
                "原有章节图片内容与备份不一致：{id}，时间 {}，索引 {index}，恢复备份已保留",
                original.ticks
            )));
        }
    }
    Ok(None)
}
