//! Issue #256: the closed launch-scoped Native Agent Extension composition.
//!
//! This domain owns the *composition* contracts of optional Agent
//! augmentation: what an empty extension set does to an ordinary runtime,
//! and that a composed extension still reaches the model through its real
//! native owner. It deliberately does not re-prove the Agent Status module
//! semantics owned by [`super::agent::status`] and
//! [`super::background`]; it proves that migrating Agent Status under
//! `extensions` changed neither.
//!
//! Every interleaving here is decided by explicit gates — watches, channels,
//! and the registry's own authoritative state — never by a sleep.

use super::{common, support};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::TimeZone;
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionObserver, AgentExecutionRequest,
    AgentExecutionResult, AgentStatusObservation,
};
use rustx::context::{
    AgentStatusClock, ContextRuntime, DefaultTokenEstimator, SessionContextPolicy,
};
use rustx::durable::ConversationStore;
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::extensions::NativeAgentExtensions;
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContextKind, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{
    AgentId, AttemptId, MessageId, ToolCallId, ToolExecutionId, ToolId,
};
use rustx::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger};
use rustx::runtime::types::CancellationReason;
use rustx::tools::background::{
    BackgroundDispatchOutcome, BackgroundLifecycle, ConversationBackgroundRegistry,
};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocation, ToolInvocationMode,
};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, success_result, tool_call_events,
};

/// The one authored surface under test. Every composition in this suite is
/// produced by parsing the real public document and freezing it, never by
/// constructing an internal configuration value directly: the contract is
/// the *public* extension surface.
fn composition(document: serde_json::Value) -> NativeAgentExtensions {
    serde_json::from_value::<rustx::extensions::NativeAgentExtensionsDocument>(document)
        .expect("the closed extension document parses")
        .resolve()
}

#[derive(Debug, Clone, Copy)]
struct FixedStatusClock;

impl AgentStatusClock for FixedStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 9, 10, 9, 30, 0)
            .single()
            .expect("fixed status timestamp")
    }
}

#[derive(Debug, Default)]
struct Recorder {
    observations: Mutex<Vec<AgentStatusObservation>>,
    events: Mutex<Vec<RuntimeEvent>>,
}

impl Recorder {
    fn observations(&self) -> Vec<AgentStatusObservation> {
        self.observations.lock().expect("observations").clone()
    }

    fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("events").clone()
    }
}

impl AgentExecutionObserver for Recorder {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        self.events.lock().expect("events").push(event.clone());
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
            .expect("observations")
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
        // A fresh inbound turn's referenced messages must carry persisted
        // timestamps; the Agent Loop validates that independently of any
        // extension.
        timestamp: Some(
            chrono::Utc
                .with_ymd_and_hms(2026, 9, 10, 9, 0, 0)
                .single()
                .expect("fixed inbound timestamp"),
        ),
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

fn tool_turn(calls: &[ScriptedCall]) -> Vec<FakeStep> {
    let mut steps = vec![FakeStep::Emit(ModelEvent::Started)];
    for (index, call) in calls.iter().enumerate() {
        steps.extend(
            tool_call_events(u32::try_from(index).expect("block index"), call)
                .into_iter()
                .map(FakeStep::Emit),
        );
    }
    steps.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    steps
}

fn stop_turn() -> Vec<FakeStep> {
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
    ]
}

