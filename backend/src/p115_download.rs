use super::{openapi_request_with_user_agent, success, tokens};
use crate::{
    AppState,
    error::{ApiError, Result},
    state::string,
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};
use tokio::{sync::Mutex, time::Instant};

type UrlCache = HashMap<(String, String), (String, Instant)>;

#[derive(Default)]
pub(super) struct Service {
    cache: Mutex<UrlCache>,
    next_cookie_request: Mutex<Option<Instant>>,
    open_lock: Mutex<()>,
}
impl Service {
    pub(super) async fn clear(&self) {
        self.cache.lock().await.clear();
    }
    async fn cached(&self, pc: &str, ua: &str) -> Option<String> {
        self.cache
            .lock()
            .await
            .get(&(pc.into(), ua.into()))
            .filter(|(_, expires)| *expires > Instant::now())
            .map(|(url, _)| url.clone())
    }
    async fn save(&self, pc: &str, ua: &str, url: &str) {
        let mut cache = self.cache.lock().await;
        cache.retain(|_, (_, expires)| *expires > Instant::now());
        if cache.len() >= 1000
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, expires))| *expires)
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(
            (pc.into(), ua.into()),
            (url.into(), Instant::now() + Duration::from_secs(300)),
        );
    }
}
fn extract_url(value: &Value) -> Option<String> {
    if let Some(url) = value
        .as_str()
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
    {
        return Some(url.into());
    }
    if let Some(object) = value.as_object() {
        for key in ["url", "download_url", "downurl", "direct_url"] {
            if let Some(url) = object.get(key).and_then(extract_url) {
                return Some(url);
            }
        }
        return object.values().find_map(extract_url);
    }
    value
        .as_array()
        .and_then(|v| v.iter().find_map(extract_url))
}
async fn cookie_url(state: &AppState, pc: &str, ua: &str) -> Result<String> {
    let downloads = &state.p115.downloads;
    let mut next = downloads.next_cookie_request.lock().await;
    if let Some(url) = downloads.cached(pc, ua).await {
        return Ok(url);
    }
    if let Some(deadline) = *next {
        tokio::time::sleep_until(deadline).await;
    }
    // Reserve before the await so a cancelled request still respects the minimum interval.
    *next = Some(Instant::now() + Duration::from_millis(1500));
    let cookie = string(&tokens(state).await?, "cookie");
    let encrypted = crate::p115_cipher::encrypt(
        serde_json::to_string(&json!({"pickcode":pc}))
            .unwrap()
            .as_bytes(),
    );
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let response = state
            .http
            .post(format!(
                "{}/app/chrome/downurl",
                state.p115.endpoints.openapi
            ))
            .header("Cookie", cookie)
            .header("User-Agent", ua)
            .form(&[("data", encrypted)])
            .send()
            .await?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "115 直链请求被风控拦截，请稍后重试".into(),
            ));
        }
        let response: Value = response.error_for_status()?.json().await?;
        if success(&response) {
            let data = if let Some(encoded) = response["data"].as_str() {
                serde_json::from_slice(&crate::p115_cipher::decrypt(encoded)?)
                    .map_err(|_| ApiError::bad("115 下载响应无效"))?
            } else {
                response["data"].clone()
            };
            if let Some(url) = extract_url(&data) {
                downloads.save(pc, ua, &url).await;
                return Ok(url);
            }
        }
        Err(ApiError::bad("115 Cookie 未返回下载地址"))
    })
    .await
    .unwrap_or_else(|_| {
        Err(ApiError(
            StatusCode::GATEWAY_TIMEOUT,
            "115 Cookie 直链请求超时".into(),
        ))
    });
    *next = Some(
        Instant::now()
            + Duration::from_millis(
                if result
                    .as_ref()
                    .is_err_and(|error| error.0 == StatusCode::METHOD_NOT_ALLOWED)
                {
                    11500
                } else {
                    1500
                },
            ),
    );
    result
}
async fn open_url(state: &AppState, pc: &str, ua: &str) -> Result<String> {
    let _guard = state.p115.downloads.open_lock.lock().await;
    if let Some(url) = state.p115.downloads.cached(pc, ua).await {
        return Ok(url);
    }
    state.p115.rate_limit(state).await;
    let response = openapi_request_with_user_agent(
        state,
        reqwest::Method::POST,
        "/open/ufile/downurl",
        &[("pick_code", pc.into())],
        ua,
    )
    .await?;
    if success(&response)
        && let Some(url) = extract_url(&response["data"])
    {
        state.p115.downloads.save(pc, ua, &url).await;
        return Ok(url);
    }
    Err(ApiError::bad("115 未返回文件下载地址"))
}
async fn repair(state: &AppState, old_pc: &str, ua: &str, cookie: bool) -> Result<String> {
    let fid: Option<String> =
        sqlx::query_scalar("SELECT id FROM p115_filesystem_cache WHERE pick_code=$1 LIMIT 1")
            .bind(old_pc)
            .fetch_optional(&state.db)
            .await?;
    let fid = fid.ok_or_else(|| ApiError::bad("未找到旧 PC 的文件缓存"))?;
    let info = super::file_info(
        state,
        fid.parse().map_err(|_| ApiError::bad("115 文件 ID 无效"))?,
    )
    .await?;
    let new_pc = super::text(&info, &["pick_code", "pc", "pickcode"]);
    if new_pc.is_empty() || new_pc == old_pc {
        return Err(ApiError::bad("115 未返回更新的 PC"));
    }
    let url = if cookie {
        cookie_url(state, &new_pc, ua).await?
    } else {
        open_url(state, &new_pc, ua).await?
    };
    let mut transaction = state.db.begin().await?;
    sqlx::query("UPDATE p115_filesystem_cache SET pick_code=NULL,updated_at=NOW() WHERE pick_code=$1 AND id<>$2")
        .bind(&new_pc).bind(&fid).execute(&mut *transaction).await?;
    let parent = super::text(&info, &["parent_id", "pid", "cid"]);
    let name = super::text(&info, &["name", "file_name", "fn", "n"]);
    let sha = super::text(&info, &["sha1", "sha", "file_sha1"]).to_uppercase();
    let size = ["size", "fs", "file_size"]
        .iter()
        .find_map(|key| {
            info[*key]
                .as_i64()
                .or_else(|| info[*key].as_str()?.parse().ok())
        })
        .unwrap_or(0);
    sqlx::query("UPDATE p115_filesystem_cache SET pick_code=$1,parent_id=COALESCE(NULLIF($2,''),parent_id),name=COALESCE(NULLIF($3,''),name),sha1=COALESCE(NULLIF($4,''),sha1),size=CASE WHEN $5>0 THEN $5 ELSE size END,updated_at=NOW() WHERE id=$6 OR pick_code=$7")
        .bind(&new_pc).bind(parent).bind(name).bind(sha).bind(size).bind(&fid).bind(old_pc)
        .execute(&mut *transaction).await?;
    sqlx::query("UPDATE media_metadata SET file_pickcode_json=(SELECT COALESCE(jsonb_agg(CASE WHEN value=to_jsonb($1::text) THEN to_jsonb($2::text) ELSE value END ORDER BY ord),'[]'::jsonb) FROM jsonb_array_elements(file_pickcode_json) WITH ORDINALITY arr(value,ord)),last_updated_at=NOW() WHERE file_pickcode_json ? $1")
        .bind(old_pc).bind(&new_pc).execute(&mut *transaction).await?;
    transaction.commit().await?;
    state.p115.downloads.save(old_pc, ua, &url).await;
    tracing::info!("115 旧 PC 已通过文件 ID 修复，缓存和媒体引用已同步");
    Ok(url)
}
pub async fn download_url(state: &AppState, pc: &str) -> Result<String> {
    download_url_with_user_agent(state, pc, "Mozilla/5.0").await
}
pub async fn download_url_with_user_agent(state: &AppState, pc: &str, ua: &str) -> Result<String> {
    if pc.is_empty() || !pc.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(ApiError::bad("115 PC 无效"));
    }
    if let Some(url) = state.p115.downloads.cached(pc, ua).await {
        return Ok(url);
    }
    if !string(&tokens(state).await?, "cookie").is_empty() {
        match cookie_url(state, pc, ua).await {
            Ok(url) => return Ok(url),
            Err(error) if error.0 == StatusCode::METHOD_NOT_ALLOWED => return Err(error),
            Err(_) => {}
        }
        match repair(state, pc, ua, true).await {
            Ok(url) => return Ok(url),
            Err(error) if error.0 == StatusCode::METHOD_NOT_ALLOWED => return Err(error),
            Err(_) => {}
        }
    }
    match open_url(state, pc, ua).await {
        Ok(url) => Ok(url),
        Err(error) => repair(state, pc, ua, false).await.or(Err(error)),
    }
}
