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
    let response = state
        .http
        .get(format!(
            "{}/Users/{}/Items/{id}",
            string(&config, "emby_server_url").trim_end_matches('/'),
            string(&config, "emby_user_id")
        ))
        .header("X-Emby-Token", string(&config, "emby_api_key"))
        .query(&[("Fields", "Id,Path")])
        .send()
        .await?;
    if response.status() != StatusCode::NOT_FOUND {
        response.error_for_status()?;
        return Ok(id.into());
    }
    let target = crate::media_edit::target(state, id).await?;
    let path = if target.path.starts_with("http") {
        let path = target
            .candidates
            .iter()
            .filter(|p| p.starts_with(string(&config, "local_strm_root")))
            .find_map(|p| {
                p.to_str()
                    .and_then(|s| s.strip_suffix("-mediainfo.json"))
                    .map(|s| format!("{s}.strm"))
            });
        let Some(path) = path else {
            return Ok(id.into());
        };
        path
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
                ("Fields", "Id,Path".into()),
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
    if new_id.is_empty() || new_id == id {
        return Ok(id.into());
    }
    let mut transaction = state.db.begin().await?;
    let current: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE emby_item_ids_json @> $1 FOR UPDATE",
    )
    .bind(json!([id]))
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(current) = current else {
        return Ok(id.into());
    };
    if !list(&current["asset_details_json"])
        .iter()
        .any(|a| a["path"] == target.path)
    {
        return Err(ApiError::bad("旧媒体路径已变化，停止恢复绑定"));
    }
    sqlx::query("UPDATE media_metadata SET emby_item_ids_json=(SELECT jsonb_agg(CASE WHEN eid=$1 THEN to_jsonb($2::text) ELSE to_jsonb(eid) END ORDER BY idx) FROM jsonb_array_elements_text(emby_item_ids_json) WITH ORDINALITY arr(eid,idx)),asset_details_json=(SELECT jsonb_agg(CASE WHEN a->>'emby_item_id'=$1 THEN jsonb_set(a,'{emby_item_id}',to_jsonb($2::text)) ELSE a END ORDER BY idx) FROM jsonb_array_elements(CASE WHEN jsonb_typeof(asset_details_json)='array' THEN asset_details_json ELSE '[]' END) WITH ORDINALITY arr(a,idx)),last_updated_at=NOW() WHERE emby_item_ids_json @> $3").bind(id).bind(&new_id).bind(json!([id])).execute(&mut *transaction).await?;
    for table in ["processed_log", "failed_log"] {
        sqlx::query(&format!("UPDATE {table} SET item_id=$1 WHERE item_id=$2 AND NOT EXISTS(SELECT 1 FROM {table} WHERE item_id=$1)"))
            .bind(&new_id).bind(id).execute(&mut *transaction).await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE item_id=$1"))
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(new_id)
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
        self.before().await?;
        let response = p115::search(self.state, &string(candidate, "stem")).await;
        self.after();
        let response = response?;
        if !p115::success(&response) {
            return Err(ApiError::bad("115搜索失败"));
        }
        let hits = list(&response["data"]);
        let Some(video) = hits.iter().find(|item| id(item) == fid) else {
            return Ok(("search_missing", 0));
        };
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
            let parent_id = current
                .parse()
                .map_err(|_| ApiError::bad("115父目录ID无效"))?;
            self.before().await?;
            let response = p115::file_info_response(self.state, parent_id).await;
            self.after();
            let response = response?;
            let exists = p115::success(&response);
            self.parents.insert(current.clone(), exists);
            exists
        };
        if exists {
            return Ok(("skipped_mismatch", 0));
        }
        if target.is_empty() {
            target = self.directory(root, &parts, true).await?;
            children = self.children(&target, false).await?;
        }
        let related = hits
            .iter()
            .filter(|item| {
                parent(item) == current && name(item).starts_with(&string(candidate, "stem"))
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
        self.before().await?;
        let response = p115::move_files(self.state, &ids, &target).await;
        self.after();
        let response = response?;
        if !p115::success(&response) {
            return Err(ApiError::bad("115移动失败"));
        }
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
    let (status, _) = reader.check(&candidate, &root).await?;
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
        match reader.check(candidate, &root).await {
            Ok((key, count)) => {
                result[key] = json!(result[key].as_u64().unwrap() + 1);
                result["repaired_sidecars"] =
                    json!(result["repaired_sidecars"].as_u64().unwrap() + count as u64);
            }
            Err(error) => {
                if context.is_cancelled() {
                    return Err(error);
                }
                result["failed"] = json!(result["failed"].as_u64().unwrap() + 1);
                result["failures"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(format!("{}：{}", candidate["path"], error.1)));
            }
        }
        context
            .update(
                ((index + 1) * 100 / candidates.len()) as u64,
                "115孤儿文件",
                &format!(
                    "已检查{}/{}，恢复{}，失败{}",
                    index + 1,
                    candidates.len(),
                    result["repaired_videos"],
                    result["failed"]
                ),
            )
            .await;
    }
    result["requests"] = json!(reader.requests);
    result["message"] = json!("当前媒体115孤儿文件检查完成");
    Ok(result)
}
