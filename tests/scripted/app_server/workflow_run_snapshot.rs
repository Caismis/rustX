//! End-to-end native `AppServer` and cold History snapshot contracts (Issue #552):
//! live admission binding, layout-only disk edits preserving older run snapshots,
//! cut boundaries, cross-conversation refusal, chunk streaming/bounds, cancelled run
//! persistence, cold session reads without runtime composition, and deletion cleanup.
#![allow(clippy::too_many_lines, clippy::large_futures)]

use std::sync::Mutex;

use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest as _, Sha256};

use super::protocol::child_delivery::ChildDelivery;
use super::protocol::{attach, call, initialize, wire_text};
use super::*;
use crate::app_server::connection::AppServerConnection;
use crate::app_server::protocol::*;
use crate::app_server::session_deletion::SessionDeletionResult;
use crate::local_runtime::session::{LineageSide, SessionNodeId};
use crate::runtime::identity::{AgentId, AttemptId, SubagentId};
use crate::runtime::types::RuntimeClock;
use crate::runtime::workflow::scheduled::lifecycle::*;
use crate::runtime::workflow::{
    WORKFLOW_SNAPSHOT_CHUNK_BYTES, WorkflowId, WorkflowRunId, WorkflowRunOrigin,
    WorkflowRunSnapshotHeader,
};
use crate::runtime_client::agent_observation::AgentObservation;
use crate::scheduler::*;

#[derive(Debug)]
struct Clock(Mutex<DateTime<Utc>>);

impl Clock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            DateTime::parse_from_rfc3339("2026-10-23T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )))
    }

    fn add(&self, seconds: i64) {
        *self.0.lock().unwrap() += Duration::seconds(seconds);
    }
}

impl RuntimeClock for Clock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

async fn schedule_create_workflow(f: &Fixture, key: &str) -> ScheduleCommandReceipt {
    let result = f
        .manager
        .schedule_command(
            &f.sessions[0].id,
            &f.sessions[0].active_node,
            ScheduleCommand {
                command_id: key.into(),
                mutation: ScheduleMutation::CreateWorkflow {
                    title: format!("Run {key}"),
                    workflow_id: WorkflowId::parse("scheduled").unwrap(),
                    arguments: serde_json::json!({}),
                    timing: ScheduleTiming::After { seconds: 60 },
                },
            },
        )
        .await
        .unwrap();
    let ScheduleCommandResult::Applied { receipt } = result else {
        panic!("expected schedule command receipt, got {result:?}");
    };
    receipt
}

async fn wait_scheduled_settled(
    f: &Fixture,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    id: &ScheduledWorkflowStartId,
) -> ScheduledWorkflowRecovery {
    loop {
        let done = runtime.settlement_signal().notified();
        tokio::pin!(done);
        done.as_mut().enable();
        let view = f
            .manager
            .scheduled_workflow_recovery(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                id.clone(),
                0,
                100,
            )
            .await
            .unwrap()
            .unwrap();
        if matches!(view.start.state, ScheduledWorkflowState::Settled { .. }) {
            return view;
        }
        done.await;
    }
}

