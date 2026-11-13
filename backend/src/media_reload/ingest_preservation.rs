//! Original resources around the final recursive ingest refresh.
use super::*;

const FIELDS: &str = "Id,Type,Name,Path,Guid,PresentationUniqueKey,ExtraType,PartCount,Size,RunTimeTicks,Chapters,MediaSources,MediaStreams,SeriesId,ParentId";

pub(crate) struct Guard {
    targets: Vec<SavedTarget>,
    observations: BTreeMap<String, Value>,
    images: chapter_images::EvidenceMap,
    bifs: BTreeMap<String, Vec<BifEvidence>>,
    files: BTreeMap<PathBuf, Snapshot>,
    json_paths: BTreeSet<PathBuf>,
    connection: Value,
    record: Option<PathBuf>,
    recovery: PathBuf,
    parent: Value,
    scope: BTreeMap<String, Value>,
}

fn observed_scope(members: &[Value]) -> BTreeMap<String, Value> {
    members.iter().map(|member| (string(member, "Id"), json!({"Id":member["Id"],"Type":member["Type"],"Path":member["Path"],"Chapters":member["Chapters"],"MediaStreams":member["MediaStreams"],"MediaSources":member["MediaSources"]}))).collect()
}

async fn members(state: &AppState, item: &Value, context: &JobContext) -> Result<Vec<Value>> {
    if item["Type"] == "Series" {
        let parent = media_edit::details(state, &string(item, "Id"), "Id,Type,Path").await?;
        if ["Id", "Type", "Path"]
            .iter()
            .any(|field| parent[*field] != item[*field])
        {
            return Err(conflict("入库刷新父项的身份或路径变化，未提交刷新"));
        }
        crate::emby::query_items(
            state,
            &[
                ("ParentId", string(item, "Id")),
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Episode,Video".into()),
                ("Fields", FIELDS.into()),
            ],
            context,
        )
        .await
    } else {
        Ok(vec![
            media_edit::details(state, &string(item, "Id"), FIELDS).await?,
        ])
    }
}

async fn snapshot_chapter_resources(
    directory: &Path,
    recovery: &Path,
    files: &mut BTreeMap<PathBuf, Snapshot>,
    id: &str,
) -> Result<()> {
    // FullRefresh may legitimately replace posters. Only original chapter
    // images and BIF files belong to this earlier preservation baseline.
    check_ancestors(&directory.join("unused")).await?;
    let chapters = directory.join("chapters");
    snapshot_directory(&chapters, recovery, files, id).await?;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        let mut entries = match tokio::fs::read_dir(path).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            if kind.is_symlink() {
                return Err(conflict("入库原资源包含符号链接，未提交刷新"));
            }
            if kind.is_dir() && entry.path() != chapters {
                pending.push(entry.path());
            }
            if entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bif"))
            {
                snapshot_file(&entry.path(), recovery, files, id).await?;
            }
        }
    }
    Ok(())
}

fn connection(config: &Value) -> Value {
    json!([
        config["emby_server_url"],
        config["emby_api_key"],
        config["emby_user_id"]
    ])
}

impl Guard {
    pub(crate) fn protected(&self) -> bool {
        !self.targets.is_empty()
    }

