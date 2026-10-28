//! Durable native child Agents and finite activation supervision.
//!
//! SubagentRegistry owns each stable AgentId, child ConversationId, lineage,
//! frozen admitted authority and frozen Session execution binding, plus zero or one current
//! activation. SubagentId names a finite activation, not the durable Agent.
//! Catalog/resolver freeze creation authority; later send_message calls reuse it.
//!
//! Active message admission and activation sealing use the same registry mutex.
//! Child SealRequested closes admission before the driver grants local sealing;
//! previously admitted envelopes drain in FIFO order. Inactive message admission
//! reserves one next activation. Stopping rejects transiently; unavailable
//! settlement or publication authority requires explicit proof or repair.
//!
//! Drivers alone own process handles, cancellation escalation, reap and nested
//! containment. Committed terminal publication with proven physical settlement
//! leaves the Agent inactive and resumable. Recovery owns later durable proof
//! without reopening the old activation. Workflow-owned finite children share
//! supervision primitives without becoming native durable Agents.
//!
//! Canonical child history owns content. Each final report enters parent canonical
//! inbound exactly once for its activation, adjacent to runtime-authored Agent
//! and activation correlation. The Event Journal owns execution/admission facts;
//! it is not a second child transcript or result channel.

/// Exact child ownership and existing-history read failures.
#[derive(Debug)]
pub(crate) enum SubagentTranscriptError {
    Unknown(SubagentId),
    Unavailable(String),
}

pub mod activity;
pub mod catalog;
pub mod invocation;
mod registry;
pub mod resolver;

pub(crate) mod anchors;

pub(crate) mod ipc;
pub(crate) mod physical_recovery;
pub(crate) mod process;

use std::path::{Path, PathBuf};

use base64::Engine as _;
use sha2::{Digest, Sha256};

/// Returns the durable database path of one child conversation.
///
/// The semantic child directory is stable for the lifetime of the durable
/// conversation. Its sibling `incarnation-*` directories are only physical
/// spawn namespaces and may be removed after execution settles. Keeping this
/// layout rule here gives the child runtime, the local inspection launcher,
/// and the process owner one identity-based lookup without exposing a path to
/// the Runtime Client protocol or TUI.
#[must_use]
pub(crate) fn child_conversation_store_path(
    parent_runtime_root: &Path,
    session_id: &crate::runtime::identity::SessionId,
    conversation_id: &ConversationId,
) -> PathBuf {
    parent_runtime_root
        .join("sessions")
        .join(session_id.as_str())
        .join("conversations")
        .join(conversation_id.as_str())
        .join("conversation.sqlite")
}

/// Returns the local live Runtime Client inspection endpoint of one child
/// conversation. The endpoint lives beside, but is not part of, the durable
/// conversation stores: it is disposable process routing state and disappears
/// with the child runtime. The filename is a deterministic short token rather
/// than the full conversation identity so the Unix socket stays within
/// platform pathname limits even when the stable store's identity component
/// is long. A stale socket is harmless because an inspector probes the
/// disposable liveness lease before selecting durable fallback.
#[must_use]
pub(crate) fn child_conversation_inspection_socket_path(
    parent_runtime_root: &Path,
    conversation_id: &ConversationId,
) -> PathBuf {
    let digest = Sha256::digest(conversation_id.as_str().as_bytes());
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..16]);
    parent_runtime_root.join(format!(".{token}"))
}

/// Returns the disposable local-runtime lease that marks a child conversation
/// as live while its process owns the Runtime Client projection. The lease is
/// a locked sidecar, not a durable conversation authority: the child removes
/// it on ordinary shutdown, and the OS releases its lock on abnormal death so
/// a later resolver can distinguish a stale marker from a live runtime.
#[must_use]
pub(crate) fn child_conversation_inspection_liveness_path(
    parent_runtime_root: &Path,
    session_id: &crate::runtime::identity::SessionId,
    conversation_id: &ConversationId,
) -> PathBuf {
    child_conversation_store_path(parent_runtime_root, session_id, conversation_id)
        .parent()
        .expect("a child conversation database has a semantic parent")
        .join(".inspection-live")
}