async fn read_live_snapshot_json(
    connection: &AppServerConnection,
    next_id: &mut i64,
    target: &AttachmentTarget,
    cut: Option<crate::observation::cut::CheckpointReadCut>,
    run_id: &WorkflowRunId,
    chunk_limit: usize,
) -> (Option<WorkflowRunSnapshotHeader>, Option<serde_json::Value>) {
    let id = *next_id;
    *next_id += 1;
    let MethodResult::WorkflowRunSnapshotHeader { header } = call(
        connection,
        id,
        Method::ConversationWorkflowRunSnapshotHeader {
            target: target.clone(),
            agent_id: None,
            cut: cut.clone(),
            run_id: run_id.clone(),
        },
    )
    .await
    else {
        panic!("expected WorkflowRunSnapshotHeader result");
    };
    let Some(header) = header else {
        return (None, None);
    };

    let mut bytes = Vec::new();
    let mut offset = 0_u64;
    loop {
        let cid = *next_id;
        *next_id += 1;
        let MethodResult::WorkflowRunSnapshotChunk { chunk } = call(
            connection,
            cid,
            Method::ConversationWorkflowRunSnapshotChunk {
                target: target.clone(),
                agent_id: None,
                cut: cut.clone(),
                run_id: run_id.clone(),
                offset,
                limit: chunk_limit,
            },
        )
        .await
        else {
            panic!("expected WorkflowRunSnapshotChunk result");
        };
        let chunk = chunk.expect("chunk present when header exists");
        let part = base64::engine::general_purpose::STANDARD
            .decode(&chunk.data)
            .expect("valid base64");
        assert!(part.len() <= chunk_limit);
        bytes.extend_from_slice(&part);
        match chunk.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(bytes.len(), usize::try_from(header.byte_length).unwrap());
    assert_eq!(sha256_hex(&bytes), header.sha256);
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("snapshot wire JSON");
    (Some(header), Some(doc))
}

async fn read_cold_snapshot_json(
    connection: &AppServerConnection,
    next_id: &mut i64,
    session_id: &SessionId,
    cut: &crate::observation::cut::CheckpointReadCut,
    run_id: &WorkflowRunId,
) -> (Option<WorkflowRunSnapshotHeader>, Option<serde_json::Value>) {
    let id = *next_id;
    *next_id += 1;
    let MethodResult::WorkflowRunSnapshotHeader { header } = call(
        connection,
        id,
        Method::SessionWorkflowRunSnapshotHeader {
            session_id: session_id.clone(),
            node_id: None,
            cut: cut.clone(),
            run_id: run_id.clone(),
        },
    )
    .await
    else {
        panic!("expected WorkflowRunSnapshotHeader result");
    };
    let Some(header) = header else {
        return (None, None);
    };

    let mut bytes = Vec::new();
    let mut offset = 0_u64;
    loop {
        let cid = *next_id;
        *next_id += 1;
        let MethodResult::WorkflowRunSnapshotChunk { chunk } = call(
            connection,
            cid,
            Method::SessionWorkflowRunSnapshotChunk {
                session_id: session_id.clone(),
                node_id: None,
                cut: cut.clone(),
                run_id: run_id.clone(),
                offset,
                limit: 16_384,
            },
        )
        .await
        else {
            panic!("expected WorkflowRunSnapshotChunk result");
        };
        let chunk = chunk.expect("chunk present");
        let part = base64::engine::general_purpose::STANDARD
            .decode(&chunk.data)
            .expect("valid base64");
        bytes.extend_from_slice(&part);
        match chunk.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(bytes.len(), usize::try_from(header.byte_length).unwrap());
    assert_eq!(sha256_hex(&bytes), header.sha256);
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("snapshot wire JSON");
    (Some(header), Some(doc))
}

fn workflow_definition_with_editor(title: &str, x: f64, y: f64) -> serde_json::Value {
    serde_json::json!({
        "description": "Native scheduled interaction",
        "block": {
            "input": {
                "type": "object",
                "properties": {
                    "answer": { "type": "string", "default": "native durable return" }
                },
                "additionalProperties": false
            },
            "nodes": {
                "workflow_start": { "type": "start" },
                "done": {
                    "type": "return",
                    "outputs": [{ "variable": "answer", "value_selector": ["args", "answer"] }]
                }
            },
            "edges": [{ "from": "workflow_start", "to": "done" }]
        },
        "editor": {
            "nodes": [{
                "address": { "block": [], "node": "workflow_start" },
                "position": { "x": x, "y": y },
                "title": title
            }]
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue552_live_and_cold_workflow_run_snapshots_preserve_admitted_revisions_and_delete() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 2).await;
        f.install_scheduled_workflow_definition(&workflow_definition_with_editor(
            "Rev 1 Scheduled",
            40.0,
            60.0,
        ));
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();

        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let mut next_id = 100_i64;

        // Run #1 admitted under Revision 1.
        let conv0 = f.id(0).await;
        let resident1 = f.load(0).await.unwrap().unwrap();
        let runtime1 = resident1.inspect_runtime().unwrap();
        let created1 = schedule_create_workflow(&f, "wf-rev-1").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start: start1,
            ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created1.id,
                    expected_revision: created1.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected WorkflowAdmitted for run 1");
        };
        let settled1 = wait_scheduled_settled(&f, &runtime1, &start1.id).await;
        assert!(matches!(
            settled1.start.state,
            ScheduledWorkflowState::Settled {
                outcome: ScheduledWorkflowOutcome::Completed,
                ..
            }
        ));

        let target0 = attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_after_run1,
        } = call(
            &connection,
            next_id,
            Method::SessionCheckpoint {
                target: target0.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("expected checkpoint");
        };
        next_id += 1;

        let (hdr1_live, doc1_live) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0,
            Some(checkpoint_after_run1.read_cut.clone()),
            &start1.run_id,
            256,
        )
        .await;
        let hdr1 = hdr1_live.expect("run 1 header live");
        let doc1 = doc1_live.expect("run 1 doc live");
        assert!(hdr1.has_source_presentation);
        assert_eq!(doc1["editor"]["nodes"][0]["title"], "Rev 1 Scheduled");
        assert_eq!(doc1["editor"]["nodes"][0]["position"]["x"], 40.0);

        // Detach connection route & unload session 0, apply layout-only edit on disk (Revision 2), reload & admit Run #2.
        call(
            &connection,
            next_id,
            Method::SessionDetach {
                target: target0.clone(),
            },
        )
        .await;
        next_id += 1;
        drop(resident1);
        drop(runtime1);
        f.manager.unload(&conv0).await.unwrap();
        std::fs::write(
            f.workspaces[0].join(".agents/workflows/scheduled.yaml"),
            serde_json::to_vec_pretty(&workflow_definition_with_editor(
                "Rev 2 Layout Only",
                800.0,
                240.0,
            ))
            .unwrap(),
        )
        .unwrap();

        let resident2 = f.load(0).await.unwrap().unwrap();
        f.manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        super::configuration::applied(&f, 0).await;
        let runtime2 = resident2.inspect_runtime().unwrap();
        let target0_reloaded = attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_before_run2,
        } = call(
            &connection,
            next_id,
            Method::SessionCheckpoint {
                target: target0_reloaded.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("expected pre-run2 checkpoint");
        };
        next_id += 1;

        let created2 = schedule_create_workflow(&f, "wf-rev-2").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start: start2,
            ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created2.id,
                    expected_revision: created2.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected WorkflowAdmitted for run 2");
        };
        let _ = wait_scheduled_settled(&f, &runtime2, &start2.id).await;

        // Run #1 still returns Revision 1; Run #2 returns Revision 2; older cut hides Run #2.
        let (hdr1_again, doc1_again) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0_reloaded,
            None,
            &start1.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        assert_eq!(hdr1_again.unwrap(), hdr1);
        assert_eq!(
            doc1_again.unwrap()["editor"]["nodes"][0]["title"],
            "Rev 1 Scheduled"
        );

        let (hdr2_live, doc2_live) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0_reloaded,
            None,
            &start2.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        let hdr2 = hdr2_live.expect("run 2 header");
        let doc2 = doc2_live.expect("run 2 doc");
        assert_eq!(hdr2.program_digest, hdr1.program_digest);
        assert_ne!(hdr2.source_revision, hdr1.source_revision);
        assert_ne!(hdr2.snapshot_digest, hdr1.snapshot_digest);
        assert_eq!(doc2["editor"]["nodes"][0]["title"], "Rev 2 Layout Only");

        // Pre-run2 cut hides Run #2 cleanly (header: null); expired cut across surface_revision fails with ReadCutExpired.
        let (before_cut_hdr, _) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0_reloaded,
            Some(checkpoint_before_run2.read_cut.clone()),
            &start2.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        assert!(before_cut_hdr.is_none());

        let expired_cut_resp = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(next_id),
                call: Method::ConversationWorkflowRunSnapshotHeader {
                    target: target0_reloaded.clone(),
                    agent_id: None,
                    cut: Some(checkpoint_after_run1.read_cut.clone()),
                    run_id: start2.run_id.clone(),
                },
            })
            .await;
        next_id += 1;
        assert!(matches!(expired_cut_resp, Response::Failure(_)));

        // Missing run_id returns null cleanly without synthesizing or falling back to YAML on disk.
        let missing_run = WorkflowRunId {
            conversation_id: conv0.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new("missing-attempt"),
            },
            invocation: 0,
        };
        let (missing_hdr, _) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0_reloaded,
            None,
            &missing_run,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        assert!(missing_hdr.is_none());

        // Cross-conversation read must fail cleanly and never leak snapshot data.
        let target1 = attach(&connection, &f, 1).await;
        let cross_err = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(next_id),
                call: Method::ConversationWorkflowRunSnapshotHeader {
                    target: target1.clone(),
                    agent_id: None,
                    cut: None,
                    run_id: start1.run_id.clone(),
                },
            })
            .await;
        next_id += 1;
        assert!(matches!(cross_err, Response::Failure(_)));

        // Detach live routes & unload session 0 completely so cold session/* history reads do NOT load runtime.
        call(
            &connection,
            next_id,
            Method::SessionDetach {
                target: target0_reloaded,
            },
        )
        .await;
        next_id += 1;
        call(
            &connection,
            next_id,
            Method::SessionDetach { target: target1 },
        )
        .await;
        next_id += 1;
        drop(resident2);
        drop(runtime2);
        f.manager.unload(&conv0).await.unwrap();

        let MethodResult::SessionTraceHistory {
            page: hist_page, ..
        } = call(
            &connection,
            next_id,
            Method::SessionTraceHistory {
                session_id: f.sessions[0].id.clone(),
                node_id: None,
                before: None,
                limit: 32,
            },
        )
        .await
        else {
            panic!("expected SessionTraceHistory");
        };
        next_id += 1;
        let hist_cut = hist_page.read_cut.expect("cold history read cut");

        let (hdr1_cold, doc1_cold) = read_cold_snapshot_json(
            &connection,
            &mut next_id,
            &f.sessions[0].id,
            &hist_cut,
            &start1.run_id,
        )
        .await;
        let (hdr2_cold, doc2_cold) = read_cold_snapshot_json(
            &connection,
            &mut next_id,
            &f.sessions[0].id,
            &hist_cut,
            &start2.run_id,
        )
        .await;
        assert_eq!(hdr1_cold.unwrap(), hdr1);
        assert_eq!(
            doc1_cold.unwrap()["editor"]["nodes"][0]["title"],
            "Rev 1 Scheduled"
        );
        assert_eq!(hdr2_cold.unwrap(), hdr2);
        assert_eq!(
            doc2_cold.unwrap()["editor"]["nodes"][0]["title"],
            "Rev 2 Layout Only"
        );

        // Delete session 0 and verify snapshot cleanup + post-delete refusal.
        let MethodResult::Deletion {
            result: SessionDeletionResult::Preview { preview },
        } = call(
            &connection,
            next_id,
            Method::SessionDeletePreview {
                session_id: f.sessions[0].id.clone(),
            },
        )
        .await
        else {
            panic!("expected deletion preview");
        };
        next_id += 1;
        let MethodResult::Deletion {
            result: SessionDeletionResult::Deleted { .. },
        } = call(
            &connection,
            next_id,
            Method::SessionDelete {
                session_id: f.sessions[0].id.clone(),
                expected_target_revision: preview.target_revision,
            },
        )
        .await
        else {
            panic!("expected Deleted");
        };
        next_id += 1;
        let deleted_resp = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(next_id),
                call: Method::SessionWorkflowRunSnapshotHeader {
                    session_id: f.sessions[0].id.clone(),
                    node_id: None,
                    cut: hist_cut,
                    run_id: start1.run_id,
                },
            })
            .await;
        assert!(matches!(deleted_resp, Response::Failure(_)));

        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue552_cancelled_workflow_preserves_snapshot_and_chunk_bounds_reject_invalid_limits() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        let review_with_editor = serde_json::json!({
            "description": "Cancellable review workflow",
            "timeout_ms": 30_000,
            "block": {
                "input": { "type": "object", "properties": {}, "additionalProperties": false },
                "nodes": {
                    "workflow_start": { "type": "start" },
                    "human": {
                        "type": "review",
                        "content": [{ "type": "literal", "value": "Approve?" }],
                        "actions": [{ "id": "approve", "title": "Approve" }],
                        "timeout_ms": 3_600_000
                    },
                    "done": {
                        "type": "return",
                        "outputs": [{ "variable": "action", "value_selector": ["human", "action"] }]
                    }
                },
                "edges": [
                    { "from": "workflow_start", "to": "human" },
                    { "from": "human", "to": "done", "port": "approve" },
                    { "from": "human", "to": "done", "port": "timeout" }
                ]
            }
        });
        f.install_scheduled_workflow_definition(&review_with_editor);
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();

        let conv0 = f.id(0).await;
        let resident = f.load(0).await.unwrap().unwrap();
        let runtime = resident.inspect_runtime().unwrap();
        let owner = runtime.interaction_test_owner();
        owner.set_provider_available(true);
        let published = owner.pending_published.notified();
        tokio::pin!(published);
        published.as_mut().enable();
        let created = schedule_create_workflow(&f, "wf-cancel").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start, ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: created.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected WorkflowAdmitted");
        };
        published.await;

        let cancelled = f
            .manager
            .cancel_scheduled_workflow(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                start.id.clone(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(cancelled.start.cancellation_requested_at.is_some());
        let settled = wait_scheduled_settled(&f, &runtime, &start.id).await;
        assert!(matches!(
            settled.start.state,
            ScheduledWorkflowState::Settled {
                outcome: ScheduledWorkflowOutcome::Cancelled,
                ..
            }
        ));

        drop(resident);
        drop(runtime);
        f.manager.unload(&conv0).await.unwrap();

        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::SessionTraceHistory {
            page: hist_page, ..
        } = call(
            &connection,
            500,
            Method::SessionTraceHistory {
                session_id: f.sessions[0].id.clone(),
                node_id: None,
                before: None,
                limit: 32,
            },
        )
        .await
        else {
            panic!("expected SessionTraceHistory");
        };

        let hist_cut = hist_page.read_cut.expect("cancelled history read cut");
        let mut next_id = 501_i64;
        let (hdr, doc) = read_cold_snapshot_json(
            &connection,
            &mut next_id,
            &f.sessions[0].id,
            &hist_cut,
            &start.run_id,
        )
        .await;
        let hdr = hdr.expect("cancelled run retains snapshot header");
        assert!(!hdr.has_source_presentation);
        assert_eq!(doc.unwrap()["description"], "Cancellable review workflow");

        // Chunk limit = 0, limit > 65_536, and offset > byte_length fail cleanly.
        for (offset, limit) in [
            (0, 0),
            (0, WORKFLOW_SNAPSHOT_CHUNK_BYTES + 1),
            (hdr.byte_length + 1, 64),
        ] {
            let err_resp = connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(next_id),
                    call: Method::SessionWorkflowRunSnapshotChunk {
                        session_id: f.sessions[0].id.clone(),
                        node_id: None,
                        cut: hist_cut.clone(),
                        run_id: start.run_id.clone(),
                        offset,
                        limit,
                    },
                })
                .await;
            next_id += 1;
            assert!(matches!(err_resp, Response::Failure(_)));
        }

        f.close().await;
    })
    .await;
}

