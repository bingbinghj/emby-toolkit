//! Episode tail matching follows ETK's Chromaprint detector; work uses the
//! same main/secondary limits and single-main cooldown as Strm Assistant.
use crate::{
    AppState,
    credits_matching::{self, Fingerprint},
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{self, Target, details, list},
    media_info,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};

const FIELDS: &str = "Id,Type,Path,Size,RunTimeTicks,MediaSources,MediaStreams,Chapters,SeriesId,SeasonId,ParentId,ParentIndexNumber,IndexNumber,SeriesName,Name";
const SAMPLE_STEPS: [u32; 2] = [180, 300];
const CACHE_VERSION: u32 = 1;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/media/{id}/extract_credits", post(submit))
}

async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let item = details(&state, &id, FIELDS).await?;
    let action = scope_action(&item, &id)?;
    let task = state
        .jobs
        .clone()
        .submit(action, move |context| async move {
            let live = details(&state, &id, FIELDS).await?;
            if scope_action(&live, &id)? != action {
                return Err(ApiError::bad("媒体类型在任务开始前变化，未提取片尾"));
            }
            if live["Type"] == "Episode" {
                return extract_items(&state, vec![live], &context).await;
            }
            context
                .activity("提取片尾", "正在读取所选范围的电视剧分集")
                .await;
            let mut items = crate::emby::query_items(
                &state,
                &[
                    ("ParentId", id.clone()),
                    ("Recursive", "true".into()),
                    ("IncludeItemTypes", "Episode".into()),
                    ("Fields", FIELDS.into()),
                ],
                &context,
            )
            .await?;
            if items
                .iter()
                .any(|candidate| !in_scope(candidate, &live, &id))
            {
                return Err(ApiError::bad("Emby 返回了所选剧或季以外的媒体，未提取片尾"));
            }
            if items.is_empty() {
                return Err(ApiError::bad("所选剧或季没有可提取片尾的分集"));
            }
            crate::media_order::sort(&mut items);
            extract_items(&state, items, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"片尾提取任务已提交"})),
    ))
}

fn scope_action(item: &Value, id: &str) -> Result<&'static str> {
    if item["Id"] == id {
        match item["Type"].as_str() {
            Some("Series") => return Ok("提取当前剧片尾"),
            Some("Season") => return Ok("提取当前季片尾"),
            Some("Episode") => return Ok("提取当前集片尾"),
            _ => {}
        }
    }
    Err(ApiError::bad(
        "片尾提取仅支持已确认的电视剧、季或分集，电影不提取",
    ))
}

fn in_scope(item: &Value, root: &Value, id: &str) -> bool {
    item["Type"] == "Episode"
        && match root["Type"].as_str() {
            Some("Series") => item["SeriesId"] == id,
            Some("Season") => {
                item["SeasonId"] == id
                    || (string(item, "SeasonId").is_empty() && item["ParentId"] == id)
            }
            _ => false,
        }
}

fn ensure_episode(item: &Value, id: &str) -> Result<()> {
    if item["Id"] != id || item["Type"] != "Episode" {
        return Err(ApiError::bad(
            "片尾提取仅支持已确认的电视剧分集，电影不提取",
        ));
    }
    Ok(())
}

struct Episode {
    item: Value,
    target: Target,
    info: Value,
    sha: String,
    pc: String,
    runtime: f64,
    label: String,
}

async fn episode(state: &AppState, item: Value) -> Result<Episode> {
    let id = string(&item, "Id");
    ensure_episode(&item, &id)?;
    if crate::emby::path_ignored(state, &string(&item, "Path")).await? {
        return Err(ApiError::bad("分集不在媒体库处理范围内"));
    }
    let target = media_edit::target(state, &id).await?;
    let info = media_edit::load(state, &target).await?;
    if target.path != string(&item, "Path") {
        return Err(ApiError::bad(
            "媒体路径与当前 Emby 分集不一致，请先同步媒体数据",
        ));
    }
    let (_, _, size) = crate::revalidation::observation(state, &item).await;
    crate::identity::verify(&json!({"size_bytes":size}), &info)?;
    let sha = target.sha.as_deref().unwrap_or("").trim().to_uppercase();
    if sha.len() != 40 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad("分集缺少可验证 SHA1，请先补齐缓存指纹"));
    }
    let pc = target
        .pc
        .as_deref()
        .filter(|pc| !pc.is_empty())
        .ok_or_else(|| ApiError::bad("分集缺少 115 PC，请先补齐缓存指纹"))?
        .to_string();
    let cached_runtime = media_info::container(&info)
        .and_then(|source| source["RunTimeTicks"].as_f64())
        .unwrap_or(0.0)
        / 10_000_000.0;
    let runtime = native_runtime(&item)
        .ok_or_else(|| ApiError::bad("Emby 未返回当前分集的有效时长，未提取片尾"))?;
    if !cached_runtime.is_finite() || cached_runtime < 18.0 {
        return Err(ApiError::bad("分集缺少有效时长，未提取片尾"));
    }
    if (cached_runtime - runtime).abs() > 1.0 {
        return Err(ApiError::bad(
            "媒体信息 JSON 时长与当前 Emby 分集时长不一致，请先校验媒体信息，未提取片尾",
        ));
    }
    let label = format!(
        "{} S{:02}E{:02}",
        string(&item, "SeriesName"),
        item["ParentIndexNumber"].as_u64().unwrap_or(0),
        item["IndexNumber"].as_u64().unwrap_or(0)
    );
    Ok(Episode {
        item,
        target,
        info,
        sha,
        pc,
        runtime,
        label,
    })
}

