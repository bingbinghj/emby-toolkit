use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    state::string,
};
use reqwest::Method;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

pub async fn ancestor_context(
    state: &AppState,
    item: &Value,
    library: Option<&Value>,
) -> (HashMap<String, String>, String) {
    let mut parents = HashMap::new();
    let guid = library
        .map(|library| string(library, "Guid"))
        .unwrap_or_default();
    let source = string(item, "_SourceLibraryId");
    let mut current = string(item, "Id");
    for _ in 0..10 {
        let details = match crate::media_edit::details(state, &current, "ParentId").await {
            Ok(details) => details,
            Err(error) => {
                tracing::warn!("实时构建爬树地图失败：{}", error.1);
                break;
            }
        };
        let parent = string(&details, "ParentId");
        if parent == source && !guid.is_empty() && guid.to_lowercase() != "none" {
            let composite = format!("{guid}_{parent}");
            parents.insert(current, composite.clone());
            parents.insert(composite, "1".into());
            break;
        }
        if parent.is_empty() || parent == "1" {
            break;
        }
        parents.insert(current, parent.clone());
        current = parent;
    }
    (parents, guid)
}

pub struct Response {
    response: reqwest::Response,
    _permit: Option<tokio::sync::SemaphorePermit<'static>>,
}
impl From<reqwest::Response> for Response {
    fn from(response: reqwest::Response) -> Self {
        Self {
            response,
            _permit: None,
        }
    }
}
impl Response {
    pub fn status(&self) -> reqwest::StatusCode {
        self.response.status()
    }
    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        self.response.headers()
    }
    pub fn error_for_status(self) -> std::result::Result<Self, reqwest::Error> {
        self.response.error_for_status_ref()?;
        Ok(self)
    }
    pub async fn bytes(self) -> std::result::Result<axum::body::Bytes, reqwest::Error> {
        self.response.bytes().await
    }
    pub async fn json<T: serde::de::DeserializeOwned>(
        self,
    ) -> std::result::Result<T, reqwest::Error> {
        self.response.json().await
    }
    pub async fn text(self) -> std::result::Result<String, reqwest::Error> {
        self.response.text().await
    }
    pub async fn chunk(
        &mut self,
    ) -> std::result::Result<Option<axum::body::Bytes>, reqwest::Error> {
        self.response.chunk().await
    }
}
pub async fn send(
    request: reqwest::RequestBuilder,
) -> std::result::Result<Response, reqwest::Error> {
    static REQUESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(10);
    let permit = REQUESTS.acquire().await.expect("Emby 请求槽位不会关闭");
    let retryable = request.try_clone();
    let mut next = request;
    for attempt in 0..=5 {
        let response = next.send().await;
        let retry = match &response {
            Ok(response) => {
                matches!(response.status().as_u16(), 500 | 502 | 503 | 504)
                    || (matches!(response.status().as_u16(), 413 | 429)
                        && response.headers().contains_key("Retry-After"))
            }
            Err(error) => error.is_connect() || error.is_timeout(),
        };
        if attempt == 5 || !retry || retryable.is_none() {
            let response = if retry && attempt == 5 {
                response.and_then(reqwest::Response::error_for_status)
            } else {
                response
            };
            return response.map(|response| Response {
                response,
                _permit: Some(permit),
            });
        }
        let delay = response
            .as_ref()
            .ok()
            .and_then(|response| retry_after(response.headers()))
            .unwrap_or_else(|| {
                Duration::from_secs(if attempt == 0 { 0 } else { 2_u64.pow(attempt) })
            });
        tracing::warn!("Emby 请求暂时失败，准备重试 {}/5。", attempt + 1);
        drop(response);
        tokio::time::sleep(delay).await;
        next = retryable
            .as_ref()
            .unwrap()
            .try_clone()
            .expect("已验证请求可以重放");
    }
    unreachable!()
}
pub(crate) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get("Retry-After")?.to_str().ok()?;
    let seconds = value.trim().parse::<i64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()
            .map(|date| date.timestamp() - chrono::Utc::now().timestamp())
    })?;
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}

