use super::*;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

fn path(state: &AppState) -> PathBuf {
    state.data_dir.join("strm-settings-recovery.json")
}

async fn connection(state: &AppState) -> String {
    let config = state.config.read().await;
    let value = json!([config["emby_server_url"], config["emby_api_key"]]);
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

// The settings mutex serializes leases. This independent journal survives a
// failed media verification, cancelled future, or deletion of media backups.
pub(super) struct Lease {
    state: AppState,
    pending: bool,
}

impl Lease {
    pub(super) async fn begin(
        state: &AppState,
        page: &str,
        original: &Value,
        desired: &Value,
    ) -> Result<Self> {
        let applied = original
            .as_object()
            .unwrap()
            .keys()
            .map(|key| (key.clone(), desired[key].clone()))
            .collect::<serde_json::Map<_, _>>();
        let pending = !applied.is_empty();
        if pending {
            if tokio::fs::try_exists(path(state)).await? {
                return Err(ApiError::bad("前次神医临时设置尚未还原，保留最初原配置"));
            }
            let record = json!({"version":1,"page_id":page,
                "connection_sha256":connection(state).await,
                "original":original,"applied":applied});
            let temporary = state.data_dir.join("strm-settings-recovery.tmp");
            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary).await?;
            file.write_all(&serde_json::to_vec(&record).unwrap())
                .await?;
            file.sync_all().await?;
            tokio::fs::rename(&temporary, path(state)).await?;
            tokio::fs::File::open(&state.data_dir)
                .await?
                .sync_all()
                .await?;
        }
        Ok(Self {
            state: state.clone(),
            pending,
        })
    }

    pub(super) async fn restore(&mut self) -> Result<()> {
        if self.pending {
            recover_locked(&self.state).await?;
            self.pending = false;
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.pending
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let state = self.state.clone();
            runtime.spawn(async move {
                if let Err(error) = recover_pending(&state).await {
                    tracing::warn!("神医原设置还原未完成，已保留记录自动重试：{}", error.1);
                }
            });
        }
    }
}

pub(super) async fn recover_locked(state: &AppState) -> Result<()> {
    let journal = path(state);
    if !tokio::fs::try_exists(&journal).await? {
        return Ok(());
    }
    let record = crate::media_recovery::read_json(&journal).await?;
    let valid_fields = record["original"].as_object().is_some_and(|fields| {
        !fields.is_empty()
            && fields.iter().all(|(key, value)| {
                let applied = &record["applied"][key];
                match key.as_str() {
                    "ExclusiveControlFeatures" => {
                        (value.is_string() || value.is_null())
                            && (applied.is_string() || applied.is_null())
                    }
                    "PersistMediaInfoMode" | "LibraryScope" | "CustomEncoderPath" => {
                        (value.is_string() || value.is_null()) && applied.is_string()
                    }
                    "EnableImageCapture" | "IncludeExtra" => {
                        value.is_boolean() && applied.is_boolean()
                    }
                    _ => false,
                }
            })
            && record["applied"]
                .as_object()
                .is_some_and(|values| values.len() == fields.len())
    });
    if record["version"] != 1
        || !valid_fields
        || record["page_id"].as_str().is_none_or(str::is_empty)
        || record["connection_sha256"] != connection(state).await
    {
        return Err(ApiError::bad(
            "神医临时设置还原记录或服务器不匹配，未覆盖当前设置",
        ));
    }
    // A custom encoder or scheduled restore must not be replaced while a
    // native scheduled task is still using it. The journal remains retryable.
    if (record["applied"].get("CustomEncoderPath").is_some()
        || record["applied"].get("PersistMediaInfoMode").is_some())
        && !media_tasks_idle(&state.emby(Method::GET, "ScheduledTasks", &[], None).await?)
    {
        return Err(ApiError::bad(
            "神医媒体任务尚未结束，原设置将在任务结束后自动还原",
        ));
    }
    if !restore_settings(
        state,
        record["page_id"].as_str().unwrap(),
        &record["original"],
        Some(&record["applied"]),
        "临时设置收尾",
        None,
    )
    .await
    {
        return Err(ApiError::bad(
            "神医原设置还原暂未成功，已保留原值等待自动重试",
        ));
    }
    tokio::fs::remove_file(journal).await?;
    tokio::fs::File::open(&state.data_dir)
        .await?
        .sync_all()
        .await?;
    Ok(())
}

pub async fn recover_pending(state: &AppState) -> Result<()> {
    let _gate = state.strm.gate.lock().await;
    recover_locked(state).await
}

pub async fn run_recovery(state: AppState, stop: tokio_util::sync::CancellationToken) {
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            result = recover_pending(&state) => {
                if let Err(error) = result {
                    tracing::warn!("神医原设置自动还原待重试：{}", error.1);
                }
            }
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
    }
}