fn context_runtime(model: &Arc<FakeModel>, extensions: &NativeAgentExtensions) -> ContextRuntime {
    let snapshot = support::attempt_model(model.clone(), "ext256-model");
    ContextRuntime::for_attempt(
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(DefaultTokenEstimator),
        // The one materialization seam under test: the frozen composition
        // decides whether this attempt owns a status engine at all.
        extensions.agent_status_engine(Arc::new(FixedStatusClock)),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("valid context runtime")
}

/// One complete scripted attempt: a fresh inbound turn (the `FreshInbound`
/// opportunity) followed by a settled foreground tool batch (the
/// `PostToolBatch` opportunity) and a stop.
async fn run(
    extensions: &NativeAgentExtensions,
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    tools: ToolRegistry,
    model: Arc<FakeModel>,
) -> (AgentExecutionResult, Recorder) {
    let capability = common::capability_lease(tools, tool_runtime).await;
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("ext256-agent"),
        conversation_id: tool_runtime.conversation_id().clone(),
        attempt_id: AttemptId::new("ext256-attempt"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(inbound("ext256-inbound", "work")),
        ])
        .expect("canonical conversation"),
        initial_turn_trigger: InitialTurnTrigger::FreshInbound(
            FreshInboundTurn::new(vec![MessageId::new("ext256-inbound")]).expect("fresh turn"),
        ),
        model: support::attempt_model(model.clone(), "ext256-model"),
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut execution = AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context_runtime(&model, extensions),
        tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    let recorder = Recorder::default();
    execution.observe(&recorder);
    (execution.run().await, recorder)
}

/// The ordinary, extension-independent shape of one attempt: every fact the
/// Agent Loop owns, with Agent Status deliberately projected out.
#[derive(Debug, PartialEq, Eq)]
struct OrdinarySemantics {
    outcome: String,
    request_count: usize,
    canonical_kinds: Vec<String>,
    tool_call_ids: Vec<String>,
    tool_statuses: Vec<String>,
    events: Vec<String>,
}

fn is_agent_status(message: &MessageBlock) -> bool {
    matches!(
        message,
        MessageBlock::User(user)
            if matches!(&user.kind, InboundKind::Context(ContextKind::AgentStatus(_)))
    )
}

fn ordinary_semantics(
    result: &AgentExecutionResult,
    store: &dyn ConversationStore,
) -> OrdinarySemantics {
    OrdinarySemantics {
        outcome: format!("{:?}", result.outcome),
        request_count: 0,
        canonical_kinds: result
            .messages()
            .iter()
            .filter(|message| !is_agent_status(message))
            .map(|message| match message {
                MessageBlock::User(user) => format!("user:{:?}", user.kind),
                MessageBlock::Assistant(_) => "assistant".to_owned(),
                MessageBlock::Tool(tool) => format!("tool:{}", tool.tool_call_id),
            })
            .collect(),
        tool_call_ids: result
            .messages()
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Tool(tool) => Some(tool.tool_call_id.to_string()),
                _ => None,
            })
            .collect(),
        tool_statuses: result
            .messages()
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Tool(tool) => Some(format!("{:?}", tool.result.status)),
                _ => None,
            })
            .collect(),
        // The durable Event Journal, with the two Agent-Status-owned fact
        // kinds removed. Everything else — admission, request lifecycle,
        // tool lifecycle, settlement, terminal ordering — must match
        // exactly, in order.
        events: store
            .read_events(None, 256)
            .expect("event history")
            .events
            .into_iter()
            .map(|record| record.event)
            .filter(|event| !matches!(event, RuntimeEvent::AgentStatusEmitted { .. }))
            .map(|event| {
                // The stable serde discriminant, which is the wire identity
                // of the fact — not a Debug rendering that would also carry
                // per-run identities.
                serde_json::to_value(&event)
                    .expect("a runtime event serializes")
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .expect("every runtime event carries its tag")
                    .to_owned()
            })
            .collect(),
    }
}