/// Child conversation identities are used as one filesystem component by the
/// local launcher. Reject separators and traversal components before turning a
/// client-supplied identity into a path.
#[must_use]
pub(crate) fn is_safe_child_conversation_component(conversation_id: &ConversationId) -> bool {
    let value = conversation_id.as_str();
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

pub use activity::{
    SubagentActivity, SubagentActivityCounters, SubagentExecutionProfile, SubagentObservation,
    SubagentWaitReason,
};
pub use catalog::{
    AgentCatalog, MAX_SUBAGENT_DEFINITIONS, MAX_SUBAGENT_EXECUTION_DEADLINE_MS,
    NamedAgentDefinition, NamedAgentDefinitionDigest, NamedAgentDefinitionError,
    SUBAGENT_DEFINITION_DIGEST_VERSION, SubagentAdmissionError, SubagentExecutionDeadline,
    SubagentExecutionDeadlineError, SubagentName, SubagentNameError,
};
pub use invocation::{
    MAX_OVERRIDE_SKILLS, MAX_OVERRIDE_TOOLS, SubagentInvocationOverride, SubagentOverrideError,
};
pub use process::SubagentSpawnPlan;
pub use registry::{
    ActivationAdmission, AgentActivationOrigin, AgentControlError, AgentMessageAccepted,
    AgentSnapshot, AgentState, AgentWaitResult, DurableAgentAuthority, MAX_AGENT_LIST_LIMIT,
    PreparedSubagent, SubagentAccepted, SubagentDurabilityFailureSink, SubagentListing,
    SubagentObserver, SubagentPhysicalSettlement, SubagentPublication, SubagentRegistry,
    SubagentRegistryConfig, SubagentSettlement, SubagentSnapshot, SubagentStartError,
    SubagentStartOutcome, SubagentStartSpec, SubagentState, SubagentSteerError,
    SubagentTerminalMode,
};
pub(crate) use registry::{
    AgentObservationValue, ChildPendingHeadProof, InteractionPublicationAuthority,
};
#[cfg(test)]
pub(crate) use registry::{CommitBoundaryHook, ResumeTestGates, TerminalAuthorityHook};
pub use resolver::{
    ResolvedSubagentSkill, ResolvedSubagentSpec, ResolvedSubagentTool,
    SUBAGENT_EXECUTION_PROFILE_DIGEST_VERSION, SubagentExecutionProfileDigest, SubagentResolution,
    SubagentResolutionError, SubagentResolver,
};

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::durable::inbox::InboundDraft;
use crate::events::types::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, SubagentOwnershipKind,
    SubagentTerminalState,
};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::identity::{AgentId, ConversationId, EventId, MessageId, SubagentId};
use crate::runtime::types::ApprovalMode;

/// The attempt-scoped subagent resolution view (Issue #144).
///
/// ```text
/// AgentExecution
///   owns Arc<RuntimeResourceSnapshot Rn>
///         |
///         v
/// ToolExecutionContext::with_subagent_context(...)
///         |
///         v
/// SubagentExecutor
///         |
///         v
/// SubagentResolver(Rn, agent)
/// ```
///
/// The invoking attempt hands the executor exactly the generation it was
/// admitted with, plus the model authority frozen at that same admission
/// boundary. The registered executor therefore never reads mutable
/// runtime-current resources, so this ordering is impossible:
///
/// ```text
/// attempt admitted under R1
/// reload commits R2
/// same attempt calls subagent
/// executor reads current R2          <- generation tearing; ruled out
/// ```
///
/// The view exposes only what resolution genuinely requires. It is not a
/// runtime handle and grants no ability to observe, mutate, or reload
/// runtime state.
///
/// The view is one shared `Arc`: it is cloned into every foreground
/// invocation of the attempt, so the frozen generation and model authority
/// are shared rather than copied into each execution future.
#[derive(Clone)]
pub struct NativeExecutionContext {
    inner: Arc<NativeExecutionContextInner>,
    pub(crate) native: Option<Arc<crate::tools::invocation::NativeInvocationServices>>,
    pub(crate) workflow_model: Option<crate::runtime::workflow::model::WorkflowModelCapability>,
    workflow_deadline: Option<Arc<NativeWorkflowDeadline>>,
}

/// Execution policies captured by the parent Attempt. Child preparation may
/// run after publication, so the registry must never supply these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InheritedExecutionPolicy {
    pub model_timeout: crate::model::ModelTimeoutPolicy,
    pub tool_deadline: crate::tools::deadline::ToolExecutionDeadlinePolicy,
    pub context: crate::context::SessionContextPolicy,
}

#[cfg(test)]
impl Default for InheritedExecutionPolicy {
    fn default() -> Self {
        Self {
            model_timeout: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: crate::local_runtime::config::ContextPolicyDocument::default().to_policy(),
        }
    }
}

struct NativeWorkflowDeadline {
    clock: Arc<dyn crate::runtime::MonotonicClock>,
    due: u64,
    trigger: Arc<crate::runtime::cancellation::ExecutionCancellationTrigger>,
}
struct NativeExecutionContextInner {
    capability: crate::capabilities::CapabilityCoordinator,
    origin: crate::runtime::workflow::WorkflowRunOrigin,
    resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
    model: crate::model::session::SessionModelConfig,
    models: crate::model::invocation::ModelBindingRegistry,
    approval_mode: ApprovalMode,
    policy: InheritedExecutionPolicy,
}

