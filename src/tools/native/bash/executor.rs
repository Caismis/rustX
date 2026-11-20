//! Native Bash delegates physical execution to the authorized Session binding.

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::types::{ToolInvocation, ToolInvocationMode};

pub(super) struct BashTool;

impl ToolExecutor for BashTool {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                if invocation.mode == ToolInvocationMode::Background {
                    return crate::tools::native::support::failed_result(
                        "background Agent Bash requires an accepted Client registration",
                    );
                }
                super::remote::run(&invocation, &context).await
            }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}
