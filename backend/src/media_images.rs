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
use std::{collections::HashMap, path::PathBuf, time::Duration};

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
async fn proxy<R: Into<crate::emby::Response>>(
    response: reqwest::Result<R>,
    fallback: StatusCode,
) -> Response {
    if let Ok(response) = response.map(Into::into)
        && response.status().is_success()
    {
        let status = response.status();
        let kind = response.headers().get(header::CONTENT_TYPE).cloned();
        if let Ok(bytes) = response.bytes().await {
            let mut response = (status, bytes).into_response();
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
            .timeout(Duration::from_secs(15))
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
            .timeout(Duration::from_secs(20))
            .send()
            .await,
        StatusCode::NOT_FOUND,
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
// Atomically publish bytes so failed writes never truncate the current image.
async fn save_image(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let stage = path.with_file_name(format!(".etk-image-{}", uuid::Uuid::new_v4().simple()));
    let saved = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)
            .await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&stage, path).await
    }
    .await;
    if saved.is_err() {
        let _ = tokio::fs::remove_file(stage).await;
    }
    Ok(saved?)
}
async fn read_image(state: &AppState, url: &str, key: &str) -> Result<Option<Vec<u8>>> {
    let response = crate::emby::send(
        state
            .http
            .get(url)
            .header("X-Emby-Token", key)
            .header(header::CACHE_CONTROL, "no-cache")
            .timeout(Duration::from_secs(20)),
    )
    .await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let bytes = response.error_for_status()?.bytes().await?.to_vec();
    image::load_from_memory(&bytes).map_err(|_| ApiError::bad("原生图片回读无法解码"))?;
    Ok(Some(bytes))
}
async fn delete_image(state: &AppState, url: &str, key: &str) -> Result<()> {
    let response = crate::emby::send(state.http.delete(url).header("X-Emby-Token", key)).await?;
    if response.status() != StatusCode::NOT_FOUND {
        response.error_for_status()?;
    }
    Ok(())
}
async fn upload_image(
    state: &AppState,
    url: &str,
    key: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut failures = vec![];
    for body in [bytes.to_vec(), STANDARD.encode(bytes).into_bytes()] {
        match crate::emby::send(
            state
                .http
                .post(url)
                .header("X-Emby-Token", key)
                .header(header::CONTENT_TYPE, mime)
                .body(body),
        )
        .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => failures.push(format!("上传请求返回 HTTP {}", response.status())),
            Err(error) => failures.push(error.status().map_or_else(
                || "图片上传请求失败，无法访问上游服务".into(),
                |status| format!("上传请求失败：HTTP {status}"),
            )),
        }
    }
    Err(ApiError::bad(format!(
        "上传Emby失败：{}",
        failures.join("；")
    )))
}
// RGB beneath a fully transparent PNG pixel is not part of the displayed image.
fn visible_pixels(image: image::DynamicImage) -> image::RgbaImage {
    let mut pixels = image.into_rgba8();
    for pixel in pixels.pixels_mut() {
        if pixel[3] == 0 {
            *pixel = image::Rgba([0; 4]);
        }
    }
    pixels
}

