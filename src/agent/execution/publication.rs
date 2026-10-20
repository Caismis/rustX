//! Publication and canonical acceptance of model output: the open release
//! stream of the in-flight request, the compound canonical commits that
//! settle it, its audit terminalization, and Event Journal emission.

use super::terminal::{CanonicalCommitError, TerminalCommitState};
use super::{AgentExecution, DurableFailureKind};
use crate::durable::TranscriptCursor;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{AssistantMessageBlock, MessageBlock};
use crate::model::event::ModelEvent;
use crate::publication::{
    PublicationCoalescer, PublicationFrame, PublicationPayload, PublicationStreamStart,
};
use crate::runtime::identity::{EventId, MessageId, PublicationStreamId, TurnId};
use crate::runtime::types::RuntimeError;
use chrono::Utc;

/// The open publication stream of one in-flight model request.
///
/// The stream owns the release plane of exactly one provider request: the
/// bounded coalescer that decides when a frame exists, and the durable facts
/// of whether U has committed. It is deliberately not the assembler: the
/// assembler builds the canonical message, this builds what rustX committed
/// for release.
pub(super) struct OpenPublication {
    /// The frozen identity, pinned to the exact request generation.
    pub(super) start: PublicationStreamStart,
    /// The bounded coalescer of this stream.
    pub(super) coalescer: PublicationCoalescer,
    /// Whether the publication terminal transaction (U) committed.
    terminal_committed: bool,
}

impl<'a> AgentExecution<'a> {
    /// Commits **C**: the assembled Assistant message joins canonical history
    /// as one compound durable transition with its publication stream.
    ///
    /// The transition validates that the exact stream is publication-complete,
    /// appends the canonical Ledger fact, advances the Surface, records
    /// `AssistantMessageCommitted`, and clears the stream's publication
    /// staging — all in one transaction. `ModelRequestCompleted` is
    /// deliberately not part of it: provider completion remains an external
    /// execution fact even when canonicalization later fails.
    pub(super) fn commit_assistant_message(
        &mut self,
        message_id: &MessageId,
        content: &[crate::message::types::AssistantContentBlock],
    ) -> Result<(), CanonicalCommitError> {
        let block = MessageBlock::Assistant(AssistantMessageBlock {
            id: message_id.clone(),
            content: content.to_vec(),
        });
        let Some(publication) = self.publication.as_ref() else {
            return Err(CanonicalCommitError::Durable(
                crate::durable::ConversationStoreError::PublicationViolation(format!(
                    "Assistant message {message_id} has no open publication stream"
                )),
            ));
        };
        let stream_id = publication.start.stream_id.clone();
        let prepared = self.conversation.prepare_commit(&block)?;
        let event = Self::canonical_event(&block).expect("an Assistant commit has its event");
        let (persisted, receipt) = self
            .store
            .commit_canonical_publication(
                &stream_id,
                prepared.message(),
                self.event_envelope(event),
            )
            .map_err(CanonicalCommitError::Durable)?;
        // The stream settled canonically: the Ledger is the long-term
        // authority and no audit may ever be created for it.
        self.publication = None;
        self.conversation.install_prepared(prepared);
        if let Some(observer) = self.observer {
            observer.observe_committed(&self.request.attempt_id, &block, &receipt);
        }
        self.record_persisted_event(persisted);
        Ok(())
    }

