//! Subagent child composition: the preparation guard with its parent-liveness
//! authority, the frozen resource loader that never discovers resources, and
//! the child path over the shared semantic core.

use super::super::config::CurrentRuntimeConfigError;
use super::LocalConversationCore;
use super::LocalRuntimeDependencies;
use super::LocalRuntimeError;
use super::runtimes::LocalConversationRuntime;
use crate::capabilities::AgentActivation;
use crate::capabilities::CapabilityCoordinator;
use crate::capabilities::CapabilityCoordinatorConfig;
use crate::durable::ConversationStoreBinding;
use crate::durable::SqliteConversationStore;
use crate::model::session::SessionModelState;
use crate::runtime::conversation_runtime::ConversationContextConfig;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::conversation_runtime::RuntimeConversationConfig;
use crate::runtime::interaction::InteractionRoute;
use crate::runtime::resources::PreparedRuntimeResources;
use crate::runtime::resources::RuntimeResourceLoadError;
use crate::runtime::resources::RuntimeResourceLoader;
use crate::runtime::resources::RuntimeResourceSnapshot;
use crate::runtime::subagent::ResolvedSubagentTool;
use crate::runtime::subagent::child_conversation_store_path;
use crate::runtime::workflow::WorkflowOutputLatch;
use crate::runtime_client::host::RuntimeClientHost;
use crate::runtime_client::host::RuntimeClientHostConfig;
use crate::skills::SkillDiscoveryConfig;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::ToolDefinition;
use futures_util::future::BoxFuture;
use std::sync::Arc;

/// The cancellable-owned-work guard of one child preparation (Issue #145).
///
/// External capability composition is no longer a cheap synchronous
/// prelude: it may start an MCP process, negotiate a protocol, list a
/// catalog, and materialize the fingerprint-keyed uv environment of a
/// managed Python tool package (Issue #174). Two authorities must therefore
/// be able to end it *before* the child ever reaches semantic work:
///
/// ```text
/// attempt-derived cancellation   the spawn attempt that owns this child
/// parent control-channel EOF     the parent process disappeared
/// ```
///
/// Both converge on the ONE preparation cancellation signal this guard
/// owns — parent loss *is* a physical cancellation authority, not merely a
/// reason to drop a waiter — and every preparatory supervised process unit
/// of the child observes that signal, so either authority physically
/// cancels the in-flight preparation work.
pub(crate) struct ChildPreparation {
    cancellation: crate::runtime::cancellation::CancellationSignal,
    parent_lost: Option<crate::local_runtime::dispatcher::ChildControlHandle>,
    // Installed before external preparation can create physical resources.
    capability: std::sync::Mutex<Option<CapabilityCoordinator>>,
}

impl ChildPreparation {
    /// The production guard: the child's own preparation cancellation plus
    /// the control channel that is its parent-liveness authority.
    pub(crate) fn new(
        cancellation: crate::runtime::cancellation::CancellationSignal,
        parent_lost: crate::local_runtime::dispatcher::ChildControlHandle,
    ) -> Self {
        Self {
            cancellation,
            parent_lost: Some(parent_lost),
            capability: std::sync::Mutex::new(None),
        }
    }

