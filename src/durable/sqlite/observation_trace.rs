//! Bounded typed read evidence over the existing original J/C/request rows.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::observation::{
    HEADER_ENCODED_MAX, ObservationReadCut, RequestReadSummary, TraceAdoptionClass,
    TraceBootstrapFact, TraceCanonicalSummary, TraceFactData, TraceFactHeader, TraceFactKind,
    TraceFactQuery, TraceFactScope, TraceRequestContextPage, TraceRequestPredecessorHeader,
    TraceTerminalClass, TraceTerminalHeader, TraceToolProposalSummary,
};
use crate::runtime_client::trace::{TraceFieldKind, TraceFieldSource};
mod canonical;
mod event;
mod inspection;
mod request;
pub(super) use canonical::{index_canonical, index_proposals};
#[cfg(test)]
pub(super) use event::{accepted_compare_work, accepted_identity_equal};
pub(super) use event::{index_event, request_core, settle_fields};
pub(super) use inspection::{adoption_page, capture_definitions, index_catalog, tool_definition};
pub(super) use request::{context, index_context, predecessor, retry_failure};

impl TraceFactKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AttemptStarted => "attempt_started",
            Self::AttemptCompleted => "attempt_completed",
            Self::AttemptFailed => "attempt_failed",
            Self::AttemptCancelled => "attempt_cancelled",
            Self::AttemptTimedOut => "attempt_timed_out",
            Self::AttemptLimitExceeded => "attempt_limit_exceeded",
            Self::TurnStarted => "turn_started",
            Self::TurnCompleted => "turn_completed",
            Self::InboundTurnAdopted => "inbound_turn_adopted",
            Self::ModelRequestStarted => "model_request_started",
            Self::ModelRequestCompleted => "model_request_completed",
            Self::ModelRequestFailed => "model_request_failed",
            Self::AssistantMessageCommitted => "assistant_message_committed",
            Self::ToolExecutionStarted => "tool_execution_started",
            Self::ToolExecutionCompleted => "tool_execution_completed",
            Self::ToolExecutionFailed => "tool_execution_failed",
            Self::ToolMessageCommitted => "tool_message_committed",
            Self::CompactionStarted => "compaction_started",
            Self::CompactionCompleted => "compaction_completed",
            Self::CompactionFailed => "compaction_failed",
            Self::BackgroundExecutionCommitted => "background_execution_committed",
            Self::BackgroundTerminalPublished => "background_terminal_published",
            Self::SubagentOwnershipCommitted => "subagent_ownership_committed",
            Self::SubagentTerminalPublished => "subagent_terminal_published",
            Self::SubagentTerminalSettled => "subagent_terminal_settled",
            Self::WorkflowStarted => "workflow_started",
            Self::WorkflowCompleted => "workflow_completed",
            Self::WorkflowFailed => "workflow_failed",
            Self::WorkflowCancelled => "workflow_cancelled",
            Self::InteractionRequested => "interaction_requested",
            Self::InteractionSettled => "interaction_settled",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        use TraceFactKind::{
            AssistantMessageCommitted, AttemptCancelled, AttemptCompleted, AttemptFailed,
            AttemptLimitExceeded, AttemptStarted, AttemptTimedOut, BackgroundExecutionCommitted,
            BackgroundTerminalPublished, CompactionCompleted, CompactionFailed, CompactionStarted,
            InboundTurnAdopted, InteractionRequested, InteractionSettled, ModelRequestCompleted,
            ModelRequestFailed, ModelRequestStarted, SubagentOwnershipCommitted,
            SubagentTerminalPublished, SubagentTerminalSettled, ToolExecutionCompleted,
            ToolExecutionFailed, ToolExecutionStarted, ToolMessageCommitted, TurnCompleted,
            TurnStarted, WorkflowCancelled, WorkflowCompleted, WorkflowFailed, WorkflowStarted,
        };
        [
            AttemptStarted,
            AttemptCompleted,
            AttemptFailed,
            AttemptCancelled,
            AttemptTimedOut,
            AttemptLimitExceeded,
            TurnStarted,
            TurnCompleted,
            InboundTurnAdopted,
            ModelRequestStarted,
            ModelRequestCompleted,
            ModelRequestFailed,
            AssistantMessageCommitted,
            ToolExecutionStarted,
            ToolExecutionCompleted,
            ToolExecutionFailed,
            ToolMessageCommitted,
            CompactionStarted,
            CompactionCompleted,
            CompactionFailed,
            BackgroundExecutionCommitted,
            BackgroundTerminalPublished,
            SubagentOwnershipCommitted,
            SubagentTerminalPublished,
            SubagentTerminalSettled,
            WorkflowStarted,
            WorkflowCompleted,
            WorkflowFailed,
            WorkflowCancelled,
            InteractionRequested,
            InteractionSettled,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == value)
    }
    pub(crate) const fn is_anchor(self) -> bool {
        matches!(
            self,
            Self::AttemptStarted
                | Self::TurnStarted
                | Self::InboundTurnAdopted
                | Self::ModelRequestStarted
                | Self::AssistantMessageCommitted
                | Self::ToolExecutionStarted
                | Self::CompactionStarted
                | Self::BackgroundExecutionCommitted
                | Self::SubagentOwnershipCommitted
                | Self::WorkflowStarted
                | Self::InteractionRequested
        )
    }
}
#[cfg(test)]
thread_local! { static READ_WORK:std::cell::Cell<(usize,usize)>=const {std::cell::Cell::new((0,0))}; }
#[cfg(test)]
pub(super) fn read_work() -> (usize, usize) {
    READ_WORK.with(|work| work.replace((0, 0)))
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ConversationStoreError> {
    if bytes.len() > HEADER_ENCODED_MAX {
        return Err(storage("native Trace header exceeds bound"));
    }
    #[cfg(test)]
    READ_WORK.with(|work| {
        let (rows, total) = work.get();
        work.set((rows + 1, total + bytes.len()));
    });
    serde_json::from_slice(bytes).map_err(|_| storage("native Trace header corrupt"))
}
fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(value, HEADER_ENCODED_MAX)
        .ok_or_else(|| storage("native Trace header exceeds bound"))
}
type HeaderRow = (Option<Vec<u8>>, Option<String>, String, Option<u64>);