fn season_key(item: &Value) -> Result<String> {
    let parent = string(item, "SeasonId");
    if !parent.is_empty() {
        return Ok(parent);
    }
    let series = string(item, "SeriesId");
    let season = item["ParentIndexNumber"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("无法确定分集所属季，未扩大提取范围"))?;
    if series.is_empty() {
        return Err(ApiError::bad("无法确定分集所属电视剧"));
    }
    Ok(format!("{series}:{season}"))
}

async fn season_items(state: &AppState, item: &Value, context: &JobContext) -> Result<Vec<Value>> {
    let season = string(item, "SeasonId");
    let parent = if season.is_empty() {
        string(item, "SeriesId")
    } else {
        season
    };
    if parent.is_empty() {
        return Err(ApiError::bad("无法确定同季分集范围"));
    }
    let key = season_key(item)?;
    let items = crate::emby::query_items(
        state,
        &[
            ("ParentId", parent),
            ("Recursive", "true".into()),
            ("IncludeItemTypes", "Episode".into()),
            ("Fields", FIELDS.into()),
        ],
        context,
    )
    .await?;
    Ok(items
        .into_iter()
        .filter(|candidate| {
            candidate["Type"] == "Episode"
                && season_key(candidate).is_ok_and(|candidate| candidate == key)
        })
        .collect())
}

pub async fn extract_all(state: &AppState, context: &JobContext) -> Result<Value> {
    context
        .activity("提取片尾", "正在扫描电视剧分集，电影不参与片尾提取")
        .await;
    let mut items = crate::emby::query_items(
        state,
        &[
            ("Recursive", "true".into()),
            ("IncludeItemTypes", "Episode".into()),
            ("Fields", FIELDS.into()),
        ],
        context,
    )
    .await?;
    crate::media_order::sort(&mut items);
    extract_items(state, items, context).await
}

pub async fn extract_ids(state: &AppState, ids: &[String], context: &JobContext) -> Result<Value> {
    let mut seen = HashSet::new();
    let mut items = vec![];
    for id in ids {
        context.checkpoint().await?;
        if seen.insert(id.clone()) {
            let item = details(state, id, FIELDS).await?;
            ensure_episode(&item, id)?;
            items.push(item);
        }
    }
    extract_items(state, items, context).await
}

