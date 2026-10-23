//! Exact schema-shape verification of an opened database.

use super::PRESENTATION_INDEXES;
use crate::durable::ConversationStoreError;
use crate::durable::sqlite::auxiliary;
use crate::durable::sqlite::request_owner_shape;
use crate::durable::sqlite::{
    content, manifest, observation, observation_request, observation_trace, projection, provenance,
    storage,
};
use rusqlite::OptionalExtension;
use rusqlite::{Connection, params};
use std::collections::BTreeSet;

#[allow(clippy::too_many_lines)] // One required-shape table, one place.
pub(in crate::durable::sqlite) fn verify_schema_shape(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    content::verify_binary_schema(connection)?;
    provenance::verify_turn_header_shape(connection)?;
    projection::verify_completion_index(connection)?;
    // Bootstrap metadata stays compact; provenance payloads have their own rows.
    // Refuse obsolete inline arrays and require the native keyed representation.
    let bootstrap_columns = connection
        .prepare("PRAGMA table_info(bootstrap_identity)")
        .map_err(|error| storage(error.to_string()))?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    if bootstrap_columns
        != [
            "occupancy_observation",
            "id",
            "message_count",
            "history_digest",
            "occupancy_json",
            "occupancy_digest",
            "turn_count",
            "response_count",
            "provenance_digest",
            "inherited_transcript_through",
            "projection_count",
            "projection_digest",
            "auxiliary_audit_count",
            "auxiliary_audit_digest",
        ]
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid bootstrap lookup field order".into(),
        ));
    }

    for (name, sql) in [
        (
            "projection_ledger_idx",
            "CREATE INDEX projection_ledger_idx ON projection_activations(ledger_frontier,activation)",
        ),
        (
            "projection_checkpoint_idx",
            "CREATE UNIQUE INDEX projection_checkpoint_idx ON projection_activations(checkpoint_id)",
        ),
    ] {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| storage(e.to_string()))?;
        if actual.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid projection index {name}"
            )));
        }
    }
    for (name, expression) in PRESENTATION_INDEXES {
        let sql: Option<String> = connection.query_row("SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1 AND tbl_name='events'", [name], |row| row.get(0)).optional().map_err(|error| storage(error.to_string()))?;
        if sql.as_deref() != Some(format!("CREATE INDEX {name} ON events({expression})").as_str()) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid presentation index {name}"
            )));
        }
    }
    for sql in [
        "CREATE UNIQUE INDEX inherited_turn_identity_idx ON inherited_turn_provenance(json_extract(payload,'$.id.conversation_id'),json_extract(payload,'$.id.attempt_id'))",
        "CREATE UNIQUE INDEX inherited_response_closing_idx ON inherited_response_provenance(json_extract(payload,'$.closing_message_id'))",
        "CREATE UNIQUE INDEX message_ledger_identity_position_idx ON message_ledger(position,message_id)",
        "CREATE UNIQUE INDEX message_ledger_native_transcript_idx ON message_ledger(native_transcript) WHERE native_transcript IS NOT NULL",
    ] {
        let name = sql.split_whitespace().nth(3).expect("literal index name");
        let retained: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        if retained.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid inherited index {name}"
            )));
        }
    }
    for (name, sql) in [
        (
            "inherited_response_owner_idx",
            "CREATE INDEX inherited_response_owner_idx ON inherited_response_provenance(json_extract(payload,'$.origin.conversation_id'),json_extract(payload,'$.origin.attempt_id'),ordinal)",
        ),
        (
            "observation_models_owner_idx",
            "CREATE INDEX observation_models_owner_idx ON observation_models(owner_anchor,ordinal)",
        ),
        (
            "events_observation_turn_ordinal_idx",
            "CREATE UNIQUE INDEX events_observation_turn_ordinal_idx ON events(observation_turn_ordinal) WHERE observation_turn_ordinal IS NOT NULL",
        ),
        (
            "events_observation_turn_anchor_idx",
            "CREATE INDEX events_observation_turn_anchor_idx ON events(observation_turn_anchor,sequence) WHERE observation_turn_header IS NOT NULL",
        ),
        (
            "events_observation_turn_holes_idx",
            "CREATE INDEX events_observation_turn_holes_idx ON events(sequence) WHERE observation_turn_required=1 AND observation_turn_header IS NULL",
        ),
        (
            "events_observation_turn_prompt_idx",
            "CREATE INDEX events_observation_turn_prompt_idx ON events(attempt_id,sequence) WHERE observation_turn_prompt_positive=1",
        ),
        (
            "events_observation_turn_prompt_holes_idx",
            "CREATE INDEX events_observation_turn_prompt_holes_idx ON events(sequence) WHERE observation_kind='inbound_turn_adopted' AND observation_turn_prompt IS NULL",
        ),
        (
            "observation_block_ledger_kind_idx",
            "CREATE INDEX observation_block_ledger_kind_idx ON observation_block_headers(ledger_position,json_extract(header_json,'$.kind'),block_index)",
        ),
        (
            "events_observation_transcript_idx",
            "CREATE UNIQUE INDEX events_observation_transcript_idx ON events(observation_transcript) WHERE observation_transcript IS NOT NULL",
        ),
    ] {
        let retained: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        if retained.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "native owner index unavailable".into(),
            ));
        }
    }
    let reverse_index: Option<String> = connection
        .query_row("SELECT sql FROM sqlite_schema WHERE type='index' AND name='inherited_membership_turn_idx'", [], |row| row.get(0))
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    if reverse_index.as_deref()
        != Some(
            "CREATE INDEX inherited_membership_turn_idx ON inherited_message_membership(turn_ordinal,assistant_message_id)",
        )
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "missing or invalid inherited reverse membership index".into(),
        ));
    }
    for table in ["inherited_turn_provenance", "inherited_response_provenance"] {
        let primary: (String, i64) = connection
            .query_row(
                &format!("SELECT type,pk FROM pragma_table_info('{table}') WHERE name='ordinal'"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| storage(error.to_string()))?;
        if primary != ("INTEGER".into(), 1) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid inherited ordinal primary key in {table}"
            )));
        }
    }
    let foreign = connection
        .prepare("PRAGMA foreign_key_list(inherited_message_membership)")
        .map_err(|error| storage(error.to_string()))?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    let expected = BTreeSet::from([
        (
            "message_ledger".into(),
            "assistant_ledger_position".into(),
            "position".into(),
        ),
        (
            "message_ledger".into(),
            "assistant_message_id".into(),
            "message_id".into(),
        ),
        (
            "inherited_turn_provenance".into(),
            "turn_ordinal".into(),
            "ordinal".into(),
        ),
        (
            "inherited_response_provenance".into(),
            "response_ordinal".into(),
            "ordinal".into(),
        ),
    ]);
    if foreign != expected {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid inherited membership foreign keys".into(),
        ));
    }
    let paired: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list('inherited_message_membership') a JOIN pragma_foreign_key_list('inherited_message_membership') b ON a.id=b.id WHERE a.\"table\"='message_ledger' AND a.\"from\"='assistant_ledger_position' AND a.\"to\"='position' AND b.\"from\"='assistant_message_id' AND b.\"to\"='message_id' AND a.seq=0 AND b.seq=1)",
        [], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    if !paired {
        return Err(ConversationStoreError::IncompatibleSchema(
            "inherited canonical identity is not paired".into(),
        ));
    }
    verify_unique_columns(connection, "message_ledger", &["position", "message_id"])?;
    verify_unique_column(
        connection,
        "inherited_message_membership",
        "assistant_ledger_position",
    )?;
    verify_witness_field_order(connection)?;
    let required = [
        (
            "projection_checkpoints",
            &[
                "checkpoint_id",
                "candidate_fingerprint",
                "candidate_json",
                "occupancy_json",
            ] as &[&str],
        ),
        (
            "projection_activations",
            &[
                "activation",
                "checkpoint_id",
                "ledger_frontier",
                "visible_journal",
                "installation_id",
                "header_digest",
                "activation_digest",
                "observation_header_witness",
                "observation_header",
                "activation_json",
            ],
        ),
        ("projection_head", &["id", "activation"]),
        (
            "compaction_operations",
            &[
                "observation_receipt_witness",
                "observation_receipt",
                "operation_id",
                "candidate_fingerprint",
                "receipt_digest",
                "receipt_json",
            ],
        ),
        (
            "rustx_store",
            &[
                "schema_version",
                "conversation_id",
                "next_inbound_sequence",
                "next_event_sequence",
                "next_transcript_position",
                "retained_transcript_count",
                "pending_observation_count",
                "reading_mutation_revision",
                "logical_step_sequence",
                "pending_unresolved_output_stream_id",
                "bootstrap_initialized",
            ] as &[&str],
        ),
        (
            "pending_inbound",
            &[
                "sequence",
                "observation_header",
                "observation_transcript",
                "revision",
                "message_id",
                "message_json",
                "correlation",
            ],
        ),
        (
            "inbound_correlation",
            &["correlation", "sequence", "message_id"],
        ),
        (
            "observation_headers",
            &[
                "reference_kind",
                "reference_id",
                "ledger_position",
                "header_json",
            ],
        ),
        (
            "content_segments",
            &[
                "source_kind",
                "source_id",
                "block_index",
                "representation",
                "segment",
                "bytes",
            ],
        ),
        (
            "content_frontiers",
            &[
                "source_kind",
                "source_id",
                "block_index",
                "representation",
                "revision",
                "readable_end",
            ],
        ),
        (
            "message_ledger",
            &[
                "observation_trace_summary",
                "observation_identity_digest",
                "observation_delivery_success",
                "observation_delivery_total",
                "position",
                "native_transcript",
                "human_activity_at",
                "human_activity_seconds",
                "human_activity_nanos",
                "opening_preview",
                "ordinary_user",
                "ordinary_human",
                "title_source_text",
                "message_id",
                "message_json",
            ],
        ),
        (
            "transcript_order",
            &["position", "reference_kind", "reference_id"],
        ),
        (
            "canonical_tool_calls",
            &[
                "observation_trace_proposal",
                "observation_tool_identity_digest",
                "observation_definition_ready",
                "observation_definition_start",
                "observation_definition_commit",
                "observation_definition_ordinal",
                "assistant_message_id",
                "block_index",
                "call_id",
                "tool_id",
                "result_message_id",
            ],
        ),
        (
            "bootstrap_identity",
            &[
                "occupancy_observation",
                "message_count",
                "history_digest",
                "turn_count",
                "response_count",
                "provenance_digest",
                "occupancy_json",
                "occupancy_digest",
                "inherited_transcript_through",
                "projection_count",
                "projection_digest",
                "auxiliary_audit_count",
                "auxiliary_audit_digest",
            ],
        ),
        (
            "inherited_turn_provenance",
            &[
                "observation_turn_display",
                "observation_turn_display_witness",
                "observation_json_witness",
                "ordinal",
                "payload",
                "payload_digest",
            ],
        ),
        (
            "inherited_response_provenance",
            &["ordinal", "payload", "payload_digest"],
        ),
        (
            "inherited_message_membership",
            &[
                "observation_identity_digest",
                "observation_membership_witness",
                "assistant_message_id",
                "assistant_ledger_position",
                "turn_ordinal",
                "response_ordinal",
                "membership_digest",
            ],
        ),
        (
            "surface_ops",
            &["revision", "compaction_generation", "op_json"],
        ),
        ("surface_head", &["revision", "compaction_generation"]),
        (
            "request_snapshots",
            &[
                "observation_step_anchor",
                "observation_retry_number",
                "observation_context_total",
                "observation_accepted_ledger",
                "observation_summary",
                "observation_source_id",
                "request_id",
                "canonical_frontier",
                "manifest_digest",
                "input_root",
                "input_root_digest",
                "input_count",
                "started_sequence",
                "completed_sequence",
                "snapshot_json",
            ],
        ),
        (
            "contribution_emission_heads",
            &[
                "producer",
                "semantic_key",
                "fingerprint",
                "emitted_at",
                "request_id",
                "message_id",
                "logical_step_origin",
                "event_sequence",
            ],
        ),
        (
            "publication_streams",
            &[
                "stream_id",
                "attempt_id",
                "turn_id",
                "request_id",
                "message_id",
                "next_frame_sequence",
                "terminal_sequence",
                "settlement",
            ],
        ),
        (
            "publication_frames",
            &["stream_id", "sequence", "frame_json", "observation_indexed"],
        ),
        (
            "publication_proposals",
            &[
                "call_id",
                "stream_id",
                "block_index",
                "tool_id",
                "tool_name",
                "state",
                "executed",
                "settlement",
            ],
        ),
        ("publication_audits", &["stream_id", "audit_json"]),
        (
            "events",
            &[
                "observation_turn_required",
                "observation_turn_ordinal",
                "observation_turn_anchor",
                "observation_turn_header",
                "observation_turn_prompt",
                "observation_turn_prompt_positive",
                "observation_trace_fact",
                "observation_trace_owner",
                "observation_trace_fact_required",
                "observation_execution_owner",
                "observation_execution_required",
                "observation_kind",
                "observation_occupancy",
                "observation_statistics_required",
                "observation_members_required",
                "observation_member_anchor",
                "sequence",
                "event_id",
                "schema_version",
                "conversation_id",
                "attempt_id",
                "turn_id",
                "event_json",
                "observation_unavailable",
                "observation_json",
            ],
        ),
        (
            "request_trace_catalogs",
            &["source_sequence", "provisional_message_id", "tool_count"],
        ),
        (
            "request_trace_definitions",
            &[
                "source_sequence",
                "ordinal",
                "tool_identity_digest",
                "tool_id",
            ],
        ),
        (
            "trace_adoption_members",
            &[
                "source_sequence",
                "ordinal",
                "ledger_position",
                "identity_digest",
            ],
        ),
        (
            "request_trace_contexts",
            &["source_sequence", "ordinal", "header_json"],
        ),
        (
            "request_observation_fields",
            &[
                "source_id",
                "path_json",
                "parent_path",
                "ordinal",
                "header_kind",
                "shape_witness",
                "header_json",
            ],
        ),
        (
            "state_observation_fields",
            &[
                "source_id",
                "path_json",
                "parent_path",
                "ordinal",
                "header_json",
            ],
        ),
        (
            "observation_models",
            &[
                "request_alias",
                "request_encoded_bytes",
                "first_sequence",
                "owner_anchor",
                "encoded_bytes",
            ],
        ),
        ("agent_authorities", &["agent_id", "authority_json"]),
        ("lifecycle_state", &["lifecycle_key", "terminal_event_id"]),
    ];
    for (table, required_columns) in required {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| storage(format!("inspect schema table {table}: {error}")))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| storage(format!("inspect schema columns {table}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("read schema columns {table}: {error}")))?;
        if table == "surface_head" && columns.iter().any(|column| column == "active_json") {
            return Err(ConversationStoreError::IncompatibleSchema(
                "surface_head retains retired active_json layout".into(),
            ));
        }
        for column in required_columns {
            if !columns.iter().any(|candidate| candidate == column) {
                return Err(ConversationStoreError::IncompatibleSchema(format!(
                    "table {table} is missing required column {column}"
                )));
            }
        }
    }
    for (table, column, target, target_column) in [
        (
            "projection_activations",
            "checkpoint_id",
            "projection_checkpoints",
            "checkpoint_id",
        ),
        (
            "projection_head",
            "activation",
            "projection_activations",
            "activation",
        ),
    ] {
        let mut statement = connection
            .prepare(&format!("PRAGMA foreign_key_list({table})"))
            .map_err(|e| storage(e.to_string()))?;
        let keys = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| storage(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| storage(e.to_string()))?;
        if !keys
            .iter()
            .any(|(t, c, tc)| t == target && c == column && tc == target_column)
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing projection dependency constraint {table}.{column}"
            )));
        }
    }
    for (table, column) in [
        ("pending_inbound", "message_id"),
        ("inbound_correlation", "message_id"),
        ("message_ledger", "message_id"),
        ("canonical_tool_calls", "result_message_id"),
        ("inherited_message_membership", "assistant_message_id"),
        ("agent_authorities", "agent_id"),
        ("events", "event_id"),
        ("request_snapshots", "request_id"),
        ("projection_checkpoints", "checkpoint_id"),
        ("compaction_operations", "operation_id"),
        ("publication_streams", "stream_id"),
        ("publication_audits", "stream_id"),
    ] {
        verify_unique_column(connection, table, column)?;
    }
    verify_unique_columns(
        connection,
        "canonical_tool_calls",
        &["assistant_message_id", "block_index"],
    )?;
    verify_unique_columns(
        connection,
        "canonical_tool_calls",
        &["assistant_message_id", "call_id"],
    )?;
    verify_unique_columns(
        connection,
        "publication_proposals",
        &["stream_id", "call_id"],
    )?;
    verify_unique_columns(
        connection,
        "transcript_order",
        &["reference_kind", "reference_id"],
    )?;
    verify_unique_columns(
        connection,
        "contribution_emission_heads",
        &["producer", "semantic_key"],
    )?;
    manifest::verify_schema(connection)?;
    content::verify_schema(connection)?;
    observation::verify_schema(connection)?;
    observation_trace::verify_schema(connection)?;
    observation_request::verify_schema(connection)?;
    auxiliary::verify_schema(connection)?;
    projection::verify_auxiliary_lineage_schema(connection)?;
    request_owner_shape::verify(connection)?;
    Ok(())
}

