//! Independent local music library. No Emby/115 media input or download routes.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::{JobContext, JobSpec},
};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static ENGINE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

// 独立进程组覆盖 fpcalc；取消、超时及 future 提前退出均释放子进程。
#[cfg(unix)]
struct WorkerGroup(u32);

#[cfg(unix)]
impl WorkerGroup {
    fn signal(&self, signal: i32) {
        // ID 来自 process_group(0) 启动的子进程，只能指向本次任务的进程组。
        unsafe {
            libc::kill(-(self.0 as i32), signal);
        }
    }
}

#[cfg(unix)]
impl Drop for WorkerGroup {
    fn drop(&mut self) {
        self.signal(libc::SIGKILL);
    }
}

fn music_config(config: &Value) -> Value {
    config
        .as_object()
        .map(|fields| {
            Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| {
                        key.starts_with("music_")
                            || matches!(
                                key.as_str(),
                                "acoustid_api_key"
                                    | "theaudiodb_api_key"
                                    | "lrclib_base_url"
                                    | "amll_base_url"
                                    | "lyrics_batch_timeout"
                                    | "lyrics_provider_retry_max_wait"
                            )
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        })
        .unwrap_or(json!({}))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/music/library", get(library))
        .route("/api/music/image", get(image))
        .route("/api/music/audio", get(audio))
        .route("/api/music/query/{action}", post(query))
        .route("/api/music/jobs/{action}", post(submit))
        .layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024))
}

pub fn validate_config(config: &Value) -> Result<()> {
    for key in ["music_library_path", "music_seed_path"] {
        if let Some(value) = config.get(key) {
            let value = value
                .as_str()
                .ok_or_else(|| ApiError::bad("音乐目录必须为本地绝对路径"))?;
            if !value.is_empty()
                && (!std::path::Path::new(value).is_absolute() || value.contains('\0'))
            {
                return Err(ApiError::bad("音乐目录必须为本地绝对路径"));
            }
        }
    }
    if let Some(sources) = config.get("music_sources") {
        let values = sources
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 3)
            .ok_or_else(|| ApiError::bad("请选择 1 至 3 个音乐资料源"))?;
        if values.iter().any(|value| {
            !matches!(
                value.as_str(),
                Some("musicbrainz" | "theaudiodb" | "doubanmusic")
            )
        }) {
            return Err(ApiError::bad("音乐资料源无效"));
        }
    }
    if let Some(policies) = config.get("music_scraping_policies") {
        for field in ["tags", "cover", "lyrics"] {
            if !matches!(
                policies[field].as_str(),
                Some("skip" | "missingOnly" | "overwrite")
            ) && !(field == "lyrics" && policies[field] == "upgrade")
            {
                return Err(ApiError::bad("音乐刮削策略无效"));
            }
        }
    }
    for key in ["lyrics_batch_timeout", "lyrics_provider_retry_max_wait"] {
        if config
            .get(key)
            .is_some_and(|value| value.as_u64().is_none_or(|number| number > 900))
        {
            return Err(ApiError::bad("音乐请求等待时间必须为 0 至 900 秒"));
        }
    }
    for key in ["music_cover_proxy", "lrclib_base_url", "amll_base_url"] {
        if let Some(value) = config.get(key) {
            let text = value
                .as_str()
                .ok_or_else(|| ApiError::bad("音乐来源地址无效"))?;
            if !text.is_empty()
                && reqwest::Url::parse(text).ok().is_none_or(|url| {
                    !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
                })
            {
                return Err(ApiError::bad("音乐来源地址必须是 HTTP 或 HTTPS 地址"));
            }
        }
    }
    Ok(())
}

