//! Issue #138: subagent conformance for retry, timeout, cancellation, and
//! unresolved-output isolation.
//!
//! A rustX subagent child is a real `ConversationRuntime` with the ordinary
//! Agent Loop, so the Issue #134–#137 recovery semantics must apply inside
//! the child unchanged while the parent subagent plane (registry + process
//! driver) never learns about provider retries, deadlines, publication
//! audits, or carryover. These tests prove that boundary by wiring a **real
//! child runtime** (scripted model, manual monotonic clock) to a **real
//! parent registry/driver** over the **real control IPC** socket pair: the
//! child side runs the exact production `serve_child_delegation` loop, and
//! the parent side is the production `SubagentRegistry` settlement path.
//! Only the OS process is a scripted stand-in (kill/reap semantics),
//! exactly like the Issue #60 registry regressions.
//!
//! # Determinism
//!
//! Every semantic race is synchronized through the child's durable event
//! journal (a committed fact linearizes before every runtime action that
//! follows it), the scripted model's watch channels, and a manually
//! advanced monotonic clock:
//!
//! - "the child is sleeping in retry backoff" means: `ModelRetryScheduled`
//!   is durably committed (which linearizes before the backoff wait starts)
//!   AND the manual clock is never advanced to the captured deadline, so
//!   the backoff cannot complete on its own. Any later settlement is
//!   therefore proof that cancellation/drain — not the clock — resolved the
//!   wait.
//! - Wall-clock time appears only as an outer anti-hang liveness guard.

use super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::durable::ConversationStore;
use rustx::events::types::{RuntimeEvent, SubagentTerminalState};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::ModelTimeoutPolicy;
use rustx::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, RuntimeConversationConfig,
};
use rustx::runtime::identity::{AgentId, ConversationId, MessageId, ToolCallId};
use rustx::runtime::types::{CancellationReason, SystemClock};
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCancellationPhase, ToolExecutionStatus};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, await_started, fake_model, model_release,
    success_result, tool_call_events,
};

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::observation::PendingObservations;
use crate::runtime::subagent::process::StagedChild;
use crate::runtime::subagent::{
    ResolvedSubagentSpec, SubagentAccepted, SubagentName, SubagentRegistry, SubagentRegistryConfig,
    SubagentSpawnPlan, SubagentStartOutcome, SubagentStartSpec, SubagentState,
};

/// The outer liveness guard of one in-process interaction. No assertion
/// depends on it; it only contains a broken fixture.
const LIVENESS: Duration = Duration::from_secs(30);

/// The frozen timeout policy the parent plane hands to every child. The
/// values are deliberately distinctive (not the defaults) so a child that
/// ignores the inherited policy is observable.
const INHERITED_RESPONSE_START_MS: u64 = 1_000;
const INHERITED_STREAM_IDLE_MS: u64 = 500;

fn inherited_policy() -> ModelTimeoutPolicy {
    ModelTimeoutPolicy::new(
        Duration::from_millis(INHERITED_RESPONSE_START_MS),
        Duration::from_millis(INHERITED_STREAM_IDLE_MS),
    )
}

/// One transient provider failure, optionally with a provider retry hint.
/// A hint of `Some(0)` collapses the backoff wait (the captured deadline is
/// "now"), so retry-to-completion tests need no clock advancement while the
/// durable schedule commit still happens.
fn transient_failure(message: &str, retry_after_ms: Option<u64>) -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::RateLimit,
            message: message.to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms,
            provider_code: Some("rate_limit_error".to_owned()),
            context_overflow: None,
        },
    }
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn text(text: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(0),
        text: text.to_owned(),
    }
}

fn reasoning(text: &str) -> ModelEvent {
    ModelEvent::ReasoningDelta {
        block_index: ContentBlockIndex::new(0),
        text: text.to_owned(),
    }
}

fn completed() -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }
}

/// A successful one-shot answer script.
fn answer_script(answer: &str) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(answer)),
        FakeStep::Emit(completed()),
    ]
}

fn timestamped_user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: rustx::message::types::InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-28T00:00:00Z")
                .expect("fixed timestamp")
                .with_timezone(&chrono::Utc),
        ),
    })
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A real child `ConversationRuntime` over the scripted model with the
/// observation bridge installed and the runtime activated — exactly the
/// state the production child driver reaches before it would send `Ready`.
struct ChildFixture {
    runtime: ConversationRuntime,
    observations: Arc<PendingObservations>,
    model: Arc<FakeModel>,
    clock: Arc<ManualMonotonicClock>,
    store: Arc<dyn ConversationStore>,
}

/// Builds the child runtime with the inherited timeout policy, an explicit
/// base tool registry, and optional seeded history.
async fn child_fixture(
    dir: &tempfile::TempDir,
    conversation_id: &ConversationId,
    scripts: Vec<Vec<FakeStep>>,
    tools: ToolRegistry,
    initial_messages: Vec<MessageBlock>,
) -> ChildFixture {
    let workspace = dir.path().join("child-workspace");
    std::fs::create_dir_all(&workspace).expect("child workspace");
    let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        conversation_id.clone(),
        &workspace,
        dir.path().join("child-artifacts"),
    )
    .expect("child tool runtime");
    let capability = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(tools),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("child-environments"),
            python_store_roots: None,
        },
    )
    .expect("child capability coordinator");
    let candidate = capability.prepare_candidate().await.expect("candidate");
    capability.commit(candidate).expect("capability commit");
    let model = fake_model(scripts);
    let adapter: Arc<dyn rustx::model::ModelAdapter> = model.clone();
    let clock = Arc::new(ManualMonotonicClock::new());
    let runtime = ConversationRuntime::with_test_monotonic_clock(
        RuntimeConversationConfig {
            agent_id: AgentId::new("agent-child-138"),
            model: support::model::scripted_session_model(adapter),
            approval_mode: rustx::runtime::ApprovalMode::Policy,
            model_timeout_policy: inherited_policy(),
            context: ConversationContextConfig {
                policy: rustx::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(rustx::context::DefaultTokenEstimator),
                status_engine: rustx::context::AgentStatusEngine::default(),
            },
            tool_runtime: tool_runtime.clone(),
            capability: capability.clone(),
            resources: Arc::new(rustx::runtime::RuntimeResourceSnapshot::new(
                rustx::runtime::RuntimeResourceRevision::new(1),
                Vec::new(),
                None,
                rustx::context::ContextAssembly::new(),
                capability.current_snapshot(),
            )),
            resource_loader: Arc::new(rustx::runtime::FilesystemRuntimeResourceLoader::new(
                &workspace,
            )),
            clock: None,
            initial_messages,
            // A child has no subagent registry: recursion is absent by
            // construction.
            subagents: None,
        },
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
    )
    .expect("child runtime composition");
    let observations = Arc::new(PendingObservations::new());
    runtime
        .install_observation_bridge(Arc::clone(&observations))
        .expect("observation bridge over the inactive child");
    runtime.activate();
    let store = tool_runtime.durable_store();
    ChildFixture {
        runtime,
        observations,
        model,
        clock,
        store,
    }
}

