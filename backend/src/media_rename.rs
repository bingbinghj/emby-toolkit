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
}
async fn source(state: &AppState, id: &str) -> Result<Source> {
    let target = media_edit::target(state, id).await?;
    if target.asset["fingerprint_pending"] == true {
        return Err(ApiError::bad("文件身份待校验"));
    }
    let item=media_edit::details(state,id,"Name,OriginalTitle,ProductionYear,ProviderIds,Type,IndexNumber,ParentIndexNumber,Path,MediaSources").await?;
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
    Ok(Source {
        target,
        item,
        cache,
        local,
        cloud,
        source,
        ext,
        fields,
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
    let value = json!({"item_id":id,"source_name":source.source,"rendered":plan["rendered"],"cloud_path":plan["cloud_path"],"local_path":plan["local_path"]});
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
            crate::strm::with_settings(&work, "preserve", || execute(&work, &id, &body, &context))
                .await
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
fn references(value: &mut Value, moves: &[(PathBuf, PathBuf)]) {
    match value {
        Value::String(s) => {
            let mapped = remap(s, moves);
            if mapped != *s {
                *s = mapped;
            } else if let Some((_, new)) = moves
                .iter()
                .find(|(old, _)| old.file_name().and_then(|n| n.to_str()) == Some(s.as_str()))
            {
                *s = new.file_name().unwrap_or_default().to_string_lossy().into();
            }
        }
        Value::Array(a) => {
            for v in a {
                references(v, moves)
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                references(v, moves)
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
async fn execute(state: &AppState, id: &str, body: &Value, context: &JobContext) -> Result<Value> {
    context.checkpoint().await?;
    let source = source(state, id).await?;
    let plan = plan(state, &source, body).await?;
    if body["preview_token"].as_str() != Some(&token(id, &source, &plan)) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "重命名内容已变化，请重新预览".into(),
        ));
    }
    let old_cloud = source.cloud.clone();
    if old_cloud.is_some()
        && source.cache.is_none()
        && old_cloud.as_deref() != plan["cloud_path"].as_str()
    {
        return Err(ApiError::bad("STRM源文件未命中115缓存，拒绝只改播放路径"));
    }
    let config = state.config.read().await.clone();
    if rename::protected_mount(&source.local.to_string_lossy(), &config) {
        return Err(ApiError::bad("禁止直接改名115挂载文件"));
    }
    let directories = list(&plan["directories"])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
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
    let mut changes = Changes {
        moves: vec![],
        contents: vec![],
        cloud: vec![],
    };
    let result=async{
        let mut current=source.local.parent().unwrap().to_path_buf();
        for name in directories.iter().rev(){let new=current.with_file_name(name);changes.move_path(current.clone(),new).await?;current=current.parent().unwrap().to_path_buf();}
        let mapped=PathBuf::from(remap(source.local.to_str().unwrap(),&changes.moves));changes.move_path(mapped,target.clone()).await?;
        for (old,new) in &sidecars {
            let mapped=PathBuf::from(remap(old.to_str().unwrap(),&changes.moves));changes.move_path(mapped,new.clone()).await?;
        }
        if let Some(cache)=&source.cache{
            let mut cloud_nodes=vec![];let mut parent=string(cache,"parent_id");
            for desired in directories.iter().rev(){
                let id=parent.parse().map_err(|_|ApiError::bad("115父目录身份无效"))?;let node=crate::p115::file_info(state,id).await?;let old=string(&node,"name");if old.is_empty(){return Err(ApiError::bad("115父目录名称缺失"));}
                cloud_nodes.push((parent.clone(),old,desired.clone()));parent=string(&node,"parent_id");
            }
            for (node,old,new) in cloud_nodes{if old!=new{crate::p115::rename(state,&node,&new).await?;changes.cloud.push((node,old));}}
            let fid=string(cache,"id");let live=crate::p115::file_info(state,fid.parse().map_err(|_|ApiError::bad("115文件ID无效"))?).await?;
            if live["pick_code"]!=cache["pick_code"]||live["sha1"]!=cache["sha1"]{return Err(ApiError::bad("115文件在预览后变化"));}
            let old=string(&live,"name");if old!=filename{crate::p115::rename(state,&fid,&filename).await?;changes.cloud.push((fid,old));}
        }
        let mut exact=HashMap::new();for old in &tree_files{let new=remap(old.to_str().unwrap(),&changes.moves);if new!=old.to_string_lossy(){exact.insert(old.to_string_lossy().to_string(),new);}}
        for old in &tree_files{
            let path=PathBuf::from(remap(old.to_str().unwrap(),&changes.moves));
            if !path.file_name().is_some_and(|n|n.to_string_lossy().ends_with("-mediainfo.json")){continue;}
            let bytes=tokio::fs::read(&path).await?;
            let mut data:Value=serde_json::from_str(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}')).map_err(|_|ApiError::bad("媒体信息JSON无效"))?;let previous=data.clone();references(&mut data,&changes.moves);
            if data!=previous{changes.contents.push((path.clone(),bytes));tokio::fs::write(path,serde_json::to_vec_pretty(&data).unwrap()).await?;}
        }
        if let Some(new)=plan["cloud_path"].as_str(){for old in &tree_files{
            if old!=&source.local && (directories.is_empty() || old.extension().is_none_or(|s|!s.to_string_lossy().eq_ignore_ascii_case("strm"))){continue;}let path=PathBuf::from(remap(old.to_str().unwrap(),&changes.moves));let bytes=tokio::fs::read(&path).await?;
            let Some(source_target)=rename::stream_target(&path).await else {continue;};let destination=if old==&source.local{new.into()}else{let name=std::path::Path::new(&crate::media_paths::clean(&source_target)).file_name().unwrap_or_default().to_string_lossy().to_string();rename::replace(&source_target,&directories,&name)?};
            if destination!=source_target{changes.contents.push((path.clone(),bytes));tokio::fs::write(path,format!("{destination}\n")).await?;}
        }}
        if source.target.path!=source.local.to_string_lossy() && let Some(cloud)=plan["cloud_path"].as_str(){exact.insert(source.target.path.clone(),cloud.to_string());}
        let mut transaction=state.db.begin().await?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE jsonb_typeof(asset_details_json)='array' FOR UPDATE").fetch_all(&mut *transaction).await?;
        for row in rows{let mut assets=list(&row["asset_details_json"]);let mut touched=false;for asset in &mut assets{if let Some(new)=exact.get(&string(asset,"path")){asset["path"]=json!(new);if new==target.to_str().unwrap(){asset.as_object_mut().unwrap().extend(crate::media_display::fields(&filename).as_object().unwrap().clone());}touched=true;}}
            if touched{sqlx::query("UPDATE media_metadata SET asset_details_json=$1,last_updated_at=NOW() WHERE tmdb_id=$2 AND item_type=$3").bind(json!(assets)).bind(string(&row,"tmdb_id")).bind(string(&row,"item_type")).execute(&mut *transaction).await?;}}
        if let Some(cache)=&source.cache{
            let old_local=string(cache,"local_path");let local=rename::replace(&old_local,&directories,&filename)?;
            let old_parts=std::path::Path::new(&old_local).parent().unwrap_or(std::path::Path::new("")).components().map(|c|c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>();
            if old_parts.len()<directories.len(){return Err(ApiError::bad("115缓存路径层级不足"));}
            let old_root=old_parts[..old_parts.len()-directories.len()].join("/");let old_tail=&old_parts[old_parts.len()-directories.len()..];
            let cache_rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_filesystem_cache c WHERE local_path IS NOT NULL FOR UPDATE").fetch_all(&mut *transaction).await?;
            for cached in cache_rows{
                let mut path=string(&cached,"local_path");
                for depth in (1..=directories.len()).rev(){
                    let old_prefix=format!("{old_root}/{}",old_tail[..depth].join("/")).trim_start_matches('/').to_string();
                    let new_prefix=format!("{old_root}/{}",directories[..depth].join("/")).trim_start_matches('/').to_string();
                    let normalized=path.trim_start_matches('/');
                    if normalized==old_prefix||normalized.starts_with(&format!("{old_prefix}/")){path=format!("{}{}{}",if path.starts_with('/') {"/"}else{""},new_prefix,&normalized[old_prefix.len()..]);break;}
                }
                if path!=string(&cached,"local_path"){sqlx::query("UPDATE p115_filesystem_cache SET local_path=$1 WHERE id=$2").bind(path).bind(string(&cached,"id")).execute(&mut *transaction).await?;}
            }
            for (node,_) in &changes.cloud {let node_name=if node==&string(cache,"id"){filename.clone()}else{crate::p115::file_info(state,node.parse().map_err(|_|ApiError::bad("115目录ID无效"))?).await?["name"].as_str().unwrap_or("").to_string()};sqlx::query("UPDATE p115_filesystem_cache SET name=$1 WHERE id=$2").bind(node_name).bind(node).execute(&mut *transaction).await?;}
            sqlx::query("UPDATE p115_filesystem_cache SET name=$1,local_path=$2 WHERE id=$3").bind(&filename).bind(local).bind(string(cache,"id")).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;Ok::<_,ApiError>(())
    }.await;
    if let Err(error) = result {
        changes.rollback(state).await;
        return Err(error);
    }
    crate::media_save::refresh(
        state,
        &[source.target.path.clone(), target.to_string_lossy().into()],
    )
    .await?;
    let mut found = None;
    for _ in 0..45 {
        let result = state
            .emby(
                reqwest::Method::GET,
                "Items",
                &[
                    ("Path", target.to_string_lossy().into()),
                    ("Recursive", "true".into()),
                    ("Fields", "Id,Path".into()),
                ],
                None,
            )
            .await?;
        let matches = list(&result["Items"])
            .into_iter()
            .filter(|v| v["Path"] == target.to_string_lossy().as_ref())
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            found = matches[0]["Id"].as_str().map(str::to_string);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if let Some(new_id) = &found {
        let mut transaction = state.db.begin().await?;
        sqlx::query("UPDATE media_metadata SET emby_item_ids_json=(SELECT jsonb_agg(CASE WHEN eid=$1 THEN to_jsonb($2::text) ELSE to_jsonb(eid) END ORDER BY idx) FROM jsonb_array_elements_text(emby_item_ids_json) WITH ORDINALITY arr(eid,idx)),asset_details_json=(SELECT jsonb_agg(CASE WHEN a->>'emby_item_id'=$1 THEN jsonb_set(a,'{emby_item_id}',to_jsonb($2::text)) ELSE a END ORDER BY idx) FROM jsonb_array_elements(asset_details_json) WITH ORDINALITY arr(a,idx)) WHERE emby_item_ids_json @> $3").bind(id).bind(new_id).bind(json!([id])).execute(&mut *transaction).await?;
        crate::orphans::migrate_item_logs(&mut transaction, id, new_id).await?;
        transaction.commit().await?;
    }
    Ok(
        json!({"message":"媒体已重命名，115、STRM和媒体信息路径已同步","source_name":source.source,"target_name":filename,"target_cloud_path":plan["cloud_path"],"target_local_path":target,"item_id":found}),
    )
}
async fn sample(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    let _ = state;
    let movie = json!({"title":"肖申克的救赎","en_title":"The Shawshank Redemption","year":1994,"tmdbid":278,"season":0,"episode":0,"season_episode":"","part":"","videoFormat":"2160p","videoCodec":"H265","audioCodec":"DTS 5.1","effect":"","fileExt":".mkv"});
    let tv = json!({"title":"凡人修仙传","en_title":"A Record of a Mortal's Journey to Immortality","year":2020,"tmdbid":95557,"season":1,"episode":1,"season_episode":"S01E01","part":"","videoFormat":"2160p","videoCodec":"H265","audioCodec":"AAC 2.0","effect":"HDR","resourceType":"WEB-DL","releaseGroup":"ADWeb","fileExt":".mp4"});
    Ok(Json(
        json!({"movie":if string(&body,"movie_format").trim().is_empty(){String::new()}else{rename::render(&string(&body,"movie_format"),&movie,".mkv")?},"tv":if string(&body,"tv_format").trim().is_empty(){String::new()}else{rename::render(&string(&body,"tv_format"),&tv,".mp4")?}}),
    ))
}
