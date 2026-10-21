#![allow(clippy::too_many_lines, clippy::type_complexity)]

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::context::resources::ResourceLimits;
use crate::durable::ConversationStoreError;
use crate::durable::auxiliary::{
    AuxiliaryCallStartCommit, AuxiliaryRequestStartCommit, AuxiliarySettlementCommit,
    AuxiliaryWriteFailure, call_settled_event_id, request_terminal_event_id,
};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::model::auxiliary::{
    AUXILIARY_AUDIT_PAGE_LIMIT_MAX, AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
    AUXILIARY_CALL_START_ENCODED_MAX, AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX,
    AUXILIARY_REQUEST_ID_BYTES_MAX, AUXILIARY_REQUEST_MODEL_BYTES_MAX,
    AUXILIARY_REQUEST_START_ENCODED_MAX, AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
    AuxiliaryAuditCut, AuxiliaryAuditFact, AuxiliaryAuditPage, AuxiliaryAuditRecord,
    AuxiliaryBusinessSource, AuxiliaryCallAudit, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition, AuxiliaryFinishReason,
    AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal, AuxiliaryRequestAudit,
    AuxiliaryRequestEvidence, AuxiliaryRequestStart, AuxiliaryRequestTerminal,
    AuxiliaryUsageCompleteness,
};
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AuxiliaryCallId, CompactionOperationId, ConversationId, EventId, MessageId, RequestId,
};

use super::journal::persist_event_tx;
use super::{
    SqliteConversationStore, decode, journal_prefix, seq_to_i64, sequence_from_i64, storage,
};

const AUXILIARY_SCHEMA: &str = r"
CREATE INDEX IF NOT EXISTS events_primary_trace_kind_idx
    ON events(observation_kind,sequence) WHERE observation_trace_fact_required=1;
CREATE TABLE IF NOT EXISTS request_owners (
    request_id TEXT PRIMARY KEY
        CHECK(length(CAST(request_id AS BLOB)) >= 1),
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('agent','auxiliary')),
    attempt_id TEXT,
    turn_id TEXT,
    retry_number INTEGER,
    call_id TEXT REFERENCES auxiliary_calls(call_id),
    auxiliary_ordinal INTEGER,
    start_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    terminal_sequence INTEGER UNIQUE REFERENCES events(sequence),
    terminal_kind TEXT CHECK(terminal_kind IN ('completed','failed')),
    CHECK(owner_kind='agent'
          OR length(CAST(request_id AS BLOB)) <= 512),
    CHECK(
        (owner_kind='agent'
         AND attempt_id IS NOT NULL AND turn_id IS NOT NULL
         AND retry_number IS NOT NULL
         AND retry_number BETWEEN 0 AND 4294967295
         AND call_id IS NULL AND auxiliary_ordinal IS NULL)
        OR
        (owner_kind='auxiliary'
         AND attempt_id IS NULL AND turn_id IS NULL AND retry_number IS NULL
         AND call_id IS NOT NULL
         AND auxiliary_ordinal IS NOT NULL
         AND auxiliary_ordinal BETWEEN 0 AND 4294967295)
    ),
    CHECK((terminal_sequence IS NULL) = (terminal_kind IS NULL))
);

CREATE TABLE IF NOT EXISTS auxiliary_calls (
    call_id TEXT PRIMARY KEY
        CHECK(length(CAST(call_id AS BLOB)) BETWEEN 1 AND 64),
    started_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    settled_sequence INTEGER UNIQUE REFERENCES events(sequence),
    outcome TEXT CHECK(outcome IN ('completed','failed','cancelled')),
    CHECK((settled_sequence IS NULL) = (outcome IS NULL))
);

CREATE TABLE IF NOT EXISTS auxiliary_requests (
    call_id TEXT NOT NULL REFERENCES auxiliary_calls(call_id),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4294967295),
    request_id TEXT NOT NULL UNIQUE
        CHECK(length(CAST(request_id AS BLOB)) BETWEEN 1 AND 512),
    model TEXT NOT NULL CHECK(length(CAST(model AS BLOB)) BETWEEN 1 AND 512),
    started_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    terminal_sequence INTEGER UNIQUE REFERENCES events(sequence),
    terminal_kind TEXT CHECK(terminal_kind IN ('completed','failed')),
    PRIMARY KEY(call_id,ordinal),
    CHECK((terminal_sequence IS NULL) = (terminal_kind IS NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS auxiliary_requests_open_idx
    ON auxiliary_requests(call_id) WHERE terminal_sequence IS NULL;
CREATE INDEX IF NOT EXISTS auxiliary_requests_request_idx
    ON auxiliary_requests(request_id,started_sequence);
CREATE INDEX IF NOT EXISTS auxiliary_requests_terminal_idx
    ON auxiliary_requests(call_id,terminal_sequence);

CREATE TABLE IF NOT EXISTS auxiliary_audit_facts (
    sequence INTEGER PRIMARY KEY REFERENCES events(sequence),
    audit_offset INTEGER NOT NULL CHECK(audit_offset >= 1),
    kind TEXT NOT NULL CHECK(kind IN (
        'call_started','request_started','request_settled','call_settled'
    )),
    call_id TEXT NOT NULL REFERENCES auxiliary_calls(call_id),
    request_id TEXT
        CHECK(request_id IS NULL OR length(CAST(request_id AS BLOB)) BETWEEN 1 AND 512),
    ordinal INTEGER,
    CHECK(
        (kind IN ('call_started','call_settled')
         AND request_id IS NULL AND ordinal IS NULL)
        OR
        (kind IN ('request_started','request_settled')
         AND request_id IS NOT NULL
         AND ordinal IS NOT NULL
         AND ordinal BETWEEN 0 AND 4294967295)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS auxiliary_audit_offset_idx
    ON auxiliary_audit_facts(audit_offset);
CREATE INDEX IF NOT EXISTS auxiliary_audit_call_idx
    ON auxiliary_audit_facts(call_id,sequence);
CREATE INDEX IF NOT EXISTS auxiliary_audit_request_idx
    ON auxiliary_audit_facts(request_id,sequence)
    WHERE request_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS auxiliary_audit_sequence_idx
    ON auxiliary_audit_facts(sequence,audit_offset);
CREATE INDEX IF NOT EXISTS auxiliary_calls_started_idx
    ON auxiliary_calls(started_sequence);
CREATE INDEX IF NOT EXISTS auxiliary_calls_settled_idx
    ON auxiliary_calls(settled_sequence)
    WHERE settled_sequence IS NOT NULL;
CREATE INDEX IF NOT EXISTS request_owners_source_idx
    ON request_owners(owner_kind,call_id,auxiliary_ordinal)
    WHERE owner_kind='auxiliary';
CREATE INDEX IF NOT EXISTS events_auxiliary_call_id_kind_idx
    ON events(json_extract(event_json,'$.event.call_id'),
              json_extract(event_json,'$.event.type'),sequence);
CREATE INDEX IF NOT EXISTS events_auxiliary_fact_type_idx
    ON events(json_extract(event_json,'$.event.type'),sequence)
    WHERE (
        json_extract(event_json,'$.event.type') IN
            ('model_request_started','model_request_completed','model_request_failed')
        AND json_extract(event_json,'$.event.owner.type')='auxiliary'
    )
    OR json_extract(event_json,'$.event.type') IN
        ('auxiliary_call_started','auxiliary_call_settled');
";

const AUXILIARY_SEQUENCE_ENCODING_DIGITS_MAX: usize = 20;

struct AuxiliaryIndexShape {
    table: &'static str,
    name: &'static str,
    unique: bool,
    partial: bool,
    columns: &'static [Option<&'static str>],
    definition: &'static str,
}

const AUXILIARY_INDEXES: &[AuxiliaryIndexShape] = &[
    AuxiliaryIndexShape {
        table: "events",
        name: "events_primary_trace_kind_idx",
        unique: false,
        partial: true,
        columns: &[Some("observation_kind"), Some("sequence")],
        definition: "CREATE INDEX events_primary_trace_kind_idx ON events(observation_kind,sequence) WHERE observation_trace_fact_required=1",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_requests",
        name: "auxiliary_requests_open_idx",
        unique: true,
        partial: true,
        columns: &[Some("call_id")],
        definition: "CREATE UNIQUE INDEX auxiliary_requests_open_idx ON auxiliary_requests(call_id) WHERE terminal_sequence IS NULL",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_requests",
        name: "auxiliary_requests_request_idx",
        unique: false,
        partial: false,
        columns: &[Some("request_id"), Some("started_sequence")],
        definition: "CREATE INDEX auxiliary_requests_request_idx ON auxiliary_requests(request_id,started_sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_requests",
        name: "auxiliary_requests_terminal_idx",
        unique: false,
        partial: false,
        columns: &[Some("call_id"), Some("terminal_sequence")],
        definition: "CREATE INDEX auxiliary_requests_terminal_idx ON auxiliary_requests(call_id,terminal_sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_offset_idx",
        unique: true,
        partial: false,
        columns: &[Some("audit_offset")],
        definition: "CREATE UNIQUE INDEX auxiliary_audit_offset_idx ON auxiliary_audit_facts(audit_offset)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_call_idx",
        unique: false,
        partial: false,
        columns: &[Some("call_id"), Some("sequence")],
        definition: "CREATE INDEX auxiliary_audit_call_idx ON auxiliary_audit_facts(call_id,sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_request_idx",
        unique: false,
        partial: true,
        columns: &[Some("request_id"), Some("sequence")],
        definition: "CREATE INDEX auxiliary_audit_request_idx ON auxiliary_audit_facts(request_id,sequence) WHERE request_id IS NOT NULL",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_sequence_idx",
        unique: false,
        partial: false,
        columns: &[Some("sequence"), Some("audit_offset")],
        definition: "CREATE INDEX auxiliary_audit_sequence_idx ON auxiliary_audit_facts(sequence,audit_offset)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_calls",
        name: "auxiliary_calls_started_idx",
        unique: false,
        partial: false,
        columns: &[Some("started_sequence")],
        definition: "CREATE INDEX auxiliary_calls_started_idx ON auxiliary_calls(started_sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_calls",
        name: "auxiliary_calls_settled_idx",
        unique: false,
        partial: true,
        columns: &[Some("settled_sequence")],
        definition: "CREATE INDEX auxiliary_calls_settled_idx ON auxiliary_calls(settled_sequence) WHERE settled_sequence IS NOT NULL",
    },
    AuxiliaryIndexShape {
        table: "request_owners",
        name: "request_owners_source_idx",
        unique: false,
        partial: true,
        columns: &[
            Some("owner_kind"),
            Some("call_id"),
            Some("auxiliary_ordinal"),
        ],
        definition: "CREATE INDEX request_owners_source_idx ON request_owners(owner_kind,call_id,auxiliary_ordinal) WHERE owner_kind='auxiliary'",
    },
    AuxiliaryIndexShape {
        table: "events",
        name: "events_auxiliary_call_id_kind_idx",
        unique: false,
        partial: false,
        columns: &[None, None, Some("sequence")],
        definition: "CREATE INDEX events_auxiliary_call_id_kind_idx ON events(json_extract(event_json,'$.event.call_id'),json_extract(event_json,'$.event.type'),sequence)",
    },
    AuxiliaryIndexShape {
        table: "events",
        name: "events_auxiliary_fact_type_idx",
        unique: false,
        partial: true,
        columns: &[None, Some("sequence")],
        definition: "CREATE INDEX events_auxiliary_fact_type_idx ON events(json_extract(event_json,'$.event.type'),sequence) WHERE (json_extract(event_json,'$.event.type') IN ('model_request_started','model_request_completed','model_request_failed') AND json_extract(event_json,'$.event.owner.type')='auxiliary') OR json_extract(event_json,'$.event.type') IN ('auxiliary_call_started','auxiliary_call_settled')",
    },
];

pub(super) fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(AUXILIARY_SCHEMA)
        .map_err(|error| storage(format!("create auxiliary owner schema: {error}")))
}

fn normalized_index_definition(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace("ifnotexists", "")
}

fn verify_required_indexes(connection: &Connection) -> Result<(), ConversationStoreError> {
    for expected in AUXILIARY_INDEXES {
        let listed: Option<(bool, bool)> = connection
            .prepare(&format!("PRAGMA index_list({})", expected.table))
            .map_err(|error| {
                storage(format!(
                    "inspect auxiliary index {} on {}: {error}",
                    expected.name, expected.table
                ))
            })?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| storage(format!("read auxiliary indexes: {error}")))?
            .find_map(|row| match row {
                Ok((name, unique, partial)) if name == expected.name => {
                    Some(Ok((unique == 1, partial == 1)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .transpose()
            .map_err(|error| storage(format!("read auxiliary index: {error}")))?;
        let Some((unique, partial)) = listed else {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing required auxiliary index {}",
                expected.name
            )));
        };
        if unique != expected.unique || partial != expected.partial {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "auxiliary index {} has the wrong uniqueness or partial shape",
                expected.name
            )));
        }
        let quoted = expected.name.replace('"', "\"\"");
        let columns = connection
            .prepare(&format!("PRAGMA index_info(\"{quoted}\")"))
            .map_err(|error| storage(format!("inspect auxiliary index columns: {error}")))?
            .query_map([], |row| row.get::<_, Option<String>>(2))
            .map_err(|error| storage(format!("read auxiliary index columns: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("collect auxiliary index columns: {error}")))?;
        if columns.len() != expected.columns.len()
            || columns
                .iter()
                .zip(expected.columns)
                .any(|(actual, expected)| actual.as_deref() != *expected)
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "auxiliary index {} has the wrong columns",
                expected.name
            )));
        }
        let definition: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type='index' AND name=?1 AND tbl_name=?2",
                params![expected.name, expected.table],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("read auxiliary index definition: {error}")))?;
        let expected_definition = normalized_index_definition(expected.definition);
        if definition
            .as_deref()
            .map(normalized_index_definition)
            .as_deref()
            != Some(expected_definition.as_str())
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "auxiliary index {} has the wrong column or predicate definition",
                expected.name
            )));
        }
    }
    Ok(())
}

pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    for (table, columns) in [
        (
            "request_owners",
            &[
                "request_id",
                "owner_kind",
                "attempt_id",
                "turn_id",
                "retry_number",
                "call_id",
                "auxiliary_ordinal",
                "start_sequence",
                "terminal_sequence",
                "terminal_kind",
            ][..],
        ),
        (
            "auxiliary_calls",
            &["call_id", "started_sequence", "settled_sequence", "outcome"][..],
        ),
        (
            "auxiliary_requests",
            &[
                "call_id",
                "ordinal",
                "request_id",
                "model",
                "started_sequence",
                "terminal_sequence",
                "terminal_kind",
            ][..],
        ),
        (
            "auxiliary_audit_facts",
            &[
                "sequence",
                "audit_offset",
                "kind",
                "call_id",
                "request_id",
                "ordinal",
            ][..],
        ),
    ] {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| storage(format!("inspect auxiliary table {table}: {error}")))?;
        let actual = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| storage(format!("read auxiliary table {table}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("collect auxiliary table {table}: {error}")))?;
        for column in columns {
            if !actual.iter().any(|candidate| candidate == column) {
                return Err(ConversationStoreError::IncompatibleSchema(format!(
                    "table {table} is missing required column {column}"
                )));
            }
        }
        if table == "auxiliary_calls"
            && actual.iter().any(|column| {
                matches!(
                    column.as_str(),
                    "purpose"
                        | "source"
                        | "trigger"
                        | "business_source"
                        | "input"
                        | "semantic_input_fingerprint"
                        | "invocation_fingerprint"
                        | "history_cut"
                )
            })
        {
            return Err(ConversationStoreError::IncompatibleSchema(
                "auxiliary_calls contains a retired metadata layout".into(),
            ));
        }
    }
    verify_required_indexes(connection)?;
    for (table, column) in [
        ("request_owners", "request_id"),
        ("request_owners", "start_sequence"),
        ("request_owners", "terminal_sequence"),
        ("auxiliary_calls", "call_id"),
        ("auxiliary_calls", "started_sequence"),
        ("auxiliary_calls", "settled_sequence"),
        ("auxiliary_requests", "request_id"),
        ("auxiliary_requests", "started_sequence"),
        ("auxiliary_requests", "terminal_sequence"),
        ("auxiliary_audit_facts", "audit_offset"),
    ] {
        super::schema::shape::verify_unique_column(connection, table, column)?;
    }
    super::schema::shape::verify_unique_columns(
        connection,
        "auxiliary_requests",
        &["call_id", "ordinal"],
    )?;
    Ok(())
}

#[derive(Default)]
struct BoundedWriter {
    bytes: usize,
    maximum: usize,
    exceeded: bool,
}

impl std::io::Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let Some(total) = self.bytes.checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::other("encoded byte count overflow"));
        };
        if total > self.maximum {
            self.exceeded = true;
            return Err(std::io::Error::other("encoded byte bound exceeded"));
        }
        self.bytes = total;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_encoded_len<T: serde::Serialize + ?Sized>(
    value: &T,
    maximum: usize,
    name: &str,
) -> Result<usize, ConversationStoreError> {
    let mut writer = BoundedWriter {
        bytes: 0,
        maximum,
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        if writer.exceeded {
            return Err(ConversationStoreError::ResourceBoundExceeded(format!(
                "{name} exceeds its {maximum}-byte encoded bound"
            )));
        }
        return Err(storage(format!("encode {name}")));
    }
    Ok(writer.bytes)
}

fn validate_call_start_input(start: &AuxiliaryCallStart) -> Result<(), ConversationStoreError> {
    let call_bytes = start.call_id.as_str().len();
    if !(1..=64).contains(&call_bytes) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary CallId must contain 1..=64 bytes".into(),
        ));
    }
    if start.selected_message_ids.len() > AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX {
        return Err(ConversationStoreError::ResourceBoundExceeded(format!(
            "auxiliary Call may select at most {AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX} MessageIds"
        )));
    }
    if start
        .selected_message_ids
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != start.selected_message_ids.len()
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary selected MessageIds must be unique".into(),
        ));
    }
    if matches!(start.input, AuxiliaryInputEvidence::Temporary)
        && !start.selected_message_ids.is_empty()
    {
        return Err(ConversationStoreError::InvalidReference(
            "temporary auxiliary input cannot name selected historical MessageIds".into(),
        ));
    }
    bounded_encoded_len(
        start,
        AUXILIARY_CALL_START_ENCODED_MAX,
        "auxiliary Call start metadata",
    )?;
    audit_record_bytes(&AuxiliaryAuditRecord {
        audit_offset: u64::MAX,
        journal_sequence: u64::MAX,
        fact: AuxiliaryAuditFact::CallSettled(AuxiliaryCallAudit {
            call_id: start.call_id.clone(),
            purpose: start.purpose.clone(),
            trigger: start.trigger.clone(),
            business_source: start.business_source.clone(),
            input: start.input.clone(),
            semantic_input_fingerprint: start.semantic_input_fingerprint.clone(),
            invocation_fingerprint: start.invocation_fingerprint.clone(),
            selected_message_ids: start.selected_message_ids.clone(),
            started_sequence: u64::MAX,
            settled_sequence: Some(u64::MAX),
            outcome: Some(AuxiliaryCallOutcome::Completed),
        }),
    })?;
    Ok(())
}

fn validate_request_start_input(
    start: &AuxiliaryRequestStart,
) -> Result<(), ConversationStoreError> {
    if start.request_id.as_str().is_empty()
        || start.request_id.as_str().len() > AUXILIARY_REQUEST_ID_BYTES_MAX
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary RequestId must contain 1..={AUXILIARY_REQUEST_ID_BYTES_MAX} bytes"
        )));
    }
    if start.model.is_empty() || start.model.len() > AUXILIARY_REQUEST_MODEL_BYTES_MAX {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request model name must contain 1..={AUXILIARY_REQUEST_MODEL_BYTES_MAX} bytes"
        )));
    }
    let maximum_usage = ModelUsage {
        input_tokens: u64::MAX,
        output_tokens: u64::MAX,
        total_tokens: u64::MAX,
        details: Some(crate::model::types::UsageDetails {
            reasoning_tokens: Some(u64::MAX),
            cached_input_tokens: Some(u64::MAX),
        }),
    };
    let worst_case = AuxiliaryAuditRecord {
        audit_offset: u64::MAX,
        journal_sequence: u64::MAX,
        fact: AuxiliaryAuditFact::RequestSettled(AuxiliaryRequestAudit {
            call_id: start.call_id.clone(),
            ordinal: start.ordinal,
            request_id: start.request_id.clone(),
            model: start.model.clone(),
            started_sequence: u64::MAX,
            terminal_sequence: Some(u64::MAX),
            local_terminal: Some(AuxiliaryRequestTerminal::Completed),
            normalized_failure_kind: Some(
                crate::model::error::ModelErrorKind::GenerationBudgetExceeded,
            ),
            generation: Some(GenerationEvidence {
                dispatch_after_start_ms: Some(u64::MAX),
                first_output_ms: Some(u64::MAX),
                last_output_ms: Some(u64::MAX),
                terminal_ms: u64::MAX,
            }),
            usage: Some(maximum_usage.clone()),
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id: start.request_id.clone(),
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::ContentFilter,
                }),
                usage: Some(maximum_usage),
                usage_completeness: AuxiliaryUsageCompleteness::Complete,
                remote_execution_uncertainty: true,
            }),
        }),
    };
    audit_record_bytes(&worst_case)?;
    bounded_encoded_len(
        start,
        AUXILIARY_REQUEST_START_ENCODED_MAX,
        "auxiliary request start metadata",
    )?;
    Ok(())
}

fn call_started_event_id(call_id: &AuxiliaryCallId) -> EventId {
    EventId::new(format!("auxiliary-call-started:{call_id}"))
}

fn request_started_event_id(request_id: &RequestId) -> EventId {
    EventId::new(format!("auxiliary-request-started:{request_id}"))
}

fn call_start_matches(event: &RuntimeEventEnvelope, start: &AuxiliaryCallStart) -> bool {
    event.attempt_id.is_none()
        && event.turn_id.is_none()
        && matches!(
            &event.event,
            RuntimeEvent::AuxiliaryCallStarted {
                call_id,
                purpose,
                trigger,
                business_source,
                input,
                semantic_input_fingerprint,
                invocation_fingerprint,
                selected_message_ids,
            } if call_id == &start.call_id
                && *purpose == start.purpose
                && *trigger == start.trigger
                && business_source == &start.business_source
                && input == &start.input
                && semantic_input_fingerprint == &start.semantic_input_fingerprint
                && invocation_fingerprint == &start.invocation_fingerprint
                && selected_message_ids == &start.selected_message_ids
        )
}

fn request_start_matches(event: &RuntimeEventEnvelope, start: &AuxiliaryRequestStart) -> bool {
    event.attempt_id.is_none()
        && event.turn_id.is_none()
        && matches!(
            &event.event,
            RuntimeEvent::ModelRequestStarted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                model,
            } if call_id == &start.call_id
                && *ordinal == start.ordinal
                && request_id == &start.request_id
                && model == &start.model
        )
}

fn terminal_kind(event: &RuntimeEvent) -> Option<&'static str> {
    match event {
        RuntimeEvent::ModelRequestCompleted { .. } => Some("completed"),
        RuntimeEvent::ModelRequestFailed { .. } => Some("failed"),
        _ => None,
    }
}

fn outcome_name(outcome: AuxiliaryCallOutcome) -> &'static str {
    match outcome {
        AuxiliaryCallOutcome::Completed => "completed",
        AuxiliaryCallOutcome::Failed => "failed",
        AuxiliaryCallOutcome::Cancelled => "cancelled",
    }
}

fn call_outcome_for_terminal(event: &RuntimeEvent) -> Option<&[AuxiliaryCallOutcome]> {
    match event {
        RuntimeEvent::ModelRequestCompleted { .. } => Some(&[AuxiliaryCallOutcome::Completed]),
        RuntimeEvent::ModelRequestFailed { .. } => Some(&[
            AuxiliaryCallOutcome::Failed,
            AuxiliaryCallOutcome::Cancelled,
        ]),
        _ => None,
    }
}

fn decimal_digits(mut value: u64) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn validate_terminal_envelope_shape(
    event: &RuntimeEventEnvelope,
    conversation_id: &ConversationId,
    require_unallocated_sequence: bool,
) -> Result<(), ConversationStoreError> {
    if event.schema_version != EVENT_SCHEMA_VERSION {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary terminal schema {} is not {}",
            event.schema_version, EVENT_SCHEMA_VERSION
        )));
    }
    if event.conversation_id != *conversation_id {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: conversation_id.clone(),
            requested: event.conversation_id.clone(),
        });
    }
    if event.attempt_id.is_some() || event.turn_id.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement envelopes must not carry an Attempt or Turn".into(),
        ));
    }
    if event.tool_occurrence.is_some() || event.observation.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement envelopes cannot carry Tool or observation evidence".into(),
        ));
    }
    if require_unallocated_sequence && event.sequence != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "a new auxiliary settlement must leave Journal sequence unallocated".into(),
        ));
    }
    if event.event_id != crate::durable::auxiliary::settlement_event_id(&event.event)? {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary terminal has a noncanonical event identity".into(),
        ));
    }
    if let RuntimeEvent::ModelRequestCompleted {
        generation: Some(generation),
        ..
    }
    | RuntimeEvent::ModelRequestFailed {
        generation: Some(generation),
        ..
    } = &event.event
        && !crate::model::auxiliary::generation_is_bounded(generation)
    {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary generation timing exceeds the common observation integer domain".into(),
        ));
    }
    let reserved_sequence_digits =
        AUXILIARY_SEQUENCE_ENCODING_DIGITS_MAX.saturating_sub(decimal_digits(event.sequence));
    let admission_maximum =
        AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX.saturating_sub(reserved_sequence_digits);
    bounded_encoded_len(event, admission_maximum, "auxiliary settlement envelope")?;
    Ok(())
}

fn validate_request_evidence(
    evidence: &AuxiliaryRequestEvidence,
    request_terminal: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let (request_id, local_completed, local_finish_reason, usage) = match &request_terminal.event {
        RuntimeEvent::ModelRequestCompleted {
            request_id,
            finish_reason,
            usage,
            ..
        } => (request_id, true, Some(finish_reason), usage),
        RuntimeEvent::ModelRequestFailed {
            request_id, usage, ..
        } => (request_id, false, None, usage),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary request evidence requires a request terminal".into(),
            ));
        }
    };
    if &evidence.request_id != request_id {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement request evidence names another RequestId".into(),
        ));
    }
    if evidence.usage != *usage {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement Usage disagrees with the physical terminal".into(),
        ));
    }
    match evidence.usage_completeness {
        AuxiliaryUsageCompleteness::Complete | AuxiliaryUsageCompleteness::Partial
            if evidence.usage.is_none() =>
        {
            return Err(ConversationStoreError::InvalidReference(
                "observed auxiliary Usage completeness requires Usage evidence".into(),
            ));
        }
        AuxiliaryUsageCompleteness::Unreported if evidence.usage.is_some() => {
            return Err(ConversationStoreError::InvalidReference(
                "unreported auxiliary Usage cannot carry a Usage value".into(),
            ));
        }
        _ => {}
    }
    if local_completed {
        match &evidence.observed_provider_terminal {
            None => {
                return Err(ConversationStoreError::InvalidReference(
                    "a locally completed auxiliary request requires an observed provider completion"
                        .into(),
                ));
            }
            Some(AuxiliaryObservedProviderTerminal::Failed { .. }) => {
                return Err(ConversationStoreError::InvalidReference(
                    "a locally completed auxiliary request cannot record provider failure as its terminal"
                        .into(),
                ));
            }
            Some(AuxiliaryObservedProviderTerminal::Completed { finish_reason })
                if Some(*finish_reason) != local_finish_reason.map(AuxiliaryFinishReason::from) =>
            {
                return Err(ConversationStoreError::InvalidReference(
                    "observed provider completion kind disagrees with the locally completed auxiliary request"
                        .into(),
                ));
            }
            Some(AuxiliaryObservedProviderTerminal::Completed { .. }) => {}
        }
    }
    Ok(())
}

