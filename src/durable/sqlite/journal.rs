//! The Event Journal domain: validated execution-fact persistence.

use self::reference::validate_event_reference_with_requests;
use super::{append_facts, attempt_start, event_insert, native_request};
use crate::durable::sqlite::statements::FixedStatements;
#[cfg(test)]
mod identity_review_tests;
#[cfg(test)]
mod identity_tests;
#[cfg(all(test, target_os = "linux"))]
mod identity_write_tests;
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

fn decode_journal_row(
    row: &rusqlite::Row<'_>,
    conversation_id: &ConversationId,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
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
    let sequence = u64::try_from(sequence).map_err(|_| storage("negative event sequence"))?;
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
        || event.conversation_id != *conversation_id
        || event.attempt_id.as_ref().map(ToString::to_string) != stored_attempt_id
        || event.turn_id.as_ref().map(ToString::to_string) != stored_turn_id
    {
        return Err(ConversationStoreError::InvalidReference(
            "Event Journal row metadata disagrees with its envelope".to_owned(),
        ));
    }
    Ok(event)
}

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
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(&error, "read private Agent authority")
            })?;
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
    fn commit_attempt_first_turn(
        &self,
        attempt: RuntimeEventEnvelope,
        turn: RuntimeEventEnvelope,
    ) -> Result<[RuntimeEventEnvelope; 2], ConversationStoreError> {
        attempt_start::commit(self, attempt, turn)
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
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("Workflow child terminal transaction: {error}"),
                )
            })?;
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
        let persisted = persist_event_tx(
            &self.conn.event_guard,
            &transaction,
            &self.conversation_id,
            event,
        )?;
        transaction.commit().map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("Workflow child terminal commit: {error}"),
            )
        })?;
        process_death::reach_event("after:workflow_child_terminal", &persisted.event.event);
        Ok(persisted.event)
    }
    #[allow(clippy::too_many_lines)] // One atomic workflow terminal boundary validates and publishes both native facts.
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
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("Workflow terminal transaction: {error}"),
                )
            })?;
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
                transaction.commit().map_err(|error| {
                    crate::durable::sqlite::sqlite_error(
                        &error,
                        format!("Workflow terminal retry commit: {error}"),
                    )
                })?;
                Ok((existing_terminal, existing_output))
            }
            (Some(_), None) | (None, Some(_)) => Err(ConversationStoreError::TerminalViolation(
                "Workflow terminal facts are only valid as an atomic pair".to_owned(),
            )),
            (None, None) => {
                let mut persisted_terminal = persist_event_tx_deferred(
                    &self.conn.event_guard,
                    &transaction,
                    &self.conversation_id,
                    terminal,
                )?;
                let mut persisted_output = persist_event_tx_deferred(
                    &self.conn.event_guard,
                    &transaction,
                    &self.conversation_id,
                    output,
                )?;
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
                transaction.commit().map_err(|error| {
                    crate::durable::sqlite::sqlite_error(
                        &error,
                        format!("Workflow terminal commit: {error}"),
                    )
                })?;
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
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("interaction audit transaction: {error}"),
                )
            })?;
        let persisted = persist_event_tx(
            &self.conn.event_guard,
            &transaction,
            &self.conversation_id,
            event,
        )?;
        let transcript_cursor = persisted.transcript_cursor.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "interaction audit did not receive a transcript cursor".to_owned(),
            )
        })?;
        process_death::reach_event("before:interaction", &persisted.event.event);
        transaction.commit().map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("interaction audit commit: {error}"),
            )
        })?;
        process_death::reach_event("after:interaction", &persisted.event.event);
        Ok((persisted.event, transcript_cursor))
    }
    fn append_configuration_change(
        &self,
        event: RuntimeEventEnvelope,
        instructions: &str,
        catalog: &str,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        if !matches!(event.event, RuntimeEvent::ConfigurationApplied { .. })
            || event.attempt_id.is_some()
            || event.turn_id.is_some()
        {
            return Err(ConversationStoreError::InvalidReference(
                "the configuration change transition accepts only Session-level change facts"
                    .to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("configuration change transaction: {error}"),
                )
            })?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: configuration change commit"));
        }
        let persisted = persist_event_tx(
            &self.conn.event_guard,
            &transaction,
            &self.conversation_id,
            event,
        )?;
        let transcript_cursor = persisted.transcript_cursor.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "configuration change did not receive a transcript cursor".to_owned(),
            )
        })?;
        for (kind, text) in [
            (
                crate::observation::content::ContentSource::CONFIGURATION_INSTRUCTIONS,
                instructions,
            ),
            (
                crate::observation::content::ContentSource::CONFIGURATION_CATALOG,
                catalog,
            ),
        ] {
            crate::durable::sqlite::content::index_utf8_field(
                &transaction,
                kind,
                &persisted.event.sequence.to_string(),
                0,
                text,
            )?;
        }
        transaction.commit().map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("configuration change commit: {error}"),
            )
        })?;
        Ok((persisted.event, transcript_cursor))
    }
    fn observe_journal(
        &self,
        observer: Arc<dyn crate::durable::presentation::JournalObserver>,
    ) -> Result<u64, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
        let through = journal_prefix(&transaction)?;
        // Committed history becomes this connection owner's baseline. No old
        // direct claims survive, and no durable history is copied or updated.
        observation_readiness::recover_journal(&transaction)?;
        observation_readiness::recover_transcript(&transaction)?;
        transaction
            .commit()
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
        *self.journal_observer.lock().expect("Journal observer lock") = Some((observer, through));
        Ok(through)
    }
    fn presentation_frontier(&self) -> Result<u64, ConversationStoreError> {
        let value: i64 = self
            .lock()?
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(&error, "presentation frontier failed")
            })?;
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
            let mut statement = connection.prepare(&sql).map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    "presentation query preparation failed",
                )
            })?;
            let mut rows = statement
                .query(rusqlite::params_from_iter(params))
                .map_err(|error| {
                    crate::durable::sqlite::sqlite_error(&error, "presentation query failed")
                })?;
            while let Some(row) = rows.next().map_err(|error| {
                crate::durable::sqlite::sqlite_error(&error, "presentation row failed")
            })? {
                #[cfg(test)]
                observation_statistics::original_read();
                let event = if matches!(query.scope, crate::durable::presentation::FactScope::All) {
                    decode_journal_row(row, &self.conversation_id)?
                } else {
                    let json: String =
                        row.get(0).map_err(|_| storage("presentation row failed"))?;
                    decode(&json, "presentation fact")?
                };
                events.push(event);
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
    fn recovery_frontier(
        &self,
    ) -> Result<crate::runtime::recovery::RecoveryFrontier, ConversationStoreError> {
        let connection = self.lock()?;
        super::recovery_frontier::authenticated(&connection)
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
            .prepare_cached(
                "SELECT sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json
                 FROM events WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
            )
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("event page: {error}")))?;
        let mut rows = statement.query(params![after, limit]).map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("event page query: {error}"))
        })?;
        let mut events = Vec::new();
        let mut next = None;
        while let Some(row) = rows.next().map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("event page row: {error}"))
        })? {
            let event = decode_journal_row(row, &self.conversation_id)?;
            let sequence = event.sequence;
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
    pub(in crate::durable::sqlite) facts: append_facts::Facts,
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
                .map_err(|error| {
                    crate::durable::sqlite::sqlite_error(&error, "commit private Agent authority")
                })?;
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
    guard: &event_insert::Guard,
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_internal(guard, transaction, conversation_id, event, true, None)
}

