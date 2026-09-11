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
        let mut tools = fixture.ordinary_registry.clone();
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
    let mut tools = fixture.ordinary_registry.clone();
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
            fixture.ordinary_registry.clone(),
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

// ===========================================================================
// Issue #259 — Todo as a stateful, Tool-providing Native Agent Extension.
//
// These regressions own the *composition* contracts the migration
// introduced: that composing Todo composes one coherent capability, that
// ordinary Tool selection is a separate authority plane which can neither
// add nor remove the extension's Tool, that Agent Status consumes Todo
// without owning it, and that neither extension changes anything the Agent
// Loop owns.
//
// The Todo state machine, its batch transaction semantics, and its recovery
// from canonical evidence are owned by `super::tools::todo_plane` and
// `super::tools::todo_transaction` and are deliberately not re-proven here.
// ===========================================================================

/// Both extension axes, spelled as the public authored document.
fn todo_and_status(todo: bool, agent_status: bool) -> NativeAgentExtensions {
    composition(serde_json::json!({
        "todo": {"enabled": todo},
        "agentStatus": {"enabled": agent_status},
    }))
}

/// The model-facing Tool names one composition publishes under one ordinary
/// activation policy, and the ordinary *available* catalog beside them.
async fn published_tools(
    extensions: &NativeAgentExtensions,
    policy: rustx::capabilities::ToolActivationPolicy,
) -> (Vec<String>, Vec<String>) {
    let fixture = common::native_fixture_with_extensions(
        Vec::new(),
        rustx::tools::native::NativeToolPolicies::default(),
        extensions,
    );
    // The base registry is the *ordinary* native plane only; the extension
    // plane is composed by the coordinator from the frozen composition.
    let mut ordinary = rustx::tools::executor::ToolRegistry::new();
    rustx::tools::native::register_native_tools(
        &mut ordinary,
        rustx::tools::NativeToolResources {
            subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
            background: fixture.runtime.background().clone(),
            subagents: None,
        },
        rustx::tools::NativeToolPolicies::default(),
    )
    .expect("ordinary native registration");
    let capability = common::capability_lease_with(ordinary, &fixture.runtime, policy).await;
    let snapshot = capability.snapshot().clone();
    let active = snapshot
        .tool_registry()
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let available = snapshot
        .available_tools()
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    (active, available)
}

/// Issue #259 regressions 1, 2 and 4.
///
/// The extension-provided `todo` Tool is composed by `extensions.todo`, and
/// by nothing else:
///
/// ```text
/// Todo on,  every ordinary default        -> todo present
/// Todo on,  defaultTools naming only read -> todo present
/// Todo on,  --tools read (exact)          -> todo present
/// Todo on,  --no-tools                    -> todo present
/// Todo off, every ordinary default        -> todo absent
/// Todo off, --no-tools                    -> no Tool at all
/// ```
///
/// The last two rows are the documented refinement of #234's exact-selection
/// contract: exact ordinary selection stays exact *within the ordinary
/// plane*, and a truly Tool-free model request needs no ordinary Tools and no
/// Tool-providing extension.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_ordinary_tool_selection_neither_adds_nor_removes_the_extension_tool() {
    use rustx::capabilities::ToolActivationPolicy as Selection;

    let enabled = todo_and_status(true, true);
    let disabled = todo_and_status(false, true);

    for policy in [
        Selection::default(),
        Selection {
            default_tools: Some(vec!["read".to_owned()]),
            ..Selection::default()
        },
        Selection {
            tools: Some(vec!["read".to_owned()]),
            ..Selection::default()
        },
        Selection {
            no_tools: true,
            ..Selection::default()
        },
        Selection {
            no_builtin_tools: true,
            ..Selection::default()
        },
    ] {
        let (active, available) = published_tools(&enabled, policy.clone()).await;
        assert!(
            active.contains(&"todo".to_owned()),
            "an enabled Todo extension publishes its Tool under {policy:?}"
        );
        assert!(
            !available.contains(&"todo".to_owned()),
            "and it is never an ordinary available capability, which is what makes it \
             unnameable on every selection surface: {policy:?}"
        );

        let (active, _) = published_tools(&disabled, policy.clone()).await;
        assert!(
            !active.contains(&"todo".to_owned()),
            "a composition without the Todo extension publishes no todo Tool under {policy:?}"
        );
    }

    // The exact shape of the two ends of the range.
    let (no_tools_with_todo, _) = published_tools(
        &enabled,
        Selection {
            no_tools: true,
            ..Selection::default()
        },
    )
    .await;
    assert_eq!(
        no_tools_with_todo,
        vec!["todo".to_owned()],
        "--no-tools selects zero ordinary capabilities and says nothing about an extension"
    );
    let (nothing, _) = published_tools(
        &todo_and_status(false, true),
        Selection {
            no_tools: true,
            ..Selection::default()
        },
    )
    .await;
    assert!(
        nothing.is_empty(),
        "a truly Tool-free request needs no ordinary Tools AND no Tool-providing extension"
    );
}

