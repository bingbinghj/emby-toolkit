use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    state::string,
};
use reqwest::Method;
use serde_json::Value;
use std::{collections::HashSet, time::Duration};

pub fn library_ignored(config: &Value, library: &str) -> bool {
    !library.is_empty()
        && crate::media_edit::list(&config["libraries_to_ignore"])
            .iter()
            .any(|id| id.as_str() == Some(library))
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
    for _ in 0..300 {
        context.checkpoint().await?;
        let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
        if tasks
            .as_array()
            .is_some_and(|tasks| !tasks.iter().any(|t| t["State"] == "Running"))
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(ApiError::bad("等待 Emby 后台任务完成超时"))
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
    }
    for _ in 0..900 {
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
            return Ok(());
        }
    }
    Err(ApiError::bad(format!("等待 Emby 任务超时：{key}")))
}
pub async fn library_for_path(state: &AppState, path: &str) -> Result<Option<Value>> {
    let folders = state
        .emby(Method::GET, "Library/VirtualFolders", &[], None)
        .await?;
    Ok(crate::media_edit::list(&folders)
        .into_iter()
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
        .map(|(_, library)| library))
}

async fn wait_item_refresh(
    state: &AppState,
    id: &str,
    previous: &str,
    context: &JobContext,
) -> Result<String> {
    for _ in 0..120 {
        context.checkpoint().await?;
        let details = crate::media_edit::details(state, id, "DateLastRefreshed").await?;
        let current = string(&details, "DateLastRefreshed").trim().to_string();
        if !current.is_empty() && current != previous {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(ApiError::bad(format!(
        "等待 Emby 项目实际刷新完成超时：{id}"
    )))
}

pub async fn finalize_refresh(state: &AppState, item: &Value, context: &JobContext) -> Result<()> {
    let id = string(item, "Id");
    let series = item["Type"] == "Series";
    idle(state, context).await?;
    let details = crate::media_edit::details(state, &id, "Path,DateLastRefreshed").await?;
    let previous = string(&details, "DateLastRefreshed").trim().to_string();
    crate::strm::with_settings(state, "preserve", || async {
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
            .await?;
        wait_item_refresh(state, &id, &previous, context).await
    })
    .await?;
    if !series {
        return Ok(());
    }
    idle(state, context).await?;
    let path = std::path::Path::new(item["Path"].as_str().unwrap_or("")).join("tvshow.nfo");
    let xml = tokio::fs::read_to_string(&path).await?;
    let details = crate::media_edit::details(state, &id, "DateLastRefreshed").await?;
    let previous = string(&details, "DateLastRefreshed").trim().to_string();
    crate::strm::with_settings(state, "preserve", || async {
        context.checkpoint().await?;
        state
            .emby(
                Method::POST,
                "Library/Media/Updated",
                &[],
                Some(serde_json::json!({"Updates":[{"Path":path,"UpdateType":"Modified"}]})),
            )
            .await?;
        wait_item_refresh(state, &id, &previous, context).await
    })
    .await?;
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
        .ok_or_else(|| ApiError::bad("NFO 首播年份无效"))?;
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
            if stable >= 3 {
                return Ok(());
            }
        } else {
            stable = 0;
            if corrections < 3 {
                corrections += 1;
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
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(ApiError::bad("Series 首播年份未稳定，无法确认入库收尾完成"))
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
