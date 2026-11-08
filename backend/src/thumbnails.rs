use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
    state::string,
};
use regex::Regex;
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
    time::Duration,
};

fn config_error(message: &str) -> ApiError {
    ApiError(axum::http::StatusCode::CONFLICT, message.into())
}
fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0)
}
fn chapters(item: &Value) -> Vec<Value> {
    list(&item["Chapters"])
        .into_iter()
        .filter(|c| matches!(c["MarkerType"].as_str(), None | Some("" | "Chapter")))
        .collect()
}
pub fn source(item: &Value) -> Result<Value> {
    crate::media_info::item_source(item, &[&string(item, "Path")])
        .cloned()
        .ok_or_else(|| ApiError::bad("无法唯一确定当前版本的媒体输入路径"))
}
pub fn repair_rule(item: &Value, probe: &Value, stderr: &str) -> Result<Option<Value>> {
    let source = source(item)?;
    let path = source["Path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| item["Path"].as_str())
        .unwrap_or("");
    if path.is_empty() || path.to_lowercase().ends_with(".strm") {
        return Err(ApiError::bad("缺少实际媒体输入路径"));
    }
    let duration = number(&probe["format"]["duration"]);
    if !duration.is_finite() || duration <= 0.0 {
        return Err(ApiError::bad("无法验证实际视频时长"));
    }
    if (number(&item["RunTimeTicks"]) / 10_000_000.0 - duration).abs() > 1.0 {
        return Err(ApiError::bad(
            "媒体信息时长与源文件不一致，请重新提取媒体信息后重试",
        ));
    }
    let video = list(&probe["streams"])
        .into_iter()
        .filter(|s| s["codec_type"] == "video" && number(&s["disposition"]["attached_pic"]) == 0.0)
        .collect::<Vec<_>>();
    let mut end = duration;
    if video.len() == 1 {
        let stream = &video[0];
        let parsed = stream["duration"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| stream["duration"].as_f64());
        let value = parsed
            .map(|d| d + number(&stream["start_time"]))
            .or_else(|| {
                static TIME: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(r"^(\d+):(\d{2}):(\d{2}(?:\.\d+)?)$").unwrap());
                let text = string(&stream["tags"], "DURATION");
                let c = TIME.captures(&text)?;
                Some(
                    c[1].parse::<f64>().ok()? * 3600.0
                        + c[2].parse::<f64>().ok()? * 60.0
                        + c[3].parse::<f64>().ok()?,
                )
            });
        if let Some(value) = value.filter(|v| v.is_finite() && *v > 0.0) {
            end = end.min(value);
        }
    }
    static EDIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"edit list \d+ - media time: -?\d+, duration: 0(?:\s|$)").unwrap()
    });
    let formats = string(&probe["format"], "format_name");
    let ignore = formats.split(',').any(|s| matches!(s, "mov" | "mp4")) && EDIT.is_match(stderr);
    let clamp = chapters(item).iter().any(|c| {
        string(c, "ImageTag").is_empty()
            && number(&c["StartPositionTicks"]) / 10_000_000.0 >= end - 0.1
    });
    Ok((ignore||clamp).then(||json!({"path":path.strip_prefix("file:").unwrap_or(path),"duration":end,"ignore_editlist":ignore,"clamp_end":clamp})))
}
fn quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}
pub fn encoder_script(encoder: &str, rules: &[Value]) -> String {
    let mut lines = vec![
        "#!/bin/sh".into(),
        "input=".into(),
        "previous=".into(),
        "for value; do".into(),
        "  if [ \"$previous\" = -i ]; then input=${value#file:}; fi".into(),
        "  previous=$value".into(),
        "done".into(),
        "ignore=0; clamp=0".into(),
        "case \"$input\" in".into(),
    ];
    for rule in rules {
        lines.push(format!(
            "  {}) ignore={}; threshold={:.6}; limit={:.6}; clamp={} ;;",
            quote(&string(rule, "path")),
            usize::from(rule["ignore_editlist"] == true),
            number(&rule["duration"]) - 0.1,
            (number(&rule["duration"]) - 1.0).max(0.0),
            usize::from(rule["clamp_end"] == true)
        ));
    }
    lines.extend(["esac","remaining=$#","while [ \"$remaining\" -gt 0 ]; do","  value=$1; shift; remaining=$((remaining - 1))","  if [ \"$value\" = -ss ] && [ \"$clamp\" = 1 ] && [ \"$remaining\" -gt 0 ]; then","    seek=$1; shift; remaining=$((remaining - 1))","    seek=$(awk -v seek=\"$seek\" -v threshold=\"$threshold\" -v limit=\"$limit\" 'BEGIN {n=split(seek,a,\":\"); t=0; for(i=1;i<=n;i++) t=t*60+a[i]; if(t>=threshold) printf \"%.6f\",limit; else printf \"%s\",seek}')","    set -- \"$@\" -ss \"$seek\"","  elif [ \"$value\" = -i ] && [ \"$ignore\" = 1 ]; then","    set -- \"$@\" -ignore_editlist 1 -i","  else set -- \"$@\" \"$value\"; fi","done"].map(str::to_string));
    lines.push(format!("exec {} \"$@\"", quote(encoder)));
    lines.push(String::new());
    lines.join("\n")
}
#[derive(Default)]
pub struct ExtractionLog {
    started: bool,
    complete: bool,
    total: Option<usize>,
    processed: BTreeSet<usize>,
    pub paths: BTreeSet<String>,
    pub encoders: BTreeSet<String>,
    last_activity: Option<String>,
    failed: usize,
    skipped: usize,
}
impl ExtractionLog {
    pub fn feed(&mut self, lines: &[String]) {
        static COUNT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"VideoThumbnailExtract - Number of items: (\d+)").unwrap()
        });
        static PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"VideoThumbnailExtract - Progress (\d+)/(\d+) - Task \d+: (.+)").unwrap()
        });
        static SKIPPED: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"VideoThumbnailExtract - Number of items skipped: (\d+)").unwrap()
        });
        static COMMAND: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"ProcessRun '[^']+' Execute:\s*(?:"([^"]+)"|(\S+))"#).unwrap()
        });
        for line in lines {
            if self.complete {
                break;
            }
            if line.contains("VideoThumbnailExtract - Scheduled Task Execute") {
                self.started = true;
            }
            if !self.started {
                continue;
            }
            if let Some(c) = COUNT.captures(line) {
                self.total = c[1].parse().ok();
            }
            if let Some(c) = PROGRESS.captures(line) {
                self.last_activity = Some(format!(
                    "正在提取视频 {}/{}：{}",
                    &c[1],
                    &c[2],
                    crate::media_paths::clean(c[3].trim())
                ));
                if let Ok(n) = c[1].parse() {
                    self.processed.insert(n);
                }
                self.paths.insert(c[3].trim().into());
            }
            if line.contains("VideoThumbnailExtract - Item failed:")
                || line.contains("VideoThumbnailExtract - Item cancelled:")
            {
                self.failed += 1;
            }
            if let Some(c) = SKIPPED.captures(line) {
                self.skipped = c[1].parse().unwrap_or(0);
            }
            if let Some(c) = COMMAND.captures(line) {
                self.encoders
                    .insert(c.get(1).or_else(|| c.get(2)).unwrap().as_str().into());
            }
            if line.contains("VideoThumbnailExtract - Scheduled Task Complete") {
                self.complete = true;
            }
        }
    }
    pub fn result(&self) -> Result<BTreeSet<String>> {
        if !self.complete
            || self.total.is_none()
            || self.processed != (1..=self.total.unwrap_or(0)).collect()
        {
            return Err(ApiError::bad(
                "本次神医提取日志不完整，无法确认失败范围，请重试",
            ));
        }
        Ok(self.paths.clone())
    }
}
async fn api(
    state: &AppState,
    method: Method,
    path: &str,
    query: &[(&str, String)],
) -> Result<Value> {
    state.emby(method, path, query, None).await
}
struct LogCursor {
    anchor: Option<String>,
}
impl LogCursor {
    async fn lines(state: &AppState, name: &str, limit: usize) -> Result<Vec<String>> {
        let name = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>();
        let result = api(
            state,
            Method::GET,
            &format!("System/Logs/{name}/Lines"),
            &[("Limit", limit.to_string())],
        )
        .await?;
        result["Items"]
            .as_array()
            .ok_or_else(|| ApiError::bad("Emby日志格式无效"))
            .map(|v| {
                v.iter()
                    .filter_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
                    .collect()
            })
    }
    fn anchor(lines: &[String]) -> Option<String> {
        static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\d ").unwrap());
        lines.iter().find(|s| DATE.is_match(s)).cloned()
    }
    async fn new(state: &AppState) -> Result<Self> {
        Ok(Self {
            anchor: Self::anchor(&Self::lines(state, "embyserver.txt", 1000).await?),
        })
    }
    async fn since(
        &self,
        state: &AppState,
        name: &str,
    ) -> Result<(Option<Vec<String>>, Vec<String>)> {
        let mut lines = vec![];
        for limit in [1000, 10000, 50000] {
            lines = Self::lines(state, name, limit).await?;
            if let Some(index) = lines.iter().position(|s| Some(s) == self.anchor.as_ref()) {
                return Ok((Some(lines[..index].to_vec()), lines));
            }
            if lines.len() < limit {
                break;
            }
        }
        Ok((None, lines))
    }
    async fn read(&mut self, state: &AppState) -> Result<Vec<String>> {
        let (mut added, latest) = self.since(state, "embyserver.txt").await?;
        if self.anchor.is_none() {
            added = Some(latest.clone());
        }
        if added.is_none() {
            let files = api(
                state,
                Method::GET,
                "System/Logs/Query",
                &[("Limit", "20".into())],
            )
            .await?;
            let mut combined = latest.clone();
            for entry in list(&files["Items"]) {
                let name = string(&entry, "Name");
                if !name.starts_with("embyserver-") {
                    continue;
                }
                let (segment, archive) = self.since(state, &name).await?;
                if let Some(segment) = segment {
                    combined.extend(segment);
                    added = Some(combined);
                    break;
                }
                combined.extend(archive);
            }
        }
        let mut added = added.ok_or_else(|| {
            ApiError::bad("无法完整接续本次Emby日志，已停止自动修复，避免混入历史任务")
        })?;
        self.anchor = Self::anchor(&latest).or(self.anchor.clone());
        added.reverse();
        Ok(added)
    }
}
async fn task(state: &AppState) -> Result<Value> {
    let tasks = api(state, Method::GET, "ScheduledTasks", &[]).await?;
    list(&tasks)
        .into_iter()
        .find(|t| t["Key"] == "VideoThumbnailExtractTask" && !string(t, "Id").is_empty())
        .ok_or_else(|| ApiError::bad("未找到神医提取视频预览缩略图计划任务"))
}
fn active(task: &Value) -> bool {
    matches!(task["State"].as_str(), Some("Running" | "Cancelling"))
}
async fn run_native(
    state: &AppState,
    timeout: Duration,
    start: u64,
    end: u64,
    expected: Option<&str>,
    context: &JobContext,
) -> Result<BTreeSet<String>> {
    let mut trace = ExtractionLog::default();
    run_with_trace(state, timeout, start, end, expected, context, &mut trace).await
}
async fn run_with_trace(
    state: &AppState,
    timeout: Duration,
    start: u64,
    end: u64,
    expected: Option<&str>,
    context: &JobContext,
    trace: &mut ExtractionLog,
) -> Result<BTreeSet<String>> {
    let mut current = task(state).await?;
    let deadline = tokio::time::Instant::now() + timeout;
    while active(&current) {
        context.checkpoint().await?;
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::bad("等待已有神医提取任务结束超时"));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        current = task(state).await?;
    }
    if current["State"] != "Idle" {
        return Err(config_error("神医缩略图任务状态未确认空闲，未提交重复提取"));
    }
    let previous = current["LastExecutionResult"]["StartTimeUtc"].clone();
    let mut cursor = LogCursor::new(state).await?;
    let id = string(&current, "Id");
    context.checkpoint().await?;
    api(
        state,
        Method::POST,
        &format!("ScheduledTasks/Running/{id}"),
        &[],
    )
    .await?;
    let mut deadline = tokio::time::Instant::now() + timeout;
    let mut log_deadline = None;
    let mut stopped = false;
    let outcome = async {
        while tokio::time::Instant::now() < deadline {
            if context.is_cancelled() && !stopped {
                api(
                    state,
                    Method::DELETE,
                    &format!("ScheduledTasks/Running/{id}"),
                    &[],
                )
                .await?;
                stopped = true;
                deadline = deadline.min(tokio::time::Instant::now() + Duration::from_secs(90));
            }
            let poll = async {
                current = task(state).await?;
                trace.feed(&cursor.read(state).await?);
                Ok::<_, ApiError>(())
            };
            let polled = tokio::select! {
                _=context.cancelled(),if !stopped=>continue,
                result=tokio::time::timeout_at(deadline,poll)=>result,
            };
            polled.map_err(|_| ApiError::bad("读取本次神医缩略图任务状态或日志超时"))??;
            let result = &current["LastExecutionResult"];
            if current["State"] == "Idle"
                && !result["StartTimeUtc"].is_null()
                && result["StartTimeUtc"] != previous
            {
                if stopped {
                    return Err(config_error("神医提取已停止"));
                }
                if result["Status"] != "Completed" {
                    return Err(ApiError::bad(format!(
                        "神医提取未正常完成：{}",
                        string(result, "Status")
                    )));
                }
                if trace.complete {
                    let paths = trace.result()?;
                    if expected
                        .is_some_and(|e| !trace.encoders.is_empty() && !trace.encoders.contains(e))
                    {
                        return Err(config_error("神医没有实际调用兼容编码器，已停止补图"));
                    }
                    return Ok(paths);
                }
                let deadline = log_deadline
                    .get_or_insert_with(|| tokio::time::Instant::now() + Duration::from_secs(10));
                if tokio::time::Instant::now() >= *deadline {
                    return Err(ApiError::bad("神医任务已结束，但未读到本次完整提取日志"));
                }
            }
            if let Some(progress) = current["CurrentProgressPercentage"]
                .as_f64()
                .filter(|n| n.is_finite())
            {
                context
                    .update(
                        start + ((end - start) as f64 * progress.clamp(0.0, 100.0) / 100.0) as u64,
                        "神医提取",
                        &trace
                            .last_activity
                            .clone()
                            .unwrap_or_else(|| format!("神医提取进度 {progress:.1}%")),
                    )
                    .await;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(ApiError::bad("等待神医提取结束超时，请检查Emby任务状态"))
    }
    .await;
    if outcome.is_err() && !stopped {
        let _ = api(
            state,
            Method::DELETE,
            &format!("ScheduledTasks/Running/{id}"),
            &[],
        )
        .await;
    }
    if outcome.is_err() {
        wait_native_idle(state, &id).await?;
    }
    outcome
}
async fn wait_native_idle(state: &AppState, id: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let result =
            tokio::time::timeout_at(deadline, api(state, Method::GET, "ScheduledTasks", &[])).await;
        if let Ok(Ok(tasks)) = result
            && list(&tasks)
                .iter()
                .any(|task| task["Id"] == id && task["State"] == "Idle")
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(config_error(
                "未确认神医缩略图任务结束，保留兼容编码器和文件，请检查原生任务状态",
            ));
        }
        tokio::time::sleep(
            Duration::from_secs(2)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
    }
}
async fn item(state: &AppState, id: &str) -> Result<Value> {
    let data = api(
        state,
        Method::GET,
        "Items",
        &[
            ("Ids", id.into()),
            (
                "Fields",
                "Name,Path,Chapters,MediaSources,Tags,ParentId,Type,SeriesName,SeriesId,ParentIndexNumber,IndexNumber".into(),
            ),
        ],
    )
    .await?;
    list(&data["Items"])
        .into_iter()
        .find(|item| item["Id"] == id)
        .ok_or_else(|| ApiError::bad("未返回请求媒体 ID 的数据，无法验收原目标"))
}
async fn missing(state: &AppState, item: &Value, options: &Value) -> Result<bool> {
    if number(&options["ThumbnailImagesIntervalSeconds"]) > 0.0 {
        let data = api(
            state,
            Method::GET,
            &format!("Items/{}/ThumbnailSet", string(item, "Id")),
            &[("Width", "320".into())],
        )
        .await?;
        let thumbnails = data["Thumbnails"]
            .as_array()
            .ok_or_else(|| ApiError::bad("Emby 未返回有效的缩略图集合，无法确认 BIF 状态"))?;
        return Ok(thumbnails.is_empty());
    }
    let chapters = chapters(item);
    Ok(chapters.is_empty() || chapters.iter().any(|c| string(c, "ImageTag").is_empty()))
}
type Targets = BTreeMap<String, (Value, Value)>;
pub(crate) struct NativeExtraction {
    pub report: Value,
    paths: BTreeSet<String>,
    targets: Targets,
}