/// Issue #259 regression 3: `todo` is refused on every ordinary Tool
/// selection surface, with a diagnostic that names the plane it belongs to.
///
/// The surfaces are enumerated from the authoring side, because that is where
/// an author actually meets them: root configuration, the CLI-facing
/// activation policy, and the shared source-qualified selection vocabulary
/// that named-role frontmatter, a Workflow Agent node's override, and the
/// model-facing `subagent` Tool's override all use.
#[test]
fn ext259_todo_is_rejected_on_every_ordinary_selection_surface() {
    use rustx::capabilities::ToolActivationPolicy as Selection;

    // Root configuration.
    let config = serde_json::json!({
        "schemaVersion": 8,
        "agentId": "agent-ext259",
        "model": {"model": "local/model-a"},
        "context": {"reserveTokens": 0, "keepRecentTokens": 0},
        "defaultTools": ["read", "todo"],
    })
    .to_string();
    let error = rustx::local_runtime::CurrentRuntimeConfig::from_jsonc_slice(config.as_bytes())
        .expect_err("defaultTools may not name an extension Tool");
    let rendered = error.to_string();
    assert!(
        rendered.contains("Agent Extension") && rendered.contains("extensions.todo"),
        "the refusal names the owning plane and the way to compose it: {rendered}"
    );

    // The CLI-facing ordinary activation policy, on all three of its lists.
    for policy in [
        Selection {
            default_tools: Some(vec!["todo".to_owned()]),
            ..Selection::default()
        },
        Selection {
            tools: Some(vec!["todo".to_owned()]),
            ..Selection::default()
        },
        Selection {
            exclude_tools: vec!["todo".to_owned()],
            ..Selection::default()
        },
    ] {
        let rendered = policy
            .validate()
            .expect_err("an extension Tool is not an ordinary selector");
        assert!(
            rendered.contains("Agent Extension"),
            "{policy:?} must be refused by name: {rendered}"
        );
    }

    // The one shared trusted selection vocabulary.
    let document: rustx::capabilities::selection::ToolSelectionDocument =
        serde_json::from_value(serde_json::json!({"builtin": ["read", "todo"]}))
            .expect("the selection document parses");
    let rendered = document
        .validate_spelling()
        .expect_err("tools.builtin may not name an extension Tool");
    assert!(
        rendered.contains("Agent Extension") && rendered.contains("extensions.todo"),
        "the shared vocabulary refuses it too: {rendered}"
    );

    // And an ordinary Builtin name remains perfectly ordinary.
    let ordinary: rustx::capabilities::selection::ToolSelectionDocument =
        serde_json::from_value(serde_json::json!({"builtin": ["read"]})).expect("parses");
    assert!(ordinary.validate_spelling().is_ok());
    assert!(
        Selection {
            tools: Some(vec!["read".to_owned()]),
            ..Selection::default()
        }
        .validate()
        .is_ok()
    );
}