/// The spawn plan of the test parent plane, carrying the frozen policy
/// exactly like the production composition root.
fn test_spawn_plan(runtime_root: &std::path::Path) -> SubagentSpawnPlan {
    SubagentSpawnPlan {
        program: std::path::PathBuf::from("/nonexistent/rustx"),
        runtime_root: runtime_root.to_path_buf(),
        // The frozen policy every child launch inherits (Issue #138).
        model_timeout_policy: inherited_policy(),
        agent_status: rustx::context::AgentStatusConfig::default(),
        context: rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
    }
}

/// A frozen named definition is the only semantic input accepted by the
/// registry. These deterministic in-process cases exercise the registry and
/// child IPC without reopening a mutable catalog; the cross-process case
/// below resolves the same name through the production resource-generation
/// path.
fn resolved_child_spec(agent: &str) -> ResolvedSubagentSpec {
    ResolvedSubagentSpec {
        agent: SubagentName::parse(agent).expect("canonical subagent name"),
        definition_digest: serde_json::from_value(serde_json::json!(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .expect("definition digest"),
        workspace_policy: rustx::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
        instructions: "Issue 138 conformance child".to_owned(),
        model: rustx::model::frozen::test_frozen_model_spec(
            serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
        ),
        tools: Vec::new(),
        skills: Vec::new(),
        project_instructions: Vec::new(),
        materialization:
            rustx::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
    }
}

/// The parent side: a real `SubagentRegistry` over an in-memory durable
/// authority, with the Issue #60 staged-child test seam for the process.
struct ParentPlane {
    registry: SubagentRegistry,
    store: Arc<dyn ConversationStore>,
    parent_agent_id: AgentId,
    runtime_root: std::path::PathBuf,
}

/// A standalone registry plane: no parent runtime adopts the terminal
/// inbound, so the parent's pending batch stays directly observable.
fn standalone_parent_plane(dir: &tempfile::TempDir, conversation: &str) -> ParentPlane {
    let runtime_root = dir.path().join("parent-runtime");
    std::fs::create_dir_all(&runtime_root).expect("parent runtime root");
    let conversation_id = ConversationId::new(conversation);
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("in-memory store"),
    );
    let mailbox = ConversationInboundMailbox::over_store(store.clone());
    let registry = SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id,
        agent_id: AgentId::new("agent-parent-138"),
        mailbox,
        clock: Arc::new(SystemClock),
        spawn: test_spawn_plan(&runtime_root),
        workspace: rustx::runtime::subagent::SubagentWorkspaceManager::new(
            dir.path().join("parent-workspace"),
            &runtime_root,
        ),
        max_active: 4,
    });
    ParentPlane {
        registry,
        store: store as Arc<dyn ConversationStore>,
        parent_agent_id: AgentId::new("agent-parent-138"),
        runtime_root,
    }
}

/// A full parent `ConversationRuntime` owning the registry: the terminal
/// inbound is adopted through the ordinary runtime path and answered by the
/// scripted parent model.
struct ParentRuntimePlane {
    plane: ParentPlane,
    runtime: ConversationRuntime,
    model: Arc<FakeModel>,
}

async fn parent_runtime_plane(
    dir: &tempfile::TempDir,
    conversation: &str,
    parent_scripts: Vec<Vec<FakeStep>>,
) -> ParentRuntimePlane {
    let conversation_id = ConversationId::new(conversation);
    let workspace = dir.path().join("parent-workspace");
    std::fs::create_dir_all(&workspace).expect("parent workspace");
    let runtime_root = dir.path().join("parent-runtime");
    std::fs::create_dir_all(&runtime_root).expect("parent runtime root");
    let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        conversation_id.clone(),
        &workspace,
        dir.path().join("parent-artifacts"),
    )
    .expect("parent tool runtime");
    let capability = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("parent-environments"),
            python_store_roots: None,
        },
    )
    .expect("parent capability coordinator");
    let candidate = capability.prepare_candidate().await.expect("candidate");
    capability.commit(candidate).expect("capability commit");
    let registry = SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id,
        agent_id: AgentId::new("agent-parent-138"),
        mailbox: tool_runtime.mailbox(),
        clock: Arc::new(SystemClock),
        spawn: test_spawn_plan(&runtime_root),
        workspace: rustx::runtime::subagent::SubagentWorkspaceManager::new(
            &workspace,
            &runtime_root,
        ),
        max_active: 4,
    });
    let model = fake_model(parent_scripts);
    let adapter: Arc<dyn rustx::model::ModelAdapter> = model.clone();
    let runtime = ConversationRuntime::new(RuntimeConversationConfig {
        agent_id: AgentId::new("agent-parent-138"),
        model: support::model::scripted_session_model(adapter),
        approval_mode: rustx::runtime::ApprovalMode::Policy,
        model_timeout_policy: inherited_policy(),
        context: ConversationContextConfig {
            policy: rustx::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(rustx::context::DefaultTokenEstimator),
            status_engine: rustx::context::AgentStatusEngine::default(),
        },
        tool_runtime: tool_runtime.clone(),
        capability: capability.clone(),
        resources: Arc::new(rustx::runtime::RuntimeResourceSnapshot::new(
            rustx::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            rustx::context::ContextAssembly::new(),
            capability.current_snapshot(),
        )),
        resource_loader: Arc::new(rustx::runtime::FilesystemRuntimeResourceLoader::new(
            &workspace,
        )),
        clock: None,
        initial_messages: Vec::new(),
        subagents: Some(registry.clone()),
    })
    .expect("parent runtime composition");
    runtime.activate();
    ParentRuntimePlane {
        plane: ParentPlane {
            registry,
            store: tool_runtime.durable_store(),
            parent_agent_id: AgentId::new("agent-parent-138"),
            runtime_root,
        },
        runtime,
        model,
    }
}

/// A child wired end to end: the parent registry drives one end of the real
/// control socket pair through the production driver, and the child end is
/// served by the production `serve_child_delegation` loop over a real child
/// runtime. The OS process is a scripted stand-in with real kill/reap
/// semantics.
struct WiredChild {
    accepted: SubagentAccepted,
    serve: tokio::task::JoinHandle<Result<(), crate::local_runtime::subagent_child::ChildExit>>,
    stop_serve: tokio::sync::oneshot::Sender<()>,
    /// The scripted stand-in process identity (crash tests signal it).
    pid: u32,
}

/// Stages and commits a wired child whose process exits immediately (kill
/// and reap of an exited process are both no-ops).
async fn launch_wired_child(plane: &ParentPlane, child: &ChildFixture, task: &str) -> WiredChild {
    launch_wired_child_with_shell(plane, child, task, "true").await
}

