//! Deterministic Issue #168 regressions.
//!
//! These tests cover the one provider-independent model stream contract from
//! both sides of the adapter boundary. Adapter fixtures prove that buffered
//! provider generation becomes ephemeral progress, while the in-crate fake
//! drives the real Agent Loop, native tool lifecycle, publication, durable
//! history, and summary path under a manual monotonic clock.

use super::super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::context::{
    AgentStatusEngine, CompactionBudgets, ContextConfig, ContextEngine, ContextRuntime,
    ContextSummarizer, ModelBackedSummarizer, SummaryRequest,
};
use rustx::conversation::ConversationState;
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::adapter::{ModelStreamItem, ModelStreamProgress};
use rustx::model::deadline::{ModelDeadlinePhase, ModelRequestDeadline, ModelTimeoutPolicy};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ModelRequest};
use rustx::model::{
    OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter, ResponsesStorageMode,
};
use rustx::publication::PublicationPayload;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::types::CancellationReason;
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCall, ToolCallStart};
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{FakeModel, FakeStep, fake_model, model_release};

const CONVERSATION: &str = "conv-168";

fn user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn timeout_policy(response_start_ms: u64, stream_idle_ms: u64) -> ModelTimeoutPolicy {
    ModelTimeoutPolicy::new(
        Duration::from_millis(response_start_ms),
        Duration::from_millis(stream_idle_ms),
    )
}

fn canonical_events(items: &[ModelStreamItem]) -> Vec<ModelEvent> {
    items
        .iter()
        .filter_map(|item| match item {
            ModelStreamItem::Event(event) => Some(event.clone()),
            ModelStreamItem::Progress(_) => None,
        })
        .collect()
}

fn progress_items(items: &[ModelStreamItem]) -> Vec<ModelStreamProgress> {
    items
        .iter()
        .filter_map(|item| match item {
            ModelStreamItem::Progress(progress) => Some(*progress),
            ModelStreamItem::Event(_) => None,
        })
        .collect()
}

fn assert_provider_progress_keeps_deadline_alive(items: &[ModelStreamItem]) {
    let clock = ManualMonotonicClock::new();
    let mut deadline = ModelRequestDeadline::new(timeout_policy(100, 10), clock.now_millis());
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            // Every provider item is less than stream-idle apart, while the
            // cumulative fixture duration crosses that timeout.
            clock.advance(6);
        }
        if matches!(
            item,
            ModelStreamItem::Event(ModelEvent::Completed { .. } | ModelEvent::Failed { .. })
        ) {
            break;
        }
        deadline.observe(item, clock.now_millis());
        assert!(
            deadline
                .deadline_millis()
                .is_some_and(|deadline_millis| deadline_millis > clock.now_millis())
        );
    }
    assert!(
        clock.now_millis() > 10,
        "fixture crosses stream-idle cumulatively"
    );
    assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
}

fn request_with_tools(protocol: ModelProtocol) -> ModelRequest {
    let mut request = common::simple_request(protocol, "gpt-test", "List the directory");
    request.tools = vec![common::model_tool("list_directory", "tool-list")];
    if protocol == ModelProtocol::OpenAiResponses {
        request.invocation.compat.responses_storage = ResponsesStorageMode::Stored;
    }
    request
}

/// The same small context runtime used by the deadline regressions. The
/// summary implementation is inert because these tests do not trigger
/// compaction in the primary large-tool scenario.
fn context_runtime() -> ContextRuntime {
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(10, 10, 10)),
    )
    .expect("valid context configuration");
    ContextRuntime::with_scripted_summarizer(
        engine,
        Arc::new(FakeContextSummarizer::new(Vec::<FakeSummaryStep>::new())),
        AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

fn request(
    model: &Arc<FakeModel>,
    conversation_id: &str,
    attempt_id: &str,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-168"),
        conversation_id: ConversationId::new(conversation_id),
        attempt_id: AttemptId::new(attempt_id),
        conversation: ConversationState::from_messages(vec![user("seed", "hello")])
            .expect("valid fixture conversation"),
        initial_turn_trigger: rustx::runtime::inbound::InitialTurnTrigger::Continuation,
        model: support::attempt_model_with_window(model.clone(), "fake-model-168", 10_000_000, 512),
    }
}

