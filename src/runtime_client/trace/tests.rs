use super::*;
use crate::context::assembly::ContextGeneration;
use crate::durable::SqliteConversationStore;
use crate::model::invocation::{ModelInvocationConfig, RequestParams};
use crate::model::{
    ModelCapabilities, ModelCompat, ModelProtocol, RequestIdentity, RequestSnapshot,
};
use crate::runtime::identity::{CapabilityRevision, ConversationId, EventId};
use chrono::TimeZone;

fn timestamp(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
}
fn event(store: &dyn ConversationStore, kind: E, seconds: i64) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: 1,
        event_id: EventId::new(format!(
            "event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(AttemptId::new("attempt-a")),
        turn_id: Some(TurnId::new("1")),
        timestamp: timestamp(seconds),
        event: kind,
    }
}
fn append(store: &dyn ConversationStore, kind: E, seconds: i64) {
    let envelope = event(store, kind, seconds);
    match &envelope.event {
        E::BackgroundExecutionCommitted { .. } => {
            crate::durable::inbox::ConversationInboundCapability::commit_background_ownership(
                store, envelope,
            )
            .unwrap();
        }
        E::BackgroundTerminalPublished {
            message_id,
            execution_id,
            ..
        } => {
            store
                .accept_inbound_with_event(
                    crate::durable::InboundDraft {
                        message_id: Some(message_id.clone()),
                        source: crate::message::types::UserSource::Runtime,
                        kind: crate::message::types::InboundKind::Message,
                        content: vec![crate::message::types::UserContentBlock::Text(
                            crate::message::TextBlock {
                                text: "settled".into(),
                            },
                        )],
                        timestamp: timestamp(seconds),
                        correlation: Some(format!("background:{execution_id}")),
                    },
                    envelope,
                )
                .unwrap();
        }
        _ => {
            store.append_event(envelope).unwrap();
        }
    }
}
fn start(store: &dyn ConversationStore) {
    append(
        store,
        E::AttemptStarted {
            attempt_id: AttemptId::new("attempt-a"),
        },
        0,
    );
    append(store, E::TurnStarted, 1);
}
fn request(store: &dyn ConversationStore, retry: u32) -> RequestSnapshot {
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-a"),
            turn: TurnId::new("1"),
            retry_number: retry,
        },
        store.load_head().unwrap().revision,
        "PRIVATE /home/private/key secret-provider-token".repeat(10000),
        vec![],
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: format!("historical-model-{retry}"),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 100,
            request_params: RequestParams::from_iter([(
                "credential".into(),
                serde_json::json!("private-mcp-token"),
            )]),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        4096,
        None,
        false,
        vec![crate::tools::types::ModelToolDefinition {
            id: ToolId::new("tool-a"),
            name: "same-name".into(),
            description: "secret /private/executor".repeat(10000),
            input_schema: serde_json::json!({"private-schema": "x".repeat(100_000)}),
        }],
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: vec![],
        },
        None,
        vec![],
    );
    store
        .commit_model_turn_start(&[], &snapshot, timestamp(2 + i64::from(retry) * 2))
        .unwrap();
    snapshot
}
fn failure(
    store: &dyn ConversationStore,
    request: &RequestSnapshot,
    kind: crate::model::error::ModelErrorKind,
) {
    append(
        store,
        E::ModelRequestFailed {
            request_id: request.request_id.clone(),
            usage: None,
            error: crate::model::error::ModelError {
                kind,
                message: "SECRET /private/provider credential".into(),
                retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
        },
        3 + i64::from(request.identity.retry_number) * 2,
    );
}
fn page(store: &dyn ConversationStore) -> TracePage {
    TraceProjection::new(store)
        .unwrap()
        .page(None, TRACE_PAGE_LIMIT)
        .unwrap()
}

#[test]
fn trace_retries_share_one_native_step_and_reopen_identically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    start(&store);
    let first = request(&store, 0);
    failure(
        &store,
        &first,
        crate::model::error::ModelErrorKind::ContextWindowExceeded,
    );
    let second = request(&store, 1);
    failure(
        &store,
        &second,
        crate::model::error::ModelErrorKind::MalformedToolProposal,
    );
    let third = request(&store, 2);
    append(
        &store,
        E::ModelRequestCompleted {
            request_id: third.request_id,
            finish_reason: crate::model::finish::ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: 7,
                output_tokens: 11,
                total_tokens: 18,
                details: None,
            }),
        },
        7,
    );
    append(&store, E::TurnCompleted, 8);
    append(
        &store,
        E::AttemptCompleted {
            attempt_id: AttemptId::new("attempt-a"),
            finish_reason: crate::model::finish::ModelFinishReason::Stop,
        },
        9,
    );
    let projected = page(&store);
    assert_eq!(
        projected
            .entries
            .iter()
            .filter(|e| e.kind == TraceKind::Step)
            .count(),
        1
    );
    let requests: Vec<_> = projected
        .entries
        .iter()
        .filter_map(|entry| entry.request.as_ref())
        .collect();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.retry_number)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(
        requests[1].previous_failure_kind,
        Some(crate::model::error::ModelErrorKind::ContextWindowExceeded)
    );
    assert_eq!(
        requests[2].previous_failure_kind,
        Some(crate::model::error::ModelErrorKind::MalformedToolProposal)
    );
    assert!(requests[0].usage.is_none());
    assert_eq!(requests[2].usage.as_ref().unwrap().input_tokens, 7);
    assert_eq!(projected.entries[0].timing.duration_ms, Some(9000));
    assert!(
        projected
            .entries
            .iter()
            .all(|entry| entry.location.step_id == Some(TurnId::new("1")))
    );
    drop(store);
    let reopened = SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(page(&reopened), projected);
}

