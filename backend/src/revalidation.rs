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
    source: &'static str,
}
impl<'a> Resolver<'a> {
    fn new(audit: Option<&'a AuditReader<'a>>) -> Self {
        Self {
            audit,
            context: None,
            previous: None,
            invalid_roots: HashSet::new(),
            directories: BTreeMap::new(),
            source: "现场查询",
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
        resolver.source = "文件详情";
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
            Ok(found) => {
                resolver.source = "云端目录";
                return Ok(found);
            }
            Err(error) if resolver.context.is_some_and(JobContext::is_cancelled) => {
                return Err(error);
            }
            Err(error) => tracing::debug!("115 目录定位失败，尝试搜索：{}", error.1),
        }
    }
    let (info, relative) = search_file(state, target, path, size, &config, resolver).await?;
    resolver.source = "搜索";
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
    write_resolved(state, snapshot, index, info, relative, false).await
}
async fn write_resolved(
    state: &AppState,
    snapshot: &mut Value,
    index: usize,
    info: &Value,
    relative: Option<String>,
    fill_only: bool,
) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    versions::lock(
        &mut transaction,
        &string(snapshot, "item_type"),
        &string(snapshot, "tmdb_id"),
        &string(snapshot, "parent_series_tmdb_id"),
    )
    .await?;
    let current:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('asset_details_json',asset_details_json,'file_sha1_json',file_sha1_json,'file_pickcode_json',file_pickcode_json) FROM media_metadata WHERE tmdb_id=$1 AND item_type=$2 AND in_library=true FOR UPDATE").bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).fetch_optional(&mut *transaction).await?;
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
    let need_pc = fingerprints::missing(&pcs[index]);
    let need_sha1 = fingerprints::missing(&hashes[index]) || need_pc;
    if !fill_only || need_sha1 {
        hashes[index] = json!(string(info, "sha1").to_uppercase());
    }
    if !fill_only || need_pc {
        pcs[index] = info["pick_code"].clone();
    }
    if !fill_only {
        assets[index]["size_bytes"] = info["size"].clone();
        assets[index]["fingerprint_pending"] = json!(false);
        assets[index]["fingerprint_revision"] =
            json!(assets[index]["fingerprint_revision"].as_u64().unwrap_or(0) + 1);
    }
    let preid:Option<String>=sqlx::query_scalar("SELECT preid FROM p115_filesystem_cache WHERE id=$1 AND UPPER(sha1)=$2 AND pick_code=$3 FOR UPDATE").bind(string(info,"id")).bind(string(info,"sha1").to_uppercase()).bind(string(info,"pick_code")).fetch_optional(&mut *transaction).await?.flatten();
    sqlx::query("DELETE FROM p115_filesystem_cache WHERE id=$1")
        .bind(string(info, "id"))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,pick_code,local_path,size,preid) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(parent_id,name) DO UPDATE SET id=EXCLUDED.id,sha1=EXCLUDED.sha1,pick_code=EXCLUDED.pick_code,local_path=EXCLUDED.local_path,size=EXCLUDED.size,preid=EXCLUDED.preid,updated_at=NOW()")
        .bind(string(info,"id")).bind(string(info,"parent_id")).bind(string(info,"name")).bind(string(info,"sha1").to_uppercase()).bind(string(info,"pick_code")).bind(relative).bind(identity::size(&info["size"]).unwrap() as i64).bind(preid).execute(&mut *transaction).await?;
    if !fill_only || need_sha1 || need_pc {
        sqlx::query("UPDATE media_metadata SET asset_details_json=$1,file_sha1_json=$2,file_pickcode_json=$3,last_updated_at=NOW() WHERE tmdb_id=$4 AND item_type=$5").bind(json!(assets)).bind(json!(hashes)).bind(json!(pcs)).bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    if !fill_only || need_sha1 || need_pc {
        snapshot["asset_details_json"] = json!(assets);
        snapshot["file_sha1_json"] = json!(hashes);
        snapshot["file_pickcode_json"] = json!(pcs);
    }
    Ok(())
}
async fn write_cached(
    state: &AppState,
    snapshot: &mut Value,
    index: usize,
    info: &Value,
) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    versions::lock(
        &mut transaction,
        &string(snapshot, "item_type"),
        &string(snapshot, "tmdb_id"),
        &string(snapshot, "parent_series_tmdb_id"),
    )
    .await?;
    let current: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('asset_details_json',asset_details_json,'file_sha1_json',file_sha1_json,'file_pickcode_json',file_pickcode_json) FROM media_metadata WHERE tmdb_id=$1 AND item_type=$2 AND in_library=true FOR UPDATE")
        .bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).fetch_optional(&mut *transaction).await?;
    if current.as_ref().is_none_or(|row| {
        ["asset_details_json", "file_sha1_json", "file_pickcode_json"]
            .iter()
            .any(|key| row[*key] != snapshot[*key])
    }) {
        return Err(ApiError::bad("媒体版本已变化，请重新校验"));
    }
    let cached: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'parent_id',parent_id,'name',name,'sha1',sha1,'pick_code',pick_code,'size',size,'local_path',local_path) FROM p115_filesystem_cache WHERE id=$1 FOR UPDATE")
        .bind(string(info,"id")).fetch_optional(&mut *transaction).await?;
    if cached.as_ref() != Some(info) {
        return Err(ApiError::bad("115 缓存已变化，请重新补齐"));
    }
    let mut hashes = list(&snapshot["file_sha1_json"]);
    let mut pcs = list(&snapshot["file_pickcode_json"]);
    let count = list(&snapshot["asset_details_json"]).len();
    hashes.resize(count.max(hashes.len()), Value::Null);
    pcs.resize(count.max(pcs.len()), Value::Null);
    let need_pc = fingerprints::missing(&pcs[index]);
    if need_pc || fingerprints::missing(&hashes[index]) {
        hashes[index] = json!(string(info, "sha1").to_uppercase());
    }
    if need_pc {
        pcs[index] = info["pick_code"].clone();
    }
    sqlx::query("UPDATE media_metadata SET file_sha1_json=$1,file_pickcode_json=$2,last_updated_at=NOW() WHERE tmdb_id=$3 AND item_type=$4")
        .bind(json!(hashes)).bind(json!(pcs)).bind(string(snapshot,"tmdb_id")).bind(string(snapshot,"item_type")).execute(&mut *transaction).await?;
    transaction.commit().await?;
    snapshot["file_sha1_json"] = json!(hashes);
    snapshot["file_pickcode_json"] = json!(pcs);
    Ok(())
}
pub(crate) fn media_label(row: &Value, asset: &Value, index: usize) -> String {
    let title = text(row, &["title", "tmdb_id"]);
    let mut label = match row["item_type"].as_str() {
        Some("Episode") => {
            let parent = text(row, &["series_title", "parent_series_title"]);
            let season = text(row, &["season_number"]);
            let episode = text(row, &["episode_number"]);
            let series = if parent.is_empty() {
                title.clone()
            } else {
                parent
            };
            let mut label = format!("《{series}》");
            if !season.is_empty() {
                label.push_str(&format!(" · 第{season}季"));
            }
            if !episode.is_empty() {
                if season.is_empty() {
                    label.push_str(" · ");
                }
                label.push_str(&format!("第{episode}集"));
            }
            if title != series && !title.is_empty() {
                label.push_str(&format!(" · {title}"));
            }
            label
        }
        _ => format!("《{title}》"),
    };
    let path = string(asset, "path").replace('\\', "/");
    if let Some(filename) = path.rsplit('/').next().filter(|name| !name.is_empty()) {
        label.push_str(&format!(" · {filename}"));
    }
    format!(
        "{label} · 版本 #{}（Item {}）",
        index + 1,
        string(asset, "emby_item_id")
    )
}