struct ExecutionSetup<'a> {
    model: &'a Arc<FakeModel>,
    conversation_id: &'a str,
    attempt_id: &'a str,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a rustx::tools::runtime::ConversationToolRuntime,
    policy: ModelTimeoutPolicy,
    clock: Arc<ManualMonotonicClock>,
    tools: ToolRegistry,
}

async fn make_execution(setup: ExecutionSetup<'_>) -> AgentExecution<'_> {
    let ExecutionSetup {
        model,
        conversation_id,
        attempt_id,
        cancellation,
        tool_runtime,
        policy,
        clock,
        tools,
    } = setup;
    let capability = common::capability_lease(tools, tool_runtime).await;
    let mut execution = AgentExecution::new(
        request(model, conversation_id, attempt_id),
        capability.into_lease(),
        cancellation,
        crate::agent::execution::AgentExecutionRuntimePolicy {
            subagent_context: None,
            workflow_output: None,
            model_timeout_policy: policy,
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            monotonic_clock: Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        },
        context_runtime(),
        tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_model_timeout_policy(policy);
    execution
}

#[tokio::test]
async fn chat_pre_identity_arguments_are_generation_progress_and_replayed_once() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "tool_arguments_before_identity_many.sse")
    })
    .await;
    let items = common::collect_items(
        &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1"))),
        request_with_tools(ModelProtocol::OpenAiChatCompletions),
    )
    .await;
    assert_provider_progress_keeps_deadline_alive(&items);
    assert_eq!(
        progress_items(&items),
        vec![
            ModelStreamProgress::Generation,
            ModelStreamProgress::Generation,
            ModelStreamProgress::Generation,
        ]
    );
    let first_canonical_tool_item = items
        .iter()
        .position(|item| {
            matches!(
                item,
                ModelStreamItem::Event(ModelEvent::ToolCallStarted { .. })
            )
        })
        .expect("identity eventually makes the tool call canonical");
    assert!(
        items[..first_canonical_tool_item]
            .iter()
            .all(|item| !matches!(
                item,
                ModelStreamItem::Event(ModelEvent::ToolCallArgumentsDelta { .. })
            ))
    );

    let events = canonical_events(&items);
    assert!(matches!(
        &events[1],
        ModelEvent::ToolCallStarted { call, .. }
            if call.id == ToolCallId::new("call-late-many") && call.name == "list_directory"
    ));
    assert!(matches!(
        &events[2],
        ModelEvent::ToolCallArgumentsDelta { arguments_delta, .. }
            if arguments_delta == "{\"path\":\".\"}"
    ));
    assert!(matches!(
        &events[3],
        ModelEvent::ToolCallCompleted { call, .. }
            if call.id == ToolCallId::new("call-late-many")
                && call.arguments == serde_json::json!({"path": "."})
    ));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            ..
        })
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelEvent::ToolCallStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelEvent::ToolCallArgumentsDelta { .. }))
            .count(),
        1
    );
    assert_eq!(server.attempt_count(), 1, "the adapter never retries");
}

