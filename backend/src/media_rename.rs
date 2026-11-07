use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{self, list},
    rename,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/media_rename/{id}", get(context))
        .route("/api/media_rename/{id}/preview", post(preview))
        .route("/api/media_rename/{id}/ffprobe", post(submit_probe))
        .route("/api/media_rename/{id}/execute", post(submit))
        .route("/api/config/rename_preview", post(sample))
}
struct Source {
    target: media_edit::Target,
    item: Value,
    cache: Option<Value>,
    local: PathBuf,
    cloud: Option<String>,
    source: String,
    ext: String,
    fields: Value,
    identity: Value,
}
async fn source(state: &AppState, id: &str) -> Result<Source> {
    let target = media_edit::target(state, id).await?;
    if target.asset["fingerprint_pending"] == true {
        return Err(ApiError::bad("文件身份待校验"));
    }
    let item=media_edit::details(state,id,"Id,Name,OriginalTitle,ProductionYear,ProviderIds,Type,IndexNumber,ParentIndexNumber,Path,Size,MediaSources").await?;
    if item["Id"] != id || string(&item, "Path") != target.path {
        return Err(ApiError::bad(
            "Emby媒体身份或路径与缓存不一致，请先同步媒体数据",
        ));
    }
    crate::identity::verify(&target.asset, &item)?;
    if !matches!(item["Type"].as_str(), Some("Movie" | "Episode" | "Video")) {
        return Err(ApiError::bad("请先选择电影或具体分集"));
    }
    let pc = crate::fingerprints::pickcode(
        &crate::media_paths::strm_target(&target.path)
            .await
            .unwrap_or(target.path.clone()),
    );
    let config = state.config.read().await.clone();
    let paths = crate::media_paths::local_candidates(
        &target.path,
        "",
        &[
            &string(&config, "local_strm_root"),
            &string(&config, "cd2_media_root"),
        ],
    );
    let cache:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_filesystem_cache c WHERE (pick_code=$1 OR ($2::text IS NOT NULL AND UPPER(sha1)=UPPER($2)) OR trim(both '/' from replace(local_path,chr(92),'/'))=ANY($3)) AND ($1::text IS NULL OR pick_code IS NULL OR pick_code=$1) AND ($2::text IS NULL OR sha1 IS NULL OR UPPER(sha1)=UPPER($2)) ORDER BY CASE WHEN pick_code=$1 THEN 0 WHEN UPPER(sha1)=UPPER($2) THEN 1 ELSE 2 END LIMIT 1").bind(pc).bind(&target.sha).bind(paths).fetch_optional(&state.db).await?;
    let mut cloud = crate::media_paths::strm_target(&target.path).await;
    if cloud.is_none()
        && let Some(cache) = &cache
    {
        let logical = string(cache, "local_path");
        if !["", ".strm"].contains(&crate::media_paths::extension(&logical).as_str()) {
            cloud = Some(rename::configured_cloud(&logical, &config));
        }
    }
    let local = rename::local_candidates(
        &target.path,
        cloud.as_deref().unwrap_or(""),
        cache.as_ref().unwrap_or(&Value::Null),
        &config,
    )
    .await
    .into_iter()
    .next()
    .ok_or_else(|| ApiError::bad("未找到本地STRM或媒体文件"))?;
    if rename::short_name(&local.file_name().unwrap_or_default().to_string_lossy()) {
        cloud = cloud.or(rename::stream_target(&local).await);
    } else if local
        .extension()
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("strm"))
    {
        cloud = rename::stream_target(&local).await.or(cloud);
    }
    let cloud_text = cloud.as_deref().unwrap_or("");
    let source = [
        cloud_text,
        target.path.as_str(),
        cache
            .as_ref()
            .and_then(|c| c["name"].as_str())
            .unwrap_or(""),
    ]
    .iter()
    .find_map(|s| {
        let ext = crate::media_paths::extension(s);
        if ext.is_empty() || ext == ".strm" {
            return None;
        }
        Some(
            std::path::Path::new(&crate::media_paths::clean(s))
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        )
    })
    .ok_or_else(|| ApiError::bad("无法确定源文件扩展名"))?;
    let ext = crate::media_paths::extension(&source);
    let fields = rename::context(
        &item,
        &source,
        &json!({"videoFormat":target.asset["resolution_display"].as_str().unwrap_or(""),"videoCodec":target.asset["codec_display"].as_str().unwrap_or(""),"effect":target.asset["effect_display"].as_str().unwrap_or("")}),
    );
    let metadata = tokio::fs::metadata(&local).await?;
    let strm = if local
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("strm"))
        || rename::short_name(&local.file_name().unwrap_or_default().to_string_lossy())
    {
        Some(tokio::fs::read(&local).await?)
    } else {
        None
    };
    let cache_identity = cache.as_ref().map(|cache| json!({"id":cache["id"],"parent_id":cache["parent_id"],"pick_code":cache["pick_code"],"sha1":cache["sha1"],"name":cache["name"],"local_path":cache["local_path"]}));
    let identity = json!({"emby_id":id,"emby_path":item["Path"],"asset":target.asset,"sha1":target.sha,"cache":cache_identity,"local":local,"size":metadata.len(),"modified":format!("{:?}",metadata.modified()?),"strm_bytes":strm});
    Ok(Source {
        target,
        item,
        cache,
        local,
        cloud,
        source,
        ext,
        fields,
        identity,
    })
}
async fn context(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    let source = source(&state, &id).await?;
    let config = state.config.read().await;
    let movie = source.item["Type"] == "Movie";
    Ok(Json(
        json!({"item_id":id,"item_type":source.item["Type"],"source_name":source.source,"file_ext":source.ext,"media_path":source.target.path,"cloud_path":source.cloud,"fields":source.fields,"rename_mode":if movie{"movie"}else{"tv"},"template":config[if movie{"movie_rename_format"}else{"tv_rename_format"}],"ffprobe_fields":{},"versions":source.target.versions}),
    ))
}
async fn plan(state: &AppState, source: &Source, body: &Value) -> Result<Value> {
    let mut fields = source.fields.clone();
    if !body["fields"].is_null() && !body["fields"].is_object() {
        return Err(ApiError::bad("fields必须是对象"));
    }
    for (key, value) in body["fields"].as_object().into_iter().flatten() {
        if key != "fileExt" && !value.is_null() {
            fields[key] = value.clone();
        }
    }
    for (key, value) in rename::pt(&string(body, "custom_name"))
        .as_object()
        .into_iter()
        .flatten()
    {
        if !["year", "season", "episode", "season_episode"].contains(&key.as_str()) {
            fields[key] = value.clone();
        }
    }
    if let Some(part) = body["fields"].get("part") {
        fields["part"] = part.clone();
    }
    let config = state.config.read().await;
    let default = string(
        &config,
        if source.item["Type"] == "Movie" {
            "movie_rename_format"
        } else {
            "tv_rename_format"
        },
    );
    let template = if body["mode"] == "custom" {
        default
    } else {
        body["template"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(default)
    };
    let probe = if body["use_ffprobe"] == true {
        probe_fields(state, source, None).await
    } else {
        json!({})
    };
    for (key, value) in probe.as_object().into_iter().flatten() {
        if fields[key].is_null() || fields[key] == "" {
            fields[key] = value.clone();
        }
    }
    let rendered = rename::render(&template, &fields, &source.ext)?;
    let parts = rendered.split('/').collect::<Vec<_>>();
    let directories = parts[..parts.len() - 1]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let filename = parts.last().unwrap().to_string();
    let layout = if source.cloud.is_some()
        && !source
            .local
            .to_string_lossy()
            .to_lowercase()
            .ends_with(".strm")
    {
        format!("{}.strm", source.local.display())
    } else {
        source.local.to_string_lossy().into_owned()
    };
    let local = rename::local(&layout, &directories, &filename)?;
    let cloud = source
        .cloud
        .as_deref()
        .map(|p| rename::replace(p, &directories, &filename))
        .transpose()?;
    Ok(
        json!({"rendered":rendered,"directories":directories,"filename":filename,"local_path":local,"cloud_path":cloud,"fields":fields,"ffprobe_fields":probe}),
    )
}
async fn probe_fields(state: &AppState, source: &Source, context: Option<&JobContext>) -> Value {
    let path = source.cloud.clone().unwrap_or(source.target.path.clone());
    if source.ext == ".iso" {
        let probe = if std::path::Path::new(&path).is_file() {
            let Ok(_slot) = crate::strm::slot(state, "main", context).await else {
                return json!({});
            };
            crate::p115_iso::probe_file(std::path::Path::new(&path), context).await
        } else {
            let pc = source
                .cache
                .as_ref()
                .map(|cache| string(cache, "pick_code"))
                .filter(|s| !s.is_empty())
                .or_else(|| crate::fingerprints::pickcode(&path))
                .unwrap_or_default();
            let size = source
                .cache
                .as_ref()
                .and_then(|cache| cache["size"].as_u64())
                .unwrap_or(0);
            if pc.is_empty() {
                let Ok(_slot) = crate::strm::slot(state, "main", context).await else {
                    return json!({});
                };
                crate::p115_iso::probe_reader(
                    crate::p115_iso::RangeReader::http(state.http.clone(), &path, "Mozilla/5.0")
                        .with_context(context),
                    &source.source,
                    size,
                    "strm-url",
                )
                .await
            } else {
                crate::p115_iso::probe_115(state, &pc, &source.source, size, context).await
            }
        };
        if let Ok(raw) = probe {
            let Ok(_slot) = crate::strm::slot(state, "secondary", context).await else {
                return json!({});
            };
            return rename::probe_fields(&raw);
        }
        if context.is_some_and(JobContext::is_cancelled) {
            return json!({});
        }
    }
    let Ok(_slot) = crate::strm::slot(state, "main", context).await else {
        return json!({});
    };
    let mut command = tokio::process::Command::new("ffprobe");
    command.kill_on_drop(true).args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_streams",
        "-show_format",
        "-i",
        &path,
    ]);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output()).await;
    drop(_slot);
    let Ok(_slot) = crate::strm::slot(state, "secondary", context).await else {
        return json!({});
    };
    match output {
        Ok(Ok(output)) if output.status.success() => serde_json::from_slice(&output.stdout)
            .map(|raw| rename::probe_fields(&raw))
            .unwrap_or(json!({})),
        _ => json!({}),
    }
}
async fn submit_probe(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task=state.jobs.submit("探测媒体命名信息",move|context|async move{
        context.checkpoint().await?;let source=source(&work,&id).await?;let fields=probe_fields(&work,&source,Some(&context)).await;
        let name=body["custom_name"].as_str().filter(|s|!s.is_empty()).unwrap_or(&source.source);
        Ok(json!({"suggested_name":rename::supplement(name,&fields,&source.ext)?,"fields":fields,"message":"ffprobe命名补充完成"}))
    }).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"探测媒体命名信息已提交"})),
    ))
}
fn token(id: &str, source: &Source, plan: &Value) -> String {
    let value = json!({"item_id":id,"identity":source.identity,"source_name":source.source,"rendered":plan["rendered"],"cloud_path":plan["cloud_path"],"local_path":plan["local_path"]});
    Sha256::digest(value.to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let source = source(&state, &id).await?;
    let plan = plan(&state, &source, &body).await?;
    Ok(Json(
        json!({"source_name":source.source,"file_ext":source.ext,"target_name":plan["filename"],"target_relative_path":plan["rendered"],"target_cloud_path":plan["cloud_path"],"target_local_path":plan["local_path"],"fields":plan["fields"],"ffprobe_fields":plan["ffprobe_fields"],"preview_token":token(&id,&source,&plan)}),
    ))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let id = state
        .jobs
        .submit("重命名媒体", move |context| async move {
            execute(&work, &id, &body, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":id,"message":"重命名媒体已提交"})),
    ))
}
struct Changes {
    moves: Vec<(PathBuf, PathBuf)>,
    contents: Vec<(PathBuf, Vec<u8>)>,
    cloud: Vec<(String, String)>,
}
fn mapped_reference(
    raw: &str,
    moves: &[(PathBuf, PathBuf)],
    cloud: &[(PathBuf, PathBuf)],
    file: Option<(&str, &str)>,
) -> String {
    if let Some((old, new)) = file
        && raw == old
    {
        return new.into();
    }
    let local = remap(raw, moves);
    if local != raw {
        local
    } else {
        remap_cloud_directory(raw, cloud)
    }
}

fn references(
    value: &mut Value,
    moves: &[(PathBuf, PathBuf)],
    cloud: &[(PathBuf, PathBuf)],
    file: Option<(&str, &str)>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                references(value, moves, cloud, file);
            }
        }
        Value::Object(values) => {
            let source_path = values
                .get("Path")
                .and_then(Value::as_str)
                .map(str::to_string);
            let source_name = values.contains_key("MediaStreams");
            for (key, value) in values.iter_mut() {
                if let Some(path) = value.as_str()
                    && ["Path", "MediaPath", "FileName"].contains(&key.as_str())
                {
                    *value = json!(mapped_reference(path, moves, cloud, file));
                } else {
                    references(value, moves, cloud, file);
                }
            }
            if source_name && let Some(old_path) = source_path {
                let new_path = mapped_reference(&old_path, moves, cloud, file);
                let old = cloud_location(&old_path).1;
                let new = cloud_location(&new_path).1;
                if values
                    .get("Name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| {
                        Some(name) == old.file_stem().and_then(|name| name.to_str())
                    })
                {
                    values.insert(
                        "Name".into(),
                        json!(new.file_stem().unwrap_or_default().to_string_lossy()),
                    );
                }
            }
        }
        _ => (),
    }
}

