//! One native row, with independently prepared optional domains. A rejected
//! compound INSERT rolls back all preparation before native-first isolation.
use super::{
    Connection, ConversationId, ConversationStoreError, MessageBlock, OptionalExtension,
    PublicationFrame, PublicationSettlement, PublicationStreamId, RequestSnapshot, RuntimeEvent,
    RuntimeEventEnvelope, Transaction, TranscriptCursor, append_facts, content, observation,
    observation_event, observation_execution, observation_members, observation_occupancy,
    observation_readiness, observation_receipt, observation_request, observation_state,
    observation_statistics, observation_trace, observation_turn, optional_display_layout, params,
};
use crate::durable::sqlite::canonical::ledger::append_transcript_reference;
use crate::durable::sqlite::statements::FixedStatements;

/// Only the schema seen at connection installation may bypass a schema scan.
/// Never seat a later cookie: a savepoint rollback followed by different DDL
/// can reuse that later version number. Main and TEMP are checked inside the
/// caller's pinned transaction, so external DDL cannot race the proof.
pub(super) struct Guard {
    installed: Option<(i64, i64)>,
    statistics: std::sync::Arc<observation_statistics::FoldCache>,
    execution: observation_execution::FoldCache,
}
impl Guard {
    #[cfg(test)]
    pub(super) fn uninstalled() -> Self {
        Self {
            installed: None,
            statistics: observation_statistics::FoldCache::uninstalled(),
            execution: observation_execution::FoldCache::default(),
        }
    }
    pub(super) fn new(
        connection: &Connection,
        statistics: std::sync::Arc<observation_statistics::FoldCache>,
    ) -> Self {
        let installed = connection
            .unchecked_transaction()
            .ok()
            .and_then(|transaction| {
                let installed = no_custom_triggers(&transaction)
                    .ok()
                    .filter(|clean| *clean)
                    .and_then(|_| schema_versions(&transaction).ok());
                transaction.commit().ok().and(installed)
            });
        if installed.is_none() {
            statistics.disable();
        }
        Self {
            installed,
            statistics,
            execution: observation_execution::FoldCache::default(),
        }
    }
    pub(super) fn can_prepare(
        &self,
        connection: &Connection,
    ) -> Result<bool, ConversationStoreError> {
        if let Some(installed) = self.installed {
            match schema_versions(connection) {
                Ok(current) if current == installed => return Ok(true),
                Err(
                    error @ (ConversationStoreError::OperationCancelled
                    | ConversationStoreError::TransactionInvalid(_)),
                ) => return Err(error),
                _ => {}
            }
        }
        self.statistics.disable();
        match no_custom_triggers(connection) {
            Ok(available) => Ok(available),
            Err(
                error @ (ConversationStoreError::OperationCancelled
                | ConversationStoreError::TransactionInvalid(_)),
            ) => Err(error),
            Err(_) => Ok(false),
        }
    }
    pub(super) fn publish_statistics(&self, connection: &Connection) {
        self.statistics.publish(connection);
    }
}
fn schema_versions(connection: &Connection) -> Result<(i64, i64), ConversationStoreError> {
    let read = |sql| {
        connection
            .query_row_cached(sql, [], |row| row.get(0))
            .map_err(|error| super::sqlite_error(&error, error.to_string()))
    };
    Ok((
        read("PRAGMA main.schema_version")?,
        read("PRAGMA temp.schema_version")?,
    ))
}

/// Triggers can observe or change intermediate native/derived rows. Preserve
/// those SQL programs' ordering, including TEMP triggers and dynamic DDL. The
/// exceptions are our exact TEMP cleanup and catalog invalidation programs:
/// neither source deletion nor catalog mutation occurs while
/// preparing or inserting an event.
fn no_custom_triggers(connection: &Connection) -> Result<bool, ConversationStoreError> {
    #[cfg(test)]
    GUARD_SCANS.with(|count| count.set(count.get() + 1));
    connection.query_row_cached(
        "SELECT NOT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND (sql IS NULL OR sql NOT IN(?3,?4,?5,?6))) AND NOT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE type='trigger' AND (sql IS NULL OR sql NOT IN(?1,?2)))",
        params![observation_readiness::EVENT_DELETION, observation_readiness::TRANSCRIPT_DELETION,
            observation_trace::CATALOG_INVALIDATIONS[0],observation_trace::CATALOG_INVALIDATIONS[1],observation_trace::CATALOG_INVALIDATIONS[2],observation_trace::CATALOG_INVALIDATIONS[3]], |row| row.get(0),
    ).map_err(|error|super::sqlite_error(&error,error.to_string()))
}

