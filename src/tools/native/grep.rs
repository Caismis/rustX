//! Native Grep tool (M5).
//!
//! A native Rust regex search over the workspace — no `grep`/`rg`
//! subprocess. Traversal is deterministic, directory symlinks are never
//! followed recursively, and non-UTF-8 files are skipped consistently.
//! Matches are ordered by relative path → line number → column, bounded by
//! [`MAX_GREP_MATCHES`] and [`MAX_MODEL_TOOL_RESULT_BYTES`] with explicit
//! truncation state.

use futures_util::future::BoxFuture;
use regex::RegexBuilder;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_GREP_MATCHES, MAX_MODEL_TOOL_RESULT_BYTES};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{failed_result, success_json_with};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "grep";

/// The tool-owned registration of the native Grep tool.
#[must_use]
pub fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition(
            "tool-grep",
            NAME,
            "Search workspace files with a Rust regex, ordered by path, line, and column.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "default": "."},
                    "glob": {"type": "string", "default": "**/*"},
                    "case_sensitive": {"type": "boolean", "default": true}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            policy,
        ),
        std::sync::Arc::new(GrepTool),
    )
}

/// The native Grep executor.
pub struct GrepTool;

impl ToolExecutor for GrepTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_grep(&invocation, &context) })
    }
}

/// One match with deterministic ordering fields.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct Match {
    path: String,
    line_number: u64,
    column: u64,
    text: String,
}

fn run_grep(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let Some(object) = invocation.arguments.as_object() else {
        return failed_result("grep arguments must be an object");
    };
    let Some(pattern) = object.get("pattern").and_then(serde_json::Value::as_str) else {
        return failed_result("grep requires a string pattern");
    };
    let search_path = object
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(".");
    let glob = object
        .get("glob")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("**/*");
    let case_sensitive = object
        .get("case_sensitive")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let regex = match RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => return failed_result(format!("invalid regex {pattern:?}: {error}")),
    };
    let matcher = match globset::GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
    {
        Ok(matcher) => matcher,
        Err(error) => return failed_result(format!("invalid glob {glob:?}: {error}")),
    };
    let root = match context.workspace.resolve(search_path) {
        Ok(root) => root,
        Err(error) => return failed_result(error.to_string()),
    };
    if !root.is_dir() {
        return failed_result(format!(
            "{} is not a directory",
            context.workspace.relative(&root).unwrap_or_default()
        ));
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return failed_result(format!("workspace traversal failed: {error}")),
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = match entry.path().strip_prefix(&root) {
            Ok(relative) => normalize_relative(relative),
            Err(_) => continue,
        };
        if matcher.is_match(&relative) {
            files.push((relative, entry.path().to_path_buf()));
        }
    }
    files.sort();
    let mut matched_results = Vec::new();
    let mut truncated_by_count = false;
    let mut truncated_by_bytes = false;
    let mut bytes = 0usize;
    'files: for (relative, entry_path) in &files {
        let _ = (entry_path, relative);
        let Ok(raw_bytes) = std::fs::read(entry_path) else {
            continue;
        };
        let Ok(file_text) = String::from_utf8(raw_bytes) else {
            // Non-UTF-8/binary files are skipped consistently.
            continue;
        };
        for (line_index, line) in file_text.lines().enumerate() {
            for found in regex.find_iter(line) {
                let entry = Match {
                    path: relative.clone(),
                    line_number: line_index as u64 + 1,
                    column: found.start() as u64 + 1,
                    text: found.as_str().to_owned(),
                };
                bytes += entry.text.len() + entry.path.len() + 16;
                if matched_results.len() >= MAX_GREP_MATCHES {
                    truncated_by_count = true;
                    break 'files;
                }
                if bytes >= MAX_MODEL_TOOL_RESULT_BYTES {
                    truncated_by_bytes = true;
                    break 'files;
                }
                matched_results.push(entry);
            }
        }
    }
    let truncated = truncated_by_count || truncated_by_bytes;
    success_json_with(
        serde_json::json!({ "matches": matched_results }),
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
        Vec::new(),
    )
}

/// Normalizes a relative path to a deterministic forward-slash string.
fn normalize_relative(path: &std::path::Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