pub(crate) async fn fingerprint_rows(
    state: &AppState,
    ids: Option<&[String]>,
) -> Result<Vec<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT to_jsonb(f) FROM (
            SELECT m.tmdb_id,m.item_type,m.title,m.original_title,m.release_year,
                   m.parent_series_tmdb_id,m.season_number,m.episode_number,
                   m.emby_item_ids_json,m.asset_details_json,m.file_sha1_json,m.file_pickcode_json,
                   p.title AS series_title
            FROM media_metadata m
            LEFT JOIN media_metadata p ON m.parent_series_tmdb_id=p.tmdb_id AND p.item_type='Series'
            WHERE m.in_library=true AND m.item_type IN ('Movie','Episode')
              AND ($1::text[] IS NULL OR m.emby_item_ids_json ?| $1)
            ORDER BY m.item_type,m.title NULLS LAST,m.tmdb_id
        ) f",
    )
    .bind(ids)
    .fetch_all(&state.db)
    .await?)
}

fn repair_size(asset: &Value) -> Option<u64> {
    ["size_bytes", "size", "Size", "file_size"]
        .iter()
        .find_map(|key| identity::size(&asset[*key]))
}

pub async fn repair(state: &AppState, context: &JobContext) -> Result<Value> {
    repair_selected(state, None, context).await
}