pub(super) fn header(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<TraceFactHeader, ConversationStoreError> {
    if sequence == 0 || sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "Trace source beyond original J cut".into(),
        ));
    }
    let row:Option<HeaderRow>=connection.query_row(
        "SELECT observation_trace_fact,observation_kind,event_timestamp,observation_trace_owner FROM events WHERE sequence=?1",
        [sequence],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(|error|storage(error.to_string()))?;
    let (bytes, kind, time, owner) = row.ok_or_else(|| {
        ConversationStoreError::InvalidReference("original Trace J absent".into())
    })?;
    let header: TraceFactHeader =
        decode(&bytes.ok_or_else(|| storage("native Trace fact header unavailable"))?)?;
    if header.sequence != sequence
        || Some(header.kind.as_str()) != kind.as_deref()
        || header.timestamp.to_rfc3339() != time
        || header.owner_anchor != owner
    {
        return Err(storage(
            "native Trace fact header disagrees with original J",
        ));
    }
    Ok(header)
}
pub(super) fn facts(
    connection: &Connection,
    cut: &ObservationReadCut,
    query: &TraceFactQuery,
) -> Result<Vec<TraceFactHeader>, ConversationStoreError> {
    if query.limit == 0 || query.limit > 32 || query.kinds.is_empty() || query.kinds.len() > 32 {
        return Err(ConversationStoreError::InvalidReference(
            "invalid bounded Trace fact query".into(),
        ));
    }
    let through = cut.read.journal;
    if matches!(query.scope, TraceFactScope::All) {
        let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_trace_holes_idx WHERE observation_trace_fact_required=1 AND observation_trace_fact IS NULL AND sequence>?1 AND sequence<=?2 AND sequence<?3)",params![query.after,through,query.before.unwrap_or(through.saturating_add(1))],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        if missing {
            return Err(storage("native Trace fact header unavailable"));
        }
    }
    let before = query.before.unwrap_or(through.saturating_add(1));
    let owner = match query.scope {
        TraceFactScope::All => None,
        TraceFactScope::Owner { anchor_sequence } => {
            if !header(connection, anchor_sequence, through)?
                .kind
                .is_anchor()
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Trace owner is not an original anchor".into(),
                ));
            }
            let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_trace_holes_idx WHERE observation_trace_fact_required=1 AND observation_trace_fact IS NULL AND sequence>=?1 AND sequence<=?2)",params![anchor_sequence,through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
            if missing {
                return Err(storage("native Trace owner relationship unavailable"));
            }
            Some(anchor_sequence)
        }
    };
    // Seek only bounded numeric candidates per closed kind, then fetch <=32
    // headers. No original body and no growing multi-kind sort is read.
    let mut sequences = Vec::with_capacity(query.limit * query.kinds.len());
    for kind in &query.kinds {
        let (index, predicate) = if owner.is_some() {
            ("events_trace_owner_idx", "observation_trace_owner=?5")
        } else {
            ("events_trace_kind_idx", "1=1")
        };
        let sql = format!(
            "SELECT sequence FROM events INDEXED BY {index} WHERE observation_kind=?1 AND sequence<?2 AND sequence>?3 AND sequence<=?4 AND {predicate} ORDER BY sequence {} LIMIT ?6",
            if query.ascending { "ASC" } else { "DESC" }
        );
        let mut statement = connection
            .prepare_cached(&sql)
            .map_err(|error| storage(error.to_string()))?;
        let rows = statement
            .query_map(
                params![
                    kind.as_str(),
                    before,
                    query.after,
                    through,
                    owner,
                    query.limit
                ],
                |row| row.get::<_, u64>(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        for row in rows {
            sequences.push(row.map_err(|error| storage(error.to_string()))?);
        }
    }
    sequences.sort_unstable();
    sequences.dedup();
    if !query.ascending {
        sequences.reverse();
    }
    sequences.truncate(query.limit);
    sequences
        .into_iter()
        .map(|sequence| header(connection, sequence, through))
        .collect()
}
pub(super) fn summary(
    connection: &Connection,
    start: u64,
    through: u64,
) -> Result<Option<RequestReadSummary>, ConversationStoreError> {
    super::observation_request::validate_start(connection, start, through)?;
    let row:Option<Option<Vec<u8>>>=connection.query_row("SELECT observation_summary FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[start],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    row.map(|bytes| decode(&bytes.ok_or_else(|| storage("native request summary unavailable"))?))
        .transpose()
}
pub(super) fn canonical(
    connection: &Connection,
    cut: &ObservationReadCut,
    position: u64,
) -> Result<TraceCanonicalSummary, ConversationStoreError> {
    if position == 0 || position > cut.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "Trace C beyond original cut".into(),
        ));
    }
    canonical_fields_ready(connection, cut, position)?;
    let bytes: Option<Option<Vec<u8>>> = connection
        .query_row(
            "SELECT observation_trace_summary FROM message_ledger WHERE position=?1",
            [position],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let mut summary: TraceCanonicalSummary = decode(
        &bytes
            .flatten()
            .ok_or_else(|| storage("native canonical Trace summary unavailable"))?,
    )?;
    for call in &mut summary.calls {
        for field in &mut call.fields {
            event::prefer_original_raw(connection, &cut.read.conversation_id, field)?;
        }
    }
    for frontier in &mut summary.collections {
        if frontier.domain == format!("messages.ledger:{position}.tools") {
            frontier.revision = cut.read.journal;
        }
    }
    Ok(summary)
}
fn canonical_fields_ready(
    connection: &Connection,
    cut: &ObservationReadCut,
    position: u64,
) -> Result<(), ConversationStoreError> {
    let committed:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_assistant_ledger_idx WHERE observation_assistant_ledger=?1 AND observation_assistant_ledger IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![position,cut.read.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    if let Some(committed) = committed
        && header(connection, committed, cut.read.journal)?.canonical_fields_ready == Some(false)
    {
        return Err(storage("native canonical Trace fields unavailable"));
    }
    Ok(())
}
pub(super) fn tool_result(
    connection: &Connection,
    cut: &ObservationReadCut,
    occurrence: &crate::events::types::ToolObservationOccurrence,
) -> Result<Option<TraceCanonicalSummary>, ConversationStoreError> {
    if occurrence.ledger_position == 0 || occurrence.ledger_position > cut.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "Trace occurrence beyond C cut".into(),
        ));
    }
    let ordinal:u64=connection.query_row("SELECT observation_tool_ordinal FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 AND block_index=?2",params![occurrence.ledger_position,occurrence.block_index.get()],|row|row.get(0)).map_err(|_|storage("native Trace occurrence unavailable"))?;
    if ordinal != occurrence.tool_ordinal {
        return Err(ConversationStoreError::InvalidReference(
            "Trace occurrence differs from native C".into(),
        ));
    }
    let position:Option<u64>=connection.query_row("SELECT result_ledger_position FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 AND block_index=?2",params![occurrence.ledger_position,occurrence.block_index.get()],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    position
        .filter(|position| *position <= cut.ledger_through)
        .map(|position| canonical(connection, cut, position))
        .transpose()
}
pub(super) fn proposal(
    connection: &Connection,
    cut: &ObservationReadCut,
    occurrence: &crate::events::types::ToolObservationOccurrence,
) -> Result<TraceToolProposalSummary, ConversationStoreError> {
    if occurrence.ledger_position == 0 || occurrence.ledger_position > cut.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "Trace proposal beyond C cut".into(),
        ));
    }
    canonical_fields_ready(connection, cut, occurrence.ledger_position)?;
    let (ordinal,bytes):(u64,Option<Vec<u8>>)=connection.query_row("SELECT observation_tool_ordinal,observation_trace_proposal FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 AND block_index=?2",params![occurrence.ledger_position,occurrence.block_index.get()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|storage("native Trace proposal occurrence unavailable"))?;
    if ordinal != occurrence.tool_ordinal {
        return Err(ConversationStoreError::InvalidReference(
            "Trace proposal occurrence differs from native C".into(),
        ));
    }
    let mut proposal: TraceToolProposalSummary =
        decode(&bytes.ok_or_else(|| storage("native Trace proposal header unavailable"))?)?;
    for field in &mut proposal.fields {
        event::prefer_original_raw(connection, &cut.read.conversation_id, field)?;
    }
    Ok(proposal)
}
fn value_source(
    field: TraceFieldKind,
    sequence: u64,
    path: Vec<String>,
    value: &impl serde::Serialize,
) -> Result<TraceFieldSource, ConversationStoreError> {
    Ok(TraceFieldSource::Value {
        field,
        domain: format!("journal.{sequence}"),
        item_offset: 0,
        path,
        encoded_bytes: super::observation_request_summary::serialized_bytes(value)?,
    })
}