    /// A guard with no parent-liveness authority, for compositions that are
    /// not driven by a control channel (tests and in-process fixtures).
    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        Self {
            cancellation: crate::runtime::cancellation::CancellationSignal::new(),
            parent_lost: None,
            capability: std::sync::Mutex::new(None),
        }
    }

    fn retain_capability(&self, capability: CapabilityCoordinator) {
        let mut owner = self
            .capability
            .lock()
            .expect("child preparation owner lock");
        assert!(
            owner.is_none(),
            "one capability owner per child preparation"
        );
        *owner = Some(capability);
    }

    /// Called only after composition has returned. Its physical owners have
    /// completed preparation, but their retirement proof still belongs here
    /// even when no `ConversationRuntime` could be constructed.
    pub(crate) async fn settle(&self) -> Result<(), Vec<String>> {
        self.cancellation.cancel();
        let capability = self
            .capability
            .lock()
            .expect("child preparation owner lock")
            .clone();
        match capability {
            Some(capability) => {
                capability.cancel_conversation_preparation();
                capability.drain_conversation_owned().await
            }
            // The composition has not crossed the resource-creation boundary.
            None => Ok(()),
        }
    }

    /// The one pre-commit cancellation authority of this preparation.
    ///
    /// Every cancellation-aware preparation step is built with it, so a
    /// settled preparation physically cancels its preparatory supervised
    /// units rather than merely dropping their waiters.
    pub(crate) fn cancellation(&self) -> crate::runtime::cancellation::CancellationSignal {
        self.cancellation.clone()
    }

    /// Whether a settlement authority has already won.
    ///
    /// A guarded step can complete *after* a settlement authority fired
    /// (the race is inherent), so the caller must check this before
    /// treating the step's output as publishable: once pre-commit
    /// cancellation has won, `Ready` must be impossible.
    pub(crate) fn is_settled(&self) -> bool {
        self.cancellation.is_cancelled()
            || self
                .parent_lost
                .as_ref()
                .is_some_and(crate::local_runtime::dispatcher::ChildControlHandle::parent_lost)
    }

    /// Runs one preparation step under both settlement authorities.
    ///
    /// The step is built with [`ChildPreparation::cancellation`]: it is
    /// cancellation-aware owned work that physically settles its
    /// preparatory supervised units before it returns. A settlement
    /// authority that wins therefore does **not** merely drop the step
    /// future — dropping a waiter never settles physical work — it fires
    /// the cancellation (parent loss first cancels the signal) and then
    /// awaits the step, so no MCP process, runtime probe, or uv build
    /// outlives a settled preparation. If the cancellation is observed
    /// *during* the step's final unguarded stretch, the step may still
    /// complete: the caller checks [`ChildPreparation::is_settled`] before
    /// publishing anything derived from it.
    pub(crate) async fn guard<T, F>(
        &self,
        step: F,
    ) -> Result<T, crate::capabilities::CapabilityPreparationError>
    where
        F: std::future::Future<Output = Result<T, crate::capabilities::CapabilityPreparationError>>,
    {
        let parent_lost = async {
            match &self.parent_lost {
                Some(handle) => handle.parent_lost_signal().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(step);
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {
                // Await the step's physical settlement: the step observes
                // this same signal and settles its preparatory units
                // before returning.
                let _ = step.await;
                Err(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(
                        "the spawn attempt cancelled this child before it was owned".to_owned(),
                    ),
                )
            },
            () = parent_lost => {
                // Parent control-channel loss is a physical cancellation
                // authority: fire the one preparation signal so every
                // preparatory unit is cancelled, then await the step's
                // settlement.
                self.cancellation.cancel();
                let _ = step.await;
                Err(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(
                        "the parent runtime disappeared while this child was still composing"
                            .to_owned(),
                    ),
                )
            },
            outcome = &mut step => outcome,
        }
    }
}

/// The controllable external-preparation seam of the Issue #145
/// regression tests (test builds only; in every other build the two entry
/// points below are inert and the environment variable is read by
/// nothing).
///
/// When armed, the child composition parks **inside the guarded external
/// preparation step** over a test-owned Unix socket: it announces the
/// exact boundary (`entered-external-preparation`) and then resolves
/// either by the test's `release` line or by the preparation cancellation
/// — exactly the contract of a well-behaved external preparation step (an
/// MCP connect, a uv build). The release arm is deliberately biased ahead
/// of the cancellation arm, so a test can drive the dangerous ordering —
/// cancellation committed, *then* the external step completes — and prove
/// the surrounding settlement checks make `Ready` impossible anyway.
#[cfg(test)]
const TEST_PREPARATION_GATE_ENV: &str = "RUSTX_ISSUE145_PREPARATION_GATE";

/// An in-process registration of the test preparation gate for ONE
/// specific child runtime root. In-process tests (which cannot arm a
/// process-global environment variable without poisoning concurrent
/// tests) register the gate for the exact child they compose, and gain
/// direct observability of the exact preparation cancellation signal the
/// gated step runs under — the proof handle of the Issue #145 local
/// race: Cancel consumed (signal set), then the step completes.
#[cfg(test)]
pub(crate) struct TestPreparationGate {
    hold_cancelled: bool,
    entered: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Sender<bool>,
    cancellation: std::sync::Mutex<Option<crate::runtime::cancellation::CancellationSignal>>,
}

/// The in-process gate registrations, keyed by the child runtime root.
#[cfg(test)]
static TEST_PREPARATION_GATES: std::sync::Mutex<
    Vec<(std::path::PathBuf, std::sync::Weak<TestPreparationGate>)>,
> = std::sync::Mutex::new(Vec::new());