#[test]
fn trace_input_is_allowlisted_bounded_and_redacted() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_8b336994-4dd2-73fa-839e-32d1aeb1f763",
    ))
    .unwrap();
    start(&store);
    let req = request(&store, 0);
    failure(
        &store,
        &req,
        crate::model::error::ModelErrorKind::Authentication,
    );
    let projected = page(&store);
    let wire = serde_json::to_string(&projected).unwrap();
    assert!(wire.len() < 4096);
    for private in [
        "PRIVATE",
        "SECRET",
        "/home",
        "/private",
        "private-mcp-token",
        "request_params",
        "context_generation",
        "runtime_resource_revision",
        "provider_state",
    ] {
        assert!(!wire.contains(private), "leaked {private}");
    }
    let req = projected
        .entries
        .iter()
        .find_map(|entry| entry.request.as_ref())
        .unwrap();
    assert!(
        req.effective_system_prompt.redacted
            && req.context_input.redacted
            && req.tool_schema.redacted
    );
    let bounded = TraceText::visible(&"界".repeat(10000));
    assert!(bounded.truncated);
    assert!(bounded.text.len() <= TRACE_TEXT_BYTES);
    assert!(!bounded.redacted);
}

#[test]
fn trace_paging_and_read_cut_are_finite_and_independent() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_26ca59cb-e63e-7f7f-8903-10861c5839ba",
    ))
    .unwrap();
    start(&store);
    for retry in 0..40 {
        let req = request(&store, retry);
        failure(&store, &req, crate::model::error::ModelErrorKind::Transport);
    }
    let before_head = store.load_head().unwrap();
    let transcript = store.load_transcript_page(None, 64).unwrap();
    let frontier = store.presentation_frontier().unwrap();
    let read = TraceProjection::new(&store).unwrap();
    let newest = read.page(None, 7).unwrap();
    assert_eq!(newest.entries.len(), 7);
    let cursor = newest.next_cursor.clone().unwrap();
    let mut all = newest.entries.clone();
    let mut next = Some(cursor.clone());
    while let Some(cursor) = next {
        let older = read.page(Some(&cursor), 7).unwrap();
        next = older.next_cursor;
        let mut entries = older.entries;
        entries.extend(all);
        all = entries;
    }
    assert_eq!(all.len(), 42);
    assert_eq!(
        all.iter()
            .map(|entry| &entry.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        all.len()
    );
    assert_eq!(store.load_head().unwrap(), before_head);
    assert_eq!(store.load_transcript_page(None, 64).unwrap(), transcript);
    assert_eq!(store.presentation_frontier().unwrap(), frontier);
    // Explicit synchronization point: native progress occurs after capture,
    // before the historical read. No sleep or race probability is involved.
    let later = request(&store, 40);
    assert_eq!(read.page(None, 7).unwrap(), newest);
    assert_eq!(
        read.page(Some(&cursor), 7).unwrap(),
        TraceProjection {
            store: &store,
            through: frontier
        }
        .page(Some(&cursor), 7)
        .unwrap()
    );
    assert!(page(&store).entries.iter().any(|entry| {
        entry
            .request
            .as_ref()
            .is_some_and(|request| request.request_id == later.request_id)
    }));
    assert!(read.page(None, 0).is_err());
    assert!(read.page(None, 33).is_err());
    for bad in [
        "1",
        "request:1",
        "transcript:1",
        "trace:-1",
        "trace:999_999999_999999_999999",
    ] {
        assert!(read.page(Some(&TraceCursor(bad.into())), 1).is_err());
    }
}

