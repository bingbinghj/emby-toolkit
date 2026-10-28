use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use futures_util::FutureExt;
use reqwest::Method;
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, sync::Arc};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct Service {
    gate: Mutex<()>,
    token: Mutex<Option<String>>,
    pub(crate) threads: Arc<crate::concurrency::Service>,
}
pub use crate::concurrency::{slot, thread_limit};
async fn ui(
    state: &AppState,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> Result<Value> {
    let config = state.config.read().await.clone();
    let base = string(&config, "emby_server_url");
    let key = string(&config, "emby_api_key");
    let mut token = state.strm.token.lock().await;
    for attempt in 0..2 {
        let mut request = state
            .http
            .request(
                method.clone(),
                format!("{}/{path}", base.trim_end_matches('/')),
            )
            .query(query);
        if let Some(token) = &*token {
            request = request.header("X-Emby-Token", token);
        } else {
            request = request.query(&[("api_key", &key)]);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = crate::emby::send(request).await?;
        if attempt == 0 && matches!(response.status().as_u16(), 400 | 401 | 403) {
            let name = string(&config, "emby_admin_user");
            let password = string(&config, "emby_admin_pass");
            if name.is_empty() {
                return Err(ApiError::bad("神医配置页需要 Emby 管理员凭证"));
            }
            let response:Value=crate::emby::send(state.http.post(format!("{}/Users/AuthenticateByName",base.trim_end_matches('/')))
                .header("X-Emby-Authorization","MediaBrowser Client=\"EmbyToolkit\", Device=\"Server\", DeviceId=\"etk-rust-strm\", Version=\"1.0\"")
                .json(&json!({"Username":name,"Pw":password}))).await?.error_for_status()?.json().await?;
            if response["User"]["Policy"]["IsAdministrator"] != true {
                return Err(ApiError::forbidden());
            }
            let access = string(&response, "AccessToken");
            if access.is_empty() {
                return Err(ApiError::bad("Emby 未返回管理员访问令牌"));
            }
            *token = Some(access);
            continue;
        }
        let bytes = response.error_for_status()?.bytes().await?;
        return if bytes.is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("神医配置响应无效"))
        };
    }
    unreachable!()
}
pub async fn admin_system(state: &AppState) -> Result<Value> {
    let config = state.config.read().await.clone();
    let name = string(&config, "emby_admin_user");
    if name.is_empty() {
        return Err(ApiError::bad("无法获取Emby管理员令牌，不能读取系统目录"));
    }
    let response:Value=crate::emby::send(state.http.post(format!("{}/Users/AuthenticateByName",string(&config,"emby_server_url").trim_end_matches('/')))
        .header("X-Emby-Authorization","MediaBrowser Client=\"EmbyToolkit\", Device=\"Server\", DeviceId=\"etk-rust-strm\", Version=\"1.0\"")
        .json(&json!({"Username":name,"Pw":string(&config,"emby_admin_pass")}))).await?.error_for_status()?.json().await?;
    if response["User"]["Policy"]["IsAdministrator"] != true {
        return Err(ApiError::forbidden());
    }
    let token = string(&response, "AccessToken");
    if token.is_empty() {
        return Err(ApiError::bad("Emby未返回管理员令牌"));
    }
    *state.strm.token.lock().await = Some(token);
    ui(state, Method::GET, "System/Info", &[], None).await
}
pub async fn config(state: &AppState) -> Result<Value> {
    read(state, &discover(state).await?).await
}
pub async fn with_changes<F, Fut, T>(state: &AppState, changes: &Value, function: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _gate = state.strm.gate.lock().await;
    let id = discover(state).await?;
    let original = read(state, &id).await?;
    let changes = changes
        .as_object()
        .ok_or_else(|| ApiError::bad("临时配置必须是对象"))?;
    let mut restore = json!({});
    let mut desired = original.clone();
    for (key, value) in changes {
        if original[key] != *value {
            restore[key] = original[key].clone();
            desired[key] = value.clone();
        }
    }
    let changed = !restore.as_object().unwrap().is_empty();
    let mode = "自定义配置";
    let mode_name = settings_mode_label(mode);
    tracing::info!("[神医临时设置] {mode_name} 开始");
    let outcome = std::panic::AssertUnwindSafe(async {
        if changed {
            write(state, &id, &desired).await?;
            if !matches(&read(state, &id).await?, &desired) {
                return Err(ApiError::bad("神医临时配置回读不一致"));
            }
            tracing::info!("[神医临时设置] {mode_name} 已启用临时配置，结束后恢复原配置");
        } else {
            tracing::info!("[神医临时设置] {mode_name} 原配置已满足要求，无需修改");
        }
        function().await
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|_| Err(ApiError::bad("任务执行异常")));
    if changed && !restore_settings(state, &id, &restore, mode, outcome.as_ref().err()).await {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "神医临时配置恢复失败，请核对自定义ffmpeg路径".into(),
        ));
    }
    settings_finished(mode, outcome)
}
fn page_id(page: &Value) -> Option<String> {
    ["Href", "NavMenuId"].iter().find_map(|key| {
        let href = page[*key].as_str()?;
        reqwest::Url::parse(&format!(
            "http://emby.invalid/{}",
            href.trim_start_matches('/')
        ))
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "PageId")
        .map(|(_, value)| value.into_owned())
    })
}
async fn discover(state: &AppState) -> Result<String> {
    let pages = state
        .emby(Method::GET, "web/ConfigurationPages", &[], None)
        .await?;
    let pages = pages
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 配置页列表无效"))?;
    if let Some(id) = pages
        .iter()
        .filter(|p| p["Name"] == "MediaInfoExtractPageView")
        .find_map(page_id)
    {
        return Ok(id);
    }
    for page in pages
        .iter()
        .filter(|p| string(p, "PluginId").to_lowercase() == "63c322b7-a371-41a3-b11f-04f8418b37d8")
    {
        let Some(id) = page_id(page) else {
            continue;
        };
        let view = ui(
            state,
            Method::GET,
            "UI/View",
            &[("PageId", id), ("ClientLocale", "zh-cn".into())],
            None,
        )
        .await?;
        if let Some(id) = view["TabPageInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|tab| {
                let id = string(tab, "PageId");
                (string(tab, "PluginId").to_lowercase() == "63c322b7-a371-41a3-b11f-04f8418b37d8"
                    && id.ends_with(":MediaInfoExtractPageView"))
                .then_some(id)
            })
        {
            return Ok(id);
        }
    }
    Err(ApiError::bad("未找到神医媒体信息提取配置页"))
}
async fn read(state: &AppState, id: &str) -> Result<Value> {
    let view = ui(
        state,
        Method::GET,
        "UI/View",
        &[("PageId", id.into()), ("ClientLocale", "zh-cn".into())],
        None,
    )
    .await?;
    let object = &view["EditObjectContainer"]["Object"];
    if !object.is_object()
        || object.get("ExclusiveControlFeatures").is_none()
        || object.get("EnableImageCapture").is_none()
    {
        return Err(ApiError::bad("神医媒体信息提取配置格式不兼容"));
    }
    Ok(object.clone())
}
async fn write(state: &AppState, id: &str, object: &Value) -> Result<()> {
    ui(state,Method::POST,"UI/Command",&[],Some(&json!({"PageId":id,"CommandId":"PageSave","Data":object.to_string(),"ItemId":null,"ClientLocale":"zh-cn"}))).await?;
    Ok(())
}
fn features(value: &Value) -> HashSet<String> {
    value
        .as_str()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
fn matches(actual: &Value, expected: &Value) -> bool {
    expected.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| {
            if key == "ExclusiveControlFeatures" {
                features(&actual[key]) == features(value)
            } else {
                actual[key] == *value
            }
        })
    })
}
fn settings_mode_label(mode: &str) -> &str {
    match mode {
        "preserve" => "忽略文件变更",
        "capture" => "图片捕获",
        _ => mode,
    }
}
async fn restore_settings(
    state: &AppState,
    id: &str,
    restore: &Value,
    mode: &str,
    failure: Option<&ApiError>,
) -> bool {
    let mode_name = settings_mode_label(mode);
    tracing::info!("[神医临时设置] {mode_name} 开始恢复原配置");
    for attempt in 1..=3 {
        let result = async {
            let mut current = read(state, id).await?;
            current
                .as_object_mut()
                .unwrap()
                .extend(restore.as_object().unwrap().clone());
            write(state, id, &current).await?;
            if !matches(&read(state, id).await?, restore) {
                return Err(ApiError::bad("恢复配置回读不一致"));
            }
            Ok::<_, ApiError>(())
        }
        .await;
        match result {
            Ok(()) => {
                tracing::info!("[神医临时设置] {mode_name} 原配置已恢复");
                return true;
            }
            Err(error) => {
                tracing::warn!(
                    "[神医临时设置] {mode_name} 第 {attempt}/3 次恢复未确认：{}",
                    error.1
                );
            }
        }
    }
    if let Some(error) = failure {
        tracing::error!("[神医临时设置] {mode_name} 执行失败：{}", error.1);
    }
    tracing::error!("[神医临时设置] {mode_name} 已结束，恢复原配置失败，请检查神医设置");
    false
}
fn settings_finished<T>(mode: &str, outcome: Result<T>) -> Result<T> {
    let mode_name = settings_mode_label(mode);
    match &outcome {
        Ok(_) => tracing::info!("[神医临时设置] {mode_name} 已结束"),
        Err(error) => tracing::warn!("[神医临时设置] {mode_name} 已结束，执行失败：{}", error.1),
    }
    outcome
}
pub async fn with_settings<F, Fut, T>(state: &AppState, mode: &str, function: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _gate = state.strm.gate.lock().await;
    apply_settings(state, mode, discover(state).await?, function).await
}
pub async fn with_optional_preserve<F, Fut, T>(state: &AppState, function: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _gate = state.strm.gate.lock().await;
    let id = match discover(state).await {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(reason=%error.1, "神医忽略文件变更不可用，沿用当前设置刷新");
            return function().await;
        }
    };
    apply_settings(state, "preserve", id, function).await
}
async fn apply_settings<F, Fut, T>(
    state: &AppState,
    mode: &str,
    id: String,
    function: F,
) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let original = read(state, &id).await?;
    let removed: &[&str] = match mode {
        "preserve" => &["CatchAllAllow", "ExtractOnFileChange", "IgnoreFileChange"],
        "capture" => &[
            "CatchAllBlock",
            "ExtractOnFileChange",
            "IgnoreFileChange",
            "CatchAllAllow",
        ],
        _ => return Err(ApiError::bad("未知神医临时设置模式")),
    };
    let mut desired = original.clone();
    let mut values = string(&original, "ExclusiveControlFeatures")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !removed.contains(s))
        .map(str::to_string)
        .collect::<Vec<_>>();
    values.push(
        if mode == "preserve" {
            "IgnoreFileChange"
        } else {
            "CatchAllAllow"
        }
        .into(),
    );
    desired["ExclusiveControlFeatures"] = json!(values.join(","));
    if mode == "capture" {
        desired["EnableImageCapture"] = json!(true);
    }
    let mut restore = json!({});
    for key in ["ExclusiveControlFeatures", "EnableImageCapture"] {
        if desired[key] != original[key] {
            restore[key] = original[key].clone();
        }
    }
    let changed = !restore.as_object().unwrap().is_empty();
    let mode_name = settings_mode_label(mode);
    tracing::info!("[神医临时设置] {mode_name} 开始");
    let outcome = std::panic::AssertUnwindSafe(async {
        if changed {
            write(state, &id, &desired).await?;
            if !matches(&read(state, &id).await?, &desired) {
                return Err(ApiError::bad("神医临时设置回读不一致"));
            }
            tracing::info!("[神医临时设置] {mode_name} 已启用临时配置，结束后恢复原配置");
        } else {
            tracing::info!("[神医临时设置] {mode_name} 原配置已满足要求，无需修改");
        }
        function().await
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|_| Err(ApiError::bad("任务执行异常")));
    if changed && !restore_settings(state, &id, &restore, mode, outcome.as_ref().err()).await {
        return Err(ApiError::bad("神医临时设置恢复失败，请核对独占模式"));
    }
    settings_finished(mode, outcome)
}
pub async fn threads(state: &AppState) -> Result<Value> {
    let id = discover(state).await?;
    let base = id.rsplit_once(':').map_or(id.as_str(), |(base, _)| base);
    let view = ui(
        state,
        Method::GET,
        "UI/View",
        &[
            ("PageId", format!("{base}:Settings")),
            ("ClientLocale", "zh-cn".into()),
        ],
        None,
    )
    .await?;
    let options = &view["EditObjectContainer"]["Object"]["GeneralOptions"];
    let mut result = json!({});
    for (key, field, min, max) in [
        ("main", "MaxConcurrentCount", 1, 20),
        ("secondary", "Tier2MaxConcurrentCount", 1, 20),
        ("cooldown", "CooldownDurationSeconds", 0, 60),
    ] {
        let value = options[field]
            .as_u64()
            .filter(|v| *v >= min && *v <= max)
            .ok_or_else(|| ApiError::bad(format!("神医线程设置 {field} 缺失或无效")))?;
        result[key] = json!(value);
    }
    Ok(result)
}
