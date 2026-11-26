//! Original Workflow node admissions and field selectors at one immutable cut.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::observation::trace::{TraceFieldKind, TraceFieldSource, TraceTiming};
use crate::observation::{
    ObservationReadCut, WorkflowNodeTraceState, WorkflowTraceNode, WorkflowTraceNodes,
};

#[cfg(test)]
mod query_work;

// These queries seek the exact run or visit before applying the immutable
// Journal cut. In particular, missing I/O and terminal facts do not scan
// later visits or unrelated runs to prove absence.
pub const TOOL_START_SQL: &str = concat!(
    "SELECT observation_trace_owner FROM events INDEXED BY events_observation_tool_idx WHERE event_tool_call_id=?1 AND attempt_key = ",
    crate::sqlite::event_owner_key::attempt_parameter!("?2"),
    " AND event_kind='tool_execution_started' AND sequence<=?3 LIMIT 2"
);

const RUN_END_SQL: &str = "SELECT min(sequence) FROM events INDEXED BY events_run_id_kind_idx WHERE event_run_id=?1 AND event_kind IN ('workflow_completed','workflow_failed','workflow_cancelled') AND sequence>?2 AND sequence<=?3";
const RUN_TOTAL_SQL: &str = "SELECT count(*) FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE event_instance_run=?3 AND event_kind='workflow_node_started' AND sequence>?1 AND sequence<=?2";
const RUN_REVISION_SQL: &str = "SELECT max(sequence) FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE event_instance_run=?3 AND event_kind IN ('workflow_node_started','workflow_node_waiting','workflow_node_settled') AND sequence>?1 AND sequence<=?2";
const RUN_NODES_SQL: &str = "SELECT sequence,event_timestamp,CASE WHEN octet_length(event_instance)<=8192 THEN event_instance END FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE event_instance_run=?3 AND event_kind='workflow_node_started' AND sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT ?4 OFFSET ?5";
const NODE_VISIT_SQL: &str = "SELECT sequence,event_timestamp FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE event_instance=?3 AND event_kind='workflow_node_started' AND sequence>?1 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const NODE_TERMINAL_SQL: &str = "SELECT event_timestamp,json_extract(event_json,'$.event.outcome') FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE event_instance=?3 AND event_kind='workflow_node_settled' AND sequence>?1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1";
const NODE_IO_SQL: &str = "SELECT sequence,json_extract(event_json,'$.event.kind'),substr(json_extract(event_json,'$.event.value'),1,192),CASE WHEN json_type(event_json,'$.event.value')='text' THEN octet_length(json_quote(json_extract(event_json,'$.event.value'))) END FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE event_instance=?4 AND event_kind=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 1";
pub const RUN_START_SQL: &str = "SELECT sequence FROM events INDEXED BY events_run_id_kind_idx WHERE event_run_id=?1 AND event_kind='workflow_started' AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
pub const INSTANCE_JSON_MAX: usize = 8192;

fn run_through(
    connection: &Connection,
    run: &str,
    start: u64,
    journal_cut: u64,
) -> Result<u64, ConversationStoreError> {
    let end: Option<u64> = connection
        .query_row(
            RUN_END_SQL,
            params![
                run,
                super::seq_to_i64(start)?,
                super::seq_to_i64(journal_cut)?
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(end.unwrap_or(journal_cut))
}

pub fn validate_run_id(
    conversation_id: &rustx_types::runtime::identity::ConversationId,
    run_id: &rustx_types::runtime::workflow::WorkflowRunId,
) -> Result<String, ConversationStoreError> {
    if run_id.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(
            "Workflow run belongs to another conversation".into(),
        ));
    }
    let origin = match &run_id.origin {
        rustx_types::runtime::workflow::WorkflowRunOrigin::Tool { attempt_id } => {
            attempt_id.as_str()
        }
        rustx_types::runtime::workflow::WorkflowRunOrigin::Scheduled { start_id } => {
            start_id.as_str()
        }
    };
    if origin.is_empty()
        || origin.len() > rustx_types::runtime::workflow::MAX_RUN_ID_COMPONENT_BYTES
        || run_id.conversation_id.as_str().len()
            > rustx_types::runtime::workflow::MAX_RUN_ID_COMPONENT_BYTES
    {
        return Err(ConversationStoreError::InvalidReference(
            "Workflow run identity exceeds admitted bounds".into(),
        ));
    }
    serde_json::to_string(run_id).map_err(|error| storage(error.to_string()))
}

