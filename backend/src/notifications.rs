use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

tokio::task_local! { static OPERATOR: String; }

pub fn current_operator() -> Option<String> {
    OPERATOR
        .try_with(Clone::clone)
        .ok()
        .filter(|id| !id.trim().is_empty())
}

pub async fn with_operator<F: std::future::Future>(
    user_id: impl Into<String>,
    future: F,
) -> F::Output {
    OPERATOR.scope(user_id.into(), future).await
}

/// Reject invalid notification controls before a configuration is persisted.
pub fn validate_config(config: &Value) -> Result<()> {
    if let Some(enabled) = config.get("telegram_enabled")
        && !enabled.is_boolean()
    {
        return Err(ApiError::bad("通知渠道开关必须是布尔值"));
    }
    for field in [
        "telegram_channel_name",
        "telegram_api_url",
        "telegram_topic_id",
        "telegram_users",
        "telegram_admins",
    ] {
        if let Some(value) = config.get(field)
            && !value.is_string()
        {
            return Err(ApiError::bad("Telegram 渠道配置必须为文本"));
        }
    }
    telegram_api_base(config, "https://api.telegram.org")?;
    topic_id(config)?;
    principal_ids(config, "telegram_users")?;
    principal_ids(config, "telegram_admins")?;
    if let Some(scope) = config.get("notification_scope") {
        let scope = scope
            .as_object()
            .ok_or_else(|| ApiError::bad("通知发送范围无效"))?;
        for (kind, action) in scope {
            if !matches!(kind.as_str(), "library_new" | "recognize_fail")
                || !matches!(
                    action.as_str(),
                    Some("user" | "admin" | "user,admin" | "all")
                )
            {
                return Err(ApiError::bad("通知消息类型或发送范围无效"));
            }
        }
    }
    if let Some(window) = config.get("notification_time")
        && (!window.is_object()
            || parse_time(&window["start"]).is_none()
            || parse_time(&window["end"]).is_none())
    {
        return Err(ApiError::bad("通知发送时间必须为 HH:MM 格式"));
    }
    Ok(())
}

/// A self-hosted Telegram Bot API uses the same base for delivery, polling and getMe.
pub fn telegram_api_base(config: &Value, fallback: &str) -> Result<String> {
    if let Some(value) = config.get("telegram_api_url")
        && !value.is_string()
    {
        return Err(ApiError::bad("Telegram API URL 必须为文本"));
    }
    let configured = string(config, "telegram_api_url");
    let base = if configured.trim().is_empty() {
        fallback
    } else {
        configured.trim()
    };
    let url = url::Url::parse(base).map_err(|_| ApiError::bad("Telegram API URL 无效"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad(
            "Telegram API URL 需要有效的 HTTP(S) 服务地址",
        ));
    }
    Ok(base.trim_end_matches('/').to_string())
}

pub fn topic_id(config: &Value) -> Result<Option<i64>> {
    let topic = string(config, "telegram_topic_id");
    let topic = topic.trim();
    if topic.is_empty() {
        return Ok(None);
    }
    if !topic.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(ApiError::bad("Telegram Topic ID 必须为正整数"));
    }
    topic
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(Some)
        .ok_or_else(|| ApiError::bad("Telegram Topic ID 必须为正整数"))
}

pub fn principal_ids(config: &Value, field: &str) -> Result<BTreeSet<String>> {
    let text = string(config, field);
    if text.trim().is_empty() {
        return Ok(BTreeSet::new());
    }
    text.split(',')
        .map(|part| {
            let id = part.trim();
            let parsed = id.parse::<i64>().ok().filter(|id| *id > 0);
            if !id.bytes().all(|digit| digit.is_ascii_digit()) || parsed.is_none() {
                return Err(ApiError::bad("Telegram 用户 ID 需为逗号分隔的正整数"));
            }
            Ok(parsed.unwrap().to_string())
        })
        .collect()
}

pub fn topic_payload(config: &Value, payload: &Value) -> Result<Value> {
    let mut payload = payload.clone();
    let channel = crate::system_ops::chat_id(&string(config, "telegram_channel_id"));
    let chat = payload["chat_id"]
        .as_str()
        .map(str::to_string)
        .or_else(|| payload["chat_id"].as_i64().map(|id| id.to_string()))
        .unwrap_or_default();
    if !channel.is_empty()
        && chat == channel
        && let Some(topic) = topic_id(config)?
    {
        payload["message_thread_id"] = json!(topic);
    }
    Ok(payload)
}

fn parse_time(value: &Value) -> Option<chrono::NaiveTime> {
    let text = value.as_str()?;
    if text.len() != 5
        || text.as_bytes()[2] != b':'
        || !text
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 2 || b.is_ascii_digit())
    {
        return None;
    }
    chrono::NaiveTime::parse_from_str(text, "%H:%M").ok()
}

pub fn in_send_window(config: &Value, now: chrono::NaiveTime) -> bool {
    let Some(window) = config.get("notification_time") else {
        return true;
    };
    let (Some(start), Some(end)) = (parse_time(&window["start"]), parse_time(&window["end"]))
    else {
        return false;
    };
    // The UI specifies minutes. Include the whole end minute, including 23:59.
    let now = chrono::Timelike::with_second(&now, 0).unwrap();
    let now = chrono::Timelike::with_nanosecond(&now, 0).unwrap();
    if start <= end {
        now >= start && now <= end
    } else {
        now >= start || now <= end
    }
}

fn enabled(config: &Value, kind: &str) -> bool {
    config["telegram_enabled"].as_bool().unwrap_or(true)
        && !string(config, "telegram_bot_token").trim().is_empty()
        && list(&config["telegram_notify_types"]).contains(&json!(kind))
}

/// Like MP, automatic admissions without an operating user fall back to administrators.
/// Emby's configured service account must never be mistaken for that operating user.
async fn targets(state: &AppState, config: &Value, kind: &str) -> BTreeSet<String> {
    let action = config["notification_scope"][kind]
        .as_str()
        .unwrap_or("admin");
    let mut actor = current_operator();
    if matches!(action, "user" | "user,admin")
        && let Some(id) = &actor
        && !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM emby_users WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(false)
    {
        actor = None;
    }
    let all = action == "all";
    let admin = matches!(action, "admin" | "user,admin") || (action == "user" && actor.is_none());
    let user = matches!(action, "user" | "user,admin") && actor.is_some();
    let mut targets: BTreeSet<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT e.telegram_chat_id FROM emby_users u JOIN emby_users_extended e ON u.id=e.emby_user_id WHERE ($1 OR ($2 AND u.is_administrator=true) OR ($3 AND u.id=$4)) AND e.telegram_chat_id IS NOT NULL AND COALESCE(e.status,'active')='active' AND COALESCE(u.policy_json->>'IsDisabled','false')='false'",
    )
    .bind(all).bind(admin).bind(user).bind(actor.as_deref())
    .fetch_all(&state.db).await
    .unwrap_or_else(|error| {
        tracing::warn!("[通知] 读取用户接收列表失败：{error}");
        vec![]
    })
    .into_iter().flatten().collect();
    let channel = string(config, "telegram_channel_id");
    if (admin || all) && !channel.trim().is_empty() {
        targets.insert(channel);
    }
    if admin || all {
        targets.extend(principal_ids(config, "telegram_admins").unwrap_or_default());
    }
    targets
        .into_iter()
        .filter_map(|target| {
            let target = target.trim();
            if target.is_empty() {
                None
            } else if target.starts_with("https://t.me/") {
                Some(format!("@{}", target.rsplit('/').next().unwrap_or("")))
            } else {
                Some(target.to_string())
            }
        })
        .collect()
}

