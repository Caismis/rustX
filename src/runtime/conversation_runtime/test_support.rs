//! Shared fixtures of the `ConversationRuntime` domain tests: headless runtime
//! construction over scripted models, durable seeds, and observation waits.

use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::durable::ProjectionStore;
pub(super) use std::sync::Arc;

pub(super) fn fixture_activation(
    runtime: &crate::tools::runtime::ConversationToolRuntime,
) -> crate::capabilities::AgentActivation {
    let mut activation = crate::capabilities::AgentActivation::default();
    activation.profile.extensions =
        crate::scripted_suites::common::plugin_document(runtime.extensions());
    activation.profile.skills = Some(crate::runtime::agent_profile::AgentSkillSelection::All);
    activation
}

/// A Builtin-only frozen named-agent specification for registry-level
/// tests: resolution is the resolver's concern, so a registry test
/// supplies its already-frozen result.
pub(super) fn test_resolved_subagent(
    agent: &str,
) -> crate::runtime::subagent::ResolvedSubagentSpec {
    crate::runtime::subagent::ResolvedSubagentSpec {
        resource_sources: std::collections::BTreeSet::new(),
        environment: Vec::new(),
        generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
        skill_roots: Vec::new(),

        selection: crate::runtime::agent_profile::FrozenAgentSelection::default(),
        agent: crate::runtime::subagent::SubagentName::parse(agent).expect("canonical name"),
        definition_digest: serde_json::from_value(serde_json::json!(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .expect("digest"),
        execution_deadline: None,
        instructions: "instructions".to_owned(),
        model: crate::model::frozen::test_frozen_model_spec(
            serde_json::from_value(serde_json::json!("local/model")).expect("model reference"),
        ),
        tools: Vec::new(),
        skills: Vec::new(),
        project_instructions: Vec::new(),
        materialization:
            crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
        extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
    }
}

pub(super) use crate::agent::{LifecycleError, PreToolDecision, PreToolPolicy, PreToolView};
pub(super) use crate::context::{
    AgentStatusEngine, ClosureTokenEstimator, ContextError, ContextErrorKind,
    DefaultTokenEstimator, TokenEstimator,
};
pub(super) use crate::durable::ConversationStore;
pub(super) use crate::events::types::RuntimeEvent;
pub(super) use crate::message::content::TextBlock;
pub(super) use crate::message::types::{InboundKind, MessageBlock, UserContentBlock, UserSource};
pub(super) use crate::model::adapter::ModelAdapter;
pub(super) use crate::runtime::ApprovalMode;
#[cfg(unix)]
pub(super) use crate::runtime::cancellation::CancellationSignal;
pub(super) use crate::runtime::conversation_runtime::{
    CancelAttemptError, ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    CoordinatorProbe, Gate, InboundAdmissionError, ManualCompactionError, ModelUpdateError,
    PendingObservations, RuntimeConversationConfig,
};
pub(super) use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, SubagentId, ToolCallId, ToolId, TurnId,
};
pub(super) use crate::runtime::interaction::{
    ApprovalDecision, InteractionOutcome, InteractionResponse, InteractionSettleGate,
    InteractionWaitCancellationGate,
};
pub(super) use crate::runtime::observation::ConversationObservation;
pub(super) use crate::runtime::request_history::RequestHistory;
pub(super) use crate::runtime::types::{
    CancellationReason, ConversationLifecycleState, DurableOperation,
};
pub(super) use crate::runtime_client::event::RuntimeClientEvent;
pub(super) use crate::runtime_client::host::{
    EventDelivery, RuntimeClientHost, RuntimeClientHostConfig,
};
pub(super) use crate::runtime_client::types::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RequestId, RuntimeClientError, RuntimeClientRequest,
    RuntimeClientResult,
};
pub(super) use crate::scripted_suites::support::fake::{
    FakeModel, FakeStep, FakeTool, model_release, success_result,
};
pub(super) use crate::scripted_suites::support::model::scripted_session_model;
pub(super) use crate::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
pub(super) use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionStatus,
    ToolInvocation, ToolOrigin, ToolReplayPolicy,
};
pub(super) use futures_util::future::BoxFuture;

/// Adopts one accepted inbound item with the durable answer obligation the
/// adoption transaction requires.
pub(super) fn adopt_accepted(
    store: &dyn ConversationStore,
    accepted: &crate::durable::inbox::AcceptedInbound,
) {
    store
        .adopt_pending_batch(accepted.sequence, None)
        .expect("adopt");
}

pub(super) fn test_resources(
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

pub(super) fn request_snapshots(history: &RequestHistory) -> Vec<crate::model::RequestSnapshot> {
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

/// A headless runtime fixture: the conversation runtime with zero
/// Runtime Client attachments, over a scripted model adapter and an
/// optional runtime-owned observation bridge the test folds itself.
pub(super) struct HeadlessFixture {
    pub(super) _dir: tempfile::TempDir,
    pub(super) runtime: ConversationRuntime,
    pub(super) model: Arc<FakeModel>,
    pub(super) pending: Option<Arc<PendingObservations>>,
}

/// The outer liveness guard of a deterministic ordering proof: the
/// synchronization below is exact, so this only turns a defect that
/// breaks the ordering into a failure instead of a hang.
pub(super) async fn within_liveness_guard<F: std::future::Future>(
    label: &str,
    future: F,
) -> F::Output {
    tokio::time::timeout(std::time::Duration::from_mins(1), future)
        .await
        .unwrap_or_else(|_| panic!("liveness guard exceeded while waiting for {label}"))
}

pub(super) fn text_content(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })]
}

/// Test-only policy used by the real `ConversationRuntime` interaction
/// shutdown regression. It is deliberately a concrete one-shot policy:
/// production runtime construction derives approval from its effective
/// `ApprovalMode` and has no policy factory in its public configuration.
pub(super) struct RuntimeAskPolicy;

