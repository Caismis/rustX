//! Result materialization: one remote `CallToolResult` or an outcome without
//! one becomes the canonical [`ToolExecutionResult`], under the shared Tool
//! Plane capture and MCP's own aggregate content accounting.

use std::path::PathBuf;
use std::time::Instant;

use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};

use crate::tools::artifacts::ArtifactStore;
use crate::tools::executor::ToolExecutionContext;
use crate::tools::limits::{FOREGROUND_TOOL_RESULT_PREVIEW_BYTES, MAX_MODEL_TOOL_RESULT_BYTES};
use crate::tools::output::{
    CapturedOutput, TextPreviewCapture, ToolOutputCapture, ToolOutputWriter,
    continuation_for_capture, truncation_for_capture,
};
use crate::tools::types::{
    ManagedOutputContinuation, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
};

use super::{bound_error, duration_ms};

pub(super) fn translate_result(
    result: CallToolResult,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    let background = context.execution_id.is_some();
    let mut capture = match open_mcp_output_capture(context) {
        Ok(capture) => capture,
        Err((locator, diagnostic)) => {
            return failed_mcp_storage(&diagnostic, locator, started);
        }
    };
    let reported_error = result.is_error == Some(true);
    let captured = capture_mcp_content(
        result.content,
        result.structured_content,
        context,
        &mut capture,
    );
    let capture_result = capture.finish(captured.storage_error.is_none());
    let output_diagnostic = captured
        .storage_error
        .as_deref()
        .or(captured.unsupported.as_deref());
    let continuation = continuation_for_capture(&capture_result, background, output_diagnostic);
    let (model_blocks, aggregate_error) =
        materialize_mcp_blocks(captured.pending_blocks, &capture_result, context.artifacts);
    let unsupported = captured.unsupported.or(aggregate_error);
    let status = if let Some(error) = captured.storage_error {
        ToolExecutionStatus::Failed {
            error: format!("MCP result output storage failed: {error}"),
        }
    } else if let Some(error) = unsupported {
        ToolExecutionStatus::Failed { error }
    } else if reported_error {
        ToolExecutionStatus::Failed {
            error: "MCP tool reported an execution error".to_owned(),
        }
    } else {
        ToolExecutionStatus::Success
    };
    let truncation = truncation_for_capture(&capture_result);
    ToolExecutionResult {
        deliveries: Vec::new(),
        status,
        content: model_blocks,
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation,
        workflow: None,
        managed_output: continuation.map(Box::new),
    }
}

/// The one MCP-owned accounting state for materialized result content.
///
/// Textual overflow is normalized by the shared Tool Plane capture first; the
/// complete spill is auxiliary and therefore does not consume this budget.
/// Every materialized MCP content component consumes this same counter,
/// including semantic image content. The complete status/content/continuation
/// model-facing aggregate is bounded later by
/// [`ToolExecutionResult::model_facing_projection`].
#[derive(Debug)]
struct McpAggregateBudget {
    remaining: usize,
}

const MCP_AGGREGATE_BUDGET_ERROR: &str =
    "MCP result content exceeds the aggregate model-facing result limit";
const MCP_IMAGE_AGGREGATE_BUDGET_ERROR: &str =
    "MCP image content exceeds the aggregate model-facing result limit";

impl McpAggregateBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_MODEL_TOOL_RESULT_BYTES,
        }
    }

    fn consume(&mut self, bytes: usize) -> bool {
        if bytes > self.remaining {
            return false;
        }
        self.remaining -= bytes;
        true
    }
}

enum McpPendingBlock {
    Text(String),
    Json(serde_json::Value),
    Image(Vec<u8>),
}

