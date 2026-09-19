//! Typed projection of canonical message and Tool content.
//!
//! This is the positive allowlist #364 replaces blanket redaction with.
//! Every function here names the canonical fields it copies, and a canonical
//! type that grows a new field does not silently start crossing the
//! boundary: the match arm has to be written.
//!
//! What crosses: canonical User, Assistant and Tool content the model itself
//! saw — text, reasoning text, refusals, structured Tool arguments and
//! results, and durable artifact references.
//!
//! What never crosses: provider continuation state (opaque provider-internal
//! resumption data with no inspection value), managed-output locators (host
//! filesystem paths owned by the output store), and every value that is not
//! reached by an explicit arm below.

use super::bounds::{TRACE_DETAIL_BLOCKS, TraceJson, TracePreview, TraceText, identity_fits};
use super::types::{TraceArtifact, TraceContentBlock, TraceToolOutcome};
use crate::message::content::{FileReference, ImageReference};
use crate::message::types::{
    AssistantContentBlock, MessageBlock, ToolMessageBlock, UserContentBlock, UserSource,
};
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

/// The provenance namespace of one canonical User message.
///
/// Only the namespace crosses; an extension's internal identity and an
/// agent's routing details stay inside the runtime.
#[must_use]
pub(super) const fn user_source_label(source: &UserSource) -> &'static str {
    match source {
        UserSource::Human => "human",
        UserSource::Agent { .. } => "agent",
        UserSource::Fleet => "fleet",
        UserSource::ExternalSystem => "external_system",
        UserSource::Runtime => "runtime",
        UserSource::Extension { .. } => "certified_extension",
    }
}

/// Projects an artifact reference without its storage location.
fn image_artifact(image: &ImageReference) -> TraceArtifact {
    TraceArtifact {
        artifact_id: image.artifact_id.clone(),
        image: true,
        name: None,
        mime_type: None,
    }
}

fn file_artifact(file: &FileReference) -> TraceArtifact {
    TraceArtifact {
        artifact_id: file.artifact_id.clone(),
        image: false,
        // Display metadata recorded by the producing Tool. It is a label, and
        // it never decides whether the artifact is an image: canonical block
        // type does that, so a file named `.png` stays a file.
        name: file.name.clone(),
        mime_type: file.mime_type.clone(),
    }
}

/// Projects canonical User content blocks.
pub(super) fn user_blocks(content: &[UserContentBlock]) -> (Vec<TraceContentBlock>, bool) {
    let mut truncated = content.len() > TRACE_DETAIL_BLOCKS;
    let mut blocks = Vec::new();
    for block in content.iter().take(TRACE_DETAIL_BLOCKS) {
        let projected = match block {
            UserContentBlock::Text(text) => Some(TraceContentBlock::Text {
                text: TraceText::detail(&text.text),
            }),
            UserContentBlock::Image(image) if identity_fits(image.artifact_id.as_str()) => {
                Some(TraceContentBlock::Image {
                    artifact: image_artifact(image),
                    alt: image.alt.clone(),
                })
            }
            UserContentBlock::File(file) if identity_fits(file.artifact_id.as_str()) => {
                Some(TraceContentBlock::File {
                    artifact: file_artifact(file),
                })
            }
            UserContentBlock::UploadedFile(upload) => Some(TraceContentBlock::Upload {
                // The Session-owned upload name only; the batch's workspace
                // location is storage detail and stays inside the runtime.
                name: upload.name.clone(),
            }),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => {
                truncated = true;
                None
            }
        };
        if let Some(block) = projected {
            blocks.push(block);
        }
    }
    (blocks, truncated)
}

/// Projects canonical Assistant content blocks.
pub(super) fn assistant_blocks(
    content: &[AssistantContentBlock],
) -> (Vec<TraceContentBlock>, bool) {
    let mut truncated = content.len() > TRACE_DETAIL_BLOCKS;
    let mut blocks = Vec::new();
    for block in content.iter().take(TRACE_DETAIL_BLOCKS) {
        let projected = match block {
            AssistantContentBlock::Text(text) => Some(TraceContentBlock::Text {
                text: TraceText::detail(&text.text),
            }),
            AssistantContentBlock::Refusal(refusal) => Some(TraceContentBlock::Refusal {
                text: TraceText::detail(&refusal.text),
            }),
            // Reasoning text is already authorized conversation content. The
            // block's `continuation` sibling is provider-internal resumption
            // state and is deliberately not reached by any arm.
            AssistantContentBlock::Reasoning(reasoning) => {
                reasoning
                    .text
                    .as_ref()
                    .map(|text| TraceContentBlock::Reasoning {
                        text: TraceText::detail(text),
                    })
            }
            AssistantContentBlock::ToolCall(call)
                if identity_fits(call.id.as_str()) && identity_fits(call.tool_id.as_str()) =>
            {
                Some(TraceContentBlock::ToolCall {
                    call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                    arguments: TraceJson::bounded(&call.arguments),
                })
            }
            AssistantContentBlock::Image(image) if identity_fits(image.artifact_id.as_str()) => {
                Some(TraceContentBlock::Image {
                    artifact: image_artifact(image),
                    alt: image.alt.clone(),
                })
            }
            AssistantContentBlock::ToolCall(_) | AssistantContentBlock::Image(_) => {
                truncated = true;
                None
            }
        };
        if let Some(block) = projected {
            blocks.push(block);
        }
    }
    (blocks, truncated)
}

