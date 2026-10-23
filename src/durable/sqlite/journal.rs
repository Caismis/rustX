//! The Event Journal domain: validated execution-fact persistence.

#[cfg(test)]
mod tests;

use super::sqlite_error;

use crate::durable::sqlite::auxiliary;
use crate::durable::sqlite::request::snapshot::find_request_start_event;
use crate::durable::sqlite::schema::validate_primary_request_owner_event;
use crate::model::ModelRequestOwner;
use rusqlite::OptionalExtension;
pub(super) mod reference;

use self::reference::{
    native_approval_key, validate_agent_admission_reference, validate_event_reference,
};
use super::canonical::ledger::{append_transcript_reference, load_message_tx};
use super::request::validate_request_start_metadata;
use super::schema::presentation_statements;
use super::{
    SqliteConversationStore, decode, encode, journal_prefix, observation, observation_event,
    observation_execution, observation_members, observation_occupancy, observation_readiness,
    observation_receipt, observation_request, observation_state, observation_statistics,
    observation_trace, observation_turn, optional_display_layout, seq_to_i64, sequence_from_i64,
    storage,
};
use crate::durable::{ConversationStoreError, EventPage, JournalStore, TranscriptCursor};
use crate::events::interaction::InteractionSubject;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{MessageBlock, UserMessageBlock};
use crate::model::snapshot::RequestSnapshot;
use crate::runtime::identity::{ConversationId, EventId, MessageId};
use crate::runtime::process_death;
use rusqlite::{Transaction, TransactionBehavior, params};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;

