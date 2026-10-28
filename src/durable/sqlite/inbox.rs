//! The Pending Inbound Inbox and Goal domain: acceptance, adoption, and exact
//! pending mutation, each committed in the caller's single transaction.

#[cfg(test)]
mod tests;

#[cfg(test)]
use super::AdmissionFaultOperation;
use super::canonical::canonical_observation_batch;
use super::canonical::ledger::{
    append_transcript_reference, ensure_surface_head, load_message_tx, load_transcript_reference,
    message_exists,
};
use super::journal::reference::validate_subagent_terminal_publication;
use super::journal::{find_event_by_id, persist_event_tx};
use super::{
    SqliteConversationStore, append_validation, content, decode, encode, map_insert_error,
    observation, observation_receipt, observation_state, optional_display_layout, read_sequence,
    reading, seq_to_i64, sequence_from_i64, storage,
};
use crate::durable::{
    AcceptedInbound, ConversationStoreError, InboundDraft, InboxStore, PendingBatch,
    PendingInboundItem, TranscriptCursor,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::runtime::identity::{ConversationId, EventId, MessageId};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::process_death;
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::sync::Arc;

impl InboxStore for SqliteConversationStore {
    fn load_goal(&self) -> Result<Option<crate::goal::GoalSnapshot>, ConversationStoreError> {
        read_goal(&*self.lock()?)
    }
    fn write_goal(
        &self,
        write: crate::goal::GoalWrite,
    ) -> Result<
        (
            crate::goal::GoalResult,
            Option<Arc<crate::durable::inbox::GoalObservationReceipt>>,
        ),
        ConversationStoreError,
    > {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Goal transaction: {error}")))?;
        let previous = read_goal(&tx)?;
        let change = write.change();
        let result = crate::goal::transition(previous.as_ref(), write);
        let mut receipt = None;
        if let Ok(goal) = &result {
            save_goal(&tx, goal)?;
            #[cfg(test)]
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage("fault injected: Goal journal commit"));
            }
            let event = persist_event_tx(
                &tx,
                &self.conversation_id,
                crate::goal::journal_envelope(
                    &self.conversation_id,
                    crate::goal::GoalFact::Written {
                        previous: previous.map(|g| g.reference),
                        current: goal.reference.clone(),
                        phase: goal.phase,
                        change,
                    },
                ),
            )?
            .event;
            receipt = Some(Arc::new(crate::durable::inbox::GoalObservationReceipt {
                view: crate::observation::goal::RuntimeClientGoalView::from_native(Some(goal)),
                native: event.observation,
                pending: None,
            }));
            tx.commit()
                .map_err(|error| storage(format!("Goal commit: {error}")))?;
        }
        Ok((result, receipt))
    }
    fn accept_goal_round(
        &self,
        expected: &crate::goal::GoalRef,
        draft: InboundDraft,
    ) -> Result<Option<AcceptedInbound>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Goal admission transaction: {error}")))?;
        let Some(mut goal) = read_goal(&tx)? else {
            return Ok(None);
        };
        if &goal.reference != expected
            || goal.phase != crate::goal::GoalPhase::Active
            || goal.autonomous_rounds_consumed >= goal.autonomous_round_budget
        {
            return Ok(None);
        }
        let pending: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM pending_inbound)", [], |row| {
                row.get(0)
            })
            .map_err(|error| storage(format!("Goal pending probe: {error}")))?;
        if pending {
            return Ok(None);
        }
        if draft.source != UserSource::Runtime
            || draft.kind != InboundKind::GoalContinuation(expected.clone())
        {
            return Err(ConversationStoreError::InvalidReference(
                "Goal admission requires its exact typed runtime trigger".to_owned(),
            ));
        }
        let mut accepted = accept_inbound_tx(self, &tx, draft)?;
        if accepted.retried {
            return Err(ConversationStoreError::InvalidReference(
                "Goal round identity already accepted".to_owned(),
            ));
        }
        goal.autonomous_rounds_consumed += 1;
        goal.reference.revision = goal
            .reference
            .revision
            .checked_add(1)
            .ok_or(ConversationStoreError::SequenceExhausted)?;
        goal.last_round_message_id = Some(accepted.message_id.clone());
        save_goal(&tx, &goal)?;
        let event = persist_event_tx(
            &tx,
            &self.conversation_id,
            crate::goal::journal_envelope(
                &self.conversation_id,
                crate::goal::GoalFact::RoundAdmitted {
                    previous: expected.clone(),
                    current: goal.reference.clone(),
                    round: goal.autonomous_rounds_consumed,
                    message_id: accepted.message_id.clone(),
                },
            ),
        )?
        .event;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: Goal accept commit"));
        }
        refresh_pending_receipt(&tx, &self.conversation_id, &mut accepted)?;
        accepted.goal_observation = Some(Arc::new(crate::durable::inbox::GoalObservationReceipt {
            view: crate::observation::goal::RuntimeClientGoalView::from_native(Some(&goal)),
            native: event.observation,
            pending: accepted.pending_observation.clone(),
        }));
        process_death::reach("before:accept_goal_round");
        tx.commit()
            .map_err(|error| storage(format!("Goal admission commit: {error}")))?;
        process_death::reach("after:accept_goal_round");
        Ok(Some(accepted))
    }
    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError> {
        reject_unaccounted_goal(&draft)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept transaction: {error}")))?;
        let accepted = accept_inbound_tx(self, &transaction, draft)?;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: accept commit"));
        }
        process_death::reach("before:accept_inbound");
        transaction
            .commit()
            .map_err(|error| storage(format!("accept commit: {error}")))?;
        process_death::reach("after:accept_inbound");
        Ok(accepted)
    }
    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        mut event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError> {
        reject_unaccounted_goal(&draft)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept/event transaction: {error}")))?;
        // Issue #192: this generic compound transition owns exactly the
        // detached background terminal domain. A normal subagent terminal
        // fact has its own semantically closed authority
        // (`accept_subagent_terminal`) and is rejected here, so no
        // `SubagentTerminalPublished` can bypass the Issue #192
        // terminal-publication contract.
        let RuntimeEvent::BackgroundTerminalPublished {
            execution_id,
            message_id: event_message_id,
            ..
        } = &event.event
        else {
            return Err(ConversationStoreError::InvalidReference(
                "inbound/event acceptance owns only a detached background terminal fact; a \
                 subagent terminal publishes through accept_subagent_terminal"
                    .to_owned(),
            ));
        };
        if event.event_id.as_str().is_empty() {
            event.event_id = EventId::new(format!("background-terminal-event:{execution_id}"));
        }
        let mut accepted = accept_inbound_tx(self, &transaction, draft)?;
        if *event_message_id != accepted.message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "detached terminal fact references {}, accepted inbound is {}",
                event_message_id, accepted.message_id
            )));
        }
        if accepted.retried {
            let Some(existing) = find_event_by_id(&transaction, &event.event_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "correlated inbound {} has no durable terminal fact",
                    accepted.message_id
                )));
            };
            if existing.event != event.event {
                return Err(ConversationStoreError::InvalidReference(
                    "correlated inbound terminal fact conflicts with the stored fact".to_owned(),
                ));
            }
            transaction
                .commit()
                .map_err(|error| storage(format!("accept/event retry commit: {error}")))?;
            return Ok((accepted, existing));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: accept/event commit"));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: accept/event journal commit"));
        }
        process_death::reach_event("before:event", &event.event);
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        refresh_pending_receipt(&transaction, &self.conversation_id, &mut accepted)?;
        if let Some(native) = &mut persisted.event.observation {
            Arc::make_mut(native)
                .pending
                .clone_from(&accepted.pending_observation);
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("accept/event commit: {error}")))?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok((accepted, persisted.event))
    }
    #[allow(clippy::too_many_lines)] // one durable linearization boundary
    fn accept_subagent_terminal(
        &self,
        notice: Option<InboundDraft>,
        draft: InboundDraft,
        mut event: RuntimeEventEnvelope,
    ) -> Result<
        (
            Option<AcceptedInbound>,
            AcceptedInbound,
            RuntimeEventEnvelope,
        ),
        ConversationStoreError,
    > {
        let _ownership = self.ownership_transition(&event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("subagent terminal transaction: {error}")))?;
        // The transition is semantically closed (Issue #192): every input
        // is validated against the durable ownership fact and the canonical
        // identities before anything is accepted, so no semantically
        // unreachable durable state can be created.
        validate_subagent_terminal_publication(&transaction, notice.as_ref(), &draft, &event)?;
        if event.event_id.as_str().is_empty()
            && let RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } = &event.event
        {
            event.event_id = EventId::new(format!("subagent-terminal-event:{subagent_id}"));
        }
        let event_message_id = match &event.event {
            RuntimeEvent::SubagentTerminalPublished { message_id, .. } => message_id.clone(),
            _ => unreachable!("the payload was validated above"),
        };
        // The notice is ordered first, so the terminal report remains the
        // last item of the publication.
        let notice = notice
            .map(|notice| accept_inbound_tx(self, &transaction, notice))
            .transpose()?;
        let mut accepted = accept_inbound_tx(self, &transaction, draft)?;
        if event_message_id != accepted.message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "subagent terminal fact references {}, accepted inbound is {}",
                event_message_id, accepted.message_id
            )));
        }
        // The notice/report pair is one atomic publication: a retry must
        // find both sides already accepted, never exactly one.
        if let Some(notice) = &notice
            && notice.retried != accepted.retried
        {
            return Err(ConversationStoreError::InvalidReference(
                "a subagent terminal notice/report pair must retry together".to_owned(),
            ));
        }
        if accepted.retried {
            let Some(existing) = find_event_by_id(&transaction, &event.event_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "correlated inbound {} has no durable terminal fact",
                    accepted.message_id
                )));
            };
            if existing.event != event.event {
                return Err(ConversationStoreError::InvalidReference(
                    "correlated inbound terminal fact conflicts with the stored fact".to_owned(),
                ));
            }
            transaction
                .commit()
                .map_err(|error| storage(format!("subagent terminal retry commit: {error}")))?;
            return Ok((notice, accepted, existing));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: subagent terminal accept commit"));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: subagent terminal journal commit"));
        }
        process_death::reach_event("before:event", &event.event);
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        refresh_pending_receipt(&transaction, &self.conversation_id, &mut accepted)?;
        if let Some(native) = &mut persisted.event.observation {
            Arc::make_mut(native)
                .pending
                .clone_from(&accepted.pending_observation);
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("subagent terminal commit: {error}")))?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok((notice, accepted, persisted.event))
    }
    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError> {
        #[cfg(test)]
        {
            if self.consume_admission_fault(AdmissionFaultOperation::SelectPendingBatch)
                || Self::consume(&self.fail_select_remaining)
            {
                return Err(storage("fault injected: select pending batch"));
            }
        }
        let connection = self.lock()?;
        let items = load_pending_rows(&connection)?;
        let Some(watermark) = items.last().map(|item| item.sequence) else {
            return Ok(None);
        };
        Ok(Some(PendingBatch {
            conversation_id: self.conversation_id.clone(),
            watermark,
            items,
        }))
    }
    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
        attempt_id: Option<crate::runtime::identity::AttemptId>,
    ) -> Result<crate::durable::inbox::AdoptedBatch, ConversationStoreError> {
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("adopt transaction: {error}")))?;
        #[cfg(test)]
        if let Some(probe) = self
            .pending_adoption_transaction_probe
            .lock()
            .unwrap()
            .take()
        {
            probe();
        }
        ensure_surface_head(&transaction)?;

        let items = load_pending_until(&transaction, watermark)?;
        for item in &items {
            let block = MessageBlock::User(item.message.clone());
            let cursor = validation.append(&transaction, &block, Some(&item.message_id))?;
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
            let _ = optional_display_layout(&transaction, || {
                content::clear_pending(&transaction, &item.sequence.get().to_string())
            })?;
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
            adoption = Some(persist_event_tx(&transaction, &self.conversation_id, fact)?.event);
        }
        #[cfg(test)]
        if self.consume_admission_fault(AdmissionFaultOperation::AdoptPendingBatch)
            || Self::consume(&self.fail_adopt_remaining)
        {
            return Err(storage("fault injected: adopt commit"));
        }
        let read_cut = reading::cut(&transaction, &self.conversation_id)?;
        let canonical_receipts = canonical_observation_batch(
            &transaction,
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
                    &transaction,
                    &self.conversation_id,
                    event,
                    Arc::clone(&receipt.observation),
                )
            });
        }
        let pending_observation = observation_receipt::pending(
            &transaction,
            &self.conversation_id,
            None,
            None,
            Some((watermark.get(), items.len() as u64)),
        );
        process_death::reach("before:adopt_pending_batch");
        transaction
            .commit()
            .map_err(|error| storage(format!("adopt commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach("after:adopt_pending_batch");
        Ok(crate::durable::inbox::AdoptedBatch {
            pending_observation,
            canonical_receipts,
            read_cut,
            items,
            adoption,
        })
    }
    fn edit_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
        text: &str,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, ConversationStoreError> {
        self.mutate_pending(expected, Some(text))
    }
    fn remove_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, ConversationStoreError> {
        self.mutate_pending(expected, None)
    }
    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        let connection = self.lock()?;
        load_pending_rows(&connection)
    }
    fn has_accepted_inbound(&self) -> Result<bool, ConversationStoreError> {
        // The durable acceptance watermark. `accept_inbound` advances it in
        // the acceptance commit, adoption never touches it, and lineage
        // seeding initializes it to zero without an acceptance, so a
        // nonzero value is permanent proof that user work was durably
        // accepted into this conversation.
        let connection = self.lock()?;
        let watermark: i64 = connection
            .query_row(
                "SELECT next_inbound_sequence FROM rustx_store WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("read inbound acceptance watermark: {error}")))?;
        Ok(watermark > 0)
    }
}

pub(super) fn rewrite_pending_body(
    transaction: &Transaction<'_>,
    item: &mut PendingInboundItem,
    expected: &crate::durable::inbox::PendingInboundRef,
    text: &str,
) -> Result<(), ConversationStoreError> {
    item.message.content = vec![UserContentBlock::Text(crate::message::content::TextBlock {
        text: text.to_owned(),
    })];
    let revision = item
        .revision
        .checked_add(1)
        .filter(|value| i64::try_from(*value).is_ok())
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    transaction
        .execute(
            "UPDATE pending_inbound SET message_json=?1, revision=?2 WHERE sequence=?3",
            params![
                encode(&item.message, "pending edit")?,
                revision,
                seq_to_i64(expected.sequence.get())?
            ],
        )
        .map_err(|error| storage(format!("pending edit: {error}")))?;
    let _ = optional_display_layout(transaction, || {
        content::index_pending(
            transaction,
            &item.message,
            expected.sequence.get(),
            revision,
        )?;
        observation::index::index_message(
            transaction,
            &MessageBlock::User(item.message.clone()),
            None,
        )
    })?;
    let _ = observation::index::index_pending_header(
        transaction,
        expected.sequence.get(),
        &item.message_id,
        None,
    );
    Ok(())
}

pub(super) fn reject_unaccounted_goal(draft: &InboundDraft) -> Result<(), ConversationStoreError> {
    if matches!(draft.kind, InboundKind::GoalContinuation(_)) {
        return Err(ConversationStoreError::InvalidReference(
            "Goal continuation must cross accept_goal_round with atomic accounting".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn read_goal(
    connection: &Connection,
) -> Result<Option<crate::goal::GoalSnapshot>, ConversationStoreError> {
    let json: Option<String> = connection
        .query_row(
            "SELECT snapshot_json FROM goal_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("read Goal: {error}")))?;
    json.map(|json| {
        serde_json::from_str(&json).map_err(|error| storage(format!("decode Goal: {error}")))
    })
    .transpose()
}

pub(super) fn save_goal(
    tx: &Transaction<'_>,
    goal: &crate::goal::GoalSnapshot,
) -> Result<(), ConversationStoreError> {
    let header = crate::observation::goal::RuntimeClientGoalView::from_native(Some(goal));
    let bytes = crate::observation::bounded::bounded_bytes(&header, 16384);
    tx.execute("INSERT INTO goal_state(singleton, snapshot_json,observation_header) VALUES(1, ?1,?2) ON CONFLICT(singleton) DO UPDATE SET snapshot_json = excluded.snapshot_json,observation_header=excluded.observation_header,observation_source_id=NULL,observation_sequence=NULL",
        params![encode(goal, "Goal")?,bytes]).map_err(|error| storage(format!("write Goal: {error}")))?;
    let _ = optional_display_layout(tx, || observation_state::goal(tx, goal))?;
    Ok(())
}

pub(super) fn refresh_pending_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    accepted: &mut AcceptedInbound,
) -> Result<(), ConversationStoreError> {
    accepted.read_cut = reading::cut(connection, conversation)?;
    accepted.pending_observation = observation_receipt::pending(
        connection,
        conversation,
        Some(accepted.sequence.get()),
        None,
        None,
    );
    Ok(())
}

pub(super) fn accept_inbound_tx(
    store: &SqliteConversationStore,
    transaction: &Transaction<'_>,
    draft: InboundDraft,
) -> Result<AcceptedInbound, ConversationStoreError> {
    if draft
        .correlation
        .as_deref()
        .is_some_and(|key| key.starts_with("scheduler:"))
        || draft
            .message_id
            .as_ref()
            .is_some_and(|id| id.as_str().starts_with("scheduler:"))
    {
        return Err(ConversationStoreError::InvalidReference(
            "Scheduler identities require atomic scheduled admission".into(),
        ));
    }
    accept_scheduled_inbound_tx(store, transaction, draft)
}

#[allow(clippy::too_many_lines)] // One acceptance transaction validates the full inbound contract.
pub(super) fn accept_scheduled_inbound_tx(
    store: &SqliteConversationStore,
    transaction: &Transaction<'_>,
    draft: InboundDraft,
) -> Result<AcceptedInbound, ConversationStoreError> {
    if draft.content.is_empty() {
        return Err(ConversationStoreError::EmptyContent);
    }

    if let Some(correlation) = draft.correlation.as_deref() {
        let existing: Option<(i64, String)> = transaction
            .query_row(
                "SELECT sequence, message_id FROM inbound_correlation WHERE correlation = ?1",
                [correlation],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("correlation probe: {error}")))?;
        if let Some((sequence, message_id)) = existing {
            let message =
                load_correlated_user_message(transaction, &MessageId::new(message_id.clone()))?;
            let same = draft
                .message_id
                .as_ref()
                .is_none_or(|id| id.as_str() == message_id)
                && draft.source == message.source
                && draft.kind == message.kind
                && draft.content == message.content
                && message.timestamp == Some(draft.timestamp);
            if !same {
                return Err(ConversationStoreError::CorrelationConflict {
                    correlation: correlation.to_owned(),
                });
            }
            let transcript_cursor = if matches!(message.kind, InboundKind::Context(_)) {
                None
            } else {
                Some(
                    load_transcript_reference(transaction, "message", &message_id)?.ok_or_else(
                        || {
                            ConversationStoreError::InvalidReference(format!(
                                "accepted inbound {message_id} is missing its transcript reference"
                            ))
                        },
                    )?,
                )
            };
            return Ok(AcceptedInbound {
                goal_observation: None,
                pending_observation: observation_receipt::pending(
                    transaction,
                    &store.conversation_id,
                    Some(sequence_from_i64(sequence)?),
                    None,
                    None,
                ),
                read_cut: reading::cut(transaction, &store.conversation_id)?,
                sequence: InboundSequence::new(sequence_from_i64(sequence)?),
                message_id: MessageId::new(message_id),
                message,
                transcript_cursor,
                retried: true,
            });
        }
    }
    let current: i64 = transaction
        .query_row(
            "SELECT next_inbound_sequence FROM rustx_store WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read inbound sequence: {error}")))?;
    let next = current
        .checked_add(1)
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    let sequence = u64::try_from(next).map_err(|_| ConversationStoreError::SequenceExhausted)?;
    let message_id = draft
        .message_id
        .clone()
        .unwrap_or_else(|| MessageId::new(format!("{}-inbound-{sequence}", store.conversation_id)));
    if message_exists(transaction, &message_id)? {
        return Err(ConversationStoreError::DuplicateMessageId(message_id));
    }
    let message = UserMessageBlock {
        id: message_id.clone(),
        content: draft.content,
        source: draft.source,
        kind: draft.kind,
        timestamp: Some(draft.timestamp),
    };
    let json = encode(&message, "inbound")?;
    transaction
        .execute(
            "INSERT INTO pending_inbound(sequence,message_id,message_json,correlation) VALUES(?1,?2,?3,?4)",
            params![next, message_id.as_str(), json, draft.correlation.as_deref()],
        )
        .map_err(|error| map_insert_error(&error, &message_id))?;
    transaction
        .execute(
            "UPDATE rustx_store SET next_inbound_sequence = ?1,pending_observation_count=pending_observation_count+1 WHERE id = 1",
            [next],
        )
        .map_err(|error| storage(format!("update inbound sequence: {error}")))?;
    if let Some(correlation) = draft.correlation.as_deref() {
        transaction
            .execute(
                "INSERT INTO inbound_correlation(correlation,sequence,message_id) VALUES(?1,?2,?3)",
                params![correlation, next, message_id.as_str()],
            )
            .map_err(|error| storage(format!("insert correlation: {error}")))?;
    }
    let _ = optional_display_layout(transaction, || {
        content::index_pending(transaction, &message, sequence, 0)?;
        observation::index::index_message(transaction, &MessageBlock::User(message.clone()), None)
    })?;
    // Acceptance is the durable-before-display frontier for an ordinary
    // inbound user message. Context facts are durable model input but are not
    // ordinary transcript content, so they never receive an ordering row.
    // Adoption later reuses this same reference when a visible body moves
    // into the Message Ledger.
    let transcript_cursor = if matches!(message.kind, InboundKind::Context(_)) {
        None
    } else {
        Some(append_transcript_reference(
            transaction,
            "message",
            message_id.as_str(),
        )?)
    };
    let _ = observation::index::index_pending_header(
        transaction,
        sequence,
        &message_id,
        transcript_cursor.map(TranscriptCursor::get),
    );
    Ok(AcceptedInbound {
        goal_observation: None,
        pending_observation: observation_receipt::pending(
            transaction,
            &store.conversation_id,
            Some(sequence),
            None,
            None,
        ),
        read_cut: reading::cut(transaction, &store.conversation_id)?,
        sequence: InboundSequence::new(sequence),
        message_id,
        message,
        transcript_cursor,
        retried: false,
    })
}

pub(super) fn pending_message_exists(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE message_id=?1)",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("pending message identity probe: {error}")))
}

pub(super) fn load_correlated_user_message(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
) -> Result<UserMessageBlock, ConversationStoreError> {
    let pending_json: Option<String> = transaction
        .query_row(
            "SELECT message_json FROM pending_inbound WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("correlated pending lookup: {error}")))?;
    if let Some(json) = pending_json {
        let message: UserMessageBlock = decode(&json, "correlated pending message")?;
        if message.id != *message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "correlation for {message_id} points to a pending body with a different identity"
            )));
        }
        return Ok(message);
    }
    let message = load_message_tx(transaction, message_id)?;
    let MessageBlock::User(message) = message else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "correlation for {message_id} points to a non-User Ledger fact"
        )));
    };
    if message.id != *message_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "correlation for {message_id} points to a Ledger body with a different identity"
        )));
    }
    Ok(message)
}

