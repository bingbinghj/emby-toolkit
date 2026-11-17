use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::SessionSigner, error::ApiError, logs::LogStore, strm};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};

struct Fixture {
    state: AppState,
    settings: Arc<Mutex<Value>>,
    reject_writes: Arc<AtomicBool>,
    lose_temporary_response: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(settings: Value) -> Self {
        let settings = Arc::new(Mutex::new(settings));
        let reject_writes = Arc::new(AtomicBool::new(false));
        let reject = reject_writes.clone();
        let lose_temporary_response = Arc::new(AtomicBool::new(false));
        let lose = lose_temporary_response.clone();
        let router = Router::new()
            .route("/web/ConfigurationPages", get(|| async {
                Json(json!([{"Name":"MediaInfoExtractPageView","Href":"UI/View?PageId=fixture"}]))
            }))
            .route("/UI/View", get(|State(settings): State<Arc<Mutex<Value>>>| async move {
                Json(json!({"EditObjectContainer":{"Object":settings.lock().await.clone()}}))
            }))
            .route("/UI/Command", post(move |State(settings): State<Arc<Mutex<Value>>>, Json(body): Json<Value>| {
                let reject = reject.clone();
                let lose = lose.clone();
                async move {
                if reject.load(Ordering::SeqCst) { return StatusCode::UNPROCESSABLE_ENTITY; }
                *settings.lock().await = serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
                if lose.load(Ordering::SeqCst) && settings.lock().await["PersistMediaInfoMode"] == "Restore" {
                    return StatusCode::UNPROCESSABLE_ENTITY;
                }
                StatusCode::NO_CONTENT
            }}))
            .route("/ScheduledTasks", get(|| async {
                Json(json!([
                    {"Key":"MediaInfoExtractTask","State":"Idle"},
                    {"Key":"IntroFingerprintExtractTask","State":"Idle"},
                    {"Key":"VideoThumbnailExtractTask","State":"Idle"}
                ]))
            }))
            .with_state(settings.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let directory = tempfile::tempdir().unwrap();
        let state = AppState {
            // This fixture exercises only the HTTP settings contract. A lazy
            // pool on an unused port makes accidental database access fail.
            db: sqlx::postgres::PgPoolOptions::new()
                .acquire_timeout(Duration::from_millis(100))
                .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
                .unwrap(),
            config: Arc::new(RwLock::new(
                json!({"emby_server_url":base,"emby_api_key":"fixture"}),
            )),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            sessions: Arc::new(SessionSigner::load(directory.path()).unwrap()),
            recovery_tokens: Arc::default(),
            data_dir: directory.path().into(),
            notifications: Arc::default(),
            webhooks: Arc::default(),
            ai: Arc::default(),
            jobs: Arc::default(),
            logs: LogStore::open(&directory.path().join("logs")).unwrap(),
            p115: Arc::default(),
            strm: Arc::default(),
            douban: Arc::default(),
            docker: Arc::new(emby_toolkit::docker_host::Service { docker: None }),
        };
        Self {
            state,
            settings,
            reject_writes,
            lose_temporary_response,
            _directory: directory,
            server,
        }
    }

    async fn close(self) {
        self.server.abort();
        self.state.db.close().await;
    }

    async fn recovery_record(
        &self,
        original: &Value,
        applied: Option<&Value>,
    ) -> std::path::PathBuf {
        let directory = self
            .state
            .data_dir
            .join("backups/media-info-reload/settings-retry");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        tokio::fs::write(
            directory.join("original-settings.json"),
            serde_json::to_vec(original).unwrap(),
        )
        .await
        .unwrap();
        if let Some(applied) = applied {
            tokio::fs::write(
                directory.join("applied-settings.json"),
                serde_json::to_vec(applied).unwrap(),
            )
            .await
            .unwrap();
        }
        tokio::fs::write(
            directory.join("pending-scan.json"),
            serde_json::to_vec(&json!({
                "kind":"native-sync", "verified":true, "ids":["fixture-item"]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        directory
    }
}

#[tokio::test]
async fn a_lost_apply_receipt_still_rolls_back_the_remote_change() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"});
    let f = Fixture::new(original.clone()).await;
    f.lose_temporary_response.store(true, Ordering::SeqCst);
    let result = strm::with_changes(
        &f.state,
        &json!({"PersistMediaInfoMode":"Restore"}),
        || async {
            panic!("the body cannot start without the apply receipt");
            #[allow(unreachable_code)]
            Ok(())
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(*f.settings.lock().await, original);
    assert!(
        !f.state
            .data_dir
            .join("strm-settings-recovery.json")
            .exists()
    );
    f.close().await;
}

#[tokio::test]
async fn background_recovery_retries_after_the_plugin_becomes_reachable() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange","EnableImageCapture":false});
    let f = Fixture::new(original.clone()).await;
    let result: Result<(), ApiError> =
        strm::with_changes(&f.state, &json!({"EnableImageCapture":true}), || async {
            f.reject_writes.store(true, Ordering::SeqCst);
            Err(ApiError::bad("temporary outage"))
        })
        .await;
    assert!(result.is_err());
    let stop = tokio_util::sync::CancellationToken::new();
    let runner = tokio::spawn(strm::run_recovery(f.state.clone(), stop.clone()));
    tokio::task::yield_now().await;
    f.reject_writes.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(7), async {
        while f
            .state
            .data_dir
            .join("strm-settings-recovery.json")
            .exists()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    stop.cancel();
    runner.await.unwrap();
    assert_eq!(*f.settings.lock().await, original);
    f.close().await;
}

#[tokio::test]
async fn temporary_settings_hard_kill_child() {
    let Ok(directory) = std::env::var("ETK_SETTINGS_CHILD_DIRECTORY") else {
        return;
    };
    let base = std::env::var("ETK_SETTINGS_CHILD_SERVER").unwrap();
    let mut f = Fixture::new(json!({})).await;
    f.state.data_dir = directory.into();
    f.state.config.write().await["emby_server_url"] = json!(base);
    let _: Result<(), ApiError> = strm::with_settings(&f.state, "restore_media_info", || async {
        std::fs::write(f.state.data_dir.join("child-applied"), b"ready").unwrap();
        std::future::pending().await
    })
    .await;
}

#[tokio::test]
async fn hard_kill_restores_settings_before_paused_or_finished_jobs_resume() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"});
    let f = Fixture::new(original.clone()).await;
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "temporary_settings_hard_kill_child",
            "--exact",
            "--nocapture",
        ])
        .env("ETK_SETTINGS_CHILD_DIRECTORY", &f.state.data_dir)
        .env(
            "ETK_SETTINGS_CHILD_SERVER",
            f.state.config.read().await["emby_server_url"]
                .as_str()
                .unwrap(),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.state.data_dir.join("child-applied").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    child.kill().await.unwrap();
    assert_eq!(f.settings.lock().await["PersistMediaInfoMode"], "Restore");
    assert!(
        f.state
            .data_dir
            .join("strm-settings-recovery.json")
            .is_file()
    );
    let mut restarted = f.state.clone();
    restarted.strm = Arc::default();
    strm::recover_pending(&restarted).await.unwrap();
    assert_eq!(*f.settings.lock().await, original);
    assert!(
        !f.state
            .data_dir
            .join("strm-settings-recovery.json")
            .exists()
    );
    f.close().await;
}

#[tokio::test]
async fn temporary_settings_restore_after_error_and_panic() {
    for panic in [false, true] {
        let original = json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"});
        let f = Fixture::new(original.clone()).await;
        let result: Result<(), ApiError> =
            strm::with_settings(&f.state, "restore_media_info", || async {
                assert!(
                    f.state
                        .data_dir
                        .join("strm-settings-recovery.json")
                        .is_file()
                );
                assert_eq!(f.settings.lock().await["PersistMediaInfoMode"], "Restore");
                if panic {
                    panic!("fixture panic after temporary mode applied");
                }
                Err(ApiError::bad("fixture operation failure"))
            })
            .await;
        assert!(result.is_err());
        assert_eq!(*f.settings.lock().await, original);
        assert!(
            !f.state
                .data_dir
                .join("strm-settings-recovery.json")
                .exists()
        );
        f.close().await;
    }
}

#[tokio::test]
async fn aborting_a_future_restores_the_durable_original_without_a_job_result() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"});
    let f = Fixture::new(original.clone()).await;
    let state = f.state.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        strm::with_settings(&state, "restore_media_info", || async move {
            started.send(()).unwrap();
            std::future::pending::<Result<(), ApiError>>().await
        })
        .await
    });
    ready.await.unwrap();
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while f
            .state
            .data_dir
            .join("strm-settings-recovery.json")
            .exists()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*f.settings.lock().await, original);
    f.close().await;
}