impl JournalStore for SqliteConversationStore {
    fn load_agent_authority(
        &self,
        agent_id: &crate::runtime::identity::AgentId,
    ) -> Result<crate::runtime::subagent::DurableAgentAuthority, ConversationStoreError> {
        let connection = self.lock()?;
        let encoded: Option<String> = connection
            .query_row(
                "SELECT authority_json FROM agent_authorities WHERE agent_id=?1",
                [agent_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage("read private Agent authority"))?;
        let encoded = encoded.ok_or_else(|| storage("missing admitted Agent authority"))?;
        let mut private: PrivateAgentAuthority = serde_json::from_str(&encoded)
            .map_err(|_| storage("decode private Agent authority"))?;
        let credentials = crate::credentials::CredentialSnapshot::new(private.credentials);
        private
            .authority
            .resolved
            .model
            .restore_admitted_credentials(&credentials)
            .map_err(|_| storage("restore private Agent provider credentials"))?;
        for binding in private
            .authority
            .resolved
            .materialization
            .sources
            .values_mut()
        {
            binding.credentials.capture(credentials.clone());
        }
        Ok(private.authority)
    }
    fn append_agent_admission(
        &self,
        event: RuntimeEventEnvelope,
        authority: &crate::runtime::subagent::DurableAgentAuthority,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        self.append_event_internal(event, Some(authority))
    }
    fn append_event(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        if requires_specialized_transition(&event.event)
            || (event.event.is_auxiliary_model_request()
                && matches!(
                    event.event,
                    RuntimeEvent::ModelRequestCompleted { .. }
                        | RuntimeEvent::ModelRequestFailed { .. }
                ))
        {
            return Err(ConversationStoreError::InvalidReference(
                "this event kind must use its specialized durable transition".to_owned(),
            ));
        }
        self.append_event_internal(event, None)
    }
    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        if !matches!(
            &event.event,
            RuntimeEvent::SubagentTerminalSettled {
                state: crate::events::types::SubagentTerminalState::Failed
                    | crate::events::types::SubagentTerminalState::Cancelled
                    | crate::events::types::SubagentTerminalState::Interrupted,
                ..
            }
        ) {
            return Err(ConversationStoreError::InvalidReference(
                "a direct Workflow child terminal transition accepts only non-success states"
                    .to_owned(),
            ));
        }
        let _ownership = self.ownership_transition(&event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Workflow child terminal transaction: {error}")))?;
        if let Some(existing) = find_event_by_id(&transaction, &event.event_id)? {
            if same_event_ignoring_sequence(&existing, &event) {
                transaction.commit().map_err(|error| {
                    storage(format!("Workflow child terminal retry commit: {error}"))
                })?;
                return Ok(existing);
            }
            return Err(ConversationStoreError::TerminalViolation(
                "Workflow child terminal retry conflicts with its durable fact".to_owned(),
            ));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: Workflow child terminal commit"));
        }
        process_death::reach_event("before:workflow_child_terminal", &event.event);
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("Workflow child terminal commit: {error}")))?;
        process_death::reach_event("after:workflow_child_terminal", &persisted.event.event);
        Ok(persisted.event)
    }
    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError> {
        let valid_pair = matches!(
            (&terminal.event, &output.event),
            (
                RuntimeEvent::SubagentTerminalSettled {
                    subagent_id: terminal_subagent,
                    state: crate::events::types::SubagentTerminalState::Succeeded,
                    physical_settlement_proven: true,
                    ..
                },
                RuntimeEvent::WorkflowAgentOutputCommitted {
                    subagent_id: output_subagent,
                    ..
                }
            ) if terminal_subagent == output_subagent
        );
        if !valid_pair {
            return Err(ConversationStoreError::InvalidReference(
                "the Workflow terminal transaction requires one matching physically settled successful child terminal and output fact".to_owned(),
            ));
        }
        if terminal.conversation_id != output.conversation_id
            || terminal.timestamp != output.timestamp
        {
            return Err(ConversationStoreError::InvalidReference(
                "the Workflow terminal facts must share conversation and timestamp identity"
                    .to_owned(),
            ));
        }

        let _ownership = self.ownership_transition(&terminal.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Workflow terminal transaction: {error}")))?;
        validate_event_reference(&transaction, &output)?;
        let existing_terminal = find_event_by_id(&transaction, &terminal.event_id)?;
        let existing_output = find_event_by_id(&transaction, &output.event_id)?;
        match (existing_terminal, existing_output) {
            (Some(existing_terminal), Some(existing_output)) => {
                if !same_event_ignoring_sequence(&existing_terminal, &terminal)
                    || !same_event_ignoring_sequence(&existing_output, &output)
                {
                    return Err(ConversationStoreError::InvalidReference(
                        "Workflow terminal retry conflicts with its durable facts".to_owned(),
                    ));
                }
                transaction
                    .commit()
                    .map_err(|error| storage(format!("Workflow terminal retry commit: {error}")))?;
                Ok((existing_terminal, existing_output))
            }
            (Some(_), None) | (None, Some(_)) => Err(ConversationStoreError::TerminalViolation(
                "Workflow terminal facts are only valid as an atomic pair".to_owned(),
            )),
            (None, None) => {
                let mut persisted_terminal =
                    persist_event_tx_deferred(&transaction, &self.conversation_id, terminal)?;
                let mut persisted_output =
                    persist_event_tx_deferred(&transaction, &self.conversation_id, output)?;
                #[cfg(test)]
                if Self::consume(&self.fail_event_remaining) {
                    return Err(storage("fault injected: Workflow terminal commit"));
                }
                let shared = observation_receipt::workflow_terminal(
                    &transaction,
                    &self.conversation_id,
                    &persisted_terminal.event,
                    &persisted_output.event,
                );
                persisted_terminal.event.observation.clone_from(&shared);
                persisted_output.event.observation = shared;
                process_death::reach_event(
                    "before:workflow_terminal",
                    &persisted_output.event.event,
                );
                transaction
                    .commit()
                    .map_err(|error| storage(format!("Workflow terminal commit: {error}")))?;
                process_death::reach_event(
                    "after:workflow_terminal",
                    &persisted_output.event.event,
                );
                Ok((persisted_terminal.event, persisted_output.event))
            }
        }
    }
    fn append_interaction_audit(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        if !matches!(
            event.event,
            RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
        ) {
            return Err(ConversationStoreError::InvalidReference(
                "the interaction audit transition accepts only interaction facts".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("interaction audit transaction: {error}")))?;
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        let transcript_cursor = persisted.transcript_cursor.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "interaction audit did not receive a transcript cursor".to_owned(),
            )
        })?;
        process_death::reach_event("before:interaction", &persisted.event.event);
        transaction
            .commit()
            .map_err(|error| storage(format!("interaction audit commit: {error}")))?;
        process_death::reach_event("after:interaction", &persisted.event.event);
        Ok((persisted.event, transcript_cursor))
    }
    fn observe_journal(
        &self,
        observer: Arc<dyn crate::durable::presentation::JournalObserver>,
    ) -> Result<u64, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(error.to_string()))?;
        let through = journal_prefix(&transaction)?;
        // A newly installed Runtime owns recovery of already committed history.
        // Previous process-local publication holes cannot survive this epoch.
        observation_readiness::recover_journal(&transaction)?;
        observation_readiness::mark(&transaction, 1, through)?;
        transaction
            .execute(
                "UPDATE event_observation_readiness SET direct=0 WHERE direct=1",
                [],
            )
            .map_err(|error| storage(error.to_string()))?;
        observation_readiness::recover_transcript(&transaction)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        *self.journal_observer.lock().expect("Journal observer lock") = Some((observer, through));
        Ok(through)
    }
    fn presentation_frontier(&self) -> Result<u64, ConversationStoreError> {
        let value: i64 = self
            .lock()?
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(|_| storage("presentation frontier failed"))?;
        sequence_from_i64(value)
    }
    fn read_presentation_events(
        &self,
        query: &crate::durable::presentation::FactQuery,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        let statements = presentation_statements(query)?;
        let connection = self.lock()?;
        let mut events: Vec<RuntimeEventEnvelope> = Vec::new();
        for (sql, params) in statements {
            let mut statement = connection
                .prepare(&sql)
                .map_err(|_| storage("presentation query preparation failed"))?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|_| storage("presentation query failed"))?;
            for row in rows {
                #[cfg(test)]
                observation_statistics::original_read();
                events.push(decode(
                    &row.map_err(|_| storage("presentation row failed"))?,
                    "presentation fact",
                )?);
            }
        }
        events.sort_by_key(|event| event.sequence);
        if !query.ascending {
            events.reverse();
        }
        events.truncate(query.limit);
        if matches!(
            query.scope,
            crate::durable::presentation::FactScope::AgentRequest
        ) {
            for event in &events {
                validate_primary_request_owner_event(&connection, event)?;
            }
        }
        Ok(events)
    }
    fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<EventPage, ConversationStoreError> {
        let connection = self.lock()?;
        let after = after_sequence.unwrap_or(0);
        let limit = i64::try_from(limit).map_err(|_| storage("event page limit is too large"))?;
        let mut statement = connection
            .prepare(
                "SELECT sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json
                 FROM events WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
            )
            .map_err(|error| storage(format!("event page: {error}")))?;
        let mut rows = statement
            .query(params![after, limit])
            .map_err(|error| storage(format!("event page query: {error}")))?;
        let mut events = Vec::new();
        let mut next = None;
        while let Some(row) = rows
            .next()
            .map_err(|error| storage(format!("event page row: {error}")))?
        {
            let sequence: i64 = row
                .get(0)
                .map_err(|error| storage(format!("event sequence: {error}")))?;
            let stored_event_id: String = row
                .get(1)
                .map_err(|error| storage(format!("event identity: {error}")))?;
            let stored_schema_version: i64 = row
                .get(2)
                .map_err(|error| storage(format!("event schema version: {error}")))?;
            let stored_conversation_id: String = row
                .get(3)
                .map_err(|error| storage(format!("event conversation identity: {error}")))?;
            let stored_attempt_id: Option<String> = row
                .get(4)
                .map_err(|error| storage(format!("event attempt identity: {error}")))?;
            let stored_turn_id: Option<String> = row
                .get(5)
                .map_err(|error| storage(format!("event turn identity: {error}")))?;
            let json: String = row
                .get(6)
                .map_err(|error| storage(format!("event json: {error}")))?;
            let event: RuntimeEventEnvelope = decode(&json, "event page")?;
            let sequence =
                u64::try_from(sequence).map_err(|_| storage("negative event sequence"))?;
            let stored_schema_version = u16::try_from(stored_schema_version).map_err(|_| {
                ConversationStoreError::InvalidReference(
                    "Event Journal row has an invalid schema version".to_owned(),
                )
            })?;
            if event.sequence != sequence
                || event.event_id.as_str() != stored_event_id
                || event.schema_version != stored_schema_version
                || event.schema_version != EVENT_SCHEMA_VERSION
                || event.conversation_id.as_str() != stored_conversation_id
                || event.conversation_id != self.conversation_id
                || event.attempt_id.as_ref().map(ToString::to_string) != stored_attempt_id
                || event.turn_id.as_ref().map(ToString::to_string) != stored_turn_id
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Event Journal row metadata disagrees with its envelope".to_owned(),
                ));
            }
            events.push(event);
            next = Some(sequence);
        }
        Ok(EventPage {
            events,
            next_sequence: next,
        })
    }
}

