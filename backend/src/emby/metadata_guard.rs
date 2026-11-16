use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    state::string,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use tokio::io::AsyncReadExt;

const FIELDS: &str = "Id,Type,Path,PresentationUniqueKey,Guid,MediaSources,MediaStreams,Chapters";
const MEDIA: &[&str] = &[
    "Id",
    "Type",
    "Path",
    "MediaSources",
    "MediaStreams",
    "Chapters",
];

#[derive(Serialize, Deserialize)]
pub(super) struct RelatedGuard {
    scope: Value,
    native: BTreeMap<String, Value>,
    files: BTreeMap<PathBuf, Option<FileSignature>>,
    directories: Vec<DirectoryScope>,
}

#[derive(Serialize, Deserialize)]
struct DirectoryScope {
    root: PathBuf,
    bif_prefix: Option<String>,
    members: Option<BTreeSet<PathBuf>>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct FileSignature {
    sha256: String,
    size: u64,
}

async fn directory_files(
    root: &std::path::Path,
    bif_prefix: Option<&str>,
) -> Result<Option<BTreeSet<PathBuf>>> {
    let kind = match tokio::fs::symlink_metadata(root).await {
        Ok(kind) => kind,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if kind.file_type().is_symlink() || !kind.is_dir() {
        return Err(ApiError::bad(
            "关联资源目录不是普通目录或包含符号链接，未提交元数据",
        ));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if bif_prefix.is_some_and(|prefix| !name.ends_with(".bif") || !name.starts_with(prefix))
            {
                continue;
            }
            let kind = entry.file_type().await?;
            if kind.is_symlink() {
                return Err(ApiError::bad("关联资源包含符号链接，未提交元数据"));
            }
            if kind.is_dir() && bif_prefix.is_none() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.insert(entry.path());
            } else {
                return Err(ApiError::bad("关联资源不是普通文件，未提交元数据"));
            }
        }
    }
    Ok(Some(files))
}

async fn file_signature(path: &std::path::Path) -> Result<Option<FileSignature>> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut sha = Sha256::new();
    let mut size = 0;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        sha.update(&buffer[..count]);
        size += count as u64;
    }
    Ok(Some(FileSignature {
        sha256: format!("{:x}", sha.finalize()),
        size,
    }))
}

