//! Model-facing Workflow Tools over the native `WorkflowRuntime`.
//!
//! One discovered Workflow id is one independent Tool. The executor captures
//! the immutable compiled program at catalog publication time and delegates
//! execution to `WorkflowRuntime`, which in turn owns only orchestration and
//! uses the existing `SubagentRegistry` for child `AgentRuns`.

use std::sync::Arc;

use crate::runtime::workflow::{WorkflowCatalog, WorkflowProgram, WorkflowRuntime};
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::NativeToolRegistration;
use crate::tools::native::support::{failed_result, success_json};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolInvocation,
    ToolOrigin, ToolReplayPolicy,
};

/// Builds one registration per explicitly model-visible Workflow id.
pub(super) fn registrations(
    runtime: &WorkflowRuntime,
    catalog: &WorkflowCatalog,
) -> Vec<NativeToolRegistration> {
    catalog
        .admitted()
        .iter()
        .map(|id| {
            let program = catalog
                .get(id)
                .expect("WorkflowCatalog validates every main id");
            NativeToolRegistration::new(
                definition(program),
                Arc::new(WorkflowToolExecutor {
                    runtime: runtime.clone(),
                    program: Arc::clone(program),
                }),
            )
            .with_foreground_policy(foreground_policy(program))
        })
        .collect()
}

fn foreground_policy(program: &WorkflowProgram) -> crate::tools::deadline::ForegroundPolicy {
    crate::tools::deadline::ForegroundPolicy::Composite {
        total: crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
            std::time::Duration::from_millis(program.timeout_ms()),
            None,
        ),
    }
}

pub(crate) fn tool_id(
    id: &crate::runtime::workflow::WorkflowId,
) -> crate::runtime::identity::ToolId {
    crate::runtime::identity::ToolId::new(format!("tool-workflow-{id}"))
}

pub(crate) fn definition(program: &WorkflowProgram) -> ToolDefinition {
    ToolDefinition {
        id: tool_id(program.id()),
        name: program.id().to_string(),
        description: program.description().to_owned(),
        input_schema: program.input_schema().clone(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

struct WorkflowToolExecutor {
    runtime: WorkflowRuntime,
    program: Arc<WorkflowProgram>,
}

#[cfg(test)]
pub(crate) fn test_executor(
    runtime: WorkflowRuntime,
    program: Arc<WorkflowProgram>,
) -> (
    Arc<dyn ToolExecutor>,
    crate::tools::deadline::ForegroundPolicy,
) {
    let policy = foreground_policy(&program);
    (Arc::new(WorkflowToolExecutor { runtime, program }), policy)
}

impl ToolExecutor for WorkflowToolExecutor {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let Some(subagent_context) = context.subagent_context().cloned() else {
            return ToolExecutionHandle::settled_by_operation(
                Box::pin(async {
                    failed_result("Workflow Tools are available only inside an admitted Agent turn")
                }),
                context.cancellation.clone(),
            );
        };
        let runtime = self.runtime.clone();
        let program = Arc::clone(&self.program);
        let identity = program.tool_identity();
        let run_id = invocation
            .id
            .canonical_call_id()
            .expect("Agent-owned invocation")
            .clone();
        let operation_cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let mut result = match runtime
                    .run_foreground(
                        program,
                        run_id,
                        subagent_context,
                        invocation.arguments,
                        operation_cancellation.clone(),
                    )
                    .await
                {
                    Ok(value) => success_json(value),
                    Err(error) => {
                        let mut result =
                            crate::tools::invocation::terminal(error.execution_status());
                        if let crate::runtime::workflow::WorkflowRunError::WorkspaceSettlement {
                            workspace,
                            candidate,
                            ..
                        } = error
                        {
                            result
                                .content
                                .push(crate::tools::types::ToolResultContent::Json {
                                value: serde_json::json!({"workspace": workspace, "candidate": candidate}),
                                });
                        }
                        result
                    }
                };
                result.workflow = Some(Box::new(identity));
                result
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}
