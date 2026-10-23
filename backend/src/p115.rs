use crate::{
    AppState, db,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[path = "p115_auth.rs"]
mod auth;

pub struct Endpoints {
    pub qrcode: String,
    pub passport: String,
    pub openapi: String,
    pub webapi: String,
    pub profile: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            qrcode: "https://qrcodeapi.115.com".into(),
            passport: "https://passportapi.115.com".into(),
            openapi: "https://proapi.115.com".into(),
            webapi: "https://webapi.115.com".into(),
            profile: "https://my.115.com".into(),
        }
    }
}
#[derive(Default)]
pub struct Service {
    endpoints: Endpoints,
    auth_lock: Mutex<()>,
    oauth_session: Mutex<Option<auth::QrSession>>,
    cookie_sessions: Mutex<HashMap<String, Arc<Mutex<auth::QrSession>>>>,
}
impl Service {
    pub fn with_endpoints(endpoints: Endpoints) -> Self {
        Self {
            endpoints,
            ..Self::default()
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(auth::router())
        .route(
            "/api/p115/cookie",
            axum::routing::post(save_cookie).delete(clear_cookie),
        )
        .route("/api/p115/status", get(status))
        .route("/api/p115/directories", get(directories))
        .route("/api/p115/system/directories", get(local_directories))
}
pub fn success(value: &Value) -> bool {
    let error = ["error", "error_msg"].iter().any(|key| {
        !matches!(&value[*key], Value::Null | Value::Bool(false))
            && value[*key] != 0
            && value[*key] != "0"
            && value[*key] != ""
    });
    if error {
        return false;
    }
    if !value["state"].is_null() {
        return value["state"] == true
            || value["state"] == 1
            || value["state"] == "1"
            || value["state"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("true"));
    }
    let code = value["errno"]
        .as_i64()
        .or_else(|| value["errno"].as_str().and_then(|s| s.parse().ok()))
        .or_else(|| value["code"].as_i64())
        .or_else(|| value["code"].as_str().and_then(|s| s.parse().ok()));
    matches!(code, Some(0 | 200))
        && string(value, "message").is_empty()
        && string(value, "msg").is_empty()
}
pub fn missing(value: &Value) -> bool {
    ["errno", "code"]
        .iter()
        .any(|key| text(value, &[key]) == "20018")
        || ["message", "error_msg"]
            .iter()
            .any(|key| string(value, key).contains("文件不存在或已删除"))
}
pub fn cookie_app(cookie: &str) -> Option<&'static str> {
    let uid = cookie.split(';').find_map(|piece| {
        let (k, v) = piece.trim().split_once('=')?;
        k.eq_ignore_ascii_case("UID").then_some(v.trim_matches('"'))
    })?;
    match uid.split('_').nth(1)?.to_ascii_uppercase().as_str() {
        "R2" => Some("alipaymini"),
        "R1" => Some("wechatmini"),
        "F3" => Some("115android"),
        "D3" => Some("115ios"),
        "A1" => Some("web"),
        "H3" => Some("115ipad"),
        "I1" => Some("tv"),
        _ => None,
    }
}
fn app_label(app: &str) -> &str {
    match app {
        "alipaymini" => "支付宝",
        "wechatmini" => "微信",
        "115android" => "安卓",
        "115ios" => "iOS",
        "115ipad" => "PAD",
        "tv" => "TV",
        _ => "网页",
    }
}
fn normalize_app(app: &str) -> Result<String> {
    let app = if app.is_empty() { "alipaymini" } else { app }
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        app.as_str(),
        "alipaymini" | "wechatmini" | "115android" | "115ios" | "115ipad" | "web" | "tv"
    ) {
        return Err(ApiError::bad("不支持的 115 客户端类型"));
    }
    Ok(app)
}
fn user_agent(app: &str) -> &'static str {
    match app {
        "alipaymini" => {
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 AlipayDefined(nt:WIFI,ws:390|844|3.0) AliApp(AP/10.5.33.8143) AlipayClient/10.5.33.8143 Language/zh-Hans Region/CN"
        }
        "wechatmini" => {
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 MicroMessenger/8.0.42(0x18002a2c) NetType/WIFI Language/zh_CN"
        }
        "tv" => {
            "Mozilla/5.0 (Linux; Android 7.1.2; 115disk Build/NHG47K; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/89.0.4389.114 Safari/537.36"
        }
        "web" => {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
        }
        _ => "Mozilla/5.0",
    }
}
async fn tokens(state: &AppState) -> Result<Value> {
    Ok(db::setting(&state.db, "p115_auth_tokens")
        .await?
        .unwrap_or(json!({})))
}
async fn save_cookie(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let cookie = string(&body, "cookie").trim().to_string();
    if cookie.is_empty() {
        return Err(ApiError::bad("Cookie 不能为空"));
    }
    let app = cookie_app(&cookie).ok_or_else(|| ApiError::bad("无法从 UID 识别 Cookie 客户端"))?;
    let requested = string(&body, "app_type");
    if !requested.is_empty() && normalize_app(&requested)? != app {
        return Err(ApiError(StatusCode::CONFLICT, "Cookie 客户端不一致".into()));
    }
    db::patch_setting(
        &state.db,
        "p115_auth_tokens",
        &json!({"cookie":cookie,"app_type":app}),
    )
    .await?;
    Ok(Json(
        json!({"success":true,"message":format!("{} Cookie 已保存",app_label(app)),"client_type":app}),
    ))
}
async fn clear_cookie(State(state): State<AppState>) -> Result<Json<Value>> {
    db::patch_setting(
        &state.db,
        "p115_auth_tokens",
        &json!({"cookie":"","app_type":"web"}),
    )
    .await?;
    Ok(Json(json!({"success":true,"message":"115 Cookie 已清理"})))
}
async fn cookie_get(
    state: &AppState,
    cookie: &str,
    url: &str,
    query: &[(&str, String)],
) -> Result<Value> {
    Ok(state
        .http
        .get(url)
        .header("Cookie", cookie)
        .header(
            "User-Agent",
            user_agent(cookie_app(cookie).unwrap_or("web")),
        )
        .timeout(Duration::from_secs(30))
        .query(query)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
/// Retry expired OpenAPI credentials once; the refresh lock coalesces concurrent callers.
async fn openapi_get(state: &AppState, path: &str, query: &[(&str, String)]) -> Result<Value> {
    openapi_request(state, reqwest::Method::GET, path, query).await
}
async fn openapi_request(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    query: &[(&str, String)],
) -> Result<Value> {
    openapi_request_with_user_agent(state, method, path, query, "Mozilla/5.0").await
}
async fn openapi_request_with_user_agent(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    query: &[(&str, String)],
    user_agent: &str,
) -> Result<Value> {
    let mut access = string(&tokens(state).await?, "access_token");
    if access.is_empty() {
        return Err(ApiError::bad("未配置 115 Token"));
    }
    for attempt in 0..2 {
        let request = state
            .http
            .request(
                method.clone(),
                format!("{}{path}", state.p115.endpoints.openapi),
            )
            .header("User-Agent", user_agent)
            .bearer_auth(&access);
        let request = if method == reqwest::Method::GET {
            request.query(query)
        } else {
            request.form(query)
        };
        let response = request.send().await?;
        let unauthorized = response.status() == StatusCode::UNAUTHORIZED;
        if !unauthorized {
            response.error_for_status_ref()?;
        }
        let value: Value = if unauthorized {
            json!({"state":false})
        } else {
            response.json().await?
        };
        let expired = unauthorized
            || (!success(&value)
                && matches!(
                    text(&value, &["code"]).as_str(),
                    "40140123" | "40140124" | "40140125" | "40140126"
                ));
        if attempt == 0 && expired {
            access = auth::refresh_token(state, &access).await?;
            continue;
        }
        return Ok(value);
    }
    unreachable!()
}
pub async fn download_url(state: &AppState, pc: &str) -> Result<String> {
    download_url_with_user_agent(state, pc, "Mozilla/5.0").await
}
pub async fn download_url_with_user_agent(
    state: &AppState,
    pc: &str,
    user_agent: &str,
) -> Result<String> {
    fn url(value: &Value) -> Option<String> {
        if let Some(text) = value
            .as_str()
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        {
            return Some(text.into());
        }
        if let Some(object) = value.as_object() {
            if let Some(found) = object.get("url").and_then(url) {
                return Some(found);
            }
            return object.values().find_map(url);
        }
        None
    }
    if pc.is_empty() || !pc.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(ApiError::bad("115 PC 无效"));
    }
    let cookie = string(&tokens(state).await?, "cookie");
    if !cookie.is_empty() {
        let encrypted = crate::p115_cipher::encrypt(
            serde_json::to_string(&json!({"pickcode":pc}))
                .unwrap()
                .as_bytes(),
        );
        let attempt = async {
            let response = state
                .http
                .post(format!(
                    "{}/app/chrome/downurl",
                    state.p115.endpoints.openapi
                ))
                .header("Cookie", cookie)
                .header("User-Agent", user_agent)
                .form(&[("data", encrypted)])
                .send()
                .await?
                .error_for_status()?;
            let response: Value = response.json().await?;
            if success(&response) {
                let data = if let Some(encoded) = response["data"].as_str() {
                    serde_json::from_slice(&crate::p115_cipher::decrypt(encoded)?)
                        .map_err(|_| ApiError::bad("115 下载响应无效"))?
                } else {
                    response["data"].clone()
                };
                if let Some(url) = url(&data) {
                    return Ok::<_, ApiError>(url);
                }
            }
            Err(ApiError::bad("115 Cookie 未返回下载地址"))
        }
        .await;
        if let Ok(url) = attempt {
            return Ok(url);
        }
    }
    let response = openapi_request_with_user_agent(
        state,
        reqwest::Method::POST,
        "/open/ufile/downurl",
        &[("pick_code", pc.into())],
        user_agent,
    )
    .await?;
    if success(&response)
        && let Some(url) = url(&response["data"])
    {
        return Ok(url);
    }
    Err(ApiError::bad("115 未返回文件下载地址"))
}
pub async fn file_info(state: &AppState, id: u64) -> Result<Value> {
    let response = file_info_response(state, id).await?;
    if !success(&response) {
        return Err(ApiError::bad("115文件信息查询失败"));
    }
    Ok(response["data"]
        .as_array()
        .and_then(|v| v.first())
        .unwrap_or(&response["data"])
        .clone())
}
pub async fn file_info_response(state: &AppState, id: u64) -> Result<Value> {
    let auth = tokens(state).await?;
    let cookie = string(&auth, "cookie");
    let query = [("file_id", id.to_string())];
    let response = if !cookie.is_empty() {
        cookie_get(
            state,
            &cookie,
            &format!("{}/files/file", state.p115.endpoints.webapi),
            &query,
        )
        .await?
    } else {
        openapi_get(state, "/open/folder/get_info", &query).await?
    };
    Ok(response)
}
pub async fn search(state: &AppState, value: &str) -> Result<Value> {
    let query = [
        ("search_value", value.into()),
        ("cid", "0".into()),
        ("limit", "100".into()),
        ("offset", "0".into()),
        ("show_dir", "1".into()),
    ];
    let cookie = string(&tokens(state).await?, "cookie");
    if cookie.is_empty() {
        openapi_get(state, "/open/ufile/search", &query).await
    } else {
        cookie_get(
            state,
            &cookie,
            &format!("{}/files/search", state.p115.endpoints.webapi),
            &query,
        )
        .await
    }
}
pub async fn mkdir(state: &AppState, parent: &str, name: &str) -> Result<Value> {
    let cookie = string(&tokens(state).await?, "cookie");
    if cookie.is_empty() {
        openapi_request(
            state,
            reqwest::Method::POST,
            "/open/folder/add",
            &[("pid", parent.into()), ("file_name", name.into())],
        )
        .await
    } else {
        Ok(state
            .http
            .post(format!("{}/files/add", state.p115.endpoints.webapi))
            .header("Cookie", cookie)
            .header("User-Agent", "Mozilla/5.0")
            .form(&[("pid", parent), ("cname", name)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
pub async fn delete_files(state: &AppState, ids: &[String]) -> Result<()> {
    if ids.is_empty()
        || ids
            .iter()
            .any(|id| id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ApiError::bad("115 删除文件 ID 无效"));
    }
    let cookie = string(&tokens(state).await?, "cookie");
    let response = if cookie.is_empty() {
        openapi_request(
            state,
            reqwest::Method::POST,
            "/open/ufile/delete",
            &[("file_ids", ids.join(","))],
        )
        .await?
    } else {
        let mut fields = vec![("ignore_warn".to_string(), "1".to_string())];
        if ids.len() == 1 {
            fields.push(("fid".into(), ids[0].clone()));
        } else {
            fields.extend(
                ids.iter()
                    .enumerate()
                    .map(|(i, id)| (format!("fid[{i}]"), id.clone())),
            );
        }
        state
            .http
            .post(format!("{}/rb/delete", state.p115.endpoints.webapi))
            .header("Cookie", cookie)
            .header("User-Agent", "Mozilla/5.0")
            .form(&fields)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?
    };
    if !success(&response) {
        return Err(ApiError::bad("115 删除失败，已保留本地源文件"));
    }
    Ok(())
}
pub async fn move_files(state: &AppState, ids: &[String], parent: &str) -> Result<Value> {
    let cookie = string(&tokens(state).await?, "cookie");
    if cookie.is_empty() {
        openapi_request(
            state,
            reqwest::Method::POST,
            "/open/ufile/move",
            &[("file_ids", ids.join(",")), ("to_cid", parent.into())],
        )
        .await
    } else {
        let mut fields = vec![("pid".to_string(), parent.to_string())];
        fields.extend(
            ids.iter()
                .enumerate()
                .map(|(i, id)| (format!("fid[{i}]"), id.clone())),
        );
        Ok(state
            .http
            .post(format!("{}/files/move", state.p115.endpoints.webapi))
            .header("Cookie", cookie)
            .header("User-Agent", "Mozilla/5.0")
            .form(&fields)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
pub async fn rename(state: &AppState, id: &str, name: &str) -> Result<()> {
    let auth = tokens(state).await?;
    let cookie = string(&auth, "cookie");
    let response = if !cookie.is_empty() {
        let response: Value = state
            .http
            .post(format!(
                "{}/files/batch_rename",
                state.p115.endpoints.webapi
            ))
            .header("Cookie", cookie)
            .header("User-Agent", "Mozilla/5.0")
            .form(&[(format!("files_new_name[{id}]"), name)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        response
    } else {
        openapi_request(
            state,
            reqwest::Method::POST,
            "/open/ufile/update",
            &[("file_id", id.into()), ("file_name", name.into())],
        )
        .await?
    };
    if !success(&response) {
        return Err(ApiError::bad("115重命名失败"));
    }
    Ok(())
}
async fn status(State(state): State<AppState>) -> Result<Json<Value>> {
    let auth = tokens(&state).await?;
    let cookie = string(&auth, "cookie");
    let token = string(&auth, "access_token");
    let app = cookie_app(&cookie).unwrap_or("web");
    let mut result = json!({"has_token":!token.is_empty(),"has_cookie":!cookie.is_empty(),"cookie_valid":null,
        "cookie_app_type":if cookie.is_empty(){None}else{Some(app)},"cookie_app_label":if cookie.is_empty(){None}else{Some(app_label(app))},
        "valid":false,"msg":"未配置任何凭证","user_info":null});
    if !cookie.is_empty() {
        match cookie_get(
            &state,
            &cookie,
            &format!("{}/files", state.p115.endpoints.webapi),
            &[("cid", "0".into()), ("limit", "1".into())],
        )
        .await
        {
            Ok(response) if success(&response) => {
                result["valid"] = json!(true);
                result["cookie_valid"] = json!(true);
                result["msg"] = json!(format!("Cookie 凭证正常（{}）", app_label(app)));
                let mut user = json!({"user_name":"Cookie用户(正常)"});
                if let Ok(profile) = cookie_get(
                    &state,
                    &cookie,
                    &format!("{}/", state.p115.endpoints.profile),
                    &[("ct", "ajax".into()), ("ac", "get_user_aq".into())],
                )
                .await
                    && success(&profile)
                {
                    let raw = &profile["data"];
                    user = raw.clone();
                    if !user.is_object() {
                        user = json!({});
                    }
                    user["user_name"] = json!(
                        raw["uname"]
                            .as_str()
                            .or_else(|| raw["user_name"].as_str())
                            .unwrap_or("Cookie用户")
                    );
                    user["user_face_m"] = raw["face"]["face_m"]
                        .as_str()
                        .or_else(|| raw["face"]["face_s"].as_str())
                        .or_else(|| raw["user_face_m"].as_str())
                        .map_or(Value::Null, |v| json!(v));
                    let mut vip = raw["vip"].as_object().cloned().unwrap_or_default();
                    if vip
                        .get("level_name")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                    {
                        vip.insert(
                            "level_name".into(),
                            json!(if vip.get("is_vip").is_some_and(|v| v == true || v == 1) {
                                "VIP"
                            } else {
                                "普通用户"
                            }),
                        );
                    }
                    user["vip_info"] = json!(vip);
                }
                if let Ok(space) = cookie_get(
                    &state,
                    &cookie,
                    &format!("{}/files/index_info", state.p115.endpoints.webapi),
                    &[("count_space_nums", "0".into())],
                )
                .await
                    && success(&space)
                {
                    user["rt_space_info"] = space["data"]["space_info"].clone();
                }
                result["user_info"] = user;
                return Ok(Json(json!({"status":"success","data":result})));
            }
            Ok(_) => {
                result["cookie_valid"] = json!(false);
                result["msg"] = json!("Cookie 已失效或被风控拦截");
            }
            Err(_) => result["msg"] = json!("Cookie 检查失败"),
        }
    }
    if !token.is_empty() {
        let response = match openapi_get(&state, "/open/user/info", &[]).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!("115 Token 检查失败: {}", error.1);
                result["msg"] = json!("Token 检查失败，请重新授权");
                return Ok(Json(json!({"status":"success","data":result})));
            }
        };
        if success(&response) {
            result["valid"] = json!(true);
            result["msg"] = json!(if cookie.is_empty() {
                "OpenAPI Token 有效"
            } else {
                "Cookie 不可用，OpenAPI Token 可用"
            });
            result["user_info"] = response["data"].clone();
        } else {
            result["msg"] = json!("Token 无效");
        }
    }
    Ok(Json(json!({"status":"success","data":result})))
}
fn text(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| match &value[*key] {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}
fn is_directory(item: &Value) -> bool {
    for key in ["fc", "file_category", "type"] {
        if !item[key].is_null() {
            return text(item, &[key]) == "0";
        }
    }
    ["is_dir", "is_folder", "is_directory"]
        .iter()
        .any(|key| item[*key] == true || item[*key] == 1)
}
async fn directories(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<(StatusCode, Json<Value>)> {
    let cid = params
        .get("cid")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or("0".into());
    let auth = tokens(&state).await?;
    let cookie = string(&auth, "cookie");
    let access = string(&auth, "access_token");
    if cookie.is_empty() && access.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"code":400,"message":"未配置可用的 115 Token/Cookie"})),
        ));
    }
    let query = [
        ("cid", cid.clone()),
        ("limit", "1000".into()),
        ("offset", "0".into()),
        ("show_dir", "1".into()),
    ];
    let prefer_openapi = string(&*state.config.read().await, "p115_api_priority") == "openapi";
    let mut response = None;
    for use_openapi in [prefer_openapi, !prefer_openapi] {
        let attempt = if use_openapi && !access.is_empty() {
            Some(openapi_get(&state, "/open/ufile/files", &query).await)
        } else if !use_openapi && !cookie.is_empty() {
            Some(
                cookie_get(
                    &state,
                    &cookie,
                    &format!("{}/files", state.p115.endpoints.webapi),
                    &query,
                )
                .await,
            )
        } else {
            None
        };
        if let Some(attempt) = attempt {
            match attempt {
                Ok(value) if success(&value) => {
                    response = Some(value);
                    break;
                }
                Ok(_) => tracing::warn!("115 目录查询失败，尝试备用凭证"),
                Err(error) => tracing::warn!("115 目录查询异常: {}", error.1),
            }
        }
    }
    let Some(response) = response.filter(success) else {
        return Ok((
            StatusCode::BAD_GATEWAY,
            Json(json!({"code":502,"message":"读取 115 目录失败，请检查凭证"})),
        ));
    };
    let breadcrumbs: Vec<_> = response["path"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            (
                text(v, &["fid", "file_id", "id", "cid"]),
                text(v, &["name", "fn", "file_name", "n"]),
            )
        })
        .collect();
    let current = breadcrumbs
        .iter()
        .filter(|(id, _)| id != "0")
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join("/");
    let mut dirs = vec![];
    if breadcrumbs.len() > 1 {
        let parent = &breadcrumbs[breadcrumbs.len() - 2];
        let path = breadcrumbs[..breadcrumbs.len() - 1]
            .iter()
            .filter(|(id, _)| id != "0")
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join("/");
        dirs.push(json!({"name":".. [返回上一级]","cid":parent.0,"path":path,"is_parent":true}));
    }
    let mut children = vec![];
    for item in response["data"].as_array().into_iter().flatten() {
        if !is_directory(item) {
            continue;
        }
        let name = text(item, &["name", "fn", "file_name", "n"]);
        let id = text(item, &["fid", "file_id", "id", "cid"]);
        if !name.is_empty() && !id.is_empty() {
            children.push(json!({"name":name,"cid":id,"path":if current.is_empty(){name.clone()}else{format!("{current}/{name}")},"is_parent":false}));
        }
    }
    children.sort_by_key(|v| string(v, "name"));
    dirs.extend(children);
    Ok((
        StatusCode::OK,
        Json(json!({"code":200,"data":dirs,"current_cid":cid,"current_path":current})),
    ))
}
async fn local_directories(
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let path = std::path::PathBuf::from(
        params
            .get("path")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or("/".into()),
    );
    match local_directory_list(path).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (
            error.0,
            Json(json!({"code":error.0.as_u16(),"message":error.1})),
        ),
    }
}
async fn local_directory_list(path: std::path::PathBuf) -> Result<Value> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(local_directory_error)?;
    if !metadata.is_dir() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "目录不存在或不是文件夹".into(),
        ));
    }
    let mut rows = vec![];
    if let Some(parent) = path.parent()
        && parent != path
        && !parent.as_os_str().is_empty()
    {
        rows.push(json!({"name":".. [返回上一级]","path":parent,"is_parent":true}));
    }
    let mut children = vec![];
    let mut entries = tokio::fs::read_dir(&path)
        .await
        .map_err(local_directory_error)?;
    while let Some(entry) = entries.next_entry().await.map_err(local_directory_error)? {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with('.') && entry.path().is_dir() {
            children.push(json!({"name":name,"path":entry.path(),"is_parent":false}));
        }
    }
    children.sort_by_key(|v| string(v, "name"));
    rows.extend(children);
    Ok(json!({"code":200,"data":rows,"current_path":path}))
}
fn local_directory_error(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => ApiError(
            StatusCode::FORBIDDEN,
            "没有权限访问该目录，请检查 Docker 映射或系统权限".into(),
        ),
        std::io::ErrorKind::NotFound => {
            ApiError(StatusCode::NOT_FOUND, "目录不存在或不是文件夹".into())
        }
        _ => error.into(),
    }
}
pub async fn files(state: &AppState, parent: &str, offset: usize) -> Result<Value> {
    let auth = tokens(state).await?;
    let query = [
        ("cid", parent.into()),
        ("offset", offset.to_string()),
        ("limit", "1000".into()),
        ("show_dir", "1".into()),
    ];
    let prefer_open = string(&*state.config.read().await, "p115_api_priority") == "openapi";
    for use_open in [prefer_open, !prefer_open] {
        let response = if use_open && !string(&auth, "access_token").is_empty() {
            openapi_get(state, "/open/ufile/files", &query).await
        } else if !use_open && !string(&auth, "cookie").is_empty() {
            cookie_get(
                state,
                &string(&auth, "cookie"),
                &format!("{}/files", state.p115.endpoints.webapi),
                &query,
            )
            .await
        } else {
            continue;
        };
        if let Ok(response) = response {
            if success(&response) {
                return Ok(response);
            }
            if missing(&response) {
                return Err(ApiError(
                    StatusCode::NOT_FOUND,
                    "115 目录不存在或已删除".into(),
                ));
            }
        }
    }
    Err(ApiError::bad("读取 115 文件列表失败，请检查凭证"))
}
