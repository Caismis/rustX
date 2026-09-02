//! Issue #138: the subagent child ownership boundary.
//!
//! A rustX subagent child is a real `ConversationRuntime` with the ordinary
//! Agent Loop. Generic retry, deadline, cancellation-phase, publication,
//! settlement, and carryover semantics are owned by the [`super::agent`]
//! suites and are deliberately **not** re-proven here; these tests prove
//! only what the subagent boundary adds:
//!
//! - the frozen `ModelTimeoutPolicy` and child definition cross into the
//!   child;
//! - child-internal retries, failed-attempt publication, and carryover
//!   never leak into the parent conversation, projection, or requests;
//! - the parent registry/driver observes exactly one terminal notice;
//! - parent cancellation/drain crosses the ownership boundary with exactly
//!   one winning cause, and child process loss is terminal and never
//!   relaunched.
//!
//! The proofs wire a **real child runtime** (scripted model, manual
//! monotonic clock) to a **real parent registry/driver** over the **real
//! control IPC** socket pair: the child side runs the exact production
//! `serve_child_delegation` loop, and the parent side is the production
//! `SubagentRegistry` settlement path. Only the OS process is a scripted
//! stand-in (kill/reap semantics), exactly like the Issue #60 registry
//! regressions. The real-process half — a launched named child consuming
//! the frozen definition through the typed spawn path — lives in the
//! external `subagent` integration target.
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

use super::super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::durable::ConversationStore;
use rustx::events::types::{RuntimeEvent, SubagentTerminalState};
use rustx::message::types::{ContentBlockIndex, MessageBlock, UserContentBlock, UserSource};
use rustx::model::ModelTimeoutPolicy;
use rustx::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::conversation_runtime::{
    ConversationContextConfig, ConversationRuntime, RuntimeConversationConfig,
};
use rustx::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
use rustx::runtime::types::{CancellationReason, SystemClock};
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::executor::ToolRegistry;
use support::fake::{FakeModel, FakeStep, FakeTool, fake_model};

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::observation::PendingObservations;
use crate::runtime::subagent::process::StagedChild;
use crate::runtime::subagent::{
    ResolvedSubagentSpec, SubagentAccepted, SubagentActivity, SubagentExecutionProfile,
    SubagentName, SubagentObservation, SubagentObserver, SubagentRegistry, SubagentRegistryConfig,
    SubagentSnapshot, SubagentSpawnPlan, SubagentStartOutcome, SubagentStartSpec, SubagentState,
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
    store: Arc<dyn ConversationStore>,
    /// The child's manually advanced monotonic clock: retry backoff only
    /// completes when the test advances it past the captured deadline.
    clock: Arc<ManualMonotonicClock>,
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
            workflow_output: None,
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
        store,
        clock,
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
    compose_parent_runtime_plane(dir, conversation, parent_scripts, false)
        .await
        .0
}

/// The same parent plane with a Runtime Client host bound before
/// activation: the host's projection then observes the registry through the
/// ordinary observation bridge, so a late-attaching client exercises the
/// real snapshot-repair path.
async fn parent_runtime_host_plane(
    dir: &tempfile::TempDir,
    conversation: &str,
    parent_scripts: Vec<Vec<FakeStep>>,
) -> (ParentRuntimePlane, rustx::runtime_client::RuntimeClientHost) {
    let (plane, host) = compose_parent_runtime_plane(dir, conversation, parent_scripts, true).await;
    (plane, host.expect("the host plane composes the host"))
}

#[allow(clippy::too_many_lines)] // fixture composition; mirrors child_fixture
async fn compose_parent_runtime_plane(
    dir: &tempfile::TempDir,
    conversation: &str,
    parent_scripts: Vec<Vec<FakeStep>>,
    with_host: bool,
) -> (
    ParentRuntimePlane,
    Option<rustx::runtime_client::RuntimeClientHost>,
) {
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
        workflow_output: None,
    })
    .expect("parent runtime composition");
    // The Runtime Client host binds before activation: its bridge handshake
    // installs the registry observer and seeds the projection at one global
    // cut over the inert runtime.
    let host = with_host.then(|| {
        rustx::runtime_client::RuntimeClientHost::new(
            rustx::runtime_client::RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
        )
        .expect("runtime client host composition")
    });
    runtime.activate();
    (
        ParentRuntimePlane {
            plane: ParentPlane {
                registry,
                store: tool_runtime.durable_store(),
                parent_agent_id: AgentId::new("agent-parent-138"),
                runtime_root,
            },
            runtime,
            model,
        },
        host,
    )
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
                approval_mode: rustx::runtime::ApprovalMode::Policy,
                task: task.to_owned(),
                context: None,
                tool_call_id: ToolCallId::new("call-138"),
                terminal: rustx::runtime::subagent::SubagentTerminalMode::Normal,
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
                None,
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

