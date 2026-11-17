use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use futures_util::FutureExt;
use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, future::Future, sync::Arc};
use tokio::sync::Mutex;

mod settings_recovery;
pub use settings_recovery::{recover_pending, run_recovery};

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
pub(crate) async fn restore_thumbnail_encoder(
    state: &AppState,
    original: &Value,
    applied: &Value,
) -> Result<()> {
    let valid = |value: &Value| {
        value.as_object().is_some_and(|fields| {
            fields.len() == 1
                && fields
                    .get("CustomEncoderPath")
                    .is_some_and(Value::is_string)
        })
    };
    if !valid(original) || !valid(applied) {
        return Err(ApiError::bad("缩略图编码器恢复记录无效"));
    }
    let _gate = state.strm.gate.lock().await;
    let id = discover(state).await?;
    check_rename_scan_guard(state, &id, None).await?;
    let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
    if !tasks.as_array().is_some_and(|tasks| {
        tasks
            .iter()
            .any(|task| task["Key"] == "VideoThumbnailExtractTask" && task["State"] == "Idle")
    }) {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "缩略图原生任务尚未确认结束，保留临时编码器供重启后继续".into(),
        ));
    }
    if !restore_settings(state, &id, original, Some(applied), "自定义配置", None).await {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "缩略图编码器恢复未通过回读验收".into(),
        ));
    }
    Ok(())
}