fn pixel_difference(a: &image::RgbaImage, b: &image::RgbaImage) -> Option<f64> {
    if a.dimensions() != b.dimensions() {
        return None;
    }
    let sum: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum();
    Some(sum as f64 / a.as_raw().len() as f64)
}
async fn verify_image(
    state: &AppState,
    url: &str,
    key: &str,
    expected: Option<&[u8]>,
    previous: Option<&[u8]>,
) -> Result<()> {
    let expected = expected
        .map(image::load_from_memory)
        .transpose()
        .map_err(|_| ApiError::bad("待验收图片无法解码"))?
        .map(visible_pixels);
    let previous = previous
        .and_then(|bytes| image::load_from_memory(bytes).ok())
        .map(visible_pixels);
    let mut error = "原生图片验收不一致".to_string();
    for attempt in 0..5 {
        match read_image(state, url, key).await {
            Ok(None) if expected.is_none() => return Ok(()),
            Ok(Some(bytes)) if expected.is_some() => {
                let actual = visible_pixels(image::load_from_memory(&bytes).unwrap());
                let expected = expected.as_ref().unwrap();
                if let Some(difference) = pixel_difference(&actual, expected) {
                    // Allow JPEG recompression, but never accept an unchanged prior image
                    // merely because the requested replacement has similar pixels.
                    let stale = previous.as_ref().is_some_and(|previous| {
                        previous != expected
                            && pixel_difference(&actual, previous)
                                .is_some_and(|old| old <= difference)
                    });
                    if difference <= 3.0 && !stale {
                        return Ok(());
                    }
                }
            }
            Ok(_) => (),
            Err(failed) => error = format!("原生图片验收失败：{}", failed.1),
        }
        if attempt < 4 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    Err(ApiError::bad(error))
}

/// 元数据定点更新后只提交本项目的侧车图片，不调用会展开关联版本的 Refresh。
pub(crate) async fn sync_local_images(
    state: &AppState,
    target: &Value,
    context: &crate::jobs::JobContext,
) -> Result<()> {
    let id = string(target, "Id");
    let current = details(
        state,
        &id,
        "Id,Type,Path,IndexNumber,MediaSources,MediaStreams,Chapters",
    )
    .await?;
    if ["Id", "Type", "Path"]
        .iter()
        .any(|field| current[*field] != target[*field])
    {
        return Err(ApiError::bad(
            "图片定点更新所属媒体身份变化，未提交其他图片",
        ));
    }
    let paths = if current["Type"] == "Episode" {
        let path = std::path::Path::new(current["Path"].as_str().unwrap_or(""));
        vec![(
            path.with_file_name(format!(
                "{}-thumb.jpg",
                path.file_stem().unwrap_or_default().to_string_lossy()
            )),
            "Primary",
        )]
    } else if current["Type"] == "Season" {
        vec![destination(&current, "poster")?]
    } else {
        ["poster", "clearlogo", "fanart", "landscape"]
            .into_iter()
            .map(|kind| destination(&current, kind))
            .collect::<Result<Vec<_>>>()?
    };
    let config = state.config.read().await.clone();
    let key = string(&config, "emby_api_key");
    for (path, kind) in paths {
        context.checkpoint().await?;
        let Some(path) = crate::images::existing_image_path(&path).await else { continue; };
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        // 缓存中无有效图像的占位文件不属于本次已经下载成功的图片。
        let Ok(local_image) = image::load_from_memory(&bytes) else { continue; };
        let scoped = context.scoped(&format!("native-image:{kind}"));
        let url = format!(
            "{}/Items/{id}/Images/{kind}",
            string(&config, "emby_server_url").trim_end_matches('/')
        );
        let native = read_image(state, &url, &key).await?;
        let intent = if let Some(saved) = scoped.plan("intent").await? {
            if saved["id"] != id
                || saved["path"] != path.to_string_lossy().as_ref()
                || base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    string(&saved, "bytes"),
                )
                .ok()
                .as_deref()
                    != Some(bytes.as_slice())
            {
                return Err(ApiError::bad(
                    "图片定点更新的原目标或图像在中断期间变化，未覆盖当前图片",
                ));
            }
            saved
        } else {
            let intent = json!({"id":id,"path":path,
                "bytes":base64::Engine::encode(&base64::engine::general_purpose::STANDARD,&bytes),
                "previous":native.as_ref().map(|bytes|base64::Engine::encode(&base64::engine::general_purpose::STANDARD,bytes))});
            scoped.save_plan("intent", &intent).await?;
            intent
        };
        let previous = intent["previous"]
            .as_str()
            .map(|value| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, value))
            .transpose()
            .map_err(|_| ApiError::bad("原图片定点更新记录无效，未覆盖当前图片"))?;
        let already_matches = native.as_deref() == Some(bytes.as_slice())
            || native.as_deref().and_then(|data| image::load_from_memory(data).ok())
                // Ingest imports MP's existing sidecar; Emby may have already
                // stored a JPEG recompression of it. Explicit image replacement
                // still uses verify_image's stricter stale-image check.
                .is_some_and(|native_image| pixel_difference(&visible_pixels(native_image), &visible_pixels(local_image))
                    .is_some_and(|difference| difference <= 3.0));
        let imported = if already_matches {
            true
        } else if native != previous {
            verify_image(state, &url, &key, Some(&bytes), previous.as_deref())
                .await
                .map_err(|_| {
                    ApiError::bad("图片定点更新后出现其他新内容，原记录已保留，未覆盖当前图片")
                })?;
            true
        } else {
            false
        };
        if scoped.completed("done").await?.is_none() && !imported {
            let mime = image::guess_format(&bytes)
                .map_err(|_| ApiError::bad("缓存图片格式不支持"))?
                .to_mime_type();
            upload_image(state, &url, &key, mime, &bytes).await?;
        }
        if !imported {
            verify_image(state, &url, &key, Some(&bytes), previous.as_deref()).await
                .map_err(|error| ApiError(error.0, format!("{id} {kind}：{}", error.1)))?;
        }
        let live = details(
            state,
            &id,
            "Id,Type,Path,MediaSources,MediaStreams,Chapters",
        )
        .await?;
        if [
            "Id",
            "Type",
            "Path",
            "MediaSources",
            "MediaStreams",
            "Chapters",
        ]
        .iter()
        .any(|field| live[*field] != current[*field])
        {
            return Err(ApiError::bad(
                "图片定点更新后原媒体信息发生变化，未记录完整入库成功",
            ));
        }
        scoped.complete("done", &intent).await?;
    }
    Ok(())
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
    let payload = json!({"id":id,"kind":kind,"url":url,"bytes":base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)});
    let work = state.clone();
    let task = state.jobs.submit_persistent("替换媒体图片", crate::jobs::JobSpec::new("media_images", payload.clone()), move |context| async move {
        Ok(execute_persisted(&work, &payload, &context).await?.1)
    }).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"替换媒体图片已提交"})),
    ))
}