async fn extract_items(state: &AppState, items: Vec<Value>, context: &JobContext) -> Result<Value> {
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut reports = vec![];
    for item in items {
        context.checkpoint().await?;
        let id = string(&item, "Id");
        if item["Type"] != "Episode"
            || crate::emby::path_ignored(state, &string(&item, "Path")).await?
        {
            reports.push(json!({"item_id":id,"status":"skipped","reason":"不是电视剧分集或媒体库不在处理范围内"}));
            continue;
        }
        match season_key(&item) {
            Ok(key) => groups.entry(key).or_default().push(item),
            Err(error) => reports.push(json!({"item_id":id,"status":"failed","error":error.1})),
        }
    }
    let total = groups.values().map(Vec::len).sum::<usize>() + reports.len();
    for selected in groups.into_values() {
        context.checkpoint().await?;
        let mut siblings = match season_items(state, &selected[0], context).await {
            Ok(items) => items,
            Err(error) if context.is_cancelled() => return Err(error),
            Err(error) => {
                for item in selected {
                    reports.push(json!({"item_id":item["Id"],"status":"failed","error":format!("同季分集读取未完成：{}",error.1)}));
                }
                continue;
            }
        };
        crate::media_order::sort(&mut siblings);
        let mut refs = vec![];
        let mut seen = HashSet::new();
        let mut selected_ids: HashSet<_> = selected.iter().map(|item| string(item, "Id")).collect();
        // Prioritize requested episodes, then other unique files from the same
        // season. Two versions of one file cannot prove a common credits song.
        for item in selected.iter().cloned().chain(siblings) {
            let id = string(&item, "Id");
            if !seen.insert(id.clone()) {
                continue;
            }
            match episode(state, item).await {
                Ok(reference) => refs.push(reference),
                Err(error) if selected_ids.contains(&id) => {
                    reports.push(json!({"item_id":id,"status":"failed","error":error.1}))
                }
                Err(error) => tracing::debug!(item_id=%id,"[片尾提取] 同季模板不可用：{}",error.1),
            }
        }
        for reference in refs
            .iter()
            .filter(|reference| selected_ids.contains(&reference.target.id))
        {
            if let Some(ticks) = credits_start(&reference.item["Chapters"])
                .filter(|ticks| safe_start(reference.runtime, *ticks))
            {
                let outcome = async {
                    guard_recovery(state, &reference.target.id).await?;
                    cache_chapters(state, reference, &reference.item["Chapters"]).await
                }
                .await;
                reports.push(match outcome {
                    Ok(())=>json!({"item_id":reference.target.id,"status":"skipped","reason":"已有有效片尾标记，已核对并同步缓存","credits_start_ticks":ticks,"chapter":marker(ticks),"chapters":reference.item["Chapters"]}),
                    Err(error)=>json!({"item_id":reference.target.id,"status":"failed","error":error.1}),
                });
            }
        }
        for report in &reports {
            selected_ids.remove(&string(report, "item_id"));
        }
        if selected_ids.is_empty() {
            continue;
        }
        let mut fingerprints: HashMap<(String, u32), Fingerprint> = HashMap::new();
        let template_indices = select_templates(&refs);
        let mut template = None;
        let mut template_errors = vec![];
        if template_indices.len() >= 2 {
            for seconds in SAMPLE_STEPS {
                let mut samples = vec![];
                for index in &template_indices {
                    match fingerprint(state, &refs[*index], seconds, context).await {
                        Ok(value) => {
                            fingerprints.insert((refs[*index].sha.clone(), seconds), value.clone());
                            samples.push(value);
                        }
                        Err(error) if context.is_cancelled() => return Err(error),
                        Err(error) => {
                            template_errors.push(format!("{}：{}", refs[*index].label, error.1))
                        }
                    }
                }
                if samples.len() < 2 {
                    continue;
                }
                let _secondary = crate::strm::slot(state, "secondary", Some(context)).await?;
                let comparison = samples.clone();
                let candidate = tokio::task::spawn_blocking(move || {
                    credits_matching::detect_common(&comparison)
                })
                .await
                .map_err(|_| ApiError::bad("片尾声纹匹配任务异常"))?;
                drop(_secondary);
                if let Some((index, start, end)) = candidate {
                    if start - samples[index].start_seconds > 8.0 {
                        template = Some((samples[index].clone(), start, end));
                        break;
                    }
                    tracing::info!("[片尾提取] 起点接近 {seconds} 秒采样边界，扩大窗口后核对");
                }
            }
        }
        for reference in refs
            .iter()
            .filter(|reference| selected_ids.contains(&reference.target.id))
        {
            context.checkpoint().await?;
            context
                .update_counted(
                    reports.len() as u64,
                    total as u64,
                    "提取片尾",
                    &format!("正在核对 {} 的片尾", reference.label),
                )
                .await;
            let outcome = async {
                guard_recovery(state,&reference.target.id).await?;
                let live=details(state,&reference.target.id,FIELDS).await?;
                if let Some(ticks)=credits_start(&live["Chapters"]).filter(|ticks|safe_start(reference.runtime,*ticks)) {
                    cache_chapters(state,reference,&live["Chapters"]).await?;
                    return Ok::<Value,ApiError>(json!({"item_id":reference.target.id,"status":"skipped","reason":"已有有效片尾标记，已核对并同步缓存","credits_start_ticks":ticks,"chapter":marker(ticks),"chapters":live["Chapters"]}));
                }
                let Some((base,start,end))=&template else {
                    let reason=if template_indices.len()<2 {"同季可比对分集不足 2 集，未猜测片尾".into()} else if !template_errors.is_empty() {format!("同季片尾指纹提取未完成：{}",template_errors.join("；"))} else {"未找到稳定公共片尾或起点仍贴近 300 秒窗口边界，未写入标记".into()};
                    return Ok::<Value,ApiError>(json!({"item_id":reference.target.id,"status":if template_indices.len()<2 {"skipped"} else {"failed"},"error":reason,"reason":reason}));
                };
                let mut own_start=None;
                for seconds in SAMPLE_STEPS {
                    let sample=if let Some(sample)=fingerprints.get(&(reference.sha.clone(),seconds)) {sample.clone()} else {
                        let sample=fingerprint(state,reference,seconds,context).await?;
                        fingerprints.insert((reference.sha.clone(),seconds),sample.clone());sample
                    };
                    let _secondary=crate::strm::slot(state,"secondary",Some(context)).await?;
                    let (own,template)= (sample.clone(),base.clone());
                    let (start,end)=(*start,*end);
                    let matched=tokio::task::spawn_blocking(move||credits_matching::match_template(&own,&template,start,end)).await.map_err(|_|ApiError::bad("分集片尾声纹匹配任务异常"))?;
                    drop(_secondary);
                    if let Some((start,_))=matched && start-sample.start_seconds>8.0 {
                        let ticks=(start*10_000_000.0).round() as u64;
                        if safe_start(reference.runtime,ticks) {own_start=Some(ticks);break;}
                    }
                }
                let ticks=own_start.ok_or_else(||ApiError::bad("当前分集没有通过独立片尾声纹匹配，未写入标记"))?;
                let chapters=apply_marker(state,reference,ticks,context).await?;
                cache_chapters(state,reference,&chapters).await?;
                let actual_ticks=credits_start(&chapters).ok_or_else(||ApiError::bad("片尾标记回读缺失"))?;
                Ok::<Value,ApiError>(json!({"item_id":reference.target.id,"status":"completed","credits_start_ticks":actual_ticks,"chapter":marker(actual_ticks),"chapters":chapters}))
            }.await;
            match outcome {
                Ok(report) => {
                    tracing::info!(
                        "[片尾提取] {}：{}",
                        reference.label,
                        string(&report, "status")
                    );
                    reports.push(report);
                }
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => {
                    tracing::warn!("[片尾提取] {} 失败：{}", reference.label, error.1);
                    reports.push(
                        json!({"item_id":reference.target.id,"status":"failed","error":error.1}),
                    );
                }
            }
        }
    }
    let updated = reports
        .iter()
        .filter(|item| item["status"] == "completed")
        .count();
    let skipped = reports
        .iter()
        .filter(|item| item["status"] == "skipped")
        .count();
    let failures = reports
        .iter()
        .filter(|item| item["status"] == "failed")
        .map(|item| format!("{}：{}", string(item, "item_id"), string(item, "error")))
        .collect::<Vec<_>>();
    let message = format!(
        "片尾提取完成：更新 {updated} 集，跳过 {skipped} 集，失败 {} 集",
        failures.len()
    );
    let warning_details = reports
        .iter()
        .filter(|item| {
            item["status"] == "skipped" && item["chapter"].is_null() && !item["error"].is_null()
        })
        .map(|item| format!("{}：{}", string(item, "item_id"), string(item, "reason")))
        .collect::<Vec<_>>();
    context
        .update_counted(total as u64, total as u64, "提取片尾", &message)
        .await;
    let mut result = json!({"state":if failures.is_empty(){"completed"}else{"failed"},"native_ended":true,"updated":updated,"skipped":skipped,"failed":failures.len(),"failures":failures,"warnings":warning_details.len(),"warning_details":warning_details,"items":reports,"message":message,"reason":message});
    if reports.len() == 1 {
        for key in ["chapter", "chapters", "credits_start_ticks"] {
            result[key] = reports[0][key].clone();
        }
    }
    Ok(result)
}

