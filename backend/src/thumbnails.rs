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
    let sources = list(&item["MediaSources"]);
    let matched = sources
        .iter()
        .filter(|s| string(s, "ItemId") == string(item, "Id"))
        .collect::<Vec<_>>();
    if matched.len() == 1 {
        return Ok(matched[0].clone());
    }
    if sources.len() == 1 {
        return Ok(sources[0].clone());
    }
    Err(ApiError::bad("无法唯一确定当前版本的媒体输入路径"))
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
}
impl ExtractionLog {
    pub fn feed(&mut self, lines: &[String]) {
        static COUNT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"VideoThumbnailExtract - Number of items: (\d+)").unwrap()
        });
        static PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"VideoThumbnailExtract - Progress (\d+)/(\d+) - Task \d+: (.+)").unwrap()
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
                if let Ok(n) = c[1].parse() {
                    self.processed.insert(n);
                }
                self.paths.insert(c[3].trim().into());
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
    let previous = current["LastExecutionResult"]["StartTimeUtc"].clone();
    let mut cursor = LogCursor::new(state).await?;
    let mut trace = ExtractionLog::default();
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
            current = task(state).await?;
            trace.feed(&cursor.read(state).await?);
            let result = &current["LastExecutionResult"];
            if !active(&current)
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
                        &format!("神医提取进度 {progress:.1}%"),
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
    outcome
}
async fn item(state: &AppState, id: &str) -> Result<Value> {
    let data = api(
        state,
        Method::GET,
        "Items",
        &[
            ("Ids", id.into()),
            ("Fields", "Path,Chapters,MediaSources,Tags,ParentId".into()),
        ],
    )
    .await?;
    list(&data["Items"])
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::bad("媒体已不可访问"))
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
        return Ok(list(&data["Thumbnails"]).is_empty());
    }
    let chapters = chapters(item);
    Ok(chapters.is_empty() || chapters.iter().any(|c| string(c, "ImageTag").is_empty()))
}
type Targets = BTreeMap<String, (Value, Value)>;
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
                ("Fields", "Path,Chapters,MediaSources,Tags,ParentId".into()),
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
            if missing(state, &item, &library["LibraryOptions"]).await? {
                targets.insert(
                    string(&item, "Id"),
                    (item, library["LibraryOptions"].clone()),
                );
            }
        }
    }
    Ok(targets)
}
async fn remaining(state: &AppState, targets: &Targets, context: &JobContext) -> Result<Targets> {
    let mut result = BTreeMap::new();
    for (id, (_, options)) in targets {
        context.checkpoint().await?;
        let item = item(state, id).await?;
        if missing(state, &item, options).await? {
            result.insert(id.clone(), (item, options.clone()));
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
    for (index, (item, _)) in targets.values().enumerate() {
        context.checkpoint().await?;
        context
            .update(
                55 + 10 * (index + 1) as u64 / targets.len() as u64,
                "检查失败原因",
                &format!("检查失败文件：{}", string(item, "Path")),
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
            Ok(Some(rule)) => rules.push(rule),
            Ok(None) => tracing::warn!("未找到可自动修复的原因：{}", string(item, "Path")),
            Err(error) => tracing::warn!("无法验证缩略图失败原因：{}", error.1),
        }
    }
    if rules.is_empty() {
        return Ok(());
    }
    context.checkpoint().await?;
    if active(&task(state).await?) {
        return Err(ApiError::bad("神医提取任务仍在运行，未切换编码器"));
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
        .await?;
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
        } else {
            tracing::warn!("暂时保留仍可能使用的缩略图编码器：{script}");
        }
        Ok::<_, ApiError>(())
    }
    .await;
    if let Err(error) = cleanup {
        tracing::warn!("未确认临时编码器已停用，保留：{}", error.1);
    }
    result
}
pub async fn repair(state: &AppState, context: &JobContext) -> Result<Value> {
    let config = crate::strm::config(state).await?;
    if config["PersistMediaInfoMode"] == "Restore" {
        return Err(ApiError::bad(
            "神医处于仅恢复模式，请关闭后再执行图片提取修复",
        ));
    }
    let paths = run_native(state, Duration::from_secs(86400), 5, 50, None, context).await?;
    context
        .update(
            50,
            "检查本次结果",
            &format!("根据本次日志定点验收{}个文件", paths.len()),
        )
        .await;
    let targets = targets(state, &paths, context).await?;
    if targets.is_empty() {
        return Ok(
            json!({"repaired":paths.len(),"failed":0,"message":"本次未发现缺失章节图或BIF"}),
        );
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
    if let Err(error) = fallback(state, &targets, timeout, context).await {
        if context.is_cancelled() || error.0 == axum::http::StatusCode::CONFLICT {
            return Err(error);
        }
        tracing::warn!("自定义编码器补图未完成：{}", error.1);
    }
    let remaining = remaining(state, &targets, context).await?;
    Ok(
        json!({"repaired":paths.len().saturating_sub(remaining.len()),"failed":remaining.len(),"message":format!("缩略图修复完成：本次{}个文件，仍缺失{}个",paths.len(),remaining.len())}),
    )
}
