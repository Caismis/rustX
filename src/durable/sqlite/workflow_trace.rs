//! Original Workflow node admissions and field selectors at one immutable cut.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::observation::{
    ObservationReadCut, WorkflowNodeTraceState, WorkflowTraceNode, WorkflowTraceNodes,
};
use crate::observation::trace::{TraceFieldKind, TraceFieldSource, TraceTiming};

#[cfg(test)]
mod query_work;

// These queries seek the exact run or visit before applying the immutable
// Journal cut. In particular, missing I/O and terminal facts do not scan
// later visits or unrelated runs to prove absence.
const RUN_END_SQL: &str = "SELECT min(sequence) FROM events INDEXED BY events_run_id_kind_idx WHERE json_extract(event_json,'$.event.run_id')=?1 AND json_extract(event_json,'$.event.type') IN ('workflow_completed','workflow_failed','workflow_cancelled') AND sequence>?2 AND sequence<=?3";
const RUN_TOTAL_SQL: &str = "SELECT count(*) FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE json_extract(event_json,'$.event.instance.block.run')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_started' AND sequence>?1 AND sequence<=?2";
const RUN_REVISION_SQL: &str = "SELECT max(sequence) FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE json_extract(event_json,'$.event.instance.block.run')=?3 AND json_extract(event_json,'$.event.type') IN ('workflow_node_started','workflow_node_waiting','workflow_node_settled') AND sequence>?1 AND sequence<=?2";
const RUN_NODES_SQL: &str = "SELECT sequence,event_timestamp,CASE WHEN octet_length(json_extract(event_json,'$.event.instance'))<=8192 THEN json_extract(event_json,'$.event.instance') END FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE json_extract(event_json,'$.event.instance.block.run')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_started' AND sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT ?4 OFFSET ?5";
const NODE_VISIT_SQL: &str = "SELECT sequence,event_timestamp FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE json_extract(event_json,'$.event.instance')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_started' AND sequence>?1 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const NODE_TERMINAL_SQL: &str = "SELECT event_timestamp,json_extract(event_json,'$.event.outcome') FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE json_extract(event_json,'$.event.instance')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_settled' AND sequence>?1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1";
const NODE_IO_SQL: &str = "SELECT sequence,json_extract(event_json,'$.event.kind'),substr(json_extract(event_json,'$.event.value'),1,192),CASE WHEN json_type(event_json,'$.event.value')='text' THEN octet_length(json_quote(json_extract(event_json,'$.event.value'))) END FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE json_extract(event_json,'$.event.instance')=?4 AND json_extract(event_json,'$.event.type')=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 1";
pub(super) const RUN_START_SQL: &str = "SELECT sequence FROM events INDEXED BY events_run_id_kind_idx WHERE json_extract(event_json,'$.event.run_id')=?1 AND json_extract(event_json,'$.event.type')='workflow_started' AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const INSTANCE_JSON_MAX: usize = 8192;

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

pub(super) fn validate_run_id(
    conversation_id: &crate::runtime::identity::ConversationId,
    run_id: &crate::runtime::workflow::WorkflowRunId,
) -> Result<String, ConversationStoreError> {
    if run_id.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(
            "Workflow run belongs to another conversation".into(),
        ));
    }
    let origin = match &run_id.origin {
        crate::runtime::workflow::WorkflowRunOrigin::Tool { attempt_id } => attempt_id.as_str(),
        crate::runtime::workflow::WorkflowRunOrigin::Scheduled { start_id } => start_id.as_str(),
    };
    if origin.is_empty()
        || origin.len() > crate::runtime::workflow::MAX_RUN_ID_COMPONENT_BYTES
        || run_id.conversation_id.as_str().len()
            > crate::runtime::workflow::MAX_RUN_ID_COMPONENT_BYTES
    {
        return Err(ConversationStoreError::InvalidReference(
            "Workflow run identity exceeds admitted bounds".into(),
        ));
    }
    serde_json::to_string(run_id).map_err(|error| storage(error.to_string()))
}