fn select_templates(refs: &[Episode]) -> Vec<usize> {
    let mut hashes = HashSet::new();
    let mut numbers = HashSet::new();
    refs.iter()
        .enumerate()
        .filter(|(_, reference)| {
            hashes.insert(reference.sha.clone())
                && numbers.insert(reference.item["IndexNumber"].as_u64())
        })
        .map(|(index, _)| index)
        .take(3)
        .collect()
}

async fn guard_recovery(state: &AppState, id: &str) -> Result<()> {
    let Json(status) = crate::media_recovery::status(State(state.clone())).await?;
    if list(&status["records"]).iter().any(|record| {
        record["target_ids"]
            .as_array()
            .is_none_or(|ids| ids.is_empty() || ids.iter().any(|target| target == id))
    }) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "当前媒体信息仍待恢复，完成恢复验收与备份清理后才能提取片尾".into(),
        ));
    }
    Ok(())
}

async fn fingerprint(
    state: &AppState,
    reference: &Episode,
    requested: u32,
    context: &JobContext,
) -> Result<Fingerprint> {
    context.checkpoint().await?;
    let seconds = (requested as f64).min(reference.runtime.floor());
    let start = (reference.runtime - seconds).max(0.0);
    let directory = state.data_dir.join("credits-fingerprints");
    let path = directory.join(format!("{}-{requested}.json", reference.sha));
    if let Ok(bytes) = tokio::fs::read(&path).await
        && let Ok(cache) = serde_json::from_slice::<Value>(&bytes)
        && cache["version"] == CACHE_VERSION
        && cache["sha1"] == reference.sha
        && cache["runtime"] == reference.runtime
        && cache["seconds"] == seconds
        && cache["start"] == start
        && let Ok(values) = serde_json::from_value::<Vec<u32>>(cache["values"].clone())
        && values.len() >= 40
        && values.len() <= 100_000
    {
        return Ok(Fingerprint {
            values,
            sample_seconds: seconds,
            start_seconds: start,
        });
    }
    guard_recovery(state, &reference.target.id).await?;
    let _main = crate::strm::slot(state, "main", Some(context)).await?;
    let url = crate::p115::download_url(state, &reference.pc).await?;
    let args = chromaprint_arguments(&url, start, seconds, audio_stream(&reference.info));
    let mut command = tokio::process::Command::new("ffmpeg");
    command.kill_on_drop(true).args(args);
    tracing::info!(
        "[片尾提取] 提取 {} 尾部 {seconds} 秒音频声纹",
        reference.label
    );
    let output = tokio::select! {
        result=tokio::time::timeout(Duration::from_secs(900),command.output())=>result.map_err(|_|ApiError::bad("片尾音频声纹提取超时"))??,
        _=context.cancelled()=>return Err(ApiError(StatusCode::CONFLICT,"片尾提取已取消".into())),
    };
    drop(_main);
    if !output.status.success() {
        return Err(ApiError::bad(
            "ffmpeg 未成功提取片尾声纹，请检查 115 直链、音轨及 chromaprint 支持",
        ));
    }
    let values = parse_raw(&output.stdout)?;
    context.checkpoint().await?;
    tokio::fs::create_dir_all(&directory).await?;
    let stage = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let write=async {
        use tokio::io::AsyncWriteExt;
        let mut file=tokio::fs::OpenOptions::new().write(true).create_new(true).open(&stage).await?;
        file.write_all(&serde_json::to_vec(&json!({"version":CACHE_VERSION,"sha1":reference.sha,"runtime":reference.runtime,"seconds":seconds,"start":start,"values":values})).unwrap()).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&stage,&path).await?;
        Ok::<_,ApiError>(())
    }.await;
    let _ = tokio::fs::remove_file(&stage).await;
    write?;
    Ok(Fingerprint {
        values,
        sample_seconds: seconds,
        start_seconds: start,
    })
}

