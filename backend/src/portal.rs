use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Multipart, State},
    http::HeaderMap,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/portal/account-info", get(account))
        .route("/api/portal/telegram-chat-id", post(chat_id))
        .route("/api/portal/telegram-bot-info", get(bot_info))
        .route(
            "/api/portal/upload-avatar",
            post(avatar).delete(delete_avatar),
        )
        .layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
}
fn user(state: &AppState, headers: &HeaderMap) -> Result<String> {
    Ok(state
        .sessions
        .from_headers(headers)
        .ok_or_else(ApiError::forbidden)?
        .user_id)
}
async fn account(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let id = user(&state, &headers)?;
    let mut row:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',u.id,'name',u.name,'is_administrator',u.is_administrator,'profile_image_tag',u.profile_image_tag,'status',e.status,'registration_date',e.registration_date,'expiration_date',e.expiration_date,'notes',e.notes,'telegram_chat_id',e.telegram_chat_id) FROM emby_users u LEFT JOIN emby_users_extended e ON e.emby_user_id=u.id WHERE u.id=$1")
        .bind(id).fetch_optional(&state.db).await?.unwrap_or(json!({}));
    row["telegram_channel_id"] = state.config.read().await["telegram_channel_id"].clone();
    Ok(Json(row))
}
async fn chat_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let id = user(&state, &headers)?;
    let chat = string(&body, "chat_id").trim().to_string();
    sqlx::query("INSERT INTO emby_users_extended(emby_user_id,telegram_chat_id) VALUES($1,$2) ON CONFLICT(emby_user_id) DO UPDATE SET telegram_chat_id=EXCLUDED.telegram_chat_id")
        .bind(id).bind(if chat.is_empty(){None}else{Some(chat)}).execute(&state.db).await?;
    Ok(Json(
        json!({"status":"ok","message":"Telegram Chat ID 保存成功！"}),
    ))
}
async fn bot_info(State(state): State<AppState>) -> Result<Json<Value>> {
    let token = string(&*state.config.read().await, "telegram_bot_token");
    if token.is_empty() {
        return Ok(Json(json!({"bot_username":null,"error":"Bot Token未配置"})));
    }
    let response = state
        .http
        .get(format!(
            "{}/bot{token}/getMe",
            state.notifications.api_base.trim_end_matches('/')
        ))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(_) => return Ok(Json(json!({"bot_username":null,"error":"网络请求失败"}))),
    };
    if response.status() != axum::http::StatusCode::OK {
        return Ok(Json(
            json!({"bot_username":null,"error":format!("HTTP错误, 状态码: {}",response.status().as_u16())}),
        ));
    }
    let info = match response.json::<Value>().await {
        Ok(info) => info,
        Err(_) => {
            return Ok(Json(
                json!({"bot_username":null,"error":"网络请求失败: 无法解析Telegram响应"}),
            ));
        }
    };
    if info["ok"] == true {
        Ok(Json(json!({"bot_username":info["result"]["username"]})))
    } else {
        Ok(Json(
            json!({"bot_username":null,"error":format!("Telegram API 错误: {}", info["description"].as_str().unwrap_or("未知API错误"))}),
        ))
    }
}
async fn delete_avatar(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let id = user(&state, &headers)?;
    state
        .emby(
            reqwest::Method::DELETE,
            &format!("Users/{id}/Images/Primary"),
            &[],
            None,
        )
        .await?;
    sqlx::query("UPDATE emby_users SET profile_image_tag=NULL,last_updated_at=NOW() WHERE id=$1")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(
        json!({"status":"ok","message":"头像已删除","new_tag":null}),
    ))
}

async fn avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    let id = user(&state, &headers)?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad("上传文件无效"))?
    {
        if field.name() != Some("avatar") {
            continue;
        }
        match field.file_name() {
            None => continue,
            Some("") => return Err(ApiError::bad("未选择文件")),
            Some(_) => (),
        }
        let mime = field.content_type().unwrap_or("image/jpeg").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|_| ApiError::bad("读取头像失败"))?;
        if bytes.len() > 5 * 1024 * 1024 {
            return Err(ApiError::bad("图片大小不能超过 5MB"));
        }
        let config = state.config.read().await;
        let url = string(&config, "emby_server_url");
        let key = string(&config, "emby_api_key");
        drop(config);
        let endpoint = format!("{}/Users/{id}/Images/Primary", url.trim_end_matches('/'));
        let _ = crate::emby::send(
            state
                .http
                .delete(&endpoint)
                .header("X-Emby-Token", &key)
                .header("Content-Type", &mime)
                .timeout(std::time::Duration::from_secs(10)),
        )
        .await;
        crate::emby::send(
            state
                .http
                .post(endpoint)
                .header("X-Emby-Token", key)
                .header("Content-Type", mime)
                .body(STANDARD.encode(bytes)),
        )
        .await?
        .error_for_status()?;
        let details = state
            .emby(reqwest::Method::GET, &format!("Users/{id}"), &[], None)
            .await;
        let mut tag = None;
        match details {
            Ok(details) if details.as_object().is_some_and(|object| !object.is_empty()) => {
                tag = details["PrimaryImageTag"].as_str().map(str::to_string);
                if let Err(error) = sqlx::query(
                    "UPDATE emby_users SET profile_image_tag=$1,last_updated_at=NOW() WHERE id=$2",
                )
                .bind(&tag)
                .bind(&id)
                .execute(&state.db)
                .await
                {
                    tracing::warn!(user_id = %id, %error, "头像已上传，更新本地图片标签失败");
                }
            }
            Err(error) => {
                tracing::warn!(user_id = %id, error = %error.1, "头像已上传，读取图片标签失败")
            }
            _ => (),
        }
        return Ok(Json(
            json!({"status":"ok","message":"头像上传成功","new_tag":tag}),
        ));
    }
    Err(ApiError::bad("未找到文件"))
}
