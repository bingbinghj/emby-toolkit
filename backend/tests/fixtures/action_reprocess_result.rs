extern crate self as emby_toolkit;
include!("../title_reverse_restore.rs");

#[test]
fn http_success_does_not_hide_failed_or_unverified_native_readback() {
    for state in ["failed", "unverified"] {
        let result = json!({
            "message":"媒体信息 JSON 已保存，Emby 刷新或回读尚未确认，请查看提醒",
            "failures":[],
            "warnings":1,
            "warning_details":["61616：Emby 回读仍返回原音轨标题"],
            "metadata_refresh":{"state":state}
        });
        let error = super::ensure_rebuild_completed(200, &result).unwrap_err();
        assert!(
            error.1.contains("61616") && error.1.contains("原音轨标题"),
            "{}",
            error.1
        );
    }
}

#[test]
fn verified_native_readback_with_cleanup_warning_allows_followup_scrape() {
    let result = json!({
        "message":"媒体信息 JSON 已保存，Emby 媒体流、标题和章节已通过回读验收",
        "failures":[],
        "warnings":1,
        "warning_details":["ETK 恢复暂存文件清理失败，已保留恢复备份"],
        "metadata_refresh":{"state":"verified"}
    });
    super::ensure_rebuild_completed(200, &result).unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; Emby is localhost"]
async fn partial_native_restore_keeps_target_failure_and_stops_followup_scrape() {
    let f = Fixture::new(false).await;
    let state = f.state.clone();
    let info = f.original.clone();
    let continued = Arc::new(AtomicBool::new(false));
    let scrape_started = continued.clone();
    let task = f
        .state
        .jobs
        .submit("重建部分结果验收", move |context| async move {
            let body =
                json!({"mediainfo":info,"target_item_ids":["61616","61617"],"stream_patches":[]});
            let rebuilt =
                std::collections::BTreeMap::from([("61616".into(), body["mediainfo"].clone())]);
            let (status, result) = emby_toolkit::media_save::save_with_rebuilt(
                &state, "61616", &body, &rebuilt, &context,
            )
            .await?;
            super::ensure_rebuild_completed(status, &result)?;
            scrape_started.store(true, Ordering::SeqCst);
            Ok(json!({"updated":1,"message":"后续元数据处理成功"}))
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let result = f.state.jobs.result(&task).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let saved: Value = serde_json::from_slice(
        &tokio::fs::read(f.directory.path().join("61616-mediainfo.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let starts = f.host.starts.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(result["state"], "failed", "{result}");
    let error = result["result"]["error"].as_str().unwrap();
    assert!(
        error.contains("HTTP 207") && error.contains("61617") && error.contains("HTTP 404"),
        "{error}"
    );
    assert!(
        !continued.load(Ordering::SeqCst),
        "the partial restore must not be replaced by a successful scrape result"
    );
    assert_eq!(
        starts, 1,
        "the valid target was actually imported before the 207 result"
    );
    assert!(emby_toolkit::media_info::recoverable(&saved));
}