fn validate_settlement_pair(
    conversation_id: &ConversationId,
    request_terminal: &RuntimeEventEnvelope,
    call_settled: &RuntimeEventEnvelope,
    require_unallocated_sequence: bool,
) -> Result<(), ConversationStoreError> {
    validate_terminal_envelope_shape(
        request_terminal,
        conversation_id,
        require_unallocated_sequence,
    )?;
    validate_terminal_envelope_shape(call_settled, conversation_id, require_unallocated_sequence)?;
    if request_terminal.conversation_id != call_settled.conversation_id {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: request_terminal.conversation_id.clone(),
            requested: call_settled.conversation_id.clone(),
        });
    }
    let Some(ModelRequestOwner::Auxiliary {
        call_id: request_call,
        ..
    }) = request_terminal.event.model_request_owner()
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request settlement requires an auxiliary-owned request terminal".into(),
        ));
    };
    let RuntimeEvent::AuxiliaryCallSettled {
        call_id: settled_call,
        outcome,
        request_evidence,
    } = &call_settled.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request settlement requires an AuxiliaryCallSettled fact".into(),
        ));
    };
    if request_call != settled_call {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request and Call settlement name different Calls".into(),
        ));
    }
    if !call_outcome_for_terminal(&request_terminal.event)
        .is_some_and(|outcomes| outcomes.contains(outcome))
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call outcome is incompatible with its request terminal".into(),
        ));
    }
    let evidence = request_evidence.as_ref().ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "final auxiliary request settlement requires request evidence".into(),
        )
    })?;
    validate_request_evidence(evidence, request_terminal)
}

fn same_terminal_envelope(stored: &RuntimeEventEnvelope, expected: &RuntimeEventEnvelope) -> bool {
    (expected.sequence == 0 || stored.sequence == expected.sequence)
        && stored.schema_version == expected.schema_version
        && stored.event_id == expected.event_id
        && stored.conversation_id == expected.conversation_id
        && stored.attempt_id == expected.attempt_id
        && stored.turn_id == expected.turn_id
        && stored.timestamp == expected.timestamp
        && stored.event == expected.event
}

fn run_auxiliary_write<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, ConversationStoreError>,
) -> Result<T, AuxiliaryWriteFailure> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            AuxiliaryWriteFailure::known(storage(format!("auxiliary write transaction: {error}")))
        })?;
    match operation(&transaction) {
        Ok(value) => transaction.commit().map_or_else(
            |error| {
                Err(AuxiliaryWriteFailure::unknown(storage(format!(
                    "auxiliary write commit: {error}"
                ))))
            },
            |()| Ok(value),
        ),
        Err(error) => match transaction.rollback() {
            Ok(()) => Err(AuxiliaryWriteFailure::known(error)),
            Err(rollback) => Err(AuxiliaryWriteFailure::unknown(storage(format!(
                "auxiliary write failed before commit ({error}) and rollback also failed ({rollback})"
            )))),
        },
    }
}

fn ledger_frontier(connection: &Connection) -> Result<u64, ConversationStoreError> {
    let value: i64 = connection
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary Ledger frontier: {error}")))?;
    u64::try_from(value)
        .map_err(|_| ConversationStoreError::InvalidReference("Ledger frontier is negative".into()))
}

fn require_message_in_ledger(
    connection: &Connection,
    message_id: &MessageId,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM message_ledger
             WHERE message_id=?1 AND position<=?2)",
            params![message_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary MessageId lookup: {error}")))?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary source MessageId {message_id} is not visible at its historical input cut"
        )));
    }
    Ok(())
}

fn require_human_message_in_ledger(
    connection: &Connection,
    message_id: &MessageId,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM message_ledger
               WHERE message_id=?1 AND position<=?2
                 AND json_extract(message_json,'$.role')='user'
                 AND json_extract(message_json,'$.source')='human'
             )",
            params![message_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary HumanMessage lookup: {error}")))?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary HumanMessage source {message_id} is not a visible canonical Human message"
        )));
    }
    Ok(())
}

fn require_compaction_operation_through(
    connection: &Connection,
    operation_id: &CompactionOperationId,
    through_journal: u64,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_compaction_operation_kind_idx
               WHERE json_extract(event_json,'$.event.operation_id')=?1
                 AND json_extract(event_json,'$.event.type') IN
                     ('compaction_started','compaction_completed','compaction_failed')
                 AND sequence<=?2
             )",
            params![operation_id.as_str(), seq_to_i64(through_journal)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary Compaction source lookup: {error}")))?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary Compaction source {operation_id} is not visible at the business-source validation frontier"
        )));
    }
    Ok(())
}

fn validate_call_start_authority_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    start: &AuxiliaryCallStart,
) -> Result<(), ConversationStoreError> {
    validate_call_start_input(start)?;
    match &start.input {
        AuxiliaryInputEvidence::Historical { cut } => {
            if cut.conversation_id != *conversation_id {
                return Err(ConversationStoreError::ConversationIdMismatch {
                    stored: conversation_id.clone(),
                    requested: cut.conversation_id.clone(),
                });
            }
            super::compaction_resources::validate(
                connection,
                conversation_id,
                cut,
                ResourceLimits::default(),
            )?;
            for message_id in &start.selected_message_ids {
                require_message_in_ledger(connection, message_id, cut.ledger.0)?;
            }
        }
        AuxiliaryInputEvidence::Temporary => {
            if !start.selected_message_ids.is_empty() {
                return Err(ConversationStoreError::InvalidReference(
                    "temporary auxiliary input cannot name selected historical MessageIds".into(),
                ));
            }
        }
    }
    // Business causality and input selection have independent coordinates.
    // A newly admitted operation may intentionally consume an older input cut.
    match &start.business_source {
        AuxiliaryBusinessSource::Independent | AuxiliaryBusinessSource::Business { .. } => {}
        AuxiliaryBusinessSource::Compaction { operation_id } => {
            require_compaction_operation_through(
                connection,
                operation_id,
                journal_prefix(connection)?,
            )?;
        }
        AuxiliaryBusinessSource::HumanMessage { message_id } => {
            require_human_message_in_ledger(connection, message_id, ledger_frontier(connection)?)?;
        }
    }
    Ok(())
}

pub(super) fn validate_call_event_tx(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &envelope.event {
        RuntimeEvent::AuxiliaryCallStarted {
            call_id,
            purpose,
            trigger,
            business_source,
            input,
            semantic_input_fingerprint,
            invocation_fingerprint,
            selected_message_ids,
        } => {
            if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call facts must not carry an Attempt or Turn envelope".into(),
                ));
            }
            if envelope.event_id != call_started_event_id(call_id) {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call start has a noncanonical event identity".into(),
                ));
            }
            let start = AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose: purpose.clone(),
                trigger: trigger.clone(),
                business_source: business_source.clone(),
                input: input.clone(),
                semantic_input_fingerprint: semantic_input_fingerprint.clone(),
                invocation_fingerprint: invocation_fingerprint.clone(),
                selected_message_ids: selected_message_ids.clone(),
            };
            validate_call_start_authority_tx(transaction, &envelope.conversation_id, &start)?;
        }
        RuntimeEvent::AuxiliaryCallSettled { call_id, .. } => {
            validate_terminal_envelope_shape(envelope, &envelope.conversation_id, true)?;
            let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} settled without a start"
                ))
            })?;
            if call.settled_sequence.is_some() {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "auxiliary Call {call_id} is already terminal"
                )));
            }
            if open_request_tx(transaction, call_id)?.is_some() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} cannot settle while a request is open"
                )));
            }
        }
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "Call validation received a non-Call fact".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_request_start_event_tx(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request start must not carry an Attempt or Turn envelope".into(),
        ));
    }
    let RuntimeEvent::ModelRequestStarted {
        owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
        request_id,
        model,
    } = &envelope.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request validation requires an auxiliary-owned start".into(),
        ));
    };
    let start = AuxiliaryRequestStart {
        call_id: call_id.clone(),
        ordinal: *ordinal,
        request_id: request_id.clone(),
        model: model.clone(),
    };
    validate_request_start_input(&start)?;
    if envelope.event_id != request_started_event_id(request_id) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request start has a noncanonical event identity".into(),
        ));
    }
    let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} requires active Call {call_id}"
        ))
    })?;
    let _ = call_audit_at_tx(
        transaction,
        &envelope.conversation_id,
        call_id,
        AuxiliaryAuditCut {
            journal: call.started_sequence,
        },
    )?
    .ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} has no authoritative Call start"
        ))
    })?;
    if call.settled_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "auxiliary Call {call_id} is terminal"
        )));
    }
    if let Some(open) = open_request_tx(transaction, call_id)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary Call {call_id} already has open request {open}"
        )));
    }
    let expected = next_request_ordinal_tx(transaction, call_id)?;
    if *ordinal != expected {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request ordinal {ordinal} is not the required next ordinal {expected}"
        )));
    }
    if request_snapshot_exists(transaction, request_id)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} conflicts with a primary Request Snapshot"
        )));
    }
    Ok(())
}

pub(super) fn validate_request_terminal_event_tx(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    validate_terminal_envelope_shape(envelope, &envelope.conversation_id, true)?;
    let (RuntimeEvent::ModelRequestCompleted {
        owner, request_id, ..
    }
    | RuntimeEvent::ModelRequestFailed {
        owner, request_id, ..
    }) = &envelope.event
    else {
        unreachable!("terminal validation received another fact")
    };
    let ModelRequestOwner::Auxiliary { call_id, ordinal } = owner else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary terminal validation received an Agent owner".into(),
        ));
    };
    let request = request_row_tx(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request terminal names unstarted request {request_id}"
        ))
    })?;
    if &request.call_id != call_id || request.ordinal != *ordinal {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request terminal for {request_id} disagrees with its authoritative owner"
        )));
    }
    if request.terminal_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "auxiliary request {request_id} already has a terminal"
        )));
    }
    let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} has no Call start"
        ))
    })?;
    if call.settled_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "auxiliary Call {call_id} is already terminal"
        )));
    }
    let indexed = owner_row_tx(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} has no owner index"
        ))
    })?;
    if indexed.owner != *owner
        || indexed.start_sequence != request.started_sequence
        || indexed.terminal_sequence.is_some()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request terminal for {request_id} disagrees with its owner index"
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct CallRow {
    started_sequence: u64,
    settled_sequence: Option<u64>,
    outcome: Option<AuxiliaryCallOutcome>,
}

fn call_row_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<Option<CallRow>, ConversationStoreError> {
    call_row_query(transaction, call_id)
}

fn call_row_query(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
) -> Result<Option<CallRow>, ConversationStoreError> {
    let row: Option<(i64, Option<i64>, Option<String>)> = connection
        .query_row(
            "SELECT started_sequence,settled_sequence,outcome
             FROM auxiliary_calls WHERE call_id=?1",
            [call_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("auxiliary Call lookup: {error}")))?;
    row.map(|(started_sequence, settled_sequence, outcome)| {
        Ok(CallRow {
            started_sequence: sequence_from_i64(started_sequence)?,
            settled_sequence: settled_sequence.map(sequence_from_i64).transpose()?,
            outcome: outcome
                .map(|value| decode(&format!("\"{value}\""), "auxiliary Call outcome"))
                .transpose()?,
        })
    })
    .transpose()
}

#[derive(Debug)]
struct RequestRow {
    call_id: AuxiliaryCallId,
    ordinal: u32,
    request_id: RequestId,
    model: String,
    started_sequence: u64,
    terminal_sequence: Option<u64>,
    terminal_kind: Option<AuxiliaryRequestTerminal>,
}

fn request_row_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RequestRow>, ConversationStoreError> {
    request_row_query(transaction, "request_id=?1", [request_id.as_str()])
}

fn request_row_query<P: rusqlite::Params>(
    connection: &Connection,
    predicate: &str,
    parameters: P,
) -> Result<Option<RequestRow>, ConversationStoreError> {
    let row: Option<(
        String,
        i64,
        String,
        String,
        i64,
        Option<i64>,
        Option<String>,
    )> = connection
        .query_row(
            &format!(
                "SELECT call_id,ordinal,request_id,model,started_sequence,
                        terminal_sequence,terminal_kind
                 FROM auxiliary_requests WHERE {predicate}"
            ),
            parameters,
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("auxiliary request lookup: {error}")))?;
    row.map(
        |(
            call_id,
            ordinal,
            request_id,
            model,
            started_sequence,
            terminal_sequence,
            terminal_kind,
        )| {
            Ok(RequestRow {
                call_id: AuxiliaryCallId::parse(call_id)
                    .map_err(ConversationStoreError::InvalidReference)?,
                ordinal: u32::try_from(ordinal).map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary request ordinal is out of range".into(),
                    )
                })?,
                request_id: RequestId::new(request_id),
                model,
                started_sequence: sequence_from_i64(started_sequence)?,
                terminal_sequence: terminal_sequence.map(sequence_from_i64).transpose()?,
                terminal_kind: terminal_kind
                    .map(|value| decode(&format!("\"{value}\""), "auxiliary request terminal kind"))
                    .transpose()?,
            })
        },
    )
    .transpose()
}

fn open_request_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<Option<RequestId>, ConversationStoreError> {
    open_request_query(transaction, call_id)
}

fn open_request_query(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
) -> Result<Option<RequestId>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT request_id FROM auxiliary_requests
             WHERE call_id=?1 AND terminal_sequence IS NULL",
            [call_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|value| value.map(RequestId::new))
        .map_err(|error| storage(format!("auxiliary open request lookup: {error}")))
}

fn call_has_requests_query(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM auxiliary_requests WHERE call_id=?1
             )",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary request-presence lookup: {error}")))
}

fn next_request_ordinal_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<u32, ConversationStoreError> {
    let maximum: Option<i64> = transaction
        .query_row(
            "SELECT max(ordinal) FROM auxiliary_requests WHERE call_id=?1",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary ordinal lookup: {error}")))?;
    maximum.map_or(Ok(0), |value| {
        u32::try_from(value)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary request ordinals are exhausted".into(),
                )
            })
    })
}

fn request_snapshot_exists(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM request_snapshots WHERE request_id=?1)",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("primary request lookup: {error}")))
}

#[derive(Debug)]
struct OwnerRow {
    owner: ModelRequestOwner,
    start_sequence: u64,
    terminal_sequence: Option<u64>,
    terminal_kind: Option<&'static str>,
}

fn owner_row_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<OwnerRow>, ConversationStoreError> {
    owner_row_query(transaction, request_id)
}