pub fn library_ignored(config: &Value, library: &str) -> bool {
    !library.is_empty()
        && crate::media_edit::list(&config["libraries_to_ignore"])
            .iter()
            .any(|id| id.as_str() == Some(library))
}
pub async fn by_ids(
    state: &AppState,
    ids: &[String],
    fields: &str,
    context: Option<&JobContext>,
) -> Result<Vec<Value>> {
    if ids.is_empty() || !state.configured().await {
        return Ok(vec![]);
    }
    let mut items = vec![];
    for (index, batch) in ids.chunks(100).enumerate() {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        match state
            .emby(
                Method::GET,
                "Items",
                &[("Ids", batch.join(",")), ("Fields", fields.into())],
                None,
            )
            .await
        {
            Ok(response) => items.extend(crate::media_edit::list(&response["Items"])),
            Err(error) => tracing::error!(
                "根据 ID 列表批量获取 Emby 项目时，处理批次 {} 失败：{}",
                index + 1,
                error.1
            ),
        }
    }
    Ok(items)
}

pub async fn existing_ids(
    state: &AppState,
    ids: &[String],
    context: Option<&JobContext>,
) -> Result<HashSet<String>> {
    let mut normalized = vec![];
    let mut seen = HashSet::new();
    for id in ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        if seen.insert(id) {
            normalized.push(id);
        }
    }
    let result = async {
        let mut existing = HashSet::new();
        for batch in normalized.chunks(100) {
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            let config = state.config.read().await.clone();
            let response = send(
                state
                    .http
                    .get(format!(
                        "{}/Items",
                        string(&config, "emby_server_url").trim_end_matches('/')
                    ))
                    .query(&[
                        ("api_key", string(&config, "emby_api_key")),
                        ("Ids", batch.join(",")),
                        ("Fields", "Id".into()),
                        ("Limit", batch.len().to_string()),
                    ])
                    .timeout(Duration::from_secs(
                        config["emby_api_timeout"]
                            .as_u64()
                            .filter(|timeout| *timeout > 0)
                            .unwrap_or(60),
                    )),
            )
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
            let items = response["Items"]
                .as_array()
                .ok_or_else(|| ApiError::bad("Emby 存活响应缺少 Items 数组"))?;
            if response.get("TotalRecordCount").is_some_and(|count| {
                count
                    .as_f64()
                    .or_else(|| count.as_bool().map(|value| f64::from(u8::from(value))))
                    != Some(items.len() as f64)
            }) {
                return Err(ApiError::bad("Emby 存活响应分页不完整"));
            }
            for item in items {
                let id = match item.get("Id") {
                    Some(Value::String(id)) if !id.is_empty() => id.clone(),
                    Some(Value::Number(id)) if id.as_f64().is_some_and(|id| id != 0.0) => {
                        id.to_string()
                    }
                    Some(Value::Bool(true)) => "True".into(),
                    _ => return Err(ApiError::bad("Emby 存活响应存在无效 Item")),
                };
                if !batch.contains(&id.as_str()) {
                    return Err(ApiError::bad("Emby 存活响应包含未请求的 Item"));
                }
                existing.insert(id);
            }
        }
        Ok(existing)
    }
    .await;
    result.inspect_err(|error| tracing::error!("批量核对 Emby Item 存活状态失败：{}", error.1))
}

pub async fn processing_libraries(state: &AppState) -> Result<Vec<String>> {
    let config = state.config.read().await.clone();
    let user = crate::state::string(&config, "emby_user_id");
    if user.is_empty() {
        return Err(ApiError::bad("未配置 Emby 用户 ID"));
    }
    let response = state
        .emby(Method::GET, &format!("Users/{user}/Views"), &[], None)
        .await?;
    let libraries = response["Items"]
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 媒体库列表无效"))?;
    Ok(libraries
        .iter()
        .filter_map(|library| library["Id"].as_str())
        .filter(|id| !id.is_empty() && !library_ignored(&config, id))
        .map(str::to_string)
        .collect())
}

