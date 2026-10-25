use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::{collections::BTreeSet, error::Error as _, time::Duration};
use tokio_util::sync::CancellationToken;

fn catalog() -> Vec<Value> {
    let value: Value = serde_json::from_str(include_str!("../assets/task_catalog.json")).unwrap();
    list(&value["all"])
}

pub fn commands(config: &Value) -> Value {
    let tasks = catalog();
    let mut allowed: Vec<String> = list(&config["tg_menu_tasks"])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if allowed.is_empty() {
        let defaults: Value =
            serde_json::from_str(include_str!("../assets/config_defaults.json")).unwrap();
        allowed = list(&defaults["tg_menu_tasks"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    for key in [
        "clear-emby-activity-log",
        "repair-truehd-core-shift",
        "restore-truehd-core-backups",
        "clear-truehd-core-backups",
    ] {
        if !allowed.iter().any(|s| s == key) {
            allowed.push(key.into());
        }
    }
    let mut seen = BTreeSet::new();
    let mut result: Vec<Value> = allowed.into_iter().filter(|key| seen.insert(key.clone())).filter_map(|key| {
        tasks.iter().find(|task| task["key"] == key).map(|task| json!({"command":key.replace('-',"_").to_lowercase(),"description":format!("🚀 {}",string(task,"name"))}))
    }).collect();
    result.push(json!({"command":"all_tasks","description":"📋 查看所有可用任务"}));
    json!({"commands":result})
}

async fn call(state: &AppState, token: &str, method: &str, payload: &Value) -> Result<Value> {
    let response = state
        .http
        .post(format!(
            "{}/bot{token}/{method}",
            state.notifications.api_base
        ))
        .timeout(Duration::from_secs(15))
        .json(payload)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError(status, format!("Telegram {method} 请求失败")));
    }
    let response: Value = response.json().await?;
    if response["ok"] != true {
        return Err(ApiError(
            StatusCode::BAD_GATEWAY,
            format!("Telegram {method} 未确认成功"),
        ));
    }
    Ok(response)
}

async fn send(
    state: &AppState,
    token: &str,
    chat: &str,
    text: &str,
    keyboard: Option<Value>,
) -> Result<()> {
    let mut payload = json!({"chat_id":chat,"text":crate::notifications::escape(text),"parse_mode":"MarkdownV2","disable_web_page_preview":true,"disable_notification":false});
    if let Some(keyboard) = keyboard {
        payload["reply_markup"] = keyboard;
    }
    call(state, token, "sendMessage", &payload).await?;
    Ok(())
}

fn id(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
        .unwrap_or_default()
}

async fn execute(state: &AppState, token: &str, chat: &str, key: &str) -> Result<()> {
    let message = if !catalog().iter().any(|task| task["key"] == key) {
        "❌ 任务不存在或已失效。".into()
    } else if !state.configured().await {
        "❌ 核心处理器未初始化。".into()
    } else {
        match crate::tasks::submit_task(state, key, false).await {
            Ok(result) => format!(
                "🚀 {}\n请在任务中心或日志中查看进度。",
                string(&result, "message")
            ),
            Err(error) if error.0 == StatusCode::CONFLICT => {
                "⚠️ 当前已有后台任务正在运行，请稍后再试。".into()
            }
            Err(error) => format!("❌ {}", error.1),
        }
    };
    send(state, token, chat, &message, None).await
}

pub async fn handle_update(state: &AppState, token: &str, update: &Value) -> Result<()> {
    let admins: BTreeSet<String> = sqlx::query_scalar::<_,String>("SELECT e.telegram_chat_id FROM emby_users u JOIN emby_users_extended e ON u.id=e.emby_user_id WHERE u.is_administrator=true AND e.telegram_chat_id IS NOT NULL AND e.telegram_chat_id!=''")
        .fetch_all(&state.db).await?.into_iter().collect();
    if let Some(callback) = update.get("callback_query") {
        let requester = id(&callback["from"]["id"]);
        if !admins.contains(&requester) {
            tracing::warn!("[TG交互] 未授权的回调请求已拒绝");
            return Ok(());
        }
        let query = string(callback, "id");
        if !query.is_empty()
            && let Err(error) = call(
                state,
                token,
                "answerCallbackQuery",
                &json!({"callback_query_id":query}),
            )
            .await
        {
            tracing::debug!("Telegram 回调确认失败：{}", error.1);
        }
        if let Some(key) = string(callback, "data").strip_prefix("run_task_") {
            let chat = id(&callback["message"]["chat"]["id"]);
            return execute(
                state,
                token,
                if chat.is_empty() { &requester } else { &chat },
                key,
            )
            .await;
        }
    } else if let Some(message) = update.get("message") {
        let chat = id(&message["chat"]["id"]);
        let requester = id(&message["from"]["id"]);
        let text = message["text"]
            .as_str()
            .or_else(|| message["caption"].as_str())
            .unwrap_or("")
            .trim();
        if chat.is_empty() || text.is_empty() {
            return Ok(());
        }
        if !admins.contains(&chat) && !admins.contains(&requester) {
            tracing::warn!("[TG交互] 未授权聊天消息已忽略");
            return Ok(());
        }
        let Some(command) = text
            .strip_prefix('/')
            .and_then(|s| s.split_whitespace().next())
            .map(|s| s.split('@').next().unwrap_or("").to_lowercase())
        else {
            return Ok(());
        };
        if ["all_tasks", "tasks", "menu"].contains(&command.as_str()) {
            let buttons = catalog().into_iter().map(|task| json!({"text":task["name"],"callback_data":format!("run_task_{}",string(&task,"key"))})).collect::<Vec<_>>();
            let rows = buttons
                .chunks(2)
                .map(|row| row.to_vec())
                .collect::<Vec<_>>();
            return send(
                state,
                token,
                &chat,
                "📋 *所有可用任务列表*\n请点击下方按钮执行对应任务：",
                Some(json!({"inline_keyboard":rows})),
            )
            .await;
        }
        if let Some(task) = catalog()
            .iter()
            .find(|task| string(task, "key").replace('-', "_").to_lowercase() == command)
        {
            return execute(state, token, &chat, &string(task, "key")).await;
        }
    }
    Ok(())
}

async fn changed(state: &AppState, token: &str) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if string(&*state.config.read().await, "telegram_bot_token") != token {
            return;
        }
    }
}

