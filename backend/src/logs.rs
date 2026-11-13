use crate::{
    AppState,
    error::{ApiError, Result},
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::get,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing_subscriber::{
    fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter, format::Writer},
    registry::LookupSpan,
};

pub struct LogFormat;
#[derive(Default)]
struct LogFields {
    message: String,
    details: Vec<String>,
    level_override: Option<String>,
}
impl LogFields {
    fn push(&mut self, field: &tracing::field::Field, value: String) {
        if field.name() == "python_level" {
            self.level_override = Some(value);
            return;
        }
        if field.name() == "message" {
            self.message = value;
            return;
        }
        let redacted = redact(&format!("{}={value}", field.name()));
        let value = redacted
            .split_once('=')
            .map_or(value.as_str(), |(_, value)| value);
        let label = match field.name() {
            "task_id" => "任务ID",
            "item_id" | "original_id" => "Emby ID",
            "refresh_id" => "刷新ID",
            "tmdb_id" => "TMDb ID",
            "douban_id" => "豆瓣ID",
            "user_id" => "用户ID",
            "name" => "名称",
            "kind" => "类型",
            "error" => "错误",
            "reason" => "原因",
            "file" | "path" => "路径",
            "root" => "根目录",
            "target" => "目标",
            "query" => "关键词",
            "people" | "actors" => "人物数",
            "versions" => "版本数",
            "added" => "新增人数",
            "photos" => "头像数",
            "watched" => "监控目录数",
            "targets" => "目标数",
            "restored" => "已还原",
            "skipped_tracks" => "跳过轨道数",
            "written" => "NFO写入",
            "count" | "total" | "total_items" | "items" => "数量",
            "requested" => "待处理",
            "translated" => "已翻译",
            "failures" => "失败数",
            "matched" => "匹配数",
            "libraries" => "媒体库数",
            "library_id" => "媒体库ID",
            "container" => "容器",
            "provider" => "来源",
            "result" => "结果",
            "progress" => "进度",
            "phase" => "阶段",
            "state" => "状态",
            "summary" => "查询摘要",
            "db.statement" => "SQL",
            "rows_affected" => "影响行数",
            "rows_returned" => "返回行数",
            "elapsed" => "耗时",
            "elapsed_secs" => return,
            "slow_threshold" => "告警阈值",
            name => name,
        };
        self.details
            .push(format!("{label}：{}", value.replace(['\r', '\n'], " ")));
    }
    fn body(mut self) -> String {
        if self.message == "slow statement: execution time exceeded alert threshold" {
            self.message = "数据库查询较慢，超过告警阈值".into();
        }
        if !self.details.is_empty() {
            self.message.push_str(" · ");
            self.message.push_str(&self.details.join("，"));
        }
        self.message
    }
}
impl tracing::field::Visit for LogFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let value = format!("{value:?}");
        self.push(
            field,
            serde_json::from_str::<String>(&value).unwrap_or(value),
        );
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.push(field, value.into());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.push(field, if value { "是" } else { "否" }.into());
    }
}
impl<S, N> FormatEvent<S, N> for LogFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let metadata = event.metadata();
        let mut fields = LogFields::default();
        event.record(&mut fields);
        let level = if fields.level_override.as_deref() == Some("CRITICAL") {
            "CRITICAL"
        } else if *metadata.level() == tracing::Level::WARN {
            "WARNING"
        } else {
            metadata.level().as_str()
        };
        let source = metadata
            .file()
            .and_then(|file| Path::new(file).file_name())
            .and_then(|file| file.to_str())
            .unwrap_or(metadata.target());
        write!(
            writer,
            "{level}: {} {source} - ",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S,%3f"),
        )?;
        let body = redact(&fields.body());
        if *metadata.level() <= tracing::Level::INFO
            && metadata.target().starts_with("emby_toolkit::")
            && metadata.target() != "emby_toolkit::jobs"
        {
            crate::jobs::record_activity(&body);
        }
        writeln!(writer, "{body}")
    }
}