impl core::fmt::Debug for NativeExecutionContext {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("NativeExecutionContext")
            .field("resource_revision", &self.inner.resources.revision())
            .field("agents", &self.inner.resources.subagents().names())
            .finish_non_exhaustive()
    }
}

impl NativeExecutionContext {
    #[cfg(test)]
    pub(crate) fn with_test_policy(mut self, policy: InheritedExecutionPolicy) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("unique test context")
            .policy = policy;
        self
    }

    #[cfg(test)]
    pub(crate) fn test_context(
        attempt_id: crate::runtime::identity::AttemptId,
        resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        model: crate::model::session::SessionModelConfig,
        models: crate::model::invocation::ModelBindingRegistry,
        approval_mode: ApprovalMode,
    ) -> Self {
        let snapshot = resources.capability();
        let registry = crate::tools::executor::ToolRegistry::from_registrations(
            snapshot.available_tools().registrations().to_vec(),
        )
        .unwrap();
        let capability = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: snapshot.conversation_id().clone(),
                workspace: crate::tools::workspace::Workspace::new(snapshot.workspace_root())
                    .unwrap(),
                environment_store_root: snapshot
                    .workspace_root()
                    .parent()
                    .unwrap()
                    .join(".test-environments"),
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                base_tool_registry: Arc::new(registry),
                extension_tools: crate::extensions::ExtensionToolPlane::none(),
                agent_activation: crate::capabilities::AgentActivation::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: snapshot.mcp_servers().clone(),
                base_environment: snapshot.effective_environment().clone(),
            },
        )
        .unwrap();
        Self::new(
            capability,
            attempt_id,
            resources,
            model,
            models,
            approval_mode,
            InheritedExecutionPolicy::default(),
        )
    }

    /// Binds one attempt's immutable generation and frozen model authority.
    ///
    /// `model` must be the invoking attempt's **frozen effective** model
    /// configuration — the configuration captured under the same admission
    /// linearization that froze `resources` — never live mutable session
    /// state and never a composition-time capture.
    #[must_use]
    pub fn new(
        capability: crate::capabilities::CapabilityCoordinator,
        attempt_id: crate::runtime::identity::AttemptId,
        resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        model: crate::model::session::SessionModelConfig,
        models: crate::model::invocation::ModelBindingRegistry,
        approval_mode: ApprovalMode,
        policy: InheritedExecutionPolicy,
    ) -> Self {
        Self {
            inner: Arc::new(NativeExecutionContextInner {
                capability,
                origin: attempt_id.into(),
                resources,
                model,
                models,
                approval_mode,
                policy,
            }),
            native: None,
            workflow_model: None,
            workflow_deadline: None,
        }
    }

    /// The native admitted attempt that owns this frozen execution context.
    #[must_use]
    pub fn attempt_id(&self) -> Option<&crate::runtime::identity::AttemptId> {
        self.inner.origin.attempt_id()
    }

    pub(crate) fn scheduled(
        capability: crate::capabilities::CapabilityCoordinator,
        start_id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        model: crate::model::session::SessionModelConfig,
        models: crate::model::invocation::ModelBindingRegistry,
        approval_mode: ApprovalMode,
        policy: InheritedExecutionPolicy,
    ) -> Self {
        Self {
            inner: Arc::new(NativeExecutionContextInner {
                capability,
                origin: crate::runtime::workflow::WorkflowRunOrigin::Scheduled { start_id },
                resources,
                model,
                models,
                approval_mode,
                policy,
            }),
            native: None,
            workflow_model: None,
            workflow_deadline: None,
        }
    }

    pub(crate) fn with_workflow_deadline(
        mut self,
        clock: Arc<dyn crate::runtime::MonotonicClock>,
        due: u64,
        trigger: Arc<crate::runtime::cancellation::ExecutionCancellationTrigger>,
    ) -> Self {
        self.workflow_deadline = Some(Arc::new(NativeWorkflowDeadline {
            clock,
            due,
            trigger,
        }));
        self
    }
    pub(crate) fn check_workflow_deadline(
        &self,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) {
        if !cancellation.is_cancelled()
            && let Some(deadline) = &self.workflow_deadline
            && deadline.clock.now_millis() >= deadline.due
        {
            deadline
                .trigger
                .cancel(crate::tools::deadline::ToolCancellationCause::Deadline(
                    crate::tools::deadline::ToolDeadlineKind::Hard,
                ));
        }
    }
    pub(crate) fn origin(&self) -> &crate::runtime::workflow::WorkflowRunOrigin {
        &self.inner.origin
    }
    pub(crate) fn prepare_schedule_workflow(
        &self,
        binding: crate::scheduler::ScheduleBinding,
        id: &crate::runtime::workflow::WorkflowId,
        args: &serde_json::Value,
    ) -> Result<crate::runtime::workflow::scheduled::PinnedScheduledWorkflow, String> {
        let preparation = crate::runtime::workflow::scheduled::ScheduledWorkflowPreparation::new(
            binding,
            self.inner.resources.clone(),
            self.inner.model.clone(),
        )
        .map_err(|e| e.to_string())?
        .with_approval_mode(self.inner.approval_mode);
        preparation
            .prepare(id, args)
            .map(|prepared| prepared.contract().clone())
            .map_err(|e| e.to_string())
    }

    /// The immutable runtime resource generation the invoking attempt owns.
    #[must_use]
    pub fn resources(&self) -> &Arc<crate::runtime::resources::RuntimeResourceSnapshot> {
        &self.inner.resources
    }

    /// The effective approval mode frozen with the invoking Agent attempt.
    ///
    /// This value changes only whether an already-authorized child Tool enters
    /// the native approval rendezvous. It does not alter the child's resolved
    /// capability set, execution mode, concurrency, or interaction authority.
    #[must_use]
    pub fn approval_mode(&self) -> ApprovalMode {
        self.inner.approval_mode
    }

    /// The immutable parent policy inherited by every derived child.
    #[must_use]
    pub fn execution_policy(&self) -> InheritedExecutionPolicy {
        self.inner.policy
    }

    /// Binds the existing Conversation owner without granting generic Tools a
    /// model API. All model selection still uses this Attempt's frozen view.
    pub(crate) fn with_workflow_models(
        mut self,
        executor: crate::runtime::auxiliary::executor::AuxiliaryExecutor,
    ) -> Self {
        self.workflow_model =
            Some(crate::runtime::workflow::model::WorkflowModelCapability::new(executor));
        self
    }

    pub(crate) fn resolve_workflow_model(
        &self,
        policy: &crate::model::AuxiliaryModelPolicy,
    ) -> Result<crate::model::ResolvedModelInvocation, crate::model::ModelInvocationError> {
        self.inner.models.resolve(
            &policy
                .selection()
                .unwrap_or_else(|| self.inner.model.selection()),
        )
    }

    /// Resolves one named agent, with an optional **model-generated**
    /// invocation override, against exactly this attempt's generation.
    ///
    /// The authority mode is fixed here, by the launch site, and is not
    /// reachable from the model's arguments. Root authorizes the named identity;
    /// its Tool and Plugin selections never impose a ceiling on the child profile.
    ///
    /// # Errors
    ///
    /// Returns the first typed [`SubagentResolutionError`].
    pub fn resolve(
        &self,
        agent: &SubagentName,
        invocation: Option<&SubagentInvocationOverride>,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        SubagentResolver::resolve(&SubagentResolution {
            resources: &self.inner.resources,
            agent,
            attempt_model: &self.inner.model,
            models: &self.inner.models,

            invocation,
        })
    }

    /// Prepare only the finite sources selected by this admitted child.
    pub(crate) async fn resolve_admitted(
        &self,
        agent: &SubagentName,
        invocation: Option<&SubagentInvocationOverride>,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        let resources = &self.inner.resources;
        let definition = resources
            .subagents()
            .get(agent)
            .filter(|_| resources.delegatable_agents().contains(agent))
            .ok_or_else(|| SubagentResolutionError::UnknownAgent {
                agent: agent.to_string(),
                available: resources
                    .delegatable_agents()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            })?;
        let invocation_default = SubagentInvocationOverride::default();
        let intent = invocation.unwrap_or(&invocation_default);
        intent
            .validate_spelling()
            .map_err(|error| SubagentResolutionError::InvalidOverride { error })?;
        let sources = intent
            .effective_tools(definition)
            .iter()
            .filter_map(|selector| selector.source().cloned())
            .collect();
        let candidate = self
            .inner
            .capability
            .prepare_admitted_sources(resources, sources, cancellation)
            .await
            .map_err(|error| SubagentResolutionError::Preparation {
                detail: error.to_string(),
            })?;
        let admitted = resources.as_ref().clone().with_admitted_capability(
            candidate.admitted_snapshot(resources.capability()),
            candidate.availability().clone(),
        );
        let resolved = SubagentResolver::resolve(&SubagentResolution {
            resources: &admitted,
            agent,
            attempt_model: &self.inner.model,
            models: &self.inner.models,
            invocation,
        });
        candidate.retire_uncommitted().await;
        resolved
    }

    /// Bind attempt model inputs to previously admitted static child capabilities.
    pub(crate) fn bind_workflow_agent(
        &self,
        agent: &resolver::FrozenAgentComposition,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        agent.bind(&self.inner.resources, &self.inner.model, &self.inner.models)
    }

    #[cfg(test)]
    pub(crate) fn with_test_workflow(&self, program: &super::workflow::WorkflowProgram) -> Self {
        let mut catalog = super::workflow::WorkflowCatalog::new([program.clone()]).unwrap();
        let resources = self.resources();
        catalog.admit(
            resources.capability().available_tools(),
            resources.capability_availability(),
            resources.capability().skills(),
            resources.subagents(),
            resources.capability().mcp_servers(),
        );
        Self {
            inner: Arc::new(NativeExecutionContextInner {
                capability: self.inner.capability.clone(),
                origin: self.inner.origin.clone(),
                resources: Arc::new((**resources).clone().with_workflow_catalog(catalog)),
                model: self.inner.model.clone(),
                models: self.inner.models.clone(),
                approval_mode: self.inner.approval_mode,
                policy: self.inner.policy,
            }),
            native: self.native.clone(),
            workflow_model: self.workflow_model.clone(),
            workflow_deadline: self.workflow_deadline.clone(),
        }
    }

    /// The bounded model-facing routing catalog of this generation.
    #[must_use]
    pub(crate) fn routing_description(&self) -> String {
        let catalog = self
            .inner
            .resources
            .subagents()
            .admitted(self.inner.resources.delegatable_agents())
            .unwrap_or_else(|_| AgentCatalog::empty());
        resolver::render_agent_routing(&catalog)
    }
}