fn status_texts(result: &AgentExecutionResult) -> Vec<String> {
    result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::User(user)
                if matches!(
                    &user.kind,
                    InboundKind::Context(ContextKind::AgentStatus(_))
                ) =>
            {
                user.content.first().and_then(|block| match block {
                    UserContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

/// Issue #256 regressions 1 and 10.
///
/// The same scripted attempt runs twice against two frozen compositions:
/// one containing the Agent Status extension, one containing nothing at all.
/// The empty composition must produce a completely ordinary, fully
/// functioning runtime — the *only* observable difference being the absence
/// of Agent Status.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext256_an_empty_extension_composition_changes_nothing_but_agent_status() {
    let call = scripted("ext256-call", "ext256-tool", "worker");
    let script = vec![tool_turn(std::slice::from_ref(&call)), stop_turn()];

    let mut outcomes = Vec::new();
    let mut status_counts = Vec::new();
    let mut status_events = Vec::new();
    for document in [
        serde_json::json!({"agentStatus": {"enabled": true}}),
        serde_json::json!({"agentStatus": {"enabled": false}}),
    ] {
        let extensions = composition(document);
        let fixture = common::native_fixture();
        let tool = FakeTool::new(
            common::tool_policies(
                "worker",
                "ext256-tool",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
            ),
            success_result("worker"),
        );
        let mut tools = fixture.registry.clone();
        tool.register(&mut tools);
        let model = fake_model(script.clone());
        let (result, recorder) = run(&extensions, &fixture.runtime, tools, model.clone()).await;

        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "both compositions settle the attempt normally: {:?} {:?}",
            result.outcome,
            result.messages()
        );
        let mut semantics = ordinary_semantics(&result, fixture.store.as_ref());
        semantics.request_count = model.requests().len();
        outcomes.push(semantics);
        status_counts.push(recorder.observations().len());
        status_events.push(
            recorder
                .events()
                .into_iter()
                .filter(|event| matches!(event, RuntimeEvent::AgentStatusEmitted { .. }))
                .count(),
        );
    }

    assert!(
        status_counts[0] > 0,
        "the composed extension still contributes Agent Status"
    );
    assert_eq!(
        status_counts[1], 0,
        "an empty extension composition emits no Agent Status observation"
    );
    assert_eq!(
        status_events[1], 0,
        "an empty extension composition commits no Agent Status durable fact"
    );
    assert_eq!(
        outcomes[0], outcomes[1],
        "admission, request lifecycle, tool lifecycle, settlement, terminal \
         ordering, and canonical history are identical with and without the \
         extension"
    );
}

/// Issue #256 regression 1, at the canonical-history level: with no Agent
/// Status extension composed there is no Agent Status message anywhere —
/// not in the attempt result, not in the durable Ledger, and not in any
/// model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext256_a_disabled_agent_status_extension_emits_nothing_anywhere() {
    let extensions = composition(serde_json::json!({"agentStatus": {"enabled": false}}));
    assert!(
        extensions.agent_status().is_none(),
        "this regression is about the Agent Status extension only; Todo composes \
         independently of it (Issue #259)"
    );
    let call = scripted("ext256-none", "ext256-tool", "worker");
    let fixture = common::native_fixture();
    let tool = FakeTool::new(
        common::tool_policies(
            "worker",
            "ext256-tool",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("worker"),
    );
    let mut tools = fixture.registry.clone();
    tool.register(&mut tools);
    let model = fake_model(vec![tool_turn(std::slice::from_ref(&call)), stop_turn()]);
    let (result, recorder) = run(&extensions, &fixture.runtime, tools, model.clone()).await;

    assert!(status_texts(&result).is_empty());
    assert!(recorder.observations().is_empty());
    assert!(
        !result.messages().iter().any(is_agent_status),
        "canonical history carries no Agent Status fact"
    );
    for request in model.requests() {
        assert!(
            !request
                .messages
                .iter()
                .any(|message| message.as_canonical().is_some_and(is_agent_status)),
            "no model request carries an Agent Status context fact"
        );
    }
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
}

/// Issue #256 regression 2: a composed Agent Status extension preserves the
/// existing Time and Background request/context semantics exactly — both
/// contributors, their authored configuration, their semantic order, and the
/// Context-Assembly-admitted Runtime context fact that carries them.
///
/// One detached background execution is committed on the conversation's own
/// authoritative registry *before* the attempt starts, and the test waits on
/// that registry's own Running state, so the Background contributor
/// provably has active state to report at the attempt's first fresh primary
/// step. Nothing here waits on a clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ext256_a_composed_agent_status_extension_preserves_time_and_background_semantics() {
    for (document, expect_time, expect_background) in [
        (
            serde_json::json!({"agentStatus": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": true}
            }}),
            true,
            true,
        ),
        (
            serde_json::json!({"agentStatus": {
                "enabled": true,
                "time": {"enabled": false},
                "background": {"enabled": true}
            }}),
            false,
            true,
        ),
        (
            serde_json::json!({"agentStatus": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": false}
            }}),
            true,
            false,
        ),
    ] {
        let extensions = composition(document);
        let fixture = common::native_fixture();
        let execution_id = seed_detached_execution(&fixture.runtime).await;
        let model = fake_model(vec![stop_turn()]);
        let (result, recorder) = run(
            &extensions,
            &fixture.runtime,
            fixture.registry.clone(),
            model,
        )
        .await;

        assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
        let rendered = status_texts(&result).join("\n");
        assert_eq!(
            rendered.contains("Current time:"),
            expect_time,
            "the Time contributor honors its own configuration: {rendered:?}"
        );
        if expect_time {
            assert!(
                rendered.contains("Asia/Shanghai"),
                "the configured timezone survives the migration: {rendered:?}"
            );
        }
        assert_eq!(
            rendered.contains("Background executions:"),
            expect_background,
            "the Background contributor honors its own configuration: {rendered:?}"
        );
        if expect_background {
            assert!(
                rendered.contains(execution_id.as_str()),
                "the active execution identity is reported: {rendered:?}"
            );
        }
        if expect_time && expect_background {
            assert!(
                rendered.find("Current time:") < rendered.find("Background executions:"),
                "semantic contributor order stays Time then Background: {rendered:?}"
            );
        }
        let observation = recorder
            .observations()
            .into_iter()
            .next()
            .expect("the composed extension contributes one status generation");
        assert!(
            observation.opportunities.fresh_inbound.is_some(),
            "the FreshInbound opportunity still owns the generation"
        );
        assert!(
            observation.opportunities.post_tool_batch.is_none(),
            "no tool batch settled before this primary step"
        );
        // Whatever the contributors decided, Context Assembly remains the one
        // request-time admission owner: every emitted status is a canonical
        // Runtime context User fact, never a second message authority.
        for message in result.messages().iter().filter(|m| is_agent_status(m)) {
            let MessageBlock::User(user) = message else {
                unreachable!("an Agent Status message is a canonical User fact")
            };
            assert_eq!(user.source, UserSource::Runtime);
            assert!(matches!(
                &user.kind,
                InboundKind::Context(ContextKind::AgentStatus(_))
            ));
        }
    }
}

