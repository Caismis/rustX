//! M5 deterministic Agent Loop scheduling tests.
//!
//! These tests prove the deterministic scheduling phases of one committed
//! tool-call batch: parallel reversed physical completion with canonical
//! result order, the exclusive sequential barrier, parallel-group plus
//! sequential-barrier composition, mixed foreground/background groups that
//! never wait for detached settlement, and the structural cancellation
//! settlement of a batch. All synchronization is explicit (watches,
//! notifies, and controller tasks); no wall-clock sleep proves any
//! concurrency invariant.

#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

mod common;

use std::sync::Arc;

use common::fake::{FakeModel, FakeStep, FakeTool, ScriptedCall, success_result, tool_call_events};
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::types::{
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ReasoningEffort};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionStatus};

fn request() -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: ConversationId::new("conv-1"),
        attempt_id: AttemptId::new("attempt-1"),
        initial_messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-1"),
            content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                text: "go".to_owned(),
            })],
            source: UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })],
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: "fake-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
    }
}

fn runtime() -> rustx::context::ContextRuntime<'static> {
    use rustx::context::{
        ContextConfig, ContextEngine, ContextRuntime, DefaultTokenEstimator,
        InMemoryCheckpointStore,
    };
    let estimator: Arc<dyn rustx::context::TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("valid context configuration");
    ContextRuntime::new(
        engine,
        Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
        Arc::new(InMemoryCheckpointStore::new()),
    )
}

async fn run(
    model: &FakeModel,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
    mailbox: Option<rustx::runtime::inbound::ConversationInboundMailbox>,
) -> (
    AgentExecutionResult,
    rustx::capabilities::CapabilityCoordinator,
) {
    let tool_runtime = match mailbox {
        Some(mailbox) => common::tool_runtime_with_mailbox("conv-1", mailbox),
        None => common::tool_runtime("conv-1"),
    };
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let (lease, coordinator) = capability.into_lease_and_coordinator();
    let result = AgentExecution::new(
        request(),
        model,
        lease,
        cancellation,
        runtime(),
        &tool_runtime,
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    (result, coordinator)
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn done(reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    }
}

/// A tool-call turn scripted from calls; the following turn is a Stop.
fn tool_turn_then_stop(calls: &[&ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(started())];
    for (block, call) in calls.iter().enumerate() {
        let block = u32::try_from(block).expect("scripted blocks fit in u32");
        for event in tool_call_events(block, call) {
            first.push(FakeStep::Emit(event));
        }
    }
    first.push(FakeStep::Emit(done(ModelFinishReason::ToolCalls)));
    vec![
        first,
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]
}

fn scripted(id: &str, tool_id: &str, name: &str, arguments: serde_json::Value) -> ScriptedCall {
    ScriptedCall {
        id: Box::leak(id.to_owned().into_boxed_str()),
        tool_id: Box::leak(tool_id.to_owned().into_boxed_str()),
        name: Box::leak(name.to_owned().into_boxed_str()),
        arguments,
    }
}

/// Tool messages committed to canonical history in order.
fn tool_messages(result: &AgentExecutionResult) -> Vec<&ToolMessageBlock> {
    result
        .messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// The next model request after the tool turn.
fn next_request(result: &AgentExecutionResult, model: &FakeModel) -> rustx::model::ModelRequest {
    let _ = result;
    let requests = model.requests();
    requests
        .get(1)
        .cloned()
        .expect("a continuation request exists")
}

/// Parallel reversed completion: both parallel foreground calls start
/// concurrently, B completes physically first, and the canonical tool
/// messages plus the next model request stay in original call order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_reversed_completion_keeps_canonical_order() {
    let call_a = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({"n": 1}));
    let call_b = scripted("call-b", "tool-beta", "beta", serde_json::json!({"n": 2}));
    let model = FakeModel::new(tool_turn_then_stop(&[&call_a, &call_b]));
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    let (tool_b, release_b) = FakeTool::parking(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let mut started_a = tool_a.started();
    let mut started_b = tool_b.started();
    let completed_b = tool_b.completed();
    let mut controller_completed_b = completed_b.clone();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    // Both calls start concurrently; B completes first; then A.
    let controller = tokio::spawn(async move {
        started_a
            .wait_for(|started| *started)
            .await
            .expect("A started");
        started_b
            .wait_for(|started| *started)
            .await
            .expect("B started");
        release_b.notify_one();
        controller_completed_b
            .wait_for(|order| order.iter().any(|name| name == "beta"))
            .await
            .expect("B completed physically first");
        release_a.notify_one();
    });
    let (result, _capability) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(&model, tools, &cancellation, None),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");

    assert_eq!(
        completed_b
            .borrow()
            .iter()
            .filter(|name| name.as_str() == "beta")
            .count(),
        1,
        "B completed exactly once"
    );
    let messages = tool_messages(&result);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b"],
        "canonical ToolMessage order is the original model call order"
    );
    let next = next_request(&result, &model);
    let tool_message_ids: Vec<&str> = next
        .messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_message_ids,
        vec!["call-a", "call-b"],
        "the next model request observes A then B"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
}

