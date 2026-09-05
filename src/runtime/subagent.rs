//! The conversation-owned asynchronous one-shot subagent plane (Issue #60).
//!
//! A rustX v1 subagent is a **conversation-owned, asynchronous, one-shot,
//! separate-OS-process child rustX runtime**. The child reuses the real
//! rustX stack — `ConversationRuntime`, the Agent Loop, Context Assembly,
//! the Tool Plane, and the ModelAdapter — headlessly, with the exact
//! capability set frozen by its named definition and an isolated
//! conversation.
//!
//! # Ownership
//!
//! ```text
//! SubagentCatalog (catalog)
//!   owns: the immutable named definitions of one runtime resource
//!         generation and their deterministic definition digests
//!   never owns: live execution state of any kind
//!
//! SubagentResolver (resolver)
//!   owns: definition + invoking RuntimeResourceSnapshot + invoking attempt
//!         model authority -> frozen ResolvedSubagentSpec
//!   never owns: the parent's active ToolRegistry, mutable runtime-current
//!               resources, live child lifecycle
//!
//! SubagentRegistry (registry)
//!   owns: SubagentId allocation/correlation, child identity correlation,
//!         committed (agent, definition_digest) identity, logical lifecycle,
//!         ownership state, capacity, cancellation intent, terminal
//!         metadata, bounded result metadata, and each owned child's
//!         whole-lifecycle execution deadline
//!   never owns: configuration/definition semantics, parent Ledger/Surface,
//!               parent InboundSequence allocation, parent AgentExecution
//!               admission, a private result queue, the OS process handle
//!
//! subagent process driver (subagent_process)
//!   owns: spawn, the OS child handle, the control channel, signal
//!         escalation, wait/reap, physical terminal proof, and the
//!         retained nested process-unit anchors of that child (Issue #145)
//!   never owns: canonical conversation state, lifecycle terminality
//! ```
//!
//! # Nested process-unit anchors (Issue #145)
//!
//! A child that runs Bash, MCP stdio, Python/uv, or Skill environment work
//! creates supervised units whose inner `setsid()` group is outside the
//! child's own process group, so killing that group cannot reach them. Each
//! such unit offers its containment anchor to this process and may not cross
//! its local `START` gate until it is acknowledged; see
//! [`anchors`] for the parent half and
//! [`crate::runtime::nested_containment`] for the generic mechanism.
//!
//! Anchor ownership follows child ownership exactly:
//!
//! ```text
//! StagedChild   direct child process + retained anchors
//!      |  exactly-once move at the ownership commit
//!      v
//! child driver task
//! ```
//!
//! and a direct child reap is not proof of physical settlement while any
//! retained anchor is unresolved.
//!
//! # Message-bus invariant
//!
//! A subagent never writes another conversation's canonical history and
//! never schedules another conversation's attempt directly. The delegated
//! task enters the child through the child's ordinary durable inbound
//! path (`UserSource::Agent(parent)`); the child's bounded result enters
//! the parent through the parent's ordinary durable inbound acceptance
//! (`UserSource::Agent(child)` on success, `UserSource::Runtime` for
//! failure/cancellation/interruption notices). Child-process IPC only
//! transports bounded envelopes and control.

pub mod activity;
pub mod catalog;
mod registry;
pub mod resolver;
pub mod workspace;

pub(crate) mod anchors;