#[test]
fn trace_missing_request_terminal_stays_incomplete_after_attempt_cancellation() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_2374d917-94b7-7f4f-84ec-9587d2cf00ae",
    ))
    .unwrap();
    start(&store);
    request(&store, 0);
    append(
        &store,
        E::AttemptCancelled {
            attempt_id: AttemptId::new("attempt-a"),
            reason: crate::runtime::types::CancellationReason::UserRequested,
        },
        5,
    );
    let entries = page(&store).entries;
    assert_eq!(entries[0].state, TraceState::Cancelled);
    assert_eq!(entries[2].state, TraceState::Incomplete);
    assert_eq!(entries[2].timing.duration_ms, None);
    assert!(
        entries
            .iter()
            .all(|entry| entry.kind != TraceKind::Assistant)
    );
}

#[test]
fn trace_attempt_terminal_vocabulary_does_not_infer_missing_outcomes() {
    use crate::events::types::{AttemptFailure, AttemptLimit};
    let outcomes = [
        (
            E::AttemptTimedOut {
                attempt_id: AttemptId::new("a"),
            },
            TraceState::TimedOut,
        ),
        (
            E::AttemptLimitExceeded {
                attempt_id: AttemptId::new("a"),
                limit: AttemptLimit::MaxTurns,
            },
            TraceState::Limited,
        ),
        (
            E::AttemptFailed {
                attempt_id: AttemptId::new("a"),
                error: AttemptFailure::Runtime {
                    error: crate::runtime::types::RuntimeError::Internal {
                        message: "secret".into(),
                    },
                },
            },
            TraceState::Failed,
        ),
        (E::TurnStarted, TraceState::Incomplete),
    ];
    for (event, expected) in outcomes {
        assert_eq!(terminal(&event), expected);
    }
    assert_eq!(
        tool_state(&ToolExecutionStatus::OutcomeUnknown {
            detail: "private".into()
        }),
        TraceState::OutcomeUnknown
    );
}

#[test]
fn trace_compaction_uses_the_next_boundary_even_far_back_in_history() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_96847128-a59a-7bfa-8bf9-526873a32546",
    ))
    .unwrap();
    // Manual compaction has no native operation ID: no timing proximity join.
    for index in 0..140 {
        append(&store, E::CompactionStarted, index * 2);
        append(
            &store,
            E::CompactionFailed {
                error: "private".into(),
            },
            index * 2 + 1,
        );
    }
    let read = TraceProjection::new(&store).unwrap();
    let first = read.page(Some(&TraceCursor::at(3)), 1).unwrap();
    assert_eq!(first.entries[0].state, TraceState::Failed);
    assert_eq!(first.entries[0].timing.duration_ms, Some(1000));
}