pub(super) fn persist_event_tx_deferred(
    guard: &event_insert::Guard,
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_internal(guard, transaction, conversation_id, event, false, None)
}

// Fresh-start-only proof, borrowed from the exact transaction that has just
// validated its stored Snapshot and reconstructed every immutable dependency.
// Generic Journal insertions always perform their own reference validation.
pub(super) struct ValidatedRequestStart<'a, 'connection> {
    pub(super) transaction: &'a Transaction<'connection>,
    pub(super) snapshot: &'a RequestSnapshot,
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One atomic event with fixed request/publication and preceding-member evidence.
pub(in crate::durable::sqlite) fn persist_event_tx_with_requests(
    guard: &event_insert::Guard,
    requests: &mut native_request::Validation<'_, '_>,
    conversation_id: &ConversationId,
    mut event: RuntimeEventEnvelope,
    prepare_observation: bool,
    validated_start: Option<&ValidatedRequestStart<'_, '_>>,
    publication: Option<&event_insert::CanonicalPublication<'_>>,
    previous_members: Option<&observation_members::Captured>,
) -> Result<PersistedEvent, ConversationStoreError> {
    let transaction = requests.transaction();
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
        validate_event_reference_with_requests(requests, &event)?;
    }
    // Fresh Snapshot admission already owns the exact first-start proof.
    // Other callers must still reject duplicate starts in either owner domain.
    if validated_start.is_none()
        && let RuntimeEvent::ModelRequestStarted { request_id, .. } = &event.event
        && find_request_start_event(transaction, request_id)?.is_some()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} already has a request-start fact"
        )));
    }
    let current: i64 = transaction
        .prepare_cached("SELECT next_event_sequence FROM rustx_store WHERE id=1")
        .and_then(|mut statement| statement.query_row([], |row| row.get(0)))
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("read event sequence: {error}"))
        })?;
    let sequence = current
        .checked_add(1)
        .ok_or_else(|| storage("event sequence exhausted"))?;
    if event.event_id.as_str().is_empty() {
        event.event_id = EventId::new(format!("{conversation_id}-event-{sequence}"));
    }
    event.sequence = u64::try_from(sequence).map_err(|_| storage("event sequence exhausted"))?;
    let mut lifecycles = Vec::new();
    for (key, terminal) in lifecycle_keys(&event) {
        #[cfg(test)]
        append_facts::lifecycle_probe();
        let existing: Option<Option<u64>> = transaction
            .query_row_cached(
                "SELECT terminal_sequence FROM lifecycle_state WHERE lifecycle_key=?1",
                [&key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(&error, format!("lifecycle probe: {error}"))
            })?;
        if existing.as_ref().is_some_and(Option::is_some) {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "lifecycle {key} is already terminal"
            )));
        }
        lifecycles.push((key, terminal, existing.is_some()));
    }
    // All terminal guards are checked before the first write. Until a write
    // occurs, these same-transaction probes also prove row existence. Once a
    // lifecycle write may have run a trigger, later keys are queried again.
    let mut lifecycle_written = false;
    for (key, terminal, known_exists) in lifecycles {
        if terminal {
            transaction
                .execute_cached(
                    "INSERT INTO lifecycle_state(lifecycle_key,terminal_sequence) VALUES(?1,?2) ON CONFLICT(lifecycle_key) DO UPDATE SET terminal_sequence=excluded.terminal_sequence",
                    params![key, event.sequence],
                )
                .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("record terminal lifecycle: {error}")))?;
            lifecycle_written = true;
        } else {
            let exists = if lifecycle_written {
                #[cfg(test)]
                append_facts::lifecycle_probe();
                transaction
                    .query_row_cached(
                        "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                        [&key],
                        |row| row.get(0),
                    )
                    .map_err(|error| {
                        crate::durable::sqlite::sqlite_error(
                            &error,
                            format!("lifecycle existence probe: {error}"),
                        )
                    })?
            } else {
                known_exists
            };
            if !exists {
                transaction
                    .execute_cached(
                        "INSERT INTO lifecycle_state(lifecycle_key,terminal_sequence) VALUES(?1,NULL)",
                        [&key],
                    )
                    .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("record lifecycle: {error}")))?;
                lifecycle_written = true;
            }
        }
    }
    if find_event_sequence(transaction, event.event_id.as_str())?.is_some() {
        return Err(storage(
            "insert runtime event: UNIQUE constraint failed: events.event_id",
        ));
    }
    let json = encode(&event, "runtime event")?;
    let persisted = if let Some(inserted) = event_insert::prepared(
        guard,
        transaction,
        conversation_id,
        &mut event,
        &json,
        prepare_observation,
        validated_start.map(|proof| proof.snapshot),
        publication,
        previous_members,
    )? {
        PersistedEvent {
            event,
            transcript_cursor: inserted.transcript_cursor,
            facts: inserted.facts,
        }
    } else {
        persist_event_isolated(
            transaction,
            conversation_id,
            event,
            &json,
            prepare_observation,
        )?
    };
    // The recovery frontier covers this fact in the same transaction.
    super::recovery_frontier::advance(transaction, conversation_id, &persisted.event)?;
    Ok(persisted)
}