/// Polls the registry read model until one snapshot satisfies `predicate`.
///
/// Same discipline as [`await_journal_fact`]: the applied read model is the
/// synchronization point (an activity frame applies synchronously when the
/// driver decodes it), so observing the projection is proof of the
/// interleaving; the timeout only contains a broken fixture.
async fn await_snapshot(
    plane: &ParentPlane,
    subagent_id: &SubagentId,
    predicate: impl Fn(&SubagentSnapshot) -> bool,
    description: &str,
) -> SubagentSnapshot {
    tokio::time::timeout(LIVENESS, async {
        loop {
            let snapshot = plane.registry.snapshot(subagent_id).expect("child record");
            if predicate(&snapshot) {
                return snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{description}: the snapshot projection did not arrive within liveness guard")
    })
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
        rustx::runtime::ApprovalMode::Policy,
        &physical_root,
        &workspace,
        &rustx::runtime::subagent::SubagentTerminalMode::Normal,
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
    // Issue #178: the successful answer never rides the live
    // observation/control projection — `detail` is diagnostics-only, and
    // the durable terminal inbound publication below is the one result
    // channel.
    assert_eq!(settled.detail, None);
    assert!(
        !serde_json::to_string(&settled)
            .expect("snapshot serializes")
            .contains("CHILD-FINAL-ANSWER"),
        "no success content in the serialized snapshot"
    );
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
    // Issue #178: the successful answer is absent from the live projection;
    // the durable terminal inbound publication below is the one result
    // channel.
    assert_eq!(settled.detail, None);
    assert!(
        !serde_json::to_string(&settled)
            .expect("snapshot serializes")
            .contains("FINAL-ANSWER"),
        "no success content in the serialized snapshot"
    );
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
    // Issue #178: the successful answer lives only in the durable terminal
    // inbound publication; the live projection's `detail` is
    // diagnostics-only and stays `None` on success.
    assert_eq!(settled.detail, None);
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
    assert_eq!(after_cancel.detail, None);
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    let after_drain = plane
        .registry
        .snapshot(&wired.accepted.subagent_id)
        .expect("child record");
    assert_eq!(after_drain.state, SubagentState::Succeeded);
    assert_eq!(after_drain.detail, None);
    assert_eq!(
        terminal_publications(&journal(&plane.store)),
        vec![SubagentTerminalState::Succeeded],
        "the committed terminal is never rewritten"
    );
    assert_eq!(
        parent_pending_texts(&plane),
        vec!["LATE-CANCEL-ANSWER".to_owned()],
        "the answer exists exactly once, in the canonical terminal inbound"
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
// Invariant 15: retry is activity, never lifecycle (Issue #178)
// ---------------------------------------------------------------------------

/// While the child sleeps in retry backoff, the parent-visible lifecycle is
/// exactly `Running` — the closed lifecycle vocabulary is proven by the
/// exhaustive match — and the parent durable authority holds nothing but
/// the ownership fact. The retry is visible exclusively on the observation
/// plane: `snapshot.observation.activity` projects `RetryingModel` with the
/// scheduled ordinal, folded from the child's durable retry schedule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_is_activity_never_lifecycle() {
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

    // The child's durable retry schedule is committed; the activity frame
    // it folds into arrives over the control channel and lands in the
    // registry read model. The snapshot poll is the synchronization point:
    // the read model is the observable, never a timing guess.
    let snapshot = tokio::time::timeout(LIVENESS, async {
        loop {
            let snapshot = plane
                .registry
                .snapshot(&wired.accepted.subagent_id)
                .expect("child record");
            if matches!(
                snapshot.observation.activity,
                crate::runtime::subagent::SubagentActivity::RetryingModel { .. }
            ) {
                return snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retry activity projection reaches the parent read model");
    match snapshot.state {
        SubagentState::Running => {}
        // The closed lifecycle vocabulary: there is no retry state; the
        // retry exists only as observation-plane activity.
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
    assert_eq!(
        snapshot.observation.activity,
        crate::runtime::subagent::SubagentActivity::RetryingModel { retry: 1 },
        "the scheduled retry is visible as activity with its ordinal"
    );
    assert_eq!(
        snapshot.observation.counters.model_retries, 1,
        "the retry counter advanced with the schedule"
    );
    assert!(snapshot.detail.is_none(), "no retry detail is projected");

    // The retry never enters the parent's durable authority: the ownership
    // fact remains the only parent-visible state.
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

// ---------------------------------------------------------------------------
// Issue #178: the live activity observation plane
//
// These tests prove what the observation plane adds across the real control
// boundary, and — just as much — what it can never touch: the parent
// lifecycle, the parent journal, the parent model context, and the result
// channel. The child side folds its own observation stream into a
// latest-value projection and forwards it over the control socket; the
// parent driver applies it synchronously into the registry read model.
//
// # Determinism
//
// The child→parent lane coalesces with latest-value semantics, so only a
// state the child is *parked at* is a stable parent-observable cut: a
// parked model stream, a parked tool execution, or retry backoff frozen by
// the manual clock. Transitions between two parked states may legitimately
// coalesce away; the tests below therefore assert the parked states, the
// counters, and the terminal-neutral reset — never an intermediate cut
// between two immediate folds.
// ---------------------------------------------------------------------------

/// One scripted tool call proposal at block index 0, then completion with
/// the tool-calls finish reason: the first request of a tool-using child
/// script.
fn tool_call_request(call: &support::fake::ScriptedCall) -> Vec<FakeStep> {
    let mut steps = vec![FakeStep::Emit(started())];
    steps.extend(
        support::fake::tool_call_events(0, call)
            .into_iter()
            .map(FakeStep::Emit),
    );
    steps.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    steps
}

/// The durable event-kind sequence of one store. Event payloads carry
/// per-run identities; the kind ordering is the comparable structure.
fn event_kinds(events: &[RuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            serde_json::to_value(event).expect("event json")["type"]
                .as_str()
                .expect("typed event")
                .to_owned()
        })
        .collect()
}

/// Invariant: an in-flight model request projects `Model` activity while
/// the lifecycle — the only authority — stays `Running`; terminal
/// settlement resets the projection to neutral with a bumped revision and
/// keeps the counters as the final record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_activity_projects_while_lifecycle_stays_running() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-model-activity");
    let (release, released) = support::fake::model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-model-activity-child"),
        vec![vec![
            FakeStep::Emit(started()),
            FakeStep::ParkUntilReleased(released),
            FakeStep::Emit(text("MODEL-ACTIVITY-ANSWER")),
            FakeStep::Emit(completed()),
        ]],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // The provider stream is parked mid-request: the projection is stable
    // at Model for as long as the request is in flight.
    let running = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            matches!(
                snapshot.observation.activity,
                SubagentActivity::Model { .. }
            )
        },
        "the in-flight model request projects Model activity",
    )
    .await;
    assert_eq!(
        running.state,
        SubagentState::Running,
        "activity never moves the lifecycle"
    );
    let SubagentActivity::Model { retry, .. } = &running.observation.activity else {
        unreachable!("the predicate matched a Model activity");
    };
    assert_eq!(*retry, 0, "the first request is not a retry");
    assert_eq!(running.observation.counters.model_requests, 1);
    assert_eq!(running.observation.counters.tool_executions, 0);
    assert!(
        running.observation.last_activity_at.is_some(),
        "an applied transition carries its live observation timestamp"
    );
    assert!(running.detail.is_none(), "no detail while running");

    // The request completes and the child settles: the projection rests at
    // neutral with a bumped revision, and the lifecycle carries the
    // terminal truth.
    release.send_replace(true);
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.observation.activity,
        SubagentActivity::AwaitingActivity,
        "settlement resets the projection to neutral"
    );
    assert!(
        settled.observation.revision > running.observation.revision,
        "the neutral reset is itself observable"
    );
    assert_eq!(
        settled.observation.counters.model_requests, 1,
        "the counters survive the reset as the final record"
    );
    assert_eq!(settled.detail, None, "the answer never rides the detail");
    await_serve(wired.serve).await;
    assert_eq!(
        parent_pending_texts(&plane),
        vec!["MODEL-ACTIVITY-ANSWER".to_owned()],
        "the answer arrives exactly once, through the canonical inbound"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Invariant: a tool execution projects `Tool` with the executing call
/// identity while it is parked; reported progress is scoped to that one
/// in-flight execution — it never sticks past the completion, which returns
/// the projection to neutral and counts the execution. The lifecycle stays
/// `Running` throughout.
///
/// Live (not yet durable) foreground progress projects while the tool still
/// executes (Issue #178, blocker 3): the parked projection carries the
/// newest report with latest-value coalescing. The durable
/// `ToolExecutionProgress` facts still commit only in the completion batch
/// commit; the completion fold then resets to neutral, and no reported
/// progress can survive into a settled or later projection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_activity_projects_identity_scoped_progress_and_counts_executions() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-tool-activity");
    let mut tools = ToolRegistry::new();
    // The first tool parks without reporting progress; the second parks
    // after three numbered progress reports.
    let (probe, probe_release) = FakeTool::parking(
        common::tool_policies(
            "probe",
            "tool-probe",
            rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("probe done"),
    );
    let mut probe_started = probe.started();
    probe.register(&mut tools);
    let (scan, scan_release) = FakeTool::parking(
        common::tool_policies(
            "scan",
            "tool-scan",
            rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("scan done"),
    );
    let scan = scan.emitting_progress(3);
    let mut scan_started = scan.started();
    scan.register(&mut tools);
    // The final answer request parks too, so the completion counters are
    // observably applied before the terminal frame can race them.
    let (answer_release, answer_released) = support::fake::model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-tool-activity-child"),
        vec![
            tool_call_request(&support::fake::ScriptedCall {
                id: "call-probe",
                tool_id: "tool-probe",
                name: "probe",
                arguments: serde_json::json!({}),
            }),
            tool_call_request(&support::fake::ScriptedCall {
                id: "call-scan",
                tool_id: "tool-scan",
                name: "scan",
                arguments: serde_json::json!({}),
            }),
            vec![
                FakeStep::Emit(started()),
                FakeStep::ParkUntilReleased(answer_released),
                FakeStep::Emit(text("TOOL-ACTIVITY-ANSWER")),
                FakeStep::Emit(completed()),
            ],
        ],
        tools,
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // Parked in the first execution with no progress reported: the tool
    // identity projects with `progress: None`.
    support::fake::await_started(&mut probe_started, "the probe tool starts").await;
    let probing = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| matches!(snapshot.observation.activity, SubagentActivity::Tool { .. }),
        "the parked tool execution projects Tool activity",
    )
    .await;
    assert_eq!(probing.state, SubagentState::Running);
    assert_eq!(
        probing.observation.activity,
        SubagentActivity::Tool {
            tool_call_id: ToolCallId::new("call-probe"),
            tool_id: rustx::runtime::identity::ToolId::new("tool-probe"),
            progress: None,
        },
        "no progress has been reported for the parked execution"
    );
    assert_eq!(probing.observation.counters.tool_executions, 0);

    // The second execution reports bounded structured progress before it
    // parks. The live reports project while the tool still executes: the
    // parked projection deterministically carries the newest report
    // (latest-value coalescing), and the durable facts still commit only
    // with the completion batch.
    probe_release.send_replace(true);
    support::fake::await_started(&mut scan_started, "the scan tool starts").await;
    let scanning = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            matches!(
                snapshot.observation.activity,
                SubagentActivity::Tool { ref tool_call_id, progress: Some(ref progress), .. }
                    if *tool_call_id == ToolCallId::new("call-scan")
                        && progress.message.as_deref() == Some("progress 2")
            )
        },
        "the parked execution projects its Tool identity with the latest live progress",
    )
    .await;
    assert_eq!(scanning.state, SubagentState::Running);
    assert_eq!(
        scanning.observation.activity,
        SubagentActivity::Tool {
            tool_call_id: ToolCallId::new("call-scan"),
            tool_id: rustx::runtime::identity::ToolId::new("tool-scan"),
            progress: Some(rustx::tools::types::ToolProgress {
                message: Some("progress 2".to_owned()),
                completed: None,
                total: None,
            }),
        },
        "live foreground progress projects in-flight, coalesced to the latest report"
    );
    assert!(
        scanning.observation.revision > probing.observation.revision,
        "revisions are strictly increasing across applied transitions"
    );
    assert_eq!(scanning.observation.counters.tool_executions, 1);

    // Both executions complete and the answer request parks in flight: the
    // completion counters are observably applied, and the progress the scan
    // tool reported stuck to nothing — it never survives the completion
    // transition it committed with.
    scan_release.send_replace(true);
    let answering = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            snapshot.observation.counters.tool_executions == 2
                && snapshot.observation.counters.model_requests == 3
        },
        "both completions and the third request are observably applied",
    )
    .await;
    assert!(
        matches!(
            answering.observation.activity,
            SubagentActivity::Model { .. } | SubagentActivity::AwaitingActivity
        ),
        "no progress outlives the completion it committed with: {:?}",
        answering.observation.activity
    );

    // The answer request completes and the child settles: the projection
    // rests at neutral and the counters hold as the final record.
    answer_release.send_replace(true);
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.observation.activity,
        SubagentActivity::AwaitingActivity
    );
    assert_eq!(settled.observation.counters.tool_executions, 2);
    assert_eq!(settled.observation.counters.model_requests, 3);
    assert!(
        settled.observation.revision > scanning.observation.revision,
        "the completion transition and the terminal reset both advanced the revision"
    );
    await_serve(wired.serve).await;
    assert_eq!(
        parent_pending_texts(&plane),
        vec!["TOOL-ACTIVITY-ANSWER".to_owned()]
    );
    // The child's tool work committed no parent journal facts at all.
    let parent_events = journal(&plane.store);
    assert_eq!(count_events(&parent_events, is_request_started), 0);
    assert_eq!(
        count_events(&parent_events, |event| matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolExecutionCompleted { .. }
        )),
        0,
        "tool activity never enters the parent durable authority"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Invariant: a real transient retry is visible as activity — never as
/// lifecycle. While the child sleeps in retry backoff the parent projects
/// `RetryingModel` with the scheduled ordinal; once the manual clock
/// releases the backoff, the next in-flight request projects `Model` with
/// the retry ordinal it was scheduled under.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transient_retry_projects_retrying_model_then_the_retried_request() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-retry-activity");
    let (release, released) = support::fake::model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-retry-activity-child"),
        vec![
            // No provider hint: the backoff deadline is 2000ms ahead on the
            // manual clock, which the test advances exactly once.
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(transient_failure("R0 boom", None)),
            ],
            vec![
                FakeStep::Emit(started()),
                FakeStep::ParkUntilReleased(released),
                FakeStep::Emit(text("RETRY-ACTIVITY-ANSWER")),
                FakeStep::Emit(completed()),
            ],
        ],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = park_child_in_retry_backoff(&child, &plane, "inspect").await;

    // Parked in backoff: the retry schedule is durably committed and the
    // projection is stable at RetryingModel.
    let retrying = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            matches!(
                snapshot.observation.activity,
                SubagentActivity::RetryingModel { .. }
            )
        },
        "the committed retry schedule projects RetryingModel activity",
    )
    .await;
    assert_eq!(retrying.state, SubagentState::Running);
    assert_eq!(
        retrying.observation.activity,
        SubagentActivity::RetryingModel { retry: 1 }
    );
    assert_eq!(retrying.observation.counters.model_retries, 1);
    assert_eq!(retrying.observation.counters.model_requests, 1);

    // The manual clock reaches the captured deadline: the retried request
    // starts and parks mid-stream, projecting Model with its retry ordinal.
    child.clock.advance(10_000);
    let retried = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            matches!(
                snapshot.observation.activity,
                SubagentActivity::Model { .. }
            )
        },
        "the retried request projects Model activity",
    )
    .await;
    assert_eq!(retried.state, SubagentState::Running);
    let SubagentActivity::Model { retry, .. } = &retried.observation.activity else {
        unreachable!("the predicate matched a Model activity");
    };
    assert_eq!(*retry, 1, "the in-flight request is the scheduled retry");
    assert_eq!(retried.observation.counters.model_requests, 2);
    assert!(retried.observation.revision > retrying.observation.revision);

    release.send_replace(true);
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.observation.activity,
        SubagentActivity::AwaitingActivity
    );
    assert_eq!(settled.observation.counters.model_retries, 1);
    assert_eq!(settled.observation.counters.model_requests, 2);
    await_serve(wired.serve).await;
    assert_eq!(
        parent_pending_texts(&plane),
        vec!["RETRY-ACTIVITY-ANSWER".to_owned()]
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Invariant: the observation consumer is structurally independent of the
/// child's execution. Two equivalent children — one whose observation
/// stream is drained and folded into activity frames, one whose queue is
/// parked from the start — produce identical provider request counts, tool
/// execution counts, durable event ordering, and terminal outcomes; the
/// drained run publishes its terminal exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_activity_consumer_never_changes_child_execution() {
    /// Runs one child to completion with or without an observation
    /// consumer and returns the execution record to compare.
    async fn run(conversation: &str, drained: bool) -> (usize, usize, Vec<String>) {
        let dir = tempfile::tempdir().expect("temp root");
        let plane = standalone_parent_plane(&dir, conversation);
        let mut tools = ToolRegistry::new();
        let probe = FakeTool::new(
            common::tool_policies(
                "probe",
                "tool-probe",
                rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
                rustx::tools::types::ToolConcurrencyPolicy::Sequential,
            ),
            support::fake::success_result("probe done"),
        );
        let probe_calls = probe.calls();
        probe.register(&mut tools);
        let child = child_fixture(
            &dir,
            &ConversationId::new(format!("{conversation}-child")),
            vec![
                tool_call_request(&support::fake::ScriptedCall {
                    id: "call-probe",
                    tool_id: "tool-probe",
                    name: "probe",
                    arguments: serde_json::json!({}),
                }),
                answer_script("OBSERVER-INDEPENDENCE-ANSWER"),
            ],
            tools,
            Vec::new(),
        )
        .await;
        if !drained {
            // The consumer never drains: the serve loop folds nothing, so
            // no activity frame ever leaves the child.
            child.observations.park();
        }
        let wired = launch_wired_child(&plane, &child, "inspect").await;

        if drained {
            let settled = plane
                .registry
                .wait_until_settled(&wired.accepted.subagent_id)
                .await
                .expect("child settles");
            assert_eq!(settled.state, SubagentState::Succeeded);
            await_serve(wired.serve).await;
            assert_eq!(
                terminal_publications(&journal(&plane.store)),
                vec![SubagentTerminalState::Succeeded],
                "exactly one terminal publication"
            );
            assert_eq!(
                parent_pending_texts(&plane),
                vec!["OBSERVER-INDEPENDENCE-ANSWER".to_owned()],
                "exactly one terminal inbound carries the answer"
            );
        } else {
            // The agent loop reaches its durable terminal with the consumer
            // stalled — the completion commit is the nonblocking proof.
            await_journal_fact(
                &child.store,
                1,
                |event| matches!(event, RuntimeEvent::AttemptCompleted { .. }),
                "the child attempt completes with the observation consumer parked",
            )
            .await;
            // Cleanup: stopping the serve loop drops the control channel;
            // the parent classifies the unpublished child as Interrupted.
            wired
                .stop_serve
                .send(())
                .expect("stop the stalled serve loop");
            await_serve(wired.serve).await;
            let settled = plane
                .registry
                .wait_until_settled(&wired.accepted.subagent_id)
                .await
                .expect("child settles");
            assert_eq!(settled.state, SubagentState::Interrupted);
        }

        let record = (
            child.model.requests().len(),
            probe_calls.borrow().len(),
            event_kinds(&journal(&child.store)),
        );
        child
            .runtime
            .shutdown()
            .await
            .expect("child runtime drains");
        record
    }

    let drained = run("conv-178-observer-drained", true).await;
    let parked = run("conv-178-observer-parked", false).await;
    assert_eq!(drained.0, 2, "one tool-call request, one answer request");
    assert_eq!(drained.1, 1, "one tool execution");
    assert_eq!(
        drained, parked,
        "the observation consumer changes nothing about the child's execution"
    );
}

