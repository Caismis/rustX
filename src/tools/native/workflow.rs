//! Model-facing Workflow Tools over the native `WorkflowRuntime`.
//!
//! One registered Workflow id is one independent Tool. The executor captures
//! the immutable compiled program at catalog publication time and delegates
//! execution to `WorkflowRuntime`, which in turn owns only orchestration and
//! uses the existing `SubagentRegistry` for child `AgentRuns`.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::runtime::workflow::{WorkflowCatalog, WorkflowProgram, WorkflowRuntime};
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::NativeToolRegistration;
use crate::tools::native::support::{cancelled_result, failed_result, success_json};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

/// Builds one registration per explicitly model-visible Workflow id.
pub(super) fn registrations(
    runtime: &WorkflowRuntime,
    catalog: &WorkflowCatalog,
) -> Vec<NativeToolRegistration> {
    catalog
        .main()
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
        })
        .collect()
}

fn definition(program: &WorkflowProgram) -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new(format!("tool-workflow-{}", program.id())),
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

impl ToolExecutor for WorkflowToolExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        let Some(subagent_context) = context.subagent_context().cloned() else {
            return Box::pin(async {
                failed_result("Workflow Tools are available only inside an admitted Agent turn")
            });
        };
        let runtime = self.runtime.clone();
        let program = Arc::clone(&self.program);
        let run_id = invocation.call_id.clone();
        let cancellation = context.cancellation.clone();
        Box::pin(async move {
            match runtime
                .run_foreground(
                    program,
                    run_id,
                    subagent_context,
                    invocation.arguments,
                    cancellation.clone(),
                )
                .await
            {
                Ok(value) => success_json(value),
                Err(error) if error.is_cancelled() => cancelled_result(cancellation.reason()),
                Err(error) => failed_result(error.to_string()),
            }
        })
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}