/// Commits one never-settling detached execution on the conversation's own
/// background registry and returns once that registry authoritatively
/// reports it Running.
async fn seed_detached_execution(
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
) -> ToolExecutionId {
    let (tool, _never_released) = FakeTool::parking(
        common::tool_policies(
            "detached",
            "ext256-detached",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("detached"),
    );
    let mut started = tool.started();
    let executor: Arc<dyn rustx::tools::executor::ToolExecutor> = Arc::new(tool);
    let registry = tool_runtime.background();
    let invocation = ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("ext256-detached-call"),
        },
        tool_id: ToolId::new("ext256-detached"),
        tool_name: "detached".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("the detached dispatch prepares");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("the detached dispatch commits ownership");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("the detached dispatch is accepted")
    };
    tokio::time::timeout(Duration::from_secs(30), started.wait_for(|value| *value))
        .await
        .expect("the detached execution starts")
        .expect("the start channel stays open");
    wait_for_running(registry, &execution_id).await;
    execution_id
}

async fn wait_for_running(registry: &ConversationBackgroundRegistry, id: &ToolExecutionId) {
    for _ in 0..2000 {
        if registry
            .snapshot(id)
            .is_some_and(|snapshot| snapshot.state == BackgroundLifecycle::Running)
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the detached execution never reached the authoritative Running state");
}
