//! Result materialization: one remote `CallToolResult` or an outcome without
//! one becomes the canonical [`ToolExecutionResult`], under the shared Tool
//! Plane capture and MCP's own aggregate content accounting.
//!
//! The `rmcp` upstream `CallToolResult` is already fully materialized in
//! memory when it reaches this adapter; bounding the HTTP receive itself is
//! tracked separately under issue #578. Foreground results are preview-only:
//! no eligible Execution Client save binding exists, so an overflow truncates
//! the preview and settles a typed `Unavailable` continuation — it never
//! allocates a local spill, never launches or queries a Client, and never
//! retains the complete output. Background results keep their
//! dispatch-owned sink unchanged.

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
    let captured = capture_mcp_content(result.content, result.structured_content, &mut capture);
    let capture_result = capture.finish(captured.storage_error.is_none());
    let output_diagnostic = captured
        .storage_error
        .as_deref()
        .or(captured.unsupported.as_deref());
    let continuation = continuation_for_capture(&capture_result, background, output_diagnostic);
    // A foreground preview that crossed its bound has no eligible Execution
    // Client save binding: the honest continuation is typed `Unavailable`
    // with no locator and no Read/Grep promise. A storage failure still
    // wins its own diagnostic above; a small complete result carries no
    // continuation at all.
    let continuation = if !background && capture_result.truncated && continuation.is_none() {
        Some(ManagedOutputContinuation::Unavailable {
            diagnostic: "Full output was not saved: no eligible Execution Client save binding \
                         at the preview threshold."
                .to_owned(),
        })
    } else {
        continuation
    };
    let (model_blocks, aggregate_error) =
        materialize_mcp_blocks(captured.pending_blocks, &capture_result, context.artifacts);
    let unsupported = captured
        .unsupported
        .or(captured.aggregate_error)
        .or(aggregate_error);
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

const MCP_AGGREGATE_BUDGET_ERROR: &str =
    "MCP result content exceeds the aggregate model-facing result limit";
const MCP_IMAGE_AGGREGATE_BUDGET_ERROR: &str =
    "MCP image content exceeds the aggregate model-facing result limit";
const MCP_BLOCK_AGGREGATE_BUDGET_ERROR: &str =
    "MCP result content exceeds the aggregate block limit";

/// The explicit ceiling on retained semantic result blocks: a remote result
/// enumerating more items is a domain aggregate excess, not a reason to
/// allocate an unbounded Vec.
const MCP_MAX_PENDING_BLOCKS: usize = 1024;

/// One semantic result block retained for the mixed-order model projection.
///
/// `Text` carries an already-bounded display candidate: the complete block
/// text when it fit the shared preview window at capture time, else its
/// deterministic head/marker/tail preview. Structured JSON is retained
/// verbatim only while the normalized input stays inside the preview bound —
/// an overflowing result keeps only the bounded serialized preview as
/// `Text`. Nothing here retains a speculative complete copy of the result.
enum McpPendingBlock {
    Text(String),
    Json(serde_json::Value),
    Image(Vec<u8>),
}

/// Finishes one bounded display candidate within the shared remaining
/// textual preview window and debits the window by what is retained.
/// `None` means nothing is displayable under the remaining window.
fn display_candidate(
    preview: TextPreviewCapture,
    remaining_textual_preview: &mut usize,
) -> Option<String> {
    // Once the shared window is exhausted an empty block contributes no
    // display payload; retaining it would only spend pending cardinality.
    if preview.total() == 0 && *remaining_textual_preview == 0 {
        return None;
    }
    if *remaining_textual_preview < 2 {
        if *remaining_textual_preview == 1
            && let Some(character) = preview.first_char()
            && character.len_utf8() == 1
        {
            *remaining_textual_preview = 0;
            return Some(character.to_string());
        }
        return None;
    }
    let (text, _) = preview.finish_within(*remaining_textual_preview);
    *remaining_textual_preview = (*remaining_textual_preview).saturating_sub(text.len());
    Some(text)
}