/// The bounded result content bounds of the child/parent result path.
pub(crate) const MAX_RESULT_CONTENT_BYTES: usize = 64 * 1024;
/// The bounded delegated-task size.
pub(crate) const MAX_TASK_BYTES: usize = 32 * 1024;
/// The bounded explicit context-package size.
pub(crate) const MAX_CONTEXT_PACKAGE_BYTES: usize = 64 * 1024;

/// The runtime-owned final-report instruction of every normal child
/// subagent child (Issue #192).
///
/// This is generic subagent execution semantics — the Subagent Final Report
/// Principle: the parent receives the child's complete final assistant
/// report and nothing else. It is owned by the runtime, composed exactly
/// once at the child instruction boundary, and never repeated in a
/// user-authored role Markdown body, never owned by a provider adapter, and
/// never rewritten per definition.
pub(crate) const SUBAGENT_FINAL_REPORT_INSTRUCTION: &str = "Your final response is the complete handoff to the parent agent. Include all findings, \
     conclusions, changes, validation results, and caveats the parent needs to continue the \
     task. Do not assume the parent can see your intermediate reasoning, tool calls, tool \
     outputs, or conversation history.";

/// Composes the child's immutable `AgentProfile` System authority from the
/// definition's instruction document (Issue #192).
///
/// The user-authored instructions are preserved exactly; the generic
/// final-report handoff rule is appended for a **normal** child activation,
/// whose final response is the whole semantic result the parent receives.
///
/// A Workflow-owned child (`workflow_output` terminal protocol) is
/// deliberately excluded: its terminal contract is the structured,
/// schema-validated `workflow_output` commit, so a free-form final-report
/// instruction would contradict the terminal protocol its Agent Loop must
/// satisfy.
pub(crate) fn compose_child_agent_profile(
    instructions: &str,
    terminal: &ipc::ChildTerminalMode,
) -> String {
    match terminal {
        ipc::ChildTerminalMode::Normal => {
            format!("{instructions}\n\n{SUBAGENT_FINAL_REPORT_INSTRUCTION}")
        }
        ipc::ChildTerminalMode::WorkflowOutput { .. } => instructions.to_owned(),
    }
}

