use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::test_fixtures::*;
use crate::agent::execution::test_sync::{ContinuationBoundaryPause, StartBoundaryPause};
use crate::agent::execution::{AgentExecution, AgentExecutionResult};
use crate::conversation::ConversationState;
use crate::durable::CanonicalStore;
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::message::types::{
    ContentBlockIndex, ContextKind, InboundKind, MessageBlock, UserSource,
};
use crate::model::error::ModelErrorKind;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{
    AttemptId, ConversationId, MessageId, RequestId, ToolCallId, ToolId,
};
use crate::runtime::inbound::InitialTurnTrigger;
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime, tool_runtime_with_store};
use crate::scripted_suites::support::fake::inbound_message;
use crate::tools::executor::{ToolExecutor, ToolRegistry};
use crate::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};
use std::sync::Arc;

fn workflow_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new(format!("runtime-{name}")),
        name: name.to_owned(),
        arguments,
    }
}

fn workflow_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "passed": {"type": "boolean"},
            "summary": {"type": "string"}
        },
        "required": ["passed", "summary"],
        "additionalProperties": false
    })
}

/// Runs an in-process child Agent Loop with the reserved Workflow
/// terminal authority. The model adapter and durable store are returned so
/// protocol feedback, canonical-history isolation, and request counts can
/// be inspected without timing-based synchronization.
async fn run_workflow_output_script(
    scripts: Vec<Vec<ModelEvent>>,
) -> (
    AgentExecutionResult,
    Arc<crate::runtime::workflow::WorkflowOutputLatch>,
    Arc<crate::durable::SqliteConversationStore>,
    Arc<ScriptedAdapter>,
) {
    let adapter = Arc::new(ScriptedAdapter::new(scripts));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("store"),
    );
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let latch = Arc::new(
        crate::runtime::workflow::WorkflowOutputLatch::new(workflow_output_schema())
            .expect("workflow output schema"),
    );
    let mut policy = crate::scripted_suites::support::default_execution_policy();
    let terminal: Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal> = latch.clone();
    policy.workflow_output = Some(terminal);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        policy,
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("workflow execution construction")
    .run()
    .await;
    (result, latch, store, adapter)
}

/// A completed canonical stream whose finish reason says `ToolCalls` but
/// contains no complete call fails the attempt before provider completion,
/// publication settlement, canonical history commit, or tool dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_tool_calls_finish_is_a_terminal_contract_failure() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
        ModelEvent::Started,
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        },
    ]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("store"),
    );
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let observer = RecordingObserver::default();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("execution construction");
    execution.observe(&observer);

    let result = execution.run().await;

    assert!(matches!(
        &result.outcome,
        AttemptOutcome::Failed {
            error: crate::events::types::AttemptFailure::Runtime {
                error: crate::runtime::types::RuntimeError::ContractViolation { message }
            }
        } if message.contains("ToolCalls")
            && message.contains("no complete canonical tool call")
    ));
    assert!(matches!(
        result.terminal_state,
        crate::agent::state::ExecutionState::Failed
    ));
    assert_eq!(
        adapter.request_count(),
        1,
        "the invalid stream is consumed once"
    );

    let events = event_history(store.as_ref());
    let terminal_events = events
        .iter()
        .filter(|event| AttemptOutcome::from_terminal_event(event).is_some())
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1, "exactly one terminal event");
    assert!(matches!(
        events.last(),
        Some(RuntimeEvent::AttemptFailed {
            error: crate::events::types::AttemptFailure::Runtime {
                error: crate::runtime::types::RuntimeError::ContractViolation { message }
            },
            ..
        }) if message.contains("no complete canonical tool call")
    ));
    assert!(events.iter().all(|event| !matches!(
        event,
        RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolExecutionStarted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. }
            | RuntimeEvent::TurnCompleted
            | RuntimeEvent::AttemptCompleted { .. }
    )));
    assert!(
        store
            .load_canonical()
            .expect("canonical history")
            .is_empty(),
        "an invalid model turn is not committed as assistant history"
    );

    let observed_events = observer.events();
    assert_eq!(
        observed_events
            .iter()
            .filter(|event| AttemptOutcome::from_terminal_event(event).is_some())
            .count(),
        1,
        "the observer sees exactly one terminal event"
    );
    assert!(matches!(
        observed_events.last(),
        Some(RuntimeEvent::AttemptFailed { .. })
    ));
}

