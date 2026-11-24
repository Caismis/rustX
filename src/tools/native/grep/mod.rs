//! Native Grep tool.
//!
//! Grep's physical engine lives on the Session's admitted Execution
//! Client: the shared file universe is enumerated descriptor-relative
//! inside the workspace, each selected file is whole-source UTF-8
//! validated and streamed through the bounded `grep-searcher` engine
//! there, and the Server owns only admission, input validation, and the
//! model-facing projection of the verified record page — the exact
//! notices and no-results text the pre-migration contract produced.

mod input;

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search_failure::search_failure;
use crate::tools::native::support::{
    cancelled_result, failed_result, interpret_path, success_text,
};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GrepInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "grep";

/// The tool-owned registration of the native Grep tool.
#[must_use]
pub(super) fn definition(policy: ToolInvocationPolicy) -> crate::tools::types::ToolDefinition {
    native_definition::<GrepInput>(
        "tool-grep",
        NAME,
        "Search UTF-8 text files for matching lines through the Session's admitted Execution Client. Resolve a relative path from the execution cwd; absolute paths must stay within the Session's admitted Client workspace. The optional limit defaults to 100 and may be larger. Results are plain text; long lines are shortened to 500 characters and bounded results include instructions for continuing or refining the search. Hidden files are included and .gitignore behavior is unchanged.",
        policy,
    )
}

pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(policy), std::sync::Arc::new(GrepTool))
}

/// The native Grep executor.
pub struct GrepTool;

impl ToolExecutor for GrepTool {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move { run_grep(&invocation, &context).await }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

async fn run_grep(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match GrepInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let pattern = input.pattern.as_str();
    // The shared wire bound runs before compilation: an over-cap pattern
    // is refused cheaply, never compiled just to be rejected on the wire.
    if pattern.len() > rustx_execution_protocol::MAX_SEARCH_PATTERN_BYTES {
        return failed_result(format!(
            "grep pattern exceeds the {}-byte bound",
            rustx_execution_protocol::MAX_SEARCH_PATTERN_BYTES
        ));
    }
    // The pattern and the file filter are compiled Server-side once —
    // under the wire's own regex budgets — so the diagnostic the model
    // sees for a malformed or oversized pattern is the Agent's own; the
    // Client recompiles the same text for the physical match.
    if let Err(error) = grep_regex::RegexMatcherBuilder::new()
        .fixed_strings(input.literal())
        .case_insensitive(input.ignore_case())
        .multi_line(false)
        .size_limit(rustx_execution_protocol::MAX_SEARCH_BUFFER_BYTES)
        .dfa_size_limit(rustx_execution_protocol::MAX_SEARCH_BUFFER_BYTES)
        .build(pattern)
    {
        return failed_result(format!("invalid pattern {pattern:?}: {error}"));
    }
    // The shared wire bound runs before the filter's own compilation.
    if let Some(glob) = input.glob.as_deref()
        && glob.len() > rustx_execution_protocol::MAX_SEARCH_PATTERN_BYTES
    {
        return failed_result(format!(
            "grep glob filter exceeds the {}-byte bound",
            rustx_execution_protocol::MAX_SEARCH_PATTERN_BYTES
        ));
    }
    if let Some(glob) = input.glob.as_deref()
        && let Err(error) = globset::GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
    {
        return failed_result(format!("invalid glob {glob:?}: {error}"));
    }
    let root = interpret_path(
        context.workspace.root(),
        input.path.as_deref().unwrap_or("."),
    );
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
    let spec = rustx_execution_protocol::SearchSpec::Grep {
        literal: input.literal(),
        ignore_case: input.ignore_case(),
        context: input.context(),
        limit: rustx_execution_protocol::DecimalU64::new(limit),
        pattern: input.pattern,
        glob: input.glob,
    };
    let result = if let Some(locator) = crate::tools::managed_import::locator_at(&root) {
        let Some(conversation) =
            rustx_execution_protocol::save::ConversationId::parse(context.conversation_id.as_str())
        else {
            return failed_result("saved output is unavailable for this Conversation");
        };
        scope
            .search_saved_output(locator, conversation, spec, &context.cancellation)
            .await
    } else if let Some(operation) = crate::tools::native::bash_operation_at(&root) {
        let conversation = context.conversation_id.as_str().to_owned();
        scope
            .inspect_bash_output(
                operation,
                move |key| rustx_execution_protocol::ClientRequest::SearchBashOutput {
                    key,
                    conversation,
                    spec,
                },
                &context.cancellation,
            )
            .await
            .map(|page| match page {
                rustx_execution_server::ContinuationPage::Search(search) => search,
                _ => {
                    unreachable!("validated Bash Grep reply")
                }
            })
    } else {
        scope.search(&root, spec, &context.cancellation).await
    };
    let result = match result {
        Ok(result) => result,
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
            return cancelled_result(context.cancellation.reason());
        }
        Err(error) => return failed_result(search_failure(&root, &error)),
    };
    project_search(result, limit)
}

fn project_search(result: rustx_execution_server::SearchResult, limit: u64) -> ToolExecutionResult {
    // A truly empty result — no records and no overflow — is the plain
    // no-match text. A byte-limited page can hold context rows with zero
    // selected matches, or overflow before the first record at all:
    // neither is "no matches", so the notice still projects.
    if result.body.is_empty() && !result.byte_limit {
        return success_text("No matches found", None);
    }
    let mut output = result.body;
    let mut notices = Vec::new();
    if result.hit_limit {
        notices.push(format!(
            "{} matches limit reached. Use limit={} for more, or refine pattern",
            limit,
            limit.saturating_mul(2)
        ));
    }
    if result.byte_limit {
        notices.push("50KB limit reached".to_owned());
    }
    if result.short_lines {
        notices
            .push("Some lines truncated to 500 chars. Use read tool to see full lines".to_owned());
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