/// Only the already validated canonical publication transition supplies this
/// fixed input. It retains original raw bytes, never an interpreted substitute.
pub(super) struct CanonicalPublication<'a> {
    pub(super) stream: &'a PublicationStreamId,
    pub(super) message: &'a MessageBlock,
    pub(super) final_frames: &'a [PublicationFrame],
}

#[allow(clippy::struct_excessive_bools)] // Independent domain availability and two fixed native owner preparations.
struct Row {
    aggregates: Option<append_facts::Aggregates>,
    assistant_ledger: Option<u64>,
    execution: Option<observation_execution::Prepared>,
    occupancy: Option<Vec<u8>>,
    source: Option<observation_event::SourceCapture>,
    trace: Option<observation_trace::Prepared>,
    turn: Option<observation_turn::Prepared>,
    transcript: Option<TranscriptCursor>,
    readiness: bool,
    request_trace_prepared: bool,
    canonical_publication_prepared: bool,
    available: bool,
}

pub(super) struct Inserted {
    pub(super) transcript_cursor: Option<TranscriptCursor>,
    pub(super) facts: append_facts::Facts,
}

#[allow(clippy::too_many_arguments)] // Fixed native append plus its request/publication owner proof.
pub(super) fn prepared(
    guard: &Guard,
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    event: &mut RuntimeEventEnvelope,
    json: &str,
    prepare_observation: bool,
    snapshot: Option<&RequestSnapshot>,
    publication: Option<&CanonicalPublication<'_>>,
    previous_members: Option<&observation_members::Captured>,
) -> Result<Option<Inserted>, ConversationStoreError> {
    if transaction.is_autocommit() {
        return Err(ConversationStoreError::TransactionInvalid(
            "native prepared event lost its owning transaction".into(),
        ));
    }
    // Auxiliary lifecycle indexing and configuration transcript capture retain
    // main's native-first order. Agent turns keep the prepared single INSERT.
    if event.event.is_auxiliary_model_request()
        || matches!(
            event.event,
            RuntimeEvent::AuxiliaryCallStarted { .. }
                | RuntimeEvent::AuxiliaryCallSettled { .. }
                | RuntimeEvent::AuxiliaryRetryScheduled { .. }
                | RuntimeEvent::ConfigurationApplied { .. }
        )
    {
        return Ok(None);
    }
    if !guard.can_prepare(transaction)? {
        return Ok(None);
    }
    transaction
        .execute_cached("SAVEPOINT rustx_prepared_event", [])
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("prepare native event: {error}"))
        })?;
    let row = match prepare(
        guard,
        transaction,
        event,
        json,
        snapshot,
        publication,
        previous_members,
    ) {
        Ok(row) => row,
        Err(
            error @ (ConversationStoreError::OperationCancelled
            | ConversationStoreError::TransactionInvalid(_)),
        ) => return Err(error),
        Err(_) => {
            rollback(transaction)?;
            return Ok(None);
        }
    };
    if let Err(error) = insert(transaction, event, json, &row) {
        if matches!(
            error,
            ConversationStoreError::OperationCancelled
                | ConversationStoreError::TransactionInvalid(_)
        ) {
            return Err(error);
        }
        // The failed statement may have run constraints or auxiliary writes.
        // No effect from preparation is retained across domain isolation.
        rollback(transaction)?;
        #[cfg(test)]
        WORK.with(|work| {
            let (ok, isolated) = work.get();
            work.set((ok, isolated + 1));
        });
        return Ok(None);
    }
    super::auxiliary::index_event_tx(transaction, event)?;
    transaction
        .execute_cached("RELEASE rustx_prepared_event", [])
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("release native event preparation: {error}"),
            )
        })?;
    #[cfg(test)]
    WORK.with(|work| {
        let (ok, isolated) = work.get();
        work.set((ok + 1, isolated));
    });
    finish(
        guard,
        transaction,
        conversation,
        event,
        row,
        prepare_observation,
    )
    .map(Some)
}

