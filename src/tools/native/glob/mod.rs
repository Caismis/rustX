//! Native Glob tool.
//!
//! Glob keeps rustX's in-process deterministic traversal. It resolves a
//! relative root against the execution cwd, preserves the existing hidden,
//! ignore-file, and symlink policies, and returns sorted POSIX-separated
//! root-relative paths as bounded plain text.

mod input;

use futures_util::future::BoxFuture;

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::NATIVE_FILE_TOOL_MAX_BYTES;
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search::SearchRoot;
use crate::tools::native::support::{failed_result, success_text};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GlobInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "glob";

/// The tool-owned registration of the native Glob tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<GlobInput>(
            "tool-glob",
            NAME,
            "Find files whose path matches a glob pattern using the in-process traversal. Resolve a relative path from the execution cwd; absolute paths are used as host filesystem paths. The optional limit defaults to 1000 and may be larger. Results are plain text, sorted lexically, relative to the search root, and use POSIX separators. Hidden files, ignore-file behavior, and symlink traversal follow rustX's existing policy.",
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

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
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
    let root = match SearchRoot::resolve(context.workspace.root(), input.path.as_deref()) {
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
    let limit = input.limit();
    let mut results = Vec::new();
    let mut bytes = 0usize;
    let mut result_limit_reached = false;
    let mut byte_limit_reached = false;
    for file in files {
        if !matcher.is_match(&file.relative) {
            continue;
        }
        if results.len() as u64 >= limit {
            result_limit_reached = true;
            break;
        }
        let cost = file
            .relative
            .len()
            .saturating_add(usize::from(!results.is_empty()));
        if bytes.saturating_add(cost) > NATIVE_FILE_TOOL_MAX_BYTES {
            byte_limit_reached = true;
            break;
        }
        bytes = bytes.saturating_add(cost);
        results.push(file.relative);
    }

    if results.is_empty() && !result_limit_reached && !byte_limit_reached {
        return success_text("No files found matching pattern", None);
    }
    let mut output = results.join("\n");
    let mut notices = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "{} results limit reached. Use limit={} for more, or refine pattern",
            limit,
            limit.saturating_mul(2)
        ));
    }
    if byte_limit_reached {
        notices.push("50KB limit reached".to_owned());
    }
    if !notices.is_empty() {
        output.push_str("\n\n[");
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