/// Issue #259 regression 16: the `todo` Tool definition is stable for the
/// lifetime of a composition.
///
/// Todo list contents are conversation state, not capability state, so
/// creating, completing, and clearing tasks must leave the published Tool
/// definition and the capability revision exactly where they were. A schema
/// that appeared only once the list was non-empty would make the model's
/// capability set a function of its own working state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_the_todo_tool_schema_is_stable_across_list_mutations() {
    use rustx::tools::todo::TodoCreate;

    let fixture = common::native_fixture_with_extensions(
        Vec::new(),
        rustx::tools::native::NativeToolPolicies::default(),
        &NativeAgentExtensions::with_todo(),
    );
    let capability = common::capability_lease_with(
        rustx::tools::executor::ToolRegistry::new(),
        &fixture.runtime,
        rustx::capabilities::ToolActivationPolicy::default(),
    )
    .await;
    let (lease, coordinator) = capability.into_lease_and_coordinator();
    let definition_of = |snapshot: &rustx::capabilities::CapabilitySnapshot| {
        snapshot
            .tool_registry()
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "todo")
            .expect("an enabled Todo extension publishes its Tool")
            .clone()
    };

    let before = definition_of(lease.snapshot());
    let revision_before = lease.revision();
    assert!(
        fixture
            .runtime
            .todo_snapshot()
            .expect("Todo is composed")
            .tasks
            .is_empty(),
        "the Tool exists over an empty list, which is the whole point"
    );

    // Drive the list through its real authority: create, complete, clear.
    let todos = fixture.runtime.todos().expect("Todo is composed");
    for step in 0..3u8 {
        let batch = todos.open_batch().expect("one batch at a time");
        let writer = batch.writer();
        let snapshot = match step {
            0 => {
                writer
                    .create(TodoCreate {
                        subject: "Write the parser".to_owned(),
                        ..TodoCreate::default()
                    })
                    .expect("create")
                    .1
            }
            1 => {
                writer
                    .update(
                        1,
                        rustx::tools::todo::TodoChange {
                            status: Some(rustx::tools::todo::TodoStatus::Completed),
                            ..rustx::tools::todo::TodoChange::default()
                        },
                    )
                    .expect("complete")
                    .1
            }
            _ => writer.clear().expect("clear").1,
        };
        batch.settle(&[common::todo_result_message(
            &format!("mutation-{step}"),
            &snapshot,
        )]);

        assert_eq!(
            definition_of(lease.snapshot()),
            before,
            "a Todo mutation never republishes the Tool definition"
        );
        assert_eq!(
            lease.revision(),
            revision_before,
            "nor does it advance the capability revision"
        );
        assert_eq!(
            coordinator.current_snapshot().revision(),
            revision_before,
            "the coordinator publishes nothing on a Todo state change either"
        );
    }
}

/// Issue #259 regressions 5, 6 and 15: Todo and Agent Status are independent
/// axes, and neither changes anything the Agent Loop owns.
///
/// All four combinations run the *same* scripted attempt — a fresh inbound
/// turn, a settled foreground tool batch, a stop — against a conversation
/// whose committed list already holds actionable work, which is precisely the
/// state a Todo reminder is about. What differs is only what each composition
/// is supposed to produce:
///
/// ```text
/// Todo on,  Status on   the Todo Tool is published, and a Todo section is
///                       admitted into Agent Status
/// Todo on,  Status off  the Todo Tool is published; no Agent Status at all
/// Todo off, Status on   Agent Status runs, with NO Todo section fabricated
/// Todo off, Status on   Time/Background are unaffected by Todo's absence
/// Todo off, Status off  neither
/// ```
///
/// Everything else — admission, request count, canonical history, tool call
/// identities and statuses, the durable Event Journal's ordering, terminal
/// settlement — is compared across all four and must be identical.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn ext259_todo_and_agent_status_are_independent_and_change_no_loop_semantics() {
    use rustx::tools::todo::TodoCreate;

    let call = scripted("ext259-call", "ext259-tool", "worker");
    let script = vec![tool_turn(std::slice::from_ref(&call)), stop_turn()];

    let mut ordinary = Vec::new();
    let mut observed = Vec::new();
    for (todo, agent_status) in [(true, true), (true, false), (false, true), (false, false)] {
        let extensions = todo_and_status(todo, agent_status);
        let fixture = common::native_fixture_with_extensions(
            Vec::new(),
            rustx::tools::native::NativeToolPolicies::default(),
            &extensions,
        );

        // Seed actionable committed work wherever a list exists, through the
        // list's real batch authority and its real canonical evidence.
        if let Some(todos) = fixture.runtime.todos() {
            let batch = todos.open_batch().expect("a fresh list opens one batch");
            let (_, snapshot) = batch
                .writer()
                .create(TodoCreate {
                    subject: "Write the parser".to_owned(),
                    ..TodoCreate::default()
                })
                .expect("create");
            batch.settle(&[common::todo_result_message("seed", &snapshot)]);
            assert_eq!(
                fixture
                    .runtime
                    .todo_snapshot()
                    .expect("Todo is composed")
                    .tasks
                    .len(),
                1
            );
        }

        let tool = FakeTool::new(
            common::tool_policies(
                "worker",
                "ext259-tool",
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
            ),
            success_result("worker"),
        );
        let mut tools = fixture.ordinary_registry.clone();
        tool.register(&mut tools);
        let model = fake_model(script.clone());
        let (result, recorder) = run(&extensions, &fixture.runtime, tools, model.clone()).await;

        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "every combination settles the attempt normally: {:?}",
            result.outcome
        );

        // The Tool half of the composition, read off the registry the attempt
        // actually ran against.
        let todo_tool_published = fixture
            .registry
            .definitions()
            .iter()
            .any(|definition| definition.name == "todo");
        // The status half: the sections Agent Status actually admitted.
        let sections: Vec<String> = recorder
            .observations()
            .iter()
            .flat_map(|observation| observation.status.sections.iter())
            .map(|section| section.id.to_string())
            .collect();

        let mut semantics = ordinary_semantics(&result, fixture.store.as_ref());
        semantics.request_count = model.requests().len();
        ordinary.push(semantics);
        observed.push((todo_tool_published, sections));

        // The list survives the attempt untouched in every composition: this
        // attempt calls `worker`, not `todo`.
        assert_eq!(
            fixture.runtime.todo_snapshot().map(|list| list.tasks.len()),
            todo.then_some(1),
            "a composition composes its list, or does not have one at all"
        );
    }

    let [both, todo_only, status_only, neither] = <[_; 4]>::try_from(observed).expect("four runs");

    // Todo on, Agent Status on: both halves present.
    assert!(both.0, "an enabled Todo extension publishes its Tool");
    assert!(
        both.1.iter().any(|id| id == "todo"),
        "and Agent Status admits the Todo section it was offered: {:?}",
        both.1
    );

    // Todo on, Agent Status off: the Tool and the list are fully composed,
    // and there is simply no reminder.
    assert!(
        todo_only.0,
        "Todo does not need Agent Status to publish its Tool"
    );
    assert!(
        todo_only.1.is_empty(),
        "with no Agent Status composed there is no status of any kind: {:?}",
        todo_only.1
    );

    // Todo off, Agent Status on: Time and Background continue, and no Todo
    // section is fabricated from anywhere.
    assert!(!status_only.0, "a disabled Todo publishes no Tool");
    assert!(
        !status_only.1.iter().any(|id| id == "todo"),
        "no Todo section is fabricated without the Todo extension: {:?}",
        status_only.1
    );
    assert!(
        status_only.1.iter().any(|id| id == "temporal"),
        "and the other contributors are entirely unaffected: {:?}",
        status_only.1
    );

    assert!(!neither.0 && neither.1.is_empty());

    // And the Agent Loop's own semantics are identical across all four.
    for (index, semantics) in ordinary.iter().enumerate().skip(1) {
        assert_eq!(
            *semantics, ordinary[0],
            "combination {index} changed admission, request count, canonical history, tool \
             identities/statuses, terminal ordering, or durable event ordering"
        );
    }
}