pub(super) struct PersistedEvent {
    pub(super) event: RuntimeEventEnvelope,
    pub(super) transcript_cursor: Option<TranscriptCursor>,
}

/// Private executable authority and every captured credential share one row.
/// This type is never included in `RuntimeEvent`, projections or archives.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrivateAgentAuthority {
    pub(super) authority: crate::runtime::subagent::DurableAgentAuthority,
    pub(super) credentials: Vec<(String, String)>,
}

pub(super) fn persist_agent_authority_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEvent,
    authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
) -> Result<(), ConversationStoreError> {
    match (event, authority) {
        (
            RuntimeEvent::SubagentOwnershipCommitted {
                child_agent_id,
                admitted_authority: Some(reference),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                ..
            },
            Some(authority),
        ) if reference == child_agent_id => {
            let mut authority = authority.clone();
            let mut credentials = Vec::new();
            authority
                .resolved
                .model
                .export_process_credentials(&mut credentials);
            for binding in authority.resolved.materialization.sources.values_mut() {
                binding
                    .credentials
                    .export_process_credentials(&mut credentials);
            }
            let encoded = serde_json::to_string(&PrivateAgentAuthority {
                authority,
                credentials,
            })
            .map_err(|_| storage("encode private Agent authority"))?;
            transaction
                .execute(
                    "INSERT INTO agent_authorities(agent_id,authority_json) VALUES(?1,?2)",
                    params![child_agent_id.as_str(), encoded],
                )
                .map_err(|_| storage("commit private Agent authority"))?;
            Ok(())
        }
        (
            RuntimeEvent::SubagentOwnershipCommitted {
                admitted_authority: Some(_),
                ..
            },
            _,
        )
        | (_, Some(_)) => Err(ConversationStoreError::InvalidReference(
            "Agent admission requires matching private authority and public identity".to_owned(),
        )),
        (_, None) => Ok(()),
    }
}

pub(super) fn persist_event_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_internal(transaction, conversation_id, event, true, None)
}

pub(super) fn persist_event_tx_deferred(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_internal(transaction, conversation_id, event, false, None)
}

// Fresh-start-only proof, borrowed from the exact transaction that has just
// validated its stored Snapshot and reconstructed every immutable dependency.
// Generic Journal insertions always perform their own reference validation.
pub(super) struct ValidatedRequestStart<'a, 'connection> {
    pub(super) transaction: &'a Transaction<'connection>,
    pub(super) snapshot: &'a RequestSnapshot,
}

