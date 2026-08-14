//! Native Edit tool (M5).
//!
//! Applies a set of exact text replacements to one UTF-8 file inside the
//! workspace.
//!
//! # The atomic multi-edit invariant
//!
//! > One Edit invocation describes one atomic transformation from one
//! > original file snapshot to one final file snapshot.
//!
//! Concretely, for every invocation:
//!
//! 1. exactly one original snapshot of the file is read;
//! 2. every `oldText` is matched against *that* snapshot — never against a
//!    partially edited intermediate, so the edits are order-independent and
//!    an earlier replacement can never change what a later one matches;
//! 3. every `oldText` must resolve to exactly one range in the snapshot;
//!    zero matches and more than one match are both deterministic failures;
//! 4. the complete replacement range set is computed before any mutation and
//!    rejected when any two ranges intersect, nest, or coincide;
//! 5. the validated ranges are ordered by their position in the snapshot and
//!    the final snapshot is built from the original plus those replacements;
//! 6. the final snapshot is committed as exactly one file mutation through
//!    the shared atomic commit of the native tool plane.
//!
//! Any validation failure leaves the file byte-for-byte unchanged: the
//! commit is only ever reached with a fully validated edit set. There is no
//! sequential "edit 1 mutates, edit 2 matches the mutated file" mode, no
//! replace-all mode, and no fuzzy/LLM edit matching.
//!
//! The model-facing argument contract is the typed [`EditInput`]; the
//! canonical schema is generated from it.

mod input;

use std::ops::Range;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{atomic_commit, failed_result, success_json};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation};

use input::{EditInput, EditReplacement};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "edit";

/// The tool-owned registration of the native Edit tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<EditInput>(
            "tool-edit",
            NAME,
            "Apply exact text replacements to one UTF-8 file inside the workspace. Every oldText \
             must occur exactly once in the file as it is now, and all edits are applied together \
             as one atomic change.",
            policy,
        ),
        std::sync::Arc::new(EditTool),
    )
}

/// The native Edit executor.
pub struct EditTool;

impl ToolExecutor for EditTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_edit(&invocation, &context) })
    }
}

fn run_edit(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match EditInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let target = match context.workspace.resolve(&input.path) {
        Ok(target) => target,
        Err(error) => return failed_result(error.to_string()),
    };
    let relative = context.workspace.relative(&target).unwrap_or_default();
    // (1) One original snapshot; every later step reads only from it.
    let bytes = match std::fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failed_result(format!("cannot read {}: {error}", target.display()));
        }
    };
    let Ok(original) = String::from_utf8(bytes) else {
        return failed_result(format!(
            "{relative} is not a UTF-8 text file; Edit never operates on binary content"
        ));
    };
    // (2)-(4) The whole edit set is validated before anything is mutated.
    let planned = match plan(&original, &input.edits, &relative) {
        Ok(planned) => planned,
        Err(error) => return failed_result(error),
    };
    // (5) One final snapshot built from the original plus the planned
    // replacements, and (6) exactly one committed mutation.
    let updated = apply(&original, &planned);
    if let Err(error) = atomic_commit(&target, updated.as_bytes()) {
        return failed_result(error);
    }
    success_json(serde_json::json!({
        "path": relative,
        "replacements": planned.len(),
    }))
}

/// One validated replacement: where it applies in the original snapshot and
/// what it puts there.
struct PlannedEdit<'a> {
    /// The byte range of the original snapshot this edit replaces.
    range: Range<usize>,
    /// The replacement text.
    replacement: &'a str,
}

/// Resolves every replacement against the original snapshot and validates
/// the resulting range set.
///
/// # Errors
///
/// Returns the deterministic diagnostic of the first violation: a zero-match
/// anchor, an ambiguous anchor, or a pair of conflicting ranges. On any
/// error the caller must not mutate the file.
fn plan<'a>(
    original: &str,
    edits: &'a [EditReplacement],
    relative: &str,
) -> Result<Vec<PlannedEdit<'a>>, String> {
    let mut planned: Vec<(usize, PlannedEdit<'a>)> = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        // Every anchor is resolved against the original snapshot, never
        // against a partially edited intermediate.
        let mut occurrences = original.match_indices(edit.old_text.as_str());
        let Some((start, found)) = occurrences.next() else {
            return Err(format!(
                "edits[{index}]: oldText not found in {relative}; no edit was applied"
            ));
        };
        if occurrences.next().is_some() {
            let total = original.matches(edit.old_text.as_str()).count();
            return Err(format!(
                "edits[{index}]: oldText occurs {total} times in {relative}; it must identify \
                 exactly one place, so no edit was applied"
            ));
        }
        planned.push((
            index,
            PlannedEdit {
                range: start..start + found.len(),
                replacement: edit.new_text.as_str(),
            },
        ));
    }
    // The deterministic position order of the replacements. Sorting by the
    // range makes the outcome independent of the order the edits arrived in;
    // the original input index only ever appears in diagnostics.
    planned.sort_by(|left, right| {
        left.1
            .range
            .start
            .cmp(&right.1.range.start)
            .then(left.1.range.end.cmp(&right.1.range.end))
            .then(left.0.cmp(&right.0))
    });
    // Intersecting, nested, and coinciding ranges are all rejected by the
    // same rule: in position order, each range must start at or after the
    // end of the previous one.
    for pair in planned.windows(2) {
        let (previous_index, previous) = (pair[0].0, &pair[0].1);
        let (next_index, next) = (pair[1].0, &pair[1].1);
        if next.range.start < previous.range.end {
            return Err(format!(
                "edits[{previous_index}] and edits[{next_index}] describe conflicting changes to \
                 the same region of {relative} (bytes {}..{} and {}..{}); no edit was applied",
                previous.range.start, previous.range.end, next.range.start, next.range.end
            ));
        }
    }
    Ok(planned.into_iter().map(|(_, edit)| edit).collect())
}

/// Builds the final snapshot from the original snapshot plus the validated,
/// position-ordered, disjoint replacements.
fn apply(original: &str, planned: &[PlannedEdit<'_>]) -> String {
    let mut updated = String::with_capacity(original.len());
    let mut cursor = 0usize;
    for edit in planned {
        updated.push_str(&original[cursor..edit.range.start]);
        updated.push_str(edit.replacement);
        cursor = edit.range.end;
    }
    updated.push_str(&original[cursor..]);
    updated
}