    /// Commits one complete `ToolResult` batch atomically.
    ///
    /// The whole batch prepares (validates) first, then appends to the
    /// durable Message Ledger in **one** transaction, and only then installs
    /// each member in memory. A durable failure of any member appends and
    /// installs none of them, so a partial tool-result group can never become
    /// canonical — the prior review's tool-batch atomicity requirement.
    ///
    /// Returns the durable transcript position the batch settled at: the
    /// highest cursor the atomic append allocated, or `None` when the batch
    /// committed no visible transcript item. That position is the batch's
    /// own, read from its receipts under the commit that allocated them, and
    /// it is the placement fact of any `PostToolBatch` status opportunity
    /// this batch establishes — produced here rather than sampled later from
    /// a moving frontier.
    pub(super) fn commit_tool_result_batch(
        &mut self,
        blocks: &[MessageBlock],
    ) -> Result<Option<TranscriptCursor>, CanonicalCommitError> {
        let mut prepared = Vec::with_capacity(blocks.len());
        for block in blocks {
            prepared.push(self.conversation.prepare_commit(block)?);
        }
        let events = blocks
            .iter()
            .map(|block| {
                Self::canonical_event(block).ok_or_else(|| {
                    CanonicalCommitError::Durable(
                        crate::durable::inbox::ConversationStoreError::InvalidReference(
                            "a ToolResult batch contains a non-tool message".to_owned(),
                        ),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let envelopes = events
            .into_iter()
            .map(|event| self.event_envelope(event))
            .collect::<Vec<_>>();
        let (persisted_events, receipts) = self
            .store
            .append_canonical_batch_with_events(blocks, &envelopes)
            .map_err(CanonicalCommitError::Durable)?;
        let mut batch_anchor: Option<TranscriptCursor> = None;
        for ((prepared, block), receipt) in prepared.into_iter().zip(blocks).zip(receipts) {
            self.conversation.install_prepared(prepared);
            if let Some(cursor) = receipt.transcript_cursor
                && batch_anchor.is_none_or(|anchor| cursor > anchor)
            {
                batch_anchor = Some(cursor);
            }
            if let Some(observer) = self.observer {
                observer.observe_committed(&self.request.attempt_id, block, &receipt);
            }
        }
        for event in persisted_events {
            self.record_persisted_event(event);
        }
        Ok(batch_anchor)
    }

    /// Opens the publication stream of one model request.
    ///
    /// The stream is pinned to the exact request generation that started it:
    /// attempt, turn, request, and provisional message identity are frozen
    /// here, so a resource, Skill, or Tool configuration edit that lands
    /// during streaming can never re-associate this stream with a newer
    /// generation, and recovery classifies it from these identities alone.
    pub(super) fn open_publication(&mut self, message_id: &MessageId) {
        let Some(request_id) = self.last_request_id.clone() else {
            self.record_publication_failure(
                "a publication stream cannot open without a started request",
            );
            return;
        };
        let start = PublicationStreamStart {
            stream_id: PublicationStreamId::for_request(&self.request.attempt_id, message_id),
            attempt_id: self.request.attempt_id.clone(),
            turn_id: TurnId::new(self.turn.to_string()),
            request_id,
            message_id: message_id.clone(),
        };
        if let Err(error) = self.store.open_publication_stream(&start) {
            self.record_publication_failure(&format!(
                "the publication stream could not be opened durably: {error}"
            ));
            return;
        }
        let coalescer = PublicationCoalescer::new(
            start.stream_id.clone(),
            message_id.clone(),
            self.publication_policy,
            std::sync::Arc::clone(&self.monotonic_clock),
        );
        if let Some(observer) = self.observer {
            observer.observe_publication_opened(&self.request.attempt_id, &start);
        }
        self.publication = Some(OpenPublication {
            start,
            coalescer,
            terminal_committed: false,
        });
    }

    /// The publication payload of one non-terminal model event, when the
    /// event carries user-facing semantic output.
    ///
    /// Usage and provider continuation state are request bookkeeping, not
    /// released output, so they produce no publication frame.
    fn publication_payload(event: &ModelEvent) -> Option<PublicationPayload> {
        match event {
            ModelEvent::TextDelta { block_index, text } => Some(PublicationPayload::TextSuffix {
                block_index: *block_index,
                suffix: text.clone(),
            }),
            ModelEvent::ReasoningDelta { block_index, text } => {
                Some(PublicationPayload::ReasoningSuffix {
                    block_index: *block_index,
                    suffix: text.clone(),
                })
            }
            ModelEvent::RefusalDelta { block_index, text } => {
                Some(PublicationPayload::RefusalSuffix {
                    block_index: *block_index,
                    suffix: text.clone(),
                })
            }
            ModelEvent::ToolCallStarted { block_index, call } => {
                Some(PublicationPayload::ProposedToolCallStarted {
                    block_index: *block_index,
                    call: call.clone(),
                })
            }
            ModelEvent::ToolCallArgumentsDelta {
                block_index,
                call_id,
                arguments_delta,
            } => Some(PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: *block_index,
                call_id: call_id.clone(),
                suffix: arguments_delta.clone(),
            }),
            ModelEvent::ToolCallCompleted { block_index, call } => {
                Some(PublicationPayload::ProposedToolCallCompleted {
                    block_index: *block_index,
                    call: call.clone(),
                })
            }
            ModelEvent::Started
            | ModelEvent::UsageUpdate { .. }
            | ModelEvent::ContinuationState { .. }
            | ModelEvent::Completed { .. }
            | ModelEvent::Failed { .. } => None,
        }
    }

    /// Buffers one non-terminal model event into the publication coalescer
    /// and performs the bounded flush the policy requires.
    pub(super) fn publish_model_event(&mut self, event: &ModelEvent) {
        let Some(payload) = Self::publication_payload(event) else {
            return;
        };
        let Some(publication) = self.publication.as_mut() else {
            self.record_publication_failure(
                "a model delta arrived with no open publication stream",
            );
            return;
        };
        let must_flush = publication.coalescer.push(payload);
        if must_flush || publication.coalescer.latency_elapsed() {
            self.flush_publication();
        }
    }

    /// Stages the buffered publication payload durably and only then releases
    /// it.
    ///
    /// The order is the whole point: nothing reaches a user-facing Runtime
    /// Client before its staging transaction committed.
    pub(super) fn flush_publication(&mut self) {
        let Some(publication) = self.publication.as_mut() else {
            return;
        };
        let frames = publication.coalescer.take_frames();
        if frames.is_empty() {
            return;
        }
        let receipts = match self.store.stage_publication_frames(&frames) {
            Ok(receipts) => receipts,
            Err(error) => {
                self.record_publication_failure(&format!(
                    "publication frames could not be staged before release: {error}"
                ));
                return;
            }
        };
        self.release_publication(&frames, &receipts);
    }

    /// Commits **U** — the final publication frame and the publication
    /// terminal marker in one transaction — and only then releases the final
    /// buffered payload.
    ///
    /// P must already be durable; the durable store enforces that. When no
    /// visible payload remains, a terminal-only frame still commits and no
    /// visible text is delayed.
    pub(super) fn commit_publication_terminal(&mut self) -> Result<(), RuntimeError> {
        let Some(publication) = self.publication.as_mut() else {
            return Ok(());
        };
        if publication.terminal_committed {
            return Ok(());
        }
        let stream_id = publication.start.stream_id.clone();
        let frames = publication.coalescer.take_terminal_frames();
        let receipts = match self.store.commit_publication_terminal(&stream_id, &frames) {
            Ok(receipts) => receipts,
            Err(error) => {
                let message =
                    format!("the publication terminal could not be committed durably: {error}");
                self.record_publication_failure(&message);
                return Err(RuntimeError::DurableStore { message });
            }
        };
        if let Some(publication) = self.publication.as_mut() {
            publication.terminal_committed = true;
        }
        self.release_publication(&frames, &receipts);
        Ok(())
    }

    /// Releases already-committed frames to the live observation seam.
    fn release_publication(
        &self,
        frames: &[PublicationFrame],
        receipts: &[crate::publication::PublicationFrameReceipt],
    ) {
        let Some(observer) = self.observer else {
            return;
        };
        assert_eq!(
            frames.len(),
            receipts.len(),
            "one native receipt per staged Frame"
        );
        for (frame, receipt) in frames.iter().zip(receipts) {
            observer.observe_publication(&self.request.attempt_id, frame, receipt);
        }
    }

    /// Settles a still-open publication stream as an audit.
    ///
    /// The audit kind is derived by the durable store from P/U evidence
    /// alone, so this path can never mislabel an Incomplete publication as
    /// Unaccepted (or the reverse) no matter which control-flow exit reached
    /// it. Canonical acceptance of the stream becomes permanently forbidden.
    pub(super) fn settle_publication_audit(&mut self) -> Result<(), RuntimeError> {
        // A failed request may have ended before the coalescer's normal byte
        // or latency boundary. Flush that pending payload first so the audit
        // retains every partial text/reasoning/refusal/tool proposal that was
        // already emitted by the provider.
        if self.has_buffered_publication() {
            self.flush_publication();
            if let Some(message) = self.durable_failure.clone() {
                return Err(RuntimeError::DurableStore { message });
            }
        }
        let Some(stream_id) = self
            .publication
            .as_ref()
            .map(|publication| publication.start.stream_id.clone())
        else {
            return Ok(());
        };
        match self
            .store
            .terminalize_publication_audit(&stream_id, Utc::now())
        {
            Ok((audit, receipt)) => {
                // Only remove the in-memory owner after the durable
                // settlement committed. On failure it remains the one
                // recoverable unsettled stream.
                self.publication.take();
                if let Some(observer) = self.observer {
                    observer.observe_publication_settled(
                        &self.request.attempt_id,
                        &audit,
                        &receipt,
                    );
                }
                Ok(())
            }
            Err(error) => {
                let message =
                    format!("the publication audit could not be terminalized durably: {error}");
                self.record_publication_failure(&message);
                self.publication_settlement_failed = true;
                self.logical_step.terminal_commit_state = TerminalCommitState::Blocked;
                Err(RuntimeError::DurableStore { message })
            }
        }
    }

    /// Whether the open publication stream is holding buffered payload.
    fn has_buffered_publication(&self) -> bool {
        self.publication
            .as_ref()
            .is_some_and(|publication| !publication.coalescer.is_empty())
    }

    /// Records a publication-plane durability failure exactly once.
    ///
    /// A publication failure is a durable-authority failure like any other:
    /// the attempt must not report a healthy durability state afterwards.
    fn record_publication_failure(&mut self, message: &str) {
        if self.durable_failure.is_none() {
            self.durable_failure_kind = Some(DurableFailureKind::Publication);
            self.durable_failure = Some(message.to_owned());
        }
    }

    fn canonical_event(block: &MessageBlock) -> Option<RuntimeEvent> {
        match block {
            MessageBlock::Assistant(message) => Some(RuntimeEvent::AssistantMessageCommitted {
                message_id: message.id.clone(),
            }),
            MessageBlock::Tool(message) => Some(RuntimeEvent::ToolMessageCommitted {
                message_id: message.id.clone(),
                tool_call_id: message.tool_call_id.clone(),
            }),
            MessageBlock::User(_) => None,
        }
    }

    pub(super) fn event_envelope(&self, event: RuntimeEvent) -> RuntimeEventEnvelope {
        // Attempt lifecycle facts are attempt-level facts, not events in the
        // model turn that just completed. Keeping their turn identity empty
        // lets the durable Journal enforce turn terminality without treating
        // the enclosing attempt lifecycle as a contradictory turn fact.
        let turn_id = if matches!(
            &event,
            RuntimeEvent::AttemptStarted { .. }
                | RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptFailed { .. }
        ) {
            None
        } else {
            Some(TurnId::new(self.turn.to_string()))
        };
        RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: self.request.conversation_id.clone(),
            attempt_id: Some(self.request.attempt_id.clone()),
            turn_id,
            timestamp: Utc::now(),
            event,
        }
    }

    pub(super) fn record_persisted_event(&self, envelope: RuntimeEventEnvelope) {
        let event = envelope.event;
        if let Some(observer) = self.observer {
            observer.observe_event(
                &self.request.attempt_id,
                &event,
                envelope.sequence,
                envelope.tool_occurrence.as_ref(),
                envelope.observation.as_deref(),
            );
        }
    }

    pub(super) fn emit(&mut self, event: RuntimeEvent) {
        debug_assert!(
            !self.terminal_emitted,
            "no runtime events may follow the terminal event"
        );
        if self.durable_failure.is_some() {
            return;
        }
        match self.store.append_event(self.event_envelope(event)) {
            Ok(envelope) => self.record_persisted_event(envelope),
            Err(error) => {
                self.durable_failure_kind = Some(DurableFailureKind::EventJournal);
                self.durable_failure = Some(format!(
                    "runtime event could not be persisted before publication: {error}"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests;
