//! Stable Goal command adapters; `AgentExecution` owns consumable Human request authority.

use crate::goal::{DEFAULT_ROUND_BUDGET, GoalMutation, GoalRef, GoalWrite};
use crate::tools::executor::{
    ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistration,
};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
    ToolResultContent,
};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) const NAMES: [&str; 3] = ["get_goal", "create_goal", "update_goal"];

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetInput {}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    objective: String,
    #[serde(default = "default_budget")]
    autonomous_round_budget: u32,
}
fn default_budget() -> u32 {
    DEFAULT_ROUND_BUDGET
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum UpdateInput {
    Complete { expected: GoalRef },
    Blocked { expected: GoalRef, reason: String },
}

fn update_schema() -> serde_json::Value {
    let mut schema = super::registration::input_schema::<UpdateInput>();
    schema["type"] = serde_json::json!("object");
    schema
}

pub(crate) fn registrations() -> Vec<ToolRegistration> {
    [
        (NAMES[0], "Read the current revisioned Goal, or null. Reading never creates, resumes, or arms a Goal.", super::registration::input_schema::<GetInput>()),
        (NAMES[1], "Create a persistent cross-turn Goal ONLY when the current Human request explicitly authorizes persistent autonomous pursuit (for example, keep working until the outcome is achieved). The user need not type /goal. Do not create a Goal merely because a task is hard, long, multi-step, uses many Tools, Workflows, or Subagents. Preserve the authorized objective without narrowing or expanding it. Runtime binds the actual initiating Human identity; arguments cannot supply it. One unfinished Goal is allowed. Budget is 1..100 autonomous continuation rounds, default 10; this Human attempt consumes zero. You cannot later increase this budget.", super::registration::input_schema::<CreateInput>()),
        (NAMES[2], "Declare an Active Goal complete or blocked using the exact observed GoalRef. A stale revision is rejected: read and reason again, never blindly retry. Complete is terminal; blocked requires a reason. Completion declares state; use ordinary facts and Tool results to judge success. Pause, resume, objective edits and budget changes belong to explicit user controls.", update_schema()),
    ].into_iter().map(|(name, description, input_schema)| ToolRegistration::plain(ToolDefinition {
        id: crate::runtime::identity::ToolId::new(format!("native.{name}")),
        name: name.to_owned(), description: description.to_owned(), input_schema,
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }, Arc::new(GoalExecutor(name)))).collect()
}

struct GoalExecutor(&'static str);
impl ToolExecutor for GoalExecutor {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let result = (|| -> Result<serde_json::Value, String> {
                    let goal_context = context
                        .goal
                        .as_deref()
                        .ok_or("Goal is unavailable in this execution context")?;
                    let domain = &goal_context.domain;
                    let mut creation_authorization = None;
                    let write = match self.0 {
                        "get_goal" => {
                            serde_json::from_value::<GetInput>(invocation.arguments)
                                .map_err(|e| e.to_string())?;
                            return serde_json::to_value(
                                domain.view().map_err(|e| e.to_string())?.current,
                            )
                            .map_err(|e| e.to_string());
                        }
                        "create_goal" => {
                            let input: CreateInput = serde_json::from_value(invocation.arguments)
                                .map_err(|e| e.to_string())?;
                            let authorization = goal_context
                                .creation_authorization
                                .lock()
                                .expect("Goal authorization mutex poisoned");
                            let origin = authorization.clone().ok_or(
                                "Goal creation requires unused current Human request authorization",
                            )?;
                            creation_authorization = Some(authorization);
                            GoalWrite::Create {
                                objective: input.objective,
                                budget: input.autonomous_round_budget,
                                origin,
                            }
                        }
                        "update_goal" => {
                            match serde_json::from_value::<UpdateInput>(invocation.arguments)
                                .map_err(|e| e.to_string())?
                            {
                                UpdateInput::Complete { expected } => GoalWrite::Mutate {
                                    expected,
                                    mutation: GoalMutation::Complete,
                                },
                                UpdateInput::Blocked { expected, reason } => GoalWrite::Mutate {
                                    expected,
                                    mutation: GoalMutation::Block { reason },
                                },
                            }
                        }
                        _ => unreachable!("closed Goal command surface"),
                    };
                    // Same lifecycle commit guard as ordinary inbound ownership.
                    // Drain cannot cross Running -> Draining during this commit.
                    match domain
                        .write_from_tool(&goal_context.mailbox, write, &context.cancellation)
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?
                    {
                        Ok(goal) => {
                            // Consume at the authoritative create success, before
                            // serialization or outer Tool cancellation settlement.
                            // Rejections and failed commits leave it untouched.
                            if let Some(mut authorization) = creation_authorization {
                                authorization.take();
                            }
                            serde_json::to_value(goal).map_err(|e| e.to_string())
                        }
                        Err(rejection) => {
                            Err(serde_json::to_string(&rejection).map_err(|e| e.to_string())?)
                        }
                    }
                })();
                match result {
                    Ok(value) => ToolExecutionResult {
                        status: ToolExecutionStatus::Success,
                        content: vec![ToolResultContent::Json { value }],
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
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
