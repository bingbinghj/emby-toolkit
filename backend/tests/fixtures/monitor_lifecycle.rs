use super::*;
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use std::{ffi::OsString, os::unix::fs::PermissionsExt};
use tokio::sync::{Notify, Semaphore};
use tracing::{
    Subscriber,
    field::{Field, Visit},
    instrument::WithSubscriber,
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[derive(Clone)]
struct Host {
    label: &'static str,
    events: Arc<tokio::sync::Mutex<Vec<String>>>,
    queried: Arc<Notify>,
    refreshed: Arc<Notify>,
    gate: Arc<Semaphore>,
    hold_library: Arc<AtomicBool>,
    hold_refresh: Arc<AtomicBool>,
}
async fn api(State(host): State<Host>, request: Request) -> Response {
    let path = request.uri().path();
    let method = request.method();
    host.events
        .lock()
        .await
        .push(format!("{} {method} {path}", host.label));
    if path == "/Library/VirtualFolders" {
        if host.hold_library.swap(false, Ordering::SeqCst) {
            host.queried.notify_one();
            host.gate.acquire().await.unwrap().forget();
        }
        return Json(json!([])).into_response();
    }
    if path == "/Items" && method == Method::GET {
        let query = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .collect::<BTreeMap<_, _>>();
        let directory = query.get("Path").map(|value| value.as_ref()).unwrap_or("");
        let name = Path::new(directory).file_name().unwrap().to_string_lossy();
        return Json(json!({"Items":[{"Id":format!("{}-{name}",host.label),"Path":directory,"Type":"Folder"}]})).into_response();
    }
    if path.ends_with("/Refresh") && method == Method::POST {
        host.refreshed.notify_one();
        if host.hold_refresh.swap(false, Ordering::SeqCst) {
            host.gate.acquire().await.unwrap().forget();
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    if path == "/movie/123" {
        return Json(json!({"original_language":"en","images":{"posters":[],"backdrops":[]}}))
            .into_response();
    }
    if path == "/app/chrome/downurl" {
        return Json(json!({"state":true,"data":{"url":"http://127.0.0.1/fixture-video"}}))
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

struct Fixture {
    state: AppState,
    admin: sqlx::PgPool,
    schema: String,
    root: tempfile::TempDir,
    hosts: Vec<Host>,
    bases: Vec<String>,
    servers: Vec<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let admin = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("monitor_lifecycle_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&search).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let artifacts = Path::new("/root/emby-toolkit-nov12-followup3-verification/monitor");
        std::fs::create_dir_all(artifacts).unwrap();
        let root = tempfile::tempdir_in(artifacts).unwrap();
        let mut state = AppState::new(pool, root.path().join("app")).await.unwrap();
        let events = Arc::new(tokio::sync::Mutex::new(vec![]));
        let mut hosts = vec![];
        let mut bases = vec![];
        let mut servers = vec![];
        for label in ["A", "B"] {
            let host = Host {
                label,
                events: events.clone(),
                queried: Arc::default(),
                refreshed: Arc::default(),
                gate: Arc::new(Semaphore::new(0)),
                hold_library: Arc::default(),
                hold_refresh: Arc::default(),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            bases.push(format!("http://{}", listener.local_addr().unwrap()));
            let app = Router::new().fallback(api).with_state(host.clone());
            servers.push(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap()
            }));
            hosts.push(host);
        }
        let media = root.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state.save_config(&json!({"monitor_enabled":true,"monitor_paths":[media],"monitor_extensions":[".strm"],"monitor_exclude_dirs":[],"monitor_exclude_refresh_delay":0,"emby_server_url":bases[0],"emby_api_key":"fixture","tmdb_api_key":"fixture","tmdb_api_base_url":bases[0],"local_strm_root":media,"monitor_sha1_pc_search":true,"extract_thumb":false})).await.unwrap();
        state.p115 = Arc::new(crate::p115::Service::with_endpoints(
            crate::p115::Endpoints {
                openapi: bases[0].clone(),
                ..Default::default()
            },
        ));
        crate::db::save_setting(
            &state.db,
            "p115_auth_tokens",
            &json!({"cookie":"fixture-cookie"}),
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO person_metadata(tmdb_person_id,primary_name) VALUES(1,'测试演员')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO media_metadata(tmdb_id,item_type,title,actors_json) VALUES('123','Movie','测试电影',$1)").bind(json!([{"tmdb_id":1}])).execute(&state.db).await.unwrap();
        Self {
            state,
            admin,
            schema,
            root,
            hosts,
            bases,
            servers,
        }
    }
    fn media(&self) -> PathBuf {
        self.root.path().join("media")
    }
    async fn movie(&self, poster: bool) -> PathBuf {
        let directory = self.media().join("Movie [tmdb-123]");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join("movie.strm");
        tokio::fs::write(&path, "https://115.invalid/d/fixturepc")
            .await
            .unwrap();
        if poster {
            tokio::fs::write(directory.join("poster.jpg"), "existing poster")
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,pick_code,sha1,local_path,size) VALUES('1','0','movie.strm','fixturepc','ABC',$1,100)").bind(path.to_str()).execute(&self.state.db).await.unwrap();
        path
    }
    async fn finished(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let result = self.state.jobs.result(task).await.unwrap();
                if result["is_running"] == false {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(self) {
        self.state.jobs.shutdown().await;
        for server in self.servers {
            server.abort();
        }
        self.state.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

struct Messages(mpsc::UnboundedSender<String>);
impl<S: Subscriber> Layer<S> for Messages {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        struct Message(String);
        impl Visit for Message {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let _ = self.0.send(message.0);
    }
}
async fn message(rx: &mut mpsc::UnboundedReceiver<String>, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if rx.recv().await.unwrap().contains(expected) {
                break;
            }
        }
    })
    .await
    .unwrap();
}
async fn start_listener(
    f: &Fixture,
) -> (
    Arc<Service>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
    mpsc::UnboundedReceiver<String>,
) {
    let service = Service::new(f.state.clone());
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let subscriber = tracing_subscriber::registry().with(Messages(tx));
    let task = tokio::spawn(
        service
            .clone()
            .run(cancel.clone())
            .with_subscriber(subscriber),
    );
    message(&mut rx, "目录监听实例已初始化").await;
    (service, cancel, task, rx)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL DATABASE_URL; HTTP servers are localhost"]
async fn monitor_refresh_does_not_mix_server_snapshot_with_new_connection() {
    let f = Fixture::new().await;
    let mut paths = vec![];
    for name in ["one", "two"] {
        let directory = f.media().join(name);
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join("movie.strm");
        tokio::fs::write(&path, "fixture").await.unwrap();
        paths.push(path);
    }
    f.hosts[0].hold_refresh.store(true, Ordering::SeqCst);
    let state = f.state.clone();
    let roots = vec![f.media()];
    let task = f
        .state
        .jobs
        .submit("monitor connection regression", move |context| async move {
            crate::monitor_ingest::refresh_files(
                &state,
                &paths,
                &roots,
                false,
                &context,
                &CancellationToken::new(),
            )
            .await?;
            Ok(json!({}))
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), f.hosts[0].refreshed.notified())
        .await
        .unwrap();
    f.state
        .save_config(&json!({"emby_server_url":f.bases[1]}))
        .await
        .unwrap();
    f.hosts[0].gate.add_permits(1);
    let result = f.finished(&task).await;
    let events = f.hosts[0].events.lock().await.clone();
    f.close().await;
    assert!(
        events.iter().all(|event| !event.starts_with("B ")),
        "operation queried new server: {events:?}; {result}"
    );
    assert!(
        events
            .iter()
            .filter(|event| event.contains(" POST "))
            .all(|event| !event.contains("/B-")),
        "new server identity was written to old server: {events:?}"
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL DATABASE_URL; HTTP servers are localhost"]
async fn monitor_unrelated_config_change_keeps_submitted_file() {
    for change in ["delay", "add-root"] {
        let f = Fixture::new().await;
        let movie = f.movie(true).await;
        f.state
            .save_config(&json!({"libraries_to_ignore":["unrelated-library"]}))
            .await
            .unwrap();
        let additional = f.root.path().join("additional");
        tokio::fs::create_dir_all(&additional).await.unwrap();
        f.hosts[0].hold_library.store(true, Ordering::SeqCst);
        let (service, cancel, listener, mut messages) = start_listener(&f).await;
        service.tx.send(movie.clone()).unwrap();
        tokio::time::timeout(Duration::from_secs(8), f.hosts[0].queried.notified())
            .await
            .unwrap();
        let updates = if change == "delay" {
            json!({"monitor_exclude_refresh_delay":1})
        } else {
            json!({"monitor_paths":[f.media(), additional]})
        };
        f.state.save_config(&updates).await.unwrap();
        message(&mut messages, "目录监听实例已停止").await;
        f.hosts[0].gate.add_permits(1);
        let refreshed =
            tokio::time::timeout(Duration::from_secs(5), f.hosts[0].refreshed.notified())
                .await
                .is_ok();
        let nfo = movie.with_extension("nfo").is_file();
        let events = f.hosts[0].events.lock().await.clone();
        cancel.cancel();
        listener.await.unwrap();
        f.close().await;
        assert!(
            refreshed && nfo,
            "submitted file was lost after {change}: {events:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL DATABASE_URL; HTTP server is localhost"]
async fn monitor_movie_batch_preserves_unsubmitted_nfo_and_writes_each_submitted_part() {
    let f = Fixture::new().await;
    let primary = f.movie(true).await;
    let directory = primary.parent().unwrap().to_path_buf();
    let part = directory.join("movie.part2.strm");
    tokio::fs::write(&part, "https://115.invalid/d/fixture-part-two")
        .await
        .unwrap();
    let other = directory.join("other.strm");
    tokio::fs::write(&other, "https://115.invalid/d/unsubmitted-other")
        .await
        .unwrap();
    let other_nfo = other.with_extension("nfo");
    let original_other =
        b"<movie><title>Unsubmitted other movie</title><plot>keep exact bytes</plot></movie>";
    tokio::fs::write(&other_nfo, original_other).await.unwrap();
    let state = f.state.clone();
    let roots = vec![f.media()];
    let paths = vec![primary.clone(), part.clone()];
    let task = f
        .state
        .jobs
        .submit("monitor NFO batch scope", move |context| async move {
            crate::monitor_ingest::prepare_batch(
                &state,
                &[paths],
                &roots,
                &context,
                &CancellationToken::new(),
            )
            .await
        })
        .await
        .unwrap();
    let result = f.finished(&task).await;
    let actual_other = tokio::fs::read(&other_nfo).await.unwrap();
    let primary_nfo = tokio::fs::read_to_string(primary.with_extension("nfo"))
        .await
        .unwrap_or_default();
    let part_nfo = tokio::fs::read_to_string(part.with_extension("nfo"))
        .await
        .unwrap_or_default();
    let original_poster = tokio::fs::read(directory.join("poster.jpg")).await.unwrap();
    f.close().await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(
        actual_other, original_other,
        "unsubmitted movie NFO was overwritten"
    );
    assert_eq!(result["result"]["prepared"], 1, "{result}");
    assert_eq!(result["result"]["failed"], 0, "{result}");
    assert_eq!(
        result["result"]["nfo_written"], 2,
        "only both submitted files should receive NFO"
    );
    assert!(primary_nfo.contains("测试电影"), "{primary_nfo}");
    assert!(part_nfo.contains("测试电影"), "{part_nfo}");
    assert_eq!(original_poster, b"existing poster");
}

struct PathGuard(Option<OsString>);
impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(path) = &self.0 {
                std::env::set_var("PATH", path);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
}
fn fake_ffmpeg(directory: &Path) -> PathGuard {
    let bin = directory.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("ffmpeg");
    std::fs::write(&executable, "#!/bin/sh\nfor arg do target=$arg; done\ndir=${target%/*}\nprintf 'fixture picture' > \"$target\"\n: > \"$dir/ffmpeg.started\"\nwhile [ ! -e \"$dir/ffmpeg.release\" ]; do sleep 0.01; done\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let previous = std::env::var_os("PATH");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        previous.as_deref().unwrap_or_default(),
    ));
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    }
    PathGuard(previous)
}
async fn until_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL DATABASE_URL; run with --test-threads=1 for fake ffmpeg PATH"]
async fn monitor_disable_during_capture_keeps_final_images_unwritten() {
    for change in ["disable", "remove-root"] {
        let f = Fixture::new().await;
        let movie = f.movie(false).await;
        let directory = movie.parent().unwrap();
        let _path = fake_ffmpeg(f.root.path());
        f.state
            .save_config(&json!({"extract_thumb":true}))
            .await
            .unwrap();
        let (service, cancel, listener, mut messages) = start_listener(&f).await;
        service.tx.send(movie.clone()).unwrap();
        until_file(&directory.join("ffmpeg.started")).await;
        let updates = if change == "disable" {
            json!({"monitor_enabled":false})
        } else {
            json!({"monitor_paths":[]})
        };
        f.state.save_config(&updates).await.unwrap();
        message(&mut messages, "目录监听实例已停止").await;
        tokio::fs::write(directory.join("ffmpeg.release"), "release")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let stages = std::fs::read_dir(directory)
                    .unwrap()
                    .flatten()
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".etk-thumb-")
                    });
                if !stages {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let landscape = directory.join("landscape.jpg").exists();
        let fanart = directory.join("fanart.jpg").exists();
        let unchanged = tokio::fs::read_to_string(&movie).await.unwrap();
        cancel.cancel();
        listener.await.unwrap();
        f.close().await;
        assert!(
            !landscape && !fanart,
            "monitor committed after {change}: landscape={landscape}, fanart={fanart}"
        );
        assert_eq!(unchanged, "https://115.invalid/d/fixturepc");
    }
}