/// Debits `bytes` from the shared aggregate result budget; on failure the
/// first explicit budget error is retained and the candidate is dropped.
fn consume_aggregate(
    remaining: &mut usize,
    bytes: usize,
    error: &str,
    aggregate_error: &mut Option<String>,
) -> bool {
    if bytes > *remaining {
        if aggregate_error.is_none() {
            *aggregate_error = Some(error.to_owned());
        }
        return false;
    }
    *remaining -= bytes;
    true
}

/// Retains one ordered semantic block under the explicit cardinality cap.
fn push_pending(captured: &mut McpContentCapture, block: McpPendingBlock) {
    if captured.pending_blocks.len() >= MCP_MAX_PENDING_BLOCKS {
        if captured.aggregate_error.is_none() {
            captured.aggregate_error = Some(MCP_BLOCK_AGGREGATE_BUDGET_ERROR.to_owned());
        }
        return;
    }
    captured.pending_blocks.push(block);
}

/// Materializes the retained semantic blocks in order. Every candidate was
/// already selected and debited online during capture; this stage only
/// applies the artifact write and the empty-display emit rule.
fn materialize_mcp_blocks(
    pending_blocks: Vec<McpPendingBlock>,
    capture_result: &CapturedOutput,
    artifacts: &ArtifactStore,
) -> (Vec<ToolResultContent>, Option<String>) {
    let textual_overflow = capture_result.truncated || !capture_result.complete;
    let mut model_blocks = Vec::with_capacity(pending_blocks.len());
    let mut aggregate_error = None;
    for pending in pending_blocks {
        match pending {
            McpPendingBlock::Text(text) => {
                // An originally empty block retains an empty candidate; the
                // overflow projection drops it like the bounded-preview path
                // always did, while a fitting result keeps the empty block.
                if text.is_empty() && textual_overflow {
                    continue;
                }
                model_blocks.push(ToolResultContent::Text(
                    crate::message::content::TextBlock { text },
                ));
            }
            McpPendingBlock::Json(value) => {
                model_blocks.push(ToolResultContent::Json { value });
            }
            McpPendingBlock::Image(bytes) => match write_artifact(artifacts, &bytes) {
                Ok(id) => model_blocks.push(ToolResultContent::Image(
                    crate::message::content::ImageReference {
                        artifact_id: id,
                        alt: None,
                    },
                )),
                Err(error) => {
                    if aggregate_error.is_none() {
                        aggregate_error = Some(error);
                    }
                }
            },
        }
    }
    (model_blocks, aggregate_error)
}