/// A Workflow Agent cannot settle from ordinary Assistant text. The
/// rejected turn stays out of canonical history, receives bounded
/// request-only feedback, and the later valid protocol turn settles the
/// child exactly once.
#[tokio::test]
async fn workflow_output_requires_the_reserved_terminal_protocol() {
    let valid = workflow_call(
        "workflow-valid",
        "workflow_output",
        serde_json::json!({
            "passed": true,
            "summary": "corrected output"
        }),
    );
    let (result, latch, store, adapter) = run_workflow_output_script(vec![
        workflow_output_turn(&[], Some("ordinary final")),
        workflow_output_turn(&[valid], None),
    ])
    .await;

    assert!(
        matches!(
            result.outcome,
            AttemptOutcome::Completed {
                finish_reason: ModelFinishReason::ToolCalls
            }
        ),
        "unexpected Workflow Agent outcome: {:?}",
        result.outcome
    );
    assert_eq!(adapter.request_count(), 2);
    assert_eq!(
        latch.committed_value(),
        Some(serde_json::json!({
            "passed": true,
            "summary": "corrected output"
        }))
    );
    assert!(
        store
            .load_canonical()
            .expect("canonical history")
            .is_empty(),
        "Workflow Agent text and terminal values are not parent history"
    );
    let requests = adapter.requests();
    for request in &requests {
        assert_eq!(
            request
                .tools
                .iter()
                .filter(|tool| tool.name == crate::runtime::workflow::WORKFLOW_OUTPUT_TOOL_NAME)
                .count(),
            1,
            "a Workflow Agent request exposes exactly one reserved protocol definition"
        );
    }
    assert!(
        requests[1]
            .effective_system_prompt
            .contains("workflow_output(value)")
    );
    let events = event_history(store.as_ref());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AssistantMessageCommitted { .. }))
            .count(),
        0
    );
}

/// Invalid, mixed, and duplicate terminal-protocol turns are rejected as
/// whole structural units. In particular, an ordinary call in a mixed
/// turn never reaches preflight or an executor; a later valid turn can
/// still correct the child within the normal Agent Loop budget.
#[tokio::test]
async fn workflow_output_rejections_are_atomic_and_correctable() {
    let invalid = workflow_call(
        "workflow-invalid",
        "workflow_output",
        serde_json::json!({"passed": "not a boolean", "summary": "bad"}),
    );
    let ordinary = workflow_call("ordinary-side-effect", "write", serde_json::json!({}));
    let terminal = workflow_call(
        "workflow-terminal",
        "workflow_output",
        serde_json::json!({"passed": true, "summary": "committed"}),
    );
    let duplicate = workflow_call(
        "workflow-duplicate",
        "workflow_output",
        serde_json::json!({"passed": false, "summary": "duplicate"}),
    );
    let (result, latch, store, adapter) = run_workflow_output_script(vec![
        workflow_output_turn(&[invalid], None),
        workflow_output_turn(&[ordinary, terminal.clone()], None),
        workflow_output_turn(&[terminal.clone(), duplicate], None),
        workflow_output_turn(&[terminal], None),
    ])
    .await;

    assert!(
        matches!(
            result.outcome,
            AttemptOutcome::Completed {
                finish_reason: ModelFinishReason::ToolCalls
            }
        ),
        "unexpected Workflow Agent outcome: {:?}",
        result.outcome
    );
    assert_eq!(adapter.request_count(), 4);
    assert_eq!(
        latch.committed_value(),
        Some(serde_json::json!({
            "passed": true,
            "summary": "committed"
        }))
    );
    let events = event_history(store.as_ref());
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolMessageCommitted { .. }))
            .count(),
        0,
        "mixed and duplicate protocol turns dispatch no ordinary tool"
    );
    assert!(
        adapter.requests()[1]
            .effective_system_prompt
            .contains("frozen Agent output schema")
    );
}

