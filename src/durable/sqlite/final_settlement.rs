//! Native final-output transaction parts. The caller owns COMMIT and publication.
#[cfg(test)]
use super::AdmissionFaultOperation;
use super::{
    Arc, ConversationStoreError, InboundSequence, MessageBlock, PublicationFrame,
    PublicationSettlement, PublicationStreamId, RuntimeEventEnvelope, SqliteConversationStore,
    Transaction, TranscriptCommitReceipt, append_validation, canonical_observation_batch,
    canonical_observation_receipt, clear_publication_staging, content, ensure_surface_head,
    load_pending_until, observation_receipt, optional_display_layout, params, persist_event_tx,
    persist_publication_progress_tx, prepare_publication_frames_tx, reading,
    rebind_published_read_layouts, request_outcome_is_durable, require_publication_stream,
    require_started_request_tx, seq_to_i64, set_canonical_range, stage_frames_tx, storage,
    validate_canonical_publication_event, validate_canonical_tool_proposals,
    validate_publication_generation,
};
impl SqliteConversationStore {
    pub(super) fn publication_terminal_tx(
        &self,
        transaction: &Transaction<'_>,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError> {
        self.validate_publication_terminal_tx(transaction, stream_id, frames)?;
        let (stream, receipts) = stage_frames_tx(transaction, frames, &self.conversation_id)?;
        Self::finish_publication_terminal_tx(transaction, stream_id, frames, &stream)?;
        Ok(receipts)
    }

    /// The final canonical transaction consumes these frames before COMMIT, so
    /// neither a staging row nor a live `ContentRef` may escape it. Retain only
    /// proposal ownership and sequence/terminal progress here; raw Tool suffixes
    /// are copied directly to canonical authority by `canonical_publication_tx`.
    fn final_publication_terminal_tx(
        &self,
        transaction: &Transaction<'_>,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError> {
        self.validate_publication_terminal_tx(transaction, stream_id, frames)?;
        let prepared = prepare_publication_frames_tx(transaction, frames)?;
        persist_publication_progress_tx(transaction, &prepared)?;
        Self::finish_publication_terminal_tx(transaction, stream_id, frames, &prepared.stream)
    }