#[test]
#[allow(clippy::too_many_lines)] // Complete controlled parallel execution scenario.
fn trace_parallel_tool_completion_keeps_exact_call_order_and_certainty() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_f9d35d43-770d-7909-8a66-3e665e82ae1d",
    ))
    .unwrap();
    start(&store);
    let owner = crate::message::types::AssistantMessageBlock {
        id: MessageId::new("canonical-tool-owner"),
        content: ["call-a", "call-b", "call-c"]
            .into_iter()
            .map(|call| {
                AssistantContentBlock::ToolCall(crate::tools::types::ToolCall {
                    id: ToolCallId::new(call),
                    tool_id: ToolId::new(if call == "call-b" { "tool-b" } else { "tool-a" }),
                    name: "same-name".into(),
                    arguments: serde_json::json!({"secret": "private-tool-credential"}),
                })
            })
            .collect(),
    };
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(owner),
            event(
                &store,
                E::AssistantMessageCommitted {
                    message_id: MessageId::new("canonical-tool-owner"),
                },
                1,
            ),
        )
        .unwrap();
    for (call, tool, at) in [
        ("call-a", "tool-a", 2),
        ("call-b", "tool-b", 3),
        ("call-c", "tool-a", 4),
    ] {
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: ToolCallId::new(call),
                tool_id: ToolId::new(tool),
            },
            at,
        );
    }
    for (call, tool, status, at) in [
        (
            "call-b",
            "tool-b",
            ToolExecutionStatus::OutcomeUnknown {
                detail: "secret executor /private/path".into(),
            },
            6,
        ),
        ("call-a", "tool-a", ToolExecutionStatus::Success, 7),
    ] {
        append(
            &store,
            E::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new(call),
                tool_id: ToolId::new(tool),
                result: crate::tools::types::ToolExecutionResult {
                    status,
                    content: vec![crate::tools::types::ToolResultContent::Text(
                        crate::message::content::TextBlock {
                            text: "private-process-token".repeat(100_000),
                        },
                    )],
                    duration_ms: 999_999,
                    exit_code: None,
                    artifacts: vec![],
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            },
            at,
        );
    }
    let projected = page(&store);
    let tools: Vec<_> = projected
        .entries
        .iter()
        .filter(|entry| entry.kind == TraceKind::Tool)
        .collect();
    assert_eq!(
        tools
            .iter()
            .map(|entry| entry.tool.as_ref().unwrap().call_id.as_str())
            .collect::<Vec<_>>(),
        ["call-a", "call-b", "call-c"]
    );
    assert_eq!(
        tools.iter().map(|entry| entry.state).collect::<Vec<_>>(),
        [
            TraceState::Completed,
            TraceState::OutcomeUnknown,
            TraceState::Incomplete
        ]
    );
    assert_eq!(tools[0].timing.duration_ms, Some(5000));
    assert_eq!(tools[1].timing.duration_ms, Some(3000));
    assert_eq!(tools[2].timing.duration_ms, None);
    let wire = serde_json::to_string(&projected).unwrap();
    assert!(!wire.contains("private"));
    assert!(!wire.contains("999_999"));
}

#[test]
fn trace_live_labels_require_exact_runtime_identity_and_never_supply_timing() {
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::{CapabilityView, RuntimeClientBackgroundExecution};
    use crate::tools::background::BackgroundLifecycle;
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_da5c0fef-42f3-73f9-8f7f-b6878d9b0cbd",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    let mut snapshot = RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    let mut entry = TraceProjection::new(&store)
        .unwrap()
        .page(None, 32)
        .unwrap()
        .entries
        .remove(0);
    entry.kind = TraceKind::Background;
    entry.native_id = Some("exec_db47f954-a31a-74a3-8706-22baacdc0747".into());
    snapshot.trace.entries = vec![entry];
    snapshot.background.push(RuntimeClientBackgroundExecution {
        execution_id: crate::runtime::identity::ToolExecutionId::new(
            "exec_68344812-64a3-79bc-815f-6c3b32dfac91",
        ),
        tool_id: ToolId::new("same-tool"),
        tool_name: "same-name".into(),
        state: BackgroundLifecycle::Running,
        progress: None,
        result: None,
    });
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.entries[0].state, TraceState::Incomplete);
    snapshot.background[0].execution_id =
        crate::runtime::identity::ToolExecutionId::new("exec_db47f954-a31a-74a3-8706-22baacdc0747");
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.entries[0].state, TraceState::Running);
    assert_eq!(snapshot.trace.entries[0].timing.duration_ms, None);
    snapshot.trace.entries[0].state = TraceState::OutcomeUnknown;
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.entries[0].state, TraceState::OutcomeUnknown);
}

