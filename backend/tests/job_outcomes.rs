use emby_toolkit::{error::Result, jobs::JobManager};
use serde_json::{Value, json};
use std::{future::Ready, sync::Arc, time::Duration};
use tokio::sync::oneshot;

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
    .expect("任务必须结束并释放运行位置")
}

#[tokio::test]
async fn response_outcomes_preserve_failures_warnings_and_clean_success() {
    let manager = Arc::new(JobManager::default());
    for (status, value, expected) in [
        (207, json!({}), "completed_with_warnings"),
        (200, json!({"error":"恢复尚未确认"}), "failed"),
        (
            200,
            json!({"warnings":0,"error":"","warning_details":["缓存未回写"]}),
            "completed_with_warnings",
        ),
        (503, json!({}), "failed"),
        (200, json!({"success":false}), "failed"),
        (200, json!({"failed":1}), "failed"),
        (207, json!({"failures":["部分项目失败"]}), "failed"),
        (200, json!({"warnings":1,"error":"恢复失败"}), "failed"),
        (200, json!({"warnings":1}), "completed_with_warnings"),
        (
            200,
            json!({"success":true,"failed":0,"failures":[],"error":"","warnings":0,"warning_details":[]}),
            "completed",
        ),
    ] {
        let response = value.clone();
        let id = manager
            .submit_response("任务结果分类", move |_| async move {
                Ok((status, response))
            })
            .await
            .unwrap();
        let result = finished(&manager, &id).await;
        assert_eq!(result["state"], expected, "{result}");
        assert_eq!(result["result_status"], status);
        assert_eq!(result["result"], value);
        if expected == "completed_with_warnings" {
            assert!(result["message"].as_str().unwrap().contains("提醒"));
        }
    }
}

#[tokio::test]
async fn cancellation_uses_the_same_result_signals_and_preserves_details() {
    let manager = Arc::new(JobManager::default());
    for (status, value, warning) in [
        (207, json!({"message":"同步未确认"}), true),
        (200, json!({"error":"恢复失败"}), true),
        (200, json!({"warning_details":["配置恢复未确认"]}), true),
        (200, json!({"error":"","warning_details":[]}), false),
    ] {
        let (ready, started) = oneshot::channel();
        let response = value.clone();
        let id = manager
            .submit_response("取消收尾分类", move |context| async move {
                context.update(25, "等待收尾", "等待上游结束").await;
                ready.send(()).unwrap();
                context.cancelled().await;
                Ok((status, response))
            })
            .await
            .unwrap();
        started.await.unwrap();
        manager.control(Some(&id), "stop").await.unwrap();
        let result = finished(&manager, &id).await;
        assert_eq!(result["state"], "cancelled");
        assert_eq!(result["progress"], 25);
        assert_eq!(result["result_status"], status);
        assert_eq!(result["result"], value);
        assert_eq!(
            result["message"].as_str().unwrap().contains("提醒"),
            warning,
            "{result}"
        );
    }
}

#[tokio::test]
async fn synchronous_future_creation_panic_finishes_and_allows_next_job() {
    let manager = Arc::new(JobManager::default());
    let id = manager
        .submit_response("创建future异常", |_| -> Ready<Result<(u16, Value)>> {
            panic!("future尚未创建");
        })
        .await
        .unwrap();
    let result = finished(&manager, &id).await;
    assert_eq!(result["state"], "failed");
    assert_eq!(result["result_status"], 500);
    assert_eq!(result["result"]["error"], "任务执行异常");
    let next = manager
        .submit("异常后的任务", |_| async {
            Ok(json!({"message":"后续任务完成"}))
        })
        .await
        .expect("创建阶段panic后必须释放active");
    assert_eq!(finished(&manager, &next).await["state"], "completed");
}

#[tokio::test]
async fn future_execution_panic_also_finishes_and_allows_next_job() {
    let manager = Arc::new(JobManager::default());
    let id = manager
        .submit_response("执行future异常", |_| async {
            tokio::task::yield_now().await;
            panic!("future执行异常");
        })
        .await
        .unwrap();
    assert_eq!(finished(&manager, &id).await["state"], "failed");
    let next = manager
        .submit("执行异常后的任务", |_| async { Ok(json!({})) })
        .await
        .unwrap();
    assert_eq!(finished(&manager, &next).await["state"], "completed");
}