async fn engine(
    state: &AppState,
    action: &str,
    body: Value,
    config: &Value,
    context: Option<&JobContext>,
) -> Result<Value> {
    let _permit = ENGINE_SLOTS
        .acquire()
        .await
        .map_err(|_| ApiError::bad("音乐引擎已关闭"))?;
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    let directory = state.data_dir.join("music");
    tokio::fs::create_dir_all(directory.join("tmp")).await?;
    let script = std::env::var_os("ETK_MUSIC_ENGINE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("music_engine/worker.py")
        });
    let python = std::env::var("ETK_MUSIC_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut command = tokio::process::Command::new(python);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .arg("-B")
        .arg(script)
        .env("ETK_MUSIC_HOST_PID", std::process::id().to_string())
        .env("TMPDIR", directory.join("tmp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    #[cfg(unix)]
    let group = WorkerGroup(child.id().ok_or_else(|| ApiError::bad("音乐进程已退出"))?);
    let payload =
        json!({"action":action,"body":body,"config":music_config(config),"cache_dir":directory});
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| ApiError::bad("无法启动音乐引擎"))?;
    input.write_all(payload.to_string().as_bytes()).await?;
    drop(input);
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::bad("音乐引擎输出不可用"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::bad("音乐引擎日志不可用"))?;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let outcome: Result<()> = {
        let finish = async {
            tokio::try_join!(
                child.wait(),
                stdout.read_to_end(&mut output),
                stderr.read_to_end(&mut errors)
            )
        };
        tokio::pin!(finish);
        let timeout = tokio::time::sleep(Duration::from_secs(if context.is_some() {
            900
        } else {
            180
        }));
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                result = &mut finish => break result.map(|_|()).map_err(ApiError::from),
                _ = &mut timeout => break Err(ApiError(StatusCode::GATEWAY_TIMEOUT,"音乐处理超时，请查看日志".into())),
                _ = async { if let Some(context)=context {context.cancelled().await;} else {std::future::pending::<()>().await;} } => break Err(ApiError(StatusCode::CONFLICT,"音乐任务已停止".into())),
            }
        }
    };
    if outcome.is_err() {
        #[cfg(unix)]
        {
            group.signal(libc::SIGTERM);
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            group.signal(libc::SIGKILL);
        }
        let _ = child.kill().await;
    }
    outcome?;
    if !errors.is_empty() {
        tracing::warn!(
            "[音乐] {}",
            crate::logs::redact(&String::from_utf8_lossy(&errors))
                .chars()
                .take(4000)
                .collect::<String>()
        );
    }
    let result: Value = serde_json::from_slice(&output)
        .map_err(|_| ApiError::bad("音乐引擎返回无效结果，请检查运行依赖和日志"))?;
    if result["success"] != true {
        return Err(ApiError::bad(
            result["error"].as_str().unwrap_or("音乐处理失败"),
        ));
    }
    Ok(result["data"].clone())
}

async fn library(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let config = state.config.read().await.clone();
    Ok(Json(engine(&state,"library",json!({"query":query.get("query"),"directory":query.get("directory"),"page":query.get("page").and_then(|v|v.parse::<u32>().ok()).unwrap_or(1),"page_size":query.get("page_size").and_then(|v|v.parse::<u32>().ok()).unwrap_or(50)}),&config,None).await?))
}

async fn image(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response> {
    let config = state.config.read().await.clone();
    let result = engine(
        &state,
        "image",
        json!({"path":query.get("path")}),
        &config,
        None,
    )
    .await?;
    let data = result["data"]
        .as_str()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "音乐封面不存在".into()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| ApiError::bad("音乐封面无效"))?;
    let mime = result["mime"]
        .as_str()
        .filter(|mime| matches!(*mime, "image/png" | "image/jpeg" | "image/webp"))
        .ok_or_else(|| ApiError::bad("音乐封面格式无效"))?;
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CACHE_CONTROL, "private, max-age=60".into()),
        ],
        bytes,
    )
        .into_response())
}

async fn query(
    State(state): State<AppState>,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    if !matches!(
        action.as_str(),
        "search"
            | "explore"
            | "detail"
            | "related"
            | "inspect"
            | "cache"
            | "selection"
            | "edit_metadata"
            | "lyrics_search"
            | "lyrics_detail"
            | "lyrics_translate"
    ) {
        return Err(ApiError::bad("未知音乐查询"));
    }
    let config = state.config.read().await.clone();
    Ok(Json(engine(&state, &action, body, &config, None).await?))
}

