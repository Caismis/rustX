//! Native Write tool.
//!
//! Write resolves relative paths against the execution cwd, creates missing
//! parent directories, and commits a complete UTF-8 snapshot atomically.
//! Absolute paths must stay within the Session's admitted Client workspace;
//! managed tool-output roots remain outside it. Runtime-owned `ManagedToolOutput` paths
//! remain read-only to model-originated mutation.

mod input;

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{failed_result, success_text};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation};

use input::WriteInput;

// Compaction file-operation metadata decodes the path of a historical
// canonical Write call through this tool-owned boundary (Issue #140).

/// The canonical model-facing name of the tool.
pub const NAME: &str = "write";

/// The canonical identity of the rustX native Write capability.
pub(crate) const TOOL_ID: &str = "tool-write";

/// The tool-owned registration of the native Write tool.
#[must_use]
pub(super) fn definition(policy: ToolInvocationPolicy) -> crate::tools::types::ToolDefinition {
    native_definition::<WriteInput>(
        TOOL_ID,
        NAME,
        "Create or replace a UTF-8 file through the Session's admitted Execution Client. Resolve relative paths from the execution cwd; absolute paths must stay within the Session's admitted Client workspace. Missing parent directories are created automatically.",
        policy,
    )
}

pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(policy), std::sync::Arc::new(WriteTool))
}

/// The native Write executor.
pub struct WriteTool;

impl ToolExecutor for WriteTool {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move { run_write(&invocation, &context).await }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

pub(super) async fn run_write(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match WriteInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let Some(bound) = context.bound_mutation.as_ref() else {
        return failed_result(
            "write mutation target was not bound at authorization; refusing to resolve independently",
        );
    };
    // Every prepare failure short-circuited at bind — a bound target
    // always holds the live ticketed owner for this exact invocation.
    let owner = bound.prepared();
    match owner.commit(&context.cancellation).await {
        Ok(_) => success_text(
            format!(
                "Successfully wrote {} bytes to {}",
                input.content.len(),
                input.path
            ),
            None,
        ),
        Err(error) => crate::tools::native::mutation::commit_error_result(
            &input.path,
            &context.cancellation,
            &error,
        ),
    }
}
