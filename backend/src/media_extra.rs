use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{self, details, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use futures_util::FutureExt;
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path as FsPath, PathBuf},
    time::{Duration, SystemTime},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/preview_processed_cast/{id}", post(submit_preview))
        .route(
            "/api/media_info/replace_episode_images/{id}",
            post(submit_images),
        )
        .route("/api/media_info/delete_version/{id}", post(submit_delete))
}
async fn submit_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task = state
        .jobs
        .submit("处理演员表", move |context| async move {
            preview_cast(&work, &id, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"处理演员表任务已提交"})),
    ))
}
async fn submit_images(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task = state
        .jobs
        .submit_response("替换剧集图", move |context| async move {
            replace_images(&work, &id, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"替换剧集图任务已提交"})),
    ))
}
pub async fn preview_cast(state: &AppState, id: &str, context: &JobContext) -> Result<Value> {
    context.checkpoint().await?;
    let item = details(state, id, "Id,Name,Type,People,ProviderIds,Genres,ProductionYear,SeriesId,ParentId,ParentIndexNumber,IndexNumber").await?;
    let kind = string(&item, "Type");
    if !matches!(kind.as_str(), "Movie" | "Series" | "Season" | "Episode") {
        return Err(ApiError::bad("仅支持影视演员预览"));
    }
    let config = state.config.read().await.clone();
    let bound: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1 LIMIT 1",
    )
    .bind(json!([id]))
    .fetch_optional(&state.db)
    .await?;
    let tmdb = item["ProviderIds"]["Tmdb"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| bound.as_ref().map(|r| string(r, "tmdb_id")))
        .unwrap_or_default();
    let mut raw = if !tmdb.is_empty()
        && matches!(kind.as_str(), "Movie" | "Series")
        && !string(&config, "tmdb_api_key").is_empty()
    {
        let mut data = crate::tmdb::details(state, &kind, &tmdb).await?;
        crate::douban::premerge_online(state, &item, &mut data).await?;
        list(&data["credits"]["cast"])
    } else if !tmdb.is_empty() {
        crate::metadata::reconstruct_full(state, &tmdb, &kind, true)
            .await?
            .map(|(_, actors)| actors)
            .unwrap_or_default()
    } else {
        vec![]
    };
    if raw.is_empty() {
        for (i, person) in list(&item["People"])
            .iter()
            .filter(|p| p["Type"] == "Actor")
            .enumerate()
        {
            let actor_id = string(&person["ProviderIds"], "Tmdb").parse::<i32>().ok();
            let cached: Option<Value> = if let Some(actor_id) = actor_id {
                sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('name',primary_name,'id',tmdb_person_id,'douban_id',douban_celebrity_id) FROM person_metadata p WHERE tmdb_person_id=$1").bind(actor_id).fetch_optional(&state.db).await?
            } else {
                None
            };
            let mut actor = cached.unwrap_or(json!({}));
            actor["id"] = json!(actor_id);
            if string(&actor, "name").is_empty() {
                actor["name"] = person["Name"].clone();
            }
            actor["character"] = person["Role"].clone();
            actor["order"] = json!(i);
            actor["emby_person_id"] = person["Id"].clone();
            if string(&actor, "profile_path").is_empty()
                && !string(person, "PrimaryImageTag").is_empty()
            {
                actor["profile_path"] = person["PrimaryImageTag"].clone();
            }
            raw.push(actor);
        }
    }
    context.checkpoint().await?;
    // Preview reads both caches and upstreams; it never persists people/media or changes Emby.
    let mut actors = crate::persons::preview(state, &raw, &item).await?;
    if config["ai_translate_actor_role"] == true && !string(&config, "ai_api_key").is_empty() {
        let mut terms = BTreeMap::new();
        let mut roles = serde_json::Map::new();
        for actor in &actors {
            for field in ["name", "character"] {
                let text = string(actor, field);
                if crate::translation::needs_translation(&text) {
                    terms.insert(text.clone(), text.clone());
                    if field == "character" {
                        roles.insert(text, actor["name"].clone());
                    }
                }
            }
        }
        let purpose = if config["ai_translation_mode"] == "quality" {
            "quality_mode"
        } else {
            "fast_mode"
        };
        let translated = crate::ai::translate_preview(
            state,
            &terms,
            purpose,
            &json!({"title":item["Name"],"year":item["ProductionYear"],"media_type":kind}),
            &json!(roles),
        )
        .await?;
        for actor in &mut actors {
            for field in ["name", "character"] {
                if let Some(value) = translated.get(&string(actor, field)) {
                    actor[field] = json!(value);
                }
            }
        }
        actors = crate::cast::format(&actors, &item, &config);
    }
    Ok(json!(actors.iter().map(|a| json!({"embyPersonId":a["emby_person_id"],"name":a["name"],"role":a["character"],"imdbId":a["imdb_id"],"doubanId":a.get("douban_id").unwrap_or(&a["douban_celebrity_id"]),"tmdbId":a["id"],"matchStatus":"已刷新"})).collect::<Vec<_>>()))
}

