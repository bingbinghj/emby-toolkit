use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::{JobContext, JobSpec},
};
use futures_util::{FutureExt, future::BoxFuture};
use serde_json::Value;

pub(crate) fn prepare_resume<'a>(
    state: &'a AppState,
    context: &'a JobContext,
) -> BoxFuture<'a, Result<()>> {
    async move {
        Box::pin(crate::activity::resume_cleanup(state, context)).await?;
        if !context.is_terminal_cleanup() {
            context.ensure_server(state).await?;
        }
        Box::pin(crate::thumbnails::resume_cleanup(state, context)).await?;
        Box::pin(crate::media_save::resume_cleanup(state, context)).await?;
        Box::pin(crate::media_rescrape::resume_cleanup(state, context)).await?;
        Box::pin(crate::media_rename::resume_cleanup(state, context)).await?;
        Box::pin(crate::media_extra::resume_cleanup(state, context)).await?;
        Box::pin(crate::media_images::resume_cleanup(state, context)).await?;
        Box::pin(crate::notifications::resume_registration(state, context)).await?;
        Ok(())
    }
    .boxed()
}

pub(crate) fn execute<'a>(
    state: &'a AppState,
    spec: &'a JobSpec,
    context: &'a JobContext,
) -> BoxFuture<'a, Result<(u16, Value)>> {
    let execution: BoxFuture<'a, Result<(u16, Value)>> = match spec.kind.as_str() {
        "tasks" => crate::tasks::execute_persisted(state, &spec.payload, context).boxed(),
        "actions" => crate::actions::execute_persisted(state, &spec.payload, context).boxed(),
        "audio_tasks" => {
            crate::audio_tasks::execute_persisted(state, &spec.payload, context).boxed()
        }
        "credits" => crate::credits::execute_persisted(state, &spec.payload, context).boxed(),
        "database_admin" => {
            crate::database_admin::execute_persisted(state, &spec.payload, context).boxed()
        }
        "fingerprint_audit" => {
            crate::fingerprint_audit::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_extra" => {
            crate::media_extra::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_images" => {
            crate::media_images::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_manual" => {
            crate::media_manual::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_recovery" => {
            crate::media_recovery::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_rename" => {
            crate::media_rename::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_save" => crate::media_save::execute_persisted(state, &spec.payload, context).boxed(),
        "orphans" => crate::orphans::execute_persisted(state, &spec.payload, context).boxed(),
        "monitor" => crate::monitor::execute_persisted(state, &spec.payload, context).boxed(),
        "webhook" => crate::webhook::execute_persisted(state, &spec.payload, context).boxed(),
        "revalidation" => {
            crate::revalidation::execute_persisted(state, &spec.payload, context).boxed()
        }
        "media_organize_queue" => async move {
            Ok((
                200,
                Box::pin(crate::media_organize_queue::resume_owned(
                    state,
                    &spec.payload,
                    context,
                ))
                .await?,
            ))
        }
        .boxed(),
        _ => async move { Err(ApiError::bad(format!("无法恢复任务类型：{}", spec.kind))) }.boxed(),
    };
    async move {
        context.ensure_server(state).await?;
        execution.await
    }
    .boxed()
}
