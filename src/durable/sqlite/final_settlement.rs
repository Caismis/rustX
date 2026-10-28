//! Native final-output transaction parts. The caller owns COMMIT and publication.
#[cfg(test)]
use super::AdmissionFaultOperation;
use super::{
    Arc, ConversationStoreError, InboundSequence, MessageBlock, PublicationFrame,
    PublicationSettlement, PublicationStreamId, PublicationStreamStart, RuntimeEventEnvelope,
    SqliteConversationStore, Transaction, TranscriptCommitReceipt, append_validation,
    canonical_observation_batch, canonical_observation_receipt, clear_publication_staging, content,
    ensure_publication_stream_tx, ensure_surface_head, load_pending_until, native_request,
    observation_receipt, optional_display_layout, params, persist_event_tx_deferred,
    persist_event_tx_with_requests, persist_publication_progress_tx, prepare_publication_frames_tx,
    reading, request_outcome_is_durable, require_publication_stream,
    require_started_request_with_validation, seq_to_i64, set_canonical_range, stage_frames_tx,
    storage, validate_canonical_publication_event, validate_canonical_tool_proposals,
    validate_publication_generation,
};
use crate::durable::sqlite::statements::FixedStatements;
impl SqliteConversationStore {
    pub(super) fn publication_terminal_tx(
        &self,
        transaction: &Transaction<'_>,
        start: &PublicationStreamStart,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError> {
        let mut requests = native_request::Validation::new(transaction);
        ensure_publication_stream_tx(&mut requests, &self.conversation_id, start)?;
        self.validate_publication_terminal_tx(&mut requests, &start.stream_id, frames)?;
        let (stream, receipts) = stage_frames_tx(transaction, frames, &self.conversation_id)?;
        Self::finish_publication_terminal_tx(transaction, &start.stream_id, frames, &stream)?;
        Ok(receipts)
    }

    /// The final canonical transaction consumes these frames before COMMIT, so
    /// neither a staging row nor a live `ContentRef` may escape it. Retain only
    /// proposal ownership and sequence/terminal progress here; raw Tool suffixes
    /// are copied directly to canonical authority by `canonical_publication_tx`.
    fn final_publication_terminal_tx(
        &self,
        requests: &mut native_request::Validation<'_, '_>,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError> {
        let transaction = requests.transaction();
        self.validate_publication_terminal_tx(requests, stream_id, frames)?;
        let prepared = prepare_publication_frames_tx(transaction, frames)?;
        persist_publication_progress_tx(transaction, &prepared)?;
        Self::finish_publication_terminal_tx(transaction, stream_id, frames, &prepared.stream)
    }

    fn validate_publication_terminal_tx(
        &self,
        requests: &mut native_request::Validation<'_, '_>,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError> {
        let transaction = requests.transaction();
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
        let (snapshot, _, completed_sequence) = require_started_request_with_validation(
            requests,
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
            .execute_cached(
                "UPDATE publication_streams SET terminal_sequence=?1 WHERE stream_id=?2",
                params![seq_to_i64(terminal_sequence)?, stream_id.as_str()],
            )
            .map_err(|error| storage(format!("commit publication terminal: {error}")))?;
        Ok(())
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One native canonical transition and its transaction-owned final suffixes.
    pub(super) fn canonical_publication_tx(
        &self,
        requests: &mut native_request::Validation<'_, '_>,
        validation: &mut append_validation::Batch<'_>,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
        final_frames: &[PublicationFrame],
    ) -> Result<(super::PersistedEvent, TranscriptCommitReceipt), ConversationStoreError> {
        let transaction = requests.transaction();
        let stream = require_publication_stream(transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_with_validation(
            requests,
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

        let mut publication_header = None;
        let transcript_cursor = if self.conn.event_guard.can_prepare(transaction) {
            validation.append_publication(transaction, message, &mut publication_header)?
        } else {
            validation.append(transaction, message, None)?
        }
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "canonical publication must receive a transcript cursor".to_owned(),
            )
        })?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical publication commit"));
        }
        let publication = super::event_insert::CanonicalPublication {
            stream: stream_id,
            message,
            final_frames,
        };
        let persisted = persist_event_tx_with_requests(
            &self.conn.event_guard,
            requests,
            &self.conversation_id,
            event,
            false,
            None,
            Some(&publication),
            None,
        )?;
        if !persisted.facts.canonical_publication_prepared {
            content::settle_canonical(transaction, stream_id, message, final_frames)?;
            let _ = optional_display_layout(transaction, || {
                super::observation_trace::settle_fields(transaction, &persisted.event)
            });
        }
        // Final raw bytes and T are now fixed. Reuse only this transaction's
        // successful pre-trim capture, retaining full repair after any failure.
        super::rebind_canonical_display(transaction, message, publication_header)?;
        // The canonical transition clears the lifecycle staging of the stream:
        // the Ledger is now the long-term authority and no transient frame
        // survives it.
        clear_publication_staging(transaction, stream_id)?;
        // Proposal ownership remains as one bounded `(stream_id, call_id)`
        // fact after C. Recovery ToolResult repair and later Tool Plane
        // transitions must still resolve the exact accepted proposal rather
        // than falling back to a bare call id.
        transaction
            .execute_cached(
                "UPDATE publication_proposals SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication proposals: {error}")))?;
        if !persisted.facts.canonical_publication_prepared {
            transaction
                .execute_cached(
                    "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                    params![
                        PublicationSettlement::Canonical.as_str(),
                        stream_id.as_str()
                    ],
                )
                .map_err(|error| storage(format!("settle canonical publication: {error}")))?;
        }
        let mut receipt = canonical_observation_receipt(
            transaction,
            &self.conversation_id,
            message,
            Some(transcript_cursor),
            persisted.event.tool_occurrence.clone(),
        )?;
        let sequence = persisted.event.sequence;
        set_canonical_range(&mut receipt, Some((sequence, sequence)));
        Ok((persisted, receipt))
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
            .execute_cached(
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
            transaction.execute_cached("UPDATE rustx_store SET pending_observation_count=pending_observation_count-?1,reading_mutation_revision=reading_mutation_revision+1 WHERE id=1",[items.len() as u64]).map_err(|error|storage(error.to_string()))?;
        }
        let mut adoption = None;
        if !items.is_empty() {
            let fact = crate::durable::inbox::inbound_adoption_event(
                &self.conversation_id,
                attempt_id,
                items.iter().map(|item| item.message_id.clone()).collect(),
            );
            adoption = Some(
                persist_event_tx_deferred(
                    &self.conn.event_guard,
                    transaction,
                    &self.conversation_id,
                    fact,
                )?
                .event,
            );
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
        let mut requests = native_request::Validation::new(&transaction);
        let stream =
            ensure_publication_stream_tx(&mut requests, &self.conversation_id, settlement.start)?;
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
        let (_, _, prior_completion) = require_started_request_with_validation(
            &mut requests,
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
        let mut provider = persist_event_tx_with_requests(
            &self.conn.event_guard,
            &mut requests,
            &self.conversation_id,
            settlement.provider,
            false,
            None,
            None,
            None,
        )?;
        super::process_death::reach("during:model_settlement_provider");
        // Final frames become canonical in this same transaction. They need no
        // transient staging or live receipts; only the canonical receipt may
        // publish the complete accepted output after COMMIT.
        self.final_publication_terminal_tx(
            &mut requests,
            &settlement.start.stream_id,
            settlement.frames,
        )?;
        super::process_death::reach("during:model_settlement_frames");
        let (canonical, mut canonical_receipt) = self.canonical_publication_tx(
            &mut requests,
            &mut validation,
            &settlement.start.stream_id,
            settlement.assistant,
            settlement.canonical,
            settlement.frames,
        )?;
        let previous_members = canonical.facts.members;
        let mut canonical = canonical.event;
        super::process_death::reach("during:model_settlement_canonical");
        let mut turn_facts = None;
        let mut terminal_facts = None;
        let mut boundary = if let Some(boundary) = settlement.boundary {
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
            let turn_completed = persist_event_tx_deferred(
                &self.conn.event_guard,
                &transaction,
                &self.conversation_id,
                boundary.turn_completed,
            )?;
            turn_facts = Some(turn_completed.facts);
            let turn_completed = turn_completed.event;
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
                    .query_row_cached("SELECT EXISTS(SELECT 1 FROM pending_inbound)", [], |row| {
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
                let terminal = persist_event_tx_with_requests(
                    &self.conn.event_guard,
                    &mut native_request::Validation::new(&transaction),
                    &self.conversation_id,
                    boundary.attempt_completed,
                    false,
                    None,
                    None,
                    previous_members.as_ref(),
                )?;
                terminal_facts = Some(terminal.facts);
                (None, Some(terminal.event))
            };
            Some(CommittedModelTurnBoundary {
                turn_completed,
                adopted,
                attempt_completed,
            })
        } else {
            None
        };
        install_model_settlement_observation(
            &transaction,
            &self.conversation_id,
            &mut provider,
            &mut canonical,
            &mut canonical_receipt,
            boundary.as_mut(),
            turn_facts.as_ref(),
            terminal_facts.as_ref(),
        );
        super::process_death::reach("before:settle_model_turn");
        transaction
            .commit()
            .map_err(|error| storage(format!("model settlement commit: {error}")))?;
        validation.committed(&connection);
        super::process_death::reach("after:settle_model_turn");
        Ok(CommittedModelTurn {
            provider: provider.event,
            canonical,
            canonical_receipt,
            boundary,
        })
    }
}

/// Bind the closed final-output family after all native rows exist. Individual
/// callbacks retain their original canonical heads and order, but only the last
/// Journal member installs/releases the complete family. A failed derived read
/// leaves the semantic commit intact and fences the entire missing receipt.
#[allow(clippy::too_many_arguments)] // One native transaction's four fixed event roles.
fn install_model_settlement_observation(
    connection: &super::Connection,
    conversation: &super::ConversationId,
    provider: &mut super::PersistedEvent,
    canonical: &mut RuntimeEventEnvelope,
    canonical_receipt: &mut TranscriptCommitReceipt,
    mut boundary: Option<&mut crate::durable::inbox::CommittedModelTurnBoundary>,
    turn_facts: Option<&super::append_facts::Facts>,
    terminal_facts: Option<&super::append_facts::Facts>,
) {
    use observation_receipt::ModelTurnBoundaryEvents;
    let last = boundary.as_ref().map_or(canonical.sequence, |boundary| {
        boundary
            .attempt_completed
            .as_ref()
            .or_else(|| {
                boundary
                    .adopted
                    .as_ref()
                    .and_then(|adopted| adopted.adoption.as_ref())
            })
            .expect("committed model boundary owns its final event")
            .sequence
    });
    Arc::make_mut(&mut canonical_receipt.observation).native_release_sequence = Some(last);
    let assistant = Arc::clone(&canonical_receipt.observation);
    let mut release = (*assistant).clone();
    let mut adoption_canonical = None;
    let mut pending = None;
    if let Some(adopted) = boundary
        .as_mut()
        .and_then(|boundary| boundary.adopted.as_mut())
    {
        let mut canonical = Arc::clone(&adopted.canonical_receipts[0].observation);
        Arc::make_mut(&mut canonical).native_release_sequence = Some(last);
        for receipt in &mut adopted.canonical_receipts {
            receipt.observation = Arc::clone(&canonical);
        }
        if let Some(receipt) = &mut adopted.pending_observation {
            Arc::make_mut(receipt).native_release_sequence = Some(last);
            pending = Some(Arc::clone(receipt));
        }
        // Only the release range is merged. Each intermediate canonical head
        // stays separately owned, including an Assistant displaced by >8 Users.
        release = (*canonical).clone();
        release.first_position = assistant.first_position;
        release.count += assistant.count;
        release.transcript_count += assistant.transcript_count;
        release.first_transcript = release
            .first_transcript
            .into_iter()
            .chain(assistant.first_transcript)
            .min();
        release.last_transcript = release
            .last_transcript
            .into_iter()
            .chain(assistant.last_transcript)
            .max();
        release.ready_through = assistant.ready_through;
        release.ready_t_through = assistant.ready_t_through;
        release.source_unavailable |= assistant.source_unavailable;
        adoption_canonical = Some(canonical);
    }
    release.journal_range = Some((provider.event.sequence, last));
    let events = boundary.as_ref().map(|boundary| {
        if let Some(attempt) = &boundary.attempt_completed {
            ModelTurnBoundaryEvents::Completed {
                turn: (&boundary.turn_completed, turn_facts),
                attempt: (attempt, terminal_facts),
            }
        } else {
            ModelTurnBoundaryEvents::Adopted {
                turn: (&boundary.turn_completed, turn_facts),
                adoption: boundary
                    .adopted
                    .as_ref()
                    .and_then(|adopted| adopted.adoption.as_ref())
                    .expect("committed adoption event"),
                canonical: adoption_canonical.expect("committed adoption canonical receipt"),
            }
        }
    });
    let shared = observation_receipt::model_turn_settlement(
        connection,
        conversation,
        (&provider.event, Some(&provider.facts)),
        (canonical, assistant),
        events,
        Arc::new(release),
        pending,
    );
    provider.event.observation.clone_from(&shared);
    canonical.observation.clone_from(&shared);
    if let Some(boundary) = boundary {
        boundary.turn_completed.observation.clone_from(&shared);
        if let Some(terminal) = &mut boundary.attempt_completed {
            terminal.observation.clone_from(&shared);
        }
        if let Some(adoption) = boundary
            .adopted
            .as_mut()
            .and_then(|adopted| adopted.adoption.as_mut())
        {
            adoption.observation = shared;
        }
    }
}
