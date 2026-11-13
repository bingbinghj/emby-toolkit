use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use emby_toolkit::{AppState, auth::SessionSigner, logs};
use futures_util::StreamExt;
use serde_json::json;
use std::{io::Write, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new(text: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = logs::LogStore::open(&directory.path().join("logs")).unwrap();
        std::fs::write(directory.path().join("logs/app.log"), text).unwrap();
        let state = AppState {
            db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
            config: Arc::new(RwLock::new(json!({}))),
            http: reqwest::Client::new(),
            sessions: Arc::new(SessionSigner::load(directory.path()).unwrap()),
            recovery_tokens: Arc::default(),
            data_dir: directory.path().into(),
            notifications: Arc::default(),
            webhooks: Arc::default(),
            ai: Arc::default(),
            jobs: Arc::default(),
            logs: store,
            p115: Arc::default(),
            strm: Arc::default(),
            douban: Arc::default(),
            docker: Arc::default(),
        };
        Self { state, directory }
    }
    fn path(&self) -> std::path::PathBuf {
        self.directory.path().join("logs/app.log")
    }
    fn append(&self, bytes: &[u8]) {
        std::fs::OpenOptions::new()
            .append(true)
            .open(self.path())
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    async fn request(&self, uri: &str) -> axum::response::Response {
        logs::router()
            .with_state(self.state.clone())
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn stream(
        &self,
    ) -> impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin {
        let response = self.request("/api/logs/stream").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-accel-buffering"], "no");
        response.into_body().into_data_stream()
    }
}

async fn message<S>(stream: &mut S) -> String
where
    S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let data = stream.next().await.expect("stream ended").unwrap();
            let text = String::from_utf8(data.to_vec()).unwrap();
            let lines: Vec<_> = text
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|body| body.strip_prefix(' ').unwrap_or(body))
                })
                .collect();
            if !lines.is_empty() {
                return lines.join("\n");
            }
        }
    })
    .await
    .expect("no log message")
}

#[tokio::test]
async fn sse_replays_fifty_complete_disk_lines_after_reconnect_including_debug() {
    let lines: Vec<_> = (0..60)
        .map(|i| format!("DEBUG: 2026-11-13 13:19:14,476 sync.rs - 记录{i}\n"))
        .collect();
    let f = Fixture::new(&(lines.join("") + "unfinished"));
    let expected = lines[10..].concat().trim_end().to_string();
    for _ in 0..2 {
        let mut stream = f.stream().await;
        assert_eq!(message(&mut stream).await, expected);
    }
}

#[tokio::test]
async fn sse_preserves_utf8_and_waits_for_the_complete_appended_line() {
    let f = Fixture::new("");
    let mut stream = f.stream().await;
    let bytes = "汉字正文\n".as_bytes();
    f.append(&bytes[..2]);
    // Heartbeat comments can be returned while an incomplete UTF-8 line waits.
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!String::from_utf8(first.to_vec()).unwrap().contains("data:"));
    f.append(&bytes[2..]);
    assert_eq!(message(&mut stream).await, "汉字正文");
}

#[tokio::test]
async fn sse_drains_old_inode_before_following_rotation_without_replaying_history() {
    let f = Fixture::new("initial\n");
    let mut stream = f.stream().await;
    assert_eq!(message(&mut stream).await, "initial");
    f.append(b"old-tail\n");
    std::fs::rename(f.path(), f.path().with_file_name("app.log.1")).unwrap();
    std::fs::write(f.path(), "new-first\n").unwrap();
    assert_eq!(message(&mut stream).await, "old-tail");
    assert_eq!(message(&mut stream).await, "new-first");
    f.append(b"new-second\n");
    assert_eq!(message(&mut stream).await, "new-second");
}

#[tokio::test]
async fn sse_keeps_an_unterminated_rotation_tail_separate_from_the_new_first_line() {
    let f = Fixture::new("initial\n旧尾部未换行");
    let mut stream = f.stream().await;
    assert_eq!(message(&mut stream).await, "initial");
    std::fs::rename(f.path(), f.path().with_file_name("app.log.1")).unwrap();
    std::fs::write(f.path(), "新文件首行\n").unwrap();
    assert_eq!(message(&mut stream).await, "旧尾部未换行");
    assert_eq!(message(&mut stream).await, "新文件首行");
}

#[tokio::test]
async fn sse_detects_in_place_truncation_even_after_the_file_regrows() {
    let f = Fixture::new("old-one\nold-two\n");
    let mut stream = f.stream().await;
    assert_eq!(message(&mut stream).await, "old-one\nold-two");
    std::fs::write(
        f.path(),
        "brand-new-first-longer-than-the-whole-old-file\nanother-new-line\n",
    )
    .unwrap();
    assert_eq!(
        message(&mut stream).await,
        "brand-new-first-longer-than-the-whole-old-file\nanother-new-line"
    );
}