pub(super) fn required(event: &crate::events::types::RuntimeEventEnvelope) -> bool {
    super::observation_event::native_kind(&event.event)
        .ok()
        .and_then(|kind| TraceFactKind::parse(&kind))
        .is_some()
}

pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    for sql in [
        "CREATE INDEX publication_streams_message_settlement_idx ON publication_streams(message_id,settlement)",
        "CREATE INDEX events_execution_owner_idx ON events(observation_execution_owner,sequence) WHERE observation_execution IS NOT NULL",
        "CREATE INDEX events_execution_holes_idx ON events(sequence) WHERE observation_execution_required=1 AND observation_execution IS NULL",
        "CREATE INDEX events_trace_kind_idx ON events(observation_kind,sequence) WHERE observation_kind IS NOT NULL",
        "CREATE INDEX events_trace_owner_idx ON events(observation_trace_owner,observation_kind,sequence) WHERE observation_trace_owner IS NOT NULL",
        "CREATE INDEX events_trace_holes_idx ON events(sequence) WHERE observation_trace_fact_required=1 AND observation_trace_fact IS NULL",
        "CREATE UNIQUE INDEX request_trace_retry_idx ON request_snapshots(observation_step_anchor,observation_retry_number) WHERE observation_step_anchor IS NOT NULL",
        "CREATE UNIQUE INDEX request_observation_children_idx ON request_observation_fields(source_id,parent_path,ordinal)",
        "CREATE UNIQUE INDEX state_observation_children_idx ON state_observation_fields(source_id,parent_path,ordinal)",
        "CREATE INDEX request_trace_catalog_identity_idx ON request_trace_catalogs(provisional_message_id,source_sequence)",
        "CREATE INDEX request_trace_definition_identity_idx ON request_trace_definitions(source_sequence,tool_id,ordinal)",
        "CREATE INDEX request_trace_definition_digest_idx ON request_trace_definitions(source_sequence,tool_identity_digest,ordinal)",
    ] {
        let name = sql
            .split("INDEX ")
            .nth(1)
            .expect("literal index")
            .split_whitespace()
            .next()
            .expect("literal name");
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        if actual.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "invalid native Trace index".into(),
            ));
        }
    }
    verify_header_layout(connection)?;
    super::verify_unique_column(connection, "request_snapshots", "observation_source_id")?;
    Ok(())
}