fn rollback(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_cached("ROLLBACK TO rustx_prepared_event", [])
        .and_then(|_| connection.execute_cached("RELEASE rustx_prepared_event", []))
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("rollback native event preparation: {error}"),
            )
        })?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn prepare(
    guard: &Guard,
    transaction: &Transaction<'_>,
    event: &mut RuntimeEventEnvelope,
    json: &str,
    snapshot: Option<&RequestSnapshot>,
    publication: Option<&CanonicalPublication<'_>>,
    previous_members: Option<&observation_members::Captured>,
) -> Result<Row, ConversationStoreError> {
    if let Some(publication) = publication {
        content::settle_canonical(
            transaction,
            publication.stream,
            publication.message,
            publication.final_frames,
        )?;
        transaction
            .execute_cached(
                "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    publication.stream.as_str()
                ],
            )
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("settle canonical publication: {error}"),
                )
            })?;
    }
    let readiness = optional_display_layout(transaction, || {
        observation_readiness::prepare(transaction, event)
    })?
    .is_some();
    let aggregate_required = observation_statistics::is_trace_anchor(&event.event)
        || observation_statistics::requires_statistics(event)
        || observation_members::requires_members(event);
    let aggregates = aggregate_required
        // The builder only reads and encodes. Its stored columns are installed
        // by the final INSERT under the outer preparation savepoint.
        .then(|| {
            super::optional_display_value(append_facts::prepare_aggregates_cached(
                transaction,
                event,
                previous_members,
                Some(&guard.statistics),
            ))
        })
        .transpose()?
        .flatten();
    let mut available = !aggregate_required || aggregates.is_some();
    if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
        transaction.execute_cached("UPDATE canonical_tool_calls SET attempt_id=?1,commit_sequence=?2 WHERE assistant_message_id=?3",params![event.attempt_id.as_ref().map(crate::runtime::identity::AttemptId::as_str),event.sequence,message_id.as_str()]).map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    }
    let assistant_ledger =
        if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
            let captured = super::optional_display_value(
                transaction
                    .query_row_cached(
                        "SELECT position FROM message_ledger WHERE message_id=?1",
                        [message_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| super::sqlite_error(&error, error.to_string())),
            )?;
            available &= captured.is_some();
            captured.flatten()
        } else {
            None
        };
    event.tool_occurrence = observation::tool_occurrence_receipt(transaction, event)?;
    let execution_required = observation_execution::required(event);
    let execution = execution_required
        .then(|| {
            optional_display_layout(transaction, || {
                observation_execution::prepare_cached(
                    transaction,
                    event,
                    true,
                    Some(&guard.execution),
                )
            })
        })
        .transpose()?
        .flatten()
        .flatten();
    available &= !execution_required || execution.is_some();
    let state_required = matches!(
        event.event,
        RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::CompactionFailed { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::Goal {
                fact: crate::goal::GoalFact::Written { .. }
                    | crate::goal::GoalFact::RoundAdmitted { .. }
            }
    );
    available &= !state_required
        || optional_display_layout(transaction, || observation_state::event(transaction, event))?
            .is_some();
    let request_required = matches!(
        event.event,
        RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::CompactionCompleted { .. }
    );
    let occupancy = if request_required {
        let captured = optional_display_layout(transaction, || {
            let occupancy = observation_occupancy::prepare(transaction, event)?;
            observation_request::terminal(transaction, event)?;
            Ok(occupancy)
        })?;
        available &= captured.is_some();
        captured.flatten()
    } else {
        None
    };
    let transcript = match event.event {
        RuntimeEvent::AttemptCancelled { .. }
        | RuntimeEvent::AttemptFailed { .. }
        | RuntimeEvent::AttemptTimedOut { .. }
        | RuntimeEvent::AttemptLimitExceeded { .. } => Some(append_transcript_reference(
            transaction,
            "attempt_terminal",
            event.event_id.as_str(),
        )?),
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. } => {
            Some(append_transcript_reference(
                transaction,
                "interaction_event",
                event.event_id.as_str(),
            )?)
        }
        _ => None,
    };
    let pending = append_facts::PendingEvent {
        statistics: aggregates.as_ref().map(|value| &value.statistics),
        execution: execution.as_ref().map(|value| &value.capture),
        transcript_cursor: transcript,
        assistant_ledger,
    };
    let source = observation_event::prepare_pending(transaction, event, json.as_bytes(), &pending)?;
    available &= source.as_ref().is_some_and(|source| source.available);
    let trace = if let Some(snapshot) = snapshot {
        // Fresh core is its own optional domain, as in request admission.
        // Failure leaves its Trace hole without poisoning the independent
        // original-event witness/source domain.
        Some(optional_display_layout(transaction, || {
            observation_trace::prepare_request_core(transaction, event, snapshot, true)
        })?)
    } else {
        optional_display_layout(transaction, || {
            observation_trace::prepare_event(transaction, event, pending.execution, true)
        })?
    };
    available &= trace.is_some();
    let turn_required = observation_turn::required(event)
        || matches!(event.event, RuntimeEvent::InboundTurnAdopted { .. });
    let turn = turn_required
        .then(|| {
            // Pending Turn preparation has no SQL writes; even the adoption
            // prompt is returned for the same final INSERT.
            super::optional_display_value(observation_turn::prepare(
                transaction,
                event,
                pending.statistics,
                Some(&pending),
            ))
        })
        .transpose()?
        .flatten();
    available &= !turn_required || turn.is_some();
    if let Some(cursor) = transcript {
        let kind = if matches!(
            event.event,
            RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
        ) {
            "interaction_event"
        } else {
            "attempt_terminal"
        };
        available &= optional_display_layout(transaction, || {
            observation::index::index_pending_native_event(
                transaction,
                kind,
                event,
                cursor,
                aggregates.as_ref().and_then(|value| value.members.as_ref()),
                pending.execution,
            )
        })?
        .is_some();
    }
    Ok(Row {
        aggregates,
        assistant_ledger,
        execution,
        occupancy,
        source,
        trace: trace.flatten(),
        turn,
        transcript,
        readiness,
        request_trace_prepared: snapshot.is_some(),
        canonical_publication_prepared: publication.is_some(),
        available,
    })
}

