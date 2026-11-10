use crate::{
    AppState, db,
    error::{ApiError, Result},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MODULES: [&str; 4] = ["storage", "statistics", "recent", "libraries"];

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Widget {
    x: u64,
    y: u64,
    w: u64,
    h: u64,
    #[serde(default, rename = "manualHeight")]
    manual_height: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    order: Vec<String>,
    hidden: Vec<String>,
    items: BTreeMap<String, Widget>,
    #[serde(default, rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    updated_at: Option<u64>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/dashboard/layout/{profile}", get(load).post(save))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

fn key(state: &AppState, headers: &HeaderMap, profile: &str) -> Result<String> {
    if !matches!(profile, "desktop" | "tablet" | "mobile") {
        return Err(ApiError::bad("无效的仪表盘设备布局"));
    }
    let session = state.sessions.from_headers(headers).filter(|session| {
        !session.setup_mode
            && !session.user_id.is_empty()
            && session.user_id.len() <= 128
            && session
                .user_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });
    let user = session.ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "请先登录".into()))?;
    Ok(format!("dashboard-layout-v2:{}:{profile}", user.user_id))
}

fn normalize(value: Value, profile: &str) -> Result<Value> {
    let layout: Layout =
        serde_json::from_value(value).map_err(|_| ApiError::bad("仪表盘布局格式无效"))?;
    let unique_known = |ids: &[String]| {
        ids.iter().all(|id| MODULES.contains(&id.as_str()))
            && ids.iter().collect::<BTreeSet<_>>().len() == ids.len()
    };
    let columns = match profile {
        "desktop" => 12,
        "tablet" => 6,
        "mobile" => 1,
        _ => return Err(ApiError::bad("无效的仪表盘设备布局")),
    };
    if layout.order.len() != MODULES.len()
        || !unique_known(&layout.order)
        || !unique_known(&layout.hidden)
        || layout
            .updated_at
            .is_some_and(|time| time > 9_007_199_254_740_991)
        || layout.items.iter().any(|(id, item)| {
            !MODULES.contains(&id.as_str())
                || item.w == 0
                || item.w > columns
                || item.x > columns - item.w
                || item.y > 10000
                || item.h == 0
                || item.h > 1000
        })
    {
        return Err(ApiError::bad("仪表盘布局包含无效模块或超出范围的尺寸"));
    }
    serde_json::to_value(layout).map_err(|_| ApiError::bad("仪表盘布局格式无效"))
}

async fn load(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(profile): Path<String>,
) -> Result<Json<Value>> {
    let key = key(&state, &headers, &profile)?;
    let layout = db::setting(&state.db, &key)
        .await?
        .map(|value| normalize(value, &profile))
        .transpose()?;
    Ok(Json(json!({"layout": layout})))
}

async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(profile): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let key = key(&state, &headers, &profile)?;
    if body
        .as_object()
        .is_none_or(|body| body.len() != 1 || !body.contains_key("layout"))
    {
        return Err(ApiError::bad("仪表盘保存请求格式无效"));
    }
    let layout = normalize(body["layout"].clone(), &profile)?;
    db::save_setting(&state.db, &key, &layout).await?;
    let saved = db::setting(&state.db, &key).await?;
    if saved.as_ref() != Some(&layout) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "仪表盘布局保存尚未确认，请重新读取".into(),
        ));
    }
    Ok(Json(json!({"layout": layout})))
}
