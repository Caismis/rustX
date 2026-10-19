//! Explicit Summary-generation lineage closure.
//!
//! This module reads bounded source audit facts for Calls named by retained
//! checkpoints and installs those facts in a destination-owned inherited-source
//! table. It never creates target-local auxiliary execution rows.

use super::super::{
    Connection, ConversationStoreError, Transaction, decode, journal_prefix, seq_to_i64,
    sequence_from_i64, storage,
};
use crate::context::checkpoint::{CheckpointCandidate, SummaryGenerationRef};
use crate::context::resources::{ResourceLimits, bounded_encoded_len};
use crate::durable::projection::ProjectionCheckpoint;
use crate::durable::reading::{
    AuxiliaryAuditCompleteness, AuxiliaryAuditOrigin, AuxiliaryAuditSnapshot,
};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::model::ModelRequestOwner;
use crate::model::auxiliary::{
    AUXILIARY_AUDIT_RECORD_ENCODED_MAX, AUXILIARY_CALL_START_ENCODED_MAX,
    AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX, AUXILIARY_REQUEST_ID_BYTES_MAX,
    AUXILIARY_REQUEST_MODEL_BYTES_MAX, AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
    AuxiliaryAttribution, AuxiliaryAuditCut, AuxiliaryAuditFact, AuxiliaryAuditRecord,
    AuxiliaryBusinessSource, AuxiliaryCallAudit, AuxiliaryCallOutcome, AuxiliaryFingerprint,
    AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal, AuxiliaryRequestAudit,
    AuxiliaryRequestEvidence, AuxiliaryRequestTerminal, AuxiliaryUsageCompleteness,
};
use crate::runtime::identity::{AuxiliaryCallId, ConversationId, EventId, MessageId, RequestId};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const TABLE: &str = "inherited_auxiliary_audits";
const PAYLOAD_MAX: usize = 16 * 1024 * 1024;
/// Bound for replacing a local origin with its longer inherited-source form.
const INHERITED_ORIGIN_OVERHEAD_MAX: usize = 2_048;

struct AuditedSnapshot {
    value: AuxiliaryAuditSnapshot,
    encoded_bytes: usize,
}

type RequestRow = (u32, String, String, i64, Option<i64>, Option<String>);
type AuditRow = (i64, i64, String, Option<String>, Option<i64>);
type OwnerRow = (
    String,
    Option<String>,
    Option<i64>,
    i64,
    Option<i64>,
    Option<String>,
);

const CREATE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS inherited_auxiliary_audits (
    source_conversation_id TEXT NOT NULL
        CHECK(length(CAST(source_conversation_id AS BLOB)) BETWEEN 1 AND 1024),
    call_id TEXT NOT NULL
        CHECK(length(CAST(call_id AS BLOB)) BETWEEN 1 AND 64),
    through_journal INTEGER NOT NULL CHECK(through_journal>0),
    completeness TEXT NOT NULL CHECK(completeness IN ('complete_at_cut','open_at_cut')),
    record_count INTEGER NOT NULL CHECK(record_count>=1),
    payload_json TEXT NOT NULL
        CHECK(octet_length(payload_json) BETWEEN 2 AND 16777216),
    payload_digest TEXT NOT NULL
        CHECK(typeof(payload_digest)='text' AND length(payload_digest)=64),
    PRIMARY KEY(source_conversation_id,call_id,through_journal)
);
CREATE INDEX IF NOT EXISTS inherited_auxiliary_audits_call_idx
    ON inherited_auxiliary_audits(source_conversation_id,call_id,through_journal);
";

pub(super) fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(CREATE_SQL)
        .map_err(|error| storage(format!("create inherited auxiliary audit schema: {error}")))
}

pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    let sql: Option<Option<String>> = connection
        .query_row(
            "SELECT CASE WHEN octet_length(sql)<=8192 THEN sql END
             FROM sqlite_schema WHERE type='table' AND name=?1",
            [TABLE],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let sql = sql.flatten().ok_or_else(|| {
        ConversationStoreError::IncompatibleSchema("missing inherited auxiliary audit table".into())
    })?;
    let normalized: String = sql
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    for required in [
        "source_conversation_id",
        "call_id",
        "through_journal",
        "completeness",
        "record_count",
        "payload_json",
        "payload_digest",
        "primarykey(source_conversation_id,call_id,through_journal)",
    ] {
        if !normalized.contains(required) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "invalid inherited auxiliary audit table".into(),
            ));
        }
    }
    let index: Option<Option<String>> = connection
        .query_row(
            "SELECT CASE WHEN octet_length(sql)<=8192 THEN sql END
             FROM sqlite_schema
             WHERE type='index' AND name='inherited_auxiliary_audits_call_idx'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let index = index.flatten().ok_or_else(|| {
        ConversationStoreError::IncompatibleSchema("missing inherited auxiliary audit index".into())
    })?;
    let normalized_index: String = index
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if !normalized_index
        .contains("oninherited_auxiliary_audits(source_conversation_id,call_id,through_journal)")
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid inherited auxiliary audit index".into(),
        ));
    }
    Ok(())
}

fn digest(encoded: &str) -> String {
    format!("{:x}", Sha256::digest(encoded.as_bytes()))
}

/// Immutable bootstrap coordinates for the inherited Summary audit closure.
pub(super) fn bootstrap_coordinates(
    connection: &Connection,
) -> Result<(u64, String), ConversationStoreError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM (SELECT 1 FROM inherited_auxiliary_audits LIMIT 8193)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("inherited Summary bootstrap count: {error}")))?;
    let count = u64::try_from(count).map_err(|_| invalid("negative inherited Summary count"))?;
    let limit = u64::try_from(ResourceLimits::default().total_items)
        .map_err(|_| storage("inherited Summary item limit overflow"))?;
    if count > limit {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "inherited Summary bootstrap count exceeds its bound".into(),
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT source_conversation_id,call_id,through_journal,
                    completeness,record_count,payload_digest
             FROM inherited_auxiliary_audits
             ORDER BY source_conversation_id,call_id,through_journal",
        )
        .map_err(|error| storage(format!("inherited Summary bootstrap rows: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|error| storage(format!("read inherited Summary bootstrap: {error}")))?;
    let mut hash = Sha256::new();
    hash.update(b"rustx-inherited-auxiliary-audits-v1\n");
    for row in rows {
        let (source, call, through, complete, records, payload_digest) =
            row.map_err(|error| storage(format!("collect inherited bootstrap: {error}")))?;
        for value in [
            source,
            call,
            through.to_string(),
            complete,
            records.to_string(),
        ] {
            hash.update(
                u64::try_from(value.len())
                    .map_err(|_| storage("inherited Summary bootstrap length overflow"))?
                    .to_be_bytes(),
            );
            hash.update(value.as_bytes());
        }
        if payload_digest.len() != 64
            || !payload_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("invalid inherited Summary payload digest"));
        }
        hash.update(payload_digest.as_bytes());
    }
    Ok((count, format!("{:x}", hash.finalize())))
}