/// Storage failure isolation uses the same current schema and domain builders.
/// Lifecycle writes have already happened exactly once in the enclosing native transaction.
#[allow(clippy::too_many_lines)]
fn persist_event_isolated(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    mut event: RuntimeEventEnvelope,
    json: &str,
    prepare_observation: bool,
) -> Result<PersistedEvent, ConversationStoreError> {
    let sequence = seq_to_i64(event.sequence)?;
    transaction
        .prepare_cached("INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp,observation_trace_required,observation_statistics_required,observation_kind,observation_members_required,observation_turn_required,observation_trace_fact_required,observation_execution_required,observation_unavailable) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1)")
        .and_then(|mut statement| statement.execute(params![
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
                Some(observation_event::native_kind(&event.event)),
                observation_members::requires_members(&event),
                observation_turn::required(&event),
                observation_trace::required(&event),
                observation_execution::required(&event),
            ]))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("insert runtime event: {error}")))?;
    auxiliary::index_event_tx(transaction, &event)?;
    // Missing auxiliary evidence is the fail-closed initial state. Independent
    // savepoints preserve own lifecycle facts if a different layout fails.
    // Readiness has its own recoverable missing-evidence flag. A newly
    // installed Runtime may repair it without clearing unrelated layout holes.
    let _ = optional_display_layout(transaction, || {
        observation_readiness::insert(transaction, &event)
    })?;
    let statistics_required = observation_statistics::is_trace_anchor(&event.event)
        || observation_statistics::requires_statistics(&event)
        || observation_members::requires_members(&event);
    let aggregate_facts = statistics_required
        .then(|| {
            optional_display_layout(transaction, || {
                append_facts::aggregates(transaction, &event)
            })
        })
        .transpose()?
        .flatten();
    let statistics_available = !statistics_required || aggregate_facts.is_some();
    if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
        transaction.execute_cached("UPDATE canonical_tool_calls SET attempt_id=?1,commit_sequence=?2 WHERE assistant_message_id=?3",params![event.attempt_id.as_ref().map(crate::runtime::identity::AttemptId::as_str),sequence,message_id.as_str()]).map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    }

    // Caller-supplied ephemeral evidence is never trusted. Native canonical
    // ownership is resolved inside this exact append transaction.
    let assistant_available = if let RuntimeEvent::AssistantMessageCommitted { message_id } =
        &event.event
    {
        optional_display_layout(transaction, || {
            transaction.execute_cached("UPDATE events SET observation_assistant_ledger=(SELECT position FROM message_ledger WHERE message_id=?2) WHERE sequence=?1",params![sequence,message_id.as_str()]).map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
            Ok(())
        })?.is_some()
    } else {
        true
    };
    event.tool_occurrence = observation::tool_occurrence_receipt(transaction, &event)?;
    let tool_available = if let Some(occurrence) = &event.tool_occurrence {
        optional_display_layout(transaction, || {
        let phase = match &event.event {
            RuntimeEvent::ToolExecutionStarted { .. } => Some(1),
            RuntimeEvent::ToolExecutionCompleted { .. }
            | RuntimeEvent::ToolExecutionFailed { .. } => Some(2),
            _ => None,
        };
        transaction.execute("UPDATE events SET observation_tool_ledger=?1,observation_tool_block=?2,observation_tool_phase=?4 WHERE sequence=?3",params![occurrence.ledger_position,occurrence.block_index.get(),sequence,phase]).map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
        Ok(())
        })?.is_some()
    } else {
        true
    };
    let execution_required = observation_execution::required(&event);
    let execution_facts = execution_required
        .then(|| {
            optional_display_layout(transaction, || {
                observation_execution::event(transaction, &event)
            })
        })
        .transpose()?
        .flatten()
        .flatten();
    let execution_available = !execution_required || execution_facts.is_some();
    let state_required = matches!(
        event.event,
        RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::CompactionFailed { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::Goal {
                fact: crate::goal::GoalFact::Written { .. }
                    | crate::goal::GoalFact::RoundAdmitted { .. },
            }
    );
    let state_available = !state_required
        || optional_display_layout(transaction, || {
            observation_state::event(transaction, &event)
        })?
        .is_some();
    let request_required = matches!(
        event.event,
        RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::CompactionCompleted { .. }
    );
    let request_available = !request_required
        || optional_display_layout(transaction, || {
            observation_occupancy::event(transaction, &event)?;
            observation_request::terminal(transaction, &event)
        })?
        .is_some();
    let witness_facts = observation_event::index_optional(
        transaction,
        &event,
        json.as_bytes(),
        execution_facts.as_ref(),
    )?;
    let witness_available = witness_facts.available;
    let trace_available = optional_display_layout(transaction, || {
        observation_trace::index_event(transaction, &event, execution_facts.as_ref())
    })?
    .is_some();
    transaction
        .prepare_cached("UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1")
        .and_then(|mut statement| statement.execute([sequence]))
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("update event sequence: {error}"))
        })?;
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
    } else if matches!(event.event, RuntimeEvent::ConfigurationApplied { .. }) {
        Some(append_transcript_reference(
            transaction,
            "configuration_event",
            event.event_id.as_str(),
        )?)
    } else {
        None
    };
    let transcript_available = enrich_event_transcript(transaction, &event, transcript_cursor)?;
    // Unowned adoptions still capture the prompt; the required flag only
    // describes the separate attempt-owned turn header.
    let turn_required = observation_turn::required(&event)
        || matches!(event.event, RuntimeEvent::InboundTurnAdopted { .. });
    let turn_available = !turn_required
        || optional_display_layout(transaction, || {
            observation_turn::event(transaction, &event, aggregate_facts.as_ref())
        })?
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
            .execute_cached(
                super::request::COMPLETE_REQUEST_SQL,
                params![sequence, request_id.as_str()],
            )
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("record provider outcome: {error}"),
                )
            })?;
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
        && transcript_available
        && turn_available
    {
        // The marker clear is auxiliary too: its failure leaves the original
        // native J/C/T committed and this row explicitly unavailable.
        let _ = optional_display_layout(transaction, || {
            transaction
                .prepare_cached("UPDATE events SET observation_unavailable=0 WHERE sequence=?1")
                .and_then(|mut statement| statement.execute([sequence]))
                .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
            Ok(())
        })?;
    }
    event.observation = prepare_observation
        .then(|| {
            observation_receipt::single_prepared(
                transaction,
                conversation_id,
                &event,
                witness_facts.witness.as_ref(),
                execution_facts.as_ref(),
            )
        })
        .flatten();
    Ok(PersistedEvent {
        event,
        transcript_cursor,
        facts: append_facts::Facts {
            request_trace_prepared: false,
            canonical_publication_prepared: false,
            witness: witness_facts.witness,
            execution: execution_facts,
            members: None,
        },
    })
}

