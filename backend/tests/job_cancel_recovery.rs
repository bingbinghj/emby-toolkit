use axum::http::StatusCode;
use emby_toolkit::{error::ApiError, jobs::JobManager};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;

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

async fn finished(manager: &JobManager, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let result = manager.result(id).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn cancelled_jobs_preserve_recovery_results_errors_and_real_progress() {
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
    let backup = "/config/backups/media-info-reload/cancel-recovery";
    for scenario in [
        "恢复失败",
        "恢复提醒",
        "恢复业务失败",
        "恢复成功",
        "正常取消",
    ] {
        let (ready, started) = oneshot::channel();
        let id = manager
            .submit_response(scenario, move |context| async move {
                context
                    .update_counted(1, 4, "等待恢复", "等待已受理操作完成")
                    .await;
                ready.send(()).unwrap();
                context.cancelled().await;
                match scenario {
                    "恢复失败" => Err(ApiError(
                        StatusCode::CONFLICT,
                        format!("Emby 恢复请求返回 HTTP 403，原资源恢复尚未确认；备份：{backup}"),
                    )),
                    "恢复提醒" => Ok((
                        207,
                        json!({"warnings":1,"warning_details":["原配置恢复尚未确认"],
                    "backup_path":backup,"message":format!("原配置恢复尚未确认；备份：{backup}")}),
                    )),
                    "恢复成功" => Ok((200, json!({"verified":1,"backup_removed":true}))),
                    "恢复业务失败" => Ok((200, json!({"success":false,"backup_path":backup,"message":format!("恢复接口未成功；备份：{backup}")}))),
                    _ => {
                        context.checkpoint().await?;
                        Ok((200, json!({})))
                    }
                }
            })
            .await
            .unwrap();
        started.await.unwrap();
        manager.control(Some(&id), "stop").await.unwrap();
        let result = finished(&manager, &id).await;
        assert_eq!(result["state"], "cancelled", "{result}");
        assert_eq!(result["progress"], 25);
        assert_eq!(result["completed"], 1);
        assert_eq!(result["total"], 4);
        let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let ended = logs
            .lines()
            .find(|line| line.contains(&format!("[任务结束] {scenario}")))
            .unwrap();
        if ["恢复失败", "恢复提醒", "恢复业务失败"].contains(&scenario) {
            assert!(
                result["message"].as_str().unwrap().contains(backup),
                "{result}"
            );
            assert!(
                ended.contains("WARN")
                    && ended.contains("已停止，有提醒")
                    && ended.contains(backup),
                "{ended}"
            );
            if scenario == "恢复失败" {
                assert_eq!(result["result_status"], 409);
                assert!(
                    result["result"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("HTTP 403")
                );
            } else if scenario == "恢复提醒" {
                assert_eq!(result["result_status"], 207);
                assert_eq!(result["result"]["backup_path"], backup);
                assert_eq!(result["result"]["warnings"], 1);
            } else {
                assert_eq!(result["result_status"], 200);
                assert_eq!(result["result"]["success"], false);
                assert_eq!(result["result"]["backup_path"], backup);
            }
        } else {
            assert_eq!(result["message"], "任务已取消");
            assert!(
                !ended.contains("WARN") && !ended.contains("ERROR"),
                "{ended}"
            );
            if scenario == "恢复成功" {
                assert_eq!(result["result_status"], 200);
                assert_eq!(result["result"]["verified"], 1);
                assert_eq!(result["result"]["backup_removed"], true);
            } else {
                assert!(result.get("result").is_none());
            }
        }
    }
    let id = manager
        .submit("业务失败对照", |_| async {
            Ok(json!({"success":false,"message":"恢复接口未成功"}))
        })
        .await
        .unwrap();
    let result = finished(&manager, &id).await;
    assert_eq!(result["state"], "failed");
    assert_eq!(result["result_status"], 200);
    assert_eq!(result["result"]["success"], false);
}
