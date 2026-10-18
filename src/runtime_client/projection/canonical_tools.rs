//! Complete native proposals can arrive without an intermediate live frame.
use super::{ForegroundToolState, RuntimeClientEvent, RuntimeClientProjection};
use crate::durable::observation::MessageHeader;
use crate::runtime::identity::AttemptId;
use crate::runtime_client::content::{
    CanonicalContentLocator, ContentRepresentation, ContentSource,
};
use crate::runtime_client::snapshot::RuntimeClientToolCall;

impl RuntimeClientProjection {
    pub(super) fn install_canonical_tool_proposals(
        &mut self,
        attempt_id: Option<&AttemptId>,
        header: &MessageHeader,
        events: &mut Vec<RuntimeClientEvent>,
    ) {
        let Some(attempt_id) = attempt_id else { return };
        if header.role != "assistant" {
            return;
        }
        let Some(attempt) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
        else {
            return;
        };
        for (ordinal, source) in header.tool_calls.iter().take(32).enumerate() {
            let existing = attempt.foreground.iter().position(|slot| {
                slot.occurrence == source.occurrence
                    || (slot.block_index == source.block_index
                        && header
                            .identity
                            .as_ref()
                            .is_some_and(|identity| slot.message_id.as_ref() == Some(identity)))
            });
            let started = existing.is_some();
            if existing.is_some_and(|index| attempt.foreground[index].assembly_complete) {
                continue;
            }
            let arguments = match &source.state {
                ForegroundToolState::Assembled { arguments }
                | ForegroundToolState::Running { arguments, .. }
                | ForegroundToolState::Settled { arguments, .. } => arguments.clone(),
            };
            if let Some(index) = existing {
                let slot = &mut attempt.foreground[index];
                slot.assembly_complete = true;
                if let ForegroundToolState::Assembled { arguments: current } = &mut slot.state {
                    current.clone_from(&arguments);
                }
            } else {
                if attempt.foreground.len() >= 32 {
                    break;
                }
                let mut slot = source.clone();
                // The existing rebind fold publishes the exact native locator
                // after the proposal facts, just as it does for a live prefix.
                if let Some(message_id) = &header.identity {
                    slot.message_id = Some(message_id.clone());
                    slot.occurrence.locator = CanonicalContentLocator::ByMessageId {
                        message_id: message_id.clone(),
                    };
                }
                attempt.foreground.push(slot);
            }
            if let Some(call_id) = &source.call_id {
                self.native_tool_occurrences
                    .insert(call_id.clone(), source.occurrence.clone());
            }
            // Deferred identities remain available through the bounded native
            // descriptor; no invented or truncated message identity is emitted.
            let Some(message_id) = &header.identity else {
                continue;
            };
            let mut call = RuntimeClientToolCall {
                id: source.call_id.clone(),
                tool_id: source.tool_id.clone(),
                name: source.name.clone(),
                arguments: None,
                metadata_ref: source.metadata_ref.clone(),
            };
            let position = u64::from(source.block_index.get());
            let foreground_position = ordinal as u64;
            if !started {
                events.push(RuntimeClientEvent::ToolCallStarted {
                    position,
                    foreground_position,
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: source.block_index,
                    call: call.clone(),
                });
            }
            call.arguments = (arguments.len() <= 1024)
                .then(|| serde_json::from_str(&arguments).ok())
                .flatten();
            let arguments_ref = header.content.iter().find_map(|binding| {
                (binding.content_ref.representation == ContentRepresentation::RawJsonUtf8
                    && matches!(&binding.content_ref.source, ContentSource::CanonicalMessageBlock { block_index, .. } if *block_index == source.block_index))
                    .then(|| binding.content_ref.clone())
            });
            events.push(RuntimeClientEvent::ToolCallAssembled {
                position,
                foreground_position,
                arguments_json: arguments,
                arguments_ref,
                attempt_id: attempt_id.clone(),
                message_id: message_id.clone(),
                block_index: source.block_index,
                call,
            });
        }
    }
}