/// Arms the test preparation gate for one child runtime root (test
/// builds only). The returned handle is the test's control and
/// observation channel; dropping it unregisters the gate.
#[cfg(test)]
pub(crate) fn arm_test_preparation_gate(
    runtime_root: &std::path::Path,
) -> std::sync::Arc<TestPreparationGate> {
    arm_preparation_gate(runtime_root, false)
}

/// Models owned preparation whose cancellation has been observed but whose
/// physical rollback still awaits the test's explicit release.
#[cfg(test)]
pub(crate) fn arm_test_preparation_drain_gate(
    runtime_root: &std::path::Path,
) -> std::sync::Arc<TestPreparationGate> {
    arm_preparation_gate(runtime_root, true)
}

#[cfg(test)]
fn arm_preparation_gate(
    runtime_root: &std::path::Path,
    hold_cancelled: bool,
) -> std::sync::Arc<TestPreparationGate> {
    let (entered, _) = tokio::sync::watch::channel(false);
    let (release, _) = tokio::sync::watch::channel(false);
    let gate = std::sync::Arc::new(TestPreparationGate {
        hold_cancelled,
        entered,
        release,
        cancellation: std::sync::Mutex::new(None),
    });
    let mut gates = TEST_PREPARATION_GATES.lock().expect("gate registry");
    gates.retain(|(root, weak)| weak.strong_count() > 0 && root != runtime_root);
    gates.push((runtime_root.to_path_buf(), std::sync::Arc::downgrade(&gate)));
    gate
}

/// The gate registered for this child runtime root, if any.
#[cfg(test)]
fn registered_preparation_gate(
    runtime_root: &std::path::Path,
) -> Option<std::sync::Arc<TestPreparationGate>> {
    TEST_PREPARATION_GATES
        .lock()
        .expect("gate registry")
        .iter()
        .find_map(|(root, weak)| (root == runtime_root).then(|| weak.upgrade()).flatten())
}

#[cfg(test)]
impl TestPreparationGate {
    /// Parks until the gated step entered: the child is provably inside
    /// external preparation.
    pub(crate) async fn entered(&self) {
        let mut entered = self.entered.subscribe();
        while !*entered.borrow_and_update() {
            entered.changed().await.expect("the gate outlives the step");
        }
    }

    /// The exact preparation cancellation signal the gated step runs
    /// under; valid once [`Self::entered`] resolved (the step captures its
    /// authority before announcing its entry).
    pub(crate) fn cancellation(&self) -> crate::runtime::cancellation::CancellationSignal {
        self.cancellation
            .lock()
            .expect("gate cancellation")
            .clone()
            .expect("the step captured its cancellation authority before entering")
    }

    /// Releases the gated step to completion (idempotent).
    ///
    /// `send_replace`, not `send`: a `watch` send drops the value when no
    /// receiver is currently subscribed, and a lost release would park the
    /// gated step forever.
    pub(crate) fn release(&self) {
        self.release.send_replace(true);
    }

    /// The gated step itself: announces the boundary, then resolves by
    /// release or cancellation — biased towards completion, exactly like
    /// the socket seam, so a settled authority still loses to a released
    /// step and the surrounding settlement checks are what must hold.
    async fn run(
        &self,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<(), crate::capabilities::CapabilityPreparationError> {
        *self.cancellation.lock().expect("gate cancellation") = Some(cancellation.clone());
        // Subscribe BEFORE announcing the entry so a release can never be
        // missed once `entered` resolved, and `send_replace` the entry
        // itself: a `watch` send drops the value when the test has not
        // subscribed yet, which would park its `entered` wait forever.
        let mut release = self.release.subscribe();
        self.entered.send_replace(true);
        tokio::select! {
            biased;
            released = release.changed() => match released {
                Ok(()) => Ok(()),
                Err(error) => Err(
                    crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                        "the test preparation gate closed without a release: {error}"
                    )),
                ),
            },
            () = cancellation.cancelled(), if !self.hold_cancelled => Err(
                crate::capabilities::CapabilityPreparationError::PreparationSettled(
                    "the preparation cancellation settled the gated external step".to_owned(),
                ),
            ),
        }
    }
}

/// Whether the test-only preparation gate is armed for this child.
#[cfg(test)]
fn preparation_gate_armed(runtime_root: &std::path::Path) -> bool {
    registered_preparation_gate(runtime_root).is_some()
        || std::env::var_os(TEST_PREPARATION_GATE_ENV).is_some()
}