#[allow(clippy::too_many_lines)] // One atomic event, lifecycle and private admission transaction.
pub(super) fn persist_event_tx_internal(
    guard: &event_insert::Guard,
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
    prepare_observation: bool,
    validated_start: Option<&ValidatedRequestStart<'_, '_>>,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_with_requests(
        guard,
        &mut native_request::Validation::new(transaction),
        conversation_id,
        event,
        prepare_observation,
        validated_start,
        None,
        None,
    )
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
    let kind = match event.event {
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. } => {
            "interaction_event"
        }
        RuntimeEvent::ConfigurationApplied { .. } => "configuration_event",
        _ => "attempt_terminal",
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
        .query_row_cached(
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

// The first seek is only a candidate: exact scalar identity is always checked.
const EVENT_SEQUENCE_SQL: &str = "SELECT min(sequence),count(*) FROM (
    SELECT sequence FROM events WHERE sequence=?2 AND event_id=?1
    UNION ALL SELECT sequence FROM events INDEXED BY events_alias_idx
    WHERE event_id=?1 AND event_id != conversation_id || '-event-' || sequence AND sequence!=?2)";

pub(super) fn find_event_sequence(
    connection: &rusqlite::Connection,
    event_id: &str,
) -> Result<Option<u64>, ConversationStoreError> {
    if event_id.is_empty() {
        return Ok(None);
    }
    let candidate = event_id
        .rsplit_once("-event-")
        .and_then(|(_, suffix)| suffix.parse::<i64>().ok())
        .filter(|sequence| *sequence > 0)
        .unwrap_or(0);
    let (sequence, matches): (Option<u64>, u8) = connection
        .query_row_cached(EVENT_SEQUENCE_SQL, params![event_id, candidate], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|error| sqlite_error(&error, format!("event identity probe: {error}")))?;
    if matches > 1 {
        return Err(storage(
            "event identity has conflicting sequence and alias rows",
        ));
    }
    Ok(sequence)
}

pub(super) fn find_event_by_id(
    connection: &rusqlite::Connection,
    event_id: &EventId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    find_event_json_by_id(connection, event_id.as_str(), "event identity probe")?
        .map(|json| decode(&json, "event identity"))
        .transpose()
}

pub(super) fn find_event_json_by_id(
    connection: &rusqlite::Connection,
    event_id: &str,
    lookup: &str,
) -> Result<Option<String>, ConversationStoreError> {
    let Some(sequence) = find_event_sequence(connection, event_id)? else {
        return Ok(None);
    };
    connection
        .query_row_cached(
            "SELECT event_json FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("{lookup}: {error}")))
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
        .query_row_cached(
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
            | RuntimeEvent::ConfigurationApplied { .. }
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
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(&error, format!("event transaction: {error}"))
            })?;
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
        let persisted = persist_event_tx(
            &self.conn.event_guard,
            &transaction,
            &self.conversation_id,
            event,
        )?;
        transaction.commit().map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("event commit: {error}"))
        })?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok(persisted.event)
    }
}