#[allow(clippy::too_many_lines)] // One atomic event, lifecycle and private admission transaction.
pub(super) fn persist_event_tx_internal(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    mut event: RuntimeEventEnvelope,
    prepare_observation: bool,
    validated_start: Option<ValidatedRequestStart<'_, '_>>,
) -> Result<PersistedEvent, ConversationStoreError> {
    if event.schema_version != EVENT_SCHEMA_VERSION {
        return Err(ConversationStoreError::InvalidReference(format!(
            "event schema {} is not {}",
            event.schema_version, EVENT_SCHEMA_VERSION
        )));
    }
    if event.conversation_id != *conversation_id {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: conversation_id.clone(),
            requested: event.conversation_id,
        });
    }
    validate_event_identity(&event)?;
    validate_attempt_start_uniqueness(transaction, &event)?;
    validate_agent_admission_reference(transaction, &event)?;
    if let Some(proof) = validated_start {
        if !std::ptr::eq(proof.transaction, transaction) {
            return Err(ConversationStoreError::InvalidReference(
                "fresh request validation belongs to a different transaction".into(),
            ));
        }
        validate_request_start_metadata(proof.snapshot, &event)?;
    } else {
        validate_event_reference(transaction, &event)?;
    }
    if let RuntimeEvent::ModelRequestStarted { request_id, .. } = &event.event
        && find_request_start_event(transaction, request_id)?.is_some()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} already has a request-start fact"
        )));
    }
    let current: i64 = transaction
        .query_row(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("read event sequence: {error}")))?;
    let sequence = current
        .checked_add(1)
        .ok_or_else(|| storage("event sequence exhausted"))?;
    if event.event_id.as_str().is_empty() {
        event.event_id = EventId::new(format!("{conversation_id}-event-{sequence}"));
    }
    event.sequence = u64::try_from(sequence).map_err(|_| storage("event sequence exhausted"))?;
    let lifecycles = lifecycle_keys(&event);
    for (key, _) in &lifecycles {
        let existing: Option<Option<String>> = transaction
            .query_row(
                "SELECT terminal_event_id FROM lifecycle_state WHERE lifecycle_key=?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sqlite_error(&error, format!("lifecycle probe: {error}")))?;
        if existing.as_ref().is_some_and(Option::is_some) {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "lifecycle {key} is already terminal"
            )));
        }
    }
    for (key, terminal) in lifecycles {
        if terminal {
            transaction
                .execute(
                    "INSERT INTO lifecycle_state(lifecycle_key,terminal_event_id) VALUES(?1,?2) ON CONFLICT(lifecycle_key) DO UPDATE SET terminal_event_id=excluded.terminal_event_id",
                    params![key, event.event_id.as_str()],
                )
                .map_err(|error| sqlite_error(&error, format!("record terminal lifecycle: {error}")))?;
        } else {
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    sqlite_error(&error, format!("lifecycle existence probe: {error}"))
                })?;
            if !exists {
                transaction
                    .execute(
                        "INSERT INTO lifecycle_state(lifecycle_key,terminal_event_id) VALUES(?1,NULL)",
                        [&key],
                    )
                    .map_err(|error| sqlite_error(&error, format!("record lifecycle: {error}")))?;
            }
        }
    }
    let json = encode(&event, "runtime event")?;
    transaction
        .execute(
            "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp,observation_trace_required,observation_statistics_required,observation_kind,observation_members_required,observation_turn_required,observation_trace_fact_required,observation_execution_required,observation_unavailable) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1)",
            params![
                sequence,
                event.event_id.as_str(),
                i64::from(event.schema_version),
                event.conversation_id.as_str(),
                event.attempt_id.as_ref().map(ToString::to_string),
                event.turn_id.as_ref().map(ToString::to_string),
                json,
                event.timestamp.to_rfc3339(),
                observation_statistics::is_trace_anchor(&event.event),
                observation_statistics::requires_statistics(&event),
                observation_event::native_kind(&event.event).ok(),
                observation_members::requires_members(&event),
                observation_turn::required(&event),
                observation_trace::required(&event),
                observation_execution::required(&event),
            ],
        )
        .map_err(|error| sqlite_error(&error, format!("insert runtime event: {error}")))?;
    auxiliary::index_event_tx(transaction, &event)?;
    // Missing auxiliary evidence is the fail-closed initial state. Independent
    // savepoints preserve own lifecycle facts if a different layout fails.
    // Readiness has its own recoverable missing-evidence flag. A newly
    // installed Runtime may repair it without clearing unrelated layout holes.
    let _ = optional_display_layout(transaction, || {
        observation_readiness::insert(transaction, &event)
    })?;
    let statistics_available = optional_display_layout(transaction, || {
        observation_statistics::event(transaction, &event)?;
        observation_members::member_event(transaction, &event)
    })?
    .is_some();
    if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
        transaction.execute("UPDATE canonical_tool_calls SET attempt_id=?1,commit_sequence=?2 WHERE assistant_message_id=?3",params![event.attempt_id.as_ref().map(crate::runtime::identity::AttemptId::as_str),sequence,message_id.as_str()]).map_err(|error|sqlite_error(&error, error.to_string()))?;
    }

    // Caller-supplied ephemeral evidence is never trusted. Native canonical
    // ownership is resolved inside this exact append transaction.
    let assistant_available = optional_display_layout(transaction, || {
        if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
            transaction.execute("UPDATE events SET observation_assistant_ledger=(SELECT position FROM message_ledger WHERE message_id=?2) WHERE sequence=?1",params![sequence,message_id.as_str()]).map_err(|error|sqlite_error(&error, error.to_string()))?;
        }
        Ok(())
    })?.is_some();
    event.tool_occurrence = observation::tool_occurrence_receipt(transaction, &event)?;
    let tool_available = optional_display_layout(transaction, || {
    if let Some(occurrence) = &event.tool_occurrence {
        let phase = match &event.event {
            RuntimeEvent::ToolExecutionStarted { .. } => Some(1),
            RuntimeEvent::ToolExecutionCompleted { .. }
            | RuntimeEvent::ToolExecutionFailed { .. } => Some(2),
            _ => None,
        };
        transaction.execute("UPDATE events SET observation_tool_ledger=?1,observation_tool_block=?2,observation_tool_phase=?4 WHERE sequence=?3",params![occurrence.ledger_position,occurrence.block_index.get(),sequence,phase]).map_err(|error|sqlite_error(&error, error.to_string()))?;
    }
        Ok(())
    })?.is_some();
    let execution_available = optional_display_layout(transaction, || {
        observation_execution::event(transaction, &event)
    })?
    .is_some();
    let state_available = optional_display_layout(transaction, || {
        observation_state::event(transaction, &event)
    })?
    .is_some();
    let request_available = optional_display_layout(transaction, || {
        observation_occupancy::event(transaction, &event)?;
        observation_request::terminal(transaction, &event)
    })?
    .is_some();
    let witness_available = observation_event::index_optional(transaction, &event)?;
    let trace_available = optional_display_layout(transaction, || {
        observation_trace::index_event(transaction, &event)
    })?
    .is_some();
    let observation_json = crate::observation::bounded::bounded_bytes(
        &event,
        crate::durable::observation::HEADER_ENCODED_MAX,
    );
    let json_available = optional_display_layout(transaction, || {
        #[cfg(test)]
        let _probe =
            super::start_probe::for_event(&event, super::AuxiliaryStartProbe::ObservationJson);
        transaction
            .execute(
                "UPDATE events SET observation_json=?2 WHERE sequence=?1",
                params![sequence, observation_json],
            )
            .map_err(|error| sqlite_error(&error, error.to_string()))?;
        Ok(())
    })?
    .is_some();
    transaction
        .execute(
            "UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1",
            [sequence],
        )
        .map_err(|error| sqlite_error(&error, format!("update event sequence: {error}")))?;
    let transcript_cursor = if matches!(
        &event.event,
        RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
    ) {
        Some(append_transcript_reference(
            transaction,
            "attempt_terminal",
            event.event_id.as_str(),
        )?)
    } else if matches!(
        &event.event,
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
    ) {
        Some(append_transcript_reference(
            transaction,
            "interaction_event",
            event.event_id.as_str(),
        )?)
    } else {
        None
    };
    let transcript_available = enrich_event_transcript(transaction, &event, transcript_cursor)?;
    let turn_available =
        optional_display_layout(transaction, || observation_turn::event(transaction, &event))?
            .is_some();
    // The **P** marker: a durable provider outcome is recorded against its
    // exact request, so the publication plane can reject U-without-P with one
    // keyed lookup instead of a Journal scan.
    if let RuntimeEvent::ModelRequestCompleted {
        owner: ModelRequestOwner::Agent { .. },
        request_id,
        ..
    } = &event.event
    {
        let updated = transaction
            .execute(
                "UPDATE request_snapshots SET completed_sequence=?1 WHERE request_id=?2 AND completed_sequence IS NULL",
                params![sequence, request_id.as_str()],
            )
            .map_err(|error| sqlite_error(&error, format!("record provider outcome: {error}")))?;
        if updated != 1 {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "request {request_id} already has a durable provider outcome"
            )));
        }
    }
    if statistics_available
        && assistant_available
        && tool_available
        && execution_available
        && state_available
        && request_available
        && witness_available
        && trace_available
        && json_available
        && transcript_available
        && turn_available
    {
        // The marker clear is auxiliary too: its failure leaves the original
        // native J/C/T committed and this row explicitly unavailable.
        let _ = optional_display_layout(transaction, || {
            transaction
                .execute(
                    "UPDATE events SET observation_unavailable=0 WHERE sequence=?1",
                    [sequence],
                )
                .map_err(|error| sqlite_error(&error, error.to_string()))?;
            Ok(())
        })?;
    }
    event.observation = prepare_observation
        .then(|| observation_receipt::single(transaction, conversation_id, &event))
        .flatten();
    Ok(PersistedEvent {
        event,
        transcript_cursor,
    })
}