fn parse_raw(output: &[u8]) -> Result<Vec<u32>> {
    if output.len() < 160 || output.len() > 400_000 || !output.len().is_multiple_of(4) {
        return Err(ApiError::bad("ffmpeg 未返回有效 raw Chromaprint 声纹"));
    }
    Ok(output
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect())
}

fn audio_stream(info: &Value) -> Option<u64> {
    let source = media_info::container(info)?;
    let streams = source["MediaStreams"].as_array()?;
    let valid = |stream: &&Value| {
        stream["Type"] == "Audio"
            && stream["IsExternal"] != true
            && stream["Index"].as_u64().is_some()
            && stream["Codec"].as_str().is_some_and(|codec| {
                !codec.is_empty()
                    && !matches!(
                        codec.to_ascii_lowercase().as_str(),
                        "unknown" | "none" | "null" | "data"
                    )
                    && !codec.bytes().all(|byte| byte.is_ascii_digit())
            })
    };
    streams
        .iter()
        .filter(valid)
        .find(|stream| stream["IsDefault"] == true)
        .or_else(|| streams.iter().find(valid))
        .and_then(|stream| stream["Index"].as_u64())
}

fn chromaprint_arguments(url: &str, start: f64, seconds: f64, stream: Option<u64>) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-user_agent",
        "Mozilla/5.0",
        "-rw_timeout",
        "15000000",
        "-reconnect",
        "1",
        "-reconnect_streamed",
        "1",
        "-reconnect_delay_max",
        "5",
        "-ss",
        &format!("{start:.3}"),
        "-i",
        url,
        "-map",
        &stream.map_or("0:a:0".into(), |index| format!("0:{index}")),
        "-t",
        &seconds.to_string(),
        "-ac",
        "1",
        "-acodec",
        "pcm_s16le",
        "-ar",
        "16000",
        "-vn",
        "-sn",
        "-dn",
        "-f",
        "chromaprint",
        "-fp_format",
        "raw",
        "-",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub(crate) fn credits_start(chapters: &Value) -> Option<u64> {
    chapters
        .as_array()?
        .iter()
        .find(|chapter| chapter["MarkerType"] == "CreditsStart")
        .and_then(|chapter| chapter["StartPositionTicks"].as_u64())
}

