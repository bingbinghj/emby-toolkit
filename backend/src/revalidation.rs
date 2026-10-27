use crate::{
    AppState,
    error::{ApiError, Result},
    fingerprints, identity,
    jobs::JobContext,
    media_edit::{details, list},
    media_paths, p115,
    state::string,
    versions,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
};

fn text(data: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| match &data[*key] {
            Value::String(value) if !value.is_empty() => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}
fn normalized(data: &Value) -> Value {
    let parent = text(data, &["parent_id", "pid", "cid"]);
    let parent = if parent.is_empty() {
        data["paths"]
            .as_array()
            .and_then(|nodes| nodes.last())
            .map(|node| text(node, &["file_id", "cid", "id"]))
            .unwrap_or_default()
    } else {
        parent
    };
    json!({"id":text(data,&["id","file_id","fid"]),"parent_id":parent,"name":text(data,&["name","file_name","fn","n"]),"pick_code":text(data,&["pick_code","pickcode","pc"]),"sha1":text(data,&["sha1","sha","file_sha1"]),"size":(["size_byte","fs","size","file_size","s"].iter().find_map(|key|identity::size(&data[*key])))})
}
fn is_directory(raw: &Value) -> bool {
    ["fc", "file_category", "type"]
        .iter()
        .find_map(|key| raw.get(*key).filter(|v| !v.is_null()))
        .is_some_and(|v| *v == "0" || *v == 0)
        || ["is_dir", "is_folder", "is_directory"]
            .iter()
            .any(|key| raw[*key] == true || raw[*key] == 1)
        || (!text(raw, &["cid"]).is_empty() && text(raw, &["fid", "file_id", "id"]).is_empty())
}
fn relative(path: &str, config: &Value) -> Result<Vec<String>> {
    let path = path.replace('\\', "/");
    for key in ["cd2_media_root", "local_strm_root"] {
        let root = string(config, key)
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        if !root.is_empty()
            && let Some(suffix) = path.strip_prefix(&format!("{root}/"))
        {
            let parts = suffix.split('/').map(str::to_string).collect::<Vec<_>>();
            if !parts.is_empty() && parts.iter().all(|s| !matches!(s.as_str(), "" | "." | "..")) {
                return Ok(parts);
            }
        }
    }
    Err(ApiError::bad("文件路径无法映射到已配置的 STRM/CD2 根目录"))
}
pub struct AuditReader<'a> {
    pub context: &'a JobContext,
    pub previous: tokio::sync::Mutex<Option<tokio::time::Instant>>,
    pub invalid_roots: tokio::sync::Mutex<HashSet<String>>,
}
impl AuditReader<'_> {
    async fn request(&self) -> Result<tokio::sync::MutexGuard<'_, Option<tokio::time::Instant>>> {
        self.context.checkpoint().await?;
        let mut previous = self.previous.lock().await;
        if let Some(before) = *previous {
            tokio::time::sleep_until(before + std::time::Duration::from_secs(5)).await;
            self.context.checkpoint().await?;
        }
        *previous = Some(tokio::time::Instant::now());
        Ok(previous)
    }
}
struct Resolver<'a> {
    audit: Option<&'a AuditReader<'a>>,
    context: Option<&'a JobContext>,
    previous: Option<tokio::time::Instant>,
    invalid_roots: HashSet<String>,
    directories: BTreeMap<String, Vec<Value>>,
}
impl<'a> Resolver<'a> {
    fn new(audit: Option<&'a AuditReader<'a>>) -> Self {
        Self {
            audit,
            context: None,
            previous: None,
            invalid_roots: HashSet::new(),
            directories: BTreeMap::new(),
        }
    }
    async fn request<T>(&mut self, future: impl Future<Output = Result<T>>) -> Result<T> {
        let mut guard = if let Some(reader) = self.audit {
            Some(reader.request().await?)
        } else {
            None
        };
        if let Some(context) = self.context {
            context.checkpoint().await?;
            if let Some(previous) = self.previous {
                tokio::time::sleep_until(previous + std::time::Duration::from_millis(500)).await;
                context.checkpoint().await?;
            }
        }
        let result = future.await;
        let now = tokio::time::Instant::now();
        self.previous = Some(now);
        if let Some(guard) = guard.as_mut() {
            **guard = Some(now);
        }
        result
    }
}
async fn children(
    state: &AppState,
    parent: &str,
    resolver: &mut Resolver<'_>,
) -> Result<Vec<Value>> {
    let mut result = vec![];
    let mut seen = HashSet::new();
    loop {
        let page = resolver
            .request(p115::files(state, parent, result.len()))
            .await?;
        let data = page["data"]
            .as_array()
            .ok_or_else(|| ApiError::bad("115 文件分页格式无效"))?;
        for item in data {
            if !seen.insert(text(item, &["fid", "file_id", "id", "cid"])) {
                return Err(ApiError::bad("115 分页未推进"));
            }
        }
        result.extend(data.clone());
        let total = page["count"]
            .as_u64()
            .or_else(|| page["count"].as_str().and_then(|s| s.parse().ok()));
        if total.is_some_and(|n| result.len() as u64 >= n) {
            return Ok(result);
        }
        if data.is_empty() {
            if total.is_some_and(|n| (result.len() as u64) < n) {
                return Err(ApiError::bad("115 目录结果不完整"));
            }
            return Ok(result);
        }
    }
}
pub async fn resolve(
    state: &AppState,
    target: &str,
    path: &str,
) -> Result<(Value, Option<String>)> {
    resolve_with_reader(state, target, path, None).await
}
pub async fn resolve_with_reader(
    state: &AppState,
    target: &str,
    path: &str,
    reader: Option<&AuditReader<'_>>,
) -> Result<(Value, Option<String>)> {
    resolve_with_size(state, target, path, None, reader).await
}
pub async fn resolve_with_size(
    state: &AppState,
    target: &str,
    path: &str,
    size: Option<u64>,
    reader: Option<&AuditReader<'_>>,
) -> Result<(Value, Option<String>)> {
    resolve_current(state, target, path, size, &mut Resolver::new(reader)).await
}
async fn resolve_current(
    state: &AppState,
    target: &str,
    path: &str,
    size: Option<u64>,
    resolver: &mut Resolver<'_>,
) -> Result<(Value, Option<String>)> {
    let config = state.config.read().await.clone();
    if target.starts_with("http://") || target.starts_with("https://") {
        let pc = fingerprints::pickcode(target)
            .ok_or_else(|| ApiError::bad("HTTP STRM 缺少可验证的 115 PickCode"))?;
        let fid = fingerprints::file_id(&pc)
            .ok_or_else(|| ApiError::bad("115 PickCode 无法转换文件 ID"))?;
        let info = normalized(&resolver.request(p115::file_info(state, fid)).await?);
        if info["id"].as_str().and_then(|s| s.parse::<u64>().ok()) != Some(fid)
            || info["pick_code"] != pc
        {
            return Err(ApiError::bad("115 返回的文件身份与 STRM PickCode 不一致"));
        }
        if ["sha1", "name", "parent_id"]
            .iter()
            .any(|k| string(&info, k).is_empty())
            || identity::size(&info["size"]).is_none()
        {
            return Err(ApiError::bad("115 文件详情不完整"));
        }
        let relative = relative(path, &config).ok().map(|mut parts| {
            *parts.last_mut().unwrap() = string(&info, "name");
            parts.join("/")
        });
        return Ok((info, relative));
    }
    let parts = relative(target, &config);
    if let Ok(parts) = &parts {
        match locate(state, &config, parts, resolver).await {
            Ok(found) => return Ok(found),
            Err(error) if resolver.context.is_some_and(JobContext::is_cancelled) => {
                return Err(error);
            }
            Err(error) => tracing::debug!("115 目录定位失败，尝试搜索：{}", error.1),
        }
    }
    let (info, relative) = search_file(state, target, path, size, &config, resolver).await?;
    let root = string(&config, "p115_fingerprint_media_root_cid");
    // Audits only read the cloud; repair/webhook tasks may restore verified orphan hits.
    if let (Some(context), Some(relative)) = (resolver.context, relative.as_deref())
        && !root.is_empty()
        && !resolver.invalid_roots.contains(&root)
    {
        let parent = string(&info, "parent_id")
            .parse()
            .map_err(|_| ApiError::bad("115 父目录ID无效"))?;
        let response = resolver
            .request(p115::file_info_response(state, parent))
            .await?;
        if !p115::success(&response) {
            if !p115::missing(&response) {
                return Err(ApiError::bad("无法确认 115 文件父目录状态"));
            }
            let restored = resolver
                .request(crate::orphans::restore_search_hit(
                    state, &info, relative, context,
                ))
                .await?;
            resolver.directories.clear();
            let restored = normalized(&restored);
            if ["id", "pick_code", "sha1", "name", "size"]
                .iter()
                .any(|key| restored[*key] != info[*key])
            {
                return Err(ApiError::bad("孤儿恢复后文件身份已变化"));
            }
            return Ok((restored, Some(relative.into())));
        }
    }
    Ok((info, relative))
}
async fn locate(
    state: &AppState,
    config: &Value,
    parts: &[String],
    resolver: &mut Resolver<'_>,
) -> Result<(Value, Option<String>)> {
    let root = string(config, "p115_fingerprint_media_root_cid");
    if root.is_empty() {
        return Err(ApiError::bad("请先配置 115 云端媒体库根目录 CID"));
    }
    if resolver.invalid_roots.contains(&root)
        || if let Some(audit) = resolver.audit {
            audit.invalid_roots.lock().await.contains(&root)
        } else {
            false
        }
    {
        return Err(ApiError::bad("115 云端媒体库根目录已失效"));
    }
    let mut parent = root.clone();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        // Reuse complete intermediate directories only within this batch. File
        // listings stay fresh so a later Item cannot inherit an older PC/SHA1.
        let all = match if !last && resolver.directories.contains_key(&parent) {
            Ok(resolver.directories[&parent].clone())
        } else {
            children(state, &parent, resolver).await
        } {
            Ok(all) => all,
            Err(error) => {
                if parent == root && error.0 == axum::http::StatusCode::NOT_FOUND {
                    resolver.invalid_roots.insert(root.clone());
                    if let Some(audit) = resolver.audit {
                        audit.invalid_roots.lock().await.insert(root.clone());
                    }
                }
                return Err(error);
            }
        };
        if !last {
            resolver
                .directories
                .entry(parent.clone())
                .or_insert_with(|| all.clone());
        }
        let matches = all
            .iter()
            .filter(|raw| {
                let directory = is_directory(raw);
                let name = text(raw, &["name", "file_name", "fn", "n"]);
                let same = if last && part.to_lowercase().ends_with(".strm") {
                    let name = std::path::Path::new(&name);
                    name.file_stem() == std::path::Path::new(part).file_stem()
                        && !media_paths::extension(name.to_str().unwrap_or("")).is_empty()
                        && media_paths::extension(name.to_str().unwrap_or("")) != ".strm"
                } else {
                    name == *part
                };
                same && directory != last
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(ApiError::bad(format!("115 目标不存在或不唯一：{part}")));
        }
        let mut info = normalized(matches[0]);
        if !last {
            info["id"] = json!(text(matches[0], &["id", "file_id", "fid", "cid"]));
        }
        if string(&info, "id").is_empty() {
            return Err(ApiError::bad("115 未返回文件身份"));
        }
        if last {
            if ["pick_code", "sha1"]
                .iter()
                .any(|k| string(&info, k).is_empty())
                || identity::size(&info["size"]).is_none()
            {
                return Err(ApiError::bad("115 文件 PC、SHA1 或大小不完整"));
            }
            info["parent_id"] = json!(parent);
            let mut relative = parts.to_vec();
            *relative.last_mut().unwrap() = string(&info, "name");
            return Ok((info, Some(relative.join("/"))));
        }
        parent = string(&info, "id");
    }
    Err(ApiError::bad("115 文件定位失败"))
}
fn search_path_matches(raw: &Value, name: &str, candidates: &[String]) -> bool {
    let nodes = raw["paths"].as_array().or_else(|| raw["path"].as_array());
    let Some(nodes) = nodes else {
        return false;
    };
    let mut parts = nodes
        .iter()
        .map(|node| text(node, &["file_name", "name", "fn", "n"]))
        .filter(|name| !name.is_empty() && !matches!(name.as_str(), "根目录" | "全部文件"))
        .collect::<Vec<_>>();
    parts.push(name.into());
    let full = parts.join("/");
    full.contains('/')
        && candidates.iter().any(|candidate| {
            candidate.contains('/')
                && (full == *candidate
                    || full.ends_with(&format!("/{candidate}"))
                    || candidate.ends_with(&format!("/{full}")))
        })
}
async fn search_file(
    state: &AppState,
    target: &str,
    path: &str,
    size: Option<u64>,
    config: &Value,
    resolver: &mut Resolver<'_>,
) -> Result<(Value, Option<String>)> {
    let candidates = media_paths::local_candidates(
        path,
        target,
        &[
            &string(config, "cd2_media_root"),
            &string(config, "local_strm_root"),
        ],
    );
    let mut names = HashSet::new();
    let mut verified = BTreeMap::new();
    for candidate in &candidates {
        let ext = media_paths::extension(candidate);
        if ext.is_empty() || ext == ".strm" {
            continue;
        }
        let Some(name) = std::path::Path::new(candidate)
            .file_name()
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        if !names.insert(name.to_string()) {
            continue;
        }
        let response = resolver.request(p115::search(state, name)).await?;
        if !p115::success(&response) {
            return Err(ApiError::bad("115 搜索失败"));
        }
        let hits = response["data"]
            .as_array()
            .ok_or_else(|| ApiError::bad("115 搜索结果格式无效"))?;
        let count = response["count"]
            .as_u64()
            .or_else(|| response["count"].as_str().and_then(|s| s.parse().ok()));
        if count.is_some_and(|count| count > hits.len() as u64)
            || (count.is_none() && hits.len() >= 100)
        {
            return Err(ApiError::bad("115 搜索结果不完整，无法确认唯一文件"));
        }
        let mut seen = HashSet::new();
        for hit in hits.iter().filter(|hit| !is_directory(hit)) {
            let listed = normalized(hit);
            if listed["name"] != name
                || size.is_some_and(|s| identity::size(&listed["size"]) != Some(s))
            {
                continue;
            }
            let fid = string(&listed, "id")
                .parse::<u64>()
                .map_err(|_| ApiError::bad("115 搜索结果缺少有效文件ID"))?;
            if !seen.insert(fid) {
                continue;
            }
            let raw = resolver.request(p115::file_info(state, fid)).await?;
            let info = normalized(&raw);
            if info["id"].as_str().and_then(|id| id.parse::<u64>().ok()) != Some(fid)
                || info["name"] != name
                || identity::size(&info["size"]).is_none()
                || size.is_some_and(|s| identity::size(&info["size"]) != Some(s))
                || ["pick_code", "sha1", "parent_id"]
                    .iter()
                    .any(|key| string(&info, key).is_empty())
                || fingerprints::file_id(&string(&info, "pick_code")) != Some(fid)
                || (!string(&listed, "pick_code").is_empty()
                    && listed["pick_code"] != info["pick_code"])
                || !search_path_matches(&raw, name, &candidates)
            {
                continue;
            }
            verified.insert(fid, info);
        }
    }
    if verified.len() != 1 {
        return Err(ApiError::bad("115 搜索目标不存在、身份不一致或不唯一"));
    }
    let info = verified.into_values().next().unwrap();
    let relative = relative(target, config)
        .or_else(|_| relative(path, config))
        .ok()
        .map(|mut parts| {
            *parts.last_mut().unwrap() = string(&info, "name");
            parts.join("/")
        });
    Ok((info, relative))
}
pub async fn observation(state: &AppState, item: &Value) -> (String, String, Option<u64>) {
    let path = string(item, "Path");
    let sources = list(&item["MediaSources"]);
    let matches = sources
        .iter()
        .filter(|s| {
            string(s, "Id").trim_start_matches("mediasource_") == string(item, "Id")
                || s["Path"] == path
        })
        .collect::<Vec<_>>();
    let source = if matches.len() == 1 {
        matches[0]
    } else {
        &Value::Null
    };
    let target = media_paths::strm_target(&path).await.unwrap_or_else(|| {
        source["Path"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&path)
            .replace('\\', "/")
    });
    let _ = state;
    (
        path,
        target,
        identity::size(&source["Size"]).or_else(|| identity::size(&item["Size"])),
    )
}
pub async fn mark_pending(
    state: &AppState,
    snapshot: &mut Value,
    index: usize,
    target: Option<&str>,
) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    let current:Option<Value>=sqlx::query_scalar("SELECT asset_details_json FROM media_metadata WHERE tmdb_id=$1 AND item_type=$2 AND in_library=true FOR UPDATE").bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).fetch_optional(&mut *transaction).await?;
    if current.as_ref() != Some(&snapshot["asset_details_json"]) {
        return Err(ApiError::bad("媒体版本已变化，请重新校验"));
    }
    let asset = &mut snapshot["asset_details_json"][index];
    asset["fingerprint_pending"] = json!(true);
    asset["fingerprint_revision"] = json!(asset["fingerprint_revision"].as_u64().unwrap_or(0) + 1);
    if let Some(target) = target {
        asset["fingerprint_target"] = json!(target);
    }
    sqlx::query(
        "UPDATE media_metadata SET asset_details_json=$1 WHERE tmdb_id=$2 AND item_type=$3",
    )
    .bind(&snapshot["asset_details_json"])
    .bind(string(snapshot, "tmdb_id"))
    .bind(string(snapshot, "item_type"))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}
