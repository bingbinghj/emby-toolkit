use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/portal/account-info", get(account))
        .route("/api/portal/users", get(users))
        .route("/api/portal/users/{id}", post(save_user_details))
        .route("/api/portal/users/{id}/password", post(save_user_password))
        .route("/api/portal/telegram-chat-id", post(chat_id))
        .route("/api/portal/telegram-bot-info", get(bot_info))
        .route(
            "/api/portal/upload-avatar",
            post(avatar).delete(delete_avatar),
        )
        .layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
}
async fn users(State(state): State<AppState>) -> Result<Json<Value>> {
    let native = state.emby(reqwest::Method::GET, "Users", &[], None).await?;
    let native = native
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 用户列表格式无效，未修改用户资料"))?;
    let mut seen = std::collections::HashSet::new();
    let details: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(e) FROM emby_users_extended e")
        .fetch_all(&state.db)
        .await?;
    let mut users = vec![];
    for user in native {
        let id = string(user, "Id");
        if id.is_empty() || string(user, "Name").is_empty() || !seen.insert(id.clone()) {
            return Err(ApiError::bad(
                "Emby 用户列表缺少唯一用户 ID，未修改用户资料",
            ));
        }
        let administrator = user["Policy"]["IsAdministrator"]
            .as_bool()
            .ok_or_else(|| ApiError::bad("Emby 用户角色尚未确认"))?;
        let disabled = user["Policy"]["IsDisabled"]
            .as_bool()
            .ok_or_else(|| ApiError::bad("Emby 用户启用状态尚未确认"))?;
        let extra = details.iter().find(|detail| detail["emby_user_id"] == id);
        users.push(json!({
            "id":id,"name":user["Name"],
            "is_administrator":administrator,
            "is_disabled":disabled,
            "profile_image_tag":user["PrimaryImageTag"],
            "last_activity_date":user["LastActivityDate"],
            "last_login_date":user["LastLoginDate"],
            "registration_date":extra.map(|detail|&detail["registration_date"]),
            "expiration_date":extra.map(|detail|&detail["expiration_date"]),
            "notes":extra.map(|detail|&detail["notes"]),
            "has_password":user["HasPassword"]
        }));
    }
    Ok(Json(json!({"users":users})))
}
fn valid_user_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
async fn native_user(state: &AppState, id: &str) -> Result<Value> {
    let native = state
        .emby(reqwest::Method::GET, &format!("Users/{id}"), &[], None)
        .await?;
    if native["Id"] != id || string(&native, "Name").is_empty() {
        return Err(ApiError::bad("Emby 未返回对应用户，未保存资料"));
    }
    if native["Policy"]["IsAdministrator"].as_bool().is_none()
        || native["Policy"]["IsDisabled"].as_bool().is_none()
    {
        return Err(ApiError::bad("Emby 用户权限和启用状态尚未确认"));
    }
    Ok(native)
}
async fn cache_native_user(state: &AppState, native: &Value) -> Result<()> {
    sqlx::query("INSERT INTO emby_users(id,name,is_administrator,profile_image_tag,policy_json,last_updated_at) VALUES($1,$2,$3,$4,$5,NOW()) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,is_administrator=EXCLUDED.is_administrator,profile_image_tag=EXCLUDED.profile_image_tag,policy_json=EXCLUDED.policy_json,last_updated_at=NOW()")
        .bind(string(native,"Id")).bind(string(native,"Name"))
        .bind(native["Policy"]["IsAdministrator"].as_bool().unwrap_or(false))
        .bind(native["PrimaryImageTag"].as_str()).bind(&native["Policy"]).execute(&state.db).await?;
    Ok(())
}
async fn save_user_details(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(HeaderMap, Json<Value>)> {
    if !valid_user_id(&id)
        || body.as_object().is_none_or(|fields| {
            fields
                .keys()
                .any(|key| !matches!(key.as_str(), "name" | "is_administrator" | "is_disabled"))
        })
    {
        return Err(ApiError::bad("无效 Emby 用户资料"));
    }
    let name = body["name"]
        .as_str()
        .ok_or_else(|| ApiError::bad("用户名必须是文本"))?
        .trim();
    let administrator = body["is_administrator"]
        .as_bool()
        .ok_or_else(|| ApiError::bad("用户角色必须是布尔值"))?;
    let disabled = body["is_disabled"]
        .as_bool()
        .ok_or_else(|| ApiError::bad("用户状态必须是布尔值"))?;
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(ApiError::bad("用户名需为 1–128 个可显示字符"));
    }
    let session = state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?;
    if session.user_id == id && (!administrator || disabled) {
        return Err(ApiError::bad("不能禁用当前登录用户或取消自己的管理员权限"));
    }
    let native = native_user(&state, &id).await?;
    if disabled && (administrator || native["Policy"]["IsAdministrator"] == true) {
        return Err(ApiError::bad("请先保存取消管理员权限，再禁用该用户"));
    }
    if native["Name"] != name && !native["Configuration"].is_object() {
        return Err(ApiError::bad("Emby 用户配置尚未确认，未修改用户名"));
    }
    let result: Result<()> = async {
        if native["Name"] != name {
            // Emby's update endpoint also replaces Configuration, even for a rename.
            let mut profile = native.clone();
            profile["Name"] = json!(name);
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Users/{id}"),
                    &[],
                    Some(profile),
                )
                .await?;
        }
        if native["Policy"]["IsAdministrator"] != administrator
            || native["Policy"]["IsDisabled"] != disabled
        {
            // Merge the two edited fields into Emby's current policy; retain access and playback permissions.
            let mut policy = native["Policy"].clone();
            policy["IsAdministrator"] = json!(administrator);
            policy["IsDisabled"] = json!(disabled);
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Users/{id}/Policy"),
                    &[],
                    Some(policy),
                )
                .await?;
        }
        Ok(())
    }
    .await;
    // A transport error can arrive after Emby applied a write. Reconcile the current native state,
    // and never turn partial or unverified changes into a successful response.
    let saved = native_user(&state, &id).await.map_err(|error| {
        ApiError(
            error.0,
            format!("Emby 用户资料尚未确认，请刷新核对当前状态：{}", error.1),
        )
    })?;
    cache_native_user(&state, &saved).await?;
    if let Err(error) = result {
        return Err(ApiError(
            error.0,
            format!("用户资料未完整保存，请刷新核对 Emby 当前状态：{}", error.1),
        ));
    }
    if saved["Name"] != name
        || saved["Policy"]["IsAdministrator"] != administrator
        || saved["Policy"]["IsDisabled"] != disabled
    {
        return Err(ApiError::bad("Emby 用户资料未完整生效，请刷新核对当前状态"));
    }
    let mut response_headers = HeaderMap::new();
    if session.user_id == id && session.name != name {
        let mut updated = session;
        updated.name = name.to_string();
        response_headers.insert(
            header::SET_COOKIE,
            crate::auth::cookie(&state.sessions.sign_administrator(&updated))
                .parse()
                .map_err(|_| ApiError::bad("登录资料更新失败"))?,
        );
    }
    Ok((
        response_headers,
        Json(json!({
            "message":"Emby 用户资料已保存","id":id,
            "user":{"id":id,"name":saved["Name"],"is_administrator":administrator,"is_disabled":disabled,"profile_image_tag":saved["PrimaryImageTag"],"last_activity_date":saved["LastActivityDate"],"last_login_date":saved["LastLoginDate"],"has_password":saved["HasPassword"]}
        })),
    ))
}
async fn save_user_password(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    if !valid_user_id(&id)
        || body
            .as_object()
            .is_none_or(|fields| fields.keys().any(|key| key != "new_password"))
    {
        return Err(ApiError::bad("无效密码请求"));
    }
    let password = body["new_password"]
        .as_str()
        .ok_or_else(|| ApiError::bad("密码必须是文本"))?;
    if password.is_empty() || password.len() > 1024 {
        return Err(ApiError::bad("请输入不超过 1024 字节的新密码"));
    }
    native_user(&state, &id).await?;
    state
        .emby(
            reqwest::Method::POST,
            &format!("Users/{id}/Password"),
            &[],
            Some(json!({"Id":id,"NewPw":password,"ResetPassword":false})),
        )
        .await?;
    Ok(Json(json!({"message":"Emby 用户密码已更新","id":id})))
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