pub(in crate::durable::sqlite) fn verify_unique_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<(), ConversationStoreError> {
    verify_unique_columns(connection, table, &[column])
}

pub(in crate::durable::sqlite) fn verify_unique_columns(
    connection: &Connection,
    table: &str,
    expected: &[&str],
) -> Result<(), ConversationStoreError> {
    let mut indexes = connection
        .prepare(&format!("PRAGMA index_list({table})"))
        .map_err(|error| storage(format!("inspect schema indexes {table}: {error}")))?;
    let index_rows = indexes
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })
        .map_err(|error| storage(format!("inspect schema indexes {table}: {error}")))?;
    for index_row in index_rows {
        let (index, unique) =
            index_row.map_err(|error| storage(format!("read schema index {table}: {error}")))?;
        if unique == 0 {
            continue;
        }
        let quoted = index.replace('"', "\"\"");
        let mut info = connection
            .prepare(&format!("PRAGMA index_info(\"{quoted}\")"))
            .map_err(|error| storage(format!("inspect schema index {index}: {error}")))?;
        let columns = info
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(|error| storage(format!("inspect schema index {index}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("read schema index {index}: {error}")))?;
        if columns
            == expected
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<Vec<_>>()
        {
            return Ok(());
        }
    }
    Err(ConversationStoreError::IncompatibleSchema(format!(
        "table {table} is missing a unique constraint on ({})",
        expected.join(", ")
    )))
}

pub(in crate::durable::sqlite) fn verify_witness_field_order(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    pub(in crate::durable::sqlite) const REQUEST_WITNESS: &[&str] = &[
        "input_root",
        "input_root_digest",
        "input_count",
        "canonical_frontier",
        "manifest_digest",
        "started_sequence",
        "completed_sequence",
    ];
    for (table, original, fields) in [
        (
            "request_snapshots",
            "manifest_digest",
            &[
                "input_root",
                "input_root_digest",
                "input_count",
                "started_sequence",
                "completed_sequence",
            ] as &[&str],
        ),
        ("request_snapshots", "previous_request_id", REQUEST_WITNESS),
        ("request_snapshots", "request_id", REQUEST_WITNESS),
        ("request_snapshots", "snapshot_json", REQUEST_WITNESS),
        (
            "request_trace_catalogs",
            "provisional_message_id",
            &["tool_count"] as &[&str],
        ),
        (
            "canonical_tool_calls",
            "assistant_message_id",
            &[
                "observation_tool_identity_digest",
                "observation_definition_ready",
                "observation_definition_start",
                "observation_definition_commit",
                "observation_definition_ordinal",
            ] as &[&str],
        ),
        (
            "message_ledger",
            "message_id",
            &[
                "observation_identity_digest",
                "observation_delivery_success",
                "observation_delivery_total",
            ] as &[&str],
        ),
        (
            "inherited_message_membership",
            "assistant_message_id",
            &[
                "observation_identity_digest",
                "observation_membership_witness",
                "assistant_ledger_position",
                "turn_ordinal",
                "response_ordinal",
                "membership_digest",
            ] as &[&str],
        ),
    ] {
        let original_position: usize = connection
            .query_row(
                "SELECT cid FROM pragma_table_info(?1) WHERE name=?2",
                params![table, original],
                |row| row.get(0),
            )
            .map_err(|_| {
                ConversationStoreError::IncompatibleSchema(
                    "missing original witness identity column".into(),
                )
            })?;
        for field in fields {
            let position: usize = connection
                .query_row(
                    "SELECT cid FROM pragma_table_info(?1) WHERE name=?2",
                    params![table, field],
                    |row| row.get(0),
                )
                .map_err(|_| {
                    ConversationStoreError::IncompatibleSchema(
                        "missing finite read witness column".into(),
                    )
                })?;
            if position >= original_position {
                return Err(ConversationStoreError::IncompatibleSchema(
                    "finite read witness follows original identity body".into(),
                ));
            }
        }
    }
    Ok(())
}