pub async fn write(
    state: &AppState,
    snapshot: &mut Value,
    index: usize,
    info: &Value,
    relative: Option<String>,
) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    versions::lock(
        &mut transaction,
        &string(snapshot, "item_type"),
        &string(snapshot, "tmdb_id"),
        &string(snapshot, "parent_series_tmdb_id"),
    )
    .await?;
    let current:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE tmdb_id=$1 AND item_type=$2 AND in_library=true FOR UPDATE").bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).fetch_optional(&mut *transaction).await?;
    if current.as_ref().is_none_or(|row| {
        ["asset_details_json", "file_sha1_json", "file_pickcode_json"]
            .iter()
            .any(|k| row[*k] != snapshot[*k])
    }) {
        return Err(ApiError::bad("媒体版本已变化，请重新校验"));
    }
    let mut hashes = list(&snapshot["file_sha1_json"]);
    let mut pcs = list(&snapshot["file_pickcode_json"]);
    let mut assets = list(&snapshot["asset_details_json"]);
    hashes.resize(assets.len().max(hashes.len()), Value::Null);
    pcs.resize(assets.len().max(pcs.len()), Value::Null);
    hashes[index] = json!(string(info, "sha1").to_uppercase());
    pcs[index] = info["pick_code"].clone();
    assets[index]["size_bytes"] = info["size"].clone();
    assets[index]["fingerprint_pending"] = json!(false);
    assets[index]["fingerprint_revision"] =
        json!(assets[index]["fingerprint_revision"].as_u64().unwrap_or(0) + 1);
    let preid:Option<String>=sqlx::query_scalar("SELECT preid FROM p115_filesystem_cache WHERE id=$1 AND UPPER(sha1)=$2 AND pick_code=$3 FOR UPDATE").bind(string(info,"id")).bind(string(info,"sha1").to_uppercase()).bind(string(info,"pick_code")).fetch_optional(&mut *transaction).await?.flatten();
    sqlx::query("DELETE FROM p115_filesystem_cache WHERE id=$1")
        .bind(string(info, "id"))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,pick_code,local_path,size,preid) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(parent_id,name) DO UPDATE SET id=EXCLUDED.id,sha1=EXCLUDED.sha1,pick_code=EXCLUDED.pick_code,local_path=EXCLUDED.local_path,size=EXCLUDED.size,preid=EXCLUDED.preid,updated_at=NOW()")
        .bind(string(info,"id")).bind(string(info,"parent_id")).bind(string(info,"name")).bind(string(info,"sha1").to_uppercase()).bind(string(info,"pick_code")).bind(relative).bind(identity::size(&info["size"]).unwrap() as i64).bind(preid).execute(&mut *transaction).await?;
    sqlx::query("UPDATE media_metadata SET asset_details_json=$1,file_sha1_json=$2,file_pickcode_json=$3,last_updated_at=NOW() WHERE tmdb_id=$4 AND item_type=$5").bind(json!(assets)).bind(json!(hashes)).bind(json!(pcs)).bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).execute(&mut *transaction).await?;
    transaction.commit().await?;
    snapshot["asset_details_json"] = json!(assets);
    snapshot["file_sha1_json"] = json!(hashes);
    snapshot["file_pickcode_json"] = json!(pcs);
    Ok(())
}
pub async fn ids(state: &AppState, ids: &[String], context: &JobContext) -> Result<Value> {
    let mut rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE in_library=true AND item_type IN ('Movie','Episode') AND emby_item_ids_json ?| $1").bind(ids).fetch_all(&state.db).await?;
    let mut counts = BTreeMap::<String, usize>::new();
    for row in &rows {
        for asset in list(&row["asset_details_json"]) {
            let id = string(&asset, "emby_item_id");
            if ids.contains(&id) && list(&row["emby_item_ids_json"]).contains(&json!(id)) {
                *counts.entry(id).or_default() += 1;
            }
        }
    }
    let mut failed_ids = ids
        .iter()
        .filter(|id| counts.get(*id) != Some(&1))
        .cloned()
        .collect::<Vec<_>>();
    let mut failed = failed_ids.len();
    let mut updated = 0;
    let mut checked = failed;
    let mut skipped = 0;
    let total = failed + counts.values().filter(|count| **count == 1).count();
    let mut previous_progress = tokio::time::Instant::now();
    context
        .update(0, "指纹校验", &format!("开始校验 {total} 个媒体项"))
        .await;
    let mut resolver = Resolver::new(None);
    resolver.context = Some(context);
    for row in &mut rows {
        for (index, asset) in list(&row["asset_details_json"]).iter().enumerate() {
            let id = string(asset, "emby_item_id");
            if !ids.contains(&id) || counts.get(&id) != Some(&1) {
                continue;
            }
            context.checkpoint().await?;
            if checked > 0 && checked % 20 == 0 || previous_progress.elapsed().as_secs() >= 15 {
                context
                    .update(
                        (100 * checked / total.max(1)) as u64,
                        "指纹校验",
                        &format!("已检查 {checked}/{total}，成功 {updated}，跳过 {skipped}，未完成 {failed}"),
                    )
                    .await;
                previous_progress = tokio::time::Instant::now();
            }
            checked += 1;
            if crate::emby::path_ignored(state, &string(asset, "path")).await? {
                skipped += 1;
                continue;
            }
            let result = async {
                mark_pending(state, row, index, None).await?;
                let item = details(state, &id, "Id,Type,Name,Path,Size,MediaSources").await?;
                let observed = observation(state, &item).await;
                if observed.0 != string(asset, "path") {
                    return Err(ApiError::bad("Emby 路径已变化，请先同步媒体数据"));
                }
                let (info, relative) =
                    resolve_current(state, &observed.1, &observed.0, observed.2, &mut resolver)
                        .await?;
                if observed
                    .2
                    .is_some_and(|size| Some(size) != identity::size(&info["size"]))
                {
                    return Err(ApiError::bad("Emby 与 115 大小不同，请等待上传完成"));
                }
                if observation(
                    state,
                    &details(state, &id, "Id,Type,Name,Path,Size,MediaSources").await?,
                )
                .await
                    != observed
                {
                    return Err(ApiError::bad("Emby 媒体信息在校验期间变化，请重试"));
                }
                mark_pending(state, row, index, Some(&observed.1)).await?;
                write(state, row, index, &info, relative).await
            }
            .await;
            match result {
                Ok(()) => updated += 1,
                Err(error) => {
                    failed += 1;
                    failed_ids.push(id.clone());
                    tracing::warn!("指纹校验 Item {id} 未更新：{}", error.1);
                }
            }
        }
    }
    context.checkpoint().await?;
    context
        .update(
            100,
            "指纹校验",
            &format!("校验结束：已检查 {checked}/{total}，成功 {updated}，跳过 {skipped}，未完成 {failed}"),
        )
        .await;
    Ok(
        json!({"updated":updated,"failed":failed,"failed_ids":failed_ids,"message":format!("指纹重新校验结束：成功 {updated}，未完成 {failed}")}),
    )
}

