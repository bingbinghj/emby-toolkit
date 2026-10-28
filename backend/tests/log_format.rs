use emby_toolkit::logs::{LogFormat, LogStore, LogWriter};

#[test]
fn python_style_logs_preserve_fields_levels_and_redaction_in_file_and_live_view() {
    let directory = tempfile::tempdir().unwrap();
    let store = LogStore::open(directory.path()).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .event_format(LogFormat)
        .with_writer(LogWriter(store.clone()))
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "emby_toolkit::sync", item_id = "101", name = "测试电影", versions = 2, "开始同步媒体");
        tracing::warn!(target: "emby_toolkit::webhook", error = "网络超时", cookie = "fixture-private-cookie", "媒体同步失败");
        tracing::error!(target: "emby_toolkit::media_edit", reason = "路径已经变化", "保存媒体失败");
        tracing::debug!(target: "emby_toolkit::p115", "[115] 正在请求 /files：目录ID=3，分页偏移=1000");
        tracing::warn!(target: "sqlx::query", summary = "SELECT …", rows_returned = 14213, elapsed = ?std::time::Duration::from_millis(2350), elapsed_secs = 2.35, slow_threshold = ?std::time::Duration::from_secs(1), "slow statement: execution time exceeded alert threshold");
    });
    let file = std::fs::read_to_string(directory.path().join("app.log")).unwrap();
    assert!(file.contains(
        " - emby_toolkit::sync - INFO - 开始同步媒体 · Emby ID：101，名称：测试电影，版本数：2"
    ));
    assert!(file.contains(" - WARNING - 媒体同步失败 · 错误：网络超时"));
    assert!(file.contains(" - ERROR - 保存媒体失败 · 原因：路径已经变化"));
    assert!(file.contains(" - DEBUG - [115] 正在请求 /files：目录ID=3，分页偏移=1000"));
    assert!(file.contains("数据库查询较慢，超过告警阈值"));
    assert!(file.contains("返回行数：14213") && file.contains("耗时：2.35s"));
    assert!(!file.contains("slow statement:") && !file.contains("elapsed_secs="));
    assert!(!file.contains("fixture-private-cookie"));
    assert!(file.contains("cookie：********"));
    let live = store.recent();
    assert_eq!(live.len(), 4);
    assert!(
        live.iter()
            .any(|line| line.contains("[WARNING] 媒体同步失败"))
    );
    assert!(live.iter().all(|line| !line.contains("[DEBUG]")));
    assert!(
        live.iter()
            .all(|line| !line.contains("fixture-private-cookie"))
    );
    print!("{file}");
}
