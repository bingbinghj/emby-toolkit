use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/wallpaper/{source}", get(wallpapers))
}

fn image_urls(value: &Value, source: &str) -> Vec<String> {
    let entries = if source == "emby" {
        &value["Items"]
    } else if source == "bing" {
        &value["images"]
    } else {
        &value["results"]
    };
    let mut images = vec![];
    for item in entries.as_array().into_iter().flatten() {
        let image = match source {
            "emby" => {
                let id = string(item, "Id");
                let tag = item["BackdropImageTags"]
                    .as_array()
                    .and_then(|tags| tags.first())
                    .and_then(Value::as_str);
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                {
                    continue;
                }
                let Some(tag) = tag.filter(|tag| !tag.is_empty()) else {
                    continue;
                };
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("tag", tag)
                    .append_pair("quality", "90")
                    .finish();
                format!("/image_proxy/Items/{id}/Images/Backdrop/0?{query}")
            }
            "bing" => {
                let path = string(item, "url");
                if !path.starts_with('/') || path.starts_with("//") {
                    continue;
                }
                format!("https://www.bing.com{path}")
            }
            _ => {
                let path = string(item, "backdrop_path");
                if !path.starts_with('/') || path.starts_with("//") {
                    continue;
                }
                format!("https://image.tmdb.org/t/p/original{path}")
            }
        };
        if !images.contains(&image) {
            images.push(image);
        }
    }
    images
}

async fn wallpapers(
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<Value>> {
    let data = match source.as_str() {
        "tmdb" => crate::tmdb::get(&state, "/movie/popular", &[]).await?,
        "emby" => {
            let user = string(&*state.config.read().await, "emby_user_id");
            if user.is_empty() {
                return Err(ApiError::bad("未配置 Emby 用户，无法读取媒体背景"));
            }
            state
                .emby(
                    Method::GET,
                    &format!("Users/{user}/Items"),
                    &[
                        ("IncludeItemTypes", "Movie,Series".into()),
                        ("Recursive", "true".into()),
                        ("ImageTypes", "Backdrop".into()),
                        ("Fields", "BackdropImageTags".into()),
                        ("SortBy", "Random".into()),
                        ("Limit", "50".into()),
                    ],
                    None,
                )
                .await?
        }
        "bing" => {
            state
                .http
                .get("https://www.bing.com/HPImageArchive.aspx")
                .query(&[("format", "js"), ("idx", "0"), ("n", "8")])
                .timeout(Duration::from_secs(15))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        }
        _ => return Err(ApiError::bad("不支持的壁纸来源")),
    };
    let images = image_urls(&data, &source);
    if images.is_empty() {
        return Err(ApiError::bad("壁纸来源未返回可用背景，当前壁纸已保留"));
    }
    Ok(Json(json!({"source":source,"images":images})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_return_their_own_images_and_never_expose_credentials() {
        let emby = image_urls(
            &json!({"Items":[{"Id":"101","BackdropImageTags":["tag&value"]},{"Id":"102","ImageTags":{"Primary":"poster"}},{"Id":"../secret","BackdropImageTags":["bad"]}]}),
            "emby",
        );
        assert_eq!(
            emby,
            ["/image_proxy/Items/101/Images/Backdrop/0?tag=tag%26value&quality=90"]
        );
        assert_eq!(
            image_urls(
                &json!({"images":[{"url":"/th?id=wallpaper"},{"url":"https://other.invalid/image"},{"url":"//other.invalid/image"}]}),
                "bing"
            ),
            ["https://www.bing.com/th?id=wallpaper"]
        );
        assert_eq!(
            image_urls(
                &json!({"results":[{"backdrop_path":"/backdrop.jpg"},{"poster_path":"/poster.jpg"},{"backdrop_path":null}]}),
                "tmdb"
            ),
            ["https://image.tmdb.org/t/p/original/backdrop.jpg"]
        );
    }
}