/// Bounds model-generated or diagnostic text by UTF-8 bytes without ever
/// splitting a Unicode scalar value.
///
/// The subagent wire and durable-publication contracts are byte bounds. The
/// greatest character boundary at or below `max_bytes` is therefore the
/// deterministic truncation point.
pub(crate) fn bound_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

/// The one canonical durable event identity of a subagent ownership fact
/// (Issue #60).
///
/// A `SubagentOwnershipCommitted` fact has exactly one deterministic
/// `EventId`, derived from the very `SubagentId` embedded in its payload:
/// `subagent-committed-event:{subagent_id}`. The durable authority enforces
/// this binding at write time and revalidates it at read/terminal-validation
/// time, so a mismatched `EventId`/`SubagentId` pair can never enter
/// durable authority and a terminal can never resolve an ownership fact
/// that does not belong to the requested child.
pub(crate) fn subagent_ownership_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-committed-event:{subagent_id}"))
}

/// The durable ownership fact of one subagent child (Issue #60).
///
/// The fact carries exactly the identity a restart needs — the subagent,
/// the child agent/conversation it owns, the delegating tool call, and the
/// frozen `(agent, definition_digest, profile_digest)` identity — never the
/// delegated task content, the process id, or any other process-local state.
/// Its event identity is the canonical [`subagent_ownership_event_id`] of the
/// embedded `SubagentId`.
///
/// The two digests are what make the fact self-describing across a reload,
/// and they are separate identities:
///
/// ```text
/// definition_digest  the SOURCE definition the child started with, so a
///                    later generation that redefines the same agent name
///                    cannot make an already-committed child appear to have
///                    the new definition
/// profile_digest     the EFFECTIVE execution profile the child started
///                    with, so two children of one role that an authorized
///                    invocation override specialized differently stay
///                    distinguishable after a restart
/// ```
///
/// Both are committed here, at the ownership boundary, from the frozen
/// specification. Neither is ever reconstructed from current resources.
#[allow(clippy::too_many_arguments)] // one durable fact, one construction boundary
pub(crate) fn ownership_event(
    parent_agent_id: &AgentId,
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    child_conversation_id: &ConversationId,
    origin: &AgentActivationOrigin,
    agent: &SubagentName,
    definition_digest: &NamedAgentDefinitionDigest,
    profile_digest: &resolver::SubagentExecutionProfileDigest,
    ownership: SubagentOwnershipKind,
    workspace: &std::path::Path,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: subagent_ownership_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: parent_agent_id.clone(),
            admitted_authority: None,
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            child_conversation_id: child_conversation_id.clone(),
            origin: origin.clone(),
            agent: agent.as_str().to_owned(),
            definition_digest: definition_digest.as_str().to_owned(),
            profile_digest: profile_digest.as_str().to_owned(),
            ownership,
            workspace: workspace.to_path_buf(),
        },
    }
}