pub(crate) mod ipc;
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
    conversation_id: &ConversationId,
) -> PathBuf {
    parent_runtime_root
        .join("subagents")
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
    conversation_id: &ConversationId,
) -> PathBuf {
    child_conversation_store_path(parent_runtime_root, conversation_id)
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
    CHILD_UNSAFE_BUILTIN_TOOLS, MAX_SUBAGENT_DEFINITIONS, MAX_SUBAGENT_EXECUTION_DEADLINE_MS,
    SUBAGENT_DEFINITION_DIGEST_VERSION, SubagentAdmissionError, SubagentCatalog,
    SubagentDefinition, SubagentDefinitionDigest, SubagentDefinitionError,
    SubagentExecutionDeadline, SubagentExecutionDeadlineError, SubagentName, SubagentNameError,
    SubagentProjectInstructionPolicy, SubagentToolSelector,
};
pub use process::SubagentSpawnPlan;
#[cfg(test)]
pub(crate) use registry::CommitBoundaryHook;
pub(crate) use registry::InteractionPublicationAuthority;
pub use registry::{
    PreparedSubagent, SubagentAccepted, SubagentDurabilityFailureSink, SubagentListing,
    SubagentObserver, SubagentRegistry, SubagentRegistryConfig, SubagentSnapshot,
    SubagentStartError, SubagentStartOutcome, SubagentStartSpec, SubagentState,
    SubagentTerminalMode, SubagentWorkspaceDisposal, SubagentWorkspaceDisposalError,
    SubagentWorkspaceResourceState,
};
pub use resolver::{
    ResolvedSubagentSkill, ResolvedSubagentSpec, ResolvedSubagentTool, SubagentDomain,
    SubagentResolutionError, SubagentResolver,
};
pub use workspace::{
    GitWorktreeSnapshot, SubagentWorkspaceManager, SubagentWorkspacePolicy, WorkspaceCleanup,
    WorkspaceDisposalError, WorkspaceDisposalSettlement, WorkspaceHandoff, WorkspaceIsolation,
    WorkspaceLease, WorkspaceSettlement, WorkspaceSettlementDisposition, WorkspaceSnapshot,
    WorkspaceUnresolvedReason,
};

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::durable::inbox::InboundDraft;
use crate::events::types::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, SubagentOwnershipKind,
    SubagentTerminalState, SubagentWorkspaceDisposalSettlement, SubagentWorkspaceTerminalResource,
};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::identity::{
    AgentId, ConversationId, EventId, MessageId, SubagentId, ToolCallId,
};
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
pub struct AttemptSubagentContext {
    inner: Arc<AttemptSubagentContextInner>,
}

struct AttemptSubagentContextInner {
    resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
    model: crate::model::session::SessionModelConfig,
    models: crate::model::invocation::ModelBindingRegistry,
    approval_mode: ApprovalMode,
}

impl core::fmt::Debug for AttemptSubagentContext {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AttemptSubagentContext")
            .field("resource_revision", &self.inner.resources.revision())
            .field("agents", &self.inner.resources.subagents().names())
            .finish_non_exhaustive()
    }
}