pub(crate) async fn execute_persisted(work: &AppState, payload: &Value, context: &crate::jobs::JobContext) -> Result<(u16, Value)> {
    if let Some(result) = context.completed("image-done").await? { return Ok((200,result)); }
    let id = string(payload, "id");
    let kind = string(payload, "kind");
    let url = string(payload, "url");
    let mut bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, string(payload, "bytes"))
        .map_err(|_| ApiError::bad("图片任务数据无效"))?;
    if let Some(saved) = context.plan("image-bytes").await? {
        bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, saved.as_str().ok_or_else(||ApiError::bad("图片恢复数据无效"))?)
            .map_err(|_|ApiError::bad("图片恢复数据无效"))?;
    }

            context.checkpoint().await?;
            let item = details(&work, &id, "Type,Path,IndexNumber").await?;
            let (path, image_type) = destination(&item, &kind)?;
            if bytes.is_empty() {
                let download = async {
                    let response = work
                        .http
                        .get(url)
                        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
                        .send()
                        .await?
                        .error_for_status()?;
                    Ok::<_, ApiError>(response.bytes().await?.to_vec())
                };
                bytes = tokio::select! {
                    _=context.cancelled()=>{context.checkpoint().await?;unreachable!()},
                    result=download=>result?,
                };
            }
            // Validate the actual bytes before replacing either the local or native image.
            image::load_from_memory(&bytes)
                .map_err(|_| ApiError::bad("图片内容无法解码，原图未修改"))?;
            context.save_plan("image-bytes", &json!(base64::Engine::encode(&base64::engine::general_purpose::STANDARD,&bytes))).await?;
            let content_type = image::guess_format(&bytes)
                .map_err(|_| ApiError::bad("图片格式不支持，原图未修改"))?
                .to_mime_type();
            context.checkpoint().await?;
            let config = work.config.read().await.clone();
            let url = format!(
                "{}/Items/{id}/Images/{image_type}",
                string(&config, "emby_server_url").trim_end_matches('/')
            );
            let key = string(&config, "emby_api_key");
            let mut native = read_image(&work, &url, &key).await?;
            let mut local = match tokio::fs::read(&path).await {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if native.as_deref() == Some(bytes.as_slice()) && local.as_deref() == Some(bytes.as_slice()) {
                let result = json!({"message":format!("{kind} 替换成功！")});
                context.complete("image-done",&result).await?;
                let backup = work.data_dir.join("backups/image-replacement").join(&context.id);
                if tokio::fs::try_exists(backup.join("manifest.json")).await? { let _ = tokio::fs::remove_dir_all(backup).await; }
                return Ok((200,result));
            }
            let backup = work
                .data_dir
                .join("backups/image-replacement")
                .join(if context.id.is_empty() { uuid::Uuid::new_v4().simple().to_string() } else { context.id.clone() });
            tokio::fs::create_dir_all(&backup).await?;
            if tokio::fs::try_exists(backup.join("manifest.json")).await? {
                let manifest: Value = serde_json::from_slice(&tokio::fs::read(backup.join("manifest.json")).await?)
                    .map_err(|_| ApiError::bad("图片任务恢复记录无效"))?;
                if manifest["id"] != id || manifest["path"] != json!(path) || manifest["image_type"] != image_type {
                    return Err(ApiError::bad("图片任务恢复范围与当前媒体不一致"));
                }
                local = if manifest["local_existed"] == true { Some(tokio::fs::read(backup.join("local-original")).await?) } else { None };
                native = if manifest["native_existed"] == true { Some(tokio::fs::read(backup.join("native-original")).await?) } else { None };
                if tokio::fs::read(backup.join("replacement")).await? != bytes {
                    return Err(ApiError::bad("图片任务恢复数据发生变化"));
                }
            } else {
            for (name, original) in [("local-original", &local), ("native-original", &native)] {
                if let Some(original) = original {
                    save_image(&backup.join(name), original).await?;
                }
            }
            save_image(&backup.join("replacement"), &bytes).await?;
            save_image(
                &backup.join("manifest.json"),
                &serde_json::to_vec_pretty(&json!({
                    "id":id,"image_type":image_type,"path":path,
                    "local_existed":local.is_some(),"native_existed":native.is_some()
                }))
                .unwrap(),
            )
            .await?;
            }
            let finishing = crate::jobs::JobContext::for_recovery(Some(&context)).await;
            context.checkpoint().await?;
            let mut local_changed = false;
            let mut native_changed = false;
            let changed = async {
                save_image(&path, &bytes).await?;
                local_changed = true;
                native_changed = true;
                delete_image(&work, &url, &key).await?;
                let uploaded = upload_image(&work, &url, &key, content_type, &bytes).await;
                let refreshed = crate::emby::refresh_item(&work, &id, false, &finishing).await;
                uploaded?;
                refreshed?;
                verify_image(&work, &url, &key, Some(&bytes), native.as_deref()).await
            }
            .await;
            if let Err(error) = changed {
                let mut failures = vec![];
                let restored = if !local_changed {
                    Ok(())
                } else {
                    match &local {
                        Some(original) => save_image(&path, original).await,
                        None => match tokio::fs::remove_file(&path).await {
                            Ok(()) => Ok(()),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                            Err(error) => Err(error.into()),
                        },
                    }
                };
                if let Err(error) = restored {
                    failures.push(format!("本地原图未恢复（{}）：{}", path.display(), error.1));
                }
                if native_changed {
                    if let Err(error) = delete_image(&work, &url, &key).await {
                        failures.push(format!("恢复前原生图片清理失败：{}", error.1));
                    }
                    if let Some(original) = &native {
                        let mime = image::guess_format(original).unwrap().to_mime_type();
                        if let Err(error) = upload_image(&work, &url, &key, mime, original).await {
                            failures.push(format!("原生图片恢复上传失败：{}", error.1));
                        }
                    }
                    if let Err(error) =
                        crate::emby::refresh_item(&work, &id, false, &finishing).await
                    {
                        failures.push(format!("恢复刷新未确认：{}", error.1));
                    }
                    if let Err(error) =
                        verify_image(&work, &url, &key, native.as_deref(), Some(&bytes)).await
                    {
                        failures.push(format!("原生图片未恢复：{}", error.1));
                    }
                }
                let outcome = if failures.is_empty() {
                    "原图已恢复".into()
                } else {
                    failures.join("；")
                };
                return Err(ApiError(
                    error.0,
                    format!(
                        "{}；{}；原图备份已保留：{}",
                        error.1,
                        outcome,
                        backup.display()
                    ),
                ));
            }
            let result = json!({"message":format!("{kind} 替换成功！")});
            context.complete("image-done",&result).await?;
            if let Err(error) = tokio::fs::remove_dir_all(&backup).await {
                tracing::warn!(%error, path=%backup.display(), "图片验收成功，旧图备份暂未清理");
            }
            Ok((200, result))
}

