use crate::durable::sqlite::{Connection, ConversationStoreError, sqlite_error};

macro_rules! constraint {
    () => {
        concat!(
            "CONSTRAINT events_identity CHECK(sequence>0 AND (",
            "event_id = conversation_id || '-event-' || sequence OR ",
            "substr(CAST(event_id AS BLOB),1,length(CAST(conversation_id AS BLOB))+7) != CAST(conversation_id || '-event-' AS BLOB) OR ",
            "NOT (CAST(substr(CAST(event_id AS BLOB),length(CAST(conversation_id AS BLOB))+8) AS TEXT) = '0' OR ",
            "CAST(substr(CAST(event_id AS BLOB),length(CAST(conversation_id AS BLOB))+8,1) AS TEXT) BETWEEN '1' AND '9') OR ",
            "ltrim(CAST(substr(CAST(event_id AS BLOB),length(CAST(conversation_id AS BLOB))+8) AS TEXT),'0123456789') != ''))"
        )
    };
}
pub(in crate::durable::sqlite) use constraint;

macro_rules! table {
    () => {
        concat!(
            "CREATE TABLE IF NOT EXISTS events (
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
                event_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL REFERENCES rustx_store(conversation_id),
                attempt_id TEXT,
                turn_id TEXT,
                attempt_key GENERATED ALWAYS AS (",
            crate::durable::sqlite::event_owner_key::attempt!("attempt_id", "conversation_id"),
            ") VIRTUAL,
                turn_key GENERATED ALWAYS AS (",
            crate::durable::sqlite::event_owner_key::turn!("turn_id"),
            ") VIRTUAL,
                event_kind GENERATED ALWAYS AS (json_extract(event_json,'$.event.type')) VIRTUAL,
                event_owner_kind GENERATED ALWAYS AS (json_extract(event_json,'$.event.owner.type')) VIRTUAL,
                event_request_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.request_id')) VIRTUAL,
                event_message_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.message_id')) VIRTUAL,
                event_tool_call_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.tool_call_id')) VIRTUAL,
                event_operation_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.operation_id')) VIRTUAL,
                event_execution_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.execution_id')) VIRTUAL,
                event_subagent_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.subagent_id')) VIRTUAL,
                event_run_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.run_id')) VIRTUAL,
                event_interaction_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.interaction_id')) VIRTUAL,
                event_call_id GENERATED ALWAYS AS (json_extract(event_json,'$.event.call_id')) VIRTUAL,
                event_instance GENERATED ALWAYS AS (json_extract(event_json,'$.event.instance')) VIRTUAL,
                event_instance_run GENERATED ALWAYS AS (json_extract(event_json,'$.event.instance.block.run')) VIRTUAL,
                event_json TEXT NOT NULL,",
            crate::durable::sqlite::schema::events::constraint!(),
            ")"
        )
    };
}
pub(in crate::durable::sqlite) use table;

pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for (name, definition) in [
        ("events", table!()),
        ("rustx_store", super::store_root::table!()),
    ] {
        let exact: bool = connection
            .query_row(
                "SELECT sql=?2 FROM sqlite_schema WHERE type='table' AND name=?1",
                [name, &definition.replacen(" IF NOT EXISTS", "", 1)],
                |row| row.get(0),
            )
            .map_err(|error| sqlite_error(&error, error.to_string()))?;
        if !exact {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid event identity table definition: {name}"
            )));
        }
    }
    Ok(())
}