fn owner_row_query(
    transaction: &Connection,
    request_id: &RequestId,
) -> Result<Option<OwnerRow>, ConversationStoreError> {
    let row: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        i64,
        Option<i64>,
        Option<String>,
    )> = transaction
        .query_row(
            "SELECT owner_kind,attempt_id,turn_id,retry_number,call_id,
                    auxiliary_ordinal,start_sequence,terminal_sequence,terminal_kind
             FROM request_owners WHERE request_id=?1",
            [request_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("request owner index lookup: {error}")))?;
    row.map(
        |(
            kind,
            attempt_id,
            turn_id,
            retry_number,
            call_id,
            ordinal,
            start_sequence,
            terminal_sequence,
            terminal_kind,
        )| {
            let owner = match kind.as_str() {
                "agent" => ModelRequestOwner::Agent {
                    identity: crate::model::snapshot::RequestIdentity {
                        attempt_id: crate::runtime::identity::AttemptId::new(
                            attempt_id.ok_or_else(|| {
                                ConversationStoreError::InvalidReference(
                                    "Agent owner index lacks AttemptId".into(),
                                )
                            })?,
                        ),
                        turn: crate::runtime::identity::TurnId::new(turn_id.ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "Agent owner index lacks TurnId".into(),
                            )
                        })?),
                        retry_number: u32::try_from(retry_number.ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "Agent owner index lacks retry ordinal".into(),
                            )
                        })?)
                        .map_err(|_| {
                            ConversationStoreError::InvalidReference(
                                "Agent owner retry ordinal is out of range".into(),
                            )
                        })?,
                    },
                },
                "auxiliary" => ModelRequestOwner::Auxiliary {
                    call_id: AuxiliaryCallId::parse(call_id.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary owner index lacks CallId".into(),
                        )
                    })?)
                    .map_err(ConversationStoreError::InvalidReference)?,
                    ordinal: u32::try_from(ordinal.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary owner index lacks request ordinal".into(),
                        )
                    })?)
                    .map_err(|_| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary owner ordinal is out of range".into(),
                        )
                    })?,
                },
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "request owner index has an unknown owner kind".into(),
                    ));
                }
            };
            let terminal_kind = terminal_kind
                .map(|value| match value.as_str() {
                    "completed" => Ok("completed"),
                    "failed" => Ok("failed"),
                    _ => Err(ConversationStoreError::InvalidReference(
                        "request owner index has an unknown terminal kind".into(),
                    )),
                })
                .transpose()?;
            Ok(OwnerRow {
                owner,
                start_sequence: sequence_from_i64(start_sequence)?,
                terminal_sequence: terminal_sequence.map(sequence_from_i64).transpose()?,
                terminal_kind,
            })
        },
    )
    .transpose()
}

fn event_at_connection(
    connection: &Connection,
    sequence: u64,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let row: Option<(i64, String, i64, String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT length(CAST(event_json AS BLOB)),
                    event_id,schema_version,conversation_id,attempt_id,turn_id
             FROM events WHERE sequence=?1",
            [seq_to_i64(sequence)?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("auxiliary event lookup: {error}")))?;
    let Some((
        encoded_bytes,
        stored_event_id,
        stored_schema_version,
        stored_conversation_id,
        stored_attempt_id,
        stored_turn_id,
    )) = row
    else {
        return Ok(None);
    };
    if u64::try_from(encoded_bytes)
        .map_err(|_| storage("auxiliary event encoded length is negative"))?
        > AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX as u64
    {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary source event exceeds its encoded byte bound".into(),
        ));
    }
    let json: String = connection
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [seq_to_i64(sequence)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary bounded event read: {error}")))?;
    if json.len() > AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary source event exceeds its encoded byte bound".into(),
        ));
    }
    let event: RuntimeEventEnvelope = decode(&json, "auxiliary event")?;
    if event.sequence != sequence
        || event.event_id.as_str() != stored_event_id.as_str()
        || i64::from(event.schema_version) != stored_schema_version
        || event.conversation_id.as_str() != stored_conversation_id.as_str()
        || event
            .attempt_id
            .as_ref()
            .map(crate::runtime::identity::AttemptId::as_str)
            != stored_attempt_id.as_deref()
        || event
            .turn_id
            .as_ref()
            .map(crate::runtime::identity::TurnId::as_str)
            != stored_turn_id.as_deref()
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary event payload disagrees with its Journal row".into(),
        ));
    }
    Ok(Some(event))
}

#[derive(Debug)]
struct AuditFactRow {
    sequence: u64,
    audit_offset: u64,
    kind: String,
    call_id: AuxiliaryCallId,
    request_id: Option<RequestId>,
    ordinal: Option<u32>,
}

fn audit_fact_row(
    connection: &Connection,
    sequence: u64,
) -> Result<Option<AuditFactRow>, ConversationStoreError> {
    let row: Option<(i64, i64, String, String, Option<String>, Option<i64>)> = connection
        .query_row(
            "SELECT sequence,audit_offset,kind,call_id,request_id,ordinal
             FROM auxiliary_audit_facts WHERE sequence=?1",
            [seq_to_i64(sequence)?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("auxiliary audit fact lookup: {error}")))?;
    row.map(
        |(sequence, audit_offset, kind, call_id, request_id, ordinal)| {
            Ok(AuditFactRow {
                sequence: sequence_from_i64(sequence)?,
                audit_offset: u64::try_from(audit_offset).map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary audit offset is negative".into(),
                    )
                })?,
                kind,
                call_id: AuxiliaryCallId::parse(call_id)
                    .map_err(ConversationStoreError::InvalidReference)?,
                request_id: request_id.map(RequestId::new),
                ordinal: ordinal
                    .map(|value| {
                        u32::try_from(value).map_err(|_| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary audit ordinal is out of range".into(),
                            )
                        })
                    })
                    .transpose()?,
            })
        },
    )
    .transpose()
}

fn validate_audit_fact_event(
    event: &RuntimeEventEnvelope,
    row: &AuditFactRow,
) -> Result<(), ConversationStoreError> {
    if event.sequence != row.sequence {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary audit sequence disagrees with its Journal fact".into(),
        ));
    }
    match (&event.event, row.kind.as_str()) {
        (
            RuntimeEvent::AuxiliaryCallStarted { call_id, .. }
            | RuntimeEvent::AuxiliaryCallSettled { call_id, .. },
            "call_started" | "call_settled",
        ) => {
            let expected_kind = if matches!(event.event, RuntimeEvent::AuxiliaryCallStarted { .. })
            {
                "call_started"
            } else {
                "call_settled"
            };
            if row.kind != expected_kind
                || &row.call_id != call_id
                || row.request_id.is_some()
                || row.ordinal.is_some()
                || event.event_id
                    != if expected_kind == "call_started" {
                        call_started_event_id(call_id)
                    } else {
                        call_settled_event_id(call_id)
                    }
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call audit payload disagrees with its Journal fact".into(),
                ));
            }
        }
        (
            RuntimeEvent::ModelRequestStarted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                ..
            },
            "request_started",
        ) => {
            if &row.call_id != call_id
                || row.request_id.as_ref() != Some(request_id)
                || row.ordinal != Some(*ordinal)
                || event.event_id != request_started_event_id(request_id)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request-start audit payload disagrees with its Journal fact".into(),
                ));
            }
        }
        (
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                ..
            }
            | RuntimeEvent::ModelRequestFailed {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                ..
            },
            "request_settled",
        ) => {
            let completed = matches!(event.event, RuntimeEvent::ModelRequestCompleted { .. });
            if &row.call_id != call_id
                || row.request_id.as_ref() != Some(request_id)
                || row.ordinal != Some(*ordinal)
                || event.event_id != request_terminal_event_id(request_id, completed)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request-terminal audit payload disagrees with its Journal fact"
                        .into(),
                ));
            }
        }
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit kind disagrees with its Journal fact".into(),
            ));
        }
    }
    if row.audit_offset == 0 {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary audit offset must be positive".into(),
        ));
    }
    Ok(())
}

fn require_audit_fact(
    connection: &Connection,
    sequence: u64,
    kind: &str,
    call_id: &AuxiliaryCallId,
    request_id: Option<&RequestId>,
    ordinal: Option<u32>,
) -> Result<u64, ConversationStoreError> {
    let row = audit_fact_row(connection, sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary {kind} evidence is missing from its transactional audit index"
        ))
    })?;
    if row.kind != kind
        || &row.call_id != call_id
        || row.request_id.as_ref() != request_id
        || row.ordinal != ordinal
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary {kind} audit payload disagrees with its mandatory identity"
        )));
    }
    let event = event_at_connection(connection, sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary {kind} audit row has no Journal fact"
        ))
    })?;
    validate_audit_fact_event(&event, &row)?;
    Ok(row.audit_offset)
}

fn request_id_from_event(event: &RuntimeEvent) -> Option<&RequestId> {
    match event {
        RuntimeEvent::ModelRequestStarted { request_id, .. }
        | RuntimeEvent::ModelRequestCompleted { request_id, .. }
        | RuntimeEvent::ModelRequestFailed { request_id, .. } => Some(request_id),
        _ => None,
    }
}

fn validate_request_terminal_for_row(
    conversation_id: &ConversationId,
    request: &RequestRow,
    terminal: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    validate_terminal_envelope_shape(terminal, conversation_id, false)?;
    let (owner, request_id, kind) = match &terminal.event {
        RuntimeEvent::ModelRequestCompleted {
            owner, request_id, ..
        } => (owner, request_id, AuxiliaryRequestTerminal::Completed),
        RuntimeEvent::ModelRequestFailed {
            owner, request_id, ..
        } => (owner, request_id, AuxiliaryRequestTerminal::Failed),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary request terminal names a non-terminal event".into(),
            ));
        }
    };
    if request_id != &request.request_id
        || owner
            != &(ModelRequestOwner::Auxiliary {
                call_id: request.call_id.clone(),
                ordinal: request.ordinal,
            })
        || request.terminal_kind != Some(kind)
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request terminal disagrees with its request index".into(),
        ));
    }
    Ok(())
}

fn validate_owner_row_tx(
    transaction: &Connection,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    indexed: &OwnerRow,
) -> Result<(), ConversationStoreError> {
    let start = event_at_connection(transaction, indexed.start_sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "request {request_id} owner index has no authoritative start fact"
        ))
    })?;
    if start.conversation_id != *conversation_id
        || request_id_from_event(&start.event) != Some(request_id)
        || start.event.model_request_owner() != Some(&indexed.owner)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} owner index disagrees with its start fact"
        )));
    }
    if let ModelRequestOwner::Auxiliary { .. } = indexed.owner
        && start.event_id != request_started_event_id(request_id)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} start has a noncanonical event identity"
        )));
    }
    match (indexed.terminal_sequence, indexed.terminal_kind) {
        (None, None) => {}
        (Some(sequence), Some(kind)) => {
            let terminal = event_at_connection(transaction, sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "request {request_id} owner terminal index has no Journal fact"
                ))
            })?;
            if terminal.conversation_id != *conversation_id
                || request_id_from_event(&terminal.event) != Some(request_id)
                || terminal.event.model_request_owner() != Some(&indexed.owner)
                || terminal_kind(&terminal.event) != Some(kind)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request {request_id} owner terminal disagrees with its Journal fact"
                )));
            }
            if let ModelRequestOwner::Auxiliary { call_id, ordinal } = &indexed.owner {
                require_audit_fact(
                    transaction,
                    sequence,
                    "request_settled",
                    call_id,
                    Some(request_id),
                    Some(*ordinal),
                )?;
            }
        }
        _ => {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request {request_id} owner terminal sequence/kind pair is incomplete"
            )));
        }
    }
    if let ModelRequestOwner::Auxiliary { call_id, ordinal } = &indexed.owner {
        require_audit_fact(
            transaction,
            indexed.start_sequence,
            "request_started",
            call_id,
            Some(request_id),
            Some(*ordinal),
        )?;
    }
    Ok(())
}

fn request_snapshot_started_sequence(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<u64>, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT started_sequence FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map_err(|error| storage(format!("request snapshot start lookup: {error}")))?
        .flatten()
        .map(sequence_from_i64)
        .transpose()
}

pub(super) fn load_request_owner_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
) -> Result<Option<ModelRequestOwner>, ConversationStoreError> {
    if let Some(indexed) = owner_row_tx(transaction, request_id)? {
        match &indexed.owner {
            ModelRequestOwner::Agent { .. } => {
                // Preserve the primary native Snapshot validator and its indexed
                // duplicate-fact probe; auxiliary encoded limits do not apply here.
                if super::request::snapshot::find_request_start_event(transaction, request_id)?
                    .is_none()
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "request {request_id} has Journal evidence without its request-start owner"
                    )));
                }
                let (_, started) = super::request::snapshot::verified_started_request_tx(
                    transaction,
                    conversation_id,
                    request_id,
                )?;
                if started.sequence != indexed.start_sequence
                    || started.event.model_request_owner() != Some(&indexed.owner)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "Agent request {request_id} owner start disagrees with its Snapshot index"
                    )));
                }
                let terminal =
                    super::request::snapshot::find_request_outcome_event(transaction, request_id)?;
                if terminal.as_ref().map(|event| event.sequence) != indexed.terminal_sequence
                    || terminal
                        .as_ref()
                        .and_then(|event| terminal_kind(&event.event))
                        != indexed.terminal_kind
                    || terminal.as_ref().is_some_and(|event| {
                        event.event.model_request_owner() != Some(&indexed.owner)
                    })
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "Agent request {request_id} owner terminal disagrees with its Journal fact"
                    )));
                }
            }
            ModelRequestOwner::Auxiliary { call_id, ordinal } => {
                validate_owner_row_tx(transaction, conversation_id, request_id, &indexed)?;
                let request = request_row_tx(transaction, request_id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} has an owner but no request index"
                    ))
                })?;
                let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} has no Call authority"
                    ))
                })?;
                if request.call_id != *call_id
                    || request.ordinal != *ordinal
                    || request.started_sequence != indexed.start_sequence
                    || call.started_sequence >= request.started_sequence
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} indexes disagree with its authority"
                    )));
                }
                let _ = request_audit_at_tx(
                    transaction,
                    conversation_id,
                    request_id,
                    AuxiliaryAuditCut {
                        journal: journal_prefix(transaction)?,
                    },
                )?;
            }
        }
        return Ok(Some(indexed.owner));
    }
    if super::request::snapshot::has_request_journal_fact_tx(transaction, request_id)?
        || request_snapshot_started_sequence(transaction, request_id)?.is_some()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} has durable evidence without its request-start owner"
        )));
    }
    Ok(None)
}