pub(super) fn verify_bootstrap(
    connection: &Connection,
    expected_count: u64,
    expected_digest: &str,
) -> Result<(), ConversationStoreError> {
    if bootstrap_coordinates(connection)? != (expected_count, expected_digest.to_owned()) {
        return Err(ConversationStoreError::InitialHistoryMismatch);
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> ConversationStoreError {
    ConversationStoreError::InvalidReference(message.into())
}

fn event_at(
    connection: &Connection,
    sequence: u64,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let text: String = connection
        .query_row(
            "SELECT CASE WHEN octet_length(event_json)<=?2 THEN event_json END
             FROM events WHERE sequence=?1",
            params![
                seq_to_i64(sequence)?,
                i64::try_from(AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX)
                    .map_err(|_| storage("auxiliary lineage event bound overflow"))?
            ],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| storage(format!("auxiliary lineage event lookup: {error}")))?
        .ok_or_else(|| invalid("auxiliary lineage event is absent"))?
        .ok_or_else(|| {
            ConversationStoreError::ResourceBoundExceeded(
                "auxiliary lineage event exceeds byte bound".into(),
            )
        })?;
    let event: RuntimeEventEnvelope = decode(&text, "auxiliary lineage event")?;
    if event.schema_version != EVENT_SCHEMA_VERSION || event.sequence != sequence {
        return Err(invalid(
            "auxiliary lineage event disagrees with its Journal coordinate",
        ));
    }
    Ok(event)
}

fn event_id(prefix: &str, id: impl std::fmt::Display) -> EventId {
    EventId::new(format!("{prefix}:{id}"))
}

fn call_start<'a>(
    event: &'a RuntimeEventEnvelope,
    call_id: &AuxiliaryCallId,
) -> Result<&'a RuntimeEvent, ConversationStoreError> {
    if event.event_id != event_id("auxiliary-call-started", call_id) {
        return Err(invalid(
            "auxiliary Summary start has a noncanonical event id",
        ));
    }
    match &event.event {
        RuntimeEvent::AuxiliaryCallStarted {
            call_id: actual, ..
        } if actual == call_id => Ok(&event.event),
        _ => Err(invalid("auxiliary Summary index names a non-start event")),
    }
}

fn complete_request_evidence(
    call_settlement: Option<&RuntimeEventEnvelope>,
    request_id: &RequestId,
) -> Option<AuxiliaryRequestEvidence> {
    let RuntimeEvent::AuxiliaryCallSettled {
        request_evidence, ..
    } = &call_settlement?.event
    else {
        return None;
    };
    request_evidence
        .as_ref()
        .filter(|evidence| &evidence.request_id == request_id)
        .cloned()
}