pub async fn path_ignored(state: &AppState, path: &str) -> Result<bool> {
    let config = state.config.read().await.clone();
    if crate::media_edit::list(&config["libraries_to_ignore"]).is_empty() {
        return Ok(false);
    }
    Ok(library_for_path(state, path)
        .await?
        .is_some_and(|library| library_ignored(&config, &crate::state::string(&library, "ItemId"))))
}

pub async fn items(
    state: &AppState,
    parent: &str,
    fields: &str,
    context: &JobContext,
) -> Result<Vec<Value>> {
    if library_ignored(&*state.config.read().await, parent) {
        return Ok(vec![]);
    }
    let mut result = vec![];
    let mut start = 0;
    let mut seen = HashSet::new();
    loop {
        context.checkpoint().await?;
        let mut page = None;
        for attempt in 0..3 {
            context.checkpoint().await?;
            let limit = if attempt == 2 { 500 } else { 1000 };
            let query = vec![
                ("ParentId", parent.into()),
                ("Recursive", "true".into()),
                ("Fields", fields.into()),
                ("StartIndex", start.to_string()),
                ("Limit", limit.to_string()),
                (
                    "IncludeItemTypes",
                    "Movie,Series,Season,Episode,Folder,CollectionFolder,UserView".into(),
                ),
            ];
            match state.emby(Method::GET, "Items", &query, None).await {
                Ok(value) if value["Items"].is_array() => {
                    page = Some((value, limit));
                    break;
                }
                Ok(_) if attempt == 2 => {
                    return Err(ApiError::bad("Emby 分页响应无效，已停止同步"));
                }
                Err(error) if attempt == 2 => return Err(error),
                _ => tokio::time::sleep(Duration::from_secs(2 * (attempt + 1))).await,
            }
        }
        let (mut page, limit) = page.expect("page or error");
        let items = page["Items"].as_array_mut().unwrap();
        if items.is_empty() {
            break;
        }
        if items
            .iter()
            .any(|i| string(i, "Id").is_empty() || !seen.insert(string(i, "Id")))
        {
            return Err(ApiError::bad("Emby 分页返回缺失或重复 ID，已停止同步"));
        }
        for item in items.iter_mut() {
            item["_SourceLibraryId"] = serde_json::json!(parent);
        }
        let count = items.len();
        result.append(items);
        if count < limit {
            break;
        }
        start += count;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(result)
}

pub async fn refresh_item(
    state: &AppState,
    id: &str,
    recursive: bool,
    context: &JobContext,
) -> Result<()> {
    context.checkpoint().await?;
    state
        .emby(
            Method::POST,
            &format!("Items/{id}/Refresh"),
            &[
                ("Recursive", recursive.to_string()),
                ("MetadataRefreshMode", "Default".into()),
                ("ImageRefreshMode", "Default".into()),
                ("ReplaceAllMetadata", "false".into()),
                ("ReplaceAllImages", "false".into()),
            ],
            None,
        )
        .await?;
    Ok(())
}
pub async fn idle(state: &AppState, context: &JobContext) -> Result<()> {
    let ignored = [
        "rotate log file",
        "check for application updates",
        "refresh guide",
        "clean up collections",
        "build douban cache",
        "extract mediainfo",
        "extract intro fingerprint",
        "extract video thumbnail",
        "update emby.apiclient",
        "merge multi versions",
    ];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        context.checkpoint().await?;
        let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
        if tasks.as_array().is_some_and(|tasks| {
            !tasks.iter().any(|task| {
                task["State"] == "Running"
                    && !ignored
                        .iter()
                        .any(|keyword| string(task, "Name").to_lowercase().contains(keyword))
            })
        }) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("等待 Emby 空闲超时 (300s)，继续提交并验收实际刷新");
            return Ok(());
        }
        context.sleep(Duration::from_secs(2)).await?;
    }
}
pub async fn scheduled(state: &AppState, key: &str, context: &JobContext) -> Result<()> {
    context.checkpoint().await?;
    let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
    let task = tasks
        .as_array()
        .and_then(|tasks| tasks.iter().find(|t| t["Key"] == key))
        .ok_or_else(|| ApiError::bad(format!("未找到 Emby 任务：{key}")))?;
    let id = string(task, "Id");
    let running = matches!(task["State"].as_str(), Some("Running" | "Cancelling"));
    let previous = task["LastExecutionResult"]["StartTimeUtc"].clone();
    let native_name = string(task, "Name");
    let task_name = if native_name == "Extract MediaInfo"
        || native_name.is_empty() && key == "MediaInfoExtractTask"
    {
        "提取媒体信息".to_string()
    } else if native_name == "Extract Intro Fingerprint"
        || native_name.is_empty() && key == "IntroFingerprintExtractTask"
    {
        "提取片头声纹".to_string()
    } else if native_name == "Extract Video Thumbnail"
        || native_name.is_empty() && key == "VideoThumbnailExtractTask"
    {
        "提取视频预览缩略图".to_string()
    } else if native_name.is_empty() {
        key.to_string()
    } else {
        native_name
    };
    let started = tokio::time::Instant::now();
    context
        .activity(
            "等待Emby计划任务",
            &format!(
                "{task_name}（任务ID：{id}）：{}",
                if running {
                    "已有任务运行，等待其实际结束"
                } else {
                    "准备提交原生计划任务"
                }
            ),
        )
        .await;
    if !running {
        context.checkpoint().await?;
        state
            .emby(
                Method::POST,
                &format!("ScheduledTasks/Running/{id}"),
                &[],
                None,
            )
            .await?;
        tracing::info!(
            "[Emby计划任务] {task_name}（任务ID：{id}）启动请求已接收，等待本次执行结果"
        );
    }
    for attempt in 0..900 {
        context.sleep(Duration::from_secs(2)).await?;
        let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
        let task = tasks
            .as_array()
            .and_then(|tasks| tasks.iter().find(|t| t["Id"] == id))
            .ok_or_else(|| ApiError::bad("等待期间 Emby 任务消失"))?;
        if task["State"] == "Idle"
            && (running || task["LastExecutionResult"]["StartTimeUtc"] != previous)
        {
            if task["LastExecutionResult"]["Status"] != "Completed" {
                let status = task["LastExecutionResult"]["Status"]
                    .as_str()
                    .unwrap_or("未知");
                return Err(ApiError::bad(format!(
                    "Emby 任务未成功完成：{key}（{status}）"
                )));
            }
            tracing::info!(
                "[Emby计划任务] {task_name}（任务ID：{id}）本次执行已确认完成，耗时 {} 秒",
                started.elapsed().as_secs()
            );
            return Ok(());
        }
        if attempt % 15 == 0 {
            let state_name = match task["State"].as_str() {
                Some("Running") => "运行中",
                Some("Cancelling") => "正在停止",
                Some("Idle") => "空闲，等待新执行结果",
                _ => "未知状态",
            };
            context
                .activity(
                    "等待Emby计划任务",
                    &format!(
                        "{task_name}（任务ID：{id}）：{state_name}，已等待 {} 秒",
                        started.elapsed().as_secs()
                    ),
                )
                .await;
        }
    }
    Err(ApiError::bad(format!(
        "等待 Emby 计划任务超时：{task_name}（任务ID：{id}，已等待 {} 秒）",
        started.elapsed().as_secs()
    )))
}
pub async fn library_for_path(state: &AppState, path: &str) -> Result<Option<Value>> {
    let folders = state
        .emby(Method::GET, "Library/VirtualFolders", &[], None)
        .await?;
    Ok(library_for_path_in(&crate::media_edit::list(&folders), path).cloned())
}