async fn assert_rejected(connection: &AppServerConnection, id: i64, method: Method) {
    let response = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call: method,
        })
        .await;
    assert!(
        matches!(response, Response::Failure(_)),
        "expected RPC denial for id={id}, got {response:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue552_resident_layout_only_edit_republishes_without_unload_and_preserves_prior_run_snapshot()
 {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        f.install_scheduled_workflow_definition(&workflow_definition_with_editor(
            "Rev 1 Resident",
            40.0,
            60.0,
        ));
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();

        // Keep the loaded resident runtime and attached route live across the edit — NO unload().
        let resident = f.load(0).await.unwrap().unwrap();
        let runtime = resident.inspect_runtime().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target0 = attach(&connection, &f, 0).await;
        let mut next_id = 700_i64;

        let created1 = schedule_create_workflow(&f, "resident-rev-1").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start: start1,
            ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created1.id,
                    expected_revision: created1.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected WorkflowAdmitted for resident run 1");
        };
        let _ = wait_scheduled_settled(&f, &runtime, &start1.id).await;

        let (hdr1_opt, doc1_opt) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0,
            None,
            &start1.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        let hdr1 = hdr1_opt.expect("resident run 1 snapshot header");
        let doc1 = doc1_opt.expect("resident run 1 snapshot document");
        assert_eq!(doc1["editor"]["nodes"][0]["title"], "Rev 1 Resident");

        // Perform a metadata/layout-only Workflow edit on disk while the session remains loaded.
        std::fs::write(
            f.workspaces[0].join(".agents/workflows/scheduled.yaml"),
            serde_json::to_vec_pretty(&workflow_definition_with_editor(
                "Rev 2 Resident Layout Only",
                800.0,
                240.0,
            ))
            .unwrap(),
        )
        .unwrap();

        f.manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        super::configuration::applied(&f, 0).await;

        // Without unloading or detaching, admit Run #2 on the SAME resident runtime.
        let created2 = schedule_create_workflow(&f, "resident-rev-2").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start: start2,
            ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created2.id,
                    expected_revision: created2.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected WorkflowAdmitted for resident run 2");
        };
        let _ = wait_scheduled_settled(&f, &runtime, &start2.id).await;

        let (hdr2_opt, doc2_opt) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0,
            None,
            &start2.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        let hdr2 = hdr2_opt.expect("resident run 2 snapshot header");
        let doc2 = doc2_opt.expect("resident run 2 snapshot document");
        assert_eq!(
            hdr2.program_digest, hdr1.program_digest,
            "layout-only edit keeps identical executable program digest"
        );
        assert_ne!(
            hdr2.source_revision, hdr1.source_revision,
            "layout-only edit updates source revision on live resident session without unload"
        );
        assert_ne!(hdr2.snapshot_digest, hdr1.snapshot_digest);
        assert_eq!(
            doc2["editor"]["nodes"][0]["title"],
            "Rev 2 Resident Layout Only"
        );
        assert_eq!(doc2["editor"]["nodes"][0]["position"]["x"], 800.0);

        // Run #1 still resolves its original admitted Revision 1 snapshot on the live resident session.
        let (hdr1_after, doc1_after) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0,
            None,
            &start1.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        assert_eq!(hdr1_after.unwrap(), hdr1);
        assert_eq!(
            doc1_after.unwrap()["editor"]["nodes"][0]["title"],
            "Rev 1 Resident"
        );

        drop(runtime);
        drop(resident);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue552_rpc_header_and_chunk_authorization_routing_and_warmed_cache_denial() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 2).await;
        f.install_scheduled_workflow_definition(&workflow_definition_with_editor(
            "Rev 1 Auth",
            40.0,
            60.0,
        ));
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();

        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let mut next_id = 900_i64;

        let conv0 = f.id(0).await;
        let resident0 = f.load(0).await.unwrap().unwrap();
        let runtime0 = resident0.inspect_runtime().unwrap();
        let target0 = attach(&connection, &f, 0).await;
        let target1 = attach(&connection, &f, 1).await;

        // Establish a completed canonical turn on session 0 so we can create an explicit branched node later.
        let settled_before = runtime0.settlement_ordinal_for_test();
        f.gates[0].release();
        call(
            &connection,
            next_id,
            Method::TurnStart {
                input_id: uuid::Uuid::new_v4().to_string(),
                target: target0.clone(),
                content: wire_text("seed turn for branch routing"),
            },
        )
        .await;
        next_id += 1;
        let _ = runtime0.child_attempt_settlement(settled_before).await;

        // Checkpoint before Run 1 (valid earlier cut before advancing surface).
        let MethodResult::Checkpoint {
            checkpoint: cp_before,
        } = call(
            &connection,
            next_id,
            Method::SessionCheckpoint {
                target: target0.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("expected pre-run checkpoint");
        };
        next_id += 1;

        let created = schedule_create_workflow(&f, "auth-wf-1").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start: start1,
            ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: created.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected WorkflowAdmitted");
        };
        let _ = wait_scheduled_settled(&f, &runtime0, &start1.id).await;

        let MethodResult::Checkpoint { checkpoint: cp0 } = call(
            &connection,
            next_id,
            Method::SessionCheckpoint {
                target: target0.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("expected checkpoint cp0");
        };
        next_id += 1;
        let MethodResult::Checkpoint { checkpoint: cp1 } = call(
            &connection,
            next_id,
            Method::SessionCheckpoint {
                target: target1.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("expected checkpoint cp1");
        };
        next_id += 1;

        // Warm WorkflowDocuments::run_snapshots cache with an authorized live read.
        let (hdr1, _) = read_live_snapshot_json(
            &connection,
            &mut next_id,
            &target0,
            Some(cp0.read_cut.clone()),
            &start1.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        assert!(hdr1.is_some());

        // Pre-run1 retained cut hides Run 1 cleanly even with warmed cache (no cache leak across cuts).
        let MethodResult::WorkflowRunSnapshotHeader { header: pre_hdr } = call(
            &connection,
            next_id,
            Method::ConversationWorkflowRunSnapshotHeader {
                target: target0.clone(),
                agent_id: None,
                cut: Some(cp_before.read_cut.clone()),
                run_id: start1.run_id.clone(),
            },
        )
        .await
        else {
            panic!("expected WorkflowRunSnapshotHeader at pre-run cut");
        };
        assert!(pre_hdr.is_none());
        next_id += 1;
        let MethodResult::WorkflowRunSnapshotChunk { chunk: pre_chunk } = call(
            &connection,
            next_id,
            Method::ConversationWorkflowRunSnapshotChunk {
                target: target0.clone(),
                agent_id: None,
                cut: Some(cp_before.read_cut.clone()),
                run_id: start1.run_id.clone(),
                offset: 0,
                limit: 64,
            },
        )
        .await
        else {
            panic!("expected WorkflowRunSnapshotChunk at pre-run cut");
        };
        assert!(pre_chunk.is_none());
        next_id += 1;

        // Both Header AND Chunk RPCs must reject unauthorized/invalid/cross-conversation/future/stale cuts even with warmed cache.
        let mut future_cut = cp0.read_cut.clone();
        future_cut.through = crate::observation::content::CONTENT_SAFE_INTEGER_MAX;
        let mut future_rev_live = cp0.read_cut.clone();
        future_rev_live.revision = crate::observation::content::CONTENT_SAFE_INTEGER_MAX;
        let mut stale_cut = cp_before.read_cut.clone();
        stale_cut.revision = 0;
        let mut no_source_cut = cp0.read_cut.clone();
        no_source_cut.source_cut = None;
        let live_denials = [
            // Cross-conversation target (even with None cut).
            (target1.clone(), None, None, start1.run_id.clone()),
            // Cross-conversation cut on session 0 target.
            (
                target0.clone(),
                None,
                Some(cp1.read_cut.clone()),
                start1.run_id.clone(),
            ),
            // Future cut (journal and revision).
            (
                target0.clone(),
                None,
                Some(future_cut),
                start1.run_id.clone(),
            ),
            (
                target0.clone(),
                None,
                Some(future_rev_live),
                start1.run_id.clone(),
            ),
            // Stale expired cut (superseded revision / surface_revision).
            (
                target0.clone(),
                None,
                Some(stale_cut),
                start1.run_id.clone(),
            ),
            // Cut missing source_cut.
            (
                target0.clone(),
                None,
                Some(no_source_cut),
                start1.run_id.clone(),
            ),
            // Unauthorized/unknown agent_id with absent cut and with valid parent cut.
            (
                target0.clone(),
                Some(AgentId::new("unknown-child-agent")),
                None,
                start1.run_id.clone(),
            ),
            (
                target0.clone(),
                Some(AgentId::new("unknown-child-agent")),
                Some(cp0.read_cut.clone()),
                start1.run_id.clone(),
            ),
        ];
        for (target, agent_id, cut, run_id) in live_denials {
            assert_rejected(
                &connection,
                next_id,
                Method::ConversationWorkflowRunSnapshotHeader {
                    target: target.clone(),
                    agent_id: agent_id.clone(),
                    cut: cut.clone(),
                    run_id: run_id.clone(),
                },
            )
            .await;
            next_id += 1;
            assert_rejected(
                &connection,
                next_id,
                Method::ConversationWorkflowRunSnapshotChunk {
                    target,
                    agent_id,
                    cut,
                    run_id,
                    offset: 0,
                    limit: 64,
                },
            )
            .await;
            next_id += 1;
        }

        // Detach target0 while cache is warm and prove revoked/detached route cannot read Header or Chunk.
        call(
            &connection,
            next_id,
            Method::SessionDetach {
                target: target0.clone(),
            },
        )
        .await;
        next_id += 1;
        assert_rejected(
            &connection,
            next_id,
            Method::ConversationWorkflowRunSnapshotHeader {
                target: target0.clone(),
                agent_id: None,
                cut: None,
                run_id: start1.run_id.clone(),
            },
        )
        .await;
        next_id += 1;
        assert_rejected(
            &connection,
            next_id,
            Method::ConversationWorkflowRunSnapshotChunk {
                target: target0.clone(),
                agent_id: None,
                cut: None,
                run_id: start1.run_id.clone(),
                offset: 0,
                limit: 64,
            },
        )
        .await;
        next_id += 1;

        call(
            &connection,
            next_id,
            Method::SessionDetach { target: target1 },
        )
        .await;
        next_id += 1;

        // Create an explicit branch node on Session 0 via SessionBranch so Session 0 owns two distinct nodes.
        let tail = cp0
            .snapshot
            .transcript
            .entries
            .last()
            .and_then(|entry| entry.completed_response.as_ref())
            .expect("completed seed turn for branch");
        let MethodResult::SessionTransition {
            session: branch_session,
            ..
        } = call(
            &connection,
            next_id,
            Method::SessionBranch {
                session_id: f.sessions[0].id.clone(),
                node_id: f.sessions[0].active_node.clone(),
                surface_revision: tail.surface_revision,
                boundary: tail.closing_locator.clone(),
                side: LineageSide::After,
            },
        )
        .await
        else {
            panic!("expected SessionTransition from SessionBranch");
        };
        next_id += 1;
        let original_node = f.sessions[0].active_node.clone();
        let branched_node = branch_session.active_node;
        assert_ne!(branched_node, original_node);

        drop(runtime0);
        drop(resident0);
        f.manager.unload(&conv0).await.unwrap();

        let MethodResult::SessionTraceHistory {
            page: hist_root, ..
        } = call(
            &connection,
            next_id,
            Method::SessionTraceHistory {
                session_id: f.sessions[0].id.clone(),
                node_id: Some(original_node.clone()),
                before: None,
                limit: 32,
            },
        )
        .await
        else {
            panic!("expected SessionTraceHistory for original node");
        };
        next_id += 1;
        let root_cut = hist_root.read_cut.expect("root node cut");

        let MethodResult::SessionTraceHistory {
            page: hist_branch, ..
        } = call(
            &connection,
            next_id,
            Method::SessionTraceHistory {
                session_id: f.sessions[0].id.clone(),
                node_id: Some(branched_node.clone()),
                before: None,
                limit: 32,
            },
        )
        .await
        else {
            panic!("expected SessionTraceHistory for branched node");
        };
        next_id += 1;
        let branch_cut = hist_branch.read_cut.expect("branch node cut");

        // Warm history cache cleanly with explicit original_node.
        let MethodResult::WorkflowRunSnapshotHeader { header: root_hdr } = call(
            &connection,
            next_id,
            Method::SessionWorkflowRunSnapshotHeader {
                session_id: f.sessions[0].id.clone(),
                node_id: Some(original_node.clone()),
                cut: root_cut.clone(),
                run_id: start1.run_id.clone(),
            },
        )
        .await
        else {
            panic!("expected explicit original_node snapshot header");
        };
        next_id += 1;
        assert!(root_hdr.is_some());

        // History Header & Chunk must reject wrong/forked node_id, missing node, cross-session, future/stale/activation cuts even while cache is warm.
        assert!(root_cut.through > 0);
        let mut future_hist_cut = root_cut.clone();
        future_hist_cut.through = crate::observation::content::CONTENT_SAFE_INTEGER_MAX;
        if let Some(source) = future_hist_cut.source_cut.as_mut() {
            source.read.journal = crate::observation::content::CONTENT_SAFE_INTEGER_MAX;
        }
        let mut mismatched_source_cut = root_cut.clone();
        mismatched_source_cut
            .source_cut
            .as_mut()
            .expect("history source_cut")
            .read
            .journal = root_cut.through - 1;
        let mut with_activation_cut = root_cut.clone();
        with_activation_cut.activation_id = Some(SubagentId::for_conversation(&conv0, 1));
        let mut no_src_hist_cut = root_cut.clone();
        no_src_hist_cut.source_cut = None;

        let history_denials = [
            // Wrong branched node with root cut.
            (
                f.sessions[0].id.clone(),
                Some(branched_node.clone()),
                root_cut.clone(),
                start1.run_id.clone(),
            ),
            // Wrong branched node with its own cut targeting root run_id.
            (
                f.sessions[0].id.clone(),
                Some(branched_node),
                branch_cut,
                start1.run_id.clone(),
            ),
            // Nonexistent node_id.
            (
                f.sessions[0].id.clone(),
                Some(SessionNodeId::new(format!("node_{}", uuid::Uuid::now_v7()))),
                root_cut.clone(),
                start1.run_id.clone(),
            ),
            // Cross-session read with root cut.
            (
                f.sessions[1].id.clone(),
                None,
                root_cut.clone(),
                start1.run_id.clone(),
            ),
            // Future sequence / revision / activation_id / missing source_cut.
            (
                f.sessions[0].id.clone(),
                Some(original_node.clone()),
                future_hist_cut,
                start1.run_id.clone(),
            ),
            (
                f.sessions[0].id.clone(),
                Some(original_node.clone()),
                mismatched_source_cut,
                start1.run_id.clone(),
            ),
            (
                f.sessions[0].id.clone(),
                Some(original_node.clone()),
                with_activation_cut,
                start1.run_id.clone(),
            ),
            (
                f.sessions[0].id.clone(),
                Some(original_node),
                no_src_hist_cut,
                start1.run_id.clone(),
            ),
        ];
        for (session_id, node_id, cut, run_id) in history_denials {
            assert_rejected(
                &connection,
                next_id,
                Method::SessionWorkflowRunSnapshotHeader {
                    session_id: session_id.clone(),
                    node_id: node_id.clone(),
                    cut: cut.clone(),
                    run_id: run_id.clone(),
                },
            )
            .await;
            next_id += 1;
            assert_rejected(
                &connection,
                next_id,
                Method::SessionWorkflowRunSnapshotChunk {
                    session_id,
                    node_id,
                    cut,
                    run_id,
                    offset: 0,
                    limit: 64,
                },
            )
            .await;
            next_id += 1;
        }

        f.close().await;

        // Verify real subagent (agent_id) authorization & activation_id/cut checks using ChildDelivery.
        let child = ChildDelivery::with_delivery(false).await;
        let MethodResult::AgentObserve {
            observation:
                AgentObservation::Checkpoint {
                    checkpoint: child_cp,
                    ..
                },
        } = call(
            &child.connection,
            next_id,
            Method::AgentObserve {
                target: child.target.clone(),
                agent_id: child.agent.clone(),
                after: None,
            },
        )
        .await
        else {
            panic!("expected live child checkpoint");
        };
        next_id += 1;
        let child_cut = child_cp.read_cut;
        let child_conv = child_cut.conversation_id.clone();

        let MethodResult::Checkpoint {
            checkpoint: parent_cp,
        } = call(
            &child.connection,
            next_id,
            Method::SessionCheckpoint {
                target: child.target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("expected parent checkpoint");
        };
        next_id += 1;

        let child_scoped_run = WorkflowRunId {
            conversation_id: child_conv.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new("child-missing-run"),
            },
            invocation: 0,
        };
        let parent_scoped_run = WorkflowRunId {
            conversation_id: parent_cp.read_cut.conversation_id.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new("parent-run"),
            },
            invocation: 0,
        };

        // Authorized child_id + valid child_cut + child-scoped run_id returns Ok(None) cleanly on both Header and Chunk.
        let MethodResult::WorkflowRunSnapshotHeader { header: child_hdr } = call(
            &child.connection,
            next_id,
            Method::ConversationWorkflowRunSnapshotHeader {
                target: child.target.clone(),
                agent_id: Some(child.agent.clone()),
                cut: Some(child_cut.clone()),
                run_id: child_scoped_run.clone(),
            },
        )
        .await
        else {
            panic!("expected authorized child snapshot header result");
        };
        next_id += 1;
        assert!(child_hdr.is_none());
        let MethodResult::WorkflowRunSnapshotChunk { chunk: child_chunk } = call(
            &child.connection,
            next_id,
            Method::ConversationWorkflowRunSnapshotChunk {
                target: child.target.clone(),
                agent_id: Some(child.agent.clone()),
                cut: Some(child_cut.clone()),
                run_id: child_scoped_run.clone(),
                offset: 0,
                limit: 64,
            },
        )
        .await
        else {
            panic!("expected authorized child snapshot chunk result");
        };
        next_id += 1;
        assert!(child_chunk.is_none());

        // Mismatched activation, absent cut/activation, wrong conversation, future cut, and unauthorized agent_id must all fail on BOTH Header and Chunk.
        let mut wrong_activation_cut = child_cut.clone();
        wrong_activation_cut.activation_id = Some(SubagentId::for_conversation(&child_conv, 999));
        let mut absent_activation_cut = child_cut.clone();
        absent_activation_cut.activation_id = None;
        let mut future_child_cut = child_cut.clone();
        future_child_cut.through = crate::observation::content::CONTENT_SAFE_INTEGER_MAX;

        let agent_denials = [
            // Missing cut with agent_id: Some(...).
            (Some(child.agent.clone()), None, child_scoped_run.clone()),
            // Parent cut supplied for child agent_id.
            (
                Some(child.agent.clone()),
                Some(parent_cp.read_cut),
                child_scoped_run.clone(),
            ),
            // Child cut with activation_id stripped.
            (
                Some(child.agent.clone()),
                Some(absent_activation_cut),
                child_scoped_run.clone(),
            ),
            // Child cut with mismatched activation_id.
            (
                Some(child.agent.clone()),
                Some(wrong_activation_cut),
                child_scoped_run.clone(),
            ),
            // Valid child cut requesting parent-scoped run_id.
            (
                Some(child.agent.clone()),
                Some(child_cut.clone()),
                parent_scoped_run,
            ),
            // Future cut on child conversation.
            (
                Some(child.agent.clone()),
                Some(future_child_cut),
                child_scoped_run.clone(),
            ),
            // Unauthorized unknown agent_id even with authentic child_cut.
            (
                Some(AgentId::new("other-unauthorized-agent")),
                Some(child_cut),
                child_scoped_run,
            ),
        ];
        for (agent_id, cut, run_id) in agent_denials {
            assert_rejected(
                &child.connection,
                next_id,
                Method::ConversationWorkflowRunSnapshotHeader {
                    target: child.target.clone(),
                    agent_id: agent_id.clone(),
                    cut: cut.clone(),
                    run_id: run_id.clone(),
                },
            )
            .await;
            next_id += 1;
            assert_rejected(
                &child.connection,
                next_id,
                Method::ConversationWorkflowRunSnapshotChunk {
                    target: child.target.clone(),
                    agent_id,
                    cut,
                    run_id,
                    offset: 0,
                    limit: 64,
                },
            )
            .await;
            next_id += 1;
        }

        child.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue552_workflow_trace_locator_routes_exact_reads_without_execution_or_history_mutation()
{
    bounded(async {
        use crate::durable::{CanonicalStore, JournalStore, SqliteConversationStore};
        let mut f = Fixture::with_session_count(None, 2).await;
        f.install_scheduled_workflow("Inspection-only locator fixture");
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let resident = f.load(0).await.unwrap().unwrap();
        let runtime = resident.inspect_runtime().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let other = attach(&connection, &f, 1).await;
        let receipt = schedule_create_workflow(&f, "located-workflow").await;
        clock.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::WorkflowAdmitted {
            start, ..
        }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: receipt.id,
                    expected_revision: receipt.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("scheduled admission")
        };
        wait_scheduled_settled(&f, &runtime, &start.id).await;
        loop {
            let MethodResult::Checkpoint { checkpoint } = call(
                &connection,
                29_999,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![],
                },
            )
            .await
            else {
                panic!("expected checkpoint");
            };
            if checkpoint.snapshot.workflows.runs.iter().any(|run| {
                run.id == start.run_id
                    && matches!(
                        run.state,
                        crate::runtime::workflow::read_model::WorkflowState::Settled { .. }
                    )
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
        let access = f
            .manager
            .session_controller()
            .acquire_session(&f.sessions[0].id, None)
            .await
            .unwrap();
        let store = SqliteConversationStore::open_existing(
            start.run_id.conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        let canonical = store.load_canonical().unwrap();
        let journal = store.presentation_frontier().unwrap();
        let requests = f.provider.request_bodies().len();
        let workflow = runtime.tool_runtime().workflows().snapshot();
        let MethodResult::TraceWorkflowLocation {
            location: Some(location),
        } = call(
            &connection,
            30_000,
            Method::TraceLocateWorkflow {
                target: target.clone(),
                run_id: start.run_id.clone(),
            },
        )
        .await
        else {
            panic!("exact Workflow trace location")
        };
        let root = location
            .page
            .records
            .iter()
            .find(|record| record.id == location.record_id)
            .unwrap();
        assert_eq!(root.kind, crate::observation::trace::TraceKind::Workflow);
        let cut = location.page.read_cut.unwrap();
        assert_eq!(cut.conversation_id, start.run_id.conversation_id);
        let collection = root
            .collections
            .iter()
            .find(|frontier| frontier.domain.ends_with(".workflow.nodes"))
            .unwrap();
        let MethodResult::CheckpointPage { page } = call(
            &connection,
            30_001,
            Method::ConversationCheckpointPage {
                target: target.clone(),
                agent_id: None,
                cut: cut.clone(),
                domain: collection.domain.clone(),
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("existing Workflow node page")
        };
        assert_eq!(page.read_cut, cut);
        assert_eq!(page.items.len(), 2);
        let first_node: crate::durable::observation::WorkflowTraceNode =
            crate::observation::structural_json::decode(page.items[0].value.clone().unwrap())
                .unwrap();
        let MethodResult::WorkflowTraceNode {
            node: Some(exact_node),
        } = call(
            &connection,
            30_002,
            Method::ConversationWorkflowTraceNode {
                target: target.clone(),
                agent_id: None,
                cut: cut.clone(),
                instance: first_node.instance.clone(),
            },
        )
        .await
        else {
            panic!("exact live workflowTraceNode lookup")
        };
        assert_eq!(exact_node, first_node);
        assert_rejected(
            &connection,
            30_003,
            Method::ConversationWorkflowTraceNode {
                target: other.clone(),
                agent_id: None,
                cut: cut.clone(),
                instance: first_node.instance.clone(),
            },
        )
        .await;
        let mut id = 30_004;
        let (header, document) = read_live_snapshot_json(
            &connection,
            &mut id,
            &target,
            Some(cut.clone()),
            &start.run_id,
            WORKFLOW_SNAPSHOT_CHUNK_BYTES,
        )
        .await;
        assert_eq!(header.unwrap().run_id, start.run_id);
        assert!(document.is_some());
        let mut absent = start.run_id.clone();
        absent.invocation += 1;
        assert!(matches!(
            call(
                &connection,
                id,
                Method::TraceLocateWorkflow {
                    target: target.clone(),
                    run_id: absent
                }
            )
            .await,
            MethodResult::TraceWorkflowLocation { location: None }
        ));
        id += 1;
        assert_rejected(
            &connection,
            id,
            Method::TraceLocateWorkflow {
                target: other,
                run_id: start.run_id.clone(),
            },
        )
        .await;
        assert_eq!(store.load_canonical().unwrap(), canonical);
        assert_eq!(store.presentation_frontier().unwrap(), journal);
        assert_eq!(f.provider.request_bodies().len(), requests);
        assert_eq!(runtime.tool_runtime().workflows().snapshot(), workflow);
        call(
            &connection,
            id + 1,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        assert_rejected(
            &connection,
            id + 2,
            Method::TraceLocateWorkflow {
                target,
                run_id: start.run_id.clone(),
            },
        )
        .await;
        let MethodResult::WorkflowTraceNode {
            node: Some(history_node),
        } = call(
            &connection,
            id + 3,
            Method::SessionWorkflowTraceNode {
                session_id: f.sessions[0].id.clone(),
                node_id: Some(f.sessions[0].active_node.clone()),
                cut,
                instance: first_node.instance,
            },
        )
        .await
        else {
            panic!("exact detached history workflowTraceNode lookup")
        };
        assert_eq!(history_node, exact_node);
        assert_eq!(store.load_canonical().unwrap(), canonical);
        assert_eq!(store.presentation_frontier().unwrap(), journal);
        drop(store);
        drop(access);
        drop(runtime);
        drop(resident);
        f.close().await;

        // The real child inspection socket accepts the same finite read and
        // supplies its own cut; the parent cannot substitute its conversation.
        let child = ChildDelivery::with_delivery(false).await;
        let MethodResult::AgentObserve {
            observation: AgentObservation::Checkpoint { checkpoint, .. },
        } = call(
            &child.connection,
            30_100,
            Method::AgentObserve {
                target: child.target.clone(),
                agent_id: child.agent.clone(),
                after: None,
            },
        )
        .await
        else {
            panic!("child checkpoint")
        };
        let run = WorkflowRunId {
            conversation_id: checkpoint.read_cut.conversation_id.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new("absent-workflow"),
            },
            invocation: 1,
        };
        let child_instance = crate::runtime::workflow::WorkflowNodeInstance {
            block: crate::runtime::workflow::WorkflowBlockInstance {
                run: run.clone(),
                definition: crate::runtime::workflow::WorkflowDefinitionPath {
                    workflow_id: crate::runtime::workflow::WorkflowId::parse("absent_workflow")
                        .unwrap(),
                    blocks: Vec::new(),
                },
                invocations: vec![0],
            },
            node: "start".into(),
            visit: 0,
        };
        let requests = child.fixture.provider.request_bodies().len();
        assert!(matches!(
            call(
                &child.connection,
                30_101,
                Method::AgentTraceLocateWorkflow {
                    target: child.target.clone(),
                    agent_id: child.agent.clone(),
                    run_id: run.clone()
                }
            )
            .await,
            MethodResult::TraceWorkflowLocation { location: None }
        ));
        assert!(matches!(
            call(
                &child.connection,
                30_102,
                Method::ConversationWorkflowTraceNode {
                    target: child.target.clone(),
                    agent_id: Some(child.agent.clone()),
                    cut: checkpoint.read_cut.clone(),
                    instance: child_instance.clone(),
                }
            )
            .await,
            MethodResult::WorkflowTraceNode { node: None }
        ));
        let mut foreign = run.clone();
        foreign.conversation_id = child.target.conversation_id.clone();
        assert_rejected(
            &child.connection,
            30_103,
            Method::AgentTraceLocateWorkflow {
                target: child.target.clone(),
                agent_id: child.agent.clone(),
                run_id: foreign.clone(),
            },
        )
        .await;
        let mut foreign_instance = child_instance.clone();
        foreign_instance.block.run = foreign;
        assert_rejected(
            &child.connection,
            30_104,
            Method::ConversationWorkflowTraceNode {
                target: child.target.clone(),
                agent_id: Some(child.agent.clone()),
                cut: checkpoint.read_cut.clone(),
                instance: foreign_instance,
            },
        )
        .await;
        assert_rejected(
            &child.connection,
            30_105,
            Method::AgentTraceLocateWorkflow {
                target: child.target.clone(),
                agent_id: AgentId::new("unknown-child"),
                run_id: run,
            },
        )
        .await;
        let mut wrong_activation_cut = checkpoint.read_cut;
        wrong_activation_cut.activation_id = Some(SubagentId::new("wrong-activation"));
        assert_rejected(
            &child.connection,
            30_106,
            Method::ConversationWorkflowTraceNode {
                target: child.target.clone(),
                agent_id: Some(child.agent.clone()),
                cut: wrong_activation_cut,
                instance: child_instance,
            },
        )
        .await;
        assert_eq!(child.fixture.provider.request_bodies().len(), requests);
        child.close().await;
    })
    .await;
}