#[allow(clippy::too_many_lines)] // One bounded closure over all linked audit indexes.
fn local_snapshot(
    connection: &Connection,
    conversation_id: &ConversationId,
    reference: &SummaryGenerationRef,
    remaining_records: usize,
    remaining_bytes: usize,
) -> Result<AuditedSnapshot, ConversationStoreError> {
    let cut = AuxiliaryAuditCut {
        journal: reference.through_journal,
    };
    let call: Option<(i64, Option<i64>, Option<String>)> = connection
        .query_row(
            "SELECT started_sequence,settled_sequence,outcome
             FROM auxiliary_calls
             WHERE call_id=?1 AND started_sequence<=?2",
            params![reference.call_id.as_str(), seq_to_i64(cut.journal)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("auxiliary Summary index lookup: {error}")))?;
    let (started_sequence, settled_sequence, indexed_outcome) =
        call.ok_or_else(|| invalid("named Summary generation Call is not local and visible"))?;
    let started_sequence = sequence_from_i64(started_sequence)?;
    let start = event_at(connection, started_sequence)?;
    if start.conversation_id != *conversation_id {
        return Err(invalid(
            "named Summary generation Call belongs to another Conversation",
        ));
    }
    let RuntimeEvent::AuxiliaryCallStarted {
        purpose,
        trigger,
        business_source,
        input,
        semantic_input_fingerprint,
        invocation_fingerprint,
        selected_message_ids,
        ..
    } = call_start(&start, &reference.call_id)?
    else {
        unreachable!("call_start checks the variant")
    };
    if purpose.as_str() != "summary" {
        return Err(invalid(
            "checkpoint generation dependency must name a Summary Call",
        ));
    }

    let indexed_settlement_sequence = settled_sequence.map(sequence_from_i64).transpose()?;
    let settled_sequence = indexed_settlement_sequence.filter(|sequence| *sequence <= cut.journal);
    let settlement = settled_sequence
        .map(|sequence| event_at(connection, sequence))
        .transpose()?;
    let mut settled_outcome = None;
    if let Some(settlement) = &settlement {
        if settlement.event_id
            != crate::durable::auxiliary::call_settled_event_id(&reference.call_id)
        {
            return Err(invalid(
                "auxiliary Summary settlement has a noncanonical event id",
            ));
        }
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id, outcome, ..
        } = &settlement.event
        else {
            return Err(invalid(
                "auxiliary Summary settlement index names a non-settlement event",
            ));
        };
        if call_id != &reference.call_id {
            return Err(invalid("auxiliary Summary settlement names another Call"));
        }
        settled_outcome = Some(*outcome);
        if indexed_outcome.as_deref().and_then(call_outcome) != Some(*outcome) {
            return Err(invalid(
                "auxiliary Summary settlement disagrees with its mandatory index",
            ));
        }
    } else if indexed_settlement_sequence.is_none() && indexed_outcome.is_some() {
        return Err(invalid(
            "auxiliary Summary has an outcome without a settlement coordinate",
        ));
    }
    let call_audit = AuxiliaryCallAudit {
        call_id: reference.call_id.clone(),
        purpose: purpose.clone(),
        trigger: trigger.clone(),
        business_source: business_source.clone(),
        input: input.clone(),
        semantic_input_fingerprint: semantic_input_fingerprint.clone(),
        invocation_fingerprint: invocation_fingerprint.clone(),
        selected_message_ids: selected_message_ids.clone(),
        started_sequence,
        settled_sequence,
        outcome: settled_outcome,
    };

    let mut statement = connection
        .prepare(
            "SELECT ordinal,request_id,model,started_sequence,terminal_sequence,terminal_kind
             FROM auxiliary_requests
             WHERE call_id=?1 AND started_sequence<=?2
             ORDER BY ordinal
             LIMIT ?3",
        )
        .map_err(|error| storage(format!("auxiliary Summary request query: {error}")))?;
    let record_limit = remaining_records.min(remaining_bytes / AUXILIARY_AUDIT_RECORD_ENCODED_MAX);
    if record_limit == 0 {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary Summary closure has no remaining record budget".into(),
        ));
    }
    let request_limit = i64::try_from(record_limit.saturating_add(1))
        .map_err(|_| storage("auxiliary request bound overflow"))?;
    let request_rows = statement
        .query_map(
            params![
                reference.call_id.as_str(),
                seq_to_i64(cut.journal)?,
                request_limit
            ],
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
        .map_err(|error| storage(format!("auxiliary Summary request rows: {error}")))?
        .collect::<Result<Vec<RequestRow>, _>>()
        .map_err(|error| storage(format!("collect auxiliary Summary requests: {error}")))?;
    if request_rows.is_empty() {
        return Err(invalid(
            "named Summary generation has no physical request prerequisite",
        ));
    }
    if request_rows.len() > record_limit {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary Summary request closure exceeds its item bound".into(),
        ));
    }
    let mut requests = BTreeMap::new();
    for (ordinal, request_id, model, request_started, request_terminal, request_terminal_kind) in
        request_rows
    {
        let request_id = RequestId::new(request_id);
        let request_started = sequence_from_i64(request_started)?;
        let request_started_sql = seq_to_i64(request_started)?;
        let owner: Option<OwnerRow> = connection
            .query_row(
                "SELECT owner_kind,call_id,auxiliary_ordinal,start_sequence,
                        terminal_sequence,terminal_kind
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
                    ))
                },
            )
            .optional()
            .map_err(|error| storage(format!("auxiliary request owner lookup: {error}")))?;
        if owner.as_ref().is_none_or(
            |(kind, call, owner_ordinal, start, terminal, terminal_kind)| {
                kind != "auxiliary"
                    || call.as_deref() != Some(reference.call_id.as_str())
                    || *owner_ordinal != Some(i64::from(ordinal))
                    || *start != request_started_sql
                    || *terminal != request_terminal
                    || terminal_kind.as_deref() != request_terminal_kind.as_deref()
            },
        ) {
            return Err(invalid(
                "auxiliary Summary request lacks its exact owner prerequisite",
            ));
        }
        let request_start = event_at(connection, request_started)?;
        let expected_start = event_id("auxiliary-request-started", &request_id);
        match &request_start.event {
            RuntimeEvent::ModelRequestStarted {
                owner:
                    ModelRequestOwner::Auxiliary {
                        call_id,
                        ordinal: owner_ordinal,
                    },
                request_id: event_request,
                model: event_model,
            } if request_start.event_id == expected_start
                && call_id == &reference.call_id
                && *owner_ordinal == ordinal
                && event_request == &request_id
                && event_model == &model
                && request_start.conversation_id == *conversation_id => {}
            _ => {
                return Err(invalid(
                    "auxiliary Summary request start disagrees with its mandatory index",
                ));
            }
        }
        let indexed_terminal_sequence = request_terminal.map(sequence_from_i64).transpose()?;
        if indexed_terminal_sequence.is_some()
            != request_terminal_kind
                .as_deref()
                .is_some_and(|kind| matches!(kind, "completed" | "failed"))
        {
            return Err(invalid(
                "auxiliary Summary terminal index is incomplete or invalid",
            ));
        }
        let terminal_sequence =
            indexed_terminal_sequence.filter(|sequence| *sequence <= cut.journal);
        let mut local_terminal = None;
        let mut normalized_failure_kind = None;
        let mut generation = None;
        let mut usage = None;
        if let Some(terminal_sequence) = terminal_sequence {
            let terminal = event_at(connection, terminal_sequence)?;
            match &terminal.event {
                RuntimeEvent::ModelRequestCompleted {
                    owner:
                        ModelRequestOwner::Auxiliary {
                            call_id,
                            ordinal: owner_ordinal,
                        },
                    request_id: event_request,
                    usage: event_usage,
                    generation: event_generation,
                    ..
                } if terminal.event_id
                    == crate::durable::auxiliary::request_terminal_event_id(&request_id, true)
                    && call_id == &reference.call_id
                    && *owner_ordinal == ordinal
                    && event_request == &request_id =>
                {
                    local_terminal = Some(AuxiliaryRequestTerminal::Completed);
                    generation = *event_generation;
                    usage.clone_from(event_usage);
                }
                RuntimeEvent::ModelRequestFailed {
                    owner:
                        ModelRequestOwner::Auxiliary {
                            call_id,
                            ordinal: owner_ordinal,
                        },
                    request_id: event_request,
                    error,
                    usage: event_usage,
                    generation: event_generation,
                } if terminal.event_id
                    == crate::durable::auxiliary::request_terminal_event_id(&request_id, false)
                    && call_id == &reference.call_id
                    && *owner_ordinal == ordinal
                    && event_request == &request_id =>
                {
                    local_terminal = Some(AuxiliaryRequestTerminal::Failed);
                    normalized_failure_kind = Some(error.kind.clone());
                    generation = *event_generation;
                    usage.clone_from(event_usage);
                }
                _ => {
                    return Err(invalid(
                        "auxiliary Summary terminal disagrees with its request start",
                    ));
                }
            }
        }
        requests.insert(
            ordinal,
            AuxiliaryRequestAudit {
                call_id: reference.call_id.clone(),
                ordinal,
                request_id: request_id.clone(),
                model,
                started_sequence: request_started,
                terminal_sequence,
                local_terminal,
                normalized_failure_kind,
                generation,
                usage,
                request_evidence: complete_request_evidence(settlement.as_ref(), &request_id),
            },
        );
    }

    let mut statement = connection
        .prepare(
            "SELECT sequence,audit_offset,kind,request_id,ordinal
             FROM auxiliary_audit_facts
             WHERE call_id=?1 AND sequence<=?2
             ORDER BY sequence
             LIMIT ?3",
        )
        .map_err(|error| storage(format!("auxiliary Summary audit query: {error}")))?;
    let audit_rows = statement
        .query_map(
            params![
                reference.call_id.as_str(),
                seq_to_i64(cut.journal)?,
                request_limit
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|error| storage(format!("auxiliary Summary audit rows: {error}")))?
        .collect::<Result<Vec<AuditRow>, _>>()
        .map_err(|error| storage(format!("collect auxiliary Summary audit: {error}")))?;
    if audit_rows.len() > record_limit {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary Summary audit closure exceeds its item bound".into(),
        ));
    }
    let expected_records = 1_usize
        .checked_add(
            requests
                .values()
                .map(|request| 1 + usize::from(request.terminal_sequence.is_some()))
                .sum::<usize>(),
        )
        .and_then(|count| count.checked_add(usize::from(settlement.is_some())))
        .ok_or_else(|| storage("auxiliary Summary record count overflow"))?;
    if audit_rows.len() != expected_records {
        return Err(invalid(
            "auxiliary Summary audit closure is incomplete at its provenance cut",
        ));
    }
    let mut records = Vec::with_capacity(audit_rows.len());
    let mut seen_sequences = BTreeSet::new();
    for (sequence, audit_offset, kind, request_id, ordinal) in audit_rows {
        let sequence = sequence_from_i64(sequence)?;
        let audit_offset = u64::try_from(audit_offset)
            .map_err(|_| invalid("auxiliary audit offset is negative"))?;
        if !seen_sequences.insert(sequence) {
            return Err(invalid(
                "auxiliary Summary audit repeats a Journal position",
            ));
        }
        let event = event_at(connection, sequence)?;
        if event.conversation_id != *conversation_id {
            return Err(invalid(
                "auxiliary Summary audit belongs to another Conversation",
            ));
        }
        let fact = match kind.as_str() {
            "call_started" => {
                if sequence != started_sequence || request_id.is_some() || ordinal.is_some() {
                    return Err(invalid("auxiliary Summary start audit is inconsistent"));
                }
                AuxiliaryAuditFact::CallStarted(call_audit.clone())
            }
            "call_settled" => {
                if settled_sequence != Some(sequence) || request_id.is_some() || ordinal.is_some() {
                    return Err(invalid(
                        "auxiliary Summary settlement audit is inconsistent",
                    ));
                }
                AuxiliaryAuditFact::CallSettled(call_audit.clone())
            }
            "request_started" | "request_settled" => {
                let request_id =
                    request_id.ok_or_else(|| invalid("auxiliary request audit lacks RequestId"))?;
                let ordinal =
                    ordinal.ok_or_else(|| invalid("auxiliary request audit lacks an ordinal"))?;
                let ordinal = u32::try_from(ordinal)
                    .map_err(|_| invalid("auxiliary request audit ordinal is out of range"))?;
                let request = requests.get(&ordinal).ok_or_else(|| {
                    invalid("auxiliary request audit has no request prerequisite")
                })?;
                if request.request_id.as_str() != request_id.as_str()
                    || match kind.as_str() {
                        "request_started" => request.started_sequence != sequence,
                        "request_settled" => request.terminal_sequence != Some(sequence),
                        _ => unreachable!(),
                    }
                {
                    return Err(invalid(
                        "auxiliary request audit disagrees with its request index",
                    ));
                }
                if kind == "request_started" {
                    AuxiliaryAuditFact::RequestStarted(request.clone())
                } else {
                    AuxiliaryAuditFact::RequestSettled(request.clone())
                }
            }
            _ => return Err(invalid("unknown auxiliary audit kind")),
        };
        records.push(AuxiliaryAuditRecord {
            audit_offset,
            journal_sequence: sequence,
            fact,
        });
    }
    let completeness = if settled_sequence.is_some() {
        AuxiliaryAuditCompleteness::CompleteAtCut
    } else {
        AuxiliaryAuditCompleteness::OpenAtCut
    };
    let encoded_bytes = records
        .len()
        .checked_mul(AUXILIARY_AUDIT_RECORD_ENCODED_MAX)
        .ok_or_else(|| storage("auxiliary Summary byte count overflow"))?;
    Ok(AuditedSnapshot {
        value: AuxiliaryAuditSnapshot {
            origin: AuxiliaryAuditOrigin::Local,
            source_conversation_id: conversation_id.clone(),
            call_id: reference.call_id.clone(),
            through_journal: cut.journal,
            completeness,
            records,
        },
        encoded_bytes,
    })
}

