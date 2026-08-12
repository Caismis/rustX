//! Native Glob tool (M5).
//!
//! A native Rust glob implementation — no shelling out, no implicit
//! gitignore semantics, and directory symlinks are never followed
//! recursively. Results are workspace-relative normalized paths, sorted
//! lexicographically so physical filesystem enumeration order can never
//! become result order, bounded by [`MAX_GLOB_RESULTS`] with explicit
//! truncation reporting.
//!
//! The model-facing argument contract is the typed [`GlobInput`]; the
//! canonical schema is generated from it.

mod input;

use std::path::Path;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::MAX_GLOB_RESULTS;
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{failed_result, success_json_with};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GlobInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "glob";

/// The tool-owned registration of the native Glob tool.
#[must_use]
pub fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<GlobInput>(
            "tool-glob",
            NAME,
            "List workspace-relative paths matching a glob pattern (no gitignore semantics).",
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
    let root = match context.workspace.resolve(&input.path) {
        Ok(root) => root,
        Err(error) => return failed_result(error.to_string()),
    };
    if !root.is_dir() {
        return failed_result(format!(
            "{} is not a directory",
            context.workspace.relative(&root).unwrap_or_default()
        ));
    }
    let mut results = Vec::new();
    for entry in walkdir::WalkDir::new(&root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return failed_result(format!("workspace traversal failed: {error}"));
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        let relative = normalize_relative(relative);
        if matcher.is_match(&relative) {
            results.push(relative);
        }
    }
    results.sort();
    let truncated = results.len() > MAX_GLOB_RESULTS;
    results.truncate(MAX_GLOB_RESULTS);
    let original_count = if truncated {
        MAX_GLOB_RESULTS + 1
    } else {
        results.len()
    };
    let _ = original_count;
    success_json_with(
        serde_json::json!({ "results": results }),
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
        Vec::new(),
    )
}

/// Normalizes a relative path to a deterministic forward-slash string.
fn normalize_relative(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
