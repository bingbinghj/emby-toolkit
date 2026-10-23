use super::*;
use axum::{
    response::{Html, IntoResponse, Response},
    routing::post,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::time::Instant;

pub(super) struct QrSession {
    params: Vec<(&'static str, String)>,
    app: String,
    verifier: String,
    created: Instant,
    result: Option<Value>,
}
impl QrSession {
    fn new(data: &Value, app: String, verifier: String) -> Result<Self> {
        let params: Vec<_> = ["uid", "time", "sign"]
            .into_iter()
            .map(|key| (key, text(data, &[key])))
            .collect();
        if params.iter().any(|(_, value)| value.is_empty()) {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "二维码登录参数不完整".into(),
            ));
        }
        Ok(Self {
            params,
            app,
            verifier,
            created: Instant::now(),
            result: None,
        })
    }
    fn expired(&self) -> bool {
        self.created.elapsed() > Duration::from_secs(600)
    }
    fn uid(&self) -> &str {
        &self.params[0].1
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/p115/qrcode", post(oauth_qrcode))
        .route("/api/p115/qrcode/status", get(oauth_status))
        .route("/api/p115/cookie_qrcode", get(cookie_qrcode))
        .route("/api/p115/cookie_qrcode/status", get(cookie_status))
        .route("/api/p115/auto_save_auth", get(auto_save_auth))
}
fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 40];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}
async fn checked(request: reqwest::RequestBuilder, message: &str) -> Result<Value> {
    let response: Value = request
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !success(&response) {
        return Err(ApiError(StatusCode::BAD_GATEWAY, message.into()));
    }
    Ok(response)
}
async fn oauth_qrcode(State(state): State<AppState>) -> Result<Json<Value>> {
    let app_id = string(&*state.config.read().await, "p115_app_id")
        .trim()
        .to_string();
    if app_id.is_empty() {
        return Err(ApiError::bad("未配置自定义 AppID，请先在设置中保存"));
    }
    let mut session = state.p115.oauth_session.lock().await;
    let (verifier, challenge) = pkce_pair();
    let response = checked(
        state
            .http
            .post(format!(
                "{}/open/authDeviceCode",
                state.p115.endpoints.passport
            ))
            .form(&[
                ("client_id", app_id),
                ("code_challenge", challenge),
                ("code_challenge_method", "sha256".into()),
            ]),
        "获取二维码失败",
    )
    .await?;
    let data = &response["data"];
    *session = Some(QrSession::new(data, String::new(), verifier)?);
    Ok(Json(
        json!({"success":true,"data":{"qrcode":data["qrcode"],"uid":data["uid"]}}),
    ))
}
fn expired(message: &str) -> Value {
    json!({"success":false,"status":"expired","message":message})
}
fn waiting(message: &str) -> Value {
    json!({"success":true,"status":"waiting","message":message})
}