pub(crate) fn library_for_path_in<'a>(folders: &'a [Value], path: &str) -> Option<&'a Value> {
    folders
        .iter()
        .filter_map(|library| {
            let length = crate::media_edit::list(&library["Locations"])
                .iter()
                .filter_map(Value::as_str)
                .filter(|root| {
                    path == *root || path.starts_with(&format!("{}/", root.trim_end_matches('/')))
                })
                .map(str::len)
                .max()?;
            Some((length, library))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, library)| library)
}

pub(crate) async fn path_ignored_in(state: &AppState, path: &str, folders: &[Value]) -> bool {
    let config = state.config.read().await;
    library_for_path_in(folders, path)
        .is_some_and(|library| library_ignored(&config, &string(library, "ItemId")))
}

async fn wait_item_refresh(
    state: &AppState,
    id: &str,
    previous: Option<&str>,
    mut trace: Option<crate::media_extra::RefreshTrace>,
    context: &JobContext,
    phase: &str,
    label: &str,
) -> Result<()> {
    let started = tokio::time::Instant::now();
    let status = |trace: Option<&crate::media_extra::RefreshTrace>| match trace {
        Some(trace) if trace.pending_count() > 0 => {
            format!("已开始，尚有 {} 次目标刷新未结束", trace.pending_count())
        }
        Some(_) => "未见本次目标刷新开始记录".into(),
        None => "刷新时间尚未变化，完成日志不可用".into(),
    };
    context
        .activity(phase, &format!("{label}：请求已接收，等待实际刷新完成"))
        .await;
    for attempt in 0..120 {
        context.checkpoint().await?;
        if let Some(previous) = previous {
            let details = crate::media_edit::details(state, id, "DateLastRefreshed").await?;
            let current = string(&details, "DateLastRefreshed").trim().to_string();
            if !current.is_empty() && current != previous {
                tracing::info!(
                    "[入库刷新] {label} · {phase} 已确认完成（依据：媒体刷新时间变化，等待 {} 秒）",
                    started.elapsed().as_secs()
                );
                return Ok(());
            }
        }
        if let Some(log_trace) = trace.as_mut() {
            let completed = async {
                let logs = state
                    .emby(
                        Method::GET,
                        "System/Logs/embyserver.txt/Lines",
                        &[("Limit", "500".into())],
                        None,
                    )
                    .await?;
                log_trace.feed(id, &logs)
            }
            .await;
            match completed {
                Ok(true) => {
                    tracing::info!(
                        "[入库刷新] {label} · {phase} 已确认完成（依据：Emby 刷新完成日志，等待 {} 秒）",
                        started.elapsed().as_secs()
                    );
                    return Ok(());
                }
                Ok(false) => {}
                Err(error) if previous.is_some_and(|value| !value.is_empty()) => {
                    tracing::warn!(
                        "[入库刷新] {label} · {phase} 完成日志不可用，继续验收刷新时间：{}",
                        error.1
                    );
                    trace = None;
                }
                Err(error) => {
                    return Err(ApiError(
                        error.0,
                        format!("{label} · {phase} 刷新日志读取或验收失败：{}", error.1),
                    ));
                }
            }
        }
        if attempt > 0 && attempt % 15 == 0 {
            context
                .activity(
                    phase,
                    &format!(
                        "{label}：已等待 {} 秒；{}",
                        started.elapsed().as_secs(),
                        status(trace.as_ref())
                    ),
                )
                .await;
        }
        context.sleep(Duration::from_secs(1)).await?;
    }
    Err(ApiError::bad(format!(
        "{label} · {phase} 等待超时（已等待 {} 秒；{}）",
        started.elapsed().as_secs(),
        status(trace.as_ref())
    )))
}

