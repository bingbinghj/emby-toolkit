use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
    state::string,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

pub async fn probe(
    state: &AppState,
    pc: &str,
    filename: &str,
    context: &JobContext,
) -> Result<(Value, Value)> {
    context.checkpoint().await?;
    context
        .activity(
            "提取媒体信息",
            &format!("正在提取 {filename} 的媒体流和章节信息"),
        )
        .await;
    let local = crate::media_input::cd2_file(state, filename, Some(pc), Some(context)).await?;
    if crate::media_paths::extension(filename) == ".iso" {
        let size: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT size FROM p115_filesystem_cache WHERE pick_code=$1 LIMIT 1",
        )
        .bind(pc)
        .fetch_optional(&state.db)
        .await?
        .flatten();
        let probe = if let Some(path) = local.as_ref() {
            let _slot = crate::strm::slot(state, "main", Some(context)).await?;
            crate::p115_iso::probe_file(path, Some(context)).await
        } else {
            crate::p115_iso::probe_115(
                state,
                pc,
                filename,
                size.unwrap_or(0).max(0) as u64,
                Some(context),
            )
            .await
        };
        match probe {
            Ok(raw) => {
                let mappings = crate::db::setting(&state.db, "language_mapping")
                    .await?
                    .unwrap_or(json!([]));
                let info = crate::probe::build(&raw, &json!({"fn":filename}), &mappings)
                    .filter(crate::media_info::recoverable)
                    .ok_or_else(|| ApiError::bad("ISO 未返回可用媒体流"))?;
                return Ok((info, raw));
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => tracing::warn!(
                "[媒体信息提取] {filename} 的 ISO 专用解析失败，回退 ffprobe：{}",
                error.1
            ),
        }
    }
    let url = match local.as_ref() {
        Some(path) => path.to_string_lossy().into_owned(),
        None => match crate::p115::download_url(state, pc).await {
            Ok(url) => url,
            Err(_) => {
                crate::p115::openapi_downurl_with_user_agent(state, pc, "Mozilla/5.0").await?
            }
        },
    };
    let _slot = crate::strm::slot(state, "main", Some(context)).await?;
    let mut command = tokio::process::Command::new("ffprobe");
    command.kill_on_drop(true);
    if local.is_none() {
        command.args(["-user_agent", "Mozilla/5.0", "-rw_timeout", "15000000"]);
    }
    command.args([
        "-hide_banner",
        "-v",
        "error",
        "-analyzeduration",
        "20000000",
        "-probesize",
        "20000000",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        &url,
    ]);
    let output = tokio::select! {
        _ = context.cancelled() => { context.checkpoint().await?; unreachable!() },
        output = tokio::time::timeout(Duration::from_secs(45), command.output()) =>
            output.map_err(|_| ApiError::bad("ffprobe 提取超时"))??,
    };
    drop(_slot);
    context.checkpoint().await?;
    if !output.status.success() {
        return Err(ApiError::bad("ffprobe 提取失败，保留现有媒体信息"));
    }
    let mut raw: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| ApiError::bad("ffprobe 返回无效 JSON"))?;
    if let Some(format) = raw["format"].as_object_mut() {
        format.remove("filename");
    }
    let mappings = crate::db::setting(&state.db, "language_mapping")
        .await?
        .unwrap_or(json!([]));
    let info = crate::probe::build(&raw, &json!({"fn":filename}), &mappings)
        .filter(crate::media_info::recoverable)
        .ok_or_else(|| ApiError::bad("ffprobe 未返回可用媒体流"))?;
    Ok((info, raw))
}

enum CacheMode {
    Reuse,
    Rebuild,
    Full,
}

fn legacy_video_fields(stream: &Value) -> bool {
    if stream["Type"] != "Video" || stream["IsExternal"] == true {
        return false;
    }
    stream["ExtendedVideoType"] == "None"
        && stream["ExtendedVideoSubType"] == "None"
        && stream["ExtendedVideoSubTypeDescription"] == "None"
        && matches!(
            stream["VideoRange"].as_str(),
            Some("" | "HDR" | "HDR10" | "HDR10+")
        )
        || stream["ExtendedVideoType"] == "DolbyVision"
            && (stream["VideoRange"] == "DolbyVision HDR10"
                || ["DoviProfile1", "DoviProfile2", "DoviProfile3", "DoviProfile4",
                    "DoviProfile5", "DoviProfile6", "DoviProfile7", "DoviProfile8", "DoviProfile9",
                ]
                    .contains(&stream["ExtendedVideoSubType"].as_str().unwrap_or("")))
}

