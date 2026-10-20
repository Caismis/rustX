mod compaction_head_tests;
mod delivery_tests;
mod initial_cut_tests;
mod member_tests;
mod native_cancellation_tests;
mod owner_tests;
mod pure_read_tests;
mod settlement_tests;
mod source_recovery_tests;
#[test]
fn permanent_agent_settlement_is_a_typed_non_retryable_error() {
    let id = crate::runtime::identity::AgentId::new("agent-unavailable");
    assert!(
        matches!(super::control::agent_control_error(&id, crate::runtime::subagent::AgentControlError::NotDelivered), crate::runtime_client::types::RuntimeClientError::AgentNotDelivered { agent_id } if agent_id == id)
    );
    assert!(
        matches!(super::control::agent_control_error(&id, crate::runtime::subagent::AgentControlError::DeliveryUnknown), crate::runtime_client::types::RuntimeClientError::AgentDeliveryUnknown { agent_id } if agent_id == id)
    );
    assert!(matches!(
        super::control::agent_control_error(&id, crate::runtime::subagent::AgentControlError::Settlement),
        crate::runtime_client::types::RuntimeClientError::AgentSettlement { agent_id } if agent_id == id
    ));
    assert!(matches!(
        super::control::agent_control_error(&id, crate::runtime::subagent::AgentControlError::Stopping),
        crate::runtime_client::types::RuntimeClientError::AgentStopping { agent_id } if agent_id == id
    ));
}

use crate::runtime::observation::ConversationObservation;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use super::{
    EventDelivery, EventSubscription, ProjectionWorker, RuntimeClientHost, RuntimeClientHostConfig,
};
use crate::context::{
    AgentStatusClock, AgentStatusConfig, AgentStatusEngine, DefaultTokenEstimator, TokenEstimator,
};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::adapter::{ModelAdapter, ModelStream};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, ConversationRuntimeError, CoordinatorProbe,
    InboundAdmissionError, ModelUpdateError, RuntimeConversationConfig,
};
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, MessageId, ToolCallId, ToolExecutionId, ToolId,
};
use crate::runtime::request_history::RequestHistory;
use crate::runtime::types::RuntimeClock;
use crate::runtime_client::event::RuntimeClientEvent;
use crate::runtime_client::host::HostConstructionError;
use crate::runtime_client::snapshot::RuntimeClientAttemptPhase;
use crate::runtime_client::types::{
    RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent, RuntimeClientRequest,
    RuntimeClientResult,
};
use crate::scripted_suites::support::model::scripted_session_model;
use crate::tools::background::{
    BackgroundDispatchError, BackgroundDispatchOutcome, BackgroundExecutionSnapshot,
    BackgroundLifecycle, ConversationBackgroundRegistry,
};
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use crate::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolReplayPolicy,
    ToolResultContent,
};

fn test_resources(
    capability: &crate::capabilities::CapabilityCoordinator,
) -> Arc<crate::runtime::RuntimeResourceSnapshot> {
    Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
        crate::runtime::RuntimeResourceRevision::new(1),
        Vec::new(),
        None,
        crate::context::ContextAssembly::new(),
        capability.current_snapshot(),
    ))
}

fn request_snapshots(history: &RequestHistory) -> Vec<crate::model::RequestSnapshot> {
    let mut snapshots = Vec::new();
    let mut cursor = None;
    loop {
        let page = history.page(cursor, 32).expect("request snapshot page");
        if page.snapshots.is_empty() {
            break;
        }
        cursor = page.next_sequence;
        snapshots.extend(page.snapshots);
    }
    snapshots
}

/// One scripted step of the gated adapter.
enum GatedStep {
    /// Yield one canonical model event.
    Emit(ModelEvent),
    /// Wait until the shared watch releases, then continue without
    /// yielding; if the attempt cancellation fires first, fail with a
    /// cancelled model error like a real adapter.
    ParkUntilReleased(watch::Receiver<bool>),
}

/// A scripted cancellation-aware model adapter: one script per
/// invocation, with deterministic park points.
struct GatedAdapter {
    scripts: Mutex<VecDeque<VecDeque<GatedStep>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    request_count: Arc<watch::Sender<usize>>,
}

impl GatedAdapter {
    fn new(scripts: Vec<Vec<GatedStep>>) -> Self {
        let (request_count, _receiver) = watch::channel(0);
        Self {
            scripts: Mutex::new(scripts.into_iter().map(VecDeque::from).collect()),
            requests: Arc::new(Mutex::new(Vec::new())),
            request_count: Arc::new(request_count),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    fn request_count(&self) -> watch::Receiver<usize> {
        self.request_count.subscribe()
    }
}

impl ModelAdapter for GatedAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
        let request_count = {
            let mut requests = self.requests.lock().expect("requests lock");
            requests.push(request);
            requests.len()
        };
        self.request_count.send_replace(request_count);
        let script = self
            .scripts
            .lock()
            .expect("scripts lock")
            .pop_front()
            .unwrap_or_default();
        Box::pin(futures_util::stream::unfold(
            (script, cancellation),
            |(mut script, cancellation)| async move {
                loop {
                    match script.pop_front() {
                        None => return None,
                        Some(GatedStep::Emit(event)) => {
                            return Some((
                                crate::model::adapter::ModelStreamItem::Event(event),
                                (script, cancellation),
                            ));
                        }
                        Some(GatedStep::ParkUntilReleased(mut release)) => {
                            tokio::select! {
                                biased;
                                () = cancellation.cancelled() => {
                                    return Some((crate::model::adapter::ModelStreamItem::Event(ModelEvent::Failed {
                                        error: ModelError {
                                            kind: ModelErrorKind::Cancelled,
                                            message: "cancelled while parked".to_owned(),
                                            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                                            retry_after_ms: None,
                                            provider_code: None,
                                            context_overflow: None,
                                            malformed_tool_proposal: None,
                                            timeout_phase: None,
                                            generation: None,
                                        },
                                    }), (VecDeque::new(), cancellation)));
                                }
                                result = release.wait_for(|released| *released) => {
                                    result.expect("release channel stays open");
                                }
                            }
                        }
                    }
                }
            },
        ))
    }
}

fn model_release() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// A parking background executor: its returned future reports entry,
/// waits on a durable release state, then settles with a fixed result.
struct ParkingBackgroundTool {
    #[allow(dead_code)] // the definition documents the tool identity
    definition: ToolDefinition,
    started: watch::Sender<bool>,
    release: watch::Sender<bool>,
    execution_gate: Option<watch::Receiver<bool>>,
}

impl ParkingBackgroundTool {
    fn new() -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
        Self::build(None)
    }

    /// Builds a fixture whose returned future waits at an explicit gate
    /// before it observes `release`. This is used by the lost-wakeup
    /// regression to force release-before-wait ordering.
    fn new_with_execution_gate() -> (
        Self,
        watch::Receiver<bool>,
        watch::Sender<bool>,
        watch::Sender<bool>,
    ) {
        let (execution_gate, execution_gate_rx) = watch::channel(false);
        let (tool, started, release) = Self::build(Some(execution_gate_rx));
        (tool, started, release, execution_gate)
    }

    fn build(
        execution_gate: Option<watch::Receiver<bool>>,
    ) -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
        let (started, started_rx) = watch::channel(false);
        let (release, _release_rx) = watch::channel(false);
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
                execution_gate,
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
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let started = self.started.clone();
        let mut execution_gate = self.execution_gate.clone();
        let mut release = self.release.subscribe();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                // This signal is published by the returned future, so
                // observing it means the deterministic execution gate has
                // actually been entered rather than merely returned by
                // `start`.
                started.send_replace(true);
                if let Some(execution_gate) = execution_gate.as_mut() {
                    execution_gate
                        .wait_for(|entered| *entered)
                        .await
                        .expect("execution gate stays open");
                }
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

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

/// Bounds only the terminal wait in background-runtime tests. The
/// release and start orderings are established by watch state; this is
/// fail-fast containment for a broken fixture or registry invariant, not
/// a synchronization primitive.
const BACKGROUND_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

async fn await_background_started(started: &mut watch::Receiver<bool>, description: &'static str) {
    tokio::time::timeout(
        BACKGROUND_LIVENESS_GUARD,
        started.wait_for(|is_started| *is_started),
    )
    .await
    .unwrap_or_else(|_| panic!("{description}: start wait exceeded liveness guard"))
    .expect("start channel stays open");
}

async fn await_background_terminal(
    registry: &ConversationBackgroundRegistry,
    execution_id: &ToolExecutionId,
    description: &'static str,
) -> BackgroundExecutionSnapshot {
    tokio::time::timeout(
        BACKGROUND_LIVENESS_GUARD,
        registry.wait_until_terminal(execution_id),
    )
    .await
    .unwrap_or_else(|_| panic!("{description}: terminal wait exceeded liveness guard"))
    .unwrap_or_else(|error| panic!("{description}: Job wait failed: {error:?}"))
}

/// A fixed deterministic status clock.
#[derive(Debug, Clone, Copy)]
struct FixedStatusClock;

impl AgentStatusClock for FixedStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("fixed clock")
            .with_timezone(&chrono::Utc)
    }
}

/// A fixed deterministic runtime clock.
#[derive(Debug, Clone, Copy)]
struct FixedRuntimeClock;

impl RuntimeClock for FixedRuntimeClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("fixed clock")
            .with_timezone(&chrono::Utc)
    }
}

/// A fixture over one conversation: the conversation runtime
/// coordinator, its Runtime Client host adapter, and the scripted
/// adapter driving attempts.
struct HostFixture {
    directory: tempfile::TempDir,
    host: RuntimeClientHost,
    runtime: ConversationRuntime,
    coordinator: crate::capabilities::CapabilityCoordinator,
    store: Option<Arc<SqliteConversationStore>>,
}

#[tokio::test]
async fn child_reading_invalid_limits_precede_ownership_resolution() {
    let (_, fixture) = host_fixture(vec![], ToolRegistry::new(), status_engine()).await;
    let id = crate::runtime::identity::AgentId::new("unknown-child");
    for limit in [0, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX + 1] {
        assert!(matches!(
            fixture.host.inner.agent_transcript_window(
                &id,
                &crate::durable::reading::ConversationWindowAt::Latest,
                limit
            ),
            Err(RuntimeClientError::InvalidRequest { .. })
        ));
        assert!(matches!(
            fixture.host.inner.agent_turns(&id, None, limit),
            Err(RuntimeClientError::InvalidRequest { .. })
        ));
        assert!(matches!(
            fixture.host.inner.transcript_page(None, limit),
            Err(RuntimeClientError::InvalidRequest { .. })
        ));
    }
    // This fixture intentionally has no Agent registry. Valid bounds enter
    // ownership resolution; invalid bounds above never reach it.
    for limit in [1, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX] {
        assert!(matches!(
            fixture.host.inner.agent_transcript_window(&id, &crate::durable::reading::ConversationWindowAt::Latest, limit),
            Err(RuntimeClientError::InvalidState { message }) if message == "Agent registry unavailable"
        ));
    }
}

async fn await_fixture_activation(host: &RuntimeClientHost) {
    let inner = host.weak_inner().upgrade().expect("resident fixture");
    tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + BACKGROUND_LIVENESS_GUARD;
        loop {
            let generation = inner.publication_gate.generation();
            let (eligibility, cursor) = {
                let state = inner.lock_published_state();
                (state.projection.snapshot_ref().configuration_adoption_eligibility.clone(), state.projection.cursor())
            };
            if eligibility == crate::runtime::adoption::AdoptionEligibility::Eligible {
                assert!(cursor.get() > 0, "activation eligibility is actually published");
                break;
            }
            assert!(std::time::Instant::now() < deadline,
                "native activation eligibility did not publish: cursor={cursor:?}, eligibility={eligibility:?}");
            inner.publication_gate.wait_after(generation, deadline);
        }
    }).await.expect("fixture activation publication wait");
}

/// Builds the conversation runtime + host over one conversation with
/// the given adapter scripts and tool registry.
async fn host_fixture(
    scripts: Vec<Vec<GatedStep>>,
    tools: ToolRegistry,
    status_engine: AgentStatusEngine,
) -> (Arc<GatedAdapter>, HostFixture) {
    host_fixture_with_native_tools(scripts, tools, status_engine, false).await
}

/// Builds the conversation runtime + host, optionally activating the
/// real native tool plane. Skill projection tests opt into this variant
/// so the fixture exercises the normal native composition and its
/// default-enabled Read capability.
async fn host_fixture_with_native_tools(
    scripts: Vec<Vec<GatedStep>>,
    tools: ToolRegistry,
    status_engine: AgentStatusEngine,
    include_native_tools: bool,
) -> (Arc<GatedAdapter>, HostFixture) {
    let (adapter, fixture) =
        host_fixture_before_activation(scripts, tools, status_engine, include_native_tools, None)
            .await;
    fixture.runtime.activate();
    await_fixture_activation(&fixture.host).await;
    (adapter, fixture)
}

#[allow(
    clippy::too_many_lines,
    reason = "one complete deterministic fixture boundary"
)]
async fn host_fixture_before_activation(
    scripts: Vec<Vec<GatedStep>>,
    mut tools: ToolRegistry,
    status_engine: AgentStatusEngine,
    include_native_tools: bool,
    monotonic_clock: Option<Arc<dyn crate::runtime::monotonic::MonotonicClock>>,
) -> (Arc<GatedAdapter>, HostFixture) {
    let adapter = Arc::new(GatedAdapter::new(scripts));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let store = Arc::new(
        SqliteConversationStore::open(
            conversation_id.clone(),
            &dir.path().join("conversation.sqlite"),
        )
        .expect("store"),
    );
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(crate::durable::ConversationStoreBinding::new(store.clone())),
            ..crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
        }
        // These Runtime Client fixtures compose Agent Status and nothing
        // else: the composition must match the status engine below and
        // the extension Tool plane the coordinator is given, because
        // `ConversationRuntime` construction now proves all three follow
        // from one frozen decision (Issue #259). The Todo extension has
        // its own suites; composing it here would only add an unrelated
        // Tool and an unrelated conversation-owned resource to every
        // host contract.
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    if include_native_tools {
        crate::tools::register_native_tools(
            &mut tools,
            crate::tools::NativeToolResources {
                subagent_catalog: crate::runtime::subagent::AgentCatalog::empty(),
                background: tool_runtime.background().clone(),
                subagents: None,
            },
            crate::tools::NativeToolPolicies::default(),
        )
        .expect("register native tools");
    }
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            agent_activation: {
                let mut activation = crate::capabilities::AgentActivation::default();
                activation.profile.skills =
                    Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                activation.profile.extensions =
                    crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                activation.profile.tools.builtin = tools
                    .definitions()
                    .into_iter()
                    .filter(|tool| tool.origin.source().is_none())
                    .map(|tool| tool.name.clone())
                    .collect();
                activation
            },
            base_tool_registry: Arc::new(tools),
            extension_tools: tool_runtime.extension_tool_plane(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.join(".agents/skills"),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator
        .prepare_candidate()
        .await
        .expect("prepare candidate");
    coordinator.commit(candidate).expect("commit candidate");
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter.clone()),
        approval_mode: crate::runtime::ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_engine: Some(status_engine),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator.clone(),
        clock: Some(Arc::new(FixedRuntimeClock)),
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    };
    let runtime = match monotonic_clock {
        Some(clock) => ConversationRuntime::with_test_monotonic_clock(config, clock),
        None => ConversationRuntime::new(config),
    }
    .expect("conversation runtime");
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: runtime.clone(),
        replay_limit: None,
    })
    .expect("runtime client host");
    (
        adapter,
        HostFixture {
            directory: dir,
            host,
            runtime,
            coordinator,
            store: Some(store),
        },
    )
}

/// Builds a fixture whose conversation runtime carries the given
/// coordinator synchronization hooks.
async fn host_fixture_with_runtime_probe(
    scripts: Vec<Vec<GatedStep>>,
    probe: CoordinatorProbe,
) -> (Arc<GatedAdapter>, HostFixture) {
    let adapter = Arc::new(GatedAdapter::new(scripts));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        // These Runtime Client fixtures compose Agent Status and nothing
        // else: the composition must match the status engine below and
        // the extension Tool plane the coordinator is given, because
        // `ConversationRuntime` construction now proves all three follow
        // from one frozen decision (Issue #259). The Todo extension has
        // its own suites; composing it here would only add an unrelated
        // Tool and an unrelated conversation-owned resource to every
        // host contract.
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: crate::capabilities::AgentActivation::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.join(".agents/skills"),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let runtime = ConversationRuntime::with_probe(
        RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(status_engine()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        },
        probe,
    )
    .expect("conversation runtime with probe");
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: runtime.clone(),
        replay_limit: None,
    })
    .expect("runtime client host");
    // The explicit lifecycle boundary: the host bound over the inert
    // runtime, so semantic execution may begin now.
    runtime.activate();
    (
        adapter,
        HostFixture {
            directory: dir,
            host,
            runtime,
            coordinator,
            store: None,
        },
    )
}

/// The default status engine over the fixed clock.
fn status_engine() -> AgentStatusEngine {
    AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(FixedStatusClock))
}

fn inbound_text(id: &str, text: &str) -> UserMessageBlock {
    UserMessageBlock {
        id: crate::runtime::identity::MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: crate::message::types::InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("parse")
                .with_timezone(&chrono::Utc),
        ),
    }
}

fn submit_content(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })]
}

fn durable_event(
    conversation_id: &ConversationId,
    event_id: &str,
    attempt_id: &AttemptId,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: Some(attempt_id.clone()),
        turn_id: None,
        timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("fixed event timestamp")
            .with_timezone(&chrono::Utc),
        event,
    }
}

fn durable_tool_result() -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Text(TextBlock {
            text: "child tool output".to_owned(),
        })],
        duration_ms: 7,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

fn one_turn_stop() -> Vec<GatedStep> {
    vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

struct DurableHistoryFixture {
    conversation_id: ConversationId,
    store: Arc<SqliteConversationStore>,
    user: MessageBlock,
    assistant: MessageBlock,
    tool: MessageBlock,
    call: ToolCall,
    tool_result: ToolExecutionResult,
}

fn durable_history_fixture() -> DurableHistoryFixture {
    let conversation_id = ConversationId::new("conv_8ae2de1c-f83b-7ff0-882f-98321c50d37d");
    let store =
        Arc::new(SqliteConversationStore::in_memory(conversation_id.clone()).expect("child store"));
    let user = MessageBlock::User(inbound_text("child-user", "inspect this child"));
    let call = ToolCall {
        id: ToolCallId::new("child-call-1"),
        tool_id: ToolId::new("tool-read"),
        name: "read".to_owned(),
        arguments: serde_json::json!({"path": "README.md"}),
    };
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("child-assistant"),
        content: vec![
            AssistantContentBlock::Text(TextBlock {
                text: "I will inspect the file.".to_owned(),
            }),
            AssistantContentBlock::ToolCall(call.clone()),
        ],
    });
    let tool_result = durable_tool_result();
    let tool = MessageBlock::Tool(ToolMessageBlock {
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            crate::runtime::identity::MessageId::new("child-assistant"),
            crate::message::types::ContentBlockIndex::new(1),
        ),
        id: MessageId::new("child-tool-result"),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: tool_result.clone(),
    });
    append_durable_history(
        &store,
        &conversation_id,
        &assistant,
        &tool,
        &call,
        &tool_result,
    );
    DurableHistoryFixture {
        conversation_id,
        store,
        user,
        assistant,
        tool,
        call,
        tool_result,
    }
}

fn append_durable_history(
    store: &SqliteConversationStore,
    conversation_id: &ConversationId,
    assistant: &MessageBlock,
    tool: &MessageBlock,
    call: &ToolCall,
    tool_result: &ToolExecutionResult,
) {
    let user = MessageBlock::User(inbound_text("child-user", "inspect this child"));
    store
        .initialize(std::slice::from_ref(&user))
        .expect("child seed");
    let attempt_id = AttemptId::new("child-attempt-1");
    store
        .append_event(durable_event(
            conversation_id,
            "child-attempt-started",
            &attempt_id,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
        ))
        .expect("attempt start");
    store
        .append_canonical_with_event(
            assistant,
            durable_event(
                conversation_id,
                "child-assistant-committed",
                &attempt_id,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: assistant.id().clone(),
                },
            ),
        )
        .expect("assistant commit");
    store
        .append_event(durable_event(
            conversation_id,
            "child-tool-started",
            &attempt_id,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        ))
        .expect("tool start");
    store
        .append_event(durable_event(
            conversation_id,
            "child-tool-completed",
            &attempt_id,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: tool_result.clone(),
            },
        ))
        .expect("tool completion");
    store
        .append_canonical_with_event(
            tool,
            durable_event(
                conversation_id,
                "child-tool-committed",
                &attempt_id,
                RuntimeEvent::ToolMessageCommitted {
                    message_id: tool.id().clone(),
                    tool_call_id: call.id.clone(),
                },
            ),
        )
        .expect("tool result commit");
    store
        .append_event(durable_event(
            conversation_id,
            "child-attempt-completed",
            &attempt_id,
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt_id.clone(),
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .expect("attempt completion");
}

/// The outer liveness guard of the event-stream helpers.
///
/// Waiting for an event is exact: the subscription wakes on
/// publication. This bounds only the total wall time of one
/// `receive_until` call, so a genuine regression fails with a message
/// instead of hanging. It is deliberately far larger than any
/// scheduling delay a loaded runner can produce, and it is a whole-call
/// budget rather than a per-event bound — a single scheduling stall can
/// never fail a correct run.
const STREAM_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

/// Receives events until the predicate matches.
async fn receive_until(
    subscription: &EventSubscription,
    mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
) -> Vec<RuntimeClientProtocolEvent> {
    tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
        let mut seen = Vec::new();
        loop {
            let delivery = subscription.next().await;
            let EventDelivery::Event(event) = delivery else {
                panic!("subscription must stay open and contiguous, got {delivery:?}");
            };
            let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &event.event else {
                panic!("one complete native transition");
            };
            assert_eq!(metadata.read_cut.revision, event.cursor.get());
            let matched = event.event.facts().iter().any(|fact| {
                predicate(&RuntimeClientProtocolEvent {
                    cursor: event.cursor,
                    event: fact.clone(),
                })
            });
            seen.push(event);
            if matched {
                return seen;
            }
        }
    })
    .await
    .expect("the observation stream must not stall")
}

async fn receive_until_metadata(
    subscription: &EventSubscription,
    mut predicate: impl FnMut(&super::super::checkpoint::ProjectionMetadata) -> bool,
) -> RuntimeClientProtocolEvent {
    tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
        loop {
            let EventDelivery::Event(event) = subscription.next().await else {
                panic!("native subscription must remain contiguous");
            };
            let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &event.event else {
                panic!("one complete native transition");
            };
            assert_eq!(metadata.read_cut.revision, event.cursor.get());
            if predicate(metadata) { return event; }
            if let Some(outcome) = event.event.facts().iter().find_map(|fact| match fact {
                RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome),
                _ => None,
            }) {
                panic!("native producer settled before the expected published block frontier: {outcome:?}");
            }
        }
    }).await.expect("the native metadata stream must advance")
}

/// A durable attachment reads the child's canonical Ledger and Event
/// Journal through the ordinary Runtime Client snapshot. It has no live
/// runtime to mutate, and detaching it leaves both authorities byte-for-
/// byte semantically unchanged.
#[test]
fn durable_attachment_replays_child_history_without_writing_or_copying() {
    let fixture = durable_history_fixture();

    let canonical_before = fixture
        .store
        .load_canonical()
        .expect("canonical before inspection");
    let events_before = fixture
        .store
        .read_events(None, 64)
        .expect("journal before inspection")
        .events;
    let host =
        RuntimeClientHost::new_durable(fixture.store.clone(), Some(16)).expect("inspection host");
    assert!(host.inner.runtime.is_none(), "inspection owns no runtime");

    let (attachment, initialized) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach child conversation");
    let RuntimeClientResult::Initialized {
        conversation_id: attached_id,
        ..
    } = initialized
    else {
        panic!("durable attach must return its bounded identity receipt");
    };
    let (snapshot, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("explicit authoritative checkpoint read");
    assert_eq!(attached_id, fixture.conversation_id);
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert_eq!(
        snapshot.messages,
        vec![
            fixture.user.clone(),
            fixture.assistant.clone(),
            fixture.tool.clone()
        ]
    );
    assert!(snapshot.transcript.entries.iter().any(|entry| {
        matches!(
            &entry.item,
            crate::observation::transcript::RuntimeClientTranscriptItem::Message { message }
                if message == &fixture.assistant
        )
    }));
    assert!(snapshot.transcript.entries.iter().any(|entry| {
        matches!(
            &entry.item,
            crate::observation::transcript::RuntimeClientTranscriptItem::Message { message }
                if message == &fixture.tool
        )
    }));
    let attempt = snapshot.attempt.expect("durable attempt history");
    assert!(matches!(
        attempt.phase,
        RuntimeClientAttemptPhase::Settled {
            outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed {
                finish_reason: ModelFinishReason::Stop,
                ..
            }
        }
    ));
    assert!(matches!(
        attempt.foreground.as_slice(),
        [crate::observation::tool::ForegroundToolExecution {
            call_id,
            state: crate::observation::tool::ForegroundToolState::Settled { result, .. },
            ..
        }] if call_id.as_ref() == Some(&fixture.call.id) && result == &fixture.tool_result
    ));

    let rejected = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("must not mutate child"),
    });
    assert!(matches!(
        rejected.error,
        Some(RuntimeClientError::InvalidState { message })
            if message == "conversation inspection is read-only"
    ));
    attachment.detach();
    assert_eq!(
        fixture
            .store
            .load_canonical()
            .expect("canonical after inspection"),
        canonical_before
    );
    assert_eq!(
        fixture
            .store
            .read_events(None, 64)
            .expect("journal after inspection")
            .events,
        events_before
    );
}

