//! Native Glob tool (M5).
//!
//! A native Rust glob implementation — no shelling out, no implicit
//! gitignore semantics, and directory symlinks are never followed
//! recursively. Results are workspace-relative normalized paths, sorted
//! lexicographically so physical filesystem enumeration order can never
//! become result order, bounded by [`MAX_GLOB_RESULTS`] with explicit
//! truncation reporting.

use std::path::Path;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::MAX_GLOB_RESULTS;
use crate::tools::native::support::{failed_result, success_json_with};
use crate::tools::native::{NativeToolPolicy, native_definition};
use crate::tools::types::{ToolDefinition, ToolExecutionResult, ToolInvocation, TruncationState};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "glob";

/// The canonical business schema of the tool.
#[must_use]
pub fn definition(policy: NativeToolPolicy) -> ToolDefinition {
    native_definition(
        "tool-glob",
        NAME,
        "List workspace-relative paths matching a glob pattern (no gitignore semantics).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string", "default": "."}
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        policy,
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
    let Some(object) = invocation.arguments.as_object() else {
        return failed_result("glob arguments must be an object");
    };
    let Some(pattern) = object.get("pattern").and_then(serde_json::Value::as_str) else {
        return failed_result("glob requires a string pattern");
    };
    let path = object
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(".");
    let matcher = match globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
    {
        Ok(matcher) => matcher,
        Err(error) => return failed_result(format!("invalid glob pattern {pattern:?}: {error}")),
    };
    let root = match context.workspace.resolve(path) {
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
