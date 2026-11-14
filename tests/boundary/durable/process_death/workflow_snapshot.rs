//! Real OS `SIGKILL` process-death conformance for workflow start & snapshot
//! publication, rollback before commit, and mid-run interrupted reopen using the
//! canonical external `ProviderEmulator`.
use rusqlite::{Connection, OpenFlags};

use crate::boundary_suites::common::provider_emulator::ProviderEmulator;
use crate::durable::observation::WorkflowNodeTraceState;
use crate::events::types::RuntimeEvent;
use crate::runtime::workflow::WorkflowRunId;
use crate::runtime::workflow::snapshot::{compute_snapshot_digest, sha256_hex};

use super::child;
use super::harness::{Lab, write_emulator_workflow_config};

const CRASH_WORKFLOW_YAML: &str = r"description: Review the request with a native child agent.
block:
  input:
    type: object
    properties:
      task:
        type: string
    required:
      - task
    additionalProperties: false
  nodes:
    workflow_start:
      type: start
    review:
      type: agent
      profile: reviewer
      task:
        - type: literal
          value: 'Review the request and commit the result.'
        - type: reference
          path:
            - args
            - task
      output:
        type: json_schema
        schema:
          type: object
          properties:
            passed:
              type: boolean
            summary:
              type: string
          required:
            - passed
            - summary
          additionalProperties: false
    decision:
      type: branch
      cases:
        - id: 'true'
          condition:
            type: boolean
            value:
              type: reference
              path:
                - review
                - passed
    success:
      type: return
      outputs:
        - variable: summary
          value_selector: [review, summary]
    failure:
      type: return
      outputs:
        - variable: summary
          value_selector: [args, task]
  edges:
    - from: workflow_start
      to: review
    - from: review
      to: decision
    - from: decision
      to: success
      port: true
    - from: decision
      to: failure
      port: false
editor:
  nodes:
    - address: { block: [], node: workflow_start }
      position: { x: 120.0, y: 80.0 }
      title: Crash Start
    - address: { block: [], node: review }
      position: { x: 360.0, y: 80.0 }
      title: Review Agent Node
";

fn snapshot_table_counts(database: &std::path::Path) -> (i64, i64, i64) {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open dead child sqlite read-only");
    let run_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM workflow_run_snapshots", [], |row| {
            row.get(0)
        })
        .unwrap();
    let def_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM workflow_definition_snapshots",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let chunk_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM workflow_snapshot_chunks", [], |row| {
            row.get(0)
        })
        .unwrap();
    (run_rows, def_rows, chunk_rows)
}