impl PreToolPolicy for RuntimeAskPolicy {
    fn evaluate<'a>(
        &'a self,
        _view: &'a PreToolView<'a>,
    ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>> {
        Box::pin(async {
            Ok(PreToolDecision::Ask {
                reason: "runtime shutdown regression".to_owned(),
            })
        })
    }
}

pub(super) fn one_turn_script() -> Vec<FakeStep> {
    text_turn_script("done")
}

pub(super) fn text_turn_script(text: &str) -> Vec<FakeStep> {
    use crate::message::types::ContentBlockIndex;
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

/// Builds the conversation runtime of one headless fixture.
pub(super) async fn headless_runtime(
    dir: &tempfile::TempDir,
    scripts: Vec<Vec<FakeStep>>,
    base_tool_registry: Option<crate::tools::executor::ToolRegistry>,
    probe: Option<CoordinatorProbe>,
) -> (ConversationRuntime, Arc<FakeModel>) {
    headless_runtime_with_options(
        dir,
        scripts,
        base_tool_registry,
        probe,
        HeadlessRuntimeOptions::default(),
    )
    .await
}

pub(super) struct HeadlessRuntimeOptions {
    pub(super) skill_discovery: crate::skills::SkillDiscoveryConfig,
    pub(super) estimator: Arc<dyn TokenEstimator>,
    pub(super) policy: crate::context::SessionContextPolicy,
    pub(super) status_engine: AgentStatusEngine,
    pub(super) initial_messages: Vec<MessageBlock>,
    pub(super) project_context_files: Vec<crate::runtime::ProjectContextFile>,
    pub(super) agent_profile: Option<String>,
    pub(super) workflow_catalog: crate::runtime::WorkflowCatalog,
    pub(super) mcp_servers: std::collections::BTreeMap<
        crate::runtime::identity::McpServerId,
        crate::tools::mcp::McpServerBinding,
    >,
    pub(super) pre_activation_mcp_close_probe:
        Option<Arc<crate::tools::mcp::test_sync::CloseProbe>>,
}

impl Default for HeadlessRuntimeOptions {
    fn default() -> Self {
        Self {
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            estimator: Arc::new(DefaultTokenEstimator),
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            status_engine: AgentStatusEngine::default(),
            initial_messages: Vec::new(),
            project_context_files: Vec::new(),
            agent_profile: None,
            workflow_catalog: crate::runtime::WorkflowCatalog::empty(),
            mcp_servers: std::collections::BTreeMap::new(),
            pre_activation_mcp_close_probe: None,
        }
    }
}

/// Skill context fixtures explicitly admit the native lazy-load dependency.
pub(super) fn with_native_read(mut registry: ToolRegistry) -> ToolRegistry {
    let read = crate::tools::native::subagent_child_definition(
        "read",
        crate::tools::NativeToolPolicies::default().read,
    )
    .unwrap();
    crate::tools::native::register_subagent_child_tools(&mut registry, &[read]).unwrap();
    registry
}

/// The configurable headless fixture variant used by exact context-input
/// regressions without changing the defaults of the broad runtime suite.
#[allow(
    clippy::too_many_lines,
    reason = "complete headless runtime fixture composition"
)]
pub(super) async fn headless_runtime_with_options(
    dir: &tempfile::TempDir,
    scripts: Vec<Vec<FakeStep>>,
    base_tool_registry: Option<crate::tools::executor::ToolRegistry>,
    probe: Option<CoordinatorProbe>,
    options: HeadlessRuntimeOptions,
) -> (ConversationRuntime, Arc<FakeModel>) {
    let conversation_id = ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        ))
        .with_extensions(
            crate::extensions::NativeAgentExtensions::with_todo()
                .and_agent_status(options.status_engine.config().clone()),
        ),
    )
    .expect("tool runtime");
    let base_tool_registry = base_tool_registry.unwrap_or_default();
    let selected_builtin = base_tool_registry
        .definitions()
        .into_iter()
        .filter(|tool| tool.origin.source().is_none())
        .map(|tool| tool.name.clone())
        .collect();
    let source_selection: std::collections::BTreeMap<_, _> = base_tool_registry
        .definitions()
        .into_iter()
        .filter_map(|definition| definition.origin.source())
        .chain(
            options
                .mcp_servers
                .keys()
                .cloned()
                .map(crate::capabilities::ToolSourceId::Mcp),
        )
        .map(|source| {
            (
                source,
                crate::capabilities::selection::SourceToolSelection::All,
            )
        })
        .collect();
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::new(
                options
                    .mcp_servers
                    .keys()
                    .cloned()
                    .map(crate::capabilities::ToolSourceId::Mcp),
                crate::runtime::resources::ManagedPythonCatalog::default(),
            ),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(base_tool_registry),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: crate::capabilities::AgentActivation {
                profile: crate::local_runtime::config::AgentProfileDocument {
                    tools: crate::capabilities::selection::ToolSelectionDocument {
                        builtin: selected_builtin,
                        sources: source_selection,
                    },
                    ..fixture_activation(&tool_runtime).profile
                },
                ..Default::default()
            },
            skill_discovery: options.skill_discovery,
            mcp_servers: options.mcp_servers.clone(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    if let Some(probe) = options.pre_activation_mcp_close_probe.clone() {
        for server_id in options.mcp_servers.keys() {
            if let Some(runtime) = coordinator.current_mcp_runtime(server_id) {
                runtime.install_close_probe(probe.clone());
            }
        }
        coordinator.retire_current_mcp_runtimes();
        assert!(
            coordinator.settle_ready_mcp_runtimes().await.is_err(),
            "the pre-activation close probe must publish an authoritative failure"
        );
    }
    let model = Arc::new(FakeModel::new(scripts));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let resources = Arc::new(
        crate::runtime::RuntimeResourceSnapshot::new(
            crate::runtime::RuntimeResourceRevision::new(1),
            options.project_context_files,
            options.agent_profile,
            crate::context::ContextAssembly::new(),
            coordinator.current_snapshot(),
        )
        .with_workflow_catalog(options.workflow_catalog),
    );
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: options.policy,
            estimator: options.estimator,
            status_engine: Some(options.status_engine),
        },
        tool_runtime,
        resources,
        capability: coordinator,
        clock: None,
        initial_messages: options.initial_messages,
        subagents: None,
        workflow_output: None,
    };
    let runtime = match probe {
        Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
        None => ConversationRuntime::new(config).expect("runtime"),
    };
    (runtime, model)
}

