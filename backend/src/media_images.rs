use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::{details, list},
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/image_proxy", get(external))
        .route("/image_proxy/{*path}", get(emby))
        .route("/api/tmdb_images/{id}", get(tmdb))
        .route("/api/update_media_image/{id}", post(submit))
        .layer(axum::extract::DefaultBodyLimit::disable())
}
const TRANSPARENT: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==";
fn empty(status: StatusCode) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "image/png")],
        STANDARD.decode(TRANSPARENT).unwrap(),
    )
        .into_response()
}
async fn proxy(response: reqwest::Result<reqwest::Response>, fallback: StatusCode) -> Response {
    if let Ok(response) = response
        && response.status().is_success()
    {
        let kind = response.headers().get(header::CONTENT_TYPE).cloned();
        if let Ok(bytes) = response.bytes().await {
            let mut response = bytes.into_response();
            if let Some(kind) = kind {
                response.headers_mut().insert(header::CONTENT_TYPE, kind);
            }
            return response;
        }
    }
    empty(fallback)
}
async fn external(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response> {
    let url = query
        .get("url")
        .ok_or_else(|| ApiError::bad("缺少 'url' 参数"))?;
    let parsed = reqwest::Url::parse(url).map_err(|_| ApiError::bad("图片URL无效"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Ok(empty(StatusCode::BAD_REQUEST));
    }
    Ok(proxy(
        state
            .http
            .get(url)
            .header(
                "User-Agent",
                string(&*state.config.read().await, "user_agent"),
            )
            .header(
                "Referer",
                format!("{}/", parsed.origin().ascii_serialization()),
            )
            .send()
            .await,
        StatusCode::NOT_FOUND,
    )
    .await)
}
async fn emby(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let config = state.config.read().await.clone();
    let mut query = query;
    query.insert("api_key".into(), string(&config, "emby_api_key"));
    proxy(
        state
            .http
            .get(format!(
                "{}/{path}",
                string(&config, "emby_server_url").trim_end_matches('/')
            ))
            .query(&query)
            .send()
            .await,
        StatusCode::OK,
    )
    .await
}
pub fn format(images: &Value) -> Value {
    let mut result = json!({});
    for field in ["posters", "backdrops", "logos"] {
        result[field]=json!(list(&images[field]).iter().filter_map(|i|{
        let path=i["file_path"].as_str()?;Some(json!({"preview":format!("https://image.tmdb.org/t/p/w500{path}"),"original":format!("https://image.tmdb.org/t/p/original{path}"),"aspect_ratio":i.get("aspect_ratio").cloned().unwrap_or(json!(1)),"width":i["width"],"height":i["height"]}))
    }).collect::<Vec<_>>());
    }
    result
}
async fn tmdb(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    let item = details(
        &state,
        &id,
        "Type,ProviderIds,SeriesId,ParentId,IndexNumber",
    )
    .await?;
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    if tmdb.is_empty() {
        return Err(ApiError::bad("该媒体缺少TMDb ID"));
    }
    let path = match item["Type"].as_str() {
        Some("Movie") => format!("/movie/{tmdb}"),
        Some("Series") => format!("/tv/{tmdb}"),
        Some("Season") => {
            let parent = item["SeriesId"]
                .as_str()
                .or_else(|| item["ParentId"].as_str())
                .ok_or_else(|| ApiError::bad("缺少父电视剧ID"))?;
            let parent = details(&state, parent, "ProviderIds").await?;
            let id = string(&parent["ProviderIds"], "Tmdb");
            let season = item["IndexNumber"]
                .as_i64()
                .ok_or_else(|| ApiError::bad("缺少季号"))?;
            if id.is_empty() {
                return Err(ApiError::bad("所属电视剧缺少TMDb ID"));
            }
            format!("/tv/{id}/season/{season}")
        }
        _ => return Err(ApiError::bad("不支持的媒体类型")),
    };
    let data = crate::tmdb::get(
        &state,
        &path,
        &[
            ("append_to_response", "images".into()),
            (
                "include_image_language",
                "zh-CN,zh-TW,zh,en,null,ja,ko".into(),
            ),
        ],
    )
    .await?;
    if !data["images"].is_object() {
        return Err(ApiError(StatusCode::NOT_FOUND, "未找到TMDb图片".into()));
    }
    Ok(Json(format(&data["images"])))
}
pub fn destination(item: &Value, kind: &str) -> Result<(PathBuf, &'static str)> {
    let (filename, image_type) = match kind {
        "poster" => ("poster.jpg", "Primary"),
        "clearlogo" => ("clearlogo.png", "Logo"),
        "fanart" => ("fanart.jpg", "Backdrop"),
        "landscape" => ("landscape.jpg", "Thumb"),
        _ => return Err(ApiError::bad("不支持的图片类型")),
    };
    let raw = string(item, "Path");
    if raw.is_empty() || raw.starts_with("http") {
        return Err(ApiError::bad("缺少本地媒体路径"));
    }
    let path = PathBuf::from(raw);
    let mut directory = if path.is_file() {
        path.parent().unwrap().to_path_buf()
    } else {
        path
    };
    let filename = if item["Type"] == "Season" && kind == "poster" {
        let season = item["IndexNumber"]
            .as_i64()
            .ok_or_else(|| ApiError::bad("缺少季号"))?;
        directory = directory
            .parent()
            .ok_or_else(|| ApiError::bad("季目录无效"))?
            .to_path_buf();
        format!("season{season:02}-poster.jpg")
    } else {
        let name = directory.file_name().unwrap_or_default().to_string_lossy();
        if regex::Regex::new(r"(?i)^(season|s)\s*\d+|specials")
            .unwrap()
            .is_match(&name)
        {
            directory = directory
                .parent()
                .ok_or_else(|| ApiError::bad("季目录无效"))?
                .to_path_buf();
        }
        filename.into()
    };
    if !directory.is_dir() {
        return Err(ApiError::bad("图片目标目录不存在"));
    }
    Ok((directory.join(filename), image_type))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<(StatusCode, Json<Value>)> {
    let content = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut kind = String::new();
    let mut url = String::new();
    let mut bytes = vec![];
    let mut content_type = String::new();
    if content.starts_with("application/json") {
        let data: Value = serde_json::from_slice(
            &axum::body::to_bytes(request.into_body(), 20 * 1024 * 1024)
                .await
                .map_err(|_| ApiError::bad("图片请求过大"))?,
        )
        .map_err(|_| ApiError::bad("JSON无效"))?;
        kind = string(&data, "image_type");
        url = string(&data, "image_url");
    } else {
        use axum::extract::FromRequest;
        let mut multipart = axum::extract::Multipart::from_request(request, &state)
            .await
            .map_err(|_| ApiError::bad("图片上传格式无效"))?;
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|_| ApiError::bad("上传失败"))?
        {
            match field.name() {
                Some("image_type") => {
                    kind = field
                        .text()
                        .await
                        .map_err(|_| ApiError::bad("图片类型无效"))?
                }
                Some("image_url") => {
                    url = field
                        .text()
                        .await
                        .map_err(|_| ApiError::bad("图片URL无效"))?
                }
                Some("file") => {
                    content_type = field.content_type().unwrap_or("").into();
                    bytes = field
                        .bytes()
                        .await
                        .map_err(|_| ApiError::bad("图片上传失败"))?
                        .to_vec();
                }
                _ => (),
            }
        }
    }
    if kind.is_empty() || bytes.is_empty() && url.is_empty() {
        return Err(ApiError::bad("必须提供image_type及图片URL或file"));
    }
    let work = state.clone();
    let task = state
        .jobs
        .submit("替换媒体图片", move |context| async move {
            context.checkpoint().await?;
            let item = details(&work, &id, "Type,Path,IndexNumber").await?;
            let (path, image_type) = destination(&item, &kind)?;
            if bytes.is_empty() {
                let response = work
                    .http
                    .get(url)
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
                    .send()
                    .await?
                    .error_for_status()?;
                content_type = response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .into();
                bytes = response.bytes().await?.to_vec();
            }
            if content_type.is_empty() {
                content_type = if path.extension().is_some_and(|s| s == "png") {
                    "image/png"
                } else {
                    "image/jpeg"
                }
                .into();
            }
            let stage =
                path.with_file_name(format!(".etk-image-{}", uuid::Uuid::new_v4().simple()));
            tokio::fs::write(&stage, &bytes).await?;
            tokio::fs::rename(stage, &path).await?;
            let config = work.config.read().await.clone();
            let url = format!(
                "{}/Items/{id}/Images/{image_type}",
                string(&config, "emby_server_url").trim_end_matches('/')
            );
            let key = string(&config, "emby_api_key");
            let _ = work
                .http
                .delete(&url)
                .header("X-Emby-Token", &key)
                .send()
                .await;
            let mut success = false;
            for body in [bytes.clone(), STANDARD.encode(bytes).into_bytes()] {
                let response = work
                    .http
                    .post(&url)
                    .header("X-Emby-Token", &key)
                    .header(header::CONTENT_TYPE, &content_type)
                    .body(body)
                    .send()
                    .await?;
                if response.status().is_success() {
                    success = true;
                    break;
                }
            }
            crate::emby::refresh_item(&work, &id, false, &context).await?;
            if !success {
                return Err(ApiError::bad("图片已保存，但上传Emby失败"));
            }
            Ok(json!({"message":format!("{kind} 替换成功！")}))
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"替换媒体图片已提交"})),
    ))
}