pub(super) fn index_event_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &event.event {
        RuntimeEvent::ModelRequestStarted {
            owner, request_id, ..
        } => {
            insert_owner_tx(transaction, request_id, owner, event.sequence)?;
        }
        RuntimeEvent::ModelRequestCompleted {
            owner, request_id, ..
        }
        | RuntimeEvent::ModelRequestFailed {
            owner, request_id, ..
        } => {
            index_terminal_tx(transaction, event, request_id, owner)?;
        }
        RuntimeEvent::AuxiliaryCallStarted { call_id, .. } => {
            bounded_encoded_len(
                event,
                AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
                "auxiliary Call start event",
            )?;
            transaction
                .execute(
                    "INSERT INTO auxiliary_calls(
                        call_id,started_sequence,settled_sequence,outcome
                     ) VALUES(?1,?2,NULL,NULL)",
                    params![call_id.as_str(), seq_to_i64(event.sequence)?],
                )
                .map_err(|error| storage(format!("index auxiliary Call start: {error}")))?;
            insert_audit_fact_tx(
                transaction,
                event.sequence,
                "call_started",
                call_id,
                None,
                None,
            )?;
        }
        RuntimeEvent::AuxiliaryCallSettled {
            call_id, outcome, ..
        } => {
            let updated = transaction
                .execute(
                    "UPDATE auxiliary_calls SET settled_sequence=?1,outcome=?2
                     WHERE call_id=?3 AND settled_sequence IS NULL",
                    params![
                        seq_to_i64(event.sequence)?,
                        outcome_name(*outcome),
                        call_id.as_str(),
                    ],
                )
                .map_err(|error| storage(format!("index auxiliary Call settlement: {error}")))?;
            if updated != 1 {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "auxiliary Call {call_id} already has a settlement"
                )));
            }
            insert_audit_fact_tx(
                transaction,
                event.sequence,
                "call_settled",
                call_id,
                None,
                None,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn insert_owner_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
    owner: &ModelRequestOwner,
    start_sequence: u64,
) -> Result<(), ConversationStoreError> {
    match owner {
        ModelRequestOwner::Agent { identity } => {
            transaction
                .execute(
                    "INSERT INTO request_owners(
                        request_id,owner_kind,attempt_id,turn_id,retry_number,
                        call_id,auxiliary_ordinal,start_sequence,terminal_sequence,terminal_kind
                     ) VALUES(?1,'agent',?2,?3,?4,NULL,NULL,?5,NULL,NULL)",
                    params![
                        request_id.as_str(),
                        identity.attempt_id.as_str(),
                        identity.turn.as_str(),
                        i64::from(identity.retry_number),
                        seq_to_i64(start_sequence)?,
                    ],
                )
                .map_err(|error| storage(format!("index Agent request owner: {error}")))?;
        }
        ModelRequestOwner::Auxiliary { call_id, ordinal } => {
            if request_id.as_str().is_empty()
                || request_id.as_str().len() > AUXILIARY_REQUEST_ID_BYTES_MAX
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary RequestId must contain 1..={AUXILIARY_REQUEST_ID_BYTES_MAX} bytes"
                )));
            }
            transaction
                .execute(
                    "INSERT INTO request_owners(
                        request_id,owner_kind,attempt_id,turn_id,retry_number,
                        call_id,auxiliary_ordinal,start_sequence,terminal_sequence,terminal_kind
                     ) VALUES(?1,'auxiliary',NULL,NULL,NULL,?2,?3,?4,NULL,NULL)",
                    params![
                        request_id.as_str(),
                        call_id.as_str(),
                        i64::from(*ordinal),
                        seq_to_i64(start_sequence)?,
                    ],
                )
                .map_err(|error| storage(format!("index auxiliary request owner: {error}")))?;
        }
    }
    Ok(())
}

fn index_terminal_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
    request_id: &RequestId,
    owner: &ModelRequestOwner,
) -> Result<(), ConversationStoreError> {
    let kind = terminal_kind(&event.event).expect("terminal shape checked");
    let indexed = owner_row_tx(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "request {request_id} terminal has no authoritative owner"
        ))
    })?;
    if indexed.owner != *owner || indexed.terminal_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "request {request_id} terminal conflicts with its authoritative owner state"
        )));
    }
    let updated = transaction
        .execute(
            "UPDATE request_owners
             SET terminal_sequence=?1,terminal_kind=?2
             WHERE request_id=?3 AND terminal_sequence IS NULL",
            params![seq_to_i64(event.sequence)?, kind, request_id.as_str(),],
        )
        .map_err(|error| storage(format!("index request terminal: {error}")))?;
    if updated != 1 {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "request {request_id} already has a terminal"
        )));
    }
    if let ModelRequestOwner::Auxiliary { call_id, ordinal } = owner {
        let request = request_row_tx(transaction, request_id)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "auxiliary request {request_id} terminal has no request index"
            ))
        })?;
        if request.call_id != *call_id || request.ordinal != *ordinal {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary request {request_id} terminal disagrees with its request index"
            )));
        }
        let updated = transaction
            .execute(
                "UPDATE auxiliary_requests
                 SET terminal_sequence=?1,terminal_kind=?2
                 WHERE request_id=?3 AND terminal_sequence IS NULL",
                params![seq_to_i64(event.sequence)?, kind, request_id.as_str(),],
            )
            .map_err(|error| storage(format!("index auxiliary request terminal: {error}")))?;
        if updated != 1 {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "auxiliary request {request_id} already has a terminal"
            )));
        }
        insert_audit_fact_tx(
            transaction,
            event.sequence,
            "request_settled",
            call_id,
            Some(request_id),
            Some(*ordinal),
        )?;
    }
    Ok(())
}

fn next_audit_offset(transaction: &Transaction<'_>) -> Result<u64, ConversationStoreError> {
    let maximum: i64 = transaction
        .query_row(
            "SELECT coalesce(max(audit_offset),0) FROM auxiliary_audit_facts
             INDEXED BY auxiliary_audit_offset_idx",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary audit frontier lookup: {error}")))?;
    let maximum =
        u64::try_from(maximum).map_err(|_| storage("auxiliary audit frontier is negative"))?;
    maximum
        .checked_add(1)
        .ok_or_else(|| storage("auxiliary audit offset exhausted"))
}

fn insert_audit_fact_tx(
    transaction: &Transaction<'_>,
    sequence: u64,
    kind: &str,
    call_id: &AuxiliaryCallId,
    request_id: Option<&RequestId>,
    ordinal: Option<u32>,
) -> Result<u64, ConversationStoreError> {
    let audit_offset = next_audit_offset(transaction)?;
    transaction
        .execute(
            "INSERT INTO auxiliary_audit_facts(
                sequence,audit_offset,kind,call_id,request_id,ordinal
             ) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                seq_to_i64(sequence)?,
                i64::try_from(audit_offset)
                    .map_err(|_| storage("auxiliary audit offset overflow"))?,
                kind,
                call_id.as_str(),
                request_id.map(RequestId::as_str),
                ordinal.map(i64::from),
            ],
        )
        .map_err(|error| storage(format!("insert auxiliary audit fact: {error}")))?;
    Ok(audit_offset)
}

fn has_call_journal_fact_through_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
    through: u64,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_auxiliary_call_id_kind_idx
               WHERE json_extract(event_json,'$.event.call_id')=?1
                 AND json_extract(event_json,'$.event.type') IN
                     ('auxiliary_call_started','auxiliary_call_settled')
                 AND sequence<=?2
             )",
            params![call_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary Call Journal probe: {error}")))
}

fn has_call_journal_fact_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<bool, ConversationStoreError> {
    has_call_journal_fact_through_tx(transaction, call_id, journal_prefix(transaction)?)
}

fn has_request_journal_fact_through_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
    through: u64,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_request_id_kind_idx
               WHERE json_extract(event_json,'$.event.request_id')=?1
                 AND sequence<=?2
             )",
            params![request_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary request Journal probe: {error}")))
}

fn has_request_terminal_journal_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_request_id_kind_idx
               WHERE json_extract(event_json,'$.event.request_id')=?1
                 AND json_extract(event_json,'$.event.type') IN
                     ('model_request_completed','model_request_failed')
             )",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary request terminal probe: {error}")))
}

fn has_call_settlement_journal_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_auxiliary_call_id_kind_idx
               WHERE json_extract(event_json,'$.event.call_id')=?1
                 AND json_extract(event_json,'$.event.type')='auxiliary_call_settled'
             )",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary Call settlement probe: {error}")))
}

fn call_audit_at_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    call_id: &AuxiliaryCallId,
    cut: AuxiliaryAuditCut,
) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError> {
    let Some(call) = call_row_query(connection, call_id)? else {
        return Ok(None);
    };
    if call.started_sequence > cut.journal {
        return Ok(None);
    }
    let start = event_at_connection(connection, call.started_sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference("auxiliary Call index has no start event".into())
    })?;
    if start.conversation_id != *conversation_id || start.event_id != call_started_event_id(call_id)
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call start belongs to a foreign or noncanonical event".into(),
        ));
    }
    let (
        purpose,
        trigger,
        business_source,
        input,
        semantic_input_fingerprint,
        invocation_fingerprint,
        selected_message_ids,
    ) = match &start.event {
        RuntimeEvent::AuxiliaryCallStarted {
            call_id: event_call,
            purpose,
            trigger,
            business_source,
            input,
            semantic_input_fingerprint,
            invocation_fingerprint,
            selected_message_ids,
        } if event_call == call_id => (
            purpose.clone(),
            trigger.clone(),
            business_source.clone(),
            input.clone(),
            semantic_input_fingerprint.clone(),
            invocation_fingerprint.clone(),
            selected_message_ids.clone(),
        ),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call index names a non-Call-start event".into(),
            ));
        }
    };
    let start_projection = AuxiliaryCallStart {
        call_id: call_id.clone(),
        purpose: purpose.clone(),
        trigger: trigger.clone(),
        business_source: business_source.clone(),
        input: input.clone(),
        semantic_input_fingerprint: semantic_input_fingerprint.clone(),
        invocation_fingerprint: invocation_fingerprint.clone(),
        selected_message_ids: selected_message_ids.clone(),
    };
    validate_call_start_input(&start_projection)?;
    validate_call_start_authority_tx(connection, conversation_id, &start_projection)?;
    require_audit_fact(
        connection,
        call.started_sequence,
        "call_started",
        call_id,
        None,
        None,
    )?;

    let settled_visible = call
        .settled_sequence
        .filter(|sequence| *sequence <= cut.journal);
    let settlement_facts = connection
        .prepare_cached(
            "SELECT sequence FROM events INDEXED BY events_auxiliary_call_id_kind_idx
         WHERE json_extract(event_json,'$.event.call_id')=?1
           AND json_extract(event_json,'$.event.type')='auxiliary_call_settled'
           AND sequence<=?2 ORDER BY sequence LIMIT 2",
        )
        .map_err(|error| storage(error.to_string()))?
        .query_map(params![call_id.as_str(), seq_to_i64(cut.journal)?], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    if settlement_facts.len() > 1
        || settlement_facts
            .first()
            .copied()
            .map(sequence_from_i64)
            .transpose()?
            != settled_visible
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index disagrees with its captured Journal facts".into(),
        ));
    }
    let outcome = if let Some(sequence) = settled_visible {
        let settlement = event_at_connection(connection, sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index has no event".into(),
            )
        })?;
        validate_terminal_envelope_shape(&settlement, conversation_id, false)?;
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id: event_call,
            outcome: event_outcome,
            request_evidence,
        } = &settlement.event
        else {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index names another event kind".into(),
            ));
        };
        if event_call != call_id || call.outcome != Some(*event_outcome) {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index disagrees with its event".into(),
            ));
        }
        require_audit_fact(connection, sequence, "call_settled", call_id, None, None)?;
        if open_request_query(connection, call_id)?.is_some() {
            return Err(ConversationStoreError::InvalidReference(
                "a settled auxiliary Call cannot retain an open request".into(),
            ));
        }
        if request_evidence.is_none() && call_has_requests_query(connection, call_id)? {
            return Err(ConversationStoreError::InvalidReference(
                "a settled auxiliary Call with requests requires exact request evidence".into(),
            ));
        }
        if let Some(evidence) = request_evidence {
            let request =
                request_row_query(connection, "request_id=?1", [evidence.request_id.as_str()])?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call request evidence names a missing request".into(),
                        )
                    })?;
            if request.call_id != *call_id {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request evidence names another Call".into(),
                ));
            }
            let terminal_sequence = request
                .terminal_sequence
                .filter(|terminal| *terminal <= cut.journal)
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary Call request evidence has no visible request terminal".into(),
                    )
                })?;
            if call.started_sequence >= request.started_sequence
                || request.started_sequence >= terminal_sequence
                || terminal_sequence >= sequence
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call/request start and terminal order is invalid".into(),
                ));
            }
            let terminal =
                event_at_connection(connection, terminal_sequence)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary request terminal index has no Journal fact".into(),
                    )
                })?;
            validate_request_terminal_for_row(conversation_id, &request, &terminal)?;
            if !call_outcome_for_terminal(&terminal.event)
                .is_some_and(|outcomes| outcomes.contains(event_outcome))
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call outcome disagrees with its request terminal".into(),
                ));
            }
            require_audit_fact(
                connection,
                terminal_sequence,
                "request_settled",
                call_id,
                Some(&request.request_id),
                Some(request.ordinal),
            )?;
            let owner = owner_row_query(connection, &request.request_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary request terminal has no owner index".into(),
                )
            })?;
            validate_owner_row_tx(connection, conversation_id, &request.request_id, &owner)?;
            if owner.start_sequence != request.started_sequence
                || owner.terminal_sequence != Some(terminal_sequence)
                || owner.terminal_kind != terminal_kind(&terminal.event)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request owner and request rows disagree".into(),
                ));
            }
            validate_request_evidence(evidence, &terminal)?;
        }
        Some(*event_outcome)
    } else {
        None
    };
    if (call.settled_sequence.is_some() && call.outcome.is_none())
        || (call.settled_sequence.is_none() && call.outcome.is_some())
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement sequence/outcome pair is incomplete".into(),
        ));
    }
    Ok(Some(AuxiliaryCallAudit {
        call_id: call_id.clone(),
        purpose,
        trigger,
        business_source,
        input,
        semantic_input_fingerprint,
        invocation_fingerprint,
        selected_message_ids,
        started_sequence: call.started_sequence,
        settled_sequence: settled_visible,
        outcome,
    }))
}

