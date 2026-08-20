//! Native Read tool (M5).
//!
//! Reads a UTF-8 text file with a deterministic line window. `offset` is the
//! 1-based first line and `limit` bounds the number of lines; both are
//! optional and default to `offset = 1`, `limit = 200`. Output is bounded by
//! [`MAX_MODEL_TOOL_RESULT_BYTES`]; invalid UTF-8 or binary input fails
//! explicitly rather than fabricating text. The model-facing `file_path` is
//! an absolute locator resolved through the one filesystem authority
//! ([`crate::tools::locator`]): the workspace root and the read-only managed
//! tool-output root are readable, nothing else.
//!
//! The model-facing argument contract is the typed [`ReadInput`]; the
//! canonical schema is generated from it.

mod input;

use futures_util::future::BoxFuture;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_MODEL_TOOL_RESULT_BYTES, bounded_text_preview};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolResultContent, TruncationState,
};

use input::ReadInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "read";

/// The tool-owned registration of the native Read tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<ReadInput>(
            "tool-read",
            NAME,
            "Read a UTF-8 text file at an absolute path. The path must resolve inside the \
             workspace root or the read-only managed tool-output root. Returns a line window \
             starting at the 1-based offset (default 1) of at most limit lines (default 200).",
            policy,
        ),
        std::sync::Arc::new(ReadTool),
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
    let input = match ReadInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed(error),
    };
    let (offset, limit) = (input.offset(), input.limit());
    let resolved = crate::tools::locator::resolve(
        context.workspace,
        context.tool_output,
        &input.file_path,
        crate::tools::locator::LocatorOperation::Read,
    );
    let resolved = match resolved {
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
            resolved.display()
        ));
    };
    let lines: Vec<&str> = text.lines().collect();
    let slice = line_window(&lines, offset, limit);
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
        managed_output: None,
    }
}

/// The deterministic 1-based line window: `offset` selects the first line
/// (1 = the first line of the file) and `limit` bounds how many lines the
/// window contains. An offset past the end of the file yields an empty
/// window rather than an error.
fn line_window<'a>(lines: &'a [&'a str], offset: u64, limit: u64) -> Vec<&'a str> {
    if offset == 0 {
        return Vec::new();
    }
    let start = usize::try_from(offset.saturating_sub(1)).unwrap_or(usize::MAX);
    if start >= lines.len() {
        return Vec::new();
    }
    let end = start
        .saturating_add(usize::try_from(limit).unwrap_or(usize::MAX))
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
        managed_output: None,
    }
}