async fn launch_wired_child_with_shell(
    plane: &ParentPlane,
    child: &ChildFixture,
    task: &str,
    shell: &str,
) -> WiredChild {
    let (driver_end, child_end) = tokio::net::UnixStream::pair().expect("control pair");
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(shell)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let process = command.spawn().expect("scripted child process");
    let pid = process.id().expect("scripted child pid");
    let child_root = plane.runtime_root.join(format!("wired-child-{pid}"));
    std::fs::create_dir_all(&child_root).expect("child runtime root");
    plane
        .registry
        .push_staged_override(StagedChild::for_test(process, driver_end, child_root));
    let prepared = plane
        .registry
        .prepare(
            &SubagentStartSpec {
                resolved: resolved_child_spec("conformance"),
                task: task.to_owned(),
                context: None,
                tool_call_id: ToolCallId::new("call-138"),
            },
            &CancellationSignal::new(),
        )
        .await
        .expect("child prepared");
    let accepted = match plane
        .registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .expect("child commit")
    {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("no cancellation was requested"),
    };
    let parent_agent_id = plane.parent_agent_id.clone();
    let child_runtime = child.runtime.clone();
    let child_observations = Arc::clone(&child.observations);
    let (stop_serve, stop_receiver) = tokio::sync::oneshot::channel();
    let serve = tokio::spawn(async move {
        let mut dispatcher =
            crate::local_runtime::dispatcher::ChildControlDispatcher::start(child_end);
        let handle = dispatcher.handle();
        let result = tokio::select! {
            result = crate::local_runtime::subagent_child::serve_child_delegation(
                &mut dispatcher,
                &handle,
                parent_agent_id,
                child_runtime,
                child_observations,
            ) => result,
            _ = stop_receiver => Ok(()),
        };
        dispatcher.shutdown().await;
        result
    });
    WiredChild {
        accepted,
        serve,
        stop_serve,
        pid,
    }
}

// ---------------------------------------------------------------------------
// Journal helpers
// ---------------------------------------------------------------------------

/// Reads the whole durable event journal of one store.
fn journal(store: &Arc<dyn ConversationStore>) -> Vec<RuntimeEvent> {
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.read_events(cursor, 256).expect("event journal");
        if page.events.is_empty() {
            return all;
        }
        cursor = page.next_sequence;
        all.extend(page.events.into_iter().map(|envelope| envelope.event));
        if cursor.is_none() {
            return all;
        }
    }
}