fn open_mcp_output_capture(
    context: &ToolExecutionContext<'_>,
) -> Result<ToolOutputCapture, (PathBuf, String)> {
    let Some(execution_id) = context.execution_id else {
        // Foreground has no eligible Execution Client save binding: the
        // capture is preview-only, no execution id is allocated, and no
        // store is consulted.
        return Ok(ToolOutputCapture::preview_only());
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
    aggregate_error: Option<String>,
}

fn capture_mcp_content(
    blocks: Vec<ContentBlock>,
    structured_content: Option<serde_json::Value>,
    capture: &mut ToolOutputCapture,
) -> McpContentCapture {
    let mut captured = McpContentCapture {
        pending_blocks: Vec::new(),
        unsupported: None,
        storage_error: None,
        aggregate_error: None,
    };
    // Ordered display candidates are selected online against ONE shared
    // textual window across every block, while the semantic image budget is
    // the same single aggregate model-facing budget — neither is spent
    // twice. Retained candidates are bounded Strings, so the pending list
    // holds at most the preview window plus the cardinality cap of small
    // entries, never per-block complete payloads.
    let mut remaining_textual_preview = FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
    let mut remaining_aggregate = MAX_MODEL_TOOL_RESULT_BYTES;
    let mut first_textual_block = true;
    {
        let mut push_textual = |text: &str| -> Result<(), String> {
            if !first_textual_block {
                capture.push("\n")?;
            }
            first_textual_block = false;
            capture.push(text)
        };
        for block in blocks {
            match block {
                ContentBlock::Text(text) => {
                    if let Err(error) = push_textual(&text.text) {
                        captured.storage_error = Some(error);
                    }
                    let mut preview = TextPreviewCapture::new(FOREGROUND_TOOL_RESULT_PREVIEW_BYTES);
                    preview.push(&text.text);
                    if let Some(display) =
                        display_candidate(preview, &mut remaining_textual_preview)
                        && consume_aggregate(
                            &mut remaining_aggregate,
                            display.len(),
                            MCP_AGGREGATE_BUDGET_ERROR,
                            &mut captured.aggregate_error,
                        )
                    {
                        push_pending(&mut captured, McpPendingBlock::Text(display));
                    }
                }
                ContentBlock::Image(image) => {
                    // Check the encoded length against the remaining semantic
                    // image budget before decoding: legal Base64 padding (at
                    // most two bytes) is the only slack, so a payload that
                    // cannot fit even with full padding is refused without
                    // materializing the decoded bytes.
                    let estimated = image.data.len() / 4 * 3;
                    if estimated.saturating_sub(2) > remaining_aggregate {
                        if captured.aggregate_error.is_none() {
                            captured.aggregate_error =
                                Some(MCP_IMAGE_AGGREGATE_BUDGET_ERROR.to_owned());
                        }
                        continue;
                    }
                    match base64::engine::general_purpose::STANDARD.decode(image.data.as_bytes()) {
                        Ok(bytes) => {
                            if consume_aggregate(
                                &mut remaining_aggregate,
                                bytes.len(),
                                MCP_IMAGE_AGGREGATE_BUDGET_ERROR,
                                &mut captured.aggregate_error,
                            ) {
                                push_pending(&mut captured, McpPendingBlock::Image(bytes));
                            }
                        }
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
        capture_mcp_structured_content(
            value,
            first_textual_block,
            capture,
            &mut remaining_textual_preview,
            &mut remaining_aggregate,
            &mut captured,
        );
    }
    captured
}

/// Normalizes structured content into the aggregate capture and selects its
/// semantic candidate under the shared textual window and aggregate budget.
fn capture_mcp_structured_content(
    value: serde_json::Value,
    first_textual_block: bool,
    capture: &mut ToolOutputCapture,
    remaining_textual_preview: &mut usize,
    remaining_aggregate: &mut usize,
    captured: &mut McpContentCapture,
) {
    if captured.storage_error.is_none()
        && !first_textual_block
        && let Err(error) = capture.push("\n")
    {
        captured.storage_error = Some(error);
    }
    let normalized_before_json = capture.total_bytes();
    // One streaming serialization feeds an independent saturating byte
    // count and a bounded serialized preview tap — both consume ALL UTF-8
    // even after the aggregate capture stops accepting writes, so a failed
    // sink never sees further bytes, the first storage diagnostic is
    // preserved, and the measured length stays complete rather than a
    // partial count.
    let mut serialized_bytes = 0usize;
    let mut capture_failed = captured.storage_error.is_some();
    let mut block_preview = TextPreviewCapture::new(FOREGROUND_TOOL_RESULT_PREVIEW_BYTES);
    let serialization_error = {
        let capture = &mut *capture;
        let block_preview = &mut block_preview;
        let serialized_bytes = &mut serialized_bytes;
        let capture_failed = &mut capture_failed;
        let storage_error = &mut captured.storage_error;
        let mut writer = ToolOutputWriter::over_sink(move |text| {
            *serialized_bytes = serialized_bytes.saturating_add(text.len());
            block_preview.push(text);
            if !*capture_failed && let Err(error) = capture.push(text) {
                *capture_failed = true;
                if storage_error.is_none() {
                    *storage_error = Some(error);
                }
            }
            Ok(())
        });
        serde_json::to_writer(&mut writer, &value)
            .map_err(|error| format!("invalid MCP structured content: {error}"))
            .and_then(|()| writer.finish())
            .err()
    };
    if let Some(error) = serialization_error {
        // A value the serializer itself rejects never becomes content —
        // neither raw Json nor a partial preview is retained.
        if captured.storage_error.is_none() {
            captured.storage_error = Some(error);
        }
    } else if captured.storage_error.is_none()
        && normalized_before_json.saturating_add(serialized_bytes as u64)
            <= FOREGROUND_TOOL_RESULT_PREVIEW_BYTES as u64
        && consume_aggregate(
            remaining_aggregate,
            serialized_bytes,
            MCP_AGGREGATE_BUDGET_ERROR,
            &mut captured.aggregate_error,
        )
    {
        // The complete normalized total — every preceding fragment and
        // separator plus the independently-measured serialization — fits
        // the preview bound and the semantic budget: the verbatim remote
        // value is the semantic block.
        push_pending(captured, McpPendingBlock::Json(value));
    } else if let Some(display) = display_candidate(block_preview, remaining_textual_preview)
        && consume_aggregate(
            remaining_aggregate,
            display.len(),
            MCP_AGGREGATE_BUDGET_ERROR,
            &mut captured.aggregate_error,
        )
    {
        // An incomplete capture or an oversized serialization retains
        // only the bounded serialized preview; the upstream value is
        // released rather than parked as a speculative save.
        push_pending(captured, McpPendingBlock::Text(display));
    }
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
    use crate::tools::types::{
        ManagedOutputContinuation, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
    };

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

    /// Asserts the typed preview-only overflow contract: `Unavailable` with
    /// the no-eligible-binding diagnostic, no locator, no Read/Grep promise,
    /// and zero spill or dispatch-owned files on disk.
    fn assert_unavailable_no_save(
        result: &ToolExecutionResult,
        tool_output: &crate::tools::managed_output::ManagedToolOutput,
    ) {
        let Some(ManagedOutputContinuation::Unavailable { diagnostic }) =
            result.managed_output.as_deref()
        else {
            panic!("foreground overflow must settle Unavailable: {result:?}");
        };
        assert_eq!(
            diagnostic,
            "Full output was not saved: no eligible Execution Client save binding at the \
             preview threshold."
        );
        let projection = result.model_facing_projection();
        assert!(
            !projection.as_text().contains("Read or Grep"),
            "an unsaved result never promises file access: {}",
            projection.as_text()
        );
        let files: Vec<String> = std::fs::read_dir(tool_output.root().expect("output namespace"))
            .expect("output namespace")
            .filter_map(|entry| {
                entry
                    .ok()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
            })
            .collect();
        assert!(
            files.iter().all(|name| !name.starts_with("r-")),
            "no result spill may be created: {files:?}"
        );
        assert!(
            files.iter().all(|name| !name.starts_with("e-")),
            "no background execution output may be allocated: {files:?}"
        );
    }

    #[test]
    fn foreground_mcp_image_before_overflow_text_preserves_order_without_save() {
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
        assert!(
            result
                .truncation
                .as_ref()
                .is_some_and(|truncation| truncation.original_bytes == Some(text.len() as u64))
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
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
        // The aggregate joined text crossed the bound once across two
        // fragments: original_bytes counts both blocks plus the separator.
        assert!(
            result
                .truncation
                .as_ref()
                .is_some_and(|truncation| truncation.original_bytes
                    == Some((first.len() + 1 + second.len()) as u64))
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
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
        assert_unavailable_no_save(&result, runtime.tool_output());
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
        assert!(matches!(
            result.managed_output.as_deref(),
            Some(ManagedOutputContinuation::Unavailable { .. })
        ));
        assert!(
            !result
                .model_facing_projection()
                .as_text()
                .contains("Complete output:")
        );
    }

    /// A storage open forced to fail must never even be attempted for a
    /// foreground preview-only overflow: the remote verdict stays Success,
    /// the continuation is `Unavailable`, and nothing is written.
    #[test]
    fn foreground_mcp_overflow_never_attempts_storage_and_is_unavailable() {
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
        assert_eq!(
            result.status,
            ToolExecutionStatus::Success,
            "the absent save binding must not rewrite a known success"
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
        assert!(result.content.iter().any(|content| matches!(
            content,
            ToolResultContent::Text(text) if text.text.len() <= crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
        )));
    }

    /// Failing every managed-output write proves no write is ever attempted:
    /// the overflow still settles `Unavailable` with the remote success kept.
    #[test]
    fn foreground_mcp_overflow_never_writes_storage() {
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
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    /// A remote `is_error` verdict is a known business failure: the absent
    /// save binding adds an `Unavailable` continuation but never rewrites
    /// the verdict.
    #[test]
    fn foreground_mcp_overflow_keeps_a_reported_remote_error() {
        let (_directory, runtime) = runtime("conv_3f9a2a40-6c51-7b32-9d5e-1a2b3c4d5e6f");
        let mut call = CallToolResult::success(vec![ContentBlock::text(
            "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
        )]);
        call.is_error = Some(true);
        let progress = NoProgress;
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());
        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("a reported remote error must stay Failed: {result:?}");
        };
        assert!(error.contains("MCP tool reported an execution error"));
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    #[test]
    fn foreground_mcp_utf8_boundary_keeps_preview_valid() {
        let (_directory, runtime) = runtime("conv_92a48cd5-55c3-7bbe-871f-c113e2736868");
        let text = format!(
            "{}😀",
            "a".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - 1)
        );
        let expected_len = text.len();
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(text)]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert!(result.content.iter().all(|content| match content {
            ToolResultContent::Text(text) => std::str::from_utf8(text.text.as_bytes()).is_ok(),
            _ => true,
        }));
        assert!(result.truncation.as_ref().is_some_and(|truncation| {
            truncation.truncated && truncation.original_bytes == Some(expected_len as u64)
        }));
        assert_unavailable_no_save(&result, runtime.tool_output());
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
            std::fs::read_dir(runtime.tool_output().root().unwrap())
                .expect("results directory")
                .filter(|entry| entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("r-")))
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
            std::fs::read_dir(runtime.tool_output().root().unwrap())
                .expect("results directory")
                .filter(|entry| entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("r-")))
                .count(),
            0
        );
    }

    /// Hundreds of large text blocks retain a shared bounded preview
    /// window, not one bounded capture per block: the summed display bytes
    /// cannot exceed the preview bound even though every block normalized
    /// through the aggregate capture.
    #[test]
    fn foreground_mcp_hundreds_of_large_text_blocks_stay_bounded() {
        let (_directory, runtime) = runtime("conv_4d8d0b8b-3d46-74b8-9e29-3c63f3d3f9a1");
        let blocks: Vec<ContentBlock> = (0..400)
            .map(|index| ContentBlock::text(format!("{index:04}-{}", "x".repeat(64 * 1024))))
            .collect();
        let normalized: u64 = blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text(text) => text.text.len() as u64,
                _ => 0,
            })
            .sum::<u64>()
            + 399;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(blocks),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        let retained: usize = result
            .content
            .iter()
            .filter_map(|content| match content {
                ToolResultContent::Text(text) => Some(text.text.len()),
                _ => None,
            })
            .sum();
        assert!(
            retained <= crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
            "retained display bytes are bounded by one shared preview window: {retained}"
        );
        assert!(
            result
                .truncation
                .as_ref()
                .is_some_and(|truncation| truncation.original_bytes == Some(normalized)),
            "every fragment and separator still counts in original_bytes"
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    /// The pending-block cardinality cap is an explicit domain rejection:
    /// a remote result enumerating more than 1024 semantic items under the
    /// textual bound fails rather than allocating per-item metadata.
    #[test]
    fn foreground_mcp_pending_block_cardinality_is_explicitly_bounded() {
        let (_directory, runtime) = runtime("conv_7c1a5d2f-9f8c-7d5a-8b4e-3a2f1c0d9e8b");
        let blocks: Vec<ContentBlock> = (0..2000).map(|_| ContentBlock::text("")).collect();
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(blocks),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("excess pending blocks must fail explicitly: {result:?}");
        };
        assert!(
            error.contains("aggregate block limit"),
            "the rejection names the block cap: {error}"
        );
    }

    /// Empty blocks after the shared preview window is exhausted are skipped
    /// rather than retained as pending items, so a legitimately overflowing
    /// result keeps its Success verdict instead of tripping the block cap.
    #[test]
    fn foreground_mcp_empty_blocks_after_exhaustion_never_spill_the_cap() {
        let (_directory, runtime) = runtime("conv_2e9f4a6d-1b7c-7e3f-9a8d-5c6b7a8f9e0d");
        let mut blocks = vec![ContentBlock::text(
            "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
        )];
        blocks.extend((0..3000).map(|_| ContentBlock::text("")));
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(blocks),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    /// An over-budget image is refused on its encoded length before
    /// decoding, while a later image that still fits the remaining
    /// aggregate budget is decoded and retained: the budget is debited per
    /// image, never doubled.
    #[test]
    fn foreground_mcp_exhausted_image_budget_still_accepts_a_later_fitting_image() {
        let (_directory, runtime) = runtime("conv_6f3e2c1a-8d5b-7c9a-b7e6-f5d4c3b2a190");
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                image_block(40 * 1024),
                image_block(40 * 1024),
                image_block(1024),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("the over-budget image must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert_eq!(content_kinds(&result.content), vec!["image", "image"]);
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Image(image)) if image.artifact_id.as_str() == "artifact_1"
        ));
        assert!(matches!(
            result.content.get(1),
            Some(ToolResultContent::Image(image)) if image.artifact_id.as_str() == "artifact_2"
        ));
    }

    #[test]
    fn foreground_mcp_invalid_base64_image_is_unsupported() {
        let (_directory, runtime) = runtime("conv_1a2b3c4d-5e6f-7a7b-8c9d-0e1f2a3b4c5d");
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::image("!!!".to_owned(), "image/png")]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("invalid Base64 image data must fail explicitly: {result:?}");
        };
        assert!(error.contains("invalid MCP image data"));
    }

    /// Legal Base64 padding is the only slack allowed between the encoded
    /// pre-check and the decoded debit: a payload that fits only with full
    /// padding is decoded and retained, not refused early.
    #[test]
    fn foreground_mcp_image_legal_padding_fits_the_aggregate_budget() {
        let (_directory, runtime) = runtime("conv_9f8e7d6c-5b4a-73f2-a1b0-c9d8e7f6a5b4");
        let image_bytes = 30 * 1024 + 1;
        let data = base64::engine::general_purpose::STANDARD.encode(vec![b'i'; image_bytes]);
        assert!(data.ends_with("=="), "the fixture uses legal padding");
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                image_block(33 * 1024),
                ContentBlock::image(data, "image/png"),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(content_kinds(&result.content), vec!["image", "image"]);
    }

    /// Structured content under the preview bound is retained verbatim as
    /// `Json`; once the normalized serialized form crosses the bound only
    /// its bounded preview survives and the upstream value is released.
    #[test]
    fn foreground_mcp_json_verbatim_under_preview_and_previewed_over() {
        for (padding, expected) in [
            (512usize, "json"),
            (
                crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
                "text",
            ),
        ] {
            let (_directory, runtime) = runtime("conv_3c4d5e6f-7a8b-7c9d-b0e1-f2a3b4c5d6e7");
            let mut call = CallToolResult::success(Vec::new());
            call.structured_content = Some(serde_json::json!({"payload": "x".repeat(padding)}));
            let progress = NoProgress;
            let result =
                translate_result(call, &context(&runtime, None, &progress), Instant::now());

            assert_eq!(result.status, ToolExecutionStatus::Success);
            if expected == "json" {
                assert_eq!(content_kinds(&result.content), vec!["json"]);
                assert!(result.managed_output.is_none());
            } else {
                assert_eq!(content_kinds(&result.content), vec!["text"]);
                assert_unavailable_no_save(&result, runtime.tool_output());
            }
        }
    }

    /// Structured content crossing the bound after earlier text blocks is
    /// rendered into the shared preview window at its own position: the
    /// JSON preview follows the retained text, ordered by the result.
    #[test]
    fn foreground_mcp_overflowing_json_keeps_position_after_text() {
        let (_directory, runtime) = runtime("conv_8b7c6d5e-4f3a-72b1-90c8-d7e6f5a4b3c2");
        let text = "T".repeat(1024);
        let mut call =
            CallToolResult::success(vec![ContentBlock::text(text.clone()), image_block(64)]);
        call.structured_content = Some(serde_json::json!({
            "payload": "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES)
        }));
        let progress = NoProgress;
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(content) if content.text == text
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(content)
                if content.text.contains("payload") && content.text.contains("truncated")
        ));
        assert!(
            !result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Json { .. })),
            "an overflowing result never retains the upstream value"
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    /// The raw-Json gate accounts for the COMPLETE normalized total, not
    /// only the serialization itself: an exact-boundary text prefix plus a
    /// tiny structured value already overflows through the separator, so the
    /// JSON becomes a bounded candidate — never a verbatim `Json`.
    #[test]
    fn foreground_mcp_json_gate_counts_preceding_text_and_separators() {
        let (_directory, runtime) = runtime("conv_2f1e0d9c-8b7a-7c6d-b5e4-f3a2b1c0d9e8");
        let preview = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        let mut call = CallToolResult::success(vec![ContentBlock::text("t".repeat(preview))]);
        call.structured_content = Some(serde_json::json!("later"));
        let serialized = serde_json::to_vec(&serde_json::json!("later"))
            .expect("JSON")
            .len() as u64;
        let progress = NoProgress;
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(content_kinds(&result.content), vec!["text"]);
        assert!(
            result.truncation.as_ref().is_some_and(
                |truncation| truncation.original_bytes == Some(preview as u64 + 1 + serialized)
            ),
            "original_bytes counts the prefix, separator, and JSON serialization"
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    /// Two short texts can leave textual window for a small serialized JSON
    /// while the separators still push the combined normalized total over
    /// the preview bound: the JSON is projected as its bounded `Text`
    /// candidate, not retained verbatim.
    #[test]
    fn foreground_mcp_json_projects_bounded_text_when_separators_push_over() {
        let (_directory, runtime) = runtime("conv_9d8c7b6a-5f4e-7d3c-a2b1-e0f9d8c7b6a5");
        let preview = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        let first = "a".repeat(preview - 8);
        let mut call = CallToolResult::success(vec![
            ContentBlock::text(first.clone()),
            ContentBlock::text("bb"),
        ]);
        call.structured_content = Some(serde_json::json!("ccc"));
        let progress = NoProgress;
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(content_kinds(&result.content), vec!["text", "text", "text"]);
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(content) if content.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(content) if content.text == "\"ccc\""
        ));
        assert!(
            result
                .truncation
                .as_ref()
                .is_some_and(|truncation| truncation.original_bytes == Some(preview as u64 + 1))
        );
        assert_unavailable_no_save(&result, runtime.tool_output());
    }

    /// A background sink failing mid-serialization keeps its first storage
    /// diagnostic and never retains the unrestricted upstream value: the
    /// failed capture selects the bounded serialized preview instead of an
    /// oversized `Json` measured by a partial count.
    #[test]
    fn background_mcp_sink_failure_mid_json_keeps_only_the_bounded_preview() {
        let (_directory, runtime) = runtime("conv_5e4d3c2b-1a0f-7e9d-b8c7-a6b5c4d3e2f1");
        let execution_id = ToolExecutionId::new("exec_01900000-0000-7000-8000-00000000000e");
        runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        // Enough bytes for the text head, separator, and the first JSON
        // fragments; every later write fails inside the serialization.
        runtime.tool_output().fail_writes_after(32);
        let mut call = CallToolResult::success(vec![ContentBlock::text("head")]);
        call.structured_content = Some(serde_json::json!(
            "x".repeat(crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES + 1024)
        ));
        let progress = NoProgress;
        let result = translate_result(
            call,
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("a mid-serialization sink failure must fail explicitly: {result:?}");
        };
        assert!(
            error.contains("MCP result output storage failed"),
            "the first storage diagnostic is preserved: {error}"
        );
        assert!(
            !result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Json { .. })),
            "an oversized upstream value is never retained: {:?}",
            result.content
        );
        for content in &result.content {
            if let ToolResultContent::Text(text) = content {
                assert!(
                    text.text.len() <= crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
                    "every retained display candidate is bounded: {} bytes",
                    text.text.len()
                );
            }
        }
        assert!(matches!(
            result.managed_output.as_deref(),
            Some(ManagedOutputContinuation::Partial { .. })
        ));
    }
}
