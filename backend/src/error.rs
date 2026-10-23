use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);
impl ApiError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn forbidden() -> Self {
        Self(
            StatusCode::FORBIDDEN,
            "需要 Emby 管理员权限才能执行此操作".into(),
        )
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"success":false,"status":"error", "error":self.1, "message":self.1})),
        )
            .into_response()
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "数据库操作失败");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "数据库操作失败，请查看日志".into(),
        )
    }
}
impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        tracing::error!(error = %error.without_url(), "上游服务请求失败");
        Self(StatusCode::BAD_GATEWAY, "无法访问上游服务".into())
    }
}
impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        tracing::error!(%error, "文件操作失败");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "文件操作失败，请查看日志".into(),
        )
    }
}
pub type Result<T> = std::result::Result<T, ApiError>;