/// The exact expected trace: one completed tool turn, then the generic
/// pre-next-turn cancellation checkpoint settles the attempt cancelled
/// before any second model turn.
/// The exact durable trace of the boundary-cancellation attempts.
///
/// `adopted` names the inbound message a safe-boundary drain adopted, when
/// the scenario has one: canonical adoption commits its durable answer
/// obligation in the same transaction, so the fact belongs in the exact
/// trace between the completed turn and the attempt terminal that consumes
/// it.
fn expected_trace(adopted: Option<&str>) -> Vec<crate::events::types::RuntimeEvent> {
    use crate::events::types::RuntimeEvent;
    let mut events = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "scripted".to_owned(),
        },
        // Assistant streaming assembly is no longer an Event Journal
        // fact (Issue #108): it lives in the durable publication plane.
        RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
            generation: None,
        },
        RuntimeEvent::AssistantMessageCommitted {
            message_id: MessageId::new("attempt-1-agent-1"),
        },
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
        },
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        },
        RuntimeEvent::ToolMessageCommitted {
            // The existing tool owner derives new message identity from the
            // canonical occurrence, independent of opaque provider call ID.
            message_id: MessageId::new("attempt-1-tool-1-0"),
            tool_call_id: ToolCallId::new("call-1"),
        },
        RuntimeEvent::TurnCompleted,
    ];
    if let Some(message_id) = adopted {
        events.push(RuntimeEvent::InboundTurnAdopted {
            message_ids: vec![MessageId::new(message_id)],
        });
    }
    events.push(RuntimeEvent::AttemptCancelled {
        attempt_id: AttemptId::new("attempt-1"),
        reason: CancellationReason::UserRequested,
    });
    events
}

/// Spawns the controller that parks until the continuation boundary,
/// makes cancellation observable there, and releases the execution.
fn boundary_controller(
    mut reached_rx: tokio::sync::watch::Receiver<bool>,
    release_tx: std::sync::mpsc::Sender<()>,
    cancellation: AgentCancellation,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        reached_rx
            .wait_for(|reached| *reached)
            .await
            .expect("continuation boundary reached");
        cancellation.cancel();
        release_tx.send(()).expect("release the execution");
    })
}

/// Issue #12 (M9b): tool continuation reaches the same gate. The first
/// request starts and completes a tool call; the second request is
/// parked immediately before its start arbitration, and cancelling
/// there is again cancellation-before-start: no second provider request,
/// no second `ModelRequestStarted`, while the first request's durable
/// facts (including the Tool message) remain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_continuation_start_stops_the_tool_turn() {
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        tool_call_script(&call),
        vec![ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }],
    ]));
    let mut tools = ToolRegistry::new();
    tools
        .register(
            InstantTool::definition("tool-alpha", "alpha"),
            std::sync::Arc::new(InstantTool),
        )
        .expect("register tool");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_start_boundary_pause(pause);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        // Request #1 reaches its start arbitration: let it start and
        // complete (including the tool execution).
        pre_start.await_park(1).await;
        pre_start.release();
        // The tool continuation reaches the same gate for request #2:
        // cancel before that start.
        pre_start.await_park(2).await;
        controller_cancellation.cancel();
        pre_start.release();
    });
    let result = execution.run().await;
    controller.await.expect("controller task");
    let events = event_history(tool_runtime.durable_store().as_ref());

    assert_eq!(
        adapter.request_count(),
        1,
        "the second provider request never started"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "exactly one ModelRequestStarted: the continuation start never committed"
    );
    assert!(
        result
            .messages()
            .iter()
            .any(|message| matches!(message, MessageBlock::Tool(_))),
        "the first request's Tool message remains a durable fact"
    );
    assert!(
        matches!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ),
        "the continuation settles cancelled before its start"
    );
    assert!(
        matches!(events.last(), Some(RuntimeEvent::AttemptCancelled { .. }))
            && events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
                .count()
                == 1,
        "the cancellation terminal is unique and last: {events:?}"
    );
}

