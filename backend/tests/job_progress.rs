use emby_toolkit::jobs::JobManager;
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn wait_for(manager: &JobManager, state: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = manager.status().await;
            if status["state"] == state {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn logs_and_status_preserve_real_activity_counts_and_task_outcome() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let capture = Capture(output.clone());
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .event_format(emby_toolkit::logs::LogFormat)
            .with_writer(move || capture.clone())
            .finish(),
    )
    .unwrap();
    let manager = Arc::new(JobManager::default());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let notify = entered.clone();
    let gate = release.clone();
    manager
        .submit("补齐115指纹验收", move |context| async move {
            context
                .activity(
                    "读取媒体",
                    "正在读取《天上掉下个猪八戒》 · 第1季第1集 · 版本 #1",
                )
                .await;
            tracing::info!(target: "emby_toolkit::webhook", "[Webhook] 正在核对《天上掉下个猪八戒》 · 第1季第1集 · cookie=fixture-private-cookie");
            notify.notify_one();
            gate.acquire().await.unwrap().forget();
            context
                .update_counted(1, 4, "补齐115指纹", "已补齐 第1集 · 版本 #1：PC=✓，SHA1=✓")
                .await;
            context
                .activity("回写115缓存", "正在保存 第1季第2集 · 版本 #1")
                .await;
            notify.notify_one();
            gate.acquire().await.unwrap().forget();
            Ok(json!({"failed":1,"message":"成功 3，未完成 1：115 文件详情不完整"}))
        })
        .await
        .unwrap();
    entered.notified().await;
    let status = manager.status().await;
    assert_eq!(status["progress_known"], false);
    assert_eq!(status["progress"], 0);
    assert!(status["message"].as_str().unwrap().contains("第1季第1集"));
    assert!(
        status["last_activity"]
            .as_str()
            .unwrap()
            .contains("正在核对")
    );
    assert!(
        !status["last_activity"]
            .as_str()
            .unwrap()
            .contains("fixture-private-cookie")
    );
    tracing::info!(target: "emby_toolkit::telegram_bot", "其他后台服务的活动");
    assert!(
        manager.status().await["last_activity"]
            .as_str()
            .unwrap()
            .contains("正在核对")
    );
    // Match the original's actual 30-second external-call activity heartbeat.
    tokio::time::sleep(Duration::from_secs(31)).await;
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let waiting = logs.lines().find(|line| line.contains("已运行")).unwrap();
    assert!(waiting.contains("补齐115指纹验收 · 读取媒体"));
    assert!(waiting.contains("天上掉下个猪八戒"));
    assert!(waiting.contains("正在核对"));
    assert!(!waiting.contains("fixture-private-cookie"));
    assert!(!waiting.contains('%'));
    assert_eq!(manager.status().await["progress_known"], false);
    release.add_permits(1);
    entered.notified().await;
    let status = manager.status().await;
    assert_eq!(status["completed"], 1);
    assert_eq!(status["total"], 4);
    assert_eq!(status["progress"], 25);
    assert_eq!(status["progress_known"], true);
    assert_eq!(status["phase"], "回写115缓存");
    release.add_permits(1);
    let failed = wait_for(&manager, "failed").await;
    assert_eq!(failed["result"]["failed"], 1);
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("补齐115指纹验收 · 补齐115指纹 · 1/4 · 25%"));
    let ended = logs
        .lines()
        .find(|line| line.contains("[任务结束]"))
        .unwrap();
    assert!(ended.contains("ERROR"));
    assert!(ended.contains("补齐115指纹验收 · 失败 · 耗时"));
    assert!(ended.contains("115 文件详情不完整"));
    assert!(!ended.contains("\"成功"));

    let notify = entered.clone();
    let gate = release.clone();
    let id = manager
        .submit("暂停取消验收", move |context| async move {
            context
                .update_counted(1, 4, "指纹校验", "正在核对《媒体》 · 版本 #1")
                .await;
            notify.notify_one();
            gate.acquire().await.unwrap().forget();
            context.checkpoint().await?;
            notify.notify_one();
            context.sleep(Duration::from_secs(60)).await?;
            Ok(json!({}))
        })
        .await
        .unwrap();
    entered.notified().await;
    manager.control(Some(&id), "pause").await.unwrap();
    release.add_permits(1);
    let paused = wait_for(&manager, "paused").await;
    assert_eq!(paused["completed"], 1);
    assert_eq!(paused["progress"], 25);
    manager.control(Some(&id), "resume").await.unwrap();
    entered.notified().await;
    manager.control(Some(&id), "stop").await.unwrap();
    let cancelled = wait_for(&manager, "cancelled").await;
    assert_eq!(cancelled["progress"], 25);
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let ended = logs
        .lines()
        .find(|line| line.contains("[任务结束] 暂停取消验收"))
        .unwrap();
    assert!(ended.contains("INFO"));
    assert!(ended.contains("已停止 · 耗时"));
    assert!(!ended.contains("WARNING") && !ended.contains("ERROR"));

    manager
        .submit("未确认项验收", |_| async {
            Ok(json!({"warnings":1,"message":"扫描完成，无法确认 1 项"}))
        })
        .await
        .unwrap();
    wait_for(&manager, "completed_with_warnings").await;
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let ended = logs
        .lines()
        .find(|line| line.contains("[任务结束] 未确认项验收"))
        .unwrap();
    assert!(ended.contains("WARN"));
    assert!(ended.contains("完成，有提醒 · 耗时"));
    assert!(ended.contains("无法确认 1 项"));
    let notify = entered.clone();
    let gate = release.clone();
    manager
        .submit("空扫描验收", move |context| async move {
            context
                .update_counted(0, 0, "扫描完成", "没有可处理的媒体")
                .await;
            notify.notify_one();
            gate.acquire().await.unwrap().forget();
            Ok(json!({"message":"没有可处理的媒体"}))
        })
        .await
        .unwrap();
    entered.notified().await;
    let empty = manager.status().await;
    assert_eq!(empty["completed"], 0);
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["progress"], 100);
    release.add_permits(1);
    wait_for(&manager, "completed").await;
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("空扫描验收 · 扫描完成 · 0/0 · 100%"));
    print!("{logs}");
}
