use crate::error::{ApiError, Result};
use bollard::{Docker, exec::StartExecResults, models::ExecConfig};
use futures_util::StreamExt;
use serde_json::Value;

pub struct Service {
    pub docker: Option<Docker>,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            docker: Docker::connect_with_local_defaults().ok(),
        }
    }
}
pub fn client(state: &crate::AppState) -> Result<Docker> {
    state
        .docker
        .docker
        .clone()
        .ok_or_else(|| ApiError::bad("Docker连接不可用"))
}
impl From<bollard::errors::Error> for ApiError {
    fn from(error: bollard::errors::Error) -> Self {
        tracing::error!(%error,"Docker操作失败");
        ApiError::bad("Docker操作失败，请查看日志")
    }
}
pub async fn exec(docker: &Docker, id: &str, command: &[String]) -> Result<Vec<u8>> {
    let (status, stdout, stderr) = exec_output(docker, id, command).await?;
    if status != 0 {
        return Err(ApiError::bad("Docker命令执行失败"));
    }
    Ok([stdout, stderr].concat())
}
pub async fn exec_output(
    docker: &Docker,
    id: &str,
    command: &[String],
) -> Result<(i64, Vec<u8>, Vec<u8>)> {
    let exec = docker
        .create_exec(
            id,
            ExecConfig {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(command.to_vec()),
                ..Default::default()
            },
        )
        .await?
        .id;
    let mut stdout = vec![];
    let mut stderr = vec![];
    if let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec, None).await? {
        while let Some(item) = output.next().await {
            match item? {
                bollard::container::LogOutput::StdErr { message } => stderr.extend(message),
                output => stdout.extend(output.into_bytes()),
            }
        }
    }
    let status = docker.inspect_exec(&exec).await?;
    Ok((
        status
            .exit_code
            .ok_or_else(|| ApiError::bad("Docker命令尚未结束"))?,
        stdout,
        stderr,
    ))
}
pub fn unique_emby(containers: &[Value]) -> Result<String> {
    let mut candidates = containers
        .iter()
        .filter(|c| {
            c["Image"]
                .as_str()
                .is_some_and(|s| s == "amilys/embyserver" || s.starts_with("amilys/embyserver:"))
        })
        .collect::<Vec<_>>();
    let running = candidates
        .iter()
        .filter(|c| c["State"] == "running")
        .cloned()
        .collect::<Vec<_>>();
    if running.len() == 1 {
        candidates = running;
    }
    if candidates.len() != 1 {
        return Err(ApiError::bad("需要唯一的amilys/embyserver容器"));
    }
    candidates[0]["Id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ApiError::bad("Docker缺少容器ID"))
}
