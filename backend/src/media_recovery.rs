use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    state::string,
};
use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
};

struct Record {
    id: String,
    directory: PathBuf,
    kind: &'static str,
    target_ids: Value,
    reason: String,
    can_recover: bool,
}

pub(crate) async fn read_json(path: &Path) -> Result<Value> {
    if !tokio::fs::symlink_metadata(path).await?.is_file() {
        return Err(ApiError::bad(format!(
            "恢复材料不是普通文件：{}",
            path.display()
        )));
    }
    serde_json::from_slice(&tokio::fs::read(path).await?)
        .map_err(|_| ApiError::bad(format!("恢复材料 JSON 无效：{}", path.display())))
}

fn cleanup_marker(directory: &Path) -> Result<PathBuf> {
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ApiError::bad("恢复目录名称无效，备份仍保留"))?;
    Ok(directory.with_file_name(format!("{name}.cleanup.json")))
}

async fn cleanup_manifest(directory: &Path, marker: &Path) -> Result<Value> {
    for ancestor in directory.ancestors() {
        match tokio::fs::symlink_metadata(ancestor).await {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(ApiError::bad(
                    "待清理备份或父目录不是原普通目录，未删除备份",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let value = read_json(marker).await?;
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let ids = value["target_ids"].as_array();
    if value["kind"] != "verified-cleanup"
        || value["version"] != 1
        || value["directory_name"] != name
        || !ids.is_some_and(|ids| {
            !ids.is_empty()
                && ids.iter().all(|id| {
                    id.as_str().is_some_and(|id| {
                        !id.is_empty()
                            && id.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                            })
                    })
                })
        })
        || value["completed_at"]
            .as_str()
            .is_none_or(|time| chrono::DateTime::parse_from_rfc3339(time).is_err())
    {
        return Err(ApiError::bad("已验收备份的清理记录无效，未删除备份"));
    }
    Ok(value)
}

async fn mark_verified(directory: &Path, target_ids: &Value) -> Result<PathBuf> {
    let marker = cleanup_marker(directory)?;
    let stage = marker.with_file_name(format!(
        ".cleanup-stage-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    let value = json!({"kind":"verified-cleanup", "version":1, "directory_name":directory.file_name().and_then(|name| name.to_str()),
        "target_ids":target_ids, "completed_at":chrono::Utc::now().to_rfc3339()});
    tokio::fs::write(&stage, serde_json::to_vec(&value).unwrap()).await?;
    tokio::fs::File::open(&stage).await?.sync_all().await?;
    tokio::fs::rename(&stage, &marker).await?;
    tokio::fs::File::open(directory.parent().unwrap())
        .await?
        .sync_all()
        .await?;
    Ok(marker)
}

async fn cleanup_verified_with<F, Fut>(directory: &Path, marker: &Path, remove: F) -> Result<()>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = std::io::Result<()>>,
{
    cleanup_manifest(directory, marker).await?;
    match remove(directory.to_path_buf()).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ApiError::bad(format!(
                "媒体已恢复，备份未删除：{error}；请重试清理 {}",
                directory.display()
            )));
        }
    }
    if tokio::fs::try_exists(directory).await? {
        return Err(ApiError::bad("媒体已恢复，备份未删除，请重试清理"));
    }
    let parent = directory.parent().unwrap();
    let finish = async {
        tokio::fs::File::open(parent).await?.sync_all().await?;
        tokio::fs::remove_file(marker).await?;
        tokio::fs::File::open(parent).await?.sync_all().await
    }
    .await;
    if let Err(error) = finish {
        return Err(ApiError::bad(format!(
            "媒体已恢复，备份清理记录未删除：{error}；请重试清理"
        )));
    }
    Ok(())
}

async fn cleanup_verified(directory: &Path, marker: &Path) -> Result<()> {
    cleanup_verified_with(directory, marker, tokio::fs::remove_dir_all).await
}