/// Contained derived-layout failures preserve the original transcript seat.
/// Transaction-integrity failures must still abort the owning native write.
pub(super) fn enrich_event_transcript(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
    cursor: Option<TranscriptCursor>,
) -> Result<bool, ConversationStoreError> {
    let Some(cursor) = cursor else {
        return Ok(true);
    };
    let coordinates = optional_display_layout(transaction, || {
        transaction
            .execute(
                "UPDATE events SET observation_transcript=?1 WHERE sequence=?2",
                params![cursor.get(), event.sequence],
            )
            .map_err(|error| sqlite_error(&error, error.to_string()))?;
        Ok(())
    })?
    .is_some();
    let witness = optional_display_layout(transaction, || {
        observation_event::attach_transcript(transaction, event.sequence, cursor)
    })?
    .is_some();
    let kind = if matches!(
        event.event,
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
    ) {
        "interaction_event"
    } else {
        "attempt_terminal"
    };
    let header = optional_display_layout(transaction, || {
        observation::index::index_native_event(transaction, kind, event, cursor)
    })?
    .is_some();
    Ok(coordinates && witness && header)
}

pub(super) fn validate_event_identity(
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if matches!(
        &envelope.event,
        RuntimeEvent::TurnStarted | RuntimeEvent::TurnCompleted
    ) && envelope.turn_id.is_none()
    {
        return Err(ConversationStoreError::InvalidReference(
            "turn lifecycle event has no turn identity".to_owned(),
        ));
    }
    let payload_attempt = match &envelope.event {
        RuntimeEvent::AttemptStarted { attempt_id }
        | RuntimeEvent::AttemptCompleted { attempt_id, .. }
        | RuntimeEvent::AttemptCancelled { attempt_id, .. }
        | RuntimeEvent::AttemptTimedOut { attempt_id }
        | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
        | RuntimeEvent::AttemptFailed { attempt_id, .. } => Some(attempt_id),
        _ => None,
    };
    if let (Some(payload_attempt), Some(envelope_attempt)) =
        (payload_attempt, envelope.attempt_id.as_ref())
        && payload_attempt != envelope_attempt
    {
        return Err(ConversationStoreError::InvalidReference(
            "event envelope attempt identity disagrees with its typed payload".to_owned(),
        ));
    }
    Ok(())
}