/// Whether the test-only preparation gate is armed: never outside test
/// builds.
#[cfg(not(test))]
const fn preparation_gate_armed(_runtime_root: &std::path::Path) -> bool {
    false
}

/// Parks at the test preparation gate when armed (test builds only).
#[cfg(test)]
async fn run_preparation_gate_if_armed(
    runtime_root: &std::path::Path,
    cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<(), crate::capabilities::CapabilityPreparationError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    if let Some(gate) = registered_preparation_gate(runtime_root) {
        return gate.run(cancellation).await;
    }
    let Some(path) = std::env::var_os(TEST_PREPARATION_GATE_ENV) else {
        return Ok(());
    };
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|error| {
            crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                "the test preparation gate is unreachable: {error}"
            ))
        })?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(b"entered-external-preparation\n")
        .await
        .map_err(|error| {
            crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                "the test preparation gate announcement failed: {error}"
            ))
        })?;
    let mut lines = tokio::io::BufReader::new(read).lines();
    tokio::select! {
        biased;
        // Biased towards completion: when the test released the gate the
        // step completes even if the cancellation already fired, modelling
        // the racy external step the settlement checks exist for.
        line = lines.next_line() => match line {
            Ok(Some(line)) if line == "release" => Ok(()),
            other => Err(
                crate::capabilities::CapabilityPreparationError::PreparationSettled(format!(
                    "the test preparation gate closed without a release: {other:?}"
                )),
            ),
        },
        () = cancellation.cancelled() => Err(
            crate::capabilities::CapabilityPreparationError::PreparationSettled(
                "the preparation cancellation settled the gated external step".to_owned(),
            ),
        ),
    }
}

/// The inert non-test twin of the gate: external preparation runs no
/// test seam in a shipped build.
#[cfg(not(test))]
#[allow(clippy::unused_async)] // async only to match the test seam's shape
async fn run_preparation_gate_if_armed(
    _runtime_root: &std::path::Path,
    _cancellation: &crate::runtime::cancellation::CancellationSignal,
) -> Result<(), crate::capabilities::CapabilityPreparationError> {
    Ok(())
}

/// Materializes the child's frozen Skill packages and returns the catalog
/// entries with their locations remapped onto the child's own copies.
fn materialize_frozen_skills(
    spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
) -> Result<crate::skills::SkillSnapshot, LocalRuntimeError> {
    let root = spec
        .runtime_root()
        .map_err(|e| LocalRuntimeError::ToolRuntime {
            detail: e.to_string(),
        })?
        .join("skills");
    let mut entries = Vec::with_capacity(spec.resolved.skills.len());
    let roots = [
        crate::skills::SkillSource::User,
        crate::skills::SkillSource::Workspace,
    ]
    .into_iter()
    .map(|source| crate::skills::AutomaticSkillRoot {
        source,
        root: root.join(source.as_str()),
    })
    .collect();
    for skill in &spec.resolved.skills {
        let destination = root
            .join(skill.provenance.source.as_str())
            .join(skill.binding.skill_id.as_str());
        let location = crate::skills::materialization::materialize_skill(
            &skill.binding,
            &skill.source_root,
            &skill.files,
            &destination,
        )
        .map_err(|error| LocalRuntimeError::Capability {
            detail: error.to_string(),
        })?;
        let mut provenance = skill.provenance.clone();
        provenance.location.clone_from(&location);
        entries.push((
            crate::skills::SkillCatalogEntry {
                location,
                ..skill.catalog_entry.clone()
            },
            skill.binding.clone(),
            provenance,
        ));
    }
    Ok(crate::skills::SkillSnapshot::from_frozen(entries, roots))
}

/// Projects the frozen specification into the child's selected-only
/// capability materialization plan.
fn selected_capability_plan(
    spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
) -> crate::capabilities::SelectedCapabilityPlan {
    use crate::runtime::subagent::ResolvedSubagentTool;
    let mut plan = crate::capabilities::SelectedCapabilityPlan::default();
    for tool in &spec.resolved.tools {
        match tool {
            ResolvedSubagentTool::Builtin { .. } => {}
            ResolvedSubagentTool::Source {
                source_id,
                name,
                identity,
                ..
            } => plan
                .source_tools
                .push(crate::capabilities::SelectedSourceTool {
                    source_id: source_id.clone(),
                    name: name.clone(),
                    identity: identity.clone(),
                }),
        }
    }
    plan
}