fn insert(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    json: &str,
    row: &Row,
) -> Result<(), ConversationStoreError> {
    let aggregate = row.aggregates.as_ref();
    let source = row.source.as_ref();
    let turn = row.turn.as_ref();
    let occurrence = event.tool_occurrence.as_ref();
    let phase = match event.event {
        RuntimeEvent::ToolExecutionStarted { .. } => Some(1),
        RuntimeEvent::ToolExecutionCompleted { .. } | RuntimeEvent::ToolExecutionFailed { .. } => {
            Some(2)
        }
        _ => None,
    };
    #[cfg(test)]
    let _probe = super::start_probe::for_event(event, super::AuxiliaryStartProbe::ObservationJson);
    connection.execute_cached(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp,
        observation_trace_required,observation_statistics_required,observation_kind,observation_members_required,observation_turn_required,observation_trace_fact_required,observation_execution_required,
        observation_readiness_missing,observation_trace_total,observation_statistics,observation_members,observation_member_anchor,
        observation_assistant_ledger,observation_tool_ledger,observation_tool_block,observation_tool_phase,observation_execution,observation_execution_owner,
        observation_occupancy,observation_source,observation_witness,observation_trace_fact,observation_trace_owner,
        observation_transcript,observation_turn_header,observation_turn_anchor,observation_turn_ordinal,observation_turn_prompt,observation_turn_prompt_positive,observation_unavailable)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38)",
        params![event.sequence,event.event_id.as_str(),i64::from(event.schema_version),event.conversation_id.as_str(),event.attempt_id.as_ref().map(ToString::to_string),event.turn_id.as_ref().map(ToString::to_string),json,event.timestamp.to_rfc3339(),
            observation_statistics::is_trace_anchor(&event.event),observation_statistics::requires_statistics(event),observation_event::native_kind(&event.event).ok(),observation_members::requires_members(event),observation_turn::required(event),observation_trace::required(event),observation_execution::required(event),
            !row.readiness,aggregate.and_then(|value|value.statistics.trace_total),aggregate.and_then(|value|value.statistics.statistics.as_ref()).map(|value|&value.bytes),aggregate.and_then(|value|value.members.as_ref()).map(|value|&value.bytes),aggregate.and_then(append_facts::Aggregates::member_anchor),
            row.assistant_ledger,occurrence.map(|value|value.ledger_position),occurrence.map(|value|value.block_index.get()),occurrence.and(phase),row.execution.as_ref().map(|value|&value.capture.bytes),row.execution.as_ref().map(|value|value.owner),
            row.occupancy,source.and_then(|value|value.inline_source.as_ref()),source.map(|value|&value.witness.bytes),row.trace.as_ref().map(|value|&value.bytes),row.trace.as_ref().and_then(|value|value.owner),
            row.transcript.map(TranscriptCursor::get),turn.and_then(|value|value.header.as_ref()),turn.and_then(|value|value.anchor),turn.and_then(|value|value.ordinal),turn.and_then(|value|value.prompt.as_ref()),turn.and_then(|value|value.prompt.as_ref()).is_some_and(|bytes|!bytes.is_empty()),!row.available])
        .map_err(|error|crate::durable::sqlite::sqlite_error(&error,format!("insert prepared runtime event: {error}")))?;
    // Immediate foreign keys require the native J row before its model and
    // adoption children. Their values were prepared above without rereading
    // history. A failed child rolls back this entire candidate, then the
    // native-first algorithm isolates the actual rejected optional domain.
    if let Some(execution) = &row.execution {
        execution.insert_dependencies(connection, event)?;
    }
    if let Some(trace) = &row.trace {
        trace.insert_dependencies(connection, event)?;
    }
    Ok(())
}