fn safe_start(runtime: f64, ticks: u64) -> bool {
    let tail = runtime - ticks as f64 / 10_000_000.0;
    runtime.is_finite() && ticks > 0 && (18.0..=308.0).contains(&tail)
}

fn native_runtime(item: &Value) -> Option<f64> {
    let ticks = media_info::item_source(item, &[item["Path"].as_str().unwrap_or("")])
        .and_then(|source| source["RunTimeTicks"].as_u64())
        .or_else(|| item["RunTimeTicks"].as_u64());
    ticks
        .filter(|ticks| *ticks > 0)
        .map(|ticks| ticks as f64 / 10_000_000.0)
}

fn marker(ticks: u64) -> Value {
    json!({"StartPositionTicks":ticks,"Name":"片尾","MarkerType":"CreditsStart"})
}

fn chapter_time(ticks: u64) -> String {
    let seconds = ticks / 10_000_000;
    format!(
        "{:02}:{:02}:{:02}.{:07}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        ticks % 10_000_000
    )
}

async fn apply_marker(
    state: &AppState,
    reference: &Episode,
    ticks: u64,
    context: &JobContext,
) -> Result<Value> {
    context.checkpoint().await?;
    guard_recovery(state, &reference.target.id).await?;
    let connection = state.config.read().await.clone();
    let plugins = state.emby(Method::GET, "Plugins", &[], None).await?;
    let plugins = plugins
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 插件列表无效，未写入章节"))?;
    let bridge = plugins.iter().any(|plugin| {
        string(plugin, "Name")
            .replace(' ', "")
            .to_ascii_lowercase()
            .contains("etkmediainfobridge")
    });
    let chapter_api = plugins.iter().any(|plugin| plugin["Name"] == "Chapter API");
    if !bridge && !chapter_api {
        return Err(ApiError::bad(
            "Emby 缺少可安全写入章节的 Chapter API 或 ETKMediaInfoBridge 插件，未清空媒体信息",
        ));
    }
    let config = state.config.read().await.clone();
    let base = string(&config, "emby_server_url");
    let key = string(&config, "emby_api_key");
    let mut request = if bridge {
        state
            .http
            .post(format!(
                "{}/Items/{}/ETKMediaInfo/Credits",
                base.trim_end_matches('/'),
                reference.target.id
            ))
            .json(&json!({"CreditsStartTicks":ticks}))
    } else {
        state
            .http
            .get(format!(
                "{}/chapter_api/update_chapters",
                base.trim_end_matches('/')
            ))
            .query(&[
                ("id", reference.target.id.clone()),
                ("action", "add".into()),
                ("type", "credits_start".into()),
                ("name", "片尾".into()),
                ("time", chapter_time(ticks)),
            ])
    };
    request = request
        .header("X-Emby-Token", key)
        .timeout(Duration::from_secs(
            config["emby_api_timeout"]
                .as_u64()
                .filter(|seconds| *seconds > 0)
                .unwrap_or(60),
        ));
    let _main = crate::strm::slot(state, "main", Some(context)).await?;
    // A main slot can wait for another extraction and its cooldown. Re-read
    // identity, recovery state and the original chapter snapshot after waiting.
    let current_connection = state.config.read().await.clone();
    if current_connection["emby_server_url"] != connection["emby_server_url"]
        || current_connection["emby_api_key"] != connection["emby_api_key"]
        || current_connection["emby_server_url"] != config["emby_server_url"]
        || current_connection["emby_api_key"] != config["emby_api_key"]
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Emby 连接在片尾等待期间变化，未写入标记".into(),
        ));
    }
    guard_recovery(state, &reference.target.id).await?;
    let before = details(state, &reference.target.id, FIELDS).await?;
    ensure_episode(&before, &reference.target.id)?;
    let current = media_edit::target(state, &reference.target.id).await?;
    if current.path != reference.target.path
        || current.sha != reference.target.sha
        || current.asset != reference.target.asset
        || crate::revalidation::observation(state, &before).await
            != crate::revalidation::observation(state, &reference.item).await
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "媒体身份在片尾提取或线程等待期间变化，未写入标记".into(),
        ));
    }
    let runtime = native_runtime(&before)
        .filter(|runtime| (*runtime - reference.runtime).abs() <= 1.0)
        .ok_or_else(|| ApiError::bad("Emby 分集时长在片尾提取或线程等待期间变化，未写入标记"))?;
    if !safe_start(runtime, ticks) {
        return Err(ApiError::bad(
            "片尾时间超出当前 Emby 分集的安全时长范围，未写入标记",
        ));
    }
    if let Some(current) = credits_start(&before["Chapters"]) {
        return if safe_start(runtime, current) {
            Ok(before["Chapters"].clone())
        } else {
            Err(ApiError::bad(
                "已有片尾标记但时间无效，请先检查章节，未自动覆盖",
            ))
        };
    }
    media_info::signature(&before["Chapters"])?;
    // Chapter API's legacy GET mutates state. Never auto-retry it: a lost
    // response may already have added the marker. Always read back first.
    let sent = request.send().await;
    drop(_main);
    let response_error = match sent {
        Ok(response) if response.status().is_success() => None,
        Ok(response) => Some(format!("章节写入返回 HTTP {}", response.status())),
        Err(_) => Some("章节写入响应未确认".into()),
    };
    for _ in 0..10 {
        let actual = details(state, &reference.target.id, FIELDS).await?;
        if credits_start(&actual["Chapters"]) == Some(ticks) {
            verify_readback(&before, &actual, ticks)?;
            return Ok(actual["Chapters"].clone());
        }
        context.sleep(Duration::from_millis(200)).await?;
    }
    Err(ApiError::bad(format!(
        "{}；Emby 未回读到本次片尾标记，未认定提取成功",
        response_error.unwrap_or_else(|| "章节写入未通过验收".into())
    )))
}