/// A new durable owner captures a later original terminal. Reads on the
/// existing owner retain its constructor cut and subscription unchanged.
#[test]
fn durable_attachment_resyncs_cancelled_child_after_runtime_disappears() {
    let conversation_id = ConversationId::new("conv_ef919f28-b766-7227-8980-3d65cbf34ff6");
    let store =
        Arc::new(SqliteConversationStore::in_memory(conversation_id.clone()).expect("child store"));
    let partial_user = MessageBlock::User(inbound_text(
        "cancelled-child-user",
        "the cancelled child started this durable turn",
    ));
    store
        .initialize(std::slice::from_ref(&partial_user))
        .expect("partial child seed");
    let host = RuntimeClientHost::new_durable(store.clone(), None).expect("inspection host");
    let (attachment, initialized) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("initial attach");
    let RuntimeClientResult::Initialized { .. } = initialized else {
        panic!("durable attach must initialize");
    };
    let initial = host.checkpoint().unwrap();
    let subscription = attachment.subscribe_events(initial.cursor).unwrap();
    let (snapshot, _) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("explicit durable checkpoint");
    assert_eq!(snapshot.messages, vec![partial_user]);
    assert!(snapshot.attempt.is_none());

    let attempt_id = AttemptId::new("cancelled-attempt");
    store
        .append_event(durable_event(
            &conversation_id,
            "cancelled-attempt-started",
            &attempt_id,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
        ))
        .expect("attempt start");
    store
        .append_event(durable_event(
            &conversation_id,
            "cancelled-attempt-cancelled",
            &attempt_id,
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt_id.clone(),
                reason: crate::runtime::types::CancellationReason::UserRequested,
            },
        ))
        .expect("attempt cancellation");

    assert_eq!(host.checkpoint().unwrap(), initial);
    assert!(matches!(subscription.try_next(), EventDelivery::Pending));
    let fresh = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let (repaired, cursor) = fresh
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("new durable owner");
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert!(matches!(
        repaired.attempt.expect("cancelled attempt").phase,
        RuntimeClientAttemptPhase::Settled {
            outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled {
                reason: crate::runtime::types::CancellationReason::UserRequested,
            }
        }
    ));
    attachment.detach();
}

/// Issue #256 regression 9: historical-only inspection reports no
/// effective native Agent Extension composition, and never fabricates
/// one.
///
/// There is no live Agent behind a durable projection, so there is no
/// authoritative composition to project. The history seeded here
/// deliberately *does* contain Agent Status evidence — a canonical Agent
/// Status context message, with its durable generation identity, exactly
/// as a runtime that once ran the extension would have left behind.
/// That evidence is not allowed to become extension configuration: it
/// describes a step that happened, not the composition of an Agent that
/// is running now. Neither is today's configuration document, which
/// describes a prospective next launch.
#[test]
fn ext256_historical_inspection_reports_no_effective_extension_composition() {
    let conversation_id = ConversationId::new("conv_576d0fd2-f9b2-75ce-8187-0c2ca5bbfe62");
    let store = Arc::new(
        SqliteConversationStore::in_memory(conversation_id.clone()).expect("durable store"),
    );
    let user = MessageBlock::User(inbound_text(
        "historical-user",
        "this turn is only durable history now",
    ));
    // Canonical Agent Status evidence in the very history being
    // inspected: a projection that inferred the extension set from what
    // it can see would report the extension as composed here.
    let mut status = inbound_text("historical-status", "<system-reminder>\n</system-reminder>");
    status.source = UserSource::Runtime;
    status.kind = crate::message::types::InboundKind::Context(
        crate::message::types::ContextKind::AgentStatus(
            crate::message::types::AgentStatusGenerationMetadata::new(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("fixed status timestamp")
                    .with_timezone(&chrono::Utc),
                [crate::context::AgentStatusModuleId::Time],
            )
            .expect("canonical Agent Status membership"),
        ),
    );
    store
        .initialize(&[user.clone(), MessageBlock::User(status)])
        .expect("durable seed");
    let attempt_id = AttemptId::new("historical-attempt");
    store
        .append_event(durable_event(
            &conversation_id,
            "historical-attempt-started",
            &attempt_id,
            crate::events::types::RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
        ))
        .expect("attempt start");

    let host = RuntimeClientHost::new_durable(store, None).expect("inspection host");
    let (attachment, initialized) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("durable attach");
    let RuntimeClientResult::Initialized { .. } = initialized else {
        panic!("durable attach must initialize");
    };
    let (snapshot, _) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("explicit durable checkpoint");
    assert_eq!(
        snapshot.settings_evidence,
        crate::runtime_client::settings::SettingsEvidence::HistoricalPartial
    );
    assert_eq!(
        snapshot.effective_plugins, None,
        "unavailable evidence is reported as unavailable, not reconstructed"
    );
    assert_eq!(
        snapshot.messages.len(),
        2,
        "the Agent Status evidence really is in the inspected history"
    );

    // A durable resync is the other read path into this projection, and
    // it must stay just as honest.
    let (resynced, _) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("durable resync");
    assert_eq!(resynced.effective_plugins, None);
    attachment.detach();
}

struct StagedFrameProbe {
    staged: std::sync::mpsc::Receiver<()>,
    proceed: std::sync::mpsc::Sender<()>,
    encoding: std::sync::mpsc::Receiver<()>,
    finish_encoding: std::sync::mpsc::Sender<()>,
    committed: std::sync::mpsc::Receiver<crate::observation::content::ContentRef>,
}

fn install_publication_encoding_probe(
    inner: &super::ClientInner,
    owner: Arc<super::ClientInner>,
    future_reference: crate::observation::content::ContentRef,
    before: RuntimeClientCursor,
    encoding_tx: std::sync::mpsc::Sender<()>,
    encode_release_rx: std::sync::mpsc::Receiver<()>,
) {
    *inner.publication_gate.encoding_hook.lock().unwrap() = Some(Box::new(move || {
        // Encoding is parked with the CPU publication gate held;
        // the raw state boundary must remain usable at C.
        let state = owner
            .state
            .try_lock()
            .expect("encoding releases raw Projection");
        assert_eq!(state.projection.cursor(), before);
        assert!(
            state
                .projection
                .validate_live_content_ref(&future_reference)
                .is_err()
        );
        let seed = state.projection.checkpoint_seed();
        drop(state);
        assert_eq!(seed.checkpoint_candidate().unwrap().cursor, before);
        assert_eq!(
            owner.checkpoint().unwrap().cursor,
            before,
            "the actual checkpoint reader materializes frozen C while D encodes"
        );
        let admitted = owner.admit_attachment_identity(true).unwrap();
        let (subscription, _) = owner
            .subscribe_events(admitted.attachment_id(), before)
            .unwrap();
        drop(subscription);
        owner.detach(admitted.attachment_id());
        encoding_tx.send(()).unwrap();
        encode_release_rx.recv().unwrap();
    }));
}

fn install_staged_frame_probe(inner: &Arc<super::ClientInner>) -> StagedFrameProbe {
    let (stage_tx, stage_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let (committed_tx, committed_rx) = std::sync::mpsc::channel();
    let (encoding_tx, encoding_rx) = std::sync::mpsc::channel();
    let (encode_release_tx, encode_release_rx) = std::sync::mpsc::channel();
    let mut encode_release_rx = Some(encode_release_rx);
    let mut candidate = None;
    let encoding_owner = Arc::clone(inner);
    *inner.publication_commit_hook.lock().unwrap() =
        Some(Box::new(move |inner, observations, committed| {
            let frame = super::publication_frame(observations).expect("actual staged frame");
            let state = inner.state.lock().unwrap();
            if committed {
                let (reference, before) = candidate.take().unwrap();
                state
                    .projection
                    .validate_live_content_ref(&reference)
                    .unwrap();
                assert!(state.projection.cursor() > before);
                committed_tx.send(reference).unwrap();
            } else {
                let source = state
                    .projection
                    .publication_source(ContentBlockIndex::new(0));
                let before = state.projection.cursor();
                drop(state);
                let reference = inner
                    .store
                    .content_descriptor_at(
                        &source,
                        crate::observation::content::ContentRepresentation::Utf8Text,
                        frame.sequence + 1,
                    )
                    .unwrap()
                    .expect("P already durable");
                assert!(reference.readable_end > 0);
                assert!(
                    inner
                        .state
                        .lock()
                        .unwrap()
                        .projection
                        .validate_live_content_ref(&reference)
                        .is_err()
                );
                let owner = Arc::clone(&encoding_owner);
                let future_reference = reference.clone();
                let encoding_tx = encoding_tx.clone();
                let encode_release_rx = encode_release_rx.take().unwrap();
                install_publication_encoding_probe(
                    inner,
                    owner,
                    future_reference,
                    before,
                    encoding_tx,
                    encode_release_rx,
                );
                candidate = Some((reference, before));
                stage_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            }
        }));
    StagedFrameProbe {
        staged: stage_rx,
        proceed: resume_tx,
        encoding: encoding_rx,
        finish_encoding: encode_release_tx,
        committed: committed_rx,
    }
}

/// T19: the real Agent stages P, then its native observer commits C before
/// returning to User Release. The worker is fenced so a later reader cannot
/// accidentally repair the missing synchronous transition for this proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t19_staged_frame_is_projected_before_observer_returns_to_release() {
    let (start_tx, start_rx) = model_release();
    let (finish_tx, finish_rx) = model_release();
    let (adapter, fixture) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(start_rx),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "π".repeat(40_000),
            }),
            GatedStep::ParkUntilReleased(finish_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let mut requests = adapter.request_count();
    attachment
        .handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("bounded publication"),
        })
        .result
        .unwrap();
    requests.wait_for(|count| *count == 1).await.unwrap();
    inner.park_projection_worker();
    let probe = install_staged_frame_probe(&inner);
    start_tx.send(true).unwrap();
    probe.staged.recv().unwrap();
    // The model task is parked after native durable P and before display C.
    // It cannot return through its observer or release the frame yet.
    assert!(probe.committed.try_recv().is_err());
    probe.proceed.send(()).unwrap();
    probe.encoding.recv().unwrap();
    assert!(
        probe.committed.try_recv().is_err(),
        "U cannot pass an unencoded D"
    );
    probe.finish_encoding.send(()).unwrap();
    let reference = probe.committed.recv().unwrap();
    assert!(reference.readable_end <= 80_000);
    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    finish_tx.send(true).unwrap();
    settled.await;
    assert_eq!(adapter.requests().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t19_native_audit_commits_with_worker_parked_before_execution_terminal() {
    let (_finish, finish_rx) = model_release();
    let (_, fixture) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "π".repeat(40_000),
            }),
            GatedStep::ParkUntilReleased(finish_rx),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let initial = inner.checkpoint().unwrap();
    let subscription = attachment.subscribe_events(initial.cursor).unwrap();
    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    fixture
        .host
        .submit_inbound(submit_content("audit ownership"))
        .unwrap();
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { .. }))
    })
    .await;
    let attempt = inner
        .state
        .lock()
        .unwrap()
        .projection
        .snapshot_ref()
        .attempt
        .as_ref()
        .unwrap()
        .attempt_id
        .clone()
        .expect("exact native fixture attempt identity");
    let (observed_tx, observed_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *inner.audit_commit_hook.lock().unwrap() = Some(Box::new(move |owner| {
        let state = owner
            .state
            .try_lock()
            .expect("native audit leaves the raw boundary available");
        let snapshot = state.projection.snapshot_ref();
        assert_eq!(snapshot.publication_settlements.len(), 1);
        assert_eq!(
            snapshot.publication_settlements[0].settlement.kind,
            crate::publication::PublicationAuditKind::Incomplete
        );
        assert!(matches!(
            snapshot.attempt.as_ref().unwrap().phase,
            RuntimeClientAttemptPhase::Running
        ));
        assert!(snapshot.attempt.as_ref().unwrap().in_flight.is_none());
        assert!(
            snapshot
                .messages
                .iter()
                .all(|message| !matches!(message, MessageBlock::Assistant(_)))
        );
        assert_eq!(
            state
                .projection
                .checkpoint_seed()
                .checkpoint_candidate()
                .unwrap()
                .snapshot
                .publication_settlements
                .len(),
            1,
        );
        observed_tx.send(()).unwrap();
        drop(state);
        release_rx.recv().unwrap();
    }));
    let preparation = inner.observation_preparation.lock().unwrap();
    assert_eq!(
        fixture.runtime.cancel_current_attempt(&attempt).unwrap(),
        attempt
    );
    observed_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("Audit CPU receipt does not wait for source preparation or the worker");
    release_tx.send(()).unwrap();
    drop(preparation);
    settled.await;
    inner.unpark_projection_worker();
}

fn assert_adopted_input(request: &ModelRequest, message_id: &MessageId) {
    assert!(request.messages.iter().any(|message| {
        matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if &user.id == message_id)
    }));
    assert!(request.messages.iter().any(|message| {
        matches!(
            message,
            crate::model::ModelInputMessage::Canonical(MessageBlock::User(user))
                if matches!(
                    &user.kind,
                    crate::message::types::InboundKind::Context(
                        crate::message::types::ContextKind::AgentStatus(_)
                    )
                )
        )
    }));
}

fn assert_read_domain_replay(
    inner: &super::ClientInner,
    after_cursor: RuntimeClientCursor,
    cursor: RuntimeClientCursor,
) {
    let mut state = inner.lock_state();
    let (subscriber, _) = state.projection.subscribe(after_cursor).unwrap();
    for expected in after_cursor.get() + 1..=cursor.get() {
        let super::super::projection::SubscriberPoll::Event(event) =
            state.projection.poll_subscriber(subscriber)
        else {
            panic!("derived suffix");
        };
        assert_eq!(event.cursor.get(), expected);
        assert!(
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
        );
    }
    drop(state);
}

fn wire_value<T: serde::Serialize + schemars::JsonSchema>(value: &T) -> serde_json::Value {
    let mut bytes = Vec::new();
    crate::observation::structural_json::to_writer(&mut bytes, value).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn many_live_block_steps(
    suffix_rx: watch::Receiver<bool>,
    finish_rx: watch::Receiver<bool>,
) -> Vec<GatedStep> {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let mut steps = vec![GatedStep::Emit(ModelEvent::Started)];
    for block in 0..32 {
        let mut text = String::new();
        for part in 0..100 {
            write!(
                text,
                "{:x}",
                sha2::Sha256::digest(format!("{block}/{part}").as_bytes())
            )
            .unwrap();
        }
        assert!(
            crate::observation::bounded::bounded_bytes(
                &super::super::snapshot::InFlightBlock::Text {
                    block_index: ContentBlockIndex::new(block),
                    text: text.clone()
                },
                crate::observation::bounded::CHECKPOINT_ITEM_BYTES,
            )
            .is_some(),
            "each individual native prefix fits; aggregate pressure causes deferral"
        );
        steps.push(GatedStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(block),
            text,
        }));
    }
    let generated_bytes: usize = steps
        .iter()
        .filter_map(|step| match step {
            GatedStep::Emit(ModelEvent::TextDelta { text, .. }) => Some(text.len()),
            _ => None,
        })
        .sum();
    let policy = crate::model::invocation::runtime_fallback_generation_safety_policy(512);
    assert!(generated_bytes > crate::observation::bounded::CHECKPOINT_TARGET_BYTES / 8);
    assert!(
        u64::try_from(generated_bytes + "exact final suffix π".len()).unwrap()
            < policy.max_generated_bytes,
        "aggregate capture pressure must remain valid native generated output"
    );
    steps.extend([
        GatedStep::ParkUntilReleased(suffix_rx),
        GatedStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(31),
            text: "exact final suffix π".into(),
        }),
        GatedStep::ParkUntilReleased(finish_rx),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]);
    steps
}

fn assert_live_block_count(
    checkpoint: &super::super::checkpoint::RuntimeClientCheckpoint,
    expected: usize,
) {
    assert_eq!(
        checkpoint
            .snapshot
            .attempt
            .as_ref()
            .unwrap()
            .in_flight
            .as_ref()
            .unwrap()
            .blocks
            .len(),
        expected
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t06_t14_many_live_blocks_keep_every_descriptor_after_inline_budget_exhaustion() {
    let (suffix_tx, suffix_rx) = model_release();
    let (finish_tx, finish_rx) = model_release();
    let steps = many_live_block_steps(suffix_rx, finish_rx);
    let (_, fixture) = host_fixture(vec![steps], ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let initial = inner.checkpoint().unwrap();
    let subscription = attachment.subscribe_events(initial.cursor).unwrap();
    fixture
        .host
        .submit_inbound(submit_content("all native block descriptors"))
        .unwrap();
    receive_until_metadata(&subscription, |metadata| {
        metadata
            .content
            .iter()
            .any(|binding| binding.path == ["attempt", "in_flight", "blocks", "31", "text"])
    })
    .await;
    let before = inner.checkpoint().unwrap();
    assert_live_block_count(&before, 32);
    let prefixes: Vec<_> = before
        .content
        .iter()
        .filter(|binding| {
            binding
                .path
                .starts_with(&["attempt".into(), "in_flight".into(), "blocks".into()])
                && binding.path.last().is_some_and(|field| field == "text")
        })
        .collect();
    assert_eq!(
        prefixes.len(),
        32,
        "each native loaded position keeps its exact frontier"
    );
    assert!(
        prefixes.iter().any(|binding| binding.deferred),
        "optional inline bytes exhausted the aggregate budget"
    );
    let last = prefixes
        .iter()
        .find(|binding| binding.path[3] == "31")
        .unwrap()
        .content_ref
        .clone();
    suffix_tx.send(true).unwrap();
    let update = receive_until_metadata(&subscription, |metadata| {
        metadata.content.iter().any(|binding| {
            binding.path == ["attempt", "in_flight", "blocks", "31", "text"]
                && binding.content_ref.readable_end > last.readable_end
        })
    })
    .await;
    let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &update.event else {
        unreachable!()
    };
    let event_ref = &metadata
        .content
        .iter()
        .find(|binding| binding.path == ["attempt", "in_flight", "blocks", "31", "text"])
        .unwrap()
        .content_ref;
    let after = inner.checkpoint().unwrap();
    assert_live_block_count(&after, 32);
    assert!(
        after
            .content
            .iter()
            .any(|binding| &binding.content_ref == event_ref)
    );
    assert!(event_ref.readable_end > last.readable_end);
    assert!(
        crate::observation::bounded::bounded_bytes(
            &after,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES
        )
        .is_some()
    );
    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    finish_tx.send(true).unwrap();
    settled.await;
    inner.unpark_projection_worker();
}

/// T16: identity and registration commit only bookkeeping under Projection.
/// Holding the source-preparation gate must not prevent any registration
/// linearization or its cleanup. The channel proves completion while the
/// gate remains held; no schedule-dependent delay is used.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t16_registration_and_identity_do_not_acquire_source_preparation() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().expect("resident host");
    inner.park_projection_worker();
    let cursor = inner.state.lock().unwrap().projection.cursor();
    let preparation = inner.observation_preparation.lock().unwrap();
    let (complete_tx, complete_rx) = std::sync::mpsc::channel();
    let worker_inner = inner.clone();
    let task = tokio::task::spawn_blocking(move || {
        let attachment = worker_inner.admit_attachment_identity(false).unwrap();
        let (first, _) = worker_inner
            .subscribe_events(attachment.attachment_id(), cursor)
            .unwrap();
        assert!(matches!(first.try_next(), EventDelivery::Pending));
        let (second, _) = worker_inner
            .subscribe_events(attachment.attachment_id(), cursor)
            .unwrap();
        assert!(worker_inner.subscription_superseded(attachment.attachment_id(), &first));
        assert!(matches!(first.try_next(), EventDelivery::Closed));
        drop(first);
        worker_inner.detach(attachment.attachment_id());
        assert!(matches!(second.try_next(), EventDelivery::Closed));
        drop(second);
        drop(attachment);
        complete_tx.send(()).unwrap();
    });
    complete_rx
        .recv()
        .expect("bookkeeping completes with preparation fenced");
    drop(preparation);
    task.await.unwrap();
}

/// First attachment succeeds, the second concurrent attachment is
/// rejected deterministically without evicting the first, detach
/// permits a later attachment with a distinct identity, and request
/// ids are attachment-scoped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachment_lifecycle_and_request_id_scope() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (first, initialized) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("first attach");
    let RuntimeClientResult::Initialized { attachment_id, .. } = &initialized else {
        panic!("initialized result");
    };
    let first_id = attachment_id.clone();
    // Activation's first publication is the runtime's adoption eligibility.
    assert_eq!(
        fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap()
            .1,
        RuntimeClientCursor::new(1)
    );

    let second = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION);
    assert!(matches!(
        second,
        Err(RuntimeClientError::AttachmentInUse {
            existing_attachment_id,
        }) if existing_attachment_id == first_id
    ));

    // The rejection never evicts the first attachment: its requests
    // still work.
    let response = first.handle_request(RuntimeClientRequest::CheckpointGet {
        id: crate::runtime_client::RequestId::new(1),
    });
    assert_eq!(response.id.get(), 1);
    assert!(response.error.is_none());
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::Checkpoint { .. })
    ));

    // Incompatible protocol version fails explicitly.
    let bad = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1);
    assert!(matches!(
        bad,
        Err(RuntimeClientError::UnsupportedProtocolVersion {
            requested,
            ..
        }) if requested == crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1
    ));

    // Explicit detach releases the attachment; a fresh attachment has
    // a distinct identity and a fresh request-id scope.
    first.detach();
    let (second_attachment, initialized) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach after detach");
    let RuntimeClientResult::Initialized {
        attachment_id: second_id,
        ..
    } = initialized
    else {
        panic!("initialized result");
    };
    assert_ne!(first_id, second_id);
    let response = second_attachment.handle_request(RuntimeClientRequest::CheckpointGet {
        id: crate::runtime_client::RequestId::new(1),
    });
    assert_eq!(response.id.get(), 1, "request ids are attachment-scoped");
    assert!(response.error.is_none());
}

/// Read-only inspection attachments share the live projection without
/// competing with the one control owner or changing provider admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_attachments_coexist_with_the_control_attachment() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (control, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("control attachment");
    let (inspection_one, _) = fixture
        .host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("first read-only attachment");
    let (inspection_two, _) = fixture
        .host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("second read-only attachment");
    assert_ne!(
        inspection_one.attachment_id(),
        inspection_two.attachment_id()
    );

    let rejection = inspection_one.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
        id: crate::runtime_client::RequestId::new(1),
    });
    assert!(matches!(
        rejection.error,
        Some(RuntimeClientError::InvalidState { message })
            if message == "conversation inspection is read-only"
    ));
    assert!(
        inspection_two
            .handle_request(RuntimeClientRequest::CheckpointGet {
                id: crate::runtime_client::RequestId::new(2),
            })
            .error
            .is_none()
    );
    assert!(
        control
            .handle_request(RuntimeClientRequest::CheckpointGet {
                id: crate::runtime_client::RequestId::new(3),
            })
            .error
            .is_none(),
        "read-only attachments do not displace control ownership"
    );
}

