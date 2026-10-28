use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    media_info,
    state::string,
};
use serde_json::{Value, json};
use std::{sync::LazyLock, time::Duration};
use tokio::sync::Mutex;

static SYNC_GATE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub fn size_matches(info: &Value, expected: u64) -> bool {
    let actual = media_info::container(info)
        .and_then(|source| source["Size"].as_u64())
        .unwrap_or(0);
    expected == 0
        || actual == 0
        || u128::from(actual.abs_diff(expected)) * 200 <= u128::from(expected)
}

pub fn valid_video(item: &Value) -> bool {
    list(&item["MediaStreams"])
        .into_iter()
        .chain(
            list(&item["MediaSources"])
                .iter()
                .flat_map(|source| list(&source["MediaStreams"])),
        )
        .any(|stream| {
            stream["Type"] == "Video"
                && stream["Width"].as_f64().is_some_and(|n| n > 0.0)
                && stream["Height"].as_f64().is_some_and(|n| n > 0.0)
        })
}

async fn sync(state: &AppState, id: &str, data: &Value) -> Result<Value> {
    let config = state.config.read().await.clone();
    let base = string(&config, "emby_server_url");
    let key = string(&config, "emby_api_key");
    for endpoint in ["emby/Items/SyncMediaInfo", "Items/SyncMediaInfo"] {
        let response = crate::emby::send(
            state
                .http
                .post(format!("{}/{endpoint}", base.trim_end_matches('/')))
                .query(&[("Id", id), ("api_key", key.as_str())])
                .json(data)
                .timeout(Duration::from_secs(600)),
        )
        .await?;
        if endpoint.starts_with("emby/") && response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        return Ok(response.error_for_status()?.json().await?);
    }
    unreachable!()
}

async fn bound_pickcode(state: &AppState, id: &str) -> Result<Option<String>> {
    let row: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1::jsonb LIMIT 1",
    )
    .bind(json!([id]))
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let ids = list(&row["emby_item_ids_json"]);
    let Some(index) = ids.iter().position(|value| value.as_str() == Some(id)) else {
        return Ok(None);
    };
    if list(&row["asset_details_json"])
        .get(index)
        .is_some_and(|asset| asset["fingerprint_pending"] == true)
    {
        return Ok(None);
    }
    Ok(list(&row["file_pickcode_json"])
        .get(index)
        .and_then(Value::as_str)
        .filter(|pc| !pc.is_empty())
        .map(str::to_string))
}

/// Emby webhook preflight. False reuses the SHA1 cache; all target media waits for video data.
pub async fn run(state: &AppState, item: &Value, context: &JobContext) -> Result<Value> {
    context.checkpoint().await?;
    tokio::select! {
        _ = context.cancelled() => Err(ApiError(axum::http::StatusCode::CONFLICT, "任务已取消".into())),
        result = preflight(state, item, context) => result,
    }
}

