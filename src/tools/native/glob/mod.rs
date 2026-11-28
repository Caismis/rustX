//! Native Glob tool.
//!
//! Glob's physical traversal lives on the Session's admitted Execution
//! Client: the shared file universe is enumerated descriptor-relative
//! on the Client, and the Server owns only admission, input
//! validation, and the model-facing projection of the verified record
//! page — the exact notices and no-results text the pre-migration
//! contract produced.

mod input;

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search_failure::search_failure;
use crate::tools::native::support::{
    cancelled_result, failed_result, interpret_file_path, success_text,
};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GlobInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "glob";

/// The tool-owned registration of the native Glob tool.
#[must_use]
pub(super) fn definition(policy: ToolInvocationPolicy) -> crate::tools::types::ToolDefinition {
    native_definition::<GlobInput>(
        "tool-glob",
        NAME,
        "Find files whose path matches a glob pattern through the Session's admitted Execution Client. Resolve a relative path from the execution cwd; absolute paths refer to the Client filesystem. The optional limit defaults to 1000 and may be larger. Results are plain text, sorted lexically, relative to the search root, and use POSIX separators. Hidden files, ignore-file behavior, and symlink traversal follow rustX's existing policy.",
        policy,
    )
}

pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(policy), std::sync::Arc::new(GlobTool))
}

/// The native Glob executor.
pub struct GlobTool;

impl ToolExecutor for GlobTool {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move { run_glob(&invocation, &context).await }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

async fn run_glob(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match GlobInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let pattern = input.pattern.as_str();
    // The shared wire bound runs before compilation: an over-cap pattern
    // is refused cheaply, never compiled just to be rejected on the wire.
    if pattern.len() > rustx_execution_protocol::MAX_SEARCH_PATTERN_BYTES {
        return failed_result(format!(
            "glob pattern exceeds the {}-byte bound",
            rustx_execution_protocol::MAX_SEARCH_PATTERN_BYTES
        ));
    }
    // The pattern is compiled Server-side once: the diagnostic the model
    // sees for a malformed glob is the Agent's own, and the Client
    // recompiles the same text for the physical match.
    if let Err(error) = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
    {
        return failed_result(format!("invalid glob pattern {pattern:?}: {error}"));
    }
    let root = match interpret_file_path(
        context.workspace.root(),
        input.path.as_deref().unwrap_or("."),
    ) {
        Ok(root) => root,
        Err(error) => return failed_result(error),
    };
    // Cancellation observable before the exchange is admitted means the
    // dispatch never starts: no request frame is ever written for a
    // cancelled search.
    if context.cancellation.is_cancelled() {
        return cancelled_result(context.cancellation.reason());
    }
    // There is no Server-local traversal: the Session's admitted
    // Execution Client is the only physical searcher, reached through
    // the non-owning route composition bound to this workspace. An
    // unbound route is an honest refusal, never a fallback.
    let Some(scope) = context.workspace.execution_scope().cloned() else {
        return failed_result("Session execution unavailable: no admitted Execution Client");
    };
    let limit = input.limit();
    let result = match scope
        .search(
            &root,
            rustx_execution_protocol::SearchSpec::Glob {
                pattern: input.pattern,
                limit: rustx_execution_protocol::DecimalU64::new(limit),
            },
            &context.cancellation,
        )
        .await
    {
        Ok(result) => result,
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
            return cancelled_result(context.cancellation.reason());
        }
        Err(error) => return failed_result(search_failure(&root, &error)),
    };
    if result.body.is_empty() && !result.hit_limit && !result.byte_limit {
        return success_text("No files found matching pattern", None);
    }
    let mut output = result.body;
    let mut notices = Vec::new();
    if result.hit_limit {
        notices.push(format!(
            "{} results limit reached. Use limit={} for more, or refine pattern",
            limit,
            limit.saturating_mul(2)
        ));
    }
    if result.byte_limit {
        notices.push("50KB limit reached".to_owned());
    }
    if !notices.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push('[');
        output.push_str(&notices.join(". "));
        output.push(']');
    }
    let truncated = !notices.is_empty();
    success_text(
        output,
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
    )
}
