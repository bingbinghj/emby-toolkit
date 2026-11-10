use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    state::string,
};
use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

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
    if !pending.is_empty() {
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
    {
        let original = read_json(&record.directory.join("original-settings.json")).await?;
        crate::strm::finish_media_recovery(state, &original, || async {
            crate::media_reload::recover_record(state, &record.directory, context).await
        })
        .await?;
        // Remove the blocking markers only after media and the recorded
        // original configuration have both passed readback.
        let pending = record.directory.join("pending-scan.json");
        if tokio::fs::try_exists(&pending).await? {
            tokio::fs::remove_file(pending).await?;
        }
        let marker = record.directory.join("media-reload.json");
        if tokio::fs::try_exists(&marker).await? {
            tokio::fs::remove_file(marker).await?;
        }
        tokio::fs::File::open(&record.directory)
            .await?
            .sync_all()
            .await?;
    }
    let warnings = match tokio::fs::remove_dir_all(&record.directory).await {
        Ok(()) => vec![],
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => vec![format!(
            "媒体与原配置已恢复，ETK恢复暂存清理失败：{error}；仍保留：{}",
            record.directory.display()
        )],
    };
    Ok((
        StatusCode::OK.as_u16(),
        json!({"message":"媒体恢复及原配置回读验收通过，可以继续保存", "recovered_id":id,"pending":false,
        "warnings":warnings.len(),"warning_details":warnings}),
    ))
}
