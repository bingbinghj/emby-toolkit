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
pub async fn clear(state: &AppState, context: &JobContext) -> Result<Value> {
    let docker = crate::docker_host::client(state)?;
    let list = docker
        .list_containers(Some(
            bollard::query_parameters::ListContainersOptionsBuilder::default()
                .all(true)
                .build(),
        ))
        .await?;
    let values = list
        .iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect::<Vec<_>>();
    let id = crate::docker_host::unique_emby(&values)?;
    let info = docker.inspect_container(&id, None).await?;
    let running = info.state.and_then(|s| s.running).unwrap_or(false);
    context.checkpoint().await?;
    let result = async {
        if running {
            docker
                .stop_container(
                    &id,
                    Some(
                        bollard::query_parameters::StopContainerOptionsBuilder::default()
                            .t(30)
                            .build(),
                    ),
                )
                .await?;
        }
        let toolkit = std::env::var("CONTAINER_NAME").unwrap_or("emby-toolkit".into());
        let info = docker.inspect_container(&toolkit, None).await?;
        let image = info.image.ok_or_else(|| ApiError::bad("ETK镜像身份缺失"))?;
        let helper = docker
            .create_container(
                None::<bollard::query_parameters::CreateContainerOptions>,
                bollard::models::ContainerCreateBody {
                    image: Some(image),
                    entrypoint: Some(vec!["/app/emby-toolkit".into()]),
                    cmd: Some(vec![
                        "--clear-activity-log".into(),
                        "/config/data/activitylog.db".into(),
                    ]),
                    user: Some("0:0".into()),
                    host_config: Some(bollard::models::HostConfig {
                        volumes_from: Some(vec![id.clone()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?
            .id;
        let result = async {
            docker
                .start_container(
                    &helper,
                    None::<bollard::query_parameters::StartContainerOptions>,
                )
                .await?;
            use futures_util::StreamExt;
            let mut stream = docker.wait_container(
                &helper,
                None::<bollard::query_parameters::WaitContainerOptions>,
            );
            while let Some(item) = stream.next().await {
                let item = item?;
                if item.status_code != 0 {
                    return Err(ApiError::bad("活动日志清理辅助进程失败"));
                }
            }
            let mut logs = docker.logs(
                &helper,
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
            Ok::<_, ApiError>(result)
        }
        .await;
        let _ = docker
            .remove_container(
                &helper,
                Some(
                    bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .build(),
                ),
            )
            .await;
        result
    }
    .await;
    let restart = if running {
        docker
            .start_container(
                &id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(ApiError::from)
    } else {
        Ok(())
    };
    restart?;
    let result = result?;
    if running {
        let base = string(&*state.config.read().await, "emby_server_url");
        let mut ready = false;
        for _ in 0..30 {
            if state
                .http
                .get(format!("{}/System/Info/Public", base.trim_end_matches('/')))
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if !ready {
            return Err(ApiError::bad("Emby重启后API未恢复"));
        }
    }
    Ok(
        json!({"deleted":result["before"].as_i64().unwrap_or(0)-result["after"].as_i64().unwrap_or(0),"backup":result["backup"],"message":"Emby活动日志清理完成"}),
    )
}