impl RelatedGuard {
    pub(super) async fn capture(
        state: &AppState,
        item: &Value,
        ids: &[String],
        context: &JobContext,
    ) -> Result<Self> {
        let mut selected = if item["Type"] == "Series" {
            ids.to_vec()
        } else {
            vec![string(item, "Id")]
        };
        selected.sort();
        selected.dedup();
        let scope = json!({"Id":item["Id"],"Type":item["Type"],"Path":item["Path"],"selected_ids":selected});
        let origin_key = format!("related-resource-origin:{}", string(item, "Id"));
        if let Some(saved) = context.plan(&origin_key).await? {
            let guard: Self = serde_json::from_value(saved)
                .map_err(|_| ApiError::bad("原关联资源保护计划无效，未重新建立基线"))?;
            if guard.scope != scope {
                return Err(ApiError::bad("原关联资源保护范围已变化，未重新建立基线"));
            }
            guard.verify(state, context).await?;
            return Ok(guard);
        }
        let inventory = crate::emby::query_items(
            state,
            &[
                ("IncludeItemTypes", "Movie,Episode".into()),
                ("Recursive", "true".into()),
                // Read full media fields only for the related versions below.
                ("Fields", "Id,Type,Path,PresentationUniqueKey,MediaSources".into()),
            ],
            context,
        )
        .await?;
        let selected_items = inventory
            .iter()
            .filter(|item| selected.contains(&string(item, "Id")))
            .collect::<Vec<_>>();
        if selected_items.len() != selected.len() {
            return Err(ApiError::bad("定点更新的当前媒体范围未确认，未提交元数据"));
        }
        let keys = selected_items
            .iter()
            .map(|item| string(item, "PresentationUniqueKey"))
            .filter(|key| !key.is_empty())
            .collect::<BTreeSet<_>>();
        let linked = selected_items
            .iter()
            .flat_map(|item| list(&item["MediaSources"]))
            .map(|source| string(&source, "ItemId"))
            .filter(|id| !id.is_empty())
            .collect::<BTreeSet<_>>();
        let mut guard = Self {
            scope,
            native: BTreeMap::new(),
            files: BTreeMap::new(),
            directories: vec![],
        };
        let config = state.config.read().await.clone();
        for item in inventory.iter().filter(|item| {
            !selected.contains(&string(item, "Id"))
                && (keys.contains(&string(item, "PresentationUniqueKey"))
                    || linked.contains(&string(item, "Id"))
                    || list(&item["MediaSources"])
                        .iter()
                        .any(|source| selected.contains(&string(source, "ItemId"))))
        }) {
            context.checkpoint().await?;
            let id = string(item, "Id");
            let live = details(state, &id, FIELDS).await?;
            let path = PathBuf::from(string(&live, "Path"));
            if !path.is_absolute() {
                return Err(ApiError::bad("关联版本的本地资源路径未确认，未提交元数据"));
            }
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            for file in [
                path.with_extension("nfo"),
                path.with_file_name(format!("{stem}-mediainfo.json")),
                path.with_file_name(format!("{stem}-thumb.jpg")),
            ] {
                guard
                    .files
                    .insert(file.clone(), file_signature(&file).await?);
            }
            if let Some(parent) = path.parent() {
                let prefix = stem.to_lowercase();
                let members = directory_files(parent, Some(&prefix)).await?;
                for file in members.iter().flatten() {
                    guard
                        .files
                        .insert(file.clone(), file_signature(file).await?);
                }
                guard.directories.push(DirectoryScope {
                    root: parent.to_path_buf(),
                    bif_prefix: Some(prefix),
                    members,
                });
            }
            let root = string(&config, "emby_metadata_path");
            let guid = string(&live, "Guid");
            if !root.is_empty() && uuid::Uuid::parse_str(&guid).is_ok() {
                let directory = PathBuf::from(root)
                    .join("library")
                    .join(&guid[..2])
                    .join(&guid)
                    .join("chapters");
                let members = directory_files(&directory, None).await?;
                for file in members.iter().flatten() {
                    guard
                        .files
                        .insert(file.clone(), file_signature(file).await?);
                }
                guard.directories.push(DirectoryScope {
                    root: directory,
                    bif_prefix: None,
                    members,
                });
            }
            guard.native.insert(id, live);
        }
        context
            .save_plan(
                &origin_key,
                &serde_json::to_value(&guard)
                    .map_err(|_| ApiError::bad("无法保存原关联资源保护计划，未提交元数据"))?,
            )
            .await?;
        guard.verify(state, context).await?;
        Ok(guard)
    }

    pub(super) async fn verify(&self, state: &AppState, context: &JobContext) -> Result<()> {
        crate::emby::idle(state, context).await?;
        for (id, before) in &self.native {
            let after = details(state, id, FIELDS).await?;
            if MEDIA.iter().any(|field| before[*field] != after[*field]) {
                return Err(ApiError::bad(
                    "定点元数据更新后关联版本的原媒体信息发生变化，未记录入库成功",
                ));
            }
        }
        for directory in &self.directories {
            if directory_files(&directory.root, directory.bif_prefix.as_deref()).await?
                != directory.members
            {
                return Err(ApiError::bad(
                    "定点元数据更新后关联版本的 BIF 或章节图片资源清单发生变化，未记录入库成功",
                ));
            }
        }
        for (path, before) in &self.files {
            if file_signature(path).await? != *before {
                return Err(ApiError::bad(
                    "定点元数据更新后关联版本的原 JSON、NFO、章节图片或 BIF 发生变化，未记录入库成功",
                ));
            }
        }
        Ok(())
    }
}