fn request_evidence_at_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    request: &RequestRow,
    cut: AuxiliaryAuditCut,
    request_terminal: Option<&RuntimeEventEnvelope>,
) -> Result<Option<AuxiliaryRequestEvidence>, ConversationStoreError> {
    let Some(call) = call_row_query(connection, &request.call_id)? else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request has no Call index".into(),
        ));
    };
    let Some(sequence) = call
        .settled_sequence
        .filter(|sequence| *sequence <= cut.journal)
    else {
        return Ok(None);
    };
    let settlement = event_at_connection(connection, sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index has no Journal event".into(),
        )
    })?;
    validate_terminal_envelope_shape(&settlement, conversation_id, false)?;
    let RuntimeEvent::AuxiliaryCallSettled {
        call_id,
        outcome,
        request_evidence,
        ..
    } = &settlement.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index names a non-settlement event".into(),
        ));
    };
    if call_id != &request.call_id {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request and Call settlement name different Calls".into(),
        ));
    }
    if call.outcome != Some(*outcome) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index disagrees with its outcome".into(),
        ));
    }
    require_audit_fact(connection, sequence, "call_settled", call_id, None, None)?;
    let Some(evidence) = request_evidence else {
        return Ok(None);
    };
    let terminal = request_terminal.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary Call carries request evidence while its request has no terminal".into(),
        )
    })?;
    if !call_outcome_for_terminal(&terminal.event)
        .is_some_and(|outcomes| outcomes.contains(outcome))
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call outcome disagrees with its request terminal".into(),
        ));
    }
    validate_request_evidence(evidence, terminal)?;
    Ok(Some(evidence.clone()))
}

#[cfg(test)]
thread_local! {
    static AFTER_REQUEST_ROW_READ:
        std::cell::RefCell<Option<(RequestId, Box<dyn FnOnce()>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn install_after_request_row_read_hook(
    request_id: RequestId,
    hook: impl FnOnce() + 'static,
) {
    AFTER_REQUEST_ROW_READ.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a request-row read hook is already installed on this thread"
        );
        *slot.borrow_mut() = Some((request_id, Box::new(hook)));
    });
}

#[cfg(test)]
fn after_request_row_read(request_id: &RequestId) {
    let hook = AFTER_REQUEST_ROW_READ.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(installed_request_id, _)| installed_request_id == request_id)
        {
            slot.take().map(|(_, hook)| hook)
        } else {
            None
        }
    });
    if let Some(hook) = hook {
        hook();
    }
}

fn request_audit_at_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    cut: AuxiliaryAuditCut,
) -> Result<Option<AuxiliaryRequestAudit>, ConversationStoreError> {
    let request = request_row_query(connection, "request_id=?1", [request_id.as_str()])?;
    let Some(request) = request else {
        return Ok(None);
    };
    if request.started_sequence > cut.journal {
        return Ok(None);
    }
    #[cfg(test)]
    after_request_row_read(request_id);
    let call =
        call_audit_at_tx(connection, conversation_id, &request.call_id, cut)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary request has no visible Call authority".into(),
            )
        })?;
    if call.started_sequence >= request.started_sequence {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request start does not follow its Call start".into(),
        ));
    }
    let start = event_at_connection(connection, request.started_sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary request index has no start event".into(),
        )
    })?;
    let start_input = AuxiliaryRequestStart {
        call_id: request.call_id.clone(),
        ordinal: request.ordinal,
        request_id: request.request_id.clone(),
        model: request.model.clone(),
    };
    validate_request_start_input(&start_input)?;
    if start.conversation_id != *conversation_id || !request_start_matches(&start, &start_input) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request index disagrees with its start event".into(),
        ));
    }
    let owner = owner_row_query(connection, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary request has no authoritative owner index".into(),
        )
    })?;
    validate_owner_row_tx(connection, conversation_id, request_id, &owner)?;
    if owner.owner
        != (ModelRequestOwner::Auxiliary {
            call_id: request.call_id.clone(),
            ordinal: request.ordinal,
        })
        || owner.start_sequence != request.started_sequence
        || owner.terminal_sequence != request.terminal_sequence
        || owner.terminal_kind
            != request.terminal_kind.map(|kind| match kind {
                AuxiliaryRequestTerminal::Completed => "completed",
                AuxiliaryRequestTerminal::Failed => "failed",
            })
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request owner and request rows disagree".into(),
        ));
    }
    require_audit_fact(
        connection,
        request.started_sequence,
        "request_started",
        &request.call_id,
        Some(request_id),
        Some(request.ordinal),
    )?;

    let terminal_sequence = request
        .terminal_sequence
        .filter(|sequence| *sequence <= cut.journal);
    let mut request_terminal_event = None;
    let mut local_terminal = None;
    let mut normalized_failure_kind = None;
    let mut generation = None;
    let mut usage = None;
    if let Some(sequence) = terminal_sequence {
        let terminal = event_at_connection(connection, sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary request terminal index has no Journal event".into(),
            )
        })?;
        validate_request_terminal_for_row(conversation_id, &request, &terminal)?;
        match &terminal.event {
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id: event_request_id,
                usage: event_usage,
                generation: event_generation,
                ..
            } if call_id == &request.call_id
                && *ordinal == request.ordinal
                && event_request_id == request_id =>
            {
                local_terminal = Some(AuxiliaryRequestTerminal::Completed);
                generation = *event_generation;
                usage.clone_from(event_usage);
            }
            RuntimeEvent::ModelRequestFailed {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id: event_request_id,
                error,
                usage: event_usage,
                generation: event_generation,
            } if call_id == &request.call_id
                && *ordinal == request.ordinal
                && event_request_id == request_id =>
            {
                local_terminal = Some(AuxiliaryRequestTerminal::Failed);
                normalized_failure_kind = Some(error.kind.clone());
                generation = *event_generation;
                usage.clone_from(event_usage);
            }
            _ => {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request terminal disagrees with its start owner".into(),
                ));
            }
        }
        require_audit_fact(
            connection,
            sequence,
            "request_settled",
            &request.call_id,
            Some(request_id),
            Some(request.ordinal),
        )?;
        request_terminal_event = Some(terminal);
    }
    let request_evidence = request_evidence_at_tx(
        connection,
        conversation_id,
        &request,
        cut,
        request_terminal_event.as_ref(),
    )?;
    Ok(Some(AuxiliaryRequestAudit {
        call_id: request.call_id,
        ordinal: request.ordinal,
        request_id: request.request_id,
        model: request.model,
        started_sequence: request.started_sequence,
        terminal_sequence,
        local_terminal,
        normalized_failure_kind,
        generation,
        usage,
        request_evidence,
    }))
}

pub(super) const AUXILIARY_AUDIT_FRONTIER_SQL: &str =
    "SELECT sequence,audit_offset FROM auxiliary_audit_facts
     INDEXED BY auxiliary_audit_sequence_idx
     WHERE sequence<=?1 ORDER BY sequence DESC LIMIT 1";

pub(super) const AUXILIARY_CALL_REQUESTS_SQL: &str =
    "SELECT sequence,request_id,ordinal FROM auxiliary_audit_facts
     INDEXED BY auxiliary_audit_call_idx
     WHERE call_id=?1 AND sequence>?2 AND sequence<=?3 AND kind='request_started'
     ORDER BY sequence LIMIT ?4";

pub(super) const AUXILIARY_AUTHORITATIVE_FRONTIER_SQL: &str = "
SELECT max(sequence) FROM (
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='auxiliary_call_started'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='model_request_started'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='model_request_completed'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='model_request_failed'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='auxiliary_call_settled'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled')
      )
)";

fn audit_total_at_connection(
    connection: &Connection,
    cut: AuxiliaryAuditCut,
) -> Result<u64, ConversationStoreError> {
    let current = journal_prefix(connection)?;
    if cut.journal > current {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary audit cut is in the future".into(),
        ));
    }
    let journal = seq_to_i64(cut.journal)?;
    let derived: Option<(i64, i64)> = connection
        .query_row(AUXILIARY_AUDIT_FRONTIER_SQL, [journal], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(|error| storage(format!("auxiliary audit total lookup: {error}")))?;
    let authoritative: Option<i64> = connection
        .query_row(AUXILIARY_AUTHORITATIVE_FRONTIER_SQL, [journal], |row| {
            row.get(0)
        })
        .map_err(|error| storage(format!("auxiliary authoritative frontier: {error}")))?;
    let authoritative = authoritative.map(sequence_from_i64).transpose()?;
    match (derived, authoritative) {
        (None, None) => Ok(0),
        (Some((derived_sequence, audit_offset)), Some(authoritative_sequence))
            if sequence_from_i64(derived_sequence)? == authoritative_sequence =>
        {
            u64::try_from(audit_offset).map_err(|_| {
                ConversationStoreError::InvalidReference("auxiliary audit total is negative".into())
            })
        }
        _ => Err(ConversationStoreError::InvalidReference(
            "auxiliary audit frontier disagrees with authoritative Journal facts".into(),
        )),
    }
}

fn audit_record_bytes(record: &AuxiliaryAuditRecord) -> Result<(), ConversationStoreError> {
    bounded_encoded_len(
        record,
        AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
        "auxiliary audit record",
    )?;
    Ok(())
}

fn audit_fact_bytes(
    fact: AuxiliaryAuditFact,
    journal_sequence: u64,
) -> Result<(), ConversationStoreError> {
    audit_record_bytes(&AuxiliaryAuditRecord {
        audit_offset: u64::MAX,
        journal_sequence,
        fact,
    })
}