impl AttemptSubagentContext {
    /// Binds one attempt's immutable generation and frozen model authority.
    ///
    /// `model` must be the invoking attempt's **frozen effective** model
    /// configuration — the configuration captured under the same admission
    /// linearization that froze `resources` — never live mutable session
    /// state and never a composition-time capture.
    #[must_use]
    pub fn new(
        resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        model: crate::model::session::SessionModelConfig,
        models: crate::model::invocation::ModelBindingRegistry,
        approval_mode: ApprovalMode,
    ) -> Self {
        Self {
            inner: Arc::new(AttemptSubagentContextInner {
                resources,
                model,
                models,
                approval_mode,
            }),
        }
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

    /// Resolves one named agent against exactly this attempt's generation.
    ///
    /// # Errors
    ///
    /// Returns the first typed [`SubagentResolutionError`].
    pub fn resolve(
        &self,
        agent: &SubagentName,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        SubagentResolver::resolve_in_domain(
            &self.inner.resources,
            agent,
            &self.inner.model,
            &self.inner.models,
            SubagentDomain::Main,
        )
    }

    /// Resolves one named profile for a Workflow `AgentRun` using the
    /// independent Workflow admission set.
    ///
    /// # Errors
    ///
    /// Returns a [`SubagentResolutionError`] when the profile is not
    /// Workflow-admitted or its frozen resources cannot be resolved.
    pub fn resolve_workflow(
        &self,
        agent: &SubagentName,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        SubagentResolver::resolve_in_domain(
            &self.inner.resources,
            agent,
            &self.inner.model,
            &self.inner.models,
            SubagentDomain::Workflow,
        )
    }

    /// The bounded model-facing routing catalog of this generation.
    #[must_use]
    pub(crate) fn routing_description(&self) -> String {
        let catalog = self
            .inner
            .resources
            .subagents()
            .admitted(self.inner.resources.subagent_main_admission())
            .unwrap_or_else(|_| SubagentCatalog::empty());
        resolver::render_agent_routing(&catalog)
    }
}

/// The bounded result content bounds of the child/parent result path.
pub(crate) const MAX_RESULT_CONTENT_BYTES: usize = 64 * 1024;
/// The bounded delegated-task size.
pub(crate) const MAX_TASK_BYTES: usize = 32 * 1024;
/// The bounded explicit context-package size.
pub(crate) const MAX_CONTEXT_PACKAGE_BYTES: usize = 64 * 1024;

/// The runtime-owned final-report instruction of every normal one-shot
/// subagent child (Issue #192).
///
/// This is generic subagent execution semantics — the Subagent Final Report
/// Principle: the parent receives the child's complete final assistant
/// report and nothing else. It is owned by the runtime, composed exactly
/// once at the child instruction boundary, and never repeated in a
/// user-authored `instructionsFile`, never owned by a provider adapter, and
/// never rewritten per definition.
pub(crate) const SUBAGENT_FINAL_REPORT_INSTRUCTION: &str = "Your final response is the complete handoff to the parent agent. Include all findings, \
     conclusions, changes, validation results, and caveats the parent needs to continue the \
     task. Do not assume the parent can see your intermediate reasoning, tool calls, tool \
     outputs, or conversation history.";

/// Composes the child's immutable `AgentProfile` System authority from the
/// definition's instruction document (Issue #192).
///
/// The user-authored instructions are preserved exactly; the generic
/// final-report handoff rule is appended for a **normal** one-shot child,
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
/// frozen `(agent, definition_digest)` identity — never the delegated task
/// content, the process id, or any other process-local state. Its event
/// identity is the canonical [`subagent_ownership_event_id`] of the
/// embedded `SubagentId`.
///
/// The digest is what makes the fact self-describing across a reload: a
/// later generation that redefines the same agent name cannot make an
/// already-committed child appear to have the new definition, because the
/// durable fact names the exact definition the child started with.
#[allow(clippy::too_many_arguments)] // one durable fact, one construction boundary
pub(crate) fn ownership_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    child_conversation_id: &ConversationId,
    tool_call_id: &ToolCallId,
    agent: &SubagentName,
    definition_digest: &SubagentDefinitionDigest,
    ownership: SubagentOwnershipKind,
    workspace: &WorkspaceSnapshot,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: subagent_ownership_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            child_conversation_id: child_conversation_id.clone(),
            tool_call_id: tool_call_id.clone(),
            agent: agent.as_str().to_owned(),
            definition_digest: definition_digest.as_str().to_owned(),
            ownership,
            workspace: workspace.clone(),
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

/// The deterministic message identity of the runtime-authored
/// retained-workspace notice accompanying a successful terminal publication
/// (Issue #192).
pub(crate) fn terminal_notice_message_id(subagent_id: &SubagentId) -> MessageId {
    MessageId::new(format!("subagent-{subagent_id}-terminal-notice"))
}

/// The one producer correlation of a retained-workspace terminal notice. It
/// shares the terminal publication's exactly-once durable transaction, so
/// the bounded retry rebuilds it byte-identically and an ambiguous commit
/// resolves as the idempotent correlation retry.
pub(crate) fn terminal_notice_correlation(subagent_id: &SubagentId) -> String {
    format!("subagent-terminal-notice:{subagent_id}")
}

/// The minimal actionable parent-facing fact of a retained changed isolated
/// workspace (Issue #192): semantic only — never a physical path, branch,
/// or commit, which remain user/Runtime Client concerns below the model
/// boundary.
pub(crate) const RETAINED_WORKSPACE_FACT: &str = "changes were retained and are not applied to your workspace; the user can inspect or \
     dispose of the retained workspace";

/// The runtime-authored adjacent notice of a successful child whose
/// terminal settlement retained changed isolated work (Issue #192).
///
/// The success report itself is child-authored, so this runtime-observed
/// settlement fact must not be concatenated into it — that would falsely
/// attribute a runtime statement to the child, and the child cannot
/// authoritatively know terminal workspace settlement anyway. The notice is
/// a separate `UserSource::Runtime` inbound item committed in the **same**
/// durable transaction as the terminal publication, ordered strictly before
/// the report: the terminal result remains the last item of the
/// publication.
pub(crate) fn retained_workspace_notice(
    subagent_id: &SubagentId,
    agent: &SubagentName,
    timestamp: DateTime<Utc>,
) -> InboundDraft {
    InboundDraft {
        message_id: Some(terminal_notice_message_id(subagent_id)),
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Subagent {subagent_id} (agent {agent}) worked in an isolated workspace: its \
                 {RETAINED_WORKSPACE_FACT}."
            ),
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

/// The deterministic event identity of the durable retained-workspace
/// disposal intent. This fact lives after the logical child terminal event
/// and opens a separate resource lifecycle for exactly one handoff.
pub(crate) fn workspace_disposal_started_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!(
        "subagent-workspace-disposal-started-event:{subagent_id}"
    ))
}

