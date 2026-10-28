use crate::{
    AppState, audio,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{self, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/media_info/repair_truehd_core/{id}", post(submit))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task = state
        .jobs
        .submit("纠正高清音轨错位", move |context| async move {
            let ids = crate::revalidation::scope(&work, &id, &context).await?;
            repair(&work, &ids, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"高清音轨纠正已提交"})),
    ))
}
fn protected(path: &std::path::Path) -> bool {
    path.components().any(|c| {
        ["cd2", "115", "115open"].contains(&c.as_os_str().to_string_lossy().to_lowercase().as_str())
    })
}
async fn backup_root(state: &AppState) -> Result<PathBuf> {
    let root = state.data_dir.join("backups/truehd-core-shift");
    if tokio::fs::symlink_metadata(&root)
        .await
        .is_ok_and(|s| s.file_type().is_symlink())
    {
        return Err(ApiError::bad("音轨备份目录不能为符号链接"));
    }
    tokio::fs::create_dir_all(&root).await?;
    if !tokio::fs::canonicalize(&root)
        .await?
        .starts_with(tokio::fs::canonicalize(&state.data_dir).await?)
    {
        return Err(ApiError::bad("音轨备份目录越界"));
    }
    Ok(root)
}
pub fn path_hash(path: &str) -> String {
    Sha256::digest(path.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..12]
        .to_string()
}
async fn reference(
    state: &AppState,
    target: &media_edit::Target,
    context: &JobContext,
) -> Result<Option<(Value, String)>> {
    let raw: Option<Value> = if let Some(sha) = &target.sha {
        sqlx::query_scalar("SELECT raw_ffprobe_json FROM p115_mediainfo_cache WHERE sha1=$1")
            .bind(sha)
            .fetch_optional(&state.db)
            .await?
            .flatten()
    } else {
        None
    };
    let mut raw = raw;
    let mut evidence = "ffprobe 原始结构".to_string();
    let source = crate::media_paths::strm_target(&target.path)
        .await
        .unwrap_or(target.path.clone());
    let is_iso = crate::media_paths::extension(&source) == ".iso"
        || string(&target.asset, "container").eq_ignore_ascii_case("iso")
        || target.path.to_lowercase().ends_with(".iso.strm");
    if is_iso
        && raw.as_ref().is_some_and(|raw| {
            raw["_iso_probe"]["playlist_pid_filtered"] == true
                || raw["_iso_probe"]["probe_scope"] != "playlist_clpi_raw"
        })
    {
        raw = None;
    }
    if raw.is_none() {
        let config = state.config.read().await.clone();
        let root = string(&config, "cd2_media_root");
        if !root.is_empty() {
            let source = crate::media_paths::strm_target(&target.path)
                .await
                .unwrap_or(target.path.clone());
            let candidates = crate::media_paths::local_candidates(
                &target.path,
                &source,
                &[&string(&config, "local_strm_root"), &root],
            );
            let path = if is_iso {
                let mut unique = std::collections::BTreeSet::new();
                let cd2_root = tokio::fs::canonicalize(&root).await?;
                for path in candidates.iter().map(|p| PathBuf::from(&root).join(p)) {
                    if path.is_file()
                        && path
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("iso"))
                    {
                        let path = tokio::fs::canonicalize(path).await?;
                        if path.starts_with(&cd2_root) {
                            unique.insert(path);
                        }
                    }
                }
                if unique.len() > 1 {
                    return Err(ApiError::bad("CD2 ISO 无法唯一定位，拒绝自动删除音轨"));
                }
                unique.into_iter().next()
            } else {
                candidates
                    .iter()
                    .map(|p| PathBuf::from(&root).join(p))
                    .find(|p| p.is_file())
            };
            if let Some(path) = path {
                if is_iso {
                    let _slot = crate::strm::slot(state, "main", Some(context)).await?;
                    raw = Some(crate::p115_iso::probe_file(&path, Some(context)).await?);
                    evidence = "CD2 蓝光 CLPI/PID".into();
                } else {
                    let _slot = crate::strm::slot(state, "main", Some(context)).await?;
                    let mut command = tokio::process::Command::new("ffprobe");
                    command
                        .kill_on_drop(true)
                        .args([
                            "-v",
                            "error",
                            "-print_format",
                            "json",
                            "-show_streams",
                            "-show_format",
                        ])
                        .arg(&path);
                    let output =
                        tokio::time::timeout(std::time::Duration::from_secs(45), command.output())
                            .await;
                    drop(_slot);
                    if let Ok(Ok(output)) = output
                        && output.status.success()
                    {
                        raw = serde_json::from_slice(&output.stdout).ok();
                        evidence = "CD2 ffprobe 原始结构".into();
                    }
                }
            }
        }
    }
    if raw.is_none() && is_iso {
        let pc = if let Some(pc) = crate::fingerprints::pickcode(&source) {
            pc
        } else {
            let pc: Option<String> = sqlx::query_scalar("SELECT pick_code FROM p115_filesystem_cache WHERE UPPER(sha1)=UPPER($1) AND pick_code IS NOT NULL LIMIT 1")
                .bind(&target.sha).fetch_optional(&state.db).await?;
            pc.unwrap_or_default()
        };
        if !pc.is_empty() {
            let mut filename = std::path::Path::new(&crate::media_paths::clean(&source))
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if !filename.to_lowercase().ends_with(".iso") {
                let name: Option<String> = sqlx::query_scalar(
                    "SELECT name FROM p115_filesystem_cache WHERE pick_code=$1 LIMIT 1",
                )
                .bind(&pc)
                .fetch_optional(&state.db)
                .await?;
                filename = name
                    .filter(|n| n.to_lowercase().ends_with(".iso"))
                    .unwrap_or_else(|| filename.trim_end_matches(".strm").to_string());
            }
            raw = Some(
                crate::p115_iso::probe_115(
                    state,
                    &pc,
                    &filename,
                    target.asset["size_bytes"].as_u64().unwrap_or(0),
                    Some(context),
                )
                .await?,
            );
        }
    }
    let Some(mut raw) = raw else {
        return Ok(None);
    };
    let stats = &raw["_iso_probe"];
    if stats.is_object() {
        if stats["playlist_pid_filtered"] == true
            || stats["probe_scope"] != "playlist_clpi_raw"
            || stats["reference_safe"] != true
        {
            return Err(ApiError::bad("ISO原始CLPI探测不完整或格式过旧"));
        }
        if !evidence.starts_with("CD2") {
            evidence = "蓝光 CLPI/PID".into();
        }
    } else {
        // DTS cores can be proven by SOURCE_ID even when container Index differs.
        let streams = list(&raw["streams"]);
        let mut previous: Option<&Value> = None;
        let mut remove = HashSet::new();
        let source_id = |s: &Value| {
            s["tags"]
                .as_object()
                .and_then(|o| {
                    o.iter()
                        .find(|(k, _)| {
                            ["source-id", "source-id-eng"]
                                .contains(&k.to_lowercase().replace('_', "-").as_str())
                        })
                        .map(|(_, v)| v.as_str().unwrap_or("").to_lowercase())
                })
                .unwrap_or_else(|| s["id"].as_str().unwrap_or("").into())
        };
        for (index, stream) in streams
            .iter()
            .enumerate()
            .filter(|(_, s)| s["codec_type"] == "audio")
        {
            if let Some(parent) = previous {
                let a = json!({"Codec":parent["codec_name"],"Profile":parent["profile"],"DisplayTitle":parent["tags"]["title"]});
                let b = json!({"Codec":stream["codec_name"],"Profile":stream["profile"],"DisplayTitle":stream["tags"]["title"],"SampleRate":stream["sample_rate"],"BitRate":stream["bit_rate"]});
                if a["Codec"] == "dts"
                    && audio::compatible(&a, &b, false)
                    && !source_id(parent).is_empty()
                    && source_id(parent) == source_id(stream)
                {
                    remove.insert(index);
                }
            }
            previous = Some(stream);
        }
        if !remove.is_empty() {
            raw["streams"] = json!(
                streams
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| !remove.contains(i))
                    .map(|(_, s)| s)
                    .collect::<Vec<_>>()
            );
            evidence = "ffprobe 同源 SOURCE_ID".into();
        }
    }
    let source = crate::media_paths::strm_target(&target.path)
        .await
        .unwrap_or(target.path.clone());
    let mappings = crate::db::setting(&state.db, "language_mapping")
        .await?
        .unwrap_or(json!([]));
    let info = crate::probe::build(&raw, &json!({"fn":source}), &mappings)
        .ok_or_else(|| ApiError::bad("可信探测无法转换媒体信息"))?;
    crate::identity::verify(&target.asset, &info)?;
    Ok(Some((info, evidence)))
}
fn removed_are_cores(info: &Value, report: &Value) -> bool {
    let Some(streams) =
        crate::media_info::container(info).and_then(|s| s["MediaStreams"].as_array())
    else {
        return false;
    };
    let audio = streams
        .iter()
        .enumerate()
        .filter(|(_, s)| s["Type"] == "Audio")
        .collect::<Vec<_>>();
    let positions = list(&report["core_stream_positions"]);
    !positions.is_empty()
        && positions.iter().all(|p| {
            let Some(index) = audio
                .iter()
                .position(|(i, _)| Some(*i as u64) == p.as_u64())
            else {
                return false;
            };
            index > 0 && crate::audio::compatible(audio[index - 1].1, audio[index].1, true)
        })
}
pub async fn repair_all(state: &AppState, context: &JobContext) -> Result<Value> {
    context
        .activity("扫描高清音轨", "正在扫描媒体库中的高清音轨兼容核心结构")
        .await;
    let user = string(&*state.config.read().await, "emby_user_id");
    if user.is_empty() {
        return Err(ApiError::bad("未配置 Emby 用户 ID"));
    }
    let libraries = state
        .emby(
            reqwest::Method::GET,
            &format!("emby/Users/{user}/Views"),
            &[],
            None,
        )
        .await?;
    let libraries = libraries["Items"]
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 媒体库列表无效"))?;
    let mut ids = std::collections::BTreeSet::new();
    let mut scanned = 0;
    for library in libraries {
        context.checkpoint().await?;
        let library_id = string(library, "Id");
        if library_id.is_empty() {
            return Err(ApiError::bad("Emby 媒体库缺少 ID"));
        }
        if crate::emby::library_ignored(&*state.config.read().await, &library_id) {
            continue;
        }
        context
            .activity(
                "扫描高清音轨",
                &format!(
                    "正在扫描媒体库《{}》，已读取 {scanned} 个媒体项",
                    string(library, "Name")
                ),
            )
            .await;
        let items = crate::emby::query_items(
            state,
            &[
                ("ParentId", library_id),
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Movie,Episode,Video".into()),
                (
                    "Fields",
                    "Name,Path,PartCount,ExtraType,MediaSources,MediaStreams,Container,Type".into(),
                ),
            ],
            context,
        )
        .await?;
        scanned += items.len();
        for item in items {
            if item["PartCount"].as_u64().unwrap_or(0) > 1 || !string(&item, "ExtraType").is_empty()
            {
                continue;
            }
            let sources = list(&item["MediaSources"]);
            if sources.is_empty() {
                ids.insert(string(&item, "Id"));
            } else {
                for source in sources {
                    let id = string(&source, "ItemId");
                    let source_id = string(&source, "Id");
                    ids.insert(if !id.is_empty() {
                        id
                    } else if !source_id.is_empty() && !source_id.starts_with("mediasource_") {
                        source_id
                    } else {
                        string(&item, "Id")
                    });
                }
            }
        }
    }
    let mut result = repair(state, &ids.into_iter().collect::<Vec<_>>(), context).await?;
    result["scanned"] = json!(scanned);
    Ok(result)
}
pub async fn repair(state: &AppState, ids: &[String], context: &JobContext) -> Result<Value> {
    let mut result = json!({"scanned":ids.len(),"candidates":0,"ambiguous":0,"ambiguous_items":[],"repaired":0,"repaired_items":[],"skipped":0,"skipped_items":[],"failed":0,"failed_items":[]});
    let root =
        tokio::fs::canonicalize(string(&*state.config.read().await, "local_strm_root")).await?;
    if protected(&root) {
        return Err(ApiError::bad("媒体根目录位于115源目录"));
    }
    let batch = backup_root(state)
        .await?
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    for (index, id) in ids.iter().enumerate() {
        context.checkpoint().await?;
        let mut label = format!("Item {id}");
        context
            .update_counted(
                index as u64,
                ids.len() as u64,
                "音轨纠正",
                &format!("正在读取第 {} 个媒体的音轨信息（Item {id}）", index + 1),
            )
            .await;
        let outcome=async{
            let item=media_edit::details(state,id,"Name,Path,MediaSources,MediaStreams,PartCount,ExtraType").await?;
            label=format!("《{}》 · {}（Item {id}）", string(&item,"Name"), string(&item,"Path").replace('\\',"/").rsplit('/').next().unwrap_or(""));
            context.activity("分析音轨", &format!("正在核对 {label} 的高清音轨与兼容核心")).await;
            if crate::emby::path_ignored(state,&string(&item,"Path")).await?{return Ok(("skipped",json!({"item_id":id,"reason":"忽略媒体库"})));}
            if item["PartCount"].as_u64().unwrap_or(0)>1||!string(&item,"ExtraType").is_empty(){return Ok(("skipped",json!({"item_id":id,"reason":"分段或附加内容"})));}
            let target=media_edit::repair_target(state,id).await?;
            let original=media_edit::load(state,&target).await?;
            let live=list(&item["MediaSources"]).into_iter().find(|s|s["Path"]==target.path||s["Id"]==id.as_str()).unwrap_or_else(||json!({"MediaStreams":item["MediaStreams"]}));
            let streams=crate::media_info::container(&original).map(|c|list(&c["MediaStreams"])).unwrap_or_default();
            let mut audio=streams.iter().filter(|s|s["Type"]=="Audio").collect::<Vec<_>>();
            let potential=audio.windows(2).any(|p|crate::audio::compatible(p[0],p[1],true));audio.sort_by_key(|s|s["Index"].as_i64().unwrap_or(0));
            if !potential&&!audio.windows(2).any(|p|crate::audio::compatible(p[0],p[1],true)){return Ok(("skipped",json!({"item_id":id,"reason":"未发现高清兼容核心结构"})));}
            let Some((reference,evidence))=reference(state,&target,context).await?else{return Ok(("ambiguous",json!({"item_id":id,"reason":"缺少可信原始探测，拒绝自动删除"})));};
            let report=audio::analyze(&original,Some(&reference),&evidence,true);let live_report=audio::analyze(&live,Some(&reference),&evidence,true);
            if report["safe"]!=true{let kind=if report["ambiguous"]==true{"ambiguous"}else{"skipped"};return Ok((kind,json!({"item_id":id,"reason":report["reason"]})));}
            if live_report["safe"]!=true||report["core_stream_positions"]!=live_report["core_stream_positions"]||report["metadata_signature"]!=live_report["metadata_signature"]{return Ok(("ambiguous",json!({"item_id":id,"reason":"Emby与本地JSON纠正结构不一致"})));}
            if evidence.contains("CLPI")&&(!removed_are_cores(&original,&report)||!removed_are_cores(&live,&live_report)){return Ok(("ambiguous",json!({"item_id":id,"reason":"CLPI与当前Core结构无法共同确认"})));}
            let mut backups=vec![];
            let version_ids=target.versions.iter().map(|v|string(v,"emby_id")).chain(std::iter::once(id.clone())).collect::<std::collections::BTreeSet<_>>();
            tokio::fs::create_dir_all(&batch).await?;
            for version in version_ids{
                let target=media_edit::repair_target(state,&version).await?;let info=media_edit::load(state,&target).await?;
                let path=target.candidates.iter().find(|p|p.is_file()).ok_or_else(||ApiError::bad("关联版本缺失媒体信息"))?;let path=tokio::fs::canonicalize(path).await?;
                if !path.starts_with(&root)||protected(&path){return Err(ApiError::bad("关联版本媒体信息路径不可写"));}
                if backups.iter().any(|(p,_,_)|p==&path){return Err(ApiError::bad("关联版本共用媒体信息JSON"));}
                let backup=batch.join(format!("{version}-{}.json",path_hash(path.to_str().unwrap())));tokio::fs::copy(&path,&backup).await?;backups.push((path,backup,info));
            }
            let (repaired,_)=audio::apply(&original,report.clone());
            let body=json!({"mediainfo":repaired,"sha1":target.sha,"media_path":target.path});
            let rebuilt=BTreeMap::from([(id.clone(),body["mediainfo"].clone())]);
            let outcome=crate::strm::with_settings(state,"preserve",||crate::media_save::save_with_rebuilt(state,id,&body,&rebuilt,context)).await;
            match outcome{
                Ok((status,value)) if status<400=>Ok(("repaired",json!({"item_id":id,"pair_count":report["pair_count"],"evidence":evidence,"result":value}))),
                other=>{
                    for (path,backup,_) in backups{tokio::fs::copy(backup,path).await?;}
                    let _=crate::media_save::refresh(state,&[target.path]).await;
                    let reason=match other{Ok((_,v))=>string(&v,"error"),Err(e)=>e.1};Err(ApiError::bad(reason))
                }
            }
        }.await;
        match outcome {
            Ok((kind, item)) => {
                if kind == "repaired" {
                    tracing::info!(
                        "[音轨纠正] {label} 已纠正 {} 组兼容核心错位",
                        item["pair_count"].as_u64().unwrap_or(0)
                    );
                } else {
                    tracing::warn!("[音轨纠正] 跳过 {label}：{}", string(&item, "reason"));
                }
                let key = match kind {
                    "repaired" => "repaired_items",
                    "ambiguous" => "ambiguous_items",
                    _ => "skipped_items",
                };
                result[key].as_array_mut().unwrap().push(item);
                result[kind] = json!(result[kind].as_u64().unwrap() + 1);
                if kind == "repaired" {
                    result["candidates"] = json!(result["candidates"].as_u64().unwrap() + 1);
                }
            }
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::error!("[音轨纠正] {label} 失败：{}", error.1);
                result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                result["failed_items"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"item_id":id,"reason":error.1}));
            }
        }
    }
    context.checkpoint().await?;
    result["message"] = json!(format!(
        "高清音轨纠正完成：成功 {}，歧义 {}，失败 {}",
        result["repaired"], result["ambiguous"], result["failed"]
    ));
    context
        .update_counted(
            ids.len() as u64,
            ids.len() as u64,
            "音轨纠正",
            &string(&result, "message"),
        )
        .await;
    let reports = state.data_dir.join("reports/truehd-core-repair");
    tokio::fs::create_dir_all(&reports).await?;
    let path = reports.join(format!(
        "truehd-core-repair-{}.md",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    let mut lines = vec![
        "# 高清音轨兼容 Core 纠正报告".to_string(),
        String::new(),
        string(&result, "message"),
    ];
    for (heading, key) in [
        ("已纠正", "repaired_items"),
        ("歧义待复核", "ambiguous_items"),
        ("跳过", "skipped_items"),
        ("失败", "failed_items"),
    ] {
        lines.extend([
            String::new(),
            format!("## {heading}"),
            String::new(),
            "| Item ID | 依据或原因 |".into(),
            "| --- | --- |".into(),
        ]);
        for item in list(&result[key]) {
            lines.push(format!(
                "| {} | {} |",
                string(&item, "item_id"),
                item["reason"]
                    .as_str()
                    .or_else(|| item["evidence"].as_str())
                    .unwrap_or("")
                    .replace('|', "\\|")
                    .replace('\n', " ")
            ));
        }
    }
    tokio::fs::write(&path, format!("{}\n", lines.join("\n"))).await?;
    result["report_path"] = json!(path);
    Ok(result)
}
async fn count_files(root: &std::path::Path, markdown: bool, context: &JobContext) -> Result<u64> {
    let mut count = 0;
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        context.checkpoint().await?;
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                directories.push(entry.path());
            } else if !tokio::fs::metadata(entry.path())
                .await
                .is_ok_and(|m| m.is_dir())
                && (!markdown
                    || entry
                        .file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .ends_with(".md"))
            {
                count += 1;
            }
        }
    }
    Ok(count)
}
pub async fn clear(state: &AppState, context: &JobContext) -> Result<Value> {
    context.checkpoint().await?;
    let root = backup_root(state).await?;
    let reports = state.data_dir.join("reports/truehd-core-repair");
    if tokio::fs::symlink_metadata(&reports)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(ApiError::bad("报告目录不能为符号链接"));
    }
    let data_root = tokio::fs::canonicalize(&state.data_dir).await?;
    for path in [state.data_dir.join("reports"), reports.clone()] {
        if tokio::fs::try_exists(&path).await?
            && !tokio::fs::canonicalize(&path)
                .await?
                .starts_with(&data_root)
        {
            return Err(ApiError::bad("音轨报告目录越界"));
        }
    }
    let deleted_files = count_files(&root, false, context).await?;
    let deleted_reports = if reports.is_dir() {
        count_files(&reports, true, context).await?
    } else {
        0
    };
    context.checkpoint().await?;
    tokio::fs::remove_dir_all(root).await?;
    if reports.is_dir() {
        tokio::fs::remove_dir_all(reports).await?;
    }
    let message = if deleted_files == 0 && deleted_reports == 0 {
        "没有可清理的音轨纠正备份或报告。".to_string()
    } else {
        format!(
            "音轨纠正备份和报告已清理，共删除 {deleted_files} 个备份文件、{deleted_reports} 个报告。"
        )
    };
    tracing::info!("[音轨备份] {message}");
    context.update(100, "清理完成", &message).await;
    Ok(json!({"message":message,"deleted_files":deleted_files,"deleted_reports":deleted_reports}))
}
pub async fn restore(state: &AppState, context: &JobContext) -> Result<Value> {
    context
        .activity("读取音轨备份", "正在检查音轨纠正任务生成的备份")
        .await;
    let backups = state.data_dir.join("backups/truehd-core-shift");
    if tokio::fs::symlink_metadata(&backups)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(ApiError::bad("音轨备份目录不能为符号链接"));
    }
    let empty = |backup_files: usize, skipped: usize| json!({"backup_files":backup_files,"restored":0,"already_restored":0,"skipped":skipped,"failed":0});
    if !backups.is_dir() {
        context
            .update(100, "无需还原", "没有可还原的音轨纠正备份。")
            .await;
        return Ok(empty(0, 0));
    }
    let backups = tokio::fs::canonicalize(backups).await?;
    if !backups.starts_with(tokio::fs::canonicalize(&state.data_dir).await?) {
        return Err(ApiError::bad("音轨备份目录越界"));
    }
    let batch_pattern = regex::Regex::new(r"^\d{8}-\d{6}$").unwrap();
    let file_pattern = regex::Regex::new(r"^(.+)-([0-9a-f]{12})\.json$").unwrap();
    let mut selected = BTreeMap::<(String, String), (PathBuf, Value)>::new();
    let mut pending = vec![backups.clone()];
    let mut backup_files = 0;
    let mut skipped = 0;
    while let Some(directory) = pending.pop() {
        let mut files = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = files.next_entry().await? {
            let path = entry.path();
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_symlink() && path.is_dir() {
                skipped += 1;
                continue;
            }
            if !name.to_lowercase().ends_with(".json") {
                continue;
            }
            backup_files += 1;
            let relative = path.strip_prefix(&backups).unwrap();
            let parts = relative.components().collect::<Vec<_>>();
            let Some(captures) = file_pattern.captures(&name) else {
                skipped += 1;
                continue;
            };
            if !kind.is_file()
                || parts.len() != 2
                || !batch_pattern.is_match(&parts[0].as_os_str().to_string_lossy())
            {
                skipped += 1;
                continue;
            }
            let info = match tokio::fs::read(&path)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            {
                Some(info)
                    if crate::media_info::container(&info).is_some_and(|c| {
                        list(&c["MediaStreams"]).iter().any(|s| {
                            matches!(s["Type"].as_str(), Some("Video" | "Audio" | "Subtitle"))
                        })
                    }) =>
                {
                    info
                }
                _ => {
                    skipped += 1;
                    continue;
                }
            };
            let key = (captures[1].to_string(), captures[2].to_string());
            if selected
                .get(&key)
                .is_none_or(|(previous, _)| previous < &path)
            {
                selected.insert(key, (path, info));
            }
        }
    }
    if selected.is_empty() {
        context
            .update(100, "无需还原", "没有有效的音轨纠正备份，无需还原。")
            .await;
        return Ok(empty(backup_files, skipped));
    }
    let root =
        tokio::fs::canonicalize(string(&*state.config.read().await, "local_strm_root")).await?;
    if !root.is_dir() || protected(&root) {
        return Err(ApiError::bad("媒体根目录不存在或位于115源目录"));
    }
    let seed_ids = selected
        .keys()
        .map(|(id, _)| id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut processed = HashSet::new();
    let mut updated = 0;
    let mut unchanged = 0;
    let mut failed = 0;
    let mut failures = vec![];
    for (index, id) in seed_ids.iter().enumerate() {
        if processed.contains(id) {
            continue;
        }
        context.checkpoint().await?;
        let snapshots = match restore_snapshots(state, id, &root).await {
            Ok(snapshots) => snapshots,
            Err(error) => {
                processed.insert(id.clone());
                skipped += 1;
                tracing::warn!("[音轨备份还原] 跳过 Item {id}：{}", error.1);
                continue;
            }
        };
        if crate::emby::path_ignored(state, &snapshots[0].1).await? {
            continue;
        }
        let backed_members = snapshots
            .iter()
            .filter(|(member, _, _, _)| selected.keys().any(|(id, _)| id == member))
            .count();
        processed.extend(snapshots.iter().map(|snapshot| snapshot.0.clone()));
        let mut members = vec![];
        let mut complete = true;
        for (member, item_path, path, original) in snapshots {
            let key = (member.clone(), path_hash(path.to_str().unwrap()));
            let Some((_, info)) = selected.get(&key) else {
                complete = false;
                break;
            };
            members.push((member, item_path, path, original, info.clone()));
        }
        if !complete {
            skipped += backed_members;
            tracing::warn!("[音轨备份还原] 跳过 Item {id}：关联版本没有匹配当前路径的备份");
            continue;
        }
        let same = members
            .iter()
            .filter(|(_, _, _, old, backup)| old == backup)
            .count();
        if same == members.len() {
            unchanged += same;
            continue;
        }
        let paths = members
            .iter()
            .map(|(_, path, _, _, _)| path.clone())
            .collect::<Vec<_>>();
        let mut stages = BTreeMap::new();
        let mut rollbacks = BTreeMap::new();
        let mut clear_attempted = false;
        let mut rollback_failed = false;
        let result = async {
            let outcome = async {
                for (member, item_path, path, original, backup) in &members {
                    let current = media_edit::details(state, member, "Path,Type,Chapters").await?;
                    if string(&current, "Path") != *item_path {
                        return Err(ApiError::bad("Emby路径在准备还原期间变化，未清理Emby数据"));
                    }
                    let empty = json!([]);
                    let current_chapters = crate::media_info::clean_chapters(
                        if current["Type"] == "Audio" && current["Chapters"].is_null() {
                            &empty
                        } else {
                            &current["Chapters"]
                        },
                    )?;
                    let mut backup = backup.clone();
                    let mut original = original.clone();
                    crate::media_info::preserve_chapters(&mut backup, &current_chapters)?;
                    crate::media_info::preserve_chapters(&mut original, &current_chapters)?;
                    stages.insert(member.clone(), restore_stage(path, &backup).await?);
                    rollbacks.insert(member.clone(), restore_stage(path, &original).await?);
                }
                context.checkpoint().await?;
                clear_attempted = true;
                let config = state.config.read().await.clone();
                crate::emby::send(
                    state
                        .http
                        .post(format!(
                            "{}/Items/{id}/ClearMediaInfo",
                            string(&config, "emby_server_url").trim_end_matches('/')
                        ))
                        .query(&[("api_key", string(&config, "emby_api_key"))])
                        .timeout(std::time::Duration::from_secs(
                            config["emby_api_timeout"]
                                .as_u64()
                                .filter(|seconds| *seconds > 0)
                                .unwrap_or(60),
                        )),
                )
                .await?
                .error_for_status()?;
                for (member, _, path, _, _) in &members {
                    tokio::fs::rename(stages.remove(member).unwrap(), path).await?;
                }
                crate::strm::with_optional_preserve(state, || {
                    crate::media_save::refresh(state, &paths)
                })
                .await?;
                Ok::<_, ApiError>(())
            }
            .await;
            if outcome.is_err() && clear_attempted {
                let rollback = async {
                    for (member, _, path, _, _) in &members {
                        tokio::fs::rename(rollbacks.remove(member).unwrap(), path).await?;
                    }
                    crate::strm::with_optional_preserve(state, || {
                        crate::media_save::refresh(state, &paths)
                    })
                    .await
                }
                .await;
                if let Err(error) = rollback {
                    rollback_failed = true;
                    tracing::error!("[音轨备份还原] Item {id} 无法恢复任务前状态：{}", error.1);
                }
            }
            outcome
        }
        .await;
        for path in stages.values() {
            let _ = tokio::fs::remove_file(path).await;
        }
        if !rollback_failed {
            for path in rollbacks.values() {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        match result {
            Ok(()) => {
                updated += members.len() - same;
                unchanged += same;
                tracing::info!(
                    "[音轨备份还原] 已还原 Item {id} 的 {} 个版本，备份继续保留",
                    members.len()
                );
            }
            Err(error) => {
                failed += members.len();
                failures.push(format!("{id}: {}", error.1));
                tracing::error!("[音轨备份还原] Item {id} 还原失败：{}", error.1);
            }
        }
        context
            .update_counted(
                index as u64 + 1,
                seed_ids.len() as u64,
                "还原音轨备份",
                &format!("已还原 {updated}，无需还原 {unchanged}，跳过 {skipped}，失败 {failed}"),
            )
            .await;
    }
    let message = format!(
        "检查 {backup_files} 个备份，还原 {updated} 个，无需还原 {unchanged} 个，跳过 {skipped} 个，失败 {failed} 个。"
    );
    context.update(100, "还原音轨备份", &message).await;
    Ok(
        json!({"backup_files":backup_files,"restored":updated,"already_restored":unchanged,"skipped":skipped,"failed":failed,"failures":failures,"message":message}),
    )
}
async fn restore_snapshots(
    state: &AppState,
    id: &str,
    root: &std::path::Path,
) -> Result<Vec<(String, String, PathBuf, Value)>> {
    let details =
        media_edit::details(state, id, "Path,PartCount,MediaSources,ExtraType,Name").await?;
    let sources = list(&details["MediaSources"]);
    let mut ids = vec![id.to_string()];
    for source in &sources {
        let source_id = string(source, "ItemId");
        let source_id = if source_id.is_empty() {
            let raw = string(source, "Id");
            if raw.to_lowercase().starts_with("mediasource_") {
                raw.split_once('_').unwrap().1.trim().to_string()
            } else {
                String::new()
            }
        } else {
            source_id
        };
        if source_id.is_empty() && sources.len() > 1 {
            return Err(ApiError::bad("关联版本缺少 ItemId"));
        }
        if !source_id.is_empty() && !ids.contains(&source_id) {
            ids.push(source_id);
        }
    }
    let mut snapshots = vec![];
    for member in ids {
        let item = if member == id {
            details.clone()
        } else {
            media_edit::details(state, &member, "Path,PartCount,ExtraType").await?
        };
        let item_path = string(&item, "Path");
        if item_path.is_empty()
            || !string(&item, "ExtraType").is_empty()
            || item["PartCount"].as_u64().unwrap_or(0) > 1
        {
            return Err(ApiError::bad("关联版本缺少原生路径，或属于分段/附加内容"));
        }
        let mut existing = None;
        for path in
            crate::media_paths::info_candidates(&item_path, root.to_str().unwrap(), "", "").await
        {
            if let Ok(path) = tokio::fs::canonicalize(path).await
                && path.starts_with(root)
                && !protected(&path)
                && path.is_file()
            {
                existing = Some(path);
                break;
            }
        }
        let path = existing.ok_or_else(|| ApiError::bad("关联版本的本地媒体信息 JSON 不存在"))?;
        if snapshots
            .iter()
            .any(|(_, _, previous, _)| previous == &path)
        {
            return Err(ApiError::bad("关联版本共用媒体信息 JSON"));
        }
        let info = serde_json::from_slice(&tokio::fs::read(&path).await?)
            .map_err(|_| ApiError::bad("关联版本媒体信息 JSON 无法读取"))?;
        snapshots.push((member, item_path, path, info));
    }
    Ok(snapshots)
}
async fn restore_stage(path: &std::path::Path, info: &Value) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;
    let stage = path.with_file_name(format!(
        ".{}.truehd-core-stage-{}",
        path.file_name().unwrap().to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)
        .await?;
    file.write_all(
        &serde_json::to_vec_pretty(info).map_err(|_| ApiError::bad("媒体信息 JSON 无效"))?,
    )
    .await?;
    file.sync_all().await?;
    Ok(stage)
}
