async fn await_save_result(state: &AppState, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(80), async {
        loop {
            let result = state.jobs.result(id).await.unwrap();
            if result["is_running"] == false {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn restart_interrupted_save(f: &ThumbnailFixture, id: &str) -> (AppState, Value) {
    let mut interrupted = f.native.state.jobs.result(id).await.unwrap();
    f.native.state.jobs.shutdown().await;
    interrupted["state"] = json!("interrupted");
    interrupted["is_running"] = json!(false);
    interrupted["stop_requested"] = json!(false);
    interrupted["pause_requested"] = json!(false);
    interrupted.as_object_mut().unwrap().remove("result");
    // A dropped process never wrote the manager's terminal proof. Keep every
    // business checkpoint, but remove the fixture wrapper's artificial result.
    sqlx::query(
        "DELETE FROM durable_job_steps WHERE job_id=$1 AND step_key='completed:__job_result__'",
    )
    .bind(id)
    .execute(&f.native.state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE durable_jobs SET state_json=$2 WHERE id=$1")
        .bind(id)
        .bind(interrupted)
        .execute(&f.native.state.db)
        .await
        .unwrap();
    let resumed = AppState::new(f.native.state.db.clone(), f.native.state.data_dir.clone())
        .await
        .unwrap();
    resumed
        .jobs
        .clone()
        .resume_pending(resumed.clone(), tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let result = await_save_result(&resumed, id).await;
    (resumed, result)
}

async fn interrupt_save_after_clear(f: &ThumbnailFixture, body: &Value) -> String {
    *f.proxy.protocol_fault.lock().await = "save-clear-held".into();
    let state = f.native.state.clone();
    let requested = body.clone();
    let observed = f.native.host.clone();
    let task = state.jobs.clone().submit_persistent_response(
        "保存半清理重启回归",
        emby_toolkit::jobs::JobSpec::new("media_save", json!({"id":"61616","body":body})),
        move |context| async move {
            tokio::select! {
                result = media_save::save(&state, "61616", &requested, &context) => result,
                _ = async {
                    loop {
                        if observed.clears.load(Ordering::SeqCst) > 0
                            && observed.item.lock().await["MediaStreams"] == json!([])
                            && !observed.directory.join("61616-mediainfo.json").exists() { break; }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                } => Err(emby_toolkit::error::ApiError::bad("fixture: process interrupted after Clear")),
            }
        },
    ).await.unwrap();
    let result = await_save_result(&f.native.state, &task).await;
    assert_eq!(result["state"], "failed", "{result}");
    *f.proxy.protocol_fault.lock().await = String::new();
    f.proxy.sync_release.notify_one();
    task
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs and media files are isolated"]
async fn media_save_resume_after_clear_uses_original_manifest_without_a_second_clear() {
    let (f, mut requested) = file_marker_fixture().await;
    requested[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("重启后继续原保存标题");
    let body = json!({"mediainfo":requested});
    let task = interrupt_save_after_clear(&f, &body).await;
    assert_eq!(f.native.host.clears.load(Ordering::SeqCst), 1);
    let original_recovery = reload_recovery(&f);
    assert!(original_recovery.join("media-reload.json").is_file());
    let (resumed, result) = restart_interrupted_save(&f, &task).await;
    assert!(
        matches!(
            result["state"].as_str(),
            Some("completed" | "completed_with_warnings")
        ),
        "{result}"
    );
    assert_eq!(result["task_id"], task);
    assert_eq!(
        f.native.host.clears.load(Ordering::SeqCst),
        1,
        "resume must not issue another Clear"
    );
    assert_eq!(
        f.native.host.item.lock().await["MediaStreams"][1]["Title"],
        "重启后继续原保存标题"
    );
    assert_eq!(
        *f.native.host.settings.lock().await,
        f.proxy.original_settings
    );
    assert!(!original_recovery.exists());
    resumed.jobs.shutdown().await;
    f.close_with_legacy(true).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs and media files are isolated"]
async fn media_save_resume_after_cleanup_keeps_finished_json_and_reuses_its_result() {
    let f = ThumbnailFixture::with_flag(Some(false)).await;
    let mut requested = f.native.original.clone();
    requested[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("完成检查点原保存标题");
    let (_, receipt) = media_save::submit(
        axum::extract::State(f.native.state.clone()),
        axum::extract::Path("61616".into()),
        axum::Json(json!({"mediainfo":requested})),
    )
    .await
    .unwrap();
    let task = receipt.0["task_id"].as_str().unwrap().to_string();
    let original = await_save_result(&f.native.state, &task).await;
    assert_eq!(original["state"], "completed", "{original}");
    let path = f.native.directory.path().join("61616-mediainfo.json");
    let bytes = tokio::fs::read(&path).await.unwrap();
    let modified = tokio::fs::metadata(&path)
        .await
        .unwrap()
        .modified()
        .unwrap();
    let clears = f.native.host.clears.load(Ordering::SeqCst);
    let settings_writes = f.native.host.config_writes.load(Ordering::SeqCst);
    let (resumed, result) = restart_interrupted_save(&f, &task).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["task_id"], task);
    assert_eq!(result["result"], original["result"]);
    assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
    assert_eq!(
        tokio::fs::metadata(&path)
            .await
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert_eq!(f.native.host.clears.load(Ordering::SeqCst), clears);
    assert_eq!(
        f.native.host.config_writes.load(Ordering::SeqCst),
        settings_writes
    );
    resumed.jobs.shutdown().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs and media files are isolated"]
async fn media_save_resume_keeps_json_changed_after_clear_and_retains_original_recovery() {
    let (f, mut requested) = file_marker_fixture().await;
    requested[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("原请求标题");
    let task = interrupt_save_after_clear(&f, &json!({"mediainfo":requested})).await;
    let recovery = reload_recovery(&f);
    let path = f.native.directory.path().join("61616-mediainfo.json");
    let mut changed = f.native.original.clone();
    changed[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("重启期间用户另存的新标题");
    let current = serde_json::to_vec_pretty(&changed).unwrap();
    tokio::fs::write(&path, &current).await.unwrap();
    let (resumed, result) = restart_interrupted_save(&f, &task).await;
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(tokio::fs::read(&path).await.unwrap(), current);
    assert_eq!(f.native.host.clears.load(Ordering::SeqCst), 1);
    assert!(recovery.is_dir());
    resumed.jobs.shutdown().await;
    f.close_with_legacy(true).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL; all APIs and media files are isolated"]
async fn media_save_legacy_resume_after_clear_restores_before_continuing_without_reclearing() {
    let f = ThumbnailFixture::with_flag(Some(false)).await;
    let mut requested = f.native.original.clone();
    requested[0]["MediaSourceInfo"]["MediaStreams"][1]["Title"] = json!("旧恢复流程的重启标题");
    let body = json!({"mediainfo":requested});
    let owner = json!({"kind":"media_save","id":"61616","body":body});
    *f.proxy.protocol_fault.lock().await = "save-clear-held".into();
    let state = f.native.state.clone();
    let input = body.clone();
    let observed = f.native.host.clone();
    let task = state.jobs.clone().submit_owned("旧保存恢复中断", owner.clone(), None, move |context| async move {
        let rebuilt = std::collections::BTreeMap::from([("61616".into(), input["mediainfo"].clone())]);
        tokio::select! {
            result = media_save::save_with_rebuilt(&state, "61616", &input, &rebuilt, &context) => result.map(|result| result.1),
            _ = async {
                loop {
                    if observed.clears.load(Ordering::SeqCst) > 0
                        && observed.item.lock().await["MediaStreams"] == json!([])
                        && !observed.directory.join("61616-mediainfo.json").exists() { break; }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => Err(emby_toolkit::error::ApiError::bad("fixture: interrupted legacy clear")),
        }
    }).await.unwrap();
    assert_eq!(
        await_save_result(&f.native.state, &task).await["state"],
        "failed"
    );
    *f.proxy.protocol_fault.lock().await = String::new();
    f.proxy.sync_release.notify_one();
    let (resumed, _) = restart_interrupted_save(&f, &task).await;
    assert_eq!(
        f.native.host.item.lock().await["MediaStreams"][1]["Title"],
        "旧恢复流程的重启标题"
    );
    assert_eq!(
        *f.native.host.settings.lock().await,
        f.proxy.original_settings
    );
    let work = resumed.clone();
    let restored_id = resumed
        .jobs
        .clone()
        .submit_owned(
            "旧保存恢复继续",
            owner,
            Some(&task),
            move |context| async move {
                media_save::save_with_rebuilt(
                    &work,
                    "61616",
                    &body,
                    &std::collections::BTreeMap::from([(
                        "61616".into(),
                        body["mediainfo"].clone(),
                    )]),
                    &context,
                )
                .await
                .map(|result| result.1)
            },
        )
        .await
        .unwrap();
    assert_eq!(restored_id, task);
    let result = await_save_result(&resumed, &task).await;
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(f.native.host.clears.load(Ordering::SeqCst), 1);
    resumed.jobs.shutdown().await;
    f.close_with_legacy(true).await;
}