pub struct Service {
    pub api_base: String,
    queue: tokio::sync::Mutex<VecDeque<(String, String, Value)>>,
}
impl Default for Service {
    fn default() -> Self {
        Self::with_api_base("https://api.telegram.org".into())
    }
}
impl Service {
    pub fn with_api_base(api_base: String) -> Self {
        Self {
            api_base,
            queue: tokio::sync::Mutex::new(VecDeque::new()),
        }
    }
    pub async fn pending(&self) -> usize {
        self.queue.lock().await.len()
    }
}

pub async fn run(state: AppState, stop: tokio_util::sync::CancellationToken) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = timer.tick() => {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = flush(&state) => {},
                }
            },
        }
    }
}

pub async fn flush(state: &AppState) {
    loop {
        let config = state.config.read().await.clone();
        if !in_send_window(&config, chrono::Local::now().time()) {
            return;
        }
        let Some((kind, method, payload)) = state.notifications.queue.lock().await.pop_front()
        else {
            return;
        };
        if !enabled(&config, &kind) {
            continue;
        }
        let token = string(&config, "telegram_bot_token");
        if let Err(error) = send(state, &token, &method, &payload).await {
            tracing::warn!("[通知] 延迟通知发送失败：{}", error.1);
        }
    }
}

async fn deliver(
    state: &AppState,
    kind: &str,
    token: &str,
    method: &str,
    payload: &Value,
) -> Result<bool> {
    let config = state.config.read().await.clone();
    if !in_send_window(&config, chrono::Local::now().time()) {
        state.notifications.queue.lock().await.push_back((
            kind.into(),
            method.into(),
            payload.clone(),
        ));
        tracing::info!("[通知] 当前不在发送时间内，通知已排队");
        Ok(false)
    } else {
        send(state, token, method, payload).await?;
        Ok(true)
    }
}

pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| {
            if "_*[]()~`>#+-=|{}.!".contains(c) {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}
pub fn ranges(episodes: &[(i64, i64)]) -> String {
    let mut seasons = BTreeMap::<i64, BTreeSet<i64>>::new();
    for (s, e) in episodes {
        seasons.entry(*s).or_default().insert(*e);
    }
    let mut parts = vec![];
    for (s, episodes) in seasons {
        let episodes = episodes.into_iter().collect::<Vec<_>>();
        let (mut start, mut previous) = (episodes[0], episodes[0]);
        let label = |start, previous| {
            if start == previous {
                format!("S{s:02}E{start:02}")
            } else {
                format!("S{s:02}E{start:02}-E{previous:02}")
            }
        };
        for e in episodes.into_iter().skip(1) {
            if e == previous + 1 {
                previous = e;
            } else {
                parts.push(label(start, previous));
                start = e;
                previous = e;
            }
        }
        parts.push(label(start, previous));
    }
    parts.join(", ")
}
async fn episode_numbers(
    state: &AppState,
    ids: &[String],
    context: Option<&crate::jobs::JobContext>,
) -> Result<(Vec<(i64, i64)>, Vec<Value>)> {
    let mut episodes = Vec::new();
    let mut native = Vec::new();
    for id in ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        let item = async {
            let _main = crate::strm::slot(state, "main", context).await?;
            crate::media_edit::details(
                state,
                id,
                "Id,Type,Path,MediaSources,IndexNumber,ParentIndexNumber",
            )
            .await
        }
        .await;
        if let Ok(item) = item.as_ref() {
            if item["Id"] == id && item["Type"] == "Episode" {
                native.push(item.clone());
            }
            if let (Some(season), Some(episode)) = (
                item["ParentIndexNumber"].as_i64(),
                item["IndexNumber"].as_i64(),
            ) && item["Id"] == id
                && season != 0
                && episode != 0
            {
                episodes.push((season, episode));
                continue;
            }
        }
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        match sqlx::query_scalar::<_, Value>("SELECT jsonb_build_array(season_number,episode_number) FROM media_metadata WHERE item_type='Episode' AND emby_item_ids_json @> $1 LIMIT 1")
            .bind(json!([id])).fetch_optional(&state.db).await
        {
            Ok(Some(numbers)) => {
                if let (Some(season), Some(episode)) = (numbers[0].as_i64(), numbers[1].as_i64())
                    && season != 0 && episode != 0 { episodes.push((season, episode)); }
            }
            Err(error) => tracing::warn!(item_id = id, "[通知] 从数据库读取分集号失败：{error}"),
            _ => (),
        }
    }
    Ok((episodes, native))
}
pub fn title_pages(name: &str, report: &Value) -> Vec<String> {
    let heading = format!("⚠️ *标题待手动填充*：{}\n", markdown_value(name));
    let mut page = heading.clone();
    let mut pages = vec![];
    for entry in list(&report["pending"]) {
        let label =
            if let (Some(s), Some(e)) = (entry["season"].as_i64(), entry["episode"].as_i64()) {
                format!("S{s:02}E{e:02}")
            } else {
                entry["name"]
                    .as_str()
                    .or_else(|| entry["item_id"].as_str())
                    .unwrap_or("")
                    .into()
            };
        let kind = match entry["type"].as_str() {
            Some("Audio") => "音轨",
            Some("Subtitle") => "字幕",
            _ => "媒体项",
        };
        let ordinal = entry["ordinal"]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let language = entry["language"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("未标注")
            .chars()
            .take(80)
            .collect::<String>();
        let reason = entry["reason"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("需手动检查")
            .chars()
            .take(300)
            .collect::<String>();
        let line = format!(
            "{}\n",
            escape(&format!(
                "{} · {kind}{ordinal}（{language}）：{reason}",
                label.chars().take(180).collect::<String>()
            ))
        );
        if page.encode_utf16().count() + line.encode_utf16().count() > 3500 {
            pages.push(page.trim_end().into());
            page = heading.clone();
        }
        page.push_str(&line);
    }
    if page != heading {
        pages.push(page.trim_end().into());
    }
    pages
}
// Built-in admission templates follow the user's MoviePilot Jinja2 template.
fn markdown_value(text: &str) -> String {
    let shortened = if text.chars().count() > 300 {
        format!("{}...", text.chars().take(300).collect::<String>())
    } else {
        text.to_string()
    };
    escape(&shortened.replace('\\', "\\\\"))
}
fn present(text: &str) -> bool {
    !matches!(text.trim(), "" | "未知" | "unknown")
}
fn compact_size(bytes: u64) -> String {
    let mut amount = bytes as f64;
    let mut unit = "B";
    for next in ["K", "M", "G", "T", "P"] {
        if amount < 1024.0 {
            break;
        }
        amount /= 1024.0;
        unit = next;
    }
    format!("{amount:.1}{unit}")
}
fn source_format(source: &Value) -> String {
    let container = string(source, "Container")
        .trim()
        .trim_start_matches('.')
        .to_lowercase();
    if container == "strm" {
        return String::new();
    }
    let known = match container.as_str() {
        "matroska" | "matroska,webm" => "MKV",
        "mov,mp4,m4a,3gp,3g2,mj2" => "MP4",
        "mpegts" | "mpeg-ts" => "TS",
        "mkv" | "mp4" | "mov" | "m4v" | "avi" | "ts" | "m2ts" | "mpeg" | "mpg" | "wmv" | "asf"
        | "flv" | "f4v" | "iso" | "rmvb" | "rm" | "webm" | "3gp" | "tp" => &container,
        _ => "",
    };
    if !known.is_empty() {
        return known.to_uppercase();
    }
    let path = crate::media_paths::clean(&string(source, "Path"));
    let extension = std::path::Path::new(&path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    match extension.as_str() {
        "mkv" | "mp4" | "mov" | "m4v" | "avi" | "ts" | "m2ts" | "mpeg" | "mpg" | "wmv" | "asf"
        | "flv" | "f4v" | "iso" | "rmvb" | "rm" | "webm" | "3gp" | "tp" => extension.to_uppercase(),
        _ => String::new(),
    }
}
fn native_size(item: &Value, source: &Value) -> Option<u64> {
    if string(source, "Container")
        .trim()
        .trim_start_matches('.')
        .eq_ignore_ascii_case("strm")
    {
        return None;
    }
    let path = string(source, "Path");
    let path = if path.is_empty() {
        string(item, "Path")
    } else {
        path
    };
    if path.is_empty() || crate::media_paths::extension(&path) == ".strm" {
        return None;
    }
    crate::identity::size(&source["Size"])
}
fn native_source(item: &Value) -> Option<Value> {
    let source = crate::media_info::item_source(item, &[&string(item, "Path")])?;
    let mut result = source.clone();
    if string(&result, "Path").is_empty() {
        result["Path"] = item["Path"].clone();
    }
    if source_format(&result).is_empty() {
        return None;
    }
    result["Size"] = native_size(item, source).map_or(Value::Null, |size| json!(size));
    Some(result)
}
fn bound_sha(records: &[Value], id: &str) -> Option<String> {
    let mut hashes = BTreeSet::new();
    for record in records {
        let ids = list(&record["emby_item_ids_json"]);
        let assets = list(&record["asset_details_json"]);
        let sha = list(&record["file_sha1_json"]);
        if ids.len() != assets.len() || ids.len() != sha.len() {
            continue;
        }
        for (index, _) in ids
            .iter()
            .enumerate()
            .filter(|(_, key)| key.as_str() == Some(id))
        {
            let asset = &assets[index];
            if asset["fingerprint_pending"] == true || asset["emby_item_id"] != id {
                return None;
            }
            let hash = sha[index].as_str()?.trim();
            if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            hashes.insert(hash.to_uppercase());
        }
    }
    (hashes.len() == 1)
        .then(|| hashes.into_iter().next())
        .flatten()
}
fn verified_cache_source(hash: &str, infos: &[Value], files: &[Value]) -> Option<Value> {
    let files = files
        .iter()
        .filter(|file| {
            string(file, "sha1").eq_ignore_ascii_case(hash)
                && crate::identity::size(&file["size"]).is_some()
                && crate::media_paths::extension(&string(file, "name")) != ".strm"
        })
        .collect::<Vec<_>>();
    let expected = files
        .iter()
        .map(|file| crate::identity::size(&file["size"]).unwrap())
        .collect::<BTreeSet<_>>();
    if expected.len() > 1 {
        return None;
    }
    let expected = expected.into_iter().next();
    let file_name = files
        .first()
        .filter(|first| files.iter().all(|file| file["name"] == first["name"]))
        .map(|file| string(file, "name"));
    let mut candidates = infos
        .iter()
        .filter(|entry| string(entry, "sha1").eq_ignore_ascii_case(hash))
        .filter_map(|entry| {
            let info = &entry["info"];
            if info.as_array().is_some_and(|entries| entries.len() != 1)
                || !crate::media_info::recoverable(info)
            {
                return None;
            }
            let source = crate::media_info::container(info)?;
            let size = crate::identity::size(&source["Size"])?;
            if expected.is_some_and(|expected| expected != size)
                || source_format(source).is_empty()
                || (crate::media_paths::extension(&string(source, "Path")) == ".strm"
                    && expected.is_none())
            {
                return None;
            }
            Some(source.clone())
        })
        .collect::<Vec<_>>();
    if let Some(first) = candidates.first()
        && candidates.iter().all(|source| {
            crate::identity::size(&source["Size"]) == crate::identity::size(&first["Size"])
                && source_format(source) == source_format(first)
        })
    {
        let mut source = candidates.pop()?;
        if (string(&source, "Path").is_empty()
            || crate::media_paths::extension(&string(&source, "Path")) == ".strm")
            && let Some(name) = file_name
                .filter(|name| source_format(&json!({"Path":name})) == source_format(&source))
        {
            source["Path"] = json!(name);
        }
        return Some(source);
    }
    let file = files.first()?;
    let format = source_format(&json!({"Path":file["name"]}));
    if format.is_empty()
        || files
            .iter()
            .any(|other| source_format(&json!({"Path":other["name"]})) != format)
    {
        return None;
    }
    Some(
        json!({"Size":expected?,"Path":file_name.unwrap_or_default(),"Container":format.to_lowercase()}),
    )
}
fn source_conflicts(current: &Value, cached: &Value) -> bool {
    let current_path = crate::media_paths::clean(&string(current, "Path"));
    let cached_path = crate::media_paths::clean(&string(cached, "Path"));
    if source_format(current) != source_format(cached)
        || (!source_format(&json!({"Path":current_path})).is_empty()
            && !source_format(&json!({"Path":cached_path})).is_empty()
            && std::path::Path::new(&current_path).file_name()
                != std::path::Path::new(&cached_path).file_name())
        || matches!((crate::identity::size(&current["RunTimeTicks"]), crate::identity::size(&cached["RunTimeTicks"])), (Some(a), Some(b)) if a != b)
    {
        return true;
    }
    let current = list(&current["MediaStreams"]);
    let cached = list(&cached["MediaStreams"]);
    for kind in ["Video", "Audio"] {
        let current = current
            .iter()
            .filter(|stream| stream["Type"] == kind)
            .collect::<Vec<_>>();
        let cached = cached
            .iter()
            .filter(|stream| stream["Type"] == kind)
            .collect::<Vec<_>>();
        if current.is_empty() || cached.is_empty() {
            continue;
        }
        if current.len() != cached.len()
            || current.iter().zip(&cached).any(|(a, b)| {
                [
                    "Codec",
                    "Width",
                    "Height",
                    "Channels",
                    "SampleRate",
                    "IsExternal",
                ]
                .iter()
                .any(|field| {
                    !a[*field].is_null()
                        && !b[*field].is_null()
                        && match (a[*field].as_str(), b[*field].as_str()) {
                            (Some(a), Some(b)) => !a.eq_ignore_ascii_case(b),
                            _ => a[*field] != b[*field],
                        }
                })
            })
        {
            return true;
        }
    }
    false
}
async fn notice_sources(
    state: &AppState,
    ids: &[String],
    records: &[Value],
    native: &[Value],
) -> BTreeMap<String, Value> {
    let mut sources = BTreeMap::new();
    for item in native
        .iter()
        .filter(|item| ids.contains(&string(item, "Id")))
    {
        let id = string(item, "Id");
        if let Some(source) = native_source(item) {
            sources.insert(id, source);
        }
    }
    let bindings = ids
        .iter()
        .filter(|id| {
            sources
                .get(*id)
                .and_then(|source| crate::identity::size(&source["Size"]))
                .is_none()
        })
        .filter_map(|id| bound_sha(records, id).map(|sha| (id.clone(), sha)))
        .collect::<BTreeMap<_, _>>();
    if bindings.is_empty() {
        return sources;
    }
    let hashes = bindings
        .values()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let infos: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('sha1',sha1,'info',mediainfo_json) FROM p115_mediainfo_cache WHERE UPPER(sha1)=ANY($1) AND mediainfo_json IS NOT NULL")
        .bind(&hashes).fetch_all(&state.db).await.unwrap_or_else(|error| {tracing::warn!("[通知] 读取本次源媒体缓存失败：{error}"); vec![]});
    let files: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('sha1',sha1,'name',name,'size',size) FROM p115_filesystem_cache WHERE UPPER(sha1)=ANY($1) AND size>0")
        .bind(&hashes).fetch_all(&state.db).await.unwrap_or_else(|error| {tracing::warn!("[通知] 读取本次115原文件信息失败：{error}"); vec![]});
    for (id, sha) in bindings {
        if let Some(source) = verified_cache_source(&sha, &infos, &files) {
            if let Some(current) = sources.get_mut(&id) {
                if !source_conflicts(current, &source) {
                    current["Size"] = source["Size"].clone();
                }
            } else {
                sources.insert(id, source);
            }
        }
    }
    sources
}
fn media_caption(
    item: &Value,
    ids: &[String],
    records: &[Value],
    episodes: &[(i64, i64)],
    sources: &BTreeMap<String, Value>,
) -> Result<String> {
    let ids = ids
        .iter()
        .filter(|id| !id.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>();
    let item_shell = crate::media_paths::extension(&string(item, "Path")) == ".strm"
        || crate::media_info::item_source(item, &[&string(item, "Path")]).is_some_and(|source| {
            string(source, "Container")
                .trim()
                .trim_start_matches('.')
                .eq_ignore_ascii_case("strm")
        });
    let mut assets = records
        .iter()
        .flat_map(|record| list(&record["asset_details_json"]))
        .filter(|asset| ids.contains(&string(asset, "emby_item_id")))
        .collect::<Vec<_>>();
    for id in &ids {
        if !assets.iter().any(|asset| asset["emby_item_id"] == *id) {
            if let Some(source) = sources.get(id) {
                assets.push(json!({"emby_item_id":id,"path":source["Path"],"container":source["Container"]}));
            } else if string(item, "Id") == *id
                && let Some(source) = crate::media_info::item_source(item, &[&string(item, "Path")])
            {
                assets.push(
                    json!({"emby_item_id":id,"path":item["Path"],"container":source["Container"]}),
                );
            }
        }
    }
    let sizes = ids
        .iter()
        .map(|id| {
            if let Some(source) = sources.get(id) {
                return crate::identity::size(&source["Size"]);
            }
            if item_shell && item["Id"] == *id {
                return native_source(item)
                    .and_then(|source| crate::identity::size(&source["Size"]));
            }
            let matching = assets
                .iter()
                .filter(|asset| string(asset, "emby_item_id") == *id)
                .collect::<Vec<_>>();
            let first = matching.first()?;
            if matching.iter().any(|asset| *asset != *first) {
                return None;
            }
            if first["fingerprint_pending"] == true {
                return None;
            }
            let path = string(first, "path");
            if crate::media_paths::extension(&path) == ".strm" {
                if string(item, "Id") != *id
                    || crate::media_paths::clean(&string(item, "Path"))
                        != crate::media_paths::clean(&path)
                {
                    return None;
                }
                let source = crate::media_info::item_source(item, &[&path])?;
                if source_format(source).is_empty() {
                    return None;
                }
                return native_size(item, source);
            }
            if string(first, "container")
                .trim()
                .trim_start_matches('.')
                .eq_ignore_ascii_case("strm")
            {
                return None;
            }
            crate::identity::size(&first["size_bytes"])
        })
        .collect::<Option<Vec<_>>>();
    let size = sizes.and_then(|sizes| sizes.into_iter().try_fold(0u64, u64::checked_add));
    let mut groups = BTreeSet::new();
    let mut qualities = BTreeSet::new();
    for asset in &assets {
        let fallback = (asset["emby_item_id"] == item["Id"])
            .then(|| native_source(item))
            .flatten();
        let source = sources
            .get(&string(asset, "emby_item_id"))
            .or(fallback.as_ref());
        let pending = asset["fingerprint_pending"] == true;
        if pending && source.is_none() {
            continue;
        }
        let asset_path = crate::media_paths::clean(&string(asset, "path"));
        let shell_asset = crate::media_paths::extension(&asset_path) == ".strm"
            || string(asset, "container")
                .trim()
                .trim_start_matches('.')
                .eq_ignore_ascii_case("strm");
        let path =
            crate::media_paths::clean(&source.map(|source| string(source, "Path")).unwrap_or_else(
                || {
                    if pending || shell_asset || (item_shell && asset["emby_item_id"] == item["Id"])
                    {
                        String::new()
                    } else {
                        asset_path.clone()
                    }
                },
            ));
        let path = if crate::media_paths::extension(&path) == ".strm" {
            String::new()
        } else {
            path
        };
        let cached_allowed =
            source.is_none() && !pending && !shell_asset && !path.is_empty() && path == asset_path;
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let fields = crate::rename::pt(&name);
        let group = string(&fields, "releaseGroup");
        if present(&group) {
            groups.insert(group);
        }
        let audio = if cached_allowed {
            list(&asset["audio_tracks"])
        } else {
            vec![]
        };
        let audio = audio
            .iter()
            .find(|audio| audio["is_default"] == true)
            .or_else(|| audio.first());
        let streams = source
            .map(|source| list(&source["MediaStreams"]))
            .unwrap_or_default();
        let video = streams.iter().find(|stream| stream["Type"] == "Video");
        let native_audio = streams
            .iter()
            .find(|stream| stream["Type"] == "Audio" && stream["IsDefault"] == true)
            .or_else(|| streams.iter().find(|stream| stream["Type"] == "Audio"))
            .map(|stream| string(stream, "Codec").to_uppercase())
            .unwrap_or_default();
        let parsed_audio = string(&fields, "audioCodec");
        let audio = if present(&native_audio) {
            native_audio
        } else if present(&parsed_audio) {
            parsed_audio
        } else {
            audio
                .map(|audio| string(audio, "codec").to_uppercase())
                .unwrap_or_default()
        };
        let format = source
            .map(source_format)
            .filter(|format| !format.is_empty())
            .unwrap_or_else(|| {
                if cached_allowed {
                    source_format(&json!({"Path":path,"Container":asset["container"]}))
                } else {
                    String::new()
                }
            });
        let mut quality = [
            ("quality_display", "resourceType"),
            ("resolution_display", "videoFormat"),
            ("effect_display", "effect"),
            ("codec_display", "videoCodec"),
        ]
        .into_iter()
        .map(|(cached, parsed)| {
            let native = video
                .map(|video| match cached {
                    "codec_display" => string(video, "Codec").to_uppercase(),
                    "effect_display" => string(video, "VideoRange"),
                    "resolution_display" => {
                        let width = crate::identity::size(&video["Width"]).unwrap_or(0);
                        let height = crate::identity::size(&video["Height"]).unwrap_or(0);
                        if width >= 3800 || height >= 2100 {
                            "2160p"
                        } else if width >= 1800 || height >= 1000 {
                            "1080p"
                        } else if width >= 1200 || height >= 700 {
                            "720p"
                        } else if width >= 700 || height >= 480 {
                            "480p"
                        } else {
                            ""
                        }
                        .into()
                    }
                    _ => String::new(),
                })
                .unwrap_or_default();
            let parsed = string(&fields, parsed);
            if present(&native) {
                native
            } else if present(&parsed) {
                parsed
            } else if cached_allowed {
                string(asset, cached)
            } else {
                String::new()
            }
        })
        .filter(|value| present(value))
        .collect::<Vec<_>>();
        if present(&audio) {
            quality.push(audio);
        }
        if !format.is_empty() {
            quality.insert(0, format);
        }
        if source.is_some() {
            for stream in streams
                .iter()
                .filter(|stream| matches!(stream["Type"].as_str(), Some("Video" | "Audio")))
            {
                let codec = string(stream, "Codec").to_uppercase();
                if present(&codec)
                    && !quality.iter().any(|value| {
                        value
                            .to_uppercase()
                            .replace('.', "")
                            .contains(&codec.replace('.', ""))
                    })
                {
                    quality.push(codec);
                }
            }
        }
        if !quality.is_empty() {
            qualities.insert(quality.join(" "));
        }
    }
    let name = item["Name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ID:{}", string(item, "Id")));
    let title = item["ProductionYear"]
        .as_i64()
        .filter(|year| *year != 0)
        .map_or_else(|| name.clone(), |year| format!("{name} ({year})"));
    let tmdb = string(&item["ProviderIds"], "Tmdb");
    let verified_tmdb = tmdb
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| tmdb.parse::<u64>().ok().filter(|id| *id > 0))
        .flatten();
    let link = verified_tmdb
        .map(|tmdb| {
            let kind = if item["Type"] == "Movie" {
                "movie"
            } else {
                "tv"
            };
            format!("https://www.themoviedb.org/{kind}/{tmdb}")
        })
        .unwrap_or_default();
    let mut genres = list(&item["Genres"])
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<BTreeSet<_>>();
    if genres.is_empty() {
        genres.extend(
            records
                .iter()
                .flat_map(|record| list(&record["genres_json"]))
                .filter_map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| value["name"].as_str().map(str::to_string))
                }),
        );
    }
    let context = json!({"file_count":ids.len(),"title_year":markdown_value(&title),
        "__mediainfo__":{"detail_link":link}, "type":if item["Type"] == "Movie" {"电影"} else {"电视剧"},
        "season_episode":markdown_value(&ranges(episodes)), "category":markdown_value(&genres.into_iter().collect::<Vec<_>>().join("、")),
        "releaseGroup":markdown_value(&groups.into_iter().collect::<Vec<_>>().join(" / ")),
        "resource_term":markdown_value(&qualities.into_iter().collect::<Vec<_>>().join(" / ")),
        "webSource":"", "audioCodec":"", "artist":"", "album":"", "audio_specs":"", "err_msg":"",
        "total_size":markdown_value(&size.filter(|_| !ids.is_empty()).map(compact_size).unwrap_or_else(|| "未知".into()))});
    let environment = minijinja::Environment::new();
    let render = |template| {
        environment
            .render_str(template, minijinja::value::Serde(&context))
            .map_err(|error| ApiError::bad(format!("通知模板渲染失败：{error}")))
    };
    Ok(format!(
        "{}{}",
        render(include_str!("../assets/notification_library_title.jinja"))?,
        render(include_str!("../assets/notification_library_text.jinja"))?
    ))
}