/// Polls one store until at least `count` events match `predicate`.
///
/// Durable visibility is the synchronization point: a committed fact
/// linearizes before every runtime action that follows it, so observing it
/// is proof of the interleaving, never a timing guess.
async fn await_journal_fact(
    store: &Arc<dyn ConversationStore>,
    count: usize,
    predicate: impl Fn(&RuntimeEvent) -> bool,
    description: &str,
) -> Vec<RuntimeEvent> {
    tokio::time::timeout(LIVENESS, async {
        loop {
            let events = journal(store);
            if events.iter().filter(|event| predicate(event)).count() >= count {
                return events;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{description}: durable fact did not commit within liveness guard"))
}

fn count_events(events: &[RuntimeEvent], predicate: impl Fn(&RuntimeEvent) -> bool) -> usize {
    events.iter().filter(|event| predicate(event)).count()
}

fn is_request_started(event: &RuntimeEvent) -> bool {
    matches!(event, RuntimeEvent::ModelRequestStarted { .. })
}

fn is_retry_scheduled(event: &RuntimeEvent) -> bool {
    matches!(event, RuntimeEvent::ModelRetryScheduled { .. })
}

fn terminal_publications(events: &[RuntimeEvent]) -> Vec<SubagentTerminalState> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::SubagentTerminalPublished { state, .. } => Some(*state),
            _ => None,
        })
        .collect()
}

/// The text blocks of the parent's pending inbound batch, if one exists.
fn parent_pending_texts(plane: &ParentPlane) -> Vec<String> {
    plane
        .store
        .select_pending_batch()
        .expect("pending batch")
        .map(|batch| {
            batch
                .items
                .iter()
                .flat_map(|item| {
                    item.message.content.iter().filter_map(|block| match block {
                        UserContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The canonical messages at the child store's durable head.
fn child_canonical_messages(child: &ChildFixture) -> Vec<MessageBlock> {
    let head = child.store.load_head().expect("child head");
    child
        .store
        .load_messages(&head.active_message_ids)
        .expect("child messages")
}

/// Awaits the wired child's serve task and asserts a clean exit.
async fn await_serve(
    serve: tokio::task::JoinHandle<Result<(), crate::local_runtime::subagent_child::ChildExit>>,
) {
    tokio::time::timeout(LIVENESS, serve)
        .await
        .expect("child serve liveness")
        .expect("child serve task")
        .expect("child serve loop");
}

/// Drives a wired child into "sleeping in retry backoff" and returns the
/// proof: `ModelRetryScheduled` is durably committed, and the manual clock
/// is never advanced to the captured deadline, so the backoff cannot
/// complete unless cancellation or drain resolves it.
async fn park_child_in_retry_backoff(
    child: &ChildFixture,
    plane: &ParentPlane,
    task: &str,
) -> WiredChild {
    let wired = launch_wired_child(plane, child, task).await;
    await_journal_fact(
        &child.store,
        1,
        is_retry_scheduled,
        "the transient retry schedule commits before the backoff wait",
    )
    .await;
    wired
}

// ---------------------------------------------------------------------------
// Launch plumbing (Invariant 2): the frozen policy reaches the child spec
// ---------------------------------------------------------------------------

/// The spawn plan's frozen policy is copied into the typed child startup
/// specification unchanged — the one launch-plumbing boundary of the
/// inherited deadline contract. The end-to-end application of that policy
/// inside a real child process is covered by the `tests/issue138_*`
/// integration binary.
#[tokio::test]
async fn the_child_spec_carries_the_frozen_timeout_policy() {
    let dir = tempfile::tempdir().expect("temp root");
    let plan = test_spawn_plan(&dir.path().join("runtime"));
    let subagent_id = rustx::runtime::identity::SubagentId::new("conv-x-subagent-1");
    let physical_root = plan
        .allocate_child_runtime_root(&subagent_id)
        .expect("physical child root");
    let workspace_path = dir.path().join("parent-workspace");
    std::fs::create_dir_all(&workspace_path).expect("parent workspace");
    let workspace = rustx::runtime::subagent::SubagentWorkspaceManager::new(
        &workspace_path,
        &plan.runtime_root,
    )
    .acquire(
        rustx::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
        &subagent_id,
        &CancellationSignal::new(),
    )
    .await
    .expect("shared workspace lease");
    let spec = plan.child_spec(
        &subagent_id,
        &ConversationId::new("conv-x-subagent-1"),
        &AgentId::new("agent-child"),
        &AgentId::new("agent-parent"),
        &resolved_child_spec("conformance"),
        &physical_root,
        &workspace,
    );
    assert_eq!(spec.model_timeout_policy, inherited_policy());
    assert_ne!(
        spec.model_timeout_policy,
        ModelTimeoutPolicy::default(),
        "the test policy is distinctive: a default-policy regression is observable"
    );
}

// ---------------------------------------------------------------------------
// Invariants 1/14/15: retry stays child-local; one parent terminal
// ---------------------------------------------------------------------------

/// A child logical model step R0 (transient), R1 (transient), R2 (success)
/// produces exactly one parent-facing success. The parent journal contains
/// the ownership fact and one `SubagentTerminalPublished` — never a request
/// start, a retry schedule, or any retry ordinal/delay; the lifecycle
/// projection goes straight from Running to Succeeded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_transient_retries_settle_one_parent_success() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-retry-success");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-retry-success-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", Some(0))),
            ],
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R1 boom", Some(0))),
            ],
            answer_script("CHILD-FINAL-ANSWER"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect the workspace").await;

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(settled.detail.as_deref(), Some("CHILD-FINAL-ANSWER"));
    await_serve(wired.serve).await;

    // The child really retried internally: three provider requests, one
    // durable schedule per transient failure.
    assert_eq!(child.model.requests().len(), 3, "R0, R1, R2 all ran");
    let child_events = journal(&child.store);
    assert_eq!(count_events(&child_events, is_request_started), 3);
    assert_eq!(count_events(&child_events, is_retry_scheduled), 2);

    // The parent saw none of it: no request/retry facts, exactly one
    // terminal publication, exactly one pending inbound authored by the
    // child agent.
    let parent_events = journal(&plane.store);
    assert_eq!(count_events(&parent_events, is_request_started), 0);
    assert_eq!(count_events(&parent_events, is_retry_scheduled), 0);
    assert_eq!(
        terminal_publications(&parent_events),
        vec![SubagentTerminalState::Succeeded],
        "any number of internal retries still publishes exactly one terminal"
    );
    let texts = parent_pending_texts(&plane);
    assert_eq!(texts, vec!["CHILD-FINAL-ANSWER".to_owned()]);
    let batch = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one batch");
    assert!(matches!(
        batch.items[0].message.source,
        UserSource::Agent { ref agent_id } if *agent_id == wired.accepted.child_agent_id
    ));
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariants 2/7: failed-attempt publication stays child-local
// ---------------------------------------------------------------------------

/// R0 emits partial text and then fails transiently; R1 succeeds. The
/// failed attempt's partial publication never becomes the child answer and
/// never crosses into the parent: the parent's one success inbound carries
/// only the final answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_attempt_publication_stays_child_local() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-partial-retry");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-partial-retry-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(text("PARTIAL-LEAK")),
                FakeStep::Emit(transient_failure("mid-stream loss", Some(0))),
            ],
            answer_script("FINAL-ANSWER"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(settled.detail.as_deref(), Some("FINAL-ANSWER"));
    await_serve(wired.serve).await;

    // The failed attempt's partial text never became a canonical child
    // assistant message: the recovered-generation publication audit stays
    // child-local residue and is not installed as carryover after an
    // internal retry success (Issue #137).
    let child_texts: Vec<String> = child_canonical_messages(&child)
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(
                assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        rustx::message::types::AssistantContentBlock::Text(text) => {
                            Some(text.text.clone())
                        }
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        child_texts
            .iter()
            .all(|text| !text.contains("PARTIAL-LEAK")),
        "the failed attempt's publication never committed: {child_texts:?}"
    );
    assert!(
        child
            .store
            .load_pending_unresolved_output_stream_id()
            .expect("child carryover pointer")
            .is_none(),
        "an internal retry success installs no carryover"
    );

    let texts = parent_pending_texts(&plane);
    assert_eq!(texts, vec!["FINAL-ANSWER".to_owned()]);
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariants 3/4/6/7: exhausted retry budget -> one bounded Failed, no
// partial output, no carryover projection
// ---------------------------------------------------------------------------

/// Every attempt emits partial text, partial reasoning, and an incomplete
/// tool-call proposal, then fails transiently until the budget is spent.
/// The parent receives exactly one bounded Runtime-authored failure notice
/// containing none of the child publication content, while the child keeps
/// its own unresolved-output carryover pointer as durable local residue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_exhaustion_publishes_one_bounded_failure_without_child_content() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-exhaustion");
    let failing_script = || {
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(reasoning("CHILD-SECRET-REASONING")),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(1),
                text: "CHILD-PARTIAL-SECRET".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(2),
                call: rustx::tools::types::ToolCallStart {
                    id: ToolCallId::new("call-secret"),
                    tool_id: rustx::runtime::identity::ToolId::new("tool-secret"),
                    name: "secret".to_owned(),
                },
            }),
            FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(2),
                call_id: ToolCallId::new("call-secret"),
                arguments_delta: "{\"secret\":\"CHILD-SECRET-ARGS\"}".to_owned(),
            }),
            // The proposal never completes: the stream fails transiently
            // with an uncommitted partial generation.
            FakeStep::Emit(transient_failure("budget food", Some(0))),
        ]
    };
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-exhaustion-child"),
        // The transient budget is three retries per logical step: four
        // failures exhaust it and the attempt settles Failed.
        vec![
            failing_script(),
            failing_script(),
            failing_script(),
            failing_script(),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Failed);
    await_serve(wired.serve).await;

    // The child really exhausted its ordinary budget: four actual provider
    // requests, three durable schedules.
    assert_eq!(child.model.requests().len(), 4);
    let child_events = journal(&child.store);
    assert_eq!(count_events(&child_events, is_request_started), 4);
    assert_eq!(count_events(&child_events, is_retry_scheduled), 3);
    // A retry-exhausted child durably retains its pending carryover
    // pointer; that is child-local residue and must not be special-cased
    // away (the pointer's body authority is the child Publication Audit).
    assert!(
        child
            .store
            .load_pending_unresolved_output_stream_id()
            .expect("child carryover pointer")
            .is_some(),
        "the retry-exhausted child keeps its pending carryover pointer"
    );

    // The parent-facing settlement: exactly one Failed publication, one
    // Runtime-authored notice, bounded diagnostic, and none of the failed
    // attempts' partial text, reasoning, proposed tool-call arguments, or
    // any carryover projection.
    let parent_events = journal(&plane.store);
    assert_eq!(count_events(&parent_events, is_request_started), 0);
    assert_eq!(count_events(&parent_events, is_retry_scheduled), 0);
    assert_eq!(
        terminal_publications(&parent_events),
        vec![SubagentTerminalState::Failed]
    );
    assert!(
        plane
            .store
            .load_pending_unresolved_output_stream_id()
            .expect("parent carryover pointer")
            .is_none(),
        "child carryover never crosses the conversation boundary"
    );
    let texts = parent_pending_texts(&plane);
    assert_eq!(texts.len(), 1, "exactly one parent-facing notice");
    let notice = &texts[0];
    assert!(
        notice.contains("failed"),
        "bounded failure notice: {notice}"
    );
    for secret in [
        "CHILD-PARTIAL-SECRET",
        "CHILD-SECRET-REASONING",
        "CHILD-SECRET-ARGS",
    ] {
        assert!(
            !notice.contains(secret),
            "failed-attempt content is structurally unavailable to the parent: {notice}"
        );
        assert!(
            !settled
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains(secret),
            "the registry detail carries no child publication content"
        );
    }
    let batch = plane
        .store
        .select_pending_batch()
        .expect("pending")
        .expect("one batch");
    assert!(matches!(batch.items[0].message.source, UserSource::Runtime));
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 6 (model-visible form): child carryover never enters a parent
// model request
// ---------------------------------------------------------------------------

/// A full parent runtime adopts the failed child's Runtime notice and runs
/// its continuation turn. No parent model request contains the child's
/// partial output, and the parent conversation never gains a pending
/// carryover pointer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_carryover_never_enters_parent_conversation_or_requests() {
    let dir = tempfile::tempdir().expect("temp root");
    let parent = parent_runtime_plane(
        &dir,
        "conv-138-carryover-parent",
        vec![answer_script("PARENT-FINAL-TURN")],
    )
    .await;
    let failing_script = || {
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(reasoning("CHILD-SECRET-REASONING")),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(1),
                text: "CHILD-PARTIAL-SECRET".to_owned(),
            }),
            FakeStep::Emit(transient_failure("budget food", Some(0))),
        ]
    };
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-carryover-child"),
        vec![
            failing_script(),
            failing_script(),
            failing_script(),
            failing_script(),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "inspect").await;

    let settled = parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Failed);
    await_serve(wired.serve).await;
    assert_eq!(
        child.model.requests().len(),
        4,
        "the child exhausted its ordinary transient budget"
    );

    // The parent runtime adopted the notice and completed its continuation
    // turn through its own ordinary Agent Loop.
    await_journal_fact(
        &parent.plane.store,
        1,
        |event| matches!(event, RuntimeEvent::AttemptCompleted { .. }),
        "the parent consumes the terminal notice in an ordinary turn",
    )
    .await;

    // The child kept its carryover residue; the parent conversation never
    // gained one.
    assert!(
        child
            .store
            .load_pending_unresolved_output_stream_id()
            .expect("child carryover pointer")
            .is_some()
    );
    assert!(
        parent
            .plane
            .store
            .load_pending_unresolved_output_stream_id()
            .expect("parent carryover pointer")
            .is_none()
    );

    // Every model request the parent ever issued is free of the child's
    // partial output and of any carryover rendering of it.
    let requests = parent.model.requests();
    assert!(
        !requests.is_empty(),
        "the parent really called its model for the continuation"
    );
    for request in &requests {
        let serialized = serde_json::to_string(request).expect("request json");
        for secret in ["CHILD-PARTIAL-SECRET", "CHILD-SECRET-REASONING"] {
            assert!(
                !serialized.contains(secret),
                "child unresolved output is not model-visible in the parent: {serialized}"
            );
        }
    }
    assert_eq!(
        terminal_publications(&journal(&parent.plane.store)),
        vec![SubagentTerminalState::Failed]
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
    parent
        .runtime
        .shutdown()
        .await
        .expect("parent runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 3: parent cancellation during retry backoff
// ---------------------------------------------------------------------------

/// The child is provably sleeping in retry backoff (the schedule is durably
/// committed and the manual clock is never advanced). The parent commits
/// cancellation through the ordinary `SubagentRegistry::cancel` path; the
/// driver forwards the explicit `Cancel` control frame; the child's
/// one-shot cancellation intent wakes the backoff; and no next
/// `ModelRequestStarted` can ever commit. The child settles
/// `AttemptCancelled`, reports one `Cancelled` candidate, and the parent
/// publishes exactly one `Cancelled` terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_cancellation_during_retry_backoff_cancels_the_child() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-cancel-backoff");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-cancel-backoff-child"),
        vec![
            // No provider hint: the backoff deadline is 2000ms ahead on the
            // manual clock, which the test never advances.
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", None)),
            ],
            answer_script("NEVER-REACHED"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = park_child_in_retry_backoff(&child, &plane, "inspect").await;

    let cancelling = plane
        .registry
        .cancel(
            &wired.accepted.subagent_id,
            CancellationReason::UserRequested,
        )
        .expect("known child");
    assert_eq!(cancelling.state, SubagentState::Cancelling);

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert!(
        settled
            .detail
            .as_deref()
            .expect("cancellation detail")
            .contains("requested by the user"),
        "the user-requested cause is preserved: {:?}",
        settled.detail
    );
    await_serve(wired.serve).await;

    // The backoff lost the cancellation race: exactly one provider request
    // and exactly one durable start exist, and no second request can have
    // started — the clock was never advanced, so only cancellation could
    // have ended the wait, and the settlement is the proof that it did.
    assert_eq!(child.model.requests().len(), 1);
    let child_events = journal(&child.store);
    assert_eq!(count_events(&child_events, is_request_started), 1);
    assert_eq!(count_events(&child_events, is_retry_scheduled), 1);
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::UserRequested,
                ..
            }
        )),
        1,
        "the child settles AttemptCancelled through its ordinary authority"
    );
    assert_eq!(
        terminal_publications(&journal(&plane.store)),
        vec![SubagentTerminalState::Cancelled]
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 4/16: runtime drain during retry backoff
// ---------------------------------------------------------------------------

/// The parent runtime drains while its child is provably sleeping in retry
/// backoff. Drain commits `RuntimeShutdown` cancellation intent, forwards
/// the ordinary `Cancel` frame, and still reaches quiescence: the child's
/// backoff loses the cancellation race, no second provider request starts,
/// and the one parent-facing terminal preserves the shutdown provenance
/// ("the runtime is shutting down"), never rewritten to a user request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_runtime_drain_during_child_retry_backoff_reaches_quiescence() {
    let dir = tempfile::tempdir().expect("temp root");
    let parent = parent_runtime_plane(&dir, "conv-138-drain-parent", Vec::new()).await;
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-drain-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", None)),
            ],
            answer_script("NEVER-REACHED"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = park_child_in_retry_backoff(&child, &parent.plane, "inspect").await;

    // Runtime drain wins while the child sleeps in backoff. This future
    // completing at all is the quiescence proof: the drain supervisor
    // awaits the child's settlement, and only the propagated cancellation
    // can resolve the parked backoff (the manual clock never advances).
    tokio::time::timeout(LIVENESS, parent.runtime.shutdown())
        .await
        .expect("drain reaches quiescence while the child sleeps in backoff")
        .expect("drain succeeds");

    let settled = parent
        .plane
        .registry
        .snapshot(&wired.accepted.subagent_id)
        .expect("child record");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert!(
        settled
            .detail
            .as_deref()
            .expect("cancellation detail")
            .contains("the runtime is shutting down"),
        "RuntimeShutdown provenance is preserved end to end: {:?}",
        settled.detail
    );
    await_serve(wired.serve).await;

    assert_eq!(child.model.requests().len(), 1, "no next provider request");
    let child_events = journal(&child.store);
    assert_eq!(count_events(&child_events, is_request_started), 1);
    // The child's own attempt settlement is the ordinary Cancel-frame
    // authority (the parent/child cancellation authorities stay separate),
    // and the exact reason committed by the parent registry crosses the
    // control boundary unchanged.
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::AttemptCancelled { .. }
        )),
        1
    );
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            }
        )),
        1,
        "the child's durable cancellation provenance is RuntimeShutdown"
    );
    assert_eq!(
        terminal_publications(&journal(&parent.plane.store)),
        vec![SubagentTerminalState::Cancelled],
        "drain publishes exactly one terminal"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// The child's own runtime drain while its attempt sleeps in retry backoff
/// settles the attempt through the generic drain authority: the cause stays
/// `RuntimeShutdown` and is never rewritten merely because the cancellation
/// machinery woke the backoff wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_runtime_drain_during_retry_backoff_preserves_runtime_shutdown() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-child-drain");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-child-drain-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", None)),
            ],
            answer_script("NEVER-REACHED"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = park_child_in_retry_backoff(&child, &plane, "inspect").await;

    // The child runtime itself drains (this is exactly what the production
    // child does when the parent disappears: control EOF -> drain -> exit).
    tokio::time::timeout(LIVENESS, child.runtime.shutdown())
        .await
        .expect("the child runtime drains its own parked backoff")
        .expect("child drain succeeds");
    await_serve(wired.serve).await;

    let child_events = journal(&child.store);
    assert_eq!(child.model.requests().len(), 1, "no next provider request");
    assert_eq!(count_events(&child_events, is_request_started), 1);
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            }
        )),
        1,
        "the child attempt settlement preserves RuntimeShutdown: {child_events:?}"
    );
    // The parent plane settles the child through the ordinary terminal
    // contract: the child reported its terminal candidate before exiting.
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        terminal_publications(&journal(&plane.store)),
        vec![SubagentTerminalState::Cancelled]
    );
}

