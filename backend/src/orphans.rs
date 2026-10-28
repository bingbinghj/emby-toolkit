use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
    p115,
    state::string,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
pub fn router() -> Router<AppState> {
    Router::new().route("/api/media_info/repair_p115_orphan/{id}", post(submit))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let id = current_id(&state, &id).await?;
    let work = state.clone();
    let task = state
        .jobs
        .submit("检查当前媒体115孤儿文件", move |context| async move {
            repair(&work, &id, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"当前媒体的115孤儿文件检查已提交。"})),
    ))
}
pub async fn current_id(state: &AppState, id: &str) -> Result<String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(ApiError::bad("无效的媒体ID"));
    }
    let config = state.config.read().await.clone();
    let base = string(&config, "emby_server_url")
        .trim_end_matches('/')
        .to_string();
    let key = string(&config, "emby_api_key");
    if base.is_empty() || key.is_empty() {
        return Err(ApiError::bad("Emby 服务器配置不完整"));
    }
    let response = crate::emby::send(
        state
            .http
            .get(format!(
                "{base}/Users/{}/Items/{id}",
                string(&config, "emby_user_id")
            ))
            .header("X-Emby-Token", key)
            .timeout(Duration::from_secs(
                config["emby_api_timeout"]
                    .as_u64()
                    .filter(|seconds| *seconds > 0)
                    .unwrap_or(60),
            ))
            .query(&[("Fields", "Id,Path")]),
    )
    .await?;
    if response.status() != StatusCode::NOT_FOUND {
        response.error_for_status()?;
        return Ok(id.into());
    }
    let bound: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1",
    )
    .bind(json!([id]))
    .fetch_all(&state.db)
    .await?;
    if bound.len() != 1 {
        return Ok(id.into());
    }
    let Some((slot, asset)) = recovery_asset(&bound[0], id) else {
        return Ok(id.into());
    };
    let target = crate::media_edit::target(state, id).await?;
    if target.path != asset["path"] {
        return Ok(id.into());
    }
    let path = if target.path.starts_with("http") {
        let pc = list(&bound[0]["file_pickcode_json"])
            .get(slot)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut paths = BTreeSet::new();
        for candidate in &target.candidates {
            if !candidate.starts_with(string(&config, "local_strm_root")) {
                continue;
            }
            let Some(stem) = candidate
                .to_str()
                .and_then(|s| s.strip_suffix("-mediainfo.json"))
            else {
                continue;
            };
            let path = format!("{stem}.strm");
            if let Some(source) = crate::media_paths::strm_target(&path).await
                && (crate::media_paths::clean(&source) == crate::media_paths::clean(&target.path)
                    || (!pc.is_empty()
                        && crate::fingerprints::pickcode(&source).as_deref() == Some(pc.as_str())))
            {
                paths.insert(path);
            }
        }
        if paths.len() != 1 {
            return Ok(id.into());
        }
        paths.into_iter().next().unwrap()
    } else {
        target.path.clone()
    };
    let result = state
        .emby(
            reqwest::Method::GET,
            "Items",
            &[
                ("Path", path.clone()),
                ("Recursive", "true".into()),
                ("Fields", "Id,Path,Type,ProviderIds".into()),
            ],
            None,
        )
        .await?;
    let matches = list(&result["Items"])
        .into_iter()
        .filter(|i| i["Path"] == path)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Ok(id.into());
    }
    let new_id = string(&matches[0], "Id");
    let provider = string(&matches[0]["ProviderIds"], "Tmdb");
    let tmdb = string(&bound[0], "tmdb_id");
    if new_id.is_empty()
        || new_id.len() > 128
        || !new_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        || new_id == id
        || matches[0]["Type"] != bound[0]["item_type"]
        || (!provider.is_empty() && tmdb.parse::<u64>().is_ok() && provider != tmdb)
    {
        return Ok(id.into());
    }
    let mut transaction = state.db.begin().await?;
    let current: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1 OR emby_item_ids_json @> $2 OR asset_details_json @> $3 ORDER BY tmdb_id,item_type FOR UPDATE",
    )
    .bind(json!([id]))
    .bind(json!([new_id]))
    .bind(json!([{"emby_item_id":new_id}]))
    .fetch_all(&mut *transaction)
    .await?;
    if current.len() != 1
        || current[0]["tmdb_id"] != bound[0]["tmdb_id"]
        || current[0]["item_type"] != bound[0]["item_type"]
        || current[0]["emby_item_ids_json"] != bound[0]["emby_item_ids_json"]
        || current[0]["asset_details_json"] != bound[0]["asset_details_json"]
        || current[0]["file_sha1_json"] != bound[0]["file_sha1_json"]
        || current[0]["file_pickcode_json"] != bound[0]["file_pickcode_json"]
        || list(&current[0]["emby_item_ids_json"])
            .iter()
            .any(|v| v == &new_id)
        || list(&current[0]["asset_details_json"])
            .iter()
            .any(|a| a["emby_item_id"] == new_id)
    {
        return Ok(id.into());
    }
    let mut ids = list(&current[0]["emby_item_ids_json"]);
    let mut assets = list(&current[0]["asset_details_json"]);
    ids[slot] = json!(new_id);
    if assets[slot]["emby_item_id"] == id {
        assets[slot]["emby_item_id"] = json!(new_id);
    }
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=$1,asset_details_json=$2,last_updated_at=NOW() WHERE tmdb_id=$3 AND item_type=$4")
        .bind(json!(ids)).bind(json!(assets)).bind(&tmdb).bind(string(&bound[0], "item_type"))
        .execute(&mut *transaction).await?;
    migrate_item_logs(&mut transaction, id, &new_id).await?;
    transaction.commit().await?;
    Ok(new_id)
}
fn recovery_asset(row: &Value, id: &str) -> Option<(usize, Value)> {
    let ids = list(&row["emby_item_ids_json"]);
    let slots = ids
        .iter()
        .enumerate()
        .filter(|(_, value)| *value == id)
        .collect::<Vec<_>>();
    if slots.len() != 1 {
        return None;
    }
    let slot = slots[0].0;
    let assets = list(&row["asset_details_json"]);
    let asset = assets.get(slot)?;
    let asset_id = string(asset, "emby_item_id");
    let path = string(asset, "path");
    if path.is_empty()
        || (!asset_id.is_empty() && asset_id != id)
        || assets.iter().enumerate().any(|(index, other)| {
            index != slot && (other["emby_item_id"] == id || other["path"] == path)
        })
    {
        return None;
    }
    Some((slot, asset.clone()))
}
pub(crate) async fn migrate_item_logs(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    old_id: &str,
    new_id: &str,
) -> Result<()> {
    if old_id.is_empty() || new_id.is_empty() || old_id == new_id {
        return Ok(());
    }
    for table in ["processed_log", "failed_log"] {
        sqlx::query(&format!("UPDATE {table} SET item_id=$1 WHERE item_id=$2 AND NOT EXISTS(SELECT 1 FROM {table} WHERE item_id=$1)"))
            .bind(new_id).bind(old_id).execute(&mut **transaction).await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE item_id=$1"))
            .bind(old_id)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}
fn text(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| {
            let v = &value[*key];
            let s = v
                .as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))?;
            (!s.trim().is_empty()).then(|| s.trim().into())
        })
        .unwrap_or_default()
}
fn id(value: &Value) -> String {
    text(value, &["id", "fid", "file_id", "cid"])
}
fn parent(value: &Value) -> String {
    text(value, &["parent_id", "pid", "cid"])
}
fn name(value: &Value) -> String {
    text(value, &["name", "file_name", "fn", "n"])
}
fn is_directory(value: &Value) -> bool {
    ["fc", "file_category", "type"]
        .iter()
        .any(|key| matches!(value[*key].as_str(), Some("0")) || value[*key] == 0)
        || ["is_dir", "is_folder", "is_directory"]
            .iter()
            .any(|key| value[*key] == true || value[*key] == 1 || value[*key] == "1")
}
fn sidecar(filename: &str, stem: &str) -> bool {
    if filename == format!("{stem}-mediainfo.json") {
        return true;
    }
    let Some(suffix) = filename.strip_prefix(&format!("{stem}.")) else {
        return false;
    };
    let extension = suffix.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "srt" | "ass" | "ssa" | "sub" | "idx" | "vtt" | "sup"
    ) || matches!(
        suffix.to_ascii_lowercase().as_str(),
        "nfo" | "jpg" | "jpeg" | "png" | "webp"
    )
}
fn slash(value: &str) -> String {
    value
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}
pub fn target_parts(path: &str, roots: &[&str]) -> Vec<String> {
    let path = crate::media_paths::clean(path);
    let parsed = reqwest::Url::parse(&path).ok();
    let path = slash(parsed.as_ref().map_or(path.as_str(), |u| u.path()));
    for root in roots {
        let root = slash(root);
        if root.is_empty() {
            continue;
        }
        let Some(relative) = path.strip_prefix(&format!("{root}/")) else {
            continue;
        };
        return relative
            .split('/')
            .take(relative.split('/').count().saturating_sub(1))
            .map(str::to_string)
            .collect();
    }
    vec![]
}
pub fn candidates(
    rows: &[Value],
    selected: Option<&BTreeSet<String>>,
    roots: &[&str],
) -> Vec<Value> {
    let mut result = vec![];
    let mut seen = BTreeSet::new();
    for row in rows {
        let pcs = list(&row["file_pickcode_json"]);
        let ids = list(&row["emby_item_ids_json"]);
        for (index, asset) in list(&row["asset_details_json"]).iter().enumerate() {
            if !asset.is_object() || index >= pcs.len() {
                continue;
            }
            let item = ids.get(index).and_then(|v| v.as_str()).unwrap_or("");
            if selected.is_some_and(|s| !s.contains(item)) {
                continue;
            }
            let path = text(asset, &["path", "Path"]);
            let pc = pcs[index].as_str().unwrap_or("").trim();
            let parts = target_parts(&path, roots);
            if path.is_empty() || pc.is_empty() || parts.is_empty() {
                continue;
            }
            let Some(fid) = crate::fingerprints::file_id(pc).map(|n| n.to_string()) else {
                continue;
            };
            if !seen.insert(fid.clone()) {
                continue;
            }
            let parsed = reqwest::Url::parse(&path).ok();
            let filename =
                std::path::Path::new(parsed.as_ref().map_or(path.as_str(), |u| u.path()))
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
            let filename = if filename.to_lowercase().ends_with(".strm") {
                &filename[..filename.len() - 5]
            } else {
                &filename
            };
            let stem = std::path::Path::new(filename)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            result.push(json!({"fid":fid,"path":path,"stem":stem,"target_parts":parts}));
        }
    }
    result
}
async fn scope(state: &AppState, id: &str) -> Result<(Vec<Value>, Option<BTreeSet<String>>)> {
    if id.trim().is_empty() {
        return Err(ApiError::bad("缺少当前媒体Item ID，已拒绝全量扫描"));
    }
    let target:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND emby_item_ids_json @> $1 LIMIT 1").bind(json!([id])).fetch_optional(&state.db).await?;
    let Some(target) = target else {
        return Ok((vec![], Some(BTreeSet::new())));
    };
    if matches!(target["item_type"].as_str(), Some("Movie" | "Episode")) {
        return Ok((vec![target], Some(BTreeSet::from([id.into()]))));
    }
    if !matches!(target["item_type"].as_str(), Some("Series" | "Season")) {
        return Ok((vec![], Some(BTreeSet::new())));
    }
    let series = if target["item_type"] == "Series" {
        string(&target, "tmdb_id")
    } else {
        string(&target, "parent_series_tmdb_id")
    };
    let season = if target["item_type"] == "Season" {
        target["season_number"].as_i64().map(|n| n as i32)
    } else {
        None
    };
    let rows=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type='Episode' AND parent_series_tmdb_id=$1 AND ($2::integer IS NULL OR season_number=$2) ORDER BY season_number,episode_number,tmdb_id").bind(series).bind(season).fetch_all(&state.db).await?;
    Ok((rows, None))
}
struct Reader<'a> {
    state: &'a AppState,
    context: &'a JobContext,
    last: Option<tokio::time::Instant>,
    requests: usize,
    children: BTreeMap<String, Vec<Value>>,
    directories: BTreeMap<(String, String), String>,
    parents: BTreeMap<String, bool>,
}
impl Reader<'_> {
    async fn before(&self) -> Result<()> {
        self.context.checkpoint().await?;
        if let Some(last) = self.last {
            tokio::time::sleep_until(last + Duration::from_millis(500)).await;
        }
        self.context.checkpoint().await
    }
    fn after(&mut self) {
        self.last = Some(tokio::time::Instant::now());
        self.requests += 1;
    }
    async fn children(&mut self, parent: &str, fresh: bool) -> Result<Vec<Value>> {
        if !fresh && let Some(cached) = self.children.get(parent) {
            return Ok(cached.clone());
        }
        let mut items = vec![];
        loop {
            self.before().await?;
            let response = p115::files(self.state, parent, items.len()).await;
            self.after();
            let response = response?;
            let data = list(&response["data"]);
            let count = response["count"]
                .as_u64()
                .or_else(|| response["count"].as_str().and_then(|s| s.parse().ok()));
            let before = items.len();
            for item in &data {
                if items.iter().any(|old| id(old) == id(item)) {
                    return Err(ApiError::bad("115目录分页未推进"));
                }
            }
            items.extend(data);
            if count.is_none_or(|count| items.len() as u64 >= count) {
                break;
            }
            if items.len() == before {
                return Err(ApiError::bad("115目录结果不完整"));
            }
        }
        self.children.insert(parent.into(), items.clone());
        Ok(items)
    }
    async fn directory(&mut self, root: &str, parts: &[String], create: bool) -> Result<String> {
        let mut current = root.to_string();
        for part in parts {
            if matches!(part.as_str(), "" | "." | "..") {
                return Err(ApiError::bad("115目标目录层级无效"));
            }
            let key = (current.clone(), part.clone());
            if let Some(id) = self.directories.get(&key) {
                if !id.is_empty() {
                    current = id.clone();
                    continue;
                }
                if !create {
                    return Ok(String::new());
                }
            }
            let children = self.children(&current, false).await?;
            let matches = children
                .iter()
                .filter(|item| name(item) == *part)
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(ApiError::bad(format!(
                    "目录{part}在父目录{current}下不唯一"
                )));
            }
            let next = if let Some(item) = matches.first() {
                id(item)
            } else {
                if !create {
                    self.directories.insert(key, String::new());
                    return Ok(String::new());
                }
                self.before().await?;
                let response = p115::mkdir(self.state, &current, part).await;
                self.after();
                let response = response?;
                if !p115::success(&response) {
                    return Err(ApiError::bad("创建115目录失败"));
                }
                self.children.remove(&current);
                let created = id(&response["data"]);
                if !created.is_empty() {
                    created
                } else {
                    let children = self.children(&current, true).await?;
                    let matches = children
                        .iter()
                        .filter(|item| name(item) == *part)
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(ApiError::bad("创建目录后无法确认目录ID"));
                    }
                    id(matches[0])
                }
            };
            if next.is_empty() {
                return Err(ApiError::bad("115目录缺少有效ID"));
            }
            self.directories.insert(key, next.clone());
            current = next;
        }
        Ok(current)
    }
    async fn check(&mut self, candidate: &Value, root: &str) -> Result<(&'static str, usize)> {
        let parts = list(&candidate["target_parts"])
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        let path = string(candidate, "path");
        self.context
            .activity(
                "定位原整理目录",
                &format!("正在定位 {path} 的原整理目录：{}", parts.join("/")),
            )
            .await;
        let mut target = self.directory(root, &parts, false).await?;
        let mut children = if target.is_empty() {
            vec![]
        } else {
            self.children(&target, false).await?
        };
        let fid = string(candidate, "fid");
        if children.iter().any(|item| id(item) == fid) {
            return Ok(("already_correct", 0));
        }
        self.context
            .activity(
                "搜索115文件",
                &format!("正在搜索 {path} 的当前云端位置（FID {fid}）"),
            )
            .await;
        self.before().await?;
        let response = p115::search(self.state, &string(candidate, "stem")).await;
        self.after();
        let response = response?;
        if !p115::success(&response) {
            return Err(ApiError::bad("115搜索失败"));
        }
        let hits = list(&response["data"]);
        let count = response["count"]
            .as_u64()
            .or_else(|| response["count"].as_str().and_then(|s| s.parse().ok()));
        if count.is_some_and(|count| count > hits.len() as u64) {
            return Err(ApiError::bad("115搜索结果不完整，拒绝移动"));
        }
        let Some(video) = hits.iter().find(|item| id(item) == fid) else {
            return Ok(("search_missing", 0));
        };
        if is_directory(video) {
            return Err(ApiError::bad("115搜索命中目录而非视频，拒绝移动"));
        }
        let current = parent(video);
        let verified = &candidate["verified"];
        if verified.is_object()
            && (current != parent(verified)
                || name(video) != name(verified)
                || text(video, &["pick_code", "pickcode", "pc"])
                    != text(verified, &["pick_code", "pickcode", "pc"]))
        {
            return Err(ApiError::bad("孤儿文件搜索身份已变化，拒绝移动"));
        }
        if current.is_empty() || (!target.is_empty() && current == target) {
            return Ok(("already_correct", 0));
        }
        let exists = if let Some(exists) = self.parents.get(&current) {
            *exists
        } else {
            self.context
                .activity(
                    "核对父目录",
                    &format!("正在确认 {path} 的原父目录 {current} 是否已失效"),
                )
                .await;
            let parent_id = current
                .parse()
                .map_err(|_| ApiError::bad("115父目录ID无效"))?;
            self.before().await?;
            let response = p115::file_info_response(self.state, parent_id).await;
            self.after();
            let response = response?;
            let exists = p115::success(&response);
            if !exists && !p115::missing(&response) {
                return Err(ApiError::bad("无法确认115父目录已失效，拒绝移动"));
            }
            self.parents.insert(current.clone(), exists);
            exists
        };
        if exists {
            return Ok(("skipped_mismatch", 0));
        }
        if target.is_empty() {
            self.context
                .activity(
                    "重建原整理目录",
                    &format!("正在为 {path} 恢复原整理目录：{}", parts.join("/")),
                )
                .await;
            target = self.directory(root, &parts, true).await?;
            children = self.children(&target, false).await?;
        }
        let video_name = name(video);
        let stem = std::path::Path::new(&video_name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let related = hits
            .iter()
            .filter(|item| {
                parent(item) == current
                    && !is_directory(item)
                    && (id(item) == fid || sidecar(&name(item), &stem))
            })
            .collect::<Vec<_>>();
        let mut ids = vec![];
        for item in &related {
            let id = id(item);
            if !id.is_empty() && !ids.contains(&id) {
                ids.push(id);
            }
        }
        if !ids.contains(&fid) {
            ids.insert(0, fid.clone());
        }
        let names = children.iter().map(name).collect::<BTreeSet<_>>();
        if related.iter().any(|item| names.contains(&name(item))) {
            return Err(ApiError::bad("目标目录存在同名文件"));
        }
        self.context
            .activity(
                "恢复孤儿文件",
                &format!(
                    "正在恢复 {path}，关联文件 {} 个",
                    ids.len().saturating_sub(1)
                ),
            )
            .await;
        self.before().await?;
        let response = p115::move_files(self.state, &ids, &target).await;
        self.after();
        let response = response?;
        if !p115::success(&response) {
            return Err(ApiError::bad("115移动失败"));
        }
        self.context
            .activity(
                "验收孤儿文件",
                &format!("正在确认 {path} 已恢复到原整理目录"),
            )
            .await;
        let verified = self.children(&target, true).await?;
        if !verified.iter().any(|item| id(item) == fid) {
            return Err(ApiError::bad("移动接口成功但目标目录未找到视频"));
        }
        Ok(("repaired_videos", ids.len().saturating_sub(1)))
    }
}
/// Restore a freshly verified search hit without first persisting its PickCode.
pub async fn restore_search_hit(
    state: &AppState,
    info: &Value,
    relative: &str,
    context: &JobContext,
) -> Result<Value> {
    let path = std::path::Path::new(relative);
    let parts = path
        .parent()
        .unwrap_or(std::path::Path::new(""))
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(ApiError::bad("孤儿文件缺少原整理目录"));
    }
    let root = string(
        &*state.config.read().await,
        "p115_fingerprint_media_root_cid",
    );
    let fid = string(info, "id");
    let candidate = json!({"fid":fid,"path":relative,"stem":path.file_stem().unwrap_or_default().to_string_lossy(),"target_parts":parts,"verified":info});
    let mut reader = Reader {
        state,
        context,
        last: None,
        requests: 0,
        children: BTreeMap::new(),
        directories: BTreeMap::new(),
        parents: BTreeMap::new(),
    };
    let (status, sidecars) = reader.check(&candidate, &root).await?;
    if !matches!(status, "repaired_videos" | "already_correct") {
        return Err(ApiError::bad(format!("孤儿文件恢复未完成：{status}")));
    }
    let target = reader.directory(&root, &parts, false).await?;
    reader.before().await?;
    let raw = p115::file_info(
        state,
        fid.parse().map_err(|_| ApiError::bad("115 文件ID无效"))?,
    )
    .await?;
    let current_parent = parent(&raw);
    let current_parent = if current_parent.is_empty() {
        raw["paths"]
            .as_array()
            .and_then(|paths| paths.last())
            .map(id)
            .unwrap_or_default()
    } else {
        current_parent
    };
    if current_parent != target {
        return Err(ApiError::bad("孤儿文件详情未确认恢复到目标目录"));
    }
    if status == "repaired_videos" {
        tracing::info!("[修复115孤儿文件] 已恢复：{relative}，关联文件 {sidecars} 个");
    } else {
        tracing::info!("[修复115孤儿文件] 文件已在原整理目录，无需移动：{relative}");
    }
    Ok(raw)
}
pub async fn repair(state: &AppState, item_id: &str, context: &JobContext) -> Result<Value> {
    let (rows, selected) = scope(state, item_id).await?;
    let config = state.config.read().await.clone();
    let root = string(&config, "p115_fingerprint_media_root_cid");
    let local = string(&config, "local_strm_root");
    let cd2 = string(&config, "cd2_media_root");
    let cloud = string(&config, "p115_fingerprint_media_root_path");
    if root.is_empty() || [&local, &cd2, &cloud].iter().all(|s| s.is_empty()) {
        return Err(ApiError::bad("未配置媒体路径根目录或115媒体库根CID"));
    }
    let candidates = candidates(&rows, selected.as_ref(), &[&local, &cd2, &cloud]);
    tracing::info!(
        "[修复115孤儿文件] 从 {} 条当前范围记录提取 {} 个视频，开始核对原整理目录",
        rows.len(),
        candidates.len()
    );
    let mut reader = Reader {
        state,
        context,
        last: None,
        requests: 0,
        children: BTreeMap::new(),
        directories: BTreeMap::new(),
        parents: BTreeMap::new(),
    };
    let mut result = json!({"checked":candidates.len(),"repaired_videos":0,"repaired_sidecars":0,"already_correct":0,"search_missing":0,"skipped_mismatch":0,"failed":0,"failures":[]});
    for (index, candidate) in candidates.iter().enumerate() {
        context.checkpoint().await?;
        let path = string(candidate, "path");
        context
            .update_counted(
                index as u64,
                candidates.len() as u64,
                "115孤儿文件",
                &format!("正在检查：{path}"),
            )
            .await;
        let message = match reader.check(candidate, &root).await {
            Ok((key, count)) => {
                result[key] = json!(result[key].as_u64().unwrap() + 1);
                result["repaired_sidecars"] =
                    json!(result["repaired_sidecars"].as_u64().unwrap() + count as u64);
                match key {
                    "repaired_videos" => {
                        tracing::info!("[修复115孤儿文件] 已恢复：{path}，关联文件 {count} 个");
                        format!("已恢复：{path}，关联文件 {count} 个")
                    }
                    "already_correct" => {
                        tracing::info!("[修复115孤儿文件] 文件已在原整理目录，无需移动：{path}");
                        format!("原位正常：{path}")
                    }
                    "search_missing" => {
                        tracing::warn!("[修复115孤儿文件] 搜索未命中，未移动：{path}");
                        format!("搜索未命中：{path}")
                    }
                    _ => {
                        tracing::warn!(
                            "[修复115孤儿文件] 文件位于其他有效目录，跳过自动移动：{path}"
                        );
                        format!("其他有效目录，跳过：{path}")
                    }
                }
            }
            Err(error) => {
                if context.is_cancelled() {
                    return Err(error);
                }
                tracing::error!("[修复115孤儿文件] 处理失败 {path}：{}", error.1);
                result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                result["failures"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(format!("{path}：{}", error.1)));
                format!("处理失败 {path}：{}", error.1)
            }
        };
        context
            .update_counted(
                (index + 1) as u64,
                candidates.len() as u64,
                "115孤儿文件",
                &format!(
                    "{message}；恢复{}，失败{}，115请求{}次",
                    result["repaired_videos"], result["failed"], reader.requests
                ),
            )
            .await;
    }
    context.checkpoint().await?;
    result["requests"] = json!(reader.requests);
    let message = format!(
        "115孤儿文件检查完成：检查 {} 个视频，恢复 {} 个，关联文件 {} 个，原位 {} 个，搜索未命中 {} 个，其他有效目录跳过 {} 个，失败 {} 个，115请求 {} 次",
        candidates.len(),
        result["repaired_videos"],
        result["repaired_sidecars"],
        result["already_correct"],
        result["search_missing"],
        result["skipped_mismatch"],
        result["failed"],
        reader.requests
    );
    context
        .update_counted(
            candidates.len() as u64,
            candidates.len() as u64,
            "115孤儿文件",
            &message,
        )
        .await;
    result["message"] = json!(message);
    Ok(result)
}