/// Invariant (backpressure): with the observation queue parked from the
/// start, the child agent loop completes its model and tool work and
/// reaches its durable terminal settlement without blocking — while the
/// parent read model provably never advanced (the observation plane is not
/// an authority: nothing about the child's progress is inferred from it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_observation_consumer_blocks_nothing_and_carries_no_authority() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-backpressure");
    let mut tools = ToolRegistry::new();
    let probe = FakeTool::new(
        common::tool_policies(
            "probe",
            "tool-probe",
            rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("probe done"),
    );
    let probe_calls = probe.calls();
    probe.register(&mut tools);
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-backpressure-child"),
        vec![
            tool_call_request(&support::fake::ScriptedCall {
                id: "call-probe",
                tool_id: "tool-probe",
                name: "probe",
                arguments: serde_json::json!({}),
            }),
            answer_script("BACKPRESSURE-ANSWER"),
        ],
        tools,
        Vec::new(),
    )
    .await;
    child.observations.park();
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    // The durable terminal commit linearizes the proof: every runtime
    // action of the attempt happened without any observation consumer.
    let child_events = await_journal_fact(
        &child.store,
        1,
        |event| matches!(event, RuntimeEvent::AttemptCompleted { .. }),
        "the child attempt completes with the observation consumer stalled",
    )
    .await;
    assert_eq!(child.model.requests().len(), 2);
    assert_eq!(probe_calls.borrow().len(), 1);
    assert_eq!(count_events(&child_events, is_request_started), 2);

    // The parent side is untouched: the lifecycle is still Running, the
    // observation projection is still the initial neutral value, and the
    // journal holds the ownership fact alone.
    let snapshot = plane
        .registry
        .snapshot(&wired.accepted.subagent_id)
        .expect("child record");
    assert_eq!(snapshot.state, SubagentState::Running);
    assert_eq!(
        snapshot.observation,
        SubagentObservation::default(),
        "no observation ever arrived; nothing is inferred"
    );
    let parent_events = journal(&plane.store);
    assert_eq!(
        parent_events.len(),
        1,
        "only the ownership fact exists so far: {:?}",
        event_kinds(&parent_events)
    );

    // Cleanup through the scripted control-plane stop: the parent settles
    // exactly one Interrupted terminal for the unpublished child.
    wired
        .stop_serve
        .send(())
        .expect("stop the stalled serve loop");
    await_serve(wired.serve).await;
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Interrupted);
    assert_eq!(
        terminal_publications(&journal(&plane.store)),
        vec![SubagentTerminalState::Interrupted]
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Invariant: a full child run with activity frames flowing leaves the
/// parent Event Journal exactly as the lifecycle authority alone would —
/// the ownership fact and the one terminal publication, and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activity_frames_commit_no_parent_journal_facts() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-journal-isolation");
    let mut tools = ToolRegistry::new();
    // The tool parks so the mid-run Tool activity is applied before the
    // terminal can settle: without a parked-stable observation point, the
    // coalesced frames could all arrive after the Result frame and be
    // dropped as post-terminal, making "activity flowed" unprovable.
    let (scan, scan_release) = FakeTool::parking(
        common::tool_policies(
            "scan",
            "tool-scan",
            rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("scan done"),
    );
    let mut scan_started = scan.started();
    scan.register(&mut tools);
    // The answer request parks as well, so the completion counter is
    // observably applied before the terminal frame can race it.
    let (answer_release, answer_released) = support::fake::model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-journal-isolation-child"),
        vec![
            tool_call_request(&support::fake::ScriptedCall {
                id: "call-scan",
                tool_id: "tool-scan",
                name: "scan",
                arguments: serde_json::json!({}),
            }),
            vec![
                FakeStep::Emit(started()),
                FakeStep::ParkUntilReleased(answer_released),
                FakeStep::Emit(text("JOURNAL-ISOLATION-ANSWER")),
                FakeStep::Emit(completed()),
            ],
        ],
        tools,
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;
    support::fake::await_started(&mut scan_started, "the scan tool starts").await;
    // Activity really flowed: the parked execution's Tool frame was applied
    // before the terminal could settle.
    let running = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            matches!(
                snapshot.observation.activity,
                SubagentActivity::Tool { ref tool_call_id, .. }
                    if *tool_call_id == ToolCallId::new("call-scan")
            )
        },
        "the parked tool execution projects Tool activity",
    )
    .await;
    assert!(running.observation.revision > 1);
    scan_release.send_replace(true);
    // The completion counter is observably applied while the answer request
    // is still parked in flight.
    let completing = await_snapshot(
        &plane,
        &wired.accepted.subagent_id,
        |snapshot| snapshot.observation.counters.tool_executions == 1,
        "the tool completion counter is applied before the terminal",
    )
    .await;
    answer_release.send_replace(true);
    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert!(
        settled.observation.revision > completing.observation.revision,
        "completion and the terminal reset advanced the revision further"
    );
    assert_eq!(settled.observation.counters.tool_executions, 1);
    await_serve(wired.serve).await;

    // The complete parent journal: ownership + terminal, in order, nothing
    // else. Every activity frame committed no durable fact.
    let parent_events = journal(&plane.store);
    assert_eq!(
        event_kinds(&parent_events),
        vec![
            "subagent_ownership_committed".to_owned(),
            "subagent_terminal_published".to_owned(),
        ],
        "the parent journal holds the lifecycle facts and nothing else"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Invariant: child activity never enters the parent model context. After a
/// full child run consumed by a real parent runtime, no parent model
/// request contains the child's activity identifiers, and the only
/// child-origin content any request ever carries is the canonical
/// Agent-authored terminal inbound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_activity_never_enters_the_parent_model_context() {
    let dir = tempfile::tempdir().expect("temp root");
    let parent = parent_runtime_plane(
        &dir,
        "conv-178-context-parent",
        vec![answer_script("PARENT-178-TURN")],
    )
    .await;
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool_policies(
            "scan178",
            "tool-178-secret",
            rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
            rustx::tools::types::ToolConcurrencyPolicy::Sequential,
        ),
        support::fake::success_result("scan done"),
    )
    .register(&mut tools);
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-context-child"),
        vec![
            tool_call_request(&support::fake::ScriptedCall {
                id: "call-178-secret",
                tool_id: "tool-178-secret",
                name: "scan178",
                arguments: serde_json::json!({}),
            }),
            answer_script("CHILD-178-ANSWER-MARKER"),
        ],
        tools,
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
    assert_eq!(settled.state, SubagentState::Succeeded);
    await_serve(wired.serve).await;

    // The parent runtime adopted the canonical terminal inbound and
    // completed its continuation turn through its ordinary Agent Loop.
    await_journal_fact(
        &parent.plane.store,
        1,
        |event| matches!(event, RuntimeEvent::AttemptCompleted { .. }),
        "the parent consumes the terminal inbound in an ordinary turn",
    )
    .await;

    let requests = parent.model.requests();
    assert!(
        !requests.is_empty(),
        "the parent really called its model for the continuation"
    );
    let mut answer_sightings = 0_usize;
    for request in &requests {
        let serialized = serde_json::to_string(request).expect("request json");
        for marker in ["call-178-secret", "tool-178-secret", "scan178"] {
            assert!(
                !serialized.contains(marker),
                "child activity identifiers never enter the parent model context: {serialized}"
            );
        }
        // The child answer appears only as the canonical Agent-authored
        // inbound message — never as assistant output, tool results, or
        // request-only context.
        for message in &request.messages {
            let serialized = serde_json::to_string(message).expect("message json");
            if serialized.contains("CHILD-178-ANSWER-MARKER") {
                answer_sightings += 1;
                assert!(
                    matches!(
                        message,
                        rustx::model::input::ModelInputMessage::Canonical(
                            MessageBlock::User(user)
                        ) if matches!(user.source, UserSource::Agent { .. })
                    ),
                    "the child answer enters the parent context only as the canonical \
                     Agent-authored terminal inbound: {serialized}"
                );
            }
        }
    }
    assert!(
        answer_sightings > 0,
        "the parent really consumed the child answer through the canonical inbound"
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

/// Invariant: the successful terminal answer never enters the observation
/// plane. A recording observer captures every snapshot the registry ever
/// published — including every applied activity frame — and none of them,
/// nor the Runtime Client view of the settled child, contains the answer;
/// the answer exists exactly once, in the canonical durable terminal
/// inbound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_successful_answer_never_enters_the_observation_plane() {
    const ANSWER: &str = "OBS-178-RESULT-SECRET";

    /// Records every published registry snapshot (called under the registry
    /// lock; the push is the whole implementation).
    #[derive(Default)]
    struct RecordingObserver(std::sync::Mutex<Vec<SubagentSnapshot>>);
    impl SubagentObserver for RecordingObserver {
        fn on_snapshot(&self, snapshot: &SubagentSnapshot) {
            self.0
                .lock()
                .expect("recording lock")
                .push(snapshot.clone());
        }
    }

    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-result-isolation");
    let recorded = Arc::new(RecordingObserver::default());
    plane
        .registry
        .install_observer_and_snapshots(Arc::clone(&recorded) as Arc<dyn SubagentObserver>);
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-result-isolation-child"),
        vec![answer_script(ANSWER)],
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

    let snapshots = recorded.0.lock().expect("recording lock").clone();
    assert!(
        snapshots
            .iter()
            .any(|snapshot| snapshot.observation.revision > 0),
        "activity frames were really applied during the run"
    );
    for snapshot in &snapshots {
        let serialized = serde_json::to_string(snapshot).expect("snapshot json");
        assert!(
            !serialized.contains(ANSWER),
            "no published snapshot — lifecycle or activity — ever carries the answer: {serialized}"
        );
    }

    // The Runtime Client view of the settled child is equally clean.
    let view = crate::runtime_client::projection::subagent_view(&settled);
    let serialized = serde_json::to_string(&view).expect("client view json");
    assert!(
        !serialized.contains(ANSWER),
        "the Runtime Client subagent view never carries the answer: {serialized}"
    );
    assert_eq!(view.detail, None);
    assert_eq!(
        view.observation.activity,
        SubagentActivity::AwaitingActivity
    );

    // The answer exists exactly once: the canonical durable terminal
    // inbound.
    assert_eq!(
        parent_pending_texts(&plane),
        vec![ANSWER.to_owned()],
        "the canonical inbound is the one result channel"
    );
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}