/// The typed outcome class of one canonical Tool status.
#[must_use]
pub(super) const fn tool_outcome(status: &ToolExecutionStatus) -> TraceToolOutcome {
    match status {
        ToolExecutionStatus::Success => TraceToolOutcome::Success,
        ToolExecutionStatus::Failed { .. } => TraceToolOutcome::Failed,
        ToolExecutionStatus::Denied { .. } => TraceToolOutcome::Denied,
        ToolExecutionStatus::Cancelled { .. } => TraceToolOutcome::Cancelled,
        ToolExecutionStatus::TimedOut => TraceToolOutcome::TimedOut,
        ToolExecutionStatus::OutcomeUnknown { .. } => TraceToolOutcome::OutcomeUnknown,
    }
}

/// The typed human-readable detail of one canonical Tool status.
///
/// Each arm names the exact typed field it reads. Nothing is produced by
/// `Debug`-formatting a status, so an internal field added to one of these
/// variants cannot start leaking through a formatter.
#[must_use]
pub(super) fn tool_status_detail(status: &ToolExecutionStatus) -> Option<String> {
    match status {
        ToolExecutionStatus::Success | ToolExecutionStatus::TimedOut => None,
        ToolExecutionStatus::Failed { error } => Some(error.clone()),
        ToolExecutionStatus::Denied { reason } => Some(reason.clone()),
        ToolExecutionStatus::Cancelled { reason, phase } => Some(format!(
            "{} ({})",
            cancellation_label(*reason),
            cancellation_phase_label(*phase)
        )),
        ToolExecutionStatus::OutcomeUnknown { detail } => Some(detail.clone()),
    }
}

const fn cancellation_label(reason: crate::runtime::types::CancellationReason) -> &'static str {
    use crate::runtime::types::CancellationReason as R;
    match reason {
        R::UserRequested => "cancelled by user request",
        R::RuntimeShutdown => "cancelled by runtime shutdown",
        R::ParentCancelled => "cancelled because its parent was cancelled",
        R::SubagentExecutionDeadlineExceeded => "cancelled by the Subagent execution deadline",
    }
}

const fn cancellation_phase_label(
    phase: crate::tools::types::ToolCancellationPhase,
) -> &'static str {
    use crate::tools::types::ToolCancellationPhase as P;
    match phase {
        P::BeforeStart => "before execution started",
        P::DuringExecution => "proven stopped after start",
    }
}

/// Projects canonical Tool result content blocks.
pub(super) fn tool_result_blocks(result: &ToolExecutionResult) -> (Vec<TraceContentBlock>, bool) {
    let mut truncated = result.content.len() > TRACE_DETAIL_BLOCKS;
    let mut blocks = Vec::new();
    for content in result.content.iter().take(TRACE_DETAIL_BLOCKS) {
        let projected = match content {
            ToolResultContent::Text(text) => Some(TraceContentBlock::Text {
                text: TraceText::detail(&text.text),
            }),
            // Tool-owned structured output stays structured, so the browser
            // can offer a JSON reader instead of an escaped string.
            ToolResultContent::Json { value } => Some(TraceContentBlock::Json {
                value: TraceJson::bounded(value),
            }),
            ToolResultContent::Image(image) if identity_fits(image.artifact_id.as_str()) => {
                Some(TraceContentBlock::Image {
                    artifact: image_artifact(image),
                    alt: image.alt.clone(),
                })
            }
            ToolResultContent::File(file) if identity_fits(file.artifact_id.as_str()) => {
                Some(TraceContentBlock::File {
                    artifact: file_artifact(file),
                })
            }
            ToolResultContent::Image(_) | ToolResultContent::File(_) => {
                truncated = true;
                None
            }
        };
        if let Some(block) = projected {
            blocks.push(block);
        }
    }
    (blocks, truncated)
}

/// Projects one canonical Tool message as a request-context item.
pub(super) fn tool_result_block(message: &ToolMessageBlock) -> TraceContentBlock {
    let (blocks, truncated) = tool_result_blocks(&message.result);
    TraceContentBlock::ToolResult {
        call_id: message.tool_call_id.clone(),
        tool_id: message.tool_id.clone(),
        outcome: tool_outcome(&message.result.status),
        blocks,
        truncated,
    }
}