/// The exclusive sequential barrier: a later sequential call cannot start
/// before the earlier call's attempt-facing result exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_barrier_blocks_later_calls() {
    let call_a = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({}));
    let call_b = scripted("call-b", "tool-beta", "beta", serde_json::json!({}));
    let model = FakeModel::new(tool_turn_then_stop(&[&call_a, &call_b]));
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("a"),
    );
    let tool_b = FakeTool::new(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("b"),
    );
    let mut started_a = tool_a.started();
    let mut started_b = tool_b.started();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let controller = tokio::spawn(async move {
        started_a
            .wait_for(|started| *started)
            .await
            .expect("A started");
        // A is still parked: B must not have started.
        assert!(
            !*started_b.borrow(),
            "B cannot start while the sequential barrier A is pending"
        );
        release_a.notify_one();
        started_b
            .wait_for(|started| *started)
            .await
            .expect("B started after A's result existed");
    });
    let (result, _capability) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(&model, tools, &cancellation, None),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");
    assert_eq!(
        tool_messages(&result)
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b"]
    );
}

/// Parallel group plus sequential barrier: P1/P2 may overlap and the
/// sequential call starts only after both attempt-facing results exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_group_then_sequential_barrier() {
    let call_p1 = scripted("call-p1", "tool-p1", "p1", serde_json::json!({}));
    let call_p2 = scripted("call-p2", "tool-p2", "p2", serde_json::json!({}));
    let call_s = scripted("call-s", "tool-s", "s", serde_json::json!({}));
    let model = FakeModel::new(tool_turn_then_stop(&[&call_p1, &call_p2, &call_s]));
    let (tool_p1, release_p1) = FakeTool::parking(
        common::tool_policies(
            "p1",
            "tool-p1",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("p1"),
    );
    let (tool_p2, release_p2) = FakeTool::parking(
        common::tool_policies(
            "p2",
            "tool-p2",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("p2"),
    );
    let tool_s = FakeTool::new(
        common::tool_policies(
            "s",
            "tool-s",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("s"),
    );
    let mut started_p1 = tool_p1.started();
    let mut started_p2 = tool_p2.started();
    let mut started_s = tool_s.started();
    let mut completed_p1 = tool_p1.completed();
    let mut completed_p2 = tool_p2.completed();
    let mut tools = ToolRegistry::new();
    tool_p1.register(&mut tools);
    tool_p2.register(&mut tools);
    tool_s.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let controller = tokio::spawn(async move {
        started_p1
            .wait_for(|started| *started)
            .await
            .expect("P1 started");
        started_p2
            .wait_for(|started| *started)
            .await
            .expect("P2 started");
        assert!(
            !*started_s.borrow(),
            "the sequential call cannot start before its barrier"
        );
        release_p1.notify_one();
        release_p2.notify_one();
        completed_p1
            .wait_for(|order| order.iter().any(|name| name == "p1"))
            .await
            .expect("P1 completed");
        completed_p2
            .wait_for(|order| order.iter().any(|name| name == "p2"))
            .await
            .expect("P2 completed");
        started_s
            .wait_for(|started| *started)
            .await
            .expect("S started only after both attempt-facing results");
    });
    let (result, _capability) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(&model, tools, &cancellation, None),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");
    assert_eq!(
        tool_messages(&result)
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-p1", "call-p2", "call-s"]
    );
}

/// Mixed foreground/background parallel group: the background accepted
/// result settles the originating attempt without waiting for detached
/// terminal settlement, and canonical result order stays original.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_foreground_background_group_does_not_wait_for_detached_terminal() {
    let call_a = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({}));
    let call_b = scripted(
        "call-b",
        "tool-beta",
        "beta",
        serde_json::json!({"__rustx_execution": "background"}),
    );
    let call_c = scripted("call-c", "tool-gamma", "gamma", serde_json::json!({}));
    let model = FakeModel::new(tool_turn_then_stop(&[&call_a, &call_b, &call_c]));
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    // The background tool parks forever: the originating attempt must not
    // wait for its detached terminal settlement.
    let (tool_b, _never_released) = FakeTool::parking(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let (tool_c, release_c) = FakeTool::parking(
        common::tool_policies(
            "gamma",
            "tool-gamma",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("c"),
    );
    let mut started_b = tool_b.started();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);
    tool_c.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let controller = tokio::spawn(async move {
        started_b
            .wait_for(|started| *started)
            .await
            .expect("the detached runner started after the dispatch commit");
        release_a.notify_one();
        release_c.notify_one();
    });
    let (result, _capability) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(&model, tools, &cancellation, None),
    )
    .await
    .expect("run terminates without the detached terminal");
    controller.await.expect("controller task");

    let messages = tool_messages(&result);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b", "call-c"],
        "canonical result order remains the original model call order"
    );
    let accepted = &messages[1].result;
    assert_eq!(accepted.status, ToolExecutionStatus::Success);
    let accepted_json = match &accepted.content[0] {
        rustx::tools::types::ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON accepted content, got {other:?}"),
    };
    assert_eq!(accepted_json["execution_id"], "exec_1");
    assert_eq!(accepted_json["state"], "starting");
    assert_eq!(accepted_json["tool"], "beta");
    let next = next_request(&result, &model);
    let tool_message_ids: Vec<&str> = next
        .messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_message_ids, vec!["call-a", "call-b", "call-c"]);
}