/// An attempt identity starts exactly once in durable authority (Issue #12,
/// M9a).
///
/// `AttemptStarted` is the first event of every attempt, so its lifecycle key
/// cannot already exist. Rejecting a second start makes accidental identity
/// reuse — most importantly a process-local attempt ordinal reset after a
/// restart — a typed durable failure instead of two logical attempts sharing
/// one durable identity.
pub(super) fn validate_attempt_start_uniqueness(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::AttemptStarted { attempt_id } = &envelope.event else {
        return Ok(());
    };
    let key = format!("attempt:{attempt_id}");
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
            [&key],
            |row| row.get(0),
        )
        .map_err(|error| {
            sqlite_error(&error, format!("attempt start uniqueness probe: {error}"))
        })?;
    if exists {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "attempt {attempt_id} already entered durable authority; an attempt identity starts exactly once"
        )));
    }
    Ok(())
}

pub(super) fn runtime_event_dependency_name(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
        RuntimeEvent::ToolExecutionProgress { .. } => "ToolExecutionProgress",
        RuntimeEvent::ToolExecutionDeadlineFired { .. } => "ToolExecutionDeadlineFired",
        RuntimeEvent::ToolExecutionCancellationRequested { .. } => {
            "ToolExecutionCancellationRequested"
        }
        RuntimeEvent::ToolExecutionSettlementObserved { .. } => "ToolExecutionSettlementObserved",
        RuntimeEvent::ToolExecutionSettlementControlFailed { .. } => {
            "ToolExecutionSettlementControlFailed"
        }
        RuntimeEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
        RuntimeEvent::ToolExecutionFailed { .. } => "ToolExecutionFailed",
        RuntimeEvent::ToolMessageCommitted { .. } => "ToolMessageCommitted",
        RuntimeEvent::BackgroundExecutionCommitted { .. } => "background side-effect authorization",
        RuntimeEvent::SubagentOwnershipCommitted { .. } => "subagent side-effect authorization",
        _ => unreachable!("dependency name requested for a non-tool event"),
    }
}

pub(super) fn find_event_by_id(
    transaction: &Transaction<'_>,
    event_id: &EventId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let Some(json) = transaction
        .query_row(
            "SELECT event_json FROM events WHERE event_id=?1",
            [event_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("event identity probe: {error}")))?
    else {
        return Ok(None);
    };
    Ok(Some(decode(&json, "event identity")?))
}

/// Compares a retried event with its durable copy. Event Journal sequence is
/// store-allocated and therefore intentionally excluded; every other field
/// is part of the producer's frozen terminal transition identity.
pub(super) fn same_event_ignoring_sequence(
    existing: &RuntimeEventEnvelope,
    requested: &RuntimeEventEnvelope,
) -> bool {
    existing.schema_version == requested.schema_version
        && existing.event_id == requested.event_id
        && existing.conversation_id == requested.conversation_id
        && existing.attempt_id == requested.attempt_id
        && existing.turn_id == requested.turn_id
        && existing.timestamp == requested.timestamp
        && existing.event == requested.event
}

pub(super) fn find_event_at_sequence(
    transaction: &Transaction<'_>,
    sequence: u64,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let sequence = seq_to_i64(sequence)?;
    let Some(json) = transaction
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("event sequence probe: {error}")))?
    else {
        return Ok(None);
    };
    Ok(Some(decode(&json, "event sequence")?))
}