/// Builds the conversation runtime of one headless fixture over a supplied
/// durable authority. The runtime derives its mailbox from this same
/// authority binding.
pub(super) async fn headless_runtime_over_store(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    store: Arc<dyn ConversationStore>,
) -> (ConversationRuntime, Arc<FakeModel>) {
    headless_runtime_over_store_with(dir, conversation_id, store, vec![one_turn_script()], None)
        .await
}

/// The same durable-authority fixture with explicit model scripts and an
/// optional coordinator probe, for the M9c supervision regressions.
pub(super) async fn headless_runtime_over_store_with(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    store: Arc<dyn ConversationStore>,
    scripts: Vec<Vec<FakeStep>>,
    probe: Option<CoordinatorProbe>,
) -> (ConversationRuntime, Arc<FakeModel>) {
    headless_runtime_over_store_with_policy(
        dir,
        conversation_id,
        store,
        scripts,
        probe,
        crate::model::ModelTimeoutPolicy::default(),
    )
    .await
}

/// The same durable-authority fixture with an explicit model deadline.
/// Long-lived parked-provider tests use this to keep their deterministic
/// channel/barrier ordering independent of full-suite wall-clock load.
pub(super) async fn headless_runtime_over_store_with_policy(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    store: Arc<dyn ConversationStore>,
    scripts: Vec<Vec<FakeStep>>,
    probe: Option<CoordinatorProbe>,
    model_timeout_policy: crate::model::ModelTimeoutPolicy,
) -> (ConversationRuntime, Arc<FakeModel>) {
    let conversation_id = ConversationId::new(conversation_id);
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(crate::durable::ConversationStoreBinding::new(store.clone())),
            ..crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            )
        },
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let model = Arc::new(FakeModel::new(scripts));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy,
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    };
    let runtime = match probe {
        Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
        None => ConversationRuntime::new(config).expect("runtime"),
    };
    (runtime, model)
}

/// Builds the same headless runtime fixture with a conversation-owned
/// subagent registry, so the runtime-level durability sink is exercised
/// instead of only the registry's isolated publication policy.
#[allow(
    clippy::too_many_lines,
    reason = "one complete deterministic fixture boundary"
)]
pub(super) async fn headless_runtime_over_store_with_subagents(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    store: Arc<dyn ConversationStore>,
    admission_gate: Option<Arc<crate::runtime::conversation_runtime::Gate>>,
) -> (
    ConversationRuntime,
    Arc<FakeModel>,
    crate::runtime::subagent::SubagentRegistry,
) {
    let conversation_id = ConversationId::new(conversation_id);
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(crate::durable::ConversationStoreBinding::new(store)),
            ..crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            )
        },
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let subagents = crate::runtime::subagent::SubagentRegistry::new(
        crate::runtime::subagent::SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-a"),
            mailbox: tool_runtime.mailbox(),
            clock: Arc::new(crate::runtime::types::SystemClock),
            monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
            spawn: crate::runtime::subagent::SubagentSpawnPlan {
                session_id: crate::runtime::identity::SessionId::new(
                    "ses_01900000-0000-7000-8000-000000000001",
                ),

                program: std::path::PathBuf::from("/nonexistent/rustx"),
                product_root: crate::runtime::local_storage::ProductRoot::create(
                    &dir.path().join("subagents"),
                )
                .expect("product root"),
            },
            workspace: std::path::PathBuf::from(&workspace),
            max_active: 4,
        },
    );
    let model = Arc::new(FakeModel::new(Vec::new()));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(DefaultTokenEstimator),
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: Some(subagents.clone()),
        workflow_output: None,
    };
    let runtime = match admission_gate {
        Some(admission_gate) => ConversationRuntime::with_probe(
            config,
            CoordinatorProbe {
                admission_gate: Some(admission_gate),
                ..CoordinatorProbe::default()
            },
        )
        .expect("runtime"),
        None => ConversationRuntime::new(config).expect("runtime"),
    };
    (runtime, model, subagents)
}

/// A runtime construction config over one store, with a subagent
/// registry built over the same tool runtime and a deliberately
/// configurable registry identity (ownership-domain validation tests).
/// Returns the registry alongside the config so a test can commit
/// children into it before attempting construction.
#[allow(clippy::too_many_lines)]
pub(super) async fn subagent_runtime_config_with_registry(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    store: Arc<dyn ConversationStore>,
    registry_conversation: &ConversationId,
    registry_agent: &AgentId,
    runtime_agent: &AgentId,
) -> (
    crate::runtime::subagent::SubagentRegistry,
    RuntimeConversationConfig,
) {
    let conversation_id = ConversationId::new(conversation_id);
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(crate::durable::ConversationStoreBinding::new(store)),
            ..crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            )
        },
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let subagents = crate::runtime::subagent::SubagentRegistry::new(
        crate::runtime::subagent::SubagentRegistryConfig {
            conversation_id: registry_conversation.clone(),
            agent_id: registry_agent.clone(),
            mailbox: tool_runtime.mailbox(),
            clock: Arc::new(crate::runtime::types::SystemClock),
            monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
            spawn: crate::runtime::subagent::SubagentSpawnPlan {
                session_id: crate::runtime::identity::SessionId::new(
                    "ses_01900000-0000-7000-8000-000000000001",
                ),

                program: std::path::PathBuf::from("/nonexistent/rustx"),
                product_root: crate::runtime::local_storage::ProductRoot::create(
                    &dir.path().join("subagents"),
                )
                .expect("product root"),
            },
            workspace: std::path::PathBuf::from(&workspace),
            max_active: 4,
        },
    );
    let model = Arc::new(FakeModel::new(Vec::new()));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: runtime_agent.clone(),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(DefaultTokenEstimator),
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: Some(subagents.clone()),
        workflow_output: None,
    };
    (subagents, config)
}