fn call_outcome(value: &str) -> Option<AuxiliaryCallOutcome> {
    match value {
        "completed" => Some(AuxiliaryCallOutcome::Completed),
        "failed" => Some(AuxiliaryCallOutcome::Failed),
        "cancelled" => Some(AuxiliaryCallOutcome::Cancelled),
        _ => None,
    }
}

fn completeness_name(value: AuxiliaryAuditCompleteness) -> &'static str {
    match value {
        AuxiliaryAuditCompleteness::CompleteAtCut => "complete_at_cut",
        AuxiliaryAuditCompleteness::OpenAtCut => "open_at_cut",
    }
}

fn inherited_snapshot(
    connection: &Connection,
    reference: &SummaryGenerationRef,
    payload_limit: usize,
) -> Result<AuditedSnapshot, ConversationStoreError> {
    let allowed = payload_limit.min(PAYLOAD_MAX);
    if allowed == 0 {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "inherited Summary audit has no remaining byte budget".into(),
        ));
    }
    let row: Option<(i64, String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT record_count,completeness,
                    CASE WHEN octet_length(payload_json)<=?4 THEN payload_json END,
                    CASE WHEN octet_length(payload_digest)=64 THEN payload_digest END
             FROM inherited_auxiliary_audits
             WHERE source_conversation_id=?1 AND call_id=?2 AND through_journal=?3",
            params![
                reference.conversation_id.as_str(),
                reference.call_id.as_str(),
                seq_to_i64(reference.through_journal)?,
                i64::try_from(allowed)
                    .map_err(|_| storage("inherited auxiliary payload bound overflow"))?
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| storage(format!("inherited auxiliary audit lookup: {error}")))?;
    let (record_count, stored_completeness, payload, stored_digest) =
        row.ok_or_else(|| invalid("named inherited Summary generation is absent"))?;
    let payload = payload.ok_or_else(|| {
        ConversationStoreError::ResourceBoundExceeded(
            "inherited auxiliary audit payload exceeds its byte bound".into(),
        )
    })?;
    let stored_digest =
        stored_digest.ok_or_else(|| invalid("invalid inherited auxiliary payload digest"))?;
    if stored_digest != digest(&payload) {
        return Err(invalid("corrupted inherited auxiliary audit payload"));
    }
    let snapshot: AuxiliaryAuditSnapshot = decode(&payload, "inherited auxiliary audit payload")?;
    let record_count = u64::try_from(record_count)
        .map_err(|_| invalid("negative inherited auxiliary record count"))?;
    if snapshot.source_conversation_id != reference.conversation_id
        || snapshot.call_id != reference.call_id
        || snapshot.through_journal != reference.through_journal
        || u64::try_from(snapshot.records.len())
            .map_err(|_| storage("inherited auxiliary record count overflow"))?
            != record_count
        || completeness_name(snapshot.completeness) != stored_completeness
    {
        return Err(invalid(
            "inherited auxiliary payload disagrees with its source key",
        ));
    }
    validate_snapshot(&snapshot)?;
    Ok(AuditedSnapshot {
        encoded_bytes: payload.len(),
        value: snapshot,
    })
}

fn validate_snapshot(snapshot: &AuxiliaryAuditSnapshot) -> Result<(), ConversationStoreError> {
    if matches!(&snapshot.origin, AuxiliaryAuditOrigin::Local) {
        return Err(invalid("invalid inherited auxiliary audit snapshot"));
    }
    validate_snapshot_facts(snapshot)
}

#[derive(serde::Serialize)]
struct BorrowedCallStart<'a> {
    call_id: &'a AuxiliaryCallId,
    purpose: &'a AuxiliaryAttribution,
    trigger: &'a AuxiliaryAttribution,
    business_source: &'a AuxiliaryBusinessSource,
    input: &'a AuxiliaryInputEvidence,
    semantic_input_fingerprint: &'a AuxiliaryFingerprint,
    invocation_fingerprint: &'a AuxiliaryFingerprint,
    selected_message_ids: &'a [MessageId],
}

fn validate_call_metadata(call: &AuxiliaryCallAudit) -> Result<(), ConversationStoreError> {
    if !(1..=64).contains(&call.call_id.as_str().len()) {
        return Err(invalid(
            "inherited Summary CallId must contain 1..=64 bytes",
        ));
    }
    if call.selected_message_ids.len() > AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "inherited Summary Call selects too many MessageIds".into(),
        ));
    }
    if call
        .selected_message_ids
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != call.selected_message_ids.len()
    {
        return Err(invalid(
            "inherited Summary selected MessageIds must be unique",
        ));
    }
    if matches!(call.input, AuxiliaryInputEvidence::Temporary)
        && !call.selected_message_ids.is_empty()
    {
        return Err(invalid(
            "temporary inherited Summary input cannot select historical MessageIds",
        ));
    }
    bounded_encoded_len(
        &BorrowedCallStart {
            call_id: &call.call_id,
            purpose: &call.purpose,
            trigger: &call.trigger,
            business_source: &call.business_source,
            input: &call.input,
            semantic_input_fingerprint: &call.semantic_input_fingerprint,
            invocation_fingerprint: &call.invocation_fingerprint,
            selected_message_ids: &call.selected_message_ids,
        },
        AUXILIARY_CALL_START_ENCODED_MAX,
    )?;
    Ok(())
}

