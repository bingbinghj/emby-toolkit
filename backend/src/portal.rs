use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/portal/account-info", get(account))
        .route("/api/portal/users", get(users).post(create_user))
        .route(
            "/api/portal/users/{id}",
            get(user_details)
                .post(save_user_details)
                .delete(delete_user),
        )
        .route("/api/portal/users/{id}/password", post(save_user_password))
        .route(
            "/api/portal/users/{id}/avatar",
            post(user_avatar).delete(delete_user_avatar),
        )
        .route("/api/portal/telegram-chat-id", post(chat_id))
        .route(
            "/api/portal/telegram-bot-info",
            get(bot_info).post(bot_info_draft),
        )
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
const POLICY_BOOLEANS: &[&str] = &[
    "IsHidden",
    "IsHiddenRemotely",
    "IsHiddenFromUnusedDevices",
    "EnableUserPreferenceAccess",
    "EnableRemoteAccess",
    "EnableMediaPlayback",
    "EnableAudioPlaybackTranscoding",
    "EnableVideoPlaybackTranscoding",
    "EnablePlaybackRemuxing",
    "EnableRemoteControlOfOtherUsers",
    "EnableSharedDeviceControl",
    "EnableContentDownloading",
    "EnableSubtitleDownloading",
    "EnableSubtitleManagement",
    "EnableContentDeletion",
    "EnableMediaConversion",
    "EnableAllFolders",
    "EnableLiveTvAccess",
    "EnableLiveTvManagement",
    "EnablePublicSharing",
];
const CONFIGURATION_BOOLEANS: &[&str] = &[
    "PlayDefaultAudioTrack",
    "RememberAudioSelections",
    "RememberSubtitleSelections",
    "EnableNextEpisodeAutoPlay",
    "HidePlayedInLatest",
    "DisplayMissingEpisodes",
    "HidePlayedInMoreLikeThis",
    "HidePlayedInSuggestions",
];
fn editable_fields(native: &Value, policy: bool) -> Value {
    let keys = if policy {
        POLICY_BOOLEANS
    } else {
        CONFIGURATION_BOOLEANS
    };
    let mut result = json!({});
    for key in keys {
        if let Some(value) = native.get(*key).filter(|value| value.is_boolean()) {
            result[*key] = value.clone();
        }
    }
    let extra = if policy {
        "EnabledFolders"
    } else {
        "SubtitleMode"
    };
    if let Some(value) = native.get(extra) {
        result[extra] = value.clone();
    }
    result
}
fn presented_user(native: &Value, details: bool) -> Value {
    let mut result = json!({
        "id":native["Id"],"name":native["Name"],
        "is_administrator":native["Policy"]["IsAdministrator"],
        "is_disabled":native["Policy"]["IsDisabled"],
        "profile_image_tag":native["PrimaryImageTag"],
        "last_activity_date":native["LastActivityDate"],"last_login_date":native["LastLoginDate"],
        "has_password":native["HasPassword"]
    });
    if details {
        result["policy"] = editable_fields(&native["Policy"], true);
        result["configuration"] = editable_fields(&native["Configuration"], false);
    }
    result
}
fn merged_editable_fields(current: &Value, patch: Option<&Value>, policy: bool) -> Result<Value> {
    let Some(patch) = patch else {
        return Ok(current.clone());
    };
    let fields = patch
        .as_object()
        .ok_or_else(|| ApiError::bad("用户权限或配置必须是对象"))?;
    if !current.is_object() {
        return Err(ApiError::bad("Emby 用户配置尚未确认，未修改资料"));
    }
    let keys = if policy {
        POLICY_BOOLEANS
    } else {
        CONFIGURATION_BOOLEANS
    };
    let mut merged = current.clone();
    for (key, value) in fields {
        let valid = if keys.contains(&key.as_str()) {
            value.is_boolean() && current.get(key).is_some_and(Value::is_boolean)
        } else if policy && key == "EnabledFolders" {
            value.as_array().is_some_and(|values| {
                values.len() <= 512
                    && values
                        .iter()
                        .all(|value| value.as_str().is_some_and(valid_user_id))
            }) && current.get(key).is_some_and(Value::is_array)
        } else if !policy && key == "SubtitleMode" {
            value.as_str().is_some_and(|mode| {
                matches!(mode, "Default" | "Always" | "OnlyForced" | "None" | "Smart")
            }) && current.get(key).is_some_and(Value::is_string)
        } else {
            false
        };
        if !valid {
            return Err(ApiError::bad(format!(
                "不支持或无效的 Emby 用户字段：{key}"
            )));
        }
        merged[key] = value.clone();
    }
    Ok(merged)
}
async fn user_details(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    if !valid_user_id(&id) {
        return Err(ApiError::bad("无效 Emby 用户 ID"));
    }
    let native = native_user(&state, &id).await?;
    let libraries = match state
        .emby(reqwest::Method::GET, "Library/VirtualFolders", &[], None)
        .await
    {
        Ok(value) => match value.as_array() {
            Some(items)
                if items.iter().all(|item| {
                    valid_user_id(&string(item, "ItemId")) && !string(item, "Name").is_empty()
                }) =>
            {
                let items: Vec<Value> = items
                    .iter()
                    .map(|item| json!({"id":item["ItemId"],"name":item["Name"]}))
                    .collect();
                json!({"items":items})
            }
            _ => json!({"items":null,"error":"Emby 媒体库列表格式无效"}),
        },
        Err(error) => json!({"items":null,"error":error.1}),
    };
    Ok(Json(
        json!({"user":presented_user(&native,true),"libraries":libraries}),
    ))
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
fn validate_details(body: &Value) -> Result<(&str, bool, bool)> {
    if body.as_object().is_none_or(|fields| {
        fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "name" | "is_administrator" | "is_disabled" | "policy" | "configuration"
            )
        })
    }) {
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
    if administrator && disabled {
        return Err(ApiError::bad("不能禁用管理员用户"));
    }
    Ok((name, administrator, disabled))
}
async fn apply_user_details(
    state: &AppState,
    id: &str,
    body: &Value,
    current_user: &str,
) -> Result<Value> {
    if !valid_user_id(id) {
        return Err(ApiError::bad("无效 Emby 用户 ID"));
    }
    let (name, administrator, disabled) = validate_details(body)?;
    if current_user == id && (!administrator || disabled) {
        return Err(ApiError::bad("不能禁用当前登录用户或取消自己的管理员权限"));
    }
    let native = native_user(state, id).await?;
    if disabled && (administrator || native["Policy"]["IsAdministrator"] == true) {
        return Err(ApiError::bad("请先保存取消管理员权限，再禁用该用户"));
    }
    let mut policy = merged_editable_fields(&native["Policy"], body.get("policy"), true)?;
    policy["IsAdministrator"] = json!(administrator);
    policy["IsDisabled"] = json!(disabled);
    let configuration =
        merged_editable_fields(&native["Configuration"], body.get("configuration"), false)?;
    if native["Name"] != name && !configuration.is_object() {
        return Err(ApiError::bad("Emby 用户配置尚未确认，未修改用户名"));
    }
    let result: Result<()> = async {
        if native["Name"] != name {
            // Emby's rename endpoint also replaces Configuration. Preserve every unedited setting.
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Users/{id}"),
                    &[],
                    Some(json!({"Id":id,"Name":name,"Configuration":configuration})),
                )
                .await?;
        } else if configuration != native["Configuration"] {
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Users/{id}/Configuration"),
                    &[],
                    Some(configuration.clone()),
                )
                .await?;
        }
        if policy != native["Policy"] {
            state
                .emby(
                    reqwest::Method::POST,
                    &format!("Users/{id}/Policy"),
                    &[],
                    Some(policy.clone()),
                )
                .await?;
        }
        Ok(())
    }
    .await;
    // The upstream may apply a write before a transport error. Reconcile but do not report partial success.
    let saved = native_user(state, id).await.map_err(|error| {
        ApiError(
            error.0,
            format!("Emby 用户资料尚未确认，请刷新核对当前状态：{}", error.1),
        )
    })?;
    cache_native_user(state, &saved).await?;
    if let Err(error) = result {
        return Err(ApiError(
            error.0,
            format!("用户资料未完整保存，请刷新核对 Emby 当前状态：{}", error.1),
        ));
    }
    if saved["Name"] != name || saved["Policy"] != policy || saved["Configuration"] != configuration
    {
        return Err(ApiError::bad("Emby 用户资料未完整生效，请刷新核对当前状态"));
    }
    Ok(saved)
}
async fn save_user_details(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(HeaderMap, Json<Value>)> {
    let session = state
        .sessions
        .from_headers(&headers)
        .ok_or_else(ApiError::forbidden)?;
    let saved = apply_user_details(&state, &id, &body, &session.user_id).await?;
    let name = string(&saved, "Name");
    let mut response_headers = HeaderMap::new();
    if session.user_id == id && session.name != name {
        let remember = state.sessions.remember_claim(&headers).unwrap_or(true);
        let mut updated = session;
        updated.name = name;
        response_headers.insert(
            header::SET_COOKIE,
            crate::auth::cookie(
                &state
                    .sessions
                    .sign_administrator_with_remember(&updated, remember),
                updated.expires,
                remember,
            )
            .parse()
            .map_err(|_| ApiError::bad("登录资料更新失败"))?,
        );
    }
    Ok((
        response_headers,
        Json(json!({"message":"Emby 用户资料已保存","id":id,"user":presented_user(&saved,true)})),
    ))
}
async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response> {
    let password = body
        .as_object_mut()
        .and_then(|body| body.remove("new_password"))
        .and_then(|password| password.as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::bad("密码必须是文本"))?;
    if password.is_empty() || password.len() > 1024 {
        return Err(ApiError::bad("请输入不超过 1024 字节的新密码"));
    }
    let (name, administrator, disabled) = validate_details(&body)?;
    if administrator && disabled {
        return Err(ApiError::bad("不能禁用管理员用户"));
    }
    // New users initially use Emby's own defaults. Advanced fields are read back before editing.
    if body.get("policy").is_some() || body.get("configuration").is_some() {
        return Err(ApiError::bad("请先新增用户，再编辑 Emby 权限与播放设置"));
    }
    let current = state.emby(reqwest::Method::GET, "Users", &[], None).await?;
    let users = current
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 用户列表格式无效"))?;
    if users
        .iter()
        .any(|user| string(user, "Name").eq_ignore_ascii_case(name))
    {
        return Err(ApiError::bad("用户名已存在"));
    }
    // Creation is not replayable: a lost response must not create another user.
    let cfg = state.config.read().await;
    let endpoint = format!(
        "{}/Users/New",
        string(&cfg, "emby_server_url").trim_end_matches('/')
    );
    let key = string(&cfg, "emby_api_key");
    drop(cfg);
    let attempt: Result<Value> = async {
        Ok(state
            .http
            .post(endpoint)
            .header("X-Emby-Token", key)
            .timeout(std::time::Duration::from_secs(60))
            .json(&json!({"Name":name}))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?)
    }
    .await;
    let created = attempt.map_err(|error| {
        ApiError(
            error.0,
            "新增 Emby 用户尚未确认，请刷新用户列表，避免重复新增".into(),
        )
    })?;
    let id = string(&created, "Id");
    if !valid_user_id(&id) || created["Name"] != name {
        return Err(ApiError::bad(
            "新增 Emby 用户尚未确认，请刷新用户列表，避免重复新增",
        ));
    }
    let result: Result<Value> = async {
        state
            .emby(
                reqwest::Method::POST,
                &format!("Users/{id}/Password"),
                &[],
                Some(json!({"Id":id,"NewPw":password,"ResetPassword":false})),
            )
            .await?;
        let session = state
            .sessions
            .from_headers(&headers)
            .ok_or_else(ApiError::forbidden)?;
        let saved = apply_user_details(&state, &id, &body, &session.user_id).await?;
        if saved["HasPassword"] != true {
            return Err(ApiError::bad("Emby 用户密码设置尚未确认"));
        }
        Ok(saved)
    }
    .await;
    match result {
        Ok(saved) => Ok(Json(json!({"success":true,"id":id,"user":presented_user(&saved,true),"message":"Emby 用户已新增"})).into_response()),
        Err(error) => {
            let mut saved = native_user(&state,&id).await.ok();
            // Do not leave a newly-created, passwordless account enabled after a failed setup.
            if let Some(native) = &saved
                && native["HasPassword"] == false && native["Policy"]["IsAdministrator"] == false
                    && native["Policy"]["IsDisabled"] == false {
                    let mut policy = native["Policy"].clone();
                    policy["IsDisabled"] = json!(true);
                    if state.emby(reqwest::Method::POST,&format!("Users/{id}/Policy"),&[],Some(policy)).await.is_ok() {
                        saved = native_user(&state,&id).await.ok();
                    }
            }
            if let Some(saved) = &saved { cache_native_user(&state,saved).await?; }
            Ok((error.0, Json(json!({"success":false,"created":true,"id":id,"user":saved.as_ref().map(|saved|presented_user(saved,true)),
                "error":format!("Emby 用户已创建，资料或密码尚未完成，请继续编辑该用户：{}",error.1)}))).into_response())
        }
    }
}
async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    if !valid_user_id(&id) {
        return Err(ApiError::bad("无效 Emby 用户 ID"));
    }
    if user(&state, &headers)? == id {
        return Err(ApiError::bad("不能删除当前登录用户"));
    }
    let native = native_user(&state, &id).await?;
    if native["Policy"]["IsAdministrator"] == true {
        let users = state.emby(reqwest::Method::GET, "Users", &[], None).await?;
        let others = users
            .as_array()
            .ok_or_else(|| ApiError::bad("Emby 用户列表格式无效"))?;
        if !others.iter().any(|user| {
            user["Id"] != id
                && user["Policy"]["IsAdministrator"] == true
                && user["Policy"]["IsDisabled"] == false
        }) {
            return Err(ApiError::bad("不能删除最后一个启用的管理员"));
        }
    }
    state
        .emby(reqwest::Method::DELETE, &format!("Users/{id}"), &[], None)
        .await?;
    let users = state.emby(reqwest::Method::GET, "Users", &[], None).await?;
    if users
        .as_array()
        .is_none_or(|users| users.iter().any(|user| user["Id"] == id))
    {
        return Err(ApiError::bad("Emby 用户删除尚未确认，请刷新核对"));
    }
    sqlx::query("DELETE FROM emby_users WHERE id=$1")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({"id":id,"message":"Emby 用户已删除"})))
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
    let config = state.config.read().await.clone();
    let token = string(&config, "telegram_bot_token");
    lookup_bot_info(&state, &token, &config).await
}
async fn bot_info_draft(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let mut config = state.config.read().await.clone();
    let mut token = string(&body, "token");
    if token == "********" {
        token = string(&config, "telegram_bot_token");
    }
    if let Some(api_url) = body.get("api_url") {
        config["telegram_api_url"] = api_url.clone();
    }
    lookup_bot_info(&state, token.trim(), &config).await
}
async fn lookup_bot_info(state: &AppState, token: &str, config: &Value) -> Result<Json<Value>> {
    if token.is_empty() {
        return Ok(Json(json!({"bot_username":null,"error":"Bot Token未配置"})));
    }
    let api_base = crate::notifications::telegram_api_base(config, &state.notifications.api_base)?;
    let response = state
        .http
        .get(format!("{}/bot{token}/getMe", api_base))
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
            json!({"bot_username":null,"error":format!("Telegram API 错误: {}", crate::logs::redact(&info["description"].as_str().unwrap_or("未知API错误").replace(token, "********").replace(&api_base, "Telegram API")))}),
        ))
    }
}
async fn delete_avatar(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let id = user(&state, &headers)?;
    reset_avatar(&state, &id).await
}
async fn delete_user_avatar(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    reset_avatar(&state, &id).await
}
async fn reset_avatar(state: &AppState, id: &str) -> Result<Json<Value>> {
    if !valid_user_id(id) {
        return Err(ApiError::bad("无效 Emby 用户 ID"));
    }
    let native = native_user(state, id).await?;
    if native["PrimaryImageTag"]
        .as_str()
        .is_some_and(|tag| !tag.is_empty())
    {
        state
            .emby(
                reqwest::Method::DELETE,
                &format!("Users/{id}/Images/Primary"),
                &[],
                None,
            )
            .await?;
    }
    let saved = native_user(state, id).await?;
    cache_native_user(state, &saved).await?;
    if saved["PrimaryImageTag"]
        .as_str()
        .is_some_and(|tag| !tag.is_empty())
    {
        return Err(ApiError::bad("Emby 默认头像恢复尚未确认，请刷新用户资料"));
    }
    Ok(Json(
        json!({"status":"ok","id":id,"message":"已重置默认头像","new_tag":null}),
    ))
}
async fn avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<Value>> {
    let id = user(&state, &headers)?;
    upload_avatar(&state, &id, multipart).await
}
async fn user_avatar(
    State(state): State<AppState>,
    Path(id): Path<String>,
    multipart: Multipart,
) -> Result<Json<Value>> {
    upload_avatar(&state, &id, multipart).await
}
fn valid_avatar(mime: &str, bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > 800 * 1024 {
        return false;
    }
    let format = match mime {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/gif" => image::ImageFormat::Gif,
        "image/webp" => image::ImageFormat::WebP,
        _ => return false,
    };
    image::guess_format(bytes).is_ok_and(|actual| actual == format)
        && image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
            .into_dimensions()
            .is_ok_and(|(width, height)| {
                width > 0 && height > 0 && width <= 16384 && height <= 16384
            })
}
async fn upload_avatar(
    state: &AppState,
    id: &str,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    if !valid_user_id(id) {
        return Err(ApiError::bad("无效 Emby 用户 ID"));
    }
    let before = native_user(state, id).await?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad("上传文件无效"))?
    {
        if field.name() != Some("avatar") || field.file_name().is_none() {
            continue;
        }
        let mime = field.content_type().unwrap_or("").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|_| ApiError::bad("读取头像失败"))?;
        if !valid_avatar(&mime, &bytes) {
            return Err(ApiError::bad(
                "请选择不超过 800 KB 的 JPG、PNG、GIF 或 WebP 图片",
            ));
        }
        let config = state.config.read().await;
        let url = string(&config, "emby_server_url");
        let key = string(&config, "emby_api_key");
        drop(config);
        // Emby replaces Primary index 0 itself; never delete the current avatar before uploading.
        crate::emby::send(
            state
                .http
                .post(format!(
                    "{}/Users/{id}/Images/Primary",
                    url.trim_end_matches('/')
                ))
                .header("X-Emby-Token", key)
                .header("Content-Type", mime)
                .timeout(std::time::Duration::from_secs(60))
                .body(STANDARD.encode(bytes)),
        )
        .await?
        .error_for_status()?;
        let saved = native_user(state, id).await?;
        cache_native_user(state, &saved).await?;
        let tag = saved["PrimaryImageTag"]
            .as_str()
            .filter(|tag| !tag.is_empty())
            .ok_or_else(|| ApiError::bad("Emby 头像更新尚未确认，请刷新用户资料"))?;
        if before["PrimaryImageTag"] == tag {
            return Err(ApiError::bad("Emby 头像更新尚未确认，请刷新用户资料"));
        }
        return Ok(Json(
            json!({"status":"ok","id":id,"message":"头像已更新","new_tag":tag}),
        ));
    }
    Err(ApiError::bad("未找到文件"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editable_patch_preserves_unknown_permissions_and_rejects_unconfirmed_fields() {
        let current = json!({"EnableRemoteAccess":true,"EnabledFolders":["movies"],"OpaqueEmbyPolicy":{"key":"keep"}});
        let merged =
            merged_editable_fields(&current, Some(&json!({"EnableRemoteAccess":false})), true)
                .unwrap();
        assert_eq!(merged["OpaqueEmbyPolicy"], current["OpaqueEmbyPolicy"]);
        assert_eq!(merged["EnabledFolders"], current["EnabledFolders"]);
        for patch in [
            json!({"OpaqueEmbyPolicy":false}),
            json!({"EnableMediaPlayback":false}),
            json!({"EnableRemoteAccess":"false"}),
            json!({"EnabledFolders":["../unsafe"]}),
        ] {
            assert!(merged_editable_fields(&current, Some(&patch), true).is_err());
        }
    }
    #[test]
    fn presented_details_exclude_credentials_and_only_show_native_confirmed_fields() {
        let native = json!({"Id":"member","Name":"Member","AccessToken":"private","Policy":{"IsAdministrator":false,"IsDisabled":false,"EnableRemoteAccess":true,"Unknown":"private"},"Configuration":{"SubtitleMode":"Smart","ProfilePin":"private"}});
        let detail = presented_user(&native, true);
        assert_eq!(detail["policy"], json!({"EnableRemoteAccess":true}));
        assert_eq!(detail["configuration"], json!({"SubtitleMode":"Smart"}));
        assert!(!detail.to_string().contains("private"));
    }
    #[test]
    fn avatar_requires_matching_image_headers_and_reasonable_dimensions() {
        let mut bytes = std::io::Cursor::new(vec![]);
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert!(valid_avatar("image/png", bytes.get_ref()));
        assert!(!valid_avatar("image/jpeg", bytes.get_ref()));
        assert!(!valid_avatar("image/png", b"not-an-image"));
        assert!(!valid_avatar("text/html", bytes.get_ref()));
        assert!(!valid_avatar("image/png", &vec![0; 800 * 1024 + 1]));
    }
}