/// Builds the child's exact base registry from its frozen specification.
fn subagent_child_registry(
    resolved: &crate::runtime::subagent::ResolvedSubagentSpec,
) -> Result<ToolRegistry, LocalRuntimeError> {
    // The exact parent-frozen definitions — identity, description, input
    // schema, and all three invocation-policy axes — never just their
    // names. Rebuilding a definition from a default policy table here would
    // silently substitute different semantics for the ones the invoking
    // generation admitted.
    let builtin: Vec<ToolDefinition> = resolved
        .tools
        .iter()
        .filter(|tool| matches!(tool, ResolvedSubagentTool::Builtin { .. }))
        .map(|tool| tool.definition().clone())
        .collect();
    let mut registry = ToolRegistry::new();
    crate::tools::native::register_subagent_child_tools(&mut registry, &builtin).map_err(
        |error| LocalRuntimeError::NativeTools {
            detail: format!("{error:?}"),
        },
    )?;
    Ok(registry)
}

/// The subagent child's resource loader.
///
/// A child never discovers resources: its whole generation was frozen by the
/// invoking parent generation. The loader therefore replays exactly the
/// frozen project instruction chain, instruction document, and Skill catalog
/// alongside a fresh base-only capability candidate, and reads no
/// configuration file, no `AGENTS.md` ancestor chain, and no Skill root. The
/// structure is the guarantee: there is no filesystem discovery path here to
/// be reached.
struct FrozenSubagentResourceLoader {
    resources: Arc<RuntimeResourceSnapshot>,
}

impl RuntimeResourceLoader for FrozenSubagentResourceLoader {
    fn prepare<'a>(
        &'a self,
        capability: &'a CapabilityCoordinator,
        _capture: Option<crate::local_runtime::configuration::ProspectiveSessionConfig>,
        _preparation_cancellation: &'a crate::runtime::cancellation::CancellationSignal,
    ) -> BoxFuture<'a, Result<PreparedRuntimeResources, RuntimeResourceLoadError>> {
        Box::pin(async move {
            let candidate = capability.prepare_base_only_candidate().map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot prepare base capability resources: {error}"
                ))
            })?;
            Ok(PreparedRuntimeResources::new(
                self.resources.project_context_files().to_vec(),
                self.resources.agent_profile().map(str::to_owned),
                self.resources.context_assembly().clone(),
                candidate,
            ))
        })
    }
}