fn validate_request_audit(request: &AuxiliaryRequestAudit) -> Result<(), ConversationStoreError> {
    if request
        .generation
        .as_ref()
        .is_some_and(|generation| !crate::model::auxiliary::generation_is_bounded(generation))
    {
        return Err(invalid(
            "inherited Summary generation timing exceeds its observation bound",
        ));
    }
    let request_id_bytes = request.request_id.as_str().len();
    if request_id_bytes == 0 || request_id_bytes > AUXILIARY_REQUEST_ID_BYTES_MAX {
        return Err(invalid(format!(
            "inherited Summary RequestId must contain 1..={AUXILIARY_REQUEST_ID_BYTES_MAX} bytes"
        )));
    }
    if request.model.is_empty() || request.model.len() > AUXILIARY_REQUEST_MODEL_BYTES_MAX {
        return Err(invalid(format!(
            "inherited Summary model must contain 1..={AUXILIARY_REQUEST_MODEL_BYTES_MAX} bytes"
        )));
    }
    if request.terminal_sequence.is_some() != request.local_terminal.is_some()
        || request.normalized_failure_kind.is_some()
            != (request.local_terminal == Some(AuxiliaryRequestTerminal::Failed))
    {
        return Err(invalid("inherited Summary terminal evidence is incomplete"));
    }
    let Some(evidence) = &request.request_evidence else {
        return Ok(());
    };
    if request.terminal_sequence.is_none()
        || evidence.request_id != request.request_id
        || evidence.usage != request.usage
    {
        return Err(invalid(
            "inherited Summary request evidence disagrees with its terminal",
        ));
    }
    match evidence.usage_completeness {
        AuxiliaryUsageCompleteness::Complete | AuxiliaryUsageCompleteness::Partial
            if evidence.usage.is_none() =>
        {
            return Err(invalid(
                "observed inherited Summary Usage completeness requires Usage evidence",
            ));
        }
        AuxiliaryUsageCompleteness::Unreported if evidence.usage.is_some() => {
            return Err(invalid(
                "unreported inherited Summary Usage cannot carry a Usage value",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn same_call_metadata(start: &AuxiliaryCallAudit, settled: &AuxiliaryCallAudit) -> bool {
    start.call_id == settled.call_id
        && start.purpose == settled.purpose
        && start.trigger == settled.trigger
        && start.business_source == settled.business_source
        && start.input == settled.input
        && start.semantic_input_fingerprint == settled.semantic_input_fingerprint
        && start.invocation_fingerprint == settled.invocation_fingerprint
        && start.selected_message_ids == settled.selected_message_ids
        && start.started_sequence == settled.started_sequence
}

fn validate_final_request(
    call_outcome: AuxiliaryCallOutcome,
    request: &AuxiliaryRequestAudit,
) -> Result<(), ConversationStoreError> {
    let local_terminal = request
        .local_terminal
        .ok_or_else(|| invalid("final inherited Summary request has no terminal"))?;
    let evidence = request.request_evidence.as_ref().ok_or_else(|| {
        invalid("final inherited Summary request settlement requires request evidence")
    })?;
    if evidence.request_id != request.request_id {
        return Err(invalid(
            "final inherited Summary request evidence names another RequestId",
        ));
    }
    if local_terminal == AuxiliaryRequestTerminal::Completed
        && !matches!(
            evidence.observed_provider_terminal,
            Some(AuxiliaryObservedProviderTerminal::Completed { .. })
        )
    {
        return Err(invalid(
            "locally completed inherited Summary request requires observed completion",
        ));
    }
    let compatible_outcome = match local_terminal {
        AuxiliaryRequestTerminal::Completed => call_outcome == AuxiliaryCallOutcome::Completed,
        AuxiliaryRequestTerminal::Failed => matches!(
            call_outcome,
            AuxiliaryCallOutcome::Failed | AuxiliaryCallOutcome::Cancelled
        ),
    };
    if !compatible_outcome {
        return Err(invalid(
            "inherited Summary Call outcome disagrees with its final request terminal",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Validate the bounded dependency closure and its cross-fact invariants together.
pub(super) fn validate_snapshot_facts(
    snapshot: &AuxiliaryAuditSnapshot,
) -> Result<(), ConversationStoreError> {
    if snapshot.through_journal == 0
        || snapshot.records.is_empty()
        || snapshot.records.len() > ResourceLimits::default().total_items
    {
        return Err(invalid("invalid inherited auxiliary audit snapshot"));
    }
    if let AuxiliaryAuditOrigin::Inherited {
        source_conversation_id,
    } = &snapshot.origin
        && source_conversation_id != &snapshot.source_conversation_id
    {
        return Err(invalid(
            "inherited auxiliary audit origin disagrees with its source Conversation",
        ));
    }
    bounded_encoded_len(snapshot, PAYLOAD_MAX)?;
    let mut seen = BTreeSet::new();
    let mut seen_offsets = BTreeSet::new();
    let mut previous = 0;
    let mut call_start = None;
    let mut call_settlement = None;
    for record in &snapshot.records {
        if record.audit_offset == 0
            || !seen_offsets.insert(record.audit_offset)
            || record.journal_sequence <= previous
            || record.journal_sequence > snapshot.through_journal
            || !seen.insert(record.journal_sequence)
        {
            return Err(invalid(
                "inherited auxiliary audit positions are invalid or future",
            ));
        }
        previous = record.journal_sequence;
        bounded_encoded_len(record, AUXILIARY_AUDIT_RECORD_ENCODED_MAX)?;
        let (call_id, purpose) = match &record.fact {
            AuxiliaryAuditFact::CallStarted(call) | AuxiliaryAuditFact::CallSettled(call) => {
                (&call.call_id, call.purpose.as_str())
            }
            AuxiliaryAuditFact::RequestStarted(request)
            | AuxiliaryAuditFact::RequestSettled(request) => (&request.call_id, "summary"),
        };
        if call_id != &snapshot.call_id || purpose != "summary" {
            return Err(invalid(
                "inherited auxiliary audit is not for the named Summary Call",
            ));
        }
        match &record.fact {
            AuxiliaryAuditFact::CallStarted(call) => {
                validate_call_metadata(call)?;
                if record.journal_sequence != call.started_sequence
                    || call.settled_sequence.is_some() != call.outcome.is_some()
                    || call_start.replace(call).is_some()
                {
                    return Err(invalid("inherited Summary start position changed"));
                }
            }
            AuxiliaryAuditFact::CallSettled(call) => {
                validate_call_metadata(call)?;
                if call.settled_sequence != Some(record.journal_sequence)
                    || call.outcome.is_none()
                    || call_settlement.replace(call).is_some()
                {
                    return Err(invalid("inherited Summary settlement position changed"));
                }
            }
            AuxiliaryAuditFact::RequestStarted(_) | AuxiliaryAuditFact::RequestSettled(_) => {}
        }
    }
    let first = snapshot
        .records
        .first()
        .ok_or_else(|| invalid("inherited Summary closure is empty"))?;
    let AuxiliaryAuditFact::CallStarted(_) = &first.fact else {
        return Err(invalid(
            "inherited Summary closure does not begin with CallStarted",
        ));
    };
    let start = call_start.ok_or_else(|| invalid("inherited Summary Call start is absent"))?;
    let has_historical_input = matches!(
        &start.input,
        crate::model::auxiliary::AuxiliaryInputEvidence::Historical { cut }
            if cut.conversation_id == snapshot.source_conversation_id
                && cut.journal.0 <= start.started_sequence
    );
    if start.purpose.as_str() != "summary"
        || !matches!(
            &start.business_source,
            AuxiliaryBusinessSource::Compaction { .. }
        )
        || !has_historical_input
        || start.started_sequence != first.journal_sequence
        || !snapshot.records.iter().any(|record| {
            matches!(
                &record.fact,
                AuxiliaryAuditFact::RequestStarted(_) | AuxiliaryAuditFact::RequestSettled(_)
            )
        })
    {
        return Err(invalid(
            "inherited Summary closure lacks physical request prerequisites",
        ));
    }
    let settled_visible = call_settlement.is_some();
    if settled_visible != (snapshot.completeness == AuxiliaryAuditCompleteness::CompleteAtCut)
        || !matches!(
            (
                settled_visible,
                snapshot.records.last().map(|record| &record.fact)
            ),
            (
                false,
                Some(AuxiliaryAuditFact::RequestStarted(_) | AuxiliaryAuditFact::RequestSettled(_))
            ) | (true, Some(AuxiliaryAuditFact::CallSettled(_)))
        )
    {
        return Err(invalid(
            "inherited Summary completeness disagrees with its retained facts",
        ));
    }
    if let Some(settled) = call_settlement
        && !same_call_metadata(start, settled)
    {
        return Err(invalid(
            "inherited Summary settlement changed Call metadata",
        ));
    }
    // Each audit item describes its identity at the captured cut, so the
    // CallStarted item also carries the terminal state visible at that cut.
    if start.settled_sequence != call_settlement.and_then(|call| call.settled_sequence)
        || start.outcome != call_settlement.and_then(|call| call.outcome)
    {
        return Err(invalid(
            "inherited Summary Call outcome disagrees across audit items",
        ));
    }
    let mut request_starts = BTreeMap::new();
    let mut request_terminals = BTreeMap::new();
    let mut request_ids = BTreeSet::new();
    for record in &snapshot.records {
        match &record.fact {
            AuxiliaryAuditFact::RequestStarted(request) => {
                validate_request_audit(request)?;
                if request_starts.insert(request.ordinal, request).is_some()
                    || request.started_sequence != record.journal_sequence
                    || !request_ids.insert(request.request_id.as_str())
                {
                    return Err(invalid("inherited Summary repeats a request start"));
                }
            }
            AuxiliaryAuditFact::RequestSettled(request) => {
                validate_request_audit(request)?;
                if request_terminals.insert(request.ordinal, request).is_some()
                    || request.terminal_sequence != Some(record.journal_sequence)
                {
                    return Err(invalid("inherited Summary repeats a request terminal"));
                }
            }
            AuxiliaryAuditFact::CallStarted(_) | AuxiliaryAuditFact::CallSettled(_) => {}
        }
    }
    if request_starts.keys().copied().collect::<Vec<_>>()
        != (0..u32::try_from(request_starts.len())
            .map_err(|_| storage("inherited Summary request ordinal overflow"))?)
            .collect::<Vec<_>>()
    {
        return Err(invalid("inherited Summary request ordinals are not dense"));
    }
    for (ordinal, request) in &request_starts {
        if request.terminal_sequence.is_some() != request_terminals.contains_key(ordinal) {
            return Err(invalid(
                "inherited Summary request terminal is absent from its closure",
            ));
        }
    }
    for (ordinal, terminal) in &request_terminals {
        let request = request_starts
            .get(ordinal)
            .ok_or_else(|| invalid("inherited Summary terminal has no request start"))?;
        if request != terminal {
            return Err(invalid(
                "inherited Summary request start and terminal disagree",
            ));
        }
    }
    if settled_visible
        && request_starts
            .keys()
            .any(|ordinal| !request_terminals.contains_key(ordinal))
    {
        return Err(invalid(
            "settled inherited Summary retains an open physical request",
        ));
    }
    let final_ordinal = request_starts
        .keys()
        .next_back()
        .copied()
        .ok_or_else(|| invalid("inherited Summary has no physical request prerequisite"))?;
    if settled_visible {
        let outcome = call_settlement
            .and_then(|call| call.outcome)
            .ok_or_else(|| invalid("inherited Summary settlement has no outcome"))?;
        let final_request = request_terminals
            .get(&final_ordinal)
            .ok_or_else(|| invalid("final inherited Summary physical request has no terminal"))?;
        if request_terminals.iter().any(|(ordinal, request)| {
            *ordinal != final_ordinal && request.request_evidence.is_some()
        }) || final_request.request_evidence.is_none()
        {
            return Err(invalid(
                "inherited Summary request evidence must name the final physical request",
            ));
        }
        validate_final_request(outcome, final_request)?;
    } else if request_starts
        .values()
        .any(|request| request.request_evidence.is_some())
    {
        return Err(invalid(
            "open inherited Summary audit cannot contain settlement evidence",
        ));
    }
    Ok(())
}

pub(super) fn read_required_snapshots(
    connection: &Connection,
    conversation_id: &ConversationId,
    checkpoints: &[ProjectionCheckpoint],
    authority_through: u64,
) -> Result<Vec<AuxiliaryAuditSnapshot>, ConversationStoreError> {
    let references = sorted_references(checkpoints);
    if references.len() > ResourceLimits::default().total_items {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "Summary generation dependency count exceeds its bound".into(),
        ));
    }
    let mut snapshots = Vec::with_capacity(references.len());
    let mut remaining_records = ResourceLimits::default().total_items;
    let mut remaining_bytes = ResourceLimits::default().total_bytes;
    for reference in references {
        if reference.through_journal == 0
            || (reference.conversation_id == *conversation_id
                && reference.through_journal > authority_through)
        {
            return Err(invalid(
                "Summary generation provenance cut is future or empty",
            ));
        }
        let snapshot = if reference.conversation_id == *conversation_id {
            local_snapshot(
                connection,
                conversation_id,
                &reference,
                remaining_records,
                remaining_bytes,
            )?
        } else {
            inherited_snapshot(connection, &reference, remaining_bytes)?
        };
        validate_snapshot_or_local(&snapshot.value)?;
        remaining_records = remaining_records
            .checked_sub(snapshot.value.records.len())
            .ok_or_else(|| {
                ConversationStoreError::ResourceBoundExceeded(
                    "Summary generation audit records exceed their cumulative bound".into(),
                )
            })?;
        remaining_bytes = remaining_bytes
            .checked_sub(snapshot.encoded_bytes)
            .ok_or_else(|| {
                ConversationStoreError::ResourceBoundExceeded(
                    "Summary generation audit bytes exceed their cumulative bound".into(),
                )
            })?;
        snapshots.push(snapshot.value);
    }
    Ok(snapshots)
}

fn validate_snapshot_or_local(
    snapshot: &AuxiliaryAuditSnapshot,
) -> Result<(), ConversationStoreError> {
    validate_snapshot_facts(snapshot)
}

fn sorted_references(checkpoints: &[ProjectionCheckpoint]) -> Vec<SummaryGenerationRef> {
    let mut references = BTreeMap::new();
    for checkpoint in checkpoints {
        if let Some(reference) = &checkpoint.candidate.summary_generation {
            references
                .entry((
                    reference.conversation_id.clone(),
                    reference.call_id.clone(),
                    reference.through_journal,
                ))
                .or_insert_with(|| reference.clone());
        }
    }
    references.into_values().collect()
}

pub(super) fn validate_local_reference(
    connection: &Connection,
    scope: &ConversationId,
    candidate: &CheckpointCandidate,
    operation_id: &crate::runtime::identity::CompactionOperationId,
) -> Result<(), ConversationStoreError> {
    let Some(reference) = &candidate.summary_generation else {
        return Ok(());
    };
    if reference.conversation_id != *scope {
        return Err(invalid(
            "new Summary generation dependency must name the local Conversation",
        ));
    }
    let authority = journal_prefix(connection)?;
    if reference.through_journal == 0 || reference.through_journal > authority {
        return Err(invalid(
            "new Summary generation dependency is outside current native authority",
        ));
    }
    let snapshot = local_snapshot(
        connection,
        scope,
        reference,
        ResourceLimits::default().total_items,
        ResourceLimits::default().total_bytes,
    )?;
    validate_snapshot_or_local(&snapshot.value)?;
    let AuxiliaryAuditFact::CallStarted(call) = &snapshot
        .value
        .records
        .first()
        .ok_or_else(|| invalid("Summary generation closure is empty"))?
        .fact
    else {
        return Err(invalid(
            "Summary generation closure does not begin with CallStarted",
        ));
    };
    if snapshot.value.completeness != AuxiliaryAuditCompleteness::CompleteAtCut
        || call.outcome != Some(AuxiliaryCallOutcome::Completed)
    {
        return Err(invalid(
            "new Summary generation requires a successfully completed Call at its declared cut",
        ));
    }
    let input_matches_capture = matches!(
        &call.input,
        crate::model::auxiliary::AuxiliaryInputEvidence::Historical {
            cut:
                crate::context::resources::ResourceReadCut {
                    conversation_id,
                    ledger,
                    transcript,
                    projection_activation,
                    ..
                }
        } if conversation_id == scope
            && *ledger <= candidate.read_cut.ledger
            && *transcript <= candidate.read_cut.transcript
            && *projection_activation <= candidate.read_cut.projection_activation
    );
    if !input_matches_capture {
        return Err(invalid(
            "Summary generation input evidence does not match the checkpoint capture",
        ));
    }
    match &call.business_source {
        AuxiliaryBusinessSource::Compaction {
            operation_id: source,
        } if source == operation_id => {}
        AuxiliaryBusinessSource::Compaction { .. } => {
            return Err(invalid(
                "Summary generation business source disagrees with the compaction operation",
            ));
        }
        AuxiliaryBusinessSource::Independent
        | AuxiliaryBusinessSource::Business { .. }
        | AuxiliaryBusinessSource::HumanMessage { .. } => {
            return Err(invalid(
                "new Summary generation requires its owning Compaction operation",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_installed_reference(
    connection: &Connection,
    scope: &ConversationId,
    candidate: &CheckpointCandidate,
) -> Result<(), ConversationStoreError> {
    let Some(reference) = &candidate.summary_generation else {
        return Ok(());
    };
    let snapshot = if reference.conversation_id == *scope {
        local_snapshot(
            connection,
            scope,
            reference,
            ResourceLimits::default().total_items,
            ResourceLimits::default().total_bytes,
        )?
    } else {
        inherited_snapshot(connection, reference, ResourceLimits::default().total_bytes)?
    };
    validate_snapshot_or_local(&snapshot.value)
}

pub(super) fn select_for_checkpoints(
    checkpoints: &[ProjectionCheckpoint],
    snapshots: &[AuxiliaryAuditSnapshot],
) -> Result<Vec<AuxiliaryAuditSnapshot>, ConversationStoreError> {
    let references = sorted_references(checkpoints);
    let mut by_key: BTreeMap<_, _> = snapshots
        .iter()
        .map(|snapshot| {
            (
                (
                    snapshot.source_conversation_id.clone(),
                    snapshot.call_id.clone(),
                    snapshot.through_journal,
                ),
                snapshot,
            )
        })
        .collect();
    if by_key.len() != snapshots.len() {
        return Err(invalid("duplicate inherited Summary audit snapshot"));
    }
    let expected: BTreeSet<_> = references
        .iter()
        .map(|reference| {
            (
                reference.conversation_id.clone(),
                reference.call_id.clone(),
                reference.through_journal,
            )
        })
        .collect();
    if by_key.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err(invalid(
            "retained checkpoint Summary dependencies and source audits differ",
        ));
    }
    Ok(references
        .into_iter()
        .filter_map(|reference| {
            by_key
                .remove(&(
                    reference.conversation_id,
                    reference.call_id,
                    reference.through_journal,
                ))
                .cloned()
        })
        .collect())
}

pub(super) fn install(
    transaction: &Transaction<'_>,
    snapshots: &[AuxiliaryAuditSnapshot],
) -> Result<(), ConversationStoreError> {
    if snapshots.is_empty() {
        return Ok(());
    }
    transaction
        .execute_batch(CREATE_SQL)
        .map_err(|error| storage(format!("create inherited auxiliary audit schema: {error}")))?;
    let mut total_bytes = 0_usize;
    for snapshot in snapshots {
        validate_snapshot_facts(snapshot)?;
        let remaining = PAYLOAD_MAX
            .checked_sub(total_bytes)
            .ok_or_else(|| storage("inherited auxiliary audit byte overflow"))?;
        let encoded_len = bounded_encoded_len(snapshot, remaining)?
            .checked_add(INHERITED_ORIGIN_OVERHEAD_MAX)
            .ok_or_else(|| storage("inherited auxiliary audit byte overflow"))?;
        if encoded_len > remaining {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "inherited Summary audit closure exceeds its byte bound".into(),
            ));
        }
        let mut inherited = snapshot.clone();
        inherited.origin = AuxiliaryAuditOrigin::Inherited {
            source_conversation_id: snapshot.source_conversation_id.clone(),
        };
        validate_snapshot(&inherited)?;
        let payload = serde_json::to_string(&inherited)
            .map_err(|error| invalid(format!("encode inherited Summary audit: {error}")))?;
        total_bytes = total_bytes
            .checked_add(encoded_len)
            .ok_or_else(|| storage("inherited auxiliary audit byte overflow"))?;
        let payload_digest = digest(&payload);
        let existing: Option<(Option<String>, Option<String>)> = transaction
            .query_row(
                "SELECT CASE WHEN octet_length(payload_json)<=?4 THEN payload_json END,
                        CASE WHEN octet_length(payload_digest)=64 THEN payload_digest END
                 FROM inherited_auxiliary_audits
                 WHERE source_conversation_id=?1 AND call_id=?2 AND through_journal=?3",
                params![
                    inherited.source_conversation_id.as_str(),
                    inherited.call_id.as_str(),
                    seq_to_i64(inherited.through_journal)?,
                    i64::try_from(PAYLOAD_MAX)
                        .map_err(|_| storage("inherited auxiliary payload bound overflow"))?
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("probe inherited Summary audit: {error}")))?;
        if let Some((stored, integrity)) = existing {
            if stored.is_none() || integrity.as_deref() != Some(payload_digest.as_str()) {
                return Err(invalid("corrupted inherited Summary audit"));
            }
            let stored = stored.expect("checked payload");
            let value: AuxiliaryAuditSnapshot =
                decode(&stored, "existing inherited Summary audit")?;
            if value != inherited {
                return Err(invalid(
                    "inherited Summary audit key was reused with different facts",
                ));
            }
            continue;
        }
        transaction
            .execute(
                "INSERT INTO inherited_auxiliary_audits(
                    source_conversation_id,call_id,through_journal,completeness,
                    record_count,payload_json,payload_digest
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    inherited.source_conversation_id.as_str(),
                    inherited.call_id.as_str(),
                    seq_to_i64(inherited.through_journal)?,
                    completeness_name(inherited.completeness),
                    i64::try_from(inherited.records.len())
                        .map_err(|_| storage("inherited auxiliary record count overflow"))?,
                    payload,
                    payload_digest,
                ],
            )
            .map_err(|error| storage(format!("insert inherited Summary audit: {error}")))?;
    }
    Ok(())
}

pub(super) fn load_all(
    connection: &Connection,
) -> Result<Vec<AuxiliaryAuditSnapshot>, ConversationStoreError> {
    let (count, bytes): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(octet_length(payload_json)),0)
             FROM inherited_auxiliary_audits",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| storage(format!("inherited Summary audit frontier: {error}")))?;
    let count =
        usize::try_from(count).map_err(|_| invalid("negative inherited Summary audit count"))?;
    let bytes = usize::try_from(bytes)
        .map_err(|_| invalid("negative inherited Summary audit byte count"))?;
    if count > ResourceLimits::default().total_items || bytes > PAYLOAD_MAX {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "inherited Summary audit closure exceeds archive bounds".into(),
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT source_conversation_id,call_id,through_journal,
                    CASE WHEN octet_length(payload_json)<=?1 THEN payload_json END,
                    CASE WHEN octet_length(payload_digest)=64 THEN payload_digest END
             FROM inherited_auxiliary_audits
             ORDER BY source_conversation_id,call_id,through_journal",
        )
        .map_err(|error| storage(format!("inherited Summary audit rows: {error}")))?;
    let rows = statement
        .query_map(
            [i64::try_from(PAYLOAD_MAX)
                .map_err(|_| storage("inherited auxiliary payload bound overflow"))?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map_err(|error| storage(format!("read inherited Summary audit: {error}")))?;
    let mut snapshots = Vec::with_capacity(count);
    for row in rows {
        let (source, call_id, through, payload, payload_digest) =
            row.map_err(|error| storage(format!("collect inherited Summary audit: {error}")))?;
        let payload = payload.ok_or_else(|| {
            ConversationStoreError::ResourceBoundExceeded(
                "inherited Summary audit payload exceeds its byte bound".into(),
            )
        })?;
        let payload_digest =
            payload_digest.ok_or_else(|| invalid("invalid inherited Summary audit digest"))?;
        if payload_digest != digest(&payload) {
            return Err(invalid("corrupted inherited Summary audit payload"));
        }
        let snapshot: AuxiliaryAuditSnapshot = decode(&payload, "inherited Summary audit")?;
        if snapshot.source_conversation_id.as_str() != source
            || snapshot.call_id.as_str() != call_id
            || seq_to_i64(snapshot.through_journal)? != through
        {
            return Err(invalid(
                "inherited Summary audit payload disagrees with its source key",
            ));
        }
        validate_snapshot(&snapshot)?;
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}

pub(super) fn validate_installed_closure(
    connection: &Connection,
    checkpoints: &[ProjectionCheckpoint],
) -> Result<(), ConversationStoreError> {
    let snapshots = load_all(connection)?;
    select_for_checkpoints(checkpoints, &snapshots)?;
    Ok(())
}

pub(super) fn verify_installed_snapshots(
    connection: &Connection,
    expected: &[AuxiliaryAuditSnapshot],
) -> Result<(), ConversationStoreError> {
    let expected: Vec<_> = expected
        .iter()
        .cloned()
        .map(|mut snapshot| {
            snapshot.origin = AuxiliaryAuditOrigin::Inherited {
                source_conversation_id: snapshot.source_conversation_id.clone(),
            };
            snapshot
        })
        .collect();
    if load_all(connection)? != expected {
        return Err(ConversationStoreError::InitialHistoryMismatch);
    }
    Ok(())
}

/// Compare every persisted checkpoint reference with the independently owned
/// inherited-source snapshots. Local references stay in the local audit domain.
#[allow(clippy::too_many_lines)] // SQL scalar scans keep the closure bounded before decoding.
pub(super) fn validate_store_closure(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    let scope: String = connection
        .query_row(
            "SELECT conversation_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("inherited Summary audit scope: {error}")))?;
    let scope = ConversationId::parse(scope).map_err(invalid)?;
    let inherited_keys = load_all(connection)?
        .into_iter()
        .map(|snapshot| {
            (
                snapshot.source_conversation_id,
                snapshot.call_id,
                snapshot.through_journal,
            )
        })
        .collect::<BTreeSet<_>>();
    let mut statement = connection
        .prepare(
            "SELECT CASE WHEN octet_length(candidate_json)<=8388608 THEN
                    json_extract(candidate_json,'$.summary_generation.conversation_id') END,
                 CASE WHEN octet_length(candidate_json)<=8388608 THEN
                    json_extract(candidate_json,'$.summary_generation.call_id') END,
                 CASE WHEN octet_length(candidate_json)<=8388608 THEN
                    json_extract(candidate_json,'$.summary_generation.through_journal') END
             FROM projection_checkpoints
             WHERE json_extract(candidate_json,'$.summary_generation.call_id') IS NOT NULL
             ORDER BY checkpoint_id",
        )
        .map_err(|error| storage(format!("checkpoint Summary audit closure: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .map_err(|error| storage(format!("read checkpoint Summary audit closure: {error}")))?;
    let mut expected = BTreeSet::new();
    let mut local = BTreeSet::new();
    for row in rows {
        let (source, call, through) =
            row.map_err(|error| storage(format!("collect checkpoint Summary audit: {error}")))?;
        let source = source.ok_or_else(|| invalid("invalid Summary source identity"))?;
        let call = call.ok_or_else(|| invalid("invalid Summary Call identity"))?;
        let through = through.ok_or_else(|| invalid("invalid Summary provenance cut"))?;
        let source = ConversationId::parse(source).map_err(invalid)?;
        let call = AuxiliaryCallId::parse(call).map_err(invalid)?;
        let through = sequence_from_i64(through)?;
        if source == scope {
            local.insert((call, through));
        } else {
            expected.insert((source, call, through));
        }
    }
    if expected != inherited_keys {
        return Err(invalid(
            "inherited Summary audit closure differs from retained checkpoints",
        ));
    }
    let mut remaining_records = ResourceLimits::default().total_items;
    let mut remaining_bytes = ResourceLimits::default().total_bytes;
    for (call_id, through_journal) in local {
        let snapshot = local_snapshot(
            connection,
            &scope,
            &SummaryGenerationRef {
                conversation_id: scope.clone(),
                call_id,
                through_journal,
            },
            remaining_records,
            remaining_bytes,
        )?;
        validate_snapshot_or_local(&snapshot.value)?;
        remaining_records = remaining_records
            .checked_sub(snapshot.value.records.len())
            .ok_or_else(|| {
                ConversationStoreError::ResourceBoundExceeded(
                    "local Summary audit records exceed archive bounds".into(),
                )
            })?;
        remaining_bytes = remaining_bytes
            .checked_sub(snapshot.encoded_bytes)
            .ok_or_else(|| {
                ConversationStoreError::ResourceBoundExceeded(
                    "local Summary audit bytes exceed archive bounds".into(),
                )
            })?;
    }
    Ok(())
}

/// Archive-facing local/imported projection. Local records remain a separate
/// audit capability; imported snapshots are never presented as local execution.
pub(super) fn archive_rows(
    connection: &Connection,
) -> Result<Vec<serde_json::Value>, ConversationStoreError> {
    let mut rows = Vec::new();
    for snapshot in load_all(connection)? {
        let AuxiliaryAuditSnapshot {
            origin,
            source_conversation_id,
            call_id,
            through_journal,
            completeness,
            records,
        } = snapshot;
        let omitted_after_cut: &[&str] = match completeness {
            AuxiliaryAuditCompleteness::CompleteAtCut => &[],
            AuxiliaryAuditCompleteness::OpenAtCut => {
                &["later_call_settlement", "later_request_outcome"]
            }
        };
        for record in records {
            rows.push(serde_json::json!({
                "origin": &origin,
                "source_conversation_id": &source_conversation_id,
                "call_id": &call_id,
                "through_journal": through_journal,
                "completeness": completeness,
                "omitted_after_cut": omitted_after_cut,
                "audit_offset": record.audit_offset,
                "journal_sequence": record.journal_sequence,
                "fact": record.fact,
            }));
        }
    }
    Ok(rows)
}
