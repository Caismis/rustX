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
const RUN_TOTAL_SQL: &str = "SELECT count(*) FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE json_extract(event_json,'$.event.instance.block.run')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_started' AND sequence>?1 AND sequence<=?2";
const RUN_REVISION_SQL: &str = "SELECT max(sequence) FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE json_extract(event_json,'$.event.instance.block.run')=?3 AND json_extract(event_json,'$.event.type') IN ('workflow_node_started','workflow_node_waiting','workflow_node_settled') AND sequence>?1 AND sequence<=?2";
const RUN_NODES_SQL: &str = "SELECT sequence,event_timestamp,CASE WHEN octet_length(json_extract(event_json,'$.event.instance'))<=8192 THEN json_extract(event_json,'$.event.instance') END FROM events INDEXED BY events_workflow_node_run_kind_idx WHERE json_extract(event_json,'$.event.instance.block.run')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_started' AND sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT ?4 OFFSET ?5";
const NODE_TERMINAL_SQL: &str = "SELECT event_timestamp,json_extract(event_json,'$.event.outcome') FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE json_extract(event_json,'$.event.instance')=?3 AND json_extract(event_json,'$.event.type')='workflow_node_settled' AND sequence>?1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1";
const NODE_IO_SQL: &str = "SELECT sequence,json_extract(event_json,'$.event.kind'),substr(json_extract(event_json,'$.event.value'),1,192),CASE WHEN json_type(event_json,'$.event.value')='text' THEN octet_length(json_quote(json_extract(event_json,'$.event.value'))) END FROM events INDEXED BY events_workflow_node_instance_kind_idx WHERE json_extract(event_json,'$.event.instance')=?4 AND json_extract(event_json,'$.event.type')=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT 1";

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
    let (workflow_id,call_id):(String,Option<String>)=connection.query_row("SELECT json_extract(event_json,'$.event.workflow_id'),json_extract(event_json,'$.event.tool_call_id') FROM events WHERE sequence=?1",[start],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
    let run_id: crate::runtime::workflow::WorkflowRunId =
        serde_json::from_str(&run).map_err(|error| storage(error.to_string()))?;
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
    let end: Option<u64> = connection.query_row(
        "SELECT min(sequence) FROM events INDEXED BY events_run_id_kind_idx WHERE json_extract(event_json,'$.event.run_id')=?1 AND json_extract(event_json,'$.event.type') IN ('workflow_completed','workflow_failed','workflow_cancelled') AND sequence>?2 AND sequence<=?3",
        params![run,start,cut.read.journal], |row| row.get(0)).map_err(|error|storage(error.to_string()))?;
    let through = end.unwrap_or(cut.read.journal);
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
    Ok(WorkflowTraceNode {
        id: format!("workflow-node:{sequence}"),
        instance: serde_json::from_str(&instance).map_err(|error| storage(error.to_string()))?,
        kind: input
            .as_ref()
            .and_then(|io| io.kind.clone())
            .map(|kind| serde_json::from_value(serde_json::Value::String(kind)))
            .transpose()
            .map_err(|error| storage(error.to_string()))?,
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
    })
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
}