pub(super) fn load_pending_rows(
    connection: &Connection,
) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT p.sequence,p.message_id,p.message_json,p.correlation,t.position,p.revision
             FROM pending_inbound p
             LEFT JOIN transcript_order t
               ON t.reference_kind='message' AND t.reference_id=p.message_id
             ORDER BY p.sequence",
        )
        .map_err(|error| storage(format!("load pending: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            let sequence: i64 = row.get(0)?;
            let message_id = MessageId::new(row.get::<_, String>(1)?);
            let json: String = row.get(2)?;
            let message: UserMessageBlock = serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(PendingInboundItem {
                revision: row.get::<_, u64>(5)?,
                sequence: InboundSequence::new(read_sequence(sequence)?),
                message_id,
                message,
                transcript_cursor: row
                    .get::<_, Option<i64>>(4)?
                    .map(|position| {
                        u64::try_from(position)
                            .map(TranscriptCursor::new)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })
                    })
                    .transpose()?,
                correlation: row.get(3)?,
            })
        })
        .map_err(|error| storage(format!("pending query: {error}")))?;
    let items: Vec<PendingInboundItem> = rows
        .map(|row| row.map_err(|error| storage(format!("pending row: {error}"))))
        .collect::<Result<_, _>>()?;
    for item in &items {
        if item.message.id != item.message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "pending row {} contains a different message identity",
                item.message_id
            )));
        }
        let visible = !matches!(item.message.kind, InboundKind::Context(_));
        if visible != item.transcript_cursor.is_some() {
            return Err(ConversationStoreError::InvalidReference(format!(
                "pending inbound {} has an invalid transcript reference",
                item.message_id
            )));
        }
    }
    Ok(items)
}