pub fn redact(text: &str) -> String {
    static PATTERNS: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
        [
        r"(?i)(/bot)\d{6,}:[A-Za-z0-9_-]{20,}",
        r"(?i)\b\d{6,}:[A-Za-z0-9_-]{20,}\b",
        r#"(?i)((?:api_key|apikey|access_token|refresh_token|token|password|passwd|secret|cookie)=)[^&\s]+"#,
        r#"(?i)((?:X-Emby-Token|emby_api_key|tmdb_api_key|ai_api_key|telegram_bot_token|emby_admin_pass|db_password|douban_cookie|emby_webhook_secret|access_token|refresh_token|cookie|token|password)["']?\s*[:=]\s*["']?)[^"',\s}]+"#,
        r#"(?i)(Authorization["']?\s*[:=]\s*["']?Bearer\s+)[^"',\s}]+"#,
    ].into_iter().map(|pattern| regex::Regex::new(pattern).expect("valid redaction pattern")).collect()
    });
    let mut output = text.to_string();
    for pattern in PATTERNS.iter() {
        output = pattern
            .replace_all(&output, |capture: &regex::Captures<'_>| {
                format!("{}********", capture.get(1).map_or("", |m| m.as_str()))
            })
            .into_owned();
    }
    output
}

struct LogInner {
    file: File,
    recent: VecDeque<String>,
    max_bytes: u64,
    backups: u64,
}
pub struct LogStore {
    path: PathBuf,
    inner: Mutex<LogInner>,
}
impl LogStore {
    pub fn open(directory: &Path) -> std::io::Result<Arc<Self>> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join("app.log");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Arc::new(Self {
            path,
            inner: Mutex::new(LogInner {
                file,
                recent: VecDeque::new(),
                max_bytes: 5 * 1024 * 1024,
                backups: 10,
            }),
        }))
    }
    fn append(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.max_bytes > 0
            && inner.backups > 0
            && inner.file.metadata()?.len() + bytes.len() as u64 > inner.max_bytes
        {
            for index in (1..inner.backups).rev() {
                let old = self.path.with_file_name(format!("app.log.{index}"));
                if old.exists() {
                    std::fs::rename(
                        old,
                        self.path.with_file_name(format!("app.log.{}", index + 1)),
                    )?;
                }
            }
            std::fs::rename(&self.path, self.path.with_file_name("app.log.1"))?;
            inner.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
        }
        inner.file.write_all(bytes)?;
        let text = String::from_utf8_lossy(bytes);
        if let Some(entry) = frontend_entry(&text) {
            inner.recent.push_back(entry);
            while inner.recent.len() > 1000 {
                inner.recent.pop_front();
            }
        }
        Ok(())
    }
    pub fn configure(&self, config: &Value) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.max_bytes = config["log_rotation_size_mb"]
            .as_u64()
            .unwrap_or(5)
            .saturating_mul(1024 * 1024);
        inner.backups = config["log_rotation_backup_count"].as_u64().unwrap_or(10);
    }
    pub fn recent(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recent
            .iter()
            .cloned()
            .collect()
    }
}
fn frontend_entry(text: &str) -> Option<String> {
    if !matches!(
        log_level(text),
        Some("INFO" | "WARNING" | "ERROR" | "CRITICAL")
    ) {
        return None;
    }
    Some(text.trim_end().to_string())
}
fn log_level(text: &str) -> Option<&str> {
    if let Some((level, _)) = text.split_once(':')
        && matches!(
            level,
            "TRACE" | "DEBUG" | "INFO" | "WARNING" | "ERROR" | "CRITICAL"
        )
    {
        return Some(level);
    }
    text.split(" - ").nth(2)
}
#[derive(Clone)]
pub struct LogWriter(pub Arc<LogStore>);
pub struct LogGuard {
    store: Arc<LogStore>,
    buffer: Vec<u8>,
}
impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = LogGuard;
    fn make_writer(&'a self) -> LogGuard {
        LogGuard {
            store: self.0.clone(),
            buffer: vec![],
        }
    }
}
impl Write for LogGuard {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Drop for LogGuard {
    fn drop(&mut self) {
        if !self.buffer.is_empty() {
            let redacted = redact(&String::from_utf8_lossy(&self.buffer));
            if log_level(&redacted) != Some("TRACE")
                && self.store.append(redacted.as_bytes()).is_err()
            {
                let _ = std::io::stderr().write_all(b"ETK: unable to write log file\n");
            }
            let _ = std::io::stdout().write_all(redacted.as_bytes());
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/logs/list", get(list))
        .route("/api/logs/view", get(view))
        .route("/api/logs/search", get(search))
        .route("/api/logs/search_context", get(context))
        .route("/api/logs/stream", get(stream))
        .route("/api/logs/download", get(download))
}
async fn checked_path(root: &Path, name: &str) -> Result<PathBuf> {
    let path = tokio::fs::canonicalize(root.join(name))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError(StatusCode::NOT_FOUND, "日志文件未找到".into())
            } else {
                error.into()
            }
        })?;
    if !path.starts_with(root) {
        return Err(ApiError(StatusCode::FORBIDDEN, "日志路径超出范围".into()));
    }
    Ok(path)
}