pub(crate) fn media_tasks_idle(tasks: &Value) -> bool {
    tasks.as_array().is_some_and(|tasks| {
        tasks
            .iter()
            .any(|task| task["Key"] == "MediaInfoExtractTask" && task["State"] == "Idle")
            && !tasks.iter().any(|task| {
                matches!(
                    task["Key"].as_str(),
                    Some(
                        "MediaInfoExtractTask"
                            | "IntroFingerprintExtractTask"
                            | "VideoThumbnailExtractTask"
                    )
                ) && task["State"] != "Idle"
            })
    })
}
pub async fn with_changes<F, Fut, T>(state: &AppState, changes: &Value, function: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _gate = state.strm.gate.lock().await;
    settings_recovery::recover_locked(state).await?;
    let id = discover(state).await?;
    check_rename_scan_guard(state, &id, None).await?;
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
    let mut settings_lease = settings_recovery::Lease::begin(state, &id, &restore, &desired).await?;
    tracing::info!("[神医临时设置] {mode_name} 开始");
    let mut applied = false;
    let outcome = std::panic::AssertUnwindSafe(async {
        if changed {
            write(state, &id, &desired).await?;
            applied = true;
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
    let restoring_media = changes
        .get("PersistMediaInfoMode")
        .is_some_and(|mode| mode == "Restore");
    let replacing_encoder = restore.get("CustomEncoderPath").is_some();
    if applied && (restoring_media || replacing_encoder) {
        let mut stopped = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let tasks = tokio::time::timeout_at(
                deadline,
                state.emby(Method::GET, "ScheduledTasks", &[], None),
            )
            .await;
            if let Ok(Ok(tasks)) = tasks
                && if restoring_media {
                    media_tasks_idle(&tasks)
                } else {
                    tasks.as_array().is_some_and(|tasks| {
                        tasks
                            .iter()
                            .find(|task| task["Key"] == "VideoThumbnailExtractTask")
                            .is_some_and(|task| task["State"] == "Idle")
                    })
                }
            {
                stopped = true;
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(
                std::time::Duration::from_secs(2)
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            )
            .await;
        }
        if !stopped {
            let message = if restoring_media {
                "未确认神医媒体信息任务结束，原设置还原已持久登记，任务结束后自动重试"
            } else {
                "未确认神医缩略图任务结束，原编码器还原已持久登记，任务结束后自动重试"
            };
            tracing::error!("[神医临时设置] {message}");
            return Err(ApiError::bad(message));
        }
    }
    settings_lease.restore().await?;
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
    if !object.is_object() || object.get("ExclusiveControlFeatures").is_none() {
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
pub(crate) fn temporary_features(original: &Value, mode: &str) -> Result<Value> {
    let removed: &[&str] = match mode {
        "preserve" | "restore_media_info" => {
            &["CatchAllAllow", "ExtractOnFileChange", "IgnoreFileChange"]
        }
        "ignore_file_change" => &["ExtractOnFileChange", "IgnoreFileChange"],
        "capture" => &[
            "CatchAllBlock",
            "ExtractOnFileChange",
            "IgnoreFileChange",
            "CatchAllAllow",
        ],
        _ => return Err(ApiError::bad("未知神医临时设置模式")),
    };
    let mut values = string(original, "ExclusiveControlFeatures")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty() && !removed.contains(value))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    values.push(
        if mode == "capture" {
            "CatchAllAllow"
        } else {
            "IgnoreFileChange"
        }
        .into(),
    );
    Ok(json!(values.join(",")))
}
pub(crate) fn matches(actual: &Value, expected: &Value) -> bool {
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
        "preserve" | "ignore_file_change" => "忽略文件变更",
        "restore_media_info" => "仅从 JSON 重载媒体信息",
        "capture" => "图片捕获",
        _ => mode,
    }
}
async fn restore_settings(
    state: &AppState,
    id: &str,
    restore: &Value,
    applied: Option<&Value>,
    mode: &str,
    failure: Option<&ApiError>,
) -> bool {
    let mode_name = settings_mode_label(mode);
    tracing::info!("[神医临时设置] {mode_name} 开始恢复原配置");
    for attempt in 1..=3 {
        let result = async {
            let current = read(state, id).await?;
            let mut restored = current.clone();
            for (key, original) in restore.as_object().unwrap() {
                let Some(expected) = applied.and_then(|applied| applied.get(key)) else {
                    // Older recovery records did not retain the applied values.
                    // Keep their explicit restore-original contract unchanged.
                    if applied.is_none() {
                        restored[key] = original.clone();
                    }
                    continue;
                };
                if key == "ExclusiveControlFeatures" {
                    let original_features = features(original);
                    let applied_features = features(expected);
                    let current_features = features(&current[key]);
                    let mut merged = current_features.clone();
                    for added in applied_features.difference(&original_features) {
                        merged.remove(added);
                    }
                    for removed in original_features.difference(&applied_features) {
                        merged.insert(removed.clone());
                    }
                    if merged != current_features {
                        restored[key] = if merged == original_features {
                            original.clone()
                        } else {
                            let mut values = merged.into_iter().collect::<Vec<_>>();
                            values.sort();
                            json!(values.join(","))
                        };
                    }
                } else if current[key] == *expected {
                    restored[key] = original.clone();
                } else if current[key] != *original {
                    tracing::warn!(
                        field = key,
                        "[神医临时设置] 配置已由其他操作修改，保留当前值"
                    );
                }
            }
            if matches(&current, &restored) {
                return Ok(());
            }
            write(state, id, &restored).await?;
            if !matches(&read(state, id).await?, &restored) {
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

pub(crate) fn valid_recovery_settings(original: &Value) -> bool {
    original.as_object().is_some_and(|fields| {
        !fields.is_empty()
            && fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "ExclusiveControlFeatures" | "PersistMediaInfoMode" | "EnableImageCapture"
                )
            })
            && fields
                .get("PersistMediaInfoMode")
                .is_none_or(Value::is_string)
            && fields
                .get("EnableImageCapture")
                .is_none_or(Value::is_boolean)
            && fields
                .get("ExclusiveControlFeatures")
                .is_some_and(|v| v.is_string() || v.is_null())
    })
}

pub(crate) async fn recovery_applied_settings(
    recovery: &std::path::Path,
    original: &Value,
) -> Result<Option<Value>> {
    let path = recovery.join("applied-settings.json");
    if !tokio::fs::try_exists(&path).await? {
        return Ok(None);
    }
    let applied = crate::media_recovery::read_json(&path).await?;
    if !applied.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| {
            original.get(key).is_some()
                && match key.as_str() {
                    "ExclusiveControlFeatures" | "PersistMediaInfoMode" => {
                        value.is_string() || (key == "ExclusiveControlFeatures" && value.is_null())
                    }
                    "EnableImageCapture" => value.is_boolean(),
                    _ => false,
                }
        })
    }) {
        return Err(ApiError::bad("本任务临时配置恢复说明无效，未修改神医配置"));
    }
    let modes: &[&str] = if original.get("EnableImageCapture").is_some() {
        &["capture"]
    } else if original.get("PersistMediaInfoMode").is_some() {
        &["restore_media_info"]
    } else {
        &["preserve", "ignore_file_change"]
    };
    let candidates = modes
        .iter()
        .map(|mode| temporary_features(original, mode))
        .collect::<Result<Vec<_>>>()?;
    let invalid_features = if let Some(value) = applied.get("ExclusiveControlFeatures") {
        !candidates
            .iter()
            .any(|expected| features(expected) == features(value))
    } else {
        !candidates
            .iter()
            .any(|expected| features(expected) == features(&original["ExclusiveControlFeatures"]))
    };
    let invalid_scalar = [
        ("PersistMediaInfoMode", json!("Restore")),
        ("EnableImageCapture", json!(true)),
    ]
    .iter()
    .any(|(key, expected)| {
        original.get(key).is_some_and(|original| {
            applied
                .get(key)
                .map_or(original != expected, |value| value != expected)
        })
    });
    if invalid_features || invalid_scalar {
        return Err(ApiError::bad(
            "本任务临时配置与原恢复说明不一致，未修改神医配置",
        ));
    }
    Ok(Some(applied))
}

pub(crate) async fn finish_media_recovery<F, Fut>(
    state: &AppState,
    original: &Value,
    applied: Option<&Value>,
    function: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    finish_media_recovery_inner(state, original, applied, false, function).await
}

pub(crate) async fn finish_media_recovery_rearmed<F, Fut>(
    state: &AppState,
    original: &Value,
    applied: Option<&Value>,
    function: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    finish_media_recovery_inner(state, original, applied, true, function).await
}

pub(crate) async fn restore_media_save_settings(state: &AppState, original: &Value, applied: &Value) -> Result<()> {
    let valid = |settings: &Value| settings.as_object().is_some_and(|fields| !fields.is_empty() && fields.iter().all(|(key, value)| {
        match key.as_str() {
            "PersistMediaInfoMode" | "LibraryScope" => value.is_string(),
            "IncludeExtra" => value.is_boolean(),
            _ => false,
        }
    }));
    if !valid(original) || !valid(applied) || original.as_object().unwrap().keys().ne(applied.as_object().unwrap().keys()) {
        return Err(ApiError::bad("原媒体保存配置说明无效，未修改当前配置"));
    }
    let _gate = state.strm.gate.lock().await;
    let id = discover(state).await?;
    if !media_tasks_idle(&state.emby(Method::GET, "ScheduledTasks", &[], None).await?) {
        return Err(ApiError::bad("原媒体恢复计划任务尚未结束，保留原恢复范围"));
    }
    if !restore_settings(state, &id, original, Some(applied), "preserve", None).await {
        return Err(ApiError::bad("原媒体保存配置恢复未通过回读验收"));
    }
    Ok(())
}

async fn finish_media_recovery_inner<F, Fut>(
    state: &AppState,
    original: &Value,
    applied: Option<&Value>,
    rearm: bool,
    function: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let _gate = state.strm.gate.lock().await;
    settings_recovery::recover_locked(state).await?;
    if !valid_recovery_settings(original) {
        return Err(ApiError::bad("原配置恢复说明无效，未修改神医配置"));
    }
    let id = discover(state).await?;
    let tasks = state.emby(Method::GET, "ScheduledTasks", &[], None).await?;
    if !media_tasks_idle(&tasks) {
        return Err(ApiError::bad("神医媒体任务尚未结束，请结束后重试恢复"));
    }
    let desired = match applied {
        Some(applied) => applied.clone(),
        None => read(state, &id).await?,
    };
    let original_fields = original.as_object().unwrap().iter()
        .filter(|(key, _)| applied.is_none_or(|applied| applied.get(*key).is_some()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let mut rearmed = None;
    if rearm && let Some(applied) = applied {
        let fields = applied.as_object().ok_or_else(|| ApiError::bad("原临时配置恢复说明无效"))?;
        let current = read(state, &id).await?;
        let mut desired = current.clone();
        for (key, value) in fields {
            if original.get(key).is_none() || !matches!(key.as_str(), "ExclusiveControlFeatures" | "PersistMediaInfoMode" | "EnableImageCapture")
                || (key == "ExclusiveControlFeatures" && !value.is_string() && !value.is_null())
                || (key == "PersistMediaInfoMode" && value != "Restore")
                || (key == "EnableImageCapture" && value != true)
            {
                return Err(ApiError::bad("原临时配置包含未授权字段，未重新启用"));
            }
            if key == "ExclusiveControlFeatures" {
                let before = features(&original[key]);
                let after = features(value);
                let mut merged = features(&current[key]);
                for removed in before.difference(&after) { merged.remove(removed); }
                for added in after.difference(&before) { merged.insert(added.clone()); }
                desired[key] = if merged == after { value.clone() } else {
                    let mut merged = merged.into_iter().collect::<Vec<_>>();
                    merged.sort();
                    json!(merged.join(","))
                };
            } else if current[key] == original[key] || current[key] == *value {
                desired[key] = value.clone();
            } else {
                return Err(ApiError::bad("恢复配置在任务中断后已被修改，未覆盖当前设置"));
            }
        }
        if !matches(&current, &desired) {
            rearmed = Some(desired);
        }
    }
    let mut settings_lease = settings_recovery::Lease::begin(
        state, &id, &Value::Object(original_fields), &desired,
    ).await?;
    let outcome = std::panic::AssertUnwindSafe(async {
        if let Some(desired) = rearmed {
            write(state, &id, &desired).await?;
            if !matches(&read(state, &id).await?, &desired) {
                return Err(ApiError::bad("重启后原临时配置启用未通过回读验收"));
            }
        }
        function().await
    }).catch_unwind().await
        .unwrap_or_else(|_| Err(ApiError::bad("媒体恢复执行异常")));
    settings_lease.restore().await?;
    outcome
}

fn settings_finished<T>(mode: &str, outcome: Result<T>) -> Result<T> {
    let mode_name = settings_mode_label(mode);
    match &outcome {
        Ok(_) => tracing::info!("[神医临时设置] {mode_name} 已结束"),
        Err(error) => tracing::warn!("[神医临时设置] {mode_name} 已结束，执行失败：{}", error.1),
    }
    outcome
}
async fn check_rename_scan_guard(
    state: &AppState,
    id: &str,
    current: Option<&std::path::Path>,
) -> Result<()> {
    let root = state.data_dir.join("backups/media-info-reload");
    let blocked = |reason: String| {
        ApiError(
            axum::http::StatusCode::CONFLICT,
            format!(
                "前次媒体刷新尚未完成验收，暂不修改神医配置；{reason}，恢复说明：{}",
                root.display()
            ),
        )
    };
    let mut pending = vec![];
    for directory in [
        &root,
        &state.data_dir.join("rename-recovery"),
        &state.data_dir.join("backups/media-info-wash"),
    ] {
        let mut entries = match tokio::fs::read_dir(directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(blocked(format!("无法读取恢复记录：{error}"))),
        };
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| blocked(format!("无法读取恢复目录：{error}")))?
        {
            if current == Some(entry.path().as_path())
                && !tokio::fs::try_exists(entry.path().join("original-settings.json")).await?
                && !tokio::fs::try_exists(entry.path().join("applied-settings.json")).await?
                && !tokio::fs::try_exists(entry.path().join("pending-scan.json")).await?
                && !tokio::fs::try_exists(entry.path().join("image-capture.json")).await?
                && tokio::fs::try_exists(entry.path().join("media-reload.json")).await?
            {
                let prepared =
                    crate::media_recovery::read_json(&entry.path().join("media-reload.json"))
                        .await?;
                let active = state.jobs.status().await;
                let config = state.config.read().await;
                let connection = json!([
                    config["emby_server_url"],
                    config["emby_api_key"],
                    config["emby_user_id"]
                ]);
                let fingerprint =
                    format!("{:x}", Sha256::digest(connection.to_string().as_bytes()));
                let mut native_intent = false;
                let mut children = tokio::fs::read_dir(entry.path()).await?;
                while let Some(child) = children.next_entry().await? {
                    native_intent |= child
                        .file_name()
                        .to_string_lossy()
                        .starts_with("clear-intent-");
                }
                if prepared["preparation_kind"] == "ingest-resource-guard"
                    && prepared["owner_task_id"]
                        .as_str()
                        .is_some_and(|owner| !owner.is_empty() && active["task_id"] == owner)
                    && prepared["connection_sha256"] == fingerprint
                    && !native_intent
                {
                    // Only the active owner's exact, still read-only preparation
                    // is exempt; every previous/submitted guard remains enforced.
                    continue;
                }
            }
            if tokio::fs::try_exists(entry.path().join("media-reload.json")).await? {
                return Err(blocked(format!(
                    "媒体信息重载尚未完成，请先核对恢复目录 {}",
                    entry.path().display()
                )));
            }
            if tokio::fs::try_exists(entry.path().join("image-capture.json")).await? {
                return Err(blocked(format!(
                    "截图刷新或媒体信息恢复尚未完成，请先核对恢复目录 {}",
                    entry.path().display()
                )));
            }
            let path = entry.path().join("pending-scan.json");
            if tokio::fs::try_exists(&path)
                .await
                .map_err(|error| blocked(format!("无法核对待确认标记：{error}")))?
            {
                pending.push(path);
            }
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    if pending.len() != 1 {
        return Err(blocked("存在多个待确认刷新记录，请核对各恢复目录".into()));
    }
    let path = &pending[0];
    let record: Value = serde_json::from_slice(
        &tokio::fs::read(path)
            .await
            .map_err(|error| blocked(format!("无法读取待确认标记：{error}")))?,
    )
    .map_err(|_| blocked("待确认标记内容无效".into()))?;
    let ids = record["ids"]
        .as_array()
        .and_then(|ids| {
            ids.iter()
                .map(|id| id.as_str().filter(|id| !id.is_empty()).map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| blocked("未记录有效的本次扫描目录，无法自动验收".into()))?;
    if record["kind"] == "native-sync" {
        if record["verified"] != true {
            return Err(blocked(
                "目标同步尚未记录有效返回和完整回读，无法自动恢复配置".into(),
            ));
        }
    } else {
        let position = record["position"]
            .as_u64()
            .ok_or_else(|| blocked("待确认标记缺少本次日志位置".into()))?;
        let logs = state
            .emby(
                Method::GET,
                "System/Logs/embyserver.txt/Lines",
                &[("Limit", "500".into())],
                None,
            )
            .await
            .map_err(|error| blocked(format!("无法回读本次扫描日志：{}", error.1)))?;
        for id in ids {
            if !crate::media_extra::RefreshTrace::new(position)
                .feed(&id, &logs)
                .map_err(|error| blocked(error.1))?
            {
                return Err(blocked(format!("尚未确认目录 {id} 的本次扫描结束")));
            }
        }
    }
    let recovery = path.parent().unwrap();
    let delete_snapshot =
        tokio::fs::try_exists(recovery.join("version-delete-snapshot.json")).await?;
    if delete_snapshot {
        crate::media_extra::restore_version_delete_snapshot(recovery)
            .await
            .map_err(|error| blocked(error.1))?;
    }
    let original: Value = serde_json::from_slice(
        &tokio::fs::read(recovery.join("original-settings.json"))
            .await
            .map_err(|error| blocked(format!("无法读取原配置：{error}")))?,
    )
    .map_err(|_| blocked("原配置恢复说明无效".into()))?;
    if !original.as_object().is_some_and(|fields| {
        !fields.is_empty()
            && fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "ExclusiveControlFeatures" | "PersistMediaInfoMode" | "EnableImageCapture"
                )
            })
            && fields
                .get("PersistMediaInfoMode")
                .is_none_or(Value::is_string)
            && fields
                .get("EnableImageCapture")
                .is_none_or(Value::is_boolean)
            && fields
                .get("ExclusiveControlFeatures")
                .is_some_and(|value| value.is_string() || value.is_null())
    }) {
        return Err(blocked("原配置恢复说明缺少有效独占模式".into()));
    }
    let applied = recovery_applied_settings(recovery, &original)
        .await
        .map_err(|error| blocked(error.1))?;
    if !restore_settings(state, id, &original, applied.as_ref(), "preserve", None).await {
        return Err(blocked("本次扫描已结束，但神医原配置恢复尚未确认".into()));
    }
    tokio::fs::remove_file(path).await.map_err(|error| {
        blocked(format!(
            "扫描已结束且原配置已恢复，但待确认标记清理失败：{error}"
        ))
    })?;
    if delete_snapshot {
        let stage = state.data_dir.join(recovery.file_name().unwrap());
        let cleanup = async {
            tokio::fs::remove_dir_all(stage).await?;
            tokio::fs::remove_dir_all(recovery).await
        }
        .await;
        if let Err(error) = cleanup {
            tracing::warn!("[神医临时设置] 版本删除已验收并恢复配置，备份清理失败：{error}");
        }
    }
    tracing::info!("[神医临时设置] 前次媒体刷新已完成验收并恢复原配置，允许继续处理");
    Ok(())
}

pub async fn with_settings<F, Fut, T>(state: &AppState, mode: &str, function: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _gate = state.strm.gate.lock().await;
    apply_settings(state, mode, discover(state).await?, function, None).await
}
pub(crate) async fn with_settings_guarded<F, Fut, T>(
    state: &AppState,
    mode: &str,
    function: F,
    confirmed: &std::sync::atomic::AtomicBool,
    recovery: &std::path::Path,
) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _gate = state.strm.gate.lock().await;
    apply_settings(
        state,
        mode,
        discover(state).await?,
        function,
        Some((confirmed, recovery)),
    )
    .await
}
async fn apply_settings<F, Fut, T>(
    state: &AppState,
    mode: &str,
    id: String,
    function: F,
    guard: Option<(&std::sync::atomic::AtomicBool, &std::path::Path)>,
) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    settings_recovery::recover_locked(state).await?;
    check_rename_scan_guard(state, &id, guard.map(|(_, directory)| directory)).await?;
    let original = read(state, &id).await?;
    if mode == "restore_media_info"
        && !media_tasks_idle(&state.emby(Method::GET, "ScheduledTasks", &[], None).await?)
    {
        return Err(ApiError::bad(
            "神医媒体信息、指纹或缩略图任务尚未结束，未清理媒体信息",
        ));
    }
    let mut desired = original.clone();
    desired["ExclusiveControlFeatures"] = temporary_features(&original, mode)?;
    if mode == "capture" {
        if !original["EnableImageCapture"].is_boolean() {
            return Err(ApiError::bad("神医截图开关配置无效，未开始截图"));
        }
        desired["EnableImageCapture"] = json!(true);
    }
    if mode == "restore_media_info" {
        if !original["PersistMediaInfoMode"].is_string() {
            return Err(ApiError::bad("神医恢复模式配置无效，未清理媒体信息"));
        }
        desired["PersistMediaInfoMode"] = json!("Restore");
    }
    let mut restore = json!({});
    if features(&desired["ExclusiveControlFeatures"])
        != features(&original["ExclusiveControlFeatures"])
    {
        restore["ExclusiveControlFeatures"] = original["ExclusiveControlFeatures"].clone();
    }
    if mode == "restore_media_info"
        && desired["PersistMediaInfoMode"] != original["PersistMediaInfoMode"]
    {
        restore["PersistMediaInfoMode"] = original["PersistMediaInfoMode"].clone();
    }
    if mode == "capture" && desired["EnableImageCapture"] != original["EnableImageCapture"] {
        restore["EnableImageCapture"] = original["EnableImageCapture"].clone();
    }
    let changed = !restore.as_object().unwrap().is_empty();
    if let Some((_, recovery)) = guard {
        use tokio::io::AsyncWriteExt;
        tokio::fs::create_dir_all(recovery).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(recovery, std::fs::Permissions::from_mode(0o700)).await?;
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(recovery.join("original-settings.json"))
            .await?;
        let mut backup = json!({"ExclusiveControlFeatures":original["ExclusiveControlFeatures"]});
        if mode == "restore_media_info" {
            backup["PersistMediaInfoMode"] = original["PersistMediaInfoMode"].clone();
        }
        if mode == "capture" {
            backup["EnableImageCapture"] = original["EnableImageCapture"].clone();
        }
        file.write_all(&serde_json::to_vec_pretty(&backup).unwrap())
            .await?;
        file.sync_all().await?;
        let applied = restore
            .as_object()
            .unwrap()
            .keys()
            .map(|key| (key.clone(), desired[key].clone()))
            .collect::<serde_json::Map<_, _>>();
        let applied_path = recovery.join("applied-settings.json");
        tokio::fs::write(&applied_path, serde_json::to_vec(&applied).unwrap()).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&applied_path, std::fs::Permissions::from_mode(0o600))
                .await?;
        }
        tokio::fs::File::open(&applied_path)
            .await?
            .sync_all()
            .await?;
        tokio::fs::File::open(recovery).await?.sync_all().await?;
    }
    let mode_name = settings_mode_label(mode);
    let mut settings_lease = settings_recovery::Lease::begin(state, &id, &restore, &desired).await?;
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
    // Configuration rollback is independent of whether media validation passed.
    // Failed rollback keeps the durable journal and is retried outside this job.
    settings_lease.restore().await?;
    if let Some((confirmed, recovery)) = guard
        && !confirmed.load(std::sync::atomic::Ordering::Acquire)
    {
        tracing::error!(
            "[神医临时设置] 原配置已还原，媒体刷新尚未验收，恢复材料位于 {}",
            recovery.display()
        );
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            format!(
                "{}媒体刷新尚未验收；神医原设置已还原，恢复说明：{}",
                outcome
                    .as_ref()
                    .err()
                    .map(|error| format!("{}；", error.1))
                    .unwrap_or_default(),
                recovery.display()
            ),
        ));
    }
    if let Some((_, recovery)) = guard {
        let pending = recovery.join("pending-scan.json");
        match tokio::fs::remove_file(&pending).await {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => {
                return Err(ApiError::bad(format!(
                    "媒体刷新已确认且神医原配置已恢复，但待确认标记清理失败：{error}，恢复说明：{}",
                    recovery.display()
                )));
            }
        }
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
