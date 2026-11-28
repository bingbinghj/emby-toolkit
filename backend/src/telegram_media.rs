//! MP v3.2.9 的原图与附件协议；普通封面仍保留 Telegram URL 语义。
use crate::error::{ApiError, Result};
use base64::Engine;
use serde_json::Value;
use std::{path::Path, time::Duration};

const IMAGE_LIMIT: usize = 10 * 1024 * 1024;
const FILE_LIMIT: usize = 50 * 1024 * 1024;
// MP v3.2.16: wait for explicit 429 refusals only, bounded to two retries / 60 s.
const RATE_LIMIT_MAX_WAIT: u64 = 60;
const RATE_LIMIT_MAX_RETRIES: usize = 2;

fn decode(data: &str, limit: usize) -> Result<Vec<u8>> {
    if data.len() > limit.div_ceil(3) * 4 {
        return Err(ApiError::bad("通知附件超过大小限制"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| ApiError::bad("通知附件编码无效"))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(ApiError::bad("通知附件为空或超过大小限制"));
    }
    Ok(bytes)
}

async fn local_file(path: &str, limit: usize) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > limit as u64 {
        return Err(ApiError::bad("通知附件不是有效文件或超过大小限制"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
    if bytes.len() > limit {
        return Err(ApiError::bad("通知附件超过大小限制"));
    }
    Ok(bytes)
}

async fn image(client: &reqwest::Client, source: &str) -> Result<(Vec<u8>, &'static str)> {
    let bytes = if let Some(data) = source.strip_prefix("data:image/") {
        let (_, data) = data
            .split_once(";base64,")
            .ok_or_else(|| ApiError::bad("通知图片编码无效"))?;
        decode(data, IMAGE_LIMIT)?
    } else if source.starts_with("https://") || source.starts_with("http://") {
        let mut response = client
            .get(source)
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|size| size > IMAGE_LIMIT as u64)
        {
            return Err(ApiError::bad("通知图片超过 10 MiB"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > IMAGE_LIMIT {
                return Err(ApiError::bad("通知图片超过 10 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        bytes
    } else {
        local_file(source, IMAGE_LIMIT).await?
    };
    let mime = match ::image::guess_format(&bytes) {
        Ok(::image::ImageFormat::Png) => "image/png",
        Ok(::image::ImageFormat::Jpeg) => "image/jpeg",
        _ => return Err(ApiError::bad("Telegram 原图只支持 JPG 或 PNG")),
    };
    Ok((bytes, mime))
}

/// 统一机器人和通知请求，上传后保留本地文件；话题和按钮随原 payload 一起发送。
pub async fn request(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    method: &str,
    payload: &Value,
) -> Result<Value> {
    let mut payload = payload.clone();
    let native = payload.get("native_image") == Some(&Value::Bool(true));
    if let Some(object) = payload.as_object_mut() {
        object.remove("native_image");
    }
    let upload = if method == "sendPhoto" && native {
        let source = payload["photo"]
            .as_str()
            .ok_or_else(|| ApiError::bad("缺少通知图片"))?;
        let (bytes, mime) = image(client, source).await?;
        Some((
            "photo",
            if mime == "image/png" {
                "image.png"
            } else {
                "image.jpg"
            }
            .to_string(),
            bytes,
            mime,
        ))
    } else if method == "sendDocument" && payload["document"].is_object() {
        let document = &payload["document"];
        let bytes = if let Some(data) = document["data"].as_str() {
            decode(data, FILE_LIMIT)?
        } else {
            local_file(
                document["path"]
                    .as_str()
                    .ok_or_else(|| ApiError::bad("缺少通知附件"))?,
                FILE_LIMIT,
            )
            .await?
        };
        let name = document["name"]
            .as_str()
            .or_else(|| document["path"].as_str())
            .unwrap_or("attachment");
        let name = Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("attachment")
            .to_string();
        Some(("document", name, bytes, "application/octet-stream"))
    } else {
        None
    };
    let mut waited = 0;
    for attempt in 0..=RATE_LIMIT_MAX_RETRIES {
        let mut request = client
            .post(format!(
                "{}/bot{token}/{method}",
                api_base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(
                if upload.is_some() || method == "sendPhoto" {
                    30
                } else {
                    15
                },
            ));
        if let Some((field, name, bytes, mime)) = &upload {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(name.clone())
                .mime_str(mime)?;
            let mut form = reqwest::multipart::Form::new().part(*field, part);
            for (key, value) in payload
                .as_object()
                .ok_or_else(|| ApiError::bad("通知格式无效"))?
            {
                if key != *field && !value.is_null() {
                    form = form.text(
                        key.clone(),
                        value
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| value.to_string()),
                    );
                }
            }
            request = request.multipart(form);
        } else {
            request = request.json(&payload);
        }
        let response = request.send().await?;
        let status = response.status();
        let data: Value = response.json().await.unwrap_or(Value::Null);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || data["error_code"] == 429 {
            let delay = data["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(1)
                .max(1);
            if attempt < RATE_LIMIT_MAX_RETRIES && delay <= RATE_LIMIT_MAX_WAIT - waited {
                waited += delay;
                tokio::time::sleep(Duration::from_secs(delay)).await;
                continue;
            }
        }
        if !status.is_success() {
            return Err(ApiError(
                reqwest::StatusCode::BAD_GATEWAY,
                format!("Telegram 返回 HTTP {}", status.as_u16()),
            ));
        }
        if data["ok"] != true {
            let reason = data["description"]
                .as_str()
                .unwrap_or("未确认发送成功")
                .replace(token, "********")
                .replace(api_base, "Telegram API");
            return Err(ApiError(
                reqwest::StatusCode::BAD_GATEWAY,
                format!("Telegram 发送失败：{}", crate::logs::redact(&reason)),
            ));
        }
        return Ok(data);
    }
    unreachable!("the final Telegram attempt always returns")
}