/// Dropping the attachment releases it (RAII detach semantics).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_the_attachment_detaches_it() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    drop(attachment);
    let (second, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach after drop");
    assert!(
        second
            .handle_request(RuntimeClientRequest::CheckpointGet {
                id: crate::runtime_client::RequestId::new(1),
            })
            .error
            .is_none()
    );
}

/// A detached handle rejects requests deterministically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detached_handle_rejects_requests() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    attachment.detach();
    let response = attachment.handle_request(RuntimeClientRequest::CheckpointGet {
        id: crate::runtime_client::RequestId::new(7),
    });
    assert_eq!(response.id.get(), 7);
    assert!(matches!(
        response.error,
        Some(RuntimeClientError::NotAttached)
    ));
}

fn print_agent_request_delta(
    turns: usize,
    current: &crate::agent::execution::attribution::AgentRequestProfile,
    previous: &crate::agent::execution::attribution::AgentRequestProfile,
) {
    for (name, current, previous) in [
        ("prepare", &current.prepare, &previous.prepare),
        ("start", &current.start, &previous.start),
        ("commit", &current.commit, &previous.commit),
        (
            "commit_evidence",
            &current.commit_evidence,
            &previous.commit_evidence,
        ),
        (
            "context_projection",
            &current.context_projection,
            &previous.context_projection,
        ),
        ("source", &current.source, &previous.source),
        (
            "source_capture",
            &current.source_capture,
            &previous.source_capture,
        ),
        (
            "source_authenticate",
            &current.source_authenticate,
            &previous.source_authenticate,
        ),
        ("source_leaf", &current.source_leaf, &previous.source_leaf),
        (
            "source_scalar",
            &current.source_scalar,
            &previous.source_scalar,
        ),
        (
            "source_paths",
            &current.source_paths,
            &previous.source_paths,
        ),
        (
            "source_length",
            &current.source_length,
            &previous.source_length,
        ),
        (
            "context_observe",
            &current.context_observe,
            &previous.context_observe,
        ),
        (
            "context_measure",
            &current.context_measure,
            &previous.context_measure,
        ),
        (
            "input_assembly",
            &current.input_assembly,
            &previous.input_assembly,
        ),
    ] {
        eprintln!(
            "native agent phase turns={turns} phase={name} calls={} inclusive_us={}",
            current.calls - previous.calls,
            current.total_us - previous.total_us,
        );
    }
}

fn print_agent_work_delta(
    turns: usize,
    current: &crate::agent::execution::attribution::AgentRequestProfile,
    previous: &crate::agent::execution::attribution::AgentRequestProfile,
) {
    for (scope, current, previous) in [
        ("request", &current.source_work, &previous.source_work),
        (
            "authenticate",
            &current.source_auth_work,
            &previous.source_auth_work,
        ),
        (
            "new_leaf",
            &current.source_leaf_work,
            &previous.source_leaf_work,
        ),
    ] {
        eprintln!(
            "native source work turns={turns} scope={scope} inclusive={:?}",
            current.since(previous)
        );
    }
    eprintln!(
        "native context work turns={turns} delta={:?}",
        current.context_work.since(&previous.context_work)
    );
    eprintln!(
        "native input work turns={turns} delta={:?}",
        current.native_work.since(&previous.native_work)
    );
}

fn assert_agent_profile_complete(
    agent: &crate::agent::execution::attribution::AgentRequestProfile,
    turns: usize,
) {
    let turns = u64::try_from(turns).unwrap();
    for phase in [
        agent.prepare,
        agent.start,
        agent.commit,
        agent.commit_evidence,
        agent.source,
        agent.source_capture,
        agent.source_scalar,
        agent.source_paths,
        agent.source_length,
    ] {
        assert_eq!(phase.calls, turns);
    }
    assert_eq!(agent.source_work.samples, turns);
    assert!(agent.source_work.index_pages > 0 && agent.source_work.index_bytes > 0);
    assert!(agent.source_work.metadata_blobs > 0 && agent.source_work.metadata_bytes > 0);
    assert!(
        agent.source_work.native_header_queries > 0 && agent.source_work.native_node_queries > 0
    );
    assert!(agent.source_work.new_leaves > 0 && agent.source_work.new_nodes > 0);
    if turns > 1 {
        assert!(agent.source_auth_work.samples > 0);
    }
}

async fn await_profile_settlement(subscription: &EventSubscription) {
    let events = receive_until(subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    assert_eq!(
        events
            .iter()
            .flat_map(|event| event.event.facts())
            .filter(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            .count(),
        1
    );
}

/// Measures inclusive elapsed phases of frozen publication and native execution.
/// Source receipts, runtime admission and every settlement remain native.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measurement only; run explicitly with --ignored --nocapture"]
async fn t19_native_publication_encoding_profile() {
    let turns = std::env::var("RUSTX_PUBLICATION_PROFILE_TURNS").map_or(20, |value| {
        value.parse::<usize>().expect("profile turn count")
    });
    assert!((1..=1000).contains(&turns));
    let (adapter, fixture) = host_fixture_with_native_tools(
        (0..turns).map(|_| one_turn_stop()).collect(),
        ToolRegistry::new(),
        status_engine(),
        true,
    )
    .await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let profile = Arc::new(Mutex::new(
        super::super::projection::PublicationEncodingProfile::default(),
    ));
    inner
        .state
        .lock()
        .unwrap()
        .projection
        .profile_encoding(Arc::clone(&profile));
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = attachment
        .subscribe_events(fixture.host.checkpoint().unwrap().cursor)
        .unwrap();
    let started = std::time::Instant::now();
    let mut previous = super::super::projection::PublicationEncodingProfile::default();
    let agent_profile = crate::agent::execution::attribution::start();
    let mut previous_agent = agent_profile.snapshot();
    for turn in 0..turns {
        fixture
            .host
            .submit_inbound(submit_content(&format!("encoding measurement {turn}")))
            .unwrap();
        await_profile_settlement(&subscription).await;
        if (turn + 1) % 25 == 0 {
            let current = profile.lock().unwrap().clone();
            eprintln!(
                "native encoding turns={} elapsed_ms={} records={} bytes={} seed_us={} capture_us={} fit_us={} probe_us={} encode_us={} content_metadata_max={}",
                turn + 1,
                started.elapsed().as_millis(),
                current.records - previous.records,
                current.bytes - previous.bytes,
                current.seed_us - previous.seed_us,
                current.capture_us - previous.capture_us,
                current.fit_us - previous.fit_us,
                current.probe_us - previous.probe_us,
                current.encode_us - previous.encode_us,
                current.content_metadata_max
            );
            previous = current;
            let current_agent = agent_profile.snapshot();
            print_agent_request_delta(turn + 1, &current_agent, &previous_agent);
            print_agent_work_delta(turn + 1, &current_agent, &previous_agent);
            previous_agent = current_agent;
        }
    }
    assert_eq!(adapter.requests().len(), turns);
    let elapsed = started.elapsed();
    fixture.runtime.shutdown().await.unwrap();
    let profile = profile.lock().unwrap();
    assert!(profile.records >= turns as u64);
    assert!(profile.bytes > 0);
    let agent = agent_profile.snapshot();
    assert_agent_profile_complete(&agent, turns);
    eprintln!(
        "native publication encoding{turns} elapsed_ms={} {profile:?} agent_inclusive={agent:?}",
        elapsed.as_millis()
    );
}

/// Captures real native authority on both sides of a contiguous stream.
/// The TypeScript regression consumes this wire capture; its expected state
/// is produced only by an independent native snapshot read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One end-to-end capture keeps the independent authorities visible.
async fn incremental_projection_independent_snapshot_capture() {
    use crate::app_server::protocol::{
        AttachmentTarget, JsonRpcVersion, MethodResult, Notification, NotificationMethod,
        RequestId as WireId, Response, Success,
    };
    let (tool, _started, release) = ParkingBackgroundTool::new();
    release.send_replace(true);
    let definition = ToolDefinition {
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        ..tool.definition.clone()
    };
    let mut tools = ToolRegistry::new();
    tools.register(definition.clone(), Arc::new(tool)).unwrap();
    let call_id = ToolCallId::new("capture-call");
    let script = vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::Emit(ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "reasoning".into(),
        }),
        GatedStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(1),
            text: "first ".into(),
        }),
        GatedStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(1),
            text: "second".into(),
        }),
        GatedStep::Emit(ModelEvent::RefusalDelta {
            block_index: ContentBlockIndex::new(2),
            text: "refusal".into(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(3),
            call: crate::tools::types::ToolCallStart {
                id: call_id.clone(),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(3),
            call_id: call_id.clone(),
            arguments_delta: "{".into(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(3),
            call_id: call_id.clone(),
            arguments_delta: "}".into(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(3),
            call: ToolCall {
                id: call_id,
                tool_id: definition.id,
                name: definition.name,
                arguments: serde_json::json!({}),
            },
        }),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ];
    let (_, fixture) = host_fixture(vec![script, one_turn_stop()], tools, status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let initial = fixture.host.inner.checkpoint().unwrap();
    let c = initial.cursor;
    let subscription = attachment.subscribe_events(c).unwrap();
    fixture
        .host
        .submit_inbound(submit_content("projection equivalence"))
        .unwrap();
    let mut events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    // Attempt settlement is a semantic event, not completion of the
    // independently owned presentation read. Capture the equivalence cut
    // only after the terminal response's decorated domain was published.
    let terminal_read = |event: &RuntimeClientProtocolEvent| {
        event.event.facts().iter().any(|fact| {
            matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { transcript, .. }
                if transcript.entries.iter().any(|entry| entry.completed_response.is_some()))
        })
    };
    if !events.iter().any(terminal_read) {
        events.extend(receive_until(&subscription, terminal_read).await);
    }
    let expected = fixture.host.inner.checkpoint().unwrap();
    let n = expected.cursor;
    while events.last().is_none_or(|event| event.cursor < n) {
        let EventDelivery::Event(event) = subscription.next().await else {
            panic!("contiguous event");
        };
        events.push(event);
    }
    let mut cursor = c.get();
    for event in &events {
        cursor += 1;
        assert_eq!(event.cursor.get(), cursor);
    }
    assert_eq!(cursor, n.get());
    let (expired_cursor, expired_reference) =
        events
            .iter()
            .find_map(|event| {
                event.event.facts().iter().find_map(|fact| match fact {
                RuntimeClientEvent::BlockContentAdvanced { content_ref, .. }
                    if content_ref.readable_end > 0
                        && matches!(
                            &content_ref.source,
                            crate::observation::content::ContentSource::LivePublicationPrefix { .. }
                        ) => Some((event.cursor, content_ref.clone())),
                _ => None,
            })
            })
            .expect("actual released nonempty LiveRef in the original native stream");
    // Canonical acceptance has expired this untouched original Live source.
    // Neither Replay retention nor a later read can redirect it to C/Audit.
    assert!(matches!(
        fixture
            .store
            .as_ref()
            .unwrap()
            .content_read(&expired_reference, 0, 1),
        Err(crate::durable::ConversationStoreError::InvalidReference(_))
    ));
    assert!(matches!(
        fixture.host.inner.content_read(&expired_reference, 0, 1),
        Err(RuntimeClientError::ContentSourceUnavailable)
    ));
    // Subscribe only after expiry. These are retained native records, not
    // the already-consumed live delivery vector or a synthetic replay.
    let replay = attachment.subscribe_events(c).unwrap();
    let mut replayed = Vec::with_capacity(events.len());
    for original in &events {
        let EventDelivery::Event(event) = replay.try_next() else {
            panic!("the exact original cut remains serviceable after Live expiry");
        };
        assert_eq!(event.cursor, original.cursor);
        replayed.push(event);
    }
    assert_eq!(replayed, events);
    assert_eq!(replayed.last().unwrap().cursor, n);
    assert!(expected.snapshot.transcript.statistics.is_some());
    assert!(
        expected
            .snapshot
            .transcript
            .entries
            .iter()
            .any(|entry| entry.completed_response.is_some())
    );
    let checkpoint = |value| {
        wire_value(&Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: WireId::String("capture".into()),
            result: MethodResult::Checkpoint {
                checkpoint: Box::new(value),
            },
        })))
    };
    let target = AttachmentTarget {
        session_id: crate::local_runtime::session::SessionId::new(
            "ses_00000000-0000-7000-8000-000000000420",
        ),
        conversation_id: initial.snapshot.conversation_id.clone(),
        runtime_incarnation: serde_json::from_str("1").unwrap(),
        attachment_id: attachment.attachment_id().clone(),
    };
    let wire: Vec<_> = replayed
        .into_iter()
        .map(|event| {
            wire_value(&Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification: NotificationMethod::Event {
                    target: target.clone(),
                    registration_id: "capture-registration".into(),
                    cursor: event.cursor,
                    event: Box::new(event.event),
                },
            })
        })
        .collect();
    if let Ok(path) = std::env::var("RUSTX_PROJECTION_CAPTURE") {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({
                "initial": checkpoint(initial), "events": wire, "expected": checkpoint(expected),
                "expired_live_reference": {
                    "cursor": expired_cursor.get().to_string(),
                    "reference": wire_value(&expired_reference),
                },
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

/// Submitting while idle admits and runs the attempt through the
/// conversation runtime's single admission path; the admission response
/// is accepted, not finished; the attempt settles and the canonical
/// history is committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_when_idle_admits_and_runs_the_attempt() {
    let (adapter, fixture) =
        host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("hello"),
    });
    let RuntimeClientResult::InboundAccepted {
        message_id,
        inbound_sequence,
    } = response.result.expect("accepted")
    else {
        panic!("accepted result");
    };
    assert_eq!(inbound_sequence.get(), 1);
    assert_eq!(
        message_id.as_str(),
        "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-1"
    );

    // The attempt settles asynchronously; the subscription observes the
    // terminal settlement exactly once.
    let mut settled = 0;
    let events = receive_until(&subscription, |event| {
        if event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        {
            settled += 1;
            return true;
        }
        false
    })
    .await;
    let settled_events: Vec<_> = events
        .iter()
        .filter(|event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .collect();
    assert_eq!(settled_events.len(), 1);
    assert_eq!(settled, 1);

    // The first model request observed the admitted Runtime Agent Status
    // fact through canonical history.
    let requests = adapter.requests();
    assert_eq!(requests.len(), 1);
    assert_adopted_input(&requests[0], &message_id);

    // The snapshot carries the committed canonical history and the
    // settled attempt.
    let (snapshot, _) = checkpoint_state(&fixture.host);
    assert_eq!(
        snapshot.messages.len(),
        3,
        "user message + admitted Agent Status + Assistant message"
    );
    assert!(matches!(
        snapshot.attempt.expect("attempt view").phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
    // The terminal settlement event is emitted by the loop, and the
    // authoritative canonical history is committed by the runtime's
    // settlement path immediately afterwards. Observing the commit is a
    // wait on that exact condition, never a delay.
    await_canonical_history(&fixture.host, &snapshot.messages).await;

    // Request facts remain in the durable ConversationStore after the
    // AgentExecutionResult transfer. Mutate the live session
    // configuration after settlement and reconstruct from the durable
    // snapshot plus its historical Surface; neither current
    // configuration nor a live contributor is consulted.
    let requests = adapter.requests();
    let history = fixture.host.request_history();
    let snapshots = request_snapshots(&history);
    assert_eq!(snapshots.len(), 1);
    let retained = snapshots[0].clone();
    let mut live_config = fixture.runtime.model_config();
    live_config.request_params.insert(
        "live_mutation".to_owned(),
        serde_json::json!("changed-after-settlement"),
    );
    fixture
        .host
        .model_set(live_config)
        .expect("live model mutation remains valid");
    let reconstructed = fixture
        .host
        .reconstruct_request(&retained.identity)
        .expect("retained request reconstructs after settlement");
    assert_eq!(reconstructed, requests[0]);
    assert_eq!(
        history.get(&retained.identity).unwrap(),
        Some(retained),
        "request history lookup is identity-based and immutable"
    );
}

/// A composed runtime keeps every actual primary request, including an
/// overflow retry, reconstructable in the durable `ConversationStore`
/// after the `AgentExecutionResult` has been transferred and dropped. The
/// retry keeps the pending fresh inbound visible while both request facts
/// remain reconstructable from their own Surface revisions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one durable overflow/retry scenario
async fn settled_host_retains_distinct_overflow_request_snapshots() {
    let (adapter, fixture) = host_fixture(
        vec![
            one_turn_stop(),
            vec![GatedStep::Emit(ModelEvent::Failed {
                error: ModelError {
                    kind: ModelErrorKind::ContextWindowExceeded,
                    message: "context window exceeded".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
            })],
            vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "historical summary".to_owned(),
                }),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
            one_turn_stop(),
        ],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("first"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    await_request_history_len(&fixture.host, 1).await;

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(2),
        content: submit_content("second"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    await_request_history_len(&fixture.host, 3).await;

    let history = fixture.host.request_history();
    let snapshots = request_snapshots(&history);
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots[0].identity.retry_number, 0);
    assert_eq!(snapshots[1].identity.retry_number, 0);
    assert_eq!(snapshots[2].identity.retry_number, 1);
    assert_eq!(
        snapshots[1].identity.attempt_id,
        snapshots[2].identity.attempt_id
    );
    assert_eq!(
        snapshots[1].context_generation, snapshots[2].context_generation,
        "overflow retry keeps the one admitted context generation"
    );
    assert_ne!(
        snapshots[1].history_cut.activation, snapshots[2].history_cut.activation,
        "compaction gives the retry a distinct typed historical activation"
    );

    let provider_requests = adapter.requests();
    assert_eq!(
        provider_requests.len(),
        4,
        "three primary requests plus summary"
    );
    for (snapshot, request) in snapshots.iter().zip([
        &provider_requests[0],
        &provider_requests[1],
        &provider_requests[3],
    ]) {
        assert_eq!(
            fixture
                .host
                .reconstruct_request(&snapshot.identity)
                .expect("settled historical request reconstructs"),
            *request
        );
    }
    assert!(
        provider_requests[3].messages.iter().any(|message| {
            matches!(
                message,
                crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id.as_str() == "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-2"
            )
        }),
        "the retry still observes the pending fresh inbound"
    );
}

/// Waits until the runtime owns the conversation state again and its
/// Message Ledger equals the expected records.
async fn await_canonical_history(host: &RuntimeClientHost, expected: &[MessageBlock]) {
    tokio::time::timeout(std::time::Duration::from_mins(2), async {
        loop {
            if host.host_ledger().as_deref() == Some(expected) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the projection mirrors the authoritative canonical history");
}

/// Waits for the durable request-fact read to expose the expected count.
async fn await_request_history_len(host: &RuntimeClientHost, expected: usize) {
    tokio::time::timeout(std::time::Duration::from_mins(2), async {
        loop {
            if request_snapshots(&host.request_history()).len() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request history transfer must settle");
}

/// Waits until the fake provider has actually received the expected
/// number of provider-neutral requests. Request snapshots are durable
/// before adapter invocation, so request-history visibility alone is not
/// a sufficient synchronization point for provider-side assertions.
async fn await_adapter_request_count(adapter: &GatedAdapter, expected: usize) {
    let mut count = adapter.request_count();
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        count.wait_for(|actual| *actual >= expected),
    )
    .await
    .expect("provider invocation must settle")
    .expect("provider request-count signal must stay open");
}

/// Submitting while an attempt is running queues the message in the
/// authoritative mailbox; the running attempt drains it at its next
/// safe boundary. An enqueue during an active attempt never creates a
/// second `AgentExecution`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_while_busy_queues_for_the_next_drain() {
    let (release_tx, release_rx) = model_release();
    let (adapter, fixture) = host_fixture(
        vec![
            // Turn 1 parks in its stream; after release it completes
            // with Stop and the safe boundary drains the queued
            // message into a second turn.
            vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "working".to_owned(),
                }),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
            one_turn_stop(),
        ],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    let first = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("first"),
    });
    assert!(matches!(
        first.result,
        Some(RuntimeClientResult::InboundAccepted { .. })
    ));

    // Wait until the first request is in flight (the adapter has been
    // asked) so the second submit provably arrives while busy.
    receive_until(&subscription, |event| {
        event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { kind, .. } if kind == "text"))
    })
    .await;
    assert_eq!(adapter.requests().len(), 1);
    assert!(fixture.host.has_current_attempt());

    let second = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(2),
        content: submit_content("second"),
    });
    let RuntimeClientResult::InboundAccepted {
        message_id: second_id,
        inbound_sequence,
    } = second.result.expect("accepted")
    else {
        panic!("accepted result");
    };
    assert_eq!(inbound_sequence.get(), 2);

    // While the attempt is parked, the second message remains pending
    // in the authoritative mailbox diagnostics, and no second
    // AgentExecution exists.
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(snapshot.inbound.pending.len(), 1);
    assert_eq!(snapshot.inbound.pending[0].message.id, second_id);
    assert_eq!(adapter.requests().len(), 1, "no second AgentExecution yet");

    // Release the parked turn: the safe boundary drains the queued
    // message and a second turn observes it within the SAME attempt.
    release_tx.send(true).expect("release");
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let requests = adapter.requests();
    assert_eq!(requests.len(), 2, "the drained batch opens a second turn");
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == second_id))
    );
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(
        snapshot.inbound.pending.is_empty(),
        "the queued message was drained exactly once"
    );
    assert!(
        snapshot
            .messages
            .iter()
            .any(|message| { matches!(message, MessageBlock::User(user) if user.id == second_id) }),
        "the drained message committed to canonical history"
    );
}

/// Cancelling the current attempt: the acceptance response is not
/// terminal settlement; the actual runtime settlement is observed
/// exactly once after the model releases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_current_attempt_acceptance_is_not_settlement() {
    let (release_tx, release_rx) = model_release();
    let (_, fixture) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(release_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
    })
    .await;

    // The cancel response is acceptance, never terminal settlement.
    let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
        id: crate::runtime_client::RequestId::new(2),
    });
    let Some(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }) = response.result
    else {
        panic!("cancellation of a running attempt is accepted, got {response:?}");
    };
    let (snapshot, _) = checkpoint_state(&fixture.host);
    let view = snapshot.attempt.expect("attempt view");
    assert_eq!(
        view.attempt_id,
        Some(attempt_id),
        "acceptance names the attempt the snapshot describes"
    );
    assert!(
        matches!(
            view.phase,
            RuntimeClientAttemptPhase::Running | RuntimeClientAttemptPhase::Settled { .. }
        ),
        "an accepted cancellation leaves the attempt running or already settled"
    );

    // A second cancel is idempotent at the signal level.
    let second_cancel = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
        id: crate::runtime_client::RequestId::new(3),
    });
    assert!(
        second_cancel.error.is_none()
            || matches!(
                second_cancel.error,
                Some(RuntimeClientError::NoCurrentAttempt)
            ),
        "a second cancel is accepted or reports no cancellable attempt, got {second_cancel:?}"
    );

    // Release the parked model so it can finish.
    let _ = release_tx.send(true);
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let settled: Vec<_> = events
        .iter()
        .filter(|event| {
            event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled {
                    outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled { .. },
                    ..
                }))
        })
        .collect();
    assert_eq!(
        settled.len(),
        1,
        "terminal cancellation observed exactly once"
    );
    let (snapshot, _) = checkpoint_state(&fixture.host);
    assert!(matches!(
        snapshot.attempt.expect("attempt view").phase,
        RuntimeClientAttemptPhase::Settled {
            outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled { .. }
        }
    ));
}

/// With no attempt running, cancel returns a deterministic typed
/// error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_with_no_attempt_fails_typed() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
        id: crate::runtime_client::RequestId::new(1),
    });
    assert!(matches!(
        response.error,
        Some(RuntimeClientError::NoCurrentAttempt)
    ));
}

/// Detaching an attachment never cancels the active attempt: the
/// attempt continues to settlement after detach, and a reattached
/// client observes the terminal settlement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_is_not_cancellation() {
    let (release_tx, release_rx) = model_release();
    let (adapter, fixture) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(release_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    // Native request-start publication precedes provider invocation. Use
    // that exact signal instead of polling snapshots through moving cuts.
    await_adapter_request_count(&adapter, 1).await;
    // Detach while the attempt is parked.
    attachment.detach();
    // A reattached client sees the attempt still running.
    let (second, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("reattach");
    let (snapshot, cursor) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(
        matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Running
        ),
        "detach never cancels the attempt"
    );
    // The attempt completes normally after release.
    release_tx.send(true).expect("release");
    let subscription = second
        .subscribe_events(cursor)
        .expect("resume from the retained cursor");
    receive_until(&subscription, |event| {
        event.event.facts().iter().any(|fact| {
            matches!(
                fact,
                RuntimeClientEvent::AttemptSettled {
                    outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed { .. },
                    ..
                }
            )
        })
    })
    .await;
}

