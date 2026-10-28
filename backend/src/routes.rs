use crate::{
    AppState,
    auth::{self, Session},
    db,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde_json::{Value, json};
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .merge(crate::p115::router())
        .merge(crate::ai::router())
        .merge(crate::tasks::router())
        .merge(crate::revalidation::router())
        .merge(crate::portal::router())
        .merge(crate::logs::router())
        .merge(crate::media::router())
        .merge(crate::media_edit::router())
        .merge(crate::media_extra::router())
        .merge(crate::media_manual::router())
        .merge(crate::media_search::router())
        .merge(crate::media_images::router())
        .merge(crate::media_rename::router())
        .merge(crate::audio_tasks::router())
        .merge(crate::fingerprint_audit::router())
        .merge(crate::covers::router())
        .merge(crate::actions::router())
        .merge(crate::orphans::router())
        .merge(crate::system_ops::router())
        .merge(crate::episodes::router())
        .merge(crate::mappings::router())
        .merge(crate::database_admin::router())
        .merge(crate::database_stats::router())
        .route("/api/system/about_info", get(about))
        .route("/api/system/menu_config", get(menu_get).post(menu_save))
        .route("/api/system/menu_config/reset", post(menu_reset))
        .route("/api/status", get(job_status))
        .route("/api/trigger_stop_task", post(job_stop))
        .route("/api/tasks/pause", post(job_pause))
        .route("/api/tasks/resume", post(job_resume))
        .route("/api/tasks/result/{id}", get(job_result))
        .route(
            "/api/mappings/config/{key}",
            get(mapping_get).post(mapping_save),
        )
        .route("/api/mappings/config/{key}/defaults", get(mapping_default))
        .route("/api/ai/prompts", get(prompts_get).post(prompts_save))
        .route("/api/ai/prompts/reset", post(prompts_reset))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin,
        ));
    let static_dir = std::env::var("ETK_STATIC_DIR").unwrap_or_else(|_| "frontend/dist".into());
    Router::new()
        .route("/health", get(health))
        .route("/api/health", get(health))
        .route("/api/auth/check_status", get(auth_status))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/setup", post(setup))
        .route("/api/auth/request_recovery", post(request_recovery))
        .route("/api/auth/verify_recovery", post(verify_recovery))
        .route("/api/config", get(config_get).post(config_save))
        .merge(protected)
        .merge(crate::webhook::router())
        .route("/api/{*path}", any(api_not_found))
        .route("/webhook/{*path}", any(api_not_found))
        .fallback_service(static_files(&static_dir))
        .with_state(state)
}
fn static_files(directory: &str) -> ServeDir<ServeFile> {
    ServeDir::new(directory).fallback(ServeFile::new(format!("{directory}/index.html")))
}
async fn health() -> Json<Value> {
    Json(json!({"status":"ok"}))
}
async fn api_not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "接口不存在".into())
}

async fn job_status(State(state): State<AppState>) -> Response {
    let mut status = state.jobs.status().await;
    status["logs"] = json!(state.logs.recent());
    ([(header::CACHE_CONTROL, "no-store")], Json(status)).into_response()
}
async fn job_stop(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    state.jobs.control(body["task_id"].as_str(), "stop").await?;
    Ok(Json(json!({"message":"已请求停止任务"})))
}
async fn job_pause(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    state
        .jobs
        .control(body["task_id"].as_str(), "pause")
        .await?;
    Ok(Json(json!({"message":"已请求暂停任务"})))
}
async fn job_resume(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>> {
    state
        .jobs
        .control(body["task_id"].as_str(), "resume")
        .await?;
    Ok(Json(json!({"message":"任务已恢复"})))
}
async fn job_result(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(state.jobs.result(&id).await.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "任务结果不存在".into())
    })?))
}

async fn auth_status(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    if !state.configured().await {
        return Ok(Json(
            json!({"status":"setup_required", "message":"系统未配置"}),
        ));
    }
    if let Some(session) = state.sessions.from_headers(&headers)
        && (state.sessions.administrator_claim(&headers) == Some(true)
            || db::is_admin(&state.db, &session.user_id).await?)
    {
        return Ok(Json(
            json!({"status":"logged_in", "user":{"id":session.user_id,"name":session.name,"is_admin":true}}),
        ));
    }
    Ok(Json(json!({"status":"login_required"})))
}