// ---------------------------------------------------------------------------
// Invariant 5: tool cancellation phases are unchanged in children
// ---------------------------------------------------------------------------

/// A pre-tool policy gate that parks the attempt inside the policy
/// evaluation — after the canonical tool call, before the executor start
/// frontier — until the test releases it.
struct GatedPolicy {
    entered: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Receiver<bool>,
}

impl rustx::agent::PreToolPolicy for GatedPolicy {
    fn evaluate<'a>(
        &'a self,
        _view: &'a rustx::agent::PreToolView<'a>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<rustx::agent::PreToolDecision, rustx::agent::LifecycleError>,
    > {
        let entered = self.entered.clone();
        let mut release = self.release.clone();
        Box::pin(async move {
            entered.send_replace(true);
            release
                .wait_for(|released| *released)
                .await
                .expect("pre-tool release channel stays open");
            Ok(rustx::agent::PreToolDecision::Allow)
        })
    }
}

/// A pre-tool policy gate parks the child's attempt after the canonical
/// tool call but before the executor start frontier. The parent's ordinary
/// cancellation lands inside that window; the call settles
/// `Cancelled { phase: BeforeStart }` with zero executor invocations and no
/// start/completion facts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_tool_cancelled_before_executor_start_records_before_start() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-tool-before");
    let call = ScriptedCall {
        id: "call-before",
        tool_id: "tool-before",
        name: "before",
        arguments: serde_json::json!({}),
    };
    let mut first = vec![FakeStep::Emit(started())];
    first.extend(tool_call_events(0, &call).into_iter().map(FakeStep::Emit));
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let tool = FakeTool::new(
        common::tool("before", "tool-before"),
        success_result("must not run"),
    );
    let calls = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-tool-before-child"),
        vec![first, answer_script("NEVER-REACHED")],
        tools,
        Vec::new(),
    )
    .await;

    // The deterministic pre-start window: the attempt parks inside the
    // pre-tool policy, after the tool call committed and before any
    // executor start.
    let (entered, mut entered_rx) = tokio::sync::watch::channel(false);
    let (release, release_rx) = tokio::sync::watch::channel(false);
    child
        .runtime
        .install_test_pre_tool_policy(Arc::new(GatedPolicy {
            entered,
            release: release_rx,
        }));

    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // The attempt is provably inside the pre-tool gate: the tool call is
    // canonical, the executor has not started. Cancellation commits
    // through the child's ordinary cancellation authority — the exact
    // entry point the `Cancel` control frame uses — synchronously, before
    // the gate is released. (The parent->frame->child propagation ordering
    // itself is proven by the retry-backoff cancellation test; this test
    // isolates the child-owned phase frontier.)
    tokio::time::timeout(LIVENESS, entered_rx.wait_for(|entered| *entered))
        .await
        .expect("pre-tool gate entered")
        .expect("pre-tool entered channel stays open");
    child
        .runtime
        .cancel_current_or_next_attempt(CancellationReason::UserRequested)
        .expect("the current attempt is cancelled while parked pre-start");
    release.send_replace(true);

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Cancelled);
    await_serve(wired.serve).await;

    // The child-owned phase fact: the canonical tool result committed as
    // Cancelled/BeforeStart; the executor never ran.
    assert!(
        calls.borrow().is_empty(),
        "executor invocation count is zero"
    );
    let tool_messages: Vec<_> = child_canonical_messages(&child)
        .into_iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(tool_messages.len(), 1, "one canonical result slot exists");
    assert!(
        matches!(
            tool_messages[0].result.status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::UserRequested,
                phase: ToolCancellationPhase::BeforeStart,
            }
        ),
        "the child records BeforeStart: {:?}",
        tool_messages[0].result.status
    );
    let child_events = journal(&child.store);
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. }
        )),
        0,
        "no start fact exists before the executor frontier"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// The child's tool executor is provably running (its start watch fired)