/// Canonical artifact references of one Tool result, first occurrence wins.
///
/// Content references precede the result's separately owned artifact list, so
/// an identity that appears as both an Image content block and a generic
/// artifact keeps its image typing. Filenames and paths never decide type.
pub(super) fn tool_result_artifacts(result: &ToolExecutionResult) -> (Vec<TraceArtifact>, bool) {
    let mut seen = std::collections::HashSet::new();
    let mut artifacts: Vec<TraceArtifact> = Vec::new();
    let mut truncated =
        result.content.len() > TRACE_DETAIL_BLOCKS || result.artifacts.len() > TRACE_DETAIL_BLOCKS;
    let candidates = result
        .content
        .iter()
        .take(TRACE_DETAIL_BLOCKS)
        .filter_map(|content| match content {
            ToolResultContent::Image(image) => Some(image_artifact(image)),
            ToolResultContent::File(file) => Some(file_artifact(file)),
            ToolResultContent::Text(_) | ToolResultContent::Json { .. } => None,
        })
        .chain(
            result
                .artifacts
                .iter()
                .take(TRACE_DETAIL_BLOCKS)
                .map(file_artifact),
        );
    for artifact in candidates {
        if !identity_fits(artifact.artifact_id.as_str()) {
            truncated = true;
            continue;
        }
        if !seen.insert(artifact.artifact_id.clone()) {
            continue;
        }
        if artifacts.len() < TRACE_DETAIL_BLOCKS {
            artifacts.push(artifact);
        } else {
            truncated = true;
        }
    }
    (artifacts, truncated)
}

/// A one-line ledger preview of one canonical message.
///
/// Prose wins over structure: a message with text is previewed by its text,
/// because that is what a reader scanning the ledger is looking for.
pub(super) fn message_preview(message: &MessageBlock) -> Option<TracePreview> {
    let text = match message {
        MessageBlock::User(user) => user.content.iter().find_map(|block| match block {
            UserContentBlock::Text(text) => Some(text.text.clone()),
            UserContentBlock::UploadedFile(upload) => Some(upload.name.clone()),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => None,
        }),
        MessageBlock::Assistant(assistant) => {
            assistant
                .content
                .iter()
                .find_map(|block| match block {
                    AssistantContentBlock::Text(text) => Some(text.text.clone()),
                    AssistantContentBlock::Refusal(refusal) => Some(refusal.text.clone()),
                    _ => None,
                })
                // A tool-call-only generation has no prose. Fall back to its
                // reasoning so the row still says what the model was doing.
                .or_else(|| {
                    assistant.content.iter().find_map(|block| match block {
                        AssistantContentBlock::Reasoning(reasoning) => reasoning.text.clone(),
                        _ => None,
                    })
                })
        }
        MessageBlock::Tool(tool) => tool
            .result
            .content
            .iter()
            .find_map(|content| match content {
                ToolResultContent::Text(text) => Some(text.text.clone()),
                ToolResultContent::Json { value } => Some(value.to_string()),
                ToolResultContent::Image(_) | ToolResultContent::File(_) => None,
            })
            .or_else(|| tool_status_detail(&tool.result.status)),
    }?;
    let preview = TracePreview::of(&text);
    (!preview.is_empty()).then_some(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::{ArtifactId, ToolCallId, ToolId};

    #[test]
    fn each_identity_bound_omission_independently_marks_partial() {
        let oversized = "x".repeat(super::super::bounds::TRACE_IDENTITY_BYTES + 1);
        let image = ImageReference {
            artifact_id: ArtifactId::new(&oversized),
            alt: None,
        };
        let file = FileReference {
            artifact_id: ArtifactId::new(&oversized),
            name: None,
            mime_type: None,
            description: None,
        };
        for block in [
            UserContentBlock::Image(image.clone()),
            UserContentBlock::File(file.clone()),
        ] {
            assert_eq!(user_blocks(&[block]), (vec![], true));
        }
        for (call_id, tool_id) in [(&oversized[..], "tool"), ("call", &oversized[..])] {
            let call = crate::tools::types::ToolCall {
                id: ToolCallId::new(call_id),
                tool_id: ToolId::new(tool_id),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            };
            assert_eq!(
                assistant_blocks(&[AssistantContentBlock::ToolCall(call)]),
                (vec![], true)
            );
        }
        assert_eq!(
            assistant_blocks(&[AssistantContentBlock::Image(image.clone())]),
            (vec![], true)
        );
        for block in [
            ToolResultContent::Image(image),
            ToolResultContent::File(file),
        ] {
            let result = ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![block],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            };
            assert_eq!(tool_result_blocks(&result), (vec![], true));
        }
    }
}
