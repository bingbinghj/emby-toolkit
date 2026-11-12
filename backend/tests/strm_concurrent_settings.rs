use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use emby_toolkit::{AppState, auth::SessionSigner, error::ApiError, logs::LogStore, strm};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::{Mutex, RwLock};

struct Fixture {
    state: AppState,
    settings: Arc<Mutex<Value>>,
    _directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(settings: Value) -> Self {
        let settings = Arc::new(Mutex::new(settings));
        let router = Router::new()
            .route("/web/ConfigurationPages", get(|| async {
                Json(json!([{"Name":"MediaInfoExtractPageView","Href":"UI/View?PageId=fixture"}]))
            }))
            .route("/UI/View", get(|State(settings): State<Arc<Mutex<Value>>>| async move {
                Json(json!({"EditObjectContainer":{"Object":settings.lock().await.clone()}}))
            }))
            .route("/UI/Command", post(|State(settings): State<Arc<Mutex<Value>>>, Json(body): Json<Value>| async move {
                *settings.lock().await = serde_json::from_str(body["Data"].as_str().unwrap()).unwrap();
                StatusCode::NO_CONTENT
            }))
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