/// when the parent's ordinary cancellation arrives. The call settles
/// `Cancelled { phase: DuringExecution }` with exactly one executor
/// invocation; no rollback is claimed and no child-specific result shape
/// exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_tool_cancelled_after_executor_start_records_during_execution() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-tool-during");
    let call = ScriptedCall {
        id: "call-during",
        tool_id: "tool-during",
        name: "during",
        arguments: serde_json::json!({}),
    };
    let mut first = vec![FakeStep::Emit(started())];
    first.extend(tool_call_events(0, &call).into_iter().map(FakeStep::Emit));
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let (tool, _release) = FakeTool::parking(
        common::tool("during", "tool-during"),
        success_result("not reached"),
    );
    let calls = tool.calls();
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-tool-during-child"),
        vec![first, answer_script("NEVER-REACHED")],
        tools,
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // The executor start frontier was crossed before cancellation.
    await_started(&mut tool_started, "child tool").await;
    let cancelling = plane
        .registry
        .cancel(
            &wired.accepted.subagent_id,
            CancellationReason::UserRequested,
        )
        .expect("known child");
    assert_eq!(cancelling.state, SubagentState::Cancelling);

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Cancelled);
    await_serve(wired.serve).await;

    assert_eq!(calls.borrow().len(), 1, "executor invocation count is one");
    let tool_messages: Vec<_> = child_canonical_messages(&child)
        .into_iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(tool_messages.len(), 1);
    assert!(
        matches!(
            tool_messages[0].result.status,
            ToolExecutionStatus::Cancelled {
                phase: ToolCancellationPhase::DuringExecution,
                ..
            }
        ),
        "the child records DuringExecution: {:?}",
        tool_messages[0].result.status
    );
    let child_events = journal(&child.store);
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. }
        )),
        1
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 2 (application): inherited deadlines drive ordinary retry
// ---------------------------------------------------------------------------

