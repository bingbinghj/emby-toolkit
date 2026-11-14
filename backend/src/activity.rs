use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    state::string,
};
use serde_json::{Value, json};
use std::path::Path;

pub fn clear_database(path: &Path) -> anyhow::Result<Value> {
    if !path.is_file() {
        anyhow::bail!("activitylog.db不存在");
    }
    let backup = path.with_file_name(format!(
        "activitylog.db.bak-etk-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    let conn = rusqlite::Connection::open(path)?;
    let before: i64 = conn.query_row("SELECT count(*) FROM ActivityLog", [], |r| r.get(0))?;
    conn.backup(rusqlite::DatabaseName::Main, &backup, None)?;
    conn.execute("DELETE FROM ActivityLog", [])?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);VACUUM;")?;
    let after: i64 = conn.query_row("SELECT count(*) FROM ActivityLog", [], |r| r.get(0))?;
    for entry in std::fs::read_dir(path.parent().unwrap())? {
        let entry = entry?;
        if entry.path() != backup
            && entry
                .file_name()
                .to_string_lossy()
                .starts_with("activitylog.db.bak-etk-")
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(json!({"before":before,"after":after,"backup":backup}))
}
#[derive(serde::Serialize, serde::Deserialize)]
struct ClearPlan {
    container_id: String,
    container_image: String,
    original_running: bool,
    toolkit_image: String,
    helper_name: String,
}

fn validate_clear_result(result: &Value) -> Result<()> {
    if result["before"].as_i64().is_none_or(|before| before < 0)
        || result["after"].as_i64() != Some(0)
        || !result["backup"]
            .as_str()
            .is_some_and(|path| path.starts_with("/config/data/activitylog.db.bak-etk-"))
    {
        return Err(ApiError::bad("活动日志清理未通过实际结果验收"));
    }
    Ok(())
}

async fn wait_for_helper(docker: &bollard::Docker, id: &str) -> Result<i64> {
    use futures_util::StreamExt;
    let mut status = None;
    let mut waiting =
        docker.wait_container(id, None::<bollard::query_parameters::WaitContainerOptions>);
    while let Some(item) = waiting.next().await {
        status = Some(item?.status_code);
    }
    status.ok_or_else(|| ApiError::bad("活动日志清理辅助进程结束状态未确认"))
}

async fn finish_helper_before_restart(
    docker: &bollard::Docker,
    plan: &ClearPlan,
    context: &JobContext,
) -> Result<()> {
    let saved = context.completed("activity:helper-created").await?;
    let lookup = saved
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or(&plan.helper_name);
    let info = match docker.inspect_container(lookup, None).await {
        Ok(info) => {
            serde_json::to_value(info).map_err(|_| ApiError::bad("清理辅助容器身份无效"))?
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if matches!(
        info["State"]["Status"].as_str(),
        Some("created" | "exited" | "dead")
    ) {
        return Ok(());
    }
    if info["Image"] != plan.toolkit_image
        || info["Config"]["Labels"]["etk.activity-job"] != plan.helper_name
    {
        return Err(ApiError::bad(
            "旧清理辅助进程身份未确认，暂未恢复Emby以防并发修改活动日志",
        ));
    }
    let id = info["Id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::bad("清理辅助容器ID缺失"))?;
    wait_for_helper(docker, id).await?;
    Ok(())
}

async fn helper_result(
    docker: &bollard::Docker,
    plan: &ClearPlan,
    context: &JobContext,
    emby_running: bool,
    allow_start: bool,
) -> Result<Value> {
    use futures_util::StreamExt;
    let saved = context.completed("activity:helper-created").await?;
    let saved = match saved {
        Some(Value::String(id)) if !id.is_empty() => Some(id),
        Some(_) => return Err(ApiError::bad("清理辅助容器恢复ID无效")),
        None => None,
    };
    let lookup = saved.as_deref().unwrap_or(&plan.helper_name);
    let found = match docker.inspect_container(lookup, None).await {
        Ok(info) => Some(info),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => None,
        Err(error) => return Err(error.into()),
    };
    let helper = if let Some(info) = found {
        let info =
            serde_json::to_value(&info).map_err(|_| ApiError::bad("清理辅助容器身份无效"))?;
        if info["Image"] != plan.toolkit_image
            || info["Config"]["Labels"]["etk.activity-job"] != plan.helper_name
            || !info["HostConfig"]["VolumesFrom"]
                .as_array()
                .is_some_and(|volumes| {
                    volumes.iter().any(|volume| {
                        volume.as_str().is_some_and(|volume| {
                            volume == plan.container_id
                                || volume == format!("{}:rw", plan.container_id)
                        })
                    })
                })
        {
            return Err(ApiError::bad("清理辅助容器与原任务不一致，已拒绝复用"));
        }
        info
    } else {
        if saved.is_some() {
            return Err(ApiError::bad(
                "清理辅助容器已丢失且结果未确认，已拒绝重复清空活动日志",
            ));
        }
        if !allow_start {
            return Err(ApiError::bad("原清理辅助进程已不存在，保留任务继续核验"));
        }
        context.checkpoint().await?;
        let created = docker
            .create_container(
                Some(
                    bollard::query_parameters::CreateContainerOptionsBuilder::default()
                        .name(&plan.helper_name)
                        .build(),
                ),
                bollard::models::ContainerCreateBody {
                    image: Some(plan.toolkit_image.clone()),
                    entrypoint: Some(vec!["/app/emby-toolkit".into()]),
                    cmd: Some(vec![
                        "--clear-activity-log".into(),
                        "/config/data/activitylog.db".into(),
                    ]),
                    user: Some("0:0".into()),
                    labels: Some(std::collections::HashMap::from([(
                        "etk.activity-job".into(),
                        plan.helper_name.clone(),
                    )])),
                    host_config: Some(bollard::models::HostConfig {
                        volumes_from: Some(vec![plan.container_id.clone()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let info = docker.inspect_container(&created.id, None).await?;
        serde_json::to_value(info).map_err(|_| ApiError::bad("清理辅助容器身份无效"))?
    };
    let id = helper["Id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::bad("清理辅助容器ID缺失"))?;
    // Retain the helper until its actual result is durable, including a crash
    // after it has cleared the database but before ETK reads its output.
    if saved.as_deref() != Some(id) {
        context
            .complete("activity:helper-created", &json!(id))
            .await?;
    }
    let status = helper["State"]["Status"].as_str().unwrap_or("");
    if !matches!(status, "created" | "running" | "exited") {
        return Err(ApiError::bad("清理辅助容器状态不确定，已拒绝重复执行"));
    }
    if status == "created" && !allow_start {
        return Err(ApiError::bad("清理辅助进程尚未开始，保留原任务等待继续"));
    }
    if status == "created" {
        context.checkpoint().await?;
        context
            .save_plan("activity:resource-state", &json!({"restored":false}))
            .await?;
    }
    if emby_running && matches!(status, "created" | "running") {
        tracing::info!("[活动日志清理] 开始停止原 Emby 容器，清理结束后恢复运行");
        docker
            .stop_container(
                &plan.container_id,
                Some(
                    bollard::query_parameters::StopContainerOptionsBuilder::default()
                        .t(30)
                        .build(),
                ),
            )
            .await?;
    }
    if status == "created" {
        // Pausing after stopping Emby must not leave the media server off.
        // Finish the critical section; cancellation still restores it below.
        if context.is_cancelled() {
            return Err(ApiError(
                axum::http::StatusCode::CONFLICT,
                "任务已取消".into(),
            ));
        }
        docker
            .start_container(id, None::<bollard::query_parameters::StartContainerOptions>)
            .await?;
    }
    // An already running helper must finish before Emby is restarted, even
    // when the operator stops the recovering task during this wait.
    if wait_for_helper(docker, id).await? != 0 {
        return Err(ApiError::bad("活动日志清理辅助进程失败"));
    }
    let mut logs = docker.logs(
        id,
        Some(
            bollard::query_parameters::LogsOptionsBuilder::default()
                .stdout(true)
                .stderr(true)
                .build(),
        ),
    );
    let mut bytes = vec![];
    while let Some(item) = logs.next().await {
        bytes.extend(item?.into_bytes());
    }
    let text = String::from_utf8_lossy(&bytes);
    let result: Value = serde_json::from_str(text.lines().last().unwrap_or(""))
        .map_err(|_| ApiError::bad("活动日志清理结果无效"))?;
    validate_clear_result(&result)?;
    context.complete("activity:cleared", &result).await?;
    Ok(result)
}

async fn wait_for_api(state: &AppState) -> Result<()> {
    let base = string(&*state.config.read().await, "emby_server_url");
    for _ in 0..30 {
        if state
            .http
            .get(format!("{}/System/Info/Public", base.trim_end_matches('/')))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            tracing::info!("[活动日志清理] Emby API 已恢复");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    tracing::error!("[活动日志清理] Emby 容器已启动，但 API 未恢复");
    Err(ApiError::bad("Emby重启后API未恢复"))
}

/// Restore the stopped media server before normal executors or a saved pause
/// can block startup. Only finish an existing helper; never launch new work.
pub(crate) async fn resume_cleanup(state: &AppState, context: &JobContext) -> Result<()> {
    if context.completed("activity:finished").await?.is_some() {
        return Ok(());
    }
    if context
        .plan("activity:resource-state")
        .await?
        .is_some_and(|state| state["restored"] == true)
    {
        return Ok(());
    }
    let Some(plan) = context.plan("activity:plan").await? else {
        return Ok(());
    };
    let plan: ClearPlan =
        serde_json::from_value(plan).map_err(|_| ApiError::bad("活动日志清理恢复计划无效"))?;
    let docker = crate::docker_host::client(state)?;
    let info = docker.inspect_container(&plan.container_id, None).await?;
    if info.image.as_deref() != Some(plan.container_image.as_str()) {
        return Err(ApiError::bad("Emby容器与原清理任务不一致，已停止恢复"));
    }
    if context.completed("activity:cleared").await?.is_none() {
        let saved = context.completed("activity:helper-created").await?;
        let lookup = saved
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or(&plan.helper_name);
        match docker.inspect_container(lookup, None).await {
            Ok(helper) => {
                let status = helper
                    .state
                    .as_ref()
                    .and_then(|state| state.status.as_ref())
                    .map(ToString::to_string)
                    .unwrap_or_default();
                if matches!(status.as_str(), "running" | "exited") {
                    if let Err(error) = helper_result(
                        &docker,
                        &plan,
                        context,
                        info.state.as_ref().and_then(|state| state.running) == Some(true),
                        false,
                    )
                    .await
                    {
                        finish_helper_before_restart(&docker, &plan, context).await?;
                        tracing::warn!(task_id=%context.id, "活动日志原清理结果未通过验收，将在任务恢复后报告：{}", error.1);
                    }
                } else if status != "created" {
                    finish_helper_before_restart(&docker, &plan, context).await?;
                }
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => (),
            Err(error) => return Err(error.into()),
        }
    }
    if plan.original_running {
        let current = docker.inspect_container(&plan.container_id, None).await?;
        if current.state.as_ref().and_then(|state| state.running) != Some(true) {
            docker
                .start_container(
                    &plan.container_id,
                    None::<bollard::query_parameters::StartContainerOptions>,
                )
                .await?;
        }
        wait_for_api(state).await?;
    }
    context
        .save_plan("activity:resource-state", &json!({"restored":true}))
        .await?;
    Ok(())
}

pub async fn clear(state: &AppState, context: &JobContext) -> Result<Value> {
    if let Some(result) = context.completed("activity:finished").await? {
        return Ok(result);
    }
    let docker = crate::docker_host::client(state)?;
    let plan = if let Some(plan) = context.plan("activity:plan").await? {
        serde_json::from_value::<ClearPlan>(plan)
            .map_err(|_| ApiError::bad("活动日志清理恢复计划无效"))?
    } else {
        context.checkpoint().await?;
        let list = docker
            .list_containers(Some(
                bollard::query_parameters::ListContainersOptionsBuilder::default()
                    .all(true)
                    .build(),
            ))
            .await?;
        let values = list
            .iter()
            .map(|container| serde_json::to_value(container).unwrap())
            .collect::<Vec<_>>();
        let id = crate::docker_host::unique_emby(&values)?;
        let info = docker.inspect_container(&id, None).await?;
        let toolkit = std::env::var("CONTAINER_NAME").unwrap_or("emby-toolkit".into());
        let toolkit = docker.inspect_container(&toolkit, None).await?;
        let plan = ClearPlan {
            container_id: id,
            container_image: info
                .image
                .ok_or_else(|| ApiError::bad("Emby镜像身份缺失"))?,
            original_running: info.state.and_then(|state| state.running).unwrap_or(false),
            toolkit_image: toolkit
                .image
                .ok_or_else(|| ApiError::bad("ETK镜像身份缺失"))?,
            helper_name: format!(
                "etk-activity-clear-{}",
                if context.id.is_empty() {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    context.id.clone()
                }
            ),
        };
        context
            .save_plan("activity:plan", &serde_json::to_value(&plan).unwrap())
            .await?;
        plan
    };
    let info = docker.inspect_container(&plan.container_id, None).await?;
    if info.image.as_deref() != Some(plan.container_image.as_str()) {
        return Err(ApiError::bad("Emby容器与原清理任务不一致，已停止恢复"));
    }
    let result = async {
        if let Some(result) = context.completed("activity:cleared").await? {
            validate_clear_result(&result)?;
            return Ok(result);
        }
        helper_result(
            &docker,
            &plan,
            context,
            info.state.as_ref().and_then(|state| state.running) == Some(true),
            true,
        )
        .await
    }
    .await;
    // Restart the recorded container even when clearing, cancellation, or
    // persistence failed. Never select a different Emby container on recovery.
    if result.is_err() {
        finish_helper_before_restart(&docker, &plan, context).await?;
    }
    let restored = context
        .plan("activity:resource-state")
        .await?
        .is_some_and(|state| state["restored"] == true);
    if !restored && plan.original_running {
        let current = docker.inspect_container(&plan.container_id, None).await?;
        if current.state.as_ref().and_then(|state| state.running) != Some(true) {
            tracing::info!("[活动日志清理] 开始恢复原 Emby 容器运行");
            docker
                .start_container(
                    &plan.container_id,
                    None::<bollard::query_parameters::StartContainerOptions>,
                )
                .await?;
        }
        wait_for_api(state).await?;
    }
    context
        .save_plan("activity:resource-state", &json!({"restored":true}))
        .await?;
    let result = result?;
    if let Some(helper) = context
        .completed("activity:helper-created")
        .await?
        .and_then(|id| id.as_str().map(str::to_string))
    {
        match docker
            .remove_container(
                &helper,
                Some(
                    bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .build(),
                ),
            )
            .await
        {
            Ok(()) => tracing::info!("[活动日志清理] 清理辅助容器已移除"),
            Err(error) => tracing::warn!(%error, "[活动日志清理] 清理辅助容器移除失败"),
        }
    }
    let result = json!({"deleted":result["before"].as_i64().unwrap_or(0)-result["after"].as_i64().unwrap_or(0),"backup":result["backup"],"message":"Emby活动日志清理完成"});
    context.complete("activity:finished", &result).await?;
    Ok(result)
}
