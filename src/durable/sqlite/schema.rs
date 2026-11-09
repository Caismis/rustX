//! Opening, creating, binding, and validating the native SQLite schema.
//!
//! Every constructor validates an existing database before changing it and
//! creates the development schema only for a new file; there is no migration
//! or legacy reader.

use super::{ConnectionSlot, event_insert};
use super::{observation_readiness, observation_response, observation_statistics};
use crate::durable::sqlite::statements::FixedStatements;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
#[cfg(test)]
mod open_benchmark;
#[cfg(test)]
mod tests;

use crate::durable::sqlite::auxiliary;
use crate::durable::sqlite::request_owner_shape;
use crate::events::RuntimeEvent;
use crate::events::RuntimeEventEnvelope;
use crate::model::ModelRequestOwner;
use crate::runtime::RequestId;
use rusqlite::OptionalExtension;
pub(super) mod shape;

use self::shape::verify_schema_shape;
use super::{
    ConnectionOwner, SQLITE_SCHEMA_VERSION, SqliteConversationStore, append_validation, content,
    count_conversation_store_open, manifest, observation_request, projection, seq_to_i64, storage,
    writer_busy_policy,
};
use crate::durable::ConversationStoreError;
use crate::runtime::identity::ConversationId;
use rusqlite::{Connection, TransactionBehavior, params};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};

pub(super) fn register_integrity_function(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    use rusqlite::functions::FunctionFlags;
    connection
        .create_scalar_function(
            "rustx_content_digest",
            1,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                use sha2::{Digest, Sha256};
                let bytes = context
                    .get_raw(0)
                    .as_str()
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;
                Ok(format!("{:x}", Sha256::digest(bytes.as_bytes())))
            },
        )
        .map_err(|e| storage(e.to_string()))
}

fn register_native_functions(connection: &Connection) -> Result<(), ConversationStoreError> {
    use rusqlite::functions::FunctionFlags;
    connection
        .create_scalar_function(
            "rustx_content_digest",
            1,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                use sha2::{Digest, Sha256};
                let bytes = context
                    .get_raw(0)
                    .as_str()
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;
                Ok(format!("{:x}", Sha256::digest(bytes.as_bytes())))
            },
        )
        .map_err(|e| storage(e.to_string()))?;
    content::register_tail_function(connection)
}

pub(super) fn configure_connection(
    connection: &mut Connection,
    in_memory: bool,
) -> Result<(), ConversationStoreError> {
    register_native_functions(connection)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL; PRAGMA page_size = 2048;",
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("configure SQLite: {error}"))
        })?;
    writer_busy_policy(connection)
        .map_err(|error| storage(format!("configure SQLite busy policy: {error}")))?;
    if !in_memory {
        let mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("configure SQLite journal: {error}"),
                )
            })?;
        if mode != "wal" {
            return Err(storage(format!("SQLite refused WAL journal mode: {mode}")));
        }
    }
    Ok(())
}

pub(super) fn validate_existing_identity(
    connection: &Connection,
    conversation_id: &ConversationId,
) -> Result<(), ConversationStoreError> {
    let stored: String = connection
        .query_row_cached(
            "SELECT conversation_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("read existing identity: {error}"))
        })?;
    if stored != conversation_id.as_str() {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: ConversationId::parse(stored).map_err(storage)?,
            requested: conversation_id.clone(),
        });
    }
    Ok(())
}

pub(super) fn reject_legacy_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    let legacy: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='inbox_meta'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("probe schema: {error}"))
        })?;
    if legacy.is_some() {
        return Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 0,
            expected: SQLITE_SCHEMA_VERSION,
        });
    }
    if !has_table(connection, "rustx_store")? {
        let unrelated: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("probe partial schema: {error}")))?;
        if let Some(table) = unrelated {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "database contains table {table} but no rustx_store schema root"
            )));
        }
    }
    Ok(())
}

pub(super) fn has_table(
    connection: &Connection,
    table: &str,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("probe table {table}: {error}"))
        })
}

pub(super) fn validate_existing_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    let version: i64 = connection
        .query_row(
            "SELECT schema_version FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("read existing schema version: {error}"),
            )
        })?;
    if version != SQLITE_SCHEMA_VERSION {
        return Err(ConversationStoreError::SchemaVersionMismatch {
            stored: version,
            expected: SQLITE_SCHEMA_VERSION,
        });
    }
    verify_schema_shape(connection)
}

// This is an index discriminator, not a second occupancy fact. The Journal
// continues to own the usage payload and the compaction's explicit absence.
// A primary terminal qualifies only with an actual input measurement, including
// zero. Output-only Usage must preserve the previous reading before LIMIT.
pub(super) const CONTEXT_READING_PREDICATE: &str = "json_extract(event_json, '$.event.type') = 'compaction_completed' OR (COALESCE(json_type(event_json, '$.event.usage.input_tokens'), 'null') != 'null' AND json_extract(event_json, '$.event.owner.type') = 'agent')";

#[derive(Clone, Copy)]
pub(super) enum PrimaryRequestFactKind {
    Started,
    Completed,
    Failed,
}

impl PrimaryRequestFactKind {
    fn terminal_kind(self) -> Option<&'static str> {
        match self {
            Self::Started => None,
            Self::Completed => Some("completed"),
            Self::Failed => Some("failed"),
        }
    }
}

pub(super) fn validate_primary_request_owner_event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let (request_id, identity, kind) = match &event.event {
        RuntimeEvent::ModelRequestStarted {
            owner: ModelRequestOwner::Agent { identity },
            request_id,
            ..
        } => (request_id, identity, PrimaryRequestFactKind::Started),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Agent { identity },
            request_id,
            ..
        } => (request_id, identity, PrimaryRequestFactKind::Completed),
        RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Agent { identity },
            request_id,
            ..
        } => (request_id, identity, PrimaryRequestFactKind::Failed),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "AgentRequest scope selected a fact without primary Agent ownership".into(),
            ));
        }
    };
    validate_primary_request_owner_fact(connection, request_id, identity, event.sequence, kind)
}