fn validate_instance(
    conversation_id: &rustx_types::runtime::identity::ConversationId,
    instance: &rustx_types::runtime::workflow::WorkflowNodeInstance,
) -> Result<(String, String), ConversationStoreError> {
    let run = validate_run_id(conversation_id, &instance.block.run)?;
    if instance.node.is_empty()
        || instance.node.len() > INSTANCE_JSON_MAX
        || !instance.block.has_valid_nesting()
        || instance
            .block
            .definition
            .blocks
            .iter()
            .any(|part| part.is_empty() || part.len() > INSTANCE_JSON_MAX)
    {
        return Err(ConversationStoreError::InvalidReference(
            "Workflow node instance exceeds admitted bounds".into(),
        ));
    }
    let encoded = serde_json::to_string(instance).map_err(|error| storage(error.to_string()))?;
    if encoded.len() > INSTANCE_JSON_MAX {
        return Err(ConversationStoreError::ObservationCapacityExceeded);
    }
    Ok((run, encoded))
}

pub fn visit(
    connection: &Connection,
    conversation_id: &rustx_types::runtime::identity::ConversationId,
    cut: &ObservationReadCut,
    instance: &rustx_types::runtime::workflow::WorkflowNodeInstance,
) -> Result<Option<WorkflowTraceNode>, ConversationStoreError> {
    let (run, encoded) = validate_instance(conversation_id, instance)?;
    let Some(start): Option<u64> = connection
        .query_row(
            RUN_START_SQL,
            params![run, super::seq_to_i64(cut.read.journal)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
    else {
        return Ok(None);
    };
    let through = run_through(connection, &run, start, cut.read.journal)?;
    let Some((sequence, timestamp)): Option<(u64, String)> = connection
        .query_row(
            NODE_VISIT_SQL,
            params![
                super::seq_to_i64(start)?,
                super::seq_to_i64(through)?,
                encoded
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
    else {
        return Ok(None);
    };
    node(connection, sequence, &timestamp, Some(encoded), through).map(Some)
}
const NODE_OWNERSHIP_SQL: &str = "SELECT json_extract(event_json,'$.event.subagent_id'),json_extract(event_json,'$.event.child_agent_id') FROM events INDEXED BY events_workflow_agent_node_idx WHERE event_workflow_agent_node=?3 AND event_kind='subagent_ownership_committed' AND json_extract(event_json,'$.event.ownership')='workflow' AND json_extract(event_json,'$.event.origin.kind')='workflow' AND sequence>?1 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const NODE_ADMITTED_SQL: &str = "SELECT json_extract(event_json,'$.event.subagent_id') FROM events INDEXED BY events_workflow_agent_node_idx WHERE event_workflow_agent_node=?3 AND event_kind='workflow_agent_admitted' AND sequence>?1 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const NODE_OWNERSHIP_BY_SUBAGENT_SQL: &str = "SELECT json_extract(event_json,'$.event.child_agent_id') FROM events INDEXED BY events_subagent_id_kind_idx WHERE event_subagent_id=?1 AND event_kind='subagent_ownership_committed' AND json_extract(event_json,'$.event.ownership')='workflow' AND json_extract(event_json,'$.event.origin.kind')='workflow' AND json_extract(event_json,'$.event.origin.node_id')=?3 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";

fn invalid() -> ConversationStoreError {
    ConversationStoreError::InvalidReference("invalid original Workflow node selection".into())
}

pub fn page(
    connection: &Connection,
    cut: &ObservationReadCut,
    start: u64,
    offset: u64,
    limit: usize,
) -> Result<WorkflowTraceNodes, ConversationStoreError> {
    if start == 0 || start > cut.read.journal || limit > 32 {
        return Err(invalid());
    }
    // Only bounded original identity fields are read; a generic J body is never hydrated.
    let run: Option<String> = connection.query_row(
        "SELECT CASE WHEN octet_length(event_run_id)<=8192 THEN event_run_id END FROM events WHERE sequence=?1 AND event_kind='workflow_started'",
        [start], |row| row.get(0)).optional().map_err(|error| crate::sqlite::sqlite_error(&error,error.to_string()))?.flatten();
    let run = run.ok_or_else(invalid)?;
    let (workflow_id, call_id): (String, Option<String>) = connection
        .query_row(
            "SELECT json_extract(event_json,'$.event.workflow_id'),event_tool_call_id FROM events WHERE sequence=?1",
            [start],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    let run_id: rustx_types::runtime::workflow::WorkflowRunId =
        serde_json::from_str(&run).map_err(|error| storage(error.to_string()))?;
    let workflow_id = rustx_types::runtime::workflow::WorkflowId::parse(&workflow_id)
        .map_err(|error| storage(error.to_string()))?;
    let mut parents = Vec::new();
    if let (Some(attempt), Some(call)) = (run_id.origin.attempt_id(), call_id) {
        let mut query = connection
            .prepare_cached(TOOL_START_SQL)
            .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
        parents = query
            .query_map(params![call, attempt.as_str(), start], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(error.to_string()))?;
    }
    // Execution proves the Workflow's tool ownership; its captured Trace owner
    // identifies the request record that remains visible across authorization.
    let parent_sequence = (parents.len() == 1).then(|| parents[0]);
    let through = run_through(connection, &run, start, cut.read.journal)?;
    let total: u64 = connection
        .query_row(RUN_TOTAL_SQL, params![start, through, run], |row| {
            row.get(0)
        })
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    let revision: Option<u64> = connection
        .query_row(RUN_REVISION_SQL, params![start, through, run], |row| {
            row.get(0)
        })
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    let mut statement = connection
        .prepare_cached(RUN_NODES_SQL)
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    let starts = statement
        .query_map(params![start, through, run, limit as u64, offset], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    let mut nodes = Vec::with_capacity(starts.len());
    for (sequence, timestamp, instance) in starts {
        nodes.push(node(connection, sequence, &timestamp, instance, through)?);
    }
    let next = offset.saturating_add(nodes.len() as u64);
    Ok(WorkflowTraceNodes {
        revision: revision.unwrap_or(start),
        run_id,
        workflow_id,
        parent_sequence,
        total,
        nodes,
        next_offset: (next < total).then_some(next),
    })
}

fn node(
    connection: &Connection,
    sequence: u64,
    timestamp: &str,
    instance: Option<String>,
    through: u64,
) -> Result<WorkflowTraceNode, ConversationStoreError> {
    let instance = instance.ok_or(ConversationStoreError::ObservationCapacityExceeded)?;
    let input = io(
        connection,
        sequence,
        through,
        &instance,
        "workflow_node_input_recorded",
        TraceFieldKind::Arguments,
    )?;
    let output = io(
        connection,
        sequence,
        through,
        &instance,
        "workflow_node_output_recorded",
        TraceFieldKind::Result,
    )?;
    // A recorded wait is historical evidence, never proof that its process is
    // alive. Only a terminal establishes durable lifecycle at this read cut.
    let ending: Option<(String, String)> = connection
        .query_row(
            NODE_TERMINAL_SQL,
            params![sequence, through, instance],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    let started_at = timestamp
        .parse::<chrono::DateTime<chrono::Utc>>()
        .map_err(|error| storage(error.to_string()))?;
    let (state, ended_at) = match ending {
        Some((time, outcome)) => (
            WorkflowNodeTraceState::Settled {
                outcome: serde_json::from_value(serde_json::Value::String(outcome))
                    .map_err(|error| storage(error.to_string()))?,
            },
            Some(
                time.parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|error| storage(error.to_string()))?,
            ),
        ),
        None => (WorkflowNodeTraceState::Incomplete, None),
    };
    let kind: Option<rustx_types::runtime::workflow::read_model::WorkflowNodeKind> = input
        .as_ref()
        .and_then(|io| io.kind.clone())
        .map(|kind| serde_json::from_value(serde_json::Value::String(kind)))
        .transpose()
        .map_err(|error| storage(error.to_string()))?;
    let (child, child_agent_id) = if matches!(
        kind,
        None | Some(rustx_types::runtime::workflow::read_model::WorkflowNodeKind::Agent)
    ) {
        child_ref(connection, sequence, through, &instance)?
    } else {
        (None, None)
    };
    Ok(WorkflowTraceNode {
        id: format!("workflow-node:{sequence}"),
        instance: serde_json::from_str(&instance).map_err(|error| storage(error.to_string()))?,
        kind,
        state,
        timing: TraceTiming {
            started_at,
            ended_at,
            duration_ms: ended_at
                .map(|end| u64::try_from((end - started_at).num_milliseconds()).unwrap_or(0)),
        },
        input: input.as_ref().and_then(|io| io.field.clone()),
        output: output.as_ref().and_then(|io| io.field.clone()),
        input_preview: input.as_ref().and_then(|io| io.preview.clone()),
        output_preview: output.as_ref().and_then(|io| io.preview.clone()),
        input_omitted: input.is_some_and(|io| io.field.is_none()),
        output_omitted: output.is_some_and(|io| io.field.is_none()),
        child,
        child_agent_id,
    })
}

fn child_ref(
    connection: &Connection,
    start: u64,
    through: u64,
    instance: &str,
) -> Result<
    (
        Option<rustx_types::runtime::identity::SubagentId>,
        Option<rustx_types::runtime::identity::AgentId>,
    ),
    ConversationStoreError,
> {
    let ownership: Option<(String, String)> = connection
        .query_row(
            NODE_OWNERSHIP_SQL,
            params![start, through, instance],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    if let Some((subagent_id, agent_id)) = ownership {
        return Ok((
            Some(rustx_types::runtime::identity::SubagentId::new(subagent_id)),
            Some(rustx_types::runtime::identity::AgentId::new(agent_id)),
        ));
    }
    let admitted: Option<String> = connection
        .query_row(
            NODE_ADMITTED_SQL,
            params![start, through, instance],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let Some(subagent_id) = admitted else {
        return Ok((None, None));
    };
    let agent_id: Option<String> = connection
        .query_row(
            NODE_OWNERSHIP_BY_SUBAGENT_SQL,
            params![subagent_id.as_str(), through, instance],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    Ok((
        Some(rustx_types::runtime::identity::SubagentId::new(subagent_id)),
        agent_id.map(rustx_types::runtime::identity::AgentId::new),
    ))
}

type IoHeader = (u64, Option<String>, Option<String>, Option<u64>);
struct Io {
    field: Option<TraceFieldSource>,
    preview: Option<String>,
    kind: Option<String>,
}
fn io(
    connection: &Connection,
    start: u64,
    through: u64,
    instance: &str,
    event: &str,
    field: TraceFieldKind,
) -> Result<Option<Io>, ConversationStoreError> {
    let row: Option<IoHeader> = connection
        .query_row(
            NODE_IO_SQL,
            params![event, start, through, instance],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(row.map(|(sequence, kind, preview, length)| Io {
        kind,
        preview,
        field: length.map(|encoded_bytes| TraceFieldSource::Value {
            field,
            domain: format!("journal.{sequence}"),
            item_offset: 0,
            path: vec!["event".into(), "value".into()],
            encoded_bytes,
        }),
    }))
}
