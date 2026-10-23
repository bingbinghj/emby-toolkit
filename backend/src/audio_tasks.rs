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
                    raw = Some(crate::p115_iso::probe_file(&path, Some(context)).await?);
                    evidence = "CD2 蓝光 CLPI/PID".into();
                } else {
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
                    if let Ok(Ok(output)) =
                        tokio::time::timeout(std::time::Duration::from_secs(45), command.output())
                            .await
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
    for id in ids {
        context.checkpoint().await?;
        let outcome=async{
            let item=media_edit::details(state,id,"Path,MediaSources,MediaStreams,PartCount,ExtraType").await?;
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
            Err(error) => {
                result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                result["failed_items"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"item_id":id,"reason":error.1}));
            }
        }
    }
    result["message"] = json!(format!(
        "高清音轨纠正完成：成功 {}，歧义 {}，失败 {}",
        result["repaired"], result["ambiguous"], result["failed"]
    ));
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
    tokio::fs::remove_dir_all(root).await?;
    if reports.is_dir() {
        tokio::fs::remove_dir_all(reports).await?;
    }
    Ok(json!({"message":"音轨纠正备份和报告已清理"}))
}
pub async fn restore(state: &AppState, context: &JobContext) -> Result<Value> {
    let root =
        tokio::fs::canonicalize(string(&*state.config.read().await, "local_strm_root")).await?;
    if protected(&root) {
        return Err(ApiError::bad("媒体根目录位于115源目录"));
    }
    let backups = backup_root(state).await?;
    let mut selected = BTreeMap::<String, (PathBuf, Value)>::new();
    let mut directories = tokio::fs::read_dir(&backups).await?;
    let batch = regex::Regex::new(r"^\d{8}-\d{6}$").unwrap();
    let file = regex::Regex::new(r"^([A-Za-z0-9_-]+)-([0-9a-f]{12})\.json$").unwrap();
    while let Some(dir) = directories.next_entry().await? {
        if !dir.file_type().await?.is_dir() || !batch.is_match(&dir.file_name().to_string_lossy()) {
            continue;
        }
        let mut files = tokio::fs::read_dir(dir.path()).await?;
        while let Some(entry) = files.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(c) = file.captures(&name) else {
                continue;
            };
            let Ok(info) = serde_json::from_slice::<Value>(&tokio::fs::read(entry.path()).await?)
            else {
                continue;
            };
            if !crate::media_info::recoverable(&info) {
                continue;
            }
            let id = c[1].to_string();
            let Ok(target) = media_edit::repair_target(state, &id).await else {
                continue;
            };
            let Some(path) = target.candidates.iter().find(|p| p.is_file()) else {
                continue;
            };
            let path = tokio::fs::canonicalize(path).await?;
            if !path.starts_with(&root)
                || protected(&path)
                || path_hash(path.to_str().unwrap()) != c[2]
            {
                continue;
            }
            let key = format!("{id}:{}", &c[2]);
            if selected.get(&key).is_none_or(|(p, _)| p < &entry.path()) {
                selected.insert(key, (entry.path(), json!({"id":id,"info":info})));
            }
        }
    }
    let mut updated = 0;
    let mut unchanged = 0;
    let mut skipped = 0;
    let mut processed = HashSet::new();
    let mut failures = vec![];
    for (_, entry) in selected.values() {
        context.checkpoint().await?;
        let id = string(entry, "id");
        if processed.contains(&id) {
            continue;
        }
        let target = media_edit::repair_target(state, &id).await?;
        let mut members = vec![];
        let mut complete = true;
        for version in target
            .versions
            .iter()
            .map(|v| string(v, "emby_id"))
            .chain(std::iter::once(id.clone()))
            .collect::<std::collections::BTreeSet<_>>()
        {
            let target = media_edit::repair_target(state, &version).await?;
            let Some(path) = target.candidates.iter().find(|p| p.is_file()) else {
                complete = false;
                break;
            };
            let path = tokio::fs::canonicalize(path).await?;
            if !path.starts_with(&root) || protected(&path) {
                complete = false;
                break;
            }
            let key = format!("{version}:{}", path_hash(path.to_str().unwrap()));
            let Some((_, backup)) = selected.get(&key) else {
                complete = false;
                break;
            };
            let original = media_edit::load(state, &target).await?;
            members.push((version, path, original, backup["info"].clone()));
        }
        if !complete {
            skipped += 1;
            continue;
        }
        processed.extend(members.iter().map(|(id, _, _, _)| id.clone()));
        if members.iter().all(|(_, _, old, backup)| old == backup) {
            unchanged += members.len();
            continue;
        }
        let rebuilt = members
            .iter()
            .map(|(member, _, _, backup)| (member.clone(), backup.clone()))
            .collect::<BTreeMap<_, _>>();
        let result = crate::strm::with_settings(state, "preserve", || async {
            let outcome = async {
                for (member, path, _, _) in &members {
                    restore_member(state, member, path, &root).await?;
                }
                for (member, path, old, backup) in &members {
                    if old == backup {
                        continue;
                    }
                    let target = restore_member(state, member, path, &root).await?;
                    if crate::media_info::container(old) != crate::media_info::container(backup) {
                        let body =
                            json!({"mediainfo":backup,"sha1":target.sha,"media_path":target.path});
                        let (status, value) = crate::media_save::save_with_rebuilt(
                            state, member, &body, &rebuilt, context,
                        )
                        .await?;
                        if status >= 400 {
                            return Err(ApiError::bad(string(&value, "error")));
                        }
                    }
                    let latest = media_edit::details(state, member, "Chapters").await?;
                    let mut restored = backup.clone();
                    crate::media_info::preserve_chapters(&mut restored, &latest["Chapters"])?;
                    restore_member(state, member, path, &root).await?;
                    tokio::fs::write(path, serde_json::to_vec_pretty(&restored).unwrap()).await?;
                    if let Some(sha) = target.sha {
                        sqlx::query(
                            "UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2",
                        )
                        .bind(restored)
                        .bind(sha)
                        .execute(&state.db)
                        .await?;
                    }
                }
                Ok::<_, ApiError>(())
            }
            .await;
            if outcome.is_err() {
                for (member, path, original, _) in &members {
                    let target = restore_member(state, member, path, &root).await?;
                    tokio::fs::write(path, serde_json::to_vec_pretty(original).unwrap()).await?;
                    if let Some(sha) = target.sha {
                        sqlx::query(
                            "UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2",
                        )
                        .bind(original)
                        .bind(sha)
                        .execute(&state.db)
                        .await?;
                    }
                }
                let _ = crate::media_save::refresh(state, std::slice::from_ref(&target.path)).await;
            }
            outcome
        })
        .await;
        match result {
            Ok(()) => {
                for (_, _, old, backup) in &members {
                    if crate::media_info::container(old) == crate::media_info::container(backup) {
                        unchanged += 1;
                    } else {
                        updated += 1;
                    }
                }
            }
            Err(error) => failures.push(format!("{id}: {}", error.1)),
        }
    }
    Ok(
        json!({"restored":updated,"already_restored":unchanged,"skipped":skipped,"failures":failures,"message":"音轨备份还原完成"}),
    )
}
async fn restore_member(
    state: &AppState,
    id: &str,
    expected: &std::path::Path,
    root: &std::path::Path,
) -> Result<media_edit::Target> {
    let current_root =
        tokio::fs::canonicalize(string(&*state.config.read().await, "local_strm_root")).await?;
    let target = media_edit::repair_target(state, id).await?;
    let path = target
        .candidates
        .iter()
        .find(|path| path.is_file())
        .ok_or_else(|| ApiError::bad("关联版本缺失媒体信息"))?;
    let path = tokio::fs::canonicalize(path).await?;
    if current_root != root
        || protected(&current_root)
        || !path.starts_with(root)
        || protected(&path)
        || path != expected
    {
        return Err(ApiError::bad("关联版本媒体信息路径不可写"));
    }
    Ok(target)
}
