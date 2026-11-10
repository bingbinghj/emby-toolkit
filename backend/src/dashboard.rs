use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{Json, Router, extract::State, routing::get};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/dashboard/media", get(media))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

async fn count(state: &AppState, user: &str, kind: &str) -> Result<u64> {
    let response = state
        .emby(
            Method::GET,
            &format!("Users/{user}/Items"),
            &[
                ("IncludeItemTypes", kind.into()),
                ("Recursive", "true".into()),
                ("Limit", "0".into()),
                ("EnableTotalRecordCount", "true".into()),
            ],
            None,
        )
        .await?;
    response["TotalRecordCount"]
        .as_u64()
        .ok_or_else(|| ApiError::bad("Emby 未返回有效媒体统计"))
}

async fn statistics(state: &AppState, user: &str) -> Value {
    let mut result = json!({});
    let values = futures_util::future::join_all(
        ["Movie", "Series", "Episode", "Audio"]
            .iter()
            .map(|kind| count(state, user, kind)),
    )
    .await;
    for ((key, _), value) in [
        ("movie_count", "Movie"),
        ("tv_count", "Series"),
        ("episode_count", "Episode"),
        ("music_count", "Audio"),
    ]
    .into_iter()
    .zip(values)
    {
        match value {
            Ok(count) => result[key] = json!({"count":count}),
            Err(error) => result[key] = json!({"count":null,"error":error.1}),
        }
    }
    result
}

async fn recent(state: &AppState, user: &str) -> Result<Value> {
    let response = state.emby(Method::GET, &format!("Users/{user}/Items"), &[
        ("IncludeItemTypes", "Movie,Series,Episode,MusicAlbum".into()),
        ("Recursive", "true".into()), ("SortBy", "DateCreated".into()),
        ("SortOrder", "Descending".into()), ("Limit", "24".into()),
        ("Fields", "DateCreated,ImageTags,PrimaryImageAspectRatio,SeriesName,SeriesId,SeriesPrimaryImageTag,ParentIndexNumber,IndexNumber,ProductionYear".into()),
        ("ImageTypeLimit", "1".into()),
    ], None).await?;
    let items = response["Items"]
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 最新入库响应无效"))?;
    let mut seen = HashSet::new();
    let mut result = vec![];
    for item in items {
        let id = string(item, "Id");
        if !valid_id(&id) || !seen.insert(id.clone()) {
            return Err(ApiError::bad("Emby 最新入库包含无效或重复媒体 ID"));
        }
        result.push(json!({"id":id,"name":item["Name"],"item_type":item["Type"],
            "production_year":item["ProductionYear"],"image_tag":item["ImageTags"]["Primary"],
            "series_id":item["SeriesId"],"series_image_tag":item["SeriesPrimaryImageTag"],
            "series_name":item["SeriesName"],"season_number":item["ParentIndexNumber"],
            "episode_number":item["IndexNumber"],"date_created":item["DateCreated"]}));
    }
    Ok(json!(result))
}

async fn libraries(state: &AppState, user: &str) -> Result<Value> {
    let response = state
        .emby(
            Method::GET,
            &format!("Users/{user}/Views"),
            &[("Fields", "ImageTags,PrimaryImageAspectRatio".into())],
            None,
        )
        .await?;
    let items = response["Items"]
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 我的媒体库响应无效"))?;
    let mut seen = HashSet::new();
    let mut result = vec![];
    for item in items {
        let id = string(item, "Id");
        if !valid_id(&id) || !seen.insert(id.clone()) {
            return Err(ApiError::bad("Emby 媒体库包含无效或重复 ID"));
        }
        if matches!(
            string(item, "CollectionType").as_str(),
            "boxsets" | "playlists"
        ) {
            continue;
        }
        result.push(
            json!({"id":id,"name":item["Name"],"collection_type":item["CollectionType"],
            "image_tag":item["ImageTags"]["Primary"]}),
        );
    }
    Ok(json!(result))
}

async fn media(State(state): State<AppState>) -> Result<Json<Value>> {
    let user = string(&*state.config.read().await, "emby_user_id");
    if !valid_id(&user) {
        return Err(ApiError::bad("未配置有效 Emby 用户 ID"));
    }
    let (statistics, recent, libraries) = tokio::join!(
        statistics(&state, &user),
        recent(&state, &user),
        libraries(&state, &user)
    );
    let mut response = json!({"statistics":statistics,"recent":null,"libraries":null,"errors":{}});
    for (key, value) in [("recent", recent), ("libraries", libraries)] {
        match value {
            Ok(value) => response[key] = value,
            Err(error) => response["errors"][key] = json!(error.1),
        }
    }
    Ok(Json(response))
}