fn diagnostic(token: &str, text: &str) -> String {
    crate::logs::redact(&text.replace(token, "********"))
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(500)
        .collect()
}

fn network_diagnostic(token: &str, error: reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "超时"
    } else if error.is_connect() {
        "连接失败"
    } else {
        "请求或响应读取失败"
    };
    let error = error.without_url();
    let mut detail = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        detail.push_str(&format!("：{cause}"));
        source = cause.source();
    }
    format!("网络{kind}：{}", diagnostic(token, &detail))
}

struct PollFailure {
    message: String,
    stop: bool,
    retry_after: Option<u64>,
}

fn response_failure(status: StatusCode, body: &Value, detail: &str, token: &str) -> PollFailure {
    let code = body["error_code"].as_u64();
    let hint = if status == StatusCode::CONFLICT || code == Some(409) {
        "；请检查是否有其他 Bot 实例同时轮询，或是否设置了 webhook"
    } else {
        ""
    };
    PollFailure {
        message: format!(
            "HTTP {}{}：{}{hint}",
            status.as_u16(),
            code.map(|code| format!("，Telegram error_code {code}"))
                .unwrap_or_default(),
            diagnostic(token, detail),
        ),
        stop: matches!(status.as_u16(), 401 | 404) || matches!(code, Some(401 | 404)),
        retry_after: if status == StatusCode::TOO_MANY_REQUESTS || code == Some(429) {
            body["parameters"]["retry_after"]
                .as_u64()
                .filter(|seconds| *seconds > 0)
        } else {
            None
        },
    }
}

async fn poll_response(
    response: std::result::Result<reqwest::Response, reqwest::Error>,
    token: &str,
) -> std::result::Result<Value, PollFailure> {
    let response = response.map_err(|error| PollFailure {
        message: network_diagnostic(token, error),
        stop: false,
        retry_after: None,
    })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| {
        response_failure(
            status,
            &Value::Null,
            &network_diagnostic(token, error),
            token,
        )
    })?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|error| {
        response_failure(
            status,
            &Value::Null,
            &format!(
                "响应不是有效 JSON（{error}）：{}",
                String::from_utf8_lossy(&bytes)
            ),
            token,
        )
    })?;
    if status.is_success() && body["ok"] == true && body["result"].is_array() {
        Ok(body)
    } else {
        Err(response_failure(
            status,
            &body,
            body["description"]
                .as_str()
                .unwrap_or("响应未确认成功或更新列表无效"),
            token,
        ))
    }
}