fn validate_instance(
    conversation_id: &crate::runtime::identity::ConversationId,
    instance: &crate::runtime::workflow::WorkflowNodeInstance,
) -> Result<(String, String), ConversationStoreError> {
    let run = validate_run_id(conversation_id, &instance.block.run)?;
    if instance.node.is_empty()
        || instance.node.len() > INSTANCE_JSON_MAX
        || instance.block.definition.blocks.len() > crate::runtime::workflow::MAX_BLOCK_DEPTH * 2
        || instance
            .block
            .definition
            .blocks
            .iter()
            .any(|part| part.is_empty() || part.len() > INSTANCE_JSON_MAX)
        || instance.block.invocations.len() > crate::runtime::workflow::MAX_BLOCK_DEPTH
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

pub(super) fn visit(
    connection: &Connection,
    conversation_id: &crate::runtime::identity::ConversationId,
    cut: &ObservationReadCut,
    instance: &crate::runtime::workflow::WorkflowNodeInstance,
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
const NODE_OWNERSHIP_SQL: &str = "SELECT json_extract(event_json,'$.event.subagent_id'),json_extract(event_json,'$.event.child_agent_id') FROM events INDEXED BY events_workflow_agent_node_idx WHERE COALESCE(json_extract(event_json,'$.event.origin.node_id'),json_extract(event_json,'$.event.node_id'))=?3 AND json_extract(event_json,'$.event.type')='subagent_ownership_committed' AND json_extract(event_json,'$.event.ownership')='workflow' AND json_extract(event_json,'$.event.origin.kind')='workflow' AND sequence>?1 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const NODE_ADMITTED_SQL: &str = "SELECT json_extract(event_json,'$.event.subagent_id') FROM events INDEXED BY events_workflow_agent_node_idx WHERE COALESCE(json_extract(event_json,'$.event.origin.node_id'),json_extract(event_json,'$.event.node_id'))=?3 AND json_extract(event_json,'$.event.type')='workflow_agent_admitted' AND sequence>?1 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";
const NODE_OWNERSHIP_BY_SUBAGENT_SQL: &str = "SELECT json_extract(event_json,'$.event.child_agent_id') FROM events INDEXED BY events_subagent_id_kind_idx WHERE json_extract(event_json,'$.event.subagent_id')=?1 AND json_extract(event_json,'$.event.type')='subagent_ownership_committed' AND json_extract(event_json,'$.event.ownership')='workflow' AND json_extract(event_json,'$.event.origin.kind')='workflow' AND json_extract(event_json,'$.event.origin.node_id')=?3 AND sequence<=?2 ORDER BY sequence ASC LIMIT 1";

fn invalid() -> ConversationStoreError {
    ConversationStoreError::InvalidReference("invalid original Workflow node selection".into())
}

pub(super) fn page(
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
        "SELECT CASE WHEN octet_length(json_extract(event_json,'$.event.run_id'))<=8192 THEN json_extract(event_json,'$.event.run_id') END FROM events WHERE sequence=?1 AND json_extract(event_json,'$.event.type')='workflow_started'",
        [start], |row| row.get(0)).optional().map_err(|error| storage(error.to_string()))?.flatten();
    let run = run.ok_or_else(invalid)?;
    let (workflow_id, call_id): (String, Option<String>) = connection
        .query_row(
            "SELECT json_extract(event_json,'$.event.workflow_id'),json_extract(event_json,'$.event.tool_call_id') FROM events WHERE sequence=?1",
            [start],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| storage(error.to_string()))?;
    let run_id: crate::runtime::workflow::WorkflowRunId =
        serde_json::from_str(&run).map_err(|error| storage(error.to_string()))?;
    let workflow_id = crate::runtime::workflow::WorkflowId::parse(&workflow_id)
        .map_err(|error| storage(error.to_string()))?;
    let mut parents = Vec::new();
    if let (Some(attempt), Some(call)) = (run_id.origin.attempt_id(), call_id) {
        let mut query=connection.prepare_cached("SELECT sequence FROM events INDEXED BY events_tool_call_scope_kind_idx WHERE json_extract(event_json,'$.event.tool_call_id')=?1 AND attempt_id=?2 AND json_extract(event_json,'$.event.type')='tool_execution_started' AND sequence<=?3 LIMIT 2").map_err(|error|storage(error.to_string()))?;
        parents = query
            .query_map(params![call, attempt.as_str(), start], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(|error| storage(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(error.to_string()))?;
    }
    let parent_sequence = (parents.len() == 1).then(|| parents[0]);
    let through = run_through(connection, &run, start, cut.read.journal)?;
    let total: u64 = connection
        .query_row(RUN_TOTAL_SQL, params![start, through, run], |row| {
            row.get(0)
        })
        .map_err(|error| storage(error.to_string()))?;
    let revision: Option<u64> = connection
        .query_row(RUN_REVISION_SQL, params![start, through, run], |row| {
            row.get(0)
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut statement = connection
        .prepare_cached(RUN_NODES_SQL)
        .map_err(|error| storage(error.to_string()))?;
    let starts = statement
        .query_map(params![start, through, run, limit as u64, offset], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?
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
        .map_err(|error| storage(error.to_string()))?;
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
    let kind: Option<crate::runtime::workflow::read_model::WorkflowNodeKind> = input
        .as_ref()
        .and_then(|io| io.kind.clone())
        .map(|kind| serde_json::from_value(serde_json::Value::String(kind)))
        .transpose()
        .map_err(|error| storage(error.to_string()))?;
    let (child, child_agent_id) = if matches!(
        kind,
        None | Some(crate::runtime::workflow::read_model::WorkflowNodeKind::Agent)
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
        Option<crate::runtime::identity::SubagentId>,
        Option<crate::runtime::identity::AgentId>,
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
            Some(crate::runtime::identity::SubagentId::new(subagent_id)),
            Some(crate::runtime::identity::AgentId::new(agent_id)),
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
        Some(crate::runtime::identity::SubagentId::new(subagent_id)),
        agent_id.map(crate::runtime::identity::AgentId::new),
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
        .map_err(|error| storage(error.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::sqlite::test_support::{envelope, store};
    use crate::durable::{CanonicalStore, ConversationStore, JournalStore, ObservationStore};
    use crate::events::RuntimeEvent as E;
    use crate::runtime::workflow::{WorkflowExecutionOutcome as Outcome, WorkflowId};

    #[test]
    fn workflow_nodes_keep_real_visits_original_cuts_and_bounded_bodies() {
        let store = store();
        store.initialize(&[]).unwrap();
        let mut first = crate::runtime::workflow::test_instance("trace_flow", "transform");
        first.block.run.conversation_id = store.conversation_id().clone();
        let run = first.block.run.clone();
        let workflow_id = WorkflowId::parse("trace_flow").unwrap();
        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            store
                .append_event(envelope(
                    store.conversation_id(),
                    &format!("wf-trace-{ordinal}"),
                    None,
                    event,
                ))
                .unwrap()
                .sequence
        };
        let start = append(E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: workflow_id.clone(),
            run_id: run.clone(),
        });
        append(E::WorkflowNodeStarted {
            instance: first.clone(),
        });
        let before = store.observation_read_cut().unwrap();
        let input = r#"{"args":{"n":18446744073709551615},"variables":{}}"#.to_owned();
        append(E::WorkflowNodeInputRecorded {
            instance: first.clone(),
            kind: crate::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: Some(input.clone()),
        });
        append(E::WorkflowNodeOutputRecorded {
            instance: first.clone(),
            value: Some(r#"{"value":"rendered"}"#.into()),
        });
        append(E::WorkflowNodeSettled {
            instance: first.clone(),
            outcome: Outcome::Completed,
        });
        let mut second = first.clone();
        second.block.invocations.push(2);
        append(E::WorkflowNodeStarted {
            instance: second.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: second.clone(),
            kind: crate::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: None,
        });
        append(E::WorkflowNodeSettled {
            instance: second.clone(),
            outcome: Outcome::Cancelled,
        });
        append(E::WorkflowCancelled {
            workflow_id,
            run_id: run,
            reason: crate::runtime::types::CancellationReason::UserRequested,
        });
        let cut = store.observation_read_cut().unwrap();
        let old = store
            .observation_workflow_nodes(&before, start, 0, 32)
            .unwrap();
        assert_eq!(old.total, 1);
        assert_eq!(old.nodes[0].state, WorkflowNodeTraceState::Incomplete);
        assert!(old.nodes[0].input.is_none());
        let first_page = store.observation_workflow_nodes(&cut, start, 0, 1).unwrap();
        assert_eq!(first_page.total, 2);
        assert_eq!(first_page.next_offset, Some(1));
        assert_eq!(first_page.nodes[0].instance, first);
        assert_eq!(
            first_page.nodes[0].state,
            WorkflowNodeTraceState::Settled {
                outcome: Outcome::Completed
            }
        );
        let Some(TraceFieldSource::Value { encoded_bytes, .. }) = &first_page.nodes[0].input else {
            panic!("original input selector")
        };
        assert_eq!(
            *encoded_bytes,
            serde_json::to_vec(&input).unwrap().len() as u64
        );
        let last = store.observation_workflow_nodes(&cut, start, 1, 1).unwrap();
        assert_eq!(last.nodes[0].instance, second);
        assert!(last.nodes[0].input_omitted);
        assert!(last.nodes[0].output.is_none());
        assert_eq!(last.next_offset, None);
        assert!(
            store
                .observation_workflow_nodes(&cut, start, 0, 33)
                .is_err()
        );
        assert!(
            store
                .observation_workflow_nodes(&cut, start + 1, 0, 1)
                .is_err()
        );
    }

    #[test]
    fn reopened_workflow_nodes_require_terminals_and_preserve_original_cuts() {
        use crate::durable::SqliteConversationStore;
        use crate::runtime::workflow::read_model::WorkflowWait;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow-trace.sqlite");
        let conversation = crate::runtime::identity::ConversationId::generate();
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize(&[]).unwrap();
        let mut started = crate::runtime::workflow::test_instance("reopened", "start-only");
        started.block.run.conversation_id = conversation.clone();
        let mut waiting = started.clone();
        waiting.node = "wait-only".into();
        let mut completed = started.clone();
        completed.node = "completed".into();
        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            owner
                .append_event(envelope(
                    &conversation,
                    &format!("reopen-node-{ordinal}"),
                    None,
                    event,
                ))
                .unwrap()
                .sequence
        };
        let start = append(E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: WorkflowId::parse("reopened").unwrap(),
            run_id: started.block.run.clone(),
        });
        for instance in [&started, &waiting, &completed] {
            append(E::WorkflowNodeStarted {
                instance: instance.clone(),
            });
        }
        append(E::WorkflowNodeWaiting {
            instance: waiting,
            reason: WorkflowWait::Review,
            auxiliary_call_id: None,
        });
        let before_terminal = owner.observation_read_cut().unwrap();
        append(E::WorkflowNodeSettled {
            instance: completed.clone(),
            outcome: Outcome::Completed,
        });
        let after_terminal = owner.observation_read_cut().unwrap();
        let finished = owner
            .observation_workflow_nodes(&after_terminal, start, 0, 32)
            .unwrap();
        assert_eq!(
            finished.nodes[2].state,
            WorkflowNodeTraceState::Settled {
                outcome: Outcome::Completed
            }
        );
        assert!(finished.nodes[2].timing.ended_at.is_some());
        drop(owner);

        // A reopened durable store has no surviving executor or recovery owner.
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        let old = reopened
            .observation_workflow_nodes(&before_terminal, start, 0, 32)
            .unwrap();
        assert_eq!(old.nodes.len(), 3);
        for node in &old.nodes {
            assert_eq!(node.state, WorkflowNodeTraceState::Incomplete);
            assert!(node.timing.ended_at.is_none());
            assert!(node.timing.duration_ms.is_none());
        }
        let current = reopened
            .observation_workflow_nodes(&after_terminal, start, 0, 32)
            .unwrap();
        assert_eq!(current.nodes, finished.nodes);
        assert_eq!(current.nodes[0].state, WorkflowNodeTraceState::Incomplete);
        assert_eq!(current.nodes[1].state, WorkflowNodeTraceState::Incomplete);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One cut/reopen scenario keeps the exact child identity witnesses together.
    fn agent_node_child_references_follow_exact_visit_and_cut() {
        use crate::durable::SqliteConversationStore;
        use crate::runtime::identity::{AgentId, SubagentId};
        use crate::runtime::subagent::{AgentActivationOrigin, SubagentName};
        use crate::runtime::workflow::read_model::WorkflowNodeKind;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow-agent-trace.sqlite");
        let conversation = crate::runtime::identity::ConversationId::generate();
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize(&[]).unwrap();
        let workflow_id = WorkflowId::parse("agent_visits").unwrap();
        let mut first = crate::runtime::workflow::test_instance("agent_visits", "worker");
        first.block.run.conversation_id = conversation.clone();
        let mut second = first.clone();
        second.visit = 2;
        let mut non_agent = first.clone();
        non_agent.node = "template".into();

        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            let id = match &event {
                E::SubagentOwnershipCommitted { subagent_id, .. } => {
                    crate::runtime::subagent::subagent_ownership_event_id(subagent_id).to_string()
                }
                _ => format!("agent-trace-{ordinal}"),
            };
            owner
                .append_event(envelope(&conversation, &id, None, event))
                .unwrap()
                .sequence
        };
        let start = append(E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: workflow_id.clone(),
            run_id: first.block.run.clone(),
        });
        append(E::WorkflowNodeStarted {
            instance: first.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: first.clone(),
            kind: WorkflowNodeKind::Agent,
            value: Some("{}".into()),
        });
        let before_admission = owner.observation_read_cut().unwrap();

        let subagent_1 = SubagentId::for_conversation(&conversation, 1);
        let child_agent_1 = AgentId::new("child-worker-1");
        append(E::SubagentOwnershipCommitted {
            parent_agent_id: AgentId::new("parent"),
            admitted_authority: None,
            subagent_id: subagent_1.clone(),
            child_agent_id: child_agent_1.clone(),
            child_conversation_id: crate::scripted_suites::common::identity::child_conversation_id(
                subagent_1.as_str(),
            ),
            origin: AgentActivationOrigin::Workflow {
                node_id: Box::new(first.clone()),
            },
            agent: "worker".into(),
            definition_digest: "sha256:def".into(),
            profile_digest: "sha256:prof".into(),
            ownership: crate::events::types::SubagentOwnershipKind::Workflow,
            workspace: directory.path().to_path_buf(),
        });
        let after_ownership = owner.observation_read_cut().unwrap();

        append(E::WorkflowAgentAdmitted {
            workflow_id: workflow_id.clone(),
            run_id: first.block.run.clone(),
            node_id: first.clone(),
            subagent_id: subagent_1.clone(),
            profile: SubagentName::parse("worker").unwrap(),
        });
        append(E::WorkflowNodeSettled {
            instance: first.clone(),
            outcome: Outcome::Completed,
        });

        // Second visit to the same named node gets its own distinct child reference.
        append(E::WorkflowNodeStarted {
            instance: second.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: second.clone(),
            kind: WorkflowNodeKind::Agent,
            value: Some("{}".into()),
        });
        let subagent_2 = SubagentId::for_conversation(&conversation, 2);
        let child_agent_2 = AgentId::new("child-worker-2");
        append(E::SubagentOwnershipCommitted {
            parent_agent_id: AgentId::new("parent"),
            admitted_authority: None,
            subagent_id: subagent_2.clone(),
            child_agent_id: child_agent_2.clone(),
            child_conversation_id: crate::scripted_suites::common::identity::child_conversation_id(
                subagent_2.as_str(),
            ),
            origin: AgentActivationOrigin::Workflow {
                node_id: Box::new(second.clone()),
            },
            agent: "worker".into(),
            definition_digest: "sha256:def".into(),
            profile_digest: "sha256:prof".into(),
            ownership: crate::events::types::SubagentOwnershipKind::Workflow,
            workspace: directory.path().to_path_buf(),
        });
        append(E::WorkflowAgentAdmitted {
            workflow_id,
            run_id: second.block.run.clone(),
            node_id: second.clone(),
            subagent_id: subagent_2.clone(),
            profile: SubagentName::parse("worker").unwrap(),
        });
        append(E::WorkflowNodeSettled {
            instance: second.clone(),
            outcome: Outcome::Completed,
        });
        append(E::WorkflowNodeStarted {
            instance: non_agent.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: non_agent.clone(),
            kind: WorkflowNodeKind::Template,
            value: Some("{}".into()),
        });
        let after_all = owner.observation_read_cut().unwrap();
        drop(owner);

        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        let before = reopened
            .observation_workflow_nodes(&before_admission, start, 0, 32)
            .unwrap();
        assert_eq!(before.nodes.len(), 1);
        assert_eq!(before.nodes[0].child, None);
        assert_eq!(before.nodes[0].child_agent_id, None);

        let mid = reopened
            .observation_workflow_nodes(&after_ownership, start, 0, 32)
            .unwrap();
        assert_eq!(mid.nodes[0].child.as_ref(), Some(&subagent_1));
        assert_eq!(mid.nodes[0].child_agent_id.as_ref(), Some(&child_agent_1));

        let final_nodes = reopened
            .observation_workflow_nodes(&after_all, start, 0, 32)
            .unwrap();
        assert_eq!(final_nodes.nodes.len(), 3);
        assert_eq!(final_nodes.nodes[0].instance, first);
        assert_eq!(final_nodes.nodes[0].child.as_ref(), Some(&subagent_1));
        assert_eq!(
            final_nodes.nodes[0].child_agent_id.as_ref(),
            Some(&child_agent_1)
        );
        assert_eq!(final_nodes.nodes[1].instance, second);
        assert_eq!(final_nodes.nodes[1].child.as_ref(), Some(&subagent_2));
        assert_eq!(
            final_nodes.nodes[1].child_agent_id.as_ref(),
            Some(&child_agent_2)
        );
        assert_eq!(final_nodes.nodes[2].instance, non_agent);
        assert_eq!(final_nodes.nodes[2].child, None);
        assert_eq!(final_nodes.nodes[2].child_agent_id, None);
    }
}

#[cfg(test)]
mod workflow_tests {
    use crate::durable::sqlite::test_support::envelope;
    use crate::durable::{CanonicalStore, JournalStore, ObservationStore, SqliteConversationStore};
    use crate::events::RuntimeEvent;
    use crate::observation::trace::TraceCursor;
    use crate::runtime::identity::{AttemptId, ConversationId};
    use crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId;
    use crate::runtime::workflow::{WorkflowId, WorkflowRunId, WorkflowRunOrigin};
    use crate::runtime_client::trace::TraceProjection;

    #[test]
    fn indexed_workflow_location_preserves_old_long_ids_origins_cuts_and_reopen_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow-locate.sqlite");
        let conversation = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        let before = store.observation_read_cut().unwrap();
        let mut run = WorkflowRunId {
            conversation_id: conversation.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new("\"".repeat(256)),
            },
            invocation: 1,
        };
        let append = |run: &WorkflowRunId, ordinal| {
            store
                .append_event(envelope(
                    &conversation,
                    &format!("locate-run-{ordinal}"),
                    None,
                    RuntimeEvent::WorkflowStarted {
                        tool_call_id: None,
                        workflow_id: WorkflowId::parse("located").unwrap(),
                        run_id: run.clone(),
                    },
                ))
                .unwrap()
                .sequence
        };
        let tool_sequence = append(&run, 1);
        let tool = run.clone();
        run.origin = WorkflowRunOrigin::Scheduled {
            start_id: ScheduledWorkflowStartId::try_from(format!("swf_{}", "a".repeat(64)))
                .unwrap(),
        };
        let scheduled_sequence = append(&run, 2);
        let scheduled = run.clone();
        for ordinal in 3..=100 {
            run.invocation = ordinal;
            append(&run, ordinal);
        }
        assert!(serde_json::to_vec(&tool).unwrap().len() > 512);
        let current = store.observation_read_cut().unwrap();
        let canonical = store.load_canonical().unwrap();
        let journal = store.read_events(None, 256).unwrap();
        assert!(
            TraceProjection::at_cut(&store, &before)
                .unwrap()
                .locate_workflow(&tool)
                .unwrap()
                .is_none()
        );
        let projection = TraceProjection::at_cut(&store, &current).unwrap();
        for (id, sequence) in [(&tool, tool_sequence), (&scheduled, scheduled_sequence)] {
            let location = projection.locate_workflow(id).unwrap().unwrap();
            assert_eq!(location.record_id, TraceCursor::at(sequence).0);
            let root = location
                .page
                .records
                .iter()
                .find(|record| record.id == location.record_id)
                .unwrap();
            assert_eq!(root.kind, crate::observation::trace::TraceKind::Workflow);
            if id == &tool {
                assert!(root.native_id.is_none());
            }
            assert!(
                root.collections.iter().any(|frontier| frontier.domain
                    == format!("trace.records.{sequence}.workflow.nodes"))
            );
            assert!(projection.locate_workflow_at(id, &before).is_err());
        }
        let mut foreign = tool.clone();
        foreign.conversation_id = ConversationId::generate();
        assert!(projection.locate_workflow(&foreign).is_err());
        assert_eq!(store.load_canonical().unwrap(), canonical);
        assert_eq!(store.read_events(None, 256).unwrap().events, journal.events);
        drop(projection);
        drop(store);
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        assert_eq!(
            TraceProjection::new(&reopened)
                .unwrap()
                .locate_workflow(&tool)
                .unwrap()
                .unwrap()
                .record_id,
            TraceCursor::at(tool_sequence).0
        );
        assert_eq!(reopened.load_canonical().unwrap(), canonical);
        assert_eq!(
            reopened.read_events(None, 256).unwrap().events,
            journal.events
        );
    }

    #[test]
    fn workflow_started_only_exposes_exact_typed_run_and_workflow_identity_without_nodes() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let owner = SqliteConversationStore::open(
            conversation.clone(),
            &directory.path().join("start-only.sqlite"),
        )
        .unwrap();
        owner.initialize(&[]).unwrap();
        let mut run = crate::runtime::workflow::test_instance("start_only_flow", "start")
            .block
            .run;
        run.conversation_id = conversation.clone();
        run.origin = WorkflowRunOrigin::Tool {
            attempt_id: AttemptId::new("\"".repeat(256)),
        };
        run.invocation = 9;
        let start = owner
            .append_event(envelope(
                &conversation,
                "start-only-1",
                None,
                RuntimeEvent::WorkflowStarted {
                    tool_call_id: None,
                    workflow_id: WorkflowId::parse("start_only_flow").unwrap(),
                    run_id: run.clone(),
                },
            ))
            .unwrap()
            .sequence;
        let cut = owner.observation_read_cut().unwrap();
        let nodes = owner
            .observation_workflow_nodes(&cut, start, 0, 32)
            .unwrap();
        assert_eq!(nodes.run_id, run);
        assert_eq!(
            nodes.workflow_id,
            WorkflowId::parse("start_only_flow").unwrap()
        );
        assert_eq!(nodes.total, 0);
        assert!(nodes.nodes.is_empty());
        assert!(nodes.next_offset.is_none());
    }

    #[test]
    fn exact_visit_lookup_preserves_cut_omitted_and_cross_conversation_fences() {
        use crate::durable::observation::WorkflowNodeTraceState;
        use crate::runtime::workflow::WorkflowExecutionOutcome as Outcome;
        let directory = tempfile::tempdir().unwrap();
        let conversation = ConversationId::new("conv_19000000-0000-7000-8000-000000000777");
        let owner = SqliteConversationStore::open(
            conversation.clone(),
            &directory.path().join("exact-visit.sqlite"),
        )
        .unwrap();
        owner.initialize(&[]).unwrap();
        let mut instance = crate::runtime::workflow::test_instance("exact_visit", "worker");
        instance.block.run.conversation_id = conversation.clone();
        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            owner
                .append_event(envelope(
                    &conversation,
                    &format!("exact-visit-{ordinal}"),
                    None,
                    event,
                ))
                .unwrap()
                .sequence
        };
        append(RuntimeEvent::WorkflowStarted {
            tool_call_id: None,
            workflow_id: WorkflowId::parse("exact_visit").unwrap(),
            run_id: instance.block.run.clone(),
        });
        let before_node = owner.observation_read_cut().unwrap();
        append(RuntimeEvent::WorkflowNodeStarted {
            instance: instance.clone(),
        });
        let after_start = owner.observation_read_cut().unwrap();
        append(RuntimeEvent::WorkflowNodeInputRecorded {
            instance: instance.clone(),
            kind: crate::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: Some(r#"{"a":1}"#.into()),
        });
        append(RuntimeEvent::WorkflowNodeOutputRecorded {
            instance: instance.clone(),
            value: None,
        });
        let after_io = owner.observation_read_cut().unwrap();
        append(RuntimeEvent::WorkflowNodeSettled {
            instance: instance.clone(),
            outcome: Outcome::Completed,
        });
        let settled_cut = owner.observation_read_cut().unwrap();

        assert!(
            owner
                .observation_workflow_node(&before_node, &instance)
                .unwrap()
                .is_none()
        );
        let at_start = owner
            .observation_workflow_node(&after_start, &instance)
            .unwrap()
            .expect("node started at cut");
        assert_eq!(at_start.state, WorkflowNodeTraceState::Incomplete);
        assert!(at_start.input.is_none() && !at_start.input_omitted);
        assert!(at_start.output.is_none() && !at_start.output_omitted);

        let with_io = owner
            .observation_workflow_node(&after_io, &instance)
            .unwrap()
            .expect("node I/O at cut");
        assert_eq!(with_io.state, WorkflowNodeTraceState::Incomplete);
        assert!(with_io.input.is_some() && !with_io.input_omitted);
        assert!(with_io.output.is_none() && with_io.output_omitted);

        let finished = owner
            .observation_workflow_node(&settled_cut, &instance)
            .unwrap()
            .expect("settled node at cut");
        assert_eq!(
            finished.state,
            WorkflowNodeTraceState::Settled {
                outcome: Outcome::Completed
            }
        );

        let mut foreign = instance.clone();
        foreign.block.run.conversation_id = ConversationId::generate();
        assert!(matches!(
            owner.observation_workflow_node(&settled_cut, &foreign),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
        let mut too_long = instance;
        too_long.node = "n".repeat(super::INSTANCE_JSON_MAX + 1);
        assert!(matches!(
            owner.observation_workflow_node(&settled_cut, &too_long),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
    }
}
