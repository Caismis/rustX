//! Issue #130 deterministic Agent Status regressions.
//!
//! These tests drive the real Agent Loop over the real canonical tool-batch
//! boundary. They prove that a settled batch contributes one attempt-local
//! opportunity to the next model step, that sibling completion order cannot
//! split the opportunity, and that a safe-boundary inbound admission can join
//! it as one combined opportunity set. The Todo case uses the native
//! conversation-owned tool authority and checks the durable emission fact
//! produced by model-turn start.

#![allow(clippy::similar_names)] // sibling tool fixtures intentionally share names

use super::{common, support};

use std::sync::{Arc, Mutex};

use chrono::TimeZone;
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionObserver, AgentExecutionRequest,
    AgentStatusObservation,
};
use rustx::context::{
    AgentStatusClock, AgentStatusConfig, AgentStatusEngine, AgentStatusModuleId, ContextRuntime,
    DefaultTokenEstimator, SessionContextPolicy, TODO_STATUS_EMISSION_KEY,
};
use rustx::durable::ConversationStore;
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::inbound::InitialTurnTrigger;
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::todo::TODO_TOOL_ID;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, await_started, fake_model, success_result,
    tool_call_events,
};

#[derive(Debug, Clone, Copy)]
struct FixedStatusClock;

impl AgentStatusClock for FixedStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 8, 27, 12, 0, 0)
            .single()
            .expect("fixed status timestamp")
    }
}

#[derive(Debug, Default)]
struct StatusRecorder {
    observations: Mutex<Vec<AgentStatusObservation>>,
    events: Mutex<Vec<RuntimeEvent>>,
}

impl StatusRecorder {
    fn observations(&self) -> Vec<AgentStatusObservation> {
        self.observations
            .lock()
            .expect("status observation lock")
            .clone()
    }

    fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("runtime event lock").clone()
    }
}

impl AgentExecutionObserver for StatusRecorder {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        self.events
            .lock()
            .expect("runtime event lock")
            .push(event.clone());
    }

    fn observe_committed(
        &self,
        _attempt_id: &AttemptId,
        _block: &MessageBlock,
        _transcript_cursor: Option<rustx::durable::TranscriptCursor>,
    ) {
    }

    fn observe_status(&self, observation: &AgentStatusObservation) {
        self.observations
            .lock()
            .expect("status observation lock")
            .push(observation.clone());
    }

    fn observe_publication_opened(
        &self,
        _attempt_id: &AttemptId,
        _start: &rustx::publication::PublicationStreamStart,
    ) {
    }

    fn observe_publication(
        &self,
        _attempt_id: &AttemptId,
        _frame: &rustx::publication::PublicationFrame,
    ) {
    }

    fn observe_publication_settled(
        &self,
        _attempt_id: &AttemptId,
        _audit: &rustx::publication::PublicationAudit,
        _transcript_cursor: rustx::durable::TranscriptCursor,
    ) {
    }
}

fn inbound(id: &str, text: &str) -> UserMessageBlock {
    UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    }
}

fn scripted(id: &'static str, tool_id: &'static str, name: &'static str) -> ScriptedCall {
    ScriptedCall {
        id,
        tool_id,
        name,
        arguments: serde_json::json!({}),
    }
}