/// Materializes semantic MCP blocks under MCP's content accounting. The
/// provider-independent complete result projection applies the final
/// aggregate model-facing bound after status and continuation are known.
fn materialize_mcp_blocks(
    pending_blocks: Vec<McpPendingBlock>,
    capture_result: &CapturedOutput,
    artifacts: &ArtifactStore,
) -> (Vec<ToolResultContent>, Option<String>) {
    let textual_overflow = capture_result.truncated || !capture_result.complete;
    let mut budget = McpAggregateBudget::new();
    let mut model_blocks = Vec::new();
    let mut aggregate_error = None;

    if textual_overflow {
        // The shared capture still owns the complete textual spill and its
        // aggregate preview state. The model-facing mixed projection needs a
        // separate bounded text budget, however: replacing all textual
        // blocks with one aggregate Text would move Images across their
        // source-relative semantic boundaries.
        let mut remaining_textual_preview = FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        for pending in pending_blocks {
            match pending {
                McpPendingBlock::Text(text) => consume_mcp_text_preview(
                    &text,
                    &mut remaining_textual_preview,
                    &mut budget,
                    &mut model_blocks,
                    &mut aggregate_error,
                ),
                McpPendingBlock::Json(value) => consume_mcp_json_preview(
                    &value,
                    &mut remaining_textual_preview,
                    &mut budget,
                    &mut model_blocks,
                    &mut aggregate_error,
                ),
                McpPendingBlock::Image(bytes) => consume_mcp_image(
                    &bytes,
                    &mut budget,
                    &mut model_blocks,
                    &mut aggregate_error,
                    artifacts,
                ),
            }
        }
    } else {
        for pending in pending_blocks {
            match pending {
                McpPendingBlock::Text(text) => {
                    consume_mcp_text(text, &mut budget, &mut model_blocks, &mut aggregate_error);
                }
                McpPendingBlock::Json(value) => {
                    consume_mcp_json(value, &mut budget, &mut model_blocks, &mut aggregate_error);
                }
                McpPendingBlock::Image(bytes) => {
                    consume_mcp_image(
                        &bytes,
                        &mut budget,
                        &mut model_blocks,
                        &mut aggregate_error,
                        artifacts,
                    );
                }
            }
        }
    }

    (model_blocks, aggregate_error)
}

fn consume_mcp_text_preview(
    text: &str,
    remaining_textual_preview: &mut usize,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    if *remaining_textual_preview < 2 {
        if *remaining_textual_preview == 1
            && let Some(character) = text.chars().next()
            && character.len_utf8() == 1
        {
            *remaining_textual_preview = 0;
            consume_mcp_text(character.to_string(), budget, model_blocks, aggregate_error);
        }
        return;
    }
    let mut preview = TextPreviewCapture::new(*remaining_textual_preview);
    preview.push(text);
    let (text, _) = preview.finish();
    *remaining_textual_preview = (*remaining_textual_preview).saturating_sub(text.len());
    if !text.is_empty() {
        consume_mcp_text(text, budget, model_blocks, aggregate_error);
    }
}

fn consume_mcp_json_preview(
    value: &serde_json::Value,
    remaining_textual_preview: &mut usize,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    match serde_json::to_string(value) {
        Ok(text) => consume_mcp_text_preview(
            &text,
            remaining_textual_preview,
            budget,
            model_blocks,
            aggregate_error,
        ),
        Err(error) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(format!("invalid MCP structured content: {error}"));
            }
        }
    }
}

fn consume_mcp_text(
    text: String,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    if budget.consume(text.len()) {
        model_blocks.push(ToolResultContent::Text(
            crate::message::content::TextBlock { text },
        ));
    } else if aggregate_error.is_none() {
        *aggregate_error = Some(MCP_AGGREGATE_BUDGET_ERROR.to_owned());
    }
}

fn consume_mcp_json(
    value: serde_json::Value,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    match serde_json::to_vec(&value) {
        Ok(bytes) if budget.consume(bytes.len()) => {
            model_blocks.push(ToolResultContent::Json { value });
        }
        Ok(_) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(MCP_AGGREGATE_BUDGET_ERROR.to_owned());
            }
        }
        Err(error) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(format!("invalid MCP structured content: {error}"));
            }
        }
    }
}

fn consume_mcp_image(
    bytes: &[u8],
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
    artifacts: &ArtifactStore,
) {
    if !budget.consume(bytes.len()) {
        if aggregate_error.is_none() {
            *aggregate_error = Some(MCP_IMAGE_AGGREGATE_BUDGET_ERROR.to_owned());
        }
        return;
    }
    match write_artifact(artifacts, bytes) {
        Ok(id) => model_blocks.push(ToolResultContent::Image(
            crate::message::content::ImageReference {
                artifact_id: id,
                alt: None,
            },
        )),
        Err(error) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(error);
            }
        }
    }
}