/// Once the model-turn start has committed, a provider failure does not
/// roll back the canonical context fact, its Surface revision, or the
/// frozen provider-neutral request boundary.
#[tokio::test]
async fn post_start_provider_failure_preserves_historical_facts() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Failed {
        error: crate::model::error::ModelError {
            kind: ModelErrorKind::ProviderError,
            message: "provider failed after start".to_owned(),
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        },
    }]]));
    let mut request = request(&adapter);
    let inbound = inbound_message("fresh-1", "fresh input", UserSource::Human);
    let fresh = crate::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("fresh-1")])
        .expect("fresh trigger");
    request.conversation = ConversationState::from_messages(vec![MessageBlock::User(inbound)])
        .expect("canonical inbound history");
    request.initial_turn_trigger = InitialTurnTrigger::FreshInbound(fresh);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let result = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    let snapshots = request_snapshot_history(tool_runtime.durable_store().as_ref());

    assert!(matches!(
        result.outcome,
        crate::events::types::AttemptOutcome::Failed { .. }
    ));
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(snapshots.len(), 1);
    assert_eq!(result.conversation.revision().get(), 2);
    assert!(result.messages().iter().any(|message| {
        matches!(
            message,
            MessageBlock::User(user)
                if matches!(
                    &user.kind,
                    InboundKind::Context(ContextKind::AgentStatus(_))
                )
        )
    }));
    let reconstructed = tool_runtime
        .durable_store()
        .reconstruct_model_request(&snapshots[0].request_id)
        .expect("historical reconstruction");
    assert_eq!(reconstructed, adapter.requests()[0]);
}

/// The generic turn-boundary invariant with no mailbox attached: turn 1
/// completes with a tool call and its result, the test control point
/// makes cancellation observable after the turn (and all of its work)
/// completed but before the next turn begins, and the generic
/// pre-next-turn checkpoint settles the attempt cancelled — the second
/// model turn never starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_at_turn_boundary_stops_next_model_request_without_mailbox() {
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![tool_call_script(&call)]));
    let mut tools = ToolRegistry::new();
    tools
        .register(
            InstantTool::definition("tool-alpha", "alpha"),
            std::sync::Arc::new(InstantTool),
        )
        .expect("register tool");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
    let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
    let execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution
        .pauses
        .continuation_pause
        .lock()
        .expect("continuation pause lock")
        .replace(pause);
    let result = execution.run().await;
    controller.await.expect("controller task");
    let events = event_history(tool_runtime.durable_store().as_ref());

    assert_eq!(
        adapter.request_count(),
        1,
        "exactly one model request total: the second model turn never begins"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, crate::events::types::RuntimeEvent::TurnStarted))
            .count(),
        1,
        "exactly one TurnStarted total"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
                )
            })
            .count(),
        1,
        "exactly one ModelRequestStarted total"
    );
    assert_eq!(
        events,
        expected_trace(None),
        "the exact trace ends with the single AttemptCancelled terminal event"
    );
    assert_eq!(
        result.outcome,
        crate::events::types::AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        }
    );
    assert!(
        result
            .messages()
            .iter()
            .all(|message| !message.is_agent_status()),
        "the attempt-local PostToolBatch marker cannot create a status turn"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::ContextContributionEmitted { .. })),
        "cancellation before the next start leaves no durable status emission"
    );
}

