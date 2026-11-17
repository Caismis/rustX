//! Atomic ordinary model output; mailbox and publication keep their own owners.
use super::terminal::CanonicalCommitError;
use super::{AgentExecution, MessageBlock, RuntimeEvent};
use crate::durable::{ConversationStoreError, ModelTurnBoundary, ModelTurnSettlement};

impl AgentExecution<'_> {
    #[allow(clippy::too_many_lines)] // One native commit and its ordered publication.
    pub(super) fn commit_model_output(
        &mut self,
        block: MessageBlock,
    ) -> Result<Option<bool>, CanonicalCommitError> {
        let prepared = self.conversation.prepare_owned_commit(block)?;
        let block = prepared.message();
        let publication = self
            .publication
            .as_ref()
            .expect("model completion owns its stream");
        let start = publication.start.clone();
        let completion = publication
            .provider_completion
            .clone()
            .expect("pending provider completion");
        let RuntimeEvent::ModelRequestCompleted { finish_reason, .. } = &completion.event else {
            unreachable!("pending model completion")
        };
        let finish_reason = finish_reason.clone();
        let has_tools = matches!(block, MessageBlock::Assistant(message) if message.content.iter().any(|part| matches!(part, crate::message::types::AssistantContentBlock::ToolCall(_))));
        let canonical = self.event_envelope(Self::canonical_event(block).expect("Assistant event"));
        let mailbox = self.tool_runtime.mailbox();
        // Match start-or-steer admission's lock order. No steer can target
        // this Attempt between selecting the boundary and committing it.
        let steering = (!has_tools).then(|| mailbox.steering.clone());
        let mut gate = steering
            .as_ref()
            .map(|gate| gate.lock().expect("steering admission"));
        let publication_lease = (!has_tools).then(|| mailbox.pending_publication());
        let committed = (|| {
            let boundary = if has_tools {
                None
            } else {
                let batch = mailbox.select_steering_batch().map_err(|error| {
                    CanonicalCommitError::Durable(ConversationStoreError::Storage(
                        error.to_string(),
                    ))
                })?;
                if let Some(batch) = &batch {
                    for item in batch.items() {
                        self.conversation
                            .prepare_commit(&crate::durable::canonical_block(item.message()))?;
                    }
                }
                Some(ModelTurnBoundary {
                    pending_watermark: batch
                        .as_ref()
                        .map(crate::runtime::inbound::InboundBatch::watermark),
                    turn_completed: self.event_envelope(RuntimeEvent::TurnCompleted),
                    attempt_completed: self.event_envelope(RuntimeEvent::AttemptCompleted {
                        attempt_id: self.request.attempt_id.clone(),
                        finish_reason,
                    }),
                })
            };
            let frames = self
                .publication
                .as_mut()
                .expect("model publication")
                .coalescer
                .take_terminal_frames();
            self.store
                .settle_model_turn(ModelTurnSettlement {
                    start: &start,
                    provider: completion,
                    frames: &frames,
                    assistant: prepared.message(),
                    canonical,
                    boundary,
                })
                .map_err(CanonicalCommitError::Durable)
        })();
        if let Some(gate) = &mut gate
            && gate.active.as_ref() == Some(&self.request.attempt_id)
            && committed.as_ref().map_or(true, |commit| {
                commit
                    .boundary
                    .as_ref()
                    .is_some_and(|boundary| boundary.attempt_completed.is_some())
            })
        {
            gate.open = false;
        }
        let committed = committed?;
        self.record_persisted_event(committed.provider);
        // The complete final output is published through its canonical receipt.
        // Live-prefix backing was consumed by the same commit; publishing those
        // transient frame receipts now would expose unreadable ContentRefs.
        self.install_observed_canonical(prepared, &committed.canonical_receipt);
        self.record_persisted_event(committed.canonical);
        self.publication = None;
        let Some(boundary) = committed.boundary else {
            return Ok(None);
        };
        self.record_persisted_event(boundary.turn_completed);
        if let Some(adopted) = boundary.adopted {
            let adopted = publication_lease
                .as_ref()
                .expect("no-tool mailbox lease")
                .publish_adoption(adopted);
            assert!(self.install_adopted_boundary(adopted));
            Ok(Some(true))
        } else {
            let terminal = boundary
                .attempt_completed
                .expect("empty boundary committed terminal");
            self.precommitted_terminal = Some(terminal.event.clone());
            debug_assert!(!self.terminal_emitted);
            self.terminal_emitted = true;
            // Publish the committed terminal before releasing the mailbox lease,
            // so a later acceptance cannot overtake this native boundary.
            self.record_persisted_event(terminal);
            Ok(Some(false))
        }
    }
}