    fn validate_publication_terminal_tx(
        &self,
        transaction: &Transaction<'_>,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError> {
        if frames.is_empty() {
            return Err(ConversationStoreError::PublicationViolation(
                "the publication terminal transaction must carry a final frame".to_owned(),
            ));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_publication_terminal_remaining) {
            return Err(storage("fault injected: publication terminal commit"));
        }
        let stream = require_publication_stream(transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
            transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        // Final frames follow the exact provider outcome in this transaction;
        // the caller may commit both together with canonical acceptance.
        if !request_outcome_is_durable(
            transaction,
            &self.conversation_id,
            &snapshot,
            completed_sequence,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication terminal for {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        Ok(())
    }

    fn finish_publication_terminal_tx(
        transaction: &Transaction<'_>,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
        stream: &crate::publication::PublicationStreamRecord,
    ) -> Result<(), ConversationStoreError> {
        if stream.start.stream_id != *stream_id {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "terminal frames belong to publication stream {}, not {stream_id}",
                stream.start.stream_id
            )));
        }
        if stream.terminal_sequence.is_some() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already committed its terminal boundary"
            )));
        }
        let terminal_sequence = frames
            .last()
            .map(|frame| frame.sequence)
            .ok_or_else(|| storage("terminal frames are empty"))?;
        // Retained frames or canonical acceptance share this marker's transaction.
        transaction
            .execute(
                "UPDATE publication_streams SET terminal_sequence=?1 WHERE stream_id=?2",
                params![seq_to_i64(terminal_sequence)?, stream_id.as_str()],
            )
            .map_err(|error| storage(format!("commit publication terminal: {error}")))?;
        Ok(())
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One native canonical transition and its transaction-owned final suffixes.
    pub(super) fn canonical_publication_tx(
        &self,
        transaction: &Transaction<'_>,
        validation: &mut append_validation::Batch<'_>,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
        final_frames: &[PublicationFrame],
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError> {
        let stream = require_publication_stream(transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
            transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        validate_canonical_publication_event(
            &self.conversation_id,
            &stream.start,
            message,
            &event,
        )?;
        validate_canonical_tool_proposals(transaction, stream_id, message)?;
        if let Some(settlement) = stream.settlement {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already settled as {}; canonical acceptance is permanently forbidden",
                settlement.as_str()
            )));
        }
        // C may never precede U for a published stream.
        if stream.terminal_sequence.is_none() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} has no durable publication terminal; canonical acceptance requires U"
            )));
        }
        if !matches!(message, MessageBlock::Assistant(_)) {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} requires an Assistant message"
            )));
        }
        if !request_outcome_is_durable(
            transaction,
            &self.conversation_id,
            &snapshot,
            completed_sequence,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        ensure_surface_head(transaction)?;

        let transcript_cursor =
            validation
                .append(transaction, message, None)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "canonical publication must receive a transcript cursor".to_owned(),
                    )
                })?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical publication commit"));
        }
        let mut persisted = persist_event_tx(transaction, &self.conversation_id, event)?;
        content::settle_canonical(transaction, stream_id, message, final_frames)?;
        rebind_published_read_layouts(transaction, message, &persisted.event)?;
        // The canonical transition clears the lifecycle staging of the stream:
        // the Ledger is now the long-term authority and no transient frame
        // survives it.
        clear_publication_staging(transaction, stream_id)?;
        // Proposal ownership remains as one bounded `(stream_id, call_id)`
        // fact after C. Recovery ToolResult repair and later Tool Plane
        // transitions must still resolve the exact accepted proposal rather
        // than falling back to a bare call id.
        transaction
            .execute(
                "UPDATE publication_proposals SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication proposals: {error}")))?;
        transaction
            .execute(
                "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication: {error}")))?;
        let mut receipt = canonical_observation_receipt(
            transaction,
            &self.conversation_id,
            message,
            Some(transcript_cursor),
            persisted.event.tool_occurrence.clone(),
        )?;
        let sequence = persisted.event.sequence;
        set_canonical_range(&mut receipt, Some((sequence, sequence)));
        persisted.event.observation = observation_receipt::canonical(
            transaction,
            &self.conversation_id,
            &persisted.event,
            Arc::clone(&receipt.observation),
        );
        Ok((persisted.event, receipt))
    }

    #[allow(clippy::too_many_lines)] // One native pending adoption transition.
    pub(super) fn adopt_pending_tx(
        &self,
        transaction: &Transaction<'_>,
        validation: &mut append_validation::Batch<'_>,
        watermark: InboundSequence,
        attempt_id: Option<crate::runtime::identity::AttemptId>,
    ) -> Result<crate::durable::inbox::AdoptedBatch, ConversationStoreError> {
        #[cfg(test)]
        if let Some(probe) = self
            .pending_adoption_transaction_probe
            .lock()
            .unwrap()
            .take()
        {
            probe();
        }
        ensure_surface_head(transaction)?;

        let items = load_pending_until(transaction, watermark)?;
        for item in &items {
            let block = MessageBlock::User(item.message.clone());
            let cursor = validation.append(transaction, &block, Some(&item.message_id))?;
            if cursor != item.transcript_cursor {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "adopted inbound {} changed its transcript cursor from {:?} to {:?}",
                    item.message_id, item.transcript_cursor, cursor
                )));
            }
        }
        transaction
            .execute(
                "DELETE FROM pending_inbound WHERE sequence <= ?1",
                [seq_to_i64(watermark.get())?],
            )
            .map_err(|error| storage(format!("adopt pending delete: {error}")))?;
        for item in &items {
            // The removed native pending row fences these derived bytes even
            // if cleanup fails; their deletion cannot refuse adoption.
            let _ = optional_display_layout(transaction, || {
                content::clear_pending(transaction, &item.sequence.get().to_string())
            });
        }
        if !items.is_empty() {
            transaction.execute("UPDATE rustx_store SET pending_observation_count=pending_observation_count-?1,reading_mutation_revision=reading_mutation_revision+1 WHERE id=1",[items.len() as u64]).map_err(|error|storage(error.to_string()))?;
        }
        let mut adoption = None;
        if !items.is_empty() {
            let fact = crate::durable::inbox::inbound_adoption_event(
                &self.conversation_id,
                attempt_id,
                items.iter().map(|item| item.message_id.clone()).collect(),
            );
            adoption = Some(persist_event_tx(transaction, &self.conversation_id, fact)?.event);
        }
        #[cfg(test)]
        if self.consume_admission_fault(AdmissionFaultOperation::AdoptPendingBatch)
            || Self::consume(&self.fail_adopt_remaining)
        {
            return Err(storage("fault injected: adopt commit"));
        }
        let read_cut = reading::cut(transaction, &self.conversation_id)?;
        let canonical_receipts = canonical_observation_batch(
            transaction,
            &self.conversation_id,
            &items
                .iter()
                .map(|item| (&item.message_id, item.transcript_cursor))
                .collect::<Vec<_>>(),
            adoption
                .as_ref()
                .map(|event| (event.sequence, event.sequence)),
        )?;
        if let Some(event) = &mut adoption {
            event.observation = canonical_receipts.first().and_then(|receipt| {
                observation_receipt::canonical(
                    transaction,
                    &self.conversation_id,
                    event,
                    Arc::clone(&receipt.observation),
                )
            });
        }
        let pending_observation = observation_receipt::pending(
            transaction,
            &self.conversation_id,
            None,
            None,
            Some((watermark.get(), items.len() as u64)),
        );
        Ok(crate::durable::inbox::AdoptedBatch {
            pending_observation,
            canonical_receipts,
            read_cut,
            items,
            adoption,
        })
    }
}