async fn audio(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> Result<Response> {
    let path = query
        .get("path")
        .ok_or_else(|| ApiError::bad("请选择本地音乐"))?;
    let config = state.config.read().await.clone();
    let value = engine(&state, "audio_path", json!({"path":path}), &config, None).await?;
    let path = value["path"]
        .as_str()
        .ok_or_else(|| ApiError::bad("音乐路径无效"))?;
    // 复用静态文件服务的 Range/HEAD 实现，拖动试听无需将整首歌曲读入内存。
    let response = tower_http::services::ServeFile::new(path)
        .try_call(request)
        .await?;
    let mut response = response.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    Ok(response)
}

fn validate_uploaded_cover(value: &Value) -> Result<()> {
    let text = value
        .as_str()
        .ok_or_else(|| ApiError::bad("上传封面格式无效"))?;
    if text.len() > 1_400_000 {
        return Err(ApiError::bad("上传封面超过 1 MiB，请缩小图片"));
    }
    let (data, format) = if let Some(data) = text.strip_prefix("data:image/jpeg;base64,") {
        (data, image::ImageFormat::Jpeg)
    } else if let Some(data) = text.strip_prefix("data:image/png;base64,") {
        (data, image::ImageFormat::Png)
    } else {
        return Err(ApiError::bad("上传封面只支持 JPEG 或 PNG"));
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| ApiError::bad("上传封面编码无效"))?;
    if bytes.len() > 1024 * 1024 {
        return Err(ApiError::bad("上传封面超过 1 MiB，请缩小图片"));
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| ApiError::bad("上传封面损坏或尺寸超过 2048 像素"))?;
    Ok(())
}

async fn submit(
    State(state): State<AppState>,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    if !matches!(
        action.as_str(),
        "scan" | "preview" | "execute" | "clear-comments"
    ) {
        return Err(ApiError::bad("未知音乐任务"));
    }
    if action == "preview" {
        if !matches!(body["operation"].as_str(), Some("rescrape" | "edit")) {
            return Err(ApiError::bad("ETK 音乐仅支持刮削和标签编辑"));
        }
        if let Some(upload) = body["metadata"]
            .get("cover_upload")
            .filter(|v| !v.is_null() && **v != "")
        {
            validate_uploaded_cover(upload)?;
        }
        let paths = body["paths"]
            .as_array()
            .filter(|paths| !paths.is_empty() && paths.len() <= 1000)
            .ok_or_else(|| ApiError::bad("请选择 1 至 1000 个音乐文件"))?;
        if paths.len() != 1
            && body["metadata"]["cover_upload"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        {
            return Err(ApiError::bad("本地封面上传只用于当前歌曲"));
        }
        if paths.iter().any(|value| {
            value.as_str().is_none_or(|text| {
                text.is_empty()
                    || text.contains('\0')
                    || text.len() > 4096
                    || std::path::Path::new(text).is_absolute()
                    || text.split('/').any(|part| matches!(part, "" | "." | ".."))
            })
        }) {
            return Err(ApiError::bad("音乐文件必须使用音乐库内的相对路径"));
        }
        let unique = paths
            .iter()
            .map(Value::to_string)
            .collect::<std::collections::HashSet<_>>();
        if unique.len() != paths.len() {
            return Err(ApiError::bad("请勿重复选择同一音乐文件"));
        }
    }
    let config = music_config(&*state.config.read().await);
    validate_config(&config)?;
    let payload = json!({"action":action,"body":body,"config":config});
    let spec = JobSpec::new("music", payload.clone());
    let work = state.clone();
    let name = match action.as_str() {
        "scan" => "扫描本地音乐库",
        "preview" => "音乐刮削预览",
        "clear-comments" => "清空音乐备注/描述",
        _ => "处理所选音乐",
    };
    let task_id = state
        .jobs
        .submit_persistent(name, spec, move |context| async move {
            Ok(execute_persisted(&work, &payload, &context).await?.1)
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task_id,"message":"音乐任务已提交"})),
    ))
}

pub(crate) async fn submit_clear_comments(state: &AppState) -> Result<Value> {
    Ok(submit(
        State(state.clone()),
        Path("clear-comments".into()),
        Json(json!({})),
    )
    .await?
    .1
    .0)
}