fn publish_pending_mutation(
    inner: &Arc<super::ClientInner>,
    outcome: crate::durable::inbox::PendingMutationOutcome,
) {
    let crate::durable::inbox::PendingMutationOutcome::Applied {
        pending_observation,
        read_cut,
    } = outcome
    else {
        panic!("native pending mutation applied");
    };
    let sink = super::NativePublicationProjection {
        inner: Arc::downgrade(inner),
    };
    crate::runtime::observation::PublicationProjectionCommit::native_pending(
        &sink,
        pending_observation.as_deref(),
        &read_cut,
    );
    if let Some(cursor) = pending_observation.as_ref().and_then(|receipt| {
        receipt
            .selected
            .as_ref()
            .and_then(|header| header.message.transcript_cursor)
            .or(receipt.removed_transcript)
    }) {
        inner
            .store
            .mark_observation_transcript_ready(cursor.get())
            .unwrap();
    }
}

fn pending_terminal_diagnostic(store: &dyn ConversationStore, attempt: &AttemptId) -> Vec<String> {
    let mut after = None;
    let mut diagnostics = Vec::new();
    for _ in 0..4 {
        let page = store
            .read_events(after, 32)
            .expect("original fixture journal remains readable");
        if page.events.is_empty() {
            break;
        }
        for envelope in page.events {
            after = Some(envelope.sequence);
            if envelope.attempt_id.as_ref() != Some(attempt) {
                continue;
            }
            match envelope.event {
                RuntimeEvent::ModelRequestFailed { error, .. } => diagnostics.push(format!(
                    "J{} model_failed {:?} phase={:?}",
                    envelope.sequence, error.kind, error.timeout_phase
                )),
                RuntimeEvent::AttemptCompleted { .. } => {
                    diagnostics.push(format!("J{} completed", envelope.sequence));
                }
                RuntimeEvent::AttemptCancelled { reason, .. } => {
                    diagnostics.push(format!("J{} cancelled {reason:?}", envelope.sequence));
                }
                RuntimeEvent::AttemptFailed { .. } => {
                    diagnostics.push(format!("J{} failed", envelope.sequence));
                }
                RuntimeEvent::AttemptTimedOut { .. } => {
                    diagnostics.push(format!("J{} timed_out", envelope.sequence));
                }
                RuntimeEvent::AttemptLimitExceeded { .. } => {
                    diagnostics.push(format!("J{} limit_exceeded", envelope.sequence));
                }
                _ => {}
            }
            if diagnostics.len() == 8 {
                return diagnostics;
            }
        }
    }
    diagnostics
}

async fn await_pending_fixture_publication(host: &RuntimeClientHost) {
    tokio::time::timeout(BACKGROUND_LIVENESS_GUARD, host.projection_settled())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "pending owner publication exceeded liveness guard: {}",
                host.weak_inner()
                    .upgrade()
                    .unwrap()
                    .projection_source_debug()
            )
        });
}

async fn assert_native_pending_removal_wakes_worker(fixture: &HostFixture, attempt: &AttemptId) {
    let mailbox = fixture.runtime.tool_runtime().mailbox();
    let sequence = mailbox
        .enqueue(inbound_text(
            "native-removal",
            "remove through mailbox owner",
        ))
        .unwrap();
    await_pending_fixture_publication(&fixture.host).await;
    let before = fixture.host.checkpoint().unwrap();
    assert_eq!(before.snapshot.inbound.pending.len(), 1);
    let removed = mailbox
        .remove_pending(&crate::durable::inbox::PendingInboundRef {
            sequence,
            revision: 0,
        })
        .unwrap();
    let crate::durable::inbox::PendingMutationOutcome::Applied {
        pending_observation: Some(receipt),
        read_cut,
    } = removed
    else {
        panic!("actual native removal receipt");
    };
    assert!(receipt.selected.is_none());
    assert!(receipt.removed_transcript.is_some());
    await_pending_fixture_publication(&fixture.host).await;
    let after = fixture.host.checkpoint().unwrap();
    assert!(after.cursor > before.cursor);
    assert!(after.snapshot.inbound.pending.is_empty());
    assert_eq!(after.read_cut.source_cut.as_ref().unwrap().read, read_cut);
    assert_eq!(
        after.snapshot.attempt.as_ref().unwrap().attempt_id.as_ref(),
        Some(attempt)
    );
    assert!(fixture.runtime.has_current_attempt());
}

async fn gated_pending_fixture(
    release_rx: tokio::sync::watch::Receiver<bool>,
) -> (Arc<GatedAdapter>, HostFixture) {
    let clock = Arc::new(crate::runtime::monotonic::ManualMonotonicClock::new());
    let (adapter, fixture) = host_fixture_before_activation(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(release_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
        false,
        Some(clock),
    )
    .await;
    fixture.runtime.activate();
    await_fixture_activation(&fixture.host).await;
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text("admitted", "running"))
        .unwrap();
    // Request-start publication precedes final prompt admission. Wait for
    // the actual provider invocation so this message stays pending behind
    // the parked Attempt instead of racing into its initial prompt.
    await_adapter_request_count(&adapter, 1).await;
    (adapter, fixture)
}

async fn finish_pending_fixture(
    fixture: &HostFixture,
    adapter: &GatedAdapter,
    release_tx: &watch::Sender<bool>,
    held_attempt: &AttemptId,
) {
    let store = fixture.runtime.tool_runtime().durable_store();
    release_tx.send(true).unwrap_or_else(|_| {
        panic!(
            "native gate receiver retired: {:?}",
            pending_terminal_diagnostic(store.as_ref(), held_attempt)
        )
    });
    let settled = fixture.runtime.child_attempt_settlement(0).await;
    assert_eq!(&settled.attempt_id, held_attempt);
    assert!(matches!(
        settled.outcome,
        crate::runtime::conversation_runtime::NativeAttemptOutcome::Completed
    ));
    assert_eq!(adapter.requests().len(), 1);
    fixture.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_snapshot_and_reattach_repair_a_committed_unpublished_mutation() {
    let (release_tx, release_rx) = model_release();
    let (adapter, fixture) = gated_pending_fixture(release_rx).await;
    let mailbox = fixture.runtime.tool_runtime().mailbox();
    let sequence = mailbox
        .enqueue(inbound_text("pending-repair", "old"))
        .unwrap();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    await_pending_fixture_publication(&fixture.host).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let (before, cursor) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .unwrap();
    assert_eq!(before.inbound.pending.len(), 1);
    let held_attempt = before
        .attempt
        .as_ref()
        .expect("actual gated native attempt")
        .attempt_id
        .clone()
        .expect("exact native fixture attempt identity");
    assert!(fixture.runtime.has_current_attempt());
    assert!(matches!(
        before.attempt.as_ref().unwrap().phase,
        RuntimeClientAttemptPhase::Running
    ));
    let store = fixture.runtime.tool_runtime().durable_store();
    let expected = crate::durable::inbox::PendingInboundRef {
        sequence,
        revision: 0,
    };
    // The durable mutation precedes its exact mailbox-owner receipt.
    // Published state remains frozen until that native owner installs it.
    let edited = store.edit_pending(&expected, "committed edit").unwrap();
    let (published_cursor, published_inbound) = {
        let published = inner.lock_published_state();
        (
            published.projection.cursor(),
            published.projection.snapshot_ref().inbound.clone(),
        )
    };
    assert_eq!(published_cursor, cursor);
    assert_eq!(published_inbound, before.inbound);
    publish_pending_mutation(&inner, edited);
    inner.unpark_projection_worker();
    await_pending_fixture_publication(&fixture.host).await;
    let (repaired, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .unwrap();
    assert_eq!(repaired.inbound.pending[0].revision, 1);
    assert_eq!(
        repaired.inbound.pending[0].message.content,
        inbound_text("x", "committed edit").content
    );
    let removed = store
        .remove_pending(&crate::durable::inbox::PendingInboundRef {
            revision: 1,
            ..expected
        })
        .unwrap();
    publish_pending_mutation(&inner, removed);
    // The existing source worker installs the completed read-domain
    // result before the reconnect baseline; public reads remain pure.
    await_pending_fixture_publication(&fixture.host).await;
    assert_native_pending_removal_wakes_worker(&fixture, &held_attempt).await;
    attachment.detach();
    let (_, initialized) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let RuntimeClientResult::Initialized { .. } = initialized else {
        panic!("initialized")
    };
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("explicit reconnect checkpoint");
    assert!(snapshot.inbound.pending.is_empty());
    assert!(!format!("{:?}", snapshot.transcript).contains("pending-repair"));
    assert!(
        fixture.runtime.has_current_attempt()
            && snapshot
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.attempt_id.as_ref() == Some(&held_attempt)
                    && matches!(attempt.phase, RuntimeClientAttemptPhase::Running)),
        "native gated execution retired before release: {:?}",
        pending_terminal_diagnostic(store.as_ref(), &held_attempt)
    );
    finish_pending_fixture(&fixture, &adapter, &release_tx, &held_attempt).await;
}

/// Detaching never cancels conversation-owned background work and
/// never drains mailbox contents: the mailbox is drained only by the
/// conversation runtime's admission/safe-boundary authority, never by
/// the client boundary (Test 2).
#[allow(clippy::too_many_lines)] // one complete detach lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_never_mutates_background_or_mailbox_state() {
    let (release_tx, release_rx) = model_release();
    let (adapter, fixture) = host_fixture(
        vec![
            vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
            one_turn_stop(),
        ],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    // Dispatch one detached background execution directly through the
    // authoritative registry.
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = fixture
        .runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-bg"),
                },
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                mode: ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            },
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
        .runtime
        .tool_runtime()
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits")
    else {
        panic!("accepted dispatch");
    };
    await_background_started(&mut started, "background runner started").await;
    // One mailbox item admitted by the runtime's idle wakeup: the first
    // attempt starts and parks in its model stream.
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text("msg-first", "admitted"))
        .expect("enqueue");
    await_adapter_request_count(&adapter, 1).await;
    // Exact pending removal cannot cancel the parked attempt or the
    // independently committed background execution.
    let mailbox = fixture.runtime.tool_runtime().mailbox();
    let sequence = mailbox
        .enqueue(inbound_text("msg-remove", "remove only this occurrence"))
        .unwrap();
    let before_background = fixture
        .runtime
        .tool_runtime()
        .background()
        .snapshot(&execution_id);
    assert!(matches!(
    mailbox
                    .remove_pending(&crate::durable::inbox::PendingInboundRef {
                        sequence,
                        revision: 0,
                    })
                    .unwrap(),
    crate::durable::inbox::PendingMutationOutcome::Applied { read_cut, .. } if read_cut.pending_total <= read_cut.pending_through
    ));
    assert_eq!(
        fixture
            .runtime
            .tool_runtime()
            .background()
            .snapshot(&execution_id),
        before_background
    );
    assert!(fixture.runtime.has_current_attempt());
    // A second mailbox item stays pending while the attempt runs.
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text("msg-pending", "kept"))
        .expect("enqueue");

    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let (before, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(before.jobs.len(), 1);
    assert!(matches!(before.jobs[0].state, BackgroundLifecycle::Running));
    assert_eq!(before.inbound.pending.len(), 1);
    assert_eq!(before.inbound.pending[0].message.id.as_str(), "msg-pending");
    attachment.detach();

    // After detach the background execution still runs and the mailbox
    // item still pends; the running attempt is untouched.
    let (after, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(after.jobs.len(), 1);
    assert!(
        matches!(after.jobs[0].state, BackgroundLifecycle::Running),
        "detach never cancels background work"
    );
    assert_eq!(
        after.inbound.pending.len(),
        1,
        "detach never drains mailbox contents"
    );
    assert!(
        after
            .attempt
            .as_ref()
            .is_some_and(|attempt| { matches!(attempt.phase, RuntimeClientAttemptPhase::Running) }),
        "detach never cancels the active attempt"
    );

    // Releasing the model lets the attempt settle: the safe boundary
    // drains the pending mailbox item into a second turn within the
    // same attempt.
    // Armed before the trigger: settlement wakes only registered waiters.
    let drained = fixture.runtime.settlement_signal().notified();
    tokio::pin!(drained);
    drained.as_mut().enable();
    release_tx.send(true).expect("release");
    await_request_history_len(&fixture.host, 2).await;
    // Request facts commit before their owner's semantic publication. The
    // native attempt completion, not durable visibility, proves the drain
    // observations have been published and may be folded by this snapshot.
    drained.await;
    let (settled, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(
        settled.inbound.pending.is_empty(),
        "the runtime drained the pending item at the safe boundary"
    );

    // The background execution settles normally after release.
    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    release.send_replace(true);
    await_background_terminal(
        fixture.runtime.tool_runtime().background(),
        &execution_id,
        "detach background execution",
    )
    .await;
    settled.await;
    let (final_snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(matches!(
        final_snapshot.jobs[0].state,
        BackgroundLifecycle::Succeeded
    ));
}

/// Detach never changes canonical conversation state, and a detached
/// conversation keeps admitting asynchronous inbound through the one
/// coordinator path (Test 2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_does_not_affect_conversation_or_future_async_admission() {
    let (adapter, fixture) =
        host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    // Armed before the trigger: settlement wakes only registered waiters.
    let first_settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(first_settled);
    first_settled.as_mut().enable();
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("first"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    first_settled.await;
    let (before, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    let ledger_before = fixture
        .runtime
        .coordinator_ledger()
        .expect("settled ledger");

    // Detach: nothing semantic changes.
    attachment.detach();
    let (after_detach, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(
        after_detach.messages, before.messages,
        "detach changes nothing"
    );
    assert_eq!(
        fixture
            .runtime
            .coordinator_ledger()
            .expect("settled ledger"),
        ledger_before,
        "detach never mutates canonical conversation state"
    );

    // A purely asynchronous enqueue with no attachment admits exactly
    // one further attempt through the runtime wake gate.
    // Armed before the trigger: settlement wakes only registered waiters.
    let async_settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(async_settled);
    async_settled.as_mut().enable();
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text(
            "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf",
            "async after detach",
        ))
        .expect("async enqueue");
    await_request_history_len(&fixture.host, 2).await;
    async_settled.await;
    assert_eq!(
        adapter.requests().len(),
        2,
        "the detached conversation admitted the next attempt"
    );
}

/// There is exactly one authoritative mutable conversation-state owner
/// at a time, and ownership transfers at the attempt boundaries.
///
/// Since Issue #54 the ownership is *structural*: admission moves the
/// one `ConversationState` out of the coordinator, so while an attempt
/// runs the coordinator holds nothing at all and physically cannot
/// mutate a competing copy.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_history_has_one_owner_at_a_time() {
    // Turn 1 calls a tool that parks. The loop therefore commits the
    // Assistant message (the model stream ended) and then blocks in tool
    // execution: exactly the "running, history already grown" window.
    let (tool, mut tool_started, release) = ParkingBackgroundTool::new();
    let definition = ToolDefinition {
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        ..tool.definition.clone()
    };
    let mut tools = ToolRegistry::new();
    tools
        .register(definition.clone(), Arc::new(tool))
        .expect("register the parking tool");
    let call_id = ToolCallId::new("call-park");
    let script = vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCallStart {
                id: call_id.clone(),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: call_id.clone(),
            arguments_delta: "{}".to_owned(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCall {
                id: call_id,
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
                arguments: serde_json::json!({}),
            },
        }),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ];
    let (adapter, fixture) = host_fixture(
        vec![script, one_turn_stop(), one_turn_stop()],
        tools,
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    // Idle: the runtime owns the conversation state, and it is the
    // projection's only source.
    assert_eq!(
        fixture.host.host_ledger(),
        Some(Vec::new()),
        "an idle runtime owns an empty conversation state"
    );
    assert!(
        fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot")
            .0
            .messages
            .is_empty()
    );

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("first"),
    });

    // Running: the loop committed the Assistant message (observable on the
    // stream) and is now parked inside tool execution, so the attempt
    // provably has not settled.
    receive_until(&subscription, |event| {
        event.event.facts().iter().any(|fact| {
            matches!(fact, RuntimeClientEvent::MessageCommitted { message, .. }
                if matches!(message, MessageBlock::Assistant(_)))
        })
    })
    .await;
    await_background_started(&mut tool_started, "the parking tool started").await;
    assert!(
        fixture.host.host_ledger().is_none(),
        "the attempt owns the conversation state; the runtime holds nothing"
    );
    assert!(
        fixture.host.host_active_ids().is_none(),
        "there is no runtime-side surface to compete with the attempt's"
    );
    let (mirror, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(
        mirror.messages.len(),
        3,
        "the projection mirrors the attempt's committed history"
    );
    assert!(fixture.host.has_current_attempt());

    // An inbound message arriving now stays mailbox-owned: the runtime
    // does not append it to a competing history.
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(2),
        content: submit_content("second"),
    });
    assert!(
        fixture.host.host_ledger().is_none(),
        "a busy-path submission never gives the runtime a competing conversation state"
    );

    // Releasing the tool lets the attempt finish its tool turn and then
    // drain the mailbox at its safe boundary. The drained message joins
    // the *execution's* history — the loop commits it — and the attempt
    // continues rather than settling.
    release.send_replace(true);
    let settlement = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let committed_during_attempt: Vec<&MessageBlock> = settlement
        .iter()
        .flat_map(|event| event.event.facts())
        .filter_map(|fact| match fact {
            RuntimeClientEvent::MessageCommitted { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert!(
        committed_during_attempt
            .iter()
            .any(|message| matches!(message, MessageBlock::Tool(_))),
        "the loop committed the tool message"
    );
    assert!(
        committed_during_attempt
            .iter()
            .any(|message| matches!(message, MessageBlock::User(_))),
        "the safe-boundary drain committed the queued inbound message into the attempt"
    );

    // A third submission while idle: its admission is the deterministic
    // proof that settlement transferred the execution's final
    // conversation state back to the runtime — admission only happens
    // once `finish_attempt` restored the state and cleared the attempt
    // slot.
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(3),
        content: submit_content("third"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;

    // The next attempt's first request began from exactly the previous
    // attempt's committed history.
    let requests = adapter.requests();
    assert_eq!(
        requests.len(),
        3,
        "two turns in the first attempt, one in the second"
    );
    let previous_committed = requests[1].messages.len();
    assert!(
        requests[2].messages.len() > previous_committed,
        "the next attempt started from the previous committed history \
         ({} vs {previous_committed} messages)",
        requests[2].messages.len()
    );

    // The externally visible history is one coherent sequence across
    // the tool turn, the safe-boundary drain, and both attempts.
    let (final_snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    let roles: Vec<&str> = final_snapshot
        .messages
        .iter()
        .map(|message| match message {
            MessageBlock::User(_) => "user",
            MessageBlock::Assistant(_) => "assistant",
            MessageBlock::Tool(_) => "tool",
        })
        .collect();
    assert_eq!(
        roles,
        vec![
            "user",
            "user",
            "assistant",
            "tool",
            "user",
            "assistant",
            "user",
            "assistant",
        ],
        "one authoritative history, with only eligible canonical Runtime \
         context facts, extended across the tool turn, the safe-boundary \
         drain, and both attempts"
    );
}

/// Blocks off the runtime until the worker-exit signal arrives.
async fn await_worker_exit(receiver: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || {
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the observation worker must terminate after the host is released");
    })
    .await
    .expect("worker exit task");
}

/// Writes one discoverable Skill package, making the next capability
/// candidate a real (non-no-op) commit.
fn write_probe_skill(workspace: &std::path::Path, name: &str) {
    let skill = workspace.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&skill).expect("skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: \"a probe skill\"\n---\nbody\n"),
    )
    .expect("SKILL.md");
}

/// Uses the public bounded checkpoint path and retains its exact cursor.
fn checkpoint_state(
    host: &RuntimeClientHost,
) -> (
    super::super::snapshot::RuntimeClientSnapshot,
    RuntimeClientCursor,
) {
    let checkpoint = host.checkpoint().expect("bounded authoritative checkpoint");
    (checkpoint.snapshot, checkpoint.cursor)
}

/// Reads the projection without draining the pending queue, so a test
/// can look at the state a consumer would see at an exact cut.
fn host_projection_snapshot(
    inner: &Arc<super::ClientInner>,
) -> (
    crate::runtime_client::snapshot::RuntimeClientSnapshot,
    RuntimeClientCursor,
) {
    inner
        .state
        .lock()
        .expect("runtime client host lock poisoned")
        .projection
        .snapshot()
        .expect("projection is live")
}

/// A surviving authoritative subsystem handle neither retains nor
/// resurrects the runtime: its observer no-ops, and its own transitions
/// still succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_surviving_subsystem_handle_never_retains_the_host() {
    let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let HostFixture {
        directory: dir,
        host,
        runtime,
        coordinator,
        store: _,
    } = fixture;

    // Clone subsystem handles out of the runtime, exactly as an embedder
    // legitimately may.
    let mailbox = runtime.tool_runtime().mailbox();
    let registry = runtime.tool_runtime().background().clone();
    let weak = host.weak_inner();
    let weak_runtime = host.weak_runtime_inner();
    let (exit_tx, exit_rx) = std::sync::mpsc::channel();
    host.install_worker_exit_probe(exit_tx);

    drop(host);
    drop(runtime);
    await_worker_exit(exit_rx).await;
    assert!(weak.upgrade().is_none(), "the host is gone");
    assert!(
        weak_runtime.upgrade().is_none(),
        "the conversation runtime is gone"
    );

    // Authoritative mailbox transition: the observer's upgrade fails and
    // the seam no-ops, but the mailbox is unaffected.
    let sequence = mailbox
        .enqueue(inbound_text("msg-after", "still authoritative"))
        .expect("the mailbox remains authoritative without a runtime");
    assert_eq!(sequence.get(), 1);
    let batch = mailbox
        .select_pending_batch()
        .expect("select")
        .expect("the select still works");
    assert_eq!(batch.items().len(), 1);

    // Authoritative background transition: same.
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-after"),
                },
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                mode: ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            },
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits")
    else {
        panic!("accepted dispatch");
    };
    await_background_started(&mut started, "background runner started").await;
    release.send_replace(true);
    await_background_terminal(
        &registry,
        &execution_id,
        "surviving registry background execution",
    )
    .await;

    // A claimed coordinator remains runtime-owned even after the runtime
    // and host are dropped; a surviving clone cannot bypass the resource
    // publication owner.
    write_probe_skill(&dir.path().join("workspace"), "after-skill");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    assert_eq!(
        coordinator.commit(candidate),
        Err(crate::capabilities::CapabilityCommitError::RuntimePublicationRequired),
        "a claimed coordinator cannot publish outside its runtime owner"
    );

    // None of those transitions resurrected the runtime or the host.
    assert_eq!(weak.strong_count(), 0);
    assert_eq!(weak_runtime.strong_count(), 0);
    assert!(
        weak.upgrade().is_none() && weak_runtime.upgrade().is_none(),
        "an observation seam can never resurrect a destroyed runtime"
    );
}

/// Attachment detach is not host destruction: the host survives, the
/// attachment slot is released, and a fresh endpoint initializes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_releases_the_attachment_but_never_the_host() {
    let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let weak = fixture.host.weak_inner();

    let endpoint = fixture.host.endpoint();
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: crate::runtime_client::RequestId::new(1),
        protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    assert!(response.error.is_none());

    // Dropping the endpoint detaches only that attachment.
    drop(endpoint);
    assert!(
        weak.upgrade().is_some(),
        "detach is not host destruction while a semantic owner remains"
    );

    // The host is still usable and the slot is free.
    fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("the host still serves reads");
    let reconnected = fixture.host.endpoint();
    let response = reconnected.handle_request(RuntimeClientRequest::Initialize {
        id: crate::runtime_client::RequestId::new(1),
        protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ),
        "reconnect remains possible while the host is owned"
    );

    // Only releasing the host itself ends its lifetime.
    drop(reconnected);
    drop(fixture);
    assert!(weak.upgrade().is_none(), "the host ends with its owner");
}

