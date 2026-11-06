//! Measure the production SQL's work, independently of machine timing.
use super::*;
use crate::durable::sqlite::test_support::{envelope, store};
use crate::durable::{CanonicalStore, ConversationStore, ObservationStore};
use crate::events::RuntimeEvent as E;
use crate::runtime::workflow::{
    WorkflowExecutionOutcome as Outcome, WorkflowId, WorkflowNodeInstance,
};
use rusqlite::StatementStatus;

fn append(connection: &Connection, sequence: &mut u64, instance: &WorkflowNodeInstance, event: E) {
    *sequence += 1;
    let mut event = envelope(
        &instance.block.run.conversation_id,
        &format!("query-work-{sequence}"),
        None,
        event,
    );
    event.sequence = *sequence;
    // Complete typed Journal envelopes in the real schema. These cost fixtures
    // bypass unrelated append projections; ordinary Store/cut behavior is tested
    // in the parent module with the native append API.
    connection.execute(
        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json) VALUES(?1,?2,?3,?4,?5,?6)",
        params![event.sequence,event.event_id.as_str(),event.schema_version,event.conversation_id.as_str(),event.timestamp.to_rfc3339(),serde_json::to_string(&event).unwrap()],
    ).unwrap();
}

fn start_run(connection: &Connection, sequence: &mut u64, instance: &WorkflowNodeInstance) {
    append(
        connection,
        sequence,
        instance,
        E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: WorkflowId::parse("query_work").unwrap(),
            run_id: instance.block.run.clone(),
        },
    );
}