// Hold the open inode so a rotation cannot discard its unread tail.
struct LogTail {
    root: PathBuf,
    file: tokio::fs::File,
    offset: u64,
    anchor: Vec<u8>,
    pending: Vec<u8>,
    initial: Option<String>,
    failed: bool,
}
impl LogTail {
    async fn open(root: PathBuf) -> Result<Self> {
        let mut file = tokio::fs::File::open(checked_path(&root, "app.log").await?).await?;
        let length = file.metadata().await?.len();
        let start = length.saturating_sub(100 * 1024);
        file.seek(SeekFrom::Start(start)).await?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(length - start)
            .read_to_end(&mut bytes)
            .await?;
        let offset = start + bytes.len() as u64;
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |i| i + 1);
        let pending = bytes[complete..].to_vec();
        let first = if start > 0 {
            bytes[..complete]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(complete, |i| i + 1)
        } else {
            0
        };
        let lines = bytes[first..complete]
            .split_inclusive(|byte| *byte == b'\n')
            .collect::<Vec<_>>();
        let initial = if lines.is_empty() {
            None
        } else {
            Some(
                String::from_utf8_lossy(&lines[lines.len().saturating_sub(50)..].concat())
                    .strip_suffix('\n')
                    .unwrap_or("")
                    .to_string(),
            )
        };
        let mut tail = Self {
            root,
            file,
            offset,
            anchor: Vec::new(),
            pending,
            initial,
            failed: false,
        };
        tail.save_anchor().await?;
        Ok(tail)
    }
    async fn read_anchor(&mut self) -> std::io::Result<Vec<u8>> {
        let start = self.offset.saturating_sub(64);
        self.file.seek(SeekFrom::Start(start)).await?;
        let mut bytes = Vec::new();
        (&mut self.file)
            .take(self.offset - start)
            .read_to_end(&mut bytes)
            .await?;
        self.file.seek(SeekFrom::Start(self.offset)).await?;
        Ok(bytes)
    }
    async fn save_anchor(&mut self) -> std::io::Result<()> {
        self.anchor = self.read_anchor().await?;
        Ok(())
    }
    async fn next(&mut self) -> Result<Option<String>> {
        let metadata = self.file.metadata().await?;
        if metadata.len() < self.offset || self.read_anchor().await? != self.anchor {
            self.offset = 0;
            self.pending.clear();
            self.file.seek(SeekFrom::Start(0)).await?;
        }
        let before = self.pending.len();
        (&mut self.file)
            .take(1024 * 1024)
            .read_to_end(&mut self.pending)
            .await?;
        self.offset += (self.pending.len() - before) as u64;
        self.save_anchor().await?;
        if let Some(end) = self.pending.iter().rposition(|byte| *byte == b'\n') {
            let bytes = self.pending.drain(..=end).collect::<Vec<_>>();
            return Ok(Some(
                String::from_utf8_lossy(&bytes[..bytes.len() - 1]).to_string(),
            ));
        }
        let path = match checked_path(&self.root, "app.log").await {
            Ok(path) => path,
            Err(ApiError(StatusCode::NOT_FOUND, _)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let current = tokio::fs::metadata(&path).await?;
        if !same_file(&metadata, &current) && self.file.metadata().await?.len() == self.offset {
            let remainder = std::mem::take(&mut self.pending);
            self.file = tokio::fs::File::open(path).await?;
            self.offset = 0;
            self.anchor.clear();
            if !remainder.is_empty() {
                return Ok(Some(String::from_utf8_lossy(&remainder).to_string()));
            }
        }
        Ok(None)
    }
}
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.created().ok() == right.created().ok()
    }
}
async fn stream(State(state): State<AppState>) -> Result<Response> {
    let jobs = state.jobs.clone();
    if jobs.is_shutting_down().await {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "服务正在关闭，不能打开日志流".into(),
        ));
    }
    let root = tokio::fs::canonicalize(state.data_dir.join("logs")).await?;
    let tail = LogTail::open(root).await?;
    let events = futures_util::stream::unfold((tail, jobs), |(mut tail, jobs)| async move {
        if tail.failed || jobs.is_shutting_down().await {
            return None;
        }
        if let Some(text) = tail.initial.take() {
            return Some((
                Ok::<_, std::io::Error>(Event::default().data(text)),
                (tail, jobs),
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if jobs.is_shutting_down().await {
            return None;
        }
        let event = match tail.next().await {
            Ok(Some(text)) => Ok(Event::default().data(text)),
            Ok(None) => Ok(Event::default().comment("keepalive")),
            Err(_) => {
                tail.failed = true;
                Err(std::io::Error::other("日志流读取失败"))
            }
        };
        Some((event, (tail, jobs)))
    });
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Sse::new(events),
    )
        .into_response())
}
async fn download(State(state): State<AppState>) -> Result<Response> {
    let root = tokio::fs::canonicalize(state.data_dir.join("logs")).await?;
    let primary = checked_path(&root, "app.log").await?;
    let mut selected = vec![("app.log".to_string(), primary)];
    let mut backups = Vec::new();
    for name in files(&state).await? {
        if name
            .strip_prefix("app.log.")
            .is_some_and(|index| index.parse::<u64>().is_ok())
        {
            let path = checked_path(&root, &name).await?;
            let modified = tokio::fs::metadata(&path).await?.modified()?;
            backups.push((name, path, modified));
        }
    }
    backups.sort_by_key(|entry| std::cmp::Reverse(entry.2));
    selected.extend(
        backups
            .into_iter()
            .take(9)
            .map(|(name, path, _)| (name, path)),
    );
    let stem = format!("app-logs-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let directory = stem.clone();
    let bytes = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, path) in selected {
            archive.start_file(format!("{directory}/{name}"), options)?;
            std::io::copy(&mut File::open(path)?, &mut archive)?;
        }
        Ok(archive.finish()?.into_inner())
    })
    .await
    .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "日志归档失败".into()))??;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{stem}.zip\""),
            ),
        ],
        bytes,
    )
        .into_response())
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.starts_with("app.log")
        && Path::new(name).file_name().and_then(|s| s.to_str()) == Some(name)
        && !name.contains(['/', '\\'])
}
async fn files(state: &AppState) -> Result<Vec<String>> {
    let root = state.data_dir.join("logs");
    let mut result = vec![];
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if valid_name(&name) && entry.file_type().await?.is_file() {
            result.push(name);
        }
    }
    result.sort_by_key(|name| {
        if name == "app.log" {
            0
        } else {
            name.strip_prefix("app.log.")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(u64::MAX)
        }
    });
    Ok(result)
}
async fn read(state: &AppState, name: &str) -> Result<String> {
    if !valid_name(name) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "禁止访问非日志文件或无效文件名".into(),
        ));
    }
    let root = tokio::fs::canonicalize(state.data_dir.join("logs")).await?;
    let path = tokio::fs::canonicalize(root.join(name))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError(StatusCode::NOT_FOUND, "日志文件未找到".into())
            } else {
                error.into()
            }
        })?;
    if !path.starts_with(&root) {
        return Err(ApiError(StatusCode::FORBIDDEN, "日志路径超出范围".into()));
    }
    Ok(String::from_utf8_lossy(&tokio::fs::read(path).await?).to_string())
}
async fn list(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(json!(files(&state).await?)))
}
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn html(blocks: &[Value], query: &str) -> String {
    let mut output="<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><style>body{background:#1e1e1e;color:#d4d4d4;font:13px/1.6 monospace;padding:15px}pre{white-space:pre-wrap;overflow-wrap:anywhere}h3{color:#aaa}mark{background:#264f78;color:white}</style>".to_string();
    let pattern = if query.is_empty() {
        None
    } else {
        regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(true)
            .build()
            .ok()
    };
    for block in blocks {
        output.push_str(&format!(
            "<section><h3>{}</h3><pre>",
            escaped(block["file"].as_str().unwrap_or(""))
        ));
        for line in block["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(pattern) = &pattern {
                let mut cursor = 0;
                for matched in pattern.find_iter(line) {
                    output.push_str(&escaped(&line[cursor..matched.start()]));
                    output.push_str(&format!("<mark>{}</mark>", escaped(matched.as_str())));
                    cursor = matched.end();
                }
                output.push_str(&escaped(&line[cursor..]));
            } else {
                output.push_str(&escaped(line));
            }
            if !line.ends_with('\n') {
                output.push('\n');
            }
        }
        output.push_str("</pre></section>");
    }
    output.push_str("</html>");
    output
}
async fn view(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response> {
    let name = params.get("filename").map(String::as_str).unwrap_or("");
    let text = read(&state, name).await?;
    let lines: Vec<_> = text.split_inclusive('\n').rev().collect();
    let format = params.get("format").map(String::as_str).unwrap_or("");
    if format.eq_ignore_ascii_case("json") || format.eq_ignore_ascii_case("html") {
        let pages = lines.len().div_ceil(500).max(1);
        let page = params
            .get("page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, pages);
        let start = (page - 1) * 500;
        let page_lines = &lines[start..(start + 500).min(lines.len())];
        if format.eq_ignore_ascii_case("json") {
            return Ok(Json(json!({
                "file": name,
                "lines": page_lines,
                "page": page,
                "pages": pages,
                "total_lines": lines.len(),
            }))
            .into_response());
        }
        return Ok((
            [
                ("content-type", "text/html; charset=utf-8".to_string()),
                ("x-log-page", page.to_string()),
                ("x-log-pages", pages.to_string()),
                ("x-log-total-lines", lines.len().to_string()),
            ],
            html(&[json!({"file":name,"lines":page_lines})], ""),
        )
            .into_response());
    }
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        lines.join(""),
    )
        .into_response())
}
async fn search(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let query = params
        .get("q")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad("搜索关键词不能为空"))?
        .to_lowercase();
    let mut result = vec![];
    for name in files(&state).await? {
        let Ok(text) = read(&state, &name).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&query) {
                result.push(json!({"file":name,"line_num":index+1,"content":line.trim(),"date":timestamp(line).unwrap_or("")}));
            }
        }
    }
    result.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
    Ok(Json(json!(result)))
}
async fn context(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response> {
    let query = params
        .get("q")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad("搜索关键词不能为空"))?;
    let task_id =
        regex::Regex::new(r#"(?:\btask_id=["']?|任务ID[：:]\s*)([A-Za-z0-9_-]+)"#).unwrap();
    let mut task: Option<(String, String, Vec<String>)> = None;
    let start = regex::Regex::new(r"Webhook: 收到入库事件\s'(.+?)'，已分派预检任务。").unwrap();
    let end =
        regex::Regex::new(r"后台任务\s'Webhook入库:\s(.+?)'\s结束，最终状态:\s处理完成。").unwrap();
    let noise = regex::Regex::new(
        r"(?:Webhook: 收到入库事件|项目|预检.+?检测到|开始检查|开始处理|处理完成)\s'(.+?)'",
    )
    .unwrap();
    let mut blocks = vec![];
    for file in files(&state).await?.into_iter().rev() {
        let Ok(text) = read(&state, &file).await else {
            continue;
        };
        let mut active = None;
        let mut lines = Vec::<String>::new();
        for line in text.split_inclusive('\n') {
            if line.trim().is_empty() {
                continue;
            }
            let line_task = task_id.captures(line).map(|capture| capture[1].to_string());
            if line.contains("[任务开始]")
                && let Some(id) = &line_task
            {
                if let Some((file, _, lines)) = task.take() {
                    push_task_context(&mut blocks, &file, lines, query);
                }
                task = Some((file.clone(), id.clone(), vec![line.into()]));
            } else if let Some((_, id, lines)) = &mut task
                && line_task.as_ref().is_none_or(|line_task| line_task == id)
            {
                lines.push(line.into());
                if line.contains("[任务结束]") && line_task.as_ref() == Some(id) {
                    let (file, _, lines) = task.take().unwrap();
                    push_task_context(&mut blocks, &file, lines, query);
                }
            }
            if let Some(capture) = start.captures(line) {
                let name = capture[1].to_string();
                if name.to_lowercase().contains(&query.to_lowercase())
                    && (active.is_none() || active.as_ref() == Some(&name))
                {
                    active = Some(name);
                    lines = vec![line.into()];
                    continue;
                }
            }
            let Some(name) = &active else { continue };
            if let Some(capture) = end.captures(line)
                && &capture[1] == name
            {
                lines.push(line.into());
                blocks.push(
                        json!({"file":file,"date":timestamp(&lines[0]).unwrap_or("Unknown Date"),"lines":lines}),
                    );
                active = None;
                lines.clear();
                continue;
            }
            if noise
                .captures(line)
                .is_some_and(|capture| &capture[1] != name)
            {
                continue;
            }
            lines.push(line.into());
        }
    }
    if let Some((file, _, lines)) = task {
        push_task_context(&mut blocks, &file, lines, query);
    }
    blocks.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
    if params
        .get("format")
        .is_some_and(|s| s.eq_ignore_ascii_case("html"))
    {
        Ok((
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html(&blocks, query),
        )
            .into_response())
    } else {
        Ok(Json(json!(blocks)).into_response())
    }
}
fn push_task_context(blocks: &mut Vec<Value>, file: &str, lines: Vec<String>, query: &str) {
    let query = query.to_lowercase();
    if lines
        .iter()
        .any(|line| line.to_lowercase().contains(&query))
    {
        blocks.push(json!({"file":file,"date":timestamp(&lines[0]).unwrap_or("Unknown Date"),"lines":lines}));
    }
}
fn timestamp(line: &str) -> Option<&str> {
    static TIMESTAMP: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^(?:(?:TRACE|DEBUG|INFO|WARNING|ERROR|CRITICAL):\s*)?(\d{4}-\d{2}-\d{2}\s\d{2}:\d{2}:\d{2})").unwrap()
    });
    TIMESTAMP
        .captures(line)?
        .get(1)
        .map(|matched| matched.as_str())
}