/// The drain+append commit-point interleaving on the generic boundary
/// hook: a tool turn completes, the safe boundary atomically drains
/// batch A and appends it to canonical history, the continuation
/// boundary control point makes cancellation observable there, and
/// after the release the generic pre-next-turn checkpoint prevents any
/// second model turn — mailbox commit semantics and generic Agent Loop
/// cancellation compose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_after_drain_append_stops_before_the_next_turn() {
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![tool_call_script(&call)]));
    let mut tools = ToolRegistry::new();
    tools
        .register(
            InstantTool::definition("tool-alpha", "alpha"),
            std::sync::Arc::new(InstantTool),
        )
        .expect("register tool");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let mailbox = tool_runtime.mailbox();
    mailbox
        .enqueue(inbound_message("msg-a", "A", UserSource::Human))
        .expect("enqueue A before the attempt");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
    let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
    let execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution
        .pauses
        .continuation_pause
        .lock()
        .expect("continuation pause lock")
        .replace(pause);
    let result = execution.run().await;
    controller.await.expect("controller task");
    let events = event_history(store.as_ref());

    assert_eq!(
        adapter.request_count(),
        1,
        "no next model turn begins after the drained batch is committed"
    );
    assert_eq!(
        events,
        expected_trace(Some("msg-a")),
        "the exact trace ends with the single AttemptCancelled terminal event"
    );
    assert_eq!(
        result.outcome,
        crate::events::types::AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        }
    );
    let current_messages = result.messages();
    let committed: Vec<&MessageBlock> = current_messages
        .iter()
        .filter(
            |block| matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-a")),
        )
        .collect();
    assert_eq!(
        committed.len(),
        1,
        "the drained batch appears exactly once in canonical history"
    );
    assert!(
        mailbox.select_pending_batch().expect("select").is_none(),
        "the adopted batch is consumed from the durable inbox and never requeued"
    );
}

/// A parking background executor: its returned future reports entry,
/// waits for durable release state, and then settles with a fixed result.
struct ParkingBackgroundTool {
    definition: ToolDefinition,
    started: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Sender<bool>,
}

impl ParkingBackgroundTool {
    fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (release, _release_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                definition: ToolDefinition {
                    id: ToolId::new("tool-bg"),
                    name: "bg".to_owned(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "object"}),
                    execution_policy: ToolExecutionPolicy::ModelSelectable,
                    concurrency_policy: ToolConcurrencyPolicy::Sequential,
                    approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
                    replay_policy: ToolReplayPolicy::Never,
                    origin: ToolOrigin::Builtin,
                },
                started,
                release: release.clone(),
            },
            started_rx,
            release,
        )
    }
}

impl ToolExecutor for ParkingBackgroundTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let started = self.started.clone();
        let mut release = self.release.subscribe();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                started.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("release channel stays open");
                ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// The outer liveness guard of the deterministic mailbox-boundary test.
///
/// Nothing in that test's ordering proof depends on this value: every
/// semantic step is an exact handshake. It bounds only total wall time
/// so a genuine regression fails with a message instead of hanging a
/// CI job, and it is deliberately far larger than any scheduling delay
/// a loaded runner can produce.
const LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