fn visit(connection: &Connection, sequence: &mut u64, instance: &WorkflowNodeInstance) -> u64 {
    append(
        connection,
        sequence,
        instance,
        E::WorkflowNodeStarted {
            instance: instance.clone(),
        },
    );
    let start = *sequence;
    append(
        connection,
        sequence,
        instance,
        E::WorkflowNodeInputRecorded {
            instance: instance.clone(),
            kind: crate::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: Some(r#"{"args":{},"variables":{}}"#.into()),
        },
    );
    append(
        connection,
        sequence,
        instance,
        E::WorkflowNodeOutputRecorded {
            instance: instance.clone(),
            value: Some(r#"{"value":"rendered"}"#.into()),
        },
    );
    append(
        connection,
        sequence,
        instance,
        E::WorkflowNodeSettled {
            instance: instance.clone(),
            outcome: Outcome::Completed,
        },
    );
    start
}

fn work(connection: &Connection, sql: &str, values: &[rusqlite::types::Value], index: &str) -> i32 {
    let plan = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(rusqlite::params_from_iter(values), |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.replace("USING COVERING INDEX", "USING INDEX")
            .contains(&format!("SEARCH events USING INDEX {index}")),
        "{plan}"
    );
    assert!(
        (plan.contains("sequence>?") && plan.contains("sequence<?"))
            || (plan.contains("rowid>?") && plan.contains("rowid<?")),
        "{plan}"
    );
    assert!(
        !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
        "{plan}"
    );
    let mut statement = connection.prepare(sql).unwrap();
    let mut rows = statement.query(rusqlite::params_from_iter(values)).unwrap();
    while rows.next().unwrap().is_some() {}
    drop(rows);
    assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
    assert_eq!(statement.get_status(StatementStatus::Sort), 0);
    statement.get_status(StatementStatus::VmStep)
}

fn number(value: u64) -> rusqlite::types::Value {
    i64::try_from(value).unwrap().into()
}

fn node_work(connection: &Connection, start: u64, through: u64, instance: &str) -> i32 {
    let mut cost = work(
        connection,
        NODE_TERMINAL_SQL,
        &[number(start), number(through), instance.to_owned().into()],
        "events_workflow_node_instance_kind_idx",
    );
    for event in [
        "workflow_node_input_recorded",
        "workflow_node_output_recorded",
    ] {
        cost += work(
            connection,
            NODE_IO_SQL,
            &[
                event.to_owned().into(),
                number(start),
                number(through),
                instance.to_owned().into(),
            ],
            "events_workflow_node_instance_kind_idx",
        );
    }
    cost
}

#[test]
fn completed_visit_queries_have_constant_work_and_preserve_exact_cuts() {
    let owner = store();
    owner.initialize(&[]).unwrap();
    let mut cut = owner.observation_read_cut().unwrap();
    let connection = owner.lock().unwrap();
    connection.execute_batch("BEGIN").unwrap();
    let mut instance = crate::runtime::workflow::test_instance("query_work", "start");
    instance.block.run.conversation_id = owner.conversation_id().clone();
    let mut sequence = 0;
    start_run(&connection, &mut sequence, &instance);
    let mut visits = Vec::new();
    // A legal 42-static-node graph: root Start/Loop/Return and 100 child
    // iterations of Start + 38 Templates = 3,903 visits / 4,003 steps.
    for root in ["start", "loop", "return"] {
        instance.node = root.into();
        visits.push((
            visit(&connection, &mut sequence, &instance),
            instance.clone(),
        ));
    }
    instance.block.definition.blocks = vec!["loop".into(), "body".into()];
    for iteration in 1..=100 {
        instance.block.invocations = vec![0, iteration];
        for ordinal in 0..39 {
            instance.node = format!("child_{ordinal}");
            visits.push((
                visit(&connection, &mut sequence, &instance),
                instance.clone(),
            ));
        }
    }
    connection.execute_batch("COMMIT").unwrap();
    assert_eq!(visits.len(), 3903);
    cut.read.journal = sequence;
    let mut total_work = 0;
    let expected_work = node_work(
        &connection,
        visits[0].0,
        sequence,
        &serde_json::to_string(&visits[0].1).unwrap(),
    );
    assert!(
        expected_work < 200,
        "one visit used {expected_work} VM steps"
    );
    for (offset, (start, instance)) in visits.iter().enumerate() {
        let identity = serde_json::to_string(instance).unwrap();
        let cost = node_work(&connection, *start, sequence, &identity);
        assert_eq!(cost, expected_work, "visit {offset}");
        total_work += cost;
        if [31, 127, 1023, 3902].contains(&offset) {
            assert_eq!(
                total_work,
                (i32::try_from(offset).unwrap() + 1) * expected_work
            );
        }
    }
    for offset in (0..visits.len()).step_by(32) {
        let page = page(&connection, &cut, 1, offset as u64, 32).unwrap();
        assert_eq!(page.total, 3903);
        assert_eq!(
            page.next_offset,
            (offset + page.nodes.len() < visits.len())
                .then_some((offset + page.nodes.len()) as u64)
        );
        for (node, (_, expected)) in page.nodes.iter().zip(&visits[offset..]) {
            assert_eq!(&node.instance, expected);
            assert_eq!(
                node.state,
                WorkflowNodeTraceState::Settled {
                    outcome: Outcome::Completed
                }
            );
            assert!(node.input.is_some() && node.output.is_some());
            assert_eq!(
                node.output_preview.as_deref(),
                Some(r#"{"value":"rendered"}"#)
            );
        }
    }
    cut.read.journal = visits[0].0;
    let old = page(&connection, &cut, 1, 0, 32).unwrap();
    assert_eq!(old.total, 1);
    assert_eq!(old.nodes[0].state, WorkflowNodeTraceState::Incomplete);
    assert!(old.nodes[0].input.is_none() && old.nodes[0].output.is_none());
}

#[test]
fn absent_visit_facts_and_run_headers_ignore_unrelated_later_history() {
    let owner = store();
    owner.initialize(&[]).unwrap();
    let mut cut = owner.observation_read_cut().unwrap();
    let connection = owner.lock().unwrap();
    connection.execute_batch("BEGIN").unwrap();
    let mut instance = crate::runtime::workflow::test_instance("query_work", "incomplete");
    instance.block.run.conversation_id = owner.conversation_id().clone();
    let identity = serde_json::to_string(&instance).unwrap();
    let run = serde_json::to_string(&instance.block.run).unwrap();
    let mut sequence = 0;
    start_run(&connection, &mut sequence, &instance);
    append(
        &connection,
        &mut sequence,
        &instance,
        E::WorkflowNodeStarted {
            instance: instance.clone(),
        },
    );
    let mut baseline = None;
    for later in 0..=4096 {
        if [0, 128, 1024, 4096].contains(&later) {
            cut.read.journal = sequence;
            let mut costs = vec![node_work(&connection, 2, sequence, &identity)];
            for sql in [RUN_TOTAL_SQL, RUN_REVISION_SQL, RUN_NODES_SQL] {
                let mut values = vec![number(1), number(sequence), run.clone().into()];
                if sql == RUN_NODES_SQL {
                    values.extend([number(32), number(0)]);
                }
                costs.push(work(
                    &connection,
                    sql,
                    &values,
                    "events_workflow_node_run_kind_idx",
                ));
            }
            assert!(costs.iter().all(|cost| *cost < 200), "{costs:?}");
            // An empty tail can terminate at EOF; a populated tail adds one
            // instruction to reject its first different index key. Growing
            // that tail must add no further work.
            for (initial, current) in baseline.get_or_insert(costs.clone()).iter().zip(&costs) {
                assert!(
                    (*current - *initial).abs() <= 1,
                    "{later} later runs: {costs:?}"
                );
            }
            let page = page(&connection, &cut, 1, 0, 32).unwrap();
            assert_eq!(page.total, 1);
            assert_eq!(page.revision, 2);
            assert_eq!(page.nodes[0].state, WorkflowNodeTraceState::Incomplete);
            assert!(page.nodes[0].input.is_none() && page.nodes[0].output.is_none());
        }
        if later < 4096 {
            instance.block.run.invocation += 1;
            start_run(&connection, &mut sequence, &instance);
            visit(&connection, &mut sequence, &instance);
        }
    }
    connection.execute_batch("COMMIT").unwrap();
}