pub async fn scope(state: &AppState, id: &str, context: &JobContext) -> Result<Vec<String>> {
    let item = details(state, id, "Id,Type,Name").await?;
    if matches!(item["Type"].as_str(), Some("Movie" | "Episode")) {
        return Ok(vec![id.into()]);
    }
    if !matches!(item["Type"].as_str(), Some("Series" | "Season")) {
        return Err(ApiError::bad("仅支持电影、电视剧、季和分集"));
    }
    Ok(crate::emby::items(state, id, "Id,Type", context)
        .await?
        .iter()
        .filter(|i| i["Type"] == "Episode")
        .map(|i| string(i, "Id"))
        .collect())
}
pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/api/media_info/revalidate_fingerprints/{id}",
        axum::routing::post(submit),
    )
}
async fn submit(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<(axum::http::StatusCode, axum::Json<Value>)> {
    let id = crate::orphans::current_id(&state, &id).await?;
    let work = state.clone();
    let task = state
        .jobs
        .submit("重新校验当前媒体指纹", move |context| async move {
            let scope = scope(&work, &id, &context).await?;
            ids(&work, &scope, &context).await
        })
        .await?;
    Ok((
        axum::http::StatusCode::ACCEPTED,
        axum::Json(json!({"message":"指纹校验任务已提交","task_id":task})),
    ))
}
