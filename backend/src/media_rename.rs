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
    async fn rollback(&self, state: &AppState) -> Vec<String> {
        let mut failures = vec![];
        for (id, name) in self.cloud.iter().rev() {
            if let Err(error) = crate::p115::rename(state, id, name).await {
                failures.push(format!(
                    "115改名回滚失败：目标名称 {name}，文件 {id}，{}",
                    error.1
                ));
            }
        }
        for (path, bytes) in self.contents.iter().rev() {
            if let Err(error) = tokio::fs::write(path, bytes).await {
                failures.push(format!("文件内容回滚失败：{}：{error}", path.display()));
            }
        }
        for (old, new) in self.moves.iter().rev() {
            if let Err(error) = tokio::fs::rename(new, old).await {
                failures.push(format!(
                    "文件改名回滚失败：{} → {}：{error}",
                    new.display(),
                    old.display()
                ));
            }
        }
        for failure in &failures {
            tracing::error!("{failure}");
        }
        failures
    }
    async fn rollback_after_error(
        &self,
        state: &AppState,
        mut error: ApiError,
        recovery: &std::path::Path,
    ) -> ApiError {
        let failures = self.rollback(state).await;
        if failures.is_empty() {
            return error;
        }
        let manifest = json!({"original_error":error.1,"moves":self.moves,
            "contents":self.contents,"cloud":self.cloud,"rollback_failures":failures});
        error.1 = format!(
            "{}；{}；恢复资料：{}",
            error.1,
            failures.join("；"),
            recovery.display()
        );
        if let Err(write_error) =
            write_recovery(&recovery.join("rollback-recovery.json"), &manifest).await
        {
            error
                .1
                .push_str(&format!("；回滚恢复清单保存失败：{}", write_error.1));
        }
        error
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
struct CapturedSidecar {
    old: PathBuf,
    new: PathBuf,
    hash: Option<[u8; 32]>,
}
fn public_sidecar(path: &std::path::Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    [
        "poster",
        "fanart",
        "folder",
        "cover",
        "banner",
        "landscape",
        "logo",
        "clearlogo",
        "clearart",
        "disc",
        "backdrop",
        "background",
        "thumb",
        "movie",
        "tvshow",
        "season",
    ]
    .iter()
    .any(|stem| {
        name.strip_prefix(stem).is_some_and(|suffix| {
            suffix
                .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '-' | '_'))
                .starts_with('.')
        })
    })
}
async fn capture_sidecars(sidecars: &[(PathBuf, PathBuf)]) -> Result<Vec<CapturedSidecar>> {
    let mut output = vec![];
    for (old, new) in sidecars {
        let extension = old
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let known = [
            "json", "nfo", "srt", "ass", "ssa", "sub", "idx", "vtt", "sup", "bif", "jpg", "jpeg",
            "png", "webp",
        ]
        .contains(&extension.as_str());
        let hash = if known && !public_sidecar(old) {
            if !tokio::fs::symlink_metadata(old)
                .await?
                .file_type()
                .is_file()
            {
                return Err(ApiError::bad("待改名旁车不是普通文件，已停止改名"));
            }
            Some(Sha256::digest(tokio::fs::read(old).await?).into())
        } else {
            None
        };
        output.push(CapturedSidecar {
            old: old.clone(),
            new: new.clone(),
            hash,
        });
    }
    Ok(output)
}
async fn cleanup_stale_sidecars(
    root: &std::path::Path,
    old_anchor: &str,
    sidecars: &[CapturedSidecar],
) -> Vec<String> {
    let mut warnings = vec![];
    for sidecar in sidecars {
        let Some(name) = sidecar.old.file_name() else {
            continue;
        };
        let candidate = sidecar.new.with_file_name(name);
        if candidate == sidecar.new {
            continue;
        }
        let outcome = async {
            let metadata = match tokio::fs::symlink_metadata(&candidate).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            let hash = sidecar
                .hash
                .ok_or_else(|| ApiError::bad("公共或未确认类型旁车已保留"))?;
            let new_metadata = tokio::fs::symlink_metadata(&sidecar.new).await?;
            let candidate_real = tokio::fs::canonicalize(&candidate).await?;
            let new_real = tokio::fs::canonicalize(&sidecar.new).await?;
            if candidate_real == new_real {
                return Ok(());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.dev() == new_metadata.dev() && metadata.ino() == new_metadata.ino() {
                    return Ok(());
                }
            }
            if !metadata.file_type().is_file()
                || !new_metadata.file_type().is_file()
                || !candidate_real.starts_with(root)
                || !new_real.starts_with(root)
            {
                return Err(ApiError::bad("旁车当前路径或文件类型已变化，未清理"));
            }
            let name = name.to_string_lossy();
            let mut entries = tokio::fs::read_dir(candidate.parent().unwrap()).await?;
            while let Some(entry) = entries.next_entry().await? {
                let kind = entry.file_type().await?;
                if (kind.is_file() || kind.is_symlink())
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("strm"))
                {
                    let owner = entry
                        .path()
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if owner != old_anchor
                        && name
                            .strip_prefix(&owner)
                            .is_some_and(|suffix| suffix.starts_with(['.', '-']))
                    {
                        return Err(ApiError::bad("旧名旁车属于当前其他STRM版本，已保留"));
                    }
                }
            }
            if <[u8; 32]>::from(Sha256::digest(tokio::fs::read(&candidate).await?)) != hash {
                return Err(ApiError::bad("旧名旁车含新内容，已保留"));
            }
            let current = tokio::fs::symlink_metadata(&candidate).await?;
            if !current.file_type().is_file()
                || current.len() != metadata.len()
                || current.modified()? != metadata.modified()?
                || !tokio::fs::canonicalize(&candidate).await?.starts_with(root)
            {
                return Err(ApiError::bad("旧名旁车在清理期间变化，已保留"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                    return Err(ApiError::bad("旧名旁车在清理期间被替换，已保留"));
                }
            }
            tokio::fs::remove_file(&candidate).await?;
            tracing::info!(
                "[媒体改名] 已清理扫描后再生成的原旁车：{}",
                candidate.display()
            );
            Ok::<_, ApiError>(())
        }
        .await;
        if let Err(error) = outcome {
            warnings.push(format!("{}：{}", candidate.display(), error.1));
        }
    }
    warnings
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
    let local_root = tokio::fs::canonicalize(&configured_root).await?;
    let recovery = state.data_dir.join("rename-recovery").join(&context.id);
    let confirmed = AtomicBool::new(true);
    let outcome = crate::strm::with_settings_guarded(state, "preserve", || async {
        context.checkpoint().await?;
        let captured_sidecars = capture_sidecars(&sidecars).await?;
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
        if let Err(error) = result { return Err(changes.rollback_after_error(state, error, &recovery).await); }
        let position = crate::media_extra::log_position(state).await?;
        write_recovery(&recovery.join("pending-scan.json"), &json!({"position":position,"ids":null})).await?;
        confirmed.store(false, Ordering::Release);
        let submitted = tokio::time::timeout(std::time::Duration::from_secs(120), crate::media_save::refresh_with_ids(state, &[source.target.path.clone(), target.to_string_lossy().into()])).await
            .map_err(|_| ApiError::bad("改名目录扫描提交超时，保护配置仍保留"))??;
        write_recovery(&recovery.join("pending-scan.json"), &json!({"position":position,"ids":submitted})).await?;
        let (found, scanned) = tokio::join!(renamed_id(state, &target, &source.item["Type"]), renamed_scan(state, &submitted, position, context));
        if scanned.is_ok() { confirmed.store(true, Ordering::Release); }
        scanned?;
        let mut warnings = cleanup_stale_sidecars(&local_root,
            &source.local.file_stem().unwrap_or_default().to_string_lossy(),
            &captured_sidecars).await;
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

#[cfg(test)]
mod rollback_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn state(directory: &std::path::Path) -> AppState {
        AppState {
            db: sqlx::postgres::PgPoolOptions::new()
                .acquire_timeout(std::time::Duration::from_millis(10))
                .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
                .unwrap(),
            config: Arc::new(RwLock::new(json!({}))),
            http: reqwest::Client::new(),
            sessions: Arc::new(crate::auth::SessionSigner::load(directory).unwrap()),
            recovery_tokens: Arc::default(),
            data_dir: directory.into(),
            notifications: Arc::default(),
            webhooks: Arc::default(),
            ai: Arc::default(),
            jobs: Arc::default(),
            logs: crate::logs::LogStore::open(&directory.join("logs")).unwrap(),
            p115: Arc::default(),
            strm: Arc::default(),
            douban: Arc::default(),
            docker: Arc::default(),
        }
    }

    #[tokio::test]
    async fn failed_rollbacks_report_targets_keep_recovery_and_attempt_all_steps() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        let old = directory.path().join("Old.strm");
        let new = directory.path().join("New.strm");
        let other_old = directory.path().join("OtherOld.strm");
        let other_new = directory.path().join("OtherNew.strm");
        let blocked_content = directory.path().join("blocked-content");
        tokio::fs::create_dir(&old).await.unwrap();
        tokio::fs::write(old.join("user-file"), b"keep me")
            .await
            .unwrap();
        tokio::fs::create_dir(&blocked_content).await.unwrap();
        tokio::fs::write(&new, b"new playback path").await.unwrap();
        tokio::fs::write(&other_new, b"other playback path")
            .await
            .unwrap();
        let changes = Changes {
            moves: vec![
                (other_old.clone(), other_new.clone()),
                (old.clone(), new.clone()),
            ],
            contents: vec![
                (new.clone(), b"original playback path".to_vec()),
                (blocked_content.clone(), b"original JSON".to_vec()),
            ],
            cloud: vec![("cloud-file".into(), "Original.mkv".into())],
        };
        let recovery = directory.path().join("recovery");
        tokio::fs::create_dir(&recovery).await.unwrap();
        let error = changes
            .rollback_after_error(
                &state,
                ApiError(StatusCode::CONFLICT, "initial database conflict".into()),
                &recovery,
            )
            .await;
        assert_eq!(error.0, StatusCode::CONFLICT);
        for expected in [
            "initial database conflict",
            "cloud-file",
            "Original.mkv",
            old.to_str().unwrap(),
            new.to_str().unwrap(),
            blocked_content.to_str().unwrap(),
            recovery.to_str().unwrap(),
        ] {
            assert!(error.1.contains(expected), "{}", error.1);
        }
        assert!(old.is_dir() && new.is_file());
        assert!(other_old.is_file() && !other_new.exists());
        assert_eq!(
            tokio::fs::read(&new).await.unwrap(),
            b"original playback path"
        );
        assert_eq!(
            tokio::fs::read(old.join("user-file")).await.unwrap(),
            b"keep me"
        );
        let manifest: Value = serde_json::from_slice(
            &tokio::fs::read(recovery.join("rollback-recovery.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["rollback_failures"].as_array().unwrap().len(), 3);
        assert_eq!(manifest["moves"], json!(changes.moves));
        assert_eq!(manifest["contents"], json!(changes.contents));
        assert_eq!(manifest["cloud"], json!(changes.cloud));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(recovery.join("rollback-recovery.json"))
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn successful_rollback_preserves_original_error_without_false_warning() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        let old = directory.path().join("Old.strm");
        let new = directory.path().join("New.strm");
        tokio::fs::write(&new, b"new playback path").await.unwrap();
        let changes = Changes {
            moves: vec![(old.clone(), new.clone())],
            contents: vec![(new.clone(), b"original playback path".to_vec())],
            cloud: vec![],
        };
        let error = changes
            .rollback_after_error(
                &state,
                ApiError(StatusCode::CONFLICT, "original failure".into()),
                &directory.path().join("recovery"),
            )
            .await;
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(error.1, "original failure");
        assert!(!new.exists());
        assert_eq!(
            tokio::fs::read(old).await.unwrap(),
            b"original playback path"
        );
    }

    #[tokio::test]
    async fn recovery_write_failure_does_not_hide_original_or_rollback_errors() {
        let directory = tempfile::tempdir().unwrap();
        let state = state(directory.path());
        let new = directory.path().join("New.strm");
        let changes = Changes {
            moves: vec![(directory.path().join("Old.strm"), new.clone())],
            contents: vec![],
            cloud: vec![],
        };
        let error = changes
            .rollback_after_error(
                &state,
                ApiError(StatusCode::CONFLICT, "original failure".into()),
                &directory.path().join("missing-recovery"),
            )
            .await;
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert!(error.1.contains("original failure"));
        assert!(error.1.contains(new.to_str().unwrap()));
        assert!(error.1.contains("回滚恢复清单保存失败"));
    }
}

#[cfg(test)]
mod stale_sidecar_tests {
    use super::*;

    async fn moved(old: &std::path::Path, new: &std::path::Path) -> Vec<CapturedSidecar> {
        tokio::fs::write(old, b"captured old data").await.unwrap();
        let captured = capture_sidecars(&[(old.into(), new.into())]).await.unwrap();
        tokio::fs::rename(old, new).await.unwrap();
        captured
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn old_name_alias_cannot_delete_the_new_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(directory.path()).await.unwrap();
        let old = root.join("Old.en.srt");
        let new = root.join("New.en.srt");
        let captured = moved(&old, &new).await;
        tokio::fs::hard_link(&new, &old).await.unwrap();
        assert!(
            cleanup_stale_sidecars(&root, "Old", &captured)
                .await
                .is_empty()
        );
        assert!(old.is_file() && new.is_file());
    }

    #[tokio::test]
    async fn removes_only_captured_old_duplicate_and_keeps_new_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(directory.path()).await.unwrap();
        let old = root.join("Old.en.srt");
        let new = root.join("New.en.srt");
        let captured = moved(&old, &new).await;
        tokio::fs::write(&new, b"new sidecar references")
            .await
            .unwrap();
        tokio::fs::write(&old, b"captured old data").await.unwrap();
        assert!(
            cleanup_stale_sidecars(&root, "Old", &captured)
                .await
                .is_empty()
        );
        assert!(!old.exists());
        assert_eq!(
            tokio::fs::read(&new).await.unwrap(),
            b"new sidecar references"
        );
    }

    #[tokio::test]
    async fn preserves_user_changes_public_images_and_more_specific_strm_owner() {
        for (old_name, new_name, contents, owner) in [
            ("Old.nfo", "New.nfo", b"user new content".as_slice(), None),
            (
                "poster.jpg",
                "NewPoster.jpg",
                b"captured old data".as_slice(),
                None,
            ),
            (
                "fanart-1.jpg",
                "NewFanart.jpg",
                b"captured old data".as_slice(),
                None,
            ),
            (
                "Old.cut.en.srt",
                "New.cut.en.srt",
                b"captured old data".as_slice(),
                Some("Old.cut.strm"),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = tokio::fs::canonicalize(directory.path()).await.unwrap();
            let old = root.join(old_name);
            let new = root.join(new_name);
            let captured = moved(&old, &new).await;
            tokio::fs::write(&old, contents).await.unwrap();
            if let Some(owner) = owner {
                tokio::fs::write(root.join(owner), b"/source.mkv")
                    .await
                    .unwrap();
            }
            let warnings = cleanup_stale_sidecars(&root, "Old", &captured).await;
            assert_eq!(warnings.len(), 1, "{old_name}: {warnings:?}");
            assert_eq!(tokio::fs::read(&old).await.unwrap(), contents);
            assert_eq!(tokio::fs::read(&new).await.unwrap(), b"captured old data");
        }
    }

    #[tokio::test]
    async fn directory_only_rename_never_deletes_the_new_name() {
        let directory = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(directory.path()).await.unwrap();
        tokio::fs::create_dir(root.join("old")).await.unwrap();
        tokio::fs::create_dir(root.join("new")).await.unwrap();
        let old = root.join("old/Name.nfo");
        let new = root.join("new/Name.nfo");
        let captured = moved(&old, &new).await;
        assert!(
            cleanup_stale_sidecars(&root, "Name", &captured)
                .await
                .is_empty()
        );
        assert_eq!(tokio::fs::read(&new).await.unwrap(), b"captured old data");
    }

    #[tokio::test]
    async fn unknown_video_sidecar_is_never_read_or_removed() {
        let directory = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(directory.path()).await.unwrap();
        let old = root.join("Old.mp4");
        let new = root.join("New.mp4");
        let captured = capture_sidecars(&[(old.clone(), new.clone())])
            .await
            .unwrap();
        assert!(
            captured[0].hash.is_none(),
            "capture must not try to read the absent video"
        );
        tokio::fs::write(&old, b"unverified video").await.unwrap();
        tokio::fs::write(&new, b"renamed video").await.unwrap();
        assert_eq!(
            cleanup_stale_sidecars(&root, "Old", &captured).await.len(),
            1
        );
        assert!(old.exists());
        assert!(new.exists());
    }
}