#[tokio::test]
async fn chat_snapshot_identity_replays_an_existing_buffered_prefix_once() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "tool_arguments_before_snapshot_identity.sse")
    })
    .await;
    let items = common::collect_items(
        &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1"))),
        request_with_tools(ModelProtocol::OpenAiChatCompletions),
    )
    .await;
    assert_eq!(
        progress_items(&items),
        vec![ModelStreamProgress::Generation],
        "the buffered prefix is the only noncanonical provider generation"
    );
    let events = canonical_events(&items);
    assert!(matches!(
        &events[1],
        ModelEvent::ToolCallStarted { call, .. }
            if call.id == ToolCallId::new("call-snapshot-late")
                && call.name == "list_directory"
    ));
    assert!(matches!(
        &events[2],
        ModelEvent::ToolCallArgumentsDelta { arguments_delta, .. }
            if arguments_delta == "{\"path\":\".\"}"
    ));
    assert!(matches!(
        &events[3],
        ModelEvent::ToolCallCompleted { call, .. }
            if call.arguments == serde_json::json!({"path": "."})
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelEvent::ToolCallStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelEvent::ToolCallArgumentsDelta { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn responses_pre_start_arguments_are_generation_progress_and_replayed_once() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "pre_start_tool_arguments.sse")
    })
    .await;
    let items = common::collect_items(
        &OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1"))),
        request_with_tools(ModelProtocol::OpenAiResponses),
    )
    .await;

    assert_provider_progress_keeps_deadline_alive(&items);
    assert_eq!(
        progress_items(&items),
        vec![
            ModelStreamProgress::Liveness,
            ModelStreamProgress::Generation,
            ModelStreamProgress::Generation,
            ModelStreamProgress::Generation,
            ModelStreamProgress::Generation,
        ]
    );
    let events = canonical_events(&items);
    assert!(matches!(
        &events[1],
        ModelEvent::ToolCallStarted { call, .. }
            if call.id == ToolCallId::new("call_late_responses")
                && call.name == "list_directory"
    ));
    assert!(matches!(
        &events[2],
        ModelEvent::ToolCallArgumentsDelta { arguments_delta, .. }
            if arguments_delta == "{\"path\":\".\"}"
    ));
    assert!(matches!(
        &events[3],
        ModelEvent::ToolCallCompleted { call, .. }
            if call.id == ToolCallId::new("call_late_responses")
                && call.arguments == serde_json::json!({"path": "."})
    ));
    assert!(
        matches!(
            events.last(),
            Some(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                ..
            })
        ),
        "canonical Responses events: {events:#?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelEvent::ToolCallStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelEvent::ToolCallArgumentsDelta { .. }))
            .count(),
        1
    );
    assert_eq!(server.attempt_count(), 1, "the adapter never retries");
}

#[tokio::test]
async fn responses_function_call_arguments_done_refreshes_stream_idle_after_start() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "pre_start_tool_arguments.sse")
    })
    .await;
    let items = common::collect_items(
        &OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1"))),
        request_with_tools(ModelProtocol::OpenAiResponses),
    )
    .await;

    let tool_started_index = items
        .iter()
        .position(|item| {
            matches!(
                item,
                ModelStreamItem::Event(ModelEvent::ToolCallStarted { .. })
            )
        })
        .expect("Responses tool call starts canonically");
    let done_progress_index = items
        .iter()
        .enumerate()
        .skip(tool_started_index + 1)
        .find_map(|(index, item)| {
            matches!(
                item,
                ModelStreamItem::Progress(ModelStreamProgress::Generation)
            )
            .then_some(index)
        })
        .expect("function_call_arguments.done is visible as post-start progress");

    let clock = ManualMonotonicClock::new();
    let mut deadline = ModelRequestDeadline::new(timeout_policy(100, 10), clock.now_millis());
    for item in &items[..done_progress_index] {
        deadline.observe(item, clock.now_millis());
    }
    assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
    assert_eq!(deadline.deadline_millis(), Some(10));

    // The provider's known finalization event arrives just before the old
    // stream-idle boundary. Its ephemeral Generation item must move that
    // boundary forward even though it emits no canonical argument delta.
    clock.advance(9);
    deadline.observe(&items[done_progress_index], clock.now_millis());
    assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
    assert_eq!(deadline.deadline_millis(), Some(19));
}

#[tokio::test]
async fn anthropic_ping_is_liveness_but_unknown_events_are_not() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("anthropic", "unknown_events.sse")
    })
    .await;
    let items = common::collect_items(
        &rustx::model::AnthropicMessagesAdapter::new(rustx::model::AnthropicAdapterConfig::new(
            "test-key",
            server.url(""),
        )),
        common::simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_eq!(
        progress_items(&items),
        vec![ModelStreamProgress::Liveness],
        "the future_event is deliberately not a generic heartbeat"
    );
    assert!(canonical_events(&items).iter().any(|event| matches!(
        event,
        ModelEvent::TextDelta { text, .. } if text == "Still works."
    )));
}