fn retry_delay(failures: u32) -> u64 {
    (5_u64 << failures.saturating_sub(1).min(4)).min(60)
}

pub async fn run(state: AppState, cancel: CancellationToken) {
    let mut active_token = String::new();
    let mut stopped_token = String::new();
    let mut menu = Value::Null;
    let mut offset: Option<i64> = None;
    let mut failures: u32 = 0;
    let mut listening = false;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let config = state.config.read().await.clone();
        let token = string(&config, "telegram_bot_token");
        if token != active_token {
            if listening {
                let reason = if token.is_empty() {
                    "Bot Token 配置已清空"
                } else {
                    "Bot Token 配置已切换"
                };
                tracing::info!("[TG交互] Telegram 交互监听已停止：{reason}");
                listening = false;
            }
            active_token = token.clone();
            stopped_token.clear();
            offset = None;
            menu = Value::Null;
            failures = 0;
        }
        if token.is_empty() || token == stopped_token {
            tokio::select! {_ = cancel.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(1))=>{}}
            continue;
        }
        let next_menu = commands(&config);
        if menu != next_menu {
            let result = tokio::select! {_ = cancel.cancelled()=>break,_ = changed(&state,&token)=>continue,result = call(&state,&token,"setMyCommands",&next_menu)=>result};
            if let Err(error) = result {
                tracing::warn!("Telegram 菜单注册失败：{}", error.1);
            } else {
                menu = next_menu;
            }
        }
        let mut request = state
            .http
            .request(
                Method::GET,
                format!("{}/bot{token}/getUpdates", state.notifications.api_base),
            )
            .timeout(Duration::from_secs(40))
            .query(&[
                ("timeout", "30"),
                ("allowed_updates", "[\"message\",\"callback_query\"]"),
            ]);
        if let Some(offset) = offset {
            request = request.query(&[("offset", offset)]);
        }
        let response = tokio::select! {_ = cancel.cancelled()=>break,_ = changed(&state,&token)=>continue,result = async {
            if !listening {
                tracing::info!("[TG交互] Telegram 交互监听已启用");
                listening = true;
            }
            request.send().await
        }=>result};
        let response = tokio::select! {_ = cancel.cancelled()=>break,_ = changed(&state,&token)=>continue,result = poll_response(response,&token)=>result};
        let mut delay = 1;
        match response {
            Ok(body) => {
                if failures > 0 {
                    tracing::info!("[TG交互] Telegram 轮询已恢复");
                }
                failures = 0;
                for update in list(&body["result"]) {
                    if cancel.is_cancelled()
                        || string(&*state.config.read().await, "telegram_bot_token") != token
                    {
                        break;
                    }
                    if let Some(number) = update["update_id"].as_i64() {
                        offset = Some(number.saturating_add(1));
                    }
                    if let Err(error) = handle_update(&state, &token, &update).await {
                        tracing::warn!("Telegram 交互处理失败：{}", error.1);
                    }
                }
            }
            Err(error) if error.stop => {
                tracing::error!(
                    "[TG交互] Telegram Bot Token 无效，交互监听已停止：{}",
                    error.message
                );
                listening = false;
                stopped_token = token;
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                delay = error.retry_after.unwrap_or_else(|| retry_delay(failures));
                tracing::warn!(
                    "[TG交互] Telegram 轮询失败：{}；连续失败 {} 次，{} 秒后重试",
                    error.message,
                    failures,
                    delay
                );
            }
        }
        tokio::select! {_ = cancel.cancelled()=>break,_ = changed(&state,&active_token)=>{},_ = tokio::time::sleep(Duration::from_secs(delay))=>{}}
    }
    if listening {
        tracing::info!("[TG交互] Telegram 交互监听已停止：服务关闭");
    }
}