fn ordered_targets<'a>(
    targets: &'a Targets,
    order: &std::collections::HashMap<String, crate::media_order::Key>,
) -> Vec<(&'a String, &'a (Value, Value))> {
    let mut items = targets.iter().collect::<Vec<_>>();
    items.sort_by_cached_key(|(id, (item, _))| {
        order
            .get(*id)
            .cloned()
            .unwrap_or_else(|| crate::media_order::key(item))
    });
    items
}
async fn targets(
    state: &AppState,
    paths: &BTreeSet<String>,
    context: &JobContext,
) -> Result<Targets> {
    let mut targets = BTreeMap::new();
    if paths.is_empty() {
        return Ok(targets);
    }
    let libraries = api(state, Method::GET, "Library/VirtualFolders", &[]).await?;
    for path in paths {
        context.checkpoint().await?;
        let items = api(
            state,
            Method::GET,
            "Items",
            &[
                ("Path", path.clone()),
                ("Recursive", "true".into()),
                (
                    "Fields",
                    "Name,Path,Chapters,MediaSources,Tags,ParentId,Type,SeriesName,SeriesId,ParentIndexNumber,IndexNumber".into(),
                ),
            ],
        )
        .await?;
        let library = list(&libraries)
            .into_iter()
            .flat_map(|l| {
                list(&l["Locations"]).into_iter().filter_map(move |root| {
                    root.as_str()
                        .filter(|root| {
                            path.starts_with(&format!("{}/", root.trim_end_matches('/')))
                        })
                        .map(|root| (root.len(), l.clone()))
                })
            })
            .max_by_key(|(n, _)| *n);
        let items = list(&items["Items"])
            .into_iter()
            .filter(|i| i["Path"] == *path && i["IsFolder"] != true)
            .collect::<Vec<_>>();
        let library = library
            .filter(|_| !items.is_empty())
            .ok_or_else(|| ApiError::bad(format!("无法定点定位本次文件或所属媒体库：{path}")))?
            .1;
        for item in items {
            if library["LibraryOptions"]["EnableChapterImageExtraction"] == false
                || list(&item["Tags"]).iter().any(|tag| {
                    tag.as_str()
                        .is_some_and(|tag| tag.eq_ignore_ascii_case("videothumbnail-exclude"))
                })
            {
                continue;
            }
            targets.insert(
                string(&item, "Id"),
                (item, library["LibraryOptions"].clone()),
            );
        }
    }
    Ok(targets)
}
async fn remaining(
    state: &AppState,
    targets: &Targets,
    order: &std::collections::HashMap<String, crate::media_order::Key>,
    context: &JobContext,
) -> Result<Targets> {
    let mut result = BTreeMap::new();
    for (id, (before, options)) in ordered_targets(targets, order) {
        context.checkpoint().await?;
        context
            .activity(
                "验收缩略图",
                &format!(
                    "正在验收《{}》：{}（Item {id}）",
                    string(before, "Name"),
                    string(before, "Path")
                ),
            )
            .await;
        let item = item(state, id).await?;
        if item["Path"] != before["Path"] {
            return Err(ApiError::bad(format!(
                "Item {id} 路径在提取期间变化，未使用新路径图片验收或修复旧目标"
            )));
        }
        if missing(state, &item, options).await? {
            result.insert(id.clone(), (item, options.clone()));
        } else {
            tracing::info!(
                "[缩略图修复] {}（Item {id}）章节图或 BIF 已齐全",
                string(&item, "Path")
            );
        }
    }
    Ok(result)
}
pub fn encoder_archive(folder: &str, encoder: &str, rules: &[Value]) -> Result<Vec<u8>> {
    let mut archive = tar::Builder::new(vec![]);
    let mut directory = tar::Header::new_gnu();
    directory.set_entry_type(tar::EntryType::Directory);
    directory.set_size(0);
    directory.set_mode(0o755);
    directory.set_cksum();
    archive.append_data(&mut directory, folder, std::io::empty())?;
    let script = encoder_script(encoder, rules);
    let mut file = tar::Header::new_gnu();
    file.set_entry_type(tar::EntryType::Regular);
    file.set_size(script.len() as u64);
    file.set_mode(0o755);
    file.set_cksum();
    archive.append_data(&mut file, format!("{folder}/rffmpeg"), script.as_bytes())?;
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o755);
    link.set_link_name("rffmpeg")?;
    link.set_cksum();
    archive.append_data(&mut link, format!("{folder}/ffmpeg"), std::io::empty())?;
    archive.into_inner().map_err(ApiError::from)
}
pub fn verify_backend(lines: &[String], entry: &str) -> Result<()> {
    let command = lines
        .iter()
        .any(|l| l.contains("ProcessRun 'ffmpeg' Execute:") && l.contains(entry));
    let status = lines
        .iter()
        .rfind(|l| l.contains("Custom ffmpeg uses rffmpeg backend:"))
        .map(|l| l.rsplit(':').next().unwrap_or("").trim());
    if !command || status != Some("True") {
        return Err(config_error(
            "神医未将兼容入口识别为rffmpeg，已取消第二轮提取并恢复配置",
        ));
    }
    Ok(())
}
async fn find_container(docker: &bollard::Docker, system: &Value) -> Result<String> {
    let containers = docker
        .list_containers(None::<bollard::query_parameters::ListContainersOptions>)
        .await?;
    let mut matches = vec![];
    for container in containers {
        let image = container.image.unwrap_or_default().to_lowercase();
        if !["embyserver", "linuxserver/emby"]
            .iter()
            .any(|s| image.contains(s))
        {
            continue;
        }
        let id = container.id.unwrap_or_default();
        let path = format!(
            "{}/data/device.txt",
            string(system, "ProgramDataPath").trim_end_matches('/')
        );
        let result = crate::docker_host::exec(docker, &id, &["cat".into(), path]).await;
        match result {
            Ok(bytes) => {
                let Ok(device) = String::from_utf8(bytes) else {
                    tracing::warn!(container=%id,"Emby身份文件编码无效");
                    continue;
                };
                if device
                    .trim_start_matches('\u{feff}')
                    .trim()
                    .replace('-', "")
                    .eq_ignore_ascii_case(&string(system, "Id").replace('-', ""))
                {
                    matches.push(id);
                }
            }
            Err(error) => tracing::warn!(container=%id,"读取容器身份失败：{}",error.1),
        }
    }
    if matches.len() != 1 {
        return Err(ApiError::bad(format!(
            "Docker已连接，但Emby身份校验未唯一通过，匹配数：{}",
            matches.len()
        )));
    }
    Ok(matches.remove(0))
}
async fn fallback(
    state: &AppState,
    targets: &Targets,
    timeout: Duration,
    context: &JobContext,
    order: &std::collections::HashMap<String, crate::media_order::Key>,
) -> Result<()> {
    let config = crate::strm::config(state)
        .await
        .map_err(|_| config_error("无法读取神医当前编码器配置"))?;
    let encoder = string(&config, "CustomEncoderPath").trim().to_string();
    let probe = config["CustomProbePath"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("/bin/ffprobe")
        .trim()
        .to_string();
    if !encoder.starts_with('/') || encoder.contains("etk-thumbnail-") {
        return Err(ApiError::bad("请先在神医设置中配置有效的自定义ffmpeg"));
    }
    let system = crate::strm::admin_system(state)
        .await
        .map_err(|_| config_error("无法使用Emby管理员上下文读取系统目录"))?;
    for key in ["Id", "ProgramDataPath", "CachePath"] {
        if string(&system, key).trim().is_empty() {
            return Err(config_error("Emby管理员系统信息缺少字段"));
        }
    }
    if ["ProgramDataPath", "CachePath"]
        .iter()
        .any(|key| !string(&system, key).starts_with('/'))
    {
        return Err(config_error("Emby系统目录不是有效的容器绝对路径"));
    }
    let docker = crate::docker_host::client(state)?;
    let id = find_container(&docker, &system).await?;
    let mut rules = vec![];
    for (index, (_, (item, _))) in ordered_targets(targets, order).into_iter().enumerate() {
        context.checkpoint().await?;
        context
            .update(
                55 + 10 * (index + 1) as u64 / targets.len() as u64,
                "检查失败原因",
                &format!(
                    "检查失败文件 {}/{}：《{}》 · {}",
                    index + 1,
                    targets.len(),
                    string(item, "Name"),
                    string(item, "Path")
                ),
            )
            .await;
        let result = async {
            let source = source(item)?;
            let path = string(&source, "Path");
            if path.is_empty() || path.to_lowercase().ends_with(".strm") {
                return Err(ApiError::bad("缺少实际媒体输入路径"));
            }
            let (status, stdout, stderr) = crate::docker_host::exec_output(
                &docker,
                &id,
                &[
                    "timeout".into(),
                    "90".into(),
                    probe.clone(),
                    "-v".into(),
                    "debug".into(),
                    "-show_format".into(),
                    "-show_streams".into(),
                    "-show_chapters".into(),
                    "-of".into(),
                    "json".into(),
                    path,
                ],
            )
            .await?;
            if status != 0 {
                return Err(ApiError::bad(format!("自定义ffprobe退出码{status}")));
            }
            let raw = serde_json::from_slice::<Value>(&stdout)
                .map_err(|_| ApiError::bad("ffprobe返回无效JSON"))?;
            repair_rule(item, &raw, &String::from_utf8_lossy(&stderr))
        }
        .await;
        context.checkpoint().await?;
        match result {
            Ok(Some(rule)) => {
                let reasons = [
                    ("ignore_editlist", "忽略零时长 MP4 edit list"),
                    ("clamp_end", "末尾章节取帧前移 1 秒"),
                ]
                .into_iter()
                .filter(|(key, _)| rule[*key] == true)
                .map(|(_, reason)| reason)
                .collect::<Vec<_>>()
                .join("、");
                tracing::info!("[缩略图修复] {}：{reasons}", string(item, "Path"));
                rules.push(rule);
            }
            Ok(None) => tracing::warn!(
                "[缩略图修复] 未找到可自动修复的原因：{}",
                string(item, "Path")
            ),
            Err(error) => tracing::warn!(
                "[缩略图修复] 无法验证 {}：{}",
                string(item, "Path"),
                error.1
            ),
        }
    }
    if rules.is_empty() {
        return Ok(());
    }
    context.checkpoint().await?;
    if task(state).await?["State"] != "Idle" {
        return Err(config_error("神医提取任务未确认空闲，未切换编码器"));
    }
    crate::docker_host::exec(
        &docker,
        &id,
        &["sh".into(), "-c".into(), "command -v awk".into()],
    )
    .await?;
    let folder = format!("etk-thumbnail-{}", uuid::Uuid::new_v4().simple());
    let directory = format!(
        "{}/temp",
        string(&system, "CachePath").trim_end_matches('/')
    );
    let script = format!("{directory}/{folder}/ffmpeg");
    let archive = encoder_archive(&folder, &encoder, &rules)?;
    tracing::info!("[缩略图临时编码器] 开始上传补图文件");
    docker
        .upload_to_container(
            &id,
            Some(
                bollard::query_parameters::UploadToContainerOptionsBuilder::default()
                    .path(&directory)
                    .build(),
            ),
            bollard::body_full(archive.into()),
        )
        .await
        .inspect_err(|error| tracing::warn!(%error, "[缩略图临时编码器] 补图文件上传失败"))?;
    tracing::info!("[缩略图临时编码器] 补图文件已上传，开始补图，结束后恢复原编码器");
    let result = async {
        let mut cursor = LogCursor::new(state).await?;
        crate::strm::with_changes(state, &json!({"CustomEncoderPath":script}), || async {
            verify_backend(&cursor.read(state).await?, &script)?;
            run_native(state, timeout, 65, 95, Some(&script), context).await?;
            Ok(())
        })
        .await
        .map_err(|error| {
            if error.1.contains("神医临时") {
                config_error(&error.1)
            } else {
                error
            }
        })
    }
    .await;
    match &result {
        Ok(()) => tracing::info!("[缩略图临时编码器] 补图执行结束，检查停用状态与文件清理"),
        Err(error) => {
            tracing::warn!(reason = %error.1, "[缩略图临时编码器] 补图执行失败，检查停用状态与文件清理")
        }
    }
    let cleanup = async {
        let config = crate::strm::config(state).await?;
        if config["CustomEncoderPath"] != script && !active(&task(state).await?) {
            crate::docker_host::exec(
                &docker,
                &id,
                &[
                    "rm".into(),
                    "-f".into(),
                    script.clone(),
                    format!("{directory}/{folder}/rffmpeg"),
                ],
            )
            .await?;
            crate::docker_host::exec(
                &docker,
                &id,
                &["rmdir".into(), format!("{directory}/{folder}")],
            )
            .await?;
            tracing::info!("[缩略图临时编码器] 已确认停用，补图文件已清理");
        } else {
            tracing::warn!("[缩略图临时编码器] 暂时保留仍可能使用的缩略图编码器：{script}");
        }
        Ok::<_, ApiError>(())
    }
    .await;
    if let Err(error) = cleanup {
        tracing::warn!(
            "[缩略图临时编码器] 未确认已停用或清理完成，保留剩余文件：{}",
            error.1
        );
    }
    result
}
pub(crate) async fn extract_verified(
    state: &AppState,
    context: &JobContext,
) -> Result<NativeExtraction> {
    extract_with_timeout(state, Duration::from_secs(1800), context).await
}
async fn extract_with_timeout(
    state: &AppState,
    timeout: Duration,
    context: &JobContext,
) -> Result<NativeExtraction> {
    let config = crate::strm::config(state).await?;
    if config["PersistMediaInfoMode"] == "Restore" {
        return Ok(NativeExtraction {
            report: json!({"state":"skipped","native_state":"not_started","verification":"not_requested","processed":0,"eligible":0,"verified":0,"failed":0,"needs_repair":false,"reason":"神医处于仅恢复模式，未启动实际图片提取"}),
            paths: BTreeSet::new(),
            targets: Targets::new(),
        });
    }
    let mut trace = ExtractionLog::default();
    let outcome = run_with_trace(state, timeout, 5, 50, None, context, &mut trace).await;
    if context.is_cancelled() {
        context.checkpoint().await?;
    }
    let ended = outcome.is_ok() || task(state).await.is_ok_and(|task| task["State"] == "Idle");
    let native_error = outcome.as_ref().err().map(|error| error.1.clone());
    let paths = match outcome {
        Ok(paths) => paths,
        Err(_) if ended => match trace.result() {
            Ok(paths) => paths,
            Err(error) => {
                return Ok(NativeExtraction {
                    report: json!({"state":"unverified","native_state":"ended","native_ended":true,"verification":"incomplete_execution_log","failed":1,"needs_repair":false,"reason":error.1}),
                    paths: BTreeSet::new(),
                    targets: Targets::new(),
                });
            }
        },
        Err(error) => {
            return Ok(NativeExtraction {
                report: json!({"state":"unverified","native_state":"unknown","native_ended":false,"verification":"not_confirmed","failed":1,"needs_repair":false,"reason":error.1}),
                paths: BTreeSet::new(),
                targets: Targets::new(),
            });
        }
    };
    let candidates = targets(state, &paths, context).await?;
    let order =
        crate::media_order::id_keys(state, &candidates.keys().cloned().collect::<Vec<_>>()).await?;
    let missing = remaining(state, &candidates, &order, context).await?;
    let failed = missing
        .len()
        .max(trace.failed)
        .max(usize::from(native_error.is_some()));
    let unknown_skips = trace.skipped > 0;
    let reason = if unknown_skips {
        format!(
            "本次日志记录 {} 个跳过目标，但未逐项标明路径；实际缺图 {} 个，保留部分未确认范围，不自动绕过原生筛选修复",
            trace.skipped,
            missing.len()
        )
    } else if let Some(error) = native_error.as_ref() {
        format!(
            "原生任务失败：{error}；实际仍缺图 {} 个，失败或取消日志 {} 项",
            missing.len(),
            trace.failed
        )
    } else if failed > 0 {
        format!(
            "原生任务已结束，实际仍缺图 {} 个，失败或取消日志 {} 项",
            missing.len(),
            trace.failed
        )
    } else if candidates.is_empty() {
        "本次没有可验收的目标，未确认任何单项图片提取结果".into()
    } else {
        format!(
            "本次 {} 个目标的 BIF 或章节图已通过实际验收",
            candidates.len()
        )
    };
    let report = json!({"state":if unknown_skips && !missing.is_empty(){"unverified"}else if failed>0{"failed"}else if candidates.is_empty(){"skipped"}else{"completed"},
        "native_state":if native_error.is_none(){"completed"}else{"failed"},"native_ended":ended,
        "verification":"current_execution_log_and_item_images","scope_verification":if unknown_skips{"partial_unknown_skips"}else{"complete"},"processed":paths.len(),"eligible":candidates.len(),"skipped":trace.skipped,
        "verified":candidates.len().saturating_sub(missing.len()),"failed":failed,"failed_ids":missing.keys().collect::<Vec<_>>(),
        "needs_repair":!missing.is_empty() && ended && !unknown_skips,"reason":reason});
    Ok(NativeExtraction {
        report,
        paths,
        targets: missing,
    })
}
pub(crate) async fn repair_extraction(
    state: &AppState,
    extraction: &NativeExtraction,
    context: &JobContext,
) -> Result<Value> {
    if extraction.report["native_ended"] != true && !extraction.targets.is_empty() {
        return Err(config_error("原生提取任务尚未确认结束，未启动自动修复"));
    }
    if extraction.report["scope_verification"] == "partial_unknown_skips" {
        return Err(ApiError::bad(
            "本次跳过目标路径未确认，未自动修复可能被原生策略排除的媒体",
        ));
    }
    if extraction.targets.is_empty() {
        return Ok(
            json!({"state":"skipped","repaired":0,"failed":0,"message":"本次没有已确认需要修复的目标，未重复启动原生提取"}),
        );
    }
    if extraction.report["needs_repair"] != true {
        return Err(ApiError::bad(
            "本次跳过目标或修复范围未确认，未自动修复可能被原生策略排除的媒体",
        ));
    }
    let targets = &extraction.targets;
    let config = crate::strm::config(state).await?;
    if config["PersistMediaInfoMode"] == "Restore" {
        return Err(ApiError::bad("神医处于仅恢复模式，未启动图片修复"));
    }
    let estimate = targets
        .values()
        .map(|(_, options)| {
            if number(&options["ThumbnailImagesIntervalSeconds"]) > 0.0 {
                1800.0
            } else {
                600.0
            }
        })
        .sum::<f64>()
        * config["ImageExtractionTimeoutMultiplier"]
            .as_f64()
            .filter(|n| n.is_finite() && *n > 0.0)
            .unwrap_or(2.0);
    let timeout = Duration::from_secs_f64(estimate.clamp(1800.0, 86400.0));
    let order =
        crate::media_order::id_keys(state, &targets.keys().cloned().collect::<Vec<_>>()).await?;
    if let Err(error) = fallback(state, targets, timeout, context, &order).await {
        if context.is_cancelled() || error.0 == axum::http::StatusCode::CONFLICT {
            return Err(error);
        }
        tracing::warn!("自定义编码器补图未完成：{}", error.1);
    }
    let remaining = remaining(state, targets, &order, context).await?;
    for (id, (item, options)) in ordered_targets(&remaining, &order) {
        let kind = if number(&options["ThumbnailImagesIntervalSeconds"]) > 0.0 {
            "BIF"
        } else {
            "章节图"
        };
        tracing::error!(
            "[缩略图修复] 仍缺少{kind}：{}（Item {id}）",
            crate::media_paths::clean(&string(item, "Path"))
        );
    }
    let repaired = targets.len().saturating_sub(remaining.len());
    context
        .update(
            100,
            "验收",
            &format!(
                "缩略图修复结束：复用本次 {} 个文件结果，已修复 {repaired} 个，仍缺失 {} 个",
                extraction.paths.len(),
                remaining.len()
            ),
        )
        .await;
    Ok(
        json!({"state":if remaining.is_empty(){"completed"}else{"failed"},"repaired":repaired,"failed":remaining.len(),"failed_ids":remaining.keys().collect::<Vec<_>>(),"verification":"item_images","message":format!("缩略图修复结束：已修复{repaired}个，仍缺失{}个",remaining.len())}),
    )
}
pub async fn repair(state: &AppState, context: &JobContext) -> Result<Value> {
    let extraction = extract_with_timeout(state, Duration::from_secs(86400), context).await?;
    if extraction.report["state"] == "skipped" {
        return Ok(
            json!({"state":"skipped","repaired":0,"failed":0,"message":extraction.report["reason"],"extraction":extraction.report}),
        );
    }
    if extraction.report["failed"].as_u64().unwrap_or(0) == 0 {
        return Ok(
            json!({"repaired":extraction.report["verified"],"failed":0,"extraction":extraction.report,"message":"本次媒体图片已通过实际验收，未发现修复目标"}),
        );
    }
    if extraction.targets.is_empty() {
        return Err(ApiError::bad(
            extraction.report["reason"]
                .as_str()
                .unwrap_or("本次提取范围或结果未确认，未启动全库重复提取"),
        ));
    }
    let mut report = repair_extraction(state, &extraction, context).await?;
    report["extraction"] = extraction.report;
    Ok(report)
}

#[derive(Default)]
struct IntroLog {
    started: bool,
    complete: bool,
    episodes: Option<usize>,
    seasons: Option<usize>,
    episode_progress: BTreeSet<usize>,
    season_progress: BTreeSet<usize>,
    episodes_skipped: usize,
    seasons_skipped: usize,
    failed: usize,
}
impl IntroLog {
    fn feed(&mut self, lines: &[String]) {
        static COUNT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"IntroFingerprintExtract - Number of (episodes|seasons)( skipped)?: (\d+)")
                .unwrap()
        });
        static PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"IntroFingerprintExtract - (Episode|Season) Progress (\d+)/(\d+) - Task ")
                .unwrap()
        });
        for line in lines {
            if self.complete {
                break;
            }
            if line.contains("IntroFingerprintExtract - Scheduled Task Execute") {
                self.started = true;
            }
            if !self.started {
                continue;
            }
            if let Some(c) = COUNT.captures(line)
                && let Ok(n) = c[3].parse()
            {
                match (&c[1], c.get(2).is_some()) {
                    ("episodes", false) => self.episodes = Some(n),
                    ("seasons", false) => self.seasons = Some(n),
                    ("episodes", true) => self.episodes_skipped = n,
                    ("seasons", true) => self.seasons_skipped = n,
                    _ => {}
                }
            }
            if let Some(c) = PROGRESS.captures(line)
                && let Ok(n) = c[2].parse()
            {
                if &c[1] == "Episode" {
                    self.episode_progress.insert(n);
                } else {
                    self.season_progress.insert(n);
                }
            }
            if line.contains("IntroFingerprintExtract - Episode failed:")
                || line.contains("IntroFingerprintExtract - Season failed:")
                || line.contains("IntroFingerprintExtract - Episode cancelled:")
                || line.contains("IntroFingerprintExtract - Season cancelled:")
            {
                self.failed += 1;
            }
            if line.contains("IntroFingerprintExtract - Scheduled Task Complete") {
                self.complete = true;
            }
        }
    }
    fn report(&self) -> Value {
        let bounded = self.complete
            && self.episodes.is_some()
            && self.seasons.is_some()
            && self.episode_progress == (1..=self.episodes.unwrap_or(0)).collect()
            && self.season_progress.len() + self.seasons_skipped == self.seasons.unwrap_or(0);
        let state = if self.failed > 0 {
            "failed"
        } else if !bounded {
            "unverified"
        } else if self.episodes == Some(0) || self.episodes_skipped == self.episodes.unwrap_or(0) {
            "skipped"
        } else {
            "processed"
        };
        let reason = if self.failed > 0 {
            format!(
                "本次声纹处理日志包含 {} 项失败或取消，{} 个分集跳过，{} 个季跳过",
                self.failed, self.episodes_skipped, self.seasons_skipped
            )
        } else if !bounded {
            "本次声纹处理日志不完整，不能确认单项结果".into()
        } else if state == "skipped" {
            format!(
                "本次没有需要处理的片头声纹目标，跳过 {} 个分集和 {} 个季",
                self.episodes_skipped, self.seasons_skipped
            )
        } else {
            "本次原生日志已核对；未以片头标记是否存在判断声纹文件生成结果".into()
        };
        json!({"state":state,"native_state":"completed","verification":"current_execution_log",
            "episodes":self.episodes,"seasons":self.seasons,"episodes_processed":self.episode_progress.len(),
            "episodes_skipped":self.episodes_skipped,"seasons_skipped":self.seasons_skipped,
            "failed":self.failed.max(usize::from(!bounded)),
            "reason":reason})
    }
}
pub(crate) async fn extract_intro_verified(
    state: &AppState,
    context: &JobContext,
) -> Result<Value> {
    if crate::strm::config(state).await?["PersistMediaInfoMode"] == "Restore" {
        return Ok(
            json!({"state":"skipped","native_state":"not_started","verification":"not_requested","failed":0,"reason":"神医处于恢复模式，未启动实际片头声纹提取"}),
        );
    }
    let mut cursor = LogCursor::new(state).await?;
    let outcome = crate::emby::scheduled(state, "IntroFingerprintExtractTask", context).await;
    if context.is_cancelled() {
        context.checkpoint().await?;
    }
    let mut trace = IntroLog::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut log_error = None;
    loop {
        let lines = tokio::time::timeout_at(deadline, cursor.read(state)).await;
        match lines {
            Ok(Ok(lines)) => trace.feed(&lines),
            Ok(Err(error)) => {
                log_error = Some(error.1);
                break;
            }
            Err(_) => {
                log_error = Some("本次片头声纹日志读取超时".into());
                break;
            }
        }
        if trace.complete || outcome.is_err() || tokio::time::Instant::now() >= deadline {
            break;
        }
        context
            .sleep(
                Duration::from_secs(1)
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            )
            .await?;
    }
    let ended = outcome.is_ok()
        || api(state, Method::GET, "ScheduledTasks", &[])
            .await
            .is_ok_and(|tasks| {
                list(&tasks).iter().any(|task| {
                    task["Key"] == "IntroFingerprintExtractTask" && task["State"] == "Idle"
                })
            });
    let mut report = trace.report();
    report["native_ended"] = json!(ended);
    if let Err(error) = outcome {
        report["state"] = json!("failed");
        report["native_state"] = json!("failed");
        report["failed"] = json!(report["failed"].as_u64().unwrap_or(0).max(1));
        report["reason"] = json!(error.1);
    } else if let Some(error) = log_error {
        report["reason"] = json!(error);
    }
    Ok(report)
}
