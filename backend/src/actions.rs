use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::details,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/actions/reprocess_item/{id}", post(submit))
        .route("/api/actions/reprocess_all_review_items", post(all))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let work = state.clone();
    let task = state
        .jobs
        .submit("重新处理项目", move |context| async move {
            reprocess(&work, &id, body["reason"].as_str().unwrap_or(""), &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"重新处理项目任务已提交"})),
    ))
}
async fn all(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let kind = string(&body, "media_type");
    if !["", "Movie", "Series"].contains(&kind.as_str()) {
        return Err(ApiError::bad("媒体类型仅支持电视剧或电影"));
    }
    let reason = string(&body, "reason");
    let work = state.clone();
    let task=state.jobs.submit("重新处理待复核项",move|context|async move{
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(f) FROM failed_log f WHERE ($1='' OR reason LIKE $1||'%') AND ($2='' OR item_type=$2 OR ($2='Series' AND item_type IN ('Season','Episode')))").bind(reason).bind(kind).fetch_all(&work.db).await?;
        let mut updated=0;let mut failures=vec![];for row in rows{context.checkpoint().await?;let id=string(&row,"item_id");match reprocess(&work,&id,&string(&row,"reason"),&context).await{Ok(_)=>updated+=1,Err(error)=>failures.push(format!("{id}: {}",error.1))}}
        Ok(json!({"updated":updated,"failures":failures,"message":"待复核项目重新处理完成"}))
    }).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"批量重新处理任务已提交"})),
    ))
}
pub async fn reprocess(
    state: &AppState,
    id: &str,
    reason: &str,
    context: &JobContext,
) -> Result<Value> {
    context.checkpoint().await?;
    let requested = details(
        state,
        id,
        "Id,Name,Type,Path,People,ProviderIds,Genres,MediaSources,SeriesId,ParentId",
    )
    .await?;
    let missing = reason.contains("缺失媒体信息");
    let mut ids = crate::revalidation::scope(state, id, context).await?;
    if matches!(requested["Type"].as_str(), Some("Series" | "Season")) && ids.is_empty() {
        return Err(ApiError::bad("无法确定在库季集范围，禁止全季处理"));
    }
    if requested["Type"] == "Movie" {
        let target = crate::media_edit::target(state, id).await?;
        for version in target.versions {
            let version = string(&version, "emby_id");
            if !version.is_empty() && !ids.contains(&version) {
                ids.push(version);
            }
        }
    }
    let mut rebuilt = std::collections::BTreeMap::new();
    for asset in &ids {
        context.checkpoint().await?;
        let target = crate::media_edit::target(state, asset).await?;
        let final_path = sidecar(state, &target).await?;
        if missing
            && let Ok(bytes) = tokio::fs::read(&final_path).await
            && let Ok(info) = serde_json::from_slice::<Value>(&bytes)
            && !has_strm_container(&info)
        {
            continue;
        }
        let (pc, sha, _) = crate::fingerprints::lookup(state, &target.path).await?;
        let pc = pc.ok_or_else(|| ApiError::bad("缺失115 PC"))?;
        let sha = sha
            .or(target.sha.clone())
            .ok_or_else(|| ApiError::bad("缺失SHA1"))?;
        let source = crate::media_paths::strm_target(&target.path)
            .await
            .unwrap_or(target.path.clone());
        let ext = crate::media_paths::extension(&source);
        let filename = format!(
            "{}{}",
            std::path::Path::new(&target.path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy(),
            if ext == ".strm" { "" } else { &ext }
        );
        let mut info = if missing {
            crate::media_restore::info(state, &sha, &pc, &filename, false, context).await?
        } else {
            crate::media_restore::rebuild_info(state, &sha, &pc, &filename, context).await?
        };
        crate::identity::verify(&target.asset, &info)?;
        if missing {
            let item = details(state, asset, "Path,Size,MediaSources,Chapters").await?;
            let before = crate::revalidation::observation(state, &item).await;
            crate::identity::verify(&json!({"size_bytes":item["Size"]}), &info)?;
            let chapters = crate::media_info::clean_chapters(&item["Chapters"])?;
            crate::media_info::preserve_chapters(&mut info, &chapters)?;
            let stage =
                final_path.with_file_name(format!(".etk-info-{}", uuid::Uuid::new_v4().simple()));
            let result = async {
                use tokio::io::AsyncWriteExt;
                let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&stage).await?;
                file.write_all(&serde_json::to_vec(&info).unwrap()).await?;
                file.sync_all().await?;
                drop(file);
                context.checkpoint().await?;
                let current = crate::media_edit::target(state, asset).await?;
                if current.path != target.path || current.sha != target.sha || current.asset != target.asset
                    || crate::media_paths::strm_target(&target.path).await.unwrap_or(target.path.clone()) != source
                    || crate::revalidation::observation(state, &details(state, asset, "Path,Size,MediaSources").await?).await != before {
                    return Err(ApiError::bad("文件身份在提取期间变化"));
                }
                tokio::fs::rename(&stage, &final_path).await?;
                sqlx::query("INSERT INTO p115_mediainfo_cache(sha1,mediainfo_json) VALUES($1,$2) ON CONFLICT(sha1) DO UPDATE SET mediainfo_json=EXCLUDED.mediainfo_json").bind(&sha).bind(&info).execute(&state.db).await?;
                Ok::<_,ApiError>(())
            }.await;
            let _ = tokio::fs::remove_file(stage).await;
            result?;
        } else {
            rebuilt.insert(asset.clone(), info);
        }
    }
    if missing {
        return crate::ingest::feedback(state, id, &ids, true, context).await;
    }
    if let Some(first) = rebuilt.keys().next() {
        let body = json!({"mediainfo":rebuilt[first],"target_item_ids":ids,"stream_patches":[]});
        let (status, result) =
            crate::media_save::save_with_rebuilt(state, first, &body, &rebuilt, context).await?;
        if status >= 400 {
            return Err(ApiError::bad(
                result["error"].as_str().unwrap_or("重建媒体信息失败"),
            ));
        }
    }
    let item = series_parent(state, requested).await?;
    let episodes = if item["Type"] == "Series" {
        ids
    } else {
        vec![]
    };
    crate::ingest::scrape(state, &item, &episodes, None, context).await
}
async fn series_parent(state: &AppState, mut item: Value) -> Result<Value> {
    for _ in 0..2 {
        if !matches!(item["Type"].as_str(), Some("Season" | "Episode")) {
            return Ok(item);
        }
        let parent = item["SeriesId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| item["ParentId"].as_str().filter(|s| !s.is_empty()))
            .ok_or_else(|| ApiError::bad("无法定位父电视剧"))?;
        item = details(
            state,
            parent,
            "Id,Name,Type,Path,People,ProviderIds,Genres,MediaSources,SeriesId,ParentId",
        )
        .await?;
    }
    if item["Type"] != "Series" {
        return Err(ApiError::bad("无法定位父电视剧"));
    }
    Ok(item)
}
async fn sidecar(
    state: &AppState,
    target: &crate::media_edit::Target,
) -> Result<std::path::PathBuf> {
    let config = state.config.read().await.clone();
    let root = string(&config, "local_strm_root");
    if root.is_empty() {
        return Err(ApiError::bad("未配置本地媒体目录"));
    }
    let root = tokio::fs::canonicalize(root).await?;
    let path = std::path::Path::new(&target.path);
    let final_path = if target.path.starts_with("http") {
        target
            .candidates
            .iter()
            .find(|p| {
                p.is_absolute() && p.starts_with(&root) && p.parent().is_some_and(|p| p.is_dir())
            })
            .cloned()
            .ok_or_else(|| ApiError::bad("无法映射本地媒体信息路径"))?
    } else {
        path.with_file_name(format!(
            "{}-mediainfo.json",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ))
    };
    let directory = tokio::fs::canonicalize(
        final_path
            .parent()
            .ok_or_else(|| ApiError::bad("媒体信息路径无效"))?,
    )
    .await?;
    if !directory.starts_with(&root)
        || tokio::fs::symlink_metadata(&final_path)
            .await
            .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(ApiError::bad("媒体信息路径越过配置目录"));
    }
    Ok(final_path)
}
pub fn has_strm_container(info: &Value) -> bool {
    let entries = info
        .as_array()
        .map_or_else(|| vec![info], |v| v.iter().collect());
    entries.into_iter().any(|entry| {
        let source = if entry["MediaSourceInfo"].is_object() {
            &entry["MediaSourceInfo"]
        } else {
            entry
        };
        source["Container"]
            .as_str()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("strm"))
    })
}