#[test]
fn trace_encoded_bound_accounts_for_escaped_strings_and_omitted_identities() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_480c9ce8-b0e2-70c4-882d-ae5878fe0801",
    ))
    .unwrap();
    start(&store);
    request(&store, 0);
    let mut entry = page(&store).entries.pop().unwrap();
    let escaped = "\0".repeat(512);
    entry.location.attempt_id = Some(AttemptId::new(escaped.clone()));
    entry.location.step_id = Some(TurnId::new(escaped.clone()));
    entry.native_id = Some(escaped.clone());
    entry.message_id = Some(MessageId::new(escaped.clone()));
    entry.calls = (0..8)
        .map(|_| TraceTool {
            call_id: ToolCallId::new(escaped.clone()),
            tool_id: ToolId::new(escaped.clone()),
            arguments: TraceText::withheld(),
        })
        .collect();
    let request = entry.request.as_mut().unwrap();
    request.request_id = RequestId::new(escaped.clone());
    request.assistant_message_id = MessageId::new(escaped);
    request.model = TraceText::visible(&"\0".repeat(2048));
    entry.artifacts.push(TraceArtifact {
        artifact_id: ArtifactId::new("oversized".repeat(1000)),
        image: true,
    });
    bound_entry(&mut entry);
    assert!(entry.truncated);
    assert!(entry.artifacts.is_empty());
    assert!(serde_json::to_vec(&entry).unwrap().len() <= 32 * 1024);
}

#[test]
fn trace_request_failure_preserves_native_cancellation_and_timeout_classes() {
    use crate::model::error::ModelErrorKind;
    for (kind, state) in [
        (ModelErrorKind::Cancelled, TraceState::Cancelled),
        (ModelErrorKind::Timeout, TraceState::TimedOut),
    ] {
        let store = SqliteConversationStore::in_memory(ConversationId::new(
            "conv_05d689cd-d3ca-7d01-8056-d79aaff8d9da",
        ))
        .unwrap();
        start(&store);
        let req = request(&store, 0);
        failure(&store, &req, kind);
        assert_eq!(page(&store).entries.last().unwrap().state, state);
    }
}