fn open_mcp_output_capture(
    context: &ToolExecutionContext<'_>,
) -> Result<ToolOutputCapture, (PathBuf, String)> {
    let Some(execution_id) = context.execution_id else {
        return Ok(ToolOutputCapture::foreground());
    };
    let locator = context.tool_output.background_output_path(execution_id);
    let sink = context
        .tool_output
        .open_background_output_sink(execution_id)
        .map_err(|error| {
            (
                locator,
                format!("cannot open the background MCP result output: {error}"),
            )
        })?;
    Ok(ToolOutputCapture::background(sink, None))
}

struct McpContentCapture {
    pending_blocks: Vec<McpPendingBlock>,
    unsupported: Option<String>,
    storage_error: Option<String>,
}

fn capture_mcp_content(
    blocks: Vec<ContentBlock>,
    structured_content: Option<serde_json::Value>,
    context: &ToolExecutionContext<'_>,
    capture: &mut ToolOutputCapture,
) -> McpContentCapture {
    let mut captured = McpContentCapture {
        pending_blocks: Vec::new(),
        unsupported: None,
        storage_error: None,
    };
    let mut first_textual_block = true;
    {
        let mut push_textual = |text: &str| -> Result<(), String> {
            if !first_textual_block {
                capture.push(
                    "\n",
                    (!capture.is_background()).then_some(context.tool_output),
                )?;
            }
            first_textual_block = false;
            capture.push(
                text,
                (!capture.is_background()).then_some(context.tool_output),
            )
        };
        for block in blocks {
            match block {
                ContentBlock::Text(text) => {
                    if let Err(error) = push_textual(&text.text) {
                        captured.storage_error = Some(error);
                    }
                    captured
                        .pending_blocks
                        .push(McpPendingBlock::Text(text.text));
                }
                ContentBlock::Image(image) => {
                    match base64::engine::general_purpose::STANDARD.decode(image.data.as_bytes()) {
                        Ok(bytes) => captured.pending_blocks.push(McpPendingBlock::Image(bytes)),
                        Err(error) => {
                            captured.unsupported = Some(format!("invalid MCP image data: {error}"));
                        }
                    }
                }
                ContentBlock::Audio(_)
                | ContentBlock::Resource(_)
                | ContentBlock::ResourceLink(_) => {
                    captured.unsupported =
                        Some("MCP content variant has no canonical M7 representation".to_owned());
                }
                _ => {
                    captured.unsupported =
                        Some("unknown MCP content variant is unsupported in M7".to_owned());
                }
            }
        }
    }
    if let Some(value) = structured_content {
        if !first_textual_block
            && let Err(error) = capture.push(
                "\n",
                (!capture.is_background()).then_some(context.tool_output),
            )
        {
            captured.storage_error = Some(error);
        }
        if captured.storage_error.is_none() {
            let foreground_store = (!capture.is_background()).then_some(context.tool_output);
            let mut writer = ToolOutputWriter::new(capture, foreground_store);
            let write_result = serde_json::to_writer(&mut writer, &value)
                .map_err(|error| format!("invalid MCP structured content: {error}"))
                .and_then(|()| writer.finish());
            if let Err(error) = write_result {
                captured.storage_error = Some(error);
            }
        }
        captured.pending_blocks.push(McpPendingBlock::Json(value));
    }
    captured
}

pub(super) fn write_artifact(
    artifacts: &ArtifactStore,
    bytes: &[u8],
) -> Result<crate::runtime::identity::ArtifactId, String> {
    use std::io::Write;
    let id = artifacts
        .create_artifact()
        .map_err(|error| error.to_string())?;
    let mut writer = artifacts
        .open_writer(&id)
        .map_err(|error| error.to_string())?;
    writer.write_all(bytes).map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(id)
}

fn failed_mcp_storage(diagnostic: &str, locator: PathBuf, started: Instant) -> ToolExecutionResult {
    let diagnostic = bound_error(diagnostic);
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Failed {
            error: format!("MCP result output storage failed: {diagnostic}"),
        },
        content: Vec::new(),
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: Some(Box::new(
            crate::tools::types::ManagedOutputContinuation::Partial {
                locator,
                diagnostic,
            },
        )),
    }
}

