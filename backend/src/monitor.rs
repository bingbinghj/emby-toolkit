use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::{JobContext, JobManager, JobSpec},
    media_edit::list,
};
use notify::{
    EventKind, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RenameMode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
#[path = "../tests/fixtures/monitor_lifecycle.rs"]
mod lifecycle_tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub enabled: bool,
    pub roots: Vec<PathBuf>,
    pub extensions: BTreeSet<String>,
    pub exclude: Vec<PathBuf>,
    pub refresh_delay: u64,
}
impl Settings {
    pub async fn read(config: &Value) -> Self {
        let mut roots = BTreeSet::new();
        for value in list(&config["monitor_paths"]) {
            if let Some(path) = value.as_str()
                && Path::new(path).is_dir()
            {
                roots.insert(PathBuf::from(path));
            }
        }
        let mut minimal = vec![];
        for root in roots {
            if !minimal
                .iter()
                .any(|parent: &PathBuf| root.starts_with(parent))
            {
                minimal.push(root);
            }
        }
        let extensions = list(&config["monitor_extensions"])
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_lowercase().replace('*', ""))
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('.') {
                    s
                } else {
                    format!(".{s}")
                }
            })
            .collect();
        Self {
            enabled: config["monitor_enabled"] == true,
            roots: minimal,
            extensions,
            exclude: list(&config["monitor_exclude_dirs"])
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect(),
            refresh_delay: config["monitor_exclude_refresh_delay"]
                .as_u64()
                .unwrap_or(0),
        }
    }
    pub async fn accepts(&self, path: &Path) -> Option<PathBuf> {
        let name = path.file_name()?.to_str()?;
        let ext = format!(".{}", path.extension()?.to_str()?.to_lowercase());
        if name.starts_with('.')
            || [".part", ".!qB", ".crdownload", ".tmp", ".aria2"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
            || !self.extensions.contains(&ext)
        {
            return None;
        }
        let canonical = tokio::fs::canonicalize(path).await.ok()?;
        if !canonical.is_file() {
            return None;
        }
        for root in &self.roots {
            if let Ok(root) = tokio::fs::canonicalize(root).await
                && canonical.starts_with(root)
            {
                return Some(path.to_path_buf());
            }
        }
        None
    }
    pub fn excluded(&self, path: &Path) -> bool {
        path_excluded(path, &self.exclude)
    }
}
pub fn normalized_path(path: &Path) -> String {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if result.file_name().is_some_and(|p| p != "..") {
                    result.pop();
                } else if !result.has_root() {
                    result.push("..");
                }
            }
            _ => result.push(part.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        ".".into()
    } else {
        result.to_string_lossy().into_owned()
    }
}
pub fn path_excluded(path: &Path, excluded: &[PathBuf]) -> bool {
    let normalized = normalized_path(path).to_lowercase();
    excluded.iter().any(|p| {
        let p = normalized_path(p).to_lowercase();
        normalized == p || normalized.starts_with(&format!("{p}/"))
    })
}
pub fn group_paths(paths: &[PathBuf]) -> Vec<Vec<PathBuf>> {
    let mut groups = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for path in paths {
        if let Some(parent) = path.parent() {
            groups.entry(parent.into()).or_default().push(path.clone());
        }
    }
    groups
        .into_values()
        .map(|mut group| {
            group.sort();
            group.dedup();
            group
        })
        .collect()
}
#[derive(Default)]
pub struct Stability {
    last: Option<u64>,
    equal: u8,
}
impl Stability {
    pub fn observe(&mut self, size: u64, strm: bool) -> bool {
        if strm && size > 0 {
            return true;
        }
        if size > 0 && self.last == Some(size) {
            self.equal = self.equal.saturating_add(1);
        } else {
            self.equal = 0;
        }
        self.last = Some(size);
        self.equal >= 3
    }
}
pub async fn stable_files(
    settings: &Settings,
    paths: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>> {
    let mut pending = paths
        .iter()
        .cloned()
        .map(|p| (p, Stability::default()))
        .collect::<BTreeMap<_, _>>();
    let mut result = vec![];
    for _ in 0..60 {
        tokio::select! { _=cancel.cancelled()=>return Err(ApiError::bad("实时监控服务已停止")), result=context.checkpoint()=>result? }
        if cancel.is_cancelled() {
            return Err(ApiError::bad("实时监控服务已停止"));
        }
        let mut completed = vec![];
        for (path, tracker) in &mut pending {
            let Some(canonical) = settings.accepts(path).await else {
                completed.push(path.clone());
                continue;
            };
            if let Ok(metadata) = tokio::fs::metadata(&canonical).await
                && tracker.observe(
                    metadata.len(),
                    canonical
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("strm")),
                )
            {
                result.push(path.clone());
                completed.push(path.clone());
            }
        }
        for path in completed {
            pending.remove(&path);
        }
        if pending.is_empty() {
            break;
        }
        tokio::select! {_=cancel.cancelled()=>return Err(ApiError::bad("实时监控服务已停止")),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
    }
    if !pending.is_empty() {
        tracing::warn!(
            count = pending.len(),
            "实时监控文件60轮后仍不稳定，跳过本批"
        );
    }
    result.sort();
    result.dedup();
    Ok(result)
}
pub struct Service {
    state: AppState,
    running: AtomicBool,
    tx: mpsc::UnboundedSender<PathBuf>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<PathBuf>>>,
}
impl Service {
    pub fn new(state: AppState) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            state,
            running: AtomicBool::new(false),
            tx,
            rx: Mutex::new(Some(rx)),
        })
    }
    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(mut rx) = self.rx.lock().unwrap().take() else {
            self.running.store(false, Ordering::SeqCst);
            return;
        };
        let prepared = tokio::select! {
            _=cancel.cancelled()=>false,
            ready=self.state.jobs.wait_recovery_ready()=>match ready {
                Ok(())=>true,
                Err(error)=>{tracing::warn!(reason=%error.1,"实时监控等待恢复准备结束，保留待办");false},
            },
        };
        if !prepared {
            *self.rx.lock().unwrap() = Some(rx);
            self.running.store(false, Ordering::SeqCst);
            return;
        }
        let mut delay = 1_u64;
        let manager = loop {
            let result = tokio::select! {
                _ = cancel.cancelled() => break None,
                result = JobManager::open_lane(self.state.db.clone(), "monitor") => result,
            };
            match result {
                Ok(manager) => break Some(Arc::new(manager)),
                Err(error) => tracing::warn!(reason=%error.1,"实时监控持久任务读取失败，保留待办并自动重试"),
            }
            tokio::select! {
                _ = cancel.cancelled() => break None,
                _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
            }
            delay = (delay * 2).min(8);
        };
        let Some(manager) = manager else {
            *self.rx.lock().unwrap() = Some(rx);
            self.running.store(false, Ordering::SeqCst);
            return;
        };
        manager.bind_config(&self.state.config);
        let dispatch_ready = Arc::new(AtomicBool::new(false));
        let restored = dispatch_ready.clone();
        let restore_manager = manager.clone();
        let restore_state = self.state.clone();
        let restore_cancel = cancel.clone();
        let restore = tokio::spawn(async move {
            let mut delay = 1_u64;
            loop {
                let result = tokio::select! {
                    _ = restore_cancel.cancelled() => return,
                    result = restore_manager.resume_pending(restore_state.clone(), restore_cancel.clone()) => result,
                };
                match result {
                    Ok(()) => {
                        if !restore_cancel.is_cancelled() {
                            restored.store(true, Ordering::Release);
                        }
                        return;
                    }
                    Err(error) => tracing::warn!(reason=%error.1,"实时监控未完成任务恢复失败，保留原任务并自动重试"),
                }
                tokio::select! {
                    _ = restore_cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(delay)) => {},
                }
                delay = (delay * 2).min(8);
            }
        });
        let mut watcher = None;
        let mut signature = String::new();
        let initial_config = self.state.config.read().await.clone();
        let mut settings = Settings::read(&initial_config).await;
        let mut queue = BTreeSet::new();
        let mut deadline = Some(tokio::time::Instant::now());
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=cancel.cancelled()=>break,
                path=rx.recv()=>if let Some(path)=path && settings.enabled && let Some(path)=settings.accepts(&path).await{
                    if let Err(error)=crate::monitor_persistence::record(&self.state,&path).await {
                        tracing::error!(reason=%error.1,"实时监控事件未能持久保存，保留事件并重试");
                        queue.insert(path);
                    }
                    deadline=Some(tokio::time::Instant::now()+Duration::from_secs(3));
                },
                _=tick.tick()=>{
                    let config=self.state.config.read().await.clone();let next_signature=json!([config["monitor_enabled"],config["monitor_paths"],config["monitor_extensions"],config["monitor_exclude_dirs"],config["monitor_exclude_refresh_delay"]]).to_string();
                    if next_signature!=signature{
                        let next_settings=Settings::read(&config).await;
                        let retains_scope = next_settings.enabled
                            && settings.extensions.is_subset(&next_settings.extensions)
                            && settings.roots.iter().all(|path| {
                                next_settings.roots.iter().any(|root| path.starts_with(root))
                            })
                            && next_settings.exclude.iter().all(|path| {
                                settings.excluded(path) || !settings.roots.iter().any(|root| {
                                    path_excluded(root, std::slice::from_ref(path))
                                        || path_excluded(path, std::slice::from_ref(root))
                                })
                            });
                        if !retains_scope && manager.status().await["is_running"]==true {
                            if let Err(error)=manager.control(None,"stop").await {
                                tracing::warn!(reason=%error.1,"实时监控范围已改变，正在停止原任务");
                            }
                        }
                        if watcher.take().is_some(){tracing::info!("[实时监控] 目录监听实例已停止，重新加载配置");}
                        settings=next_settings;signature=next_signature;
                        let mut retained=BTreeSet::new();for path in &queue{if let Some(path)=settings.accepts(path).await{retained.insert(path);}}queue=retained;
                        if let Err(error)=crate::monitor_persistence::retain(&self.state,&settings).await {
                            tracing::error!(reason=%error.1,"实时监控待办范围核对失败，保留待办");
                        }
                        if settings.enabled&&!settings.roots.is_empty(){let tx=self.tx.clone();match notify::recommended_watcher(move|event:notify::Result<notify::Event>|{match event{
                            Ok(event)=>{let paths=match event.kind{EventKind::Create(CreateKind::File|CreateKind::Any)=>event.paths,EventKind::Modify(ModifyKind::Name(RenameMode::To))=>event.paths,EventKind::Modify(ModifyKind::Name(RenameMode::Both))=>event.paths.into_iter().skip(1).collect(),_=>vec![]};for path in paths{let _=tx.send(path);}},
                            Err(error)=>tracing::warn!(%error,"实时监控文件事件读取失败")
                        }}){Ok(mut next)=>{let mut watched=0;for root in &settings.roots{match next.watch(root,RecursiveMode::Recursive){Ok(())=>watched+=1,Err(error)=>tracing::warn!(path=%root.display(),%error,"无法监控目录")}}
                            tracing::info!(watched,total=settings.roots.len(),"[实时监控] 目录监听实例已初始化");watcher=Some(next);},Err(error)=>tracing::warn!(%error,"实时监控初始化失败")}}
                        if !settings.enabled{queue.clear();deadline=None;}
                        else {deadline=Some(tokio::time::Instant::now()+Duration::from_secs(3));}
                    }
                    if settings.enabled && dispatch_ready.load(Ordering::Acquire) && deadline.is_some_and(|d|d<=tokio::time::Instant::now()) && manager.status().await["is_running"]!=true {
                        let mut saved=vec![];
                        for path in &queue {
                            if crate::monitor_persistence::record(&self.state,path).await.is_ok(){saved.push(path.clone());}
                        }
                        for path in saved {queue.remove(&path);}
                        match crate::monitor_persistence::next(&self.state,&settings).await {
                            Ok(Some(payload))=>{
                                let work=self.state.clone();let spec=JobSpec::new("monitor",payload.clone());
                                if let Err(error)=manager.submit_persistent("实时监控入库预处理",spec,move|context|async move{
                                    execute_persisted(&work,&payload,&context).await.map(|(_,result)|result)
                                }).await {
                                    tracing::warn!(reason=%error.1,"实时监控队列提交失败，保留持久批次");
                                }
                            },
                            Ok(None) if queue.is_empty()=>deadline=None,
                            Ok(None)=>{},
                            Err(error)=>tracing::error!(reason=%error.1,"实时监控持久待办读取失败，未执行文件操作"),
                        }
                    }
                }
            }
        }
        if watcher.take().is_some() {
            tracing::info!("[实时监控] 目录监听实例已停止，服务正在关闭");
        }
        manager.shutdown().await;
        let _ = restore.await;
        *self.rx.lock().unwrap() = Some(rx);
        self.running.store(false, Ordering::SeqCst);
    }
}
pub async fn execute_persisted(
    state: &AppState,
    payload: &Value,
    context: &JobContext,
) -> Result<(u16, Value)> {
    let settings: Settings = serde_json::from_value(payload["settings"].clone())
        .map_err(|_| ApiError::bad("实时监控持久批次配置无效"))?;
    let paths: Vec<PathBuf> = serde_json::from_value(payload["paths"].clone())
        .map_err(|_| ApiError::bad("实时监控持久批次路径无效"))?;
    let current = Settings::read(&state.config.read().await.clone()).await;
    if !current.enabled {
        return Ok((
            200,
            json!({"prepared":0,"failed":0,"status":"monitor_disabled","message":"实时监控已关闭，未继续原批次"}),
        ));
    }
    let mut eligible = vec![];
    for path in paths {
        if settings.accepts(&path).await.is_some() && current.accepts(&path).await.is_some() {
            eligible.push(path);
        }
    }
    let result = process_batch(
        state,
        &settings,
        &eligible,
        context,
        &CancellationToken::new(),
    )
    .await?;
    Ok((200, result))
}
async fn stable_for_phase(
    settings: &Settings,
    paths: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
    phase: &str,
) -> Result<Vec<PathBuf>> {
    let mut candidates = paths.to_vec();
    if let Some(saved) = context.plan(phase).await? {
        let selected: Vec<PathBuf> = serde_json::from_value(saved["paths"].clone())
            .map_err(|_| ApiError::bad("实时监控文件范围检查点无效"))?;
        candidates = selected
            .into_iter()
            .filter(|path| paths.contains(path))
            .collect();
        if crate::monitor_persistence::revision(&candidates).await? == saved["revision"] {
            return Ok(candidates);
        }
    }
    let stable = stable_files(settings, &candidates, context, cancel).await?;
    let revision = crate::monitor_persistence::revision(&stable).await?;
    context
        .save_plan(phase, &json!({"paths":stable,"revision":revision}))
        .await?;
    Ok(stable)
}
async fn process_batch(
    state: &AppState,
    settings: &Settings,
    paths: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<Value> {
    let (scrape, refresh): (Vec<_>, Vec<_>) = paths
        .iter()
        .cloned()
        .partition(|path| !settings.excluded(path));
    let scrape_work = async {
        let stable =
            stable_for_phase(settings, &scrape, context, cancel, "monitor-scrape-files").await?;
        if stable.is_empty() {
            return Ok(json!({"prepared":0,"failed":0}));
        }
        crate::monitor_ingest::prepare_batch(
            state,
            &group_paths(&stable),
            &settings.roots,
            context,
            cancel,
        )
        .await
    };
    let refresh_work = async {
        let refresh =
            stable_for_phase(settings, &refresh, context, cancel, "monitor-refresh-files").await?;
        if refresh.is_empty() {
            return Ok::<_, ApiError>(0);
        }
        let revision = crate::monitor_persistence::revision(&refresh).await?;
        if context
            .completed("monitor-refresh")
            .await?
            .is_some_and(|saved| saved["revision"] == revision)
        {
            return Ok(refresh.len());
        }
        context.checkpoint().await?;
        let mut remaining = context
            .plan("monitor-refresh-delay")
            .await?
            .and_then(|saved| saved.as_u64())
            .unwrap_or(settings.refresh_delay);
        context
            .save_plan("monitor-refresh-delay", &json!(remaining))
            .await?;
        while remaining > 0 {
            tokio::select! {_=cancel.cancelled()=>return Err(ApiError::bad("实时监控服务已停止")),result=context.sleep(Duration::from_secs(1))=>result?}
            remaining -= 1;
            context
                .save_plan("monitor-refresh-delay", &json!(remaining))
                .await?;
        }
        let config = state.config.read().await.clone();
        let current = Settings::read(&config).await;
        if current.enabled {
            let mut eligible = vec![];
            for path in &refresh {
                if let Some(path) = current.accepts(path).await {
                    eligible.push(path);
                }
            }
            context.checkpoint().await?;
            crate::monitor_ingest::refresh_files(
                state,
                &eligible,
                &current.roots,
                false,
                context,
                cancel,
            )
            .await?;
        }
        context
            .complete("monitor-refresh", &json!({"revision":revision}))
            .await?;
        Ok(refresh.len())
    };
    let work = async {
        let (scrape_result, refresh_result) = tokio::join!(scrape_work, refresh_work);
        let mut result = scrape_result?;
        result["refresh_only"] = json!(refresh_result?);
        Ok(result)
    };
    tokio::pin!(work);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            context.request_stop();
            work.await
        },
        result = &mut work => result,
    }
}
