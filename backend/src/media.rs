use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/emby_libraries", get(libraries_simple))
        .route("/api/media_center/libraries", get(libraries))
        .route("/api/media_center/libraries/{id}/items", get(library_items))
        .route("/api/emby/user/{id}/views", get(user_views))
}
fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err(ApiError::bad("无效的媒体 ID"));
    }
    Ok(())
}
async fn user_id(state: &AppState) -> Result<String> {
    let id = string(&*state.config.read().await, "emby_user_id");
    valid_id(&id).map_err(|_| ApiError::bad("未配置 Emby 用户 ID"))?;
    Ok(id)
}
async fn views(state: &AppState) -> Result<Vec<Value>> {
    let response = state
        .emby(
            Method::GET,
            &format!("emby/Users/{}/Views", user_id(state).await?),
            &[],
            None,
        )
        .await?;
    response["Items"]
        .as_array()
        .cloned()
        .ok_or_else(|| ApiError(StatusCode::BAD_GATEWAY, "Emby 媒体库响应无效".into()))
}
fn filter(library: &Value) -> &'static str {
    match string(library, "CollectionType").to_lowercase().as_str() {
        "movie" | "movies" => "Movie",
        "tvshows" | "tv" => "Series",
        "music" => "MusicAlbum",
        _ => "Movie,Series",
    }
}
fn library_supported(library: &Value) -> bool {
    !string(library, "Id").is_empty()
        && !string(library, "Name").is_empty()
        && !matches!(
            string(library, "CollectionType").to_lowercase().as_str(),
            "boxsets" | "playlists"
        )
}
async fn items(state: &AppState, query: &[(&str, String)]) -> Result<Value> {
    state
        .emby(
            Method::GET,
            &format!("Users/{}/Items", user_id(state).await?),
            query,
            None,
        )
        .await
}
async fn libraries_simple(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(
        views(&state)
            .await?
            .iter()
            .filter(|v| library_supported(v) && filter(v) != "MusicAlbum")
            .map(|v| json!({"Name":v["Name"],"Id":v["Id"]}))
            .collect::<Vec<_>>()
    )))
}
async fn libraries(State(state): State<AppState>) -> Result<Json<Value>> {
    let mut result = vec![];
    for library in views(&state).await?.iter().filter(|v| library_supported(v)) {
        let count = items(
            &state,
            &[
                ("ParentId", string(library, "Id")),
                ("IncludeItemTypes", filter(library).into()),
                ("Recursive", "true".into()),
                ("Limit", "0".into()),
            ],
        )
        .await;
        let count = count
            .ok()
            .and_then(|v| v["TotalRecordCount"].as_u64())
            .unwrap_or(0);
        result.push(json!({"id":library["Id"],"name":library["Name"],"collection_type":library["CollectionType"],"image_tag":library["ImageTags"]["Primary"],"item_count":count}));
    }
    Ok(Json(json!({"libraries":result})))
}
fn number(params: &HashMap<String, String>, key: &str, default: i64) -> Result<i64> {
    params.get(key).map_or(Ok(default), |v| {
        v.parse().map_err(|_| ApiError::bad("分页参数无效"))
    })
}
async fn anchor(state: &AppState, common: &[(&str, String)], letter: &str) -> Result<u64> {
    let mut query = common.to_vec();
    query.push(("Limit", "0".into()));
    let total = items(state, &query).await?["TotalRecordCount"]
        .as_u64()
        .unwrap_or(0);
    query.push(("NameStartsWithOrGreater", letter.into()));
    let after = items(state, &query).await?["TotalRecordCount"]
        .as_u64()
        .unwrap_or(0);
    Ok(total.saturating_sub(after))
}
async fn library_items(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    valid_id(&id)?;
    let page = number(&params, "page", 1)?.max(1);
    let size = number(&params, "page_size", 48)?.clamp(1, 100) as u64;
    let explicit_start = params
        .get("start_index")
        .is_some_and(|s| !s.trim().is_empty());
    let library = views(&state)
        .await?
        .into_iter()
        .find(|v| string(v, "Id") == id && library_supported(v))
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "媒体库不存在或不支持浏览".into()))?;
    let mut query = vec![
        ("ParentId", id),
        ("IncludeItemTypes", filter(&library).into()),
        ("Recursive", "true".into()),
        ("SortBy", "SortName".into()),
        ("SortOrder", "Ascending".into()),
    ];
    if let Some(search) = params.get("search").filter(|s| !s.trim().is_empty()) {
        query.push(("SearchTerm", search.trim().into()));
    }
    let letter = params.get("letter").map_or("", String::as_str).trim();
    let anchor_index = if explicit_start {
        number(&params, "start_index", 0)?.max(0) as u64
    } else if !letter.is_empty() {
        anchor(&state, &query, letter).await.unwrap_or(0)
    } else {
        0
    };
    let start = if explicit_start {
        anchor_index
    } else {
        anchor_index / size * size
    };
    query.extend([("StartIndex",start.to_string()),("Limit",size.to_string()),("Fields","PrimaryImageAspectRatio,ImageTags,BackdropImageTags,Name,Type,ProductionYear,ProviderIds,ChildCount,CommunityRating,OfficialRating".into())]);
    let response = items(&state, &query).await?;
    let list = response["Items"]
        .as_array()
        .cloned()
        .ok_or_else(|| ApiError(StatusCode::BAD_GATEWAY, "Emby 媒体项响应无效".into()))?;
    Ok(Json(
        json!({"library":{"id":library["Id"],"name":library["Name"],"collection_type":library["CollectionType"],"media_type_filter":filter(&library)},
        "next_start_index":start+list.len() as u64,"items":list,"total":response["TotalRecordCount"].as_u64().unwrap_or(0),"start_index":start,
        "anchor_index":anchor_index,"anchor_item_id":"","page":page,"page_size":size}),
    ))
}
async fn user_views(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    valid_id(&id)?;
    let token = headers
        .get("X-Emby-Token")
        .and_then(|v| v.to_str().ok())
        .or_else(|| params.get("api_key").map(String::as_str))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad("缺少用户访问令牌(api_key或X-Emby-Token)"))?
        .to_string();
    let mut params = params;
    params.insert("api_key".into(), token.clone());
    let mut forwarded = headers;
    forwarded.remove("Host");
    forwarded.remove("Accept-Encoding");
    let base = string(&*state.config.read().await, "emby_server_url");
    let response = state
        .http
        .get(format!(
            "{}/emby/Users/{id}/Views",
            base.trim_end_matches('/')
        ))
        .headers(forwarded)
        .header("Accept-Encoding", "identity")
        .header("X-Emby-Token", token)
        .query(&params)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(Json(response))
}