/// A pure checkpoint capture and the native cancellation callback have
/// one lock order: capture completes, then the finite callback installs.
/// Neither read-side preparation nor Registry re-entry is required.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_background_transition_and_checkpoint_capture_have_one_lock_order() {
    let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
    let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
    await_fixture_activation(&fixture.host).await;
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = fixture
        .runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-bg"),
                },
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                mode: ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            },
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
        .runtime
        .tool_runtime()
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits")
    else {
        panic!("accepted dispatch");
    };
    await_background_started(&mut started, "background runner started").await;

    // T1 takes the host lock and parks inside it.
    probe.arm_snapshot();
    let parked_host = fixture.host.clone();
    let snapshot_task = tokio::task::spawn_blocking(move || {
        parked_host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
    });
    probe.wait_snapshot_entered();

    // T2 enters actual cancellation concurrently with the parked
    // capture. Its callback may wait for the finite raw capture.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let registry = fixture.runtime.tool_runtime().background().clone();
    let cancel_id = execution_id.clone();
    let transition = tokio::task::spawn_blocking(move || {
        entered_tx.send(()).expect("the test still listens");
        registry.cancel(&cancel_id).expect("known execution")
    });

    entered_rx
        .recv_timeout(BACKGROUND_LIVENESS_GUARD)
        .expect("actual cancellation entered while capture was parked");
    probe.release_snapshot();
    snapshot_task
        .await
        .expect("snapshot task")
        .expect("snapshot");
    let cancelled = transition.await.expect("transition task");
    assert!(matches!(cancelled.state, BackgroundLifecycle::Cancelling));

    // The synchronous native callback was installed before cancellation
    // returned. The executor now releases and commits its terminal fact.
    release.send_replace(true);
    await_background_terminal(
        fixture.runtime.tool_runtime().background(),
        &execution_id,
        "snapshot-fold background execution",
    )
    .await;
    // The terminal notice has its own root attempt; settle that finite
    // transition before asking for the final authoritative cut.
    fixture.runtime.child_attempt_settlement(0).await;
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(snapshot.jobs.len(), 1);
    assert_eq!(snapshot.jobs[0].job_id, execution_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_snapshot_cut_excludes_commits_after_cursor_capture() {
    let (_, fixture) =
        host_fixture_with_native_tools(Vec::new(), ToolRegistry::new(), status_engine(), true)
            .await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let (baseline, cursor) = checkpoint_state(&fixture.host);
    let subscription = attachment.subscribe_events(cursor).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *inner.trace_cut_hook.lock().unwrap() = Some(Box::new(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    }));
    let host = fixture.host.clone();
    let task = tokio::task::spawn_blocking(move || {
        host.checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap()
    });
    entered_rx.recv().unwrap(); // Cursor and Journal prefix captured; host lock released.
    let committed = inner
        .store
        .append_event(crate::events::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: 1,
            event_id: crate::runtime::identity::EventId::new("after-trace-cut"),
            sequence: 0,
            conversation_id: inner.conversation_id.clone(),
            attempt_id: Some(crate::runtime::identity::AttemptId::new("after-cut")),
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: crate::events::RuntimeEvent::AttemptStarted {
                attempt_id: crate::runtime::identity::AttemptId::new("after-cut"),
            },
        })
        .unwrap();
    assert!(inner.pending.has_unpublished());
    assert_eq!(host_projection_snapshot(&inner).1, cursor);
    release_tx.send(()).unwrap();
    let (before, returned) = task.await.unwrap();
    assert_eq!(returned, cursor);
    assert_eq!(before.trace, baseline.trace);
    assert!(matches!(subscription.try_next(), EventDelivery::Pending));
    // Historical reads have their own frontier, but never fold the queue or move C.
    let _ = inner.trace_page(None, 32, Vec::new()).unwrap();
    assert_eq!(host_projection_snapshot(&inner).1, cursor);
    let sink = super::NativePublicationProjection {
        inner: Arc::downgrade(&inner),
    };
    assert!(
        crate::runtime::observation::PublicationProjectionCommit::native_batch(
            &sink,
            committed.observation.as_deref().unwrap(),
        )
        .is_installed()
    );
    inner
        .store
        .mark_observation_direct(committed.sequence, committed.sequence)
        .unwrap();
    let (continuous, after_cursor) = checkpoint_state(&fixture.host);
    assert!(after_cursor > cursor);
    let id = format!("trace:{}", committed.sequence);
    // Default checkpoints preserve their published bounded read cache.
    // The read-only native Trace page discovers the newly represented J.
    let RuntimeClientResult::TracePage { page } = inner.trace_page(None, 32, Vec::new()).unwrap()
    else {
        panic!("native Trace page");
    };
    assert_eq!(
        page.records.iter().filter(|entry| entry.id == id).count(),
        1
    );
    assert_eq!(
        host_projection_snapshot(&inner).1,
        after_cursor,
        "discovering historical records does not publish a new display cursor"
    );
    assert!(matches!(subscription.try_next(), EventDelivery::Event(_)));
    drop(attachment);
    let (_, reconnect) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let RuntimeClientResult::Initialized { .. } = reconnect else {
        panic!("snapshot")
    };
    let (reconnect, cursor) = checkpoint_state(&fixture.host);
    assert_eq!(reconnect.trace, continuous.trace);
    assert!(cursor >= after_cursor);
    assert_read_domain_replay(&inner, after_cursor, cursor);
    inner.unpark_projection_worker();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one two-cut native publication and terminal settlement contract
async fn background_durable_commit_cannot_publish_a_half_semantic_snapshot() {
    struct ReleaseOnDrop(Arc<crate::tools::background::test_sync::CommitBoundaryHook>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.proceed();
        }
    }
    let (_, fixture) =
        host_fixture_with_native_tools(Vec::new(), ToolRegistry::new(), status_engine(), true)
            .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .unwrap();
    let registry = fixture.runtime.tool_runtime().background().clone();
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("cut-background"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .unwrap();
    // Preparation independently publishes Busy. Include that semantic fact
    // in the baseline before parking the next Job publication; otherwise it
    // can advance the cursor while this test expects an unchanged cut.
    receive_until(&subscription, |event| {
        event.event.facts().iter().any(|fact| {
            matches!(
                fact,
                RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged {
                    eligibility: crate::runtime::adoption::AdoptionEligibility::Busy
                }
            )
        })
    })
    .await;
    let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
    let _release_commit = ReleaseOnDrop(hook.clone());
    registry.install_publication_hook(hook.clone());
    let (baseline, cursor) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .unwrap();
    let commit_registry = registry.clone();
    let commit = tokio::task::spawn_blocking(move || {
        commit_registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .unwrap()
    });
    // Condvar waits belong to the blocking pool, including during failure
    // teardown. The guards release parked producers if an assertion panics.
    tokio::task::spawn_blocking({
        let hook = hook.clone();
        move || hook.wait_entered()
    })
    .await
    .unwrap();
    // SQLite COMMIT complete; native registry installation deliberately paused.
    let (during, during_cursor) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .unwrap();
    assert_eq!(during_cursor, cursor);
    assert_eq!(during.jobs, baseline.jobs);
    assert_eq!(during.trace, baseline.trace);
    hook.proceed();
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = commit.await.unwrap() else {
        panic!("accepted")
    };
    await_background_started(&mut started, "published background").await;
    receive_until(&subscription, |event| event.event.facts().iter().any(|fact| {
        matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { trace, .. }
            if trace.records.iter().any(|record| record.native_id.as_deref() == Some(execution_id.as_str())))
    })).await;
    fixture.host.projection_settled().await;
    let after_checkpoint = fixture.host.checkpoint().unwrap();
    let after_cursor = after_checkpoint.cursor;
    let after_cut = after_checkpoint.read_cut;
    let after = after_checkpoint.snapshot;
    assert!(after_cursor > cursor);
    assert!(
        after
            .jobs
            .iter()
            .any(|record| record.job_id == execution_id)
    );
    assert!(
        after
            .trace
            .records
            .iter()
            .any(|record| record.native_id.as_deref() == Some(execution_id.as_str()))
    );
    let position = after
        .trace
        .records
        .iter()
        .find(|record| record.native_id.as_deref() == Some(execution_id.as_str()))
        .unwrap()
        .position
        .clone();
    // Isolate native terminal publication from the separate root-attempt
    // admission. Adopting the terminal notice removes Pending Inbox rows
    // and invalidates the historical cut this test deliberately holds.
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    fixture
        .runtime
        .install_admission_gate(admission_gate.clone());
    let _release_admission = admission_gate.arm_scoped();
    let terminal_hook =
        Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
    let _release_terminal = ReleaseOnDrop(terminal_hook.clone());
    registry.install_publication_hook(terminal_hook.clone());
    release.send(true).unwrap();
    tokio::task::spawn_blocking({
        let hook = terminal_hook.clone();
        move || hook.wait_entered()
    })
    .await
    .unwrap();
    // Terminal COMMIT complete; native lifecycle still running.
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let during_checkpoint = inner
        .checkpoint_with_trace(std::slice::from_ref(&position))
        .unwrap_or_else(|error| {
            panic!(
                "terminal publication checkpoint: {error:?}; prior={after_cut:?}; durable={:?}; published_cursor={:?}",
                fixture.store.as_ref().unwrap().observation_read_cut(),
                host_projection_snapshot(&inner).1,
            )
        });
    let terminal_cursor = during_checkpoint.cursor;
    assert_eq!(during_checkpoint.read_cut, after_cut);
    let during_terminal = during_checkpoint.snapshot;
    assert_eq!(terminal_cursor, after_cursor);
    assert_eq!(during_terminal.trace_updates.len(), 1);
    assert_eq!(
        during_terminal.trace_updates[0].state,
        crate::observation::trace::TraceState::Running
    );
    assert_eq!(during_terminal.trace_updates[0].timing.duration_ms, None);
    let RuntimeClientResult::TracePage { page: historical } =
        inner.trace_page(None, 32, Vec::new()).unwrap()
    else {
        panic!("Trace page")
    };
    assert_eq!(
        historical
            .records
            .iter()
            .find(|entry| entry.native_id.as_deref() == Some(execution_id.as_str()))
            .unwrap()
            .state,
        crate::observation::trace::TraceState::Running
    );
    assert_eq!(host_projection_snapshot(&inner).1, terminal_cursor);
    terminal_hook.proceed();
    await_background_terminal(&registry, &execution_id, "settled background").await;
    fixture.host.projection_settled().await;
    let settled_checkpoint = inner.checkpoint_with_trace(&[position]).unwrap();
    let settled_cursor = settled_checkpoint.cursor;
    let settled = settled_checkpoint.snapshot;
    assert!(settled_cursor > terminal_cursor);
    assert_eq!(
        settled.trace_updates[0].id,
        during_terminal.trace_updates[0].id
    );
    assert_eq!(
        settled.trace_updates[0].state,
        crate::observation::trace::TraceState::Completed
    );
}

fn assert_source_failure_preserves_retained_cut(
    host: &RuntimeClientHost,
    attachment: &crate::runtime_client::RuntimeAttachment,
    subscriber: &EventSubscription,
    retained: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
) -> crate::runtime_client::projection::ReadDomainCut {
    let inner = &host.inner;
    let (cut, cursor) = {
        let state = inner.lock_state();
        assert!(matches!(
            state.projection.snapshot_ref_checked(),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        assert!(
            state.projection.read_domain_cut().is_none(),
            "no automatic error spin"
        );
        (
            state.projection.read_domain_fence(),
            state.projection.cursor(),
        )
    };
    assert_eq!(cursor, retained.cursor);
    // Current checkpoint reads reject without clearing the failure;
    // the retained historical cut remains independently readable.
    assert!(matches!(
        host.checkpoint(),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert!(matches!(
        attachment.subscribe_events(cursor),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert!(matches!(
        subscriber.try_next(),
        EventDelivery::Unavailable { .. }
    ));
    let old_page = inner
        .checkpoint_page(&retained.read_cut, "capabilities.available_tools", 0, 1)
        .unwrap();
    assert_eq!(old_page.read_cut, retained.read_cut);
    assert_eq!(inner.lock_state().projection.cursor(), cursor);
    let failed = {
        let state = inner.state.lock().unwrap();
        state.projection.snapshot_ref_checked().is_err()
    };
    assert!(failed);
    cut
}

fn assert_source_repair_resumes_normal_events(
    inner: &super::ClientInner,
    recovered: &EventSubscription,
    cursor: RuntimeClientCursor,
) {
    super::publication::commit_display(&inner.state, &inner.publication_gate, |projection| {
        projection.apply(ConversationObservation::Event {
            attempt_id: AttemptId::new("failed-cut"),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("failed-cut"),
            },
        });
    });
    let EventDelivery::Event(started) = recovered.try_next() else {
        panic!("normal start delivery resumes after genuine source repair");
    };
    assert_eq!(started.cursor.get(), cursor.get() + 1);
    let cursor = started.cursor;
    super::publication::commit_display(&inner.state, &inner.publication_gate, |projection| {
        projection.apply(ConversationObservation::Event {
            attempt_id: AttemptId::new("failed-cut"),
            event: RuntimeEvent::TurnStarted,
        });
    });
    let EventDelivery::Event(event) = recovered.try_next() else {
        panic!("normal delivery resumes without another snapshot");
    };
    assert_eq!(event.cursor.get(), cursor.get() + 1);
    assert!(
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { turn: 1, .. }))
    );
}

#[tokio::test]
async fn panicked_projection_source_installs_same_cut_unavailability_without_spinning() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    inner.projection_settled().await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let (cursor, cut) = {
        let state = inner.lock_state();
        (
            state.projection.cursor(),
            state.projection.read_domain_fence(),
        )
    };
    let subscriber = attachment.subscribe_events(cursor).unwrap();
    *inner.read_domain_hook.lock().unwrap() = Some(Box::new(|| {
        panic!("private Source panic payload must not enter display diagnostics");
    }));
    inner
        .lock_state()
        .projection
        .request_read_domains_for_test();
    inner
        .pending
        .push(ConversationObservation::JournalSourceReady);
    let EventDelivery::Unavailable { message } = subscriber.next().await else {
        panic!("a Source panic must resolve subscribers as unavailable");
    };
    assert!(message.contains("native projection Source task panicked"));
    assert!(!message.contains("private Source panic payload"));
    let mut turns = inner.worker_turns.subscribe();
    turns.borrow_and_update();
    inner
        .pending
        .push(ConversationObservation::JournalSourceReady);
    turns.changed().await.unwrap();
    {
        let state = inner.lock_state();
        assert_eq!(state.projection.cursor(), cursor);
        assert_eq!(state.projection.read_domain_fence(), cut);
        assert!(state.projection.read_domain_cut().is_none());
        assert!(matches!(
            state.projection.validate_published_source(),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
    }
    assert!(inner.projection_read.lock().await.is_none());
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_failure_is_unavailable_until_same_cut_native_read_repair() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let cursor = inner.lock_state().projection.cursor();
    let subscriber = attachment.subscribe_events(cursor).unwrap();
    let retained = fixture.host.checkpoint().unwrap();
    let mut source_fault = source_recovery_tests::OccupancySourceFault::new(&fixture);
    *inner.read_domain_hook.lock().unwrap() = Some(Box::new(|| {
        Err(crate::durable::ConversationStoreError::InvalidReference(
            "one read fault".into(),
        ))
    }));
    let journal = inner.store.presentation_frontier().unwrap();
    inner
        .lock_state()
        .projection
        .request_read_domains_for_test();
    inner
        .pending
        .push(ConversationObservation::JournalSourceReady);
    let EventDelivery::Unavailable { message } = subscriber.next().await else {
        panic!("Source failure reports unavailability without an ordinary event");
    };
    assert!(message.contains("one read fault"));
    let cut = assert_source_failure_preserves_retained_cut(
        &fixture.host,
        &attachment,
        &subscriber,
        &retained,
    );
    assert_eq!(inner.store.presentation_frontier().unwrap(), journal);
    // The source worker's existing owner reads outside the raw lock and
    // installs one successful result at the exact failed fence.
    source_fault.repair();
    let read = cut.read(inner.store.as_ref());
    super::publication::commit_display(&inner.state, &inner.publication_gate, |projection| {
        assert!(projection.install_read_domains(&cut, read, true));
    });
    let repaired = fixture.host.checkpoint().unwrap();
    let recovered_cursor = repaired.cursor;
    assert_eq!(recovered_cursor.get(), cursor.get() + 1);
    assert_eq!(repaired.read_cut.source_cut, retained.read_cut.source_cut);
    assert_eq!(inner.store.presentation_frontier().unwrap(), journal);
    let (same_fence, valid, no_spin) = {
        let state = inner.state.lock().unwrap();
        (
            state.projection.read_domain_fence() == cut,
            state.projection.snapshot_ref_checked().is_ok(),
            state.projection.read_domain_cut().is_none(),
        )
    };
    assert!(same_fence && valid && no_spin);
    assert_eq!(fixture.host.checkpoint().unwrap(), repaired);
    let EventDelivery::Event(repair) = subscriber.try_next() else {
        panic!("genuine Source repair publishes one complete transition");
    };
    assert_eq!(repair.cursor, recovered_cursor);
    assert!(
        repair
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
    );
    let recovered = attachment.subscribe_events(recovered_cursor).unwrap();
    assert!(matches!(subscriber.try_next(), EventDelivery::Closed));
    assert!(matches!(recovered.try_next(), EventDelivery::Pending));
    assert_source_repair_resumes_normal_events(&inner, &recovered, recovered_cursor);
}

#[tokio::test]
async fn historical_candidate_cannot_repair_newer_dirty_or_failed_cut() {
    for fail_newer in [false, true] {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let before = inner.snapshot_candidate().unwrap();
        let state = inner.state.clone();
        let publication_gate = inner.publication_gate.clone();
        *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
            super::publication::commit_display(&state, &publication_gate, |projection| {
                projection.apply(ConversationObservation::Event {
                    attempt_id: AttemptId::new("newer-cut"),
                    event: RuntimeEvent::AttemptStarted {
                        attempt_id: AttemptId::new("newer-cut"),
                    },
                });
            });
            if fail_newer {
                super::publication::commit_display(&state, &publication_gate, |projection| {
                    let cut = projection.read_domain_cut().unwrap();
                    assert!(projection.install_read_domains(
                        &cut,
                        Err(crate::durable::ConversationStoreError::InvalidReference(
                            "newer failure".into()
                        )),
                        true
                    ));
                });
            }
        }));
        let candidate = inner.snapshot_candidate().unwrap();
        assert_eq!(candidate.cursor, before.cursor);
        assert_eq!(candidate.snapshot, before.snapshot);
        {
            let state = inner.lock_state();
            assert_eq!(
                state.projection.cursor().get(),
                before.cursor.get() + 1,
                "no false derived publication"
            );
            assert_ne!(state.projection.read_domain_fence(), candidate.read_cut);
            assert_eq!(state.projection.read_domain_cut().is_some(), !fail_newer);
            assert_eq!(state.projection.snapshot_ref_checked().is_err(), fail_newer);
            assert!(state.projection.snapshot_ref().attempt.is_some());
        }
        *inner.snapshot_read_hook.lock().unwrap() = None;
        let current = inner.snapshot_candidate().unwrap();
        let state = inner.lock_state();
        assert!(state.projection.read_domain_cut().is_none());
        assert_eq!(
            state.projection.snapshot_ref_checked().unwrap().transcript,
            current.snapshot.transcript
        );
        assert!(current.snapshot.attempt.is_some());
    }
}

#[tokio::test]
async fn snapshot_candidate_survives_moving_head_and_replays_exact_suffix() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    super::publication::commit_display(&inner.state, &inner.publication_gate, |projection| {
        projection.apply(ConversationObservation::Event {
            attempt_id: AttemptId::new("candidate"),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("candidate"),
            },
        });
    });
    let cursor = inner.lock_state().projection.cursor();
    let state = inner.state.clone();
    let publication_gate = inner.publication_gate.clone();
    *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
        for _ in 0..8 {
            super::publication::commit_display(&state, &publication_gate, |projection| {
                projection.apply(ConversationObservation::Event {
                    attempt_id: AttemptId::new("candidate"),
                    event: RuntimeEvent::TurnStarted,
                });
            });
        }
    }));
    let candidate = inner.snapshot_candidate().unwrap();
    assert_eq!(candidate.cursor, cursor);
    assert_eq!(candidate.snapshot.attempt.as_ref().unwrap().turn, 0);
    let mut state = inner.lock_state();
    let (subscriber, _) = state.projection.subscribe(candidate.cursor).unwrap();
    for turn in 1..=8 {
        let super::super::projection::SubscriberPoll::Event(event) =
            state.projection.poll_subscriber(subscriber)
        else {
            panic!("exact replay suffix");
        };
        assert_eq!(event.cursor.get(), cursor.get() + u64::from(turn));
        assert!(
            event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { turn: actual, .. } if *actual == turn))
        );
    }
    assert_eq!(
        state
            .projection
            .snapshot_ref()
            .attempt
            .as_ref()
            .unwrap()
            .turn,
        8
    );
}

fn fold_captured_snapshot_fact(
    snapshot: &mut super::super::snapshot::RuntimeClientSnapshot,
    event: &RuntimeClientEvent,
) {
    match event {
        RuntimeClientEvent::MessageDescriptorCommitted {
            head,
            evicted_message_id,
            ..
        } => {
            snapshot.message_descriptors.clone_from(head);
            if let Some(id) = evicted_message_id {
                snapshot.messages.retain(|message| message.id() != id);
            }
        }
        RuntimeClientEvent::MessageCommitted {
            message,
            evicted_message_id,
            ..
        } => {
            if let Some(id) = evicted_message_id {
                snapshot.messages.retain(|message| message.id() != id);
            }
            if let Some(current) = snapshot
                .messages
                .iter_mut()
                .find(|current| current.id() == message.id())
            {
                current.clone_from(message);
            } else {
                snapshot.messages.push(message.clone());
            }
        }
        RuntimeClientEvent::ReadDomainsUpdated {
            projection_head,
            transcript,
            trace,
            occupancy,
            todos,
        } => {
            snapshot.context.projection_head.clone_from(projection_head);
            snapshot.transcript.clone_from(transcript);
            snapshot.trace.clone_from(trace);
            snapshot.context.occupancy.clone_from(occupancy);
            snapshot.todos.clone_from(todos);
        }
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
            snapshot.clone_from(&checkpoint.snapshot);
        }
        _ => panic!("unexpected captured-cut fact"),
    }
}

#[tokio::test]
async fn captured_snapshot_excludes_later_durable_messages_and_replay_converges() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let baseline = inner.snapshot_candidate().unwrap();
    let old_cut = inner.lock_state().projection.read_domain_fence();
    let store = inner.store.clone();
    let state = inner.state.clone();
    let publication = Arc::clone(&inner.publication_gate);
    *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
        for id in ["candidate-later-one", "candidate-later-two"] {
            let message = MessageBlock::User(inbound_text(id, id));
            let receipt = store.append_canonical(&message).unwrap();
            let position = receipt.ledger_position;
            super::publication::commit_display(&state, &publication, |projection| {
                projection.apply(ConversationObservation::CanonicalCommitted {
                    attempt_id: None,
                    receipt,
                });
            });
            store
                .mark_canonical_transcript_ready(position, position)
                .unwrap();
        }
    }));
    let candidate = inner.snapshot_candidate().unwrap();
    assert_eq!(candidate.cursor, baseline.cursor);
    assert_eq!(
        candidate.snapshot, baseline.snapshot,
        "later durable bodies cannot leak backward"
    );
    inner.snapshot_read_hook.lock().unwrap().take();
    let mut replayed = candidate.snapshot.clone();
    let mut state = inner.lock_state();
    assert!(
        !state.projection.install_read_domains(
            &old_cut,
            Ok(super::super::projection::ReadDomainValues {
                projection_head: candidate.snapshot.context.projection_head.clone(),
                transcript: candidate.snapshot.transcript.clone(),
                occupancy: candidate.snapshot.context.occupancy.clone(),
                trace: candidate.snapshot.trace.clone(),
                trace_total: 0,
            }),
            true
        ),
        "returnable candidate is stale for live installation"
    );
    drop(state);
    let current = inner.snapshot_candidate().unwrap();
    let mut state = inner.lock_published_state();
    let (subscriber, _) = state.projection.subscribe(candidate.cursor).unwrap();
    let mut through = candidate.cursor;
    while through < state.projection.cursor() {
        let super::super::projection::SubscriberPoll::Event(event) =
            state.projection.poll_subscriber(subscriber)
        else {
            panic!("contiguous replay");
        };
        assert_eq!(event.cursor.get(), through.get() + 1);
        through = event.cursor;
        for fact in event.event.facts() {
            fold_captured_snapshot_fact(&mut replayed, fact);
        }
    }
    assert!(matches!(
        state.projection.poll_subscriber(subscriber),
        super::super::projection::SubscriberPoll::Pending
    ));
    drop(state);
    assert_eq!(
        replayed, current.snapshot,
        "replay equals independent authoritative completion"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn captured_pending_body_survives_later_edit_removal_and_commit() {
    let (release, gate) = model_release();
    let (adapter, fixture) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(gate),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let mailbox = fixture.runtime.tool_runtime().mailbox();
    mailbox
        .enqueue(inbound_text("candidate-active", "running"))
        .unwrap();
    await_adapter_request_count(&adapter, 1).await;
    let sequence = mailbox
        .enqueue(inbound_text("candidate-pending", "captured body"))
        .unwrap();
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let store = inner.store.clone();
    let candidate = inner.snapshot_candidate().unwrap();
    let frozen = candidate.snapshot.clone();
    {
        let expected = crate::durable::inbox::PendingInboundRef {
            sequence,
            revision: 0,
        };
        store.edit_pending(&expected, "later edit").unwrap();
        store
            .remove_pending(&crate::durable::inbox::PendingInboundRef {
                revision: 1,
                ..expected
            })
            .unwrap();
        store
            .append_canonical(&MessageBlock::User(inbound_text("future-commit", "future")))
            .unwrap();
    }
    assert!(matches!(
        candidate.read_domains(store.as_ref()),
        Err(crate::durable::ConversationStoreError::ObservationCutExpired)
    ));
    assert_eq!(candidate.snapshot, frozen);
    assert_eq!(candidate.snapshot.inbound.pending[0].revision, 0);
    assert_eq!(
        candidate.snapshot.inbound.pending[0].message.content,
        inbound_text("x", "captured body").content
    );
    let transcript = format!("{:?}", candidate.snapshot.transcript);
    assert!(transcript.contains("captured body"));
    assert!(!transcript.contains("later edit"));
    assert!(!transcript.contains("future-commit"));
    release.send(true).unwrap();
    fixture.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn captured_snapshot_survives_replay_eviction_but_subscribe_requires_resync() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let state = inner.state.clone();
    *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
        let mut state = state.lock().unwrap();
        for _ in 0..=super::super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT {
            state.projection.apply(ConversationObservation::Shutdown);
        }
    }));
    let candidate = inner.snapshot_candidate().unwrap();
    assert!(!candidate.snapshot.shutting_down);
    assert!(matches!(
        inner.lock_state().projection.subscribe(candidate.cursor),
        Err(RuntimeClientError::ResyncRequired { .. })
    ));
}

