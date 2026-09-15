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
    store.append_event(event(store, kind, seconds)).unwrap();
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
    let id = ConversationId::new("trace-reopen");
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
    let store = SqliteConversationStore::in_memory(ConversationId::new("safe")).unwrap();
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
    let store = SqliteConversationStore::in_memory(ConversationId::new("paging")).unwrap();
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
    let store = SqliteConversationStore::in_memory(ConversationId::new("cancel")).unwrap();
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
    let store = SqliteConversationStore::in_memory(ConversationId::new("compaction")).unwrap();
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
    let store = SqliteConversationStore::in_memory(ConversationId::new("tools")).unwrap();
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
    let store = SqliteConversationStore::in_memory(ConversationId::new("live-trace")).unwrap();
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
    entry.native_id = Some("execution-a".into());
    snapshot.trace.entries = vec![entry];
    snapshot.background.push(RuntimeClientBackgroundExecution {
        execution_id: crate::runtime::identity::ToolExecutionId::new("execution-b"),
        tool_id: ToolId::new("same-tool"),
        tool_name: "same-name".into(),
        state: BackgroundLifecycle::Running,
        progress: None,
        result: None,
    });
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.entries[0].state, TraceState::Incomplete);
    snapshot.background[0].execution_id =
        crate::runtime::identity::ToolExecutionId::new("execution-a");
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.entries[0].state, TraceState::Running);
    assert_eq!(snapshot.trace.entries[0].timing.duration_ms, None);
    snapshot.trace.entries[0].state = TraceState::OutcomeUnknown;
    repair_live(&mut snapshot);
    assert_eq!(snapshot.trace.entries[0].state, TraceState::OutcomeUnknown);
}

#[test]
fn trace_encoded_bound_accounts_for_escaped_strings_and_omitted_identities() {
    let store = SqliteConversationStore::in_memory(ConversationId::new("encoded-bound")).unwrap();
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
        let store =
            SqliteConversationStore::in_memory(ConversationId::new("terminal-class")).unwrap();
        start(&store);
        let req = request(&store, 0);
        failure(&store, &req, kind);
        assert_eq!(page(&store).entries.last().unwrap().state, state);
    }
}

#[test]
fn trace_workflow_runs_join_exact_native_identity_not_definition_name() {
    use crate::runtime::workflow::{WorkflowId, WorkflowRunId};
    let store = SqliteConversationStore::in_memory(ConversationId::new("workflow-trace")).unwrap();
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
