use emby_toolkit::jobs::JobManager;
use serde_json::json;
use std::{sync::Arc, time::Duration};
#[tokio::test]
async fn task_status_matches_original_history_window_order_and_result_separation() {
    let manager = Arc::new(JobManager::default());
    let mut ids = Vec::new();
    for index in 0..31 {
        let id = manager
            .submit("历史保留核对", move |_| async move {
                Ok(json!({"index":index,"message":"完成"}))
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if manager.result(&id).await.unwrap()["is_running"] == false {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        ids.push(id);
    }
    assert!(manager.result(&ids[0]).await.is_none());
    assert!(manager.result(&ids[1]).await.is_some());
    let status = manager.status().await;
    assert!(status.get("result").is_none());
    let history = status["history"].as_array().unwrap();
    assert_eq!(history.len(), 30);
    assert_eq!(history[0]["task_id"], ids[30]);
    assert_eq!(history[29]["task_id"], ids[1]);
    assert!(history.iter().all(|x| x.get("result").is_none()));
    assert_eq!(
        manager.result(&ids[30]).await.unwrap()["result"]["index"],
        30
    );
}