#[tokio::test]
async fn failed_restore_survives_service_restart_and_blocks_a_new_temporary_baseline() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange","PersistMediaInfoMode":"Default"});
    let f = Fixture::new(original.clone()).await;
    let _: Result<(), ApiError> = strm::with_changes(
        &f.state,
        &json!({"PersistMediaInfoMode":"Restore"}),
        || async {
            f.reject_writes.store(true, Ordering::SeqCst);
            Err(ApiError::bad("fixture network unavailable during rollback"))
        },
    )
    .await;
    let path = f.state.data_dir.join("strm-settings-recovery.json");
    let persisted = std::fs::read(&path).unwrap();
    let forbidden = strm::with_changes(&f.state, &json!({"EnableImageCapture":true}), || async {
        Ok(())
    })
    .await;
    assert!(forbidden.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), persisted);
    assert_eq!(f.settings.lock().await["PersistMediaInfoMode"], "Restore");
    let mut restarted = f.state.clone();
    restarted.strm = Arc::default();
    // The old instance's one-shot fallback finishes with the service still down.
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.reject_writes.store(false, Ordering::SeqCst);
    strm::recover_pending(&restarted).await.unwrap();
    assert_eq!(*f.settings.lock().await, original);
    assert!(!path.exists());
    f.close().await;
}