/// The deterministic event identity of one durable retained-workspace
/// disposal settlement. Intermediate and final settlement facts have
/// distinct identities so `WorktreeRemoved` can remain open until the exact
/// branch compare-delete settles.
pub(crate) fn workspace_disposal_settled_event_id(
    subagent_id: &SubagentId,
    settlement: SubagentWorkspaceDisposalSettlement,
) -> EventId {
    let phase = match settlement {
        SubagentWorkspaceDisposalSettlement::WorktreeRemoved => "worktree-removed",
        SubagentWorkspaceDisposalSettlement::Disposed => "disposed",
    };
    EventId::new(format!(
        "subagent-workspace-disposal-{phase}-event:{subagent_id}"
    ))
}

/// Builds the durable post-terminal retained-workspace disposal intent.
/// Durable validation independently binds the handoff to the child ownership
/// and terminal facts before the intent can authorize physical mutation.
pub(crate) fn workspace_disposal_started_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    workspace_handoff: &WorkspaceHandoff,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: workspace_disposal_started_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentWorkspaceDisposalStarted {
            subagent_id: subagent_id.clone(),
            workspace_handoff: workspace_handoff.clone(),
        },
    }
}

/// Builds one durable physical settlement of an admitted retained-workspace
/// disposal. The exact handoff is repeated so each phase is self-describing;
/// durable validation compares it with the intent and terminal facts.
pub(crate) fn workspace_disposal_settled_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    workspace_handoff: &WorkspaceHandoff,
    settlement: SubagentWorkspaceDisposalSettlement,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: workspace_disposal_settled_event_id(subagent_id, settlement),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentWorkspaceDisposalSettled {
            subagent_id: subagent_id.clone(),
            workspace_handoff: workspace_handoff.clone(),
            settlement,
        },
    }
}

/// Converts the closed workspace settlement disposition into the durable
/// terminal resource fact. A preserved unresolved workspace carries its
/// reason and diagnostic, but never a fabricated `WorkspaceHandoff`.
pub(crate) fn terminal_workspace_resource(
    settlement: &WorkspaceSettlement,
) -> SubagentWorkspaceTerminalResource {
    match &settlement.disposition {
        WorkspaceSettlementDisposition::Shared | WorkspaceSettlementDisposition::Removed => {
            SubagentWorkspaceTerminalResource::None
        }
        WorkspaceSettlementDisposition::Retained { handoff, .. } => {
            SubagentWorkspaceTerminalResource::Retained {
                handoff: handoff.clone(),
            }
        }
        WorkspaceSettlementDisposition::PreservedUnresolved { reason, detail } => {
            SubagentWorkspaceTerminalResource::PreservedUnresolved {
                reason: *reason,
                detail: bound_utf8(
                    detail.clone(),
                    workspace::MAX_WORKSPACE_SETTLEMENT_DETAIL_BYTES,
                ),
            }
        }
    }
}