async fn records(state: &AppState) -> Result<Vec<Record>> {
    let mut records = vec![];
    for (prefix, root) in [
        ("media", state.data_dir.join("backups/media-info-reload")),
        ("rename", state.data_dir.join("rename-recovery")),
    ] {
        let mut entries = match tokio::fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(ApiError::bad(format!("无法读取恢复目录：{e}"))),
        };
        while let Some(entry) = entries.next_entry().await? {
            let directory = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(directory_name) = name.strip_suffix(".cleanup.json") {
                if matches!(directory_name, "" | "." | "..") {
                    return Err(ApiError::bad("备份清理记录的目录名称无效，未删除备份"));
                }
                let restored = root.join(directory_name);
                let material = cleanup_manifest(&restored, &directory).await;
                records.push(Record {
                    id: format!("{prefix}:{directory_name}"),
                    directory: restored,
                    kind: "verified-cleanup",
                    target_ids: material
                        .as_ref()
                        .map(|value| value["target_ids"].clone())
                        .unwrap_or(json!([])),
                    reason: material
                        .as_ref()
                        .map(|_| "媒体及原配置已验收，备份清理尚未完成，可重试删除".into())
                        .unwrap_or_else(|error| error.1.clone()),
                    can_recover: material.is_ok(),
                });
                continue;
            }
            if tokio::fs::try_exists(cleanup_marker(&directory)?).await? {
                continue;
            }
            if !entry.file_type().await?.is_dir() {
                if entry.file_type().await?.is_symlink() {
                    records.push(Record {
                        id: format!("{prefix}:{name}"),
                        directory,
                        kind: "unknown",
                        target_ids: json!([]),
                        reason: "恢复目录是符号链接，请核对原备份".into(),
                        can_recover: false,
                    });
                }
                continue;
            }
            let kind = if tokio::fs::try_exists(directory.join("media-reload.json")).await? {
                "media-reload"
            } else if tokio::fs::try_exists(directory.join("image-capture.json")).await? {
                "image-capture"
            } else if tokio::fs::try_exists(directory.join("pending-scan.json")).await? {
                "pending-scan"
            } else {
                continue;
            };
            let material = read_json(&directory.join(format!("{kind}.json"))).await;
            let original = read_json(&directory.join("original-settings.json")).await;
            let mut reason = match kind {
                "media-reload" => "媒体信息重载尚未完成，可重试恢复并验收".to_string(),
                "pending-scan" => "媒体刷新或原配置恢复尚未完成，可重试验收".to_string(),
                _ => "截图恢复记录需核对原备份，尚不能自动收尾".to_string(),
            };
            let mut can_recover = kind != "image-capture";
            if kind == "pending-scan"
                && material.as_ref().is_ok_and(|v| v["verification"].is_null())
            {
                can_recover = false;
                reason =
                    "该旧记录缺少完整媒体验收材料，不能仅凭刷新完成标记解除保护，请核对原恢复目录"
                        .into();
            }
            for result in [&material, &original] {
                if let Err(error) = result {
                    can_recover = false;
                    reason = error.1.clone();
                }
            }
            if let Ok(original) = &original
                && !crate::strm::valid_recovery_settings(original)
            {
                can_recover = false;
                reason = "原配置恢复说明无效，未修改神医配置".into();
            }
            let target_ids = material
                .as_ref()
                .ok()
                .map(|v| {
                    if kind == "media-reload" {
                        v["target_ids"].clone()
                    } else {
                        v["ids"].clone()
                    }
                })
                .unwrap_or(json!([]));
            records.push(Record {
                id: format!("{prefix}:{name}"),
                directory,
                kind,
                target_ids,
                reason,
                can_recover,
            });
        }
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}

pub async fn status(State(state): State<AppState>) -> Result<Json<Value>> {
    let records = records(&state).await?;
    Ok(Json(
        json!({"pending":!records.is_empty(),"records":records.iter().map(|r|json!({
        "id":r.id,"directory":r.directory,"target_ids":r.target_ids,"kind":r.kind,
        "reason":r.reason,"can_recover":r.can_recover
    })).collect::<Vec<_>>()}),
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let id = string(&body, "recovery_id");
    if !records(&state).await?.iter().any(|record| record.id == id) {
        return Err(ApiError::bad("未找到该待恢复记录，请重新检查恢复状态"));
    }
    let task = state
        .jobs
        .clone()
        .submit_response("恢复媒体信息", move |context| async move {
            recover(&state, &id, &context).await
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"message":"恢复任务已提交，正在核对媒体与原配置"})),
    ))
}

