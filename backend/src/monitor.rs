use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::list,
};
use notify::{
    EventKind, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RenameMode},
};
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

#[derive(Clone, Debug)]
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
                && let Ok(path) = tokio::fs::canonicalize(path).await
                && path.is_dir()
            {
                roots.insert(path);
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
        (canonical.is_file() && self.roots.iter().any(|root| canonical.starts_with(root)))
            .then_some(canonical)
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
                result.push(canonical);
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
        let mut watcher = None;
        let mut signature = String::new();
        let mut batch_cancel = cancel.child_token();
        let initial_config = self.state.config.read().await.clone();
        let mut settings = Settings::read(&initial_config).await;
        let mut queue = BTreeSet::new();
        let mut deadline = None;
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=cancel.cancelled()=>break,
                path=rx.recv()=>if let Some(path)=path && settings.enabled && let Some(path)=settings.accepts(&path).await{queue.insert(path);deadline=Some(tokio::time::Instant::now()+Duration::from_secs(3));},
                _=tick.tick()=>{
                    let config=self.state.config.read().await.clone();let next_signature=json!([config["monitor_enabled"],config["monitor_paths"],config["monitor_extensions"],config["monitor_exclude_dirs"],config["monitor_exclude_refresh_delay"]]).to_string();
                    if next_signature!=signature{batch_cancel.cancel();batch_cancel=cancel.child_token();
                        if watcher.take().is_some(){tracing::info!("[实时监控] 目录监听实例已停止，重新加载配置");}
                        settings=Settings::read(&config).await;signature=next_signature;
                        let mut retained=BTreeSet::new();for path in &queue{if let Some(path)=settings.accepts(path).await{retained.insert(path);}}queue=retained;
                        if settings.enabled&&!settings.roots.is_empty(){let tx=self.tx.clone();match notify::recommended_watcher(move|event:notify::Result<notify::Event>|{match event{
                            Ok(event)=>{let paths=match event.kind{EventKind::Create(CreateKind::File|CreateKind::Any)=>event.paths,EventKind::Modify(ModifyKind::Name(RenameMode::To))=>event.paths,EventKind::Modify(ModifyKind::Name(RenameMode::Both))=>event.paths.into_iter().skip(1).collect(),_=>vec![]};for path in paths{let _=tx.send(path);}},
                            Err(error)=>tracing::warn!(%error,"实时监控文件事件读取失败")
                        }}){Ok(mut next)=>{let mut watched=0;for root in &settings.roots{match next.watch(root,RecursiveMode::Recursive){Ok(())=>watched+=1,Err(error)=>tracing::warn!(path=%root.display(),%error,"无法监控目录")}}
                            tracing::info!(watched,total=settings.roots.len(),"[实时监控] 目录监听实例已初始化");watcher=Some(next);},Err(error)=>tracing::warn!(%error,"实时监控初始化失败")}}
                        if !settings.enabled{queue.clear();deadline=None;}
                    }
                    if settings.enabled&&!queue.is_empty()&&deadline.is_some_and(|d|d<=tokio::time::Instant::now()){
                        if self.state.jobs.status().await["is_running"]==true{continue;}
                        let paths=queue.iter().cloned().collect::<Vec<_>>();let work=self.state.clone();let snapshot=settings.clone();let shutdown=batch_cancel.clone();
                        let submitted=self.state.jobs.submit("实时监控入库预处理",move|context|async move{process_batch(&work,&snapshot,&paths,&context,&shutdown).await}).await;
                        match submitted{Ok(_)=>{queue.clear();deadline=None;},Err(error)if error.0==axum::http::StatusCode::CONFLICT=>{},Err(error)=>tracing::warn!(reason=%error.1,"实时监控队列提交失败，保留待处理文件")}
                    }
                }
            }
        }
        if watcher.take().is_some() {
            tracing::info!("[实时监控] 目录监听实例已停止，服务正在关闭");
        }
        *self.rx.lock().unwrap() = Some(rx);
        self.running.store(false, Ordering::SeqCst);
    }
}
async fn process_batch(
    state: &AppState,
    settings: &Settings,
    paths: &[PathBuf],
    context: &JobContext,
    cancel: &CancellationToken,
) -> Result<Value> {
    let stable = stable_files(settings, paths, context, cancel).await?;
    let (scrape, refresh): (Vec<_>, Vec<_>) = stable
        .into_iter()
        .partition(|path| !settings.excluded(path));
    let mut result = if scrape.is_empty() {
        json!({"prepared":0,"failed":0})
    } else {
        crate::monitor_ingest::prepare_batch(
            state,
            &group_paths(&scrape),
            &settings.roots,
            context,
            cancel,
        )
        .await?
    };
    if !refresh.is_empty() {
        context.checkpoint().await?;
        let mut remaining = settings.refresh_delay;
        while remaining > 0 {
            tokio::select! {_=cancel.cancelled()=>return Err(ApiError::bad("实时监控服务已停止")),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
            context.checkpoint().await?;
            remaining -= 1;
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
    }
    result["refresh_only"] = json!(refresh.len());
    Ok(result)
}