async fn login(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Response> {
    if !state.configured().await {
        return Ok((StatusCode::PRECONDITION_REQUIRED, Json(json!({"status":"error", "code":"SETUP_REQUIRED","message":"系统尚未配置 Emby 连接"}))).into_response());
    }
    let failure = || {
        (StatusCode::UNAUTHORIZED, Json(json!({"status":"error","message":"登录失败：用户名/密码错误，或无法连接 Emby 服务器"}))).into_response()
    };
    let username = string(&body, "username");
    if username.is_empty() {
        return Ok(failure());
    }
    let config = state.config.read().await;
    let url = format!(
        "{}/Users/AuthenticateByName",
        string(&config, "emby_server_url").trim_end_matches('/')
    );
    let timeout = config["emby_api_timeout"]
        .as_u64()
        .filter(|seconds| *seconds > 0)
        .unwrap_or(60);
    drop(config);
    let result = crate::emby::send(state.http.post(url)
        .header("X-Emby-Authorization", "Emby Client=\"Emby Web\", Device=\"Chrome\", DeviceId=\"my-emby-toolkit-auth-v4\", Version=\"4.8.0.80\"")
        .json(&json!({"Username":username,"Pw":string(&body,"password"),"LoginType":"Manual"}))
        .timeout(std::time::Duration::from_secs(timeout))).await;
    let result = match result {
        Ok(result) if result.status() == StatusCode::OK => result,
        Ok(result) => {
            tracing::error!(
                "登录失败，Emby 返回的原始错误信息：{}",
                result.text().await.unwrap_or_default()
            );
            return Ok(failure());
        }
        Err(error) => {
            tracing::error!("认证用户 {username} 时发生网络请求错误：{error}");
            return Ok(failure());
        }
    };
    let data: Value = match result.json().await {
        Ok(data) => data,
        Err(_) => return Ok(failure()),
    };
    if string(&data, "AccessToken").is_empty()
        || data["User"].as_object().is_none_or(|user| user.is_empty())
    {
        tracing::error!("登录成功但响应格式不正确：缺少 User 或 AccessToken");
        return Ok(failure());
    }
    let user = &data["User"];
    if !user["Policy"]["IsAdministrator"].as_bool().unwrap_or(false) {
        return Ok((
            StatusCode::FORBIDDEN,
            [(
                header::SET_COOKIE,
                "etk_session=; Max-Age=0; HttpOnly; SameSite=Lax; Path=/",
            )],
            Json(json!({"status":"error","message":"登录失败：仅允许 Emby 管理员登录"})),
        )
            .into_response());
    }
    let id = string(user, "Id");
    if id.is_empty() {
        return Err(ApiError::bad("Emby 未返回用户 ID"));
    }
    let name = string(user, "Name");
    if let Err(error) = sqlx::query("INSERT INTO emby_users(id,name,is_administrator,policy_json,last_seen_at,last_updated_at) VALUES($1,$2,TRUE,$3,NOW(),NOW()) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,is_administrator=TRUE,policy_json=EXCLUDED.policy_json,last_seen_at=NOW(),last_updated_at=NOW()")
        .bind(&id).bind(&name).bind(&user["Policy"]).execute(&state.db).await {
        tracing::warn!("登录时同步用户信息失败：{error}");
    }
    let token = state.sessions.sign_administrator(&Session {
        user_id: id.clone(),
        name: name.clone(),
        setup_mode: false,
        expires: chrono::Utc::now().timestamp() + 2678400,
    });
    Ok(([(header::SET_COOKIE, auth::cookie(&token))], Json(json!({"status":"ok", "user":{"id":id,"name":name,"is_admin":true,"user_type":"emby_user"}}))).into_response())
}
async fn logout() -> impl IntoResponse {
    (
        [(
            header::SET_COOKIE,
            "etk_session=; Max-Age=0; HttpOnly; SameSite=Lax; Path=/",
        )],
        Json(json!({"status":"ok"})),
    )
}
async fn can_configure(state: &AppState, headers: &HeaderMap) -> Result<bool> {
    if !state.configured().await {
        return Ok(true);
    }
    if let Some(session) = state.sessions.from_headers(headers) {
        let recovery = session.setup_mode;
        return Ok(recovery || db::is_admin(&state.db, &session.user_id).await?);
    }
    Ok(false)
}
async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response> {
    let configured = state.configured().await;
    let session = state.sessions.from_headers(&headers);
    let allowed = session.is_some_and(|session| session.setup_mode);
    if configured && !allowed {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "系统已配置，且无重置权限".into(),
        ));
    }
    let url = string(&body, "url");
    let key = string(&body, "api_key");
    if url.is_empty() || key.is_empty() {
        return Err(ApiError::bad("URL 和 API Key 不能为空"));
    }
    let parsed = reqwest::Url::parse(&url).map_err(|_| ApiError::bad("Emby URL 无效"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::bad("Emby URL 只支持 HTTP/HTTPS"));
    }
    let response = crate::emby::send(
        state
            .http
            .get(format!("{}/emby/System/Info", url.trim_end_matches('/')))
            .header("X-Emby-Token", &key),
    )
    .await
    .map_err(|_| ApiError::bad("连接测试失败: 无法连接到服务器，请检查 URL 或网络"))?
    .error_for_status()
    .map_err(|error| {
        ApiError::bad(match error.status().map(|status| status.as_u16()) {
            Some(401) => "连接测试失败: API Key 无效或无权限".into(),
            Some(404) => "连接测试失败: 找不到 Emby 服务器 (404)，请检查 URL".into(),
            Some(status) => format!("连接测试失败: 连接失败 (HTTP {status})"),
            None => "连接测试失败: 无法连接到服务器，请检查 URL 或网络".into(),
        })
    })?;
    drop(response);
    if state.configured().await && !allowed {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "系统已配置，且无重置权限".into(),
        ));
    }
    state
        .save_config(&json!({"emby_server_url":url,"emby_api_key":key}))
        .await?;
    Ok((
        [(
            header::SET_COOKIE,
            "etk_session=; Max-Age=0; HttpOnly; SameSite=Lax; Path=/",
        )],
        Json(json!({"status":"ok","message":"配置保存成功，请登录"})),
    )
        .into_response())
}