/// Exact deterministic proof for the background terminal inbound.
///
/// The production finite-watermark contract under test (Issue #63):
///
/// ```text
/// once a safe-boundary selection froze its finite watermark, an inbound
/// whose durable acceptance linearizes after that selection can never
/// join the selected batch — it belongs to the next safe boundary
/// ```
///
/// The test constructs the exact happens-before chain using only the
/// test-only continuation-boundary pause and the parking background
/// executor; no sleep or scheduler-timing assumption participates in the
/// proof:
///
/// ```text
/// [human] durably accepted before the attempt starts (sequence 1)
/// turn 1 (continuation) dispatches the parking background tool
/// turn 1 completes → safe-boundary selection/adoption #1 commits
///   [human] (sequence 1) — the first finite batch is frozen forever
/// Agent Loop parks at the continuation boundary before turn 2
/// background runner settled → terminal durably accepted (sequence 2)
///   — provably after adoption #1 (the loop is parked)
/// Agent Loop released → turn 2 request observes [human], no terminal
/// turn 2 completes → adoption #2 commits [terminal] (sequence 2)
/// turn 3 request observes the terminal exactly once
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn terminal_inbound_after_snapshot_can_never_join_the_first_batch() {
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let mailbox = tool_runtime.mailbox();
    mailbox
        .enqueue(inbound_message("msg-human", "hello", UserSource::Human))
        .expect("accept the human message");

    let call = ToolCall {
        id: ToolCallId::new("call-bg"),
        tool_id: ToolId::new("tool-bg"),
        name: "bg".to_owned(),
        arguments: serde_json::json!({"execution_mode": "background"}),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        tool_call_script(&call),
        vec![
            ModelEvent::Started,
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "turn two".to_owned(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ],
        vec![
            ModelEvent::Started,
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "turn three".to_owned(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ],
    ]));
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let mut tools = ToolRegistry::new();
    tools
        .register(tool.definition.clone(), Arc::new(tool))
        .expect("register bg tool");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let background = tool_runtime.background().clone();
    let (pause, mut pause_reached, pause_release) = ContinuationBoundaryPause::install();
    let controller = tokio::spawn(async move {
        // 1. The detached background runner is provably started.
        tokio::time::timeout(LIVENESS_GUARD, started.wait_for(|started| *started))
            .await
            .expect("bg runner start wait exceeded liveness guard")
            .expect("bg runner started");
        // 2. Turn 1 completed and adoption #1 committed [human]; the
        //    loop is parked before turn 2, so no later selection can
        //    run until it is released.
        tokio::time::timeout(LIVENESS_GUARD, pause_reached.wait_for(|reached| *reached))
            .await
            .expect("continuation wait exceeded liveness guard")
            .expect("continuation boundary reached");
        // 3. Settle the runner: its terminal inbound is durably accepted
        //    after adoption #1 already froze the first finite batch. The
        //    registry terminal observation proves the durable enqueue
        //    completed (finish publishes before notifying state).
        release.send_replace(true);
        let execution_id = background
            .all_snapshots()
            .into_iter()
            .next()
            .unwrap()
            .execution_id;
        background
            .wait_until_terminal(&execution_id)
            .await
            .expect("the terminal state is durably published");
        // 4. Release the boundary twice: turn 2 runs and adopts
        //    [terminal] at its own safe boundary; turn 3 then observes
        //    the terminal as its fresh inbound.
        pause_release
            .send(())
            .expect("release the first continuation boundary");
        pause_release
            .send(())
            .expect("release the second continuation boundary");
        format!("background-{execution_id}-terminal")
    });
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
    let execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution
        .pauses
        .continuation_pause
        .lock()
        .expect("continuation pause lock")
        .replace(pause);
    // Outer liveness guards only: every step above is an exact
    // handshake, so these bounds never participate in the proof — they
    // exist so a regression fails loudly instead of hanging.
    let _result = tokio::time::timeout(LIVENESS_GUARD, execution.run())
        .await
        .expect("the attempt terminates");
    let terminal_id = tokio::time::timeout(LIVENESS_GUARD, controller)
        .await
        .expect("the controller terminates")
        .expect("controller task");

    let requests = adapter.requests.lock().expect("requests lock").clone();
    assert_eq!(
        requests.len(),
        3,
        "the constructed ordering is exactly: human turn, terminal turn, stop turn"
    );
    let second_request = &requests[1];
    assert!(
        second_request.messages.iter().any(|message| {
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == MessageId::new("msg-human"))
        }),
        "the human message joins the second request"
    );
    assert!(
        !second_request.messages.iter().any(|message| {
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id.as_str() == terminal_id)
        }),
        "the terminal can never appear in the first drained batch"
    );
    assert!(
        requests[2]
            .messages
            .iter()
            .any(|message| matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id.as_str() == terminal_id)),
        "the terminal inbound waits for the next drained batch"
    );
    let terminal_occurrences = requests
        .iter()
        .flat_map(|request| &request.messages)
        .filter(|message| {
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id.as_str() == terminal_id)
        })
        .count();
    assert_eq!(
        terminal_occurrences, 1,
        "the terminal inbound is drained and committed exactly once"
    );
    assert!(
        mailbox.select_pending_batch().expect("select").is_none(),
        "the durable inbox is drained"
    );
}