#[test]
fn explicit_progress_preserves_response_start_and_stream_idle_semantics() {
    let policy = timeout_policy(10, 20);
    let clock = ManualMonotonicClock::new();
    let mut deadline = ModelRequestDeadline::new(policy, clock.now_millis());
    deadline.observe(
        &ModelStreamItem::Event(ModelEvent::Started),
        clock.now_millis(),
    );

    clock.advance(5);
    deadline.observe(
        &ModelStreamItem::Progress(ModelStreamProgress::Liveness),
        clock.now_millis(),
    );
    assert_eq!(deadline.phase(), ModelDeadlinePhase::AwaitingGeneration);
    assert_eq!(deadline.deadline_millis(), Some(10));

    deadline.observe(
        &ModelStreamItem::Progress(ModelStreamProgress::Generation),
        clock.now_millis(),
    );
    assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
    assert_eq!(deadline.deadline_millis(), Some(25));

    clock.advance(5);
    deadline.observe(
        &ModelStreamItem::Progress(ModelStreamProgress::Liveness),
        clock.now_millis(),
    );
    assert_eq!(deadline.deadline_millis(), Some(30));

    deadline.observe(
        &ModelStreamItem::Event(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
        clock.now_millis(),
    );
    assert_eq!(deadline.phase(), ModelDeadlinePhase::Terminal);
    assert_eq!(deadline.deadline_millis(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn true_silence_after_generation_times_out_once_without_adapter_retry() {
    let (release, release_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Progress(ModelStreamProgress::Generation),
        FakeStep::ParkUntilReleased(release_rx),
    ]]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let clock = Arc::new(ManualMonotonicClock::new());
    let policy = timeout_policy(100, 10);
    let (pause, mut reached, pause_release) =
        crate::agent::execution::test_sync::ModelStreamItemPause::install();
    let mut execution = make_execution(ExecutionSetup {
        model: &model,
        conversation_id: CONVERSATION,
        attempt_id: "attempt-168-silence",
        cancellation: &cancellation,
        tool_runtime: &tool_runtime,
        policy,
        clock: clock.clone(),
        tools: ToolRegistry::new(),
    })
    .await;
    execution.install_model_stream_item_pause(pause);

    let controller_clock = clock.clone();
    let controller_cancellation = cancellation.clone();
    let mut parked = model.parked();
    let mut exited = model.streams_exited();
    let controller = tokio::spawn(async move {
        for count in 1..=2 {
            reached
                .wait_for(|observed| *observed >= count)
                .await
                .expect("stream item pause remains open");
            pause_release.send(()).expect("release model item");
        }
        parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("provider reaches true silence");
        controller_clock.advance(11);
        exited
            .wait_for(|count| *count >= 1)
            .await
            .expect("timed-out stream exits");
        // Stop the Agent Loop's ordinary transient retry before a second
        // request is admitted. The timeout itself never touches this signal.
        controller_cancellation.cancel();
        drop(release);
    });
    let publication = common::RecordingPublicationObserver::default();
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        execution.observe(&publication);
        common::durable_agent_result_with_publication(
            execution.run().await,
            tool_runtime.durable_store().as_ref(),
            &publication,
        )
    })
    .await
    .expect("true-silence timeout must settle");
    controller.await.expect("silence controller completes");

    assert_eq!(model.requests().len(), 1, "the adapter was never retried");
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. }))
            .count(),
        1
    );
    assert!(audit.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestFailed { error, .. }
            if error.kind == rustx::model::ModelErrorKind::Timeout
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_write_progress_crosses_idle_timeout_without_retry_or_duplicate_execution() {
    const GENERATION_GAP_MS: u64 = 6;
    const STREAM_IDLE_MS: u64 = 10;

    let fixture = common::native_fixture();
    let content = "0123456789abcdef".repeat(4096);
    let arguments = serde_json::json!({
        "path": "large.txt",
        "content": content,
    });
    let arguments_json = serde_json::to_string(&arguments).expect("serialize write arguments");
    let call = ToolCall {
        id: ToolCallId::new("call-write-168"),
        tool_id: ToolId::new("tool-write"),
        name: "write".to_owned(),
        arguments: arguments.clone(),
    };
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Progress(ModelStreamProgress::Generation),
            FakeStep::Progress(ModelStreamProgress::Generation),
            FakeStep::Progress(ModelStreamProgress::Generation),
            FakeStep::Progress(ModelStreamProgress::Generation),
            FakeStep::Progress(ModelStreamProgress::Generation),
            FakeStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            }),
            FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                arguments_delta: arguments_json,
            }),
            FakeStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call,
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let clock = Arc::new(ManualMonotonicClock::new());
    let policy = timeout_policy(100, STREAM_IDLE_MS);
    let (pause, mut reached, pause_release) =
        crate::agent::execution::test_sync::ModelStreamItemPause::install();
    let mut execution = make_execution(ExecutionSetup {
        model: &model,
        conversation_id: fixture.runtime.conversation_id().as_str(),
        attempt_id: "attempt-168-write",
        cancellation: &cancellation,
        tool_runtime: &fixture.runtime,
        policy,
        clock: clock.clone(),
        tools: fixture.registry.clone(),
    })
    .await;
    execution.install_model_stream_item_pause(pause);

    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        for count in 1..=13 {
            reached
                .wait_for(|observed| *observed >= count)
                .await
                .expect("large-write item pause remains open");
            // `reached == count` is the linearization point after the current
            // provider item has been processed and before the next provider
            // item can be admitted. Advance the clock while the Agent Loop is
            // held at that point, then release exactly one next item.
            // The five explicit Generation items are six logical milliseconds
            // apart: total generation time is 30ms while stream idle is 10ms.
            if (1..=5).contains(&count) {
                let before = controller_clock.now_millis();
                controller_clock.advance(GENERATION_GAP_MS);
                let interval = controller_clock.now_millis() - before;
                assert_eq!(interval, GENERATION_GAP_MS);
                assert!(interval < STREAM_IDLE_MS);
            }
            pause_release.send(()).expect("release large-write item");
        }
    });
    let publication = common::RecordingPublicationObserver::default();
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        execution.observe(&publication);
        common::durable_agent_result_with_publication(
            execution.run().await,
            fixture.store.as_ref(),
            &publication,
        )
    })
    .await
    .expect("large-write execution must settle");
    controller.await.expect("large-write controller completes");

    assert_eq!(clock.now_millis(), GENERATION_GAP_MS * 5);
    assert!(clock.now_millis() > STREAM_IDLE_MS);
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(
        model.requests().len(),
        2,
        "one tool turn and one continuation"
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
            .count(),
        0,
        "ongoing provider progress never schedules a retry"
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(
                event,
                RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
                    if tool_call_id == &ToolCallId::new("call-write-168")
            ))
            .count(),
        1
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(
                event,
                RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                    if tool_call_id == &ToolCallId::new("call-write-168")
            ))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(fixture.runtime.workspace().root().join("large.txt"))
            .expect("native write output"),
        content
    );

    let canonical_tool_messages = audit
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(
                assistant
                    .content
                    .iter()
                    .filter(|block| {
                        matches!(
                            block,
                            rustx::message::types::AssistantContentBlock::ToolCall(_)
                        )
                    })
                    .count(),
            ),
            _ => None,
        })
        .sum::<usize>();
    assert_eq!(
        canonical_tool_messages, 1,
        "one canonical tool call in history"
    );

    let started_frames = audit
        .publication_frames
        .iter()
        .filter(|frame| {
            matches!(
                &frame.payload,
                PublicationPayload::ProposedToolCallStarted { call, .. }
                    if call.id == ToolCallId::new("call-write-168")
            )
        })
        .count();
    let argument_frames = audit
        .publication_frames
        .iter()
        .filter_map(|frame| match &frame.payload {
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                call_id, suffix, ..
            } if call_id == &ToolCallId::new("call-write-168") => Some(suffix.as_str()),
            _ => None,
        })
        .collect::<String>();
    let completed_frames = audit
        .publication_frames
        .iter()
        .filter(|frame| {
            matches!(
                &frame.payload,
                PublicationPayload::ProposedToolCallCompleted { call, .. }
                    if call.id == ToolCallId::new("call-write-168")
            )
        })
        .count();
    assert_eq!(started_frames, 1);
    assert_eq!(argument_frames, serde_json::to_string(&arguments).unwrap());
    assert_eq!(completed_frames, 1);
    assert!(
        audit
            .event_history
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summarizer_consumes_generation_and_liveness_progress_without_retry() {
    let (release_one, release_one_rx) = model_release();
    let (release_two, release_two_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Progress(ModelStreamProgress::Generation),
        FakeStep::ParkUntilReleased(release_one_rx),
        FakeStep::Progress(ModelStreamProgress::Liveness),
        FakeStep::ParkUntilReleased(release_two_rx),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "summary".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]);
    let invocation =
        support::attempt_model_with_window(model.clone(), "fake-summary-168", 10_000_000, 128)
            .summary_invocation()
            .clone();
    let clock = Arc::new(ManualMonotonicClock::new());
    let summarizer = ModelBackedSummarizer::new(
        invocation,
        timeout_policy(100, 10),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    let mut parks = model.parks();
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        parks
            .wait_for(|count| *count >= 1)
            .await
            .expect("first summary park remains observable");
        controller_clock.advance(6);
        release_one.send_replace(true);
        parks
            .wait_for(|count| *count >= 2)
            .await
            .expect("second summary park remains observable");
        controller_clock.advance(6);
        release_two.send_replace(true);
    });
    let summary = tokio::time::timeout(
        Duration::from_secs(2),
        summarizer.summarize(
            SummaryRequest {
                retired: vec![user("retired", "old")],
            },
            rustx::runtime::CancellationSignal::new(),
        ),
    )
    .await
    .expect("summary progress test must settle")
    .expect("valid generation progress must not fail summary");
    controller.await.expect("summary controller completes");
    assert_eq!(summary, "summary");
    assert_eq!(model.requests().len(), 1, "summarizer has no generic retry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_progress_remains_first_in_simultaneous_timeout_arbitration() {
    let (provider_release, provider_release_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilReleased(provider_release_rx),
        FakeStep::Progress(ModelStreamProgress::Generation),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let clock = Arc::new(ManualMonotonicClock::new());
    let (pause, mut reached, pause_release) =
        crate::agent::execution::test_sync::ModelStreamItemPause::install();
    let (arbitration_pause, mut arbitration_reached, arbitration_release) =
        crate::agent::execution::test_sync::ModelArbitrationPause::install(1);
    let mut execution = make_execution(ExecutionSetup {
        model: &model,
        conversation_id: CONVERSATION,
        attempt_id: "attempt-168-arbitration",
        cancellation: &cancellation,
        tool_runtime: &tool_runtime,
        policy: timeout_policy(10, 100),
        clock: clock.clone(),
        tools: ToolRegistry::new(),
    })
    .await;
    execution.install_model_stream_item_pause(pause);
    execution.install_model_arbitration_pause(arbitration_pause);

    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|observed| *observed >= 1)
            .await
            .expect("Started is processed");
        pause_release.send(()).expect("release Started");
        arbitration_reached
            .wait_for(|entered| *entered)
            .await
            .expect("provider arbitration pause remains open");
        provider_release
            .send(true)
            .expect("release provider progress");
        controller_clock.advance(10);
        arbitration_release
            .send(())
            .expect("release provider-first arbitration");
        reached
            .wait_for(|observed| *observed >= 2)
            .await
            .expect("Generation progress wins and is processed");
        pause_release.send(()).expect("release Generation progress");
        reached
            .wait_for(|observed| *observed >= 3)
            .await
            .expect("Completed is processed");
        pause_release.send(()).expect("release Completed");
    });
    let publication = common::RecordingPublicationObserver::default();
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        execution.observe(&publication);
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref())
    })
    .await
    .expect("provider-first arbitration must settle");
    controller.await.expect("arbitration controller completes");
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(model.requests().len(), 1);
    assert!(
        !audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. }))
    );
}