pub async fn media(
    state: &AppState,
    item: &Value,
    _updated: bool,
    episode_ids: &[String],
    title_report: &Value,
    native: &[Value],
    context: Option<&crate::jobs::JobContext>,
) -> Result<()> {
    let config = state.config.read().await.clone();
    let token = string(&config, "telegram_bot_token");
    if !enabled(&config, "library_new") {
        return Ok(());
    }
    let fallback_name = format!("ID:{}", string(item, "Id"));
    let name = item["Name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or(&fallback_name);
    let title = item["ProductionYear"]
        .as_i64()
        .filter(|year| *year != 0)
        .map_or_else(|| name.to_string(), |year| format!("{name} ({year})"));
    let ids = if item["Type"] == "Series" || !episode_ids.is_empty() {
        episode_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        vec![string(item, "Id")]
    };
    let mut query_ids = ids.clone();
    query_ids.push(string(item, "Id"));
    let records: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM media_metadata m WHERE m.emby_item_ids_json ?| $1",
    )
    .bind(&query_ids)
    .fetch_all(&state.db)
    .await
    .unwrap_or_else(|error| {
        tracing::warn!("[通知] 《{title}》读取入库详情失败：{error}");
        vec![]
    });
    let (episodes, mut native_items) = if item["Type"] == "Series" {
        episode_numbers(state, &ids, context).await?
    } else {
        (vec![], vec![])
    };
    native_items.extend_from_slice(native);
    native_items.push(item.clone());
    if let Some(context) = context {
        context.checkpoint().await?;
    }
    let sources = notice_sources(state, &ids, &records, &native_items).await;
    let mut caption = media_caption(item, &ids, &records, &episodes, &sources)?;
    let mut pages = title_pages(name, title_report);
    if !pages.is_empty() {
        if pages.len() == 1
            && caption.encode_utf16().count() + pages[0].encode_utf16().count() + 2 <= 1000
        {
            caption.push_str("\n\n");
            caption.push_str(&pages.pop().unwrap());
        } else {
            caption.push_str("\n\n⚠️ *标题待手动填充*：清单见后续消息");
        }
    }
    let image: Option<String> = sqlx::query_scalar("SELECT COALESCE(NULLIF(m.backdrop_path,''),NULLIF(m.poster_path,''),CASE WHEN m.item_type='Episode' THEN COALESCE(NULLIF(p.backdrop_path,''),NULLIF(p.poster_path,'')) END) FROM media_metadata m LEFT JOIN media_metadata p ON m.parent_series_tmdb_id=p.tmdb_id AND p.item_type='Series' WHERE m.emby_item_ids_json @> $1 LIMIT 1")
        .bind(json!([string(item,"Id")])).fetch_optional(&state.db).await
        .unwrap_or_else(|error| { tracing::warn!("[通知] 《{title}》读取通知图片失败：{error}"); None }).flatten();
    for target in targets(state, &config, "library_new").await {
        if let Some(context) = context {
            context.checkpoint().await?;
        }
        tracing::info!(item_id = %string(item, "Id"), "[通知] 开始发送《{title}》入库结果");
        let (method, payload) = if let Some(image) = image
            .as_ref()
            .filter(|_| caption.encode_utf16().count() <= 1000)
        {
            (
                "sendPhoto",
                json!({"chat_id":target,"photo":format!("https://image.tmdb.org/t/p/w780{image}"),"caption":caption,"parse_mode":"MarkdownV2","disable_notification":false}),
            )
        } else {
            (
                "sendMessage",
                json!({"chat_id":target,"text":caption,"parse_mode":"MarkdownV2","disable_web_page_preview":true,"disable_notification":false}),
            )
        };
        match deliver(state, "library_new", &token, method, &payload).await {
            Ok(sent) => {
                let result = if sent { "已发送" } else { "已排队" };
                tracing::info!(item_id = %string(item, "Id"), "[通知] 《{title}》入库结果{result}")
            }
            Err(error) => {
                tracing::warn!(item_id = %string(item, "Id"), target, "[通知] 《{title}》入库结果发送失败：{}", error.1)
            }
        }
        for page in &pages {
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            if let Err(error) = deliver(state, "library_new", &token, "sendMessage", &json!({"chat_id":target,"text":page,"parse_mode":"MarkdownV2","disable_web_page_preview":true,"disable_notification":false})).await {
                tracing::warn!(item_id = %string(item, "Id"), target, "[通知] 《{title}》后续消息发送失败：{}", error.1);
            }
        }
    }
    Ok(())
}