impl SqliteConversationStore {
    #[allow(clippy::too_many_lines)] // One auditable native final-output transaction.
    pub(super) fn settle_model_turn_native(
        &self,
        settlement: crate::durable::inbox::ModelTurnSettlement<'_>,
    ) -> Result<crate::durable::inbox::CommittedModelTurn, ConversationStoreError> {
        use crate::durable::inbox::{CommittedModelTurn, CommittedModelTurnBoundary};
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(super::TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("model settlement transaction: {error}")))?;
        let stream = require_publication_stream(&transaction, settlement.stream_id)?;
        if stream.terminal_sequence.is_some() || stream.settlement.is_some() {
            return Err(ConversationStoreError::PublicationViolation(
                "model output is already terminal or settled".into(),
            ));
        }
        if settlement.provider.conversation_id != self.conversation_id
            || settlement.provider.attempt_id != Some(stream.start.attempt_id.clone())
            || settlement.provider.turn_id.as_ref() != Some(&stream.start.turn_id)
            || !matches!(&settlement.provider.event, super::RuntimeEvent::ModelRequestCompleted { request_id, .. } if request_id == &stream.start.request_id)
        {
            return Err(ConversationStoreError::PublicationViolation(
                "model completion does not own the stream".into(),
            ));
        }
        let (_, _, prior_completion) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        if prior_completion.is_some() {
            return Err(ConversationStoreError::PublicationViolation(
                "model output already has a provider completion".into(),
            ));
        }
        let super::RuntimeEvent::ModelRequestCompleted { finish_reason, .. } =
            &settlement.provider.event
        else {
            unreachable!("validated completion")
        };
        let finish_reason = finish_reason.clone();
        let has_tools = matches!(settlement.assistant, MessageBlock::Assistant(message) if message.content.iter().any(|block| matches!(block, crate::message::types::AssistantContentBlock::ToolCall(_))));
        if has_tools == settlement.boundary.is_some() {
            return Err(ConversationStoreError::PublicationViolation(
                "model settlement has the wrong tool/inbox boundary".into(),
            ));
        }
        let provider =
            persist_event_tx(&transaction, &self.conversation_id, settlement.provider)?.event;
        super::process_death::reach("during:model_settlement_provider");
        // Final frames become canonical in this same transaction. They need no
        // transient staging or live receipts; only the canonical receipt may
        // publish the complete accepted output after COMMIT.
        self.final_publication_terminal_tx(&transaction, settlement.stream_id, settlement.frames)?;
        super::process_death::reach("during:model_settlement_frames");
        let (canonical, canonical_receipt) = self.canonical_publication_tx(
            &transaction,
            &mut validation,
            settlement.stream_id,
            settlement.assistant,
            settlement.canonical,
            settlement.frames,
        )?;
        super::process_death::reach("during:model_settlement_canonical");
        let boundary = if let Some(boundary) = settlement.boundary {
            if !matches!(
                boundary.turn_completed.event,
                super::RuntimeEvent::TurnCompleted
            ) || !matches!(&boundary.attempt_completed.event, super::RuntimeEvent::AttemptCompleted { attempt_id, finish_reason: terminal_reason } if attempt_id == &stream.start.attempt_id && terminal_reason == &finish_reason)
                || [&boundary.turn_completed, &boundary.attempt_completed]
                    .iter()
                    .any(|event| {
                        event.conversation_id != self.conversation_id
                            || event.attempt_id != Some(stream.start.attempt_id.clone())
                    })
                || boundary.turn_completed.turn_id.as_ref() != Some(&stream.start.turn_id)
                || boundary.attempt_completed.turn_id.is_some()
            {
                return Err(ConversationStoreError::PublicationViolation(
                    "model boundary does not own the stream".into(),
                ));
            }
            let turn_completed =
                persist_event_tx(&transaction, &self.conversation_id, boundary.turn_completed)?
                    .event;
            let (adopted, attempt_completed) = if let Some(watermark) = boundary.pending_watermark {
                let adopted = self.adopt_pending_tx(
                    &transaction,
                    &mut validation,
                    watermark,
                    Some(stream.start.attempt_id.clone()),
                )?;
                if adopted.items.is_empty() {
                    return Err(ConversationStoreError::PublicationViolation(
                        "selected model boundary inbox became empty".into(),
                    ));
                }
                (Some(adopted), None)
            } else {
                let pending: bool = transaction
                    .query_row("SELECT EXISTS(SELECT 1 FROM pending_inbound)", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| storage(error.to_string()))?;
                if pending {
                    return Err(ConversationStoreError::PublicationViolation(
                        "empty model boundary has pending inbound".into(),
                    ));
                }
                #[cfg(test)]
                {
                    self.terminal_event_attempts
                        .fetch_add(1, super::Ordering::SeqCst);
                    if Self::consume(&self.fail_terminal_event_remaining) {
                        return Err(storage("fault injected: final model terminal commit"));
                    }
                }
                let terminal = persist_event_tx(
                    &transaction,
                    &self.conversation_id,
                    boundary.attempt_completed,
                )?
                .event;
                (None, Some(terminal))
            };
            Some(CommittedModelTurnBoundary {
                turn_completed,
                adopted,
                attempt_completed,
            })
        } else {
            None
        };
        super::process_death::reach("before:settle_model_turn");
        transaction
            .commit()
            .map_err(|error| storage(format!("model settlement commit: {error}")))?;
        validation.committed(&connection);
        super::process_death::reach("after:settle_model_turn");
        Ok(CommittedModelTurn {
            provider,
            canonical,
            canonical_receipt,
            boundary,
        })
    }
}