pub async fn recover(state: &AppState, id: &str, context: &JobContext) -> Result<(u16, Value)> {
    let mut pending = records(state).await?;
    let position = pending
        .iter()
        .position(|record| record.id == id)
        .ok_or_else(|| ApiError::bad("该恢复记录已经处理或不存在，请重新检查"))?;
    let record = pending.remove(position);
    if !record.can_recover {
        return Err(ApiError(StatusCode::CONFLICT, record.reason));
    }
    if record.kind != "verified-cleanup"
        && pending
            .iter()
            .any(|pending| pending.kind != "verified-cleanup" || !pending.can_recover)
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "存在多份待恢复记录，无法确定原配置顺序，请核对恢复目录".into(),
        ));
    }
    context
        .activity(
            "恢复媒体信息",
            &format!("正在核对 {}", record.directory.display()),
        )
        .await;
    let marker = if record.kind == "verified-cleanup" {
        cleanup_marker(&record.directory)?
    } else {
        let original = read_json(&record.directory.join("original-settings.json")).await?;
        crate::strm::finish_media_recovery(state, &original, || async {
            crate::media_reload::recover_record(state, &record.directory, context).await
        })
        .await?;
        // Keep the proof outside the directory: a partial recursive deletion
        // must not remove the only record that allows a cleanup-only retry.
        mark_verified(&record.directory, &record.target_ids).await?
    };
    if let Err(error) = cleanup_verified(&record.directory, &marker).await {
        tracing::warn!("[媒体恢复] 媒体已通过验收，恢复备份清理未完成：{}", error.1);
        let backup_deleted = !tokio::fs::try_exists(&record.directory).await?;
        let pending = tokio::fs::try_exists(&marker).await?;
        return Ok((
            StatusCode::MULTI_STATUS.as_u16(),
            json!({"message":error.1, "recovered_id":id,
            "pending":pending, "backup_deleted":backup_deleted,
            "warnings":1, "warning_details":[error.1]}),
        ));
    }
    Ok((
        StatusCode::OK.as_u16(),
        json!({"message":"媒体恢复及原配置回读验收通过，备份已删除，可以继续保存", "recovered_id":id,"pending":!records(state).await?.is_empty(),
        "backup_deleted":true, "warnings":0,"warning_details":[]}),
    ))
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[tokio::test]
    async fn failed_backup_deletion_retains_a_durable_cleanup_only_retry_record() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("saved-verified");
        tokio::fs::create_dir(&directory).await.unwrap();
        tokio::fs::write(directory.join("requested.json"), b"original backup")
            .await
            .unwrap();
        let marker = mark_verified(&directory, &json!(["61616"])).await.unwrap();
        let error = cleanup_verified_with(&directory, &marker, |_| async {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "fixture deletion denied",
            ))
        })
        .await
        .unwrap_err();
        assert!(error.1.contains("媒体已恢复，备份未删除"));
        assert_eq!(
            tokio::fs::read(directory.join("requested.json"))
                .await
                .unwrap(),
            b"original backup"
        );
        assert_eq!(
            cleanup_manifest(&directory, &marker).await.unwrap()["kind"],
            "verified-cleanup"
        );
        cleanup_verified(&directory, &marker).await.unwrap();
        assert!(!directory.exists());
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn partial_backup_deletion_does_not_destroy_the_cleanup_retry_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("saved-partial");
        tokio::fs::create_dir(&directory).await.unwrap();
        let marker = mark_verified(&directory, &json!(["61616"])).await.unwrap();
        // Emulate deletion of the directory followed by failure before unlinking
        // the sibling completion record (e.g. process interruption).
        tokio::fs::remove_dir(&directory).await.unwrap();
        assert!(cleanup_manifest(&directory, &marker).await.is_ok());
        cleanup_verified(&directory, &marker).await.unwrap();
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn invalid_cleanup_proof_does_not_delete_a_backup() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("saved-invalid");
        tokio::fs::create_dir(&directory).await.unwrap();
        let marker = cleanup_marker(&directory).unwrap();
        tokio::fs::write(&marker, br#"{"verified":true}"#)
            .await
            .unwrap();
        assert!(cleanup_verified(&directory, &marker).await.is_err());
        assert!(directory.exists());
        assert!(marker.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn changed_backup_directory_symlink_cannot_delete_an_external_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("saved-symlink");
        let outside = temporary.path().join("outside");
        tokio::fs::create_dir(&directory).await.unwrap();
        tokio::fs::create_dir(&outside).await.unwrap();
        let marker = mark_verified(&directory, &json!(["61616"])).await.unwrap();
        tokio::fs::write(outside.join("keep.json"), b"unrelated")
            .await
            .unwrap();
        tokio::fs::remove_dir(&directory).await.unwrap();
        std::os::unix::fs::symlink(&outside, &directory).unwrap();
        assert!(cleanup_verified(&directory, &marker).await.is_err());
        assert_eq!(
            tokio::fs::read(outside.join("keep.json")).await.unwrap(),
            b"unrelated"
        );
        assert!(marker.exists());
        let child = outside.join("saved-parent-link");
        tokio::fs::create_dir(&child).await.unwrap();
        let child_marker = mark_verified(&child, &json!(["61616"])).await.unwrap();
        let parent_link = temporary.path().join("parent-link");
        std::os::unix::fs::symlink(&outside, &parent_link).unwrap();
        assert!(
            cleanup_verified(
                &parent_link.join("saved-parent-link"),
                &parent_link.join("saved-parent-link.cleanup.json")
            )
            .await
            .is_err()
        );
        assert!(child.exists());
        assert!(child_marker.exists());
    }
}