/// Stages a stubborn real child for a registry test; the driver must
/// escalate to SIGKILL after the scripted terminal frame and then reap it.
pub(super) fn stage_runtime_test_child(
    runtime_root: &std::path::Path,
) -> (
    crate::runtime::subagent::process::StagedChild,
    tokio::net::UnixStream,
) {
    std::fs::create_dir_all(runtime_root).expect("runtime root");
    let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("control pair");
    let (observation_end, _observation_peer) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("trap '' TERM; exec sleep 60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .expect("staged child");
    (
        crate::runtime::subagent::process::StagedChild::for_test(
            child,
            driver_end,
            observation_end,
            runtime_root.to_path_buf(),
        ),
        test_end,
    )
}

/// A headless fixture whose runtime has the runtime-owned observation
/// bridge installed (no Runtime Client exists): the test folds
/// `ConversationObservation`s from the shared queue.
pub(super) async fn headless_fixture_with(probe: Option<CoordinatorProbe>) -> HeadlessFixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, model) = headless_runtime(&dir, vec![one_turn_script()], None, probe).await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("first bridge install");
    // The bridge installed over the inert runtime; activation is the
    // cut, exactly as in the interactive composition.
    runtime.activate();
    HeadlessFixture {
        _dir: dir,
        runtime,
        model,
        pending: Some(pending),
    }
}

pub(super) async fn headless_fixture() -> HeadlessFixture {
    headless_fixture_with(None).await
}

/// A runtime-source inbound message with runtime-assigned metadata.
pub(super) fn runtime_inbound_message(id: &str) -> crate::message::types::UserMessageBlock {
    crate::message::types::UserMessageBlock {
        id: crate::runtime::identity::MessageId::new(id),
        content: text_content("async"),
        source: UserSource::Runtime,
        kind: crate::message::types::InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("parse")
                .with_timezone(&chrono::Utc),
        ),
    }
}

pub(super) fn assert_recovered_manual_publication(
    host: &crate::runtime_client::RuntimeClientHost,
    subscription: &crate::runtime_client::EventSubscription,
    before: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    original: &crate::durable::projection::ProjectionCommitReceipt,
) {
    use crate::runtime_client::{EventDelivery, RuntimeClientEvent};
    let after = host.checkpoint().unwrap();
    let native = original.completed.observation.as_ref().unwrap();
    assert!(!native.source_unavailable);
    assert_eq!(after.read_cut.source_cut.as_ref(), Some(&native.read_cut));
    assert_eq!(after.read_cut.through, original.completed.sequence);
    assert!(after.cursor > before.cursor);
    let latest = after.snapshot.context.latest_compaction.as_ref().unwrap();
    assert_eq!(
        latest.checkpoint_id,
        original.checkpoint.candidate.checkpoint_id
    );
    assert_eq!(
        latest.activation_id,
        original.checkpoint.activation.activation
    );
    let head = after.snapshot.context.projection_head.as_ref().unwrap();
    assert_eq!(head.checkpoint_id, latest.checkpoint_id);
    assert_eq!(head.activation_id, latest.activation_id);
    let mut cursor = before.cursor;
    let mut completions = 0;
    loop {
        match subscription.try_next() {
            EventDelivery::Event(event) => {
                assert_eq!(event.cursor.get(), cursor.get() + 1);
                cursor = event.cursor;
                for fact in event.event.facts() {
                    if let RuntimeClientEvent::ContextCompacted { context, .. } = fact {
                        assert_eq!(context.latest_compaction.as_ref(), Some(latest));
                        completions += 1;
                    }
                }
            }
            EventDelivery::Pending => break,
            other => panic!("recovered native receipt cannot lose continuity: {other:?}"),
        }
    }
    assert_eq!(cursor, after.cursor);
    assert_eq!(completions, 1);
}

