//! In-flight Assistant blocks and foreground Tool slots, with their inline bounds.
use super::{
    ContentBlockIndex, ForegroundToolExecution, ForegroundToolState, InFlightAssistantMessage,
    InFlightBlock, RuntimeClientAttemptFailure, RuntimeClientOutcome, RuntimeClientSnapshot,
    ToolCallId, ToolExecutionResult,
};

pub(super) fn defer_result_content(result: &mut ToolExecutionResult) {
    result.content.clear();
    result.artifacts.clear();
    result.deliveries.clear();
    result.workflow = None;
}
pub(super) fn inline_label(value: &str) -> String {
    let mut end = value.len().min(256);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
pub(super) fn inline_prefix(value: &str) -> String {
    let mut end = value
        .len()
        .min(crate::observation::bounded::CHECKPOINT_ITEM_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
pub(super) fn append_inline_prefix(prefix: &mut String, suffix: &str) {
    if suffix.len()
        <= crate::observation::bounded::CHECKPOINT_ITEM_BYTES.saturating_sub(prefix.len())
    {
        prefix.push_str(suffix);
    }
}
pub(super) fn bounded_diagnostic(value: &str) -> String {
    let mut end = value.len().min(1024);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
pub(super) fn bounded_outcome(outcome: &RuntimeClientOutcome) -> RuntimeClientOutcome {
    use crate::runtime::types::RuntimeError as Error;
    match outcome {
        RuntimeClientOutcome::Failed {
            error:
                RuntimeClientAttemptFailure::Model {
                    kind,
                    message,
                    retry_after_ms,
                },
            error_deferred,
            error_source,
        } => RuntimeClientOutcome::Failed {
            error_deferred: *error_deferred,
            error_source: error_source.clone(),
            error: RuntimeClientAttemptFailure::Model {
                kind: kind.clone(),
                message: bounded_diagnostic(message),
                retry_after_ms: *retry_after_ms,
            },
        },
        RuntimeClientOutcome::Failed {
            error: RuntimeClientAttemptFailure::Runtime { error },
            error_deferred,
            error_source,
        } => RuntimeClientOutcome::Failed {
            error_deferred: *error_deferred,
            error_source: error_source.clone(),
            error: RuntimeClientAttemptFailure::Runtime {
                error: match error {
                    Error::Internal { message } => Error::Internal {
                        message: bounded_diagnostic(message),
                    },
                    Error::InvalidState { message } => Error::InvalidState {
                        message: bounded_diagnostic(message),
                    },
                    Error::Unsupported { message } => Error::Unsupported {
                        message: bounded_diagnostic(message),
                    },
                    Error::UnknownTool { name } => Error::UnknownTool {
                        name: bounded_diagnostic(name),
                    },
                    Error::DurableStore { message } => Error::DurableStore {
                        message: bounded_diagnostic(message),
                    },
                    Error::ContractViolation { message } => Error::ContractViolation {
                        message: bounded_diagnostic(message),
                    },
                    Error::ContextPreparationFailed { message } => {
                        Error::ContextPreparationFailed {
                            message: bounded_diagnostic(message),
                        }
                    }
                    Error::ContextCompactionFailed { message } => Error::ContextCompactionFailed {
                        message: bounded_diagnostic(message),
                    },
                    Error::PreStepRejected { reason } => Error::PreStepRejected {
                        reason: bounded_diagnostic(reason),
                    },
                    Error::PreStepPolicyFailed { message } => Error::PreStepPolicyFailed {
                        message: bounded_diagnostic(message),
                    },
                    Error::ToolResultObservationFailed { message } => {
                        Error::ToolResultObservationFailed {
                            message: bounded_diagnostic(message),
                        }
                    }
                    Error::RestartInterrupted { message } => Error::RestartInterrupted {
                        message: bounded_diagnostic(message),
                    },
                    Error::DeferredContextRejected { message } => Error::DeferredContextRejected {
                        message: bounded_diagnostic(message),
                    },
                },
            },
        },
        RuntimeClientOutcome::Completed {
            finish_reason: crate::model::ModelFinishReason::Other { reason },
            finish_reason_deferred,
            finish_reason_source,
        } => RuntimeClientOutcome::Completed {
            finish_reason: crate::model::ModelFinishReason::Other {
                reason: bounded_diagnostic(reason),
            },
            finish_reason_deferred: *finish_reason_deferred || reason.len() > 1024,
            finish_reason_source: finish_reason_source.clone(),
        },
        other => other.clone(),
    }
}
/// The block index of one in-flight block.
pub(super) fn block_index_of(block: &InFlightBlock) -> ContentBlockIndex {
    match block {
        InFlightBlock::Text { block_index, .. }
        | InFlightBlock::Reasoning { block_index, .. }
        | InFlightBlock::Refusal { block_index, .. }
        | InFlightBlock::ToolCall { block_index, .. } => *block_index,
    }
}
/// Appends one block in its native publication creation order.
pub(super) fn push_in_flight_block(
    in_flight: &mut Option<InFlightAssistantMessage>,
    block: InFlightBlock,
) {
    let Some(message) = in_flight.as_mut() else {
        return;
    };
    let index = block_index_of(&block);
    if !message
        .blocks
        .iter()
        .any(|existing| block_index_of(existing) == index)
    {
        if message.blocks.len() >= 32 {
            return;
        }
        message.blocks.push(block);
    }
}
/// The mutable foreground slot of one logical call.
pub(super) fn foreground_slot_for_event<'a>(
    slots: &'a mut [ForegroundToolExecution],
    call_id: &ToolCallId,
    occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
) -> Option<&'a mut ForegroundToolExecution> {
    if let Some(native) = occurrence {
        return slots.iter_mut().find(|slot| {
            slot.occurrence.block_index == native.block_index
                && slot.occurrence.locator
                    == crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                        position: native.ledger_position,
                    }
        });
    }
    foreground_slot_mut(slots, call_id)
}
pub(super) fn foreground_slot_mut<'a>(
    foreground: &'a mut [ForegroundToolExecution],
    call_id: &ToolCallId,
) -> Option<&'a mut ForegroundToolExecution> {
    foreground
        .iter_mut()
        .find(|slot| slot.call_id.as_ref() == Some(call_id))
}
pub(super) fn update_stored_tool_slots(
    snapshot: &mut RuntimeClientSnapshot,
    occurrence: &crate::observation::tool::RuntimeClientToolOccurrence,
    mut update: impl FnMut(&mut crate::observation::tool::ForegroundToolExecution),
) {
    for slot in snapshot
        .message_descriptors
        .iter_mut()
        .flat_map(|descriptor| &mut descriptor.tool_calls)
        .chain(
            snapshot
                .transcript
                .entries
                .iter_mut()
                .flat_map(|entry| &mut entry.tool_calls),
        )
    {
        if &slot.occurrence == occurrence {
            update(slot);
        }
    }
}
pub(super) fn deferred_live_block(block: &InFlightBlock) -> InFlightBlock {
    match block {
        InFlightBlock::Text { block_index, .. } => InFlightBlock::Text {
            block_index: *block_index,
            text: String::new(),
        },
        InFlightBlock::Reasoning { block_index, .. } => InFlightBlock::Reasoning {
            block_index: *block_index,
            text: String::new(),
        },
        InFlightBlock::Refusal { block_index, .. } => InFlightBlock::Refusal {
            block_index: *block_index,
            text: String::new(),
        },
        InFlightBlock::ToolCall {
            block_index,
            call_id,
            metadata_ref,
            tool_id,
            name,
            ..
        } => InFlightBlock::ToolCall {
            block_index: *block_index,
            call_id: call_id.clone(),
            metadata_ref: metadata_ref.clone(),
            tool_id: tool_id.clone(),
            name: name.clone(),
            arguments: String::new(),
        },
    }
}
/// The arguments string of one foreground state.
pub(super) fn arguments_of(state: &ForegroundToolState) -> String {
    match state {
        ForegroundToolState::Assembled { arguments }
        | ForegroundToolState::Running { arguments, .. }
        | ForegroundToolState::Settled { arguments, .. } => arguments.clone(),
    }
}
pub(super) fn inline_block_bytes(block: &InFlightBlock, field: &str) -> u64 {
    let value = match block {
        InFlightBlock::Text { text, .. }
        | InFlightBlock::Reasoning { text, .. }
        | InFlightBlock::Refusal { text, .. } => text,
        InFlightBlock::ToolCall { arguments, .. } if field == "arguments" => arguments,
        InFlightBlock::ToolCall { .. } => return 0,
    };
    value.len() as u64
}
/// A complete canonical view refines C independently of the installed J state.
pub(super) fn retain_tool_execution(
    source: &mut ForegroundToolExecution,
    current: &ForegroundToolExecution,
) {
    let committed = source.committed_result.take().map(|result| {
        (
            result,
            source.committed_result_deferred,
            source.result_metadata_ref.take(),
        )
    });
    source.clone_from(current);
    if let Some((result, deferred, metadata)) = committed {
        source.committed_result = Some(result);
        source.committed_result_deferred = deferred;
        source.result_metadata_ref = metadata;
    }
}
