//! Native Glob tool (M5).
//!
//! Lists the files of the workspace whose path matches a glob pattern. The
//! file universe comes from the shared native-search substrate
//! ([`crate::tools::native::search`]), which is the same universe Grep
//! observes: no shelling out, no implicit ignore-file semantics, hidden
//! files visible, and symlinks never followed. Results are normalized paths
//! relative to the search root, sorted lexicographically so physical
//! filesystem enumeration order can never become result order, and bounded
//! by [`MAX_GLOB_RESULTS`] and [`MAX_MODEL_TOOL_RESULT_BYTES`] with explicit
//! truncation reporting. Ordering is never by modification time.
//!
//! The model-facing argument contract is the typed [`GlobInput`]; the
//! canonical schema is generated from it.

mod input;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_GLOB_RESULTS, MAX_MODEL_TOOL_RESULT_BYTES};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search::SearchRoot;
use crate::tools::native::support::{failed_result, success_json_with};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GlobInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "glob";

/// The serialized cost one result path adds beyond its own bytes (the two
/// JSON quotes and the separating comma). The byte cap is a deterministic
/// bound on the model-facing payload, not an exact serialization measure.
const RESULT_ENVELOPE_BYTES: usize = 3;

/// The tool-owned registration of the native Glob tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<GlobInput>(
            "tool-glob",
            NAME,
            "Find workspace files whose path matches a glob pattern. Returns paths relative to \
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
    let root = match SearchRoot::resolve(context.workspace, input.path.as_deref()) {
        Ok(root) => root,
        Err(error) => return failed_result(error),
    };
    let files = match root.files() {
        Ok(files) => files,
        Err(error) => return failed_result(error),
    };

    // The shared traversal already yields the universe in lexical order of
    // the normalized relative path, so filtering preserves that order.
    let mut results: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut bytes = 0usize;
    for file in files {
        if !matcher.is_match(&file.relative) {
            continue;
        }
        if results.len() >= MAX_GLOB_RESULTS {
            truncated = true;
            break;
        }
        bytes += file.relative.len() + RESULT_ENVELOPE_BYTES;
        if bytes > MAX_MODEL_TOOL_RESULT_BYTES {
            truncated = true;
            break;
        }
        results.push(file.relative);
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