/// The one producer correlation of a subagent terminal publication.
///
/// The live settlement path and startup recovery share this exact key, so
/// an ambiguous commit observed as an error resolves as an idempotent
/// retry and the two paths can never publish twice.
pub(crate) fn terminal_correlation(subagent_id: &SubagentId) -> String {
    format!("subagent-terminal:{subagent_id}")
}

/// The deterministic message identity of a subagent terminal publication.
pub(crate) fn terminal_message_id(subagent_id: &SubagentId) -> MessageId {
    MessageId::new(format!("subagent-{subagent_id}-terminal"))
}

/// The deterministic message identity of the runtime-authored terminal
/// notice accompanying a successful terminal publication (Issue #192).
pub(crate) fn terminal_notice_message_id(subagent_id: &SubagentId) -> MessageId {
    MessageId::new(format!("subagent-{subagent_id}-terminal-notice"))
}

/// The one producer correlation of a successful terminal's runtime-authored
/// notice. It shares the terminal publication's exactly-once durable
/// transaction, so the bounded retry rebuilds it byte-identically and an
/// ambiguous commit resolves as the idempotent correlation retry.
pub(crate) fn terminal_notice_correlation(subagent_id: &SubagentId) -> String {
    format!("subagent-terminal-notice:{subagent_id}")
}

/// Runtime-owned correlation for one final report. Stable Agent identity and
/// finite activation identity are explicit and never supplied by child content.
/// The store validates this exact notice in the same transaction as the report.
pub(crate) fn terminal_notice_text(
    agent_id: &AgentId,
    subagent_id: &SubagentId,
    agent: &str,
) -> String {
    format!(
        "Agent {agent_id} activation {subagent_id} (profile \"{agent}\") completed; the message that follows \
         is its final report."
    )
}

/// The runtime-authored adjacent notice of a successful normal child
/// terminal (Issue #192).
///
/// The success report itself is child-authored, so the runtime correlation
/// metadata must not be concatenated into it — that would falsely attribute
/// runtime statements to the child. The notice is a separate
/// `UserSource::Runtime` inbound item committed in the **same** durable
/// transaction as the terminal publication, ordered strictly before the
/// report: the terminal result remains the last item of the publication.
pub(crate) fn terminal_notice(
    agent_id: &AgentId,
    subagent_id: &SubagentId,
    agent: &SubagentName,
    timestamp: DateTime<Utc>,
) -> InboundDraft {
    InboundDraft {
        message_id: Some(terminal_notice_message_id(subagent_id)),
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: terminal_notice_text(agent_id, subagent_id, agent.as_str()),
        })],
        timestamp,
        correlation: Some(terminal_notice_correlation(subagent_id)),
    }
}

/// The deterministic event identity of a subagent terminal publication.
pub(crate) fn terminal_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-terminal-event:{subagent_id}"))
}

/// The deterministic event identity of a Workflow-owned child terminal
/// settlement. This is a lifecycle fact only: unlike a normal subagent
/// terminal publication it has no parent message or delivery correlation.
pub(crate) fn terminal_settlement_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-terminal-settled-event:{subagent_id}"))
}