/// A failed admission is a real organization result, independently of success notifications.
pub async fn failure(state: &AppState, item: &Value, reason: &str) -> Result<()> {
    let config = state.config.read().await.clone();
    if !enabled(&config, "recognize_fail") {
        return Ok(());
    }
    let token = string(&config, "telegram_bot_token");
    let name = item["Name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("未知媒体");
    let reason = crate::logs::redact(reason)
        .chars()
        .take(1500)
        .collect::<String>();
    let text = format!(
        "❌ *整理失败*：{}\n\n⚠️ 失败：{}\n\n请在媒体整理中查看失败记录。",
        markdown_value(name),
        escape(&reason.replace('\\', "\\\\"))
    );
    for target in targets(state, &config, "recognize_fail").await {
        if let Err(error) = deliver(
            state,
            "recognize_fail",
            &token,
            "sendMessage",
            &json!({
                "chat_id": target, "text": text, "parse_mode":"MarkdownV2",
                "disable_web_page_preview":true, "disable_notification":false
            }),
        )
        .await
        {
            tracing::warn!(item_id = %string(item, "Id"), "[通知] 《{name}》整理失败通知发送失败：{}", error.1);
        }
    }
    Ok(())
}

pub async fn test_message(state: &AppState, token: &str, chat: &str) -> Result<()> {
    test_message_draft(state, token, chat, &Value::Null).await
}

pub async fn test_message_draft(
    state: &AppState,
    token: &str,
    chat: &str,
    draft: &Value,
) -> Result<()> {
    let mut config = state.config.read().await.clone();
    for (source, target) in [
        ("api_url", "telegram_api_url"),
        ("topic_id", "telegram_topic_id"),
    ] {
        if let Some(value) = draft.get(source) {
            config[target] = value.clone();
        }
    }
    if draft.get("topic_id").is_some() {
        config["telegram_channel_id"] = json!(chat);
    }
    validate_config(&config)?;
    let text = format!(
        "🔔 Emby Toolkit 测试消息\n\n这是一条测试消息。\n时间：{}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    // A test is an explicit user action and, like MP, bypasses the scheduled window.
    send_with_config(
        state,
        &config,
        token,
        "sendMessage",
        &json!({
            "chat_id":chat, "text":text, "disable_web_page_preview":true
        }),
    )
    .await
}

async fn send(state: &AppState, token: &str, method: &str, payload: &Value) -> Result<()> {
    let config = state.config.read().await.clone();
    send_with_config(state, &config, token, method, payload).await
}

async fn send_with_config(
    state: &AppState,
    config: &Value,
    token: &str,
    method: &str,
    payload: &Value,
) -> Result<()> {
    let api_base = telegram_api_base(config, &state.notifications.api_base)?;
    let payload = topic_payload(config, payload)?;
    let response = state
        .http
        .post(format!("{}/bot{token}/{method}", api_base))
        .timeout(std::time::Duration::from_secs(if method == "sendPhoto" {
            30
        } else {
            15
        }))
        .json(&payload)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(ApiError(
            reqwest::StatusCode::BAD_GATEWAY,
            format!("Telegram 返回 HTTP {}", response.status().as_u16()),
        ));
    }
    let response: Value = response.json().await?;
    if response["ok"] != true {
        return Err(ApiError(
            reqwest::StatusCode::BAD_GATEWAY,
            format!(
                "Telegram 发送失败：{}",
                crate::logs::redact(
                    &response["description"]
                        .as_str()
                        .unwrap_or("未确认发送成功")
                        .replace(token, "********")
                        .replace(&api_base, "Telegram API")
                )
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod template_tests {
    use super::*;
    fn record(assets: Value) -> Value {
        json!({"asset_details_json":assets,"genres_json":["动作","家庭"]})
    }
    #[test]
    fn library_template_renders_title_link_known_asset_and_mp_details() {
        let item = json!({"Id":"101","Type":"Movie","Name":"奈克瑟斯","ProductionYear":2005,"ProviderIds":{"Tmdb":"1284316"}});
        let records = [record(
            json!([{"emby_item_id":"101","path":"/media/Film.2005.720p.WEB-DL.H264.AC3.2.0-QH.mkv","size_bytes":1073741824}]),
        )];
        let text = media_caption(&item, &["101".into()], &records, &[], &BTreeMap::new()).unwrap();
        assert!(
            text.starts_with(
                "📂 已入库1个文件\n[奈克瑟斯 \\(2005\\)](https://www.themoviedb.org/movie/1284316)"
            ),
            "{text}"
        );
        assert!(text.contains("🎭 类别：动作、家庭"));
        assert!(text.contains("👥 小组：QH"));
        assert!(text.contains("🌟 质量： MKV WEB\\-DL 720p H264 AC3 2\\.0"));
        assert!(text.ends_with("💾 大小：1\\.0G"));
        assert!(!text.contains("暂无剧情") && !text.contains("时间：") && !text.contains("评分"));
        let cached_audio = [record(
            json!([{"emby_item_id":"101","path":"/media/Film.mkv","size_bytes":1024,"quality_display":"WEB-DL","codec_display":"H.264","audio_tracks":[{"codec":"ac3","is_default":true}]}]),
        )];
        let text =
            media_caption(&item, &["101".into()], &cached_audio, &[], &BTreeMap::new()).unwrap();
        assert!(
            text.contains("H\\.264 AC3"),
            "known cached audio is retained without a filename token: {text}"
        );
    }
    #[test]
    fn library_template_counts_only_unique_admitted_ids_and_rejects_partial_totals() {
        let item =
            json!({"Id":"series","Type":"Series","Name":"盖亚奥特曼","Overview":"S01E01-E99"});
        let records = [record(json!([
            {"emby_item_id":"ep1","path":"/media/1.mkv","size_bytes":1024},
            {"emby_item_id":"old","path":"/media/old.mkv","size_bytes":1073741824000u64}
        ]))];
        let text = media_caption(
            &item,
            &["ep1".into(), "ep1".into(), "ep2".into()],
            &records,
            &[(1, 1), (1, 2)],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(text.starts_with("📂 已入库2个文件\n盖亚奥特曼"));
        assert!(text.contains("📺 集数：S01E01\\-E02"));
        assert!(text.ends_with("💾 大小：未知"));
        let empty = media_caption(&item, &[], &records, &[], &BTreeMap::new()).unwrap();
        assert!(empty.starts_with("📂 已入库0个文件"));
        assert!(
            !empty.contains("📺 集数："),
            "overview does not prove an admitted episode scope"
        );
    }
    #[test]
    fn library_template_does_not_publish_strm_shell_or_pending_fingerprint_sizes() {
        let mut item = json!({"Id":"101","Type":"Movie","Name":"电影","Path":"/media/film.strm","Size":50,"MediaSources":[{"ItemId":"101","Path":"/cd2/film.mkv","Size":1048576,"Container":"mkv"}]});
        let records = [record(
            json!([{"emby_item_id":"101","path":"/media/film.strm","container":"mkv","size_bytes":50}]),
        )];
        let text = media_caption(&item, &["101".into()], &records, &[], &BTreeMap::new()).unwrap();
        assert!(
            text.ends_with("💾 大小：1\\.0M"),
            "actual owned native source, never item.Size: {text}"
        );
        item["MediaSources"] = json!([]);
        assert!(
            media_caption(&item, &["101".into()], &records, &[], &BTreeMap::new())
                .unwrap()
                .ends_with("💾 大小：未知")
        );
        let pending = [record(
            json!([{"emby_item_id":"101","path":"/media/film.mkv","size_bytes":1048576,"fingerprint_pending":true}]),
        )];
        assert!(
            media_caption(&item, &["101".into()], &pending, &[], &BTreeMap::new())
                .unwrap()
                .ends_with("💾 大小：未知")
        );
        let hash = "a".repeat(40);
        let original = json!({"emby_item_ids_json":["ep1"],"file_sha1_json":[hash],"asset_details_json":[{"emby_item_id":"ep1","path":"/media/ep1.strm","size_bytes":50,"container":"strm"}]});
        let bound = bound_sha(std::slice::from_ref(&original), "ep1").unwrap();
        let infos = [
            json!({"sha1":hash,"info":[{"MediaSourceInfo":{"Size":1073741824,"Container":"matroska","MediaStreams":[{"Type":"Video","Codec":"H264"}]}}]}),
        ];
        let files = [json!({"sha1":hash,"size":1073741824,"name":"Original.mkv"})];
        let source = verified_cache_source(&bound, &infos, &files).unwrap();
        assert_eq!(source_format(&source), "MKV");
        let sources = BTreeMap::from([("ep1".into(), source)]);
        let text = media_caption(
            &json!({"Id":"series","Type":"Series","Name":"剧集"}),
            &["ep1".into()],
            std::slice::from_ref(&original),
            &[(1, 1)],
            &sources,
        )
        .unwrap();
        assert!(text.ends_with("💾 大小：1\\.0G"));
        assert!(text.contains("MKV") && !text.contains("STRM"));
        let mut incorrect = infos[0].clone();
        incorrect["info"][0]["MediaSourceInfo"]["Size"] = json!(50);
        assert_eq!(
            verified_cache_source(&bound, &[incorrect], &files).unwrap()["Size"],
            1073741824
        );
        for field in ["file_sha1_json", "asset_details_json"] {
            let mut misaligned = original.clone();
            misaligned[field] = json!([]);
            assert!(bound_sha(&[misaligned], "ep1").is_none());
        }
        let mut stale = original.clone();
        stale["asset_details_json"][0]["fingerprint_pending"] = json!(true);
        assert!(bound_sha(&[stale], "ep1").is_none());
        let mut wrong_owner = original;
        wrong_owner["asset_details_json"][0]["emby_item_id"] = json!("other");
        assert!(bound_sha(&[wrong_owner], "ep1").is_none());
        assert_eq!(
            source_format(&json!({"Container":"mov,mp4,m4a,3gp,3g2,mj2"})),
            "MP4"
        );
        assert!(source_format(&json!({"Container":"strm"})).is_empty());
        let shell = json!({"Id":"101","Type":"Movie","Name":"电影","Path":"/media/film.strm","Size":50,"MediaSources":[{"ItemId":"101","Path":"/media/film.strm","Size":50,"Container":"mkv"}]});
        let shell_records = [record(
            json!([{"emby_item_id":"101","path":"/media/film.strm","size_bytes":50,"container":"mkv"}]),
        )];
        let sanitized = native_source(&shell).unwrap();
        assert!(sanitized["Size"].is_null());
        for sources in [BTreeMap::new(), BTreeMap::from([("101".into(), sanitized)])] {
            let text =
                media_caption(&shell, &["101".into()], &shell_records, &[], &sources).unwrap();
            assert!(text.ends_with("💾 大小：未知") && !text.contains("50B"));
        }
        for path in ["/download.php", "/download.ashx", "/stream.m3u8"] {
            assert_eq!(
                source_format(&json!({"Path":path,"Container":"matroska"})),
                "MKV"
            );
            assert!(source_format(&json!({"Path":path})).is_empty());
        }
        assert_eq!(
            source_format(&json!({"Path":"/stream.m3u8","Container":"mpegts"})),
            "TS"
        );
        assert_eq!(
            source_format(&json!({"Path":"/renamed.mp4","Container":"matroska"})),
            "MKV"
        );
        for extension in ["m4v", "3gp", "f4v", "tp"] {
            let fallback =
                [json!({"sha1":bound,"name":format!("Original.{extension}"),"size":1073741824})];
            let source = verified_cache_source(&bound, &[], &fallback).unwrap();
            assert_eq!(source_format(&source), extension.to_uppercase());
            assert_eq!(source["Size"], 1073741824);
        }
        let declared_shell = json!({"Id":"101","Type":"Movie","Name":"电影","Path":"/media/film.strm","MediaSources":[{"ItemId":"101","Path":"/cd2/film.mkv","Size":50,"Container":"strm","MediaStreams":[{"Type":"Video","Codec":"H264"},{"Type":"Audio","Codec":"AAC"}]}]});
        assert!(source_format(&declared_shell["MediaSources"][0]).is_empty());
        assert!(native_size(&declared_shell, &declared_shell["MediaSources"][0]).is_none());
        assert!(native_source(&declared_shell).is_none());
        let text = media_caption(
            &declared_shell,
            &["101".into()],
            &shell_records,
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(text.ends_with("💾 大小：未知") && !text.contains("50B"));
        assert!(
            !text.contains("🌟 质量")
                && !text.contains("MKV")
                && !text.contains("H264")
                && !text.contains("AAC"),
            "a rejected STRM source must not reenter through the quality fallback: {text}"
        );
        let mapped_shell_records = [record(
            json!([{"emby_item_id":"101","path":"/cd2/film.mkv","size_bytes":50,"container":".StRm"}]),
        )];
        let text = media_caption(
            &declared_shell,
            &["101".into()],
            &mapped_shell_records,
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            text.ends_with("💾 大小：未知") && !text.contains("50B"),
            "mapping a shell path must not make its size trustworthy: {text}"
        );
        assert!(!text.contains("🌟 质量"), "{text}");
        let mut mapped_alias_records = mapped_shell_records;
        mapped_alias_records[0]["asset_details_json"][0]["container"] = json!("mkv");
        let text = media_caption(
            &declared_shell,
            &["101".into()],
            &mapped_alias_records,
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            text.ends_with("💾 大小：未知") && !text.contains("50B") && !text.contains("🌟 质量"),
            "a mapped asset must not bypass the native item's rejected STRM source: {text}"
        );
        let declared_cache = [
            json!({"sha1":bound,"info":[{"MediaSourceInfo":{"Size":50,"Path":"/cd2/film.mkv","Container":"strm","MediaStreams":[{"Type":"Video","Codec":"H264"}]}}]}),
        ];
        assert!(verified_cache_source(&bound, &declared_cache, &[]).is_none());
    }
    #[test]
    fn library_template_uses_verified_source_name_and_streams_without_shell_metadata() {
        let hash = "a".repeat(40);
        let item = json!({"Id":"series","Type":"Series","Name":"剧集"});
        let record = json!({"emby_item_ids_json":["ep1"],"file_sha1_json":[hash],"asset_details_json":[
            {"emby_item_id":"ep1","path":"/media/Film.720p.WEB-DL.H264.AC3-OLD.strm","container":"strm","size_bytes":50,
             "quality_display":"WEB-DL","resolution_display":"720p","codec_display":"H.264","audio_tracks":[{"codec":"AC3","is_default":true}]}
        ]});
        let hash = bound_sha(std::slice::from_ref(&record), "ep1").unwrap();
        let info = json!({"sha1":hash,"info":[{"MediaSourceInfo":{"Size":1073741824,"Container":"matroska","MediaStreams":[
            {"Type":"Video","Codec":"HEVC","Width":3840,"Height":2160},
            {"Type":"Audio","Codec":"AAC","IsDefault":true}
        ]}}]});
        let file =
            json!({"sha1":hash,"name":"Film.2160p.BLURAY.H265.AAC-REAL.mkv","size":1073741824});
        let source = verified_cache_source(
            &hash,
            std::slice::from_ref(&info),
            std::slice::from_ref(&file),
        )
        .unwrap();
        assert_eq!(source["Path"], file["name"]);
        let text = media_caption(
            &item,
            &["ep1".into()],
            std::slice::from_ref(&record),
            &[(1, 1)],
            &BTreeMap::from([("ep1".into(), source)]),
        )
        .unwrap();
        assert!(text.ends_with("💾 大小：1\\.0G"), "{text}");
        assert!(
            text.contains("MKV")
                && text.contains("2160p")
                && text.contains("HEVC")
                && text.contains("AAC"),
            "{text}"
        );
        assert!(text.contains("REAL") && !text.contains("OLD"), "{text}");
        for stale in ["WEB", "720p", "H264", "H\\.264", "AC3", "STRM", "50B"] {
            assert!(
                !text.contains(stale),
                "shell metadata {stale} leaked into source details: {text}"
            );
        }
        let mut duplicate = file.clone();
        duplicate["name"] = json!("Alias.1080p.WEB-DL.H264.AC3-OTHER.mkv");
        let source = verified_cache_source(&hash, &[info], &[file, duplicate]).unwrap();
        assert!(
            source["Path"].as_str().is_none_or(str::is_empty),
            "an ambiguous same-SHA filename must not be guessed: {source}"
        );
        let text = media_caption(
            &item,
            &["ep1".into()],
            &[record],
            &[(1, 1)],
            &BTreeMap::from([("ep1".into(), source)]),
        )
        .unwrap();
        assert!(
            text.contains("MKV") && text.contains("HEVC") && text.contains("AAC"),
            "{text}"
        );
        assert!(
            !text.contains("小组") && !text.contains("WEB") && !text.contains("AC3"),
            "{text}"
        );
    }
    #[test]
    fn library_template_escapes_values_and_uses_plain_title_without_a_verified_tmdb_link() {
        let item = json!({"Id":"101","Type":"Movie","Name":"A [B](C) \\ *D*","ProviderIds":{"Tmdb":"42?token=hidden"}});
        let text = media_caption(&item, &["101".into()], &[], &[], &BTreeMap::new()).unwrap();
        assert!(text.contains("A \\[B\\]\\(C\\) \\\\ \\*D\\*"), "{text}");
        assert!(!text.contains("themoviedb") && !text.contains("hidden"));
        assert!(!text.contains("类别：") && !text.contains("小组：") && !text.contains("质量："));
        for tmdb in ["0".to_string(), "+42".to_string(), "9".repeat(5000)] {
            let mut invalid = item.clone();
            invalid["ProviderIds"]["Tmdb"] = json!(tmdb);
            let text =
                media_caption(&invalid, &["101".into()], &[], &[], &BTreeMap::new()).unwrap();
            assert!(!text.contains("themoviedb"));
            assert!(text.encode_utf16().count() < 4096);
        }
        let mut leading_zero = item.clone();
        leading_zero["ProviderIds"]["Tmdb"] = json!(format!("{}42", "0".repeat(5000)));
        let text =
            media_caption(&leading_zero, &["101".into()], &[], &[], &BTreeMap::new()).unwrap();
        assert!(text.encode_utf16().count() < 4096);
        if text.contains("themoviedb") {
            assert!(text.contains("](https://www.themoviedb.org/movie/42)"));
        }
    }
    #[test]
    fn library_template_bounds_each_raw_value_without_splitting_markdown_escapes() {
        for raw in ["_*\\[".repeat(1200), "😀".repeat(1200)] {
            let item = json!({"Id":"101","Type":"Movie","Name":raw,"Genres":[raw],"ProviderIds":{"Tmdb":"42"}});
            let records = [record(
                json!([{"emby_item_id":"101","path":"/media/file.mkv","quality_display":raw,"resolution_display":raw,"size_bytes":1024}]),
            )];
            let text =
                media_caption(&item, &["101".into()], &records, &[], &BTreeMap::new()).unwrap();
            assert!(
                text.encode_utf16().count() > 1000,
                "fixture must exercise photo fallback"
            );
            assert!(text.encode_utf16().count() < 4096);
            assert!(text.ends_with("💾 大小：1\\.0K"));
            assert!(text.contains("](https://www.themoviedb.org/movie/42)"));
        }
        let pending = (1..=50).map(|episode| json!({"season":1,"episode":episode,"type":"Subtitle","ordinal":10,"language":"😀".repeat(80),"reason":"😀".repeat(300)})).collect::<Vec<_>>();
        let pages = title_pages(&"😀".repeat(5000), &json!({"pending":pending}));
        assert!(pages.len() > 1);
        assert!(pages.iter().all(|page| page.encode_utf16().count() <= 3500));
        for episode in 1..=50 {
            assert!(
                pages
                    .iter()
                    .any(|page| page.contains(&format!("S01E{episode:02}")))
            );
        }
    }
}