pub(super) fn recovered_manual_receipt(
    sqlite: &crate::durable::SqliteConversationStore,
    host: &crate::runtime_client::RuntimeClientHost,
) -> crate::durable::projection::ProjectionCommitReceipt {
    let completed = sqlite
        .read_events(None, 64)
        .unwrap()
        .events
        .into_iter()
        .filter(|event| {
            matches!(
                event.event,
                crate::events::types::RuntimeEvent::CompactionCompleted { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    let crate::events::types::RuntimeEvent::CompactionCompleted { operation_id, .. } =
        &completed[0].event
    else {
        unreachable!()
    };
    let checkpoint = sqlite.projection_head().unwrap().unwrap();
    let original = sqlite
        .projection_receipt(operation_id, &checkpoint.candidate.fingerprint())
        .unwrap()
        .unwrap();
    assert_eq!(original.completed.sequence, completed[0].sequence);
    assert!(original.completed.observation.is_some());
    assert!(
        host.checkpoint()
            .unwrap()
            .snapshot
            .context
            .latest_compaction
            .is_none()
    );
    original
}

/// Waits until an observation satisfying the predicate reaches the
/// queue; returns every observation drained up to and including it.
pub(super) async fn await_observation(
    pending: &PendingObservations,
    mut predicate: impl FnMut(&ConversationObservation) -> bool,
) -> Vec<ConversationObservation> {
    let mut folded = Vec::new();
    loop {
        while let Some(observation) = pending.next_fixture_fact() {
            let matched = predicate(&observation);
            folded.push(observation);
            if matched {
                return folded;
            }
        }
        pending.wait().await;
    }
}

/// Waits for the settlement handoff (the runtime-owned settlement
/// signal) and returns the authoritative ledger.
pub(super) async fn await_settled_ledger(
    runtime: &ConversationRuntime,
    after: u64,
) -> Vec<MessageBlock> {
    // Original terminal Journal commitment can precede this physical
    // coordinator handoff. Its retained native receipt prevents a lost
    // Notify edge when the execution settles before the assertion waits.
    runtime.child_attempt_settlement(after).await;
    runtime
        .coordinator_ledger()
        .expect("the settlement handoff restored the conversation state")
}

pub(super) fn project_file(
    path: &std::path::Path,
    content: &str,
) -> crate::runtime::ProjectContextFile {
    crate::runtime::ProjectContextFile {
        path: path.to_path_buf(),
        content: content.to_owned(),
    }
}

/// Whether an internal runtime event is one of the terminal settlement
/// events of an attempt.
pub(super) fn is_terminal_event(event: &crate::events::types::RuntimeEvent) -> bool {
    matches!(
        event,
        crate::events::types::RuntimeEvent::AttemptCompleted { .. }
            | crate::events::types::RuntimeEvent::AttemptCancelled { .. }
            | crate::events::types::RuntimeEvent::AttemptTimedOut { .. }
            | crate::events::types::RuntimeEvent::AttemptLimitExceeded { .. }
            | crate::events::types::RuntimeEvent::AttemptFailed { .. }
    )
}

pub(super) fn count_terminal(observations: &[ConversationObservation]) -> usize {
    observations
        .iter()
        .filter(|observation| {
            matches!(
                observation,
                ConversationObservation::Event { event, .. } if is_terminal_event(event)
            )
        })
        .count()
}

/// A gated background executor for the owning-runtime degradation
/// regression: signals its start, parks until released, then returns a
/// fixed success result.
pub(super) struct GatedBackgroundExecutor {
    pub(super) started: tokio::sync::watch::Sender<bool>,
    pub(super) release: tokio::sync::watch::Sender<bool>,
}

impl GatedBackgroundExecutor {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (release, _release_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                release: release.clone(),
            },
            started_rx,
            release,
        )
    }
}

impl crate::tools::executor::ToolExecutor for GatedBackgroundExecutor {
    fn start<'a>(
        &'a self,
        _invocation: crate::tools::types::ToolInvocation,
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
                crate::tools::types::ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: crate::tools::types::ToolExecutionStatus::Success,
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

/// Commits one conversation-owned background execution through the
/// authoritative registry of the runtime's tool runtime.
pub(super) fn commit_background(
    runtime: &ConversationRuntime,
    executor: &Arc<dyn crate::tools::executor::ToolExecutor>,
    call_id: &str,
) -> crate::runtime::identity::ToolExecutionId {
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new(call_id),
        },
        tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &invocation,
            executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        runtime
            .tool_runtime()
            .background()
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("commit")
    else {
        panic!("accepted");
    };
    execution_id
}

/// Drains `pending` until the runtime's latest published configuration-
/// adoption eligibility is `expected`. Consecutive publications must
/// differ: the runtime never republishes an unchanged value. The queue's
/// wake permit makes the drain/wait pair lossless, so nothing polls.
pub(super) async fn published_eligibility_until(
    pending: &PendingObservations,
    latest: &mut Option<crate::runtime::adoption::AdoptionEligibility>,
    expected: crate::runtime::adoption::AdoptionEligibility,
) {
    within_liveness_guard("published adoption eligibility", async {
        loop {
            for observation in pending.drain_fixture_facts() {
                if let ConversationObservation::AdoptionEligibility(eligibility) = observation {
                    assert_ne!(
                        latest.as_ref(),
                        Some(&eligibility),
                        "eligibility republished unchanged"
                    );
                    *latest = Some(eligibility);
                }
            }
            if latest.as_ref() == Some(&expected) {
                return;
            }
            pending.wait().await;
        }
    })
    .await;
}

// Shared native child invariant for ordinary Subagents and Workflow Agents.
// Parent admission is held solely to keep the fixture occurrence pending.
pub(super) fn assert_pending_mutation_preserves_child(
    runtime: &ConversationRuntime,
    subagents: &crate::runtime::subagent::SubagentRegistry,
    child_id: &crate::runtime::identity::SubagentId,
) {
    let _admission = runtime.inner.state.lock().expect("coordinator");
    let before = subagents.snapshot(child_id).unwrap();
    let item = runtime
        .tool_runtime()
        .durable_store()
        .accept_inbound(crate::durable::InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![crate::message::types::UserContentBlock::Text(TextBlock {
                text: "pending user control".into(),
            })],
            timestamp: chrono::Utc::now(),
            correlation: None,
        })
        .unwrap();
    let expected = crate::durable::inbox::PendingInboundRef {
        sequence: item.sequence,
        revision: 0,
    };
    assert!(matches!(
    runtime
                    .tool_runtime()
                    .mailbox()
                    .edit_pending(&expected, "edited pending")
                    .unwrap(),
    crate::durable::inbox::PendingMutationOutcome::Applied { read_cut, .. } if read_cut.pending_total <= read_cut.pending_through
    ));
    assert!(matches!(
    runtime
                    .tool_runtime()
                    .mailbox()
                    .remove_pending(&crate::durable::inbox::PendingInboundRef {
                        revision: 1,
                        ..expected
                    })
                    .unwrap(),
    crate::durable::inbox::PendingMutationOutcome::Applied { read_cut, .. } if read_cut.pending_total <= read_cut.pending_through
    ));
    assert_eq!(subagents.snapshot(child_id).unwrap(), before);
}

/// A foreground tool that starts, parks until cancellation is observable,
/// and then reads the cancellation **cause from its execution context's
/// authority** — exactly what a real cancellable executor does when it
/// normalizes its own terminal status.
pub(super) struct CauseProbeTool {
    pub(super) started: tokio::sync::watch::Sender<bool>,
    pub(super) observed: Arc<std::sync::Mutex<Option<CancellationReason>>>,
}

impl CauseProbeTool {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        Arc<std::sync::Mutex<Option<CancellationReason>>>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let observed = Arc::new(std::sync::Mutex::new(None));
        (
            Self {
                started,
                observed: observed.clone(),
            },
            started_rx,
            observed,
        )
    }
}