/// Loads the User notification a detached terminal fact references: the
/// pending row when it has not been adopted yet, otherwise the canonical
/// Ledger row.
pub(super) fn load_user_notification_tx(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
    domain: &str,
) -> Result<UserMessageBlock, ConversationStoreError> {
    let pending_json: Option<String> = transaction
        .query_row(
            "SELECT message_json FROM pending_inbound WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| {
            sqlite_error(&error, format!("{domain} notification reference: {error}"))
        })?;
    let message = if let Some(json) = pending_json {
        MessageBlock::User(decode::<UserMessageBlock>(
            &json,
            "detached pending notification",
        )?)
    } else {
        load_message_tx(transaction, message_id)?
    };
    let MessageBlock::User(notification) = message else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "{domain} terminal fact references non-User notification {message_id}"
        )));
    };
    if notification.id != *message_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "{domain} terminal fact references {message_id}, but the stored notification is {}",
            notification.id
        )));
    }
    Ok(notification)
}

/// Whether an event has a transcript-visible or otherwise compound durable
/// owner that must return through a purpose-specific transition.
pub(super) fn requires_specialized_transition(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::Goal {
            fact: crate::goal::GoalFact::Written { .. }
                | crate::goal::GoalFact::RoundAdmitted { .. }
        } | RuntimeEvent::InboundTurnAdopted { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::AuxiliaryCallStarted { .. }
            | RuntimeEvent::AuxiliaryCallSettled { .. }
            | RuntimeEvent::AuxiliaryRetryScheduled { .. }
            | RuntimeEvent::ContextContributionEmitted { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
            | RuntimeEvent::InteractionRequested { .. }
            | RuntimeEvent::InteractionSettled { .. }
    )
}

pub(super) fn validate_canonical_event_for_message(
    message: &MessageBlock,
    event: &RuntimeEvent,
) -> Result<(), ConversationStoreError> {
    let message_id = crate::conversation::message_id_of(message);
    match (message, event) {
        (
            MessageBlock::Assistant(_),
            RuntimeEvent::AssistantMessageCommitted {
                message_id: event_id,
            },
        ) if event_id == &message_id => Ok(()),
        (
            MessageBlock::Tool(tool),
            RuntimeEvent::ToolMessageCommitted {
                message_id: event_id,
                tool_call_id,
            },
        ) if event_id == &message_id && tool_call_id == &tool.tool_call_id => Ok(()),
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "canonical event does not identify the exact committed message {message_id}"
        ))),
    }
}

#[allow(clippy::too_many_lines)] // Closed mapping from durable facts to exact terminal domains.
pub(super) fn lifecycle_keys(event: &RuntimeEventEnvelope) -> Vec<(String, bool)> {
    match &event.event {
        RuntimeEvent::AuxiliaryCallStarted { call_id, .. } => {
            return vec![(format!("auxiliary-call:{call_id}"), false)];
        }
        RuntimeEvent::AuxiliaryCallSettled { call_id, .. } => {
            return vec![(format!("auxiliary-call:{call_id}"), true)];
        }
        RuntimeEvent::ModelRequestStarted {
            owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
            request_id,
            ..
        } => {
            return vec![
                (format!("auxiliary-call:{call_id}:request:{ordinal}"), false),
                (format!("auxiliary-request:{request_id}"), false),
            ];
        }
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
            request_id,
            ..
        }
        | RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
            request_id,
            ..
        } => {
            return vec![
                (format!("auxiliary-call:{call_id}:request:{ordinal}"), true),
                (format!("auxiliary-request:{request_id}"), true),
            ];
        }
        RuntimeEvent::CompactionStarted { operation_id } => {
            return vec![(format!("compaction:{operation_id}"), false)];
        }
        RuntimeEvent::CompactionCompleted { operation_id, .. }
        | RuntimeEvent::CompactionFailed { operation_id, .. } => {
            return vec![(format!("compaction:{operation_id}"), true)];
        }
        _ => {}
    }
    // The detached-execution lifecycle is opened by the ownership commit and
    // closed exactly once by the terminal publication. Recording the open
    // state durably is what lets a restart tell "owned and unsettled" from
    // "never existed", and what makes a second terminal publication — live
    // or recovery-generated — a typed `TerminalViolation` rather than a
    // duplicate model-visible notification.
    if let RuntimeEvent::BackgroundExecutionCommitted { execution_id, .. } = &event.event {
        return vec![(format!("background:{execution_id}"), false)];
    }
    if let RuntimeEvent::BackgroundTerminalPublished { execution_id, .. } = &event.event {
        return vec![(format!("background:{execution_id}"), true)];
    }
    // The subagent lifecycle is the same shape: opened by the ownership
    // commit (before the child may begin any semantic work) and closed
    // exactly once by the terminal publication, so a restart can tell an
    // owned-but-unsettled child from one that never existed, and a second
    // terminal publication is a typed `TerminalViolation`.
    if let RuntimeEvent::AgentActivationAdmission {
        activation_id,
        phase,
        ..
    } = &event.event
    {
        return vec![(
            format!("agent-admission:{activation_id}"),
            matches!(
                phase,
                crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven: true
                }
            ),
        )];
    }
    if let RuntimeEvent::SubagentOwnershipCommitted {
        subagent_id,
        admitted_authority,
        ownership,
        ..
    } = &event.event
    {
        let mut keys = vec![(format!("subagent:{subagent_id}"), false)];
        if admitted_authority.is_none()
            && *ownership == crate::events::types::SubagentOwnershipKind::Normal
        {
            keys.push((format!("agent-admission:{subagent_id}"), true));
        }
        return keys;
    }
    if let RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } = &event.event {
        return vec![(format!("subagent:{subagent_id}"), true)];
    }
    if let RuntimeEvent::SubagentTerminalSettled { subagent_id, .. } = &event.event {
        return vec![(format!("subagent:{subagent_id}"), true)];
    }
    // The interaction lifecycle (Issue #109) is the same shape: opened by the
    // requested fact — which commits before the prompt reaches a client — and
    // closed exactly once by the settled fact. It is deliberately its own
    // lifecycle domain and never touches the enclosing `attempt:`/`turn:`
    // keys, so an interaction audit fact can neither be blocked by nor block
    // the attempt's own terminal transition. An interaction that stays open
    // across a restart is durable evidence of an unanswered prompt; it is
    // never an instruction to recreate a waiter.
    if let RuntimeEvent::InteractionRequested {
        interaction_id,
        subject,
    } = &event.event
    {
        let mut keys = vec![(format!("interaction:{interaction_id}"), false)];
        if let InteractionSubject::Approval {
            invocation_id: id @ crate::tools::types::ToolInvocationId::Workflow { .. },
            ..
        } = subject
        {
            keys.push((native_approval_key(id), true));
        }
        return keys;
    }
    if let RuntimeEvent::InteractionSettled { interaction_id, .. } = &event.event {
        return vec![(format!("interaction:{interaction_id}"), true)];
    }
    let attempt = event.attempt_id.as_ref().or(match &event.event {
        RuntimeEvent::AttemptStarted { attempt_id }
        | RuntimeEvent::AttemptCompleted { attempt_id, .. }
        | RuntimeEvent::AttemptCancelled { attempt_id, .. }
        | RuntimeEvent::AttemptTimedOut { attempt_id }
        | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
        | RuntimeEvent::AttemptFailed { attempt_id, .. } => Some(attempt_id),
        _ => None,
    });
    let mut keys = Vec::new();
    let attempt_terminal = is_terminal(&event.event);
    if let Some(attempt) = attempt {
        keys.push((format!("attempt:{attempt}"), attempt_terminal));
    }
    if !attempt_terminal && let Some(turn) = event.turn_id.as_ref() {
        let owner = event
            .attempt_id
            .as_ref()
            .map_or_else(|| event.conversation_id.to_string(), ToString::to_string);
        keys.push((
            format!("turn:{owner}:{turn}"),
            matches!(&event.event, RuntimeEvent::TurnCompleted),
        ));
    }
    keys
}

