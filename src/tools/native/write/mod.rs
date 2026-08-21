//! Native Write tool.
//!
//! Write resolves relative paths against the execution cwd, creates missing
//! parent directories, and commits a complete UTF-8 snapshot atomically.
//! Absolute paths are ordinary host filesystem paths and are not subject to
//! the workspace containment policy. Runtime-owned `ManagedToolOutput` paths
//! remain read-only to model-originated mutation.

mod input;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{
    atomic_commit, failed_result, prepare_mutation_target, resolve_path, success_text,
};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation};

use input::WriteInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "write";

/// The tool-owned registration of the native Write tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<WriteInput>(
            "tool-write",
            NAME,
            "Create or replace a UTF-8 file. Resolve relative paths from the execution cwd; absolute paths are used as host filesystem paths. Missing parent directories are created automatically.",
            policy,
        ),
        std::sync::Arc::new(WriteTool),
    )
}

/// The native Write executor.
pub struct WriteTool;

impl ToolExecutor for WriteTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_write(&invocation, &context) })
    }
}

fn run_write(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match WriteInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let requested = resolve_path(context.workspace.root(), &input.path);
    let target = match prepare_mutation_target(&requested, context.tool_output) {
        Ok(target) => target,
        Err(error) => return failed_result(error),
    };
    if let Some(parent) = target.path().parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return failed_result(format!(
            "cannot create parent directories for {}: {error}",
            target.path().display()
        ));
    }
    if let Err(error) = atomic_commit(&target, input.content.as_bytes()) {
        return failed_result(error);
    }
    success_text(
        format!(
            "Successfully wrote {} bytes to {}",
            input.content.len(),
            input.path
        ),
        None,
    )
}
