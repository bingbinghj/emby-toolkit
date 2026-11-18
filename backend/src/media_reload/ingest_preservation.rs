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
    selected_ids: Vec<String>,
    require_clear_scope: bool,
}

fn observed_scope(members: &[Value]) -> BTreeMap<String, Value> {
    members.iter().map(|member| (string(member, "Id"), json!({"Id":member["Id"],"Type":member["Type"],"Path":member["Path"],"Chapters":member["Chapters"],"MediaStreams":member["MediaStreams"],"MediaSources":member["MediaSources"]}))).collect()
}

fn ingest_json_preserved(original: &[u8], current: &[u8], path: &Path) -> Result<bool> {
    let original = parse_recovery_json(original, path)?;
    let mut current = parse_recovery_json(current, path)?;
    if original.as_array().is_some_and(|items| items.len() == 1)
        && current.as_array().is_some_and(|items| items.len() == 1)
        && media_info::chapters_restored(&original[0]["Chapters"], &current[0]["Chapters"])
    {
        current[0]["Chapters"] = original[0]["Chapters"].clone();
    }
    Ok(current == original)
}

async fn members(
    state: &AppState,
    item: &Value,
    context: &JobContext,
    selected_ids: &[String],
) -> Result<Vec<Value>> {
    if item["Type"] == "Series" {
        let parent = media_edit::details(state, &string(item, "Id"), "Id,Type,Path").await?;
        if ["Id", "Type", "Path"]
            .iter()
            .any(|field| parent[*field] != item[*field])
        {
            return Err(conflict("入库刷新父项的身份或路径变化，未提交刷新"));
        }
        if !selected_ids.is_empty() {
            let members = crate::emby::query_items(
                state,
                &[
                    ("Ids", selected_ids.join(",")),
                    ("IncludeItemTypes", "Episode".into()),
                    ("Fields", FIELDS.into()),
                ],
                context,
            )
            .await?;
            if members.len() != selected_ids.len()
                || members.iter().any(|member| {
                    member["Type"] != "Episode"
                        || member["SeriesId"] != item["Id"]
                        || !selected_ids.contains(&string(member, "Id"))
                })
            {
                return Err(conflict("入库刷新目标不属于当前电视剧，未扩大处理范围"));
            }
            return Ok(members);
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

fn metadata_binding_asset(original: &Value, current: &Value) -> bool {
    let mut original = original.clone();
    let mut current = current.clone();
    // These fields are derived by assets::parse from media already verified by
    // this guard. Preserve every other field, including unknown provenance.
    for field in [
        "container", "video_codec", "video_bitrate_mbps", "bit_depth", "frame_rate",
        "audio_tracks", "subtitles", "date_added_to_library", "runtime_minutes", "width", "height",
        "resolution_display", "quality_display", "effect_display", "codec_display",
        "audio_display", "subtitle_display", "audio_languages_raw", "subtitle_languages_raw", "release_group_raw",
    ] {
        original.as_object_mut().unwrap().remove(field);
        current.as_object_mut().unwrap().remove(field);
    }
    for field in ["source_library_id", "ancestor_ids"] {
        if original[field].is_null()
            || original[field].as_str().is_some_and(|value| value.is_empty())
            || original[field].as_array().is_some_and(|value| value.is_empty())
        {
            let valid = current[field].is_null() || if field == "source_library_id" {
                current[field].is_string()
            } else {
                current[field].as_array().is_some_and(|ids| {
                        ids.iter().all(|id| id.as_str().is_some_and(|value| !value.is_empty()))
                    })
            };
            if !valid { return false; }
            original.as_object_mut().unwrap().remove(field);
            current.as_object_mut().unwrap().remove(field);
        }
    }
    original == current
}

async fn metadata_target_details(
    state: &AppState,
    target: &SavedTarget,
    before: &Value,
) -> Result<(SavedTarget, Value)> {
    let binding = media_edit::repair_target(state, &target.id).await?;
    if binding.path != target.path || binding.sha != target.sha
        || target.asset.as_ref().is_some_and(|asset| !metadata_binding_asset(asset, &binding.asset))
        || before.get("_ingest_guard_pick_code").is_some_and(|pc| *pc != json!(binding.pc))
    {
        return Err(conflict("重载期间媒体资产身份变化，已保留恢复备份"));
    }
    let mut checked = target.clone();
    checked.asset = target.asset.as_ref().map(|_| binding.asset.clone());
    let live = identity_details(state, &checked).await?;
    if ["SeriesId", "ParentId"].iter().any(|field| {
        before.get(*field).is_some_and(|value| *value != live[*field])
    }) {
        return Err(conflict("原媒体所属剧集或父项在元数据刷新期间变化，恢复材料已保留",
        ));
    }
    if let Some(original) = &target.asset {
        let added_library = string(original, "source_library_id").is_empty()
            && !string(&binding.asset, "source_library_id").is_empty();
        let added_ancestors = media_edit::list(&original["ancestor_ids"]).is_empty()
            && !media_edit::list(&binding.asset["ancestor_ids"]).is_empty();
        if added_library || added_ancestors {
            let library = crate::emby::library_for_path(state, &target.path).await?
                .ok_or_else(|| conflict("新增媒体库绑定未能按原生路径确认"))?;
            if added_library && binding.asset["source_library_id"] != library["ItemId"] {
                return Err(conflict("新增媒体库绑定与原生文件路径不一致"));
            }
            if added_ancestors {
                let mut item = live.clone();
                item["_SourceLibraryId"] = library["ItemId"].clone();
                let (parents, guid) = if item["Type"] == "Movie" {
                    crate::emby::ancestor_context(state, &item, Some(&library)).await
                } else { (std::collections::HashMap::new(), String::new()) };
                let parsed = crate::assets::parse_with_cache(&item, &parents, &guid, &json!([]), &Value::Null,
                );
                if parsed["ancestor_ids"] != binding.asset["ancestor_ids"] {
                    return Err(conflict("新增祖先绑定与原生媒体树不一致"));
                }
            }
        }
    }
    Ok((checked, live))
}

async fn load_saved_guard(
    state: &AppState, recovery: &Path, record: &mut Value, context: &JobContext,
) -> Result<Guard> {
    context.ensure_server(state).await?;
    let connection = connection(&*state.config.read().await);
    if record["connection_sha256"]
        .as_str()
        .is_some_and(|expected| {
            expected != format!("{:x}", Sha256::digest(connection.to_string().as_bytes()))
        })
    {
        return Err(conflict("原媒体刷新使用的 Emby 连接已变化，保留原保护材料"));
    }
    if record["preparation_kind"] == "ingest-resource-guard" {
        let observations = record["observations"].clone();
        for saved in record["targets"].as_array_mut().ok_or_else(|| conflict("原入库保护目标记录无效"))? {
            let target = SavedTarget {
                id: string(saved, "id"), path: string(saved, "path"),
                source: saved["source"].as_str().map(str::to_string),
                sha: saved["sha"].as_str().map(str::to_string),
                asset: saved["asset"].is_object().then(|| saved["asset"].clone()),
                info: Value::Null, refresh_required: true,
            };
            let (checked, _) = metadata_target_details(state, &target, &observations[&target.id]).await?;
            saved["asset"] = json!(checked.asset);
        }
    }
    let (targets, requested) = prepare_recovery_json(state, recovery, &record).await?;
    let owners = targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    let images = chapter_images::load(recovery, &record, &owners).await?;
    let mut files = BTreeMap::new();
    let mut bifs = BTreeMap::<String, Vec<BifEvidence>>::new();
    for file in media_edit::list(&record["files"]) {
        let path = PathBuf::from(string(&file, "path"));
        let file_owners = media_edit::list(&file["owner_ids"])
            .into_iter()
            .filter_map(|owner| owner.as_str().map(str::to_string))
            .collect::<BTreeSet<_>>();
        if !path.is_absolute() || file_owners.is_empty() || !file_owners.is_subset(&owners) {
            return Err(conflict("原媒体保护文件的路径或所属项目不完整"));
        }
        let saved = Snapshot {
            path: path.clone(),
            backup: recovery_backup(recovery, &file["backup"]).await?,
            owners: file_owners,
            sha256: snapshot_hash(&file)?,
        };
        let bytes = snapshot_bytes(&saved).await?;
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bif"))
        {
            if file["unused_bif_sha256"].is_null() {
                let frames = recovery_bif_times(&bytes)?.as_array().unwrap().len();
                for owner in &saved.owners {
                    bifs.entry(owner.clone()).or_default().push(BifEvidence {
                        path: path.clone(),
                        bytes: bytes.clone(),
                        frames,
                    });
                }
            } else if file["unused_bif_sha256"].as_str()
                != Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
                || recovery_bif_times(&bytes).is_ok()
            {
                return Err(conflict("原保护记录的未使用 BIF 证据无效"));
            }
        }
        files.insert(path, saved);
    }
    let mut json_paths = BTreeSet::new();
    for (path, _) in requested.values() {
        json_paths.insert(tokio::fs::canonicalize(path).await?);
    }
    Ok(Guard {
        targets,
        observations: serde_json::from_value(record["observations"].clone())
            .map_err(|_| conflict("原媒体保护观察记录不完整"))?,
        images,
        bifs,
        files,
        json_paths,
        connection,
        record: None,
        recovery: recovery.into(),
        parent: Value::Null,
        scope: BTreeMap::new(),
        selected_ids: vec![],
        require_clear_scope: record["require_clear_scope"] != false,
    })
}

/// Finish a submitted metadata refresh without importing or replacing media.
pub(crate) async fn finish_saved_guard(
    state: &AppState,
    recovery: &Path,
    context: &JobContext,
) -> Result<()> {
    let mut record = recovery_record(recovery).await?;
    let guard = load_saved_guard(state, recovery, &mut record, context).await?;
    let owners = guard.targets.iter().map(|target| target.id.as_str()).collect::<BTreeSet<_>>();
    let finishing = JobContext::for_recovery(Some(context)).await;
    let pending = recovery.join("pending-scan.json");
    let has_pending_scan = tokio::fs::try_exists(&pending).await?;
    if has_pending_scan {
        let scan = crate::media_recovery::read_json(&pending).await?;
        let position = scan["position"]
            .as_u64()
            .ok_or_else(|| conflict("原刷新日志基线缺失，保留媒体保护"))?;
        let ids = scan["ids"]
            .as_array()
            .filter(|ids| !ids.is_empty())
            .ok_or_else(|| conflict("原刷新目标缺失，保留媒体保护"))?;
        for id in ids {
            let id = id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| conflict("原刷新目标无效"))?;
            if !owners.contains(id) {
                let parent = if record["refresh_parent"]["Id"] == id {
                    record["refresh_parent"].clone()
                } else {
                    media_edit::list(&record["refresh_ancestors"])
                        .into_iter()
                        .find(|parent| parent["Id"] == id)
                        .ok_or_else(|| conflict("原刷新目标不属于保护记录"))?
                };
                let current = media_edit::details(state, id, "Id,Type,Path").await?;
                if ["Id", "Type", "Path"]
                    .iter()
                    .any(|field| current[*field] != parent[*field])
                {
                    return Err(conflict("原刷新父项身份或路径已经变化"));
                }
            }
            crate::emby::wait_item_refresh(
                state,
                id,
                scan["previous"][id].as_str(),
                Some(crate::media_extra::RefreshTrace::new(position)),
                &finishing,
                "恢复前次元数据刷新",
                id,
            )
            .await?;
        }
    }
    let original =
        crate::media_recovery::read_json(&recovery.join("original-settings.json")).await?;
    let applied = crate::strm::recovery_applied_settings(recovery, &original).await?;
    let mut native_intent = has_pending_scan;
    let mut entries = tokio::fs::read_dir(recovery).await?;
    while let Some(entry) = entries.next_entry().await? {
        native_intent |= entry
            .file_name()
            .to_string_lossy()
            .starts_with("clear-intent-");
    }
    let preparation_only = !has_pending_scan
        && !native_intent
        && record["preparation_kind"] == "ingest-resource-guard"
        && record["owner_task_id"]
            .as_str()
            .is_some_and(|owner| !owner.is_empty() && owner == context.id)
        && crate::strm::valid_recovery_settings(&original)
        && crate::strm::matches(&crate::strm::config(state).await?, &original);
    let verify = async {
        guard.verify(state, &finishing).await?;
        guard.verify_unchanged_json().await
    };
    if preparation_only {
        // No native refresh remains and settings already match the original.
        // Discard verified duplicate preparation materials without changing
        // configuration or requiring an unrelated media-extraction task.
        verify.await?;
    } else {
        crate::strm::finish_media_recovery(state, &original, applied.as_ref(), || verify).await?;
    }
    let marker = crate::media_recovery::mark_verified(recovery, &record["target_ids"]).await?;
    crate::media_recovery::cleanup_verified(
        recovery,
        &marker,
        &state.data_dir.join("backups/media-info-conflicts"),
    )
    .await
}

/// A later admission may close an earlier ingest guard only after its original
/// resources and settings independently pass readback. Never replay old writes.
pub(crate) async fn check_previous_admissions(
    state: &AppState, ids: &[String], context: &JobContext,
) -> Result<()> {
    let root = state.data_dir.join("backups/media-info-reload");
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        context.checkpoint().await?;
        let path = entry.path();
        if !entry.file_type().await?.is_dir() || !tokio::fs::try_exists(path.join("media-reload.json")).await? {
            continue;
        }
        let result = async {
            let mut record = recovery_record(&path).await?;
            if record["preparation_kind"] != "ingest-resource-guard" || record["require_clear_scope"] != true
                || !media_edit::list(&record["target_ids"]).iter().any(|id| ids.iter().any(|candidate| id == candidate))
            {
                return Ok(());
            }
            let owner = string(&record, "owner_task_id");
            if owner == context.id || owner.is_empty() { return Ok(()); }
            let finished: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_jobs
                WHERE id=$1 AND state_json->>'state' IN ('completed','completed_with_warnings','failed','cancelled'))")
                .bind(owner).fetch_one(&state.db).await?;
            if !finished { return Ok(()); }
            let _settings = state.strm.gate.lock().await;
            let original = crate::media_recovery::read_json(&path.join("original-settings.json")).await?;
            if !crate::strm::valid_recovery_settings(&original)
                || !crate::strm::matches(&crate::strm::config(state).await?, &original)
            {
                return Err(conflict("原配置尚未通过回读"));
            }
            if tokio::fs::try_exists(path.join("pending-scan.json")).await? {
                let scan = crate::media_recovery::read_json(&path.join("pending-scan.json")).await?;
                let position = scan["position"].as_u64().ok_or_else(|| conflict("原刷新缺少日志位置"))?;
                let scan_ids = scan["ids"].as_array().filter(|ids| !ids.is_empty()).ok_or_else(|| conflict("原刷新缺少目标"))?;
                let logs = crate::media_extra::refresh_log_window(state, position).await?;
                for id in scan_ids {
                    let id = id.as_str().ok_or_else(|| conflict("原刷新目标无效"))?;
                    if !crate::media_extra::RefreshTrace::new(position).feed(id, &logs)? {
                        return Err(conflict("原目标刷新尚未确认完成"));
                    }
                }
            }
            let guard = load_saved_guard(state, &path, &mut record, context).await?;
            guard.verify(state, context).await?;
            guard.verify_unchanged_json().await?;
            context.checkpoint().await?;
            let marker = crate::media_recovery::mark_verified(&path, &record["target_ids"]).await?;
            crate::media_recovery::cleanup_verified(&path, &marker, &state.data_dir.join("backups/media-info-conflicts")).await?;
            tracing::info!("前次入库媒体、原资源和神医设置已验收，已清理恢复备份 {}", path.display());
            Ok::<(), ApiError>(())
        }.await;
        if let Err(error) = result {
            tracing::debug!("前次入库备份继续保留 {}：{}", path.display(), error.1);
        }
    }
    Ok(())
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
        if observed_scope(&members(state, &self.parent, context, &self.selected_ids).await?)
            != self.scope
        {
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
            if self.require_clear_scope {
                ensure_single_clear_scope_prepared(state, target, &live, context, &mut inventory)
                .await?;
            }
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
            let (_, live) = metadata_target_details(state, target, &self.observations[&target.id]).await?;
            let mut reasons = differences(target, &live);
            // Emby can append #DB intro/credits markers while refreshing NFO.
            // Preservation requires the original chapters to survive; additions
            // do not authorize clearing them or constitute lost media data.
            if media_info::chapters_restored(
                &media_info::object(&target.info).unwrap()["Chapters"],
                &live["Chapters"],
            ) {
                reasons.retain(|reason| reason != "章节或片头片尾标记未一致");
            }
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

    pub(crate) async fn verify_unchanged_json(&self) -> Result<()> {
        for path in &self.json_paths {
            let saved = self
                .files
                .get(path)
                .ok_or_else(|| conflict("原媒体 JSON 备份范围未确认"))?;
            check_ancestors(path).await?;
            let current = tokio::fs::read(path).await?;
            let original = snapshot_bytes(saved).await?;
            // Ingest can retain an export with additional native chapters.
            // Rescraping still requires the original JSON bytes, and cleanup
            // never rewrites the current JSON to discard newly added markers.
            if current != original
                && !(self.require_clear_scope && ingest_json_preserved(&original, &current, path)?)
            {
                return Err(conflict("重新刮削期间媒体 JSON 发生变化，原材料已保留"));
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
    capture_inner(state, item, context, recovery, false, &[], true).await
}

pub(crate) async fn capture_strict(
    state: &AppState,
    item: &Value,
    context: &JobContext,
    recovery: &Path,
) -> Result<Guard> {
    capture_inner(state, item, context, recovery, true, &[], false).await
}

pub(crate) async fn capture_targets(
    state: &AppState,
    item: &Value,
    selected_ids: &[String],
    context: &JobContext,
    recovery: &Path,
) -> Result<Guard> {
    if selected_ids.is_empty() {
        capture(state, item, context, recovery).await
    } else {
        // A submitted subset refresh needs real JSON/stream recovery evidence
        // even when its selected episode has no chapter or BIF files yet.
        capture_inner(state, item, context, recovery, true, selected_ids, true).await
    }
}

async fn capture_inner(
    state: &AppState,
    item: &Value,
    context: &JobContext,
    recovery: &Path,
    force_media: bool,
    selected_ids: &[String],
    require_clear_scope: bool,
) -> Result<Guard> {
    context.checkpoint().await?;
    let connection = connection(&*state.config.read().await);
    crate::emby::idle(state, context).await?;
    let members = members(state, item, context, selected_ids).await?;
    let mut refresh_ancestors = vec![];
    if item["Type"] == "Series" && !selected_ids.is_empty() {
        let parents = members
            .iter()
            .filter_map(|member| member["ParentId"].as_str())
            .filter(|parent| !parent.is_empty())
            .collect::<BTreeSet<_>>();
        for parent in parents {
            let current = media_edit::details(state, parent, "Id,Type,Path,ParentId").await?;
            if current["Id"] != parent
                || current["Type"] != "Season"
                || current["ParentId"] != item["Id"]
            {
                return Err(conflict("本批分集的父季身份不一致，未扩大保护范围"));
            }
            refresh_ancestors.push(current);
        }
    }
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
        selected_ids: selected_ids.to_vec(),
        require_clear_scope,
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
        if !force_media
            && media_edit::list(&member["Chapters"]).is_empty()
            && !has_thumbs
            && guard.files.len() == before_files
        {
            continue;
        }
        let mut binding = media_edit::repair_target(state, &id).await?;
        if binding.path != target.path {
            return Err(conflict("入库原资源的媒体路径在准备期间变化，未提交刷新"));
        }
        target.path = binding.path.clone();
        target.source = crate::media_paths::strm_target(&target.path).await;
        target.sha = binding.sha.clone();
        target.asset = (!binding.kind.is_empty()).then_some(binding.asset.clone());
        if !require_clear_scope {
            // This snapshot protects existing resources during metadata-only
            // refresh. It does not accept them as a completed new-file import.
            if let Some(asset) = binding.asset.as_object_mut() {
                asset.remove("moviepilot_media_pending");
            }
        }
        target.info = media_info::restore_json(&media_edit::load(state, &binding).await?)?;
        let live = identity_details(state, &target).await?;
        media_info::preserve_live_streams(
            &mut target.info,
            &live,
            &[&target.path, target.source.as_deref().unwrap_or("")],
        )?;
        media_info::preserve_chapters(&mut target.info, &live["Chapters"])?;
        if require_clear_scope {
            ensure_single_clear_scope_prepared(state, &target, &live, context, &mut inventory).await?;
        }
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
        let mut observation = live;
        observation["_ingest_guard_pick_code"] = json!(binding.pc);
        guard.observations.insert(id, observation);
        guard.targets.push(target);
    }
    if guard.protected() {
        let record = json!({"target_ids":guard.targets.iter().map(|target| &target.id).collect::<Vec<_>>(),
            "preparation_kind":"ingest-resource-guard","owner_task_id":context.id,"selected_ids":selected_ids,"require_clear_scope":require_clear_scope,
            "connection_sha256":format!("{:x}",Sha256::digest(guard.connection.to_string().as_bytes())),
            "refresh_parent":{"Id":guard.parent["Id"],"Type":guard.parent["Type"],"Path":guard.parent["Path"]},
            "refresh_ancestors":refresh_ancestors,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_json_cleanup_keeps_appended_markers_but_rejects_changed_media_or_lost_chapters() {
        let original = json!([{"MediaSourceInfo":{"Container":"mkv","Size":1286350287_u64},"Chapters":[
            {"Name":"片头","MarkerType":"Chapter","StartPositionTicks":0},
            {"Name":"正片","MarkerType":"Chapter","StartPositionTicks":1240000000_u64},
            {"Name":"片尾","MarkerType":"Chapter","StartPositionTicks":10660000000_u64}
        ]}]);
        let mut exported = original.clone();
        exported[0]["Chapters"][0]["ChapterIndex"] = json!(0);
        exported[0]["Chapters"].as_array_mut().unwrap().extend([
            json!({"Name":"IntroStart#DB","MarkerType":"IntroStart","StartPositionTicks":0}),
            json!({"Name":"IntroEnd#DB","MarkerType":"IntroEnd","StartPositionTicks":1240000000_u64}),
            json!({"Name":"CreditsStart#DB","MarkerType":"CreditsStart","StartPositionTicks":10660000000_u64}),
        ]);
        let matches = |value: &Value| ingest_json_preserved(
            &serde_json::to_vec(&original).unwrap(), &serde_json::to_vec(value).unwrap(), Path::new("/fixture/e200-mediainfo.json")
        ).unwrap();
        assert!(matches(&exported));
        let mut missing = exported.clone();
        missing[0]["Chapters"].as_array_mut().unwrap().remove(1);
        assert!(!matches(&missing));
        let mut changed = exported.clone();
        changed[0]["MediaSourceInfo"]["Container"] = json!("mp4");
        assert!(!matches(&changed));
        changed = exported;
        changed[0]["UnknownProvenance"] = json!("changed");
        assert!(!matches(&changed));
    }
}
