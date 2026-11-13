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
    assert!(regex::Regex::new(r"(?m)^INFO: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2},\d{3} log_format\.rs - 开始同步媒体 · Emby ID：101，名称：测试电影，版本数：2$").unwrap().is_match(&file));
    assert!(file.contains(" log_format.rs - 媒体同步失败 · 错误：网络超时"));
    assert!(file.contains(" log_format.rs - 保存媒体失败 · 原因：路径已经变化"));
    assert!(file.lines().any(|line| line.starts_with("DEBUG: ")
        && line.contains(" log_format.rs - [115] 正在请求 /files：目录ID=3，分页偏移=1000")));
    assert!(!file.contains("emby_toolkit::sync"));
    assert!(file.contains("数据库查询较慢，超过告警阈值"));
    assert!(file.contains("返回行数：14213") && file.contains("耗时：2.35s"));
    assert!(!file.contains("slow statement:") && !file.contains("elapsed_secs="));
    assert!(!file.contains("fixture-private-cookie"));
    assert!(file.contains("cookie：********"));
    let live = store.recent();
    assert_eq!(live.len(), 4);
    assert!(live.iter().any(
        |line| line.starts_with("WARNING: ") && line.contains(" log_format.rs - 媒体同步失败")
    ));
    assert!(live.iter().all(|line| !line.starts_with("DEBUG: ")));
    assert!(
        live.iter()
            .all(|line| !line.contains("fixture-private-cookie"))
    );
    print!("{file}");
}

#[test]
fn mp_style_live_logs_preserve_full_dates_sources_multiline_and_filter_levels() {
    use std::io::Write;
    use tracing_subscriber::fmt::MakeWriter;

    let directory = tempfile::tempdir().unwrap();
    let store = LogStore::open(directory.path()).unwrap();
    let writer = LogWriter(store.clone());
    let logs = [
        "INFO: 2026-11-11 23:59:59,990 sync.rs - 昨日记录\n原生续行\n",
        "WARNING: 2026-11-12 00:00:00,010 webhook.rs - 今日记录\n",
        "DEBUG: 2026-11-12 00:00:00,020 p115.rs - 调试详情\n",
        "TRACE: 2026-11-12 00:00:00,030 p115.rs - 请求详情\n",
        "CRITICAL: 2026-11-12 00:00:00,040 media_reload.rs - 必须保留恢复材料\n",
    ];
    for line in &logs {
        writer.make_writer().write_all(line.as_bytes()).unwrap();
    }
    assert_eq!(
        store.recent(),
        [logs[0], logs[1], logs[4]]
            .map(|line| line.trim_end().to_string())
            .to_vec()
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("app.log")).unwrap(),
        [logs[0], logs[1], logs[2], logs[4]].join("")
    );
}

#[test]
fn live_logs_preserve_native_dates_sources_and_milliseconds_across_midnight() {
    use std::io::Write;
    use tracing_subscriber::fmt::MakeWriter;

    let directory = tempfile::tempdir().unwrap();
    let store = LogStore::open(directory.path()).unwrap();
    let writer = LogWriter(store.clone());
    let lines = [
        "2026-11-11 23:59:59,990 - emby_toolkit::sync - INFO - 昨日记录\n",
        "2026-11-12 00:00:00,010 - emby_toolkit::sync - INFO - 今日记录\n",
    ];
    for line in &lines {
        writer.make_writer().write_all(line.as_bytes()).unwrap();
    }
    assert_eq!(
        store.recent(),
        lines.map(|line| line.trim_end().to_string()).to_vec()
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("app.log")).unwrap(),
        lines.join("")
    );
}