pub(super) fn failed_mcp(
    message: &str,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    mcp_empty_terminal(
        ToolExecutionStatus::Failed {
            error: bound_error(message),
        },
        context,
        started,
    )
}

/// Retains the dispatch-owned background locator for MCP outcomes that do
/// not carry a remote `CallToolResult` (unknown outcomes after dispatch or
/// a local protocol error). A healthy preallocated empty file is a complete
/// record of a call that produced no logical result; a sink-open failure is
/// explicitly Partial.
pub(super) fn mcp_empty_terminal(
    status: ToolExecutionStatus,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    let Some(execution_id) = context.execution_id else {
        return ToolExecutionResult {
            deliveries: Vec::new(),
            status,
            content: Vec::new(),
            duration_ms: duration_ms(started),
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        };
    };
    let locator = context.tool_output.background_output_path(execution_id);
    match context
        .tool_output
        .open_background_output_sink(execution_id)
    {
        Ok(sink) => {
            drop(sink);
            ToolExecutionResult {
                deliveries: Vec::new(),
                status,
                content: Vec::new(),
                duration_ms: duration_ms(started),
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: Some(Box::new(ManagedOutputContinuation::Complete { locator })),
            }
        }
        Err(error) => {
            let diagnostic = format!("cannot open the background MCP result output: {error}");
            // An output-storage failure never rewrites execution-outcome
            // certainty: a known failure gains the storage diagnostic, while
            // any other status keeps its own certainty claim and reports the
            // storage failure only through the managed-output continuation.
            let status = match status {
                ToolExecutionStatus::Failed { error } => ToolExecutionStatus::Failed {
                    error: bound_error(&format!(
                        "{error}; MCP result output storage failed: {diagnostic}"
                    )),
                },
                status => status,
            };
            ToolExecutionResult {
                deliveries: Vec::new(),
                status,
                content: Vec::new(),
                duration_ms: duration_ms(started),
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: Some(Box::new(ManagedOutputContinuation::Partial {
                    locator,
                    diagnostic: bound_error(&diagnostic),
                })),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use base64::Engine as _;
    use rmcp::model::{CallToolResult, ContentBlock};

    use super::{mcp_empty_terminal, translate_result};
    use crate::runtime::identity::ToolExecutionId;
    use crate::runtime::types::CancellationReason;
    use crate::tools::mcp::test_context::{NoProgress, context, runtime};
    use crate::tools::types::{ManagedOutputContinuation, ToolExecutionStatus, ToolResultContent};

    fn image_block(bytes: usize) -> ContentBlock {
        ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(vec![b'i'; bytes]),
            "image/png",
        )
    }

    fn content_kinds(content: &[ToolResultContent]) -> Vec<&'static str> {
        content
            .iter()
            .map(|content| match content {
                ToolResultContent::Text(_) => "text",
                ToolResultContent::Json { .. } => "json",
                ToolResultContent::File(_) => "file",
                ToolResultContent::Image(_) => "image",
            })
            .collect()
    }

    /// Issue #240: `structuredContent` is arbitrary JSON at the adapter
    /// boundary and stays arbitrary JSON in the canonical projection.
    ///
    /// The canonical representation is deliberately untyped
    /// (`ToolResultContent::Json`): rustX projects the remote value
    /// verbatim and derives nothing from the tool's `outputSchema`. A scalar
    /// and an array are therefore ordinary values here, not shapes some
    /// object-only narrowing has to be taught about.
    #[test]
    fn mcp_structured_content_is_not_narrowed_to_a_json_object() {
        for value in [
            serde_json::json!(42),
            serde_json::json!([1, 2, 3]),
            serde_json::json!("text"),
            serde_json::json!(null),
            serde_json::json!({"nested": true}),
        ] {
            let (_directory, runtime) = runtime("conv_469d6f38-f702-7a09-ad3e-64df30fb0cab");
            let progress = NoProgress;
            let mut call = CallToolResult::success(Vec::new());
            call.structured_content = Some(value.clone());
            let result =
                translate_result(call, &context(&runtime, None, &progress), Instant::now());

            assert_eq!(
                result.status,
                ToolExecutionStatus::Success,
                "structured content of shape {value} is accepted"
            );
            assert_eq!(content_kinds(&result.content), vec!["json"]);
            assert!(
                matches!(
                    result.content.first(),
                    Some(ToolResultContent::Json { value: projected }) if *projected == value
                ),
                "the remote value is projected verbatim: {:?}",
                result.content
            );
        }
    }

    /// Issue #240: the MCP 2026 complete result framing — a `resultType` of
    /// `complete` alongside both content and structured content — is
    /// translated as one ordinary successful tool result.
    #[test]
    fn mcp_2026_complete_result_framing_carries_content_and_structured_content() {
        let (_directory, runtime) = runtime("conv_882529ea-9e3c-7445-b7fb-e82d5d7aefa5");
        let progress = NoProgress;
        let value = serde_json::json!([1, 2, 3]);
        let call = CallToolResult::structured(value.clone());
        assert_eq!(
            call.result_type,
            Some(rmcp::model::ResultType::COMPLETE),
            "the fixture result carries the 2026 complete discriminator"
        );
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(content_kinds(&result.content), vec!["text", "json"]);
        assert!(matches!(
            result.content.last(),
            Some(ToolResultContent::Json { value: projected }) if *projected == value
        ));
    }

    #[test]
    fn foreground_mcp_aggregate_budget_accepts_exact_text_image_boundary() {
        let (_directory, runtime) = runtime("conv_6b37ff56-9e45-76b6-ad53-720d83646781");
        let text_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        let image_bytes = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES - text_bytes;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text("t".repeat(text_bytes)),
                image_block(image_bytes),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert!(result.managed_output.is_none());
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Text(text)) if text.text.len() == text_bytes
        ));
        assert!(matches!(
            result.content.get(1),
            Some(ToolResultContent::Image(_))
        ));
    }

    #[test]
    fn foreground_mcp_multiple_images_share_the_aggregate_budget() {
        let (_directory, runtime) = runtime("conv_9805c367-479f-7441-9652-ecd2ca778da1");
        let image_bytes = 40 * 1024;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![image_block(image_bytes), image_block(image_bytes)]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("aggregate image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert_eq!(
            result
                .content
                .iter()
                .filter(|content| matches!(content, ToolResultContent::Image(_)))
                .count(),
            1
        );
        assert!(result.managed_output.is_none());
    }

    #[test]
    fn foreground_mcp_text_and_image_share_the_aggregate_budget() {
        let (_directory, runtime) = runtime("conv_d16d93ad-07fe-7e30-b67d-2d55c93ea27e");
        let text_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        let image_bytes = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES - text_bytes + 1;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text("t".repeat(text_bytes)),
                image_block(image_bytes),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("text plus image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Text(text)) if text.text.len() == text_bytes
        ));
        assert!(
            !result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Image(_)))
        );
        assert!(result.managed_output.is_none());
    }

    #[test]
    fn foreground_mcp_structured_json_and_image_share_the_aggregate_budget() {
        let (_directory, runtime) = runtime("conv_9a23ba04-30a5-7fde-ac71-eeb10ae35a7b");
        let value = serde_json::json!({"payload": "x".repeat(1024)});
        let structured_bytes = serde_json::to_vec(&value).expect("structured JSON").len();
        let image_bytes = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES - structured_bytes + 1;
        let mut call = CallToolResult::success(vec![image_block(image_bytes)]);
        call.structured_content = Some(value);
        let progress = NoProgress;
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("structured JSON plus image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert!(
            result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Image(_)))
        );
        assert!(
            !result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Json { .. }))
        );
        assert!(result.managed_output.is_none());
    }

    #[test]
    fn foreground_mcp_image_before_overflow_text_preserves_order_and_spill() {
        let (_directory, runtime) = runtime("conv_9945bc63-c737-7640-a9af-ea113d6a24d1");
        let text = "B".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![image_block(1024), ContentBlock::text(text.clone())]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(content_kinds(&result.content), vec!["image", "text"]);
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Text(text) if text.text.contains('B')
        ));
        let projection = result.model_facing_projection();
        assert!(projection.as_text().contains("Read or Grep"));
        let Some(ManagedOutputContinuation::Complete { locator }) =
            result.managed_output.as_deref()
        else {
            panic!("overflow MCP text must remain Complete: {result:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete text spill"),
            text.as_bytes()
        );
        assert_eq!(
            std::fs::read_dir(locator.parent().expect("results directory"))
                .expect("results directory")
                .count(),
            1
        );
    }

    #[test]
    fn foreground_mcp_text_image_text_overflow_preserves_order() {
        let (_directory, runtime) = runtime("conv_d6ae35e5-4186-71fd-a6e0-e4a88245fd6a");
        let first_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2;
        let second_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - first_bytes;
        let first = "A".repeat(first_bytes);
        let second = "B".repeat(second_bytes);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text(first.clone()),
                image_block(1024),
                ContentBlock::text(second.clone()),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(text) if text.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(text) if text.text == second
        ));
        let Some(ManagedOutputContinuation::Complete { locator }) =
            result.managed_output.as_deref()
        else {
            panic!("mixed overflow MCP text must remain Complete: {result:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete mixed text spill"),
            format!("{first}\n{second}").as_bytes()
        );
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("Read or Grep")
        );
    }

    #[test]
    fn foreground_mcp_text_image_overflow_text_image_preserves_order() {
        let (_directory, runtime) = runtime("conv_01c2f363-4cf5-771f-8ac3-361cd4d4e1c2");
        let first = "A".repeat(64);
        let second = "B".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text(first.clone()),
                image_block(1024),
                ContentBlock::text(second.clone()),
                image_block(1024),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text", "image"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(text) if text.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(text) if text.text.contains('B')
        ));
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Image(image) if image.artifact_id.as_str() == "artifact_1"
        ));
        assert!(matches!(
            &result.content[3],
            ToolResultContent::Image(image) if image.artifact_id.as_str() == "artifact_2"
        ));
        let Some(ManagedOutputContinuation::Complete { locator }) =
            result.managed_output.as_deref()
        else {
            panic!("mixed overflow MCP text must remain Complete: {result:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete mixed text spill"),
            format!("{first}\n{second}").as_bytes()
        );
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("Read or Grep")
        );
    }

    #[test]
    fn foreground_mcp_aggregate_pressure_keeps_order_before_rejecting_late_image() {
        let (_directory, runtime) = runtime("conv_ae52e05c-bb61-7f6d-9baa-326b0908c733");
        let first = "A".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2);
        let second = "B".repeat(
            crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
                - crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2,
        );
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text(first.clone()),
                image_block(30 * 1024),
                ContentBlock::text(second.clone()),
                image_block(30 * 1024),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("late aggregate image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(text) if text.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(text) if text.text == second
        ));
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Image(image) if image.artifact_id.as_str() == "artifact_1"
        ));
        assert!(result.managed_output.is_some());
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("Complete output:")
        );
    }

    #[test]
    fn foreground_mcp_storage_allocation_failure_is_unavailable() {
        let (_directory, runtime) = runtime("conv_7d8a4876-dd29-7af8-bcf0-bcde5f8df913");
        runtime.tool_output().set_force_open_failures(true);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            )]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
        assert!(matches!(
            result.managed_output.as_deref(),
            Some(ManagedOutputContinuation::Unavailable { .. })
        ));
        assert!(result.content.iter().any(|content| matches!(
            content,
            ToolResultContent::Text(text) if text.text.len() <= crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
        )));
    }

    #[test]
    fn foreground_mcp_storage_write_failure_is_partial() {
        let (_directory, runtime) = runtime("conv_5c577666-2ed8-7539-b8a0-46670c5c6b54");
        runtime.tool_output().fail_writes_after(0);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            )]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        let Some(ManagedOutputContinuation::Partial { locator, .. }) =
            result.managed_output.as_deref()
        else {
            panic!("MCP write failure must remain Partial: {result:?}");
        };
        assert!(locator.exists());
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
    }

    #[test]
    fn foreground_mcp_utf8_boundary_keeps_preview_valid_and_spill_complete() {
        let (_directory, runtime) = runtime("conv_92a48cd5-55c3-7bbe-871f-c113e2736868");
        let text = format!(
            "{}😀",
            "a".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - 1)
        );
        let expected = text.clone();
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(text)]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let Some(ManagedOutputContinuation::Complete { locator }) =
            result.managed_output.as_deref()
        else {
            panic!("MCP UTF-8 result must be complete in managed output: {result:?}");
        };
        assert_eq!(
            std::fs::read_to_string(locator).expect("UTF-8 MCP spill"),
            expected
        );
        assert!(result.content.iter().all(|content| match content {
            ToolResultContent::Text(text) => std::str::from_utf8(text.text.as_bytes()).is_ok(),
            _ => true,
        }));
        assert!(
            result
                .truncation
                .as_ref()
                .is_some_and(|truncation| truncation.truncated)
        );
    }

    #[test]
    fn background_mcp_empty_terminal_reuses_the_dispatch_locator() {
        let (_directory, runtime) = runtime("conv_b9fa698c-d676-7a72-8ddf-b9a5eb8d09cf");
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-00000000000b");
        let advertised = runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        let progress = NoProgress;
        let result = mcp_empty_terminal(
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::UserRequested,
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            },
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Cancelled { .. }
        ));
        assert_eq!(
            result.managed_output.map(|value| *value),
            Some(ManagedOutputContinuation::Complete {
                locator: advertised
            })
        );
        assert_eq!(
            std::fs::read(runtime.tool_output().background_output_path(&execution_id))
                .expect("empty background output"),
            b""
        );
        assert_eq!(
            std::fs::read_dir(runtime.tool_output().root().join("results"))
                .expect("results directory")
                .count(),
            0
        );
    }

    /// Issue #202: an output-sink-open failure never rewrites execution
    /// outcome certainty. A non-`Failed` terminal status keeps its own
    /// certainty claim; the storage failure is reported only through the
    /// managed-output continuation as explicitly Partial.
    #[test]
    fn background_mcp_sink_open_failure_preserves_outcome_certainty() {
        let (_directory, runtime) = runtime("conv_e1de8e98-031f-7225-a705-b3c8b7bbd52a");
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-00000000000c");
        let advertised = runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        runtime.tool_output().set_force_open_failures(true);
        let progress = NoProgress;
        let result = mcp_empty_terminal(
            ToolExecutionStatus::OutcomeUnknown {
                detail: "MCP transport closed during tools/call without a response".to_owned(),
            },
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        assert!(
            matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }),
            "a storage failure must not rewrite the outcome status: {:?}",
            result.status
        );
        let Some(ManagedOutputContinuation::Partial {
            locator,
            diagnostic,
        }) = result.managed_output.as_deref()
        else {
            panic!("a sink-open failure is honestly Partial: {result:?}");
        };
        assert_eq!(*locator, advertised);
        assert!(
            diagnostic.contains("cannot open the background MCP result output"),
            "the continuation names the storage failure: {diagnostic}"
        );
    }

    /// Issue #202: with the same sink-open failure, a known `Failed` status
    /// stays `Failed` and gains the storage diagnostic appended.
    #[test]
    fn background_mcp_sink_open_failure_appends_the_diagnostic_to_a_known_failure() {
        let (_directory, runtime) = runtime("conv_b8aea0c2-995c-7b75-ae2d-babec6181e66");
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-00000000000d");
        runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        runtime.tool_output().set_force_open_failures(true);
        let progress = NoProgress;
        let result = mcp_empty_terminal(
            ToolExecutionStatus::Failed {
                error: "peer said no".to_owned(),
            },
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("a known failure stays Failed: {result:?}");
        };
        assert!(
            error.contains("peer said no") && error.contains("MCP result output storage failed"),
            "the failure gains the storage diagnostic: {error}"
        );
        assert!(matches!(
            result.managed_output.as_deref(),
            Some(ManagedOutputContinuation::Partial { .. })
        ));
    }

    #[test]
    fn background_mcp_storage_write_failure_keeps_the_dispatch_locator_partial() {
        let (_directory, runtime) = runtime("conv_cf322b11-3d5e-788f-ab17-a026fb84e14f");
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-000000000004");
        let advertised = runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        runtime.tool_output().fail_writes_after(0);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            )]),
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        let Some(ManagedOutputContinuation::Partial { locator, .. }) =
            result.managed_output.as_deref()
        else {
            panic!("background MCP write failure must remain Partial: {result:?}");
        };
        assert_eq!(locator, &advertised);
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
        assert_eq!(
            std::fs::read_dir(runtime.tool_output().root().join("results"))
                .expect("results directory")
                .count(),
            0
        );
    }
}