pub(super) fn is_terminal(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::InteractionSettled { .. }
    )
}

impl SqliteConversationStore {
    /// Bound a management traversal while unrelated execution may append events.
    pub(crate) fn event_high_watermark(&self) -> Result<u64, ConversationStoreError> {
        self.lock()?
            .query_row("SELECT coalesce(max(sequence),0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(|e| storage(format!("event high watermark: {e}")))
    }
    pub(super) fn ownership_transition(
        &self,
        event: &RuntimeEvent,
    ) -> Result<Option<crate::runtime::local_storage::OwnershipMutation>, ConversationStoreError>
    {
        if matches!(
            event,
            RuntimeEvent::AgentActivationAdmission { .. }
                | RuntimeEvent::SubagentOwnershipCommitted { .. }
                | RuntimeEvent::SubagentTerminalPublished { .. }
                | RuntimeEvent::SubagentPhysicalSettlementProven { .. }
                | RuntimeEvent::SubagentTerminalSettled { .. }
        ) {
            self.lifecycle
                .as_ref()
                .map(|access| {
                    access
                        .ownership_mutation()
                        .map_err(|e| storage(e.to_string()))
                })
                .transpose()
        } else {
            Ok(None)
        }
    }
    pub(super) fn append_event_internal(
        &self,
        event: RuntimeEventEnvelope,
        authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        let _ownership = self.ownership_transition(&event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("event transaction: {error}")))?;
        // Recovery may retry after losing a commit acknowledgement. This
        // monotonic resource proof has one canonical event identity; return the
        // already committed receipt (including its original timestamp) rather
        // than stranding an obligation behind the unique event index.
        if matches!(
            event.event,
            RuntimeEvent::SubagentPhysicalSettlementProven { .. }
                | RuntimeEvent::AgentActivationAdmission {
                    phase: crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                        physical_settlement_proven: true
                    },
                    ..
                }
        ) && let Some(existing) = find_event_by_id(&transaction, &event.event_id)?
        {
            let mut retry = event;
            retry.timestamp = existing.timestamp;
            if !same_event_ignoring_sequence(&existing, &retry) {
                return Err(ConversationStoreError::InvalidReference(
                    "physical settlement proof retry conflicts with its committed identity".into(),
                ));
            }
            return Ok(existing);
        }
        #[cfg(test)]
        if matches!(
            &event.event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptFailed { .. }
        ) {
            self.terminal_event_attempts.fetch_add(1, Ordering::SeqCst);
            if Self::consume(&self.fail_terminal_event_remaining) {
                return Err(storage("fault injected: terminal event commit"));
            }
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: event commit"));
        }
        persist_agent_authority_tx(&transaction, &event.event, authority)?;
        process_death::reach_event("before:event", &event.event);
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("event commit: {error}")))?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok(persisted.event)
    }
}