pub(crate) async fn resume_cleanup(state: &AppState, context: &crate::jobs::JobContext) -> Result<()> {
    let backup = state.data_dir.join("backups/image-replacement").join(&context.id);
    if !tokio::fs::try_exists(backup.join("manifest.json")).await? { return Ok(()); }
    if context.completed("image-done").await?.is_some() { let _ = tokio::fs::remove_dir_all(&backup).await; return Ok(()); }
    let manifest: Value = serde_json::from_slice(&tokio::fs::read(backup.join("manifest.json")).await?)
        .map_err(|_|ApiError::bad("图片恢复记录无效"))?;
    let id = string(&manifest,"id");
    let kind = string(&manifest,"image_type");
    let path = std::path::PathBuf::from(string(&manifest,"path"));
    let item = details(state,&id,"Id,Path,Type,IndexNumber").await?;
    let (expected_path,expected_kind) = destination(&item,&kind)?;
    if item["Id"] != id || path != expected_path || kind != expected_kind {
        return Err(ApiError::bad("图片恢复所属媒体身份或路径变化"));
    }
    let config = state.config.read().await.clone();
    let url = format!("{}/Items/{id}/Images/{kind}",string(&config,"emby_server_url").trim_end_matches('/'));
    let key = string(&config,"emby_api_key");
    let replacement = tokio::fs::read(backup.join("replacement")).await?;
    let original_local = if manifest["local_existed"] == true { Some(tokio::fs::read(backup.join("local-original")).await?) } else { None };
    let original_native = if manifest["native_existed"] == true { Some(tokio::fs::read(backup.join("native-original")).await?) } else { None };
    let current_local = match tokio::fs::read(&path).await {
        Ok(bytes)=>Some(bytes), Err(error) if error.kind()==std::io::ErrorKind::NotFound=>None, Err(error)=>return Err(error.into()),
    };
    let current_native = read_image(state,&url,&key).await?;
    if current_local.as_deref() == Some(replacement.as_slice()) && current_native.as_deref() == Some(replacement.as_slice()) { return Ok(()); }
    if current_local.as_ref().is_some_and(|bytes|bytes != &replacement && Some(bytes) != original_local.as_ref())
        || current_native.as_ref().is_some_and(|bytes|bytes != &replacement && Some(bytes) != original_native.as_ref()) {
        return Err(ApiError::bad("图片中断后出现新内容，原图备份保留，未覆盖当前图片"));
    }
    match &original_local {
        Some(bytes)=>save_image(&path,bytes).await?,
        None=>match tokio::fs::remove_file(&path).await { Ok(())=>(), Err(error) if error.kind()==std::io::ErrorKind::NotFound=>(), Err(error)=>return Err(error.into()) },
    }
    if current_native != original_native {
        delete_image(state,&url,&key).await?;
        if let Some(bytes) = &original_native {
            let mime = image::guess_format(bytes).map_err(|_|ApiError::bad("原图备份不能解码"))?.to_mime_type();
            upload_image(state,&url,&key,mime,bytes).await?;
        }
        let finishing = crate::jobs::JobContext::for_recovery(Some(context)).await;
        crate::emby::refresh_item(state,&id,false,&finishing).await?;
    }
    verify_image(state,&url,&key,original_native.as_deref(),Some(&replacement)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ingest_tolerance_accepts_jpeg_recompression_but_not_another_cover() {
        let image = image::RgbImage::from_fn(64, 64, |x,y| image::Rgb([(x*3) as u8,(y*3) as u8,80]));
        let encode = |quality| {
            let mut bytes=vec![];
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes,quality).encode_image(&image).unwrap();
            visible_pixels(image::load_from_memory(&bytes).unwrap())
        };
        let original=encode(95);
        let recompressed=encode(80);
        assert_ne!(original,recompressed);
        assert!(pixel_difference(&original,&recompressed).unwrap() <= 3.0);
        let wrong=image::RgbaImage::from_pixel(64,64,image::Rgba([255,0,0,255]));
        assert!(pixel_difference(&original,&wrong).unwrap() > 3.0);
    }

    #[test]
    fn invisible_png_rgb_is_ignored_but_visible_color_and_alpha_are_verified() {
        let pixels = |rgb: u8, alpha: u8| visible_pixels(image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_pixel(2, 2, image::Rgba([rgb, 0, 0, alpha]))));
        assert_eq!(pixels(0, 0), pixels(255, 0));
        assert_ne!(pixels(0, 1), pixels(255, 1));
        assert_ne!(pixels(255, 0), pixels(255, 1));
    }
}