async fn configured_file(state: &AppState, path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let candidate = tokio::fs::canonicalize(path).await.ok()?;
    let config = state.config.read().await.clone();
    for key in ["local_strm_root", "cd2_media_root"] {
        if let Ok(root) = tokio::fs::canonicalize(string(&config, key)).await
            && candidate.starts_with(root)
            && candidate.is_file()
        {
            return Some(candidate);
        }
    }
    None
}
async fn episode_images(state: &AppState, path: &str) -> Vec<PathBuf> {
    let Some(local) = configured_file(state, path).await.filter(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("strm"))
    }) else {
        return vec![];
    };
    let mut stems = BTreeSet::from([local
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()]);
    if let Some(target) = crate::media_paths::strm_target(path).await {
        let clean = crate::media_paths::clean(&target);
        let parsed = reqwest::Url::parse(&clean).ok();
        let path = parsed.as_ref().map_or(clean.as_str(), |url| url.path());
        if let Some(stem) = FsPath::new(path).file_stem() {
            stems.insert(stem.to_string_lossy().into_owned());
        }
    }
    stems
        .into_iter()
        .flat_map(|stem| {
            [
                ".jpg",
                ".jpeg",
                ".png",
                "-thumb.jpg",
                "-thumb.jpeg",
                "-thumb.png",
            ]
            .map(|suffix| local.with_file_name(format!("{stem}{suffix}")))
        })
        .collect()
}
async fn atomic_write(path: &FsPath, bytes: &[u8]) -> Result<()> {
    let stage = path.with_file_name(format!(".etk-restore-{}", uuid::Uuid::new_v4().simple()));
    tokio::fs::write(&stage, bytes).await?;
    let result = tokio::fs::rename(&stage, path).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(stage).await;
    }
    result?;
    Ok(())
}
#[derive(Default)]
pub struct RefreshTrace {
    position: u64,
    pending: usize,
    completed: bool,
}
impl RefreshTrace {
    pub fn new(position: u64) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }
    pub fn feed(&mut self, id: &str, response: &Value) -> Result<bool> {
        let total = response["TotalRecordCount"]
            .as_u64()
            .ok_or_else(|| ApiError::bad("Emby刷新日志缺少行数"))?;
        let lines = response["Items"]
            .as_array()
            .ok_or_else(|| ApiError::bad("Emby刷新日志格式无效"))?;
        let lines = if lines.first().is_some_and(|l| l == "") {
            &lines[1..]
        } else {
            lines.as_slice()
        };
        let added = if total >= self.position {
            total - self.position
        } else {
            total
        } as usize;
        if added > lines.len() {
            return Err(ApiError::bad("Emby日志增长超出读取窗口，无法完整验收刷新"));
        }
        let pattern = regex::Regex::new(&format!(
            r"RefreshItem (Start|Complete): {}\s",
            regex::escape(id)
        ))
        .unwrap();
        for line in lines[..added].iter().rev() {
            let line = line
                .as_str()
                .ok_or_else(|| ApiError::bad("Emby日志行格式无效"))?;
            if let Some(found) = pattern.captures(line) {
                if &found[1] == "Start" {
                    self.pending += 1;
                } else if self.pending > 0 {
                    self.pending -= 1;
                    self.completed = true;
                }
            }
        }
        self.position = total;
        Ok(self.completed && self.pending == 0)
    }
}
pub(crate) async fn log_position(state: &AppState) -> Result<u64> {
    state
        .emby(
            Method::GET,
            "System/Logs/embyserver.txt/Lines",
            &[("Limit", "0".into())],
            None,
        )
        .await?["TotalRecordCount"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("无法读取Emby刷新日志，已停止操作"))
}
async fn delete_image(state: &AppState, id: &str) -> Result<bool> {
    let config = state.config.read().await.clone();
    let response = crate::emby::send(
        state
            .http
            .delete(format!(
                "{}/Items/{id}/Images/Primary",
                string(&config, "emby_server_url").trim_end_matches('/')
            ))
            .header("X-Emby-Token", string(&config, "emby_api_key")),
    )
    .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    response.error_for_status()?;
    Ok(true)
}
async fn changed(states: &BTreeMap<PathBuf, Option<SystemTime>>) -> usize {
    let mut count = 0;
    for (path, before) in states {
        if tokio::fs::metadata(path).await.is_ok_and(|m| {
            m.len() > 0
                && m.modified()
                    .ok()
                    .is_some_and(|after| before.is_none_or(|before| after > before))
        }) {
            count += 1;
        }
    }
    count
}
async fn save_recovery(
    state: &AppState,
    id: &str,
    snapshots: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<PathBuf> {
    let root = state
        .data_dir
        .join("episode-image-recovery")
        .join(format!("{id}-{}", uuid::Uuid::new_v4().simple()));
    tokio::fs::create_dir_all(&root).await?;
    let mut manifest = serde_json::Map::new();
    for (i, (path, bytes)) in snapshots.iter().enumerate() {
        let name = format!("{i}.json");
        tokio::fs::write(root.join(&name), bytes).await?;
        manifest.insert(path.to_string_lossy().into_owned(), json!(name));
    }
    tokio::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .await?;
    Ok(root)
}
pub async fn replace_images(
    state: &AppState,
    id: &str,
    context: &JobContext,
) -> Result<(u16, Value)> {
    context.checkpoint().await?;
    let item = details(state, id, "Id,Type,Name,ParentId,SeriesId,Path,ImageTags").await?;
    let kind = string(&item, "Type");
    if !matches!(kind.as_str(), "Series" | "Season" | "Episode") {
        return Err(ApiError::bad("只有电视剧、季度或单集可以替换剧集图"));
    }
    let mut episodes = if kind == "Episode" {
        vec![item.clone()]
    } else {
        crate::emby::query_items(
            state,
            &[
                ("ParentId", id.into()),
                ("IncludeItemTypes", "Episode".into()),
                ("Recursive", "true".into()),
                (
                    "Fields",
                    "Id,Name,Type,ParentId,SeriesId,SeriesName,ParentIndexNumber,IndexNumber,Path,ImageTags".into(),
                ),
            ],
            context,
        )
        .await?
    };
    crate::media_order::sort_items(state, &mut episodes).await?;
    if episodes.is_empty() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "当前范围没有找到已入库的分集".into(),
        ));
    }
    let config = state.config.read().await.clone();
    let mut snapshots = BTreeMap::new();
    let mut states = BTreeMap::new();
    for episode in &episodes {
        for path in crate::media_paths::info_candidates(
            &string(episode, "Path"),
            &string(&config, "local_strm_root"),
            &string(&config, "cd2_media_root"),
            "",
        )
        .await
        {
            if let Some(path) = configured_file(state, path.to_str().unwrap_or("")).await {
                snapshots.insert(path.clone(), tokio::fs::read(&path).await?);
            }
        }
        let thumbs = episode_images(state, &string(episode, "Path"))
            .await
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains("-thumb."))
            })
            .collect::<Vec<_>>();
        if let Some(path) = thumbs
            .iter()
            .find(|p| p.is_file())
            .or_else(|| thumbs.first())
        {
            states.insert(
                path.clone(),
                tokio::fs::metadata(path)
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok()),
            );
        }
    }
    let mut failures = vec![];
    let mut deleted = 0;
    let mut missing = 0;
    let mut started = false;
    let mut complete = false;
    let mut generated = 0;
    context.checkpoint().await?;
    log_position(state).await?;
    let outcome = crate::strm::with_settings(state, "capture", || async {
        context.checkpoint().await?;
        for (i, episode) in episodes.iter().enumerate() {
            // Once an image is removed, finish the accepted refresh and restoration even on stop.
            match delete_image(state, &string(episode, "Id")).await {
                Ok(true) => deleted += 1,
                Ok(false) => missing += 1,
                Err(e) => failures.push(format!(
                    "{}: 删除Primary失败：{}",
                    string(episode, "Id"),
                    e.1
                )),
            }
            context
                .update(
                    (10 * (i + 1) / episodes.len()) as u64,
                    "清理旧图",
                    &format!("正在清理旧图 {}/{}", i + 1, episodes.len()),
                )
                .await;
        }
        if deleted == 0 && missing == 0 {
            return Ok(());
        }
        for episode in &episodes {
            for path in episode_images(state, &string(episode, "Path")).await {
                if path.is_file() {
                    tokio::fs::remove_file(path).await?;
                }
            }
        }
        let position = log_position(state).await?;
        started = true;
        state
            .emby(
                Method::POST,
                &format!("Items/{id}/Refresh"),
                &[
                    ("Recursive", (kind != "Episode").to_string()),
                    ("MetadataRefreshMode", "FullRefresh".into()),
                    ("ImageRefreshMode", "FullRefresh".into()),
                    ("ReplaceAllMetadata", "true".into()),
                    ("ReplaceAllImages", "true".into()),
                    ("ReplaceThumbnailImages", "false".into()),
                    ("ReplaceIntroMarkers", "false".into()),
                ],
                None,
            )
            .await?;
        let capture = crate::strm::config(state).await?;
        let per_episode = (capture["MediaInfoExtractionTimeoutMs"]
            .as_u64()
            .unwrap_or(60000)
            / 1000
            * capture["ImageExtractionTimeoutMultiplier"]
                .as_u64()
                .unwrap_or(2)
            + 30)
            .max(180);
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs((episodes.len() as u64 * per_episode).max(600));
        let mut trace = RefreshTrace::new(position);
        loop {
            // An Emby refresh already in flight must finish before restoring the original files/settings.
            let lines = state
                .emby(
                    Method::GET,
                    "System/Logs/embyserver.txt/Lines",
                    &[("Limit", "500".into())],
                    None,
                )
                .await?;
            generated = changed(&states).await;
            context
                .update(
                    10 + (80 * generated / episodes.len()) as u64,
                    if context.is_cancelled() {
                        "等待刷新结束后恢复配置"
                    } else {
                        "生成截图"
                    },
                    &format!(
                        "{}：截图 {}/{}",
                        string(&item, "Name"),
                        generated,
                        episodes.len()
                    ),
                )
                .await;
            if trace.feed(id, &lines)? {
                complete = true;
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ApiError::bad("未确认Emby完成本次刷新"));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if states.is_empty() {
            generated = 0;
            for episode in &episodes {
                let live = details(state, &string(episode, "Id"), "Id,ImageTags").await?;
                generated += usize::from(!string(&live["ImageTags"], "Primary").is_empty());
            }
        }
        if generated < episodes.len() {
            failures.push(format!(
                "仍有 {} 集未生成新截图",
                episodes.len() - generated
            ));
        }
        Ok(())
    })
    .await;
    if started && !complete {
        let recovery = save_recovery(state, id, &snapshots).await?;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "未确认Emby刷新结束，已保留原媒体信息备份：{}；未提前覆盖正在刷新的文件",
                recovery.display()
            ),
        ));
    }
    if complete {
        async {
            let changed = snapshots
                .iter()
                .filter(|(path, bytes)| std::fs::read(path).ok().as_ref() != Some(bytes))
                .collect::<Vec<_>>();
            let changed_paths = changed
                .iter()
                .map(|(p, _)| (*p).clone())
                .collect::<BTreeSet<_>>();
            let mut protected = vec![];
            for episode in &episodes {
                let candidates = crate::media_paths::info_candidates(
                    &string(episode, "Path"),
                    &string(&config, "local_strm_root"),
                    &string(&config, "cd2_media_root"),
                    "",
                )
                .await;
                if candidates.iter().any(|p| changed_paths.contains(p)) {
                    protected.push(episode);
                }
            }
            for episode in &protected {
                if let Err(e) = state
                    .emby(
                        Method::POST,
                        &format!("Items/{}/ClearMediaInfo", string(episode, "Id")),
                        &[],
                        None,
                    )
                    .await
                {
                    failures.push(e.1);
                }
            }
            if !protected.is_empty() {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            for (path, bytes) in changed {
                if let Err(e) = atomic_write(path, bytes).await {
                    failures.push(e.1);
                }
            }
            if !protected.is_empty() {
                if let Err(e) = crate::media_save::refresh(
                    state,
                    &protected
                        .iter()
                        .map(|e| string(e, "Path"))
                        .collect::<Vec<_>>(),
                )
                .await
                {
                    failures.push(e.1);
                }
                let mut pending = protected
                    .iter()
                    .map(|e| string(e, "Id"))
                    .collect::<BTreeSet<_>>();
                for _ in 0..120 {
                    let mut ready = vec![];
                    for id in &pending {
                        if details(state, id, "MediaStreams,MediaSources")
                            .await
                            .is_ok_and(|live| {
                                crate::media_info::container(&live).is_some_and(|source| {
                                    list(&source["MediaStreams"]).iter().any(|stream| {
                                        matches!(stream["Type"].as_str(), Some("Video" | "Audio"))
                                    })
                                })
                            })
                        {
                            ready.push(id.clone());
                        }
                    }
                    for id in ready {
                        pending.remove(&id);
                    }
                    if pending.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                if !pending.is_empty() {
                    failures.push(format!(
                        "恢复后Emby媒体信息仍为空：{}",
                        pending.into_iter().collect::<Vec<_>>().join(",")
                    ));
                }
            }
            Ok::<_, ApiError>(())
        }
        .await?;
    }
    outcome?;
    Ok((
        if failures.is_empty() { 200 } else { 207 },
        json!({"message":format!("剧集图处理完成：已生成 {}/{} 集，失败 {} 项",generated,episodes.len(),failures.len()),"submitted_count":if started{episodes.len()}else{0},"episode_count":episodes.len(),"deleted_count":deleted,"missing_count":missing,"refreshed_count":generated,"failures":failures}),
    ))
}

async fn submit_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task = state
        .jobs
        .submit("删除媒体版本", move |context| async move {
            delete_version(
                &work,
                &id,
                &context,
                &[1, 2, 4, 8, 15].map(Duration::from_secs),
            )
            .await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"删除媒体版本任务已提交"})),
    ))
}
struct DeleteResource {
    target: media_edit::Target,
    local: PathBuf,
    fid: String,
    source: String,
}
async fn verify_delete_resources(state: &AppState, resources: &[DeleteResource]) -> Result<()> {
    for resource in resources {
        let current = media_edit::target(state, &resource.target.id).await?;
        let source = crate::media_paths::strm_target(resource.local.to_str().unwrap_or(""))
            .await
            .unwrap_or(current.path.clone());
        if current.path != resource.target.path
            || current.sha != resource.target.sha
            || current.asset != resource.target.asset
            || source != resource.source
            || !resource.local.is_file()
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "文件身份在准备删除期间变化，已停止删除".into(),
            ));
        }
    }
    Ok(())
}
async fn delete_resource(state: &AppState, id: &str) -> Result<DeleteResource> {
    let target = media_edit::target(state, id).await?;
    if target.asset["fingerprint_pending"] == true {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "文件身份待校验，停止删除，请先重新校验指纹".into(),
        ));
    }
    let live = details(state, id, "Id,Path,Type,MediaSources").await?;
    let path = live["Path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .unwrap_or(&target.path);
    let source = crate::media_paths::strm_target(path)
        .await
        .unwrap_or_else(|| path.into());
    let config = state.config.read().await.clone();
    let pc = crate::fingerprints::pickcode(&source);
    let candidates = crate::media_paths::local_candidates(
        path,
        &source,
        &[
            &string(&config, "local_strm_root"),
            &string(&config, "cd2_media_root"),
        ],
    );
    let cache:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_filesystem_cache c WHERE ($1::text IS NOT NULL AND pick_code=$1) OR ($1::text IS NULL AND $2::text IS NOT NULL AND UPPER(sha1)=UPPER($2)) OR ($1::text IS NULL AND $2::text IS NULL AND trim(both '/' from replace(local_path,chr(92),'/'))=ANY($3)) ORDER BY CASE WHEN pick_code=$1 THEN 0 ELSE 1 END LIMIT 1").bind(pc.as_ref()).bind(&target.sha).bind(&candidates).fetch_optional(&state.db).await?;
    let mut paths = vec![PathBuf::from(path), PathBuf::from(&target.path)];
    if let Some(cache) = &cache {
        let logical = string(cache, "local_path");
        let root = string(&config, "local_strm_root");
        for raw in [
            logical.clone(),
            format!(
                "{}/{}",
                root.trim_end_matches('/'),
                logical.trim_start_matches('/')
            ),
        ] {
            let path = PathBuf::from(raw);
            paths.push(path.clone());
            paths.push(path.with_extension("strm"));
            paths.push(path.with_file_name(format!(
                "{}.strm",
                path.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
    }
    let mut local = None;
    let root = tokio::fs::canonicalize(string(&config, "local_strm_root"))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::CONFLICT,
                "未配置有效的本地STRM根目录，已拒绝删除".into(),
            )
        })?;
    for path in paths {
        if let Some(path) = configured_file(state, path.to_str().unwrap_or("")).await
            && path.starts_with(&root)
        {
            local = Some(path);
            break;
        }
    }
    let local =
        local.ok_or_else(|| ApiError(StatusCode::CONFLICT, "未找到本地STRM，已拒绝删除".into()))?;
    let source = crate::media_paths::strm_target(local.to_str().unwrap_or(""))
        .await
        .unwrap_or(source);
    let fid = cache
        .as_ref()
        .map(|c| string(c, "id"))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            pc.as_ref()
                .and_then(|p| crate::fingerprints::file_id(p).map(|id| id.to_string()))
        })
        .unwrap_or_default();
    Ok(DeleteResource {
        target,
        local,
        fid,
        source,
    })
}
pub async fn local_asset_paths(primary: &FsPath, info: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let directory = primary
        .parent()
        .ok_or_else(|| ApiError::bad("本地媒体目录无效"))?;
    let mut entries = tokio::fs::read_dir(directory).await?;
    let mut files = vec![];
    let mut anchors = vec![];
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("strm"))
            {
                anchors.push(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
            files.push(path);
        }
    }
    let anchor = primary
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut output = BTreeSet::from([primary.to_path_buf()]);
    output.extend(info.iter().filter(|p| p.is_file()).cloned());
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy();
        let owns = |value: &str| {
            name.strip_prefix(value)
                .is_some_and(|s| s.starts_with(['.', '-']))
        };
        if owns(&anchor)
            && anchors
                .iter()
                .filter(|a| owns(a))
                .max_by_key(|s| s.len())
                .is_none_or(|a| a == &anchor)
        {
            output.insert(path);
        }
    }
    Ok(output)
}
async fn resolve_missing_fid(state: &AppState, resource: &DeleteResource) -> Result<String> {
    let clean = crate::media_paths::clean(&resource.source);
    let parsed = reqwest::Url::parse(&clean).ok();
    let path = parsed.as_ref().map_or(clean.as_str(), |url| url.path());
    let name = FsPath::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        return Err(ApiError::bad("缺少云端文件名，无法安全核对115"));
    }
    let response = crate::p115::search(state, &name).await?;
    if !crate::p115::success(&response) {
        return Err(ApiError::bad("115搜索未成功，已停止删除"));
    }
    let raw = response["data"]
        .as_array()
        .or_else(|| response["data"]["list"].as_array())
        .or_else(|| response["data"]["items"].as_array())
        .or_else(|| response["data"]["files"].as_array())
        .ok_or_else(|| ApiError::bad("115搜索返回无效文件列表"))?;
    if raw.len() >= 100
        || response["count"]
            .as_u64()
            .or_else(|| response["total"].as_u64())
            .is_some_and(|n| n > raw.len() as u64)
    {
        return Err(ApiError::bad("115搜索结果未完整返回，停止删除"));
    }
    let config = state.config.read().await.clone();
    let candidates = crate::media_paths::local_candidates(
        resource.local.to_str().unwrap_or(""),
        &resource.source,
        &[
            &string(&config, "local_strm_root"),
            &string(&config, "cd2_media_root"),
        ],
    );
    let mut matched = BTreeSet::new();
    for item in raw {
        let candidate_name = ["name", "file_name", "fileName"]
            .iter()
            .find_map(|k| item[*k].as_str())
            .unwrap_or("");
        if candidate_name != name {
            continue;
        }
        let fid = ["id", "fid", "file_id"]
            .iter()
            .find_map(|k| {
                item[*k]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| item[*k].as_u64().map(|n| n.to_string()))
            })
            .ok_or_else(|| ApiError::bad("115同名文件缺少ID"))?;
        let info = crate::p115::file_info(
            state,
            fid.parse().map_err(|_| ApiError::bad("115文件ID无效"))?,
        )
        .await?;
        let paths = info["paths"]
            .as_array()
            .or_else(|| info["path"].as_array())
            .ok_or_else(|| ApiError::bad("115同名文件缺少完整路径，停止删除"))?;
        let mut parts = paths
            .iter()
            .filter_map(|p| {
                ["file_name", "name", "fn", "n"]
                    .iter()
                    .find_map(|k| p[*k].as_str())
            })
            .filter(|n| !n.is_empty() && !matches!(*n, "根目录" | "全部文件"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        parts.push(name.clone());
        let full = parts.join("/");
        if !full.contains('/') {
            return Err(ApiError::bad("115同名文件缺少目录，停止删除"));
        }
        if candidates.iter().any(|c| {
            full == *c || full.ends_with(&format!("/{c}")) || c.ends_with(&format!("/{full}"))
        }) {
            matched.insert(fid);
        }
    }
    if matched.len() > 1 {
        return Err(ApiError::bad("115存在多个同名文件，停止删除避免误删"));
    }
    Ok(matched.into_iter().next().unwrap_or_default())
}
struct LocalSnapshot {
    stage: PathBuf,
    roots: Vec<(PathBuf, PathBuf)>,
    targets: BTreeSet<PathBuf>,
}
async fn copy_tree(source: &FsPath, destination: &FsPath) -> Result<()> {
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        tokio::fs::create_dir_all(&to).await?;
        let mut entries = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            let path = entry.path();
            let out = to.join(entry.file_name());
            if kind.is_dir() {
                pending.push((path, out));
            } else if kind.is_file() {
                tokio::fs::copy(path, out).await?;
            } else {
                return Err(ApiError::bad("媒体目录含符号链接或特殊文件，已停止删除"));
            }
        }
    }
    Ok(())
}
impl LocalSnapshot {
    async fn create(state: &AppState, targets: BTreeSet<PathBuf>) -> Result<Self> {
        let stage = state
            .data_dir
            .join(format!("version-delete-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir_all(&stage).await?;
        let roots = targets
            .iter()
            .filter_map(|p| p.parent().map(FsPath::to_path_buf))
            .collect::<BTreeSet<_>>();
        let stage_real = tokio::fs::canonicalize(&stage).await?;
        if roots.iter().any(|root| stage_real.starts_with(root)) {
            let _ = tokio::fs::remove_dir_all(&stage).await;
            return Err(ApiError::bad(
                "版本备份目录位于待保护媒体目录内，已停止删除",
            ));
        }
        let mut output = Self {
            stage: stage.clone(),
            roots: vec![],
            targets,
        };
        for (i, root) in roots.into_iter().enumerate() {
            let backup = stage.join(i.to_string());
            if let Err(e) = copy_tree(&root, &backup).await {
                let _ = tokio::fs::remove_dir_all(stage).await;
                return Err(e);
            }
            output.roots.push((root, backup));
        }
        Ok(output)
    }
    async fn restore(&self, include_target: bool) -> Result<()> {
        for (root, backup) in &self.roots {
            let mut pending = vec![backup.clone()];
            while let Some(dir) = pending.pop() {
                let mut entries = tokio::fs::read_dir(dir).await?;
                while let Some(entry) = entries.next_entry().await? {
                    let path = entry.path();
                    let kind = entry.file_type().await?;
                    if kind.is_dir() {
                        pending.push(path);
                    } else if kind.is_file() {
                        let destination = root.join(path.strip_prefix(backup).unwrap());
                        if include_target || !self.targets.contains(&destination) {
                            tokio::fs::create_dir_all(destination.parent().unwrap()).await?;
                            atomic_write(&destination, &tokio::fs::read(path).await?).await?;
                        }
                    }
                }
            }
        }
        if !include_target {
            for path in &self.targets {
                match tokio::fs::remove_file(path).await {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
}
async fn regroup(state: &AppState, ids: &[String]) -> Result<()> {
    if ids.len() > 1 {
        state
            .emby(
                Method::POST,
                "Videos/MergeVersions",
                &[("Ids", ids.join(","))],
                None,
            )
            .await?;
    }
    Ok(())
}
pub async fn delete_version(
    state: &AppState,
    id: &str,
    context: &JobContext,
    protection_delays: &[Duration],
) -> Result<Value> {
    context.checkpoint().await?;
    let target = media_edit::target(state, id).await?;
    if target.asset["fingerprint_pending"] == true {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "文件身份待校验，停止删除，请先重新校验指纹".into(),
        ));
    }
    let mut versions = target
        .versions
        .iter()
        .map(|v| string(v, "emby_id"))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if !versions.contains(&id.into()) {
        versions.push(id.into());
    }
    let mut seen = BTreeSet::new();
    versions.retain(|id| seen.insert(id.clone()));
    if versions.len() < 2 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "至少保留一个媒体版本，不能删除最后一个版本".into(),
        ));
    }
    let live = details(state, id, "Id,PartCount,MediaSources,Path").await?;
    let mut resource_ids = vec![id.into()];
    if live["PartCount"].as_u64().unwrap_or(1) > 1 {
        resource_ids.extend(
            media_edit::additional_parts(state, id, &live)
                .await?
                .iter()
                .map(|p| string(p, "Id"))
                .filter(|id| !id.is_empty()),
        );
    }
    let auth = crate::db::setting(&state.db, "p115_auth_tokens")
        .await?
        .unwrap_or(json!({}));
    if string(&auth, "cookie").is_empty() && string(&auth, "access_token").is_empty() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "115客户端未初始化，已拒绝删除".into(),
        ));
    }
    let mut targets = BTreeSet::new();
    let mut fids = BTreeSet::new();
    let mut resources = vec![];
    // All local/cloud identities and the complete snapshot are resolved before Emby deletion.
    for resource_id in &resource_ids {
        context.checkpoint().await?;
        let mut resource = delete_resource(state, resource_id).await?;
        if resource.fid.is_empty() {
            resource.fid = resolve_missing_fid(state, &resource).await?;
        }
        if !resource.fid.is_empty() {
            fids.insert(resource.fid.clone());
        }
        let infos = resource
            .target
            .candidates
            .iter()
            .filter(|p| p.is_file())
            .filter(|p| p.parent() == resource.local.parent())
            .cloned()
            .collect::<Vec<_>>();
        targets.extend(local_asset_paths(&resource.local, &infos).await?);
        resources.push(resource);
    }
    let snapshot = LocalSnapshot::create(state, targets).await?;
    let remaining = versions
        .iter()
        .filter(|v| *v != id)
        .cloned()
        .collect::<Vec<_>>();
    let mut split = false;
    let mut deleted = false;
    let mut committed = false;
    let mut recovery_failed = false;
    let outcome=crate::strm::with_settings(state,"preserve",|| async {
        let result = std::panic::AssertUnwindSafe(async {
        context.checkpoint().await?;
        verify_delete_resources(state, &resources).await?;
        state.emby(Method::POST,&format!("Videos/{}/AlternateSources/Delete",versions[0]),&[],None).await?;split=true;
        let survivors=crate::versions::existing(state,&versions).await?;if versions.iter().any(|v|!survivors.contains(v)){return Err(ApiError::bad("Emby版本组拆分后未保留全部版本，源文件未删除"));}
        context.checkpoint().await?;
        verify_delete_resources(state, &resources).await?;
        // Finish source rollback/protection after this point even if a stop is requested.
        state.webhooks.mark_manual_delete(&versions).await;
        state.emby(Method::DELETE,&format!("Items/{id}"),&[],None).await?;deleted=true;
        let survivors=crate::versions::existing(state,&remaining).await?;if remaining.iter().any(|v|!survivors.contains(v)){return Err(ApiError::bad("Emby连带删除了其他版本，115和本地源文件已保留，请扫描媒体库恢复"));}
        if !fids.is_empty(){crate::p115::delete_files(state,&fids.iter().cloned().collect::<Vec<_>>()).await?;}
        committed=true;
        snapshot.restore(false).await?;
        if !fids.is_empty(){sqlx::query("DELETE FROM p115_filesystem_cache WHERE id=ANY($1)").bind(fids.iter().cloned().collect::<Vec<_>>()).execute(&state.db).await?;}
        crate::versions::unbind(state,&[id.into()]).await?;
        let merge_warning=if regroup(state,&remaining).await.is_err(){"；剩余版本重新合并失败，请手动刷新版本组"}else{""};
        crate::media_save::refresh(state,&snapshot.targets.iter().map(|p|p.to_string_lossy().into_owned()).collect::<Vec<_>>()).await?;
        for (i,delay)in protection_delays.iter().enumerate(){context.update(85+(14*(i+1)/protection_delays.len())as u64,"保护剩余版本","正在安全收尾，恢复外部增量任务可能误删的剩余版本文件").await;tokio::time::sleep(*delay).await;snapshot.restore(false).await?;}
        Ok(json!({"message":format!("指定媒体版本已删除，115、STRM、媒体信息和Emby已刷新{merge_warning}"),"deleted_item_id":id,"remaining_item_id":remaining.first()}))
        }).catch_unwind().await.unwrap_or_else(|_| Err(ApiError::bad("版本删除执行异常")));
    if !deleted {
        state.webhooks.clear_manual_delete(&versions).await;
    }
    if result.is_err() && (split || deleted) {
        if let Err(e) = snapshot.restore(!committed).await {
            recovery_failed = true;
            tracing::error!(
                "版本删除恢复失败，保留备份 {}: {}",
                snapshot.stage.display(),
                e.1
            );
            return Err(ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("版本删除恢复失败，备份已保留：{}", snapshot.stage.display()),
            ));
        }
        if split && !deleted {
            let _ = regroup(state, &versions).await;
        }
    }
        result
    }).await;
    if recovery_failed {
        return outcome;
    }
    let _ = tokio::fs::remove_dir_all(snapshot.stage).await;
    outcome
}
