//! Native Glob tool (M5).
//!
//! Lists the files of the workspace whose path matches a glob pattern. The
//! file universe comes from the shared native-search substrate
//! ([`crate::tools::native::search`]), which is the same universe Grep
//! observes: no shelling out, no implicit ignore-file semantics, hidden
//! files visible, and symlinks never followed. Results are normalized paths
//! relative to the search root, sorted lexicographically so physical
//! filesystem enumeration order can never become result order.
//!
//! The result is bounded twice: by [`MAX_GLOB_RESULTS`] entries and by
//! [`MAX_MODEL_TOOL_RESULT_BYTES`] of **actually serialized** model-facing
//! JSON. The byte budget is charged the exact serialization of each path
//! plus its array separator, on top of a measured envelope, so JSON escaping
//! of quotes, backslashes, and control characters inside a filename can
//! never push the delivered payload past the cap. Reaching either bound is
//! reported explicitly and nothing is dropped silently. Ordering is never by
//! modification time.
//!
//! The model-facing argument contract is the typed [`GlobInput`]; the
//! canonical schema is generated from it.

mod input;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_GLOB_RESULTS, MAX_MODEL_TOOL_RESULT_BYTES};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search::SearchRoot;
use crate::tools::native::support::{
    failed_result, json_array_element_cost, json_bytes, success_json_with,
};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GlobInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "glob";

/// The serialized size of the result envelope with an empty result array.
///
/// It is measured, not estimated, and it uses `"truncated": false` because
/// `false` serializes one byte longer than `true`: whichever value the run
/// finally reports, the real envelope is no larger than the reserved one.
fn envelope_bytes() -> usize {
    json_bytes(&serde_json::json!({ "results": [], "truncated": false }))
}

/// The tool-owned registration of the native Glob tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<GlobInput>(
            "tool-glob",
            NAME,
            "Find files whose path matches a glob pattern. The optional path is an absolute \
             directory locator inside the workspace root or the read-only managed tool-output \
             root; omit it to search the workspace root. Returns paths relative to \
             the search root in lexical order. Hidden files are included and ignore files such as \
             .gitignore are not applied.",
            policy,
        ),
        std::sync::Arc::new(GlobTool),
    )
}

/// The native Glob executor.
pub struct GlobTool;

impl ToolExecutor for GlobTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_glob(&invocation, &context) })
    }
}

fn run_glob(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match GlobInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let pattern = input.pattern.as_str();
    let matcher = match globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
    {
        Ok(matcher) => matcher,
        Err(error) => return failed_result(format!("invalid glob pattern {pattern:?}: {error}")),
    };
    let root = match SearchRoot::resolve(
        context.workspace,
        context.tool_output,
        input.path.as_deref(),
    ) {
        Ok(root) => root,
        Err(error) => return failed_result(error),
    };
    if root.is_file() {
        return failed_result("glob searches a directory; the path names a single file");
    }
    let files = match root.files() {
        Ok(files) => files,
        Err(error) => return failed_result(error),
    };

    // The shared traversal already yields the universe in lexical order of
    // the normalized relative path, so filtering preserves that order.
    //
    // The count cap is checked before the byte cap so that which results are
    // dropped stays a function of the ordering alone, never of how expensive
    // a path happens to be to serialize.
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut truncated = false;
    let mut payload = envelope_bytes();
    for file in files {
        if !matcher.is_match(&file.relative) {
            continue;
        }
        if results.len() >= MAX_GLOB_RESULTS {
            truncated = true;
            break;
        }
        let entry = serde_json::Value::String(file.relative);
        let cost = json_array_element_cost(json_bytes(&entry), results.len());
        if payload.saturating_add(cost) > MAX_MODEL_TOOL_RESULT_BYTES {
            truncated = true;
            break;
        }
        payload += cost;
        results.push(entry);
    }
    success_json_with(
        serde_json::json!({ "results": results, "truncated": truncated }),
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
        Vec::new(),
    )
}