pub(super) fn load_pending_exact(
    connection: &Connection,
    sequence: InboundSequence,
) -> Result<Option<PendingInboundItem>, ConversationStoreError> {
    type PendingIdentityRow = (String, String, Option<String>, Option<u64>, u64);
    let row:Option<PendingIdentityRow>=connection.query_row("SELECT p.message_id,p.message_json,p.correlation,t.position,p.revision FROM pending_inbound p LEFT JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=p.message_id WHERE p.sequence=?1",[sequence.get()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(|error|storage(error.to_string()))?;
    row.map(|(id, json, correlation, cursor, revision)| {
        let message: UserMessageBlock =
            serde_json::from_str(&json).map_err(|error| storage(error.to_string()))?;
        if message.id.as_str() != id
            || matches!(message.kind, InboundKind::Context(_)) == cursor.is_some()
        {
            return Err(ConversationStoreError::InvalidReference(
                "pending source identity or transcript mismatch".into(),
            ));
        }
        Ok(PendingInboundItem {
            revision,
            sequence,
            message_id: MessageId::new(id),
            message,
            transcript_cursor: cursor.map(TranscriptCursor::new),
            correlation,
        })
    })
    .transpose()
}

pub(super) fn load_pending_until(
    transaction: &Transaction<'_>,
    watermark: InboundSequence,
) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
    Ok(load_pending_rows(transaction)?
        .into_iter()
        .filter(|item| item.sequence <= watermark)
        .collect())
}

impl SqliteConversationStore {
    pub(super) fn mutate_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
        text: Option<&str>,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, ConversationStoreError> {
        use crate::durable::inbox::PendingMutationOutcome as Outcome;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("pending mutation: {error}")))?;
        #[cfg(test)]
        if let Some(probe) = self
            .pending_mutation_transaction_probe
            .lock()
            .unwrap()
            .take()
        {
            probe();
        }
        let revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM pending_inbound WHERE sequence=?1",
                [expected.sequence.get()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        let Some(revision) = revision else {
            return Ok(Outcome::NotPending);
        };
        if revision != expected.revision {
            return Ok(Outcome::Conflict);
        }
        let Some(mut item) = load_pending_exact(&transaction, expected.sequence)? else {
            return Ok(Outcome::NotPending);
        };
        // Producer-correlated runtime work is not user-authored queue content.
        if item.message.source != UserSource::Human
            || item.message.kind != InboundKind::Message
            || item.correlation.is_some()
        {
            return Ok(Outcome::InvalidItem);
        }
        if let Some(text) = text {
            if text.trim().is_empty()
                || !matches!(item.message.content.as_slice(), [UserContentBlock::Text(_)])
            {
                return Ok(Outcome::InvalidItem);
            }
            rewrite_pending_body(&transaction, &mut item, expected, text)?;
        } else {
            let _ = optional_display_layout(&transaction, || {
                content::clear_pending(&transaction, &item.sequence.get().to_string())
            })?;
            transaction
                .execute(
                    "DELETE FROM pending_inbound WHERE sequence=?1",
                    [seq_to_i64(expected.sequence.get())?],
                )
                .map_err(|error| storage(format!("pending remove: {error}")))?;
            let _ = optional_display_layout(&transaction, || {
                transaction.execute("DELETE FROM observation_headers WHERE reference_kind='message' AND reference_id=?1",[item.message_id.as_str()]).map_err(|error|storage(error.to_string()))?;
                Ok(())
            })?;
            transaction.execute("DELETE FROM transcript_order WHERE reference_kind='message' AND reference_id=?1", [item.message_id.as_str()])
                .map_err(|error| storage(format!("pending transcript remove: {error}")))?;
            transaction.execute("UPDATE rustx_store SET retained_transcript_count=retained_transcript_count-1,pending_observation_count=pending_observation_count-1 WHERE id=1", []).map_err(|error|storage(error.to_string()))?;
        }
        transaction.execute("UPDATE rustx_store SET reading_mutation_revision=reading_mutation_revision+1 WHERE id=1", [])
            .map_err(|error| storage(format!("reading mutation: {error}")))?;
        let read_cut = reading::cut(&transaction, &self.conversation_id)?;
        let pending_observation = observation_receipt::pending(
            &transaction,
            &self.conversation_id,
            text.map(|_| expected.sequence.get()),
            text.is_none()
                .then_some((expected.sequence.get(), item.transcript_cursor)),
            None,
        );
        transaction
            .commit()
            .map_err(|error| storage(format!("pending mutation commit: {error}")))?;
        Ok(Outcome::Applied {
            read_cut,
            pending_observation,
        })
    }
}
