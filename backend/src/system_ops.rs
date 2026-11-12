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
    let mut token = string(&body, "token");
    if token == "********" {
        token = string(&*state.config.read().await, "telegram_bot_token");
    }
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
    crate::notifications::test_message_draft(&state, token.trim(), &chat_id(&chat), &body).await?;
    Ok((
        StatusCode::OK,
        Json(json!({"success":true,"message":"测试消息发送成功！请检查您的 Telegram。"})),
    ))
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