fn repair_cached_video_fields(info: &Value, rebuilt: &Value) -> Result<Value> {
    let refuse =
        || ApiError::bad("旧媒体信息备份的视频分类缺少一致的原始探测证据，请重新整理入库后重试");
    let source = crate::media_info::container(info).ok_or_else(refuse)?;
    let current = crate::media_info::container(rebuilt).ok_or_else(refuse)?;
    if crate::identity::size(&source["Size"]).is_none_or(|size| size == 0)
        || info.as_array().is_some_and(|bundles| bundles.len() != 1)
        || ["Size", "Container", "RunTimeTicks"]
            .iter()
            .any(|field| source[*field] != current[*field])
    {
        return Err(refuse());
    }
    let current_streams = current["MediaStreams"].as_array().ok_or_else(refuse)?;
    let mut repaired = info.clone();
    for stream in crate::media_info::streams_mut(&mut repaired)
        .ok_or_else(refuse)?
        .iter_mut()
        .filter(|stream| legacy_video_fields(stream))
    {
        let candidates = current_streams
            .iter()
            .filter(|current| current["Type"] == "Video" && current["Index"] == stream["Index"])
            .collect::<Vec<_>>();
        let [current] = candidates.as_slice() else {
            return Err(refuse());
        };
        if !string(stream, "Codec").eq_ignore_ascii_case(&string(current, "Codec"))
            || [
                "ColorTransfer",
                "ColorPrimaries",
                "ColorSpace",
                "Width",
                "Height",
                "BitDepth",
                "PixelFormat",
            ]
            .iter()
            .any(|field| stream[*field] != current[*field])
        {
            return Err(refuse());
        }
        for field in [
            "VideoRange",
            "ExtendedVideoType",
            "ExtendedVideoSubType",
            "ExtendedVideoSubTypeDescription",
        ] {
            stream[field] = current[field].clone();
        }
    }
    Ok(repair_cached_subtitle_fields(repaired))
}

fn repair_cached_subtitle_fields(mut info: Value) -> Value {
    if let Some(streams) = crate::media_info::streams_mut(&mut info) {
        for stream in streams.iter_mut().filter(|stream| stream["Type"] == "Subtitle" && stream["IsExternal"] == false) {
            let Some(codec) = stream["Codec"].as_str().filter(|codec| !codec.is_empty()) else { continue; };
            let Some(index) = stream["Index"].as_i64().filter(|index| i32::try_from(*index).is_ok()) else { continue; };
            let (text, location) = crate::probe::native_internal_subtitle_fields(codec, index);
            stream["IsTextSubtitleStream"] = json!(text);
            stream["SupportsExternalStream"] = json!(text);
            stream["SubtitleLocationType"] = json!(location);
        }
    }
    info
}