    pub(crate) async fn publish(&self) -> Result<()> {
        if let Some(record) = &self.record {
            tokio::fs::rename(record, self.recovery.join("media-reload.json")).await?;
            tokio::fs::File::open(&self.recovery)
                .await?
                .sync_all()
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn check(&self, state: &AppState, context: &JobContext) -> Result<()> {
        if connection(&*state.config.read().await) != self.connection {
            return Err(conflict("Emby 连接在入库刷新保护期间变化，未提交刷新"));
        }
        if observed_scope(&members(state, &self.parent, context).await?) != self.scope {
            return Err(conflict(
                "入库最终刷新范围或原章节在准备期间变化，未提交刷新",
            ));
        }
        let mut inventory = None;
        for target in &self.targets {
            context.checkpoint().await?;
            let live = identity_details(state, target).await?;
            if !differences(target, &live).is_empty()
                || live["PresentationUniqueKey"]
                    != self.observations[&target.id]["PresentationUniqueKey"]
            {
                return Err(conflict(
                    "原媒体流、章节或身份在入库刷新准备期间变化，备份已保留",
                ));
            }
            ensure_single_clear_scope_prepared(state, target, &live, context, &mut inventory)
                .await?;
        }
        for saved in self.files.values() {
            if tokio::fs::read(&saved.path).await? != snapshot_bytes(saved).await? {
                return Err(conflict("原资源在入库刷新准备期间变化，备份已保留"));
            }
        }
        Ok(())
    }

    pub(crate) async fn verify(&self, state: &AppState, context: &JobContext) -> Result<()> {
        if connection(&*state.config.read().await) != self.connection {
            return Err(conflict("Emby 连接在入库刷新期间变化，全部恢复材料已保留"));
        }
        for target in &self.targets {
            context.checkpoint().await?;
            let live = identity_details(state, target).await?;
            let reasons = differences(target, &live);
            if !reasons.is_empty() {
                return Err(conflict(format!(
                    "{}：入库刷新后原媒体信息未保留：{}；全部恢复材料已保留",
                    target.id,
                    reasons.join("；")
                )));
            }
            if let Some(reason) =
                chapter_images::difference(state, &target.id, &live, &self.images[&target.id])
                    .await?
            {
                return Err(conflict(reason));
            }
            if let Some(bifs) = self.bifs.get(&target.id)
                && let Some(reason) = native_bif_difference(state, &target.id, bifs).await?
            {
                return Err(conflict(reason));
            }
        }
        // API images may still be served from a matching BIF or cache; verify
        // standalone chapter files too, without accepting changed local bytes.
        for saved in self
            .files
            .values()
            .filter(|saved| !self.json_paths.contains(&saved.path))
        {
            check_ancestors(&saved.path).await?;
            if !tokio::fs::symlink_metadata(&saved.path).await?.is_file()
                || tokio::fs::read(&saved.path).await? != snapshot_bytes(saved).await?
            {
                return Err(conflict(
                    "入库刷新后原章节图片或 BIF 文件发生变化，全部恢复材料已保留",
                ));
            }
        }
        Ok(())
    }
}

pub(crate) async fn capture(
    state: &AppState,
    item: &Value,
    context: &JobContext,
    recovery: &Path,
) -> Result<Guard> {
    context.checkpoint().await?;
    let connection = connection(&*state.config.read().await);
    crate::emby::idle(state, context).await?;
    let members = members(state, item, context).await?;
    let material = recovery.join(format!("ingest-refresh-{}", uuid::Uuid::new_v4().simple()));
    tokio::fs::create_dir_all(&material).await?;
    let mut guard = Guard {
        targets: vec![],
        observations: BTreeMap::new(),
        images: BTreeMap::new(),
        bifs: BTreeMap::new(),
        files: BTreeMap::new(),
        json_paths: BTreeSet::new(),
        connection,
        record: None,
        recovery: recovery.into(),
        parent: item.clone(),
        scope: observed_scope(&members),
    };
    let mut json_files = vec![];
    let mut unused_bifs = BTreeMap::new();
    let mut inventory = None;
    let mut json_settings = None;
    for member in members {
        context.checkpoint().await?;
        let id = string(&member, "Id");
        let path = string(&member, "Path");
        if !Path::new(&path).is_absolute() || id.is_empty() {
            return Err(conflict(
                "入库刷新范围缺少有效媒体 ID 或绝对路径，未提交刷新",
            ));
        }
        let mut target = SavedTarget {
            id: id.clone(),
            path,
            source: None,
            sha: None,
            asset: None,
            info: Value::Null,
            refresh_required: true,
        };
        let before_files = guard.files.len();
        snapshot_local_bif(&target, &material, &mut guard.files).await?;
        let thumbs = state
            .emby(
                Method::GET,
                &format!("Items/{id}/ThumbnailSet"),
                &[("Width", "320".into())],
                None,
            )
            .await?;
        let has_thumbs = thumbs["Thumbnails"]
            .as_array()
            .ok_or_else(|| conflict("入库刷新前缩略图集合格式无效，未提交刷新"))?
            .iter()
            .next()
            .is_some();
        if media_edit::list(&member["Chapters"]).is_empty() {
            // Empty native chapters do not authorize discarding an existing
            // formal chapter backup, including when title filling is disabled.
            let config = state.config.read().await.clone();
            let source = crate::media_paths::strm_target(&target.path).await;
            let mut candidates = crate::media_paths::info_candidates(
                &target.path,
                &string(&config, "local_strm_root"),
                &string(&config, "cd2_media_root"),
                "",
            )
            .await;
            // This is a read-only preparation phase: read the native JSON
            // location once, including PRO's custom absolute-root mapping.
            if json_settings.is_none() {
                json_settings = Some(crate::strm::config(state).await?);
            }
            let formal = json_path(&target, json_settings.as_ref().unwrap())?;
            if !candidates.contains(&formal) {
                candidates.push(formal);
            }
            for path in candidates {
                if let Ok(bytes) = tokio::fs::read(path).await
                    && let Ok(info) = serde_json::from_slice::<Value>(&bytes)
                    && media_info::recoverable(&info)
                    && crate::media_paths::matches_container(&info, &target.path)
                {
                    media_info::recovery_chapters(
                        &info,
                        &member,
                        &[&target.path, source.as_deref().unwrap_or("")],
                    )?;
                }
            }
        }
        if media_edit::list(&member["Chapters"]).is_empty()
            && !has_thumbs
            && guard.files.len() == before_files
        {
            continue;
        }
        let binding = media_edit::repair_target(state, &id).await?;
        if binding.path != target.path {
            return Err(conflict("入库原资源的媒体路径在准备期间变化，未提交刷新"));
        }
        target.path = binding.path.clone();
        target.source = crate::media_paths::strm_target(&target.path).await;
        target.sha = binding.sha.clone();
        target.asset = (!binding.kind.is_empty()).then_some(binding.asset.clone());
        target.info = media_info::restore_json(&media_edit::load(state, &binding).await?)?;
        let live = identity_details(state, &target).await?;
        media_info::preserve_live_streams(
            &mut target.info,
            &live,
            &[&target.path, target.source.as_deref().unwrap_or("")],
        )?;
        media_info::preserve_chapters(&mut target.info, &live["Chapters"])?;
        ensure_single_clear_scope_prepared(state, &target, &live, context, &mut inventory).await?;
        let has_images = media_edit::list(&live["Chapters"])
            .iter()
            .any(|chapter| !string(chapter, "ImageTag").is_empty());
        if has_images || has_thumbs {
            let configured = string(&*state.config.read().await, "emby_metadata_path");
            let root = if configured.is_empty() {
                PathBuf::from(string(
                    &state.emby(Method::GET, "System/Info", &[], None).await?,
                    "InternalMetadataPath",
                ))
            } else {
                PathBuf::from(configured)
            };
            if !root.is_absolute() {
                return Err(conflict("入库原图片的元数据根目录不可访问，未提交刷新"));
            }
            let root = tokio::fs::canonicalize(root).await?;
            let directory = metadata_directory(state, &root, &live).await?;
            snapshot_chapter_resources(&directory, &material, &mut guard.files, &id).await?;
        }
        let images = chapter_images::capture(state, &id, &live, Some(&material)).await?;
        let path = json_path(&target, &crate::strm::config(state).await?)?;
        snapshot_file(&path, &material, &mut guard.files, &id).await?;
        if let Ok(path) = tokio::fs::canonicalize(&path).await {
            guard.json_paths.insert(path);
        }
        let backup = material.join(format!("prepared-{id}.json"));
        write_recovery_file(&backup, &serde_json::to_vec(&target.info).unwrap()).await?;
        json_files.push(json!({"item_id":id,"path":path,"backup":backup}));
        for saved in guard.files.values().filter(|saved| {
            saved.owners.contains(&id)
                && saved
                    .path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("bif"))
        }) {
            let bytes = tokio::fs::read(&saved.backup).await?;
            if let Ok(times) = recovery_bif_times(&bytes) {
                guard.bifs.entry(id.clone()).or_default().push(BifEvidence {
                    path: saved.path.clone(),
                    bytes,
                    frames: times.as_array().unwrap().len(),
                });
            } else {
                unused_bifs.insert(saved.path.clone(), format!("{:x}", Sha256::digest(&bytes)));
            }
        }
        if has_thumbs && !guard.bifs.contains_key(&id) {
            return Err(conflict(
                "原生缩略图没有可访问的原 BIF 备份，未提交入库刷新",
            ));
        }
        if let Some(bifs) = guard.bifs.get(&id)
            && let Some(reason) = native_bif_difference(state, &id, bifs).await?
        {
            return Err(conflict(reason));
        }
        guard.images.insert(id.clone(), images);
        guard.observations.insert(id, live);
        guard.targets.push(target);
    }
    if guard.protected() {
        let record = json!({"target_ids":guard.targets.iter().map(|target| &target.id).collect::<Vec<_>>(),
            "targets":guard.targets.iter().map(|target|json!({"id":target.id,"path":target.path,"source":target.source,"sha":target.sha,"asset":target.asset})).collect::<Vec<_>>(),
            "observations":guard.observations,"chapter_images":guard.images,"json":json_files,
            "files":guard.files.values().map(|saved|json!({"path":saved.path,"backup":saved.backup,"owner_ids":saved.owners,"sha256":saved.sha256,"unused_bif_sha256":unused_bifs.get(&saved.path)})).collect::<Vec<_>>()});
        let stage = material.join("media-reload.json");
        write_recovery_file(&stage, &serde_json::to_vec(&record).unwrap()).await?;
        guard.record = Some(stage);
    } else {
        tokio::fs::remove_dir(&material).await?;
    }
    Ok(guard)
}