/// The child's response-start deadline (from the inherited frozen policy)
/// fires while the provider never answers. The timeout enters the ordinary
/// generic retry path — one durable `ModelRequestFailed { Timeout }`, one
/// `ModelRetryScheduled` — and the retry succeeds. The parent receives
/// exactly one success and never observes a deadline or a retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_response_start_timeout_enters_generic_retry() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-response-start");
    let (_release, release_rx) = model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-response-start-child"),
        vec![
            // The provider connects and then never produces an event.
            vec![FakeStep::ParkUntilReleased(release_rx)],
            answer_script("ANSWER-AFTER-TIMEOUT"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // The request was dispatched and is awaiting its first event.
    let mut streams_started = child.model.streams_started();
    tokio::time::timeout(LIVENESS, streams_started.wait_for(|count| *count >= 1))
        .await
        .expect("the child request starts")
        .expect("stream watch stays open");
    // The inherited response-start deadline fires on the manual clock.
    child.clock.advance(INHERITED_RESPONSE_START_MS);
    // The timeout is transient under Issue #134: the schedule commit is the
    // synchronization point for the backoff wait.
    await_journal_fact(
        &child.store,
        1,
        is_retry_scheduled,
        "the response-start timeout enters ordinary generic retry",
    )
    .await;
    // Release the backoff: the captured deadline is 2000ms ahead.
    child.clock.advance(2_000);

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(settled.detail.as_deref(), Some("ANSWER-AFTER-TIMEOUT"));
    await_serve(wired.serve).await;

    let child_events = journal(&child.store);
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::ModelRequestFailed { error, .. }
                if error.kind == ModelErrorKind::Timeout
        )),
        1,
        "one response-start timeout outcome"
    );
    assert_eq!(count_events(&child_events, is_retry_scheduled), 1);
    assert_eq!(child.model.requests().len(), 2, "the retry really ran");
    let parent_events = journal(&plane.store);
    assert_eq!(count_events(&parent_events, is_request_started), 0);
    assert_eq!(count_events(&parent_events, is_retry_scheduled), 0);
    assert_eq!(
        terminal_publications(&parent_events),
        vec![SubagentTerminalState::Succeeded]
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// The child's stream-idle deadline fires after generation began. The
/// partial text from the timed-out stream stays child-local; the ordinary
/// retry succeeds and the parent receives only the final answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_stream_idle_timeout_enters_generic_retry() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-stream-idle");
    let (_release, release_rx) = model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-stream-idle-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(text("IDLE-PARTIAL")),
                FakeStep::ParkUntilReleased(release_rx),
            ],
            answer_script("ANSWER-AFTER-IDLE"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // Generation began: the stream-idle deadline now owns the request.
    let mut parked = child.model.parked();
    tokio::time::timeout(LIVENESS, parked.wait_for(|parked| *parked))
        .await
        .expect("the child stream idles")
        .expect("parked watch stays open");
    child.clock.advance(INHERITED_STREAM_IDLE_MS);
    await_journal_fact(
        &child.store,
        1,
        is_retry_scheduled,
        "the stream-idle timeout enters ordinary generic retry",
    )
    .await;
    child.clock.advance(2_000);

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(settled.detail.as_deref(), Some("ANSWER-AFTER-IDLE"));
    await_serve(wired.serve).await;

    let child_events = journal(&child.store);
    assert_eq!(
        count_events(&child_events, |event| matches!(
            event,
            RuntimeEvent::ModelRequestFailed { error, .. }
                if error.kind == ModelErrorKind::Timeout
        )),
        1
    );
    assert_eq!(count_events(&child_events, is_retry_scheduled), 1);
    assert_eq!(child.model.requests().len(), 2);
    let texts = parent_pending_texts(&plane);
    assert_eq!(texts, vec!["ANSWER-AFTER-IDLE".to_owned()]);
    assert!(
        !texts[0].contains("IDLE-PARTIAL"),
        "the timed-out stream's partial text stays child-local"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 12: the child's compaction summarizer follows #135 semantics
// ---------------------------------------------------------------------------

/// The child composition's model-backed summarizer receives the same
/// inherited policy and shared clock: a parked summary stream times out on
/// the manual clock and fails as `SummaryFailed` — never converted into
/// generic primary-model retry (exactly one summary request exists).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_compaction_summarizer_timeout_is_not_generic_retry() {
    let dir = tempfile::tempdir().expect("temp root");
    let (_release, release_rx) = model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-summary-child"),
        vec![vec![
            FakeStep::Emit(started()),
            FakeStep::ParkUntilReleased(release_rx),
        ]],
        ToolRegistry::new(),
        vec![timestamped_user("retired", &"old history ".repeat(512))],
    )
    .await;

    // Manual compaction is the real runtime/context admission path. The
    // parked next pull proves the summarizer consumed `Started` before the
    // shared manual clock advances past the inherited deadline.
    let summary_runtime = child.runtime.clone();
    let summary_task = tokio::spawn(async move { summary_runtime.compact_context().await });
    let mut parked = child.model.parked();
    tokio::time::timeout(LIVENESS, parked.wait_for(|parked| *parked))
        .await
        .expect("summary provider reaches its parked next pull")
        .expect("summary parked watch stays open");
    child
        .clock
        .advance(INHERITED_RESPONSE_START_MS + INHERITED_STREAM_IDLE_MS);
    let summary_error = tokio::time::timeout(LIVENESS, summary_task)
        .await
        .expect("summary deadline settles")
        .expect("summary task joins")
        .expect_err("the child summary must time out");
    assert!(
        matches!(
            summary_error,
            rustx::runtime::conversation_runtime::ManualCompactionError::Context(ref error)
                if error.kind == rustx::context::ContextErrorKind::SummaryFailed
        ),
        "the child summarizer follows Issue #135 deadline semantics: {summary_error:?}"
    );
    assert_eq!(
        child.model.requests().len(),
        1,
        "the summarizer never enters generic primary-model retry"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 8/13: child process loss is not provider retry
// ---------------------------------------------------------------------------

/// The child process dies while the child attempt sleeps in retry backoff.
/// The parent settles exactly one `Interrupted` terminal with the bounded
/// runtime-authored notice: the process/IPC outcome is unknown, not a model
/// failure. The parent never schedules or observes any retry and nothing
/// relaunches the child.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_process_loss_during_backoff_is_terminal_and_never_relaunched() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-crash");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-crash-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", None)),
            ],
            answer_script("NEVER-REACHED"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child_with_shell(&plane, &child, "inspect", "exec sleep 60").await;
    await_journal_fact(
        &child.store,
        1,
        is_retry_scheduled,
        "the child is in retry backoff when the process dies",
    )
    .await;

    // An actual abrupt child death: the OS process is SIGKILLed and the
    // control channel is gone (the serve task is aborted, dropping the
    // child endpoint) — the driver's only possible observation.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(wired.pid).expect("pid fits")),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("SIGKILL the scripted child process");
    wired
        .stop_serve
        .send(())
        .expect("stop the scripted child control loop");
    tokio::time::timeout(LIVENESS, wired.serve)
        .await
        .expect("child control loop shutdown")
        .expect("child control loop task")
        .expect("child control loop result");

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Interrupted);
    let detail = settled.detail.expect("bounded interruption detail");
    assert!(
        detail.contains("exited without a terminal result"),
        "the physical outcome is classified as Interrupted, not a retry: {detail}"
    );
    let parent_events = journal(&plane.store);
    assert_eq!(count_events(&parent_events, is_request_started), 0);
    assert_eq!(count_events(&parent_events, is_retry_scheduled), 0);
    assert_eq!(
        terminal_publications(&parent_events),
        vec![SubagentTerminalState::Interrupted],
        "one interruption terminal, fail-closed"
    );
    let notices = parent_pending_texts(&plane);
    assert_eq!(
        notices.len(),
        1,
        "exactly one parent-facing interruption notice"
    );
    assert!(
        notices[0].contains("interrupted") && notices[0].contains("unknown"),
        "the notice communicates unknown outcome without claiming model failure: {}",
        notices[0]
    );
    // No relaunch: the registry owns exactly this one terminal record, and
    // the child conversation's journal ends at the durable backoff
    // frontier — the crash consumed no retry and started no new request.
    assert_eq!(plane.registry.all_snapshots().len(), 1);
    let child_events = journal(&child.store);
    assert_eq!(count_events(&child_events, is_request_started), 1);
    assert_eq!(count_events(&child_events, is_retry_scheduled), 1);
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

// ---------------------------------------------------------------------------
// Invariant 9: exactly one terminal; late cancellation cannot rewrite it
// ---------------------------------------------------------------------------

/// Cancellation that arrives after the child terminalized is absorbed: the
/// settled snapshot is returned unchanged and no second publication exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_terminalization_cannot_rewrite_the_result() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-late-cancel");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-late-cancel-child"),
        vec![answer_script("LATE-CANCEL-ANSWER")],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    await_serve(wired.serve).await;

    // Both cancellation authorities arrive strictly after terminalization.
    let after_cancel = plane
        .registry
        .cancel(
            &wired.accepted.subagent_id,
            CancellationReason::UserRequested,
        )
        .expect("known child");
    assert_eq!(after_cancel.state, SubagentState::Succeeded);
    assert_eq!(after_cancel.detail.as_deref(), Some("LATE-CANCEL-ANSWER"));
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    let after_drain = plane
        .registry
        .snapshot(&wired.accepted.subagent_id)
        .expect("child record");
    assert_eq!(after_drain.state, SubagentState::Succeeded);
    assert_eq!(after_drain.detail.as_deref(), Some("LATE-CANCEL-ANSWER"));
    assert_eq!(
        terminal_publications(&journal(&plane.store)),
        vec![SubagentTerminalState::Succeeded],
        "the committed terminal is never rewritten"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Parent cancellation and runtime drain are two contenders for one
/// cancellation authority; whichever commits first owns the cause, and the
/// loser cannot rewrite it. Both orders are proven.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_and_drain_have_exactly_one_winning_cause() {
    for (user_first, expected_detail) in [
        (true, "requested by the user"),
        (false, "the runtime is shutting down"),
    ] {
        let dir = tempfile::tempdir().expect("temp root");
        let plane = standalone_parent_plane(&dir, "conv-138-cause-race");
        let child = child_fixture(
            &dir,
            &ConversationId::new("conv-138-cause-race-child"),
            vec![
                vec![
                    FakeStep::Emit(started()),
                    FakeStep::Emit(transient_failure("R0 boom", None)),
                ],
                answer_script("NEVER-REACHED"),
            ],
            ToolRegistry::new(),
            Vec::new(),
        )
        .await;
        let wired = park_child_in_retry_backoff(&child, &plane, "inspect").await;

        // The two requests are issued in a fixed order; the registry's one
        // cancellation-intent commit linearizes them.
        if user_first {
            let _ = plane.registry.cancel(
                &wired.accepted.subagent_id,
                CancellationReason::UserRequested,
            );
            plane
                .registry
                .cancel_all(CancellationReason::RuntimeShutdown);
        } else {
            plane
                .registry
                .cancel_all(CancellationReason::RuntimeShutdown);
            let _ = plane.registry.cancel(
                &wired.accepted.subagent_id,
                CancellationReason::UserRequested,
            );
        }
        let settled = plane
            .registry
            .wait_until_settled(&wired.accepted.subagent_id)
            .await
            .expect("child settles");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert!(
            settled
                .detail
                .as_deref()
                .expect("cancellation detail")
                .contains(expected_detail),
            "the first committed cause wins and is never rewritten: {:?}",
            settled.detail
        );
        await_serve(wired.serve).await;
        let child_events = journal(&child.store);
        let expected_reason = if user_first {
            CancellationReason::UserRequested
        } else {
            CancellationReason::RuntimeShutdown
        };
        assert_eq!(
            count_events(&child_events, |event| matches!(
                event,
                RuntimeEvent::AttemptCancelled { reason, .. } if *reason == expected_reason
            )),
            1,
            "the child journal records the same first-winner reason as the parent"
        );
        assert_eq!(
            count_events(&child_events, |event| matches!(
                event,
                RuntimeEvent::AttemptCancelled { .. }
            )),
            1,
            "the child settles exactly once"
        );
        assert_eq!(
            terminal_publications(&journal(&plane.store)),
            vec![SubagentTerminalState::Cancelled],
            "one terminal with one cause"
        );
        child
            .runtime
            .shutdown()
            .await
            .expect("child runtime drains");
    }
}

// ---------------------------------------------------------------------------
// Invariant 15: the parent projection carries no retry state
// ---------------------------------------------------------------------------

/// While the child sleeps in retry backoff, the parent-visible lifecycle is
/// exactly `Running` with no detail, and the parent durable authority holds
/// nothing but the ownership fact. The lifecycle vocabulary itself is
/// proven closed by the exhaustive match; no "retrying" state, ordinal,
/// delay, or provider-attempt channel exists to project.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_parent_projection_has_no_retry_state() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-138-projection");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-138-projection-child"),
        vec![
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", None)),
            ],
            answer_script("NEVER-REACHED"),
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = park_child_in_retry_backoff(&child, &plane, "inspect").await;

    // The child is mid-retry; the parent projection must not reflect it.
    let snapshot = plane
        .registry
        .snapshot(&wired.accepted.subagent_id)
        .expect("child record");
    match snapshot.state {
        SubagentState::Running => {}
        // The closed lifecycle vocabulary: there is no retry state to show.
        SubagentState::Cancelling
        | SubagentState::PublishingTerminal
        | SubagentState::Succeeded
        | SubagentState::Failed
        | SubagentState::Cancelled => {
            panic!("a child in retry backoff is Running: {:?}", snapshot.state)
        }
        SubagentState::Interrupted => {
            panic!(
                "a child in retry backoff cannot be interrupted: {:?}",
                snapshot.state
            )
        }
    }
    assert!(snapshot.detail.is_none(), "no retry detail is projected");
    let parent_events = journal(&plane.store);
    assert_eq!(count_events(&parent_events, is_request_started), 0);
    assert_eq!(count_events(&parent_events, is_retry_scheduled), 0);
    assert_eq!(
        count_events(&parent_events, |event| matches!(
            event,
            RuntimeEvent::SubagentOwnershipCommitted { .. }
        )),
        1,
        "the ownership fact is the only parent-visible state so far"
    );

    // Clean up through the ordinary cancellation path.
    let _ = plane.registry.cancel(
        &wired.accepted.subagent_id,
        CancellationReason::UserRequested,
    );
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Cancelled);
    await_serve(wired.serve).await;
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}