/// Issue #259 regressions 12 and 13: extension state and canonical history
/// are different facts, and a launch decides only the former.
///
/// One conversation is driven through three consecutive compositions over the
/// *same* durable store, which is exactly what restart/resume is:
///
/// ```text
/// launch 1  Todo composed    commits a real todo result
/// launch 2  Todo absent      no current list, no Tool; history untouched
/// launch 3  Todo composed    rebuilds launch 1's accepted snapshot exactly
/// ```
///
/// The third launch is the reconstruction contract: it must recover the
/// latest accepted authoritative snapshot from canonical evidence, and it
/// must do so by *reading* the newest result rather than replaying
/// mutations — so it commits no new `ToolResult` and emits no new event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_disabling_todo_preserves_history_and_re_enabling_reconstructs_it() {
    use rustx::durable::ConversationStore;
    use rustx::tools::todo::TodoCreate;

    let dir = tempfile::tempdir().expect("lab");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let conversation = rustx::runtime::identity::ConversationId::new("conv-ext259-recovery");
    let store: Arc<dyn ConversationStore> = Arc::new(
        rustx::durable::SqliteConversationStore::open(
            conversation.clone(),
            &dir.path().join("conversation.sqlite"),
        )
        .expect("durable store"),
    );
    let launch = |todo: bool| {
        rustx::tools::runtime::ConversationToolRuntime::from_config(
            conversation.clone(),
            rustx::tools::runtime::ConversationRuntimeConfig {
                durable_binding: Some(rustx::durable::ConversationStoreBinding::new(Arc::clone(
                    &store,
                ))),
                ..rustx::tools::runtime::ConversationRuntimeConfig::new(
                    &workspace,
                    dir.path().join("artifacts"),
                )
            }
            .with_extensions(if todo {
                NativeAgentExtensions::with_todo()
            } else {
                NativeAgentExtensions::none()
            }),
        )
        .expect("tool runtime")
    };

    // ---- Launch 1: Todo composed, one accepted mutation ----
    let first = launch(true);
    let todos = first.todos().expect("launch 1 composes Todo");
    let batch = todos.open_batch().expect("one batch");
    let (_, accepted) = batch
        .writer()
        .create(TodoCreate {
            subject: "Write the parser".to_owned(),
            ..TodoCreate::default()
        })
        .expect("create");
    let evidence = common::todo_result_message("accepted", &accepted);
    // The canonical evidence is what makes the settlement truthful, so it is
    // committed to the Ledger, not merely handed to the batch.
    store
        .initialize(std::slice::from_ref(&evidence))
        .expect("commit the canonical todo result");
    batch.settle(std::slice::from_ref(&evidence));
    assert_eq!(first.todo_snapshot().expect("composed"), accepted);
    let canonical_before = store.load_canonical().expect("canonical history");
    let events_before = store.read_events(None, 256).expect("events").events.len();
    drop(first);

    // ---- Launch 2: Todo absent ----
    let second = launch(false);
    assert!(
        second.todos().is_none() && second.todo_snapshot().is_none(),
        "a Todo-disabled launch composes no current list at all"
    );
    let mut extension_registry = rustx::tools::executor::ToolRegistry::new();
    second
        .extension_tool_plane()
        .register_into(&mut extension_registry)
        .expect("an unmaterialized extension registers nothing");
    assert!(
        extension_registry.names().is_empty(),
        "and publishes no current todo Tool"
    );
    assert_eq!(
        store.load_canonical().expect("canonical history"),
        canonical_before,
        "disabling the extension rewrites, hides, and deletes nothing"
    );
    drop(second);

    // ---- Launch 3: Todo composed again ----
    let third = launch(true);
    assert_eq!(
        third.todo_snapshot().expect("launch 3 composes Todo"),
        accepted,
        "re-enabling reconstructs the latest accepted authoritative snapshot"
    );
    assert_eq!(
        store.load_canonical().expect("canonical history"),
        canonical_before,
        "reconstruction reads the newest result; it never replays a mutation, so it \
         commits no duplicate ToolResult"
    );
    assert_eq!(
        store.read_events(None, 256).expect("events").events.len(),
        events_before,
        "and generates no duplicate events"
    );
    // The reconstructed list is a live authority, not a frozen copy: it opens
    // batches and allocates ids from where the accepted snapshot left off.
    let batch = third
        .todos()
        .expect("composed")
        .open_batch()
        .expect("the reconstructed list opens a batch");
    let (task, _) = batch
        .writer()
        .create(TodoCreate {
            subject: "Write the tests".to_owned(),
            ..TodoCreate::default()
        })
        .expect("create");
    assert_eq!(
        task.id, accepted.next_id,
        "id allocation continues from the accepted snapshot rather than restarting"
    );
    batch.discard();
}