pub async fn repair_ids(state: &AppState, ids: &[String], context: &JobContext) -> Result<Value> {
    repair_selected(state, Some(ids), context).await
}

async fn repair_selected(
    state: &AppState,
    ids: Option<&[String]>,
    context: &JobContext,
) -> Result<Value> {
    context
        .activity("扫描在库视频", "正在扫描缺失 PC/SHA1 的在库视频...")
        .await;
    let mut rows = fingerprint_rows(state, ids).await?;
    let config = state.config.read().await.clone();
    let requested: Option<HashSet<_>> = ids.map(|ids| ids.iter().collect());
    let eligible = |asset: &Value| {
        let path = text(asset, &["path", "Path"]);
        asset.is_object()
            && requested
                .as_ref()
                .is_none_or(|ids| ids.contains(&text(asset, &["emby_item_id"])))
            && (asset["fingerprint_pending"] == true
                || (!path.is_empty()
                    && (std::path::Path::new(&media_paths::clean(&path))
                        .extension()
                        .is_none()
                        || !media_paths::extension(&path).is_empty())))
    };
    let total = rows
        .iter()
        .flat_map(|row| list(&row["asset_details_json"]))
        .filter(eligible)
        .count();
    let (
        mut checked,
        mut scanned,
        mut missing,
        mut fixed,
        mut cache_fixed,
        mut api_fixed,
        mut cache_updates,
        mut skipped,
        mut failed,
    ) = (0, 0, 0, 0, 0, 0, 0, 0, 0);
    let mut updated_rows = HashSet::new();
    let mut failed_ids = vec![];
    let mut resolver = Resolver::new(None);
    resolver.context = Some(context);
    tracing::info!("[补齐115指纹] 开始检查 {total} 个在库视频，优先使用 115 本地缓存");
    context
        .update_counted(
            0,
            total as u64,
            "补齐115指纹",
            &format!("正在检查 {total} 个在库视频..."),
        )
        .await;
    for row in &mut rows {
        for (index, asset) in list(&row["asset_details_json"])
            .iter()
            .enumerate()
            .filter(|(_, asset)| eligible(asset))
        {
            context.checkpoint().await?;
            let label = media_label(row, asset, index);
            context
                .update_counted(
                    checked,
                    total as u64,
                    "补齐115指纹",
                    &format!("正在检查 {label} 的 PC/SHA1 和 115 缓存"),
                )
                .await;
            checked += 1;
            if asset["fingerprint_pending"] == true {
                failed += 1;
                failed_ids.push(string(asset, "emby_item_id"));
                let message = format!("{label} 文件身份待校验，跳过旧缓存补齐");
                tracing::warn!("[补齐115指纹] {message}");
                context
                    .update_counted(checked, total as u64, "补齐115指纹", &message)
                    .await;
                continue;
            }
            scanned += 1;
            let stored_pc = list(&row["file_pickcode_json"])
                .get(index)
                .cloned()
                .unwrap_or(Value::Null);
            let stored_sha1 = list(&row["file_sha1_json"])
                .get(index)
                .cloned()
                .unwrap_or(Value::Null);
            let need_pc = fingerprints::missing(&stored_pc);
            let need_sha1 = fingerprints::missing(&stored_sha1) || need_pc;
            let needs_fields = need_pc || need_sha1;
            missing += usize::from(needs_fields);
            let result = async {
                let path = media_paths::clean(&text(asset, &["path", "Path"]));
                let strm_target = media_paths::strm_target(&path).await;
                let target = strm_target.as_deref().unwrap_or(&path);
                let candidates = media_paths::local_candidates(
                    &path,
                    strm_target.as_deref().unwrap_or(""),
                    &[
                        &string(&config, "local_strm_root"),
                        &string(&config, "cd2_media_root"),
                    ],
                );
                let pc = (!need_pc).then(|| {
                    stored_pc
                        .as_str()
                        .map(str::trim)
                        .map(str::to_owned)
                        .unwrap_or_else(|| stored_pc.to_string())
                });
                let pc = pc.as_deref();
                let mut cached = fingerprints::cached(state, pc, &candidates).await?;
                if cached.is_none() && pc.is_none() {
                    if let Some(target_pc) = fingerprints::pickcode(target) {
                        cached = fingerprints::cached(state, Some(&target_pc), &candidates).await?;
                    }
                }
                if let Some(info) = cached.as_ref().filter(|info| {
                    let mut effective = (*info).clone();
                    if !need_sha1 {
                        effective["sha1"] = stored_sha1.clone();
                    }
                    if let Some(pc) = pc {
                        effective["pick_code"] = json!(pc);
                    }
                    if let Some(size) = repair_size(asset) {
                        effective["size"] = json!(size);
                    }
                    if !path.to_lowercase().ends_with(".strm")
                        && let Some(name) = std::path::Path::new(&path)
                            .file_name()
                            .and_then(|name| name.to_str())
                    {
                        effective["name"] = json!(name);
                    }
                    fingerprints::cache_complete(&effective)
                }) {
                    if needs_fields {
                        context
                            .activity(
                                "缓存补齐指纹",
                                &format!("{label} 命中 115 本地缓存，正在补齐 PC/SHA1"),
                            )
                            .await;
                        write_cached(state, row, index, info).await?;
                    }
                    return Ok((needs_fields, false, false));
                }
                context
                    .activity(
                        "查询115云端",
                        &format!("{label} 的 PC/SHA1 或缓存字段缺失，正在现场查询115"),
                    )
                    .await;
                let known_pc = pc
                    .map(str::to_string)
                    .or_else(|| {
                        cached
                            .as_ref()
                            .map(|info| string(info, "pick_code"))
                            .filter(|pc| !pc.is_empty())
                    })
                    .or_else(|| fingerprints::pickcode(target));
                let fid = known_pc
                    .as_deref()
                    .and_then(fingerprints::file_id)
                    .or_else(|| {
                        cached
                            .as_ref()
                            .and_then(|info| string(info, "id").parse().ok())
                    });
                let (info, relative) = if let Some(fid) = fid {
                    resolver.source = "文件详情";
                    let info = normalized(&resolver.request(p115::file_info(state, fid)).await?);
                    if !fingerprints::cache_complete(&info)
                        || string(&info, "id").parse::<u64>().ok() != Some(fid)
                        || fingerprints::file_id(&string(&info, "pick_code")) != Some(fid)
                        || pc.is_some_and(|pc| info["pick_code"] != pc)
                    {
                        return Err(ApiError::bad(
                            "115 文件详情不完整或文件身份与已有 PickCode 不一致",
                        ));
                    }
                    let cache_path = cached
                        .as_ref()
                        .map(|info| string(info, "local_path"))
                        .filter(|path| !path.is_empty());
                    let relative = cache_path.or_else(|| {
                        relative(target, &config)
                            .or_else(|_| relative(&path, &config))
                            .ok()
                            .map(|mut parts| {
                                *parts.last_mut().unwrap() = string(&info, "name");
                                parts.join("/")
                            })
                    });
                    (info, relative)
                } else {
                    resolve_current(state, target, &path, repair_size(asset), &mut resolver).await?
                };
                tracing::info!(
                    "[补齐115指纹] 通过 115 {} 命中 {label}：{}",
                    resolver.source,
                    string(&info, "name")
                );
                // Python repair fills absent values; it never replaces an already populated
                // media/cache fingerprint with a fresh audit result.
                let mut values = cached.clone().unwrap_or_else(|| json!({}));
                values["id"] = info["id"].clone();
                values["name"] = info["name"].clone();
                if !need_sha1 {
                    values["sha1"] = stored_sha1.clone();
                }
                if let Some(pc) = pc {
                    values["pick_code"] = json!(pc);
                }
                if let Some(size) = repair_size(asset) {
                    values["size"] = json!(size);
                }
                for key in ["parent_id", "sha1", "pick_code", "size"] {
                    if fingerprints::missing(&values[key])
                        || (key == "size" && identity::size(&values[key]).is_none())
                    {
                        values[key] = info[key].clone();
                    }
                }
                context
                    .activity(
                        "回写补齐结果",
                        &format!("正在保存 {label} 的指纹补齐结果和115缓存"),
                    )
                    .await;
                write_resolved(state, row, index, &values, relative, true).await?;
                Ok::<_, ApiError>((false, true, true))
            }
            .await;
            let message = match result {
                Ok((used_cache, used_api, updated_cache)) => {
                    api_fixed += usize::from(used_api);
                    cache_updates += usize::from(updated_cache);
                    if needs_fields {
                        fixed += 1;
                        cache_fixed += usize::from(used_cache);
                        updated_rows.insert((string(row, "tmdb_id"), string(row, "item_type")));
                        let source = if used_cache {
                            "115 本地缓存"
                        } else {
                            "115 现场查询"
                        };
                        let message = format!("已补齐 {label}：PC=✓，SHA1=✓（{source}）");
                        tracing::info!("[补齐115指纹] {message}");
                        message
                    } else if updated_cache {
                        let message = format!("已体检并修复本地缓存 {label}");
                        tracing::info!("[补齐115指纹] {message}");
                        message
                    } else {
                        skipped += 1;
                        format!("{label} 的 PC/SHA1 和115缓存完整，已跳过现场查询")
                    }
                }
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => {
                    failed += 1;
                    failed_ids.push(string(asset, "emby_item_id"));
                    let message = format!("暂无法补齐 {label}：{}", error.1);
                    tracing::warn!("[补齐115指纹] {message}");
                    message
                }
            };
            context.update_counted(checked, total as u64, "补齐115指纹", &format!("{message}；已补齐 {fixed}，缓存命中 {cache_fixed}，现场查询 {api_fixed}，失败 {failed}")).await;
        }
    }
    context.checkpoint().await?;
    let message = format!(
        "补齐115指纹完成：扫描资产 {scanned}，缺失 {missing}，成功补齐 {fixed}，缓存命中 {cache_fixed}，现场查询 {api_fixed}，写回媒体项 {}，回写115缓存 {cache_updates}，完整跳过 {skipped}，失败 {failed}。",
        updated_rows.len()
    );
    tracing::info!("[补齐115指纹] {message}");
    context
        .update_counted(checked, total as u64, "补齐115指纹", &message)
        .await;
    Ok(
        json!({"scanned_assets":scanned,"missing_assets":missing,"fixed_assets":fixed,"cache_fixed_assets":cache_fixed,"api_fixed_assets":api_fixed,"updated_rows":updated_rows.len(),"cache_updates":cache_updates,"updated":fixed,"skipped":skipped,"failed_assets":failed,"failed":failed,"failed_ids":failed_ids,"message":message}),
    )
}

