use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/telegram/test", post(telegram))
        .route("/api/system/restart", post(restart))
}
pub fn chat_id(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("https://t.me/")
        && let Some(name) = value.rsplit('/').next().filter(|s| !s.is_empty())
    {
        return format!("@{name}");
    }
    value.into()
}
async fn telegram(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let token = string(&body, "token");
    let chat = match &body["chat_id"] {
        Value::String(value) => value.clone(),
        Value::Number(value) if value.as_i64() != Some(0) => value.to_string(),
        _ => String::new(),
    };
    if token.is_empty() || chat.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"success":false,"message":"缺少 Token 或 Chat ID"})),
        ));
    }
    let text = format!(
        "🔔 *Emby Toolkit 测试消息*\n\n这是一条测试消息，证明您的机器人配置正确。\n⏱ 时间: `{}`",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    let response = state
        .http
        .post(format!(
            "{}/bot{token}/sendMessage",
            state.notifications.api_base.trim_end_matches('/')
        ))
        .timeout(std::time::Duration::from_secs(15))
        .json(&json!({"chat_id":chat_id(&chat),"text":text,"parse_mode":"MarkdownV2","disable_web_page_preview":true}))
        .send()
        .await;
    match response {
        Ok(response) if response.status() == StatusCode::OK => Ok((
            StatusCode::OK,
            Json(json!({"success":true,"message":"测试消息发送成功！请检查您的 Telegram。"})),
        )),
        Ok(response) => {
            let status = response.status();
            let message = response.text().await.unwrap_or_default();
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    json!({"success":false,"message":format!("发送失败 (HTTP {}): {message}",status.as_u16())}),
                ),
            ))
        }
        Err(error) => {
            let message = if error.is_connect() && error.is_timeout() {
                "连接 Telegram 服务器超时。"
            } else {
                "连接 Telegram 服务器失败。"
            };
            tracing::warn!(error=%error.without_url(),"Telegram测试失败");
            Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success":false,"message":message})),
            ))
        }
    }
}
async fn restart(State(state): State<AppState>) -> Result<Json<Value>> {
    let config = state.config.read().await.clone();
    let name = config
        .get("container_name")
        .map(|value| value.as_str().unwrap_or(""))
        .unwrap_or("emby-toolkit");
    if name.trim().is_empty() {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "未在配置中指定容器名称。".into(),
        ));
    }
    let docker = crate::docker_host::client(&state)
        .map_err(|error| ApiError(StatusCode::INTERNAL_SERVER_ERROR, error.1))?;
    match docker.inspect_container(name, None).await {
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                format!("API: 尝试重启容器，但名为 '{name}' 的容器未找到。"),
            ));
        }
        result => {
            result.map_err(restart_error)?;
        }
    }
    docker
        .restart_container(
            name,
            None::<bollard::query_parameters::RestartContainerOptions>,
        )
        .await
        .map_err(restart_error)?;
    Ok(Json(
        json!({"message":format!("已向容器 '{name}' 发送重启指令。应用将在片刻后恢复。")}),
    ))
}

fn restart_error(error: bollard::errors::Error) -> ApiError {
    tracing::error!(%error, "重启容器失败");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Docker操作失败，请查看日志".into(),
    )
}
