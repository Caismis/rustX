//! Thin model adapter. Native execution supplies scope, provenance and budget.
use super::registration::{NativeToolRegistration, input_schema};
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
    ToolResultContent,
};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) const NAMES: [&str; 3] = ["get_schedule", "list_schedules", "schedule_command"];

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Get {
    id: crate::scheduler::ScheduleId,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct List {
    #[serde(default)]
    after_ordinal: u64,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    limit: u32,
}
fn page_size() -> u32 {
    20
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Command {
    mutation: crate::scheduler::ScheduleMutation,
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    [
        (NAMES[0], "Read one schedule in your exact native conversation. Reading grants no authority to change it.", input_schema::<Get>()),
        (NAMES[1], "Read a bounded ordinal page of schedules in your exact native conversation. Completed means occurrence admission ended, not execution success. Reading consumes no mutation budget.", input_schema::<List>()),
        (NAMES[2], "Change schedules ONLY when the current Human request explicitly asks for scheduling or an edit/delete. Do not infer scheduling intent from task complexity or reminder text. Native execution binds the actual Human request and exact destination; arguments cannot supply or renew authority. At most eight successful mutations per Human request; multiple reminders may use separate calls. Read before edits and supply the exact expected revision; never blindly retry a conflict. Relative timing is anchored at the first successful native commit. Disabling/deleting affects future occurrences only; admitted work keeps its ownership.", input_schema::<Command>()),
    ].into_iter().map(|(name,description,input_schema)| ToolDefinition {
        id: crate::runtime::identity::ToolId::new(format!("native.{name}")), name: name.into(), description: description.into(), input_schema,
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }).collect()
}
pub(crate) fn registrations() -> Vec<NativeToolRegistration> {
    definitions()
        .into_iter()
        .enumerate()
        .map(|(index, definition)| {
            NativeToolRegistration::new(definition, Arc::new(ScheduleExecutor(index)))
        })
        .collect()
}
struct ScheduleExecutor(usize);
impl ToolExecutor for ScheduleExecutor {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let result = (|| -> Result<serde_json::Value, String> {
                    if context.cancellation.is_cancelled() {
                        return Err("Schedule control cancelled".into());
                    }
                    let control = context
                        .schedule
                        .as_ref()
                        .ok_or("Schedule control is unavailable in this execution context")?;
                    match self.0 {
                        0 => {
                            let args: Get = serde_json::from_value(invocation.arguments)
                                .map_err(|e| e.to_string())?;
                            serde_json::to_value(control.control.get(&args.id)?)
                                .map_err(|e| e.to_string())
                        }
                        1 => {
                            let args: List = serde_json::from_value(invocation.arguments)
                                .map_err(|e| e.to_string())?;
                            let rows = control
                                .control
                                .list(args.after_ordinal, args.limit)?
                                .into_iter()
                                .map(crate::scheduler::ScheduleHeader::from)
                                .collect::<Vec<_>>();
                            serde_json::to_value(rows).map_err(|e| e.to_string())
                        }
                        2 => {
                            let args: Command = serde_json::from_value(invocation.arguments)
                                .map_err(|e| e.to_string())?;
                            let _call = invocation.id.canonical_call_id().ok_or(
                                "Schedule mutation requires a root canonical Tool invocation",
                            )?;
                            let command = crate::scheduler::ScheduleCommand {
                                command_id: control.command_id.clone(),
                                mutation: args.mutation,
                            };
                            serde_json::to_value(control.control.command(
                                control.grant,
                                &command,
                                context.subagent_context(),
                                &context.cancellation,
                            )?)
                            .map_err(|e| e.to_string())
                        }
                        _ => unreachable!("closed Schedule tool group"),
                    }
                })();
                match result {
                    Ok(value) => ToolExecutionResult {
                        status: ToolExecutionStatus::Success,
                        content: vec![ToolResultContent::Json { value }],
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        deliveries: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    },
                    Err(error) => super::support::failed_result(error),
                }
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}