async fn restore(
    state: &AppState,
    config: &Value,
    id: &str,
    path: &str,
    context: &JobContext,
    report: &mut Value,
) -> Result<()> {
    let filename = path.replace('\\', "/");
    let media_name = filename.rsplit('/').next().unwrap_or(path);
    if config["monitor_sha1_pc_search"] == false {
        return Ok(());
    }
    let (mut pc, mut sha, _) = crate::fingerprints::lookup(state, path).await?;
    if pc.is_none() && sha.is_none() {
        pc = bound_pickcode(state, id).await?;
        if let Some(pc) = &pc {
            sha = crate::fingerprints::lookup(state, &format!("https://115.invalid/d/{pc}"))
                .await?
                .1;
        }
    }
    if let Some(sha) = sha.as_deref().filter(|sha| !sha.is_empty()) {
        let expected: Option<i64> = sqlx::query_scalar(
            "SELECT size FROM p115_filesystem_cache WHERE UPPER(sha1)=UPPER($1) AND size>0 LIMIT 1",
        )
        .bind(sha)
        .fetch_optional(&state.db)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!("入库媒体信息缓存查询失败：《{media_name}》，Emby {id}，{error}");
            None
        });
        let expected = expected.unwrap_or(0).max(0) as u64;
        let cached: Option<Value> = sqlx::query_scalar(
            "SELECT mediainfo_json FROM p115_mediainfo_cache WHERE UPPER(sha1)=UPPER($1) AND mediainfo_json IS NOT NULL LIMIT 1",
        )
        .bind(sha)
        .fetch_optional(&state.db)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!("入库媒体信息缓存查询失败：《{media_name}》，Emby {id}，{error}");
            None
        });
        let mut data = cached
            .filter(media_info::recoverable)
            .unwrap_or(Value::Null);
        let mut from_cache = !data.is_null();
        if from_cache {
            if let Err(error) = sqlx::query("UPDATE p115_mediainfo_cache SET hit_count=COALESCE(hit_count,0)+1 WHERE UPPER(sha1)=UPPER($1)")
                .bind(sha).execute(&state.db).await {
                tracing::warn!("入库媒体信息缓存命中计数失败：《{media_name}》，Emby {id}，{error}");
            }
            report["cache_hit"] = json!(true);
            tracing::info!("入库媒体信息缓存命中：《{media_name}》，Emby {id}，SHA1 {sha}");
        }
        for attempt in 0..15 {
            context.checkpoint().await?;
            let guard = SYNC_GATE.lock().await;
            context.checkpoint().await?;
            let response = sync(state, id, &data).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            drop(guard);
            context.checkpoint().await?;
            report["sync_attempts"] = json!(attempt + 1);
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(
                        "入库媒体信息同步失败：《{media_name}》，Emby {id}，{}",
                        error.1
                    );
                    break;
                }
            };
            if response.as_array().is_some_and(Vec::is_empty) {
                tracing::debug!(
                    "入库媒体信息提取等待：《{media_name}》，Emby {id}，轮次 {}/15",
                    attempt + 1
                );
                context.sleep(Duration::from_secs(5)).await?;
                continue;
            }
            if !media_info::recoverable(&response) {
                break;
            }
            if !size_matches(&response, expected) {
                tracing::warn!(
                    "入库媒体信息大小不符：《{media_name}》，Emby {id}，清除错误插件数据后重新提取"
                );
                if let Err(error) = state
                    .emby(
                        reqwest::Method::POST,
                        &format!("Items/{id}/ClearMediaInfo"),
                        &[("api_key", string(config, "emby_api_key"))],
                        None,
                    )
                    .await
                {
                    tracing::warn!(
                        "入库媒体信息清除失败：《{media_name}》，Emby {id}，{}",
                        error.1
                    );
                }
                data = Value::Null;
                from_cache = false;
                report["invalid_sizes"] = json!(report["invalid_sizes"].as_u64().unwrap() + 1);
                context.sleep(Duration::from_secs(2)).await?;
                continue;
            }
            if !from_cache {
                match sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json")
                    .bind(sha).bind(&response).execute(&state.db).await {
                    Ok(_) => report["cached"] = json!(true),
                    Err(error) => tracing::warn!("入库媒体信息缓存写入失败：《{media_name}》，Emby {id}，{error}"),
                }
            }
            tracing::info!(
                "入库媒体信息同步完成：《{media_name}》，Emby {id}，数据来源 {}",
                if from_cache {
                    "本地媒体信息缓存"
                } else {
                    "现场提取"
                }
            );
            break;
        }
    }
    Ok(())
}

async fn preflight(state: &AppState, item: &Value, context: &JobContext) -> Result<Value> {
    if !matches!(item["Type"].as_str(), Some("Movie" | "Episode")) {
        return Ok(json!({"skipped":true}));
    }
    let id = string(item, "Id");
    let media_name = string(item, "Name");
    let path = item["Path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            list(&item["MediaSources"])
                .first()
                .map(|source| string(source, "Path"))
                .unwrap_or_default()
        });
    if id.is_empty() {
        return Ok(json!({"skipped":true}));
    }
    let config = state.config.read().await.clone();
    let reuse = config["p115_generate_mediainfo"] != true;
    tracing::info!("入库媒体信息预检开始：《{media_name}》，Emby {id}");
    let mut report = json!({"skipped":false,"cache_hit":false,"cached":false,"sync_attempts":0,"invalid_sizes":0,"video_ready":false});
    if reuse && let Err(error) = restore(state, &config, &id, &path, context, &mut report).await {
        context.checkpoint().await?;
        tracing::warn!(
            "入库媒体信息缓存联动失败：《{media_name}》，Emby {id}，{}，继续检查视频流",
            error.1
        );
    }
    let sidecar = std::path::Path::new(&path).with_file_name(format!(
        "{}-mediainfo.json",
        std::path::Path::new(&path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    for attempt in 0..6 {
        context.checkpoint().await?;
        if !path.is_empty() && tokio::fs::try_exists(&sidecar).await.unwrap_or(false) {
            report["video_ready"] = json!(true);
            break;
        }
        match details(state, &id, "MediaSources,MediaStreams").await {
            Ok(current) if valid_video(&current) => {
                report["video_ready"] = json!(true);
                break;
            }
            Err(error) => tracing::warn!(
                "入库视频流检查失败：《{media_name}》，Emby {id}，{}",
                error.1
            ),
            _ => (),
        }
        tracing::debug!(
            "入库视频流提取等待：《{media_name}》，Emby {id}，轮次 {}/6",
            attempt + 1
        );
        context.sleep(Duration::from_secs(10)).await?;
    }
    if report["video_ready"] == false {
        tracing::warn!(
            "入库媒体信息预检超时：《{media_name}》，Emby {id}，继续分发并由最终质检判定"
        );
    } else {
        tracing::info!("入库媒体信息预检完成：《{media_name}》，Emby {id}，有效视频流已就绪");
    }
    Ok(report)
}