#[test]
fn trace_workflow_runs_join_exact_native_identity_not_definition_name() {
    use crate::runtime::workflow::{WorkflowId, WorkflowRunId};
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_141ff882-0973-7251-89c7-6389cfe2e736",
    ))
    .unwrap();
    let id = WorkflowId::parse("same-workflow").unwrap();
    let runs: Vec<_> = (1..=2)
        .map(|invocation| WorkflowRunId {
            conversation_id: store.conversation_id().clone(),
            attempt_id: AttemptId::new("attempt-a"),
            invocation,
        })
        .collect();
    for (index, run) in runs.iter().enumerate() {
        append(
            &store,
            E::WorkflowStarted {
                tool_call_id: ToolCallId::new("same-call"),
                workflow_id: id.clone(),
                run_id: run.clone(),
            },
            i64::try_from(index).unwrap(),
        );
    }
    append(
        &store,
        E::WorkflowCompleted {
            workflow_id: id,
            run_id: runs[1].clone(),
        },
        10,
    );
    let entries = page(&store).entries;
    assert_eq!(entries[0].state, TraceState::Incomplete);
    assert_eq!(entries[0].timing.duration_ms, None);
    assert_eq!(entries[1].state, TraceState::Completed);
    assert_eq!(entries[1].timing.duration_ms, Some(9000));
    assert_eq!(
        entries[1].native_id.as_deref(),
        Some(serde_json::to_string(&runs[1]).unwrap().as_str())
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Two independent native lifecycles across one controlled history.
fn old_background_and_workflow_records_are_repaired_and_settle_by_identity() {
    use crate::runtime::identity::{RuntimeResourceRevision, ToolExecutionId};
    use crate::runtime::workflow::read_model::{WorkflowRunView, WorkflowState};
    use crate::runtime::workflow::{WorkflowId, WorkflowRunId};
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::{CapabilityView, RuntimeClientBackgroundExecution};
    use crate::tools::background::BackgroundLifecycle;
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_7cc9b826-cbd4-78d6-87f0-53569f654a7b",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    let execution = ToolExecutionId::new("exec_6600d36f-a2f9-7057-8735-85a8c316b8af");
    let workflow = WorkflowId::parse("old-workflow").unwrap();
    let run = WorkflowRunId {
        conversation_id: store.conversation_id().clone(),
        attempt_id: AttemptId::new("attempt-a"),
        invocation: 1,
    };
    append(
        &store,
        E::BackgroundExecutionCommitted {
            execution_id: execution.clone(),
            tool_call_id: ToolCallId::new("background-call"),
            tool_id: ToolId::new("tool"),
            tool_name: "tool".into(),
        },
        0,
    );
    append(
        &store,
        E::WorkflowStarted {
            tool_call_id: ToolCallId::new("workflow-call"),
            workflow_id: workflow.clone(),
            run_id: run.clone(),
        },
        1,
    );
    let old = page(&store);
    let positions: Vec<_> = old
        .entries
        .iter()
        .map(|entry| entry.position.clone())
        .collect();
    for n in 2..70 {
        append(&store, E::TurnStarted, n);
    }
    let mut snapshot = RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    snapshot.background.push(RuntimeClientBackgroundExecution {
        execution_id: execution.clone(),
        tool_id: ToolId::new("tool"),
        tool_name: "tool".into(),
        state: BackgroundLifecycle::Running,
        progress: None,
        result: None,
    });
    snapshot.workflows.runs.push(WorkflowRunView {
        id: run.clone(),
        workflow_id: workflow.clone(),
        program_digest: "digest".into(),
        resource_revision: RuntimeResourceRevision::new(1),
        tool_call_id: ToolCallId::new("workflow-call"),
        state: WorkflowState::Running,
        instances: vec![],
        omitted_instances: 0,
        steps_consumed: 0,
        steps_max: 10,
        agents_consumed: 0,
        candidate: None,
        candidate_users: 0,
        handoff: None,
    });
    let projection = TraceProjection::new(&store).unwrap();
    snapshot.trace = projection.page(None, 32).unwrap();
    assert!(
        snapshot
            .trace
            .entries
            .iter()
            .all(|entry| !positions.contains(&entry.position))
    );
    let mut historical = snapshot.trace.clone();
    while let Some(before) = historical.next_cursor {
        historical = projection.page(Some(&before), 32).unwrap();
    }
    repair_entries(&mut historical.entries, &snapshot);
    for position in &positions {
        let entry = historical
            .entries
            .iter()
            .find(|entry| &entry.position == position)
            .unwrap();
        assert_eq!(entry.state, TraceState::Running);
        assert_eq!(entry.timing.duration_ms, None);
    }
    let inactive = projection.refresh(&positions, None).unwrap();
    assert!(
        inactive
            .iter()
            .all(|entry| entry.state == TraceState::Incomplete)
    );
    let live = projection.refresh(&positions, Some(&snapshot)).unwrap();
    assert!(live.iter().all(|entry| entry.state == TraceState::Running));
    append(
        &store,
        E::BackgroundTerminalPublished {
            execution_id: execution,
            message_id: MessageId::new("terminal"),
            state: crate::events::BackgroundTerminalState::Succeeded,
        },
        100,
    );
    append(
        &store,
        E::WorkflowCompleted {
            workflow_id: workflow,
            run_id: run,
        },
        101,
    );
    // The captured old cut cannot pick up later terminals, even with the same snapshot.
    assert_eq!(
        projection.refresh(&positions, Some(&snapshot)).unwrap(),
        live
    );
    let settled = TraceProjection::new(&store)
        .unwrap()
        .refresh(&positions, Some(&snapshot))
        .unwrap();
    assert_eq!(
        settled.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        live.iter().map(|entry| &entry.id).collect::<Vec<_>>()
    );
    assert!(
        settled
            .iter()
            .all(|entry| entry.state == TraceState::Completed
                && entry.timing.duration_ms == Some(100_000))
    );
    // Durable terminal facts win even over a copied, positively running runtime view.
    assert_eq!(settled.len(), 2);
}

#[test]
fn loaded_lifecycle_refresh_is_bounded_and_never_repeats_internal_request_input() {
    use crate::runtime_client::projection::RuntimeClientProjection;
    use crate::runtime_client::snapshot::CapabilityView;
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_08e8fef2-9708-70e8-875e-0815ed73b267",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    start(&store);
    request(&store, 0); // Oversized private prompt/schema/provider/MCP fields.
    let projection = TraceProjection::new(&store).unwrap();
    let page = projection.page(None, 32).unwrap();
    let position = page
        .entries
        .iter()
        .find(|entry| entry.kind == TraceKind::Request)
        .unwrap()
        .position
        .clone();
    let snapshot = RuntimeClientProjection::new(
        store.conversation_id().clone(),
        vec![],
        CapabilityView {
            revision: CapabilityRevision::new(1),
            tools: vec![],
            available_tools: vec![],
            skills: vec![],
            sources: vec![],
        },
        None,
        16,
    )
    .snapshot()
    .unwrap()
    .0;
    let updates = projection
        .refresh(&vec![position.clone(); TRACE_RECORD_LIMIT], Some(&snapshot))
        .unwrap();
    assert_eq!(updates.len(), TRACE_RECORD_LIMIT);
    for update in &updates {
        assert!(serde_json::to_vec(update).unwrap().len() <= 1024);
    }
    let wire = serde_json::to_string(&updates).unwrap();
    assert!(wire.len() <= TRACE_RECORD_LIMIT * 1025 + 2);
    for forbidden in [
        "PRIVATE",
        "credential",
        "/home/private",
        "private-mcp",
        "private-schema",
        "effective_system_prompt",
        "context_input",
        "tool_schema",
    ] {
        assert!(!wire.contains(forbidden));
    }
    assert!(
        projection
            .refresh(&vec![position; TRACE_RECORD_LIMIT + 1], Some(&snapshot))
            .is_err()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn canonical_tool_artifacts_merge_by_identity_with_bounded_first_occurrence() {
    use crate::message::content::{FileReference, ImageReference};
    use crate::tools::types::{ToolExecutionResult, ToolResultContent};
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_ac56fc5d-a6f5-7885-8745-ac1fad19bb38",
    ))
    .unwrap();
    start(&store);
    let file = |id: &str| FileReference {
        artifact_id: ArtifactId::new(id),
        name: Some("/private/host/path".into()),
        mime_type: None,
        description: Some("private executor metadata".into()),
    };
    for (index, content) in [
        vec![],
        vec![ToolResultContent::Image(ImageReference {
            artifact_id: ArtifactId::new("artifact-one"),
            alt: None,
        })],
    ]
    .into_iter()
    .enumerate()
    {
        let call = ToolCallId::new(format!("artifact-call-{index}"));
        let tool = ToolId::new("artifact-tool");
        let owner = MessageId::new(format!("artifact-owner-{index}"));
        store
            .append_canonical_with_event(
                &MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
                    id: owner.clone(),
                    content: vec![AssistantContentBlock::ToolCall(
                        crate::tools::types::ToolCall {
                            id: call.clone(),
                            tool_id: tool.clone(),
                            name: "files".into(),
                            arguments: serde_json::json!({}),
                        },
                    )],
                }),
                event(
                    &store,
                    E::AssistantMessageCommitted { message_id: owner },
                    1,
                ),
            )
            .unwrap();
        append(
            &store,
            E::ToolExecutionStarted {
                tool_call_id: call.clone(),
                tool_id: tool.clone(),
            },
            2,
        );
        let result = ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content,
            duration_ms: 0,
            exit_code: None,
            artifacts: vec![file("artifact-one"), file("artifact-two")],
            truncation: None,
            workflow: None,
            managed_output: None,
        };
        let id = MessageId::new(format!("artifact-result-{index}"));
        store
            .append_canonical_with_event(
                &MessageBlock::Tool(crate::message::types::ToolMessageBlock {
                    id: id.clone(),
                    tool_call_id: call.clone(),
                    tool_id: tool.clone(),
                    result,
                }),
                event(
                    &store,
                    E::ToolMessageCommitted {
                        message_id: id,
                        tool_call_id: call.clone(),
                    },
                    3,
                ),
            )
            .unwrap();
        let projected = page(&store);
        let entry = projected
            .entries
            .iter()
            .find(|entry| entry.tool.as_ref().is_some_and(|view| view.call_id == call))
            .unwrap();
        assert_eq!(
            entry
                .artifacts
                .iter()
                .map(|artifact| artifact.artifact_id.as_str())
                .collect::<Vec<_>>(),
            ["artifact-one", "artifact-two"]
        );
        assert_eq!(entry.artifacts[0].image, index == 1);
        assert!(!entry.artifacts[1].image);
        assert!(!serde_json::to_string(entry).unwrap().contains("private"));
    }
}