fn feature_set(settings: &Value) -> BTreeSet<String> {
    settings["ExclusiveControlFeatures"]
        .as_str()
        .unwrap()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn nullable_legacy_features_restore_without_leaving_a_journal() {
    let original = json!({"ExclusiveControlFeatures":"ExtractOnFileChange"});
    let f = Fixture::new(original.clone()).await;
    let result: Result<(), ApiError> = strm::with_changes(
        &f.state,
        &json!({"ExclusiveControlFeatures":null}),
        || async {
            assert!(f.settings.lock().await["ExclusiveControlFeatures"].is_null());
            Err(ApiError::bad("failed after nullable feature change"))
        },
    )
    .await;
    assert!(result.unwrap_err().1.contains("nullable feature"));
    assert_eq!(*f.settings.lock().await, original);
    assert!(
        !f.state
            .data_dir
            .join("strm-settings-recovery.json")
            .exists()
    );
    f.close().await;
}

#[tokio::test]
async fn settings_cleanup_preserves_external_feature_additions_and_deletions() {
    let fixture =
        Fixture::new(json!({"ExclusiveControlFeatures":"ExtractOnFileChange,NoIntroProtect"}))
            .await;
    let settings = fixture.settings.clone();
    strm::with_settings(&fixture.state, "preserve", || async move {
        let mut current = settings.lock().await;
        assert_eq!(
            feature_set(&current),
            BTreeSet::from(["IgnoreFileChange".into(), "NoIntroProtect".into()])
        );
        current["ExclusiveControlFeatures"] = json!("IgnoreFileChange,IgnoreExtSubChange");
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        feature_set(&*fixture.settings.lock().await),
        BTreeSet::from(["ExtractOnFileChange".into(), "IgnoreExtSubChange".into()])
    );
    fixture.close().await;
}

#[tokio::test]
async fn custom_settings_cleanup_keeps_external_scalar_edits() {
    let fixture = Fixture::new(json!({"ExclusiveControlFeatures":"NONE", "CustomEncoderPath":"original", "EnableImageCapture":false})).await;
    let settings = fixture.settings.clone();
    strm::with_changes(
        &fixture.state,
        &json!({"CustomEncoderPath":"temporary", "EnableImageCapture":true}),
        || async move {
            let mut current = settings.lock().await;
            assert_eq!(current["CustomEncoderPath"], "temporary");
            current["CustomEncoderPath"] = json!("operator-selection");
            Ok(())
        },
    )
    .await
    .unwrap();
    let current = fixture.settings.lock().await.clone();
    assert_eq!(current["CustomEncoderPath"], "operator-selection");
    assert_eq!(current["EnableImageCapture"], false);
    fixture.close().await;
}

#[tokio::test]
async fn failed_operations_also_preserve_external_settings_during_cleanup() {
    let fixture = Fixture::new(
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange", "EnableImageCapture":false}),
    )
    .await;
    let settings = fixture.settings.clone();
    let result: Result<(), ApiError> = strm::with_changes(
        &fixture.state,
        &json!({"EnableImageCapture":true}),
        || async move {
            let mut current = settings.lock().await;
            current["ExclusiveControlFeatures"] = json!("ExtractOnFileChange,IgnoreExtSubChange");
            Err(ApiError::bad("fixture operation failed"))
        },
    )
    .await;
    assert!(result.unwrap_err().1.contains("fixture operation failed"));
    let current = fixture.settings.lock().await.clone();
    assert_eq!(current["EnableImageCapture"], false);
    assert_eq!(
        feature_set(&current),
        BTreeSet::from(["ExtractOnFileChange".into(), "IgnoreExtSubChange".into()])
    );
    fixture.close().await;
}

#[tokio::test]
async fn recovery_cleanup_uses_retained_applied_values_and_keeps_external_edits() {
    let fixture = Fixture::new(json!({
        "ExclusiveControlFeatures":"IgnoreFileChange,IgnoreExtSubChange", "PersistMediaInfoMode":"None"
    })).await;
    let directory = fixture.recovery_record(
        &json!({"ExclusiveControlFeatures":"ExtractOnFileChange,NoIntroProtect", "PersistMediaInfoMode":"Default"}),
        Some(&json!({"ExclusiveControlFeatures":"IgnoreFileChange,NoIntroProtect", "PersistMediaInfoMode":"Restore"})),
    ).await;
    strm::with_settings(&fixture.state, "preserve", || async { Ok(()) })
        .await
        .unwrap();
    let current = fixture.settings.lock().await.clone();
    assert_eq!(current["PersistMediaInfoMode"], "None");
    assert_eq!(
        feature_set(&current),
        BTreeSet::from(["IgnoreExtSubChange".into(), "ExtractOnFileChange".into()])
    );
    assert!(!directory.join("pending-scan.json").exists());
    fixture.close().await;
}

#[tokio::test]
async fn recovery_keeps_legacy_records_compatible_and_does_not_restore_unchanged_fields() {
    for applied in [None, Some(json!({}))] {
        let current = if applied.is_some() {
            json!({"ExclusiveControlFeatures":"IgnoreExtSubChange", "PersistMediaInfoMode":"None"})
        } else {
            json!({"ExclusiveControlFeatures":"IgnoreFileChange", "PersistMediaInfoMode":"Restore"})
        };
        let fixture = Fixture::new(current.clone()).await;
        let original = if applied.is_some() {
            // The guarded operation was already configured and applied no
            // changes. Keep the administrator's subsequent edits.
            json!({"ExclusiveControlFeatures":"IgnoreFileChange", "PersistMediaInfoMode":"Restore"})
        } else {
            json!({"ExclusiveControlFeatures":"ExtractOnFileChange", "PersistMediaInfoMode":"Default"})
        };
        let directory = fixture.recovery_record(&original, applied.as_ref()).await;
        strm::with_settings(&fixture.state, "preserve", || async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(
            *fixture.settings.lock().await,
            if applied.is_some() { current } else { original }
        );
        assert!(!directory.join("pending-scan.json").exists());
        fixture.close().await;
    }
}

#[tokio::test]
async fn recovery_rejects_missing_or_inconsistent_temporary_deltas() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange", "PersistMediaInfoMode":"Default"});
    let current =
        json!({"ExclusiveControlFeatures":"IgnoreFileChange", "PersistMediaInfoMode":"Restore"});
    for applied in [
        json!({}),
        json!({"ExclusiveControlFeatures":"IgnoreFileChange"}),
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange", "PersistMediaInfoMode":"Restore"}),
        json!({"ExclusiveControlFeatures":"IgnoreFileChange", "PersistMediaInfoMode":"None"}),
    ] {
        let fixture = Fixture::new(current.clone()).await;
        let directory = fixture.recovery_record(&original, Some(&applied)).await;
        let result: Result<(), ApiError> =
            strm::with_settings(&fixture.state, "preserve", || async { Ok(()) }).await;
        assert!(
            result.is_err(),
            "invalid task delta unexpectedly recovered: {applied}"
        );
        assert_eq!(*fixture.settings.lock().await, current);
        assert!(directory.join("pending-scan.json").exists());
        fixture.close().await;
    }
}

#[tokio::test]
async fn invalid_applied_recovery_proof_never_changes_settings() {
    let original =
        json!({"ExclusiveControlFeatures":"ExtractOnFileChange", "PersistMediaInfoMode":"Default"});
    let fixture = Fixture::new(original.clone()).await;
    let directory = fixture
        .recovery_record(&original, Some(&json!({"PersistMediaInfoMode":42})))
        .await;
    let result: Result<(), ApiError> =
        strm::with_settings(&fixture.state, "preserve", || async { Ok(()) }).await;
    assert!(result.unwrap_err().1.contains("本任务临时配置恢复说明无效"));
    assert_eq!(*fixture.settings.lock().await, original);
    assert!(directory.join("pending-scan.json").exists());
    fixture.close().await;
}