fn verify_readback(before: &Value, actual: &Value, ticks: u64) -> Result<()> {
    if actual["Id"] != before["Id"]
        || actual["Type"] != "Episode"
        || actual["Path"] != before["Path"]
        || actual["Size"] != before["Size"]
        || source_signatures(&actual["MediaSources"]) != source_signatures(&before["MediaSources"])
        || actual["MediaStreams"] != before["MediaStreams"]
        || !matches!((native_runtime(before),native_runtime(actual)),(Some(before),Some(actual)) if (before-actual).abs()<=1.0)
    {
        return Err(ApiError::bad(
            "片尾写入后媒体身份或原有媒体流发生变化，请检查 Emby，未完成验收",
        ));
    }
    if credits_start(&actual["Chapters"]) != Some(ticks)
        || !native_runtime(actual).is_some_and(|runtime| safe_start(runtime, ticks))
        || !media_info::chapters_restored(&before["Chapters"], &actual["Chapters"])
    {
        return Err(ApiError::bad("片尾写入后原有章节或新标记未通过回读验收"));
    }
    for chapter in list(&before["Chapters"]) {
        if !string(&chapter, "ImageTag").is_empty()
            && !list(&actual["Chapters"]).iter().any(|live| {
                live["StartPositionTicks"] == chapter["StartPositionTicks"]
                    && live["MarkerType"] == chapter["MarkerType"]
                    && live["ImageTag"] == chapter["ImageTag"]
            })
        {
            return Err(ApiError::bad("片尾写入后原有章节图片标记变化，未完成验收"));
        }
    }
    Ok(())
}

fn source_signatures(sources: &Value) -> Value {
    if !sources.is_array() {
        return sources.clone();
    }
    json!(
        list(sources)
            .iter()
            .map(|source| {
                let mut signature = serde_json::Map::new();
                for key in [
                    "Id",
                    "ItemId",
                    "Path",
                    "Size",
                    "Container",
                    "RunTimeTicks",
                    "MediaStreams",
                ] {
                    if let Some(value) = source.get(key) {
                        signature.insert(key.into(), value.clone());
                    }
                }
                Value::Object(signature)
            })
            .collect::<Vec<_>>()
    )
}