impl crate::tools::executor::ToolExecutor for CauseProbeTool {
    fn start<'a>(
        &'a self,
        _invocation: crate::tools::types::ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let started = self.started.clone();
        let observed = self.observed.clone();
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                // The context is built at tool start, before any cancellation
                // exists: a start-time copy of the cause could only ever be
                // the attempt's default.
                assert!(!context.cancellation.is_cancelled());
                started.send_replace(true);
                context.cancellation.cancelled().await;
                let reason = context.cancellation.reason();
                *observed.lock().expect("observed cause lock") = Some(reason);
                crate::tools::types::ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: crate::tools::types::ToolExecutionStatus::Cancelled {
                        reason,
                        phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// Builds the one-tool-call model script the parked foreground-probe
/// regressions (cancellation cause, deadline, settlement) drive.
pub(super) fn cause_probe_registry_and_script(
    tool: Arc<dyn ToolExecutor>,
) -> (crate::tools::executor::ToolRegistry, Vec<FakeStep>) {
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
    };
    let definition = ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-cause-probe"),
        name: "cause_probe".to_owned(),
        description: "park until cancellation and report the winning cause".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let mut registry = crate::tools::executor::ToolRegistry::new();
    registry
        .register(definition.clone(), tool)
        .expect("cause probe registration");
    let call_id = ToolCallId::new("call-cause-probe");
    let script = vec![
        FakeStep::Emit(crate::model::event::ModelEvent::Started),
        FakeStep::Emit(crate::model::event::ModelEvent::ToolCallStarted {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCallStart {
                id: call_id.clone(),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
            },
        }),
        FakeStep::Emit(crate::model::event::ModelEvent::ToolCallArgumentsDelta {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call_id: call_id.clone(),
            arguments_delta: "{}".to_owned(),
        }),
        FakeStep::Emit(crate::model::event::ModelEvent::ToolCallCompleted {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCall {
                id: call_id,
                tool_id: definition.id,
                name: definition.name,
                arguments: serde_json::json!({}),
            },
        }),
        FakeStep::Emit(crate::model::event::ModelEvent::Completed {
            finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ];
    (registry, script)
}

/// A foreground executor whose physical settlement evidence arrives
/// late: it signals `started`, observes the cancellation request,
/// signals `cancel_observed`, then parks on a settle gate before
/// returning its `Cancelled` result — exactly the Issue #204 case-M cut
/// where runtime drain must keep awaiting the executor's real terminal
/// evidence.
pub(super) struct SettleGateProbeTool {
    pub(super) started: tokio::sync::watch::Sender<bool>,
    pub(super) cancel_observed: tokio::sync::watch::Sender<bool>,
    pub(super) settle_gate: tokio::sync::watch::Sender<bool>,
}

impl SettleGateProbeTool {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
        let (settle_gate, _) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                cancel_observed,
                settle_gate: settle_gate.clone(),
            },
            started_rx,
            cancel_observed_rx,
            settle_gate,
        )
    }
}

impl ToolExecutor for SettleGateProbeTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let started = self.started.clone();
        let cancel_observed = self.cancel_observed.clone();
        let settle_gate = self.settle_gate.clone();
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                assert!(!context.cancellation.is_cancelled());
                started.send_replace(true);
                context.cancellation.cancelled().await;
                cancel_observed.send_replace(true);
                // The settlement gate models late physical settlement
                // evidence: the outcome is already decided, but the
                // settlement plane's drive of the operation does not
                // resolve until the executor's settlement is real.
                let mut released = settle_gate.subscribe();
                released
                    .wait_for(|is_released| *is_released)
                    .await
                    .expect("settle gate stays open");
                let reason = context.cancellation.reason();
                crate::tools::types::ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Cancelled {
                        reason,
                        phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// A foreground executor with a genuinely split handle (Issue #204
/// drain cut, confirmed path): the physical completion plane parks
/// forever, so only the independent cancellation/settlement control
/// plane can report. The settlement plane observes the cancellation
/// request, signals `cancel_observed`, then parks on the settle gate
/// before reporting `Confirmed` with its proven cancellation result —
/// the case where runtime drain must keep awaiting the settlement
/// authority even though the completion plane can never return.
pub(super) struct DetachedConfirmGateTool {
    pub(super) started: tokio::sync::watch::Sender<bool>,
    pub(super) cancel_observed: tokio::sync::watch::Sender<bool>,
    pub(super) settle_gate: tokio::sync::watch::Sender<bool>,
}

impl DetachedConfirmGateTool {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
        let (settle_gate, _) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                cancel_observed,
                settle_gate: settle_gate.clone(),
            },
            started_rx,
            cancel_observed_rx,
            settle_gate,
        )
    }
}

