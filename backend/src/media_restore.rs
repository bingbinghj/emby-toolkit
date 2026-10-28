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
    if crate::media_paths::extension(filename) == ".iso" {
        let size: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT size FROM p115_filesystem_cache WHERE pick_code=$1 LIMIT 1",
        )
        .bind(pc)
        .fetch_optional(&state.db)
        .await?
        .flatten();
        match crate::p115_iso::probe_115(
            state,
            pc,
            filename,
            size.unwrap_or(0).max(0) as u64,
            Some(context),
        )
        .await
        {
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
    let url = match crate::p115::download_url(state, pc).await {
        Ok(url) => url,
        Err(_) => crate::p115::openapi_downurl_with_user_agent(state, pc, "Mozilla/5.0").await?,
    };
    let _slot = crate::strm::slot(state, "main", Some(context)).await?;
    let mut command = tokio::process::Command::new("ffprobe");
    command.kill_on_drop(true).args([
        "-hide_banner",
        "-v",
        "error",
        "-user_agent",
        "Mozilla/5.0",
        "-rw_timeout",
        "15000000",
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
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .map_err(|_| ApiError::bad("ffprobe 提取超时"))??;
    drop(_slot);
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
                return Ok(cache["mediainfo_json"].clone());
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
    sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,raw_ffprobe_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET raw_ffprobe_json=EXCLUDED.raw_ffprobe_json").bind(sha).bind(raw).execute(&state.db).await?;
    Ok(info)
}

pub async fn restore(state: &AppState, full: bool, context: &JobContext) -> Result<Value> {
    if full {
        return crate::strm::with_settings(state, "preserve", || {
            restore_files(state, full, context)
        })
        .await;
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
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Episode')").fetch_all(&state.db).await?;
    let mut assets = BTreeMap::<PathBuf, Vec<(String, Value)>>::new();
    let mut labels = BTreeMap::new();
    for row in rows {
        let ids = list(&row["emby_item_ids_json"]);
        for (index, asset) in list(&row["asset_details_json"]).into_iter().enumerate() {
            if crate::emby::path_ignored(state, &string(&asset, "path")).await? {
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
                assets.entry(path).or_default().push((id, asset));
            }
        }
    }
    if !full {
        let mut pending = vec![root];
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
    let mut affected = std::collections::BTreeSet::new();
    let total = assets.len();
    for (index, (path, bindings)) in assets.into_iter().enumerate() {
        context.checkpoint().await?;
        let label = labels.get(&path).cloned().unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        context
            .update_counted(
                index as u64,
                total as u64,
                "还原媒体信息",
                &format!("正在检查 {label}"),
            )
            .await;
        if crate::emby::path_ignored(state, path.to_str().unwrap_or("")).await? {
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
                    continue;
                }
            };
            if crate::media_info::container(&info)
                .is_none_or(|c| !string(c, "Container").eq_ignore_ascii_case("strm"))
            {
                tracing::info!("[媒体信息还原] 跳过 {label}：已有可用媒体信息文件");
                continue;
            }
        }
        let result=async {
            context.activity("还原媒体信息", &format!("正在还原 {label} 的媒体流和章节信息")).await;
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
            let mut info=info(state,&sha,&pc,&filename,full,context).await?;
            if let Some(size)=size{crate::identity::verify(&json!({"size_bytes":size}),&info)?;}
            for (_,asset) in &bindings{crate::identity::verify(asset,&info)?;}
            let mut chapters=None;
            let mut observations=vec![];
            if full {
                for (id,asset) in &bindings {
                    let item=crate::media_edit::details(state,id,"Path,Size,MediaSources,Chapters").await?;
                    observations.push((id.clone(),crate::revalidation::observation(state,&item).await));
                    if string(&item,"Path")!=string(asset,"path"){return Err(ApiError::bad("Emby 路径已变化"));}
                    crate::identity::verify(&json!({"size_bytes":item["Size"]}),&info)?;
                    let current=crate::media_info::clean_chapters(&item["Chapters"])?;
                    if chapters.as_ref().is_some_and(|c|c!=&current){return Err(ApiError::bad("同路径的章节标记不一致"));}
                    chapters=Some(current);
                }
            }
            if let Some(chapters)=chapters{crate::media_info::preserve_chapters(&mut info,&chapters)?;}
            let stage=path.with_file_name(format!(".etk-mediainfo-{}",uuid::Uuid::new_v4().simple()));
            let mut file=tokio::fs::OpenOptions::new().write(true).create_new(true).open(&stage).await?;
            let write=async {
                file.write_all(&serde_json::to_vec(&info).unwrap()).await?;file.sync_all().await?;drop(file);
                context.checkpoint().await?;
                if tokio::fs::read_to_string(&path).await?.trim().replace('\\',"/")!=source{return Err(ApiError::bad("STRM 内容在提取期间变化"));}
                for (id,before) in &observations{
                    let current=crate::media_edit::details(state,id,"Path,Size,MediaSources").await?;
                    if &crate::revalidation::observation(state,&current).await!=before{return Err(ApiError::bad("Emby 文件在提取期间变化"));}
                }
                let mut errors=vec![];
                if full {
                    for (id,_) in &bindings {
                        if let Err(error)=state.emby(reqwest::Method::POST,&format!("Items/{id}/ClearMediaInfo"),&[],None).await{errors.push(error.1);}
                    }
                }
                // Finish replacement after clear even when a stop request arrives.
                tokio::fs::rename(&stage,&final_path).await?;
                sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json").bind(sha).bind(info).execute(&state.db).await?;
                if !errors.is_empty(){return Err(ApiError::bad(format!("文件已恢复，但Emby清理失败：{}",errors.join(","))));}
                Ok::<_,ApiError>(())
            }.await;
            let _=tokio::fs::remove_file(&stage).await;
            write?;
            if let Some(library)=crate::emby::library_for_path(state,path.to_str().unwrap_or("")).await?{affected.insert(string(&library,"ItemId"));}
            Ok::<_,ApiError>(())
        }.await;
        match result {
            Ok(()) => {
                updated += 1;
                tracing::info!("[媒体信息还原] {label} 已恢复：{}", final_path.display());
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!("[媒体信息还原] {label} 失败：{}", error.1);
                failures.push(format!("{}: {}", path.display(), error.1));
            }
        }
    }
    for id in affected {
        context
            .activity(
                "刷新Emby",
                &format!("媒体信息文件已恢复，正在刷新所属媒体库（Item {id}）"),
            )
            .await;
        crate::emby::refresh_item(state, &id, true, context).await?;
    }
    context
        .update_counted(
            total as u64,
            total as u64,
            "媒体信息还原",
            &format!(
                "媒体信息还原完成：恢复 {updated} 个文件，失败 {} 个",
                failures.len()
            ),
        )
        .await;
    Ok(
        json!({"updated":updated,"failures":failures,"message":format!("媒体信息还原完成：恢复 {updated} 个文件，失败 {} 个", failures.len())}),
    )
}