pub(super) fn validate_primary_request_owner_fact(
    connection: &Connection,
    request_id: &RequestId,
    identity: &crate::model::snapshot::RequestIdentity,
    sequence: u64,
    kind: PrimaryRequestFactKind,
) -> Result<(), ConversationStoreError> {
    type PrimaryOwnerRow = (
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        i64,
        Option<i64>,
        Option<String>,
    );
    if request_id != &identity.request_id() {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Agent request {request_id} does not match its canonical RequestIdentity"
        )));
    }
    let indexed: Option<PrimaryOwnerRow> = connection
        .query_row(
            "SELECT owner_kind,attempt_id,turn_id,retry_number,start_sequence,
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
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("primary request owner lookup: {error}"),
            )
        })?;
    let Some((
        owner_kind,
        attempt_id,
        turn_id,
        retry_number,
        start_sequence,
        terminal_sequence,
        terminal_kind,
    )) = indexed
    else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Agent request {request_id} fact has no mandatory owner row"
        )));
    };
    if owner_kind != "agent" {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Agent request {request_id} owner row has a foreign owner kind"
        )));
    }
    if attempt_id.as_deref() != Some(identity.attempt_id.as_str())
        || turn_id.as_deref() != Some(identity.turn.as_str())
        || retry_number != Some(i64::from(identity.retry_number))
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Agent request {request_id} owner identity disagrees with its canonical fact"
        )));
    }
    let sequence = seq_to_i64(sequence)?;
    match kind {
        PrimaryRequestFactKind::Started if start_sequence != sequence => {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Agent request {request_id} owner start sequence disagrees with its canonical fact"
            )));
        }
        PrimaryRequestFactKind::Completed | PrimaryRequestFactKind::Failed
            if terminal_sequence != Some(sequence)
                || terminal_kind.as_deref() != kind.terminal_kind() =>
        {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Agent request {request_id} owner terminal sequence or kind disagrees with its canonical fact"
            )));
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Closed indexed scope vocabulary in one place.
/// The actual bounded SQL used by the reader and its query-plan contract tests.
pub(super) fn presentation_statements(
    query: &crate::durable::presentation::FactQuery,
) -> Result<Vec<(String, Vec<rusqlite::types::Value>)>, ConversationStoreError> {
    use crate::durable::presentation::FactScope;
    use rusqlite::types::Value;
    if query.limit == 0 || query.limit > 129 || query.kinds.is_empty() || query.kinds.len() > 32 {
        return Err(storage("invalid presentation fact query bound"));
    }
    let mut values = vec![
        Value::Integer(seq_to_i64(query.through)?),
        Value::Integer(seq_to_i64(
            query.before.unwrap_or(query.through.saturating_add(1)),
        )?),
    ];
    let scope = match &query.scope {
        FactScope::All => String::new(),
        FactScope::AgentRequest => {
            " AND json_extract(event_json, '$.event.owner.type') = 'agent'".to_owned()
        }
        FactScope::ContextReading => format!(" AND ({CONTEXT_READING_PREDICATE}) = 1"),
        FactScope::Compaction(operation) => {
            values.push(Value::Text(operation.to_string()));
            format!(
                " AND json_extract(event_json, '$.event.operation_id') = ?{}",
                values.len()
            )
        }
        FactScope::Step(attempt, turn) => {
            values.push(Value::Text(attempt.to_string()));
            values.push(Value::Text(turn.to_string()));
            " AND attempt_id = ?3 AND turn_id = ?4".to_owned()
        }
        FactScope::Attempt(attempt) => {
            values.push(Value::Text(attempt.to_string()));
            " AND attempt_id = ?3".to_owned()
        }
        FactScope::ToolCall {
            call_id,
            attempt,
            turn,
        } => {
            values.push(Value::Text(call_id.clone()));
            values.push(
                attempt
                    .as_ref()
                    .map_or(Value::Null, |id| Value::Text(id.to_string())),
            );
            values.push(
                turn.as_ref()
                    .map_or(Value::Null, |id| Value::Text(id.to_string())),
            );
            " AND json_extract(event_json, '$.event.tool_call_id') = ?3 AND attempt_id IS ?4 AND turn_id IS ?5".to_owned()
        }
        other => {
            let (field, value) = match other {
                FactScope::Request(value) => ("request_id", value),
                FactScope::Compaction(_) => unreachable!("compaction scope is handled separately"),
                FactScope::Execution(value) => ("execution_id", value),
                FactScope::Subagent(value) => ("subagent_id", value),
                FactScope::Workflow(value) => ("run_id", value),
                FactScope::Interaction(value) => ("interaction_id", value),
                FactScope::Message(value) => ("message_id", value),
                FactScope::All
                | FactScope::AgentRequest
                | FactScope::ContextReading
                | FactScope::Step(..)
                | FactScope::Attempt(..)
                | FactScope::ToolCall { .. } => unreachable!(),
            };
            values.push(Value::Text(value.clone()));
            format!(" AND json_extract(event_json, '$.event.{field}') = ?3")
        }
    };
    let index = match query.scope {
        FactScope::All => "events_kind_idx",
        FactScope::AgentRequest => "events_primary_request_kind_idx",
        FactScope::ContextReading => "events_context_reading_idx",
        FactScope::Message(..) => "events_message_id_kind_idx",
        FactScope::Attempt(..) => "events_attempt_kind_idx",
        FactScope::Step(..) => "events_step_kind_idx",
        FactScope::Request(..) => "events_request_id_kind_idx",
        FactScope::Compaction(..) => "events_compaction_operation_kind_idx",
        FactScope::ToolCall { .. } => "events_tool_call_scope_kind_idx",
        FactScope::Execution(..) => "events_execution_id_kind_idx",
        FactScope::Subagent(..) => "events_subagent_id_kind_idx",
        FactScope::Workflow(..) => "events_run_id_kind_idx",
        FactScope::Interaction(..) => "events_interaction_id_kind_idx",
    };
    // Index selection is part of the bounded read contract, independent of
    // SQLite statistics and unrelated kind cardinalities.
    let mut statements = Vec::new();
    for kind in &query.kinds {
        let mut params = values.clone();
        params.push(Value::Text((*kind).to_owned()));
        let kind_param = params.len();
        params.push(Value::Integer(seq_to_i64(query.after)?));
        let after_param = params.len();
        params.push(Value::Integer(
            i64::try_from(query.limit).map_err(|_| storage("fact limit"))?,
        ));
        let direction = if query.ascending { "ASC" } else { "DESC" };
        let sql = format!("SELECT event_json FROM events INDEXED BY {index} WHERE sequence <= ?1 AND sequence < ?2{scope}
                AND json_extract(event_json, '$.event.type') = ?{kind_param} AND sequence > ?{after_param}
                ORDER BY sequence {direction} LIMIT ?{}", params.len());
        statements.push((sql, params));
    }
    Ok(statements)
}

// Full index suffixes, including predicates that every matching reader implies.
pub(super) const PRESENTATION_INDEXES: &[(&str, &str)] = &[
    (
        "events_primary_request_kind_idx",
        "(json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.owner.type') = 'agent'",
    ),
    (
        "events_context_reading_idx",
        "(json_extract(event_json, '$.event.type'), (json_extract(event_json, '$.event.type') = 'compaction_completed' OR (COALESCE(json_type(event_json, '$.event.usage.input_tokens'), 'null') != 'null' AND json_extract(event_json, '$.event.owner.type') = 'agent'))) WHERE (json_extract(event_json, '$.event.type') = 'compaction_completed' OR (COALESCE(json_type(event_json, '$.event.usage.input_tokens'), 'null') != 'null' AND json_extract(event_json, '$.event.owner.type') = 'agent')) = 1",
    ),
    (
        "events_message_id_kind_idx",
        "(json_extract(event_json, '$.event.message_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.message_id') IS NOT NULL",
    ),
    (
        "events_kind_idx",
        "(json_extract(event_json, '$.event.type'))",
    ),
    (
        "events_attempt_kind_idx",
        "(attempt_id, json_extract(event_json, '$.event.type'))",
    ),
    (
        "events_step_kind_idx",
        "(attempt_id, turn_id, json_extract(event_json, '$.event.type')) WHERE attempt_id IS NOT NULL AND turn_id IS NOT NULL",
    ),
    (
        "events_compaction_operation_kind_idx",
        "(json_extract(event_json, '$.event.operation_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.operation_id') IS NOT NULL",
    ),
    (
        "events_request_id_kind_idx",
        "(json_extract(event_json, '$.event.request_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.request_id') IS NOT NULL",
    ),
    (
        "events_tool_call_scope_kind_idx",
        "(json_extract(event_json, '$.event.tool_call_id'), attempt_id, turn_id, json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.tool_call_id') IS NOT NULL",
    ),
    (
        "events_execution_id_kind_idx",
        "(json_extract(event_json, '$.event.execution_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.execution_id') IS NOT NULL",
    ),
    (
        "events_subagent_id_kind_idx",
        "(json_extract(event_json, '$.event.subagent_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.subagent_id') IS NOT NULL",
    ),
    (
        "events_run_id_kind_idx",
        "(json_extract(event_json, '$.event.run_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.run_id') IS NOT NULL",
    ),
    (
        "events_workflow_coordinates_idx",
        "(json_extract(event_json,'$.event.run_id.conversation_id'),json_extract(event_json,'$.event.run_id.origin.type'),coalesce(json_extract(event_json,'$.event.run_id.origin.attempt_id'),json_extract(event_json,'$.event.run_id.origin.start_id')),json_extract(event_json,'$.event.run_id.invocation')) WHERE json_extract(event_json,'$.event.type')='workflow_started'",
    ),
    (
        "events_workflow_node_run_kind_idx",
        "(json_extract(event_json, '$.event.instance.block.run'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.instance.block.run') IS NOT NULL",
    ),
    (
        "events_workflow_node_instance_kind_idx",
        "(json_extract(event_json, '$.event.instance'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.instance') IS NOT NULL",
    ),
    (
        "events_interaction_id_kind_idx",
        "(json_extract(event_json, '$.event.interaction_id'), json_extract(event_json, '$.event.type')) WHERE json_extract(event_json, '$.event.interaction_id') IS NOT NULL",
    ),
];

#[allow(clippy::too_many_lines)] // One fixed development schema; no migrations.
pub(super) fn create_schema(connection: &mut Connection) -> Result<(), ConversationStoreError> {
    register_native_functions(connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("create schema transaction: {error}"),
            )
        })?;
    let connection = &transaction;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS rustx_store (
                id INTEGER PRIMARY KEY CHECK(id=1),
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                next_inbound_sequence INTEGER NOT NULL CHECK(next_inbound_sequence >= 0),
                next_event_sequence INTEGER NOT NULL CHECK(next_event_sequence >= 0),
                next_transcript_position INTEGER NOT NULL CHECK(next_transcript_position >= 0),
                retained_transcript_count INTEGER NOT NULL DEFAULT 0 CHECK(retained_transcript_count >= 0),
                pending_observation_count INTEGER NOT NULL DEFAULT 0 CHECK(pending_observation_count >= 0),
                observation_statistics BLOB CHECK(observation_statistics IS NULL OR length(observation_statistics)<=49152),
                reading_mutation_revision INTEGER NOT NULL DEFAULT 0 CHECK(reading_mutation_revision >= 0),
                logical_step_sequence INTEGER NOT NULL CHECK(logical_step_sequence >= 0),
                pending_unresolved_output_stream_id TEXT,
                bootstrap_initialized INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_initialized IN (0,1))
            );
            CREATE TABLE IF NOT EXISTS observation_block_headers (
                ledger_position INTEGER NOT NULL REFERENCES message_ledger(position),
                block_index INTEGER NOT NULL CHECK(block_index>=0),
                header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                message_id TEXT NOT NULL REFERENCES message_ledger(message_id),
                PRIMARY KEY(ledger_position,block_index),UNIQUE(message_id,block_index)
            );
            CREATE TABLE IF NOT EXISTS audit_observation_blocks (
                transcript_position INTEGER NOT NULL REFERENCES transcript_order(position) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                PRIMARY KEY(transcript_position,ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS observation_headers (
                header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                tool_result_json BLOB CHECK(tool_result_json IS NULL OR length(tool_result_json)<=16384),
                transcript_position INTEGER UNIQUE,ledger_position INTEGER,
                journal_position INTEGER NOT NULL DEFAULT 0 CHECK(journal_position>=0),
                reference_kind TEXT NOT NULL, reference_id TEXT NOT NULL,
                PRIMARY KEY(reference_kind,reference_id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS observation_headers_ledger_idx ON observation_headers(ledger_position) WHERE ledger_position IS NOT NULL;
            CREATE TABLE IF NOT EXISTS surface_observation_boundaries (
                ordinal INTEGER PRIMARY KEY,ledger_position INTEGER NOT NULL UNIQUE,surface_revision INTEGER NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS content_segments (
                source_kind TEXT NOT NULL, source_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                representation TEXT NOT NULL, segment INTEGER NOT NULL, bytes BLOB NOT NULL CHECK(length(bytes)<=65536),
                PRIMARY KEY(source_kind,source_id,block_index,representation,segment)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS content_frontiers (
                source_kind TEXT NOT NULL, source_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                representation TEXT NOT NULL, revision INTEGER NOT NULL, readable_end INTEGER NOT NULL,
                bytes BLOB CHECK(bytes IS NULL OR length(bytes)<=65536),
                PRIMARY KEY(source_kind,source_id,block_index,representation,revision)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS publication_metadata_segments (
                stream_id TEXT NOT NULL,block_index INTEGER NOT NULL,revision INTEGER NOT NULL,
                segment INTEGER NOT NULL,bytes BLOB NOT NULL CHECK(length(bytes)<=65536),
                PRIMARY KEY(stream_id,block_index,revision,segment)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS publication_observation_blocks (
                stream_id TEXT NOT NULL, block_index INTEGER NOT NULL, opened_revision INTEGER NOT NULL,
                creation_ordinal INTEGER NOT NULL,tool_ordinal INTEGER,kind TEXT NOT NULL, PRIMARY KEY(stream_id,block_index),UNIQUE(stream_id,creation_ordinal),UNIQUE(stream_id,tool_ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS publication_observation_counts (
                stream_id TEXT NOT NULL, revision INTEGER NOT NULL, total INTEGER NOT NULL,tool_total INTEGER NOT NULL,
                PRIMARY KEY(stream_id,revision)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS goal_state (
                observation_header BLOB CHECK(observation_header IS NULL OR length(observation_header)<=16384),
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                observation_source_id TEXT, observation_sequence INTEGER,
                snapshot_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS observation_block_ledger_kind_idx ON observation_block_headers(ledger_position,json_extract(header_json,'$.kind'),block_index);
            CREATE INDEX IF NOT EXISTS observation_block_kind_idx ON observation_block_headers(message_id,json_extract(header_json,'$.kind'),block_index);
            CREATE TABLE IF NOT EXISTS observation_models (
                owner_anchor INTEGER NOT NULL REFERENCES events(sequence),
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                first_sequence INTEGER NOT NULL REFERENCES events(sequence) ON DELETE CASCADE,
                request_alias TEXT CHECK(request_alias IS NULL OR length(CAST(request_alias AS BLOB))<=512),
                request_encoded_bytes INTEGER NOT NULL CHECK(request_encoded_bytes>=2),
                model_hash BLOB NOT NULL CHECK(length(model_hash)=32),
                encoded_bytes INTEGER NOT NULL CHECK(encoded_bytes>=0),
                attempt_id TEXT NOT NULL,
                PRIMARY KEY(attempt_id,ordinal),
                UNIQUE(attempt_id,model_hash)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS observation_models_owner_idx ON observation_models(owner_anchor,ordinal);
            CREATE TABLE IF NOT EXISTS pending_inbound (
                sequence INTEGER PRIMARY KEY,
                observation_header BLOB CHECK(observation_header IS NULL OR length(observation_header)<=16384),
                observation_transcript INTEGER,
                revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
                message_id TEXT NOT NULL UNIQUE,
                message_json TEXT NOT NULL,
                correlation TEXT
            );
            CREATE TABLE IF NOT EXISTS inbound_correlation (
                correlation TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL,
                message_id TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS message_ledger (
                observation_trace_summary BLOB CHECK(observation_trace_summary IS NULL OR length(observation_trace_summary)<=16384),
                observation_identity_digest BLOB CHECK(observation_identity_digest IS NULL OR length(observation_identity_digest)=32),
                observation_delivery_success INTEGER CHECK(observation_delivery_success IS NULL OR observation_delivery_success IN (0,1)),
                observation_delivery_total INTEGER CHECK(observation_delivery_total IS NULL OR observation_delivery_total>=0),
                native_transcript INTEGER,
                human_activity_at TEXT,
                human_activity_seconds INTEGER,
                human_activity_nanos INTEGER,
                opening_preview TEXT,
                ordinary_user INTEGER NOT NULL DEFAULT 0 CHECK(ordinary_user IN (0,1)),
                ordinary_human INTEGER NOT NULL DEFAULT 0 CHECK(ordinary_human IN (0,1)),
                title_source_text TEXT CHECK(title_source_text IS NULL OR length(CAST(title_source_text AS BLOB))<=4096),
                native_todo INTEGER NOT NULL DEFAULT 0 CHECK(native_todo IN (0,1)),
                boundary_indexed INTEGER NOT NULL DEFAULT 0 CHECK(boundary_indexed IN (0,1)),
                first_append_revision INTEGER,
                position INTEGER PRIMARY KEY,
                message_id TEXT NOT NULL UNIQUE,
                message_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS message_ledger_first_human_idx ON message_ledger(position) WHERE ordinary_human=1;
            CREATE UNIQUE INDEX IF NOT EXISTS message_ledger_identity_position_idx ON message_ledger(position,message_id);
            CREATE UNIQUE INDEX IF NOT EXISTS message_ledger_native_transcript_idx ON message_ledger(native_transcript) WHERE native_transcript IS NOT NULL;
            CREATE TABLE IF NOT EXISTS canonical_tool_calls (
                observation_trace_proposal BLOB CHECK(observation_trace_proposal IS NULL OR length(observation_trace_proposal)<=16384),
                observation_tool_identity_digest BLOB CHECK(observation_tool_identity_digest IS NULL OR length(observation_tool_identity_digest)=32),
                observation_definition_ready INTEGER NOT NULL DEFAULT 0 CHECK(observation_definition_ready IN (0,1)),
                observation_definition_start INTEGER REFERENCES events(sequence),
                observation_definition_commit INTEGER REFERENCES events(sequence),
                observation_definition_ordinal INTEGER CHECK(observation_definition_ordinal IS NULL OR observation_definition_ordinal>=0),
                observation_ledger INTEGER NOT NULL,observation_tool_ordinal INTEGER NOT NULL,observation_call_id TEXT,observation_tool_id TEXT,observation_name TEXT NOT NULL, result_ledger_position INTEGER,
                block_index INTEGER NOT NULL CHECK(block_index >= 0),
                assistant_message_id TEXT NOT NULL REFERENCES message_ledger(message_id),
                call_id TEXT NOT NULL,
                tool_id TEXT NOT NULL,
                attempt_id TEXT,
                commit_sequence INTEGER,
                result_message_id TEXT UNIQUE REFERENCES message_ledger(message_id),
                PRIMARY KEY(assistant_message_id,block_index),
                UNIQUE(assistant_message_id,call_id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS canonical_tool_calls_observation_idx ON canonical_tool_calls(observation_ledger,block_index);
            CREATE UNIQUE INDEX IF NOT EXISTS canonical_tool_calls_result_ledger_idx ON canonical_tool_calls(result_ledger_position) WHERE result_ledger_position IS NOT NULL;
            CREATE INDEX IF NOT EXISTS canonical_tool_calls_scope_idx ON canonical_tool_calls(attempt_id,call_id,commit_sequence,assistant_message_id,block_index);
            CREATE INDEX IF NOT EXISTS canonical_tool_calls_pending
                ON canonical_tool_calls(assistant_message_id,block_index) WHERE result_message_id IS NULL;
            CREATE TABLE IF NOT EXISTS transcript_order (
                position INTEGER PRIMARY KEY,
                reference_kind TEXT NOT NULL,
                reference_id TEXT NOT NULL,
                observation_readiness_missing INTEGER NOT NULL DEFAULT 1 CHECK(observation_readiness_missing IN (0,1)),
                UNIQUE(reference_kind,reference_id)
            );
            CREATE INDEX IF NOT EXISTS transcript_observation_readiness_missing_idx ON transcript_order(position) WHERE observation_readiness_missing=1;
            CREATE TABLE IF NOT EXISTS bootstrap_identity (
                occupancy_observation BLOB CHECK(occupancy_observation IS NULL OR length(occupancy_observation)<=16384),
                id INTEGER PRIMARY KEY CHECK(id=1),
                message_count INTEGER NOT NULL,
                history_digest TEXT NOT NULL,
                occupancy_json TEXT NOT NULL,
                occupancy_digest TEXT NOT NULL,
                turn_count INTEGER NOT NULL CHECK(turn_count>=0),
                response_count INTEGER NOT NULL CHECK(response_count>=0),
                provenance_digest TEXT NOT NULL,
                inherited_transcript_through INTEGER NOT NULL CHECK(inherited_transcript_through >= 0),
                projection_count INTEGER NOT NULL DEFAULT 0 CHECK(projection_count>=0),
                projection_digest TEXT NOT NULL DEFAULT '',
                auxiliary_audit_count INTEGER NOT NULL DEFAULT 0 CHECK(auxiliary_audit_count>=0),
                auxiliary_audit_digest TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS inherited_turn_provenance (
                observation_turn_display_witness BLOB CHECK(observation_turn_display_witness IS NULL OR length(observation_turn_display_witness)=32),
                observation_json_witness BLOB CHECK(observation_json_witness IS NULL OR length(observation_json_witness)=32),
                observation_turn_display BLOB CHECK(observation_turn_display IS NULL OR length(observation_turn_display)<=16384),
                display_provenance BLOB CHECK(display_provenance IS NULL OR length(display_provenance)<=16384),
                observation_members BLOB CHECK(observation_members IS NULL OR length(observation_members)<=49152),
                observation_json BLOB CHECK(observation_json IS NULL OR length(observation_json)<=16384),
                ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),
                payload_digest TEXT NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS inherited_turn_identity_idx ON inherited_turn_provenance(json_extract(payload,'$.id.conversation_id'),json_extract(payload,'$.id.attempt_id'));
            CREATE TABLE IF NOT EXISTS inherited_response_provenance (
                closing_ledger_position INTEGER NOT NULL UNIQUE,
                display_provenance BLOB CHECK(display_provenance IS NULL OR length(display_provenance)<=16384),
                ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),
                payload TEXT NOT NULL,
                payload_digest TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS inherited_response_closing_idx ON inherited_response_provenance(json_extract(payload,'$.closing_message_id'));
            CREATE INDEX IF NOT EXISTS inherited_response_owner_idx ON inherited_response_provenance(json_extract(payload,'$.origin.conversation_id'),json_extract(payload,'$.origin.attempt_id'),ordinal);
            CREATE TABLE IF NOT EXISTS inherited_message_membership (
                observation_identity_digest BLOB CHECK(observation_identity_digest IS NULL OR length(observation_identity_digest)=32),
                observation_membership_witness BLOB CHECK(observation_membership_witness IS NULL OR length(observation_membership_witness)=32),
                assistant_ledger_position INTEGER NOT NULL UNIQUE,
                turn_ordinal INTEGER REFERENCES inherited_turn_provenance(ordinal),
                response_ordinal INTEGER REFERENCES inherited_response_provenance(ordinal),
                membership_digest TEXT NOT NULL,
                assistant_message_id TEXT PRIMARY KEY REFERENCES message_ledger(message_id),
                FOREIGN KEY(assistant_ledger_position,assistant_message_id) REFERENCES message_ledger(position,message_id)
            );
            CREATE INDEX IF NOT EXISTS inherited_membership_turn_idx ON inherited_message_membership(turn_ordinal,assistant_message_id);
            CREATE TABLE IF NOT EXISTS surface_ops (
                revision INTEGER PRIMARY KEY,
                compaction_generation INTEGER NOT NULL,
                op_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS surface_head (
                id INTEGER PRIMARY KEY CHECK(id=1),
                revision INTEGER NOT NULL,
                compaction_generation INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projection_checkpoints (
                checkpoint_id TEXT PRIMARY KEY,
                candidate_fingerprint TEXT NOT NULL,
                candidate_json TEXT NOT NULL,
                occupancy_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projection_activations (
                activation INTEGER PRIMARY KEY CHECK(activation>0),
                checkpoint_id TEXT NOT NULL REFERENCES projection_checkpoints(checkpoint_id),
                ledger_frontier INTEGER NOT NULL CHECK(ledger_frontier>=0),
                visible_journal INTEGER NOT NULL CHECK(visible_journal>=0),
                installation_id TEXT,
                header_digest TEXT NOT NULL,
                activation_digest TEXT NOT NULL,
                observation_header_witness BLOB CHECK(observation_header_witness IS NULL OR length(observation_header_witness)=32),
                observation_header BLOB CHECK(observation_header IS NULL OR length(observation_header)<=16384),
                activation_json TEXT NOT NULL,
                CHECK((installation_id IS NULL AND visible_journal>0) OR (installation_id IS NOT NULL AND visible_journal=0))
            );
            CREATE INDEX IF NOT EXISTS projection_ledger_idx ON projection_activations(ledger_frontier,activation);
            CREATE UNIQUE INDEX IF NOT EXISTS projection_checkpoint_idx ON projection_activations(checkpoint_id);
            CREATE TABLE IF NOT EXISTS projection_head (
                id INTEGER PRIMARY KEY CHECK(id=1),
                activation INTEGER NOT NULL REFERENCES projection_activations(activation)
            );
            CREATE TABLE IF NOT EXISTS compaction_operations (
                observation_receipt_witness BLOB CHECK(observation_receipt_witness IS NULL OR length(observation_receipt_witness)=32),
                observation_receipt BLOB CHECK(observation_receipt IS NULL OR length(observation_receipt)<=65536),
                operation_id TEXT PRIMARY KEY,
                candidate_fingerprint TEXT NOT NULL,
                receipt_digest TEXT NOT NULL,
                receipt_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS request_configurations (
                digest TEXT PRIMARY KEY CHECK(typeof(digest)='text' AND octet_length(digest)=64),
                configuration_json TEXT NOT NULL CHECK(typeof(configuration_json)='text' AND octet_length(configuration_json)<=67108864)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS request_snapshots (
                observation_step_anchor INTEGER, observation_retry_number INTEGER, observation_context_total INTEGER, observation_accepted_ledger INTEGER,
                observation_summary BLOB CHECK(observation_summary IS NULL OR length(observation_summary)<=16384),
                retry_ledger_position INTEGER,
                observation_terminal_sequence INTEGER,
                observation_terminal_header BLOB CHECK(observation_terminal_header IS NULL OR length(observation_terminal_header)<=16384),
                observation_source_id TEXT UNIQUE CHECK(observation_source_id IS NULL OR (length(observation_source_id)<=20 AND CAST(observation_source_id AS INTEGER)=started_sequence)),
                canonical_frontier INTEGER NOT NULL,
                input_root INTEGER REFERENCES native_input_nodes(id),
                input_root_digest BLOB,
                input_count INTEGER NOT NULL CHECK(input_count BETWEEN 0 AND 8192),
                started_sequence INTEGER,
                completed_sequence INTEGER,
                manifest_digest TEXT NOT NULL CHECK(typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64),
                previous_request_id TEXT,
                request_id TEXT PRIMARY KEY,
                retry_message_id TEXT,
                snapshot_json TEXT NOT NULL,
                CHECK((input_root IS NULL AND input_root_digest IS NULL AND input_count=0) OR
                    (input_root IS NOT NULL AND typeof(input_root_digest)='blob' AND length(input_root_digest)=32 AND input_count>0))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS request_trace_retry_idx ON request_snapshots(observation_step_anchor,observation_retry_number) WHERE observation_step_anchor IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS request_snapshots_started_idx ON request_snapshots(started_sequence) WHERE started_sequence IS NOT NULL;
            CREATE TABLE IF NOT EXISTS state_observation_sources (
                domain TEXT NOT NULL,source_id TEXT NOT NULL,ledger_position INTEGER,journal_sequence INTEGER,
                todo_header BLOB CHECK(todo_header IS NULL OR length(todo_header)<=16384),
                PRIMARY KEY(domain,source_id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS state_observation_ledger_idx ON state_observation_sources(domain,ledger_position,source_id);
            CREATE INDEX IF NOT EXISTS state_observation_journal_idx ON state_observation_sources(domain,journal_sequence,source_id);
            CREATE TABLE IF NOT EXISTS request_trace_catalogs (
                source_sequence INTEGER PRIMARY KEY REFERENCES events(sequence),
                tool_count INTEGER NOT NULL CHECK(tool_count>=0),
                provisional_message_id TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS request_trace_catalog_identity_idx ON request_trace_catalogs(provisional_message_id,source_sequence);
            CREATE TABLE IF NOT EXISTS request_trace_definitions (
                source_sequence INTEGER NOT NULL REFERENCES request_trace_catalogs(source_sequence) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                tool_identity_digest BLOB NOT NULL CHECK(length(tool_identity_digest)=32),
                tool_id TEXT NOT NULL,
                PRIMARY KEY(source_sequence,ordinal)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS request_trace_definition_digest_idx ON request_trace_definitions(source_sequence,tool_identity_digest,ordinal);
            CREATE INDEX IF NOT EXISTS request_trace_definition_identity_idx ON request_trace_definitions(source_sequence,tool_id,ordinal);
            CREATE TABLE IF NOT EXISTS trace_adoption_members (
                source_sequence INTEGER NOT NULL REFERENCES events(sequence) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                ledger_position INTEGER NOT NULL REFERENCES message_ledger(position),
                identity_digest BLOB NOT NULL CHECK(length(identity_digest)=32),
                PRIMARY KEY(source_sequence,ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS request_trace_contexts (
                source_sequence INTEGER NOT NULL REFERENCES events(sequence),
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                PRIMARY KEY(source_sequence,ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS contribution_emission_heads (
                producer TEXT NOT NULL,
                semantic_key TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                emitted_at TEXT NOT NULL,
                request_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                logical_step_origin INTEGER NOT NULL CHECK(logical_step_origin >= 0),
                event_sequence INTEGER NOT NULL CHECK(event_sequence >= 0),
                PRIMARY KEY(producer, semantic_key)
            );
            CREATE TABLE IF NOT EXISTS publication_streams (
                stream_id TEXT PRIMARY KEY,
                attempt_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                next_frame_sequence INTEGER NOT NULL CHECK(next_frame_sequence >= 0),
                terminal_sequence INTEGER,
                settlement TEXT
            );
            CREATE TABLE IF NOT EXISTS publication_frames (
                stream_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                frame_json TEXT NOT NULL,
                observation_indexed INTEGER NOT NULL CHECK(observation_indexed IN (0,1)),
                PRIMARY KEY(stream_id, sequence)
            );
            CREATE TABLE IF NOT EXISTS publication_proposals (
                stream_id TEXT NOT NULL,
                call_id TEXT NOT NULL,
                block_index INTEGER NOT NULL CHECK(block_index >= 0),
                tool_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('started','completed')),
                executed INTEGER NOT NULL CHECK(executed IN (0,1)),
                settlement TEXT,
                PRIMARY KEY(stream_id, call_id)
            );
            CREATE TABLE IF NOT EXISTS publication_audits (
                stream_id TEXT PRIMARY KEY,
                audit_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS agent_authorities (
                agent_id TEXT PRIMARY KEY,
                authority_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
                observation_trace_fact BLOB CHECK(observation_trace_fact IS NULL OR length(observation_trace_fact)<=49152),
                observation_trace_owner INTEGER,
                observation_trace_fact_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_trace_fact_required IN (0,1)),
                observation_kind TEXT CHECK(observation_kind IS NULL OR length(observation_kind)<=128),
                observation_unavailable INTEGER NOT NULL DEFAULT 0 CHECK(observation_unavailable IN (0,1)),
                observation_transcript INTEGER,
                observation_witness BLOB CHECK(observation_witness IS NULL OR length(observation_witness)<=49152),
                observation_source BLOB CHECK(observation_source IS NULL OR (typeof(observation_source)='blob' AND length(observation_source) BETWEEN 33 AND 16385)),
                observation_assistant_ledger INTEGER,
                observation_tool_ledger INTEGER,observation_tool_block INTEGER,
                observation_tool_phase INTEGER CHECK(observation_tool_phase IS NULL OR observation_tool_phase IN (1,2)),
                observation_turn_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_turn_required IN (0,1)),
                observation_turn_ordinal INTEGER, observation_turn_anchor INTEGER,
                observation_turn_header BLOB CHECK(observation_turn_header IS NULL OR length(observation_turn_header)<=16384),
                observation_turn_prompt BLOB CHECK(observation_turn_prompt IS NULL OR length(observation_turn_prompt)<=512),
                observation_turn_prompt_positive INTEGER NOT NULL DEFAULT 0 CHECK(observation_turn_prompt_positive IN (0,1)),
                observation_execution_owner INTEGER,
                observation_execution_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_execution_required IN(0,1)),
                observation_execution BLOB CHECK(observation_execution IS NULL OR length(observation_execution)<=49152),
                observation_members BLOB CHECK(observation_members IS NULL OR length(observation_members)<=49152),
                event_timestamp TEXT NOT NULL,
                observation_member_anchor INTEGER,
                observation_statistics BLOB CHECK(observation_statistics IS NULL OR length(observation_statistics)<=98340),
                observation_occupancy BLOB CHECK(observation_occupancy IS NULL OR length(observation_occupancy)<=16384),
                observation_readiness_missing INTEGER NOT NULL DEFAULT 1 CHECK(observation_readiness_missing IN (0,1)),
                observation_statistics_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_statistics_required IN (0,1)),
                observation_members_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_members_required IN (0,1)),
                observation_trace_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_trace_required IN (0,1)),
                observation_trace_total INTEGER CHECK(observation_trace_total IS NULL OR observation_trace_total>=0),
                observation_aggregate_holes INTEGER GENERATED ALWAYS AS (CASE WHEN observation_trace_fact_required=1 AND observation_trace_fact IS NULL THEN 1 ELSE 0 END+CASE WHEN observation_trace_required=1 AND observation_trace_total IS NULL THEN 2 ELSE 0 END+CASE WHEN observation_statistics_required=1 AND observation_statistics IS NULL THEN 4 ELSE 0 END) VIRTUAL,
                observation_owner_holes INTEGER GENERATED ALWAYS AS (CASE WHEN observation_execution_required=1 AND observation_execution IS NULL THEN 1 ELSE 0 END+CASE WHEN observation_turn_required=1 AND observation_turn_header IS NULL THEN 2 ELSE 0 END+CASE WHEN observation_members_required=1 AND observation_members IS NULL THEN 4 ELSE 0 END) VIRTUAL,
                observation_aggregate_heads INTEGER GENERATED ALWAYS AS ((observation_trace_total IS NOT NULL)+2*(observation_statistics IS NOT NULL)) VIRTUAL,
                observation_attempt_heads INTEGER GENERATED ALWAYS AS ((observation_execution IS NOT NULL)+2*(observation_members IS NOT NULL)) VIRTUAL,
                sequence INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL UNIQUE,
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                attempt_id TEXT,
                turn_id TEXT,
                event_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS events_aggregate_holes_idx ON events(observation_aggregate_holes) WHERE observation_aggregate_holes>0;
            CREATE INDEX IF NOT EXISTS events_owner_holes_idx ON events(observation_owner_holes) WHERE observation_owner_holes>0;
            CREATE INDEX IF NOT EXISTS events_trace_owner_idx ON events(observation_trace_owner,observation_kind) WHERE observation_trace_owner IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_readiness_missing_idx ON events(sequence) WHERE observation_readiness_missing=1;
            CREATE INDEX IF NOT EXISTS events_observation_assistant_ledger_idx ON events(observation_assistant_ledger) WHERE observation_assistant_ledger IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_aggregate_heads_idx ON events(observation_aggregate_heads) WHERE observation_aggregate_heads>0;
            CREATE INDEX IF NOT EXISTS transcript_order_audit_position_idx ON transcript_order(position) WHERE reference_kind='publication_audit';
            CREATE INDEX IF NOT EXISTS events_observation_tool_lifecycle_idx ON events(observation_tool_ledger,observation_tool_block,observation_tool_phase) WHERE observation_tool_phase IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_tool_occurrence_idx ON events(observation_tool_ledger,observation_tool_block) WHERE observation_tool_ledger IS NOT NULL AND json_extract(event_json,'$.event.type')='tool_execution_started';
            CREATE INDEX IF NOT EXISTS events_observation_tool_idx ON events(json_extract(event_json,'$.event.tool_call_id'),attempt_id,json_extract(event_json,'$.event.type')) WHERE json_extract(event_json,'$.event.tool_call_id') IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_attempt_heads_idx ON events(attempt_id,observation_attempt_heads) WHERE observation_attempt_heads>0;
            CREATE INDEX IF NOT EXISTS events_observation_member_holes_idx ON events(attempt_id) WHERE observation_members_required=1 AND observation_members IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_member_anchor_idx ON events(observation_member_anchor) WHERE observation_members IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_unavailable_idx ON events(sequence) WHERE observation_unavailable=1;
            CREATE UNIQUE INDEX IF NOT EXISTS events_observation_transcript_idx ON events(observation_transcript) WHERE observation_transcript IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS events_observation_turn_ordinal_idx ON events(observation_turn_ordinal) WHERE observation_turn_ordinal IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_turn_anchor_idx ON events(observation_turn_anchor) WHERE observation_turn_header IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_turn_prompt_idx ON events(attempt_id) WHERE observation_turn_prompt_positive=1;
            CREATE INDEX IF NOT EXISTS events_observation_turn_prompt_holes_idx ON events(sequence) WHERE observation_kind='inbound_turn_adopted' AND observation_turn_prompt IS NULL;
            CREATE INDEX IF NOT EXISTS events_execution_owner_idx ON events(observation_execution_owner) WHERE observation_execution IS NOT NULL;
            CREATE TABLE IF NOT EXISTS lifecycle_state (
                lifecycle_key TEXT PRIMARY KEY,
                terminal_event_id TEXT
            );
            CREATE INDEX IF NOT EXISTS message_ledger_append_position_idx ON message_ledger(position,first_append_revision) WHERE first_append_revision IS NOT NULL;
            CREATE INDEX IF NOT EXISTS message_ledger_todo_idx ON message_ledger(position) WHERE native_todo=1;
            CREATE INDEX IF NOT EXISTS message_ledger_opening_idx ON message_ledger(position) WHERE ordinary_user=1;
            CREATE INDEX IF NOT EXISTS message_ledger_human_activity_idx ON message_ledger(human_activity_seconds,human_activity_nanos,position) WHERE human_activity_at IS NOT NULL;
            CREATE INDEX IF NOT EXISTS message_ledger_boundary_holes_idx ON message_ledger(first_append_revision) WHERE ordinary_user=1 AND boundary_indexed=0 AND first_append_revision IS NOT NULL;

            CREATE INDEX IF NOT EXISTS request_snapshots_canonical_frontier_idx ON request_snapshots(canonical_frontier);
            CREATE INDEX IF NOT EXISTS publication_frames_unindexed_idx ON publication_frames(stream_id,sequence) WHERE observation_indexed=0;
            CREATE INDEX IF NOT EXISTS publication_streams_settlement_idx ON publication_streams(settlement);
            CREATE INDEX IF NOT EXISTS publication_streams_message_settlement_idx ON publication_streams(message_id,settlement);",
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("create schema: {error}")))?;
    content::create_binary_schema(connection)?;
    manifest::create_schema(connection)?;
    observation_request::composed::create_schema(connection)?;
    observation_request::state_preview::create_schema(connection)?;
    auxiliary::create_schema(connection)?;
    super::schedule::create_schema(connection)?;
    super::scheduled_workflow::create_schema(connection)?;
    request_owner_shape::create_schema(connection)?;
    projection::create_auxiliary_lineage_schema(connection)?;
    connection
        .execute_batch(&projection::completion_index_sql())
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    for (name, definition) in PRESENTATION_INDEXES {
        connection
            .execute_batch(&format!("CREATE INDEX {name} ON events{definition};"))
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("create presentation index: {error}"),
                )
            })?;
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO rustx_store(id,schema_version,conversation_id,next_inbound_sequence,next_event_sequence,next_transcript_position,logical_step_sequence,pending_unresolved_output_stream_id) VALUES(1,?1,'',0,0,0,0,NULL)",
            params![SQLITE_SCHEMA_VERSION],
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,format!("create schema root: {error}")))?;
    let version: i64 = connection
        .query_row(
            "SELECT schema_version FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("read schema version: {error}"))
        })?;
    if version != SQLITE_SCHEMA_VERSION {
        return Err(ConversationStoreError::SchemaVersionMismatch {
            stored: version,
            expected: SQLITE_SCHEMA_VERSION,
        });
    }
    verify_schema_shape(connection)?;
    transaction.commit().map_err(|error| {
        crate::durable::sqlite::sqlite_error(&error, format!("create schema commit: {error}"))
    })
}

pub(super) fn bind_identity(
    connection: &mut Connection,
    conversation_id: &ConversationId,
) -> Result<(), ConversationStoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("bind conversation transaction: {error}"),
            )
        })?;
    let stored: String = transaction
        .query_row_cached(
            "SELECT conversation_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("read conversation identity: {error}"),
            )
        })?;
    if stored.is_empty() {
        transaction
            .execute(
                "UPDATE rustx_store SET conversation_id=?1 WHERE id=1",
                [conversation_id.as_str()],
            )
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("bind conversation identity: {error}"),
                )
            })?;
    } else if stored != conversation_id.as_str() {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: ConversationId::parse(stored).map_err(storage)?,
            requested: conversation_id.clone(),
        });
    }
    transaction.commit().map_err(|error| {
        crate::durable::sqlite::sqlite_error(&error, format!("bind conversation commit: {error}"))
    })?;
    Ok(())
}

impl SqliteConversationStore {
    pub(crate) fn with_lifecycle(
        mut self,
        guard: Arc<crate::runtime::local_storage::ConversationAccess>,
    ) -> Self {
        self.lifecycle = Some(guard);
        self
    }
    pub(crate) fn close_storage_writer(&mut self) -> Result<(), ConversationStoreError> {
        let owner = Arc::get_mut(&mut self.conn)
            .ok_or_else(|| storage("storage writer has outstanding connection owners"))?;
        let slot = owner
            .available
            .get_mut()
            .map_err(|_| storage("storage connection mutex poisoned"))?;
        let connection = slot
            .connection
            .take()
            .ok_or_else(|| storage("storage writer has an outstanding lease"))?;
        if let Err((connection, error)) = connection.close() {
            slot.returned(Some(connection));
            return Err(storage(format!("close storage writer: {error}")));
        }
        Ok(())
    }
    /// Opens a durable store at `path`, creating its development schema when
    /// the file is new.
    ///
    /// # Errors
    ///
    /// Returns an error if `SQLite` cannot be opened/configured or if the file
    /// contains an incompatible development schema.
    pub fn open(
        conversation_id: ConversationId,
        path: &Path,
    ) -> Result<Self, ConversationStoreError> {
        count_conversation_store_open();
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::State,
        );
        let mut connection = Connection::open(path).map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("open {}: {error}", path.display()),
            )
        })?;
        // Refuse obsolete development state before changing journal settings.
        reject_legacy_schema(&connection)?;
        if has_table(&connection, "rustx_store")? {
            register_native_functions(&connection)?;
            validate_existing_schema(&connection)?;
            validate_existing_identity(&connection, &conversation_id)?;
        }
        configure_connection(&mut connection, false)?;
        Self::from_connection(conversation_id, connection)
    }
    /// Opens an existing bound store for management without initializing state.
    ///
    /// # Errors
    /// Missing, unbound, incompatible and wrong-identity stores are rejected.
    pub fn open_existing(
        conversation_id: ConversationId,
        path: &Path,
    ) -> Result<Self, ConversationStoreError> {
        Self::open_bound_existing(
            conversation_id,
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
    }
    /// Startup recovery is an explicit writer operation, never a management read.
    /// It may recover WAL or convert a compatible rollback journal, but cannot
    /// create, initialize, or migrate a store.
    pub(crate) fn recover_existing(
        conversation_id: ConversationId,
        path: &Path,
        guard: &Arc<crate::runtime::local_storage::ProductController>,
    ) -> Result<Self, ConversationStoreError> {
        let path = guard
            .confined(path)
            .map_err(|error| storage(error.to_string()))?;
        let access = Arc::new(
            crate::runtime::local_storage::ConversationAccess::existing_for_controller(
                guard.clone(),
                path.parent().ok_or_else(|| storage("missing allocation"))?,
            )
            .map_err(|e| storage(e.to_string()))?,
        );
        let store = Self::open_bound_existing(
            conversation_id,
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        configure_connection(&mut *store.lock()?, false)?;
        Ok(store.with_lifecycle(access))
    }
    pub(crate) fn open_schedule_reader(
        conversation_id: ConversationId,
        path: &Path,
        scope: &Arc<super::schedule_read::ScheduleReadScope>,
    ) -> Result<Self, ConversationStoreError> {
        Self::open_existing_scoped(
            conversation_id,
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            Some(scope),
        )
    }
    pub(super) fn open_bound_existing(
        conversation_id: ConversationId,
        path: &Path,
        flags: rusqlite::OpenFlags,
    ) -> Result<Self, ConversationStoreError> {
        Self::open_existing_scoped(conversation_id, path, flags, None)
    }
    fn open_existing_scoped(
        conversation_id: ConversationId,
        path: &Path,
        flags: rusqlite::OpenFlags,
        scope: Option<&Arc<super::schedule_read::ScheduleReadScope>>,
    ) -> Result<Self, ConversationStoreError> {
        use std::io::Read as _;
        if let Some(scope) = scope {
            scope.check()?;
        }
        count_conversation_store_open();
        // Reject unsupported file formats before SQLite access. WAL readers may
        // create/rebuild physical sidecars; that is not application-state repair.
        let mut header = [0; 20];
        let file = if scope.is_some() {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(path)
                .and_then(|file| {
                    if file.metadata()?.is_file() {
                        Ok(file)
                    } else {
                        Err(std::io::Error::other(
                            "schedule reader requires a regular database file",
                        ))
                    }
                })
        } else {
            std::fs::File::open(path)
        };
        file.and_then(|mut file| file.read_exact(&mut header))
            .map_err(|error| storage(format!("read existing database header: {error}")))?;
        if &header[..16] != b"SQLite format 3\0" || !matches!(&header[18..20], [1, 1] | [2, 2]) {
            return Err(storage(
                "existing-only access requires a supported SQLite file format",
            ));
        }
        let connection = Connection::open_with_flags(path, flags).map_err(|error| {
            crate::durable::sqlite::sqlite_error(
                &error,
                format!("open existing {}: {error}", path.display()),
            )
        })?;
        if let Some(scope) = scope {
            scope.install(&connection)?;
        }
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(&error, format!("read journal mode: {error}"))
            })?;
        if mode != "wal"
            && !(mode == "delete" && flags.contains(rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE))
        {
            return Err(storage("existing-only management requires WAL storage"));
        }
        if scope.is_none() {
            writer_busy_policy(&connection)
                .map_err(|error| storage(format!("configure SQLite busy policy: {error}")))?;
        }
        register_native_functions(&connection)?;
        register_integrity_function(&connection)?;
        validate_existing_schema(&connection)?;
        validate_existing_identity(&connection, &conversation_id)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")
            .map_err(|error| {
                crate::durable::sqlite::sqlite_error(
                    &error,
                    format!("configure existing SQLite: {error}"),
                )
            })?;
        if let Some(scope) = scope {
            scope.check()?;
        }
        Self::from_validated_connection(conversation_id, connection)
    }
    /// Creates an in-memory store for tests and headless ephemeral runs.
    ///
    /// # Errors
    ///
    /// Returns an error if the in-memory `SQLite` connection cannot be
    /// configured or initialized.
    pub fn in_memory(conversation_id: ConversationId) -> Result<Self, ConversationStoreError> {
        count_conversation_store_open();
        let mut connection = Connection::open_in_memory().map_err(|error| {
            crate::durable::sqlite::sqlite_error(&error, format!("in-memory: {error}"))
        })?;
        configure_connection(&mut connection, true)?;
        Self::from_connection(conversation_id, connection)
    }
    pub(super) fn from_connection(
        conversation_id: ConversationId,
        mut connection: Connection,
    ) -> Result<Self, ConversationStoreError> {
        register_native_functions(&connection)?;
        register_integrity_function(&connection)?;
        reject_legacy_schema(&connection)?;
        if has_table(&connection, "rustx_store")? {
            validate_existing_schema(&connection)?;
        } else {
            create_schema(&mut connection)?;
        }
        bind_identity(&mut connection, &conversation_id)?;
        Self::from_validated_connection(conversation_id, connection)
    }
    pub(super) fn from_validated_connection(
        conversation_id: ConversationId,
        connection: Connection,
    ) -> Result<Self, ConversationStoreError> {
        observation_readiness::install(&connection)?;
        observation_request::composed::install_workspace(&connection)?;
        // Native capture, receipts and publication alternate on one connection.
        // A repeated-turn trace observed 171 distinct prepared shapes even with
        // the old 64-entry cache. Give that fixed workload 256 bounded slots;
        // this caches plans only, never read results or evidence.
        connection.set_prepared_statement_cache_capacity(256);
        // Native readers select bounded index ranges explicitly. Do not
        // recompile their plans whenever a historical cut or identity changes:
        // bundled SQLite's STAT4 parameter-sensitive planning otherwise does
        // so even for reused statements. Schema changes still invalidate plans.
        connection
            .set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_QPSG,
                true,
            )
            .map_err(|error| storage(format!("configure stable SQLite plans: {error}")))?;
        let append_validation = append_validation::Owner::new(&connection)?;
        let statistics = observation_statistics::FoldCache::new(&connection)?;
        let transaction_writes = Arc::new(AtomicBool::new(false));
        let rollback_writes = Arc::clone(&transaction_writes);
        let statistics_rollback = Arc::clone(&statistics);
        let rollback = Arc::clone(&append_validation);
        connection
            .rollback_hook(Some(move || {
                if rollback_writes.swap(false, Ordering::AcqRel) {
                    rollback.invalidate();
                    statistics_rollback.invalidate();
                }
            }))
            .map_err(|error| storage(format!("install canonical rollback observer: {error}")))?;
        let response = observation_response::Owner::new(&connection)?;
        let manifest = manifest::Owner::install(
            &connection,
            Arc::clone(&append_validation),
            Arc::clone(&response),
            Arc::clone(&statistics),
            Arc::clone(&transaction_writes),
        )?;
        let event_guard = event_insert::Guard::new(&connection, statistics);
        Ok(Self::from_connection_owner(
            conversation_id,
            Arc::new(ConnectionOwner {
                available: Mutex::new(ConnectionSlot::new(connection)),
                manifest,
                append_validation,
                response,
                event_guard,
                transaction_writes,
                wake: Condvar::new(),
                poisoned: AtomicBool::new(false),
                lease_generation: AtomicU64::new(0),
                active_interrupt: Mutex::new(None),
            }),
        ))
    }
}