impl Changes {
    async fn rollback(&self, state: &AppState) {
        for (id, name) in self.cloud.iter().rev() {
            if let Err(error) = crate::p115::rename(state, id, name).await {
                tracing::error!("115改名回滚失败：目标名称 {name}，文件 {id}，{}", error.1);
            }
        }
        for (path, bytes) in self.contents.iter().rev() {
            if let Err(error) = tokio::fs::write(path, bytes).await {
                tracing::error!(%error,"文件内容回滚失败：{}", path.display());
            }
        }
        for (old, new) in self.moves.iter().rev() {
            if let Err(error) = tokio::fs::rename(new, old).await {
                tracing::error!(%error,"文件改名回滚失败：{} → {}", new.display(), old.display());
            }
        }
    }
    async fn move_path(&mut self, old: PathBuf, new: PathBuf) -> Result<()> {
        if old == new {
            return Ok(());
        }
        if new.exists() {
            return Err(ApiError::bad(format!("目标已存在: {}", new.display())));
        }
        tokio::fs::rename(&old, &new).await?;
        self.moves.push((old, new.clone()));
        if !rename::exact_entry(&new).await {
            return Err(ApiError::bad(format!(
                "文件系统未保留目标长文件名: {}",
                new.display()
            )));
        }
        Ok(())
    }
}
fn remap(path: &str, moves: &[(PathBuf, PathBuf)]) -> String {
    let mut path = PathBuf::from(path);
    for (old, new) in moves {
        if let Ok(suffix) = path.strip_prefix(old) {
            path = if suffix.as_os_str().is_empty() {
                new.clone()
            } else {
                new.join(suffix)
            };
        }
    }
    path.to_string_lossy().into()
}
fn cloud_location(raw: &str) -> (Option<reqwest::Url>, PathBuf) {
    let url = reqwest::Url::parse(raw)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https" | "file"));
    let path = url.as_ref().map_or(raw, |url| url.path());
    (url.clone(), PathBuf::from(crate::media_paths::clean(path)))
}
fn cloud_directory_moves(
    source: &str,
    names: &[(String, String)],
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let (_, source) = cloud_location(source);
    let mut parent = source
        .parent()
        .ok_or_else(|| ApiError::bad("115来源缺少目录路径"))?
        .to_path_buf();
    let mut moves = vec![];
    for (old, new) in names {
        if parent.file_name().and_then(|name| name.to_str()) != Some(old.as_str()) {
            return Err(ApiError::bad(
                "115来源目录与当前目录身份不一致，无法安全映射关联STRM",
            ));
        }
        if old != new {
            moves.push((parent.clone(), parent.with_file_name(new)));
        }
        parent.pop();
    }
    Ok(moves)
}
fn remap_cloud_directory(raw: &str, moves: &[(PathBuf, PathBuf)]) -> String {
    let (url, path) = cloud_location(raw);
    let mapped = remap(path.to_str().unwrap_or(""), moves);
    if mapped == path.to_string_lossy() {
        return raw.into();
    }
    if let Some(mut url) = url {
        url.set_path(&mapped);
        url.to_string()
    } else {
        mapped
    }
}
async fn write_recovery(path: &std::path::Path, value: &Value) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(&serde_json::to_vec_pretty(value).unwrap())
        .await?;
    file.sync_all().await?;
    Ok(())
}
async fn renamed_id(
    state: &AppState,
    path: &std::path::Path,
    kind: &Value,
) -> Result<Option<String>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
    let path = path.to_string_lossy().to_string();
    loop {
        let query = [
            ("Path", path.clone()),
            ("Recursive", "true".into()),
            ("Fields", "Id,Path,Type".into()),
        ];
        let response = tokio::time::timeout_at(
            deadline,
            state.emby(reqwest::Method::GET, "Items", &query, None),
        )
        .await;
        let response = match response {
            Ok(result) => result?,
            Err(_) => return Ok(None),
        };
        let matches = list(&response["Items"])
            .into_iter()
            .filter(|item| item["Path"] == path && item["Type"] == *kind)
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return Ok(matches[0]["Id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_string));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(
            std::time::Duration::from_secs(1)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
    }
}
async fn renamed_scan(
    state: &AppState,
    ids: &[String],
    position: u64,
    context: &JobContext,
) -> Result<()> {
    if ids.is_empty() {
        return Err(ApiError::bad("改名后没有提交有效目录扫描，保护配置仍保留"));
    }
    let mut traces = ids
        .iter()
        .map(|id| (id, crate::media_extra::RefreshTrace::new(position), false))
        .collect::<Vec<_>>();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut heartbeat = tokio::time::Instant::now();
    loop {
        let logs = tokio::time::timeout_at(
            deadline,
            state.emby(
                reqwest::Method::GET,
                "System/Logs/embyserver.txt/Lines",
                &[("Limit", "500".into())],
                None,
            ),
        )
        .await
        .map_err(|_| ApiError::bad("改名目录扫描完成日志读取超时"))??;
        for (id, trace, complete) in &mut traces {
            if !*complete {
                *complete = trace.feed(id, &logs)?;
            }
        }
        if traces.iter().all(|(_, _, complete)| *complete) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::bad(
                "120秒内未确认本次改名目录扫描结束，保护配置仍保留",
            ));
        }
        if heartbeat.elapsed() >= std::time::Duration::from_secs(15) {
            context
                .activity(
                    "等待改名扫描",
                    "文件与路径已提交，等待全部本次目录扫描完成后恢复保护配置",
                )
                .await;
            heartbeat = tokio::time::Instant::now();
        }
        tokio::time::sleep(
            std::time::Duration::from_secs(1)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
    }
}
async fn execute(state: &AppState, id: &str, body: &Value, context: &JobContext) -> Result<Value> {
    use std::sync::atomic::{AtomicBool, Ordering};
    context.checkpoint().await?;
    let source = source(state, id).await?;
    let plan = plan(state, &source, body).await?;
    if body["preview_token"].as_str() != Some(&token(id, &source, &plan)) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "重命名内容或文件身份已变化，请重新预览".into(),
        ));
    }
    if source.cloud.is_some()
        && source.cache.is_none()
        && source.cloud.as_deref() != plan["cloud_path"].as_str()
    {
        return Err(ApiError::bad("STRM源文件未命中115缓存，拒绝只改播放路径"));
    }
    let config = state.config.read().await.clone();
    if rename::protected_mount(&source.local.to_string_lossy(), &config) {
        return Err(ApiError::bad("禁止直接改名115挂载文件"));
    }
    let directories = list(&plan["directories"])
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let target = PathBuf::from(string(&plan, "local_path"));
    let filename = string(&plan, "filename");
    let mut pending = vec![source.local.parent().unwrap().to_path_buf()];
    for _ in 1..directories.len() {
        pending[0] = pending[0]
            .parent()
            .ok_or_else(|| ApiError::bad("路径层级不足"))?
            .to_path_buf();
    }
    let configured_root = PathBuf::from(string(&config, "local_strm_root"));
    if !directories.is_empty()
        && (pending[0] == configured_root || !pending[0].starts_with(&configured_root))
    {
        return Err(ApiError::bad("命名目录层级越过本地媒体根目录"));
    }
    let mut tree_files = vec![];
    while let Some(dir) = pending.pop() {
        context.checkpoint().await?;
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                tree_files.push(entry.path());
            }
        }
    }
    let sidecars = rename::sidecar_moves(
        &source.local,
        &target,
        &tree_files,
        source.cloud.as_deref().unwrap_or(&source.target.path),
        &string(&source.item["ProviderIds"], "Tmdb"),
    )
    .await?;
    let recovery = state.data_dir.join("rename-recovery").join(&context.id);
    let confirmed = AtomicBool::new(true);
    let outcome = crate::strm::with_settings_guarded(state, "preserve", || async {
        context.checkpoint().await?;
        // Resolve every cloud read before filesystem changes or database row locks.
        let mut operations = vec![];
        let mut cloud_directories = vec![];
        let mut cache_directories = vec![];
        let mut new_cache_path = None;
        if let Some(cache) = &source.cache {
            let fid = string(cache, "id");
            let live = crate::p115::file_info(state, fid.parse().map_err(|_| ApiError::bad("115文件ID无效"))?).await?;
            for field in ["sha1", "pick_code", "parent_id", "name"] {
                let expected = string(cache, field);
                let actual = string(&live, field);
                if !expected.is_empty() && !actual.is_empty() && (if field == "sha1" { !expected.eq_ignore_ascii_case(&actual) } else { expected != actual }) {
                    return Err(ApiError::bad("115文件身份在预览后变化，未执行改名"));
                }
            }
            if source.target.sha.as_ref().is_some_and(|sha| !string(&live, "sha1").eq_ignore_ascii_case(sha)) {
                return Err(ApiError::bad("115文件SHA与当前资产不一致，未执行改名"));
            }
            crate::identity::verify(&source.target.asset, &json!({"Size":live["size"]}))?;
            let mut parent = string(cache, "parent_id");
            let mut names = vec![];
            for desired in directories.iter().rev() {
                context.checkpoint().await?;
                let node = crate::p115::file_info(state, parent.parse().map_err(|_| ApiError::bad("115父目录身份无效"))?).await?;
                let old = string(&node, "name");
                if old.is_empty() { return Err(ApiError::bad("115父目录名称缺失")); }
                names.push((old.clone(), desired.clone()));
                if old != *desired { operations.push((parent.clone(), old, desired.clone())); }
                parent = string(&node, "parent_id");
            }
            let old_local = string(cache, "local_path");
            if names.iter().any(|(old, new)| old != new) {
                let cached_source = rename::configured_cloud(&old_local, &config);
                cloud_directories = [source.cloud.as_deref().unwrap_or(""), &cached_source].iter()
                    .find_map(|path| cloud_directory_moves(path, &names).ok())
                    .ok_or_else(|| ApiError::bad("无法确定115目录改名范围，未执行改名"))?;
                cache_directories = cloud_directory_moves(&old_local, &names)?;
            }
            let old_name = string(&live, "name");
            if old_name.is_empty() { return Err(ApiError::bad("115文件名称缺失")); }
            if old_name != filename { operations.push((fid, old_name, filename.clone())); }
            new_cache_path = Some(rename::replace(&old_local, &directories, &filename)?);
        }
        let current = self::source(state, id).await?;
        if current.identity != source.identity { return Err(ApiError(StatusCode::CONFLICT, "准备改名期间文件身份变化，未执行改名".into())); }
        context.checkpoint().await?;
        let mut original_json = vec![];
        for path in &tree_files {
            if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with("-mediainfo.json")) {
                let info: Value = serde_json::from_slice(&tokio::fs::read(path).await?).map_err(|_| ApiError::bad("媒体信息JSON无效，未执行改名"))?;
                original_json.push(json!({"path":path,"mediainfo":info}));
            }
        }
        write_recovery(&recovery.join("metadata-original.json"), &json!(original_json)).await?;
        let mut changes = Changes { moves: vec![], contents: vec![], cloud: vec![] };
        let result = async {
            let mut current = source.local.parent().unwrap().to_path_buf();
            for name in directories.iter().rev() {
                let new = current.with_file_name(name);
                changes.move_path(current.clone(), new).await?;
                current = current.parent().unwrap().to_path_buf();
            }
            changes.move_path(PathBuf::from(remap(source.local.to_str().unwrap(), &changes.moves)), target.clone()).await?;
            for (old, new) in &sidecars { changes.move_path(PathBuf::from(remap(old.to_str().unwrap(), &changes.moves)), new.clone()).await?; }
            for (node, old, new) in &operations {
                crate::p115::rename(state, node, new).await?;
                changes.cloud.push((node.clone(), old.clone()));
            }
            let mut exact = HashMap::new();
            for old in &tree_files {
                let new = remap(old.to_str().unwrap(), &changes.moves);
                if new != old.to_string_lossy() { exact.insert(old.to_string_lossy().to_string(), new); }
            }
            if source.target.path != source.local.to_string_lossy() && let Some(cloud) = plan["cloud_path"].as_str() { exact.insert(source.target.path.clone(), cloud.into()); }
            let file = source.cloud.as_deref().zip(plan["cloud_path"].as_str());
            let mut reference_cloud = cloud_directories.clone();
            if let Some((old, new)) = file {
                reference_cloud.insert(0, (cloud_location(old).1, cloud_location(new).1));
                let old_parent = cloud_location(old).1.parent().map(std::path::Path::to_path_buf);
                let new_parent = cloud_location(new).1.parent().map(std::path::Path::to_path_buf);
                if let Some((old_parent, new_parent)) = old_parent.zip(new_parent) {
                    for (old, new) in &sidecars {
                        reference_cloud.insert(0, (old_parent.join(old.file_name().unwrap()), new_parent.join(new.file_name().unwrap())));
                    }
                }
            }
            let mut updated_json = vec![];
            for old in &tree_files {
                let path = PathBuf::from(remap(old.to_str().unwrap(), &changes.moves));
                if !path.file_name().is_some_and(|name| name.to_string_lossy().ends_with("-mediainfo.json")) { continue; }
                let bytes = tokio::fs::read(&path).await?;
                let mut info: Value = serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("媒体信息JSON无效"))?;
                let previous = info.clone();
                references(&mut info, &changes.moves, &reference_cloud, file);
                if info != previous { changes.contents.push((path.clone(), bytes)); tokio::fs::write(&path, serde_json::to_vec_pretty(&info).unwrap()).await?; }
                updated_json.push(json!({"path":path,"mediainfo":info}));
            }
            if let Some(new) = plan["cloud_path"].as_str() {
                for old in &tree_files {
                    if old != &source.local && (directories.is_empty() || old.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("strm"))) { continue; }
                    let path = PathBuf::from(remap(old.to_str().unwrap(), &changes.moves));
                    let Some(previous) = rename::stream_target(&path).await else { continue; };
                    let destination = if old == &source.local { new.into() } else { remap_cloud_directory(&previous, &cloud_directories) };
                    if previous != destination { changes.contents.push((path.clone(), tokio::fs::read(&path).await?)); tokio::fs::write(&path, format!("{destination}\n")).await?; }
                }
            }
            let mut paths = exact.keys().cloned().chain(file.map(|(old, _)| old.to_string())).chain(reference_cloud.iter().map(|(old, _)| old.to_string_lossy().to_string())).collect::<Vec<_>>();
            let mut prefixes = changes.moves.iter().chain(cloud_directories.iter()).map(|(old, _)| old.to_string_lossy().to_string()).collect::<Vec<_>>();
            for values in [&mut paths, &mut prefixes] {
                for path in values.clone() { let mut url = reqwest::Url::parse("http://path.invalid").unwrap(); url.set_path(&path); if url.path() != path { values.push(url.path().into()); } }
            }
            let mut transaction = state.db.begin().await?;
            let rows: Vec<Value> = sqlx::query_scalar(r"SELECT to_jsonb(m) FROM media_metadata m WHERE jsonb_typeof(asset_details_json)='array' AND EXISTS(SELECT 1 FROM jsonb_array_elements(asset_details_json) a CROSS JOIN LATERAL (SELECT regexp_replace(replace(a->>'path',chr(92),'/'),'^(https?|file)://[^/?#]*([^?#]*).*$','\2','i') AS path,regexp_replace(replace(a->>'fingerprint_target',chr(92),'/'),'^(https?|file)://[^/?#]*([^?#]*).*$','\2','i') AS fingerprint_target) r WHERE a->>'path'=ANY($1) OR a->>'fingerprint_target'=ANY($1) OR r.path=ANY($1) OR r.fingerprint_target=ANY($1) OR EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE r.path=p OR starts_with(r.path,p||'/') OR r.fingerprint_target=p OR starts_with(r.fingerprint_target,p||'/'))) ORDER BY tmdb_id,item_type FOR UPDATE")
                .bind(&paths).bind(&prefixes).fetch_all(&mut *transaction).await?;
            let mut shas = std::collections::BTreeSet::new();
            for row in rows {
                let mut assets = list(&row["asset_details_json"]);
                let sha = list(&row["file_sha1_json"]);
                let mut touched = false;
                for (index, asset) in assets.iter_mut().enumerate() {
                    if asset["emby_item_id"] == id && *asset != source.target.asset { return Err(ApiError::bad("媒体资产在准备提交改名时变化")); }
                    for key in ["path", "fingerprint_target"] {
                        let old = string(asset, key);
                        if old.is_empty() { continue; }
                        let new = exact.get(&old).cloned().unwrap_or_else(|| mapped_reference(&old, &changes.moves, &reference_cloud, file));
                        if old != new { asset[key] = json!(new); touched = true; }
                    }
                    if touched && let Some(sha) = sha.get(index).and_then(Value::as_str) { shas.insert(sha.to_uppercase()); }
                    if asset["emby_item_id"] == id { asset.as_object_mut().unwrap().extend(crate::media_display::fields(&filename).as_object().unwrap().clone()); }
                }
                if touched { sqlx::query("UPDATE media_metadata SET asset_details_json=$1,last_updated_at=NOW() WHERE tmdb_id=$2 AND item_type=$3").bind(json!(assets)).bind(string(&row,"tmdb_id")).bind(string(&row,"item_type")).execute(&mut *transaction).await?; }
            }
            if let Some(sha) = &source.target.sha { shas.insert(sha.to_uppercase()); }
            let info_rows: Vec<(String, Value)> = sqlx::query_as("SELECT sha1,mediainfo_json FROM p115_mediainfo_cache WHERE UPPER(sha1)=ANY($1) AND mediainfo_json IS NOT NULL FOR UPDATE")
                .bind(shas.into_iter().collect::<Vec<_>>()).fetch_all(&mut *transaction).await?;
            for (sha, mut info) in info_rows {
                let old = info.clone(); references(&mut info, &changes.moves, &reference_cloud, file);
                if info != old { sqlx::query("UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2").bind(info).bind(sha).execute(&mut *transaction).await?; }
            }
            if let Some(cache) = &source.cache {
                let fid = string(cache, "id");
                let prefixes = cache_directories.iter().map(|(old,_)| old.to_string_lossy().trim_start_matches('/').to_string()).collect::<Vec<_>>();
                let cached_rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_filesystem_cache c WHERE id=ANY($1) OR EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE trim(leading '/' from replace(local_path,chr(92),'/'))=p OR starts_with(trim(leading '/' from replace(local_path,chr(92),'/')),p||'/')) ORDER BY id FOR UPDATE")
                    .bind(operations.iter().map(|(id,_,_)|id.clone()).chain(std::iter::once(fid.clone())).collect::<Vec<_>>()).bind(&prefixes).fetch_all(&mut *transaction).await?;
                for row in cached_rows {
                    let old = string(&row, "local_path");
                    let normalized = cache_directories.iter().map(|(old,new)| (PathBuf::from(old.to_string_lossy().trim_start_matches('/')),PathBuf::from(new.to_string_lossy().trim_start_matches('/')))).collect::<Vec<_>>();
                    let mapped = remap(old.trim_start_matches('/'), &normalized);
                    let new = format!("{}{}", if old.starts_with('/') {"/"} else {""}, mapped);
                    let name = operations.iter().find(|(node,_,_)| *node == string(&row,"id")).map(|(_,_,new)|new.clone()).unwrap_or_else(|| string(&row,"name"));
                    let new = if row["id"] == fid { new_cache_path.as_deref().unwrap_or(&new).to_string() } else { new };
                    if old != new || row["name"] != name { sqlx::query("UPDATE p115_filesystem_cache SET local_path=$1,name=$2 WHERE id=$3").bind(new).bind(name).bind(string(&row,"id")).execute(&mut *transaction).await?; }
                }
            }
            write_recovery(&recovery.join("metadata-updated.json"), &json!(updated_json)).await?;
            transaction.commit().await?;
            Ok::<_,ApiError>(())
        }.await;
        if let Err(error) = result { changes.rollback(state).await; return Err(error); }
        let position = crate::media_extra::log_position(state).await?;
        write_recovery(&recovery.join("pending-scan.json"), &json!({"position":position,"ids":null})).await?;
        confirmed.store(false, Ordering::Release);
        let submitted = tokio::time::timeout(std::time::Duration::from_secs(120), crate::media_save::refresh_with_ids(state, &[source.target.path.clone(), target.to_string_lossy().into()])).await
            .map_err(|_| ApiError::bad("改名目录扫描提交超时，保护配置仍保留"))??;
        write_recovery(&recovery.join("pending-scan.json"), &json!({"position":position,"ids":submitted})).await?;
        let (found, scanned) = tokio::join!(renamed_id(state, &target, &source.item["Type"]), renamed_scan(state, &submitted, position, context));
        if scanned.is_ok() { confirmed.store(true, Ordering::Release); }
        scanned?;
        let mut warnings = vec![];
        let found = match found { Ok(found) => found, Err(error) => { warnings.push(format!("新Emby条目查询失败：{}",error.1)); None } };
        if let Some(new_id) = &found {
            let mut transaction = state.db.begin().await?;
            sqlx::query("UPDATE media_metadata SET emby_item_ids_json=(SELECT jsonb_agg(CASE WHEN eid=$1 THEN to_jsonb($2::text) ELSE to_jsonb(eid) END ORDER BY idx) FROM jsonb_array_elements_text(emby_item_ids_json) WITH ORDINALITY arr(eid,idx)),asset_details_json=(SELECT jsonb_agg(CASE WHEN a->>'emby_item_id'=$1 THEN jsonb_set(a,'{emby_item_id}',to_jsonb($2::text)) ELSE a END ORDER BY idx) FROM jsonb_array_elements(asset_details_json) WITH ORDINALITY arr(a,idx)) WHERE emby_item_ids_json @> $3")
                .bind(id).bind(new_id).bind(json!([id])).execute(&mut *transaction).await?;
            crate::orphans::migrate_item_logs(&mut transaction,id,new_id).await?;
            transaction.commit().await?;
        } else { warnings.push("45秒内未关联到唯一新Emby条目；目录扫描已确认，改名文件和路径已保留，请同步媒体数据".into()); }
        Ok(json!({"message":if warnings.is_empty(){"媒体已重命名，路径与Emby关联已通过验收"}else{"媒体已重命名，目录扫描已完成，但Emby关联仍需处理"},"source_name":source.source,"target_name":filename,"target_cloud_path":plan["cloud_path"],"target_local_path":target,"item_id":found,"association_pending":found.is_none(),"warnings":warnings.len(),"warning_details":warnings,"recovery_path":recovery}))
    }, &confirmed, &recovery).await;
    if outcome
        .as_ref()
        .is_ok_and(|value| value["warnings"].as_u64() == Some(0))
    {
        let _ = tokio::fs::remove_dir_all(&recovery).await;
    }
    outcome
}
async fn sample(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    let _ = state;
    let movie = json!({"title":"肖申克的救赎","en_title":"The Shawshank Redemption","year":1994,"tmdbid":278,"season":0,"episode":0,"season_episode":"","part":"","videoFormat":"2160p","videoCodec":"H265","audioCodec":"DTS 5.1","effect":"","fileExt":".mkv"});
    let tv = json!({"title":"凡人修仙传","en_title":"A Record of a Mortal's Journey to Immortality","year":2020,"tmdbid":95557,"season":1,"episode":1,"season_episode":"S01E01","part":"","videoFormat":"2160p","videoCodec":"H265","audioCodec":"AAC 2.0","effect":"HDR","resourceType":"WEB-DL","releaseGroup":"ADWeb","fileExt":".mp4"});
    Ok(Json(
        json!({"movie":if string(&body,"movie_format").trim().is_empty(){String::new()}else{rename::render(&string(&body,"movie_format"),&movie,".mkv")?},"tv":if string(&body,"tv_format").trim().is_empty(){String::new()}else{rename::render(&string(&body,"tv_format"),&tv,".mp4")?}}),
    ))
}