impl ToolExecutor for DetachedConfirmGateTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        self.started.send_replace(true);
        // The physical completion plane can never resolve: this
        // execution settles through its settlement authority or not at
        // all.
        let completion: BoxFuture<'a, crate::tools::types::ToolExecutionResult> =
            Box::pin(std::future::pending());
        let cancel_observed = self.cancel_observed.clone();
        let settle_gate = self.settle_gate.clone();
        let settlement: BoxFuture<'a, crate::tools::executor::ToolSettlement> =
            Box::pin(async move {
                context.cancellation.cancelled().await;
                cancel_observed.send_replace(true);
                // The executor's settlement evidence arrives only when
                // its physical cancellation path has genuinely run to
                // its end.
                let mut released = settle_gate.subscribe();
                released
                    .wait_for(|is_released| *is_released)
                    .await
                    .expect("settle gate stays open");
                let reason = context.cancellation.reason();
                crate::tools::executor::ToolSettlement::Confirmed(
                    crate::tools::types::ToolExecutionResult {
                        deliveries: Vec::new(),
                        status: ToolExecutionStatus::Cancelled {
                            reason,
                            phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                        },
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    },
                )
            });
        crate::tools::executor::ToolExecutionHandle::new(completion, settlement)
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// A foreground executor modelling the one VALID `Unconfirmed` case
/// (Issue #204 drain cut): a remote external effect with fully
/// reclaimed local ownership. `start` returns a completion plane that
/// parks forever; its settlement plane reports `Unconfirmed` promptly
/// once it observes the cancellation request. The fixture spawns no
/// task, thread, or process: when the settlement plane returns, every
/// rustX-owned local execution ownership of the call has already
/// settled, and only the remote external effect beyond rustX's
/// ownership domain remains uncertain — so runtime drain may complete.
pub(super) struct DetachedUnconfirmedTool {
    pub(super) started: tokio::sync::watch::Sender<bool>,
    pub(super) cancel_observed: tokio::sync::watch::Sender<bool>,
}

impl DetachedUnconfirmedTool {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Receiver<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                cancel_observed,
            },
            started_rx,
            cancel_observed_rx,
        )
    }
}

impl ToolExecutor for DetachedUnconfirmedTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        self.started.send_replace(true);
        // No local task or process exists outside the handle's futures:
        // the entire local ownership of this execution is the parked
        // completion shell and the settlement plane below.
        let completion: BoxFuture<'a, crate::tools::types::ToolExecutionResult> =
            Box::pin(std::future::pending());
        let cancel_observed = self.cancel_observed.clone();
        let settlement: BoxFuture<'a, crate::tools::executor::ToolSettlement> =
            Box::pin(async move {
                context.cancellation.cancelled().await;
                cancel_observed.send_replace(true);
                // Local rustX execution ownership fully settled; only
                // the remote effect's terminality is unprovable.
                crate::tools::executor::ToolSettlement::Unconfirmed {
                    detail: "the dispatched operation crossed the external-effect frontier; \
                             remote terminality cannot be proven"
                        .to_owned(),
                }
            });
        crate::tools::executor::ToolExecutionHandle::new(completion, settlement)
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// The execution-fact sequence of one call within one event list, in
/// journal order — the drain-cut counterpart of the scripted suite's
/// fact-sequence audit.
pub(super) fn drain_call_facts(events: &[RuntimeEvent], call_id: &ToolCallId) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } if tool_call_id == call_id => {
                Some("started")
            }
            RuntimeEvent::ToolExecutionProgress { tool_call_id, .. } if tool_call_id == call_id => {
                Some("progress")
            }
            RuntimeEvent::ToolExecutionDeadlineFired { tool_call_id, .. }
                if tool_call_id == call_id =>
            {
                Some("deadline")
            }
            RuntimeEvent::ToolExecutionCancellationRequested { tool_call_id, .. }
                if tool_call_id == call_id =>
            {
                Some("cancellation-requested")
            }
            RuntimeEvent::ToolExecutionSettlementObserved { tool_call_id, .. }
                if tool_call_id == call_id =>
            {
                Some("settlement")
            }
            RuntimeEvent::ToolExecutionSettlementControlFailed { tool_call_id, .. }
                if tool_call_id == call_id =>
            {
                Some("settlement-control-failed")
            }
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                if tool_call_id == call_id =>
            {
                Some("completed")
            }
            _ => None,
        })
        .collect()
}

/// A foreground executor that violates the settlement contract at the
/// runtime level (Issue #204 drain cut, control-plane failure): it
/// signals `started`, records the drain's cancellation request through
/// `cancel_observed`, and then never settles — its operation parks
/// forever, so the settlement plane driving it never returns. The
/// operation future holds a drop guard flipping `operation_dropped`:
/// the fixture keeps all local ownership inside the handle's futures
/// (the `settled_by_operation` contract), so when the settlement
/// control-plane guard expires and the lifecycle drops the handle, the
/// drop provably consumes the operation — the bounded ownership rule of
/// the guard path.
pub(super) struct GuardViolationTool {
    pub(super) started: tokio::sync::watch::Sender<bool>,
    pub(super) cancel_observed: tokio::sync::watch::Sender<bool>,
    pub(super) operation_dropped: Arc<std::sync::atomic::AtomicBool>,
}

impl GuardViolationTool {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Receiver<bool>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
        let operation_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        (
            Self {
                started,
                cancel_observed,
                operation_dropped: operation_dropped.clone(),
            },
            started_rx,
            cancel_observed_rx,
            operation_dropped,
        )
    }
}