/// Cancellation during a mixed batch: in-flight foreground work receives
/// the physical cancellation signal and settles as cancelled, the committed
/// background dispatch stays conversation-owned, the complete batch commits
/// in call order, no next model request starts, and exactly one
/// `AttemptCancelled` terminal event ends the trace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn cancellation_during_mixed_batch_settles_structurally() {
    let call_a = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({}));
    let call_b = scripted(
        "call-b",
        "tool-beta",
        "beta",
        serde_json::json!({"__rustx_execution": "background"}),
    );
    let call_c = scripted("call-c", "tool-gamma", "gamma", serde_json::json!({}));
    let call_d = scripted("call-d", "tool-delta", "delta", serde_json::json!({}));
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &call_a)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &call_a)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &call_a)[2].clone()),
        FakeStep::Emit(tool_call_events(1, &call_b)[0].clone()),
        FakeStep::Emit(tool_call_events(1, &call_b)[1].clone()),
        FakeStep::Emit(tool_call_events(1, &call_b)[2].clone()),
        FakeStep::Emit(tool_call_events(2, &call_c)[0].clone()),
        FakeStep::Emit(tool_call_events(2, &call_c)[1].clone()),
        FakeStep::Emit(tool_call_events(2, &call_c)[2].clone()),
        FakeStep::Emit(tool_call_events(3, &call_d)[0].clone()),
        FakeStep::Emit(tool_call_events(3, &call_d)[1].clone()),
        FakeStep::Emit(tool_call_events(3, &call_d)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    // alpha: parallel foreground, parking; beta: parallel background,
    // parking (committed and conversation-owned); gamma: parallel
    // foreground, parking; delta: sequential foreground after the parallel
    // group — it must never start.
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    let (tool_b, _never_released) = FakeTool::parking(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let (tool_c, release_c) = FakeTool::parking(
        common::tool_policies(
            "gamma",
            "tool-gamma",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("c"),
    );
    let tool_d = FakeTool::new(
        common::tool_policies(
            "delta",
            "tool-delta",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("d"),
    );
    let mut started_a = tool_a.started();
    let mut started_c = tool_c.started();
    let started_d = tool_d.started();
    let mut started_b = tool_b.started();
    let mut completed_b = tool_b.completed();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);
    tool_c.register(&mut tools);
    tool_d.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        // The parallel group started: A and C are parked, B's detached
        // runner started after its dispatch commit.
        started_a
            .wait_for(|started| *started)
            .await
            .expect("A started");
        started_c
            .wait_for(|started| *started)
            .await
            .expect("C started");
        started_b
            .wait_for(|started| *started)
            .await
            .expect("detached B started");
        // Attempt cancellation wins while the foreground work is in flight.
        controller_cancellation.cancel();
        // The committed background dispatch must remain running: it never
        // observes the attempt signal and never settles.
        let settled = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            completed_b.wait_for(|order| !order.is_empty()),
        )
        .await;
        assert!(
            settled.is_err(),
            "the conversation-owned background execution must not settle from attempt cancellation"
        );
        let _ = release_a;
        let _ = release_c;
    });
    let (result, _capability) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(&model, tools, &cancellation, None),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");

    assert!(
        !*started_d.borrow(),
        "the sequential call after the cancelling group never starts"
    );
    let messages = tool_messages(&result);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b", "call-c", "call-d"],
        "the complete batch commits in original call order"
    );
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    assert_eq!(messages[1].result.status, ToolExecutionStatus::Success);
    let accepted = match &messages[1].result.content[0] {
        rustx::tools::types::ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    };
    assert_eq!(accepted["execution_id"], "exec_1");
    assert!(matches!(
        messages[2].result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    assert!(matches!(
        messages[3].result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    assert_eq!(
        model.requests().len(),
        1,
        "no next model request starts after cancellation"
    );
    let terminal_events: Vec<&RuntimeEvent> = result
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        })
        .collect();
    assert_eq!(terminal_events.len(), 1);
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert_eq!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    );
}

