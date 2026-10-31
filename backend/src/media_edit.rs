use crate::{
    AppState,
    error::{ApiError, Result},
    identity, language, media_info, media_paths,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use reqwest::Method;
use serde_json::{Value, json};
use sqlx::Row;
use std::{collections::HashSet, path::PathBuf};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/media_info/edit/{id}",
        get(edit).post(crate::media_save::submit),
    )
}
pub struct Target {
    pub id: String,
    pub top_id: String,
    pub kind: String,
    pub path: String,
    pub sha: Option<String>,
    pub asset: Value,
    pub versions: Vec<Value>,
    pub candidates: Vec<PathBuf>,
}
pub fn list(value: &Value) -> Vec<Value> {
    if let Some(items) = value.as_array() {
        return items.clone();
    }
    value
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}
fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
pub async fn details(state: &AppState, id: &str, fields: &str) -> Result<Value> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(ApiError::bad("无效的媒体 ID"));
    }
    let user = string(&*state.config.read().await, "emby_user_id");
    if user.is_empty() {
        return Err(ApiError::bad("未配置 Emby 用户 ID"));
    }
    state
        .emby(
            Method::GET,
            &format!("Users/{user}/Items/{id}"),
            &[
                ("Fields", fields.into()),
                ("PersonFields", "ImageTags,ProviderIds".into()),
            ],
            None,
        )
        .await
}
pub async fn build(
    state: &AppState,
    id: &str,
    path: &str,
    mut sha: Option<String>,
    pc: Option<String>,
) -> Result<Target> {
    let config = state.config.read().await.clone();
    let local_root = string(&config, "local_strm_root");
    let cd2_root = string(&config, "cd2_media_root");
    let source = media_paths::strm_target(path).await.unwrap_or_default();
    let paths = media_paths::local_candidates(path, &source, &[&local_root, &cd2_root]);
    let cache=sqlx::query("SELECT sha1,pick_code,local_path FROM p115_filesystem_cache WHERE (($1::text IS NOT NULL AND pick_code=$1) OR ($2::text IS NOT NULL AND UPPER(sha1)=UPPER($2)) OR trim(both '/' from replace(local_path,chr(92),'/'))=ANY($3)) AND ($1::text IS NULL OR pick_code IS NULL OR pick_code=$1) AND ($2::text IS NULL OR sha1 IS NULL OR UPPER(sha1)=UPPER($2)) ORDER BY CASE WHEN pick_code=$1 THEN 0 WHEN UPPER(sha1)=UPPER($2) THEN 1 ELSE 2 END LIMIT 1")
        .bind(&pc).bind(&sha).bind(&paths).fetch_optional(&state.db).await?;
    if let Some(cache) = &cache {
        sha = sha.or(cache.try_get("sha1")?);
    }
    let cache_path = cache
        .as_ref()
        .and_then(|r| r.try_get::<Option<String>, _>("local_path").ok().flatten())
        .unwrap_or_default();
    let candidates = media_paths::info_candidates(path, &local_root, &cd2_root, &cache_path).await;
    Ok(Target {
        id: id.into(),
        top_id: id.into(),
        kind: String::new(),
        path: path.into(),
        sha,
        asset: json!({}),
        versions: vec![],
        candidates,
    })
}
async fn database_target(state: &AppState, id: &str) -> Result<Option<Target>> {
    let row=sqlx::query("SELECT item_type,asset_details_json,file_sha1_json,file_pickcode_json,emby_item_ids_json FROM media_metadata WHERE emby_item_ids_json @> $1::jsonb LIMIT 1")
        .bind(json!([id])).fetch_optional(&state.db).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let assets = list(
        &row.try_get::<Option<Value>, _>("asset_details_json")?
            .unwrap_or(Value::Null),
    );
    let ids = list(&row.try_get::<Value, _>("emby_item_ids_json")?)
        .iter()
        .map(text)
        .collect::<Vec<_>>();
    let sha = list(&row.try_get::<Value, _>("file_sha1_json")?);
    let pc = list(&row.try_get::<Value, _>("file_pickcode_json")?);
    let index = assets
        .iter()
        .position(|a| string(a, "emby_item_id") == id)
        .or_else(|| ids.iter().position(|v| v == id));
    let asset = index
        .and_then(|i| assets.get(i))
        .filter(|a| a.is_object())
        .cloned()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "未找到对应的媒体项路径".into()))?;
    let path = string(&asset, "path");
    if path.is_empty() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "目标媒体项缺少路径信息".into(),
        ));
    }
    let index = index.unwrap();
    let target_sha = sha.get(index).map(text).filter(|s| !s.is_empty());
    let target_pc = pc.get(index).map(text).filter(|s| !s.is_empty());
    let mut target = build(state, id, &path, target_sha, target_pc).await?;
    target.asset = asset;
    target.kind = row.try_get("item_type")?;
    let mut seen = HashSet::new();
    let total = ids.len().max(assets.len());
    for i in 0..total {
        let asset = assets.get(i).cloned().unwrap_or(json!({}));
        let version_id = string(&asset, "emby_item_id");
        let version_id = if version_id.is_empty() {
            ids.get(i).cloned().unwrap_or_default()
        } else {
            version_id
        };
        if !version_id.is_empty() && seen.insert(version_id.clone()) {
            target.versions.push(
                version(
                    state,
                    &asset,
                    &version_id,
                    i,
                    total,
                    if sha.len() == assets.len() {
                        sha.get(i)
                    } else {
                        None
                    },
                    if pc.len() == assets.len() {
                        pc.get(i)
                    } else {
                        None
                    },
                )
                .await?,
            );
        }
    }
    Ok(Some(target))
}
pub async fn target(state: &AppState, id: &str) -> Result<Target> {
    if let Some(target) = database_target(state, id).await? {
        return Ok(target);
    }
    let part = details(state, id, "Type,ExtraType,ParentId,Path,MediaSources").await?;
    let parent = string(&part, "ParentId");
    if part["Type"] != "Video" || part["ExtraType"] != "AdditionalPart" || parent.is_empty() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "数据库中未找到该媒体项".into(),
        ));
    }
    let parent_target = database_target(state, &parent)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "额外分段的父电影不可编辑".into()))?;
    let path = part["MediaSources"][0]["Path"]
        .as_str()
        .or_else(|| part["Path"].as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad("额外分段缺少媒体路径"))?;
    let mut target = build(state, id, path, None, None).await?;
    target.top_id = parent;
    target.kind = "Movie".into();
    target.asset["fingerprint_pending"] = parent_target.asset["fingerprint_pending"].clone();
    target.versions = parent_target.versions;
    Ok(target)
}
// Maintenance may inspect media not yet cached by ETK. Read its real JSON through
// build/load; a live MediaSource snapshot is never a recovery file.
pub async fn repair_target(state: &AppState, id: &str) -> Result<Target> {
    if let Some(target) = database_target(state, id).await? {
        return Ok(target);
    }
    let item = details(state, id, "Type,Path,Size,MediaSources,PartCount,ExtraType").await?;
    if !matches!(item["Type"].as_str(), Some("Movie" | "Episode" | "Video")) {
        return Err(ApiError::bad("音轨纠正目标不是视频媒体"));
    }
    let path = string(&item, "Path");
    if path.is_empty() {
        return Err(ApiError::bad("音轨纠正目标缺少 Emby 原生路径"));
    }
    let mut target = build(state, id, &path, None, None).await?;
    target.asset = json!({"size_bytes":item["Size"]});
    let sources = list(&item["MediaSources"]);
    let mut seen = HashSet::new();
    for source in sources {
        let source_path = string(&source, "Path");
        let source_id = string(&source, "ItemId");
        let source_id = if !source_id.is_empty() {
            source_id
        } else if source_path == path {
            id.into()
        } else {
            let source_id = string(&source, "Id");
            if source_id.is_empty() || source_id.starts_with("mediasource_") {
                return Err(ApiError::bad("关联媒体源没有可验证的 Emby Item ID"));
            }
            source_id
        };
        if seen.insert(source_id.clone()) {
            target
                .versions
                .push(json!({"emby_id":source_id,"path":source_path}));
        }
    }
    Ok(target)
}
pub async fn load(state: &AppState, target: &Target) -> Result<Value> {
    if target.asset["fingerprint_pending"] == true {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "文件身份待校验，请先重新校验指纹".into(),
        ));
    }
    for path in &target.candidates {
        if let Ok(bytes) = tokio::fs::read(path).await
            && let Ok(info) = serde_json::from_slice::<Value>(&bytes)
            && media_info::recoverable(&info)
            && media_paths::matches_container(&info, &target.path)
        {
            identity::verify(&target.asset, &info)?;
            return Ok(info);
        }
    }
    if let Some(sha) = &target.sha {
        let info = sqlx::query_scalar::<_, Option<Value>>(
            "SELECT mediainfo_json FROM p115_mediainfo_cache WHERE sha1=$1",
        )
        .bind(sha)
        .fetch_optional(&state.db)
        .await?
        .flatten();
        if let Some(info) = info
            && media_info::recoverable(&info)
            && media_paths::matches_container(&info, &target.path)
        {
            identity::verify(&target.asset, &info)?;
            return Ok(info);
        }
    }
    if let Some(sha) = &target.sha {
        let raw: Option<Value> =
            sqlx::query_scalar("SELECT raw_ffprobe_json FROM p115_mediainfo_cache WHERE sha1=$1")
                .bind(sha)
                .fetch_optional(&state.db)
                .await?
                .flatten();
        if let Some(raw) = raw {
            let mappings = crate::db::setting(&state.db, "language_mapping")
                .await?
                .unwrap_or(json!([]));
            let node = json!({"fn":std::path::Path::new(&target.path).file_name().unwrap_or_default().to_string_lossy()});
            if let Some(info) = crate::probe::build(&raw, &node, &mappings)
                && media_info::recoverable(&info)
                && media_paths::matches_container(&info, &target.path)
            {
                identity::verify(&target.asset, &info)?;
                sqlx::query("UPDATE p115_mediainfo_cache SET mediainfo_json=$1 WHERE sha1=$2")
                    .bind(&info)
                    .bind(sha)
                    .execute(&state.db)
                    .await?;
                return Ok(info);
            }
        }
    }
    Err(ApiError(
        StatusCode::CONFLICT,
        "未找到可验证的媒体信息 JSON，已禁止自动提取，未清理 Emby 数据".into(),
    ))
}
async fn version(
    state: &AppState,
    asset: &Value,
    id: &str,
    index: usize,
    total: usize,
    sha: Option<&Value>,
    pc: Option<&Value>,
) -> Result<Value> {
    let path = string(asset, "path");
    let _ = sha;
    let cache=sqlx::query("SELECT name,local_path FROM p115_filesystem_cache WHERE $1::text IS NOT NULL AND pick_code=$1 LIMIT 1")
        .bind(pc.map(text).filter(|s|!s.is_empty())).fetch_optional(&state.db).await?;
    let name = cache
        .as_ref()
        .and_then(|c| c.try_get::<String, _>("name").ok())
        .unwrap_or_default();
    let cache_path = cache
        .as_ref()
        .and_then(|c| c.try_get::<Option<String>, _>("local_path").ok().flatten())
        .unwrap_or_default();
    let strm = media_paths::strm_target(&path).await.unwrap_or_default();
    let sources = [
        strm.as_str(),
        name.as_str(),
        cache_path.as_str(),
        path.as_str(),
    ];
    let source = sources
        .iter()
        .find(|s| !s.is_empty() && !media_paths::extension(s).is_empty())
        .or_else(|| sources.iter().find(|s| !s.is_empty()))
        .copied()
        .unwrap_or("");
    let mut filename = std::path::Path::new(&media_paths::clean(source))
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if filename.to_lowercase().ends_with(".strm") {
        filename.truncate(filename.len() - 5);
    }
    let format = sources
        .iter()
        .map(|s| media_paths::extension(s))
        .find(|s| !s.is_empty() && s != ".strm")
        .unwrap_or_else(|| string(asset, "container"))
        .trim_start_matches('.')
        .to_uppercase();
    let format = if format == "STRM" {
        String::new()
    } else {
        format
    };
    if !filename.is_empty() && media_paths::extension(&filename).is_empty() && !format.is_empty() {
        filename.push_str(&format!(".{}", format.to_lowercase()));
    }
    let display = crate::media_display::fields(&filename);
    let display_value = |key: &str| {
        let current = string(asset, key);
        if matches!(current.to_lowercase().as_str(), "" | "unknown" | "未知") {
            string(&display, key)
        } else {
            current
        }
    };
    let resolution = display_value("resolution_display");
    let quality = display_value("quality_display");
    let effect = display_value("effect_display");
    let detail = crate::media_display::version_detail(&filename);
    let technical = [resolution.as_str(), quality.as_str(), effect.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty() && *s != "无")
        .collect::<Vec<_>>()
        .join(" ");
    let summary = [format.as_str(), detail.as_str(), technical.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let base = if total > 1 && !filename.is_empty() && !summary.is_empty() {
        format!("{summary} · {filename}")
    } else if !filename.is_empty() {
        filename.clone()
    } else if !summary.is_empty() {
        summary
    } else {
        format!("Item {id}")
    };
    Ok(
        json!({"emby_id":id,"label":if total>1 {format!("版本 {} · {base}",index+1)} else {base.clone()},"base_label":base,
        "path":path,"resolution":resolution,"quality":quality,"effect":effect,"format":format,"file_name":filename,"version_detail":detail,"version_number":if total>1 {index+1} else {1}}),
    )
}
pub async fn additional_parts(state: &AppState, id: &str, info: &Value) -> Result<Vec<Value>> {
    let user = string(&*state.config.read().await, "emby_user_id");
    let mut parts = vec![];
    let mut positions = std::collections::HashMap::new();
    for source in info["MediaSources"].as_array().into_iter().flatten() {
        let source_id = string(source, "Id").trim().to_string();
        if source_id.is_empty() {
            continue;
        }
        let response = state
            .emby(
                Method::GET,
                &format!("Videos/{id}/AdditionalParts"),
                &[
                    ("UserId", user.clone()),
                    ("mediaSourceId", source_id),
                    ("Fields", "Path,MediaSources,ExtraType,ParentId".into()),
                ],
                None,
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    "[Emby额外分段] 查询 Item {id} / MediaSource 失败：{}",
                    error.1
                );
                continue;
            }
        };
        for part in response["Items"].as_array().into_iter().flatten() {
            let part_id = string(part, "Id").trim().to_string();
            let parent = string(part, "ParentId").trim().to_string();
            if !part_id.is_empty() && (parent.is_empty() || parent == id) {
                if let Some(position) = positions.get(&part_id) {
                    parts[*position] = part.clone();
                } else {
                    positions.insert(part_id, parts.len());
                    parts.push(part.clone());
                }
            }
        }
    }
    Ok(parts)
}
async fn edit(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    let result = edit_data(&state, &id).await;
    match &result {
        Ok(Json(data)) => {
            tracing::info!(item_id = %id, versions = data["versions"].as_array().map_or(0, Vec::len), "打开音轨字幕编辑页完成，媒体路径 {}", string(data, "media_path"))
        }
        Err(error) => tracing::warn!(item_id = %id, error = %error.1, "打开音轨字幕编辑页失败"),
    }
    result
}
async fn edit_data(state: &AppState, id: &str) -> Result<Json<Value>> {
    let mut target = target(state, id).await?;
    let info = load(state, &target).await?;
    let mut options = vec![];
    let mut seen = HashSet::new();
    for stream in media_info::container(&info)
        .and_then(|v| v["MediaStreams"].as_array())
        .into_iter()
        .flatten()
    {
        let source = string(stream, "Language");
        if matches!(stream["Type"].as_str(), Some("Audio" | "Subtitle"))
            && seen.insert(language::token(&source))
            && let Some(mut mapping) = language::lookup(&source, &json!([]))
        {
            mapping["source"] = json!(source);
            options.push(mapping);
        }
    }
    let mut native = vec![];
    if target.kind == "Movie" && target.versions.len() > 1 {
        let first = string(&target.versions[0], "emby_id");
        if let Ok(item) = details(state, &first, "ParentId").await {
            let parent = string(&item, "ParentId");
            let user = string(&*state.config.read().await, "emby_user_id");
            if !parent.is_empty()
                && let Ok(response) = state
                    .emby(
                        Method::GET,
                        &format!("Users/{user}/Items"),
                        &[
                            ("ParentId", parent),
                            ("IncludeItemTypes", "Movie".into()),
                            ("Recursive", "false".into()),
                            ("Fields", "Path".into()),
                        ],
                        None,
                    )
                    .await
            {
                let ids: HashSet<_> = target
                    .versions
                    .iter()
                    .map(|v| string(v, "emby_id"))
                    .collect();
                if let Some(primary) = response["Items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|v| ids.contains(&string(v, "Id")))
                    && let Ok(details) =
                        details(state, &string(primary, "Id"), "MediaSources").await
                {
                    for source in details["MediaSources"].as_array().into_iter().flatten() {
                        let id = source["ItemId"]
                            .as_str()
                            .or_else(|| source["Id"].as_str())
                            .unwrap_or("")
                            .replace("mediasource_", "");
                        if ids.contains(&id) && !native.contains(&id) {
                            native.push(id);
                        }
                    }
                }
            }
        }
        if native.len() == target.versions.len() {
            target
                .versions
                .sort_by_key(|v| native.iter().position(|id| id == &string(v, "emby_id")));
        }
    }
    let total = target.versions.len();
    for (index, version) in target.versions.iter_mut().enumerate() {
        version["version_number"] = json!(index + 1);
        let base = string(version, "base_label");
        version["label"] = json!(if total > 1 {
            format!("版本 {} · {base}", index + 1)
        } else {
            base
        });
        version["segments"] = json!([]);
        let id = string(version, "emby_id");
        if let Ok(details) = details(state, &id, "Path,PartCount,MediaSources").await
            && details["PartCount"].as_u64().unwrap_or(0) > 1
        {
            let mut segments = vec![];
            let path = details["Path"]
                .as_str()
                .unwrap_or(version["path"].as_str().unwrap_or(""));
            segments.push(json!({"emby_id":id,"label":format!("分段 1 · {}",std::path::Path::new(path).file_name().and_then(|s|s.to_str()).unwrap_or(&id)),"path":path}));
            for (index, part) in additional_parts(state, &id, &details)
                .await?
                .iter()
                .enumerate()
            {
                let path = string(part, "Path");
                let part_id = string(part, "Id");
                segments.push(json!({"emby_id":part_id,"label":format!("分段 {} · {}",index+2,std::path::Path::new(&path).file_name().and_then(|s|s.to_str()).unwrap_or(&part_id)),"path":path}));
            }
            if segments.len() > 1 {
                version["segments"] = json!(segments);
            }
        }
    }
    let path = target
        .candidates
        .iter()
        .find(|p| p.exists())
        .or_else(|| target.candidates.first())
        .cloned();
    Ok(Json(
        json!({"sha1":target.sha,"fingerprint_pending":false,"media_path":target.path,"mediainfo_path":path,"versions":target.versions,
        "current_version_id":target.top_id,"iso_language_options":options,"subtitle_chinese_default_language":state.config.read().await["subtitle_chinese_default_language"],"mediainfo":info}),
    ))
}