#[tokio::test]
async fn blocked_read_materialization_does_not_own_shutdown_or_projection_drain() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let reader = inner.read_store.clone();
    *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
        reader.park_presentation_read_until_cancelled(entered);
        Ok(())
    }));
    inner.pending.push(ConversationObservation::Event {
        attempt_id: AttemptId::new("read-cut"),
        event: RuntimeEvent::TurnStarted,
    });
    entered_rx.await.unwrap(); // The derived read is blocked, with one Tokio worker.
    inner.pending.push(ConversationObservation::Shutdown);
    assert!(inner.lock_state().projection.snapshot_ref().shutting_down);
    assert!(matches!(
        inner.shutdown().await.unwrap(),
        RuntimeClientResult::ShutdownCompleted
    ));
    inner.drain_projection().await.unwrap(); // Joins the cancelled read; no test release.
    assert!(inner.pending.is_closed());
}

#[tokio::test]
async fn projection_drain_retry_retains_the_original_worker_after_waiter_cancellation() {
    use std::future::Future as _;
    struct ReleaseOnDrop(std::sync::mpsc::Sender<()>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (cancelled, cancelled_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let release_on_unwind = ReleaseOnDrop(release.clone());
    let reader = inner.read_store.clone();
    *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
        reader.park_presentation_read_until_cancelled(entered);
        cancelled.send(()).unwrap();
        // This is the actual blocking Source task, after cancellation;
        // it cannot release its original ownership until the gate opens.
        release_rx.recv().unwrap();
        Ok(())
    }));
    inner.pending.push(ConversationObservation::Event {
        attempt_id: AttemptId::new("cancelled-join-waiter"),
        event: RuntimeEvent::TurnStarted,
    });
    entered_rx.await.unwrap();
    fixture.runtime.shutdown().await.unwrap();
    let mut first = Box::pin(inner.drain_projection());
    tokio::select! {
        biased;
        result = &mut first => panic!("the original gated Source must still be owned: {result:?}"),
        result = cancelled_rx => result.unwrap(),
    }
    drop(first);
    assert!(matches!(
        *inner.worker.try_lock().unwrap(),
        ProjectionWorker::Running(_)
    ));
    let mut retry = Box::pin(inner.drain_projection());
    std::future::poll_fn(|context| {
        assert!(
            retry.as_mut().poll(context).is_pending(),
            "a retry cannot claim a detached original worker has joined"
        );
        std::task::Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    retry.await.unwrap();
    assert!(matches!(
        *inner.worker.try_lock().unwrap(),
        ProjectionWorker::Vacant
    ));
    assert!(inner.pending.is_closed());
    drop(release_on_unwind);
}

async fn failed_projection_worker_keeps_its_physical_read(abort: bool) {
    use std::future::Future as _;
    struct ReleaseOnDrop(std::sync::mpsc::Sender<()>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let capacity = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = capacity.clone().acquire_owned().await.unwrap();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let release_on_unwind = ReleaseOnDrop(release.clone());
    *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
        let _permit = permit;
        entered.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(())
    }));
    inner.pending.push(ConversationObservation::Event {
        attempt_id: AttemptId::new("failed-worker-read"),
        event: RuntimeEvent::TurnStarted,
    });
    entered_rx.await.unwrap();
    if abort {
        let worker = inner.worker.lock().await;
        let ProjectionWorker::Running(task) = &*worker else {
            panic!("worker must be running")
        };
        task.abort();
    } else {
        // This is the real production panic boundary while a Source task
        // remains outstanding, rather than a replacement worker fixture.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = inner.observation_preparation.lock().unwrap();
            panic!("poison preparation before the worker's next turn");
        }));
        inner
            .pending
            .push(ConversationObservation::JournalSourceReady);
    }
    loop {
        let finished = match &*inner.worker.lock().await {
            ProjectionWorker::Running(task) => task.is_finished(),
            _ => panic!("only drain may consume the worker proof"),
        };
        if finished {
            break;
        }
        tokio::task::yield_now().await;
    }
    inner.observation_preparation.clear_poison();
    fixture.runtime.shutdown().await.unwrap();
    let mut first = Box::pin(inner.drain_projection());
    std::future::poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(capacity.available_permits(), 0);
    drop(first);
    assert!(matches!(
        *inner.worker.try_lock().unwrap(),
        ProjectionWorker::Joined(Err(_))
    ));
    assert!(inner.projection_read.try_lock().unwrap().is_some());
    let mut retry = Box::pin(inner.drain_projection());
    std::future::poll_fn(|cx| {
        assert!(
            retry.as_mut().poll(cx).is_pending(),
            "failed worker cannot prove its detached read ended"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(capacity.available_permits(), 0);
    release.send(()).unwrap();
    let error = retry.await.unwrap_err();
    assert_eq!(error.is_cancelled(), abort);
    assert_eq!(error.is_panic(), !abort);
    assert_eq!(capacity.available_permits(), 1);
    assert!(inner.projection_read.try_lock().unwrap().is_none());
    inner.drain_projection().await.unwrap(); // Both physical tasks are now joined.
    drop(release_on_unwind);
}

#[tokio::test]
async fn projection_drain_joins_residual_read_after_worker_panic_and_waiter_cancellation() {
    failed_projection_worker_keeps_its_physical_read(false).await;
}

#[tokio::test]
async fn projection_drain_joins_residual_read_after_worker_abort_and_waiter_cancellation() {
    failed_projection_worker_keeps_its_physical_read(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_projection_fold_cannot_retain_the_host_or_storage_authority() {
    let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
    let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
    let weak = fixture.host.weak_inner();
    let pending = weak.upgrade().unwrap().pending.clone();
    let (exited, exit) = std::sync::mpsc::channel();
    fixture.host.install_worker_exit_probe(exited);
    probe.arm_publish();
    pending.push(ConversationObservation::GoalChanged(
        crate::observation::goal::RuntimeClientGoalView { current: None },
    ));
    probe.wait_publish_entered(); // Worker holds the projection mutex inside a fold.
    drop(fixture.host);
    let host_released = weak.upgrade().is_none();
    probe.release_publish();
    await_worker_exit(exit).await;
    assert!(
        host_released,
        "a read-model fold must not own the host's resource authority"
    );
}

/// The exact snapshot/cursor race, interleaving A (snapshot wins): the
/// snapshot linearizes first and the concurrent transition is observed
/// by a resume after the snapshot's cursor.
///
/// The runtime admission gate makes the interleaving exact: the parked
/// snapshot captures the immutable published seed before the submit, and the
/// admission commit is released only after the snapshot returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_cursor_race_snapshot_wins() {
    let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let submit_arrival = Arc::new(tokio::sync::Notify::new());
    let (_, fixture) = host_fixture_probe_with_runtime_gate(
        probe.clone(),
        vec![one_turn_stop()],
        CoordinatorProbe {
            admission_gate: Some(admission_gate.clone()),
            settlement_gate: None,
            before_restore_gate: None,
            activation_gate: None,
            manual_compaction_settlement_gate: None,
            submit_gate: None,
            submit_arrival: Some(submit_arrival.clone()),
            shutdown_arrival: None,
            mcp_failure_drain_gate: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            drain_linearization: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
            ..CoordinatorProbe::default()
        },
    )
    .await;
    await_fixture_activation(&fixture.host).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    admission_gate.arm();
    probe.arm_snapshot();
    let snapshot_probe = probe.clone();
    let host = fixture.host.clone();
    let snapshot_task = tokio::task::spawn_blocking(move || {
        host.checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
    });
    snapshot_probe.wait_snapshot_entered();

    // The concurrent transition: a submit whose admission is gated
    // until after the snapshot returns.
    let submitting = fixture.host.clone();
    let submit_task = tokio::task::spawn_blocking(move || {
        submitting
            .submit_inbound(submit_content("racing"))
            .expect("accepted")
    });
    // The native submit entered while capture owns raw. Its finite
    // pending callback can finish only after that capture releases raw.
    submit_arrival.notified().await;
    snapshot_probe.release_snapshot();
    let (snapshot, cursor) = snapshot_task
        .await
        .expect("snapshot task")
        .expect("snapshot");
    assert!(snapshot.inbound.pending.is_empty());
    submit_task.await.expect("submit task");
    admission_gate.wait_entered();

    // Release the admission: the transition commits and is observed
    // after C. Resume and receive the admission events, never a gap.
    admission_gate.release();
    let subscription = attachment
        .subscribe_events(cursor)
        .expect("resume after the snapshot cursor");
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::InboundEnqueued { .. }))
    })
    .await;
    assert!(
        events.iter().all(|event| event.cursor > cursor),
        "every resumed event is strictly after the snapshot cursor"
    );
    assert!(events.iter().any(|event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::InboundEnqueued { .. }))
    }));
    // Drain the attempt so the fixture settles cleanly.
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
}

/// The exact snapshot/cursor race, interleaving B (publish wins): the
/// concurrent transition linearizes before the snapshot, so the
/// snapshot at its cursor already reflects it.
///
/// The runtime admission gate and the projection publish gate make the
/// interleaving exact: the admission commits while the publish of its
/// observations is parked, and the snapshot acquires the projection
/// lock only after the fold completed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_cursor_race_publish_wins() {
    let (pause, control, _) =
        crate::agent::execution::test_sync::StartBoundaryPause::install(true, false);
    let mut control = control.unwrap();
    let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (_, fixture) = host_fixture_probe_with_runtime_gate(
        probe.clone(),
        vec![one_turn_stop()],
        CoordinatorProbe {
            admission_gate: Some(admission_gate.clone()),
            start_boundary_pause: Some(pause),
            ..CoordinatorProbe::default()
        },
    )
    .await;
    await_fixture_activation(&fixture.host).await;
    // Baseline: an idle host at some cursor C.
    let (before, cursor) = checkpoint_state(&fixture.host);
    assert!(before.inbound.pending.is_empty());

    // Submit; the admission parks at the runtime gate before committing.
    admission_gate.arm();
    let submitting = fixture.host.clone();
    let submit_task = tokio::task::spawn_blocking(move || {
        submitting
            .submit_inbound(submit_content("racing"))
            .expect("accepted")
    });
    let _accepted = submit_task.await.expect("submit task");
    admission_gate.wait_entered();

    // Submit's exact pending receipt has already published. The gate
    // below therefore parks the later admission transition.
    let (folded, _) = checkpoint_state(&fixture.host);
    assert_eq!(folded.inbound.pending.len(), 1);

    // Release the admission while the projection publish gate is armed:
    // the fold of the commit observations parks at the gate.
    probe.arm_publish();
    admission_gate.release();
    probe.wait_publish_entered();
    // Admission publication completes before the Agent can reach its
    // request-start gate. Release the earlier owner before awaiting that
    // next durable boundary, then capture the completed admission cut.
    probe.release_publish();
    control.await_park(1).await;
    let snapshot_host = fixture.host.clone();
    let snapshot_task = tokio::task::spawn_blocking(move || {
        snapshot_host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
    });
    // The pure snapshot reads the admission cut already installed above.
    let (after_snapshot, after_cursor) = snapshot_task
        .await
        .expect("snapshot task")
        .expect("snapshot");
    assert!(after_cursor > cursor, "the transition advanced the cursor");
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(cursor)
        .expect("resume from the pre-transition cursor");
    control.release();
    let mut saw_inbound = false;
    loop {
        let delivery = tokio::time::timeout(STREAM_LIVENESS_GUARD, subscription.next())
            .await
            .expect("stream must not stall");
        let EventDelivery::Event(event) = delivery else {
            panic!("subscription stays open and contiguous, got {delivery:?}");
        };
        if event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::InboundEnqueued { .. }))
        {
            saw_inbound = true;
        }
        if event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        {
            break;
        }
    }
    assert!(saw_inbound, "the transition event is on the stream");
    assert!(
        after_snapshot.messages.iter().any(|message| matches!(
            message,
            MessageBlock::User(user) if user.id.as_str() == "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-1"
        )),
        "the snapshot reflects the transition state"
    );
}

#[tokio::test]
async fn resubscription_is_superseded_before_local_handle_publication() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let (_, cursor) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .unwrap();
    let old = attachment.subscribe_events(cursor).unwrap();
    // Split the native transaction from local handle publication, exactly
    // the interval in which a waiting notification consumer can wake. No
    // timer or scheduler race is needed to hold this boundary open.
    let (new, _) = fixture
        .host
        .subscribe_events(attachment.attachment_id(), cursor)
        .unwrap();
    assert!(matches!(old.try_next(), EventDelivery::Closed));
    assert!(attachment.subscription().unwrap().same_registration(&old));
    assert!(
        attachment.superseded(&old),
        "resync must not retire the live attachment"
    );
    assert!(!attachment.superseded(&new));
    attachment.store_subscription(new.clone());
    assert!(attachment.subscription().unwrap().same_registration(&new));
    assert!(attachment.superseded(&old));
    attachment.detach();
    assert!(
        !attachment.superseded(&new),
        "real detach is not replacement"
    );
    assert!(matches!(new.try_next(), EventDelivery::Closed));
}

/// The bounded replay/resync contract: a serviceable resume has no
/// gap, an expired cursor returns `resync_required`, and a fresh
/// snapshot repairs all state. The cursor survives detach (it belongs
/// to the observation stream, not the attachment).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_resync_and_cursor_survival() {
    let (_, fixture) =
        host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let terminal_cursor = events.last().expect("terminal event").cursor;

    // Detach: the cursor is stream-owned and survives.
    attachment.detach();
    let (second, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("reattach");
    let second_subscription = second
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("resume from the retained stream");
    let replayed = receive_until(&second_subscription, |event| {
        event.cursor == terminal_cursor
    })
    .await;
    assert_eq!(
        replayed.last().expect("terminal replayed").cursor,
        terminal_cursor,
        "the full retained stream is replayable after reconnect"
    );
    assert!(
        replayed
            .windows(2)
            .all(|pair| pair[0].cursor < pair[1].cursor)
    );

    // A cursor ahead of the stream is unserviceable.
    let error = second
        .subscribe_events(RuntimeClientCursor::new(terminal_cursor.get() + 100))
        .expect_err("ahead of the stream");
    assert!(matches!(error, RuntimeClientError::ResyncRequired { .. }));
}

fn dispatch_background_fixture(
    fixture: &HostFixture,
    executor: &Arc<dyn ToolExecutor>,
    call_id: &str,
) -> crate::runtime::identity::ToolExecutionId {
    let prepared = fixture
        .runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new(call_id),
                },
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                mode: ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            },
            executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
        .runtime
        .tool_runtime()
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits")
    else {
        panic!("accepted");
    };
    execution_id
}

/// The background lifecycle projection: Starting/Running/Cancelling/
/// terminal transitions project from the authoritative registry, the
/// protocol cancel is acceptance-not-settlement, and terminal records
/// stay visible after detach.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_lifecycle_projection_and_protocol_cancel() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let execution_id = dispatch_background_fixture(&fixture, &executor, "call-bg");
    await_background_started(&mut started, "runner started").await;

    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let (snapshot, cursor) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(matches!(
        snapshot.jobs[0].state,
        BackgroundLifecycle::Running
    ));

    // Intent commits synchronously; the independent wait still owns physical settlement.
    let receipt = attachment
        .job_cancel(&execution_id)
        .expect("accepted intent");
    assert!(
        matches!(receipt, RuntimeClientResult::JobCancellationAccepted { job_id } if job_id == execution_id)
    );
    let wait = attachment.job_wait(&execution_id);
    tokio::pin!(wait);
    assert!(futures_util::poll!(&mut wait).is_pending());
    assert_eq!(
        fixture
            .runtime
            .background_status(&execution_id)
            .expect("job")
            .state,
        BackgroundLifecycle::Cancelling
    );
    let unknown = fixture
        .host
        .inner
        .job_status(&crate::runtime::identity::ToolExecutionId::new(
            "exec_9f85dfef-c2b2-7a62-837d-620fed38822f",
        ));
    assert!(matches!(
        unknown,
        Err(RuntimeClientError::UnknownBackgroundExecution { .. })
    ));

    // Settlement: the executor raced past the cancellation request and
    // proved success. Cancellation intent owns the request fact and its
    // reason, never the physical outcome (Issue #202), so the execution
    // settles as `Succeeded`, not `Cancelled`.
    let subscription = attachment
        .subscribe_events(cursor)
        .expect("observe terminal notice attempt");
    release.send_replace(true);
    let RuntimeClientResult::JobOutcome { job_id, state } =
        wait.await.expect("physical settlement after intent")
    else {
        panic!("Job result")
    };
    assert_eq!(job_id, execution_id);
    assert_eq!(state, BackgroundLifecycle::Succeeded);

    let terminal = await_background_terminal(
        fixture.runtime.tool_runtime().background(),
        &execution_id,
        "background execution settlement after a racing cancel",
    )
    .await;
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    // The background notice also starts a root attempt. Its settlement is
    // the exact authoritative boundary for the final snapshot.
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(snapshot.jobs[0].state, BackgroundLifecycle::Succeeded);
    assert!(snapshot.jobs[0].result.is_some());
}

#[tokio::test]
async fn job_intent_and_waits_preserve_abandoned_publication_for_the_captured_job() {
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let registry = fixture.runtime.background_registry();
    let invocation = ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("job-failure-call"),
        },
        tool_id: ToolId::new("tool-bg"),
        tool_name: "bg".into(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .unwrap();
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .unwrap()
    else {
        panic!("accepted")
    };
    started.wait_for(|started| *started).await.unwrap();
    let wait = fixture.host.inner.job_wait(&execution_id);
    tokio::pin!(wait);
    assert!(futures_util::poll!(&mut wait).is_pending());
    let receipt = fixture
        .host
        .inner
        .job_cancel_intent(&execution_id)
        .expect("accepted intent");
    assert!(
        matches!(receipt, RuntimeClientResult::JobCancellationAccepted { job_id } if job_id == execution_id)
    );
    let after_intent = fixture.host.inner.job_wait(&execution_id);
    tokio::pin!(after_intent);
    assert!(futures_util::poll!(&mut after_intent).is_pending());
    fixture.store.as_ref().unwrap().arm_fail_accept_times(2);
    release.send_replace(true);
    let results = tokio::time::timeout(BACKGROUND_LIVENESS_GUARD, async {
        [wait.await, after_intent.await]
    })
    .await
    .expect("abandoned Job observers must terminate");
    for result in results {
        assert!(
            matches!(result, Err(RuntimeClientError::JobPublicationAbandoned { job_id }) if job_id == execution_id)
        );
    }
    assert_eq!(
        registry.snapshot(&execution_id).unwrap().state,
        BackgroundLifecycle::PublishingTerminal
    );
    assert!(
        matches!(fixture.host.inner.job_wait(&execution_id).await, Err(RuntimeClientError::JobPublicationAbandoned { job_id }) if job_id == execution_id)
    );
}

#[tokio::test]
async fn job_list_preserves_domain_bound_order_and_omission_metadata() {
    use crate::tools::background::MAX_JOB_LIST_LIMIT;
    let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(super::super::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .unwrap();
    let registry = fixture.runtime.background_registry();
    let mut ids = Vec::new();
    for index in 0..MAX_JOB_LIST_LIMIT + 3 {
        let (tool, _, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let invocation = ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new(format!("job-list-{index}")),
            },
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".into(),
            mode: ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = registry
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .unwrap()
        else {
            panic!("accepted")
        };
        release.send_replace(true);
        registry.wait_until_terminal(&execution_id).await.unwrap();
        ids.push(execution_id);
    }
    let listing = registry.listing(false, MAX_JOB_LIST_LIMIT);
    let (returned, matched, limit, truncated) = (
        listing.returned,
        listing.matched,
        listing.limit,
        listing.truncated,
    );
    let jobs: Vec<_> = listing
        .snapshots
        .iter()
        .map(super::background_view)
        .collect();
    assert_eq!(
        (returned, matched, limit, truncated),
        (MAX_JOB_LIST_LIMIT, ids.len(), MAX_JOB_LIST_LIMIT, true)
    );
    assert_eq!(
        jobs.iter().map(|job| &job.job_id).collect::<Vec<_>>(),
        ids.iter()
            .rev()
            .take(MAX_JOB_LIST_LIMIT)
            .collect::<Vec<_>>()
    );
    receive_until(&subscription, |event| event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::JobUpdated { job, .. } if Some(&job.job_id) == ids.last() && job.state == crate::tools::background::BackgroundLifecycle::Succeeded))).await;
    let RuntimeClientResult::DomainPage { page } = attachment.job_list().unwrap() else {
        panic!("bounded jobs page");
    };
    assert_eq!(page.total, ids.len() as u64);
    assert_eq!(page.items.len(), 32);
    assert_eq!(page.next_offset, Some(32));
    let mut observed: Vec<_> = page
        .items
        .iter()
        .map(|item| item.identity.clone())
        .collect();
    let mut after = page.next_offset;
    while let Some(offset) = after {
        let next = fixture
            .host
            .inner
            .checkpoint_page(&page.read_cut, "jobs", offset, 32)
            .unwrap();
        assert!(next.items.len() <= 32);
        observed.extend(next.items.iter().map(|item| item.identity.clone()));
        assert!(next.next_offset.is_none_or(|next| next > offset));
        after = next.next_offset;
    }
    assert_eq!(
        observed,
        ids.iter()
            .rev()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
}