/// Invariant: a Runtime Client that attaches after activity already flowed
/// repairs from the snapshot alone — the initialized snapshot and an
/// explicit `snapshot_get` both serve the latest observation (revision,
/// activity, counters), the redacted execution profile, and the start time,
/// without the client having consumed any intermediate `SubagentUpdated`
/// event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_repair_serves_the_latest_subagent_observation() {
    let dir = tempfile::tempdir().expect("temp root");
    let (parent, host) = parent_runtime_host_plane(
        &dir,
        "conv-178-repair",
        vec![answer_script("PARENT-178-DONE")],
    )
    .await;
    let (release, released) = support::fake::model_release();
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-repair-child"),
        vec![vec![
            FakeStep::Emit(started()),
            FakeStep::ParkUntilReleased(released),
            FakeStep::Emit(text("REPAIR-ANSWER")),
            FakeStep::Emit(completed()),
        ]],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&parent.plane, &child, "inspect").await;

    // The child is parked mid-request; the registry read model holds the
    // stable Model projection.
    let live = await_snapshot(
        &parent.plane,
        &wired.accepted.subagent_id,
        |snapshot| {
            matches!(
                snapshot.observation.activity,
                SubagentActivity::Model { .. }
            )
        },
        "the in-flight model request projects Model activity",
    )
    .await;
    // The host projection folded the same observation: poll the projection
    // itself so the attach below provably seeds from the folded state.
    tokio::time::timeout(LIVENESS, async {
        loop {
            let (snapshot, _) = host.snapshot().expect("projection snapshot");
            if snapshot
                .subagents
                .iter()
                .any(|view| view.observation == live.observation)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the host projection folds the activity observation");

    // A fresh client attaches now. Its initialized snapshot is the repair
    // path: it consumed no SubagentUpdated event at all.
    let (attachment, initialized) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let rustx::runtime_client::RuntimeClientResult::Initialized { snapshot, .. } = initialized
    else {
        panic!("attach initializes: {initialized:?}");
    };
    let view = snapshot
        .subagents
        .iter()
        .find(|view| view.subagent_id == wired.accepted.subagent_id)
        .expect("the child is in the snapshot");
    assert_eq!(
        view.observation, live.observation,
        "the repaired snapshot serves the latest observation"
    );
    assert_eq!(
        view.execution_profile,
        Some(SubagentExecutionProfile {
            model: "local/model".to_owned(),
            reasoning_profile: None,
            reasoning_enabled: false,
        }),
        "the redacted profile repairs with the snapshot"
    );
    assert_eq!(view.started_at, live.started_at);
    assert_eq!(view.state, rustx::runtime::subagent::SubagentState::Running);

    // An explicit snapshot_get agrees — the same repair primitive.
    let response =
        attachment.handle_request(rustx::runtime_client::RuntimeClientRequest::SnapshotGet {
            id: rustx::runtime_client::RequestId::new(1),
        });
    let Some(rustx::runtime_client::RuntimeClientResult::Snapshot { snapshot, .. }) =
        response.result
    else {
        panic!("snapshot_get succeeds: {response:?}");
    };
    assert_eq!(
        snapshot
            .subagents
            .iter()
            .find(|view| view.subagent_id == wired.accepted.subagent_id)
            .expect("the child is in the snapshot")
            .observation,
        live.observation
    );

    // Cleanup: release the child, let it settle, drain both runtimes.
    release.send_replace(true);
    let settled = parent
        .plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    await_serve(wired.serve).await;
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

/// Invariant: the snapshot's execution profile is derived from the frozen
/// child specification and carries only the redacted model facts — never
/// credentials or endpoints of the frozen provider binding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_snapshot_projects_only_the_frozen_execution_profile() {
    let dir = tempfile::tempdir().expect("temp root");
    let plane = standalone_parent_plane(&dir, "conv-178-profile");
    let child = child_fixture(
        &dir,
        &ConversationId::new("conv-178-profile-child"),
        vec![answer_script("PROFILE-ANSWER")],
        ToolRegistry::new(),
        Vec::new(),
    )
    .await;
    let wired = launch_wired_child(&plane, &child, "inspect").await;

    let snapshot = plane
        .registry
        .snapshot(&wired.accepted.subagent_id)
        .expect("child record");
    let expected = SubagentExecutionProfile::from_frozen(&resolved_child_spec("conformance").model);
    assert_eq!(
        snapshot.profile.as_ref(),
        Some(&expected),
        "the profile is derived from the frozen child specification"
    );
    let serialized = serde_json::to_string(&snapshot.profile).expect("profile json");
    for secret in ["test-only-secret", "127.0.0.1"] {
        assert!(
            !serialized.contains(secret),
            "no credential or endpoint material crosses into the profile: {serialized}"
        );
    }

    // The Runtime Client view carries the same profile under the
    // `execution_profile` wire key; the obsolete bare `profile` key stays
    // retired.
    let view = crate::runtime_client::projection::subagent_view(&snapshot);
    assert_eq!(view.execution_profile, snapshot.profile);
    let wire = serde_json::to_value(&view).expect("view json");
    assert_eq!(wire["execution_profile"]["model"], "local/model");
    assert!(
        wire.get("profile").is_none(),
        "the obsolete profile key is absent: {wire}"
    );

    let settled = plane
        .registry
        .wait_until_settled(&wired.accepted.subagent_id)
        .await
        .expect("child settles");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.profile, snapshot.profile,
        "the frozen profile survives settlement unchanged"
    );
    await_serve(wired.serve).await;
    child
        .runtime
        .shutdown()
        .await
        .expect("child runtime drains");
}