impl LocalConversationCore {
    /// Composes the **subagent child** core from its typed startup
    /// specification (Issue #60).
    ///
    /// This is the same semantic stack as [`LocalConversationCore::compose`]
    /// — the real `ConversationRuntime`, Agent Loop, Context Assembly, Tool
    /// Plane, and `ModelAdapter` — with the child-specific composition
    /// differences made explicit and deny-by-construction:
    ///
    /// - the startup input is the typed [`SubagentChildSpec`], never a
    ///   current runtime configuration file: the child never opens
    ///   `rustx.toml` and never looks its own agent name up;
    /// - the base tool registry is exactly the Builtin capability set the
    ///   parent's resolution froze, registered through
    ///   [`register_subagent_child_tools`]; nothing is added, substituted,
    ///   or force-activated; recursive `subagent` is structurally
    ///   unregistrable, while `ask_user` is materialized only when the frozen
    ///   definition explicitly selected it;
    /// - the capability plane is **base-only**: no Skill discovery, no
    ///   Python/Node environments, and no MCP servers; it never opens or
    ///   creates Python package storage (Issue #81), so a broken Python
    ///   store location cannot fail child composition;
    /// - the Skill catalog is exactly the parent-resolved allowlist, handed
    ///   over by value, with progressive disclosure preserved: only catalog
    ///   metadata is frozen, never a `SKILL.md` body;
    /// - project instructions are exactly the parent-frozen chain. The
    ///   child performs **no** ancestor discovery of its own, which is what
    ///   makes the boundary correct once a child's filesystem ancestry can
    ///   differ from the parent workspace;
    /// - the definition's instruction document, composed with the
    ///   runtime-owned terminal-mode-aware final-report rule (Issue #192),
    ///   is the immutable `AgentProfile` System authority and canonical
    ///   history starts empty;
    /// - the durable authority is the stable child store beside the physical
    ///   spawn-incarnation directory, disjoint from the parent's store. The
    ///   physical root remains disposable execution/artifact state.
    ///
    /// The caller (the child driver) still owns activation: the returned
    /// core is inert until `into_subagent_child_with_route` or `into_headless`.
    ///
    /// # Errors
    ///
    /// Returns the first composition failure, exactly like the ordinary
    /// composition path.
    #[allow(clippy::too_many_lines)] // one coherent child composition pipeline
    pub(crate) async fn compose_subagent_child(
        spec: &crate::runtime::subagent::ipc::SubagentChildSpec,
        dependencies: &LocalRuntimeDependencies,
        preparation: &ChildPreparation,
    ) -> Result<Self, LocalRuntimeError> {
        // Keep the one-shot preparation state off the awaiting caller's stack.
        Box::pin(async move {
            let runtime_root = spec
                .runtime_root()
                .map_err(|e| LocalRuntimeError::ToolRuntime {
                    detail: e.to_string(),
                })?;
            let lifecycle = Arc::new(
                crate::runtime::local_storage::ProductRoot::existing(&spec.product_root)
                    .and_then(|root| {
                        crate::runtime::local_storage::ConversationAccess::existing(
                            &root,
                            child_conversation_store_path(
                                root.root(),
                                &spec.session_id,
                                &spec.child_conversation_id,
                            )
                            .parent()
                            .unwrap(),
                        )
                    })
                    .map_err(|e| LocalRuntimeError::ToolRuntime {
                        detail: e.to_string(),
                    })?,
            );
            lifecycle
                .confined(&runtime_root)
                .map_err(|e| LocalRuntimeError::ToolRuntime {
                    detail: e.to_string(),
                })?;
            // 1-4. The child's model authority, materialized from the
            // parent-frozen resolved invocation. There is deliberately no model
            // catalog step here: `rustx.toml` is mutable, and reopening it
            // would let a catalog edit between the parent's freeze and this
            // composition silently change the child's provider binding,
            // protocol, context window, output budget, reasoning semantics,
            // request parameters, compat metadata, or effective capabilities —
            // or fail to resolve a model that was valid when the child was
            // authorized. The only work done here is physical: adapter
            // construction and credential resolution through this process's own
            // ordinary credential boundary.
            let captured_credentials = crate::credentials::CredentialSnapshot::capture();
            let credentials = dependencies
                .credentials
                .as_deref()
                .unwrap_or(&captured_credentials);
            let model = SessionModelState::frozen(&spec.resolved.model, credentials)?;

            // 5-6. The child conversation tool runtime over the authoritative
            // project workspace selected by the parent and the exact
            // spawn-incarnation-private runtime root. The child authorizes no
            // environment entries. Its durable Message Ledger/Event Journal is
            // deliberately one level above that physical root: execution
            // artifacts can be discarded after settlement while the conversation
            // identity remains inspectable.
            let base_environment =
                ToolEnvironment::from_authorized(spec.resolved.environment.clone())
                    .map_err(CurrentRuntimeConfigError::Environment)
                    .map_err(LocalRuntimeError::RuntimeConfig)?;
            let mut runtime_config = crate::tools::runtime::ConversationRuntimeConfig::new(
                &spec.workspace,
                child_conversation_store_path(
                    lifecycle.root(),
                    &spec.session_id,
                    &spec.child_conversation_id,
                )
                .parent()
                .expect("child Conversation directory"),
            )
            // The child's frozen composition, handed to its materialization
            // owner, from exactly the extension set its invoking generation froze
            // into `ResolvedSubagentSpec` (Issue #259). The Todo list is rebuilt
            // from this *child conversation's* own canonical history — empty at
            // birth — which is why a child's list can never alias its parent's,
            // or a concurrent sibling's, and why no child list merges upward.
            .with_extensions(spec.resolved.extensions.clone());
            let durable_store_path = lifecycle
                .confined(&child_conversation_store_path(
                    lifecycle.root(),
                    &spec.session_id,
                    &spec.child_conversation_id,
                ))
                .map_err(|e| LocalRuntimeError::ToolRuntime {
                    detail: e.to_string(),
                })?;
            runtime_config.lifecycle = Some(lifecycle);
            runtime_config.environment = Some(base_environment.clone());
            let durable_store = Arc::new(
                SqliteConversationStore::open(
                    spec.child_conversation_id.clone(),
                    &durable_store_path,
                )
                .map_err(|error| LocalRuntimeError::ToolRuntime {
                    detail: format!(
                        "open child durable conversation store {}: {error}",
                        durable_store_path.display()
                    ),
                })?
                .with_lifecycle(runtime_config.lifecycle.as_ref().unwrap().clone()),
            );
            runtime_config.durable_binding = Some(ConversationStoreBinding::new(durable_store));
            let mut tool_runtime = ConversationToolRuntime::from_config(
                spec.child_conversation_id.clone(),
                runtime_config,
            )
            .map_err(|error| LocalRuntimeError::ToolRuntime {
                detail: format!("{error:?}"),
            })?;

            tool_runtime.uploads = Some(Arc::new(
                super::super::session::uploads::SessionUploadResolver::for_session(
                    &spec.product_root,
                    spec.session_id.clone(),
                ),
            ));

            // 7. The exact frozen Skill packages are materialized into the
            // child-private runtime root and their frozen `SkillVersionId` is
            // re-proven over the materialized bytes. Discovery is never
            // restarted, and the model-visible location is remapped onto the
            // child's own copy so the frozen identity stays authoritative even
            // once the child's filesystem ancestry differs from the parent's.
            let skills = materialize_frozen_skills(spec)?;

            // 8. The deny-by-construction base registry: exactly the Builtin
            // capabilities the parent's resolution authorized, with their exact
            // admitted definitions.
            let base_registry = subagent_child_registry(&spec.resolved)?;

            // 9. The capability plane sees exactly the frozen selected MCP
            // server bindings — never a configured set the child could widen.
            // Managed Python packages cross as ordinary frozen MCP bindings
            // (Issue #174); the child never opens Python store state itself.
            let plan = selected_capability_plan(spec);
            let mut mcp_servers: crate::tools::mcp::McpServerBindings = spec
                .resolved
                .materialization
                .sources
                .iter()
                .map(|(source, binding)| {
                    (crate::tools::mcp::source_server_id(source), binding.clone())
                })
                .collect();
            for binding in mcp_servers.values_mut() {
                binding.credentials.capture_from(credentials);
            }
            let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: tool_runtime.conversation_id().clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(base_registry),
                // The child's extension Tool surfaces, derived from the owners
                // its own tool runtime materialized above. The child never
                // rereads a role file or a configuration document to reinterpret
                // which extensions it owns.
                extension_tools: tool_runtime.extension_tool_plane_for(&spec.resolved.extensions),
                agent_activation: AgentActivation::default(),
                skill_discovery: SkillDiscoveryConfig::default(),
                mcp_servers,
                base_environment,
                environment_store_root: runtime_root.join("environments"),
            })
            .map_err(|error| LocalRuntimeError::Capability {
                detail: format!("{error:?}"),
            })?;
            preparation.retain_capability(capability.clone());
            // 10-11. Materialization. A child with no external requirement takes
            // the deterministic base-only path it always did; a child with one
            // takes the selected-only realization path, which is cancellable
            // owned work: cancellation and parent loss settle it instead of
            // finishing a long MCP connect or uv build for an owner that is
            // already gone, and every preparatory supervised unit observes the
            // one preparation cancellation authority.
            let candidate = if plan.is_empty() && !preparation_gate_armed(&runtime_root) {
                capability
                    .prepare_selected_candidate(&plan, &preparation.cancellation())
                    .await
                    .map_err(|error| LocalRuntimeError::Capability {
                        detail: format!("{error:?}"),
                    })?
            } else {
                let cancellation = preparation.cancellation();
                let step = async {
                    run_preparation_gate_if_armed(&runtime_root, &cancellation).await?;
                    capability
                        .prepare_selected_candidate(&plan, &cancellation)
                        .await
                };
                match preparation.guard(step).await {
                    Ok(candidate) => {
                        // The step may complete after a settlement authority
                        // already won (the race is inherent to the final
                        // unguarded stretch): a settled preparation must never
                        // publish a startable candidate. Retire its physical
                        // runtimes to settlement and report the settlement.
                        if preparation.is_settled() {
                            candidate.retire_uncommitted().await;
                            capability.cancel_conversation_preparation();
                            return Err(LocalRuntimeError::Capability {
                                detail: format!(
                                "{:?}",
                                crate::capabilities::CapabilityPreparationError::PreparationSettled(
                                    "a settlement authority won before the candidate completed"
                                        .to_owned(),
                                )
                            ),
                            });
                        }
                        candidate
                    }
                    Err(error) => {
                        // Settlement fires the one preparation cancellation
                        // authority, so every MCP connect owner and Python/uv
                        // build the step already spawned is physically
                        // cancelled and the guard awaited its settlement.
                        // Cancelling the coordinator's preparation root
                        // additionally covers any owner the step's own
                        // authority did not reach.
                        capability.cancel_conversation_preparation();
                        return Err(LocalRuntimeError::Capability {
                            detail: format!("{error:?}"),
                        });
                    }
                }
            };
            capability
                .commit(candidate.with_frozen_child(&spec.resolved, skills))
                .map_err(|error| LocalRuntimeError::Capability {
                    detail: format!("{error:?}"),
                })?;
            // The child's project instructions and Skill catalog are the
            // parent-frozen values, by value. `load_project_context_files` is
            // deliberately never called on this path: the child observes only
            // what its invoking generation froze. The AgentProfile authority is
            // the definition's instruction document composed with the
            // runtime-owned terminal-mode-aware final-report rule (Issue #192).
            let resources = Arc::new(
                RuntimeResourceSnapshot::new(
                    spec.resolved.generation,
                    spec.resolved.project_instructions.clone(),
                    Some(crate::runtime::subagent::compose_child_agent_profile(
                        &spec.resolved.instructions,
                        &spec.terminal,
                    )),
                    crate::context::ContextAssembly::new(),
                    capability.current_snapshot(),
                )
                .with_capability_availability(capability.availability()),
            );
            let workflow_output_latch = match &spec.terminal {
                crate::runtime::subagent::ipc::ChildTerminalMode::Normal => None,
                crate::runtime::subagent::ipc::ChildTerminalMode::WorkflowOutput {
                    output_schema,
                } => Some(Arc::new(
                    WorkflowOutputLatch::new(output_schema.clone())
                        .map_err(|detail| LocalRuntimeError::Capability { detail })?,
                )),
            };
            let workflow_output = workflow_output_latch.clone().map(
                |latch| -> Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal> { latch },
            );
            let resource_loader: Arc<dyn RuntimeResourceLoader> =
                Arc::new(FrozenSubagentResourceLoader {
                    resources: Arc::clone(&resources),
                });

            // 12-13. The one child conversation runtime. The definition's
            // instructions enter the request-time AgentProfile System section,
            // never canonical history.
            let runtime = ConversationRuntime::new(RuntimeConversationConfig {
                explicit_model: true,
                agent_id: spec.child_agent_id.clone(),
                model,
                approval_mode: spec.approval_mode,
                model_timeout_policy: spec.model_timeout_policy,
                tool_deadline_policy: spec.tool_deadline_policy,
                context: ConversationContextConfig {
                    policy: spec.context,
                    estimator: Arc::clone(&dependencies.estimator),
                    // The child materializes exactly the extension composition
                    // its invoking generation froze into `ResolvedSubagentSpec`
                    // (Issue #256). No host, project, or role document is read
                    // on this path, so nothing here can widen or reinterpret it.
                    status_engine: spec
                        .resolved
                        .extensions
                        .agent_status_engine(Arc::new(crate::context::SystemClock)),
                },
                tool_runtime: tool_runtime.clone(),
                capability: capability.clone(),
                resources,
                resource_loader,
                clock: None,
                initial_messages: Vec::new(),
                // A child runtime has no subagent registry: recursive
                // delegation is absent by construction.
                subagents: None,
                workflow_output,
            })?;

            Ok(Self {
                runtime,
                tool_runtime,
                capability,
                workflow_output: workflow_output_latch,
            })
        })
        .await
    }

    /// Finishes composition for a child with a reliable route to the parent
    /// registry. The route is installed before activation and carries only
    /// the child's already-authoritative interaction facts.
    pub(crate) fn into_subagent_child_with_route(
        self,
        activation: crate::runtime::identity::SubagentId,
        route: Arc<dyn InteractionRoute>,
    ) -> Result<LocalConversationRuntime, LocalRuntimeError> {
        let host = RuntimeClientHost::new_for_activation(
            RuntimeClientHostConfig {
                runtime: self.runtime.clone(),
                replay_limit: None,
            },
            activation,
        )?;
        let runtime = LocalConversationRuntime { core: self, host };
        runtime.runtime().install_interaction_route(route);
        runtime.runtime().gate_child_turns();
        runtime.activate();
        Ok(runtime)
    }
}

#[cfg(test)]
mod tests;