/// Detached background work stays visible after the originating
/// attempt terminates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_survives_attempt_termination() {
    let (_, fixture) =
        host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = fixture
        .runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-bg"),
                },
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                mode: ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            },
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
        .runtime
        .tool_runtime()
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits")
    else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;

    // Run one attempt to completion.
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;

    // The detached execution remains visible after the attempt
    // terminated and settles on its own schedule.
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(matches!(
        snapshot.attempt.expect("attempt view").phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
    assert_eq!(snapshot.jobs.len(), 1);
    assert!(matches!(
        snapshot.jobs[0].state,
        BackgroundLifecycle::Running
    ));
    release.send_replace(true);
    let terminal = await_background_terminal(
        fixture.runtime.tool_runtime().background(),
        &execution_id,
        "detached background execution",
    )
    .await;
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
}

/// Agent Status is admitted from the exact same composition the model
/// path consumes: the client event's rendered text equals the canonical
/// Runtime context fact sent in the model request.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_status_projection_shares_one_composition() {
    let (adapter, fixture) =
        host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AgentStatusComposed { .. }))
    })
    .await;
    let facts: Vec<_> = events
        .iter()
        .flat_map(|event| event.event.facts())
        .collect();
    let status_index = facts
        .iter()
        .position(|fact| matches!(fact, RuntimeClientEvent::AgentStatusComposed { .. }))
        .expect("status event index");
    let committed_index = facts
        .iter()
        .position(|fact| {
            matches!(fact, RuntimeClientEvent::MessageCommitted { message, .. }
            if matches!(
                message,
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    )
            ))
        })
        .expect("canonical status commit event");
    assert!(
        committed_index < status_index,
        "canonical Agent Status publication precedes structured observation"
    );
    let status_event = facts
        .iter()
        .find_map(|fact| match fact {
            RuntimeClientEvent::AgentStatusComposed { status, .. } => Some(status),
            _ => None,
        })
        .expect("status event");
    // The model request is recorded slightly after the status
    // observation; wait for the attempt to settle so the request is
    // provably recorded.
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let requests = adapter.requests();
    assert_eq!(requests.len(), 1);
    let model_rendered = requests[0]
        .messages
        .iter()
        .find_map(|message| match message.as_canonical() {
            Some(MessageBlock::User(user))
                if matches!(
                    &user.kind,
                    crate::message::types::InboundKind::Context(
                        crate::message::types::ContextKind::AgentStatus(_)
                    )
                ) =>
            {
                user.content.first().and_then(|content| match content {
                    crate::message::types::UserContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("model path carries canonical Agent Status");
    assert_eq!(
        status_event.rendered, model_rendered,
        "the client view derives from the same composition as the model path"
    );
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    let committed_status = snapshot
        .messages
        .iter()
        .find_map(|message| match message {
            MessageBlock::User(user)
                if matches!(
                    &user.kind,
                    crate::message::types::InboundKind::Context(
                        crate::message::types::ContextKind::AgentStatus(_)
                    )
                ) =>
            {
                Some(user)
            }
            _ => None,
        })
        .expect("canonical status message");
    assert_eq!(
        status_event.status_message_id,
        Some(committed_status.id.clone())
    );
    assert!(
        status_event.opportunities.fresh_inbound.is_some(),
        "the current FreshInbound-only producer still populates its opportunity"
    );
    assert!(snapshot.messages.iter().any(|message| {
        matches!(
            message,
            MessageBlock::User(user)
                if user.id
                    == status_event
                        .opportunities
                        .fresh_inbound
                        .as_ref()
                        .expect("FreshInbound is populated by the current producer")
                        .target_message_id.as_ref().expect("native inline FreshInbound identity").clone()
        )
    }));
    assert!(matches!(
        status_event.sections.first(),
        Some(crate::observation::status::RuntimeClientStatusSection::Temporal { .. })
    ));
    assert_eq!(
        snapshot
            .statuses
            .last()
            .expect("status view")
            .rendered
            .clone(),
        model_rendered
    );
}

/// Shutdown is distinct from detach: it drains the current attempt to
/// quiescence, and detach remains available afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete shutdown lifecycle
async fn shutdown_is_not_detach_and_reaches_quiescence() {
    let (release_tx, release_rx) = model_release();
    let (_, fixture) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(release_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
    })
    .await;

    release_tx
        .send(true)
        .expect("release current model request");
    let response = attachment
        .handle_request_async(RuntimeClientRequest::Shutdown {
            id: crate::runtime_client::RequestId::new(2),
        })
        .await;
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));

    let first_shutdown_events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown))
    })
    .await;
    let (after_shutdown, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot after native shutdown publication");
    assert!(after_shutdown.shutting_down);
    assert_eq!(
        first_shutdown_events
            .iter()
            .filter(|event| event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown)))
            .count(),
        1
    );
    let repeated = attachment
        .handle_request_async(RuntimeClientRequest::Shutdown {
            id: crate::runtime_client::RequestId::new(5),
        })
        .await;
    assert!(matches!(
        repeated.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));
    let mut duplicate_shutdown = false;
    loop {
        match subscription.try_next() {
            EventDelivery::Event(event) => {
                duplicate_shutdown |= event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown));
            }
            EventDelivery::Pending => break,
            delivery => panic!("subscription remains open after repeat: {delivery:?}"),
        }
    }
    assert!(
        !duplicate_shutdown,
        "repeated shutdown publishes no duplicate fact"
    );
    let snapshot_response = attachment.handle_request(RuntimeClientRequest::CheckpointGet {
        id: crate::runtime_client::RequestId::new(4),
    });
    let Some(RuntimeClientResult::Checkpoint { checkpoint }) = snapshot_response.result else {
        panic!("checkpoint returns the shutdown state: {snapshot_response:?}");
    };
    assert!(checkpoint.snapshot.shutting_down);

    // Further admission fails explicitly.
    let submit = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(3),
        content: submit_content("too late"),
    });
    assert!(matches!(
        submit.error,
        Some(RuntimeClientError::RuntimeShutdown)
    ));

    // The current attempt settled before shutdown completed; detach still
    // works independently of runtime lifetime.
    attachment.detach();
    let (reattached, initialized) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach after shutdown still works");
    let RuntimeClientResult::Initialized { .. } = initialized else {
        panic!("fresh initialize returns a bounded identity receipt");
    };
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .unwrap();
    assert!(snapshot.shutting_down);
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(matches!(
        snapshot.attempt.expect("attempt view").phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
    assert!(
        reattached
            .handle_request(RuntimeClientRequest::CheckpointGet {
                id: crate::runtime_client::RequestId::new(1),
            })
            .error
            .is_none()
    );
}

/// One human inbound through the Runtime Client and one Runtime/Agent
/// inbound through the native publisher reach the same coordinator
/// admission path: one finite batch, mailbox order preserved, exactly
/// one attempt (Test 3).
///
/// The admission gate makes the interleaving exact: the async enqueue
/// starts an admission that parks before the coordinator lock, so the
/// human submit provably lands in the mailbox before the finite drain.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_and_runtime_inbound_share_one_admission_path() {
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (adapter, fixture) = host_fixture_probe_with_runtime_gate(
        Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default()),
        vec![one_turn_stop()],
        CoordinatorProbe {
            admission_gate: Some(admission_gate.clone()),
            settlement_gate: None,
            before_restore_gate: None,
            activation_gate: None,
            manual_compaction_settlement_gate: None,
            submit_gate: None,
            submit_arrival: None,
            shutdown_arrival: None,
            mcp_failure_drain_gate: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            drain_linearization: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
            ..CoordinatorProbe::default()
        },
    )
    .await;
    admission_gate.arm();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    // The native Runtime producer publishes first (a background-style
    // terminal notification), waking the admission worker; the worker
    // parks at the runtime gate before the coordinator lock, so the
    // human submit below provably lands before the finite drain.
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text(
            "conv_15466ddf-043b-7e5a-9915-c32c837b38bc",
            "runtime",
        ))
        .expect("runtime enqueue");
    admission_gate.wait_entered();
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("human"),
    });

    // Release: one admission drains both messages in mailbox order.
    admission_gate.release();
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;

    // Exactly one attempt observed both messages in mailbox order.
    let requests = adapter.requests();
    assert_eq!(requests.len(), 1, "one admission, one attempt");
    let inbound_ids: Vec<&str> = requests[0]
        .messages
        .iter()
        .filter_map(|message| match message.as_canonical() {
            Some(MessageBlock::User(user))
                if user.kind == crate::message::types::InboundKind::Message =>
            {
                Some(user.id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        inbound_ids,
        vec![
            "conv_15466ddf-043b-7e5a-9915-c32c837b38bc",
            "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-2"
        ],
        "both producers sequence through one durable sequence domain in order"
    );
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(
        snapshot.inbound.pending.is_empty(),
        "the batch was consumed exactly once"
    );
}

/// An enqueue racing attempt settlement loses nothing and creates at
/// most one next attempt (Test 6).
///
/// The settlement gate parks `finish_attempt` after the conversation
/// state is restored and the current-attempt slot is cleared, before
/// the next-admission handoff; the enqueue during that park provably
/// races the settlement boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enqueue_racing_settlement_admits_exactly_one_next_attempt() {
    let settlement_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (adapter, fixture) = host_fixture_with_runtime_probe(
        vec![one_turn_stop(), one_turn_stop()],
        CoordinatorProbe {
            admission_gate: None,
            settlement_gate: Some(settlement_gate.clone()),
            before_restore_gate: None,
            activation_gate: None,
            manual_compaction_settlement_gate: None,
            submit_gate: None,
            submit_arrival: None,
            shutdown_arrival: None,
            mcp_failure_drain_gate: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            drain_linearization: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
            ..CoordinatorProbe::default()
        },
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");

    // Run the first attempt; its settlement handoff parks at the gate
    // after the conversation restore and the slot clear.
    settlement_gate.arm();
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("first"),
    });
    settlement_gate.wait_entered();

    // An ordinary async enqueue lands while the settlement handoff is
    // parked (the gate holds the coordinator lock after the
    // conversation restore and the slot clear, so the test never reads
    // coordinator state while it is parked).
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text(
            "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf",
            "racing settlement",
        ))
        .expect("async enqueue");

    // Release the handoff: exactly one next attempt consumes the
    // inbound; the settlement path never consumes it again. Waiting on
    // the request-history transfer makes both settlements provable
    // before the assertions below.
    settlement_gate.release();
    await_request_history_len(&fixture.host, 2).await;
    await_adapter_request_count(&adapter, 2).await;
    let requests = adapter.requests();
    assert_eq!(
        requests.len(),
        2,
        "exactly one next attempt (never two, never zero)"
    );
    assert!(
        requests[1].messages.iter().any(|message| matches!(
            message.as_canonical(),
            Some(MessageBlock::User(user)) if user.id.as_str() == "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf"
        )),
        "the racing inbound was consumed exactly once by the next attempt"
    );
    let (snapshot, _) = fixture
        .host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(
        snapshot.inbound.pending.is_empty(),
        "no second consumption of the same batch"
    );
}

/// The safe-boundary tool-batch invariant (Test 8): with sibling tool
/// calls A and B, an async inbound arriving while A executes is never
/// interleaved between the tool results — the full sibling structural
/// settlement lands before the inbound enters model-visible context.
#[allow(clippy::too_many_lines)] // one complete tool-batch lifecycle
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safe_boundary_keeps_sibling_tool_results_together() {
    use crate::runtime::identity::ToolCallId;
    let (tool_a, mut a_started, a_release) = ParkingBackgroundTool::new();
    let (tool_b, mut b_started, b_release) = ParkingBackgroundTool::new();
    let definition_a = ToolDefinition {
        id: ToolId::new("tool-a"),
        name: "a".to_owned(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Parallel,
        ..tool_a.definition.clone()
    };
    let definition_b = ToolDefinition {
        id: ToolId::new("tool-b"),
        name: "b".to_owned(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Parallel,
        ..tool_b.definition.clone()
    };
    let mut tools = ToolRegistry::new();
    tools
        .register(definition_a.clone(), Arc::new(tool_a))
        .expect("register a");
    tools
        .register(definition_b.clone(), Arc::new(tool_b))
        .expect("register b");
    let call_a = ToolCallId::new("call-a");
    let call_b = ToolCallId::new("call-b");
    let script = vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCallStart {
                id: call_a.clone(),
                tool_id: definition_a.id.clone(),
                name: definition_a.name.clone(),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: call_a.clone(),
            arguments_delta: "{}".to_owned(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCall {
                id: call_a.clone(),
                tool_id: definition_a.id.clone(),
                name: definition_a.name.clone(),
                arguments: serde_json::json!({}),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(1),
            call: crate::tools::types::ToolCallStart {
                id: call_b.clone(),
                tool_id: definition_b.id.clone(),
                name: definition_b.name.clone(),
            },
        }),
        GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(1),
            call_id: call_b.clone(),
            arguments_delta: "{}".to_owned(),
        }),
        GatedStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(1),
            call: crate::tools::types::ToolCall {
                id: call_b.clone(),
                tool_id: definition_b.id.clone(),
                name: definition_b.name.clone(),
                arguments: serde_json::json!({}),
            },
        }),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ];
    let (adapter, fixture) =
        host_fixture(vec![script, one_turn_stop()], tools, status_engine()).await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("go"),
    });
    // Both sibling tool calls start (the loop executes the batch); A
    // parks.
    await_background_started(&mut a_started, "tool A started").await;
    await_background_started(&mut b_started, "tool B started").await;

    // An async inbound arrives while the sibling batch is in flight.
    fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .enqueue(inbound_text(
            "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf",
            "during batch",
        ))
        .expect("async enqueue");

    // Release both tools; the batch settles structurally, the safe
    // boundary drains the inbound into the next turn.
    a_release.send_replace(true);
    b_release.send_replace(true);
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let requests = adapter.requests();
    assert_eq!(requests.len(), 2, "tool turn + drained-inbound turn");
    // The second request replays the whole conversation; the model-
    // visible tail must be ToolResult A, ToolResult B, then the drained
    // inbound — the sibling structural settlement always lands before
    // the inbound enters model-visible context.
    let roles: Vec<&str> = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message.as_canonical() {
            Some(MessageBlock::Tool(_)) => Some("tool"),
            Some(MessageBlock::User(user))
                if user.kind == crate::message::types::InboundKind::Message =>
            {
                Some("user")
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        &roles[roles.len() - 3..],
        &["tool", "tool", "user"],
        "ToolResult A, ToolResult B, then the inbound — never interleaved"
    );
    assert!(
        requests[1].messages.iter().any(|message| matches!(
            message.as_canonical(),
            Some(MessageBlock::User(user)) if user.id.as_str() == "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf"
        )),
        "the drained inbound is the async one"
    );
}

/// Model configuration freezes at the admission boundary (Test 10):
/// an update that linearizes before admission is observed by the
/// admitted attempt; one that linearizes after admission affects only
/// future attempts.
#[allow(clippy::too_many_lines)] // two full freeze interleavings
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_update_freezes_at_admission() {
    // Interleaving B: the update linearizes after the admission.
    let (release_b_tx, release_b_rx) = model_release();
    let (adapter_b, fixture_b) = host_fixture(
        vec![vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::ParkUntilReleased(release_b_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment_b, _) = fixture_b
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription_b = attachment_b
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment_b.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(1),
        content: submit_content("first"),
    });
    // The attempt is provably admitted (its model stream is parked).
    receive_until(&subscription_b, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
    })
    .await;

    // The update lands mid-attempt: the running attempt keeps its
    // frozen snapshot.
    let mut updated = fixture_b.runtime.model_config();
    updated.request_params.insert(
        "frozen_probe".to_owned(),
        serde_json::json!("updated-after-admission"),
    );
    fixture_b
        .host
        .model_set(updated)
        .expect("active selection commits");

    release_b_tx.send(true).expect("release");
    receive_until(&subscription_b, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let requests = adapter_b.requests();
    assert!(
        requests[0].request_params().get("frozen_probe").is_none(),
        "the admitted attempt never observes the post-admission update"
    );

    // A later attempt observes the selection already committed while active.
    attachment_b.handle_request(RuntimeClientRequest::SubmitInbound {
        id: crate::runtime_client::RequestId::new(2),
        content: submit_content("second"),
    });
    receive_until(&subscription_b, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    let requests = adapter_b.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].request_params().get("frozen_probe"),
        Some(&serde_json::json!("updated-after-admission")),
        "a future attempt observes the update"
    );
}

/// Builds a host with the projection linearization probe installed.
async fn host_fixture_probe(
    probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
    scripts: Vec<Vec<GatedStep>>,
) -> (Arc<GatedAdapter>, HostFixture) {
    host_fixture_probe_with_native_tools(probe, scripts, false).await
}

/// Probe fixture variant with the real native tool plane activated.
#[allow(
    clippy::too_many_lines,
    reason = "one complete deterministic fixture boundary"
)]
async fn host_fixture_probe_with_native_tools(
    probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
    scripts: Vec<Vec<GatedStep>>,
    include_native_tools: bool,
) -> (Arc<GatedAdapter>, HostFixture) {
    let adapter = Arc::new(GatedAdapter::new(scripts));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        // These Runtime Client fixtures compose Agent Status and nothing
        // else: the composition must match the status engine below and
        // the extension Tool plane the coordinator is given, because
        // `ConversationRuntime` construction now proves all three follow
        // from one frozen decision (Issue #259). The Todo extension has
        // its own suites; composing it here would only add an unrelated
        // Tool and an unrelated conversation-owned resource to every
        // host contract.
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let mut tools = ToolRegistry::new();
    if include_native_tools {
        crate::tools::register_native_tools(
            &mut tools,
            crate::tools::NativeToolResources {
                subagent_catalog: crate::runtime::subagent::AgentCatalog::empty(),
                background: tool_runtime.background().clone(),
                subagents: None,
            },
            crate::tools::NativeToolPolicies::default(),
        )
        .expect("register native tools");
    }
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            agent_activation: {
                let mut activation = crate::capabilities::AgentActivation::default();
                activation.profile.skills =
                    Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                activation.profile.extensions =
                    crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                activation.profile.tools.builtin = tools
                    .definitions()
                    .into_iter()
                    .filter(|tool| tool.origin.source().is_none())
                    .map(|tool| tool.name.clone())
                    .collect();
                activation
            },
            base_tool_registry: Arc::new(tools),
            extension_tools: tool_runtime.extension_tool_plane(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.join(".agents/skills"),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let runtime = ConversationRuntime::new(RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: crate::scripted_suites::support::model::scripted_session_model(adapter.clone()),
        approval_mode: crate::runtime::ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_engine: Some(status_engine()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator.clone(),
        clock: Some(Arc::new(FixedRuntimeClock)),
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    })
    .expect("conversation runtime");
    let host = RuntimeClientHost::with_probe(
        RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        },
        (*probe).clone(),
    )
    .expect("host");
    runtime.activate();
    (
        adapter,
        HostFixture {
            directory: dir,
            host,
            runtime,
            coordinator,
            store: None,
        },
    )
}

/// Builds a host whose runtime carries both the projection probe and
/// the coordinator synchronization hooks.
async fn host_fixture_probe_with_runtime_gate(
    probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
    scripts: Vec<Vec<GatedStep>>,
    runtime_probe: CoordinatorProbe,
) -> (Arc<GatedAdapter>, HostFixture) {
    let adapter = Arc::new(GatedAdapter::new(scripts));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        // These Runtime Client fixtures compose Agent Status and nothing
        // else: the composition must match the status engine below and
        // the extension Tool plane the coordinator is given, because
        // `ConversationRuntime` construction now proves all three follow
        // from one frozen decision (Issue #259). The Todo extension has
        // its own suites; composing it here would only add an unrelated
        // Tool and an unrelated conversation-owned resource to every
        // host contract.
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: crate::capabilities::AgentActivation::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.join(".agents/skills"),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let runtime = ConversationRuntime::with_probe(
        RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: crate::scripted_suites::support::model::scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(status_engine()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        },
        runtime_probe,
    )
    .expect("conversation runtime with probe");
    let host = RuntimeClientHost::with_probe(
        RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        },
        (*probe).clone(),
    )
    .expect("host");
    runtime.activate();
    (
        adapter,
        HostFixture {
            directory: dir,
            host,
            runtime,
            coordinator,
            store: None,
        },
    )
}

/// A fixture over one conversation runtime **without** a Runtime
/// Client host, so a test controls host construction itself (the
/// Issue #61 bootstrap regressions).
struct RuntimeOnlyFixture {
    _dir: tempfile::TempDir,
    runtime: ConversationRuntime,
}

/// Builds the conversation runtime alone (no host), with the given
/// scripts, tool registry, and optional coordinator probe.
async fn runtime_only_fixture(
    scripts: Vec<Vec<GatedStep>>,
    tools: ToolRegistry,
    probe: Option<CoordinatorProbe>,
) -> (Arc<GatedAdapter>, RuntimeOnlyFixture) {
    let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
    let adapter = Arc::new(GatedAdapter::new(scripts));
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        // These Runtime Client fixtures compose Agent Status and nothing
        // else: the composition must match the status engine below and
        // the extension Tool plane the coordinator is given, because
        // `ConversationRuntime` construction now proves all three follow
        // from one frozen decision (Issue #259). The Todo extension has
        // its own suites; composing it here would only add an unrelated
        // Tool and an unrelated conversation-owned resource to every
        // host contract.
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            agent_activation: {
                let mut activation = crate::capabilities::AgentActivation::default();
                activation.profile.skills =
                    Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                activation.profile.extensions =
                    crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                activation.profile.tools.builtin = tools
                    .definitions()
                    .into_iter()
                    .filter(|tool| tool.origin.source().is_none())
                    .map(|tool| tool.name.clone())
                    .collect();
                activation
            },
            base_tool_registry: Arc::new(tools),
            extension_tools: tool_runtime.extension_tool_plane(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.join(".agents/skills"),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter.clone()),
        approval_mode: crate::runtime::ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_engine: Some(status_engine()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator.clone(),
        clock: Some(Arc::new(FixedRuntimeClock)),
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    };
    let runtime = match probe {
        Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
        None => ConversationRuntime::new(config).expect("runtime"),
    };
    (adapter, RuntimeOnlyFixture { _dir: dir, runtime })
}

/// A marker-bearing alternate session model configuration.
fn marked_model_config(
    runtime: &ConversationRuntime,
    marker: &str,
) -> crate::model::session::SessionModelConfig {
    let mut config = runtime.model_config();
    config
        .request_params
        .insert(marker.to_owned(), serde_json::json!("changed"));
    config
}

/// Test B1 — the interactive composition: construct the runtime, bind
/// the Runtime Client host over the inert runtime, activate, and run a
/// real turn end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interactive_pre_activation_bind_runs_a_real_turn() {
    let (adapter, fixture) =
        runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
    assert!(
        !fixture.runtime.is_activated(),
        "a freshly constructed runtime is inert"
    );

    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .expect("a host binds before activation");
    fixture.runtime.activate();
    assert!(fixture.runtime.is_activated());

    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let (_, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    let subscription = attachment.subscribe_events(cursor).expect("subscribe");
    attachment
        .handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("drive a turn"),
        })
        .result
        .expect("accepted");
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. })))
            .count(),
        1,
        "exactly one terminal settlement"
    );
    assert_eq!(adapter.requests().len(), 1, "the real provider path ran");
}

/// Test B2 — the headless composition: construct the runtime, bind no
/// host at all, activate, and run a real turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_activation_runs_without_any_client_host() {
    let (adapter, fixture) =
        runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
    fixture.runtime.activate();

    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    fixture
        .runtime
        .submit_inbound(submit_content("headless"))
        .expect("accepted");
    settled.await;
    assert_eq!(adapter.requests().len(), 1, "the real provider path ran");
    assert!(
        !fixture.runtime.tool_runtime().is_runtime_client_bound(),
        "no Runtime Client host ever existed"
    );
}

/// Test B3 — a Runtime Client host bind after activation is refused
/// with the explicit lifecycle error: no panic, no partial bridge, and
/// the one-time client binding claim is not consumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_host_bind_after_activation_is_rejected_typed() {
    let (adapter, fixture) = runtime_only_fixture(
        vec![one_turn_stop(), one_turn_stop()],
        ToolRegistry::new(),
        None,
    )
    .await;
    fixture.runtime.activate();
    // Semantic execution really started before the bind attempt.
    // Armed before the trigger: settlement wakes only registered waiters.
    let started = fixture.runtime.settlement_signal().notified();
    tokio::pin!(started);
    started.as_mut().enable();
    fixture
        .runtime
        .submit_inbound(submit_content("start executing"))
        .expect("accepted");
    started.await;
    assert_eq!(adapter.requests().len(), 1);

    let rejected = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    });
    match rejected {
        Err(HostConstructionError::RuntimeAlreadyActivated { conversation_id }) => {
            assert_eq!(
                conversation_id.as_str(),
                "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b"
            );
        }
        _ => panic!("a post-activation host bind must fail typed"),
    }
    // Transactional: the rejected construction consumed no binding
    // claim and installed no bridge.
    assert!(
        !fixture.runtime.tool_runtime().is_runtime_client_bound(),
        "the tool runtime binding claim was not consumed"
    );
    assert!(
        !fixture.runtime.capability().is_runtime_client_bound(),
        "the capability binding claim was not consumed"
    );
    // The runtime keeps executing normally afterwards.
    // Armed before the trigger: settlement wakes only registered waiters.
    let kept_going = fixture.runtime.settlement_signal().notified();
    tokio::pin!(kept_going);
    kept_going.as_mut().enable();
    fixture
        .runtime
        .submit_inbound(submit_content("keep going"))
        .expect("accepted");
    kept_going.await;
    assert_eq!(
        adapter.requests().len(),
        2,
        "the runtime keeps executing after the rejected bind"
    );
}