pub async fn ids(state: &AppState, ids: &[String], context: &JobContext) -> Result<Value> {
    let mut rows = fingerprint_rows(state, Some(ids)).await?;
    let requested: HashSet<_> = ids.iter().collect();
    let mut counts = BTreeMap::<String, usize>::new();
    for row in &rows {
        for asset in list(&row["asset_details_json"]) {
            let id = string(&asset, "emby_item_id");
            if requested.contains(&id) && list(&row["emby_item_ids_json"]).contains(&json!(id)) {
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
    for id in &failed_ids {
        let reason = if counts.get(id).is_some_and(|count| *count > 1) {
            "媒体版本绑定不唯一"
        } else {
            "缺少在库媒体记录或有效版本绑定"
        };
        tracing::warn!("[指纹重新校验] Item {id} 未更新：{reason}");
    }
    context
        .update_counted(
            checked as u64,
            total as u64,
            "指纹校验",
            &format!("开始校验 {total} 个媒体项，绑定异常 {failed} 项"),
        )
        .await;
    let mut resolver = Resolver::new(None);
    resolver.context = Some(context);
    for row in &mut rows {
        for (index, asset) in list(&row["asset_details_json"]).iter().enumerate() {
            let id = string(asset, "emby_item_id");
            if !requested.contains(&id) || counts.get(&id) != Some(&1) {
                continue;
            }
            context.checkpoint().await?;
            let label = media_label(row, asset, index);
            context
                .update_counted(
                    checked as u64,
                    total as u64,
                    "读取媒体信息",
                    &format!("正在读取 {label} 的 Emby 信息"),
                )
                .await;
            checked += 1;
            if crate::emby::path_ignored(state, &string(asset, "path")).await? {
                skipped += 1;
                let message = format!("已跳过 {label}：路径在忽略范围内");
                tracing::info!("[指纹重新校验] {message}");
                context
                    .update_counted(checked as u64, total as u64, "指纹校验", &message)
                    .await;
                continue;
            }
            let result = async {
                mark_pending(state, row, index, None).await?;
                let item = details(state, &id, "Id,Type,Name,Path,Size,MediaSources").await?;
                let observed = observation(state, &item).await;
                if observed.0 != string(asset, "path") {
                    return Err(ApiError::bad("Emby 路径已变化，请先同步媒体数据"));
                }
                context
                    .activity("查询115云端", &format!("正在核对 {label} 的当前文件身份"))
                    .await;
                let (info, relative) =
                    resolve_current(state, &observed.1, &observed.0, observed.2, &mut resolver)
                        .await?;
                if observed
                    .2
                    .is_some_and(|size| Some(size) != identity::size(&info["size"]))
                {
                    return Err(ApiError::bad("Emby 与 115 大小不同，请等待上传完成"));
                }
                context
                    .activity(
                        "回读媒体信息",
                        &format!("正在确认 {label} 在校验期间未变化"),
                    )
                    .await;
                if observation(
                    state,
                    &details(state, &id, "Id,Type,Name,Path,Size,MediaSources").await?,
                )
                .await
                    != observed
                {
                    return Err(ApiError::bad("Emby 媒体信息在校验期间变化，请重试"));
                }
                context
                    .activity(
                        "回写指纹",
                        &format!("正在保存 {label} 的 PC/SHA1 和 115 缓存"),
                    )
                    .await;
                mark_pending(state, row, index, Some(&observed.1)).await?;
                write(state, row, index, &info, relative).await
            }
            .await;
            let message = match result {
                Ok(()) => {
                    updated += 1;
                    tracing::info!("[指纹重新校验] {label} 已核对并更新 PC/SHA1");
                    format!("已核对并更新 {label} 的 PC/SHA1")
                }
                Err(error) if context.is_cancelled() => return Err(error),
                Err(error) => {
                    failed += 1;
                    failed_ids.push(id.clone());
                    tracing::warn!("[指纹重新校验] {label} 未更新：{}", error.1);
                    format!("{label} 未更新：{}", error.1)
                }
            };
            context
                .update_counted(
                    checked as u64,
                    total as u64,
                    "指纹校验",
                    &format!("{message}；成功 {updated}，跳过 {skipped}，未完成 {failed}"),
                )
                .await;
        }
    }
    context.checkpoint().await?;
    context
        .update_counted(
            checked as u64,
            total as u64,
            "指纹校验",
            &format!("校验结束：已检查 {checked}/{total}，成功 {updated}，跳过 {skipped}，未完成 {failed}"),
        )
        .await;
    Ok(
        json!({"updated":updated,"skipped":skipped,"failed":failed,"failed_ids":failed_ids,"message":format!("指纹重新校验结束：成功 {updated}，跳过 {skipped}，未完成 {failed}")}),
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