pub(crate) fn admission_event_id(
    activation_id: &SubagentId,
    phase: &crate::events::types::AgentActivationAdmissionPhase,
) -> EventId {
    let phase = match phase {
        crate::events::types::AgentActivationAdmissionPhase::Reserved => "reserved",
        crate::events::types::AgentActivationAdmissionPhase::RolledBack {
            physical_settlement_proven: false,
        } => "rollback-unproven",
        crate::events::types::AgentActivationAdmissionPhase::RolledBack {
            physical_settlement_proven: true,
        } => "rolled-back",
    };
    EventId::new(format!("agent-admission:{activation_id}:{phase}"))
}

pub(crate) fn admission_event(
    conversation_id: &ConversationId,
    agent_id: &AgentId,
    activation_id: &SubagentId,
    origin: &AgentActivationOrigin,
    phase: crate::events::types::AgentActivationAdmissionPhase,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: admission_event_id(activation_id, &phase),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::AgentActivationAdmission {
            agent_id: agent_id.clone(),
            activation_id: activation_id.clone(),
            origin: origin.clone(),
            phase,
        },
    }
}

/// The deterministic event identity of a committed Workflow Agent value.
/// This fact is committed in the same transaction as the corresponding
/// `SubagentTerminalSettled` lifecycle fact.
pub(crate) fn physical_settlement_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-physical-settlement:{subagent_id}"))
}

pub(crate) fn physical_settlement_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: physical_settlement_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentPhysicalSettlementProven {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
        },
    }
}

pub(crate) fn workflow_output_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("workflow-agent-output-event:{subagent_id}"))
}

/// Builds the direct terminal-settlement fact used by a Workflow-owned child.
/// The result is consumed by `WorkflowRuntime` through the native registry, so
/// no parent inbound item is created and no notification-delivery phase is
/// introduced.
pub(crate) fn terminal_settlement(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    physical_settlement_proven: bool,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: terminal_settlement_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentTerminalSettled {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            state,
            physical_settlement_proven,
        },
    }
}

/// Builds the durable Workflow Agent value fact. The value is execution
/// evidence for the immutable `WorkflowRun` snapshot; it is not canonical
/// parent conversation content and never instructs recovery to replay a run.
#[allow(clippy::too_many_arguments)] // one typed Workflow terminal fact
pub(crate) fn workflow_output_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    workflow_id: &crate::runtime::workflow::WorkflowId,
    run_id: &crate::runtime::workflow::WorkflowRunId,
    node_id: &crate::runtime::workflow::WorkflowNodeInstance,
    output: serde_json::Value,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: workflow_output_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::WorkflowAgentOutputCommitted {
            workflow_id: workflow_id.clone(),
            run_id: run_id.clone(),
            node_id: node_id.to_owned(),
            subagent_id: subagent_id.clone(),
            output,
        },
    }
}

/// Builds the terminal publication pair: the inbound draft (exactly-once
/// correlated) and the dependent durable terminal fact, committed together
/// through the narrow `accept_subagent_terminal` transition — the one
/// durable authority for every normal `SubagentTerminalPublished` fact
/// (Issue #192).
#[allow(clippy::too_many_arguments)] // One terminal publication carries exact identity, content and physical proof.
pub(crate) fn terminal_publication(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    content: Vec<UserContentBlock>,
    physical_settlement_proven: bool,
    timestamp: DateTime<Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    debug_assert!(
        matches!(state, SubagentTerminalState::Succeeded)
            == matches!(
                content_source(state, child_agent_id),
                UserSource::Agent { .. }
            ),
        "a successful terminal is authored by the child agent; every other terminal is a runtime notice"
    );
    let message = UserMessageBlock {
        id: terminal_message_id(subagent_id),
        content,
        source: content_source(state, child_agent_id),
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    };
    let event = RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: terminal_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentTerminalPublished {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            message_id: message.id.clone(),
            state,
            physical_settlement_proven,
        },
    };
    let draft = InboundDraft {
        message_id: Some(message.id.clone()),
        source: message.source,
        kind: message.kind,
        content: message.content,
        timestamp,
        correlation: Some(terminal_correlation(subagent_id)),
    };
    (draft, event)
}

/// The provenance of one terminal publication: a successful answer is
/// authored by the child agent; every other terminal is the runtime
/// speaking about the child.
fn content_source(state: SubagentTerminalState, child_agent_id: &AgentId) -> UserSource {
    match state {
        SubagentTerminalState::Succeeded => UserSource::Agent {
            agent_id: child_agent_id.clone(),
        },
        SubagentTerminalState::Failed
        | SubagentTerminalState::Cancelled
        | SubagentTerminalState::Interrupted => UserSource::Runtime,
    }
}