/// Test B4 — attachments stay dynamic after activation: attach, detach
/// while an attempt is running, and reattach, without ever affecting
/// semantic execution.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachments_remain_dynamic_after_activation() {
    let (release_tx, release_rx) = model_release();
    let (adapter, fixture) = runtime_only_fixture(
        vec![
            vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "working".to_owned(),
                }),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
            one_turn_stop(),
        ],
        ToolRegistry::new(),
        None,
    )
    .await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .expect("host binds before activation");
    fixture.runtime.activate();

    let (first, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let (_, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    let subscription = first.subscribe_events(cursor).expect("subscribe");
    first
        .handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        })
        .result
        .expect("accepted");
    // Wait for a real in-flight streaming fact through the client
    // projection, then detach mid-attempt.
    receive_until(&subscription, |event| {
        event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { kind, .. } if kind == "text"))
    })
    .await;
    first.detach();

    // The detached attempt still settles canonically.
    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    release_tx.send(true).expect("release the parked attempt");
    settled.await;
    assert_eq!(adapter.requests().len(), 1, "the attempt ran to settlement");

    // Reattach: a fresh attachment over the same host and the same
    // semantic owners, and the projection observed the settlement it
    // was detached for.
    let (second, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("reattach");
    let (snapshot, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert!(
        matches!(
            snapshot.attempt.as_ref().map(|attempt| &attempt.phase),
            Some(RuntimeClientAttemptPhase::Settled { .. })
        ),
        "the detach never altered semantic execution"
    );
    let subscription = second.subscribe_events(cursor).expect("subscribe");
    second
        .handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        })
        .result
        .expect("accepted");
    receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    assert_eq!(adapter.requests().len(), 2, "the reattached turn ran");
}

/// Regression for the PR #70 test-fixture lost wakeup: release the
/// background execution while its returned future is intentionally held
/// before the old `Notify::notified()` wait point. Durable release state
/// must still settle the one committed record exactly once, without a
/// second registry record or an invented admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_release_before_future_wait_cannot_be_lost() {
    let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
    let registry = fixture.tool_runtime.background().clone();
    let (tool, mut started, release, execution_gate) =
        ParkingBackgroundTool::new_with_execution_gate();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("call-lost-wakeup"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted dispatch");
    };

    await_background_started(&mut started, "lost-wakeup regression runner").await;
    // This is the critical ordering: release is durable before the
    // returned future is allowed to reach its wait point.
    release.send_replace(true);
    execution_gate.send_replace(true);

    let terminal =
        await_background_terminal(&registry, &execution_id, "lost-wakeup regression").await;
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    assert_eq!(
        registry.all_snapshots().len(),
        1,
        "no duplicate background record"
    );
    assert!(
        terminal.result.is_some(),
        "the single execution has one result"
    );
}

/// The standalone pre-runtime pieces of one ownership-transfer test:
/// the tool runtime and the capability coordinator exist over one
/// conversation, but no `ConversationRuntime` owns them yet.
struct OwnershipFixture {
    _dir: tempfile::TempDir,
    adapter: Arc<GatedAdapter>,
    tool_runtime: crate::tools::runtime::ConversationToolRuntime,
    coordinator: crate::capabilities::CapabilityCoordinator,
}

/// Builds the standalone pieces exactly like the runtime fixtures, but
/// stops before the `ConversationRuntime` construction.
async fn ownership_fixture(scripts: Vec<Vec<GatedStep>>, tools: ToolRegistry) -> OwnershipFixture {
    let adapter = Arc::new(GatedAdapter::new(scripts));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        // These Runtime Client fixtures compose Agent Status and nothing
        // else: the composition must match the status engine below and
        // the extension Tool plane the coordinator is given, because
        // `ConversationRuntime` construction now proves all three follow
        // from one frozen decision (Issue #259). The Todo extension has
        // its own suites; composing it here would only add an unrelated
        // Tool and an unrelated conversation-owned resource to every
        // host contract.
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            agent_activation: {
                let mut activation = crate::capabilities::AgentActivation::default();
                activation.profile.skills =
                    Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                activation.profile.extensions =
                    crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                activation.profile.tools.builtin = tools
                    .definitions()
                    .into_iter()
                    .filter(|tool| tool.origin.source().is_none())
                    .map(|tool| tool.name.clone())
                    .collect();
                activation
            },
            base_tool_registry: Arc::new(tools),
            extension_tools: tool_runtime.extension_tool_plane(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.join(".agents/skills"),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    OwnershipFixture {
        _dir: dir,
        adapter,
        tool_runtime,
        coordinator,
    }
}

/// The `RuntimeConversationConfig` over one ownership fixture.
fn claim_config(fixture: &OwnershipFixture) -> RuntimeConversationConfig {
    RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-claim"),
        model: scripted_session_model(fixture.adapter.clone()),
        approval_mode: crate::runtime::ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(DefaultTokenEstimator),
            status_engine: Some(status_engine()),
        },
        tool_runtime: fixture.tool_runtime.clone(),
        resources: test_resources(&fixture.coordinator),
        capability: fixture.coordinator.clone(),
        clock: Some(Arc::new(FixedRuntimeClock)),
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    }
}

/// A background invocation for the ownership-transfer tests.
fn claim_background_invocation(call_id: &str) -> ToolInvocation {
    ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new(call_id),
        },
        tool_id: ToolId::new("tool-bg"),
        tool_name: "bg".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    }
}

/// The ownership transfer rejects a tool runtime whose background plane
/// already holds a **committed** standalone execution: typed
/// `ToolRuntimeNotQuiescent`, no coordinator claim consumed, no
/// capability claim consumed, the mailbox stays standalone/unbound, and
/// the detached execution continues under standalone semantics.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conversation_runtime_claim_rejects_committed_standalone_background_work() {
    let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
    let registry = fixture.tool_runtime.background().clone();
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("call-committed"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("a standalone commit succeeds");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "the standalone runner starts").await;
    assert_eq!(
        registry.all_snapshots().len(),
        1,
        "the standalone execution is committed and running"
    );

    // The ownership transfer is refused typed...
    let refused = ConversationRuntime::new(claim_config(&fixture))
        .expect_err("a tool runtime with committed background work is not claimable");
    assert_eq!(
        refused,
        ConversationRuntimeError::ToolRuntimeNotQuiescent {
            conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
        }
    );

    // ...and consumed nothing: no coordinator claim, no capability
    // claim, and the mailbox remains standalone.
    assert!(
        !fixture.tool_runtime.is_conversation_runtime_bound(),
        "the failed claim consumed no tool-runtime ownership"
    );
    assert!(
        !fixture.coordinator.is_conversation_runtime_bound(),
        "the failed claim consumed no capability ownership"
    );
    fixture
        .tool_runtime
        .mailbox()
        .enqueue(inbound_text("standalone-1", "still standalone"))
        .expect("the mailbox remains standalone/unbound");

    // The detached execution keeps its standalone semantics and
    // settles normally.
    release.send_replace(true);
    let terminal =
        await_background_terminal(&registry, &execution_id, "standalone background execution")
            .await;
    assert_eq!(
        terminal.state,
        BackgroundLifecycle::Succeeded,
        "the standalone execution settles normally"
    );
}

/// The ownership transfer rejects a tool runtime with a **prepared but
/// not committed** dispatch: typed `ToolRuntimeNotQuiescent`, no claim
/// consumed, the mailbox stays standalone, and the prepared handle
/// keeps its standalone semantics — dropping it rolls the dispatch back
/// with no fabricated record. Once the background plane is pristine
/// again, a fresh construction of the same identity succeeds, proving
/// the failed claim did not consume the one-time ownership.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conversation_runtime_claim_rejects_a_prepared_standalone_dispatch() {
    let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
    let registry = fixture.tool_runtime.background().clone();
    let (tool, started, _release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("call-prepared"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    assert!(
        registry.all_snapshots().is_empty(),
        "preparation publishes no record"
    );

    // The ownership transfer is refused typed and consumes nothing.
    let refused = ConversationRuntime::new(claim_config(&fixture))
        .expect_err("a staged dispatch makes the background plane non-quiescent");
    assert_eq!(
        refused,
        ConversationRuntimeError::ToolRuntimeNotQuiescent {
            conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
        }
    );
    assert!(!fixture.tool_runtime.is_conversation_runtime_bound());
    assert!(!fixture.coordinator.is_conversation_runtime_bound());
    fixture
        .tool_runtime
        .mailbox()
        .enqueue(inbound_text("standalone-2", "still standalone"))
        .expect("the mailbox remains standalone/unbound");

    // The prepared handle stays valid under standalone semantics:
    // dropping it rolls the dispatch back and fabricates no record.
    drop(prepared);
    assert!(
        registry.all_snapshots().is_empty(),
        "the rolled-back dispatch published no record"
    );
    assert!(!*started.borrow(), "the rolled-back runner never begins");

    // The one-time claim was not consumed by the failed construction:
    // a fresh claim of the same identity succeeds once the background
    // plane is pristine again.
    let runtime = ConversationRuntime::new(claim_config(&fixture))
        .expect("a fresh claim succeeds after the prepared dispatch rolled back");
    assert!(
        fixture.tool_runtime.is_conversation_runtime_bound(),
        "the retried construction owns the identity"
    );
    assert!(!runtime.is_activated(), "the runtime is still inactive");
    runtime.activate();
}

/// Interleaving A of the ownership-transfer race: a standalone
/// background commit parked exactly at its ownership-commit boundary
/// (holding the registry synchronization lock) beats the racing
/// `ConversationRuntime::new`. The claim provably linearizes after the
/// committed record, fails typed `ToolRuntimeNotQuiescent`, and
/// consumes nothing — the mailbox stays standalone and the detached
/// execution stays valid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_commit_racing_the_runtime_claim_wins_and_construction_fails_typed() {
    let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
    let registry = fixture.tool_runtime.background().clone();
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("call-race-a"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
    registry.install_commit_boundary_hook(hook.clone());

    // The commit enters its critical section and parks there, holding
    // the registry lock at the ownership-commit boundary.
    let commit_registry = registry.clone();
    let commit_task = tokio::task::spawn_blocking(move || {
        commit_registry.commit_dispatch(prepared, &CancellationSignal::new())
    });
    {
        let hook = hook.clone();
        tokio::task::spawn_blocking(move || hook.wait_entered())
            .await
            .expect("the commit entered the ownership boundary");
    }

    // Race the runtime claim: the rendezvous marker proves the claim
    // thread is in flight while the commit is parked, and because the
    // commit holds the registry lock, the claim's quiescence
    // observation necessarily linearizes *after* the commit's record
    // publication.
    let (marker_tx, marker_rx) = std::sync::mpsc::sync_channel(0);
    let claim_config = claim_config(&fixture);
    let claim_task = tokio::task::spawn_blocking(move || {
        marker_tx.send(()).expect("the claim is in flight");
        ConversationRuntime::new(claim_config)
    });
    marker_rx
        .recv()
        .expect("the claim thread started while the commit was parked");

    // Release the boundary: the commit wins, publishes the record, and
    // only then may the claim acquire the registry lock.
    {
        let hook = hook.clone();
        tokio::task::spawn_blocking(move || hook.proceed())
            .await
            .expect("the commit boundary was released");
    }
    let outcome = commit_task
        .await
        .expect("commit outcome")
        .expect("the standalone commit succeeds");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "the standalone runner starts").await;

    let refused = claim_task
        .await
        .expect("claim outcome")
        .expect_err("the claim linearizes after the committed background record");
    assert_eq!(
        refused,
        ConversationRuntimeError::ToolRuntimeNotQuiescent {
            conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
        }
    );
    assert!(!fixture.tool_runtime.is_conversation_runtime_bound());
    assert!(!fixture.coordinator.is_conversation_runtime_bound());
    fixture
        .tool_runtime
        .mailbox()
        .enqueue(inbound_text("standalone-3", "still standalone"))
        .expect("the mailbox remains standalone/unbound");

    release.send_replace(true);
    let terminal = await_background_terminal(
        &registry,
        &execution_id,
        "racing standalone background execution",
    )
    .await;
    assert_eq!(
        terminal.state,
        BackgroundLifecycle::Succeeded,
        "the racing standalone execution settles normally"
    );
}

/// Interleaving B of the ownership-transfer race: the runtime ownership
/// transfer linearizes first, and a background commit that arrives
/// afterwards fails typed `ConversationInactive` — no record published,
/// the prepared runner rolls back, and the runtime remains inert until
/// `activate()`. Ordinary capability commits are independently rejected
/// because live publication belongs to native configuration coordination; after activation,
/// a fresh background dispatch commits normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_claim_racing_the_background_commit_wins_and_commit_fails_inactive() {
    let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
    let registry = fixture.tool_runtime.background().clone();

    // The ownership transfer completes first: the runtime exists,
    // inactive, with its mailbox bound inactive.
    let runtime = ConversationRuntime::new(claim_config(&fixture))
        .expect("the ownership transfer wins the race");
    assert!(
        fixture.tool_runtime.is_conversation_runtime_bound(),
        "the runtime owns the tool runtime identity"
    );
    assert!(
        fixture.coordinator.is_conversation_runtime_bound(),
        "the runtime owns the capability identity"
    );
    assert!(!runtime.is_activated(), "the runtime is still inactive");

    // A background commit that linearizes after the transfer is refused
    // typed: no record, no runner start, the prepared dispatch rolls
    // back completely.
    let (tool, mut started, release) = ParkingBackgroundTool::new();
    let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("call-race-b"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("preparation is still allowed");
    let refused = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect_err("a commit after the transfer observes the inactive runtime");
    assert_eq!(
        refused,
        BackgroundDispatchError::ConversationInactive {
            conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
        }
    );
    assert!(
        registry.all_snapshots().is_empty(),
        "the refused commit published no record"
    );
    assert!(!*started.borrow(), "the rolled-back runner never begins");

    // Activation is the single semantic-open boundary: a fresh dispatch
    // commits normally afterwards.
    runtime.activate();
    let prepared = registry
        .prepare_dispatch(
            &claim_background_invocation("call-race-b-2"),
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare after activation");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("a post-activation commit succeeds");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "the post-activation runner starts").await;
    release.send_replace(true);
    let terminal = await_background_terminal(
        &registry,
        &execution_id,
        "post-activation background execution",
    )
    .await;
    assert_eq!(
        terminal.state,
        BackgroundLifecycle::Succeeded,
        "the post-activation execution settles normally"
    );
}

/// Transactional construction: when the capability claim fails after
/// the tool-runtime ownership transfer, the transfer is rolled back to
/// its exact previous standalone state — the coordinator claim is
/// cleared and the mailbox is unbound again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_capability_claim_rolls_back_the_tool_runtime_ownership_transfer() {
    let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
    // Consume the capability identity's one-time claim *before*
    // construction, so the capability claim inside `new` fails after
    // the tool-runtime transfer already succeeded.
    assert!(
        fixture
            .coordinator
            .claim_conversation_runtime(&crate::runtime::types::ConversationLifecycle::new())
            .is_some()
    );

    let refused = ConversationRuntime::new(claim_config(&fixture))
        .expect_err("a claimed capability identity rejects construction");
    assert_eq!(
        refused,
        ConversationRuntimeError::RuntimeAlreadyBound {
            conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
        }
    );
    assert!(
        !fixture.tool_runtime.is_conversation_runtime_bound(),
        "the failed construction released the tool-runtime claim"
    );
    fixture
        .tool_runtime
        .mailbox()
        .enqueue(inbound_text("standalone-4", "still standalone"))
        .expect("the rolled-back mailbox accepts standalone inbound");
}

/// The host-binding vs activation race: `RuntimeClientHost::new` and
/// `ConversationRuntime::activate` race against the same lifecycle
/// transition, serialized by the one coordinator lock the transition
/// commits under. This interleaving proves "host wins": the host
/// binds while `activate` is parked before the lifecycle transition,
/// completes with the bootstrap seed at cursor 0, and the transition
/// then commits — the first cursor belongs to a real post-activation
/// transition. The activation-wins interleaving is proven by
/// `late_host_bind_after_activation_is_rejected_typed`.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_bind_racing_activation_has_one_clean_linearization() {
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (_adapter, fixture) = runtime_only_fixture(
        vec![one_turn_stop()],
        ToolRegistry::new(),
        Some(CoordinatorProbe {
            admission_gate: None,
            settlement_gate: None,
            before_restore_gate: None,
            activation_gate: Some(gate.clone()),
            manual_compaction_settlement_gate: None,
            submit_gate: None,
            submit_arrival: None,
            shutdown_arrival: None,
            mcp_failure_drain_gate: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            drain_linearization: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    gate.arm();

    // Park `activate` before the lifecycle transition: the
    // conversation is provably still Inactive, so the host bind wins
    // the race and completes with the full bootstrap seed.
    let runtime = fixture.runtime.clone();
    let activate_task = tokio::task::spawn_blocking(move || runtime.activate());
    {
        let gate = gate.clone();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .expect("activate entered the gate");
    }
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .expect("the host binds before the lifecycle transition");
    assert!(
        fixture.runtime.tool_runtime().is_runtime_client_bound(),
        "the successful bind consumed the one-time claim"
    );
    let (snapshot, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert!(
        snapshot.jobs.is_empty(),
        "the inert runtime contributes no background seed"
    );

    // Release the transition: activation commits and the one-time
    // post-activation kick runs.
    {
        let gate = gate.clone();
        tokio::task::spawn_blocking(move || gate.release())
            .await
            .expect("the activation gate was released");
    }
    activate_task.await.expect("activate completes");
    assert!(fixture.runtime.is_activated());

    // The first cursor belongs to a real post-activation transition.
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe from the bootstrap cursor");
    host.submit_inbound(submit_content("first transition"))
        .expect("accepted");
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
    })
    .await;
    assert_eq!(
        events[0].cursor,
        RuntimeClientCursor::new(1),
        "the first cursor is a real post-activation transition"
    );
}

/// Concurrent `activate` calls are idempotent: exactly one call
/// commits the lifecycle transition (`Inactive -> Running` CAS) and
/// performs the one-time post-transition work — worker spawn and the
/// admission kick — and every other call observes `Running` and returns
/// without changing anything. A single inbound item therefore admits
/// exactly one attempt, never two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_activation_is_idempotent_and_creates_one_worker() {
    let (adapter, fixture) =
        runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
    let runtime = fixture.runtime.clone();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let a = {
        let runtime = runtime.clone();
        let barrier = barrier.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            runtime.activate();
        })
    };
    let b = {
        let runtime = runtime.clone();
        let barrier = barrier.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            runtime.activate();
        })
    };
    barrier.wait();
    a.await.expect("activate a");
    b.await.expect("activate b");
    assert!(fixture.runtime.is_activated());

    // One inbound item admits exactly one attempt: a duplicated
    // activation kick can never admit a second attempt from one item,
    // and a duplicated worker is structurally impossible (one CAS
    // winner, one `worker_started` guard).
    // Armed before the trigger: settlement wakes only registered waiters.
    let settled = fixture.runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    fixture
        .runtime
        .submit_inbound(submit_content("one item"))
        .expect("accepted");
    settled.await;
    assert_eq!(
        adapter.requests().len(),
        1,
        "exactly one attempt from one activation epoch"
    );
}

/// Test A — a model mutation while the runtime is inactive is rejected
/// typed and consumes nothing: the model is unchanged, the host
/// snapshot stays at cursor 0, and no `SessionModelChanged` event
/// exists. After activation the same update succeeds and is delivered
/// exactly once with the first real cursor.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_set_while_inactive_is_rejected_and_consumes_nothing() {
    let (_adapter, fixture) =
        runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .expect("host construction");

    // The live mutation is refused typed while inactive...
    let refused = fixture
        .runtime
        .model_set(marked_model_config(&fixture.runtime, "early"))
        .expect_err("a model update while inactive is refused");
    assert_eq!(refused, ModelUpdateError::Inactive);

    // ...consumes nothing: the model is unchanged, the snapshot is the
    // bootstrap state at cursor 0, and no event exists.
    let (snapshot, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert!(
        snapshot
            .model
            .as_ref()
            .unwrap()
            .configured
            .request_params
            .get("early")
            .is_none(),
        "the rejected update left the model unchanged"
    );
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    assert!(
        matches!(subscription.try_next(), EventDelivery::Pending),
        "the rejected update published no event"
    );

    // After activation the same update succeeds and receives the first
    // real cursor, exactly once.
    fixture.runtime.activate();
    fixture
        .runtime
        .model_set(marked_model_config(&fixture.runtime, "early"))
        .expect("the update succeeds after activation");
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. }))
    })
    .await;
    assert_eq!(
        events[0].cursor,
        RuntimeClientCursor::new(1),
        "the first cursor belongs to the real post-activation transition"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. })))
            .count(),
        1,
        "the post-activation update is delivered exactly once"
    );
}

/// Test B — a shutdown while the runtime is inactive is rejected typed
/// and is non-semantic: the runtime is not marked shutting down, the
/// snapshot stays at cursor 0, and no `RuntimeShutdown` event exists.
/// After activation shutdown retains the existing runtime semantics.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_while_inactive_is_rejected_and_consumes_nothing() {
    let (_adapter, fixture) =
        runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .expect("host construction");

    assert_eq!(
        fixture.runtime.shutdown().await,
        Err(crate::runtime::conversation_runtime::ShutdownError::Inactive),
        "a shutdown while inactive is refused typed"
    );
    let (snapshot, cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert!(
        !snapshot.shutting_down,
        "the refused shutdown never marked the runtime shutting down"
    );
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    assert!(
        matches!(subscription.try_next(), EventDelivery::Pending),
        "the refused shutdown published no event"
    );

    // After activation shutdown completes its one semantic drain,
    // publishes the drain observation exactly once, and gates inbound
    // afterwards.
    fixture.runtime.activate();
    fixture
        .runtime
        .shutdown()
        .await
        .expect("accepted after activation");
    let events = receive_until(&subscription, |event| {
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown))
    })
    .await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown)))
            .count(),
        1,
        "the post-activation shutdown publishes exactly one event"
    );
    assert_eq!(
        fixture.runtime.submit_inbound(submit_content("late")),
        Err(InboundAdmissionError::Shutdown)
    );
}

/// A model update that linearizes after activation — that is, after
/// the bootstrap cut — arrives through the live observation stream
/// exactly once and is never lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_activation_model_transition_is_delivered_exactly_once() {
    let (_adapter, fixture) =
        runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .expect("host construction");
    fixture.runtime.activate();

    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe from the bootstrap cursor");

    fixture
        .runtime
        .model_set(marked_model_config(&fixture.runtime, "after-cut"))
        .expect("model transition after the cut");

    let (snapshot, _cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("snapshot");
    assert_eq!(
        snapshot
            .model
            .as_ref()
            .unwrap()
            .configured
            .request_params
            .get("after-cut"),
        Some(&serde_json::json!("changed")),
        "the post-cut transition must be visible in the projection"
    );
    let mut session_model_events = 0;
    receive_until(&subscription, |event| {
        if event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. }))
        {
            session_model_events += 1;
        }
        event
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. }))
    })
    .await;
    assert_eq!(
        session_model_events, 1,
        "the post-cut transition is delivered exactly once"
    );
}

/// Failed host construction never leaves a claimed-but-invalid
/// binding: when the observation bridge handshake fails (a previous
/// headless bridge exists), the one-time client binding claim is
/// released again and the failure is typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_host_construction_releases_the_client_binding() {
    let (_adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
    // A headless observation bridge already exists over the runtime.
    let queue = Arc::new(crate::runtime::observation::PendingObservations::new());
    fixture
        .runtime
        .install_observation_bridge(queue)
        .expect("headless bridge");

    let rejected = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    });
    match rejected {
        Err(HostConstructionError::ObservationBridgeAlreadyInstalled { conversation_id }) => {
            assert_eq!(
                conversation_id.as_str(),
                "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b"
            );
        }
        _ => panic!("the bridge conflict must fail typed"),
    }
    // The failed construction released the binding claim: no
    // claimed-but-invalid binding remains.
    assert!(
        !fixture.runtime.tool_runtime().is_runtime_client_bound(),
        "the tool runtime binding was released"
    );
    assert!(
        !fixture.runtime.capability().is_runtime_client_bound(),
        "the capability binding was released"
    );
}