#[tokio::test]
async fn sse_limits_only_each_read_batch_and_preserves_all_large_appends() {
    let f = Fixture::new("initial\n");
    let mut stream = f.stream().await;
    assert_eq!(message(&mut stream).await, "initial");
    let text = (0..18000)
        .map(|i| format!("大日志{i:05} {}\n", "内容".repeat(20)))
        .collect::<String>();
    f.append(text.as_bytes());
    let mut received = String::new();
    while received.len() < text.len() {
        let chunk = message(&mut stream).await;
        received.push_str(&chunk);
        received.push('\n');
    }
    assert_eq!(received, text);
}

#[tokio::test]
async fn zip_download_includes_current_and_latest_nine_rotations_with_exact_contents() {
    let f = Fixture::new("current正文\n");
    for i in (1..=12).rev() {
        std::fs::write(
            f.path().with_file_name(format!("app.log.{i}")),
            format!("rotation-{i}\n"),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    std::fs::write(f.path().with_file_name("app.log-unrelated"), "unrelated").unwrap();
    let response = f.request("/api/logs/download").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zip");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let disposition = response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap();
    let stem = disposition
        .split('"')
        .nth(1)
        .unwrap()
        .strip_suffix(".zip")
        .unwrap()
        .to_string();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 10);
    for name in
        std::iter::once("app.log".to_string()).chain((1..=9).map(|i| format!("app.log.{i}")))
    {
        let mut file = archive.by_name(&format!("{stem}/{name}")).unwrap();
        assert_eq!(file.compression(), zip::CompressionMethod::Deflated);
        let mut content = String::new();
        std::io::Read::read_to_string(&mut file, &mut content).unwrap();
        assert_eq!(
            content,
            if name == "app.log" {
                "current正文\n".into()
            } else {
                format!("rotation-{}\n", name.rsplit('.').next().unwrap())
            }
        );
    }
}

#[tokio::test]
async fn zip_download_selects_the_latest_modified_rotations_even_when_number_order_differs() {
    let f = Fixture::new("current\n");
    for i in 1..=12 {
        std::fs::write(
            f.path().with_file_name(format!("app.log.{i}")),
            format!("rotation-{i}\n"),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    let response = f.request("/api/logs/download").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names = (0..archive.len())
        .map(|i| {
            archive
                .by_index(i)
                .unwrap()
                .name()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        std::iter::once("app.log".to_string())
            .chain((4..=12).rev().map(|i| format!("app.log.{i}")))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn live_and_archive_routes_require_the_existing_administrator_gate() {
    let f = Fixture::new("private-log\n");
    for path in ["/api/logs/stream", "/api/logs/download"] {
        let response = emby_toolkit::router(f.state.clone())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(
            !String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("private-log")
        );
    }
}

#[tokio::test]
async fn graceful_shutdown_closes_an_open_log_stream_and_still_waits_for_safe_job_cleanup() {
    let f = Fixture::new("INFO: 2026-11-13 13:19:14,476 sync.rs - initial\n");
    let (cleanup_started, started) = tokio::sync::oneshot::channel();
    let (release_cleanup, cleanup_gate) = tokio::sync::oneshot::channel();
    f.state
        .jobs
        .submit("安全收尾测试", move |context| async move {
            context.cancelled().await;
            cleanup_started.send(()).unwrap();
            cleanup_gate.await.unwrap();
            Ok(json!({"success":true,"cleanup_completed":true}))
        })
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = logs::router().with_state(f.state.clone());
    let jobs = f.state.jobs.clone();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                stopping.await.unwrap();
                // This is the same ordering as main: task shutdown completes before
                // Axum starts waiting for all accepted connections to close.
                jobs.shutdown().await;
            })
            .await
            .unwrap();
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/logs/stream"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    let initial = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&initial).contains("initial"));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();

    // Keep the real client connected. EOF must come from the service rather
    // than dropping the client to make graceful shutdown finish artificially.
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(chunk) = stream.next().await {
            chunk.unwrap();
        }
    })
    .await
    .expect("open SSE did not reach EOF during service shutdown");
    assert!(
        !server.is_finished(),
        "server must still wait for the task cleanup gate"
    );
    release_cleanup.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut server)
        .await
        .expect("graceful server did not exit")
        .unwrap();
    assert_eq!(f.state.jobs.status().await["state"], "cancelled");
    assert_eq!(f.state.jobs.status().await["is_running"], false);
}

#[tokio::test]
async fn new_log_streams_are_rejected_after_service_shutdown_begins() {
    let f = Fixture::new("private-log\n");
    f.state.jobs.shutdown().await;
    let response = f.request("/api/logs/stream").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[cfg(unix)]
#[tokio::test]
async fn live_and_archive_reject_a_primary_log_symlink_outside_the_log_directory() {
    let f = Fixture::new("");
    let secret = f.directory.path().join("private.txt");
    std::fs::write(&secret, "private-sentinel").unwrap();
    std::fs::remove_file(f.path()).unwrap();
    std::os::unix::fs::symlink(&secret, f.path()).unwrap();
    for path in ["/api/logs/stream", "/api/logs/download"] {
        let response = f.request(path).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert!(
            !String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("private-sentinel")
        );
    }
}