/// Issue #259 regression 17: a resource reload cannot hot-install or
/// hot-remove a Tool-providing extension.
///
/// The proof is structural rather than behavioural: the extension Tool set is
/// composed once at coordinator construction and stored outside
/// `CapabilityResourceInputs`, which is the *only* value a reload replaces.
/// This drives a real reload through the coordinator's own publication
/// boundary — a complete new resource-input generation that genuinely changes
/// the ordinary capability plane, proven by the advanced revision — and shows
/// the extension Tool surviving it unchanged in both directions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_a_resource_reload_cannot_install_or_remove_the_todo_extension() {
    for composed in [true, false] {
        let extensions = if composed {
            NativeAgentExtensions::with_todo()
        } else {
            NativeAgentExtensions::none()
        };
        let fixture = common::native_fixture_with_extensions(
            Vec::new(),
            rustx::tools::native::NativeToolPolicies::default(),
            &extensions,
        );
        let ordinary_base = || {
            let mut registry = rustx::tools::executor::ToolRegistry::new();
            rustx::tools::native::register_native_tools(
                &mut registry,
                rustx::tools::NativeToolResources {
                    subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
                    background: fixture.runtime.background().clone(),
                    subagents: None,
                },
                rustx::tools::NativeToolPolicies::default(),
            )
            .expect("ordinary native registration");
            registry
        };
        // The first generation activates the whole ordinary native plane.
        let capability = common::capability_lease_with(
            ordinary_base(),
            &fixture.runtime,
            rustx::capabilities::ToolActivationPolicy::default(),
        )
        .await;
        let (lease, coordinator) = capability.into_lease_and_coordinator();
        let names = |coordinator: &rustx::capabilities::CapabilityCoordinator| {
            coordinator
                .current_snapshot()
                .tool_registry()
                .names()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let before = names(&coordinator);
        assert_eq!(before.contains(&"todo".to_owned()), composed);
        assert!(before.contains(&"read".to_owned()));
        let revision_before = coordinator.current_snapshot().revision();
        // An attempt lease pins the published generation, so a reload is
        // refused while one is held. Releasing it is the ordinary boundary,
        // not a timing trick.
        drop(lease);

        // A genuinely new resource generation that empties the *ordinary*
        // plane entirely — the strongest ordinary statement there is.
        let candidate = coordinator
            .prepare_candidate_with_inputs(rustx::capabilities::CapabilityResourceInputs {
                python_sources: std::collections::BTreeMap::new(),
                base_tool_registry: Arc::new(ordinary_base()),
                tool_activation: rustx::capabilities::ToolActivationPolicy {
                    no_tools: true,
                    ..rustx::capabilities::ToolActivationPolicy::default()
                },
                skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: fixture.runtime.environment().clone(),
            })
            .await
            .expect("the reload prepares a candidate");
        coordinator.commit(candidate).expect("the reload publishes");
        let after = names(&coordinator);
        assert!(
            coordinator.current_snapshot().revision() > revision_before,
            "the reload really did publish a new capability generation"
        );
        assert!(
            !after.contains(&"read".to_owned()),
            "and it really did change the ordinary plane"
        );
        assert_eq!(
            after.contains(&"todo".to_owned()),
            composed,
            "a resource reload cannot hot-remove a Tool-providing extension"
        );

        // The symmetric direction from the same coordinator: an ordinary
        // plane that activates everything cannot install the extension into a
        // composition that does not have it.
        let candidate = coordinator
            .prepare_candidate_with_inputs(rustx::capabilities::CapabilityResourceInputs {
                python_sources: std::collections::BTreeMap::new(),
                base_tool_registry: Arc::new(ordinary_base()),
                tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
                skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: fixture.runtime.environment().clone(),
            })
            .await
            .expect("the second reload prepares a candidate");
        coordinator
            .commit(candidate)
            .expect("the second reload publishes");
        let restored = names(&coordinator);
        assert!(restored.contains(&"read".to_owned()));
        assert_eq!(
            restored.contains(&"todo".to_owned()),
            composed,
            "and cannot hot-install one either"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #259 — one frozen composition, four facets.
//
// The contract these prove is the one the whole migration rests on:
//
// ```text
// NativeAgentExtensions          one frozen value, stored by the conversation
//   |                            tool runtime that materializes it
//   |-- ConversationTodoList     Todo state authority
//   |-- ExtensionToolPlane       the model Tool surface, DERIVED from that
//   |                            authority — the only constructor reads the
//   |                            owners, never a configuration value
//   |-- AgentStatusEngine        the status materialization
//   `-- native_extensions()      the Runtime Client effective projection,
//                                read straight off the stored value
// ```
//
// So the invalid states #259 is about are not "rejected at runtime"; the
// non-empty Tool plane has no constructor that does not take the state owner,
// and the `ConversationRuntime` ownership-transfer boundary refuses the one
// remaining way to build an incoherent runtime — pairing facets materialized
// for two different compositions.
// ---------------------------------------------------------------------------

/// Composes one `ConversationRuntime` over an explicit conversation tool
/// runtime and an explicit extension Tool plane (Issue #259).
///
/// This is the **real** ownership-transfer boundary — the place a
/// conversation's Todo state owner, its extension Tool plane, and its Agent
/// Status engine become one runtime — so it is where the single-composition
/// invariant is proved, rather than at `LocalConversationCore`. Passing the
/// plane separately is what lets a test hand this boundary two facets
/// materialized for two different compositions; production never can, because
/// both real composition sites derive the plane from the tool runtime they
/// just built.
///
/// The Agent Status engine is derived from the tool runtime's own frozen
/// composition, for the same reason: it is a facet of one decision, not a
/// second input.
///
/// # Errors
///
/// Returns the construction failure, including
/// `ConversationRuntimeError::ExtensionCompositionMismatch` when the facets
/// do not follow from one frozen composition.
async fn conversation_runtime_with_extension_plane(
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    extension_tools: rustx::extensions::ExtensionToolPlane,
) -> Result<ComposedRuntime, rustx::runtime::ConversationRuntimeError> {
    let dir = tempfile::tempdir().expect("capability temp dir");
    let coordinator = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            python_sources: std::collections::BTreeMap::new(),
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: std::sync::Arc::new(rustx::tools::executor::ToolRegistry::new()),
            extension_tools,
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("capability coordinator");
    let candidate = coordinator
        .prepare_candidate()
        .await
        .expect("candidate preparation");
    coordinator.commit(candidate).expect("candidate commit");
    let estimator: std::sync::Arc<dyn rustx::context::TokenEstimator> =
        std::sync::Arc::new(rustx::context::DefaultTokenEstimator);
    let runtime =
        rustx::runtime::ConversationRuntime::new(rustx::runtime::RuntimeConversationConfig {
            agent_id: rustx::runtime::identity::AgentId::new("agent-extension-composition"),
            model: support::model::scripted_session_model(support::fake::fake_model(Vec::new())),
            approval_mode: rustx::runtime::ApprovalMode::Policy,
            model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: rustx::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: rustx::runtime::ConversationContextConfig {
                policy: rustx::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                // The status facet of the same frozen decision.
                status_engine: tool_runtime
                    .extensions()
                    .agent_status_engine(std::sync::Arc::new(rustx::context::SystemClock)),
            },
            tool_runtime: tool_runtime.clone(),
            resources: std::sync::Arc::new(rustx::runtime::RuntimeResourceSnapshot::new(
                rustx::runtime::RuntimeResourceRevision::new(1),
                Vec::new(),
                None,
                rustx::context::ContextAssembly::new(),
                coordinator.current_snapshot(),
            )),
            resource_loader: std::sync::Arc::new(
                rustx::runtime::FilesystemRuntimeResourceLoader::new(
                    coordinator.current_snapshot().workspace_root(),
                ),
            ),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        })?;
    Ok(ComposedRuntime { runtime, _dir: dir })
}

/// One composed runtime and the environment-store root it outlives.
///
/// The directory is declared **last**: struct fields drop in declaration
/// order, so the runtime and every handle taken from it are released before
/// the directory is removed.
struct ComposedRuntime {
    runtime: rustx::runtime::ConversationRuntime,
    _dir: tempfile::TempDir,
}

/// One conversation tool runtime frozen on an explicit composition.
fn todo_tool_runtime(
    label: &str,
    extensions: &NativeAgentExtensions,
) -> (
    tempfile::TempDir,
    rustx::tools::runtime::ConversationToolRuntime,
) {
    let dir = tempfile::tempdir().expect("lab");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let runtime = rustx::tools::runtime::ConversationToolRuntime::from_config(
        rustx::runtime::identity::ConversationId::new(label),
        rustx::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(extensions.clone()),
    )
    .expect("tool runtime");
    (dir, runtime)
}

/// Issue #259 regressions 1 and 4: one frozen composition decides Todo
/// state, the Todo Tool, and the effective projection together.
///
/// The three facets are read back from one materialization, across both
/// authored spellings of the public document. Nothing here configures the
/// Tool surface: it is derived from the state owner, which is why the two
/// columns cannot come apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_one_frozen_composition_decides_todo_state_tool_and_projection() {
    for composed in [true, false] {
        let extensions = composition(serde_json::json!({"todo": {"enabled": composed}}));
        let (_dir, runtime) = todo_tool_runtime("conv-ext259-one-decision", &extensions);

        // Facet 1: the conversation-owned Todo state authority.
        assert_eq!(runtime.todos().is_some(), composed);
        assert_eq!(runtime.todo_snapshot().is_some(), composed);

        // Facet 2: the extension Tool plane, derived from facet 1.
        let plane = runtime.extension_tool_plane();
        assert_eq!(
            plane.tool_names(),
            if composed {
                vec!["todo".to_owned()]
            } else {
                Vec::new()
            },
            "the published extension Tool set follows the materialized owner"
        );

        // Facet 3: the stored composition the effective projection reads.
        assert_eq!(runtime.extensions(), &extensions);
        assert_eq!(runtime.extensions().todo().is_some(), composed);

        // And the capability plane a coordinator composes from that plane
        // agrees with both, under an ordinary activation policy that selects
        // every ordinary capability.
        let capability =
            common::capability_lease(rustx::tools::executor::ToolRegistry::new(), &runtime).await;
        assert_eq!(
            capability
                .snapshot()
                .tool_registry()
                .names()
                .contains(&"todo"),
            composed,
            "the model's Tool set cannot disagree with the conversation's Todo state"
        );
    }
}

/// Issue #259 regressions 2 and 3: neither mismatched runtime is
/// constructible.
///
/// "Todo Tool on / Todo state off" is proved *unrepresentable* rather than
/// rejected: [`ExtensionToolPlane`] has exactly two constructors, and the one
/// that can publish `todo` takes the materialized `ConversationTodoList`. The
/// public one produces the empty plane and nothing else, so there is no value
/// in the process that offers `todo` without a list behind it.
///
/// "Todo state on / Todo Tool off" is what remains representable — a
/// coordinator composed from *another* conversation's materialization — and
/// it is refused at the `ConversationRuntime` ownership-transfer boundary,
/// which is the real construction seam rather than `LocalConversationCore`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_neither_mismatched_todo_runtime_can_be_constructed() {
    use rustx::extensions::ExtensionToolPlane;

    // ---- Todo Tool without Todo state: unrepresentable ----
    //
    // The only publicly constructible plane is the empty one. A plane that
    // publishes `todo` can be obtained from nowhere but a tool runtime that
    // materialized the list, so the deterministic `tool_runtime.todos()`
    // failure this blocker is about has no reachable precondition.
    assert!(
        ExtensionToolPlane::none().tool_names().is_empty(),
        "the one public plane constructor cannot publish an extension Tool"
    );
    let (_absent_dir, absent) =
        todo_tool_runtime("conv-ext259-absent", &NativeAgentExtensions::none());
    assert!(absent.todos().is_none() && absent.extension_tool_plane().tool_names().is_empty());

    // ---- Todo state without the Todo Tool: refused at construction ----
    //
    // Both runtimes are real and individually coherent; the coordinator is
    // composed from the *wrong* one's materialization. That is the only way
    // left to spell the mismatch, and it fails closed.
    let (_owning_dir, owning) =
        todo_tool_runtime("conv-ext259-owning", &NativeAgentExtensions::with_todo());
    assert!(owning.todos().is_some());
    let mismatched =
        conversation_runtime_with_extension_plane(&owning, absent.extension_tool_plane()).await;
    assert!(
        matches!(
            mismatched,
            Err(
                rustx::runtime::ConversationRuntimeError::ExtensionCompositionMismatch {
                    composed_todo: true,
                    materialized_todo_state: true,
                    published_todo_tool: false,
                    ..
                }
            )
        ),
        "a Todo-owning conversation may not be served a Tool plane without its Tool: {:?}",
        mismatched.err()
    );

    // The symmetric pairing is refused for the symmetric reason: a
    // conversation that owns no list may not be served a plane that offers
    // the Tool.
    let (_second_owner_dir, second_owner) = todo_tool_runtime(
        "conv-ext259-second-owner",
        &NativeAgentExtensions::with_todo(),
    );
    let reversed =
        conversation_runtime_with_extension_plane(&absent, second_owner.extension_tool_plane())
            .await;
    assert!(
        matches!(
            reversed,
            Err(
                rustx::runtime::ConversationRuntimeError::ExtensionCompositionMismatch {
                    composed_todo: false,
                    materialized_todo_state: false,
                    published_todo_tool: true,
                    ..
                }
            )
        ),
        "a conversation with no list may not be offered the todo Tool: {:?}",
        reversed.err()
    );

    // And the coherent pairing of the same two facets constructs normally,
    // so the two refusals above are about the mismatch and not about the
    // fixture.
    let coherent =
        conversation_runtime_with_extension_plane(&owning, owning.extension_tool_plane()).await;
    assert!(
        coherent.is_ok(),
        "the coherent composition constructs: {:?}",
        coherent.err()
    );
}

/// Issue #259 regression 4: the Runtime Client effective Todo projection is
/// the same stored decision the Tool Plane was proved against.
///
/// `native_extensions()` no longer reconstructs the composition from
/// materialized parts — it returns the one value the conversation tool
/// runtime stored — and construction already proved every facet followed from
/// it. So "projection says Todo composed, Tool Plane says otherwise" is not a
/// disagreement the runtime can reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext259_the_effective_projection_cannot_disagree_with_the_tool_plane() {
    for composed in [true, false] {
        let extensions = composition(serde_json::json!({
            "todo": {"enabled": composed},
            "agentStatus": {"enabled": false},
        }));
        let (_dir, tool_runtime) = todo_tool_runtime("conv-ext259-projection", &extensions);
        let composed_runtime = conversation_runtime_with_extension_plane(
            &tool_runtime,
            tool_runtime.extension_tool_plane(),
        )
        .await
        .expect("the coherent composition constructs");
        let runtime = &composed_runtime.runtime;

        let projected = runtime.native_extensions();
        assert_eq!(
            projected, extensions,
            "the projection is the frozen decision"
        );
        assert_eq!(
            projected.todo().is_some(),
            composed,
            "and it is the authoritative Todo answer"
        );
        assert_eq!(
            rustx::runtime_client::settings::EffectiveNativeAgentExtensions::project(&projected)
                .todo
                .is_some(),
            composed,
            "the wire projection carries exactly that fact"
        );
        assert_eq!(
            runtime.tool_runtime().todos().is_some(),
            composed,
            "beside the Todo state the same decision materialized"
        );
        assert_eq!(
            runtime
                .tool_runtime()
                .extension_tool_plane()
                .tool_names()
                .contains(&"todo".to_owned()),
            composed,
            "and the Tool Plane the same decision published"
        );
    }
}