pub async fn info(
    state: &AppState,
    sha: &str,
    pc: &str,
    filename: &str,
    full: bool,
    context: &JobContext,
) -> Result<Value> {
    cached_info(
        state,
        sha,
        pc,
        filename,
        if full {
            CacheMode::Full
        } else {
            CacheMode::Reuse
        },
        context,
    )
    .await
}
pub async fn rebuild_info(
    state: &AppState,
    sha: &str,
    pc: &str,
    filename: &str,
    context: &JobContext,
) -> Result<Value> {
    cached_info(state, sha, pc, filename, CacheMode::Rebuild, context).await
}
async fn cached_info(
    state: &AppState,
    sha: &str,
    pc: &str,
    filename: &str,
    mode: CacheMode,
    context: &JobContext,
) -> Result<Value> {
    if !matches!(mode, CacheMode::Full) {
        let cache: Option<Value> =
            sqlx::query_scalar("SELECT to_jsonb(c) FROM p115_mediainfo_cache c WHERE sha1=$1")
                .bind(sha)
                .fetch_optional(&state.db)
                .await?;
        if let Some(cache) = cache {
            if matches!(mode, CacheMode::Reuse)
                && crate::media_info::recoverable(&cache["mediainfo_json"])
                && crate::media_info::container(&cache["mediainfo_json"])
                    .is_some_and(|c| !string(c, "Container").eq_ignore_ascii_case("strm"))
                && crate::media_paths::matches_container(&cache["mediainfo_json"], filename)
            {
                tracing::info!("[媒体信息还原] {filename} 命中已有媒体信息备份");
                let info = &cache["mediainfo_json"];
                if crate::media_info::container(info).is_some_and(|source| {
                    list(&source["MediaStreams"])
                        .iter()
                        .any(legacy_video_fields)
                }) {
                    let mappings = crate::db::setting(&state.db, "language_mapping")
                        .await?
                        .unwrap_or(json!([]));
                    let rebuilt = crate::probe::build(
                        &cache["raw_ffprobe_json"],
                        &json!({"fn":filename}),
                        &mappings,
                    )
                    .filter(crate::media_info::recoverable)
                    .ok_or_else(|| {
                        ApiError::bad(
                            "旧媒体信息备份的视频分类缺少原始探测缓存，请重新整理入库后重试",
                        )
                    })?;
                    return repair_cached_video_fields(info, &rebuilt);
                }
                return Ok(repair_cached_subtitle_fields(info.clone()));
            }
            let raw = &cache["raw_ffprobe_json"];
            let unknown_text = list(&raw["streams"]).iter().any(|s| {
                s["codec_type"] == "subtitle"
                    && !matches!(
                        s["codec_name"].as_str(),
                        Some("hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle")
                    )
                    && matches!(
                        s["tags"]["language"].as_str(),
                        None | Some("" | "und" | "unknown")
                    )
            });
            if !unknown_text {
                let mappings = crate::db::setting(&state.db, "language_mapping")
                    .await?
                    .unwrap_or(json!([]));
                if let Some(info) = crate::probe::build(raw, &json!({"fn":filename}), &mappings)
                    .filter(crate::media_info::recoverable)
                {
                    tracing::info!("[媒体信息还原] {filename} 已从原始探测缓存重建媒体信息");
                    return Ok(info);
                }
            }
        }
    }
    let (info, raw) = probe(state, pc, filename, context).await?;
    context.checkpoint().await?;
    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,raw_ffprobe_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET raw_ffprobe_json=EXCLUDED.raw_ffprobe_json").bind(sha).bind(raw).execute(&state.db).await?;
    Ok(info)
}

