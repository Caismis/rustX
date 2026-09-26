//! Model-facing controls for finite detached Tool jobs.
//!
//! The conversation's background registry owns identity, ordering, cancellation,
//! physical settlement and completion publication. These adapters never create a
//! job and never suppress its canonical completion notification.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::runtime::identity::{ToolExecutionId, ToolId};
use crate::tools::background::{BackgroundExecutionSnapshot, ConversationBackgroundRegistry};
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

use super::input::decode;
use super::registration::{NativeToolRegistration, input_schema};
use super::support::{cancelled_result, failed_result, success_json};

pub(crate) const NAMES: [&str; 4] = ["job_list", "job_status", "job_wait", "job_cancel"];
const LIST_LIMIT: usize = 64;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default)]
    active_only: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetInput {
    job_id: ToolExecutionId,
}

pub(super) fn definitions() -> Vec<ToolDefinition> {
    [
        (NAMES[0], "List this conversation's finite background Tool jobs, newest first, bounded to 64. Reports truncation; excludes Agent conversations and ordinary foreground calls.", input_schema::<ListInput>()),
        (NAMES[1], "Read the authoritative current snapshot of one background Tool job without waiting. Completion is delivered proactively in this conversation.", input_schema::<TargetInput>()),
        (NAMES[2], "Wait for this exact finite background Tool job to reach terminal physical settlement. A terminal job never resumes. Does not consume or suppress its completion notification.", input_schema::<TargetInput>()),
        (NAMES[3], "Cancel this exact background Tool job and wait for its terminal physical settlement. Already-terminal jobs stay terminal. Does not suppress completion notification.", input_schema::<TargetInput>()),
    ].into_iter().map(|(name, description, input_schema)| ToolDefinition {
        id: ToolId::new(format!("tool-{name}")),
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }).collect()
}

pub(super) fn registrations(
    background: &ConversationBackgroundRegistry,
) -> Vec<NativeToolRegistration> {
    definitions()
        .into_iter()
        .map(|definition| {
            NativeToolRegistration::new(definition, Arc::new(JobExecutor(background.clone())))
        })
        .collect()
}

struct JobExecutor(ConversationBackgroundRegistry);

impl ToolExecutor for JobExecutor {
    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }

    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                if invocation.tool_name == NAMES[0] {
                    let input: ListInput = match decode(NAMES[0], &invocation.arguments) {
                        Ok(input) => input,
                        Err(error) => return failed_result(error),
                    };
                    let listing = self.0.listing(input.active_only, LIST_LIMIT);
                    let jobs: Vec<_> = listing
                        .snapshots
                        .iter()
                        .map(|snapshot| {
                            serde_json::json!({
                                "job_id": snapshot.execution_id,
                                "tool": snapshot.tool_name,
                                "state": snapshot.state,
                            })
                        })
                        .collect();
                    return success_json(serde_json::json!({
                        "returned": jobs.len(), "matched": listing.matched,
                        "truncated": listing.matched > jobs.len(), "limit": LIST_LIMIT,
                        "jobs": jobs,
                    }));
                }
                let input: TargetInput = match decode(&invocation.tool_name, &invocation.arguments)
                {
                    Ok(input) => input,
                    Err(error) => return failed_result(error),
                };
                let job_id = input.job_id;
                let Some(snapshot) = self.0.snapshot(&job_id) else {
                    return failed_result(format!("unknown job {job_id}"));
                };
                if invocation.tool_name == NAMES[1] {
                    return snapshot_result(&snapshot);
                }
                if invocation.tool_name == NAMES[3] {
                    let _ = self.0.cancel(&job_id);
                }
                // Cancelling this waiting Tool ends observation only. A job's
                // cancellation authority is the explicit job_cancel operation.
                // Subscribe-before-read inside the owner avoids missed wakes;
                // the captured immutable ID can never name a later execution.
                tokio::select! {
                    biased;
                    snapshot = self.0.wait_until_terminal(&job_id) => match snapshot {
                        Some(snapshot) => snapshot_result(&snapshot),
                        None => failed_result(format!("job {job_id} is unavailable")),
                    },
                    () = cancellation.cancelled() => cancelled_result(cancellation.reason()),
                }
            }),
            context.cancellation.clone(),
        )
    }
}

fn snapshot_result(snapshot: &BackgroundExecutionSnapshot) -> ToolExecutionResult {
    success_json(serde_json::json!({
        "job_id": snapshot.execution_id,
        "tool": snapshot.tool_name,
        "state": snapshot.state,
        "progress": snapshot.progress,
        "result": snapshot.result,
    }))
}