pub async fn finalize_refresh(state: &AppState, item: &Value, context: &JobContext) -> Result<()> {
    let id = string(item, "Id");
    let series = item["Type"] == "Series";
    let name = string(item, "Name");
    let label = format!("《{name}》（媒体ID：{id}）");
    context
        .activity(
            "等待后台扫描",
            &format!("{label}：等待 Emby 初次入库扫描结束"),
        )
        .await;
    idle(state, context).await?;
    let details = crate::media_edit::details(state, &id, "Path,DateLastRefreshed").await?;
    let series_path = string(&details, "Path");
    let previous = string(&details, "DateLastRefreshed").trim().to_string();
    let trace = match crate::media_extra::log_position(state).await {
        Ok(position) => Some(crate::media_extra::RefreshTrace::new(position)),
        Err(error) if previous.is_empty() => {
            return Err(ApiError(
                error.0,
                format!("{label}：首次元数据刷新无法读取完成日志基线：{}", error.1),
            ));
        }
        Err(_) => None,
    };
    context
        .activity(
            "首次元数据刷新",
            &format!(
                "{label}：提交{}元数据和图片刷新",
                if series { "电视剧" } else { "电影" }
            ),
        )
        .await;
    context.checkpoint().await?;
    state
        .emby(
            Method::POST,
            &format!("Items/{id}/Refresh"),
            &[
                ("Recursive", series.to_string()),
                ("MetadataRefreshMode", "FullRefresh".into()),
                ("ImageRefreshMode", "FullRefresh".into()),
                ("ReplaceAllMetadata", "false".into()),
                ("ReplaceAllImages", "false".into()),
            ],
            None,
        )
        .await
        .map_err(|error| {
            ApiError(
                error.0,
                format!("{label}：首次元数据刷新请求失败：{}", error.1),
            )
        })?;
    tracing::info!("[入库刷新] {label}：首次刷新请求已接收，等待实际完成");
    tracing::debug!("[入库刷新参数] 媒体ID {id}：递归 {series}，元数据/图片完整刷新，全部替换否");
    wait_item_refresh(
        state,
        &id,
        Some(&previous),
        trace,
        context,
        "首次元数据刷新",
        &label,
    )
    .await?;
    if !series {
        return Ok(());
    }
    context
        .activity(
            "等待后台扫描",
            &format!("{label}：首次刷新已完成，等待后台扫描结束后重读顶层NFO"),
        )
        .await;
    idle(state, context).await?;
    if series_path.is_empty() {
        return Err(ApiError::bad(format!(
            "{label}：Emby 未返回剧集根目录，无法定位顶层NFO"
        )));
    }
    let path = std::path::Path::new(&series_path).join("tvshow.nfo");
    context
        .activity(
            "顶层NFO扫描",
            &format!("{label}：重读顶层NFO，准备刷新剧集元数据"),
        )
        .await;
    let xml = tokio::fs::read_to_string(&path).await.map_err(|error| {
        ApiError::bad(format!(
            "{label}：无法读取顶层NFO {}：{error}",
            path.display()
        ))
    })?;
    let position = crate::media_extra::log_position(state)
        .await
        .map_err(|error| {
            ApiError(
                error.0,
                format!("{label}：顶层NFO扫描无法读取完成日志基线：{}", error.1),
            )
        })?;
    let trace = crate::media_extra::RefreshTrace::new(position);
    context.checkpoint().await?;
    tracing::debug!(
        "[顶层NFO扫描] {label}：立即刷新根目录 {series_path}，递归是，元数据/图片使用默认刷新，全部替换否；刷新日志起始行 {position}"
    );
    refresh_item(state, &id, true, context)
        .await
        .map_err(|error| {
            ApiError(
                error.0,
                format!("{label}：顶层NFO扫描请求失败：{}", error.1),
            )
        })?;
    tracing::info!(
        "[顶层NFO扫描] {label}：目录刷新请求已接收，等待本次开始与完成日志；ETK仅重读NFO文件"
    );
    wait_item_refresh(
        state,
        &id,
        None,
        Some(trace),
        context,
        "顶层NFO扫描",
        &label,
    )
    .await?;
    context
        .activity(
            "首播年份验收",
            &format!("{label}：顶层NFO扫描已确认完成，开始核对首播年和结束日期"),
        )
        .await;
    let tag = |name| {
        regex::Regex::new(&format!(r"(?is)<{name}[^>]*>(.*?)</{name}>"))
            .unwrap()
            .captures(&xml)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default()
    };
    let year = tag("year")
        .parse::<i32>()
        .ok()
        .or_else(|| tag("premiered").get(..4)?.parse().ok())
        .filter(|n| (1000..=9999).contains(n))
        .ok_or_else(|| {
            ApiError::bad(format!(
                "{label}：顶层NFO {} 的首播年份无效",
                path.display()
            ))
        })?;
    tracing::info!("[年份验收] {label}：NFO首播年份 {year}，需要连续3次一致；保留原连载状态");
    let mut stable = 0;
    let mut corrections = 0;
    for _ in 0..30 {
        context.checkpoint().await?;
        let mut details =
            crate::media_edit::details(state, &id, "ProductionYear,EndDate,Status").await?;
        if details["ProductionYear"].as_i64() == Some(year as i64)
            && details["EndDate"].as_str().is_none_or(str::is_empty)
        {
            stable += 1;
            tracing::info!(
                "[年份验收] {label}：首播年份 {year}、结束日期为空，连续一致 {stable}/3 次"
            );
            if stable >= 3 {
                return Ok(());
            }
        } else {
            stable = 0;
            if corrections < 3 {
                corrections += 1;
                tracing::info!(
                    "[年份验收] {label}：第 {corrections}/3 次纠正，Emby年份 {} → {year}，结束日期 {} → 空",
                    details["ProductionYear"],
                    details["EndDate"]
                );
                details["ProductionYear"] = serde_json::json!(year);
                details["EndDate"] = Value::Null;
                for key in [
                    "TagItems",
                    "MediaStreams",
                    "MediaSources",
                    "Chapters",
                    "RecursiveItemCount",
                    "ChildCount",
                    "ImageTags",
                    "SeriesTimerId",
                    "RunTimeTicks",
                ] {
                    details.as_object_mut().unwrap().remove(key);
                }
                state
                    .emby(Method::POST, &format!("Items/{id}"), &[], Some(details))
                    .await?;
            }
        }
        context.sleep(Duration::from_secs(1)).await?;
    }
    Err(ApiError::bad(format!(
        "{label}：电视剧首播年份未稳定，无法确认入库收尾完成"
    )))
}