pub async fn restore(state: &AppState, full: bool, context: &JobContext) -> Result<Value> {
    if let Some(report) = context.completed("media-restore-result").await? {
        return Ok(report);
    }
    if full && let Some(saved) = context.completed("media-restore-full-save").await? {
        if saved["task_result"].is_object() {
            return Ok(saved["task_result"].clone());
        }
    }
    restore_files(state, full, context).await
}
async fn restore_files(state: &AppState, full: bool, context: &JobContext) -> Result<Value> {
    context
        .activity("扫描本地媒体", "正在扫描需要还原媒体信息的 STRM 文件")
        .await;
    let config = state.config.read().await.clone();
    let root = PathBuf::from(string(&config, "local_strm_root"));
    if string(&config, "local_strm_root").is_empty() || !root.is_dir() {
        return Err(ApiError::bad("未配置本地媒体库目录或目录不存在"));
    }
    let saved_plan = context.plan("media-restore-files").await?;
    let mut rows:Vec<Value> = if saved_plan.is_some() { vec![] } else {
        sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Episode')").fetch_all(&state.db).await?
    };
    crate::media_order::sort_rows(state, &mut rows).await?;
    let folders = list(
        &state
            .emby(reqwest::Method::GET, "Library/VirtualFolders", &[], None)
            .await?,
    );
    let mut assets = BTreeMap::<PathBuf, Vec<(String, Value)>>::new();
    let mut labels = BTreeMap::new();
    let mut order = BTreeMap::new();
    for row in rows {
        let ids = list(&row["emby_item_ids_json"]);
        for (index, asset) in list(&row["asset_details_json"]).into_iter().enumerate() {
            if crate::emby::path_ignored_in(state, &string(&asset, "path"), &folders).await {
                continue;
            }
            let raw = string(&asset, "path");
            let local = if raw.starts_with("http") {
                let (pc, _, cache_path) = crate::fingerprints::lookup(state, &raw).await?;
                crate::fingerprints::asset_path(
                    state,
                    &raw,
                    pc.as_deref(),
                    &cache_path,
                    &json!({}),
                    "http-restore",
                )
                .await
            } else {
                raw
            };
            let path = PathBuf::from(local);
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("strm"))
                && path.starts_with(&root)
            {
                let id = ids
                    .get(index)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                labels
                    .entry(path.clone())
                    .or_insert_with(|| crate::revalidation::media_label(&row, &asset, index));
                order.entry(path.clone()).or_insert_with(|| {
                    let mut key = crate::media_order::key(&row);
                    key.4 = index as u64;
                    key
                });
                assets.entry(path).or_default().push((id, asset));
            }
        }
    }
    if !full && saved_plan.is_none() {
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            context.checkpoint().await?;
            let mut entries = tokio::fs::read_dir(directory).await?;
            while let Some(entry) = entries.next_entry().await? {
                let kind = entry.file_type().await?;
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if kind.is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("strm"))
                {
                    assets.entry(entry.path()).or_default();
                }
            }
        }
    }
    let mut updated = 0;
    let mut failures = vec![];
    let mut full_rebuilt = BTreeMap::<String, Value>::new();
    let mut full_paths = std::collections::BTreeSet::new();
    let mut native_imports = BTreeMap::<String, ImportTarget>::new();
    let mut pending_import = vec![];
    let mut full_report = Value::Null;
    let mut verified_ids = vec![];
    let assets: Vec<(PathBuf, Vec<(String, Value)>)> = if let Some(plan) = saved_plan {
        labels = serde_json::from_value(plan["labels"].clone())
            .map_err(|_| ApiError::bad("媒体信息还原的已保存名称计划无效"))?;
        serde_json::from_value(plan["assets"].clone())
            .map_err(|_| ApiError::bad("媒体信息还原的已保存文件计划无效"))?
    } else {
        let mut assets = assets.into_iter().collect::<Vec<_>>();
        assets.sort_by_cached_key(|(path, _)| {
            order
                .get(path)
                .cloned()
                .unwrap_or_else(|| crate::media_order::key(&json!({"Name":path.to_string_lossy()})))
        });
        context.save_plan("media-restore-files", &json!({"assets":assets,"labels":labels}),
            ).await?;
        assets
    };
    let total = assets.len();
    for (index, (path, bindings)) in assets.into_iter().enumerate() {
        context.checkpoint().await?;
        let label = labels.get(&path).cloned().unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        if !path.starts_with(&root) {
            failures.push(format!("{}：文件已不在当前配置的本地媒体目录，未恢复", path.display()));
            continue;
        }
        context
            .update_counted(
                index as u64,
                total as u64,
                "还原媒体信息",
                &format!("正在检查 {label}"),
            )
            .await;
        if crate::emby::path_ignored_in(state, path.to_str().unwrap_or(""), &folders).await {
            continue;
        }
        let final_path = path.with_file_name(format!(
            "{}-mediainfo.json",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ));
        if !full && final_path.is_file() {
            let existing = match tokio::fs::read(&final_path).await {
                Ok(existing) => existing,
                Err(error) => {
                    tracing::warn!(
                        "[媒体信息还原] {} 无法读取，保留并继续其他文件：{error}",
                        final_path.display()
                    );
                    failures.push(format!(
                        "{}：现有 JSON 不可读取，未覆盖",
                        final_path.display()
                    ));
                    continue;
                }
            };
            let info = match serde_json::from_slice::<Value>(&existing) {
                Ok(info) => info,
                Err(error) => {
                    tracing::warn!(
                        "[媒体信息还原] {} 的 JSON 无法解析，保留并继续其他文件：{error}",
                        final_path.display()
                    );
                    failures.push(format!(
                        "{}：现有 JSON 无法解析，未覆盖",
                        final_path.display()
                    ));
                    continue;
                }
            };
            if crate::media_info::recoverable(&info)
                && crate::media_info::restore_json(&info).is_ok()
                && crate::media_info::container(&info)
                    .is_some_and(|c| !string(c, "Container").eq_ignore_ascii_case("strm"))
            {
                if let Some(prepared) = context.plan(&format!("media-restore-source:{}", path.display())).await? {
                    if prepared["written"] == true
                        && prepared["source"] == crate::media_paths::strm_target(path.to_str().unwrap_or("")).await.unwrap_or_default()
                        && crate::media_info::restore_json(&prepared["info"]).ok() == crate::media_info::restore_json(&info).ok()
                    {
                        updated += 1;
                    }
                }
                for (id, asset) in bindings.iter().filter(|(id, _)| !id.is_empty()) {
                    let candidate=async {
                        crate::identity::verify(asset,&info)?;
                        let expected=crate::media_info::restore_json(&info)?;
                        let expected_path=string(asset,"path");
                        let source=crate::media_paths::strm_target(&expected_path).await.unwrap_or_default();
                        if !crate::media_paths::matches_container(&expected,&source){return Err(ApiError::bad("备份格式与当前 STRM 来源不一致"));}
                        let live=crate::media_edit::details(state,id,"Id,Path,Type,Size,MediaSources,MediaStreams,Chapters").await?;
                        if live["Id"]!=*id || string(&live,"Path")!=expected_path{return Err(ApiError::bad("Emby 媒体身份或路径变化"));}
                        crate::identity::verify(&json!({"size_bytes":live["Size"]}),&expected)?;
                        if crate::media_info::has_media_streams(&live,&[&expected_path,&source]){return Ok::<_,ApiError>(None);}
                        let current=crate::media_info::recovery_chapters(&expected,&live,&[&expected_path,&source])?;
                        if crate::media_info::signature(&current)?!=crate::media_info::signature(&crate::media_info::object(&expected).unwrap()["Chapters"])? {
                            return Err(ApiError::bad("现有 JSON 与当前章节标记不一致，未自动覆盖或清理，请重新保存媒体信息"));
                        }
                        Ok(Some(ImportTarget{path:expected_path,source,info:expected,asset:asset.clone()}))
                    }.await;
                    match candidate {
                        Ok(Some(target)) => {
                            native_imports.insert(id.clone(), target);
                        }
                        Ok(None) => {
                            if context.completed(&format!("media-restore-import:{id}")).await?.is_some() {
                                verified_ids.push(id.clone());
                            }
                        }
                        Err(error) if context.is_cancelled() => return Err(error),
                        Err(error) => {
                            failures.push(format!("{id}：未导入现有备份：{}", error.1));
                        }
                    }
                }
                if bindings.iter().all(|(id, _)| id.is_empty()) {
                    pending_import.push(path.to_string_lossy().to_string());
                }
                tracing::info!(
                    "[媒体信息还原] {label} 已有可用 JSON，保留文件并核对缺失的 Emby 媒体信息"
                );
                continue;
            }
        }
        let result=async {
            context.activity("还原媒体信息", &format!("正在准备 {label} 的媒体流和章节信息")).await;
            if bindings.iter().any(|(_,a)|a["fingerprint_pending"]==true){return Err(ApiError::bad("文件身份待校验"));}
            let source=tokio::fs::read_to_string(&path).await?;
            let source=source.trim().replace('\\',"/");
            if source.is_empty(){return Err(ApiError::bad("STRM 内容为空"));}
            let (pc,sha,filename,size)=if full || !string(&config,"p115_fingerprint_media_root_cid").is_empty(){
                let (live,_)=crate::revalidation::resolve(state,&source,path.to_str().unwrap_or("")).await?;
                (string(&live,"pick_code"),string(&live,"sha1"),string(&live,"name"),crate::identity::size(&live["size"]))
            }else{
                let (pc,sha,_)=crate::fingerprints::lookup(state,path.to_str().unwrap_or("")).await?;
                let filename=Path::new(&crate::media_paths::clean(&source)).file_name().unwrap_or_default().to_string_lossy().to_string();
                (pc.unwrap_or_default(),sha.unwrap_or_default(),filename,None)
            };
            if sha.is_empty() || pc.is_empty(){return Err(ApiError::bad("缺少115 PC或SHA1"));}
            let prepared_key = format!("media-restore-source:{}", path.display());
            let prepared = context.plan(&prepared_key).await?;
            let reusable = prepared.as_ref().filter(|prepared| prepared["source"] == source
                && prepared["pick_code"] == pc && prepared["sha1"] == sha && prepared["size"] == json!(size));
            let mut info = if let Some(prepared) = reusable {
                crate::media_info::restore_json(&prepared["info"])?
            } else {
                crate::media_info::restore_json(&info(state,&sha,&pc,&filename,full,context).await?)?
            };
            if let Some(size)=size{crate::identity::verify(&json!({"size_bytes":size}),&info)?;}
            for (_,asset) in &bindings{crate::identity::verify(asset,&info)?;}
            let mut chapters=None;
            let mut observations=vec![];
            let mut imports=vec![];
            let members=bindings.iter().filter(|(id,_)|!id.is_empty()).collect::<Vec<_>>();
            for (id,asset) in &members {
                let item=crate::media_edit::details(state,id,"Path,Type,Size,MediaSources,MediaStreams,Chapters").await?;
                observations.push(((*id).clone(),crate::revalidation::observation(state,&item).await));
                if string(&item,"Path")!=string(asset,"path"){return Err(ApiError::bad("Emby 路径已变化"));}
                crate::identity::verify(&json!({"size_bytes":item["Size"]}),&info)?;
                let expected_path=string(&item,"Path");
                let current=crate::media_info::recovery_chapters(&info,&item,&[&expected_path,&source])?;
                if chapters.as_ref().is_some_and(|c|c!=&current){return Err(ApiError::bad("同路径的章节标记不一致"));}
                chapters=Some(current);
                if !crate::media_info::has_media_streams(&item,&[&expected_path,&source]) {
                    imports.push(((*id).clone(),expected_path,(*asset).clone()));
                }
            }
            if let Some(chapters)=chapters{crate::media_info::preserve_chapters(&mut info,&chapters)?;}
            context.checkpoint().await?;
            if tokio::fs::read_to_string(&path).await?.trim().replace('\\',"/")!=source{return Err(ApiError::bad("STRM 内容在提取期间变化"));}
            for (id,before) in &observations {
                let current=crate::media_edit::details(state,id,"Path,Size,MediaSources").await?;
                if &crate::revalidation::observation(state,&current).await!=before{return Err(ApiError::bad("Emby 文件在提取期间变化"));}
            }
            let already_written = reusable.is_some_and(|prepared| prepared["written"] == true);
            context.save_plan(&prepared_key, &json!({"source":source,"pick_code":pc,"sha1":sha,"size":size,"info":info,"written":already_written})).await?;
            if full && !members.is_empty() {
                // Prepare every selected version before any clear. The shared save
                // protects linked versions that are outside the selected path list.
                for (id,_) in members { full_rebuilt.insert(id.clone(),info.clone()); }
                full_paths.insert(path.clone());
                return Ok::<_,ApiError>(false);
            }
            let stage=path.with_file_name(format!(".etk-mediainfo-{}",uuid::Uuid::new_v4().simple()));
            let write=async {
                let mut file=tokio::fs::OpenOptions::new().write(true).create_new(true).open(&stage).await?;
                file.write_all(&serde_json::to_vec(&info).unwrap()).await?;file.sync_all().await?;drop(file);
                context.checkpoint().await?;
                if tokio::fs::read_to_string(&path).await?.trim().replace('\\',"/")!=source{return Err(ApiError::bad("STRM 内容在保存期间变化"));}
                tokio::fs::rename(&stage,&final_path).await?;
                sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                    .bind(&sha).bind(&info).execute(&state.db).await?;
                Ok::<_,ApiError>(())
            }.await;
            let _=tokio::fs::remove_file(&stage).await;
            write?;
            context.save_plan(&prepared_key, &json!({"source":source,"pick_code":pc,"sha1":sha,"size":size,"info":info,"written":true})).await?;
            if members.is_empty() { pending_import.push(path.to_string_lossy().to_string()); }
            for (id,item_path,asset) in imports { native_imports.insert(id,ImportTarget{path:item_path,source:source.clone(),info:info.clone(),asset}); }
            Ok::<_,ApiError>(true)
        }.await;
        match result {
            Ok(written) => {
                if written {
                    updated += 1;
                    tracing::info!(
                        "[媒体信息还原] {label} 本地 JSON 已恢复：{}",
                        final_path.display()
                    );
                } else {
                    tracing::info!("[媒体信息还原] {label} 已准备，等待全版本共同保存与恢复");
                }
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!("[媒体信息还原] {label} 失败：{}", error.1);
                failures.push(format!("{}: {}", path.display(), error.1));
            }
        }
    }
    // The plugin task scans the selected library, not a list of Item IDs.
    // Do not let a valid item cause a rejected stale backup in the same batch
    // to be imported as a side effect of starting that library-wide task.
    let blocked = !failures.is_empty();
    let deferred_ids = if blocked {
        full_rebuilt
            .keys()
            .chain(native_imports.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    if blocked && !deferred_ids.is_empty() {
        failures.push(format!("本批存在无法确认身份或备份的目标，未清理或启动媒体库恢复任务；已准备 {} 个绑定目标待处理",deferred_ids.len()));
    }
    if !blocked && let Some(first) = full_rebuilt.keys().next().cloned() {
        context.checkpoint().await?;
        let ids = full_rebuilt.keys().cloned().collect::<Vec<_>>();
        let body =
            json!({"mediainfo":full_rebuilt[&first],"target_item_ids":ids,"stream_patches":[]});
        let saved = if let Some(previous) = context.completed("media-restore-full-save").await? {
            Ok((previous["status"].as_u64().unwrap_or(500) as u16, previous["report"].clone(),
            ))
        } else {
            crate::media_save::save_with_rebuilt(state, &first, &body, &full_rebuilt, context).await
        };
        match saved {
            Ok((status, report)) => {
                if status < 400 && report["failures"].as_array().is_none_or(Vec::is_empty) {
                    let task_result = restore_report(updated + full_paths.len(), &ids, &pending_import, &[], &report, &[],
                    );
                    context.complete("media-restore-full-save", &json!({"status":status,"report":report,"ids":ids,"task_result":task_result})).await?;
                    updated += full_paths.len();
                    verified_ids.extend(ids);
                } else {
                    failures.push(format!(
                        "全版本媒体信息恢复未全部完成：{}",
                        report["error"].as_str().unwrap_or("查看全版本恢复报告")
                    ));
                }
                full_report = report;
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => failures.push(format!("全版本媒体信息恢复失败：{}", error.1)),
        }
    }
    if !blocked && !native_imports.is_empty() {
        context.checkpoint().await?;
        match restore_imports(state, &native_imports, context).await {
            Ok(()) => verified_ids.extend(native_imports.keys().cloned()),
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => failures.push(format!("本地 JSON 已恢复，原生导入尚未确认：{}", error.1)),
        }
    }
    let report = restore_report(updated, &verified_ids, &pending_import, &deferred_ids, &full_report, &failures,
    );
    if failures.is_empty() {
        context.complete("media-restore-result", &report).await?;
    }
    let message = report["message"].as_str().unwrap();
    tracing::info!("[媒体信息还原] {message}");
    context
        .update_counted(total as u64, total as u64, "媒体信息还原", message)
        .await;
    Ok(report)
}

fn restore_report(
    updated: usize,
    verified_ids: &[String],
    pending_import: &[String],
    deferred_ids: &[String],
    full_report: &Value,
    failures: &[String],
) -> Value {
    let message = format!(
        "媒体信息还原结束：本地恢复 {updated} 个文件，Emby 回读确认 {} 个目标，未绑定待导入 {} 个，失败 {} 项",
        verified_ids.len(), pending_import.len(), failures.len()
    );
    json!({"updated":updated,"verified_ids":verified_ids,"pending_import":pending_import,"deferred_ids":deferred_ids,"full_restore":full_report,"failures":failures,"message":message})
}

struct ImportTarget {
    path: String,
    source: String,
    info: Value,
    asset: Value,
}
async fn restore_imports(
    state: &AppState,
    imports: &BTreeMap<String, ImportTarget>,
    context: &JobContext,
) -> Result<()> {
    let mut pending = std::collections::BTreeSet::new();
    for (id, target) in imports {
        let completion_key = format!("media-restore-import:{id}");
        if let Some(previous) = context.completed(&completion_key).await? {
            if previous["path"] != target.path || previous["source"] != target.source {
                return Err(ApiError::bad(format!("{id}：已验收导入的目标路径或来源发生变化")));
            }
            continue;
        }
        let current = crate::media_edit::target(state, id).await?;
        if current.path != target.path || current.asset != target.asset {
            return Err(ApiError::bad(format!(
                "{id}：恢复前媒体绑定或身份变化，未启动导入"
            )));
        }
        crate::identity::verify(&current.asset, &target.info)?;
        if crate::media_paths::strm_target(&target.path)
            .await
            .unwrap_or_default()
            != target.source
        {
            return Err(ApiError::bad(format!(
                "{id}：恢复前 STRM 来源变化，未启动导入"
            )));
        }
        let live =
            crate::media_edit::details(state, id, "Id,Path,Size,MediaSources,MediaStreams,Chapters",
        ).await?;
        if live["Id"] != *id || string(&live, "Path") != target.path {
            return Err(ApiError::bad(format!(
                "{id}：恢复前 Emby 路径或身份变化，未启动导入"
            )));
        }
        crate::identity::verify(&json!({"size_bytes":live["Size"]}), &target.info)?;
        let chapters = &crate::media_info::object(&target.info).unwrap()["Chapters"];
        if crate::media_info::stream_differences(&target.info, &live, &[&target.path, &target.source],
        ).is_empty()
            && crate::media_info::chapters_restored(chapters, &live["Chapters"])
        {
            context.complete(&completion_key, &json!({"path":target.path,"source":target.source}),
                ).await?;
        } else {
            pending.insert(id.clone());
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    let targets = pending.iter().map(|id| (id.clone(), imports[id].path.clone())).collect::<Vec<_>>();
    let settings = crate::media_save::restoration_settings(state, &targets, false).await?;
    let recovery = JobContext::for_recovery(Some(context)).await;
    crate::strm::with_changes(state, &settings, || async {
        crate::emby::scheduled(state, "MediaInfoExtractTask", &recovery).await
    })
    .await?;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut last = String::new();
    while !pending.is_empty() {
        let paused = tokio::time::Instant::now();
        context.checkpoint().await?;
        deadline += paused.elapsed();
        for id in pending.clone() {
            let target = &imports[&id];
            let path = &target.path;
            let expected = &target.info;
            let live=tokio::select! {
                _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
                result=tokio::time::timeout_at(deadline,crate::media_edit::details(state,&id,"Id,Path,Type,MediaSources,MediaStreams,Chapters"))=>result,
            }.map_err(|_|ApiError::bad("Emby 导入回读超时，JSON 已保留"))??;
            let source = crate::media_paths::strm_target(path)
                .await
                .unwrap_or_default();
            if source != target.source {
                return Err(ApiError::bad(format!(
                    "{id}：导入期间 STRM 来源变化，JSON 已保留"
                )));
            }
            let mut differences =
                crate::media_info::stream_differences(expected, &live, &[path, &source]);
            if live["Id"] != id || string(&live, "Path") != *path {
                differences.push("媒体身份或路径变化".into());
            }
            let chapters = &crate::media_info::object(expected).unwrap()["Chapters"];
            if !crate::media_info::chapters_restored(chapters, &live["Chapters"]) {
                differences.push("章节或片头片尾标记未恢复".into());
            }
            if differences.is_empty() {
                context.complete(&format!("media-restore-import:{id}"), &json!({"path":path,"source":source}),
                    ).await?;
                pending.remove(&id);
            } else {
                last = format!("{id}：{}", differences.join("；"));
            }
        }
        if pending.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::bad(format!(
                "Emby 导入未确认：{last}；JSON 已保留"
            )));
        }
        let delay = Duration::from_secs(1)
            .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        tokio::select! {
            _=context.cancelled()=>{context.checkpoint().await?;},
            _=tokio::time::sleep(delay)=>{},
        }
    }
    Ok(())
}