async fn cache_chapters(state: &AppState, reference: &Episode, chapters: &Value) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    let mut current = sqlx::query_as::<_,(String,Option<Value>)>(
        "SELECT sha1,mediainfo_json FROM p115_mediainfo_cache WHERE UPPER(sha1)=UPPER($1) ORDER BY sha1 FOR UPDATE",
    )
    .bind(&reference.sha)
    .fetch_all(&mut *transaction)
    .await?;
    if current.is_empty() {
        current.push((
            reference
                .target
                .sha
                .clone()
                .unwrap_or_else(|| reference.sha.clone()),
            None,
        ));
    }
    // SHA1 is a case-sensitive primary key. Update the actual existing key(s),
    // so older lowercase rows keep receiving the marker and no duplicate is
    // created by the normalized fingerprint-cache key.
    for (key, info) in current {
        let mut info = info.unwrap_or_else(|| reference.info.clone());
        crate::identity::verify(&reference.target.asset, &info)?;
        media_info::preserve_chapters(&mut info, chapters)?;
        sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json").bind(key).bind(info).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn television_scope_requires_matching_identity_and_parent() {
        for kind in ["Series", "Season", "Episode"] {
            assert!(scope_action(&json!({"Id":"1","Type":kind}), "1").is_ok());
            assert!(scope_action(&json!({"Id":"2","Type":kind}), "1").is_err());
        }
        assert!(scope_action(&json!({"Id":"1","Type":"Movie"}), "1").is_err());
        let series = json!({"Type":"Series"});
        let season = json!({"Type":"Season"});
        let episode =
            json!({"Type":"Episode","SeriesId":"series","SeasonId":"season","ParentId":"season"});
        assert!(in_scope(&episode, &series, "series"));
        assert!(in_scope(&episode, &season, "season"));
        assert!(!in_scope(&episode, &series, "other"));
        assert!(!in_scope(&episode, &season, "other"));
        assert!(!in_scope(
            &json!({"Type":"Movie","SeriesId":"series"}),
            &series,
            "series"
        ));
        assert!(in_scope(
            &json!({"Type":"Episode","ParentId":"season"}),
            &season,
            "season"
        ));
        assert!(!in_scope(
            &json!({"Type":"Episode","SeasonId":"other","ParentId":"season"}),
            &season,
            "season"
        ));
    }
    #[test]
    fn movies_and_unconfirmed_ids_never_enter_episode_extraction() {
        assert!(ensure_episode(&json!({"Id":"1","Type":"Movie"}), "1").is_err());
        assert!(ensure_episode(&json!({"Id":"2","Type":"Episode"}), "1").is_err());
        assert!(ensure_episode(&json!({"Id":"1","Type":"Episode"}), "1").is_ok());
    }
    #[test]
    fn tail_runtime_and_window_are_bounded() {
        assert!(safe_start(1200.0, 11_100_000_000));
        assert!(safe_start(1200.0, 9_000_000_000));
        assert!(!safe_start(1200.0, 8_800_000_000));
        assert!(!safe_start(1200.0, 11_999_000_000));
        assert!(!safe_start(f64::NAN, 11_100_000_000));
        assert_eq!(chapter_time(11_100_123_456), "00:18:30.0123456");
    }
    #[test]
    fn chromaprint_raw_is_unsigned_little_endian_and_requires_valid_sample() {
        let raw = [0xff, 0xff, 0xff, 0x80].repeat(40);
        assert_eq!(parse_raw(&raw).unwrap(), vec![0x80ff_ffff; 40]);
        assert!(parse_raw(&raw[..159]).is_err());
        assert!(parse_raw(&vec![0; 400_004]).is_err());
    }
    #[test]
    fn fingerprint_uses_default_internal_audio_and_exact_tail_seek() {
        let info = json!({"MediaStreams":[{"Type":"Audio","Index":1,"Codec":"aac"},{"Type":"Audio","Index":2,"Codec":"ac3","IsDefault":true,"IsExternal":true},{"Type":"Audio","Index":3,"Codec":"aac","IsDefault":true}]});
        assert_eq!(audio_stream(&info), Some(3));
        let args = chromaprint_arguments(
            "https://fixture.invalid/media",
            1020.0,
            180.0,
            audio_stream(&info),
        );
        assert!(args.windows(2).any(|pair| pair == ["-ss", "1020.000"]));
        assert!(args.windows(2).any(|pair| pair == ["-map", "0:3"]));
        assert!(args.windows(2).any(|pair| pair == ["-f", "chromaprint"]));
        assert!(args.windows(2).any(|pair| pair == ["-fp_format", "raw"]));
    }
    #[test]
    fn chapter_append_readback_requires_original_images_and_streams() {
        let before = json!({"Id":"1","Type":"Episode","Path":"/media/a.strm","Size":100,"RunTimeTicks":12_000_000_000_u64,"MediaStreams":[{"Type":"Video","Codec":"h264"}],"Chapters":[{"Name":"章1","MarkerType":"Chapter","StartPositionTicks":0,"ImageTag":"image"}]});
        let mut actual = before.clone();
        actual["Chapters"]
            .as_array_mut()
            .unwrap()
            .push(marker(11_100_000_000));
        assert!(verify_readback(&before, &actual, 11_100_000_000).is_ok());
        actual["Chapters"][0]["ImageTag"] = json!("lost");
        assert!(verify_readback(&before, &actual, 11_100_000_000).is_err());
        actual["Chapters"][0]["ImageTag"] = json!("image");
        actual["MediaStreams"] = json!([]);
        assert!(verify_readback(&before, &actual, 11_100_000_000).is_err());
    }
}