/// None means the device confirmed login. Other values are terminal or waiting UI responses.
async fn poll(state: &AppState, session: &QrSession) -> Result<Option<Value>> {
    if session.expired() {
        return Ok(Some(expired("二维码已过期，请重新获取")));
    }
    let response = state
        .http
        .get(format!("{}/get/status/", state.p115.endpoints.qrcode))
        .query(&session.params)
        .timeout(Duration::from_secs(30))
        .send()
        .await;
    let response = match response {
        Err(error) if error.is_timeout() => return Ok(Some(waiting("轮询超时，继续等待..."))),
        other => other?,
    };
    let response: Value = response.error_for_status()?.json().await?;
    if !success(&response) {
        let message = text(&response, &["message", "msg", "error", "error_msg"]);
        if response["state"] == 0
            || response["state"] == false
            || message.to_ascii_lowercase().contains("key invalid")
            || message.contains("过期")
        {
            return Ok(Some(expired("二维码已过期，请重新获取")));
        }
        return Err(ApiError(
            StatusCode::BAD_GATEWAY,
            "检查二维码状态失败".into(),
        ));
    }
    let status = text(&response["data"], &["status"]);
    Ok(match status.as_str() {
        "2" => None,
        "1" => Some(waiting("已扫码，请在设备上确认")),
        "0" | "" => Some(waiting("等待扫码...")),
        "-1" => Some(expired("二维码已过期")),
        "-2" => Some(expired("用户已取消登录")),
        _ => return Err(ApiError(StatusCode::BAD_GATEWAY, "未知扫码状态".into())),
    })
}
fn token_patch(data: &Value) -> Result<Value> {
    let access = string(data, "access_token").trim().to_string();
    let refresh = string(data, "refresh_token").trim().to_string();
    if access.is_empty() || refresh.is_empty() {
        return Err(ApiError::bad("授权数据缺少 Token"));
    }
    Ok(json!({"access_token":access,"refresh_token":refresh}))
}
async fn save_tokens(state: &AppState, patch: &Value) -> Result<()> {
    let _lock = state.p115.auth_lock.lock().await;
    db::patch_setting(&state.db, "p115_auth_tokens", patch).await
}
async fn oauth_status(State(state): State<AppState>) -> Result<Json<Value>> {
    let mut guard = state.p115.oauth_session.lock().await;
    let Some(session) = guard.as_mut() else {
        return Ok(Json(waiting("请先获取二维码")));
    };
    if session.expired() {
        return Ok(Json(expired("二维码已过期，请重新获取")));
    }
    if let Some(result) = &session.result {
        return Ok(Json(result.clone()));
    }
    if let Some(result) = poll(&state, session).await? {
        return Ok(Json(result));
    }
    let response = checked(
        state
            .http
            .post(format!(
                "{}/open/deviceCodeToToken",
                state.p115.endpoints.passport
            ))
            .form(&[("uid", session.uid()), ("code_verifier", &session.verifier)]),
        "获取 Token 失败",
    )
    .await?;
    let patch = token_patch(&response["data"])?;
    save_tokens(&state, &patch).await?;
    session.verifier.clear();
    let result = json!({"success":true,"status":"success","message":"授权成功！"});
    session.result = Some(result.clone());
    tracing::info!("115 扫码成功，Token 已保存");
    Ok(Json(result))
}
async fn cookie_qrcode(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let app = normalize_app(params.get("app").map_or("", String::as_str))?;
    // Fetch the neutral web QR token, then bind the selected client on the server.
    let response = checked(
        state.http.get(format!(
            "{}/api/1.0/web/1.0/token/",
            state.p115.endpoints.qrcode
        )),
        "获取二维码失败",
    )
    .await?;
    let data = &response["data"];
    let session = QrSession::new(data, app.clone(), String::new())?;
    let qrcode = data["qrcode"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("https://115.com/scan/dg-{}", session.uid()));
    let session_id = uuid::Uuid::new_v4().simple().to_string();
    let mut sessions = state.p115.cookie_sessions.lock().await;
    // Busy sessions are retained; their own poll checks expiration as well.
    sessions.retain(|_, session| session.try_lock().map_or(true, |value| !value.expired()));
    sessions.insert(session_id.clone(), Arc::new(Mutex::new(session)));
    Ok(Json(
        json!({"success":true,"data":{"qrcode":qrcode,"session_id":session_id,"client_type":app,"client_label":app_label(&app)}}),
    ))
}
fn login_cookie(response: &Value) -> String {
    match &response["data"]["cookie"] {
        Value::String(cookie) => cookie.trim().to_string(),
        Value::Object(pairs) => pairs
            .iter()
            .filter_map(|(key, value)| {
                let value = value.as_str()?;
                (!key.is_empty() && !value.is_empty()).then(|| format!("{key}={value}"))
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    }
}
async fn cookie_status(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let session_id = params.get("session_id").map_or("", String::as_str).trim();
    let session = state
        .p115
        .cookie_sessions
        .lock()
        .await
        .get(session_id)
        .cloned();
    let Some(session) = session else {
        return Ok(Json(expired("请先获取二维码")));
    };
    let mut session = session.lock().await;
    if session.expired() {
        return Ok(Json(expired("二维码已过期")));
    }
    if let Some(result) = &session.result {
        return Ok(Json(result.clone()));
    }
    if let Some(mut result) = poll(&state, &session).await? {
        if result["status"] == "expired" {
            session.result = Some(result.clone());
        }
        result["client_type"] = json!(session.app);
        return Ok(Json(result));
    }
    let response = checked(
        state
            .http
            .post(format!(
                "{}/app/1.0/{}/1.0/login/qrcode/",
                state.p115.endpoints.qrcode, session.app
            ))
            .form(&[("account", session.uid())]),
        "获取 Cookie 失败",
    )
    .await?;
    let cookie = login_cookie(&response);
    let actual = cookie_app(&cookie).ok_or_else(|| {
        ApiError(
            StatusCode::BAD_GATEWAY,
            "无法识别 Cookie 实际客户端，已拒绝保存".into(),
        )
    })?;
    if actual != session.app {
        session.result = Some(expired("Cookie 客户端不一致，请重新获取二维码"));
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!(
                "Cookie 客户端不一致：选择的是 {}，实际返回 {}，已拒绝保存",
                app_label(&session.app),
                app_label(actual)
            ),
        ));
    }
    db::patch_setting(
        &state.db,
        "p115_auth_tokens",
        &json!({"cookie":cookie,"app_type":actual}),
    )
    .await?;
    let result = json!({"success":true,"status":"success","message":format!("{} Cookie 获取成功",app_label(actual)),"client_type":actual,"client_label":app_label(actual)});
    session.result = Some(result.clone());
    Ok(Json(result))
}
async fn auto_save_auth(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response> {
    let data = params
        .get("data")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::bad("授权失败：缺少 Token 数据"))?;
    let data = STANDARD
        .decode(data.trim())
        .map_err(|_| ApiError::bad("无法解析授权数据"))?;
    let data: Value =
        serde_json::from_slice(&data).map_err(|_| ApiError::bad("无法解析授权数据"))?;
    save_tokens(&state, &token_patch(&data)?).await?;
    Ok(Html("<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>授权成功</title><h2>授权成功！</h2><p>Token 已自动保存到 ETK 系统。本窗口将在 3 秒后自动关闭。</p><script>setTimeout(()=>window.close(),3000)</script></html>").into_response())
}

pub(super) async fn refresh_token(state: &AppState, failed: &str) -> Result<String> {
    let _lock = state.p115.auth_lock.lock().await;
    let current = tokens(state).await?;
    let access = string(&current, "access_token");
    if !access.is_empty() && access != failed {
        return Ok(access);
    }
    let refresh = string(&current, "refresh_token");
    if refresh.is_empty() {
        return Err(ApiError::bad("115 Token 已过期，请重新扫码"));
    }
    let response = checked(
        state
            .http
            .post(format!(
                "{}/open/refreshToken",
                state.p115.endpoints.passport
            ))
            .form(&[("refresh_token", refresh)]),
        "115 Token 续期失败，请重新扫码",
    )
    .await?;
    let patch = token_patch(&response["data"])?;
    db::patch_setting(&state.db, "p115_auth_tokens", &patch).await?;
    tracing::info!("115 Token 自动续期成功");
    Ok(string(&patch, "access_token"))
}