fn started_run(durable: &super::harness::Durable) -> (u64, WorkflowRunId) {
    durable
        .journal()
        .into_iter()
        .find_map(|envelope| match envelope.event {
            RuntimeEvent::WorkflowStarted { run_id, .. } => Some((envelope.sequence, run_id)),
            _ => None,
        })
        .expect("committed WorkflowStarted event")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_start_crash_before_snapshot_rolls_back_all_rows() {
    let Some(emulator) = ProviderEmulator::start("workflow_output").await else {
        return;
    };
    let lab = Lab::new();
    write_emulator_workflow_config(
        &lab,
        &emulator.openai_base_url(),
        "review_pr",
        CRASH_WORKFLOW_YAML,
    );
    let mut process = lab.spawn(
        child::WORKFLOW_RUN_START,
        Some("after:workflow_start_event_before_snapshot"),
    );
    process.wait_reached("after:workflow_start_event_before_snapshot");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::WorkflowStarted { .. })),
        0,
        "SIGKILL before snapshot publication must roll back WorkflowStarted with its transaction"
    );
    assert_eq!(
        snapshot_table_counts(&lab.database()),
        (0, 0, 0),
        "precommit SIGKILL must leave zero workflow snapshot run/definition/chunk rows"
    );

    let requests_before_reopen = emulator.requests().await.len();
    assert_eq!(requests_before_reopen, 1);

    let report = durable.recover();
    assert!(report.subagent_classes().is_empty());
    assert!(report.reconciliation().subagent_terminals.is_empty());

    let journal_before_inspect = durable.journal().len();
    let canonical_before_inspect = durable.canonical().len();
    assert_eq!(snapshot_table_counts(&lab.database()), (0, 0, 0));
    assert_eq!(durable.journal().len(), journal_before_inspect);
    assert_eq!(durable.canonical().len(), canonical_before_inspect);

    let mut reopened = lab.spawn(child::WORKFLOW_RUN_REOPEN, None);
    let note = reopened.wait_note_prefixed("workflow-reopen:");
    assert_eq!(note, "workflow-reopen:live_runs=0");
    reopened.sigkill();

    let journal_after_reopen = durable.journal().len();
    let canonical_after_reopen = durable.canonical().len();
    assert_eq!(emulator.requests().await.len(), requests_before_reopen);
    assert_eq!(snapshot_table_counts(&lab.database()), (0, 0, 0));
    let _cut_after = durable.store().observation_read_cut().unwrap();
    assert_eq!(durable.journal().len(), journal_after_reopen);
    assert_eq!(durable.canonical().len(), canonical_after_reopen);
    emulator.abort().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_start_crash_after_commit_retains_complete_valid_binding() {
    let Some(emulator) = ProviderEmulator::start("workflow_output").await else {
        return;
    };
    let lab = Lab::new();
    write_emulator_workflow_config(
        &lab,
        &emulator.openai_base_url(),
        "review_pr",
        CRASH_WORKFLOW_YAML,
    );
    let mut process = lab.spawn(child::WORKFLOW_RUN_START, Some("after:workflow_start"));
    process.wait_reached("after:workflow_start");
    process.sigkill();

    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::WorkflowStarted { .. })),
        1
    );
    let (start_seq, run_id) = started_run(&durable);
    assert_eq!(snapshot_table_counts(&lab.database()), (1, 1, 1));
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::WorkflowNodeStarted { .. })),
        0,
        "SIGKILL immediately after WorkflowStarted commit leaves empty node collection"
    );

    let journal_before = durable.journal().len();
    let canonical_before = durable.canonical().len();
    let requests_before_reopen = emulator.requests().await.len();
    let cut = durable.store().observation_read_cut().unwrap();
    let trace_nodes = durable
        .store()
        .observation_workflow_nodes(&cut, start_seq, 0, 32)
        .unwrap();
    assert_eq!(trace_nodes.run_id, run_id);
    assert_eq!(trace_nodes.total, 0);
    assert!(trace_nodes.nodes.is_empty());
    let header = durable
        .store()
        .observation_workflow_run_snapshot_header(&cut, &run_id)
        .unwrap()
        .expect("committed run snapshot header survives SIGKILL after commit");
    let (doc_header, document) = durable
        .store()
        .observation_workflow_run_snapshot_document(&cut, &run_id)
        .unwrap()
        .expect("committed run snapshot document survives SIGKILL after commit");
    assert_eq!(header, doc_header);
    assert!(header.has_source_presentation);
    let payload_sha256 = sha256_hex(&serde_json::to_vec(&document).unwrap());
    assert_eq!(
        compute_snapshot_digest(
            &header.workflow_id,
            &header.program_digest,
            &header.source_revision,
            header.has_source_presentation,
            &payload_sha256,
        ),
        header.snapshot_digest
    );
    let editor = document.editor.as_ref().expect("retained editor metadata");
    assert!(
        editor.nodes.iter().any(|node| {
            node.title.as_deref() == Some("Crash Start")
                && node
                    .position
                    .is_some_and(|position| position.x.to_bits() == 120.0_f64.to_bits())
        }),
        "retained snapshot must preserve exact admitted editor layout"
    );

    // Inspection and reopen never append canonical history or replay model/tool work.
    assert_eq!(durable.journal().len(), journal_before);
    assert_eq!(durable.canonical().len(), canonical_before);

    let mut reopened = lab.spawn(child::WORKFLOW_RUN_REOPEN, None);
    assert_eq!(
        reopened.wait_note_prefixed("workflow-reopen:"),
        "workflow-reopen:live_runs=0"
    );
    reopened.sigkill();

    assert_eq!(emulator.requests().await.len(), requests_before_reopen);
    let journal_after_reopen = durable.journal().len();
    let canonical_after_reopen = durable.canonical().len();
    let cut_after = durable.store().observation_read_cut().unwrap();
    let reopened_header = durable
        .store()
        .observation_workflow_run_snapshot_header(&cut_after, &run_id)
        .unwrap()
        .expect("snapshot stays readable after process reopen");
    assert_eq!(reopened_header, header);
    assert_eq!(durable.journal().len(), journal_after_reopen);
    assert_eq!(durable.canonical().len(), canonical_after_reopen);
    emulator.abort().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_interrupted_node_and_held_child_retains_visit_and_snapshot_without_phantom_live_owner()
 {
    let Some(emulator) = ProviderEmulator::start("workflow_output").await else {
        return;
    };
    let lab = Lab::new();
    write_emulator_workflow_config(
        &lab,
        &emulator.openai_base_url(),
        "review_pr",
        CRASH_WORKFLOW_YAML,
    );
    let mut process = lab.spawn(child::WORKFLOW_RUN_START, None);
    // Block until the external provider emulator receives the admitted child agent request
    // and holds the child turn mid-stream before emitting `workflow_output`.
    emulator.await_gate("workflow-child-admitted").await;
    process.sigkill();

    let durable = lab.durable();
    let (start_seq, run_id) = started_run(&durable);
    assert!(
        durable.count_events(|event| matches!(event, RuntimeEvent::WorkflowNodeStarted { .. }))
            >= 2,
        "at least workflow_start and review visits committed before crash"
    );
    assert_eq!(
        durable.count_events(|event| matches!(event, RuntimeEvent::WorkflowAgentAdmitted { .. })),
        1
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentOwnershipCommitted { .. })),
        1
    );
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::WorkflowCompleted { .. } | RuntimeEvent::WorkflowFailed { .. }
        )),
        0
    );
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::SubagentTerminalPublished { .. }
                | RuntimeEvent::SubagentTerminalSettled { .. }
        )),
        0
    );

    let canonical_before = durable.canonical().len();
    let requests_before_reopen = emulator.requests().await.len();
    assert_eq!(requests_before_reopen, 2);

    let cut_before = durable.store().observation_read_cut().unwrap();
    let header_before = durable
        .store()
        .observation_workflow_run_snapshot_header(&cut_before, &run_id)
        .unwrap()
        .expect("retained snapshot header for interrupted workflow");
    let (_, doc_before) = durable
        .store()
        .observation_workflow_run_snapshot_document(&cut_before, &run_id)
        .unwrap()
        .expect("retained snapshot document for interrupted workflow");
    assert_eq!(
        doc_before.editor.as_ref().unwrap().nodes.len(),
        2,
        "retained editor presentation preserves all admitted node positions"
    );

    let nodes_before = durable
        .store()
        .observation_workflow_nodes(&cut_before, start_seq, 0, 32)
        .unwrap();
    assert!(
        nodes_before
            .nodes
            .iter()
            .any(|node| node.instance.node.as_str() == "review"
                && node.state == WorkflowNodeTraceState::Incomplete),
        "cold interrupted review visit must report Incomplete, never phantom Running/Waiting: {:?}",
        nodes_before.nodes
    );
    assert!(
        nodes_before.nodes.iter().all(|node| !matches!(
            node.state,
            WorkflowNodeTraceState::Running
                | WorkflowNodeTraceState::Waiting { .. }
                | WorkflowNodeTraceState::Pending
                | WorkflowNodeTraceState::Draining
        )),
        "reopened trace must never fabricate active/live node states: {:?}",
        nodes_before.nodes
    );
    assert_eq!(durable.canonical().len(), canonical_before);

    // Real reopened runtime process settles the dead held child cleanly without inventing a live workflow owner or dispatching model/tool work.
    let mut reopened = lab.spawn(child::WORKFLOW_RUN_REOPEN, None);
    assert_eq!(
        reopened.wait_note_prefixed("workflow-reopen:"),
        "workflow-reopen:live_runs=0"
    );
    reopened.sigkill();

    assert_eq!(emulator.requests().await.len(), requests_before_reopen);
    assert_eq!(
        durable.count_events(|event| matches!(
            event,
            RuntimeEvent::SubagentTerminalSettled {
                state: crate::events::types::SubagentTerminalState::Interrupted,
                ..
            }
        )),
        1,
        "startup recovery settles held dead workflow child once"
    );
    assert_eq!(
        durable
            .count_events(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. })),
        0,
        "workflow child settlement closes native ownership directly without parent inbound notice"
    );
    let journal_after_reopen = durable.journal().len();
    let canonical_after_reopen = durable.canonical().len();

    let cut_after = durable.store().observation_read_cut().unwrap();
    let header_after = durable
        .store()
        .observation_workflow_run_snapshot_header(&cut_after, &run_id)
        .unwrap()
        .expect("retained snapshot stays readable after reopen");
    assert_eq!(header_after, header_before);
    let nodes_after = durable
        .store()
        .observation_workflow_nodes(&cut_after, start_seq, 0, 32)
        .unwrap();
    assert!(
        nodes_after
            .nodes
            .iter()
            .any(|node| node.instance.node.as_str() == "review"
                && node.state == WorkflowNodeTraceState::Incomplete),
        "review visit remains retained and non-live after reopen: {:?}",
        nodes_after.nodes
    );
    assert_eq!(durable.journal().len(), journal_after_reopen);
    assert_eq!(durable.canonical().len(), canonical_after_reopen);
    emulator.abort().await;
}