/// The deterministic event identity of a committed Workflow Agent value.
/// This fact is committed in the same transaction as the corresponding
/// `SubagentTerminalSettled` lifecycle fact.
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
    workspace_resource: &SubagentWorkspaceTerminalResource,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
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
            workspace_resource: workspace_resource.clone(),
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
    run_id: &ToolCallId,
    node_id: &str,
    output: serde_json::Value,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
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
/// through the narrow `accept_inbound_with_event` transition.
pub(crate) fn terminal_publication(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    content: Vec<UserContentBlock>,
    workspace_resource: &SubagentWorkspaceTerminalResource,
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
            workspace_resource: workspace_resource.clone(),
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
    workspace_resource: &SubagentWorkspaceTerminalResource,
    timestamp: DateTime<Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    // The notice is runtime-authored, so the retained-workspace fact folds
    // into the same message without any provenance ambiguity.
    let retained = matches!(
        workspace_resource,
        SubagentWorkspaceTerminalResource::Retained { .. }
    )
    .then(|| format!(" Its {RETAINED_WORKSPACE_FACT}."))
    .unwrap_or_default();
    terminal_publication(
        conversation_id,
        subagent_id,
        child_agent_id,
        SubagentTerminalState::Interrupted,
        vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Subagent {subagent_id} (agent {agent}, definition {definition_digest}) was \
                 interrupted by a runtime restart: its actual outcome is unknown and it was \
                 not restarted.{retained}"
            ),
        })],
        workspace_resource,
        timestamp,
    )
}

#[cfg(test)]
mod tests {
    use super::bound_utf8;
    use super::*;

    /// The recovery-authored interruption notice preserves the "actual
    /// outcome unknown" semantics and folds the runtime-observed retained
    /// workspace fact into the same Runtime-authored message (Issue #192).
    #[test]
    fn the_recovery_interruption_notice_is_runtime_authored_and_carries_the_retained_fact() {
        let subagent_id = SubagentId::new("conv-1-subagent-1");
        let child_agent_id = AgentId::new("agent-child");
        let conversation_id = ConversationId::new("conv-1");
        let timestamp = chrono::Utc::now();
        let handoff = crate::runtime::subagent::WorkspaceHandoff {
            logical_workspace: std::path::PathBuf::from("/physical/worktree"),
            physical_worktree_root: std::path::PathBuf::from("/physical/worktree"),
            branch: "rustx/subagent/secret-branch".to_owned(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            head_commit: "89abcdef012345670123456789abcdef01234567".to_owned(),
            dirty: true,
        };
        let retained = SubagentWorkspaceTerminalResource::Retained {
            handoff: handoff.clone(),
        };
        let (draft, event) = super::recovery_terminal_publication(
            &conversation_id,
            &subagent_id,
            &child_agent_id,
            "explore",
            "sha256:d1",
            &retained,
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
        assert!(
            text.contains("changes were retained and are not applied to your workspace"),
            "the retained fact is reported: {text}"
        );
        for physical in ["/physical/worktree", "secret-branch"] {
            assert!(
                !text.contains(physical),
                "no physical fact crosses into the notice: {physical} in {text}"
            );
        }
        assert!(matches!(
            event.event,
            RuntimeEvent::SubagentTerminalPublished {
                state: SubagentTerminalState::Interrupted,
                ..
            }
        ));

        // Without a retained settlement, no retained fact is fabricated.
        let (draft, _) = super::recovery_terminal_publication(
            &conversation_id,
            &subagent_id,
            &child_agent_id,
            "explore",
            "sha256:d1",
            &SubagentWorkspaceTerminalResource::None,
            timestamp,
        );
        let text = match &draft.content[0] {
            UserContentBlock::Text(text) => text.text.clone(),
            other => panic!("the notice is text: {other:?}"),
        };
        assert!(
            !text.contains("retained"),
            "no retained-workspace claim without a retained settlement: {text}"
        );
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
