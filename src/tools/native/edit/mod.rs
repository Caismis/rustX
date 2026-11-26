//! Native Edit tool.
//!
//! Edit is a precise, atomic mutation primitive. All anchors are resolved
//! against one original snapshot, exact matching is preferred, and a
//! NFKC-based fuzzy fallback is used only when exact matching cannot find an
//! anchor for that individual edit. Any validation failure leaves the target
//! unchanged; runtime-owned `ManagedToolOutput` paths are read-only.

mod input;

use rustx_text_edit::EditFailure;

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{failed_result, success_text};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation};

use input::EditInput;

// Compaction file-operation metadata decodes the path of a historical
// canonical Edit call through this tool-owned boundary (Issue #140).

/// The canonical model-facing name of the tool.
pub const NAME: &str = "edit";

/// The canonical identity of the rustX native Edit capability.
pub(crate) const TOOL_ID: &str = "tool-edit";

/// The tool-owned registration of the native Edit tool.
#[must_use]
pub(super) fn definition(policy: ToolInvocationPolicy) -> crate::tools::types::ToolDefinition {
    native_definition::<EditInput>(
        TOOL_ID,
        NAME,
        "Apply precise text replacements to a UTF-8 file through the Session's admitted Execution Client. Resolve relative paths from the execution cwd; absolute paths refer to the Client filesystem. Official global skill directories are read-only. All oldText values match the same original snapshot. Exact matches are preferred, with a cautious Unicode-normalized fallback; ambiguous, overlapping, missing, or no-op edits fail without changing the file.",
        policy,
    )
}

pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(policy), std::sync::Arc::new(EditTool))
        .with_normalizer(input::normalize_arguments)
}

/// The native Edit executor.
pub struct EditTool;

impl ToolExecutor for EditTool {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move { run_edit(&invocation, &context).await }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

pub(super) async fn run_edit(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match EditInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let Some(bound) = context.bound_mutation.as_ref() else {
        return failed_result(
            "edit mutation target was not bound at authorization; refusing to resolve independently",
        );
    };
    // Every prepare failure short-circuited at bind — a bound target
    // always holds the live ticketed owner for this exact invocation.
    let owner = bound.prepared();
    if let Err(error) = owner.commit(&context.cancellation).await {
        return crate::tools::native::mutation::commit_error_result(
            &input.path,
            &context.cancellation,
            &error,
        );
    }
    success_text(
        format!(
            "Successfully replaced {} block(s) in {}.",
            input.edits.len(),
            input.path
        ),
        None,
    )
}

fn not_found_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        "Could not find the exact text in".to_owned()
            + &format!(
                " {path}. The old text must match exactly including all whitespace and newlines."
            )
    } else {
        format!(
            "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn ambiguous_error(path: &str, index: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

fn unsafe_fuzzy_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not safely map the fuzzy match in {path} to one original source range. Please provide more context."
        )
    } else {
        format!(
            "Could not safely map edits[{index}] in {path} to one original source range. Please provide more context."
        )
    }
}

/// Renders one typed [`EditFailure`] with this invocation's path and
/// edit count — the historical wording the model sees, unchanged.
pub(crate) fn edit_failure(path: &str, total: usize, failure: EditFailure) -> String {
    let index = |index: u32| usize::try_from(index).unwrap_or(usize::MAX);
    match failure {
        EditFailure::EmptyEdits => "edit requires at least one replacement".to_owned(),
        EditFailure::EmptyAnchor { index: at } => {
            format!(
                "edit requires a non-empty oldText; edits[{}] is empty",
                index(at)
            )
        }
        EditFailure::Missing { index: at } => not_found_error(path, index(at), total),
        EditFailure::Ambiguous {
            index: at,
            occurrences,
        } => ambiguous_error(path, index(at), total, occurrences as usize),
        EditFailure::Overlap { first, second } => format!(
            "edits[{first}] and edits[{second}] overlap in {path}. Merge them into one edit or target disjoint regions."
        ),
        EditFailure::UnsafeNormalization { index: at } => {
            unsafe_fuzzy_error(path, index(at), total)
        }
        EditFailure::NoChange => no_change_error(path, total),
        EditFailure::TooLarge => format!(
            "Could not apply the edit to {path}: the source or the replacement set exceeds a bounded edit resource."
        ),
    }
}