fn verify_header_layout(connection: &Connection) -> Result<(), ConversationStoreError> {
    for (table, proof) in [
        (
            "events",
            "CHECK(observation_trace_fact IS NULL OR length(observation_trace_fact)<=16384)",
        ),
        (
            "message_ledger",
            "CHECK(observation_trace_summary IS NULL OR length(observation_trace_summary)<=16384)",
        ),
        (
            "canonical_tool_calls",
            "CHECK(observation_trace_proposal IS NULL OR length(observation_trace_proposal)<=16384)",
        ),
        (
            "canonical_tool_calls",
            "CHECK(observation_definition_ready IN (0,1))",
        ),
        (
            "canonical_tool_calls",
            "CHECK(observation_definition_ordinal IS NULL OR observation_definition_ordinal>=0)",
        ),
        (
            "trace_adoption_members",
            "CHECK(length(identity_digest)=32)",
        ),
        ("request_trace_catalogs", "CHECK(tool_count>=0)"),
        ("request_trace_definitions", "CHECK(ordinal>=0)"),
        (
            "request_trace_definitions",
            "CHECK(length(tool_identity_digest)=32)",
        ),
        (
            "canonical_tool_calls",
            "CHECK(observation_tool_identity_digest IS NULL OR length(observation_tool_identity_digest)=32)",
        ),
        (
            "observation_models",
            "CHECK(request_alias IS NULL OR length(CAST(request_alias AS BLOB))<=512)",
        ),
        (
            "request_trace_contexts",
            "CHECK(length(header_json)<=16384)",
        ),
        (
            "request_snapshots",
            "CAST(observation_source_id AS INTEGER)=started_sequence",
        ),
        (
            "request_observation_fields",
            "REFERENCES request_snapshots(observation_source_id)",
        ),
    ] {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        if !sql.contains(proof) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "invalid native Trace header layout".into(),
            ));
        }
    }
    Ok(())
}
