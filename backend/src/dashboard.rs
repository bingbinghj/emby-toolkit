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

async fn music_count(state: &AppState, user: &str) -> Result<u64> {
    // MoviePilot reports albums when Emby has album records, then falls back
    // to songs for libraries without them.
    match count(state, user, "MusicAlbum").await? {
        0 => count(state, user, "Audio").await,
        albums => Ok(albums),
    }
}

async fn statistics(state: &AppState, user: &str) -> Value {
    let mut result = json!({});
    let (movies, series, episodes, music) = tokio::join!(
        count(state, user, "Movie"),
        count(state, user, "Series"),
        count(state, user, "Episode"),
        music_count(state, user)
    );
    for ((key, _), value) in [
        ("movie_count", "Movie"),
        ("tv_count", "Series"),
        ("episode_count", "Episode"),
        ("music_count", "Audio"),
    ]
    .into_iter()
    .zip([movies, series, episodes, music])
    {
        match value {
            Ok(count) => result[key] = json!({"count":count}),
            Err(error) => result[key] = json!({"count":null,"error":error.1}),
        }
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::media_organize_history::monthly_statistics(state, chrono::Local::now()),
    )
    .await
    {
        Ok(Ok(monthly)) => {
            for key in ["movie_count", "tv_count", "episode_count", "music_count"] {
                result[key]["month_count"] = monthly[key].clone();
            }
            result["month"] = monthly["month"].clone();
        }
        error => {
            let reason = match error {
                Ok(Err(error)) => error.1,
                _ => "本月入库统计读取超时".into(),
            };
            for key in ["movie_count", "tv_count", "episode_count", "music_count"] {
                result[key]["month_count"] = Value::Null;
            }
            result["month"] = json!({"period":chrono::Local::now().format("%Y-%m").to_string(),"state":"unavailable","source":"etk-history","error":reason});
        }
    }
    result
}

async fn recent(state: &AppState, user: &str) -> Result<Value> {
    // MoviePilot Emby.get_latest uses Latest's native grouping: a newly added
    // episode surfaces its series, rather than filling the dashboard with episodes.
    let response = state
        .emby(
            Method::GET,
            &format!("Users/{user}/Items/Latest"),
            &[
                ("MediaTypes", "Video".into()),
                ("Limit", "100".into()),
                (
                    "Fields",
                    "ProductionYear,ImageTags,PrimaryImageAspectRatio".into(),
                ),
                ("ImageTypeLimit", "1".into()),
            ],
            None,
        )
        .await?;
    let items = response
        .as_array()
        .ok_or_else(|| ApiError::bad("Emby 最新入库响应无效"))?;
    let mut seen = HashSet::new();
    let mut result = vec![];
    for item in items {
        if !matches!(string(item, "Type").as_str(), "Movie" | "Series") {
            continue;
        }
        let id = string(item, "Id");
        if !valid_id(&id) || !seen.insert(id.clone()) {
            return Err(ApiError::bad("Emby 最新入库包含无效或重复媒体 ID"));
        }
        result.push(json!({"id":id,"name":item["Name"],"item_type":item["Type"],
            "production_year":item["ProductionYear"],"image_tag":item["ImageTags"]["Primary"]}));
        if result.len() == 100 {
            break;
        }
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
    let counts = futures_util::future::join_all(result.iter().map(|library| async {
        let kind = match string(library, "collection_type").as_str() {
            "movies" | "movie" => "Movie",
            "tvshows" | "tv" => "Series",
            "music" => "MusicAlbum",
            _ => "Movie,Series",
        };
        let response = state
            .emby(
                Method::GET,
                &format!("Users/{user}/Items"),
                &[
                    ("ParentId", string(library, "id")),
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
            .ok_or_else(|| ApiError::bad("Emby 未返回有效媒体库数量"))
    }))
    .await;
    for (library, count) in result.iter_mut().zip(counts) {
        match count {
            Ok(count) => library["item_count"] = json!(count),
            Err(error) => {
                library["item_count"] = Value::Null;
                library["count_error"] = json!(error.1);
            }
        }
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