/// The runtime-authored terminal publication of one subagent child whose
/// process/IPC outcome is unknown (live physical loss or restart recovery):
/// a bounded notice with the [`SubagentTerminalState::Interrupted`] fact.
///
/// The identity contract is deliberately identical to the live settlement
/// path — the same `MessageId` and the same producer correlation — so a
/// live publication and a recovery publication are mutually exclusive by
/// construction. Nothing is relaunched and no old process is reattached.
#[must_use]
pub fn recovery_terminal_publication(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    agent: &str,
    definition_digest: &str,
    timestamp: DateTime<Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    terminal_publication(
        conversation_id,
        subagent_id,
        child_agent_id,
        SubagentTerminalState::Interrupted,
        vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Subagent {subagent_id} (agent {agent}, definition {definition_digest}) was \
                 interrupted by a runtime restart: its actual outcome is unknown and it was \
                 not restarted."
            ),
        })],
        false,
        timestamp,
    )
}

#[cfg(test)]
mod tests {
    use super::bound_utf8;
    use super::*;

    /// The recovery-authored interruption notice preserves the "actual
    /// outcome unknown" semantics as a Runtime-authored message (Issue #192).
    #[test]
    fn the_recovery_interruption_notice_is_runtime_authored() {
        let subagent_id = SubagentId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-1");
        let child_agent_id = AgentId::new("agent-child");
        let conversation_id = ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
        let timestamp = chrono::Utc::now();
        let (draft, event) = super::recovery_terminal_publication(
            &conversation_id,
            &subagent_id,
            &child_agent_id,
            "explore",
            "sha256:d1",
            timestamp,
        );
        assert_eq!(draft.source, UserSource::Runtime);
        let text = match &draft.content[0] {
            UserContentBlock::Text(text) => text.text.clone(),
            other => panic!("the notice is text: {other:?}"),
        };
        assert!(
            text.contains("its actual outcome is unknown and it was not restarted"),
            "interruption never becomes a proven failure or cancellation: {text}"
        );
        assert!(!text.contains("retained"), "{text}");
        assert!(matches!(
            event.event,
            RuntimeEvent::SubagentTerminalPublished {
                state: SubagentTerminalState::Interrupted,
                physical_settlement_proven: false,
                ..
            }
        ));
    }

    /// The successful-terminal notice correlates the exact typed execution
    /// handle the creation result returned, plus the named agent — and no
    /// internal child identity (Issue #192).
    #[test]
    fn the_terminal_notice_correlates_the_exact_execution_handle() {
        let subagent_id = SubagentId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-2");
        let plain =
            super::terminal_notice_text(&AgentId::new("agent-child"), &subagent_id, "explore");
        assert_eq!(
            plain,
            "Agent agent-child activation conv_36524fd8-f674-7fc2-8125-06d01fee0e18-subagent-2 \
             (profile \"explore\") completed; the message that follows is its final report."
        );

        let agent = SubagentName::parse("explore").expect("name");
        let timestamp = chrono::Utc::now();
        let draft = super::terminal_notice(
            &AgentId::new("agent-child"),
            &subagent_id,
            &agent,
            timestamp,
        );
        assert_eq!(draft.source, UserSource::Runtime);
        assert_eq!(draft.kind, InboundKind::Message);
        assert_eq!(
            draft.message_id,
            Some(super::terminal_notice_message_id(&subagent_id))
        );
        assert_eq!(
            draft.correlation,
            Some(super::terminal_notice_correlation(&subagent_id))
        );
        let text = match &draft.content[0] {
            UserContentBlock::Text(text) => text.text.clone(),
            other => panic!("the notice is text: {other:?}"),
        };
        for internal in [
            "child_agent_id",
            "child_conversation_id",
            "definition_digest",
            "tool_call_id",
            "sha256:",
        ] {
            assert!(
                !text.contains(internal),
                "no internal child identity crosses into the notice: {internal} in {text}"
            );
        }
    }

    #[test]
    fn utf8_bounds_are_byte_caps_at_character_boundaries() {
        let chinese = "界".repeat(32);
        let chinese_bound = bound_utf8(chinese.clone(), 65);
        assert!(chinese_bound.len() <= 65);
        assert_eq!(chinese_bound.len() % "界".len(), 0);
        assert_eq!(chinese_bound, "界".repeat(21));

        let emoji = "🙂".repeat(20);
        let emoji_bound = bound_utf8(emoji.clone(), 65);
        assert!(emoji_bound.len() <= 65);
        assert_eq!(emoji_bound.len() % "🙂".len(), 0);
        assert_eq!(emoji_bound, "🙂".repeat(16));

        assert_eq!(bound_utf8("ascii".to_owned(), 5), "ascii");
        assert_eq!(bound_utf8("ascii".to_owned(), 4), "asci");
        assert_eq!(bound_utf8("short🙂".to_owned(), 64), "short🙂");
    }
}