impl SqliteConversationStore {
    pub(super) fn commit_auxiliary_call_start(
        &self,
        start: &AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<AuxiliaryCallStartCommit, AuxiliaryWriteFailure> {
        validate_call_start_input(start).map_err(AuxiliaryWriteFailure::known)?;
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            if let Some(existing) = call_row_tx(transaction, &start.call_id)? {
                let event = event_at_connection(transaction, existing.started_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(format!(
                            "auxiliary Call {} has no start fact",
                            start.call_id
                        ))
                    })?;
                if !call_start_matches(&event, start)
                    || event.event_id != call_started_event_id(&start.call_id)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary Call {} was retried with a conflicting payload",
                        start.call_id
                    )));
                }
                require_audit_fact(
                    transaction,
                    event.sequence,
                    "call_started",
                    &start.call_id,
                    None,
                    None,
                )?;
                let call = call_audit_at_tx(
                    transaction,
                    &self.conversation_id,
                    &start.call_id,
                    AuxiliaryAuditCut {
                        journal: event.sequence,
                    },
                )?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "existing auxiliary Call is missing from its audit index".into(),
                    )
                })?;
                return Ok(AuxiliaryCallStartCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    call,
                    event,
                });
            }
            if has_call_journal_fact_tx(transaction, &start.call_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {} has Journal evidence without a start index",
                    start.call_id
                )));
            }
            let event = RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: call_started_event_id(&start.call_id),
                sequence: 0,
                conversation_id: self.conversation_id.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp,
                event: RuntimeEvent::AuxiliaryCallStarted {
                    call_id: start.call_id.clone(),
                    purpose: start.purpose.clone(),
                    trigger: start.trigger.clone(),
                    business_source: start.business_source.clone(),
                    input: start.input.clone(),
                    semantic_input_fingerprint: start.semantic_input_fingerprint.clone(),
                    invocation_fingerprint: start.invocation_fingerprint.clone(),
                    selected_message_ids: start.selected_message_ids.clone(),
                },
            };
            let persisted = persist_event_tx(transaction, &self.conversation_id, event)?;
            let cut = AuxiliaryAuditCut {
                journal: persisted.event.sequence,
            };
            let call = call_audit_at_tx(transaction, &self.conversation_id, &start.call_id, cut)?
                .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "committed auxiliary Call is missing from its audit index".into(),
                )
            })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::CallStarted(call.clone()),
                persisted.event.sequence,
            )?;
            Ok(AuxiliaryCallStartCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                call,
                event: persisted.event,
            })
        })
    }

    pub(super) fn commit_auxiliary_request_start(
        &self,
        start: &AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<AuxiliaryRequestStartCommit, AuxiliaryWriteFailure> {
        validate_request_start_input(start).map_err(AuxiliaryWriteFailure::known)?;
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            if let Some(existing) = request_row_tx(transaction, &start.request_id)? {
                let event = event_at_connection(transaction, existing.started_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(format!(
                            "auxiliary request {} has no start fact",
                            start.request_id
                        ))
                    })?;
                if !request_start_matches(&event, start)
                    || event.event_id != request_started_event_id(&start.request_id)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {} was retried with a conflicting payload",
                        start.request_id
                    )));
                }
                require_audit_fact(
                    transaction,
                    event.sequence,
                    "request_started",
                    &start.call_id,
                    Some(&start.request_id),
                    Some(start.ordinal),
                )?;
                let indexed = owner_row_tx(transaction, &start.request_id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {} is missing its owner index",
                        start.request_id
                    ))
                })?;
                if indexed.owner
                    != (ModelRequestOwner::Auxiliary {
                        call_id: start.call_id.clone(),
                        ordinal: start.ordinal,
                    })
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {} owner index conflicts with its start",
                        start.request_id
                    )));
                }
                let request = request_audit_at_tx(
                    transaction,
                    &self.conversation_id,
                    &start.request_id,
                    AuxiliaryAuditCut {
                        journal: event.sequence,
                    },
                )?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "existing auxiliary request is missing from its audit index".into(),
                    )
                })?;
                return Ok(AuxiliaryRequestStartCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    request,
                    event,
                });
            }
            if super::request::snapshot::has_request_journal_fact_tx(
                transaction,
                &start.request_id,
            )? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {} has Journal evidence without a request-start index",
                    start.request_id
                )));
            }
            let event = RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: request_started_event_id(&start.request_id),
                sequence: 0,
                conversation_id: self.conversation_id.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp,
                event: RuntimeEvent::ModelRequestStarted {
                    owner: ModelRequestOwner::Auxiliary {
                        call_id: start.call_id.clone(),
                        ordinal: start.ordinal,
                    },
                    request_id: start.request_id.clone(),
                    model: start.model.clone(),
                },
            };
            let persisted = persist_event_tx(transaction, &self.conversation_id, event)?;
            transaction
                .execute(
                    "INSERT INTO auxiliary_requests(
                        call_id,ordinal,request_id,model,started_sequence,
                        terminal_sequence,terminal_kind
                     ) VALUES(?1,?2,?3,?4,?5,NULL,NULL)",
                    params![
                        start.call_id.as_str(),
                        i64::from(start.ordinal),
                        start.request_id.as_str(),
                        start.model.as_str(),
                        seq_to_i64(persisted.event.sequence)?,
                    ],
                )
                .map_err(|error| storage(format!("index auxiliary request start: {error}")))?;
            insert_audit_fact_tx(
                transaction,
                persisted.event.sequence,
                "request_started",
                &start.call_id,
                Some(&start.request_id),
                Some(start.ordinal),
            )?;
            let cut = AuxiliaryAuditCut {
                journal: persisted.event.sequence,
            };
            let request =
                request_audit_at_tx(transaction, &self.conversation_id, &start.request_id, cut)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "committed auxiliary request is missing from its audit index".into(),
                        )
                    })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::RequestStarted(request.clone()),
                persisted.event.sequence,
            )?;
            Ok(AuxiliaryRequestStartCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                request,
                event: persisted.event,
            })
        })
    }

    pub(super) fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure> {
        validate_settlement_pair(
            &self.conversation_id,
            &request_terminal,
            &call_settled,
            true,
        )
        .map_err(AuxiliaryWriteFailure::known)?;
        let (request_id, call_id) = settlement_ids(&request_terminal, &call_settled)
            .map_err(AuxiliaryWriteFailure::known)?;
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            let request = request_row_tx(transaction, &request_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary request settlement names unstarted request {request_id}"
                ))
            })?;
            let call = call_row_tx(transaction, &call_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary request settlement names unstarted Call {call_id}"
                ))
            })?;
            if let (Some(request_sequence), Some(call_sequence)) =
                (request.terminal_sequence, call.settled_sequence)
            {
                let stored_request = event_at_connection(transaction, request_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary request terminal index has no Journal fact".into(),
                        )
                    })?;
                let stored_call =
                    event_at_connection(transaction, call_sequence)?.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call settlement index has no Journal fact".into(),
                        )
                    })?;
                if !same_terminal_envelope(&stored_request, &request_terminal)
                    || !same_terminal_envelope(&stored_call, &call_settled)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} settlement was retried with a conflicting payload"
                    )));
                }
                let cut = AuxiliaryAuditCut {
                    journal: journal_prefix(transaction)?,
                };
                let request_audit =
                    request_audit_at_tx(transaction, &self.conversation_id, &request_id, cut)?;
                let call_audit =
                    call_audit_at_tx(transaction, &self.conversation_id, &call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "existing auxiliary Call is missing from its audit index".into(),
                            )
                        })?;
                return Ok(AuxiliarySettlementCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    request: request_audit,
                    call: call_audit,
                    request_terminal: Some(stored_request),
                    call_settled: stored_call,
                });
            }
            if request.terminal_sequence.is_some() || call.settled_sequence.is_some() {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "auxiliary request {request_id} has a partial settlement"
                )));
            }
            let persisted_request =
                persist_event_tx(transaction, &self.conversation_id, request_terminal)?;
            let persisted_call =
                persist_event_tx(transaction, &self.conversation_id, call_settled)?;
            let cut = AuxiliaryAuditCut {
                journal: persisted_call.event.sequence,
            };
            let request_audit =
                request_audit_at_tx(transaction, &self.conversation_id, &request_id, cut)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "committed auxiliary request is missing from its audit index".into(),
                        )
                    })?;
            let call_audit = call_audit_at_tx(transaction, &self.conversation_id, &call_id, cut)?
                .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "committed auxiliary Call is missing from its audit index".into(),
                )
            })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::RequestSettled(request_audit.clone()),
                persisted_request.event.sequence,
            )?;
            audit_fact_bytes(
                AuxiliaryAuditFact::CallSettled(call_audit.clone()),
                persisted_call.event.sequence,
            )?;
            Ok(AuxiliarySettlementCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                request: Some(request_audit),
                call: call_audit,
                request_terminal: Some(persisted_request.event),
                call_settled: persisted_call.event,
            })
        })
    }

    pub(super) fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure> {
        validate_terminal_envelope_shape(&call_settled, &self.conversation_id, true)
            .map_err(AuxiliaryWriteFailure::known)?;
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            request_evidence,
            ..
        } = &call_settled.event
        else {
            return Err(AuxiliaryWriteFailure::known(
                ConversationStoreError::InvalidReference(
                    "Call-only settlement requires AuxiliaryCallSettled".into(),
                ),
            ));
        };
        if request_evidence.is_some() {
            return Err(AuxiliaryWriteFailure::known(
                ConversationStoreError::InvalidReference(
                    "Call-only settlement cannot carry request evidence".into(),
                ),
            ));
        }
        let settled_call_id = call_id.clone();
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            let call = call_row_tx(transaction, &settled_call_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {settled_call_id} settled without a start"
                ))
            })?;
            if call_has_requests_query(transaction, &settled_call_id)? {
                return Err(ConversationStoreError::InvalidReference(
                    "Call-only reconciliation requires a zero-request auxiliary Call".into(),
                ));
            }
            if let Some(sequence) = call.settled_sequence {
                let stored = event_at_connection(transaction, sequence)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary Call settlement index has no Journal fact".into(),
                    )
                })?;
                if !same_terminal_envelope(&stored, &call_settled) {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary Call {settled_call_id} was retried with a conflicting settlement"
                    )));
                }
                let cut = AuxiliaryAuditCut {
                    journal: journal_prefix(transaction)?,
                };
                let call =
                    call_audit_at_tx(transaction, &self.conversation_id, &settled_call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "existing auxiliary Call is missing from its audit index".into(),
                            )
                        })?;
                return Ok(AuxiliarySettlementCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    request: None,
                    call,
                    request_terminal: None,
                    call_settled: stored,
                });
            }
            let persisted = persist_event_tx(transaction, &self.conversation_id, call_settled)?;
            let cut = AuxiliaryAuditCut {
                journal: persisted.event.sequence,
            };
            let call = call_audit_at_tx(transaction, &self.conversation_id, &settled_call_id, cut)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "committed auxiliary Call is missing from its audit index".into(),
                    )
                })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::CallSettled(call.clone()),
                persisted.event.sequence,
            )?;
            Ok(AuxiliarySettlementCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                request: None,
                call,
                request_terminal: None,
                call_settled: persisted.event,
            })
        })
    }
}

fn settlement_ids(
    request_terminal: &RuntimeEventEnvelope,
    call_settled: &RuntimeEventEnvelope,
) -> Result<(RequestId, AuxiliaryCallId), ConversationStoreError> {
    let (RuntimeEvent::ModelRequestCompleted { request_id, .. }
    | RuntimeEvent::ModelRequestFailed { request_id, .. }) = &request_terminal.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement requires a request terminal".into(),
        ));
    };
    let RuntimeEvent::AuxiliaryCallSettled { call_id, .. } = &call_settled.event else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement requires a Call terminal".into(),
        ));
    };
    Ok((request_id.clone(), call_id.clone()))
}