async fn request_recovery(State(state): State<AppState>) -> Json<Value> {
    let token = uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase();
    let now = chrono::Utc::now().timestamp();
    let mut tokens = state.recovery_tokens.write().await;
    tokens.retain(|_, expires| *expires > now);
    tokens.insert(token.clone(), now + 300);
    tracing::error!(
        python_level = "CRITICAL",
        "============================================================"
    );
    tracing::error!(
        python_level = "CRITICAL",
        "【安全警告】收到重置连接配置请求。"
    );
    tracing::error!(
        python_level = "CRITICAL",
        "若这是您本人的操作，请在页面输入以下安全令牌进入设置模式:"
    );
    tracing::error!(python_level = "CRITICAL", "安全令牌:  {token}");
    tracing::error!(python_level = "CRITICAL", "令牌有效期: 5 分钟");
    tracing::error!(
        python_level = "CRITICAL",
        "============================================================"
    );
    Json(json!({"status":"ok","message":"安全令牌已写入 Docker 日志，请查阅后输入。"}))
}
async fn verify_recovery(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response> {
    let token = string(&body, "token").trim().to_uppercase();
    let expiry = state
        .recovery_tokens
        .write()
        .await
        .remove(&token)
        .unwrap_or(0);
    if expiry <= chrono::Utc::now().timestamp() {
        return Err(ApiError(StatusCode::FORBIDDEN, "令牌无效或已过期".into()));
    }
    let token = state.sessions.sign(&Session {
        user_id: String::new(),
        name: "setup".into(),
        setup_mode: true,
        expires: chrono::Utc::now().timestamp() + 2678400,
    });
    Ok((
        [(
            header::SET_COOKIE,
            format!("etk_session={token}; HttpOnly; SameSite=Lax; Path=/"),
        )],
        Json(json!({"status":"ok","message":"验证成功"})),
    )
        .into_response())
}
async fn config_get(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    if !can_configure(&state, &headers).await? {
        return Err(ApiError::forbidden());
    }
    Ok(Json(state.config.read().await.clone()))
}
async fn config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut value): Json<Value>,
) -> Result<Json<Value>> {
    if !can_configure(&state, &headers).await? {
        return Err(ApiError::forbidden());
    }
    if value.as_object().is_none_or(|object| object.is_empty()) {
        return Err(ApiError::bad("请求体中未包含配置数据"));
    }
    if value.get("emby_user_id").is_some() {
        let user = string(&value, "emby_user_id").trim().to_string();
        if user.is_empty() {
            return Err(ApiError::bad("Emby User ID 不能为空！"));
        }
        if user.len() != 32 || !user.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(ApiError::bad("Emby User ID 格式不正确！"));
        }
        value["emby_user_id"] = json!(user);
    }
    if value.get("libraries_to_ignore").is_some_and(|ids| {
        ids.as_array()
            .is_none_or(|ids| ids.iter().any(|id| id.as_str().is_none()))
    }) {
        return Err(ApiError::bad("忽略媒体库必须是 ID 字符串数组"));
    }
    let config = state.config.read().await.clone();
    for key in [
        "db_password",
        "emby_api_key",
        "emby_admin_pass",
        "tmdb_api_key",
        "douban_cookie",
        "ai_api_key",
        "telegram_bot_token",
        "emby_webhook_secret",
    ] {
        if value[key] == "********" {
            value[key] = config.get(key).cloned().unwrap_or(json!(""));
        }
    }
    if let Some(language) = value.get("subtitle_chinese_default_language")
        && !matches!(language.as_str(), Some("chs" | "zho"))
    {
        value["subtitle_chinese_default_language"] = json!("chs");
    }
    let mut effective = config.clone();
    effective
        .as_object_mut()
        .unwrap()
        .extend(value.as_object().unwrap().clone());
    let base = string(&effective, "emby_server_url");
    let key = string(&effective, "emby_api_key");
    let user = string(&effective, "emby_user_id");
    if !base.is_empty()
        && !key.is_empty()
        && !user.is_empty()
        && value["libraries_to_ignore"].is_array()
    {
        let libraries = async {
            let response = crate::emby::send(
                state
                    .http
                    .get(format!(
                        "{}/emby/Users/{user}/Views",
                        base.trim_end_matches('/')
                    ))
                    .header("X-Emby-Token", key),
            )
            .await?
            .error_for_status()?;
            response.json::<Value>().await
        }
        .await;
        if let Ok(libraries) = libraries {
            let items = libraries.get("Items").cloned().unwrap_or(json!([]));
            if let Some(items) = items.as_array() {
                let valid = items
                    .iter()
                    .filter_map(|item| item["Id"].as_str())
                    .collect::<std::collections::HashSet<_>>();
                value["libraries_to_ignore"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|id| id.as_str().is_some_and(|id| valid.contains(id)));
            }
        } else {
            tracing::warn!("无法从Emby获取媒体库列表，本次保存跳过净化步骤");
        }
    }
    state.save_config(&value).await?;
    state.refresh_server_identity().await;
    Ok(Json(
        json!({"message":"配置已成功保存并自动净化！","saved_fields":value}),
    ))
}