fn finish(
    guard: &Guard,
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    event: &mut RuntimeEventEnvelope,
    row: Row,
    prepare_observation: bool,
) -> Result<Inserted, ConversationStoreError> {
    transaction
        .execute_cached(
            "UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1",
            [event.sequence],
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("update event sequence: {error}"))
        })?;
    if let RuntimeEvent::ModelRequestCompleted {
        owner: crate::model::ModelRequestOwner::Agent { .. },
        request_id,
        ..
    } = &event.event
    {
        let updated = transaction
            .execute_cached(
                super::request::COMPLETE_REQUEST_SQL,
                params![event.sequence, request_id.as_str()],
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
    let witness = row.source.map(|source| source.witness);
    let execution = row.execution.map(|execution| execution.capture);
    event.observation = prepare_observation
        .then(|| {
            observation_receipt::single_prepared(
                transaction,
                conversation,
                event,
                witness.as_ref(),
                execution.as_ref(),
            )
        })
        .flatten();
    Ok(Inserted {
        transcript_cursor: row.transcript,
        facts: append_facts::Facts {
            request_trace_prepared: row.request_trace_prepared,
            canonical_publication_prepared: row.canonical_publication_prepared,
            witness,
            execution,
            members: row.aggregates.and_then(|aggregate| {
                aggregate.statistics.remember(&guard.statistics);
                aggregate.members
            }),
        },
    })
}

#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))}; }
#[cfg(test)]
pub(super) fn take_work() -> (usize, usize) {
    WORK.with(|work| work.replace((0, 0)))
}

#[cfg(test)]
thread_local! { static GUARD_SCANS: std::cell::Cell<usize> = const {std::cell::Cell::new(0)}; }
#[cfg(test)]
mod guard_tests;