fn tool_turn_then_stop(calls: &[ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (index, call) in calls.iter().enumerate() {
        first.extend(
            tool_call_events(u32::try_from(index).expect("scripted block index"), call)
                .into_iter()
                .map(FakeStep::Emit),
        );
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![
        first,
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

fn context_runtime(model: &Arc<FakeModel>) -> ContextRuntime {
    let snapshot = support::attempt_model(model.clone(), "issue-130-model");
    ContextRuntime::for_attempt(
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(DefaultTokenEstimator),
        AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(FixedStatusClock)),
        &snapshot,
    )
    .expect("valid context runtime")
}

async fn run_attempt(
    conversation_id: &str,
    attempt_id: &str,
    model: Arc<FakeModel>,
    tools: ToolRegistry,
    conversation: Vec<MessageBlock>,
    initial_turn_trigger: InitialTurnTrigger,
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
) -> (rustx::agent::AgentExecutionResult, StatusRecorder) {
    let capability = common::capability_lease(tools, tool_runtime).await;
    let (lease, _coordinator) = capability.into_lease_and_coordinator();
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("issue-130-agent"),
        conversation_id: ConversationId::new(conversation_id),
        attempt_id: AttemptId::new(attempt_id),
        conversation: rustx::conversation::ConversationState::from_messages(conversation)
            .expect("canonical conversation"),
        initial_turn_trigger,
        model: support::attempt_model(model.clone(), "issue-130-model"),
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        context_runtime(&model),
        tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    let recorder = StatusRecorder::default();
    execution.observe(&recorder);
    (execution.run().await, recorder)
}

fn status_messages(result: &rustx::agent::AgentExecutionResult) -> Vec<&UserMessageBlock> {
    result
        .messages()
        .iter()
        .filter(|message| message.is_agent_status())
        .filter_map(|message| match message {
            MessageBlock::User(user) => Some(user),
            _ => None,
        })
        .collect()
}

fn tool_message_ids(result: &rustx::agent::AgentExecutionResult) -> Vec<String> {
    result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool.tool_call_id.to_string()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_settled_tool_batch_creates_one_post_tool_opportunity() {
    let call = ScriptedCall {
        id: "call-one",
        tool_id: TODO_TOOL_ID,
        name: "todo",
        arguments: serde_json::json!({
            "action": "create",
            "subject": "Keep the plan visible"
        }),
    };
    let model = fake_model(tool_turn_then_stop(std::slice::from_ref(&call)));
    let fixture = common::native_fixture();
    let store = fixture.store.clone();
    let (result, recorder) = run_attempt(
        "conv-m5",
        "attempt-post-only",
        model.clone(),
        fixture.registry.clone(),
        vec![MessageBlock::User(inbound("bootstrap", "work"))],
        InitialTurnTrigger::Continuation,
        &fixture.runtime,
    )
    .await;

    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(
        model.requests().len(),
        2,
        "PostToolBatch adds no model turn"
    );
    let observations = recorder.observations();
    assert_eq!(
        observations.len(),
        1,
        "one primary step owns one status generation"
    );
    assert!(observations[0].opportunities.fresh_inbound.is_none());
    assert!(observations[0].opportunities.post_tool_batch.is_some());

    let statuses = status_messages(&result);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].id, observations[0].status_message_id);
    let tool_index = result
        .messages()
        .iter()
        .position(|message| matches!(message, MessageBlock::Tool(_)))
        .expect("canonical ToolResult");
    let status_index = result
        .messages()
        .iter()
        .position(|message| message.id() == &observations[0].status_message_id)
        .expect("canonical Agent Status");
    assert!(
        tool_index < status_index,
        "status follows the settled tool batch"
    );
    assert_eq!(
        store
            .read_events(None, 32)
            .expect("event history")
            .events
            .iter()
            .filter(|event| matches!(event.event, RuntimeEvent::AgentStatusEmitted { .. }))
            .count(),
        1,
        "the Todo status has one suppression fact"
    );

    let durable_start_facts = store
        .read_events(None, 32)
        .expect("event history")
        .events
        .into_iter()
        .filter(|event| {
            matches!(
                event.event,
                RuntimeEvent::ModelRequestStarted { .. } | RuntimeEvent::AgentStatusEmitted { .. }
            )
        })
        .map(|event| event.event)
        .collect::<Vec<_>>();
    let live_start_facts = recorder
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::ModelRequestStarted { .. } | RuntimeEvent::AgentStatusEmitted { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        live_start_facts, durable_start_facts,
        "the live observer receives every newly committed start-owned fact in durable order"
    );
    assert!(matches!(
        live_start_facts.as_slice(),
        [
            RuntimeEvent::ModelRequestStarted { .. },
            RuntimeEvent::ModelRequestStarted { .. },
            RuntimeEvent::AgentStatusEmitted { .. },
        ]
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sibling_completion_order_cannot_split_post_tool_opportunity() {
    let todo_call = ScriptedCall {
        id: "call-todo",
        tool_id: TODO_TOOL_ID,
        name: "todo",
        arguments: serde_json::json!({
            "action": "create",
            "subject": "Keep the plan visible"
        }),
    };
    let call_a = scripted("call-a", "tool-a", "a");
    let call_b = scripted("call-b", "tool-b", "b");
    let model = fake_model(tool_turn_then_stop(&[
        todo_call,
        call_a.clone(),
        call_b.clone(),
    ]));
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "a",
            "tool-a",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    let (tool_b, release_b) = FakeTool::parking(
        common::tool_policies(
            "b",
            "tool-b",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let mut started_a = tool_a.started();
    let mut started_b = tool_b.started();
    let mut completed_b = tool_b.completed();
    let fixture = common::native_fixture();
    let mut tools = fixture.registry.clone();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);

    let controller = tokio::spawn(async move {
        await_started(&mut started_a, "A started").await;
        await_started(&mut started_b, "B started").await;
        release_b.send_replace(true);
        completed_b
            .wait_for(|order| order.iter().any(|name| name == "b"))
            .await
            .expect("B completes first");
        release_a.send_replace(true);
    });

    let (result, recorder) = run_attempt(
        "conv-m5",
        "attempt-parallel",
        model.clone(),
        tools,
        vec![MessageBlock::User(inbound("bootstrap", "work"))],
        InitialTurnTrigger::Continuation,
        &fixture.runtime,
    )
    .await;
    controller.await.expect("completion-order controller");

    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(model.requests().len(), 2);
    assert_eq!(
        tool_message_ids(&result),
        vec!["call-todo", "call-a", "call-b"]
    );
    let observations = recorder.observations();
    assert_eq!(
        observations.len(),
        1,
        "the complete sibling batch is one opportunity"
    );
    assert!(observations[0].opportunities.post_tool_batch.is_some());
    let status_index = result
        .messages()
        .iter()
        .position(|message| message.id() == &observations[0].status_message_id)
        .expect("canonical Agent Status");
    let tool_indices = result
        .messages()
        .iter()
        .enumerate()
        .filter_map(|(index, message)| matches!(message, MessageBlock::Tool(_)).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(tool_indices.len(), 3);
    assert!(tool_indices.into_iter().all(|index| index < status_index));
}

#[tokio::test]
async fn fresh_inbound_and_post_tool_batch_are_one_combined_set() {
    let call = scripted("call-combined", "tool-combined", "combined");
    let model = fake_model(tool_turn_then_stop(std::slice::from_ref(&call)));
    let tool = FakeTool::new(
        common::tool_policies(
            "combined",
            "tool-combined",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("combined"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let fixture = common::tool_runtime("issue-130-combined");
    fixture
        .mailbox()
        .enqueue(support::fake::inbound_message(
            "fresh-inbound",
            "new work",
            UserSource::Human,
        ))
        .expect("enqueue inbound before the batch boundary");

    let (result, recorder) = run_attempt(
        "issue-130-combined",
        "attempt-combined",
        model.clone(),
        tools,
        vec![MessageBlock::User(inbound("bootstrap", "work"))],
        InitialTurnTrigger::Continuation,
        &fixture,
    )
    .await;

    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(model.requests().len(), 2);
    let observations = recorder.observations();
    assert_eq!(
        observations.len(),
        1,
        "simultaneous opportunities compose once"
    );
    assert_eq!(
        observations[0]
            .opportunities
            .fresh_inbound
            .as_ref()
            .expect("FreshInbound opportunity")
            .target_message_id,
        MessageId::new("fresh-inbound")
    );
    assert!(observations[0].opportunities.post_tool_batch.is_some());
    assert_eq!(status_messages(&result).len(), 1);
    assert!(result.messages().iter().any(|message| {
        matches!(message, MessageBlock::User(user) if user.id == MessageId::new("fresh-inbound"))
    }));
}

#[tokio::test]
async fn committed_todo_state_emits_one_bounded_post_tool_reminder() {
    let call = ScriptedCall {
        id: "call-todo",
        tool_id: TODO_TOOL_ID,
        name: "todo",
        arguments: serde_json::json!({
            "action": "create",
            "subject": "Write the parser"
        }),
    };
    let model = fake_model(tool_turn_then_stop(std::slice::from_ref(&call)));
    let fixture = common::native_fixture();
    let (result, recorder) = run_attempt(
        "conv-m5",
        "attempt-todo-status",
        model.clone(),
        fixture.registry.clone(),
        vec![MessageBlock::User(inbound("bootstrap", "plan the work"))],
        InitialTurnTrigger::Continuation,
        &fixture.runtime,
    )
    .await;

    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(model.requests().len(), 2);
    assert_eq!(fixture.runtime.todo_snapshot().tasks.len(), 1);
    let observations = recorder.observations();
    assert_eq!(observations.len(), 1);
    assert!(observations[0].opportunities.post_tool_batch.is_some());
    let todo = observations[0]
        .status
        .sections
        .iter()
        .find_map(|section| match &section.data {
            rustx::context::AgentStatusSectionData::Todo { presentation } => Some(presentation),
            _ => None,
        })
        .expect("committed Todo status section");
    assert_eq!(todo.active_count, 1);
    assert_eq!(todo.tasks.len(), 1);
    assert_eq!(todo.tasks[0].subject, "Write the parser");

    let head = fixture
        .store
        .latest_agent_status_emission(AgentStatusModuleId::Todo, TODO_STATUS_EMISSION_KEY)
        .expect("latest Todo emission lookup")
        .expect("Todo emission committed with model start");
    assert_eq!(head.canonical_message_id, observations[0].status_message_id);
    assert_eq!(head.module_id, AgentStatusModuleId::Todo);
    assert_eq!(head.key, TODO_STATUS_EMISSION_KEY);
    assert!(!head.fingerprint.is_empty());
    assert_eq!(
        common::read_event_history(fixture.store.as_ref(), &result.attempt_id)
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AgentStatusEmitted { .. }))
            .count(),
        1,
        "one Todo emission fact settles with the one status message"
    );
}