impl ToolExecutor for GuardViolationTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        struct OperationDropGuard(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for OperationDropGuard {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        let started = self.started.clone();
        let cancel_observed = self.cancel_observed.clone();
        let drop_guard = OperationDropGuard(self.operation_dropped.clone());
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                // The operation future owns this guard: dropping the
                // operation drops the guard.
                let _drop_guard = drop_guard;
                started.send_replace(true);
                context.cancellation.cancelled().await;
                cancel_observed.send_replace(true);
                // The contract violation under test: the cancellation
                // request was observed, yet the operation — and with it
                // the settlement plane — never settles.
                std::future::pending::<()>().await;
                unreachable!("the contract-violating execution never settles")
            }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// Builds a conversation runtime over an existing artifacts directory
/// (whose `conversation.sqlite` may already be populated), returning the
/// construction result so recovery-gate tests can assert the typed error.
pub(super) async fn runtime_with_model_at(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    initial_messages: Vec<MessageBlock>,
    scripts: Vec<Vec<FakeStep>>,
) -> Result<(ConversationRuntime, Arc<FakeModel>), ConversationRuntimeError> {
    runtime_with_model_probe_at(dir, conversation_id, initial_messages, scripts, None).await
}

/// The probe-armed variant of [`runtime_with_model_at`] (Issue #12, M9b
/// deterministic race tests).
pub(super) async fn runtime_with_model_probe_at(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    initial_messages: Vec<MessageBlock>,
    scripts: Vec<Vec<FakeStep>>,
    probe: Option<CoordinatorProbe>,
) -> Result<(ConversationRuntime, Arc<FakeModel>), ConversationRuntimeError> {
    let conversation_id = ConversationId::new(conversation_id);
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(&workspace, &artifacts)
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
            base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: fixture_activation(&tool_runtime),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let model = Arc::new(FakeModel::new(scripts));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator,
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages,
        subagents: None,
        workflow_output: None,
    };
    match probe {
        Some(probe) => ConversationRuntime::with_probe(config, probe),
        None => ConversationRuntime::new(config),
    }
    .map(|runtime| (runtime, model))
}

/// The conversation-runtime-only variant of [`runtime_with_model_at`].
pub(super) async fn runtime_at(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    initial_messages: Vec<MessageBlock>,
    scripts: Vec<Vec<FakeStep>>,
) -> Result<ConversationRuntime, ConversationRuntimeError> {
    runtime_with_model_at(dir, conversation_id, initial_messages, scripts)
        .await
        .map(|(runtime, _)| runtime)
}

pub(super) fn fixed_time() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
        .expect("parse")
        .with_timezone(&chrono::Utc)
}

pub(super) fn assistant_tool_block(id: &str, call: &str) -> MessageBlock {
    MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
        id: crate::runtime::identity::MessageId::new(id),
        content: vec![crate::message::types::AssistantContentBlock::ToolCall(
            crate::tools::types::ToolCall {
                id: crate::runtime::identity::ToolCallId::new(call),
                tool_id: crate::runtime::identity::ToolId::new("tool-a"),
                name: "alpha".to_owned(),
                arguments: serde_json::json!({}),
            },
        )],
    })
}

pub(super) fn tool_result_block(call: &str) -> MessageBlock {
    MessageBlock::Tool(crate::message::types::ToolMessageBlock {
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            crate::runtime::identity::MessageId::new("assistant-tool"),
            crate::message::types::ContentBlockIndex::new(0),
        ),
        id: crate::runtime::identity::MessageId::new(format!("tool-{call}")),
        tool_call_id: crate::runtime::identity::ToolCallId::new(call),
        tool_id: crate::runtime::identity::ToolId::new("tool-a"),
        result: crate::tools::types::ToolExecutionResult {
            deliveries: Vec::new(),
            status: crate::tools::types::ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    })
}

pub(super) fn seed_user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(crate::message::types::UserMessageBlock {
        id: crate::runtime::identity::MessageId::new(id),
        content: text_content(text),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(fixed_time()),
    })
}

// -----------------------------------------------------------------
// Issue #12 (M9a) — the runtime half of durable startup recovery
//
// The durable-evidence and classification regressions live in
// `tests/issue12_recovery.rs`; these are the ones that need a real
// admission and a driven model turn. The crash boundary is the same
// everywhere: an exact committed durable prefix written through a store
// handle that is then dropped, followed by a fresh runtime over the same
// database.
// -----------------------------------------------------------------

/// Seeds an exact durable crash prefix at the path `runtime_at` uses, and
/// returns the durable database path so a test can reopen it afterwards.
pub(super) fn seed_crash_prefix(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    seed: &[MessageBlock],
    commit: impl FnOnce(&crate::durable::SqliteConversationStore, &ConversationId),
) -> std::path::PathBuf {
    let conversation_id = ConversationId::new(conversation_id);
    std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let store_path = artifacts.join("conversation.sqlite");
    {
        let store =
            crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                .expect("open");
        store.initialize(seed).expect("seed");
        commit(&store, &conversation_id);
    }
    store_path
}

pub(super) fn attempt_event(
    conversation_id: &ConversationId,
    event_id: &str,
    attempt_id: &AttemptId,
    event: crate::events::types::RuntimeEvent,
) -> crate::events::types::RuntimeEventEnvelope {
    crate::events::types::RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
        event_id: crate::runtime::identity::EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: Some(attempt_id.clone()),
        turn_id: None,
        timestamp: fixed_time(),
        event,
    }
}

pub(super) fn append_completed_request(
    store: &crate::durable::SqliteConversationStore,
    conversation_id: &ConversationId,
    attempt_id: &AttemptId,
    request_id: &crate::runtime::identity::RequestId,
) {
    let mut event = attempt_event(
        conversation_id,
        "request-completed",
        attempt_id,
        crate::events::types::RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: attempt_id.clone(),
                    turn: TurnId::new("0"),
                    retry_number: 0,
                },
            },
            request_id: request_id.clone(),
            finish_reason: crate::model::finish::ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    event.turn_id = Some(TurnId::new("0"));
    store.append_event(event).expect("request completed");
}
