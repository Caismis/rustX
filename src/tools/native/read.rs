//! Native Read tool (M5).
//!
//! Reads a UTF-8 text file with deterministic line slicing. `start_line` is
//! 1-based and `line_count` bounds the number of lines; both are optional.
//! Output is bounded by [`MAX_MODEL_TOOL_RESULT_BYTES`]; invalid UTF-8 or
//! binary input fails explicitly rather than fabricating text. Path
//! resolution goes through the workspace contract — nothing outside the
//! workspace is ever read.

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_MODEL_TOOL_RESULT_BYTES, bounded_text_preview};
use crate::tools::native::{NativeToolPolicy, native_definition};
use crate::tools::types::{
    ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolResultContent,
    TruncationState,
};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "read";

/// The canonical business schema of the tool.
#[must_use]
pub fn definition(policy: NativeToolPolicy) -> ToolDefinition {
    native_definition(
        "tool-read",
        NAME,
        "Read a UTF-8 text file inside the workspace with 1-based line slicing.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "start_line": {"type": "integer", "minimum": 1},
                "line_count": {"type": "integer", "minimum": 1}
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        policy,
    )
}

/// The native Read executor.
pub struct ReadTool;

impl ToolExecutor for ReadTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_read(&invocation, &context) })
    }
}

fn run_read(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let Some(object) = invocation.arguments.as_object() else {
        return failed("read arguments must be an object");
    };
    let Some(path) = object.get("path").and_then(serde_json::Value::as_str) else {
        return failed("read requires a string path");
    };
    let start_line = object
        .get("start_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let line_count = object
        .get("line_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(200);
    let resolved = match context.workspace.resolve(path) {
        Ok(resolved) => resolved,
        Err(error) => return failed(error.to_string()),
    };
    let bytes = match std::fs::read(&resolved) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failed(format!("cannot read {}: {error}", resolved.display()));
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return failed(format!(
            "{} is not a UTF-8 text file; binary content is never fabricated as text",
            context.workspace.relative(&resolved).unwrap_or_default()
        ));
    };
    let lines: Vec<&str> = text.lines().collect();
    let slice = slice_lines(&lines, start_line, line_count);
    let output = slice.join("\n");
    let (preview, truncated) = bounded_text_preview(output.as_bytes(), MAX_MODEL_TOOL_RESULT_BYTES);
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Text(
            crate::message::content::TextBlock { text: preview },
        )],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: Some(output.len() as u64),
        }),
    }
}

/// Deterministic 1-based line slicing: `start_line` selects the first line
/// (1 = first line) and `line_count` bounds the number of selected lines.
fn slice_lines<'a>(lines: &'a [&'a str], start_line: u64, line_count: u64) -> Vec<&'a str> {
    if start_line == 0 {
        return Vec::new();
    }
    let start = usize::try_from(start_line.saturating_sub(1)).unwrap_or(usize::MAX);
    if start >= lines.len() {
        return Vec::new();
    }
    let end = start
        .saturating_add(usize::try_from(line_count).unwrap_or(usize::MAX))
        .min(lines.len());
    lines[start..end].to_vec()
}

fn failed(error: impl Into<String>) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: error.into(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}