pub async fn query_items(
    state: &AppState,
    parameters: &[(&str, String)],
    context: &JobContext,
) -> Result<Vec<Value>> {
    let mut result = vec![];
    let mut seen = std::collections::HashSet::new();
    loop {
        context.checkpoint().await?;
        let mut query = parameters.to_vec();
        query.extend([
            ("StartIndex", result.len().to_string()),
            ("Limit", "500".into()),
        ]);
        let response = state.emby(Method::GET, "Items", &query, None).await?;
        let items = response["Items"]
            .as_array()
            .ok_or_else(|| ApiError::bad("Emby 分页格式无效"))?;
        if items
            .iter()
            .any(|i| string(i, "Id").is_empty() || !seen.insert(string(i, "Id")))
        {
            return Err(ApiError::bad("Emby 分页返回缺失或重复 ID"));
        }
        if items.is_empty() {
            if response["TotalRecordCount"]
                .as_u64()
                .is_some_and(|n| (result.len() as u64) < n)
            {
                return Err(ApiError::bad("Emby 分页未完整返回"));
            }
            break;
        }
        result.extend(items.clone());
        if response["TotalRecordCount"]
            .as_u64()
            .is_some_and(|n| (result.len() as u64) >= n)
        {
            break;
        }
    }
    Ok(result)
}
pub async fn persons(state: &AppState, context: &JobContext) -> Result<Vec<Value>> {
    let libraries = processing_libraries(state).await?;
    let mut ids = std::collections::BTreeSet::new();
    for library in libraries {
        for item in items(state, &library, "People", context).await? {
            for person in crate::media_edit::list(&item["People"]) {
                let id = string(&person, "Id");
                if !id.is_empty() {
                    ids.insert(id);
                }
            }
        }
    }
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut result = vec![];
    for chunk in ids.into_iter().collect::<Vec<_>>().chunks(500) {
        context.checkpoint().await?;
        let response = state
            .emby(
                Method::GET,
                "Items",
                &[
                    ("Ids", chunk.join(",")),
                    ("Fields", "ProviderIds,Name".into()),
                ],
                None,
            )
            .await?;
        result.extend(
            response["Items"]
                .as_array()
                .ok_or_else(|| ApiError::bad("Emby 人物批量响应无效"))?
                .clone(),
        );
    }
    Ok(result)
}