pub fn default_mapping(key: &str) -> Option<Value> {
    let name = match key {
        "language_mapping" => "DEFAULT_LANGUAGE_MAPPING",
        "country_mapping" => "DEFAULT_COUNTRY_MAPPING",
        "keyword_mapping" => "DEFAULT_KEYWORD_MAPPING",
        "studio_mapping" => "DEFAULT_STUDIO_MAPPING",
        "rating_mapping" => "DEFAULT_RATING_MAPPING",
        "rating_priority" => "DEFAULT_RATING_PRIORITY",
        "unified_ratings" => "UNIFIED_RATING_CATEGORIES",
        _ => return None,
    };
    let data: Value = serde_json::from_str(include_str!("../assets/defaults.json"))
        .expect("valid bundled defaults");
    Some(if key == "unified_ratings" {
        json!(
            data[name]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| json!({"label":v}))
                .collect::<Vec<_>>()
        )
    } else {
        data[name].clone()
    })
}
async fn mapping_get(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    let default = default_mapping(&key)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "映射类型不存在".into()))?;
    let stored = db::setting(&state.db, &key)
        .await?
        .filter(|value| match value {
            Value::Null | Value::Bool(false) => false,
            Value::Array(values) => !values.is_empty(),
            Value::Object(values) => !values.is_empty(),
            Value::String(value) => !value.is_empty(),
            Value::Number(value) => value.as_f64() != Some(0.0),
            _ => true,
        })
        .unwrap_or(default.clone());
    Ok(Json(
        if matches!(key.as_str(), "rating_mapping" | "rating_priority") {
            stored
        } else {
            crate::mappings::ensure_list(stored, default)
        },
    ))
}
async fn mapping_save(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(value): Json<Value>,
) -> Result<Json<Value>> {
    if default_mapping(&key).is_none() {
        return Err(ApiError::bad("不支持的映射类型"));
    }
    if !value.is_array() && !value.is_object() {
        return Err(ApiError::bad("映射必须是数组或对象"));
    }
    if key == "rating_mapping" && !value.is_object()
        || key == "rating_priority" && !value.is_array()
    {
        return Err(ApiError::bad("映射数据格式错误"));
    }
    db::save_setting(&state.db, &key, &value).await?;
    Ok(Json(json!({"message":"保存成功"})))
}
async fn mapping_default(Path(key): Path<String>) -> Result<Json<Value>> {
    Ok(Json(default_mapping(&key).ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "映射类型不存在".into())
    })?))
}
async fn prompts_get(State(state): State<AppState>) -> Result<Json<Value>> {
    let defaults: Value =
        serde_json::from_str(include_str!("../assets/defaults.json")).expect("valid defaults");
    let mut prompts = defaults["DEFAULT_AI_PROMPTS"].clone();
    if let Some(Value::Object(stored)) = db::setting(&state.db, "ai_user_prompts").await? {
        prompts.as_object_mut().unwrap().extend(stored);
    }
    Ok(Json(prompts))
}
async fn prompts_save(
    State(state): State<AppState>,
    Json(value): Json<Value>,
) -> Result<Json<Value>> {
    if !value.is_object() {
        return Err(ApiError::bad("提示词必须是 JSON 对象"));
    }
    db::save_setting(&state.db, "ai_user_prompts", &value).await?;
    Ok(Json(json!({"message":"保存成功"})))
}
async fn prompts_reset(State(state): State<AppState>) -> Result<Json<Value>> {
    sqlx::query("DELETE FROM app_settings WHERE setting_key='ai_user_prompts'")
        .execute(&state.db)
        .await?;
    let defaults: Value =
        serde_json::from_str(include_str!("../assets/defaults.json")).expect("bundled defaults");
    Ok(Json(
        json!({"message":"已恢复默认提示词","prompts":defaults["DEFAULT_AI_PROMPTS"]}),
    ))
}
async fn about() -> Json<Value> {
    Json(json!({"current_version":"10.7.5","releases":[]}))
}
async fn menu_get(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(
        db::setting(&state.db, "custom_menu_config")
            .await?
            .unwrap_or(json!({})),
    ))
}
async fn menu_save(State(state): State<AppState>, Json(value): Json<Value>) -> Result<Json<Value>> {
    if !value.is_array() && !value.is_object() {
        return Err(ApiError::bad("无效的数据格式"));
    }
    db::save_setting(&state.db, "custom_menu_config", &value).await?;
    Ok(Json(json!({"message":"菜单配置已保存"})))
}
async fn menu_reset(State(state): State<AppState>) -> Result<Json<Value>> {
    sqlx::query("DELETE FROM app_settings WHERE setting_key='custom_menu_config'")
        .execute(&state.db)
        .await?;
    Ok(Json(json!({"message":"已恢复默认菜单"})))
}
