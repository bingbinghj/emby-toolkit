use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use emby_toolkit::{
    AppState,
    auth::SessionSigner,
    logs::{self, LogStore},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

async fn request(text: &str, uri: &str) -> (StatusCode, Vec<u8>) {
    let root = tempfile::tempdir().unwrap();
    let store = LogStore::open(&root.path().join("logs")).unwrap();
    std::fs::write(root.path().join("logs/app.log"), text).unwrap();
    let state = AppState {
        db: sqlx::PgPool::connect_lazy("postgres://localhost/unused").unwrap(),
        config: Arc::new(RwLock::new(json!({}))),
        http: reqwest::Client::new(),
        sessions: Arc::new(SessionSigner::load(root.path()).unwrap()),
        recovery_tokens: Arc::default(),
        data_dir: root.path().into(),
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
    let response = logs::router()
        .with_state(state)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}

#[tokio::test]
async fn history_json_pages_preserve_full_lines_unicode_and_boundaries() {
    let lines: Vec<_> = (0..1001).map(|index| format!("2026-11-10 12:00:00,123 - emby_toolkit::media - CRITICAL - 恢复记录{index} <script> & 完整结尾\n")).collect();
    let text = lines.join("");
    for (page, expected_page, count, latest) in [
        (1, 1, 500, 1000),
        (2, 2, 500, 500),
        (3, 3, 1, 0),
        (999, 3, 1, 0),
        (0, 1, 500, 1000),
    ] {
        let (status, bytes) = request(
            &text,
            &format!("/api/logs/view?filename=app.log&format=json&page={page}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["file"], "app.log");
        assert_eq!(value["page"], expected_page);
        assert_eq!(value["pages"], 3);
        assert_eq!(value["total_lines"], 1001);
        assert_eq!(value["lines"].as_array().unwrap().len(), count);
        assert_eq!(value["lines"][0], lines[latest]);
        assert_eq!(value["lines"][count - 1], lines[latest + 1 - count]);
    }
}

#[tokio::test]
async fn empty_history_json_has_one_empty_page_and_retains_path_validation() {
    let (status, bytes) = request("", "/api/logs/view?filename=app.log&format=json&page=4").await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value,
        json!({"file":"app.log","lines":[],"page":1,"pages":1,"total_lines":0})
    );
    let (status, _) = request("secret", "/api/logs/view?filename=../app.log&format=json").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn native_cards_leave_plain_and_escaped_html_history_interfaces_available() {
    let text = "2026-11-10 12:00:00,123 - module - INFO - <script>正文 & 尾部</script>\n";
    let (status, bytes) = request(text, "/api/logs/view?filename=app.log").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(String::from_utf8(bytes).unwrap(), text);
    let (_, bytes) = request(text, "/api/logs/view?filename=app.log&format=html").await;
    let html = String::from_utf8(bytes).unwrap();
    assert!(html.contains("&lt;script&gt;正文 &amp; 尾部&lt;/script&gt;"));
    assert!(!html.contains("<script>正文"));
}

#[tokio::test]
async fn history_context_recognizes_current_chinese_task_ids_and_preserves_complete_flow() {
    let text = "2026-11-10 12:00:00,000 - emby_toolkit::jobs - INFO - [任务开始] 测试恢复 · 任务ID：task-101\n2026-11-10 12:00:01,000 - emby_toolkit::media - ERROR - 媒体 JSON 在失败后被修改，保留当前文件及备份 · 任务ID：task-101\n2026-11-10 12:00:02,000 - emby_toolkit::jobs - ERROR - [任务结束] 测试恢复 · 失败 · 任务ID：task-101\n";
    let (status, bytes) = request(text, "/api/logs/search_context?q=JSON").await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    let lines = value[0]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].as_str().unwrap().contains("[任务开始]"));
    assert!(lines[1].as_str().unwrap().contains("保留当前文件及备份"));
    assert!(lines[2].as_str().unwrap().contains("[任务结束]"));
}

#[tokio::test]
async fn mixed_mp_and_legacy_history_keeps_dates_and_newest_first_search_contexts() {
    let text = "2026-11-11 23:59:59,000 - emby_toolkit::jobs - INFO - [任务开始] 旧恢复 · 任务ID：old-101\n2026-11-11 23:59:59,100 - emby_toolkit::media - ERROR - 旧 JSON 恢复材料保留 · 任务ID：old-101\n2026-11-11 23:59:59,200 - emby_toolkit::jobs - INFO - [任务结束] 旧恢复 · 任务ID：old-101\nINFO: 2026-11-12 00:00:00,000 jobs.rs - [任务开始] 新恢复 · 任务ID：new-102\nERROR: 2026-11-12 00:00:00,100 media_reload.rs - 新 JSON 恢复材料保留 · 任务ID：new-102\nINFO: 2026-11-12 00:00:00,200 jobs.rs - [任务结束] 新恢复 · 任务ID：new-102\n";
    let (status, bytes) = request(text, "/api/logs/search?q=JSON").await;
    assert_eq!(status, StatusCode::OK);
    let matches: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(matches.as_array().unwrap().len(), 2);
    assert_eq!(matches[0]["date"], "2026-11-12 00:00:00");
    assert!(matches[0]["content"].as_str().unwrap().contains("新 JSON"));
    assert_eq!(matches[1]["date"], "2026-11-11 23:59:59");

    let (status, bytes) = request(text, "/api/logs/search_context?q=JSON").await;
    assert_eq!(status, StatusCode::OK);
    let blocks: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(blocks.as_array().unwrap().len(), 2);
    assert_eq!(blocks[0]["date"], "2026-11-12 00:00:00");
    assert_eq!(blocks[0]["lines"].as_array().unwrap().len(), 3);
    assert!(
        blocks[0]["lines"][0]
            .as_str()
            .unwrap()
            .starts_with("INFO: ")
    );
    assert_eq!(blocks[1]["date"], "2026-11-11 23:59:59");
    assert_eq!(blocks[1]["lines"].as_array().unwrap().len(), 3);
}