impl SqliteConversationStore {
    pub(super) fn confirm_auxiliary_call_start(
        &self,
        expected: &AuxiliaryCallStart,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliaryCallStartCommit>, ConversationStoreError> {
        validate_call_start_input(expected)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary Call start: {error}")))?;
        let Some(call) = call_row_query(&transaction, &expected.call_id)? else {
            if has_call_journal_fact_tx(&transaction, &expected.call_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {} has Journal evidence without its start index",
                    expected.call_id
                )));
            }
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let event = event_at_connection(&transaction, call.started_sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary Call index has no authoritative start".into(),
            )
        })?;
        if !call_start_matches(&event, expected)
            || event.event_id != call_started_event_id(&expected.call_id)
        {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary Call {} confirmation conflicts with its committed payload",
                expected.call_id
            )));
        }
        let audit = call_audit_at_tx(
            &transaction,
            &self.conversation_id,
            &expected.call_id,
            AuxiliaryAuditCut {
                journal: event.sequence,
            },
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "existing auxiliary Call is missing from its audit index".into(),
            )
        })?;
        Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
            AuxiliaryCallStartCommit {
                disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                call: audit,
                event,
            },
        ))
    }

    pub(super) fn confirm_auxiliary_request_start(
        &self,
        expected: &AuxiliaryRequestStart,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliaryRequestStartCommit>, ConversationStoreError>
    {
        validate_request_start_input(expected)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary request start: {error}")))?;
        let Some(request) = request_row_tx(&transaction, &expected.request_id)? else {
            if super::request::snapshot::has_request_journal_fact_tx(
                &transaction,
                &expected.request_id,
            )? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {} has Journal evidence without its start index",
                    expected.request_id
                )));
            }
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let event =
            event_at_connection(&transaction, request.started_sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary request index has no authoritative start".into(),
                )
            })?;
        if !request_start_matches(&event, expected)
            || event.event_id != request_started_event_id(&expected.request_id)
        {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary request {} confirmation conflicts with its committed payload",
                expected.request_id
            )));
        }
        let audit = request_audit_at_tx(
            &transaction,
            &self.conversation_id,
            &expected.request_id,
            AuxiliaryAuditCut {
                journal: event.sequence,
            },
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "existing auxiliary request is missing from its audit index".into(),
            )
        })?;
        Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
            AuxiliaryRequestStartCommit {
                disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                request: audit,
                event,
            },
        ))
    }

    pub(super) fn confirm_auxiliary_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliarySettlementCommit>, ConversationStoreError>
    {
        validate_settlement_pair(&self.conversation_id, request_terminal, call_settled, false)?;
        let (request_id, call_id) = settlement_ids(request_terminal, call_settled)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary settlement: {error}")))?;
        let request = request_row_tx(&transaction, &request_id)?;
        let call = call_row_tx(&transaction, &call_id)?;
        match (
            request.and_then(|request| request.terminal_sequence),
            call.and_then(|call| call.settled_sequence),
        ) {
            (None, None) => {
                if has_request_terminal_journal_tx(&transaction, &request_id)?
                    || has_call_settlement_journal_tx(&transaction, &call_id)?
                {
                    return Err(ConversationStoreError::InvalidReference(
                        "auxiliary settlement Journal evidence lacks its transactional indexes"
                            .into(),
                    ));
                }
                Ok(AuxiliaryCommitConfirmation::Unknown)
            }
            (Some(request_sequence), Some(call_sequence)) => {
                let stored_request = event_at_connection(&transaction, request_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary request settlement index has no Journal fact".into(),
                        )
                    })?;
                let stored_call =
                    event_at_connection(&transaction, call_sequence)?.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call settlement index has no Journal fact".into(),
                        )
                    })?;
                validate_terminal_envelope_shape(&stored_request, &self.conversation_id, false)?;
                validate_terminal_envelope_shape(&stored_call, &self.conversation_id, false)?;
                if !same_terminal_envelope(&stored_request, request_terminal)
                    || !same_terminal_envelope(&stored_call, call_settled)
                {
                    return Err(ConversationStoreError::InvalidReference(
                        "auxiliary settlement confirmation conflicts with committed facts".into(),
                    ));
                }
                let cut = AuxiliaryAuditCut {
                    journal: journal_prefix(&transaction)?,
                };
                let request_audit =
                    request_audit_at_tx(&transaction, &self.conversation_id, &request_id, cut)?;
                let call_audit =
                    call_audit_at_tx(&transaction, &self.conversation_id, &call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "existing auxiliary Call is missing from its audit index".into(),
                            )
                        })?;
                Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
                    AuxiliarySettlementCommit {
                        disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                        request: request_audit,
                        call: call_audit,
                        request_terminal: Some(stored_request),
                        call_settled: stored_call,
                    },
                ))
            }
            _ => Err(ConversationStoreError::InvalidReference(
                "auxiliary settlement has partial durable evidence".into(),
            )),
        }
    }

    pub(super) fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliarySettlementCommit>, ConversationStoreError>
    {
        validate_terminal_envelope_shape(call_settled, &self.conversation_id, false)?;
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            request_evidence,
            ..
        } = &call_settled.event
        else {
            return Err(ConversationStoreError::InvalidReference(
                "Call-only confirmation requires AuxiliaryCallSettled".into(),
            ));
        };
        if request_evidence.is_some() {
            return Err(ConversationStoreError::InvalidReference(
                "Call-only confirmation cannot carry request evidence".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary Call settlement: {error}")))?;
        let Some(call) = call_row_tx(&transaction, call_id)? else {
            if has_call_journal_fact_tx(&transaction, call_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} has Journal evidence without its start index"
                )));
            }
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let Some(sequence) = call.settled_sequence else {
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let stored = event_at_connection(&transaction, sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index has no Journal fact".into(),
            )
        })?;
        validate_terminal_envelope_shape(&stored, &self.conversation_id, false)?;
        if !same_terminal_envelope(&stored, call_settled) {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call settlement confirmation conflicts with committed facts".into(),
            ));
        }
        let audit = call_audit_at_tx(
            &transaction,
            &self.conversation_id,
            call_id,
            AuxiliaryAuditCut {
                journal: journal_prefix(&transaction)?,
            },
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "existing auxiliary Call is missing from its audit index".into(),
            )
        })?;
        Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
            AuxiliarySettlementCommit {
                disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                request: None,
                call: audit,
                request_terminal: None,
                call_settled: stored,
            },
        ))
    }

    pub(super) fn auxiliary_audit_cut(&self) -> Result<AuxiliaryAuditCut, ConversationStoreError> {
        let connection = self.lock()?;
        Ok(AuxiliaryAuditCut {
            journal: journal_prefix(&connection)?,
        })
    }

    pub(super) fn auxiliary_audit_total(
        &self,
        cut: AuxiliaryAuditCut,
    ) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        audit_total_at_connection(&connection, cut)
    }

    pub(super) fn read_auxiliary_audit(
        &self,
        cut: AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<AuxiliaryAuditPage, ConversationStoreError> {
        if limit == 0 || limit > AUXILIARY_AUDIT_PAGE_LIMIT_MAX {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary audit limit must be 1..={AUXILIARY_AUDIT_PAGE_LIMIT_MAX}"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("auxiliary audit snapshot: {error}")))?;
        // Pin one WAL snapshot for the cut, cursor and related authority rows.
        let total = audit_total_at_connection(&transaction, cut)?;
        let after = after_offset.unwrap_or(0);
        if after > total {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit offset is after its captured cut".into(),
            ));
        }
        let mut previous_journal = 0;
        if after > 0 {
            let anchor_sequence: Option<i64> = transaction
                .query_row(
                    "SELECT sequence FROM auxiliary_audit_facts
                     WHERE audit_offset=?1 AND sequence<=?2",
                    params![
                        i64::try_from(after)
                            .map_err(|_| storage("auxiliary audit cursor overflow"))?,
                        seq_to_i64(cut.journal)?,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("auxiliary audit cursor anchor: {error}")))?;
            let anchor_sequence = anchor_sequence.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary audit cursor does not name a visible dense record".into(),
                )
            })?;
            let anchor = audit_fact_row(&transaction, sequence_from_i64(anchor_sequence)?)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary audit cursor anchor has no indexed row".into(),
                    )
                })?;
            if anchor.audit_offset != after {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary audit cursor anchor has a conflicting offset".into(),
                ));
            }
            previous_journal = anchor.sequence;
            let event = event_at_connection(&transaction, anchor.sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary audit cursor anchor has no Journal fact".into(),
                )
            })?;
            validate_audit_fact_event(&event, &anchor)?;
            match anchor.kind.as_str() {
                "call_started" | "call_settled" => {
                    if call_audit_at_tx(&transaction, &self.conversation_id, &anchor.call_id, cut)?
                        .is_none()
                    {
                        return Err(ConversationStoreError::InvalidReference(
                            "auxiliary audit cursor Call anchor has no audit projection".into(),
                        ));
                    }
                }
                "request_started" | "request_settled" => {
                    let request_id = anchor.request_id.as_ref().ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary audit cursor request anchor has no RequestId".into(),
                        )
                    })?;
                    if request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?
                        .is_none()
                    {
                        return Err(ConversationStoreError::InvalidReference(
                            "auxiliary audit cursor request anchor has no audit projection".into(),
                        ));
                    }
                }
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "auxiliary audit cursor anchor has an unknown kind".into(),
                    ));
                }
            }
        }
        let requested =
            i64::try_from(limit + 1).map_err(|_| storage("auxiliary audit page limit overflow"))?;
        let mut statement = transaction
            .prepare(
                "SELECT sequence,audit_offset,kind,call_id,request_id,ordinal
                 FROM auxiliary_audit_facts
                 WHERE audit_offset>?1 AND audit_offset<=?4 AND sequence<=?2
                 ORDER BY audit_offset LIMIT ?3",
            )
            .map_err(|error| storage(format!("auxiliary audit page query: {error}")))?;
        let rows = statement
            .query_map(
                params![
                    i64::try_from(after).map_err(|_| storage("auxiliary audit cursor overflow"))?,
                    seq_to_i64(cut.journal)?,
                    requested,
                    i64::try_from(total).map_err(|_| storage("auxiliary audit total overflow"))?,
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .map_err(|error| storage(format!("auxiliary audit page rows: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("collect auxiliary audit page: {error}")))?;
        let has_more = rows.len() > limit;
        let selected = &rows[..rows.len().min(limit)];
        if total > after && selected.is_empty() {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit total has no corresponding indexed facts".into(),
            ));
        }
        let mut records = Vec::with_capacity(selected.len());
        let mut expected_offset = after.checked_add(1).ok_or_else(|| {
            ConversationStoreError::InvalidReference("auxiliary audit cursor overflow".into())
        })?;
        for (sequence, audit_offset, kind, call_id, request_id, ordinal) in selected {
            let row = AuditFactRow {
                sequence: sequence_from_i64(*sequence)?,
                audit_offset: u64::try_from(*audit_offset).map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary audit offset is negative".into(),
                    )
                })?,
                kind: kind.clone(),
                call_id: AuxiliaryCallId::parse(call_id.clone())
                    .map_err(ConversationStoreError::InvalidReference)?,
                request_id: request_id.clone().map(RequestId::new),
                ordinal: ordinal
                    .map(|value| {
                        u32::try_from(value).map_err(|_| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary audit ordinal is out of range".into(),
                            )
                        })
                    })
                    .transpose()?,
            };
            if row.audit_offset != expected_offset {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary audit offsets are not dense and advancing".into(),
                ));
            }
            if row.sequence <= previous_journal {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary audit records are not in Journal order".into(),
                ));
            }
            expected_offset = expected_offset.checked_add(1).ok_or_else(|| {
                ConversationStoreError::InvalidReference("auxiliary audit offset overflow".into())
            })?;
            previous_journal = row.sequence;
            let event = event_at_connection(&transaction, row.sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary audit fact has no Journal event".into(),
                )
            })?;
            validate_audit_fact_event(&event, &row)?;
            let fact = match row.kind.as_str() {
                "call_started" => AuxiliaryAuditFact::CallStarted(
                    call_audit_at_tx(&transaction, &self.conversation_id, &row.call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary Call start fact has no audit row".into(),
                            )
                        })?,
                ),
                "call_settled" => AuxiliaryAuditFact::CallSettled(
                    call_audit_at_tx(&transaction, &self.conversation_id, &row.call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary Call settlement fact has no audit row".into(),
                            )
                        })?,
                ),
                "request_started" | "request_settled" => {
                    let request_id = row.request_id.as_ref().ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary request audit fact has no RequestId".into(),
                        )
                    })?;
                    let audit =
                        request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?
                            .ok_or_else(|| {
                                ConversationStoreError::InvalidReference(
                                    "auxiliary request audit fact has no request row".into(),
                                )
                            })?;
                    if row.ordinal != Some(audit.ordinal) {
                        return Err(ConversationStoreError::InvalidReference(
                            "auxiliary audit ordinal disagrees with its request".into(),
                        ));
                    }
                    if row.kind == "request_started" {
                        AuxiliaryAuditFact::RequestStarted(audit)
                    } else {
                        AuxiliaryAuditFact::RequestSettled(audit)
                    }
                }
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "unknown auxiliary audit fact kind".into(),
                    ));
                }
            };
            let record = AuxiliaryAuditRecord {
                audit_offset: row.audit_offset,
                journal_sequence: row.sequence,
                fact,
            };
            audit_record_bytes(&record)?;
            records.push(record);
        }
        let next_offset = if has_more {
            Some(records.last().expect("nonempty page").audit_offset)
        } else {
            None
        };
        Ok(AuxiliaryAuditPage {
            cut,
            total,
            records,
            next_offset,
        })
    }

    pub(super) fn load_auxiliary_call(
        &self,
        call_id: &AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError> {
        let connection = self.lock()?;
        let current = journal_prefix(&connection)?;
        if cut.journal > current {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit cut is in the future".into(),
            ));
        }
        let audit = call_audit_at_tx(&connection, &self.conversation_id, call_id, cut)?;
        if audit.is_none() {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|error| storage(format!("auxiliary Call audit probe: {error}")))?;
            if has_call_journal_fact_through_tx(&transaction, call_id, cut.journal)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} has Journal evidence without its audit index"
                )));
            }
        }
        Ok(audit)
    }

    pub(super) fn read_auxiliary_call_requests(
        &self,
        call_id: &AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuxiliaryRequestAudit>, ConversationStoreError> {
        if !(1..=AUXILIARY_AUDIT_PAGE_LIMIT_MAX).contains(&limit) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary Call request limit must be 1..={AUXILIARY_AUDIT_PAGE_LIMIT_MAX}"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("auxiliary Call request snapshot: {error}")))?;
        if cut.journal > journal_prefix(&transaction)? {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit cut is in the future".into(),
            ));
        }
        if after_started_sequence.is_some_and(|after| after == 0 || after > cut.journal) {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call request cursor is outside its captured cut".into(),
            ));
        }
        let call = call_audit_at_tx(&transaction, &self.conversation_id, call_id, cut)?;
        if call.is_none() {
            if after_started_sequence.is_some()
                || has_call_journal_fact_through_tx(&transaction, call_id, cut.journal)?
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request lookup has no visible Call authority".into(),
                ));
            }
            return Ok(Vec::new());
        }
        let mut next_ordinal = Some(0_u32);
        if let Some(after) = after_started_sequence {
            let anchor = audit_fact_row(&transaction, after)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor has no audit fact".into(),
                )
            })?;
            if anchor.kind != "request_started" || anchor.call_id != *call_id {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor must name this Call's request start".into(),
                ));
            }
            let request_id = anchor.request_id.as_ref().ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor has no request identity".into(),
                )
            })?;
            let request =
                request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call request cursor has no visible request authority".into(),
                        )
                    })?;
            if request.call_id != *call_id
                || request.started_sequence != after
                || Some(request.ordinal) != anchor.ordinal
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor disagrees with its request".into(),
                ));
            }
            next_ordinal = request.ordinal.checked_add(1);
        }
        let rows = transaction
            .prepare_cached(AUXILIARY_CALL_REQUESTS_SQL)
            .map_err(|error| storage(format!("auxiliary Call request lookup: {error}")))?
            .query_map(
                params![
                    call_id.as_str(),
                    seq_to_i64(after_started_sequence.unwrap_or(0))?,
                    seq_to_i64(cut.journal)?,
                    i64::try_from(limit)
                        .map_err(|_| storage("auxiliary request limit overflow"))?,
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .map_err(|error| storage(format!("auxiliary Call request query: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("auxiliary Call request row: {error}")))?;
        let mut requests = Vec::with_capacity(rows.len());
        for (sequence, request_id, ordinal) in rows {
            let request = request_audit_at_tx(
                &transaction,
                &self.conversation_id,
                &RequestId::new(request_id),
                cut,
            )?
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary Call request fact has no visible request authority".into(),
                )
            })?;
            if request.call_id != *call_id
                || request.started_sequence != sequence_from_i64(sequence)?
                || request.ordinal != ordinal
                || Some(ordinal) != next_ordinal
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request facts disagree with sequential request authority"
                        .into(),
                ));
            }
            bounded_encoded_len(
                &request,
                AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
                "auxiliary Call request audit",
            )?;
            next_ordinal = ordinal.checked_add(1);
            requests.push(request);
        }
        if requests.len() < limit
            && let Some(ordinal) = next_ordinal
        {
            let indexed_start: Option<i64> = transaction
                .query_row(
                    "SELECT started_sequence FROM auxiliary_requests
                     WHERE call_id=?1 AND ordinal=?2
                     UNION ALL
                     SELECT start_sequence FROM request_owners
                     INDEXED BY request_owners_source_idx
                     WHERE owner_kind='auxiliary' AND call_id=?1 AND auxiliary_ordinal=?2
                     ORDER BY 1 LIMIT 1",
                    params![call_id.as_str(), ordinal],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    storage(format!("auxiliary Call request completeness: {error}"))
                })?;
            if indexed_start
                .map(sequence_from_i64)
                .transpose()?
                .is_some_and(|sequence| sequence <= cut.journal)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request page is missing indexed request evidence".into(),
                ));
            }
        }
        Ok(requests)
    }

    pub(super) fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryRequestAudit>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("auxiliary request audit snapshot: {error}")))?;
        // The request, Call and owner rows must describe one committed state.
        if cut.journal > journal_prefix(&transaction)? {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit cut is in the future".into(),
            ));
        }
        let audit = request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?;
        if audit.is_none() {
            if let Some(owner) = owner_row_query(&transaction, request_id)? {
                validate_owner_row_tx(&transaction, &self.conversation_id, request_id, &owner)?;
                if matches!(owner.owner, ModelRequestOwner::Agent { .. }) {
                    return Ok(None);
                }
                if request_row_query(&transaction, "request_id=?1", [request_id.as_str()])?
                    .is_some_and(|request| request.started_sequence > cut.journal)
                {
                    return Ok(None);
                }
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {request_id} owner has no request index"
                )));
            }
            if has_request_journal_fact_through_tx(&transaction, request_id, cut.journal)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {request_id} has Journal evidence without its audit index"
                )));
            }
        }
        Ok(audit)
    }
}

impl crate::durable::AuxiliaryStore for SqliteConversationStore {
    fn commit_auxiliary_call_start(
        &self,
        start: &crate::model::auxiliary::AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryCallStartCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_call_start(self, start, timestamp)
    }

    fn commit_auxiliary_request_start(
        &self,
        start: &crate::model::auxiliary::AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryRequestStartCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_request_start(self, start, timestamp)
    }

    fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_request_settlement(
            self,
            request_terminal,
            call_settled,
        )
    }

    fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_call_settlement(self, call_settled)
    }

    fn confirm_auxiliary_call_start(
        &self,
        expected: &crate::model::auxiliary::AuxiliaryCallStart,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliaryCallStartCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_call_start(self, expected)
    }

    fn confirm_auxiliary_request_start(
        &self,
        expected: &crate::model::auxiliary::AuxiliaryRequestStart,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliaryRequestStartCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_request_start(self, expected)
    }

    fn confirm_auxiliary_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliarySettlementCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_settlement(self, request_terminal, call_settled)
    }

    fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliarySettlementCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_call_settlement(self, call_settled)
    }

    fn auxiliary_audit_cut(
        &self,
    ) -> Result<crate::model::auxiliary::AuxiliaryAuditCut, ConversationStoreError> {
        SqliteConversationStore::auxiliary_audit_cut(self)
    }

    fn read_auxiliary_audit(
        &self,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::model::auxiliary::AuxiliaryAuditPage, ConversationStoreError> {
        SqliteConversationStore::read_auxiliary_audit(self, cut, after_offset, limit)
    }

    fn auxiliary_audit_total(
        &self,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<u64, ConversationStoreError> {
        SqliteConversationStore::auxiliary_audit_total(self, cut)
    }

    fn load_auxiliary_call(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<Option<crate::model::auxiliary::AuxiliaryCallAudit>, ConversationStoreError> {
        SqliteConversationStore::load_auxiliary_call(self, call_id, cut)
    }

    fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<Option<crate::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError>
    {
        SqliteConversationStore::load_auxiliary_request(self, request_id, cut)
    }

    fn read_auxiliary_call_requests(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<crate::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError> {
        SqliteConversationStore::read_auxiliary_call_requests(
            self,
            call_id,
            cut,
            after_started_sequence,
            limit,
        )
    }
}

#[cfg(test)]
mod tests;