pub(crate) async fn execute_persisted(
    state: &AppState,
    payload: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    let mut result = execute_inner(state, payload, context).await;
    if matches!(
        payload["action"].as_str(),
        Some("execute" | "clear-comments")
    ) && !context.is_interrupted()
    {
        if let Err(error) = engine(
            state,
            "cleanup",
            json!({"task_id":context.id}),
            &payload["config"],
            None,
        )
        .await
        {
            tracing::warn!("[音乐] 清理任务暂存文件失败: {}", error.1);
            match &mut result {
                Ok((_, value)) => {
                    value["warning_details"] =
                        json!([format!("音乐暂存清理未完成，将在恢复时重试：{}", error.1)]);
                }
                Err(original) => {
                    original.1 = format!("{}；音乐暂存清理未完成：{}", original.1, error.1)
                }
            }
        }
    }
    result
}

async fn execute_inner(
    state: &AppState,
    payload: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    let config = &payload["config"];
    if matches!(
        payload["action"].as_str(),
        Some("execute" | "clear-comments")
    ) {
        context.save_plan("music-cleanup-config", config).await?;
    }
    // 恢复任务不借用后来更改的目录；必须仍与当前配置一致。
    let current = state.config.read().await;
    for key in ["music_library_path", "music_seed_path"] {
        if current[key] != config[key] {
            return Err(ApiError::bad("音乐目录配置已变化，请重新提交任务"));
        }
    }
    drop(current);
    let action = payload["action"].as_str().unwrap_or("");
    if action == "preview" {
        let body = &payload["body"];
        let grouping = if let Some(value) = context.plan("music-preview-groups").await? {
            value
        } else {
            let value = engine(
                state,
                "preview_groups",
                json!({"paths":body["paths"]}),
                config,
                Some(context),
            )
            .await?;
            context.save_plan("music-preview-groups", &value).await?;
            value
        };
        let groups: std::collections::BTreeMap<String, Vec<Value>> =
            serde_json::from_value(grouping).map_err(|_| ApiError::bad("音乐专辑范围无效"))?;
        if body["selection"]["music_type"] == "recording"
            && body["paths"]
                .as_array()
                .is_some_and(|paths| paths.len() != 1)
        {
            return Err(ApiError::bad("单曲资料只能用于一个所选音频，请选择专辑"));
        }
        let previous = if let Some(id) = body["previous_preview_task_id"].as_str() {
            let old = state
                .jobs
                .result(id)
                .await
                .ok_or_else(|| ApiError::bad("原音乐预览已不存在，请重新预览"))?;
            let value = old["result"]["preview"].clone();
            let paths = value["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| item["path"].clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut expected = body["paths"].as_array().cloned().unwrap_or_default();
            let mut actual = paths;
            actual.sort_by_key(Value::to_string);
            expected.sort_by_key(Value::to_string);
            if actual != expected
                || value["operation"] != body["operation"]
                || value["library_path"] != config["music_library_path"]
                || value["seed_path"] != config["music_seed_path"]
                || value["cue_enable"] != config["music_cue_enable"]
                || value["write_policy"] != config["music_scraping_policies"]
                || value["simplified"] != config["music_metadata_to_simplified"]
                || (body["operation"] == "edit" && value["metadata"] != body["metadata"])
            {
                return Err(ApiError::bad("原音乐预览范围或配置已变化，请重新预览"));
            }
            Some(value)
        } else {
            None
        };
        let mut rows = vec![];
        for (index, (directory, paths)) in groups.iter().enumerate() {
            context.checkpoint().await?;
            let key = format!("music-preview-group:{index}");
            let value = if let Some(saved) = context.completed(&key).await? {
                saved
            } else {
                context
                    .update_counted(index as u64, groups.len() as u64, "音乐识别", directory)
                    .await;
                let reuse = previous.as_ref().filter(|old| {
                    old["selections"][directory] == body["selections"][directory]
                        && old["selection"] == body["selection"]
                });
                let value = if let Some(old) = reuse {
                    json!({"items":old["items"].as_array().unwrap().iter().filter(|item|paths.contains(&item["path"])).cloned().collect::<Vec<_>>()})
                } else {
                    let mut group_body = body.clone();
                    group_body["paths"] = json!(paths);
                    engine(state, "preview", group_body, config, Some(context)).await?
                };
                context.complete(&key, &value).await?;
                value
            };
            rows.extend(
                value["items"]
                    .as_array()
                    .cloned()
                    .ok_or_else(|| ApiError::bad("音乐预览结果无效"))?,
            );
        }
        let value = json!({"operation":body["operation"],"metadata":body["metadata"],"items":rows,"selection":body["selection"],"selections":body["selections"],"library_path":config["music_library_path"],"seed_path":config["music_seed_path"],"cue_enable":config["music_cue_enable"],"write_policy":config["music_scraping_policies"],"simplified":config["music_metadata_to_simplified"]});
        context.save_plan("music-preview", &value).await?;
        return Ok((200, json!({"preview":value,"message":"音乐预览已完成"})));
    }
    let items = if let Some(plan) = context.plan("music-scope").await? {
        plan
    } else {
        let items = if action == "scan" {
            engine(state, "scope", json!({}), config, Some(context)).await?["paths"].clone()
        } else if action == "clear-comments" {
            context
                .update(0, "清空音乐备注/描述", "读取本地音乐标签")
                .await;
            context.save_plan("music-operation", &json!("edit")).await?;
            engine(
                state,
                "clear_comments_preview",
                json!({}),
                config,
                Some(context),
            )
            .await?
        } else {
            let task = payload["body"]["preview_task_id"]
                .as_str()
                .ok_or_else(|| ApiError::bad("请先完成音乐预览"))?;
            let preview = state
                .jobs
                .result(task)
                .await
                .ok_or_else(|| ApiError::bad("音乐预览已不存在，请重新预览"))?;
            let preview = &preview["result"]["preview"];
            if preview["library_path"] != config["music_library_path"]
                || preview["seed_path"] != config["music_seed_path"]
                || preview["cue_enable"] != config["music_cue_enable"]
            {
                return Err(ApiError::bad("音乐目录或 CUE 设置已变化，请重新预览"));
            }
            if !preview["items"].is_array() {
                return Err(ApiError::bad("音乐预览尚未完成"));
            }
            if preview["write_policy"] != config["music_scraping_policies"]
                || preview["simplified"] != config["music_metadata_to_simplified"]
            {
                return Err(ApiError::bad("音乐刮削设置已变化，请重新预览"));
            }
            if preview["items"].as_array().is_none_or(|items| {
                items.is_empty() || items.iter().any(|item| item["success"] != true)
            }) {
                return Err(ApiError::bad(
                    "音乐预览存在未通过的文件，请完成纠正后再刮削",
                ));
            }
            context
                .save_plan("music-operation", &preview["operation"])
                .await?;
            preview["items"].clone()
        };
        context.save_plan("music-scope", &items).await?;
        items
    };
    let items = items
        .as_array()
        .ok_or_else(|| ApiError::bad("音乐任务范围无效"))?;
    if action == "scan" {
        let mut results = vec![];
        for (batch, group) in items.chunks(100).enumerate() {
            context.checkpoint().await?;
            let key = format!("music-scan:{batch}");
            let values = if let Some(value) = context.completed(&key).await? {
                value
            } else {
                context
                    .update(
                        (batch * 10000 / items.len().max(1)) as u64,
                        "扫描音乐库",
                        &format!("读取 {}/{}", batch * 100, items.len()),
                    )
                    .await;
                let value = engine(
                    state,
                    "scan_batch",
                    json!({"paths":group,"scan_id":context.id}),
                    config,
                    Some(context),
                )
                .await?;
                context.complete(&key, &value).await?;
                value
            };
            results.extend(
                values
                    .as_array()
                    .cloned()
                    .ok_or_else(|| ApiError::bad("音乐扫描结果无效"))?,
            );
        }
        let failed = results.iter().filter(|row| row["success"] == false).count();
        if failed == 0 {
            engine(
                state,
                "scan_finish",
                json!({"scan_id":context.id}),
                config,
                Some(context),
            )
            .await?;
        }
        return Ok((
            200,
            json!({"items":results,"failed":failed,"warnings":failed,"message":format!("音乐库扫描完成，共 {} 项，失败 {} 项",items.len(),failed)}),
        ));
    }
    let operation = context
        .plan("music-operation")
        .await?
        .unwrap_or(Value::Null);
    let mut results = vec![];
    let mut lyrics_used = 0.0;
    for (index, item) in items.iter().enumerate() {
        context.checkpoint().await?;
        let key = format!("music-item:{index}");
        if let Some(value) = context.completed(&key).await? {
            if value["success"] == true {
                clear_receipt(state, context, index).await;
            }
            lyrics_used += value["lyrics_elapsed"].as_f64().unwrap_or(0.0);
            results.push(value);
            continue;
        }
        context
            .update(
                (index * 100 / items.len().max(1)) as u64,
                "音乐",
                &format!("处理 {}/{}", index + 1, items.len()),
            )
            .await;
        let body = json!({"item":item,"operation":operation,"execution_id":format!("{}-{:x}",context.id,index),"lyrics_budget":(config["lyrics_batch_timeout"].as_f64().unwrap_or(120.0)-lyrics_used).max(0.0)});
        let result = if item["success"] != true {
            item.clone()
        } else if action == "clear-comments" && item["skip"] == true {
            json!({"path":item["path"],"success":true,"skipped":true,"message":"没有备注或描述，已跳过"})
        } else {
            match engine(state, "execute", body, config, Some(context)).await {
                Ok(value) => value,
                Err(error) => {
                    context.checkpoint().await?;
                    json!({"path":item["path"].clone(),"success":false,"message":error.1})
                }
            }
        };
        context.complete(&key, &result).await?;
        if result["success"] == true {
            clear_receipt(state, context, index).await;
        }
        lyrics_used += result["lyrics_elapsed"].as_f64().unwrap_or(0.0);
        results.push(result);
    }
    let failed = results.iter().filter(|row| row["success"] == false).count();
    let skipped = results.iter().filter(|row| row["skipped"] == true).count();
    let message = if action == "clear-comments" {
        format!(
            "音乐备注/描述清理完成，共 {} 项，清空 {} 项，跳过 {skipped} 项，失败 {failed} 项",
            items.len(),
            items.len() - skipped - failed
        )
    } else {
        format!("音乐任务完成，共 {} 项，失败 {} 项", items.len(), failed)
    };
    Ok((
        200,
        json!({"items":results,"failed":failed,"warnings":failed,"skipped":skipped,"message":message}),
    ))
}

async fn clear_receipt(state: &AppState, context: &JobContext, index: usize) {
    // 只有持久任务逐项完成记录已提交后，才能删除重启恢复回执。
    let receipt = state
        .data_dir
        .join("music/operations")
        .join(format!("{}-{:x}.json", context.id, index));
    if let Err(error) = tokio::fs::remove_file(receipt).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("[音乐] 清理已完成恢复记录失败: {error}");
        }
    }
}

pub(crate) async fn resume_cleanup(state: &AppState, context: &JobContext) -> Result<()> {
    if context.is_terminal_cleanup() {
        if let Some(config) = context.plan("music-cleanup-config").await? {
            engine(
                state,
                "cleanup",
                json!({"task_id":context.id}),
                &config,
                None,
            )
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod cover_upload_tests {
    use super::*;

    #[test]
    fn validates_image_contents_and_size_before_scheduling() {
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let png = base64::engine::general_purpose::STANDARD.encode(image.into_inner());
        assert!(validate_uploaded_cover(&json!(format!("data:image/png;base64,{png}"))).is_ok());
        for value in [
            json!("data:image/png;base64,YQ=="),
            json!("data:image/svg+xml;base64,YQ=="),
            json!(12),
            json!("x".repeat(1_400_001)),
        ] {
            assert!(validate_uploaded_cover(&value).is_err());
        }
    }
}