/// A business schema violation is a normal failed result slot: the executor
/// never runs, the batch still commits, and the attempt continues.
#[allow(clippy::doc_markdown)]
#[tokio::test]
async fn business_schema_violation_is_a_normal_failed_result_slot() {
    let call = scripted(
        "call-1",
        "tool-read",
        "read",
        serde_json::json!({"path": 42}),
    );
    let model = FakeModel::new(tool_turn_then_stop(&[&call]));
    let tool = FakeTool::new(
        common::tool_policies(
            "read",
            "tool-read",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("unexpected"),
    );
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    // The fixture definition is overridden with a strict schema so the
    // business arguments fail validation.
    let mut definition = common::tool_policies(
        "read",
        "tool-read",
        ToolExecutionPolicy::ForegroundOnly,
        ToolConcurrencyPolicy::Sequential,
    );
    definition.input_schema = serde_json::json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"],
        "additionalProperties": false
    });
    tools
        .register(definition, Arc::new(tool))
        .expect("register");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (result, _capability) = run(&model, tools, &cancellation, None).await;
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            calls_seen.wait_for(|calls| !calls.is_empty()),
        )
        .await
        .is_err(),
        "the executor must never run for a rejected invocation"
    );
}

/// A missing `ModelSelectable` execution field is a rejected result slot,
/// not a structural failure.
#[tokio::test]
async fn missing_model_selectable_field_is_rejected_without_executor() {
    let call = scripted("call-1", "tool-sel", "sel", serde_json::json!({}));
    let model = FakeModel::new(tool_turn_then_stop(&[&call]));
    let tool = FakeTool::new(
        common::tool_policies(
            "sel",
            "tool-sel",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("unexpected"),
    );
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (result, _capability) = run(&model, tools, &cancellation, None).await;
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            calls_seen.wait_for(|calls| !calls.is_empty()),
        )
        .await
        .is_err(),
        "the executor must never run"
    );
}

/// An identity mismatch in the model stream is a structural contract
/// failure: the attempt fails and the agent tool-call message is never
/// committed.
#[tokio::test]
async fn identity_mismatch_is_a_structural_contract_failure() {
    let call = scripted("call-1", "tool-alpha", "wrong-name", serde_json::json!({}));
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let tool = FakeTool::new(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("a"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (result, _capability) = run(&model, tools, &cancellation, None).await;
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(
        result.messages.len(),
        1,
        "the agent tool-call message is never committed for a structurally unresolvable call"
    );
}
