//! The conversation-owned logical owner and registry of subagent children
//! (Issue #60).
//!
//! The registry owns the **logical** boundary of every subagent child of
//! one conversation:
//!
//! ```text
//! ownership        (prepare/commit linearization; the durable
//!                   SubagentOwnershipCommitted event is the single commit)
//! lifecycle        (typed SubagentLifecycle; terminal publication
//!                   exactly-once through the durable compound transaction)
//! identity         (SubagentId ordinals; never a PID)
//! physical root    (one fresh spawn-incarnation namespace per child)
//! cancellation     (intent commit -> driver command -> escalation)
//! settlement       (physical outcome -> terminal candidate -> durable
//!                   result acceptance -> capacity release)
//! recovery         (ordinal reseed from the durable authority)
//! ```
//!
//! It never holds an OS process handle. It stages a [`StagedChild`], and
//! the one ownership commit moves the handle into the driver task — the
//! sole low-level process owner ([`super::process`]). The registry holds
//! only the narrow driver command handle for cancellation.
//!
//! # Two-stage start
//!
//! [`SubagentRegistry::prepare`] performs every fallible stage privately —
//! input validation, identity allocation, process spawn, version
//! handshake, runtime activation — without publishing any conversation
//! state. [`SubagentRegistry::commit`] is then the one commit/rollback
//! linearization point: a failed or cancelled commit tears the staged child
//! down completely and leaves no registry record, no capacity consumption,
//! and no durable trace. A successful commit is the point of no return:
//! the attempt's later cancellation cannot reclaim the child.
//!
//! # Durability failure posture
//!
//! A terminal publication that cannot reach the durable authority enters
//! `PublishingTerminal`, is retried on the bounded policy, and is then
//! marked abandoned with a notification-plane failure diagnostic; it can
//! never become another record, and the runtime's durability-failed state
//! bars new submissions through the ordinary path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};

use crate::events::types::{SubagentOwnershipKind, SubagentWorkspaceTerminalResource};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::interaction::{
    InteractionOutcome, InteractionRef, InteractionRequest, RoutedInteraction,
    RoutedInteractionError,
};
use crate::runtime::types::{CancellationReason, DurabilityGate};
use crate::runtime::workflow::WorkflowId;
use crate::runtime::{MonotonicClock, RuntimeClock};

use super::activity::{SubagentExecutionProfile, SubagentObservation};
use super::catalog::{SubagentDefinitionDigest, SubagentExecutionDeadline, SubagentName};
use super::ipc::DelegationFrame;
use super::process::{PhysicalOutcome, PhysicalSettlement, StagedChild, SubagentSpawnPlan};
use super::resolver::ResolvedSubagentSpec;
use super::workspace::{
    SubagentWorkspaceManager, WorkspaceDisposalPhase, WorkspaceDisposalSettlement,
    WorkspaceHandoff, WorkspaceLease, WorkspaceSettlementDisposition, WorkspaceSnapshot,
    WorkspaceUnresolvedReason,
};
use super::{
    MAX_CONTEXT_PACKAGE_BYTES, MAX_RESULT_CONTENT_BYTES, MAX_TASK_BYTES, SubagentTerminalState,
    bound_utf8, ownership_event, terminal_publication, terminal_settlement, workflow_output_event,
};

/// The highest lifecycle state of one subagent child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentLifecycle {
    /// Ownership committed; the delegation is in flight or running.
    Running,
    /// Cancellation intent is committed; escalation may be in flight.
    Cancelling,
    /// The terminal outcome is known but its publication has not yet
    /// reached the durable authority.
    PublishingTerminal,
    /// The terminal result is durably published.
    Succeeded,
    /// The terminal failure is durably published.
    Failed,
    /// The terminal cancellation is durably published.
    Cancelled,
    /// The child process/control plane settled without a valid semantic
    /// terminal result, so the child outcome is unknown.
    Interrupted,
}

impl SubagentLifecycle {
    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    const fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// The canonicalized terminal outcome awaiting publication.
/// The decision of the commit linearization point.
enum Decision {
    Accepted {
        started_at: DateTime<Utc>,
        deadline_at_millis: Option<u64>,
    },
    RolledBack,
    Failed(SubagentStartError),
}

/// The canonicalized terminal outcome awaiting publication.
#[derive(Debug, Clone)]
struct TerminalCandidate {
    state: TerminalState,
    /// The bounded result content (succeeded only).
    content: Option<String>,
    /// The parent-validated Workflow value, when this is a successful
    /// Workflow-owned child. Keeping the parsed value on the frozen terminal
    /// candidate lets a retry rebuild the exact compound durable transition.
    workflow_value: Option<serde_json::Value>,
    /// The bounded failure diagnostic (failed only).
    diagnostic: Option<String>,
    /// The cancellation detail (cancelled only).
    reason: Option<CancellationReason>,
    /// The publication timestamp, frozen at canonicalization so a bounded
    /// retry rebuilds the byte-identical draft and an ambiguous commit
    /// resolves as the idempotent correlation retry, never a conflict.
    timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

/// The parent-side terminal protocol selected for a child.
///
/// `Normal` preserves the asynchronous `subagent` intrinsic's ordinary
/// parent-inbound result publication. `WorkflowOutput` routes the child's
/// validated JSON terminal content to the `WorkflowRuntime` boundary instead;
/// it never creates a normal parent `ToolResult` or inbound transcript fact.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SubagentTerminalMode {
    /// Publish the ordinary named-subagent result through its native path.
    #[default]
    Normal,
    /// Require the reserved `workflow_output` protocol with this frozen schema.
    WorkflowOutput {
        /// The output contract the child Agent Loop must satisfy.
        output_schema: serde_json::Value,
        /// The immutable Workflow identity owning this `AgentRun`.
        workflow_id: WorkflowId,
        /// The immutable Workflow invocation identity.
        run_id: ToolCallId,
        /// The stable Workflow node identity owning this `AgentRun`.
        node_id: String,
    },
}

/// The notification-plane state of one terminal result, mirroring the
/// background execution notification vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationState {
    None,
    Delivered,
    Failed,
}

impl NotificationState {
    const fn has_pending_delivery(self) -> bool {
        matches!(self, Self::Failed)
    }
}

struct SubagentRecord {
    subagent_id: SubagentId,
    child_agent_id: AgentId,
    child_conversation_id: ConversationId,
    tool_call_id: ToolCallId,
    agent: SubagentName,
    definition_digest: SubagentDefinitionDigest,
    terminal: SubagentTerminalMode,
    workspace: WorkspaceSnapshot,
    handoff: Option<WorkspaceHandoff>,
    /// The post-terminal physical-resource state. This never changes the
    /// absorbing logical subagent terminal state.
    workspace_resource_state: SubagentWorkspaceResourceState,
    /// The exact handoff and already-crossed physical phase of a durable
    /// disposal intent. This remains private to the resource owner even when
    /// the public snapshot stops advertising a removed physical path.
    workspace_disposal: Option<WorkspaceDisposalRecord>,
    /// Durable terminal settlement preserved a physical workspace without a
    /// proven handoff. This authority is private because callers may only
    /// retry disposal by `SubagentId` through the registry.
    workspace_unresolved: Option<WorkspaceUnresolvedRecord>,
    lifecycle: SubagentLifecycle,
    cancel_reason: Option<CancellationReason>,
    /// The one live deadline task, owned by this record and aborted as soon
    /// as cancellation intent or terminal settlement becomes authoritative.
    deadline_task: Option<tokio::task::JoinHandle<()>>,
    /// The narrow cancellation handle into the driver task — never an OS
    /// process handle.
    control: Option<tokio::sync::mpsc::Sender<super::process::DriverCommand>>,
    /// The bounded terminal failure/cancellation diagnostic. A successful
    /// child's answer content never appears here: the durable terminal
    /// inbound publication is the one result channel (Issue #178).
    detail: Option<String>,
    /// The latest live activity projection reported by the child (Issue
    /// #178). Observation-plane state only: never a lifecycle input.
    observation: SubagentObservation,
    /// The redacted execution profile frozen at child start (Issue #178).
    /// `None` only for recovery-projected records, whose frozen launch
    /// specification no longer exists in this process.
    profile: Option<SubagentExecutionProfile>,
    /// The parent-validated Workflow output value of a successful
    /// Workflow-owned child: the live Workflow result channel, deliberately
    /// kept out of the observation snapshot.
    terminal_workflow_value: Option<serde_json::Value>,
    pending_terminal: Option<TerminalCandidate>,
    publication_abandoned: bool,
    notification: NotificationState,
    started_at: DateTime<Utc>,
}

impl SubagentRecord {
    fn terminal_workspace_resource(&self) -> SubagentWorkspaceTerminalResource {
        match self.workspace_resource_state {
            SubagentWorkspaceResourceState::None | SubagentWorkspaceResourceState::Disposed => {
                SubagentWorkspaceTerminalResource::None
            }
            SubagentWorkspaceResourceState::Retained => {
                let handoff = self
                    .handoff
                    .clone()
                    .expect("retained resource has a proven workspace handoff");
                SubagentWorkspaceTerminalResource::Retained { handoff }
            }
            SubagentWorkspaceResourceState::PreservedUnresolved => {
                let unresolved = self
                    .workspace_unresolved
                    .as_ref()
                    .expect("unresolved resource has durable safety authority");
                SubagentWorkspaceTerminalResource::PreservedUnresolved {
                    reason: unresolved.reason,
                    detail: unresolved.detail.clone(),
                }
            }
            SubagentWorkspaceResourceState::DisposalInProgress
            | SubagentWorkspaceResourceState::WorktreeRemoved => {
                panic!("a terminal publication cannot be rebuilt after disposal began")
            }
        }
    }

    fn snapshot(&self) -> SubagentSnapshot {
        let state = match self.lifecycle {
            SubagentLifecycle::Running => SubagentState::Running,
            SubagentLifecycle::Cancelling => SubagentState::Cancelling,
            SubagentLifecycle::PublishingTerminal => SubagentState::PublishingTerminal,
            SubagentLifecycle::Succeeded => SubagentState::Succeeded,
            SubagentLifecycle::Failed => SubagentState::Failed,
            SubagentLifecycle::Cancelled => SubagentState::Cancelled,
            SubagentLifecycle::Interrupted => SubagentState::Interrupted,
        };
        SubagentSnapshot {
            subagent_id: self.subagent_id.clone(),
            child_agent_id: self.child_agent_id.clone(),
            child_conversation_id: self.child_conversation_id.clone(),
            tool_call_id: self.tool_call_id.clone(),
            agent: self.agent.as_str().to_owned(),
            definition_digest: self.definition_digest.as_str().to_owned(),
            workspace: self.workspace.clone(),
            handoff: self.handoff.clone(),
            workspace_resource_state: self.workspace_resource_state,
            state,
            // The reason is meaningful exactly while cancellation intent is
            // live or the child settled cancelled; a successful (Workflow)
            // settlement that raced an in-flight intent reports no stale
            // reason.
            cancel_reason: match self.lifecycle {
                SubagentLifecycle::Cancelling | SubagentLifecycle::Cancelled => {
                    self.cancel_reason
                }
                _ => None,
            },
            detail: self.detail.clone(),
            observation: self.observation.clone(),
            profile: self.profile.clone(),
            publication_abandoned: self.publication_abandoned,
            settled: self.lifecycle.is_terminal() && !self.publication_abandoned,
            started_at: self.started_at,
        }
    }
}

struct RegistryState {
    next_ordinal: u64,
    next_response_id: u64,
    records: Vec<SubagentRecord>,
    index: HashMap<SubagentId, usize>,
    /// Live routed interactions owned by child coordinators. This is a root
    /// projection cache only: it contains no waiter, settlement, or
    /// cancellation authority. The originating child coordinator remains the
    /// semantic owner of every entry.
    routed_interactions: HashMap<InteractionRef, RoutedInteraction>,
    observer: Option<Arc<dyn SubagentObserver>>,
    failure_sink: Option<Arc<dyn SubagentDurabilityFailureSink>>,
    /// The owning `ConversationRuntime`'s durability frontier (Issue #60):
    /// a new conversation-owned durable ownership commit must linearize
    /// against the runtime's `DurabilityFailed` commit on this shared gate.
    /// Installed by `ConversationRuntime::new` after the ownership
    /// transfer; a standalone registry has none and commits through the
    /// unbound-mailbox path.
    durability_gate: Option<Arc<DurabilityGate>>,
    #[cfg(test)]
    commit_hook: Option<Arc<CommitBoundaryHook>>,
    #[cfg(test)]
    control_handoff_hook: Option<Arc<ControlHandoffHook>>,
    #[cfg(test)]
    gate_release_hook: Option<Arc<GateReleaseHook>>,
    /// Test seam: pauses the first live cancellation contender (deadline
    /// expiry or an explicit caller) immediately before it can acquire the
    /// registry mutex that commits `Running -> Cancelling`.
    #[cfg(test)]
    cancellation_boundary_hook: Option<Arc<CancellationBoundaryHook>>,
    /// Test seam: pauses the terminal settlement path immediately before it
    /// can acquire the registry mutex that creates the terminal candidate
    /// and commits `... -> PublishingTerminal`.
    #[cfg(test)]
    terminal_authority_hook: Option<Arc<TerminalAuthorityHook>>,
    /// Test seam: one-shot completion latches for fired record-owned
    /// deadline tasks. A test registers a latch (by predictable child
    /// identity) before the child starts; the owning commit claims it when
    /// it creates the deadline task, and the latch resolves only after the
    /// fired deadline's cancellation call has fully returned.
    #[cfg(test)]
    deadline_completion: HashMap<SubagentId, tokio::sync::oneshot::Sender<()>>,
    /// Test seam: pre-staged children `prepare` consumes instead of
    /// spawning the real child binary.
    #[cfg(test)]
    staged_overrides: std::collections::VecDeque<StagedChild>,
}

/// The public lifecycle vocabulary of one subagent snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentState {
    /// Ownership committed; the delegation is in flight or running.
    Running,
    /// Cancellation intent committed; escalation may be in flight.
    Cancelling,
    /// The terminal outcome is known; publication is not yet durable.
    PublishingTerminal,
    /// The result is durably published.
    Succeeded,
    /// The failure is durably published.
    Failed,
    /// The cancellation is durably published.
    Cancelled,
    /// The child process/control plane settled without a valid semantic
    /// terminal result; the child's outcome is unknown.
    Interrupted,
}

impl SubagentState {
    /// Whether this state is terminal (absorbing).
    ///
    /// This is the domain's own lifecycle classification, kept beside the
    /// vocabulary it classifies: `PublishingTerminal` is deliberately **not**
    /// terminal — the outcome is known but its durable publication has not
    /// committed, so the child still owns unfinished settlement work.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Whether this state is active (non-terminal).
    #[must_use]
    pub const fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// The post-terminal physical-resource lifecycle of one subagent workspace.
///
/// This is deliberately separate from [`SubagentState`]. A child remains in
/// its absorbing logical terminal state while its retained worktree moves
/// through this bounded resource protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentWorkspaceResourceState {
    /// No retained isolated worktree exists for this child.
    None,
    /// A changed isolated worktree is available for handoff.
    Retained,
    /// A runtime-created physical workspace may remain, but terminal
    /// settlement did not establish a complete handoff proof.
    PreservedUnresolved,
    /// Disposal intent is durable, but physical settlement is unfinished.
    DisposalInProgress,
    /// The worktree was runtime-authorized and removed; branch settlement is
    /// still pending or was refused by compare-and-delete.
    WorktreeRemoved,
    /// The exact authorized worktree and branch are durably settled.
    Disposed,
}

#[derive(Debug, Clone)]
struct WorkspaceDisposalRecord {
    handoff: WorkspaceHandoff,
    phase: WorkspaceDisposalPhase,
}

#[derive(Debug, Clone)]
struct WorkspaceUnresolvedRecord {
    reason: WorkspaceUnresolvedReason,
    detail: String,
}

/// The deterministic public outcome of a retained-workspace disposal request.
///
/// The outcome is deliberately separate from [`SubagentState`]. Disposing a
/// physical handoff never adds a logical subagent terminal state.
#[derive(Debug, Clone, PartialEq)]
pub enum SubagentWorkspaceDisposal {
    /// The exact retained worktree and runtime branch were removed.
    Disposed(SubagentSnapshot),
    /// The same retained resource was already disposed earlier. No Git
    /// lookup or deletion is attempted for this idempotent result.
    AlreadyDisposed(SubagentSnapshot),
    /// The exact worktree is gone, but compare-and-delete has not settled the
    /// runtime branch yet. The caller can retry by identity.
    DisposalPending(SubagentSnapshot),
    /// This child has no retained isolated resource: it was shared, or its
    /// unchanged isolated worktree was removed during ordinary settlement.
    NoRetainedWorkspace(SubagentSnapshot),
}

/// A retained-workspace disposal request failed before it could produce a
/// successful or idempotent resource outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentWorkspaceDisposalError {
    /// The requested child is not in the authoritative registry.
    UnknownSubagent { subagent_id: SubagentId },
    /// The child has not reached a durable terminal boundary, or its
    /// terminal publication was abandoned. Disposal cannot race settlement.
    NotTerminal { state: SubagentState },
    /// The durable/runtime facts and current Git state did not prove one
    /// exact retained resource. No destructive mutation was attempted.
    OwnershipMismatch { detail: String },
    /// A durable mailbox or other runtime-owned operation failed. The exact
    /// physical operation may already have completed; the in-memory record is
    /// still updated to reflect the physical resource's absence.
    Backend { detail: String },
}

impl core::fmt::Display for SubagentWorkspaceDisposalError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownSubagent { subagent_id } => {
                write!(formatter, "unknown subagent {subagent_id}")
            }
            Self::NotTerminal { state } => write!(
                formatter,
                "retained workspace disposal requires a durably terminal subagent, but its state is {state:?}"
            ),
            Self::OwnershipMismatch { detail } => {
                write!(
                    formatter,
                    "retained workspace ownership could not be proven: {detail}"
                )
            }
            Self::Backend { detail } => {
                write!(formatter, "retained workspace disposal failed: {detail}")
            }
        }
    }
}

impl std::error::Error for SubagentWorkspaceDisposalError {}

fn map_workspace_disposal_error(
    error: super::workspace::WorkspaceDisposalError,
) -> SubagentWorkspaceDisposalError {
    match error {
        super::workspace::WorkspaceDisposalError::OwnershipMismatch { detail } => {
            SubagentWorkspaceDisposalError::OwnershipMismatch { detail }
        }
        super::workspace::WorkspaceDisposalError::Git { operation, detail } => {
            SubagentWorkspaceDisposalError::Backend {
                detail: format!("{operation}: {detail}"),
            }
        }
    }
}

/// A consistency snapshot of one subagent child.
///
/// Read-model materialization only: every field is derived from the
/// registry's state machine, never an authority of its own. This is the
/// **rich runtime-truth projection** consumed by the Runtime Client, the
/// TUI, recovery, and internal diagnostics. The model-facing
/// `execution(status)` response is the deliberately minimal
/// `SubagentExecutionSnapshot` projection of this snapshot (Issue #192),
/// owned by the `execution` intrinsic — this authoritative type never
/// weakens to fit the model boundary.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SubagentSnapshot {
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity (provenance of its answer).
    pub child_agent_id: AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The delegating tool call.
    pub tool_call_id: ToolCallId,
    /// The canonical named-agent identity frozen at start (Issue #144).
    pub agent: String,
    /// The deterministic definition digest frozen at start (Issue #144).
    ///
    /// The snapshot reports the definition the child actually started with,
    /// so a resource reload that redefines the same agent name can never
    /// make an already-running child appear to have the new definition.
    pub definition_digest: String,
    /// The immutable project-workspace authority selected before ownership.
    pub workspace: WorkspaceSnapshot,
    /// Retained work-product metadata, when terminal settlement preserves an
    /// isolated worktree for handoff.
    pub handoff: Option<WorkspaceHandoff>,
    /// The post-terminal physical-resource projection, independent of the
    /// absorbing logical subagent lifecycle.
    pub workspace_resource_state: SubagentWorkspaceResourceState,
    /// The lifecycle state.
    pub state: SubagentState,
    /// The committed cancellation reason, while cancellation intent exists
    /// or the child settled cancelled with one.
    ///
    /// A child that reported `Cancelled` without a committed parent
    /// cancellation intent has no semantic reason (`None`): the runtime
    /// never fabricates one.
    pub cancel_reason: Option<CancellationReason>,
    /// The bounded failure/cancellation diagnostic, once known.
    ///
    /// A successful child's answer content never appears here (Issue #178):
    /// the durable terminal inbound publication is the one result channel,
    /// and the live observation/control projection carries diagnostics only.
    pub detail: Option<String>,
    /// The latest live activity projection reported by the child (Issue
    /// #178). Observation-plane state only: it never changes lifecycle
    /// semantics, and every terminal settlement resets it to neutral.
    pub observation: SubagentObservation,
    /// The redacted execution profile frozen at child start (Issue #178);
    /// `None` for recovery-projected records.
    pub profile: Option<SubagentExecutionProfile>,
    /// Whether a terminal publication could not reach the durable
    /// authority and was abandoned.
    pub publication_abandoned: bool,
    /// Whether the child reached a settled state (terminal, publication
    /// not abandoned).
    pub settled: bool,
    /// When the ownership committed.
    pub started_at: DateTime<Utc>,
}

/// The subagent domain's own bounded discovery read model (Issue #180).
///
/// This type is owned by the subagent domain because everything in it is a
/// subagent-domain fact: which children the registry still knows, the
/// registry's own authoritative newest-first allocation order, which of
/// them match the requested lifecycle filter under
/// [`SubagentState::is_active`], and how many matched. Nothing here is a
/// model-facing concern, and the domain deliberately does **not** depend on
/// the model-facing `execution` control plane that consumes it: the
/// consumer knows the producer, never the other way round.
///
/// `snapshots` carries at most the caller's requested number of
/// authoritative snapshots, most recently started first, and `matched`
/// reports how many records matched the filter *before* that bound was
/// applied. Reporting `matched` separately is what keeps truncation honest:
/// a consumer can always tell a complete listing from a bounded prefix
/// without the registry ever materializing the whole set.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentListing {
    /// At most the requested number of authoritative snapshots, in the
    /// registry's newest-first authoritative order.
    pub snapshots: Vec<SubagentSnapshot>,
    /// How many records matched the filter in total, before the bound.
    pub matched: usize,
}

/// The inputs of one subagent start.
///
/// `resolved` is already the complete frozen outcome of resolving one named
/// definition against the invoking attempt's runtime resource generation.
/// The registry never resolves configuration itself: it owns live child
/// lifecycle only.
#[derive(Debug, Clone)]
pub struct SubagentStartSpec {
    /// The frozen named-agent specification of the child.
    pub resolved: ResolvedSubagentSpec,
    /// The effective approval mode frozen by the invoking Agent attempt.
    /// This changes approval decisions only for Tools already present in
    /// resolved; it never widens the child's capability set.
    pub approval_mode: crate::runtime::types::ApprovalMode,
    /// The delegated task.
    pub task: String,
    /// The explicit bounded context package.
    pub context: Option<String>,
    /// The delegating tool call.
    pub tool_call_id: ToolCallId,
    /// The child terminal protocol owned by the caller.
    pub terminal: SubagentTerminalMode,
}

/// A privately prepared subagent start: everything fallible already
/// succeeded, but nothing is published or owned yet.
#[derive(Debug)]
pub struct PreparedSubagent {
    subagent_id: SubagentId,
    child_agent_id: AgentId,
    child_conversation_id: ConversationId,
    tool_call_id: ToolCallId,
    agent: SubagentName,
    definition_digest: SubagentDefinitionDigest,
    terminal: SubagentTerminalMode,
    task: String,
    context: Option<String>,
    /// The definition-level deadline frozen before preparation. It is only
    /// scheduled after durable ownership commits.
    execution_deadline: Option<SubagentExecutionDeadline>,
    /// The redacted execution profile derived from the frozen model
    /// authority at preparation time (Issue #178).
    profile: SubagentExecutionProfile,
    staged: StagedChild,
}

/// The outcome of a successful ownership commit.
///
/// This is a **runtime acceptance value**: it carries the runtime facts a
/// caller may legitimately need (the owned identity and its committed
/// provenance), never a model-facing tool result. The model-facing
/// `subagent` creation projection is built by the `subagent` intrinsic alone
/// (Issue #192).
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentAccepted {
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity.
    pub child_agent_id: AgentId,
    /// The child conversation identity.
    pub child_conversation_id: ConversationId,
    /// The canonical named-agent identity.
    pub agent: String,
    /// The deterministic definition digest frozen at start.
    pub definition_digest: String,
}

/// The outcome of one [`SubagentRegistry::commit`].
#[derive(Debug)]
pub enum SubagentStartOutcome {
    /// Ownership committed; the child is running behind the start gate
    /// release.
    Accepted(SubagentAccepted),
    /// The attempt cancellation won the race against the commit; the
    /// staged child was torn down and nothing was published.
    RolledBack,
}

/// A typed start failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentStartError {
    /// The owning conversation is draining or draining-complete.
    ConversationInactive,
    /// The delegated task is empty or exceeds [`MAX_TASK_BYTES`].
    InvalidTask {
        /// The offending byte length.
        bytes: usize,
    },
    /// The explicit context package exceeds [`MAX_CONTEXT_PACKAGE_BYTES`].
    ContextOversized {
        /// The offending byte length.
        bytes: usize,
    },
    /// The per-conversation concurrency bound is reached.
    CapacityExceeded {
        /// The configured bound.
        max: usize,
    },
    /// Staging the child process failed.
    Spawn {
        /// The failure detail.
        detail: String,
    },
    /// The local workspace policy or Git acquisition failed. This is a
    /// failure of this named child start, not a global runtime-health fault.
    Workspace {
        /// The bounded workspace diagnostic.
        detail: String,
    },
    /// The isolated-worktree policy rejected acquisition because the parent
    /// workspace has uncommitted changes (Issue #188).
    ///
    /// This is deliberately a distinct typed reason rather than a
    /// [`Workspace`](Self::Workspace) string: the workspace manager owns the
    /// execution fact, this boundary preserves its semantic identity, and the
    /// model-facing `subagent` tool boundary owns the actionable public
    /// configuration remediation. Collapsing it into a diagnostic here would
    /// force that boundary to parse prose.
    WorkspaceDirtyParent {
        /// The exact committed source `HEAD` captured before the dirty
        /// observation. Retained as an internal execution fact.
        base_commit: String,
    },
    /// The durable ownership commit failed.
    Durability {
        /// The failure detail.
        detail: String,
    },
    /// The owning conversation runtime's durable authority is in the
    /// explicit `DurabilityFailed` state (Issue #63): no new
    /// conversation-owned durable semantic ownership commit may begin until
    /// the runtime is reconstructed. The staged child is torn down
    /// conclusively and no ownership fact, record, or Delegate exists.
    DurabilityFailed {
        /// The owning runtime's bounded failure diagnostic.
        detail: String,
    },
    /// The ownership decision could not return while rollback was proven
    /// complete.
    Rollback {
        /// The failure detail.
        detail: String,
    },
    /// The invoking attempt's cancellation became observable before the
    /// durable ownership commit (Issue #145): the child never reached a
    /// startable `Ready` *as a committed start*, no ownership record,
    /// event, or capacity consumption survives, and every staged physical
    /// resource settled before this was returned.
    Cancelled,
}

impl core::fmt::Display for SubagentStartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ConversationInactive => {
                write!(f, "the owning conversation is no longer active")
            }
            Self::InvalidTask { bytes } => write!(
                f,
                "the delegated task is empty or exceeds the {MAX_TASK_BYTES}-byte bound \
                 ({bytes} bytes)"
            ),
            Self::ContextOversized { bytes } => write!(
                f,
                "the context package exceeds the {MAX_CONTEXT_PACKAGE_BYTES}-byte bound \
                 ({bytes} bytes)"
            ),
            Self::CapacityExceeded { max } => {
                write!(f, "the per-conversation subagent bound ({max}) is reached")
            }
            Self::Spawn { detail } => write!(f, "could not start the child runtime: {detail}"),
            Self::Workspace { detail } => {
                write!(f, "could not prepare the child workspace: {detail}")
            }
            // Domain wording only. The actionable configuration remediation
            // is rendered at the model-facing `subagent` tool boundary.
            Self::WorkspaceDirtyParent { .. } => write!(
                f,
                "could not prepare the child workspace: the parent workspace has \
                 uncommitted changes and the clean-parent policy rejected isolated \
                 workspace acquisition"
            ),
            Self::Durability { detail } => {
                write!(f, "the durable ownership commit failed: {detail}")
            }
            Self::DurabilityFailed { detail } => write!(
                f,
                "the conversation runtime's durable authority has failed; no new subagent ownership may begin: {detail}"
            ),
            Self::Rollback { detail } => {
                write!(f, "the child rollback was not proven complete: {detail}")
            }
            Self::Cancelled => write!(
                f,
                "the invoking attempt was cancelled before the child was owned"
            ),
        }
    }
}

impl std::error::Error for SubagentStartError {}

/// The observation seam of the subagent plane (TUI / Runtime Client).
///
/// Two publication classes (Issue #178): [`on_snapshot`](Self::on_snapshot)
/// is the **reliable** lifecycle/identity publication — every transition
/// reaches the consumer exactly once, in order — and
/// [`on_activity`](Self::on_activity) is the **disposable** latest-value
/// activity publication, which the consumer may coalesce or drop.
pub trait SubagentObserver: Send + Sync {
    /// Called under the registry lock with each new consistency snapshot;
    /// the implementation must be cheap and nonblocking.
    fn on_snapshot(&self, snapshot: &SubagentSnapshot);

    /// Called under the registry lock for a retained-workspace resource
    /// transition. This is reliable but separate from lifecycle snapshots:
    /// disposal changes physical-resource state only. The default forwards
    /// to the lifecycle callback for observers that do not distinguish the
    /// projections.
    fn on_workspace(&self, snapshot: &SubagentSnapshot) {
        self.on_snapshot(snapshot);
    }

    /// Called under the registry lock with each new live-activity snapshot
    /// (Issue #178).
    ///
    /// This is a disposable, latest-value publication: the consumer may
    /// coalesce or drop intermediate values, and it must never treat an
    /// activity snapshot as lifecycle evidence. The default body forwards
    /// to [`on_snapshot`](Self::on_snapshot), so an observer that does not
    /// distinguish the two classes keeps capturing everything.
    fn on_activity(&self, snapshot: &SubagentSnapshot) {
        self.on_snapshot(snapshot);
    }

    /// Called under the registry lock when a child coordinator publishes a
    /// live interaction request. This is reliable semantic control, not
    /// disposable activity. Observers that only project lifecycle/activity
    /// state can leave this callback empty.
    fn on_interaction_pending(&self, _interaction: &RoutedInteraction) {}

    /// Called under the registry lock when a child coordinator settles one
    /// routed interaction. The child audit remains in the child's own
    /// conversation; this callback carries only root projection data.
    fn on_interaction_settled(&self, _interaction: &InteractionRef, _outcome: &InteractionOutcome) {
    }

    /// Called under the registry lock when a child process dies before its
    /// interaction coordinator can publish a terminal transition. This is a
    /// presentation removal only; it must not synthesize a child settlement.
    fn on_interaction_removed(&self, _interaction: &InteractionRef) {}
}

/// The root Runtime Client's publication-admission authority.
///
/// The authority is intentionally narrower than the interaction domain: it
/// answers only whether a capable root human-facing control attachment exists
/// at the root host's synchronized admission frontier. It owns no child
/// interaction state, waiter, audit, cancellation, settlement, or execution
/// authority.
pub(crate) trait InteractionPublicationAuthority: Send + Sync {
    /// Attempts to admit publication of one exact routed interaction.
    fn admit(&self, interaction: &InteractionRef) -> bool;
}

/// The durability-failure reporting seam of the subagent plane.
pub trait SubagentDurabilityFailureSink: Send + Sync {
    /// A terminal publication could not reach the durable authority.
    fn terminal_publication_failed(&self, subagent_id: &SubagentId, diagnostic: &str);
}

/// The narrow live-activity sink the child driver task holds (Issue #178).
///
/// Cheaply cloneable; routes decoded activity frames into the registry's
/// read model synchronously (one brief lock acquisition, no await). It
/// carries no authority: it cannot touch lifecycle, journal, or mailbox
/// state, and every update passes through
/// [`SubagentRegistry::apply_activity`]'s drop rules.
#[derive(Clone)]
pub(crate) struct SubagentActivitySink {
    subagent_id: SubagentId,
    registry: SubagentRegistry,
}

impl SubagentActivitySink {
    /// Applies one decoded child activity projection.
    pub(crate) fn apply(&self, observation: SubagentObservation) {
        self.registry.apply_activity(&self.subagent_id, observation);
    }
}

/// The narrow reliable semantic sink the child driver task holds for one
/// child. It can update the root's presentation projection only; it cannot
/// answer, cancel, or settle the child's interaction.
#[derive(Clone)]
pub(crate) struct SubagentInteractionSink {
    subagent_id: SubagentId,
    registry: SubagentRegistry,
}

impl SubagentInteractionSink {
    /// Applies one child-owned interaction request received over reliable
    /// control transport.
    pub(crate) fn apply_requested(&self, request: InteractionRequest) {
        self.registry
            .apply_child_interaction_requested(&self.subagent_id, request);
    }

    /// Applies one child-owned terminal interaction transition received over
    /// reliable control transport.
    pub(crate) fn apply_settled(&self, interaction: &InteractionRef, outcome: &InteractionOutcome) {
        self.registry
            .apply_child_interaction_settled(&self.subagent_id, interaction, outcome);
    }

    /// Checks publication admission at the root authority's linearization
    /// frontier. A successful result is only an ephemeral transport permit;
    /// the originating child coordinator still commits and owns the request.
    pub(crate) fn admit_publication(&self, interaction: &InteractionRef) -> bool {
        self.registry
            .admit_child_interaction_publication(&self.subagent_id, interaction)
    }
}

/// The composition inputs of the registry.
#[derive(Clone)]
pub struct SubagentRegistryConfig {
    /// The owning conversation identity.
    pub conversation_id: ConversationId,
    /// The parent (delegating) agent identity.
    pub agent_id: AgentId,
    /// The conversation inbound mailbox (durable authority).
    pub mailbox: ConversationInboundMailbox,
    /// The runtime clock.
    pub clock: Arc<dyn RuntimeClock>,
    /// The monotonic clock used only for whole-child execution deadlines.
    /// The timer is created after ownership commits; this clock is separate
    /// from the UTC timestamp clock used by durable events.
    pub monotonic_clock: Arc<dyn MonotonicClock>,
    /// The process spawn plan.
    pub spawn: SubagentSpawnPlan,
    /// The sole owner of physical named-subagent workspace acquisition and
    /// settlement. The registry supplies policy/identity but never runs Git.
    pub workspace: SubagentWorkspaceManager,
    /// The per-conversation concurrency bound.
    pub max_active: usize,
}

/// The sole logical owner and registry of a conversation's subagent
/// children.
///
/// Cheaply cloneable: every clone shares the one registry state, the same
/// contract as the background registry.
pub struct SubagentRegistry {
    config: SubagentRegistryConfig,
    state: Arc<Mutex<RegistryState>>,
    state_version: tokio::sync::watch::Sender<u64>,
    /// An early root Runtime Client human-provider availability hint. Each
    /// child driver receives a reliable subscription for fast fail-closed
    /// behavior, but the publication authority below remains the admission
    /// decision and is not inferred from this cached value.
    provider_available: tokio::sync::watch::Sender<bool>,
    /// The root Runtime Client publication frontier. This is installed by a
    /// live root host before activation; without it, child publication fails
    /// closed even if a stale provider watch says `true`.
    publication_authority: Arc<Mutex<Option<Arc<dyn InteractionPublicationAuthority>>>>,
    /// Serializes the logical resource transition with the physical manager
    /// operation. The manager has its own shared lock for Git calls; this
    /// outer lock also makes two client requests converge on one public
    /// disposal result before either can stale-read the handoff.
    workspace_disposal_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Clone for SubagentRegistry {
    fn clone(&self) -> Self {
        self.clone_for_task()
    }
}

impl SubagentRegistry {
    /// Creates the registry for one conversation.
    #[must_use]
    pub fn new(config: SubagentRegistryConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(RegistryState {
                next_ordinal: 1,
                next_response_id: 1,
                records: Vec::new(),
                index: HashMap::new(),
                routed_interactions: HashMap::new(),
                observer: None,
                failure_sink: None,
                durability_gate: None,
                #[cfg(test)]
                commit_hook: None,
                #[cfg(test)]
                control_handoff_hook: None,
                #[cfg(test)]
                gate_release_hook: None,
                #[cfg(test)]
                cancellation_boundary_hook: None,
                #[cfg(test)]
                terminal_authority_hook: None,
                #[cfg(test)]
                deadline_completion: HashMap::new(),
                #[cfg(test)]
                staged_overrides: std::collections::VecDeque::new(),
            })),
            state_version: tokio::sync::watch::Sender::new(0),
            provider_available: tokio::sync::watch::channel(false).0,
            publication_authority: Arc::new(Mutex::new(None)),
            workspace_disposal_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Installs the owning runtime's durability frontier (Issue #60).
    ///
    /// `ConversationRuntime::new` installs it after the ownership transfer;
    /// the runtime remains inactive until activation, so no ownership
    /// commit can race the installation. A standalone registry never has
    /// one.
    pub(crate) fn install_durability_gate(&self, gate: Arc<DurabilityGate>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.durability_gate = Some(gate);
    }

    /// The conversation this registry belongs to (construction ownership
    /// validation of the runtime that consumes it).
    #[must_use]
    pub(crate) fn conversation_id(&self) -> &ConversationId {
        &self.config.conversation_id
    }

    /// The parent (delegating) agent identity of this registry's domain.
    #[must_use]
    pub(crate) fn parent_agent_id(&self) -> &AgentId {
        &self.config.agent_id
    }

    /// Whether this registry's canonical mailbox is exactly the supplied
    /// mailbox: structural identity (same durable inbound capability and
    /// same process-local mailbox state), never a file-path comparison.
    #[must_use]
    pub(crate) fn shares_mailbox_domain(&self, other: &ConversationInboundMailbox) -> bool {
        self.config.mailbox.shares_domain_with(other)
    }

    /// Whether the registry owns no committed child record yet.
    ///
    /// A `ConversationRuntime` construction requires a pristine logical
    /// subagent plane: a registry with live children can never be silently
    /// adopted by a runtime that did not own their start.
    #[must_use]
    pub(crate) fn is_pristine(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.records.is_empty()
    }

    /// Reseeds the ordinal sequence from the durable authority during
    /// startup recovery, so a recovered conversation never reissues an
    /// ordinal that already entered durable authority.
    pub fn restore_sequence_watermark(&self, highest_ordinal: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_ordinal = state.next_ordinal.max(highest_ordinal + 1);
    }

    /// Restores the read-model entry of a terminal child whose durable
    /// terminal fact retained a workspace handoff. This is projection-only:
    /// the child process is not recreated, and the preserved worktree is not
    /// reacquired or cleaned up during startup.
    pub(crate) fn restore_recovered_handoff(
        &self,
        recovered: &crate::runtime::recovery::RecoveredSubagentHandoff,
    ) {
        let Ok(agent) = SubagentName::parse(&recovered.evidence.agent) else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.index.contains_key(&recovered.evidence.subagent_id) {
            return;
        }
        let lifecycle = match recovered.state {
            SubagentTerminalState::Succeeded => SubagentLifecycle::Succeeded,
            SubagentTerminalState::Failed => SubagentLifecycle::Failed,
            SubagentTerminalState::Cancelled => SubagentLifecycle::Cancelled,
            SubagentTerminalState::Interrupted => SubagentLifecycle::Interrupted,
        };
        let detail = Some(match recovered.state {
            SubagentTerminalState::Succeeded => {
                "the child completed; its changed workspace was preserved for handoff".to_owned()
            }
            SubagentTerminalState::Failed => {
                "the child failed; its changed workspace was preserved for handoff".to_owned()
            }
            SubagentTerminalState::Cancelled => {
                "the child was cancelled; its changed workspace was preserved for handoff"
                    .to_owned()
            }
            SubagentTerminalState::Interrupted => {
                "the child was interrupted; its changed workspace was preserved for handoff"
                    .to_owned()
            }
        });
        let record = SubagentRecord {
            subagent_id: recovered.evidence.subagent_id.clone(),
            child_agent_id: recovered.evidence.child_agent_id.clone(),
            child_conversation_id: recovered.evidence.child_conversation_id.clone(),
            tool_call_id: recovered.evidence.tool_call_id.clone(),
            agent,
            definition_digest: serde_json::from_value(serde_json::Value::String(
                recovered.evidence.definition_digest.clone(),
            ))
            .expect("durable subagent digest is validated before recovery"),
            terminal: SubagentTerminalMode::Normal,
            workspace: recovered.evidence.workspace.clone(),
            handoff: Some(recovered.handoff.clone()),
            workspace_resource_state: SubagentWorkspaceResourceState::Retained,
            workspace_disposal: None,
            workspace_unresolved: None,
            lifecycle,
            cancel_reason: None,
            deadline_task: None,
            control: None,
            detail,
            // A recovery-projected record has no live activity and no
            // frozen launch profile in this process.
            observation: SubagentObservation::default(),
            profile: None,
            terminal_workflow_value: None,
            pending_terminal: None,
            publication_abandoned: false,
            notification: NotificationState::Delivered,
            started_at: recovered.evidence.started_at,
        };
        let index = state.records.len();
        state
            .index
            .insert(recovered.evidence.subagent_id.clone(), index);
        state.records.push(record);
    }

    /// Restores a terminal read-model record whose durable terminal fact
    /// preserved a possible physical worktree without a complete handoff
    /// proof. This never fabricates a `WorkspaceHandoff`: the immutable
    /// ownership snapshot remains the authority for a later identity-based
    /// re-proof.
    pub(crate) fn restore_recovered_unresolved(
        &self,
        recovered: &crate::runtime::recovery::RecoveredSubagentUnresolvedWorkspace,
    ) {
        let Ok(agent) = SubagentName::parse(&recovered.evidence.agent) else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.index.contains_key(&recovered.evidence.subagent_id) {
            return;
        }
        let lifecycle = match recovered.state {
            SubagentTerminalState::Succeeded => SubagentLifecycle::Succeeded,
            SubagentTerminalState::Failed => SubagentLifecycle::Failed,
            SubagentTerminalState::Cancelled => SubagentLifecycle::Cancelled,
            SubagentTerminalState::Interrupted => SubagentLifecycle::Interrupted,
        };
        let detail = Some(match recovered.state {
            SubagentTerminalState::Succeeded => {
                format!(
                    "the child completed; workspace settlement remains unresolved: {}",
                    recovered.detail
                )
            }
            SubagentTerminalState::Failed => {
                format!(
                    "the child failed; workspace settlement remains unresolved: {}",
                    recovered.detail
                )
            }
            SubagentTerminalState::Cancelled => {
                format!(
                    "the child was cancelled; workspace settlement remains unresolved: {}",
                    recovered.detail
                )
            }
            SubagentTerminalState::Interrupted => {
                format!(
                    "the child was interrupted; workspace settlement remains unresolved: {}",
                    recovered.detail
                )
            }
        });
        let record = SubagentRecord {
            subagent_id: recovered.evidence.subagent_id.clone(),
            child_agent_id: recovered.evidence.child_agent_id.clone(),
            child_conversation_id: recovered.evidence.child_conversation_id.clone(),
            tool_call_id: recovered.evidence.tool_call_id.clone(),
            agent,
            definition_digest: serde_json::from_value(serde_json::Value::String(
                recovered.evidence.definition_digest.clone(),
            ))
            .expect("durable subagent digest is validated before recovery"),
            terminal: SubagentTerminalMode::Normal,
            workspace: recovered.evidence.workspace.clone(),
            handoff: None,
            workspace_resource_state: SubagentWorkspaceResourceState::PreservedUnresolved,
            workspace_disposal: None,
            workspace_unresolved: Some(WorkspaceUnresolvedRecord {
                reason: recovered.reason,
                detail: recovered.detail.clone(),
            }),
            lifecycle,
            cancel_reason: None,
            deadline_task: None,
            control: None,
            detail,
            observation: SubagentObservation::default(),
            profile: None,
            terminal_workflow_value: None,
            pending_terminal: None,
            publication_abandoned: false,
            notification: NotificationState::Delivered,
            started_at: recovered.evidence.started_at,
        };
        let index = state.records.len();
        state
            .index
            .insert(recovered.evidence.subagent_id.clone(), index);
        state.records.push(record);
    }

    /// Restores a terminal read-model record whose retained physical resource
    /// entered the durable disposal lifecycle before this process started.
    /// The recovered phase is resource state only; the logical lifecycle
    /// stays at the terminal state carried by the original terminal fact.
    pub(crate) fn restore_recovered_disposal(
        &self,
        recovered: &crate::runtime::recovery::RecoveredSubagentDisposal,
    ) {
        let Ok(agent) = SubagentName::parse(&recovered.evidence.agent) else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.index.contains_key(&recovered.evidence.subagent_id) {
            return;
        }
        let lifecycle = match recovered.state {
            SubagentTerminalState::Succeeded => SubagentLifecycle::Succeeded,
            SubagentTerminalState::Failed => SubagentLifecycle::Failed,
            SubagentTerminalState::Cancelled => SubagentLifecycle::Cancelled,
            SubagentTerminalState::Interrupted => SubagentLifecycle::Interrupted,
        };
        let resource_detail = match recovered.phase {
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Authorized => {
                "disposal was durably authorized; the exact retained resource remains retryable"
            }
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::WorktreeRemoved => {
                "the runtime-authorized worktree was removed; branch settlement remains retryable"
            }
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Disposed => {
                "the exact retained workspace was durably disposed"
            }
        };
        let detail = match recovered.state {
            SubagentTerminalState::Succeeded => {
                format!("the child completed; {resource_detail}")
            }
            SubagentTerminalState::Failed => format!("the child failed; {resource_detail}"),
            SubagentTerminalState::Cancelled => {
                format!("the child was cancelled; {resource_detail}")
            }
            SubagentTerminalState::Interrupted => {
                format!("the child was interrupted; {resource_detail}")
            }
        };
        let (workspace_resource_state, workspace_disposal) = match recovered.phase {
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Authorized => (
                SubagentWorkspaceResourceState::DisposalInProgress,
                Some(WorkspaceDisposalRecord {
                    handoff: recovered.handoff.clone(),
                    phase: WorkspaceDisposalPhase::Authorized,
                }),
            ),
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::WorktreeRemoved => (
                SubagentWorkspaceResourceState::WorktreeRemoved,
                Some(WorkspaceDisposalRecord {
                    handoff: recovered.handoff.clone(),
                    phase: WorkspaceDisposalPhase::WorktreeRemoved,
                }),
            ),
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Disposed => {
                (SubagentWorkspaceResourceState::Disposed, None)
            }
        };
        let record = SubagentRecord {
            subagent_id: recovered.evidence.subagent_id.clone(),
            child_agent_id: recovered.evidence.child_agent_id.clone(),
            child_conversation_id: recovered.evidence.child_conversation_id.clone(),
            tool_call_id: recovered.evidence.tool_call_id.clone(),
            agent,
            definition_digest: serde_json::from_value(serde_json::Value::String(
                recovered.evidence.definition_digest.clone(),
            ))
            .expect("durable subagent digest is validated before recovery"),
            terminal: SubagentTerminalMode::Normal,
            workspace: recovered.evidence.workspace.clone(),
            handoff: None,
            workspace_resource_state,
            workspace_disposal,
            workspace_unresolved: None,
            lifecycle,
            cancel_reason: None,
            deadline_task: None,
            control: None,
            detail: Some(detail),
            observation: SubagentObservation::default(),
            profile: None,
            terminal_workflow_value: None,
            pending_terminal: None,
            publication_abandoned: false,
            notification: NotificationState::Delivered,
            started_at: recovered.evidence.started_at,
        };
        let index = state.records.len();
        state
            .index
            .insert(recovered.evidence.subagent_id.clone(), index);
        state.records.push(record);
    }

    /// Installs the observation seam and immediately emits the current
    /// snapshot of every known record.
    pub fn install_observer_and_snapshots(
        &self,
        observer: Arc<dyn SubagentObserver>,
    ) -> Vec<SubagentSnapshot> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshots: Vec<SubagentSnapshot> =
            state.records.iter().map(SubagentRecord::snapshot).collect();
        for snapshot in &snapshots {
            observer.on_snapshot(snapshot);
        }
        let interactions: Vec<RoutedInteraction> =
            state.routed_interactions.values().cloned().collect();
        for interaction in &interactions {
            observer.on_interaction_pending(interaction);
        }
        state.observer = Some(observer);
        snapshots
    }

    /// Updates the root human-provider state for all live and future child
    /// conversations. This changes only whether a new interaction may be
    /// published; it never settles an interaction already pending.
    pub(crate) fn set_interaction_provider_available(&self, available: bool) {
        self.provider_available.send_replace(available);
    }

    /// Installs the root Runtime Client's synchronized publication frontier.
    /// This is a composition seam, not an interaction owner, and is installed
    /// before the runtime activates so every child route sees one authority.
    pub(crate) fn install_interaction_publication_authority(
        &self,
        authority: Arc<dyn InteractionPublicationAuthority>,
    ) {
        let mut installed = self
            .publication_authority
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        debug_assert!(installed.is_none(), "one publication authority only");
        *installed = Some(authority);
    }

    /// Subscribes one child driver to the current root provider state.
    pub(crate) fn interaction_provider_receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.provider_available.subscribe()
    }

    /// Admits one child interaction publication at the root Runtime Client's
    /// synchronized provider frontier. The registry lock covers the child
    /// liveness/identity check while the root authority lock defines the
    /// attach-vs-detach ordering. No child semantic state is created here.
    pub(crate) fn admit_child_interaction_publication(
        &self,
        subagent_id: &SubagentId,
        interaction: &InteractionRef,
    ) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return false;
        };
        let record = &state.records[index];
        if !record.lifecycle.is_active()
            || record.publication_abandoned
            || record.child_conversation_id != interaction.conversation_id
        {
            return false;
        }
        let authority = self
            .publication_authority
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        authority.is_some_and(|authority| authority.admit(interaction))
    }

    /// Returns the live child interaction projection for the Runtime Client
    /// bootstrap cut. These values are never used as recovery authority.
    #[must_use]
    pub(crate) fn pending_interaction_projection(&self) -> Vec<RoutedInteraction> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut interactions: Vec<_> = state.routed_interactions.values().cloned().collect();
        interactions.sort_by(|left, right| left.interaction.cmp(&right.interaction));
        interactions
    }

    /// Installs the durability-failure sink.
    pub fn install_failure_sink(&self, sink: Arc<dyn SubagentDurabilityFailureSink>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.failure_sink = Some(sink);
    }

    /// **Prepare.** Runs every fallible stage privately: input validation,
    /// identity allocation, process spawn, and the activation handshake.
    /// Nothing is published, no capacity is consumed, and a failure leaves
    /// no trace.
    ///
    /// `preparation_cancellation` is the invoking attempt's cancellation
    /// authority (Issue #145): it owns the *whole* pre-commit lifecycle,
    /// not merely the commit decision. If it becomes observable while the
    /// child is still staging, the child never reaches a startable `Ready`,
    /// every staged physical resource settles, and this returns
    /// [`SubagentStartError::Cancelled`].
    ///
    /// # Errors
    ///
    /// Returns the typed [`SubagentStartError`] of the first failing stage,
    /// or [`SubagentStartError::Cancelled`] when the attempt cancellation
    /// won before the ownership commit.
    #[allow(clippy::too_many_lines)] // one ordered staged-start pipeline
    pub async fn prepare(
        &self,
        spec: &SubagentStartSpec,
        preparation_cancellation: &CancellationSignal,
    ) -> Result<PreparedSubagent, SubagentStartError> {
        if preparation_cancellation.is_cancelled() {
            return Err(SubagentStartError::Cancelled);
        }
        let task_bytes = spec.task.len();
        if spec.task.trim().is_empty() || task_bytes > MAX_TASK_BYTES {
            return Err(SubagentStartError::InvalidTask { bytes: task_bytes });
        }
        if let Some(context) = &spec.context {
            let bytes = context.len();
            if bytes > MAX_CONTEXT_PACKAGE_BYTES {
                return Err(SubagentStartError::ContextOversized { bytes });
            }
        }
        if self.config.mailbox.begin_running_admission().is_err() {
            return Err(SubagentStartError::ConversationInactive);
        }
        // The redacted observation-plane profile derives from the frozen
        // model authority exactly once, at preparation time.
        let profile = SubagentExecutionProfile::from_frozen(&spec.resolved.model);
        // Workspace acquisition and physical-root allocation are both staged
        // child ownership. A pre-commit crash can leave a durable store for
        // the ordinal that was never published; skip that identity rather
        // than ever allowing a new child to append to its history.
        let (subagent_id, child_conversation_id, child_agent_id, workspace_lease, runtime_root) = loop {
            if preparation_cancellation.is_cancelled() {
                return Err(SubagentStartError::Cancelled);
            }
            let ordinal = {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let ordinal = state.next_ordinal;
                state.next_ordinal += 1;
                ordinal
            };
            let subagent_id = SubagentId::for_conversation(&self.config.conversation_id, ordinal);
            let child_conversation_id = ConversationId::new(subagent_id.as_str());
            let child_agent_id = AgentId::new(format!("agent-{subagent_id}"));
            // Workspace acquisition is staged child ownership. It happens
            // after resolution/freeze and before any child preparation, but
            // the lease is not durable until the commit below succeeds.
            let workspace_lease = self
                .config
                .workspace
                .acquire(
                    spec.resolved.workspace_policy,
                    &subagent_id,
                    preparation_cancellation,
                )
                .await
                .map_err(|error| match error {
                    super::workspace::WorkspaceAcquireError::Cancelled => {
                        SubagentStartError::Cancelled
                    }
                    super::workspace::WorkspaceAcquireError::Settlement { detail } => {
                        SubagentStartError::Rollback { detail }
                    }
                    // Issue #188: the dirty-parent rejection keeps its typed
                    // identity across this boundary. Flattening it into a
                    // string here would destroy the only fact the
                    // model-facing tool boundary needs to render actionable
                    // remediation without parsing prose.
                    super::workspace::WorkspaceAcquireError::DirtyParent { base_commit } => {
                        SubagentStartError::WorkspaceDirtyParent { base_commit }
                    }
                    error => SubagentStartError::Workspace {
                        detail: error.to_string(),
                    },
                })?;
            #[cfg(test)]
            {
                let override_child = self
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .staged_overrides
                    .pop_front();
                if let Some(staged) = override_child {
                    return Ok(PreparedSubagent {
                        subagent_id,
                        child_agent_id,
                        child_conversation_id,
                        tool_call_id: spec.tool_call_id.clone(),
                        agent: spec.resolved.agent.clone(),
                        definition_digest: spec.resolved.definition_digest.clone(),
                        terminal: spec.terminal.clone(),
                        task: spec.task.clone(),
                        context: spec.context.clone(),
                        execution_deadline: spec.resolved.execution_deadline,
                        profile,
                        staged: staged.with_workspace(workspace_lease),
                    });
                }
            }
            let runtime_root = match self.config.spawn.allocate_child_runtime_root(&subagent_id) {
                Ok(runtime_root) => runtime_root,
                Err(super::process::SpawnError::ConversationIdentityInUse { .. }) => {
                    if let Err(error) = workspace_lease.settle_staged().await {
                        return Err(SubagentStartError::Rollback {
                            detail: error.detail,
                        });
                    }
                    continue;
                }
                Err(error) => {
                    let start_error = SubagentStartError::Spawn {
                        detail: error.to_string(),
                    };
                    return Err(settle_staged_workspace(workspace_lease, start_error).await);
                }
            };
            break (
                subagent_id,
                child_conversation_id,
                child_agent_id,
                workspace_lease,
                runtime_root,
            );
        };
        let child_spec = self.config.spawn.child_spec(
            &subagent_id,
            &child_conversation_id,
            &child_agent_id,
            &self.config.agent_id,
            &spec.resolved,
            spec.approval_mode,
            &runtime_root,
            &workspace_lease,
            &spec.terminal,
        );
        let staged = match super::process::spawn_staged(
            &self.config.spawn,
            &child_spec,
            runtime_root,
            workspace_lease,
            preparation_cancellation,
        )
        .await
        {
            Ok(staged) => staged,
            Err(error) => {
                let start_error = match error {
                    super::process::SpawnError::Cancelled => SubagentStartError::Cancelled,
                    error => SubagentStartError::Spawn {
                        detail: error.to_string(),
                    },
                };
                return Err(start_error);
            }
        };
        Ok(PreparedSubagent {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id: spec.tool_call_id.clone(),
            agent: spec.resolved.agent.clone(),
            definition_digest: spec.resolved.definition_digest.clone(),
            terminal: spec.terminal.clone(),
            task: spec.task.clone(),
            context: spec.context.clone(),
            execution_deadline: spec.resolved.execution_deadline,
            profile,
            staged,
        })
    }

    /// Installs a pre-staged child `prepare` consumes instead of spawning
    /// (tests only).
    #[cfg(test)]
    pub(crate) fn push_staged_override(&self, staged: StagedChild) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.staged_overrides.push_back(staged);
    }

    /// **Commit.** The one commit/rollback linearization point.
    ///
    /// A rolled-back or failed commit tears the staged child down
    /// completely (killed, reaped, runtime root removed) before returning;
    /// a successful commit publishes the durable ownership event, creates
    /// the record, releases the start gate, and returns the tool result.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentStartError::ConversationInactive`] when the
    /// conversation is shutting down, [`SubagentStartError::Capacity`] when
    /// the active bound is full at the linearization point, or
    /// [`SubagentStartError::Durability`] when the ownership commit fails.
    ///
    /// # Panics
    ///
    /// Panics only if the registry loses its accepted ownership record between
    /// the durable commit and control-handle publication, which would violate
    /// the registry's own ownership invariant.
    #[allow(clippy::too_many_lines)] // One commit path, asserted end to end.
    pub async fn commit(
        &self,
        prepared: PreparedSubagent,
        attempt_cancellation: &CancellationSignal,
    ) -> Result<SubagentStartOutcome, SubagentStartError> {
        // Retain the counted lifecycle admission through the entire
        // prepared-to-driver handoff, including conclusive rollback. This
        // prevents runtime drain from declaring quiescence between the
        // durable ownership decision and publication of the driver control
        // path; the registry's own cancellation state still handles a drain
        // that wins after the record is visible.
        let Ok(_admission) = self.config.mailbox.begin_running_admission() else {
            return match prepared.staged.rollback().await {
                Ok(()) => Err(SubagentStartError::ConversationInactive),
                Err(error) => Err(SubagentStartError::Rollback {
                    detail: error.to_string(),
                }),
            };
        };
        let PreparedSubagent {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id,
            agent,
            definition_digest,
            terminal,
            task,
            context,
            execution_deadline,
            profile,
            staged,
        } = prepared;
        let decision = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let mailbox = self.config.mailbox.clone();
            let clock = self.config.clock.clone();
            let monotonic_clock = self.config.monotonic_clock.clone();
            let config = &self.config;
            // The lease carried by the staged child is the sole workspace
            // authority. The prepared wrapper does not keep a second mutable
            // copy that could drift from the physical owner before commit.
            let workspace = staged.workspace_snapshot().clone();
            // Runtime durability frontier (Issue #60): a new
            // conversation-owned durable ownership commit must linearize
            // against the owning runtime's `DurabilityFailed` commit on one
            // synchronization boundary. The permission guard is held across
            // the durable ownership write and the record publication below,
            // so a failure that wins the gate first rejects this start (and
            // the staged child rolls back conclusively), and an ownership
            // that wins first is already durably owned before the failure
            // can be published. A standalone registry has no runtime gate
            // and commits through the unbound-mailbox path. The gate handle
            // is copied out of the registry state first: the guard borrows
            // the gate, never the registry state, so the ownership commit
            // below may still mutate the state while the guard is held.
            let durability_gate = state.durability_gate.clone();
            let ownership_permission = durability_gate
                .as_ref()
                .map(|gate| gate.enter_ownership_commit());
            if let Some(Err(refused)) = &ownership_permission {
                Decision::Failed(SubagentStartError::DurabilityFailed {
                    detail: refused.diagnostic.clone(),
                })
            } else {
                // `ownership_permission` stays alive to the end of this
                // block: the gate guard spans the whole ownership commit.
                let decision = match mailbox.with_running_commit(|| {
                    if mailbox.is_bound_inactive() {
                        return Decision::Failed(SubagentStartError::ConversationInactive);
                    }
                    #[cfg(test)]
                    if let Some(hook) = &state.commit_hook {
                        hook.wait();
                    }
                    let active = state
                        .records
                        .iter()
                        // PublishingTerminal remains an owned, unresolved
                        // settlement and therefore still consumes capacity. A
                        // durability-failed runtime separately rejects new
                        // mutations, but capacity must not silently reopen.
                        .filter(|record| record.lifecycle.is_active())
                        .count();
                    if active >= config.max_active {
                        return Decision::Failed(SubagentStartError::CapacityExceeded {
                            max: config.max_active,
                        });
                    }
                    if attempt_cancellation.is_cancelled() {
                        return Decision::RolledBack;
                    }
                    let started_at = clock.now();
                    if let Err(error) = mailbox.commit_subagent_ownership(ownership_event(
                        &config.conversation_id,
                        &subagent_id,
                        &child_agent_id,
                        &child_conversation_id,
                        &tool_call_id,
                        &agent,
                        &definition_digest,
                        match &terminal {
                            SubagentTerminalMode::Normal => SubagentOwnershipKind::Normal,
                            SubagentTerminalMode::WorkflowOutput { .. } => {
                                SubagentOwnershipKind::Workflow
                            }
                        },
                        &workspace,
                        started_at,
                    )) {
                        return Decision::Failed(SubagentStartError::Durability {
                            detail: error.to_string(),
                        });
                    }
                    // The deadline starts only after the durable ownership
                    // event succeeds. Sampling the monotonic clock here
                    // keeps the whole owned lifecycle covered without
                    // allowing an uncommitted staged child to be cancelled.
                    let deadline_at_millis = execution_deadline.map(|deadline| {
                        monotonic_clock
                            .now_millis()
                            .saturating_add(deadline.as_millis())
                    });
                    Decision::Accepted {
                        started_at,
                        deadline_at_millis,
                    }
                }) {
                    Ok(decision) => decision,
                    Err(_) => Decision::Failed(SubagentStartError::ConversationInactive),
                };
                if let Decision::Accepted { started_at, .. } = &decision {
                    let record = SubagentRecord {
                        subagent_id: subagent_id.clone(),
                        child_agent_id: child_agent_id.clone(),
                        child_conversation_id: child_conversation_id.clone(),
                        tool_call_id,
                        agent: agent.clone(),
                        definition_digest: definition_digest.clone(),
                        terminal: terminal.clone(),
                        workspace: workspace.clone(),
                        handoff: None,
                        workspace_resource_state: SubagentWorkspaceResourceState::None,
                        workspace_disposal: None,
                        workspace_unresolved: None,
                        lifecycle: SubagentLifecycle::Running,
                        cancel_reason: None,
                        deadline_task: None,
                        control: None,
                        detail: None,
                        observation: SubagentObservation::default(),
                        profile: Some(profile),
                        terminal_workflow_value: None,
                        pending_terminal: None,
                        publication_abandoned: false,
                        notification: NotificationState::None,
                        started_at: *started_at,
                    };
                    let index = state.records.len();
                    state.index.insert(subagent_id.clone(), index);
                    state.records.push(record);
                    publish_snapshot(&mut state, &self.state_version, index);
                }
                decision
            }
        };
        match decision {
            Decision::RolledBack => match staged.rollback().await {
                Ok(()) => Ok(SubagentStartOutcome::RolledBack),
                Err(error) => Err(SubagentStartError::Rollback {
                    detail: error.to_string(),
                }),
            },
            Decision::Failed(error) => match staged.rollback().await {
                Ok(()) => Err(error),
                Err(rollback) => Err(SubagentStartError::Rollback {
                    detail: format!("{error}; {rollback}"),
                }),
            },
            Decision::Accepted {
                deadline_at_millis, ..
            } => {
                // The driver routes the child's live activity frames into
                // the registry read model through this narrow sink; it is
                // observation-plane traffic only.
                let activity = SubagentActivitySink {
                    subagent_id: subagent_id.clone(),
                    registry: self.clone_for_task(),
                };
                let interactions = SubagentInteractionSink {
                    subagent_id: subagent_id.clone(),
                    registry: self.clone_for_task(),
                };
                let provider_available = self.interaction_provider_receiver();
                let driver = staged.into_driver(
                    DelegationFrame {
                        task,
                        context,
                        // The driver overwrites this with the watch's current
                        // value immediately before sending Delegate. Keeping
                        // the field explicit makes the wire contract clear
                        // without creating a second provider authority.
                        interaction_provider_available: false,
                    },
                    Some(activity),
                    Some(interactions),
                    Some(provider_available),
                );
                let (commands, start_gate, task) = driver.split();
                // The task is created only after the durable ownership event
                // and Running record exist. It calls the same synchronous
                // registry cancellation authority as an explicit cancel;
                // it never creates a terminal result or sends a driver
                // command directly.
                let deadline_task = deadline_at_millis.map(|deadline_at_millis| {
                    let clock = self.config.monotonic_clock.clone();
                    let registry = self.clone_for_task();
                    let deadline_id = subagent_id.clone();
                    // Test-only: claim the test's one-shot completion latch
                    // (if any) for this child, so a fired deadline's full
                    // return is awaitable by the race regression.
                    #[cfg(test)]
                    let completion = {
                        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                        state.deadline_completion.remove(&deadline_id)
                    };
                    tokio::spawn(async move {
                        clock.wait_until_millis(deadline_at_millis).await;
                        let _ = registry.cancel(
                            &deadline_id,
                            CancellationReason::SubagentExecutionDeadlineExceeded,
                        );
                        // Test-only: fires only after the deadline's
                        // cancellation call has fully returned.
                        #[cfg(test)]
                        if let Some(completion) = completion {
                            let _ = completion.send(());
                        }
                    })
                });
                // This hook is outside the registry lock and after the
                // durable ownership fact, the Running record, and the
                // driver task all exist. It pauses before the gate-release
                // critical section, so a concurrent cancellation commits
                // while the command handle is still None — the
                // deterministic "cancel lock wins first" edge.
                #[cfg(test)]
                let control_handoff_hook = {
                    self.state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .control_handoff_hook
                        .clone()
                };
                #[cfg(test)]
                if let Some(hook) = control_handoff_hook {
                    hook.wait();
                }

                // Point of no return: the child is conversation-owned. The
                // OS handle moves into the driver task; the registry keeps
                // only the narrow command handle.
                //
                // One synchronization point: the command-handle install,
                // the lifecycle read, and the start-gate release all happen
                // under the registry mutex, so the mutex is the exact
                // arbitration boundary between start-gate release and
                // explicit cancellation. A cancellation that acquired the
                // mutex first resolved the gate cancelled — the driver
                // sends Cancel before Delegate and never allows child
                // semantic work to begin. A gate release that acquired the
                // mutex first defines an already-started child whose later
                // cancellation is in-flight cancellation.
                let deadline_task_to_abort = {
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    // Test-only: extract the pause handle before the record
                    // borrow so the gate-release section is one critical
                    // section.
                    #[cfg(test)]
                    let gate_release_hook = state.gate_release_hook.clone();
                    let index = *state
                        .index
                        .get(&subagent_id)
                        .expect("accepted ownership has a registry record");
                    let record = &mut state.records[index];
                    record.control = Some(commands);
                    record.deadline_task = deadline_task;
                    // Test-only: the exact remaining edge — the command
                    // handle is installed but the start gate is not yet
                    // released. The pause parks while holding the registry
                    // mutex, so a concurrent `cancel` provably blocks: the
                    // edge is unobservable, never best-effort. Production
                    // has no pause and no equivalent semantic state.
                    #[cfg(test)]
                    if let Some(hook) = gate_release_hook {
                        hook.wait();
                    }
                    let cancel_before_start =
                        matches!(record.lifecycle, SubagentLifecycle::Cancelling).then(|| {
                            record
                                .cancel_reason
                                .expect("cancelling child has a committed reason")
                        });
                    let deadline_task_to_abort = cancel_before_start
                        .is_some()
                        .then(|| record.deadline_task.take())
                        .flatten();
                    // Sending `Some(reason)` resolves the gate cancelled:
                    // the driver sends that exact reason before Delegate
                    // and never allows child semantic work to begin.
                    // Sending `None` opens the normal gate. The release is
                    // synchronous under the same mutex acquisition, so
                    // start-vs-cancel has exactly one arbitration boundary.
                    let _ = start_gate.send(cancel_before_start);
                    deadline_task_to_abort
                };
                abort_deadline_task(deadline_task_to_abort);
                let registry = self.clone_for_task();
                let settlement_id = subagent_id.clone();
                tokio::spawn(async move {
                    let settlement = task.await.unwrap_or_else(|_| {
                        PhysicalSettlement::of(PhysicalOutcome::ControlFailure {
                            diagnostic: "the child driver task failed".to_owned(),
                        })
                    });
                    registry.settle_from_driver(&settlement_id, settlement);
                });
                Ok(SubagentStartOutcome::Accepted(SubagentAccepted {
                    subagent_id,
                    child_agent_id,
                    child_conversation_id,
                    agent: agent.as_str().to_owned(),
                    definition_digest: definition_digest.as_str().to_owned(),
                }))
            }
        }
    }

    fn clone_for_task(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            state_version: self.state_version.clone(),
            provider_available: self.provider_available.clone(),
            publication_authority: Arc::clone(&self.publication_authority),
            workspace_disposal_lock: Arc::clone(&self.workspace_disposal_lock),
        }
    }

    /// The consistency snapshot of one subagent, if the registry knows it.
    #[must_use]
    pub fn snapshot(&self, subagent_id: &SubagentId) -> Option<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .index
            .get(subagent_id)
            .map(|&index| state.records[index].snapshot())
    }

    /// **Observation plane (Issue #178).** Applies one live activity
    /// projection reported by the child driver.
    ///
    /// This is a synchronous, lock-brief read-model update: it never touches
    /// lifecycle, the journal, or the mailbox, and it never blocks on any
    /// observation consumer. Two drop rules keep the projection honest:
    ///
    /// - post-terminal (or terminal-publishing) updates are dropped, so
    ///   late activity can never resurrect live-ness after settlement;
    /// - stale or reordered revisions are dropped, preserving latest-value
    ///   semantics under coalesced delivery.
    pub fn apply_activity(&self, subagent_id: &SubagentId, observation: SubagentObservation) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        let record = &mut state.records[index];
        if record.lifecycle.is_terminal()
            || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal)
        {
            return;
        }
        if observation.revision <= record.observation.revision {
            return;
        }
        record.observation = observation;
        publish_activity_snapshot(&mut state, &self.state_version, index);
    }

    /// Applies one child-owned interaction request to the root-facing
    /// projection cache. The request is accepted only from the active child
    /// incarnation whose conversation identity it names; the cache has no
    /// authority to settle the child.
    pub(crate) fn apply_child_interaction_requested(
        &self,
        subagent_id: &SubagentId,
        request: InteractionRequest,
    ) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        let record = &state.records[index];
        if !record.lifecycle.is_active()
            || record.publication_abandoned
            || record.child_conversation_id != request.conversation_id
        {
            return;
        }
        let routed = RoutedInteraction::subagent(
            record.subagent_id.clone(),
            record.child_conversation_id.clone(),
            record.agent.clone(),
            request,
        );
        let previous = state
            .routed_interactions
            .insert(routed.interaction.clone(), routed.clone());
        debug_assert!(
            previous.is_none(),
            "a live child interaction identity was reused"
        );
        if let Some(observer) = state.observer.clone() {
            observer.on_interaction_pending(&routed);
        }
    }

    /// Applies one child-owned terminal interaction transition to the
    /// root-facing projection. The originating child coordinator has already
    /// selected and durably audited the outcome; this method only removes the
    /// presentation entry and forwards its route event.
    pub(crate) fn apply_child_interaction_settled(
        &self,
        subagent_id: &SubagentId,
        interaction: &InteractionRef,
        outcome: &InteractionOutcome,
    ) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        let record = &state.records[index];
        if record.child_conversation_id != interaction.conversation_id {
            return;
        }
        let Some(_) = state.routed_interactions.remove(interaction) else {
            // Duplicate/late child route events are harmless projection
            // duplicates. The coordinator's own pending map already made the
            // semantic transition exactly once.
            return;
        };
        if let Some(observer) = state.observer.clone() {
            observer.on_interaction_settled(interaction, outcome);
        }
    }

    /// Forwards a root response to the live child identified by the routed
    /// conversation identity. The response id is transport correlation only;
    /// the child coordinator validates the semantic target and response.
    pub(crate) async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: crate::runtime::interaction::InteractionResponse,
    ) -> Result<(), RoutedInteractionError> {
        let (control, response_id, result, receiver) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.routed_interactions.contains_key(interaction) {
                return Err(RoutedInteractionError::NotPending {
                    interaction: interaction.clone(),
                });
            }
            let control = state
                .records
                .iter()
                .find(|record| {
                    record.child_conversation_id == interaction.conversation_id
                        && record.lifecycle.is_active()
                        && !record.publication_abandoned
                })
                .and_then(|record| record.control.clone());
            let Some(control) = control else {
                return Err(RoutedInteractionError::NotPending {
                    interaction: interaction.clone(),
                });
            };
            let response_id = state.next_response_id;
            if response_id == 0 {
                return Err(RoutedInteractionError::NotPending {
                    interaction: interaction.clone(),
                });
            }
            state.next_response_id = response_id.checked_add(1).unwrap_or(0);
            let (result, receiver) = tokio::sync::oneshot::channel();
            (control, response_id, result, receiver)
        };
        // Keep the response's semantic identity intact across the process
        // boundary; only the response_id is newly allocated transport data.
        if control
            .send(super::process::DriverCommand::InteractionRespond {
                response_id,
                interaction: interaction.clone(),
                response,
                result,
            })
            .await
            .is_err()
        {
            return Err(RoutedInteractionError::NotPending {
                interaction: interaction.clone(),
            });
        }
        receiver.await.unwrap_or_else(|_| {
            Err(RoutedInteractionError::NotPending {
                interaction: interaction.clone(),
            })
        })
    }

    /// The durably committed Workflow output value of one settled
    /// Workflow-owned child (Issue #83).
    ///
    /// This is the live Workflow result channel: the value was validated
    /// and committed atomically with the child's terminal lifecycle fact,
    /// and it deliberately never rides the observation snapshot (Issue
    /// #178). `None` for any non-Workflow child, any non-successful
    /// settlement, and any record that has not settled.
    #[must_use]
    pub(crate) fn workflow_agent_output(
        &self,
        subagent_id: &SubagentId,
    ) -> Option<serde_json::Value> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let &index = state.index.get(subagent_id)?;
        let record = &state.records[index];
        if record.lifecycle != SubagentLifecycle::Succeeded {
            return None;
        }
        record.terminal_workflow_value.clone()
    }

    /// The consistency snapshots of every known subagent, in ordinal order.
    #[must_use]
    pub fn all_snapshots(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.records.iter().map(SubagentRecord::snapshot).collect()
    }

    /// The registry's bounded authoritative listing for conversation-owned
    /// execution discovery (Issue #180).
    ///
    /// This is the discovery read model of the subagent domain: the registry
    /// alone decides which children exist, in which authoritative order,
    /// which of them are lifecycle-active, and how many matched. The listing
    /// carries at most `limit` snapshots in **reverse ordinal order** — the
    /// most recently started child first — so applying the bound keeps the
    /// children a caller is most likely to still be acting on.
    ///
    /// `matched` reports how many records matched before the bound, so a
    /// caller can report truncation without the registry ever materializing
    /// an unbounded response. `limit` is the caller's materialization
    /// bound and nothing more: the registry has no opinion on how large a
    /// model-facing response may be, and never sees one.
    ///
    /// `active_only` selects the non-terminal (`Running`/`Cancelling`/
    /// `PublishingTerminal`) records exactly as [`SubagentState::is_active`]
    /// defines them; otherwise every record the registry still knows is
    /// listed, terminal ones included.
    ///
    /// This is a pure read: it takes the same lock every query takes and
    /// mutates nothing — no lifecycle, no observation revision, no
    /// notification state, and no observer seam. Listing a child is
    /// indistinguishable, from the child's side, from never having been
    /// listed at all.
    #[must_use]
    pub fn listing(&self, active_only: bool, limit: usize) -> SubagentListing {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let matching = state
            .records
            .iter()
            .rev()
            .filter(|record| !active_only || record.lifecycle.is_active());
        let matched = matching.clone().count();
        let snapshots = matching.take(limit).map(SubagentRecord::snapshot).collect();
        SubagentListing { snapshots, matched }
    }

    /// The unsettled subagents in deterministic ordinal order (drain).
    #[must_use]
    pub fn unsettled_snapshot(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .filter(|record| {
                (record.lifecycle.is_active()
                    || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal))
                    && !record.publication_abandoned
            })
            .map(SubagentRecord::snapshot)
            .collect()
    }

    /// The subagents whose terminal publication was abandoned.
    #[must_use]
    pub fn abandoned_publications(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .filter(|record| record.publication_abandoned)
            .map(SubagentRecord::snapshot)
            .collect()
    }

    /// Whether any terminal notification still owes observable delivery
    /// work (drain observability).
    #[must_use]
    pub fn has_unresolved_delivery_work(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .any(|record| record.notification.has_pending_delivery())
    }

    /// **Cancellation.** Commits the cancellation intent under the
    /// registry lock and forwards it into the driver task.
    ///
    /// The call is synchronous and never blocks on process teardown: the
    /// driver owns the Cancel frame, the escalation, and the reap; the
    /// terminal settlement follows through [`Self::settle_from_driver`].
    /// Cancelling an unknown, terminal, or abandoned record is a no-op
    /// returning the current snapshot.
    #[must_use]
    pub fn cancel(
        &self,
        subagent_id: &SubagentId,
        reason: CancellationReason,
    ) -> Option<SubagentSnapshot> {
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return self.snapshot(subagent_id);
        }
        // Test-only: park the first live cancellation contender (the fired
        // deadline task or an explicit caller) immediately before the
        // registry mutex that commits `Running -> Cancelling`. The pause
        // holds no registry lock, so a second cancellation source — or
        // terminal settlement — can acquire the boundary first; the parked
        // contender's release then observes the already-committed
        // transition and cannot overwrite the winner.
        #[cfg(test)]
        let cancellation_boundary_hook = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.cancellation_boundary_hook.clone()
        };
        #[cfg(test)]
        if let Some(hook) = cancellation_boundary_hook {
            hook.wait();
        }
        let (snapshot, deadline_task) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let &index = state.index.get(subagent_id)?;
            let record = &mut state.records[index];
            if record.lifecycle.is_terminal() || record.publication_abandoned {
                return Some(record.snapshot());
            }
            let deadline_task = if record.lifecycle.is_active() {
                record.deadline_task.take()
            } else {
                None
            };
            if matches!(record.lifecycle, SubagentLifecycle::Running) {
                // This mutex transition is the cancellation linearization
                // point. The first source to change Running to Cancelling
                // owns the reason; later sources can only observe it.
                record.lifecycle = SubagentLifecycle::Cancelling;
                record.cancel_reason = Some(reason);
                if let Some(control) = &record.control {
                    let _ = control.try_send(super::process::DriverCommand::Cancel { reason });
                }
                publish_snapshot(&mut state, &self.state_version, index);
            }
            (state.records[index].snapshot(), deadline_task)
        };
        abort_deadline_task(deadline_task);
        Some(snapshot)
    }

    /// Cancels every active subagent (runtime drain).
    pub fn cancel_all(&self, reason: CancellationReason) {
        let ids: Vec<SubagentId> = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state
                .records
                .iter()
                .filter(|record| {
                    matches!(
                        record.lifecycle,
                        SubagentLifecycle::Running | SubagentLifecycle::Cancelling
                    )
                })
                .map(|record| record.subagent_id.clone())
                .collect()
        };
        for id in ids {
            let _ = self.cancel(&id, reason);
        }
    }

    /// Disposes one retained child worktree through the workspace owner.
    ///
    /// This operation is intentionally outside the logical subagent state
    /// machine. The registry first re-proves the exact retained handoff, then
    /// commits a durable post-terminal disposal intent before allowing the
    /// workspace owner to cross the physical Git boundary. Every later
    /// outcome remains in that separate resource lifecycle, including a
    /// partial worktree-removed state and a final-settlement append failure.
    ///
    /// The physical worktree removal is the destructive linearization point.
    /// The runtime serializes its own requests and re-proves immediately
    /// before invoking Git; external Git mutation in the separate process
    /// window is handled best-effort by the proof and branch
    /// compare-and-delete, not by a claim of atomic proof/use.
    ///
    /// # Errors
    ///
    /// Returns a typed unknown-child, non-terminal, ownership-mismatch, or
    /// backend result without changing the logical lifecycle state. An
    /// unresolved preserved workspace is never treated as absent: physical
    /// disposal first obtains a fresh exact Git proof, and nested-containment
    /// uncertainty remains a hard refusal.
    ///
    /// # Panics
    ///
    /// Panics only if the serialized registry loses the record between the
    /// physical operation and its in-memory resource projection update.
    #[allow(clippy::too_many_lines)] // One ordered resource settlement protocol.
    pub async fn dispose_retained_workspace(
        &self,
        subagent_id: &SubagentId,
    ) -> Result<SubagentWorkspaceDisposal, SubagentWorkspaceDisposalError> {
        let _request = self.workspace_disposal_lock.lock().await;
        let (workspace, handoff, phase, fresh_intent) = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return Err(SubagentWorkspaceDisposalError::UnknownSubagent {
                    subagent_id: subagent_id.clone(),
                });
            };
            let record = &state.records[index];
            if !record.lifecycle.is_terminal() || record.publication_abandoned {
                return Err(SubagentWorkspaceDisposalError::NotTerminal {
                    state: record.snapshot().state,
                });
            }
            match record.workspace_resource_state {
                SubagentWorkspaceResourceState::Disposed => {
                    return Ok(SubagentWorkspaceDisposal::AlreadyDisposed(
                        record.snapshot(),
                    ));
                }
                SubagentWorkspaceResourceState::None => {
                    return Ok(SubagentWorkspaceDisposal::NoRetainedWorkspace(
                        record.snapshot(),
                    ));
                }
                SubagentWorkspaceResourceState::Retained => {
                    let Some(handoff) = record.handoff.clone() else {
                        return Err(SubagentWorkspaceDisposalError::Backend {
                            detail: "retained workspace state has no handoff".to_owned(),
                        });
                    };
                    (
                        record.workspace.clone(),
                        Some(handoff),
                        WorkspaceDisposalPhase::Authorized,
                        true,
                    )
                }
                SubagentWorkspaceResourceState::PreservedUnresolved => {
                    let Some(unresolved) = record.workspace_unresolved.clone() else {
                        return Err(SubagentWorkspaceDisposalError::Backend {
                            detail: "unresolved workspace state has no durable safety authority"
                                .to_owned(),
                        });
                    };
                    if unresolved.reason == WorkspaceUnresolvedReason::NestedContainment {
                        return Err(SubagentWorkspaceDisposalError::OwnershipMismatch {
                            detail: format!(
                                "workspace disposal is refused while nested process containment remains unresolved: {}",
                                unresolved.detail
                            ),
                        });
                    }
                    (
                        record.workspace.clone(),
                        None,
                        WorkspaceDisposalPhase::Authorized,
                        true,
                    )
                }
                SubagentWorkspaceResourceState::DisposalInProgress
                | SubagentWorkspaceResourceState::WorktreeRemoved => {
                    let Some(disposal) = record.workspace_disposal.clone() else {
                        return Err(SubagentWorkspaceDisposalError::Backend {
                            detail: "disposal state has no durable resource authority".to_owned(),
                        });
                    };
                    (
                        record.workspace.clone(),
                        Some(disposal.handoff),
                        disposal.phase,
                        false,
                    )
                }
            }
        };

        let _settlement = self
            .config
            .mailbox
            .begin_settlement_admission()
            .map_err(|error| SubagentWorkspaceDisposalError::Backend {
                detail: error.to_string(),
            })?;

        let handoff = if fresh_intent {
            let handoff = if let Some(handoff) = handoff {
                self.config
                    .workspace
                    .prove_retained_workspace(subagent_id, &workspace, &handoff)
                    .await
                    .map_err(map_workspace_disposal_error)?;
                handoff
            } else {
                self.config
                    .workspace
                    .reprove_unresolved_workspace(subagent_id, &workspace)
                    .await
                    .map_err(map_workspace_disposal_error)?
            };
            // The child start timestamp is immutable and gives an ambiguous
            // retry of the canonical intent the same frozen envelope. The
            // event identity is still keyed by SubagentId, while the handoff
            // binding prevents it from widening into another resource.
            let timestamp = {
                let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let index = *state
                    .index
                    .get(subagent_id)
                    .expect("workspace disposal record remains owned by the serialized registry");
                state.records[index].started_at
            };
            let intent = super::workspace_disposal_started_event(
                &self.config.conversation_id,
                subagent_id,
                &handoff,
                timestamp,
            );
            self.config
                .mailbox
                .commit_subagent_workspace_disposal_intent(intent)
                .map_err(|error| SubagentWorkspaceDisposalError::Backend {
                    detail: error.to_string(),
                })?;
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let index = *state
                .index
                .get(subagent_id)
                .expect("workspace disposal record remains owned by the serialized registry");
            let record = &mut state.records[index];
            record.handoff = None;
            record.workspace_resource_state = SubagentWorkspaceResourceState::DisposalInProgress;
            record.workspace_unresolved = None;
            record.workspace_disposal = Some(WorkspaceDisposalRecord {
                handoff: handoff.clone(),
                phase: WorkspaceDisposalPhase::Authorized,
            });
            publish_workspace_snapshot(&mut state, &self.state_version, index);
            handoff
        } else {
            handoff.expect("continuing disposal has an exact durable handoff")
        };

        let physical = self
            .config
            .workspace
            .dispose_authorized_workspace(subagent_id, &workspace, &handoff, phase)
            .await
            .map_err(map_workspace_disposal_error)?;

        let was_already_disposed = matches!(physical, WorkspaceDisposalSettlement::AlreadyDisposed);
        let durable_phase = match &physical {
            WorkspaceDisposalSettlement::NothingRemoved { detail } => {
                return Err(SubagentWorkspaceDisposalError::Backend {
                    detail: format!("physical disposal did not remove the worktree: {detail}"),
                });
            }
            WorkspaceDisposalSettlement::WorktreeRemoved { .. } => {
                self.settle_workspace_resource_phase(
                    subagent_id,
                    &handoff,
                    WorkspaceDisposalPhase::WorktreeRemoved,
                    SubagentWorkspaceResourceState::WorktreeRemoved,
                );
                let event = super::workspace_disposal_settled_event(
                    &self.config.conversation_id,
                    subagent_id,
                    &handoff,
                    crate::events::types::SubagentWorkspaceDisposalSettlement::WorktreeRemoved,
                    self.workspace_disposal_timestamp(subagent_id),
                );
                if let Err(error) = self
                    .config
                    .mailbox
                    .commit_subagent_workspace_disposal_settlement(event)
                {
                    return Err(SubagentWorkspaceDisposalError::Backend {
                        detail: error.to_string(),
                    });
                }
                return Ok(SubagentWorkspaceDisposal::DisposalPending(
                    self.snapshot(subagent_id)
                        .expect("workspace disposal record remains owned"),
                ));
            }
            WorkspaceDisposalSettlement::Disposed
            | WorkspaceDisposalSettlement::AlreadyDisposed => {
                self.settle_workspace_resource_phase(
                    subagent_id,
                    &handoff,
                    WorkspaceDisposalPhase::PhysicalResourcesRemoved,
                    SubagentWorkspaceResourceState::DisposalInProgress,
                );
                crate::events::types::SubagentWorkspaceDisposalSettlement::Disposed
            }
        };
        // The physical branch compare-delete has completed at this point.
        // Keep a deterministic crash seam between that irreversible step and
        // the durable settlement append so recovery is tested against the
        // exact intent-only, fully-removed state.
        crate::runtime::process_death::reach("after:subagent_workspace_branch_cleanup");
        let event = super::workspace_disposal_settled_event(
            &self.config.conversation_id,
            subagent_id,
            &handoff,
            durable_phase,
            self.workspace_disposal_timestamp(subagent_id),
        );
        if let Err(error) = self
            .config
            .mailbox
            .commit_subagent_workspace_disposal_settlement(event)
        {
            return Err(SubagentWorkspaceDisposalError::Backend {
                detail: error.to_string(),
            });
        }
        let snapshot = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let index = *state
                .index
                .get(subagent_id)
                .expect("workspace disposal record remains owned by the serialized registry");
            let record = &mut state.records[index];
            record.workspace_resource_state = SubagentWorkspaceResourceState::Disposed;
            record.workspace_disposal = None;
            record.workspace_unresolved = None;
            record.handoff = None;
            publish_workspace_snapshot(&mut state, &self.state_version, index);
            state.records[index].snapshot()
        };
        if was_already_disposed {
            Ok(SubagentWorkspaceDisposal::AlreadyDisposed(snapshot))
        } else {
            Ok(SubagentWorkspaceDisposal::Disposed(snapshot))
        }
    }

    fn workspace_disposal_timestamp(&self, subagent_id: &SubagentId) -> DateTime<Utc> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let index = *state
            .index
            .get(subagent_id)
            .expect("workspace disposal record remains owned by the serialized registry");
        state.records[index].started_at
    }

    fn settle_workspace_resource_phase(
        &self,
        subagent_id: &SubagentId,
        handoff: &WorkspaceHandoff,
        phase: WorkspaceDisposalPhase,
        resource_state: SubagentWorkspaceResourceState,
    ) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let index = *state
            .index
            .get(subagent_id)
            .expect("workspace disposal record remains owned by the serialized registry");
        let record = &mut state.records[index];
        record.handoff = None;
        record.workspace_resource_state = resource_state;
        record.workspace_unresolved = None;
        record.workspace_disposal = Some(WorkspaceDisposalRecord {
            handoff: handoff.clone(),
            phase,
        });
        publish_workspace_snapshot(&mut state, &self.state_version, index);
    }

    /// Waits until one subagent is settled or abandoned (runtime drain;
    /// never agent-loop blocking).
    pub async fn wait_until_settled(&self, subagent_id: &SubagentId) -> Option<SubagentSnapshot> {
        let mut rx = self.state_version.subscribe();
        loop {
            let snapshot = self.snapshot(subagent_id)?;
            if snapshot.settled || snapshot.publication_abandoned {
                return Some(snapshot);
            }
            if rx.changed().await.is_err() {
                return None;
            }
        }
    }

    /// Resolves with the newest snapshot of `subagent_id` once `predicate`
    /// holds for it; `None` if the record does not exist. Driven by the
    /// registry state-version watch — no polling.
    pub async fn wait_for_snapshot(
        &self,
        subagent_id: &SubagentId,
        predicate: impl Fn(&SubagentSnapshot) -> bool,
    ) -> Option<SubagentSnapshot> {
        let mut rx = self.state_version.subscribe();
        loop {
            let snapshot = self.snapshot(subagent_id)?;
            if predicate(&snapshot) {
                return Some(snapshot);
            }
            if rx.changed().await.is_err() {
                return None;
            }
        }
    }

    /// **Settlement.** Canonicalizes the driver's physical outcome against
    /// the lifecycle, then drives the durable result acceptance.
    ///
    /// Cancellation intent is canonical once its typed frame is delivered:
    /// a physical loss caused by that cancellation settles as cancelled with
    /// the registry's committed reason. An unexpected loss with proven
    /// physical settlement is Interrupted; a low-level control or
    /// containment failure stays an explicit Failed infrastructure
    /// outcome. The durable compound transaction makes the publication
    /// exactly-once.
    #[allow(clippy::too_many_lines)] // one coherent physical-to-durable settlement pipeline
    fn settle_from_driver(&self, subagent_id: &SubagentId, settlement: PhysicalSettlement) {
        // Child death is the end of interaction actionability even when the
        // parent mailbox has already entered a degraded/draining state and
        // cannot accept another lifecycle publication. Remove presentation
        // entries first; the terminal lifecycle snapshot below is separate.
        self.remove_child_interactions(subagent_id);
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return;
        }
        let PhysicalSettlement {
            outcome,
            nested,
            runtime_root_cleanup_error,
            workspace,
        } = settlement;
        // An unproven nested settlement, workspace settlement, or failed
        // exact-root cleanup is a terminal classification input, not an
        // ignorable warning. A successful semantic frame cannot become a
        // parent-facing success until every required physical boundary is
        // proven settled.
        let settlement_diagnostic = [
            nested.unproven_diagnostic(),
            workspace
                .error()
                .map(|detail| format!("the child workspace was not settled: {detail}")),
            runtime_root_cleanup_error
                .as_ref()
                .map(|detail| format!("the child physical runtime root was not removed: {detail}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let settlement_diagnostic =
            (!settlement_diagnostic.is_empty()).then_some(settlement_diagnostic.join("; "));
        let physical_settlement_unproven = !nested.unproven.is_empty()
            || workspace.error().is_some()
            || runtime_root_cleanup_error.is_some();
        let workspace_handoff = workspace.handoff().cloned();
        let (workspace_resource_state, workspace_unresolved) = match &workspace.disposition {
            WorkspaceSettlementDisposition::Shared | WorkspaceSettlementDisposition::Removed => {
                (SubagentWorkspaceResourceState::None, None)
            }
            WorkspaceSettlementDisposition::Retained { .. } => {
                (SubagentWorkspaceResourceState::Retained, None)
            }
            WorkspaceSettlementDisposition::PreservedUnresolved { reason, detail } => (
                SubagentWorkspaceResourceState::PreservedUnresolved,
                Some(WorkspaceUnresolvedRecord {
                    reason: *reason,
                    detail: detail.clone(),
                }),
            ),
        };
        // Test-only: park the terminal settlement path after every physical
        // boundary (nested containment, workspace disposition, runtime root)
        // is settled and immediately before the registry mutex that creates
        // the terminal candidate and commits `... -> PublishingTerminal`.
        // The pause holds no registry lock: while the terminal authority
        // contender is parked here, a concurrent deadline can race the exact
        // authority boundary and commit `Running -> Cancelling` first; the
        // released settlement then observes that intent and must honor it.
        #[cfg(test)]
        let terminal_authority_hook = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.terminal_authority_hook.clone()
        };
        #[cfg(test)]
        if let Some(hook) = terminal_authority_hook {
            hook.wait();
        }
        let candidate = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            let record = &mut state.records[index];
            if record.lifecycle.is_terminal() || record.publication_abandoned {
                return;
            }
            // Terminal candidate creation and timer invalidation share this
            // registry mutex. A deadline that has not already committed
            // cancellation is stopped before this child can publish any
            // terminal fact; a timer that acquired the mutex first has
            // already made the record Cancelling and is reflected below.
            let deadline_task = record.deadline_task.take();
            record.handoff = workspace_handoff;
            record.workspace_resource_state = workspace_resource_state;
            record.workspace_unresolved = workspace_unresolved;
            record.workspace_disposal = None;
            // Lifecycle is the terminal truth; activity is live-only. Every
            // terminal settlement resets the projection to neutral (with a
            // bumped revision, so the reset itself is observable) while
            // keeping the counters and last-activity timestamp as the final
            // record of what the child did.
            record.observation.settle_neutral();
            let cancelling = matches!(record.lifecycle, SubagentLifecycle::Cancelling);
            let workflow_output = matches!(
                &record.terminal,
                SubagentTerminalMode::WorkflowOutput { .. }
            );
            // The publication timestamp freezes at canonicalization: every
            // later bounded retry rebuilds the byte-identical draft, so an
            // ambiguous commit resolves as the idempotent correlation
            // retry, never a conflict.
            let timestamp = self.config.clock.now();
            let candidate = match outcome {
                PhysicalOutcome::Completed(frame) => {
                    match (workflow_output, cancelling, frame.status) {
                        // A Workflow Agent can report `Succeeded` only after the
                        // child Agent Loop has committed the reserved
                        // `workflow_output` latch. That frame is the
                        // cross-process observation of the output/cancellation
                        // linearization point, so a cancellation request that
                        // arrived while the frame was in flight cannot rewrite
                        // the committed value.
                        (true, _, super::ipc::ChildResultStatus::Succeeded)
                        | (false, false, super::ipc::ChildResultStatus::Succeeded) => {
                            TerminalCandidate {
                                state: TerminalState::Succeeded,
                                content: Some(bound_utf8(
                                    frame.content.unwrap_or_default(),
                                    MAX_RESULT_CONTENT_BYTES,
                                )),
                                workflow_value: None,
                                diagnostic: None,
                                reason: None,
                                timestamp,
                            }
                        }
                        (_, false, super::ipc::ChildResultStatus::Failed) => TerminalCandidate {
                            state: TerminalState::Failed,
                            content: None,
                            workflow_value: None,
                            diagnostic: Some(bound_utf8(
                                frame
                                    .diagnostic
                                    .unwrap_or_else(|| "the child attempt failed".to_owned()),
                                MAX_RESULT_CONTENT_BYTES,
                            )),
                            reason: None,
                            timestamp,
                        },
                        (_, false, super::ipc::ChildResultStatus::Cancelled) => TerminalCandidate {
                            state: TerminalState::Cancelled,
                            content: None,
                            workflow_value: None,
                            diagnostic: None,
                            // A child result without a committed parent
                            // cancellation has no semantic reason in its wire
                            // envelope. Do not fabricate UserRequested.
                            reason: None,
                            timestamp,
                        },
                        // Cancellation intent is canonical: a completed frame
                        // after the intent settles as cancelled.
                        (_, true, _) => TerminalCandidate {
                            state: TerminalState::Cancelled,
                            content: None,
                            workflow_value: None,
                            diagnostic: None,
                            reason: record.cancel_reason,
                            timestamp,
                        },
                    }
                }
                PhysicalOutcome::Lost {
                    diagnostic,
                    cancellation_delivered,
                    ..
                } => {
                    if physical_settlement_unproven {
                        // A missing semantic result plus an unproven
                        // containment/cleanup boundary is an explicit
                        // infrastructure failure, never a clean
                        // Interrupted state.
                        TerminalCandidate {
                            state: TerminalState::Failed,
                            content: None,
                            workflow_value: None,
                            diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                            reason: None,
                            timestamp,
                        }
                    } else if cancelling && cancellation_delivered {
                        // The child died after the registry's committed
                        // cancellation was delivered. This includes driver
                        // escalation: physical death cannot erase the
                        // logical cancellation cause.
                        TerminalCandidate {
                            state: TerminalState::Cancelled,
                            content: None,
                            workflow_value: None,
                            diagnostic: None,
                            reason: record.cancel_reason,
                            timestamp,
                        }
                    } else {
                        // The direct process/control plane settled, but no
                        // valid semantic terminal arrived. The outcome is
                        // unknown, not a known model failure.
                        TerminalCandidate {
                            state: TerminalState::Interrupted,
                            content: None,
                            workflow_value: None,
                            diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                            reason: None,
                            timestamp,
                        }
                    }
                }
                PhysicalOutcome::ControlFailure { diagnostic } => TerminalCandidate {
                    // A required process/control operation was not proven.
                    // This is an explicit infrastructure failure, including
                    // after a cancellation intent.
                    state: TerminalState::Failed,
                    content: None,
                    workflow_value: None,
                    diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                    reason: None,
                    timestamp,
                },
            };
            let candidate = match settlement_diagnostic {
                None => candidate,
                Some(diagnostic) if physical_settlement_unproven => {
                    // Physical settlement failure is an infrastructure
                    // failure even when the child emitted a semantic success
                    // (or a cancellation frame). Never carry successful
                    // child content across this failed provenance boundary.
                    let diagnostic = match candidate.diagnostic {
                        Some(existing) => format!("{existing}; {diagnostic}"),
                        None => diagnostic,
                    };
                    TerminalCandidate {
                        state: TerminalState::Failed,
                        content: None,
                        workflow_value: None,
                        diagnostic: Some(bound_utf8(
                            format!(
                                "required child physical settlement was not proven: {diagnostic}"
                            ),
                            MAX_RESULT_CONTENT_BYTES,
                        )),
                        reason: None,
                        timestamp: candidate.timestamp,
                    }
                }
                Some(diagnostic) => TerminalCandidate {
                    diagnostic: Some(bound_utf8(
                        match candidate.diagnostic {
                            Some(existing) => format!("{existing}; {diagnostic}"),
                            None => diagnostic,
                        },
                        MAX_RESULT_CONTENT_BYTES,
                    )),
                    ..candidate
                },
            };
            let candidate = validate_workflow_candidate(&record.terminal, candidate);
            record.pending_terminal = Some(candidate.clone());
            // Terminal authority linearizes under the same registry mutex as
            // cancellation. Once this transition commits, a late deadline
            // or explicit cancel can only observe PublishingTerminal; it
            // cannot create a second cancellation intent or send a driver
            // command after physical settlement has already won.
            record.lifecycle = SubagentLifecycle::PublishingTerminal;
            // Issue #178: the successful answer content never rides the live
            // observation/control projection. It exists only in the durable
            // terminal publication draft (`terminal_publication`, unchanged);
            // `detail` keeps failure/cancellation diagnostics only. The
            // Workflow output value is the separate live Workflow result
            // channel (`workflow_agent_output`), equally kept out of the
            // snapshot.
            record.detail = candidate.diagnostic.clone().or_else(|| {
                candidate
                    .reason
                    .map(|reason| reason_text(reason).to_owned())
            });
            if candidate.state == TerminalState::Succeeded {
                record
                    .terminal_workflow_value
                    .clone_from(&candidate.workflow_value);
            }
            abort_deadline_task(deadline_task);
            candidate
        };
        self.publish_terminal(subagent_id, &candidate);
    }

    /// Removes every actionable interaction owned by one child incarnation.
    ///
    /// The registry deliberately has no terminal outcome to assign here: the
    /// child coordinator is process-owned and dies with the child. The only
    /// root-side fact is that the presentation entry is no longer answerable.
    fn remove_child_interactions(&self, subagent_id: &SubagentId) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        let child_conversation_id = state.records[index].child_conversation_id.clone();
        let mut removed = Vec::new();
        state.routed_interactions.retain(|interaction, _| {
            if interaction.conversation_id == child_conversation_id {
                removed.push(interaction.clone());
                false
            } else {
                true
            }
        });
        if let Some(observer) = state.observer.clone() {
            for interaction in removed {
                observer.on_interaction_removed(&interaction);
            }
        }
    }

    /// Attempts the durable terminal publication; on failure, enters
    /// `PublishingTerminal` and schedules the bounded retry.
    #[allow(clippy::too_many_lines)] // terminal publication is one native linearization boundary
    fn publish_terminal(&self, subagent_id: &SubagentId, candidate: &TerminalCandidate) {
        let initial_failure = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            let record = &state.records[index];
            // A Workflow-owned child has no parent Conversation mailbox
            // result. Its physical terminal candidate is already the native
            // WorkflowRuntime handoff: publication below commits the
            // registry lifecycle and wakes waiters, but deliberately does
            // not create a normal ToolResult/inbound message.
            if matches!(
                &record.terminal,
                SubagentTerminalMode::WorkflowOutput { .. }
            ) {
                let event = terminal_settlement(
                    &self.config.conversation_id,
                    subagent_id,
                    &record.child_agent_id,
                    candidate_state(candidate),
                    &record.terminal_workspace_resource(),
                    candidate.timestamp,
                );
                let result = if candidate.state == TerminalState::Succeeded {
                    match (candidate.workflow_value.clone(), &record.terminal) {
                        (
                            Some(value),
                            SubagentTerminalMode::WorkflowOutput {
                                workflow_id,
                                run_id,
                                node_id,
                                ..
                            },
                        ) => self
                            .config
                            .mailbox
                            .commit_workflow_agent_terminal(
                                event,
                                workflow_output_event(
                                    &self.config.conversation_id,
                                    subagent_id,
                                    workflow_id,
                                    run_id,
                                    node_id,
                                    value,
                                    candidate.timestamp,
                                ),
                            )
                            .map(|_| ())
                            .map_err(|error| error.to_string()),
                        (None, _) => Err(
                            "a successful Workflow terminal candidate has no validated value"
                                .to_owned(),
                        ),
                        (_, _) => Err(
                            "a successful Workflow terminal candidate has no Workflow protocol"
                                .to_owned(),
                        ),
                    }
                } else {
                    self.config
                        .mailbox
                        .commit_subagent_terminal(event)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                };
                match result {
                    Ok(()) => {
                        let record = &mut state.records[index];
                        record.lifecycle = match candidate.state {
                            TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                            TerminalState::Failed => SubagentLifecycle::Failed,
                            TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                            TerminalState::Interrupted => SubagentLifecycle::Interrupted,
                        };
                        record.pending_terminal = None;
                        // Workflow terminalization is a direct native handoff
                        // to the waiting WorkflowRuntime. There is no parent
                        // mailbox delivery phase to represent, so the
                        // ordinary `settled -> delivered` notification state
                        // is deliberately not entered.
                        record.notification = NotificationState::None;
                        publish_snapshot(&mut state, &self.state_version, index);
                        None
                    }
                    Err(error) => {
                        let record = &mut state.records[index];
                        record.lifecycle = SubagentLifecycle::PublishingTerminal;
                        record.notification = NotificationState::Failed;
                        publish_snapshot(&mut state, &self.state_version, index);
                        Some(error)
                    }
                }
            } else {
                let (draft, event) = terminal_publication(
                    &self.config.conversation_id,
                    subagent_id,
                    &record.child_agent_id,
                    candidate_state(candidate),
                    terminal_blocks(record, candidate),
                    &record.terminal_workspace_resource(),
                    candidate.timestamp,
                );
                let result = self.config.mailbox.accept_draft_with_event(draft, event);
                let record = &mut state.records[index];
                match result {
                    Ok(_) => {
                        record.lifecycle = match candidate.state {
                            TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                            TerminalState::Failed => SubagentLifecycle::Failed,
                            TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                            TerminalState::Interrupted => SubagentLifecycle::Interrupted,
                        };
                        record.pending_terminal = None;
                        record.notification = NotificationState::Delivered;
                        publish_snapshot(&mut state, &self.state_version, index);
                        None
                    }
                    Err(error) => {
                        record.lifecycle = SubagentLifecycle::PublishingTerminal;
                        record.notification = NotificationState::Failed;
                        let diagnostic = error.to_string();
                        publish_snapshot(&mut state, &self.state_version, index);
                        Some(diagnostic)
                    }
                }
            }
        };
        let Some(initial_failure) = initial_failure else {
            return;
        };
        // Bounded publication retry; the candidate is stable from
        // pending_terminal.
        let registry = self.clone_for_task();
        let id = subagent_id.clone();
        tokio::spawn(async move {
            let mut diagnostic = initial_failure;
            for _ in 0..2 {
                match registry.retry_terminal_publication(&id) {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(error) => diagnostic = error,
                }
            }
            // Reporting exhausted terminal durability is a callback into the
            // owning ConversationRuntime. It happens only after the bounded
            // retry budget is spent and never while the registry mutex is
            // held; the candidate remains retained as PublishingTerminal.
            registry.report_terminal_publication_failure(&id, &diagnostic);
        });
    }

    /// One bounded publication retry. Returns whether the terminal is now
    /// durably committed.
    #[allow(clippy::too_many_lines)] // retry preserves the same terminal boundary and state machine
    fn retry_terminal_publication(&self, subagent_id: &SubagentId) -> Result<bool, String> {
        let _settlement = self
            .config
            .mailbox
            .begin_settlement_admission()
            .map_err(|error| format!("terminal settlement admission failed: {error}"))?;
        let candidate = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return Ok(true);
            };
            let record = &state.records[index];
            if record.lifecycle.is_terminal() {
                return Ok(true);
            }
            record.pending_terminal.clone()
        };
        let Some(candidate) = candidate else {
            return Ok(true);
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return Ok(true);
        };
        let record = &state.records[index];
        let result = if matches!(
            &record.terminal,
            SubagentTerminalMode::WorkflowOutput { .. }
        ) {
            let event = terminal_settlement(
                &self.config.conversation_id,
                subagent_id,
                &record.child_agent_id,
                candidate_state(&candidate),
                &record.terminal_workspace_resource(),
                candidate.timestamp,
            );
            if candidate.state == TerminalState::Succeeded {
                match (candidate.workflow_value.clone(), &record.terminal) {
                    (
                        Some(value),
                        SubagentTerminalMode::WorkflowOutput {
                            workflow_id,
                            run_id,
                            node_id,
                            ..
                        },
                    ) => self
                        .config
                        .mailbox
                        .commit_workflow_agent_terminal(
                            event,
                            workflow_output_event(
                                &self.config.conversation_id,
                                subagent_id,
                                workflow_id,
                                run_id,
                                node_id,
                                value,
                                candidate.timestamp,
                            ),
                        )
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    _ => {
                        return Err(
                            "a successful Workflow terminal candidate has no validated value"
                                .to_owned(),
                        );
                    }
                }
            } else {
                self.config
                    .mailbox
                    .commit_subagent_terminal(event)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        } else {
            let (draft, event) = terminal_publication(
                &self.config.conversation_id,
                subagent_id,
                &record.child_agent_id,
                candidate_state(&candidate),
                terminal_blocks(record, &candidate),
                &record.terminal_workspace_resource(),
                candidate.timestamp,
            );
            self.config
                .mailbox
                .accept_draft_with_event(draft, event)
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        match result {
            Ok(()) => {
                let record = &mut state.records[index];
                record.lifecycle = match candidate.state {
                    TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                    TerminalState::Failed => SubagentLifecycle::Failed,
                    TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                    TerminalState::Interrupted => SubagentLifecycle::Interrupted,
                };
                record.pending_terminal = None;
                record.publication_abandoned = false;
                record.notification = if matches!(
                    &record.terminal,
                    SubagentTerminalMode::WorkflowOutput { .. }
                ) {
                    NotificationState::None
                } else {
                    NotificationState::Delivered
                };
                publish_snapshot(&mut state, &self.state_version, index);
                Ok(true)
            }
            Err(error) => {
                state.records[index].notification = NotificationState::Failed;
                publish_snapshot(&mut state, &self.state_version, index);
                Err(error)
            }
        }
    }

    /// Reports an exhausted terminal-publication budget to the owning
    /// runtime and only then exposes the explicit abandoned/unresolved fact.
    /// The failure sink is copied while the registry is locked, but invoked
    /// after that guard is dropped: the lock graph is
    /// `ConversationRuntime -> SubagentRegistry`, never the reverse.
    fn report_terminal_publication_failure(&self, subagent_id: &SubagentId, diagnostic: &str) {
        let Ok(_settlement) = self.config.mailbox.begin_settlement_admission() else {
            return;
        };
        let sink = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.failure_sink.clone()
        };
        if let Some(sink) = sink {
            sink.terminal_publication_failed(subagent_id, diagnostic);
        }
        self.mark_publication_abandoned(subagent_id);
    }

    /// Marks a terminal publication abandoned after the bounded retry.
    fn mark_publication_abandoned(&self, subagent_id: &SubagentId) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        state.records[index].publication_abandoned = true;
        publish_snapshot(&mut state, &self.state_version, index);
    }

    /// Installs a commit-boundary hook (tests only).
    #[cfg(test)]
    pub fn install_commit_boundary_hook(&self, hook: Arc<CommitBoundaryHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.commit_hook = Some(hook);
    }

    /// Installs the exact test-only pause between durable ownership/record
    /// publication and the gate-release critical section.
    #[cfg(test)]
    pub fn install_control_handoff_hook(&self, hook: Arc<ControlHandoffHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.control_handoff_hook = Some(hook);
    }

    /// Installs the test-only pause at the exact remaining start-gate edge:
    /// the command handle is installed in the record but the start gate is
    /// not yet released. The pause parks inside the gate-release critical
    /// section while holding the registry mutex.
    #[cfg(test)]
    pub fn install_gate_release_hook(&self, hook: Arc<GateReleaseHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.gate_release_hook = Some(hook);
    }

    /// Installs the test-only pause for the first live cancellation
    /// contender (deadline expiry or an explicit cancel) at the exact
    /// pre-commit edge of the cancellation linearization point: the
    /// contender has passed settlement admission and is parked immediately
    /// before the registry mutex that commits `Running -> Cancelling`.
    #[cfg(test)]
    pub fn install_cancellation_boundary_hook(&self, hook: Arc<CancellationBoundaryHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.cancellation_boundary_hook = Some(hook);
    }

    /// Installs the test-only pause for the terminal settlement path at the
    /// exact pre-commit edge of the terminal-authority linearization point:
    /// physical settlement is complete and the path is parked immediately
    /// before the registry mutex that commits the terminal candidate and
    /// `... -> PublishingTerminal`.
    #[cfg(test)]
    pub fn install_terminal_authority_hook(&self, hook: Arc<TerminalAuthorityHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.terminal_authority_hook = Some(hook);
    }

    /// Test-only: registers a one-shot completion latch for this child's
    /// record-owned deadline task. The receiver resolves only after the
    /// fired deadline's cancellation call has fully returned. Must be
    /// called before the child starts: the owning commit claims the latch
    /// when it creates the deadline task.
    #[cfg(test)]
    pub fn watch_deadline_completion(
        &self,
        subagent_id: &SubagentId,
    ) -> tokio::sync::oneshot::Receiver<()> {
        let (completion, receiver) = tokio::sync::oneshot::channel();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .deadline_completion
            .insert(subagent_id.clone(), completion);
        receiver
    }
}

/// Settles a lease when physical child-root allocation fails before the lease
/// can be transferred to `spawn_staged`. A clean disposable lease is removed;
/// if physical cleanliness cannot be proven, the original start failure is
/// strengthened to a rollback failure and the workspace manager preserves the
/// evidence.
async fn settle_staged_workspace(
    workspace: WorkspaceLease,
    original: SubagentStartError,
) -> SubagentStartError {
    match workspace.settle_staged().await {
        Ok(_) => original,
        Err(error) => SubagentStartError::Rollback {
            detail: format!("{original}; {}", error.detail),
        },
    }
}

/// Revalidates a Workflow terminal candidate at the parent boundary before
/// it is durably accepted. The child already validates against its frozen
/// latch, but the parent owns the cross-process result and must not commit a
/// malformed or schema-invalid value merely because a peer sent a successful
/// frame.
fn validate_workflow_candidate(
    terminal: &SubagentTerminalMode,
    candidate: TerminalCandidate,
) -> TerminalCandidate {
    let SubagentTerminalMode::WorkflowOutput { output_schema, .. } = terminal else {
        return candidate;
    };
    if candidate.state != TerminalState::Succeeded {
        return candidate;
    }
    let value = candidate
        .content
        .as_deref()
        .ok_or_else(|| "the Workflow Agent returned no terminal value".to_owned())
        .and_then(|content| {
            serde_json::from_str::<serde_json::Value>(content)
                .map_err(|error| format!("the Workflow Agent terminal value was not JSON: {error}"))
        })
        .and_then(|value| {
            let validator = jsonschema::Validator::new(output_schema)
                .map_err(|error| format!("the Workflow Agent output schema is invalid: {error}"))?;
            if validator.is_valid(&value) {
                Ok(value)
            } else {
                Err(
                    "the Workflow Agent terminal value violated its frozen output schema"
                        .to_owned(),
                )
            }
        });
    match value {
        Ok(value) => TerminalCandidate {
            workflow_value: Some(value),
            ..candidate
        },
        Err(diagnostic) => TerminalCandidate {
            state: TerminalState::Failed,
            content: None,
            workflow_value: None,
            diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
            reason: None,
            ..candidate
        },
    }
}

/// Builds the bounded content blocks of a terminal publication.
fn terminal_blocks(
    record: &SubagentRecord,
    candidate: &TerminalCandidate,
) -> Vec<crate::message::types::UserContentBlock> {
    let text = match candidate.state {
        TerminalState::Succeeded => candidate.content.clone().unwrap_or_default(),
        TerminalState::Failed => format!(
            "Subagent {} (agent {}) failed: {}",
            record.subagent_id,
            record.agent,
            candidate
                .diagnostic
                .clone()
                .unwrap_or_else(|| "unknown failure".to_owned())
        ),
        TerminalState::Cancelled => format!(
            "Subagent {} (agent {}) was cancelled ({}).",
            record.subagent_id,
            record.agent,
            candidate.reason.map_or("cancelled", reason_text)
        ),
        TerminalState::Interrupted => format!(
            "Subagent {} (agent {}) was interrupted: its actual outcome is unknown and it was not restarted.",
            record.subagent_id, record.agent,
        ),
    };
    vec![crate::message::types::UserContentBlock::Text(
        crate::message::content::TextBlock {
            text: bound_utf8(text, MAX_RESULT_CONTENT_BYTES),
        },
    )]
}

/// Maps the registry's terminal vocabulary onto the durable event's.
const fn candidate_state(candidate: &TerminalCandidate) -> SubagentTerminalState {
    match candidate.state {
        TerminalState::Succeeded => SubagentTerminalState::Succeeded,
        TerminalState::Failed => SubagentTerminalState::Failed,
        TerminalState::Cancelled => SubagentTerminalState::Cancelled,
        TerminalState::Interrupted => SubagentTerminalState::Interrupted,
    }
}

/// Stops the record-owned deadline task after its cancellation/terminal winner
/// has been selected under the registry mutex.
fn abort_deadline_task(task: Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

/// Maps a typed cancellation reason to the parent-facing diagnostic.
const fn reason_text(reason: CancellationReason) -> &'static str {
    match reason {
        CancellationReason::UserRequested => "requested by the user",
        CancellationReason::RuntimeShutdown => "the runtime is shutting down",
        CancellationReason::ParentCancelled => "the parent operation was cancelled",
        CancellationReason::SubagentExecutionDeadlineExceeded => {
            "the subagent execution deadline expired"
        }
    }
}

/// Emits the record's snapshot to the observer and bumps the watch
/// version. Called under the registry lock.
fn publish_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
) {
    let snapshot = state.records[index].snapshot();
    if let Some(observer) = &state.observer {
        observer.on_snapshot(&snapshot);
    }
    version.send_modify(|v| *v += 1);
}

/// Emits a reliable retained-resource projection without classifying the
/// update as another logical subagent lifecycle transition.
fn publish_workspace_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
) {
    let snapshot = state.records[index].snapshot();
    if let Some(observer) = &state.observer {
        observer.on_workspace(&snapshot);
    }
    version.send_modify(|v| *v += 1);
}

/// The disposable sibling of [`publish_snapshot`] for live-activity
/// updates (Issue #178): emits the record's snapshot through
/// [`SubagentObserver::on_activity`] — the publication the consumer may
/// coalesce or drop — and bumps the watch version. Called under the
/// registry lock.
fn publish_activity_snapshot(
    state: &mut RegistryState,
    version: &tokio::sync::watch::Sender<u64>,
    index: usize,
) {
    let snapshot = state.records[index].snapshot();
    if let Some(observer) = &state.observer {
        observer.on_activity(&snapshot);
    }
    version.send_modify(|v| *v += 1);
}

/// A test-only pause inside the ownership-commit critical section
/// (production is unwired; mirrors the background dispatch hook).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct CommitBoundaryHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

/// A test-only pause after the ownership fact and Running record commit,
/// the driver task exists, but before the gate-release critical section.
/// Production has no pause or equivalent semantic state; the hook exists
/// only to force the deterministic cancellation-before-install
/// interleaving in a regression.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct ControlHandoffHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

/// A test-only pause at the exact remaining start-gate edge: the driver
/// command handle is installed in the registry record but the start gate
/// has not yet been released. The pause parks inside the gate-release
/// critical section **while holding the registry mutex**, so a concurrent
/// `cancel` provably blocks on that mutex: the install+release section is
/// atomic with respect to cancellation. Production has no pause and no
/// equivalent semantic state; the hook exists only to prove the remaining
/// edge is serialized, never best-effort.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct GateReleaseHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
impl GateReleaseHook {
    /// Blocks the gate release until [`Self::release`].
    pub fn wait(&self) {
        let mut state = self.state.lock().expect("subagent gate-release hook");
        *state = CommitHookState::Entered;
        self.changed.notify_all();
        while matches!(*state, CommitHookState::Entered) {
            state = self
                .changed
                .wait(state)
                .expect("subagent gate-release hook");
        }
    }

    /// Waits until the gate-release pause has been reached.
    pub fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("subagent gate-release hook");
        while matches!(*state, CommitHookState::Idle) {
            state = self
                .changed
                .wait(state)
                .expect("subagent gate-release hook");
        }
    }

    /// Releases the gate-release pause.
    pub fn release(&self) {
        let mut state = self.state.lock().expect("subagent gate-release hook");
        *state = CommitHookState::Released;
        self.changed.notify_all();
    }
}

#[cfg(test)]
impl ControlHandoffHook {
    /// Blocks the handoff until [`Self::release`].
    pub fn wait(&self) {
        let mut state = self.state.lock().expect("subagent handoff hook");
        *state = CommitHookState::Entered;
        self.changed.notify_all();
        while matches!(*state, CommitHookState::Entered) {
            state = self.changed.wait(state).expect("subagent handoff hook");
        }
    }

    /// Waits until the handoff pause has been reached.
    pub fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("subagent handoff hook");
        while matches!(*state, CommitHookState::Idle) {
            state = self.changed.wait(state).expect("subagent handoff hook");
        }
    }

    /// Releases the handoff pause.
    pub fn release(&self) {
        let mut state = self.state.lock().expect("subagent handoff hook");
        *state = CommitHookState::Released;
        self.changed.notify_all();
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CommitHookState {
    #[default]
    Idle,
    Entered,
    Released,
}

#[cfg(test)]
impl CommitBoundaryHook {
    /// Blocks the commit section until [`Self::release`].
    pub fn wait(&self) {
        let mut state = self.state.lock().expect("subagent commit hook");
        *state = CommitHookState::Entered;
        self.changed.notify_all();
        while matches!(*state, CommitHookState::Entered) {
            state = self.changed.wait(state).expect("subagent commit hook");
        }
    }

    /// Waits until a commit section has entered the hook.
    pub fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("subagent commit hook");
        while matches!(*state, CommitHookState::Idle) {
            state = self.changed.wait(state).expect("subagent commit hook");
        }
    }

    /// Releases the paused commit section.
    pub fn release(&self) {
        let mut state = self.state.lock().expect("subagent commit hook");
        *state = CommitHookState::Released;
        self.changed.notify_all();
    }
}

/// The phases of the one-shot cancellation-boundary pause.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CancellationHookPhase {
    /// No contender has reached the boundary yet; the next arrival parks.
    #[default]
    Armed,
    /// The first contender is parked immediately before the commit; later
    /// arrivals pass straight through.
    Parked,
    /// The parked contender has been released; every later arrival passes
    /// straight through (the pause is spent).
    Open,
}

/// A test-only pause at the exact pre-commit edge of the registry
/// cancellation linearization point. The FIRST live cancellation contender
/// to arrive (deadline expiry or an explicit caller) parks here — after
/// settlement admission and immediately before the registry mutex that
/// commits `Running -> Cancelling` — while contenders that arrive while one
/// is parked (or after the release) pass straight through. A test can
/// therefore prove that the parked contender reached the true commit
/// boundary, let a second contender commit the transition first, and only
/// then release the parked loser, whose release must observe the committed
/// winner. Production has no pause and no equivalent semantic state; the
/// hook exists only to force the deterministic deadline/cancellation
/// interleavings in the race regressions.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct CancellationBoundaryHook {
    state: std::sync::Mutex<CancellationHookPhase>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
impl CancellationBoundaryHook {
    /// Blocks the first arriving contender until [`Self::release`]; every
    /// later arrival (parked or after the release) passes straight through.
    pub fn wait(&self) {
        let mut phase = self.state.lock().expect("subagent cancellation hook");
        if matches!(*phase, CancellationHookPhase::Armed) {
            *phase = CancellationHookPhase::Parked;
            self.changed.notify_all();
            while matches!(*phase, CancellationHookPhase::Parked) {
                phase = self
                    .changed
                    .wait(phase)
                    .expect("subagent cancellation hook");
            }
        }
    }

    /// Waits until the first contender is parked at the boundary.
    pub fn wait_until_parked(&self) {
        let mut phase = self.state.lock().expect("subagent cancellation hook");
        while matches!(*phase, CancellationHookPhase::Armed) {
            phase = self
                .changed
                .wait(phase)
                .expect("subagent cancellation hook");
        }
    }

    /// Releases the parked contender and spends the pause: every later
    /// arrival passes straight through.
    pub fn release(&self) {
        let mut phase = self.state.lock().expect("subagent cancellation hook");
        if matches!(*phase, CancellationHookPhase::Parked) {
            *phase = CancellationHookPhase::Open;
            self.changed.notify_all();
        }
    }
}

/// A test-only pause for the terminal settlement path at the exact
/// pre-commit edge of the terminal-authority linearization point: every
/// required physical boundary (nested containment, workspace disposition,
/// runtime root) is settled and the path is parked immediately before the
/// registry mutex that creates the terminal candidate and commits
/// `... -> PublishingTerminal`. While parked, a concurrent deadline (or
/// explicit cancellation) races the same authority; the pause holds no
/// registry lock. Production has no pause and no equivalent semantic
/// state; the hook exists only to force the deterministic
/// terminal-vs-deadline interleaving in the race regression.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct TerminalAuthorityHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
impl TerminalAuthorityHook {
    /// Blocks the terminal settlement path until [`Self::release`].
    pub fn wait(&self) {
        let mut state = self.state.lock().expect("subagent terminal hook");
        *state = CommitHookState::Entered;
        self.changed.notify_all();
        while matches!(*state, CommitHookState::Entered) {
            state = self.changed.wait(state).expect("subagent terminal hook");
        }
    }

    /// Waits until the terminal settlement path has reached the pause.
    pub fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("subagent terminal hook");
        while matches!(*state, CommitHookState::Idle) {
            state = self.changed.wait(state).expect("subagent terminal hook");
        }
    }

    /// Releases the paused terminal settlement path.
    pub fn release(&self) {
        let mut state = self.state.lock().expect("subagent terminal hook");
        *state = CommitHookState::Released;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::SubagentTerminalState;
    use super::super::catalog::{SubagentExecutionDeadline, SubagentToolSelector};
    use super::super::ipc::{ChildFrame, ChildResultStatus, ParentFrame, ResultFrame};
    use super::*;
    use crate::durable::ConversationStore;
    use crate::runtime::types::{CancellationReason, SystemClock};

    /// A registry over a real (in-memory) durable store with a test seam
    /// for staged children.
    struct TestPlane {
        dir: tempfile::TempDir,
        registry: SubagentRegistry,
        store: Arc<crate::durable::SqliteConversationStore>,
        conversation_id: ConversationId,
        runtime_root: std::path::PathBuf,
        monotonic_clock: Arc<crate::runtime::ManualMonotonicClock>,
        workspace_settlement_hook: Arc<super::super::workspace::WorkspaceSettlementHook>,
        workspace_disposal_hook: Arc<super::super::workspace::WorkspaceDisposalHook>,
    }

    fn plane(max_active: usize) -> TestPlane {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace = dir.path().join("workspace");
        let runtime_root = dir.path().join("runtime");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&runtime_root).expect("runtime root");
        let conversation_id = ConversationId::new("conv-test");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("in-memory store"),
        );
        let mailbox = ConversationInboundMailbox::over_store(store.clone());
        let workspace_settlement_hook =
            Arc::new(super::super::workspace::WorkspaceSettlementHook::new());
        let workspace_disposal_hook =
            Arc::new(super::super::workspace::WorkspaceDisposalHook::new());
        let monotonic_clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
        let mut workspace_manager = SubagentWorkspaceManager::new(&workspace, &runtime_root);
        workspace_manager.install_settlement_hook(workspace_settlement_hook.clone());
        workspace_manager.install_disposal_hook(workspace_disposal_hook.clone());
        let registry = SubagentRegistry::new(SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-parent"),
            mailbox,
            clock: Arc::new(SystemClock),
            monotonic_clock: monotonic_clock.clone(),
            spawn: SubagentSpawnPlan {
                program: std::path::PathBuf::from("/nonexistent/rustx"),
                runtime_root: runtime_root.clone(),
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                agent_status: crate::context::AgentStatusConfig::default(),
                context: SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
            },
            workspace: workspace_manager,
            max_active,
        });
        TestPlane {
            dir,
            registry,
            store,
            conversation_id,
            runtime_root,
            monotonic_clock,
            workspace_settlement_hook,
            workspace_disposal_hook,
        }
    }

    fn git(cwd: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "rustX tests")
            .env("GIT_AUTHOR_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "rustX tests")
            .env("GIT_COMMITTER_EMAIL", "rustx-tests@example.invalid")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn ref_exists(path: &std::path::Path, branch: &str) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .expect("git")
            .success()
    }

    fn make_clean_git_workspace(plane: &TestPlane) {
        let workspace = plane.dir.path().join("workspace");
        git(&workspace, &["init"]);
        std::fs::write(workspace.join("tracked.txt"), "committed\n").expect("tracked file");
        git(&workspace, &["add", "tracked.txt"]);
        git(&workspace, &["commit", "-m", "initial"]);
    }

    fn head(workspace: &std::path::Path) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse HEAD");
        assert!(output.status.success(), "git rev-parse HEAD failed");
        String::from_utf8(output.stdout)
            .expect("utf-8 commit")
            .trim()
            .to_owned()
    }

    fn make_dirty_git_workspace(plane: &TestPlane) {
        make_clean_git_workspace(plane);
        let workspace = plane.dir.path().join("workspace");
        std::fs::write(workspace.join("tracked.txt"), "dirty parent\n").expect("dirty file");
    }

    /// A scripted child: one trivial real process (kill/reap semantics) and
    /// the test-held end of the control channel (protocol semantics).
    struct ScriptedChild {
        peer: tokio::net::UnixStream,
        pid: u32,
    }

    /// Stages a scripted child whose process exits immediately; the test
    /// drives the protocol over `peer`.
    fn stage_exit0(plane: &TestPlane) -> ScriptedChild {
        stage_process(plane, "true")
    }

    /// Stages a child with an intentionally uncontainable nested anchor. The
    /// impossible group id is accepted by the wire-shape seam but cannot be
    /// adopted by this process, so terminal workspace settlement must remain
    /// explicitly unresolved.
    fn stage_with_unresolved_anchor(plane: &TestPlane) -> ScriptedChild {
        stage_process_inner(plane, "true", true)
    }

    /// Stages a scripted child whose process ignores everything and must be
    /// killed; used for cancellation-escalation tests.
    fn stage_stubborn(plane: &TestPlane) -> ScriptedChild {
        stage_process(plane, "trap '' TERM; exec sleep 60")
    }

    fn stage_process(plane: &TestPlane, shell: &str) -> ScriptedChild {
        stage_process_inner(plane, shell, false)
    }

    fn stage_process_inner(
        plane: &TestPlane,
        shell: &str,
        unresolved_anchor: bool,
    ) -> ScriptedChild {
        let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("pair");
        let (observation_end, _observation_peer) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(shell)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .expect("scripted child process");
        let pid = child.id().expect("scripted child pid");
        let child_runtime_root = plane.runtime_root.join(format!("test-child-{pid}"));
        std::fs::create_dir_all(&child_runtime_root).expect("child runtime root");
        let mut staged =
            StagedChild::for_test(child, driver_end, observation_end, child_runtime_root);
        if unresolved_anchor {
            staged.retain_for_test(
                crate::runtime::identity::ProcessUnitId::new("unit-unresolved"),
                i32::MAX,
            );
        }
        plane.registry.push_staged_override(staged);
        ScriptedChild {
            peer: test_end,
            pid,
        }
    }

    impl ScriptedChild {
        /// Awaits one parent-to-child control frame with a liveness guard.
        /// The ordering assertions in deadline tests are driven by the
        /// manual monotonic clock and registry watch, not by this guard.
        async fn read_frame(&mut self) -> ParentFrame {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                super::super::ipc::read_parent_frame(&mut self.peer),
            )
            .await
            .expect("driver control liveness")
            .expect("driver frame")
            .expect("parent frame")
        }

        /// Sends the child's terminal frame after the test has established
        /// the intended lifecycle ordering.
        async fn send_result(&mut self, status: ChildResultStatus, content: Option<&str>) {
            super::super::ipc::write_child_frame(
                &mut self.peer,
                &ChildFrame::Result(ResultFrame {
                    status,
                    content: content.map(str::to_owned),
                    diagnostic: None,
                }),
            )
            .await
            .expect("result frame");
        }

        /// Awaits the delegated task and answers with one terminal result.
        async fn complete(mut self, status: ChildResultStatus, content: Option<&str>) {
            let frame = self.read_frame().await;
            assert!(
                matches!(frame, ParentFrame::Delegate(_)),
                "the committed child is delegated first"
            );
            self.send_result(status, content).await;
        }
    }

    /// A Builtin-only frozen specification: the registry owns live child
    /// lifecycle, so resolution is already complete before it is involved.
    fn resolved(agent: &str) -> ResolvedSubagentSpec {
        ResolvedSubagentSpec {
            agent: SubagentName::parse(agent).expect("canonical name"),
            definition_digest: serde_json::from_value(serde_json::json!(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            ))
            .expect("digest"),
            execution_deadline: None,
            workspace_policy: crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
            instructions: "instructions".to_owned(),
            model: crate::model::frozen::test_frozen_model_spec(
                serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
            ),
            tools: Vec::new(),
            skills: Vec::new(),
            project_instructions: Vec::new(),
            materialization:
                crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
        }
    }

    fn spec(task: &str) -> SubagentStartSpec {
        SubagentStartSpec {
            resolved: resolved("explore"),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            task: task.to_owned(),
            context: None,
            tool_call_id: ToolCallId::new("call-1"),
            terminal: SubagentTerminalMode::Normal,
        }
    }

    fn workflow_spec(task: &str) -> SubagentStartSpec {
        SubagentStartSpec {
            terminal: SubagentTerminalMode::WorkflowOutput {
                output_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"summary": {"type": "string"}},
                    "required": ["summary"],
                    "additionalProperties": false
                }),
                workflow_id: crate::runtime::workflow::WorkflowId::parse("test_workflow")
                    .expect("workflow id"),
                run_id: ToolCallId::new("workflow-run"),
                node_id: "agent".to_owned(),
            },
            ..spec(task)
        }
    }

    fn start_spec(task: &str) -> SubagentStartSpec {
        spec(task)
    }

    fn deadline_spec(task: &str, millis: u64) -> SubagentStartSpec {
        let mut spec = start_spec(task);
        spec.resolved.execution_deadline =
            Some(SubagentExecutionDeadline::from_millis(millis).expect("valid test deadline"));
        spec
    }

    async fn wait_for_cancelling(plane: &TestPlane, subagent_id: &SubagentId) -> SubagentSnapshot {
        plane
            .registry
            .wait_for_snapshot(subagent_id, |snapshot| {
                snapshot.state == SubagentState::Cancelling
            })
            .await
            .expect("deadline cancellation intent")
    }

    fn published_terminal_states(
        plane: &TestPlane,
        subagent_id: &SubagentId,
    ) -> Vec<SubagentTerminalState> {
        events(plane)
            .into_iter()
            .filter_map(|event| match event {
                crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                    subagent_id: event_id,
                    state,
                    ..
                } if event_id == *subagent_id => Some(state),
                _ => None,
            })
            .collect()
    }

    /// Records every snapshot the registry publishes, in publish order. The
    /// callback runs synchronously under the registry lock (never
    /// coalesced), so the log is an exact observable sequence of lifecycle
    /// transitions — the deterministic side-effect counter for the deadline
    /// race regressions. Activity/workspace projections are never published
    /// by these races, so the sequence is purely lifecycle.
    #[derive(Default)]
    struct RecordingObserver(std::sync::Mutex<Vec<SubagentSnapshot>>);

    impl SubagentObserver for RecordingObserver {
        fn on_snapshot(&self, snapshot: &SubagentSnapshot) {
            self.0
                .lock()
                .expect("recording observer lock")
                .push(snapshot.clone());
        }
    }

    impl RecordingObserver {
        fn states(&self) -> Vec<SubagentState> {
            self.0
                .lock()
                .expect("recording observer lock")
                .iter()
                .map(|snapshot| snapshot.state)
                .collect()
        }
    }

    /// Installs a recording observer on a fresh plane (no records yet), so
    /// the published lifecycle sequence from the ownership commit onward is
    /// captured exactly.
    fn recording_observer(plane: &TestPlane) -> Arc<RecordingObserver> {
        let recorded = Arc::new(RecordingObserver::default());
        plane
            .registry
            .install_observer_and_snapshots(Arc::clone(&recorded) as Arc<dyn SubagentObserver>);
        recorded
    }

    /// Asserts the driver control wire carries no further frame: reads to
    /// EOF (the driver shuts down its write half after physical
    /// settlement), panicking on any frame. Used to prove a losing
    /// cancellation source produced no second driver command after the
    /// winner committed and the terminal result settled the child.
    async fn drain_control_frames(child: &mut ScriptedChild) {
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::ipc::read_parent_frame(&mut child.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame");
        if let Some(frame) = frame {
            panic!("unexpected extra driver frame on the wire: {frame:?}");
        }
    }

    #[tokio::test]
    async fn strict_dirty_parent_fails_before_registry_ownership_commit() {
        let plane = plane(4);
        make_dirty_git_workspace(&plane);
        let mut start = spec("strict workspace");
        start.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };

        let error = plane
            .registry
            .prepare(&start, &CancellationSignal::new())
            .await
            .expect_err("strict dirty parent must reject preparation");
        // Issue #188 ownership: the dirty-parent rejection keeps its typed
        // identity across this boundary. It is structurally distinguishable
        // from an arbitrary Git/worktree failure, from settlement/rollback,
        // and from cancellation — no string is parsed to tell them apart —
        // and it still carries the exact committed HEAD the workspace layer
        // captured before observing the dirty parent.
        let SubagentStartError::WorkspaceDirtyParent { base_commit } = &error else {
            panic!("the dirty-parent reason must survive as its own variant: {error:?}");
        };
        assert_eq!(
            base_commit.as_str(),
            head(&plane.dir.path().join("workspace")),
            "the preserved reason retains the exact captured committed HEAD"
        );
        // The remediation vocabulary is not owned here: this layer states the
        // execution fact, and the native `subagent` tool renders the
        // actionable configuration guidance for the model.
        let message = error.to_string();
        assert!(
            message.contains("uncommitted changes") && message.contains("clean-parent"),
            "unexpected lifecycle dirty-parent diagnostic: {message}"
        );
        for leaked in [
            "requireCleanParent",
            "subagent definition",
            "porcelain",
            "rev-parse",
        ] {
            assert!(
                !message.contains(leaked),
                "the lifecycle diagnostic must not own {leaked:?}: {message}"
            );
        }
        assert!(plane.registry.all_snapshots().is_empty());
        assert!(!events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
        )));
        assert!(!plane.runtime_root.join("worktrees").exists());
    }

    /// Issue #145 removed the temporary #144 refusal: an externally
    /// sourced capability is no longer rejected in `prepare`. The registry
    /// stages the child exactly as it does for any other frozen
    /// specification, and physical realization (with its own identity
    /// verification) happens inside the child, before it answers `Ready`.
    #[tokio::test]
    async fn an_external_origin_requirement_is_no_longer_refused_by_the_registry() {
        let plane = plane(2);
        let mut spec = spec("inspect");
        let definition = crate::tools::types::ToolDefinition {
            id: crate::runtime::identity::ToolId::new("tool-get-issue"),
            name: "get_issue".to_owned(),
            description: "issue".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: crate::tools::types::ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: crate::tools::types::ToolConcurrencyPolicy::Sequential,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: crate::tools::types::ToolReplayPolicy::Never,
            origin: crate::tools::types::ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("github"),
            },
        };
        spec.resolved.tools = vec![super::super::resolver::ResolvedSubagentTool::Mcp {
            server_id: crate::runtime::identity::McpServerId::new("github"),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity: crate::tools::mcp::identity::definition_identity(&definition)
                .expect("an MCP definition has an MCP identity"),
            definition,
        }];
        // The staged override consumes `prepare` before any real process is
        // spawned, so this asserts exactly one thing: the registry no longer
        // has a capability-shaped refusal of its own.
        let (parent, _peer) = tokio::net::UnixStream::pair().expect("control pair");
        let (observation_end, _observation_peer) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a staged stand-in");
        let root = std::env::temp_dir().join(format!(
            "rustx-staged-override-{}-{}",
            std::process::id(),
            "external-origin"
        ));
        std::fs::create_dir_all(&root).expect("root");
        plane.registry.push_staged_override(StagedChild::for_test(
            child,
            parent,
            observation_end,
            root,
        ));
        let prepared = plane
            .registry
            .prepare(&spec, &CancellationSignal::new())
            .await
            .expect("an externally sourced capability no longer refuses staging");
        assert!(
            prepared.staged.retained_anchor_count() == 0,
            "a freshly staged child has anchored no nested process unit yet"
        );
        prepared.staged.rollback().await.expect("rollback");
        // The selector vocabulary is unchanged: #145 removed a physical
        // limitation, not a capability model.
        assert_eq!(
            SubagentToolSelector::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("github"),
                name: "get_issue".to_owned(),
            }
            .canonical(),
            "mcp:github/get_issue"
        );
    }

    async fn start(plane: &TestPlane, spec: &SubagentStartSpec) -> SubagentAccepted {
        let prepared = plane
            .registry
            .prepare(spec, &CancellationSignal::new())
            .await
            .expect("prepared");
        match plane
            .registry
            .commit(prepared, &CancellationSignal::new())
            .await
            .expect("commit")
        {
            SubagentStartOutcome::Accepted(accepted) => accepted,
            SubagentStartOutcome::RolledBack => panic!("no cancellation was requested"),
        }
    }

    /// Starts one real isolated child and leaves its changed worktree at the
    /// terminal retained-handoff boundary. Disposal tests share this helper
    /// so every resource phase begins with the same durable terminal facts.
    async fn retained_git_child(
        plane: &TestPlane,
        task: &str,
    ) -> (SubagentAccepted, SubagentSnapshot) {
        make_clean_git_workspace(plane);
        let child = stage_stubborn(plane);
        let mut spec = start_spec(task);
        spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };
        let accepted = start(plane, &spec).await;
        let workspace = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot")
            .workspace
            .logical_workspace;
        std::fs::write(workspace.join("retained.txt"), "retain this child work\n")
            .expect("child work");
        child
            .complete(ChildResultStatus::Succeeded, Some("child answer"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled snapshot");
        assert_eq!(settled.state, SubagentState::Succeeded);
        assert!(
            settled.handoff.is_some(),
            "the changed worktree is retained"
        );
        (accepted, settled)
    }

    /// Starts one isolated child whose final workspace inspection is
    /// deterministically unresolved. The durable terminal fact therefore
    /// carries the immutable ownership snapshot plus an unresolved resource
    /// disposition, but no fabricated handoff.
    async fn unresolved_git_child(
        plane: &TestPlane,
        task: &str,
    ) -> (SubagentAccepted, SubagentSnapshot) {
        make_clean_git_workspace(plane);
        let child = stage_exit0(plane);
        let mut spec = start_spec(task);
        spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };
        plane
            .workspace_settlement_hook
            .fail_next("injected final workspace inspection failure");
        let accepted = start(plane, &spec).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("child answer"))
            .await;
        plane.workspace_settlement_hook.wait_until_entered().await;
        plane.workspace_settlement_hook.release().await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled unresolved child");
        assert_eq!(
            settled.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(settled.handoff.is_none());
        (accepted, settled)
    }

    fn commit_disposal_intent(plane: &TestPlane, snapshot: &SubagentSnapshot) {
        let handoff = snapshot.handoff.as_ref().expect("retained handoff");
        let event = crate::runtime::subagent::workspace_disposal_started_event(
            &plane.conversation_id,
            &snapshot.subagent_id,
            handoff,
            snapshot.started_at,
        );
        plane
            .store
            .commit_subagent_workspace_disposal_intent(event)
            .expect("durable disposal intent");
    }

    fn recovered_registry(
        plane: &TestPlane,
    ) -> (SubagentRegistry, crate::runtime::recovery::RecoveryPlan) {
        let evidence =
            crate::runtime::recovery::RecoveryEvidence::reconstruct(plane.store.as_ref())
                .expect("recovery evidence");
        let plan = crate::runtime::recovery::RecoveryPlan::classify(&evidence);
        let registry = SubagentRegistry::new(plane.registry.config.clone());
        for handoff in plan.settled_subagent_handoffs() {
            registry.restore_recovered_handoff(handoff);
        }
        for unresolved in plan.settled_subagent_unresolved() {
            registry.restore_recovered_unresolved(unresolved);
        }
        for disposal in plan.settled_subagent_disposals() {
            registry.restore_recovered_disposal(disposal);
        }
        (registry, plan)
    }

    fn assert_recovered_unresolved_does_not_rearm_deadline(
        plane: &TestPlane,
        recovered: &SubagentRegistry,
        plan: &crate::runtime::recovery::RecoveryPlan,
        subagent_id: &SubagentId,
    ) {
        assert_eq!(plan.settled_subagent_unresolved().len(), 1);
        assert_eq!(
            plan.settled_subagent_unresolved()[0].reason,
            crate::runtime::subagent::WorkspaceUnresolvedReason::NestedContainment
        );
        let recovered_snapshot = recovered
            .snapshot(subagent_id)
            .expect("recovered unresolved resource");
        assert_eq!(recovered_snapshot.state, SubagentState::Failed);
        assert_eq!(
            recovered_snapshot.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(recovered_snapshot.handoff.is_none());

        // Recovery projects durable terminal/resource facts only. It never
        // creates fresh deadline authority for a terminal record.
        let recovered_event_count = events(plane).len();
        plane
            .monotonic_clock
            .advance(super::super::catalog::MAX_SUBAGENT_EXECUTION_DEADLINE_MS);
        assert_eq!(
            recovered
                .snapshot(subagent_id)
                .expect("terminal snapshot")
                .state,
            SubagentState::Failed
        );
        let _ = recovered.cancel(subagent_id, CancellationReason::UserRequested);
        assert_eq!(events(plane).len(), recovered_event_count);
    }

    /// Reads the durable event journal.
    fn events(plane: &TestPlane) -> Vec<crate::events::types::RuntimeEvent> {
        let mut all = Vec::new();
        let mut cursor = None;
        loop {
            let page = plane.store.read_events(cursor, 100).expect("events");
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

    #[tokio::test]
    async fn a_successful_child_settles_through_the_durable_inbound() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect the workspace")).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("the answer"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Succeeded);
        // Issue #178: the successful answer content never rides the live
        // observation/control projection — `detail` is diagnostics-only,
        // and the durable pending inbound below is the one result channel.
        assert_eq!(settled.detail, None);
        // The result entered the parent's durable pending inbound with the
        // child agent provenance, exactly once.
        let pending = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("one pending batch");
        assert_eq!(pending.items.len(), 1);
        let item = &pending.items[0];
        assert_eq!(
            item.correlation.as_deref(),
            Some(super::super::terminal_correlation(&accepted.subagent_id).as_str())
        );
        assert!(matches!(
            item.message.source,
            crate::message::types::UserSource::Agent { ref agent_id }
                if *agent_id == accepted.child_agent_id
        ));
        let journal = events(&plane);
        assert!(journal.iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. }
                if *subagent_id == accepted.subagent_id
        )));
        assert!(journal.iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                state: SubagentTerminalState::Succeeded,
                ..
            } if *subagent_id == accepted.subagent_id
        )));
    }

    #[tokio::test]
    async fn a_workflow_child_closes_native_lifecycle_without_parent_inbound() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &workflow_spec("return a structured result")).await;
        child
            .complete(
                ChildResultStatus::Succeeded,
                Some(r#"{"summary":"answer"}"#),
            )
            .await;

        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Succeeded);
        assert!(
            plane
                .store
                .select_pending_batch()
                .expect("pending")
                .is_none()
        );
        let journal = events(&plane);
        assert!(journal.iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalSettled {
                subagent_id,
                state: SubagentTerminalState::Succeeded,
                ..
            } if *subagent_id == accepted.subagent_id
        )));
        assert!(!journal.iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id, ..
            } if *subagent_id == accepted.subagent_id
        )));
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::WorkflowAgentOutputCommitted {
                        subagent_id, output, ..
                    } if *subagent_id == accepted.subagent_id
                        && *output == serde_json::json!({"summary": "answer"})
                ))
                .count(),
            1,
            "the Workflow value is committed exactly once with the child terminal fact"
        );
        // The live Workflow result channel is the registry's committed
        // value, not the observation snapshot (Issue #178): the snapshot
        // detail is diagnostics-only even for a successful Workflow child.
        assert_eq!(settled.detail, None);
        assert_eq!(
            plane.registry.workflow_agent_output(&accepted.subagent_id),
            Some(serde_json::json!({"summary": "answer"})),
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // This is the end-to-end lifecycle/resource regression.
    async fn successful_worktree_child_publishes_a_valid_handoff_and_client_projection() {
        let plane = plane(4);
        make_clean_git_workspace(&plane);
        let child = stage_stubborn(&plane);
        let mut spec = start_spec("write a source change");
        spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };
        let accepted = start(&plane, &spec).await;
        let workspace = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot")
            .workspace
            .logical_workspace;
        std::fs::write(workspace.join("child-work.txt"), "retain me\n").expect("child work");

        child
            .complete(ChildResultStatus::Succeeded, Some("the answer"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");

        assert_eq!(settled.state, SubagentState::Succeeded);
        let handoff = settled.handoff.clone().expect("handoff");
        assert!(handoff.dirty);
        assert_eq!(handoff.base_commit, handoff.head_commit);
        let view = crate::runtime_client::projection::subagent_view(&settled);
        assert_eq!(
            view.workspace
                .handoff
                .as_ref()
                .map(|item| &item.physical_worktree_root),
            Some(&handoff.physical_worktree_root)
        );
        assert!(events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                state: SubagentTerminalState::Succeeded,
                workspace_resource:
                    crate::events::types::SubagentWorkspaceTerminalResource::Retained {
                        handoff: actual,
                    },
                ..
            } if *subagent_id == accepted.subagent_id && actual == &handoff
        )));

        let disposed = plane
            .registry
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("the retained resource is disposable");
        let SubagentWorkspaceDisposal::Disposed(after_disposal) = disposed else {
            panic!("the first disposal must remove the retained resource");
        };
        assert_eq!(after_disposal.state, SubagentState::Succeeded);
        assert!(after_disposal.handoff.is_none());

        std::fs::create_dir_all(&handoff.physical_worktree_root).expect("replacement path");
        std::fs::write(
            handoff.physical_worktree_root.join("replacement-sentinel"),
            "leave this unrelated path alone\n",
        )
        .expect("replacement sentinel");
        let repeated = plane
            .registry
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("repeated disposal is deterministic");
        let SubagentWorkspaceDisposal::AlreadyDisposed(after_repeat) = repeated else {
            panic!("the second disposal must be an idempotent outcome");
        };
        assert_eq!(after_repeat.state, SubagentState::Succeeded);
        assert!(after_repeat.handoff.is_none());
        assert_eq!(
            std::fs::read_to_string(handoff.physical_worktree_root.join("replacement-sentinel"))
                .expect("replacement path remains untouched"),
            "leave this unrelated path alone\n"
        );

        let journal = events(&plane);
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                        subagent_id,
                        ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "workspace disposal never adds a second logical terminal event"
        );
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalSettled {
                        subagent_id,
                        settlement:
                            crate::events::types::SubagentWorkspaceDisposalSettlement::Disposed,
                        ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "repeated disposal does not append another final settlement fact"
        );
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalStarted {
                        subagent_id,
                        ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "repeated disposal does not append another intent"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn worktree_removed_branch_failure_settles_as_pending_and_survives_recovery() {
        let plane = plane(4);
        let (accepted, settled) = retained_git_child(&plane, "partial disposal").await;
        let handoff = settled.handoff.clone().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();

        plane
            .workspace_disposal_hook
            .fail_branch_cleanup("injected branch settlement failure");
        let first = plane
            .registry
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("partial physical settlement is a successful resource outcome");
        let SubagentWorkspaceDisposal::DisposalPending(first_snapshot) = first else {
            panic!("worktree removal plus branch failure must be pending");
        };
        assert_eq!(
            first_snapshot.workspace_resource_state,
            SubagentWorkspaceResourceState::WorktreeRemoved
        );
        assert!(first_snapshot.handoff.is_none());
        assert!(
            !physical.exists(),
            "the removed worktree is never re-advertised"
        );
        assert!(ref_exists(&plane.dir.path().join("workspace"), &branch));
        assert!(events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalSettled {
                subagent_id,
                settlement: crate::events::types::SubagentWorkspaceDisposalSettlement::WorktreeRemoved,
                ..
            } if *subagent_id == accepted.subagent_id
        )));

        // A fresh registry projection comes only from the durable intent and
        // partial settlement. It never falls back to ordinary Retained.
        let (recovered, plan) = recovered_registry(&plane);
        assert_eq!(plan.settled_subagent_disposals().len(), 1);
        assert_eq!(
            plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::WorktreeRemoved
        );
        let recovered_snapshot = recovered
            .snapshot(&accepted.subagent_id)
            .expect("recovered pending resource");
        assert_eq!(
            recovered_snapshot.workspace_resource_state,
            SubagentWorkspaceResourceState::WorktreeRemoved
        );
        assert!(recovered_snapshot.handoff.is_none());

        let completed = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("recovered branch settlement");
        let SubagentWorkspaceDisposal::Disposed(completed_snapshot) = completed else {
            panic!("the exact residual branch should settle on retry");
        };
        assert_eq!(
            completed_snapshot.workspace_resource_state,
            SubagentWorkspaceResourceState::Disposed
        );
        assert!(!ref_exists(&plane.dir.path().join("workspace"), &branch));
        assert!(!physical.exists());
        assert_eq!(
            events(&plane)
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                        subagent_id, ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "resource disposal never adds a logical terminal event"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn moved_branch_after_worktree_removal_is_never_deleted_by_registry_retry() {
        let plane = plane(4);
        let (accepted, settled) = retained_git_child(&plane, "moved branch").await;
        let handoff = settled.handoff.clone().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        plane.workspace_disposal_hook.arm_after_worktree_removal();

        let registry = plane.registry.clone();
        let subagent_id = accepted.subagent_id.clone();
        let request =
            tokio::spawn(async move { registry.dispose_retained_workspace(&subagent_id).await });
        plane
            .workspace_disposal_hook
            .wait_until_worktree_removed()
            .await;
        assert!(!physical.exists(), "the worktree step has committed");

        let parent = plane.dir.path().join("workspace");
        git(
            &parent,
            &["commit", "--allow-empty", "-m", "move runtime branch"],
        );
        let moved_head = head(&parent);
        let reference = format!("refs/heads/{branch}");
        git(
            &parent,
            &["update-ref", reference.as_str(), moved_head.as_str()],
        );
        plane
            .workspace_disposal_hook
            .release_after_worktree_removal()
            .await;

        let first = request
            .await
            .expect("disposal task")
            .expect("partial settlement result");
        assert!(matches!(
            first,
            SubagentWorkspaceDisposal::DisposalPending(snapshot)
                if snapshot.workspace_resource_state
                    == SubagentWorkspaceResourceState::WorktreeRemoved
        ));
        assert!(ref_exists(&parent, &branch), "the moved branch remains");

        let (recovered, plan) = recovered_registry(&plane);
        assert_eq!(
            plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::WorktreeRemoved
        );
        let second = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("retry reports the residual branch");
        assert!(matches!(
            second,
            SubagentWorkspaceDisposal::DisposalPending(snapshot)
                if snapshot.workspace_resource_state
                    == SubagentWorkspaceResourceState::WorktreeRemoved
        ));
        assert!(ref_exists(&parent, &branch));
        assert_eq!(head(&parent), moved_head);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn durable_intent_recovers_before_physical_mutation_and_continues_exactly() {
        let plane = plane(4);
        let (accepted, settled) = retained_git_child(&plane, "recover before deletion").await;
        let handoff = settled.handoff.clone().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        commit_disposal_intent(&plane, &settled);

        let (recovered, plan) = recovered_registry(&plane);
        assert_eq!(
            plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Authorized
        );
        let pending = recovered
            .snapshot(&accepted.subagent_id)
            .expect("intent-only recovery projection");
        assert_eq!(
            pending.workspace_resource_state,
            SubagentWorkspaceResourceState::DisposalInProgress
        );
        assert!(pending.handoff.is_none());
        assert!(physical.exists());
        assert!(ref_exists(&plane.dir.path().join("workspace"), &branch));

        let result = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("authorized recovery continuation");
        assert!(matches!(result, SubagentWorkspaceDisposal::Disposed(_)));
        assert!(!physical.exists());
        assert!(!ref_exists(&plane.dir.path().join("workspace"), &branch));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn crash_after_worktree_removal_recovers_authorized_partial_state() {
        let plane = plane(4);
        let (accepted, settled) =
            retained_git_child(&plane, "recover after worktree removal").await;
        let handoff = settled.handoff.clone().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        commit_disposal_intent(&plane, &settled);

        // Simulate process death after the physical worktree command but
        // before either the branch cleanup or its durable partial settlement.
        // The durable intent is the only recovery authority available.
        plane
            .workspace_disposal_hook
            .fail_branch_cleanup("simulated process death before branch cleanup");
        let physical_result = plane
            .registry
            .config
            .workspace
            .dispose_authorized_workspace(
                &accepted.subagent_id,
                &settled.workspace,
                &handoff,
                WorkspaceDisposalPhase::Authorized,
            )
            .await
            .expect("the physical partial result is typed");
        assert!(matches!(
            physical_result,
            WorkspaceDisposalSettlement::WorktreeRemoved { .. }
        ));
        assert!(!physical.exists());
        assert!(ref_exists(&plane.dir.path().join("workspace"), &branch));

        let (recovered, plan) = recovered_registry(&plane);
        assert_eq!(
            plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Authorized
        );
        let pending = recovered
            .snapshot(&accepted.subagent_id)
            .expect("authorized partial recovery projection");
        assert_eq!(
            pending.workspace_resource_state,
            SubagentWorkspaceResourceState::DisposalInProgress
        );
        assert!(pending.handoff.is_none());

        let result = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("recovery continues exact branch settlement");
        assert!(matches!(result, SubagentWorkspaceDisposal::Disposed(_)));
        assert!(!ref_exists(&plane.dir.path().join("workspace"), &branch));
        assert!(!physical.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn final_settlement_append_failure_recovers_to_already_disposed() {
        let plane = plane(4);
        let (accepted, settled) = retained_git_child(&plane, "recover after deletion").await;
        let handoff = settled.handoff.clone().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        commit_disposal_intent(&plane, &settled);
        let (recovered, _) = recovered_registry(&plane);

        // The next event is the final settlement, so this failure occurs only
        // after both irreversible Git operations have completed.
        plane.store.arm_fail_event_times(1);
        let error = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect_err("the durable final settlement failure is surfaced");
        assert!(matches!(
            error,
            SubagentWorkspaceDisposalError::Backend { .. }
        ));
        let after_failure = recovered
            .snapshot(&accepted.subagent_id)
            .expect("in-memory partial resource projection");
        assert_eq!(
            after_failure.workspace_resource_state,
            SubagentWorkspaceResourceState::DisposalInProgress
        );
        assert!(after_failure.handoff.is_none());
        assert!(!physical.exists());
        assert!(!ref_exists(&plane.dir.path().join("workspace"), &branch));

        // Restart sees the durable intent, not a fabricated retained handoff,
        // and recognizes that both exact resources are already gone.
        let (recovered_again, plan) = recovered_registry(&plane);
        assert_eq!(
            plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Authorized
        );
        let result = recovered_again
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("durable intent makes the fully removed state idempotent");
        assert!(matches!(
            result,
            SubagentWorkspaceDisposal::AlreadyDisposed(_)
        ));
        let final_snapshot = recovered_again
            .snapshot(&accepted.subagent_id)
            .expect("final resource projection");
        assert_eq!(
            final_snapshot.workspace_resource_state,
            SubagentWorkspaceResourceState::Disposed
        );

        let (cold, final_plan) = recovered_registry(&plane);
        assert_eq!(
            final_plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Disposed
        );
        assert!(matches!(
            cold.dispose_retained_workspace(&accepted.subagent_id)
                .await
                .expect("stable repeated disposal"),
            SubagentWorkspaceDisposal::AlreadyDisposed(_)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_disposal_requests_have_one_physical_owner() {
        let plane = plane(4);
        let (accepted, settled) = retained_git_child(&plane, "concurrent disposal").await;
        let handoff = settled.handoff.clone().expect("retained handoff");
        let physical = handoff.physical_worktree_root.clone();
        let branch = handoff.branch.clone();
        plane.workspace_disposal_hook.arm_after_worktree_removal();

        let first_registry = plane.registry.clone();
        let first_id = accepted.subagent_id.clone();
        let first =
            tokio::spawn(async move { first_registry.dispose_retained_workspace(&first_id).await });
        plane
            .workspace_disposal_hook
            .wait_until_worktree_removed()
            .await;
        let second_registry = plane.registry.clone();
        let second_id = accepted.subagent_id.clone();
        let second =
            tokio::spawn(
                async move { second_registry.dispose_retained_workspace(&second_id).await },
            );
        plane
            .workspace_disposal_hook
            .release_after_worktree_removal()
            .await;

        assert!(matches!(
            first.await.expect("first request").expect("first result"),
            SubagentWorkspaceDisposal::Disposed(_)
        ));
        assert!(matches!(
            second
                .await
                .expect("second request")
                .expect("second result"),
            SubagentWorkspaceDisposal::AlreadyDisposed(_)
        ));
        assert!(!physical.exists());
        assert!(!ref_exists(&plane.dir.path().join("workspace"), &branch));
        let journal = events(&plane);
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalStarted {
                        subagent_id, ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1
        );
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalSettled {
                        subagent_id,
                        settlement: crate::events::types::SubagentWorkspaceDisposalSettlement::Disposed,
                        ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1
        );
        assert_eq!(
            journal
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                        subagent_id, ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "concurrent resource requests never duplicate logical terminality"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn semantic_success_with_workspace_settlement_failure_preserves_unresolved_resource() {
        let plane = plane(4);
        make_clean_git_workspace(&plane);
        plane
            .workspace_settlement_hook
            .fail_next("injected final workspace inspection failure");
        let child = stage_stubborn(&plane);
        let mut spec = start_spec("write a source change");
        spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };
        let accepted = start(&plane, &spec).await;

        child
            .complete(
                ChildResultStatus::Succeeded,
                Some("child success must not leak"),
            )
            .await;
        // The manager hook is reached only after the driver has received the
        // semantic result, reaped the direct child, and settled nested units.
        plane.workspace_settlement_hook.wait_until_entered().await;
        plane.workspace_settlement_hook.release().await;

        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Failed);
        assert!(settled.handoff.is_none());
        assert_eq!(
            settled.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        let physical = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace authority")
            .physical_worktree_root
            .clone();
        assert!(
            physical.exists(),
            "unresolved settlement preserves the worktree"
        );
        let detail = settled.detail.as_deref().expect("failure diagnostic");
        assert!(detail.contains("required child physical settlement was not proven"));
        assert!(detail.contains("workspace"));
        assert!(!detail.contains("child success must not leak"));

        let pending = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("terminal notice");
        assert!(matches!(
            pending.items[0].message.source,
            crate::message::types::UserSource::Runtime
        ));
        let parent_notice = match &pending.items[0].message.content[0] {
            crate::message::types::UserContentBlock::Text(text) => &text.text,
            other => panic!("unexpected terminal content: {other:?}"),
        };
        assert!(parent_notice.contains("physical settlement was not proven"));
        assert!(!parent_notice.contains("child success must not leak"));
        assert!(events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                state: SubagentTerminalState::Failed,
                workspace_resource:
                    crate::events::types::SubagentWorkspaceTerminalResource::PreservedUnresolved {
                        reason: crate::runtime::subagent::WorkspaceUnresolvedReason::PhysicalSettlement,
                        ..
                    },
                ..
            } if *subagent_id == accepted.subagent_id
        )));
        assert!(!events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                state: SubagentTerminalState::Succeeded,
                ..
            } if *subagent_id == accepted.subagent_id
        )));
        let view = crate::runtime_client::projection::subagent_view(&settled);
        assert_eq!(view.state, crate::runtime::subagent::SubagentState::Failed);
        assert_eq!(
            view.workspace.resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(view.workspace.handoff.is_none());
        assert_eq!(view.detail.as_deref(), Some(detail));

        let disposal = plane
            .registry
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("unresolved resource uses the normal identity-based disposal path");
        assert!(matches!(
            disposal,
            SubagentWorkspaceDisposal::Disposed(snapshot)
                if snapshot.workspace_resource_state == SubagentWorkspaceResourceState::Disposed
        ));
        assert!(!physical.exists());
    }

    #[tokio::test]
    async fn semantic_success_with_unresolved_nested_containment_is_failed() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        let settlement = PhysicalSettlement {
            outcome: PhysicalOutcome::Completed(ResultFrame {
                status: ChildResultStatus::Succeeded,
                content: Some("child success must not leak".to_owned()),
                diagnostic: None,
            }),
            nested: super::super::anchors::NestedUnitSettlement {
                contained: Vec::new(),
                unproven: vec![(
                    crate::runtime::identity::ProcessUnitId::new("unit-unresolved"),
                    "test containment proof is unavailable".to_owned(),
                )],
            },
            runtime_root_cleanup_error: None,
            workspace: super::super::workspace::WorkspaceSettlement::shared(
                WorkspaceSnapshot::shared(std::path::PathBuf::from("<shared-workspace>")),
            ),
        };
        plane
            .registry
            .settle_from_driver(&accepted.subagent_id, settlement);
        drop(child.peer);

        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Failed);
        assert!(
            settled
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("nested supervised process unit"))
        );
        assert!(
            !settled
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("child success must not leak")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unresolved_nested_containment_preserves_owned_resource_through_recovery() {
        let plane = plane(4);
        make_clean_git_workspace(&plane);
        let child = stage_with_unresolved_anchor(&plane);
        let mut spec = deadline_spec("nested containment", 100);
        spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };
        let accepted = start(&plane, &spec).await;
        let running = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot");
        let physical = running
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .physical_worktree_root
            .clone();
        let parent = plane.dir.path().join("workspace");
        let branch = running
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .branch
            .clone();

        child
            .complete(ChildResultStatus::Succeeded, Some("child answer"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled nested-unresolved child");
        assert_eq!(settled.state, SubagentState::Failed);
        assert_eq!(
            settled.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(settled.handoff.is_none());
        assert!(
            physical.exists(),
            "nested uncertainty preserves the worktree"
        );
        assert!(
            ref_exists(&parent, &branch),
            "the runtime branch remains owned"
        );
        assert!(events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                workspace_resource:
                    crate::events::types::SubagentWorkspaceTerminalResource::PreservedUnresolved {
                        reason: crate::runtime::subagent::WorkspaceUnresolvedReason::NestedContainment,
                        ..
                    },
                ..
            } if *subagent_id == accepted.subagent_id
        )));

        let (recovered, plan) = recovered_registry(&plane);
        assert_recovered_unresolved_does_not_rearm_deadline(
            &plane,
            &recovered,
            &plan,
            &accepted.subagent_id,
        );

        let error = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect_err("nested containment uncertainty is not Git-only authority");
        assert!(matches!(
            error,
            SubagentWorkspaceDisposalError::OwnershipMismatch { detail }
                if detail.contains("nested process containment")
        ));
        let after = recovered
            .snapshot(&accepted.subagent_id)
            .expect("unresolved resource remains visible");
        assert_eq!(
            after.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(physical.exists());
        assert!(ref_exists(&parent, &branch));
        assert!(!events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalStarted {
                subagent_id, ..
            } if *subagent_id == accepted.subagent_id
        )));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unresolved_resource_external_disappearance_fails_closed_without_intent() {
        let plane = plane(4);
        let (accepted, settled) = unresolved_git_child(&plane, "external disappearance").await;
        let worktree = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .physical_worktree_root
            .clone();
        let parent = plane.dir.path().join("workspace");
        let branch = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .branch
            .clone();
        let worktree_arg = worktree.to_str().expect("worktree path");
        git(
            &parent,
            &["worktree", "remove", "--force", "--", worktree_arg],
        );
        assert!(
            !worktree.exists(),
            "the external actor removed the worktree"
        );
        assert!(
            ref_exists(&parent, &branch),
            "the branch was not removed externally"
        );

        let error = plane
            .registry
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect_err("missing physical facts without intent fail closed");
        assert!(matches!(
            error,
            SubagentWorkspaceDisposalError::OwnershipMismatch { .. }
        ));
        let after = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("unresolved resource remains owned");
        assert_eq!(
            after.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(after.handoff.is_none());
        assert!(ref_exists(&parent, &branch), "no unrelated ref was deleted");
        assert!(!events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalStarted {
                subagent_id, ..
            } if *subagent_id == accepted.subagent_id
        )));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unresolved_resource_branch_tampering_fails_closed_without_intent() {
        let plane = plane(4);
        let (accepted, settled) = unresolved_git_child(&plane, "tampered unresolved branch").await;
        let worktree = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .physical_worktree_root
            .clone();
        let parent = plane.dir.path().join("workspace");
        let branch = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .branch
            .clone();
        git(&parent, &["commit", "--allow-empty", "-m", "tamper branch"]);
        let moved_head = head(&parent);
        let reference = format!("refs/heads/{branch}");
        git(
            &parent,
            &["update-ref", reference.as_str(), moved_head.as_str()],
        );

        let error = plane
            .registry
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect_err("a changed unresolved branch fails closed");
        assert!(matches!(
            error,
            SubagentWorkspaceDisposalError::OwnershipMismatch { .. }
        ));
        let after = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("unresolved resource remains owned");
        assert_eq!(
            after.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        assert!(after.handoff.is_none());
        assert!(worktree.exists(), "failed proof leaves the worktree intact");
        assert!(ref_exists(&parent, &branch), "the tampered branch remains");
        assert!(!events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalStarted {
                subagent_id, ..
            } if *subagent_id == accepted.subagent_id
        )));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unresolved_resource_reproof_after_restart_disposes_idempotently() {
        let plane = plane(4);
        let (accepted, settled) = unresolved_git_child(&plane, "retry unresolved disposal").await;
        let worktree = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .physical_worktree_root
            .clone();
        let branch = settled
            .workspace
            .git_worktree()
            .expect("isolated workspace")
            .branch
            .clone();
        let (recovered, plan) = recovered_registry(&plane);
        assert_eq!(plan.settled_subagent_unresolved().len(), 1);
        let disposed = recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("exact unresolved ownership re-proof succeeds");
        assert!(matches!(
            disposed,
            SubagentWorkspaceDisposal::Disposed(snapshot)
                if snapshot.workspace_resource_state == SubagentWorkspaceResourceState::Disposed
        ));
        assert!(!worktree.exists());
        assert!(!ref_exists(&plane.dir.path().join("workspace"), &branch));

        let (after_restart, plan) = recovered_registry(&plane);
        assert_eq!(plan.settled_subagent_disposals().len(), 1);
        assert_eq!(
            plan.settled_subagent_disposals()[0].phase,
            crate::runtime::recovery::RecoveredSubagentDisposalPhase::Disposed
        );
        let repeated = after_restart
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .expect("disposed unresolved resource is idempotent");
        assert!(matches!(
            repeated,
            SubagentWorkspaceDisposal::AlreadyDisposed(snapshot)
                if snapshot.workspace_resource_state == SubagentWorkspaceResourceState::Disposed
        ));
    }

    #[tokio::test]
    async fn a_failed_child_settles_as_a_runtime_notice() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        child.complete(ChildResultStatus::Failed, None).await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Failed);
        let pending = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("one pending batch");
        assert!(matches!(
            pending.items[0].message.source,
            crate::message::types::UserSource::Runtime
        ));
    }

    #[tokio::test]
    async fn cancellation_is_canonical_over_a_late_result() {
        let plane = plane(4);
        let child = stage_stubborn(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        let cancelled = plane
            .registry
            .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
            .expect("known");
        assert_eq!(cancelled.state, SubagentState::Cancelling);
        child
            .complete(ChildResultStatus::Succeeded, Some("late"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        // The committed cancellation intent wins over the late success.
        assert_eq!(settled.state, SubagentState::Cancelled);
        let pending = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("one pending batch");
        assert!(matches!(
            pending.items[0].message.source,
            crate::message::types::UserSource::Runtime
        ));
    }

    /// The deadline has no authority during prepare or the durable ownership
    /// critical section. Once ownership commits, advancing the manual clock
    /// drives the ordinary cancellation path and the child still settles
    /// through its driver and one terminal publication.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn deadline_starts_only_after_ownership_commit_and_settles_once() {
        let plane = plane(4);
        let hook = Arc::new(CommitBoundaryHook::default());
        plane.registry.install_commit_boundary_hook(hook.clone());
        let mut child = stage_exit0(&plane);
        let registry = plane.registry.clone();
        let spec = deadline_spec("deadline after commit", 100);
        let committer = tokio::spawn(async move {
            let prepared = registry
                .prepare(&spec, &CancellationSignal::new())
                .await
                .expect("prepared");
            registry.commit(prepared, &CancellationSignal::new()).await
        });

        hook.wait_until_entered();
        // The ownership mutex is parked before the durable ownership event.
        // No deadline task exists, so elapsed manual time cannot create a
        // record or a cancellation fact here.
        plane.monotonic_clock.advance(1_000);
        assert!(events(&plane).is_empty());

        hook.release();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
            .await
            .expect("commit liveness")
            .expect("committer")
            .expect("commit succeeds");
        let accepted = match outcome {
            SubagentStartOutcome::Accepted(accepted) => accepted,
            SubagentStartOutcome::RolledBack => panic!("ownership already committed"),
        };
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("accepted snapshot")
                .state,
            SubagentState::Running
        );
        assert_eq!(
            events(&plane)
                .iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                        subagent_id, ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "the deadline begins only after one durable ownership fact"
        );

        // The deadline was sampled at manual time 1_000 during ownership.
        // Reaching 1_100 now fires it, deterministically and without a wall
        // clock wait.
        plane.monotonic_clock.advance(100);
        let cancelling = wait_for_cancelling(&plane, &accepted.subagent_id).await;
        assert_eq!(cancelling.detail, None);
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
        assert!(matches!(
            child.read_frame().await,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
            }
        ));
        // The staged test child has already exited; closing the controlled
        // protocol endpoint lets the ordinary driver prove settlement.
        drop(child.peer);

        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("the subagent execution deadline expired")
        );
        let pending = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("deadline notice");
        assert!(matches!(
            pending.items[0].message.source,
            crate::message::types::UserSource::Runtime
        ));
        let content = match &pending.items[0].message.content[0] {
            crate::message::types::UserContentBlock::Text(text) => &text.text,
            other => panic!("unexpected deadline notice content: {other:?}"),
        };
        assert!(content.contains("the subagent execution deadline expired"));
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Cancelled]
        );
    }

    /// Both registry orderings of a deadline versus a physical success are
    /// forced at the actual linearization boundary, with both contenders
    /// genuinely live:
    ///
    /// - **Deadline wins**: the child's success result is fully processed
    ///   physically, so terminal authority is a real contender parked by
    ///   the terminal-authority hook immediately before its
    ///   `... -> PublishingTerminal` commit; only then does the manual
    ///   clock fire the deadline, which commits `Running -> Cancelling`;
    ///   the released settlement observes the committed cancellation
    ///   authority and must publish one canonical `Cancelled` with the
    ///   deadline reason — never the success.
    /// - **Terminal authority wins**: the manual clock fires the deadline,
    ///   which parks by the cancellation-boundary hook immediately before
    ///   its `Running -> Cancelling` commit; while parked, the terminal
    ///   settlement acquires the registry critical section and commits one
    ///   `Succeeded`; the released deadline's cancellation call returns as
    ///   a no-op.
    ///
    /// Every interleaving is driven by hook/barrier synchronization plus
    /// the manual monotonic clock — never by wall-clock sleeps — and the
    /// published snapshot sequence is recorded exactly, proving one
    /// observable `Running -> Cancelling` transition (or none) and one
    /// final terminal publication.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn deadline_and_success_have_one_deterministic_terminal_winner() {
        // Ordering 1: the fired deadline wins the registry linearization
        // over a terminal-success contender parked immediately before its
        // terminal-authority commit.
        let deadline_wins_plane = plane(4);
        let terminal_authority = Arc::new(TerminalAuthorityHook::default());
        deadline_wins_plane
            .registry
            .install_terminal_authority_hook(terminal_authority.clone());
        let recorded = recording_observer(&deadline_wins_plane);
        let mut child = stage_exit0(&deadline_wins_plane);
        let accepted = start(
            &deadline_wins_plane,
            &deadline_spec("deadline wins the authority race", 100),
        )
        .await;
        assert_eq!(recorded.states(), vec![SubagentState::Running]);
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

        // The child's success result is consumed and every physical
        // boundary is settled; terminal authority is now a real contender
        // parked immediately before its `... -> PublishingTerminal` commit.
        // Terminal state is provably NOT committed yet.
        child
            .send_result(ChildResultStatus::Succeeded, Some("success contender"))
            .await;
        terminal_authority.wait_until_entered();
        assert_eq!(
            deadline_wins_plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("running snapshot")
                .state,
            SubagentState::Running,
            "the terminal contender is parked before its commit; Running is not yet Cancelling"
        );
        assert_eq!(recorded.states(), vec![SubagentState::Running]);

        // Fire the manual deadline while terminal authority is parked: the
        // deadline contender acquires the registry mutex first and commits
        // `Running -> Cancelling` with its typed reason.
        deadline_wins_plane.monotonic_clock.advance(100);
        let cancelling = wait_for_cancelling(&deadline_wins_plane, &accepted.subagent_id).await;
        assert_eq!(cancelling.state, SubagentState::Cancelling);
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Cancelling],
            "exactly one observable Running -> Cancelling transition"
        );

        // Release the paused terminal contender: it now observes the
        // committed cancellation authority under the same registry mutex
        // and cannot publish success.
        terminal_authority.release();
        let settled = deadline_wins_plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("deadline cancellation settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("the subagent execution deadline expired"),
            "the winning deadline reason is preserved"
        );
        assert_eq!(
            published_terminal_states(&deadline_wins_plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Cancelled],
            "exactly one final terminal result, never the racing success"
        );
        assert_eq!(
            recorded.states(),
            vec![
                SubagentState::Running,
                SubagentState::Cancelling,
                SubagentState::Cancelled
            ],
            "one cancellation transition and one final terminal state"
        );
        // No timer survives terminal authority: further elapsed time is
        // inert and produces no extra publication.
        deadline_wins_plane.monotonic_clock.advance(100_000);
        assert_eq!(
            deadline_wins_plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("cancelled snapshot")
                .state,
            SubagentState::Cancelled
        );
        assert_eq!(
            recorded.states(),
            vec![
                SubagentState::Running,
                SubagentState::Cancelling,
                SubagentState::Cancelled
            ]
        );
        // The physical child was already settled when the deadline fired,
        // so the driver wire carries no cancellation frame at all.
        drain_control_frames(&mut child).await;
        drop(child.peer);

        // Ordering 2: terminal authority wins while the fired deadline is
        // parked immediately before its cancellation commit.
        let plane = plane(4);
        let cancellation_boundary = Arc::new(CancellationBoundaryHook::default());
        plane
            .registry
            .install_cancellation_boundary_hook(cancellation_boundary.clone());
        let recorded = recording_observer(&plane);
        let mut child = stage_exit0(&plane);
        // Register the fired-deadline completion latch before the child
        // starts: the owning commit claims it when it creates the deadline
        // task (the first child of a fresh plane is ordinal 1).
        let deadline_done = plane
            .registry
            .watch_deadline_completion(&SubagentId::for_conversation(&plane.conversation_id, 1));
        let accepted = start(&plane, &deadline_spec("terminal authority wins", 100)).await;
        assert_eq!(recorded.states(), vec![SubagentState::Running]);
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

        // Fire the manual deadline: the fired deadline task is now a live
        // cancellation contender parked immediately before its
        // `Running -> Cancelling` commit. The mutation has NOT occurred.
        plane.monotonic_clock.advance(100);
        cancellation_boundary.wait_until_parked();
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("running snapshot")
                .state,
            SubagentState::Running,
            "the deadline contender is parked before its commit"
        );
        assert_eq!(recorded.states(), vec![SubagentState::Running]);

        // While the deadline contender is parked, terminal settlement
        // acquires the registry critical section and commits one
        // `PublishingTerminal`/`Succeeded`.
        child
            .send_result(ChildResultStatus::Succeeded, Some("terminal wins"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("success settled");
        assert_eq!(settled.state, SubagentState::Succeeded);
        assert_eq!(
            settled.detail, None,
            "no cancellation reason replaces the success"
        );
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Succeeded]
        );
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Succeeded],
            "terminal authority won; no Cancelling was ever observable"
        );

        // Release the parked deadline contender: its cancellation call
        // returns as a no-op over a terminal record.
        cancellation_boundary.release();
        tokio::time::timeout(std::time::Duration::from_secs(10), deadline_done)
            .await
            .expect("fired deadline liveness")
            .expect("the fired deadline's cancellation call returned");
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("succeeded snapshot")
                .state,
            SubagentState::Succeeded
        );
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Succeeded],
            "the released deadline produced no second transition"
        );
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Succeeded],
            "no extra terminal publication"
        );
        // No timer effect after terminal authority, and no driver
        // cancellation ever reached the wire.
        plane.monotonic_clock.advance(100_000);
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("succeeded snapshot")
                .state,
            SubagentState::Succeeded
        );
        drain_control_frames(&mut child).await;
        drop(child.peer);
    }

    /// Both orderings of the deadline-versus-explicit-cancellation race are
    /// forced at the actual `Running -> Cancelling` linearization boundary,
    /// with both cancellation sources genuinely live:
    ///
    /// - **Deadline wins**: explicit cancellation is spawned first and
    ///   parks by the cancellation-boundary hook immediately before the
    ///   commit; the manual clock then fires the deadline, which reaches
    ///   the same cancellation authority and is allowed to commit first;
    ///   the released explicit caller observes the committed reason and
    ///   cannot overwrite it.
    /// - **Explicit wins**: the manual clock fires the deadline first and
    ///   it parks immediately before the commit; explicit cancellation is
    ///   then allowed to commit `UserRequested`; the released deadline
    ///   cannot overwrite the reason.
    ///
    /// Each ordering proves exactly one observable `Running -> Cancelling`
    /// transition, exactly one effective `Cancel` frame on the driver
    /// wire, one winning reason preserved into the terminal detail, and
    /// one canonical terminal publication. Synchronization is hook/barrier
    /// driven with the manual monotonic clock — never wall-clock sleeps.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn deadline_and_explicit_cancellation_preserve_the_first_reason() {
        // Ordering 1: the fired deadline wins; explicit cancellation is the
        // parked loser.
        let deadline_wins_plane = plane(4);
        let cancellation_boundary = Arc::new(CancellationBoundaryHook::default());
        deadline_wins_plane
            .registry
            .install_cancellation_boundary_hook(cancellation_boundary.clone());
        let recorded = recording_observer(&deadline_wins_plane);
        let mut child = stage_exit0(&deadline_wins_plane);
        let accepted = start(
            &deadline_wins_plane,
            &deadline_spec("deadline wins the cancel race", 100),
        )
        .await;
        assert_eq!(recorded.states(), vec![SubagentState::Running]);
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

        // Explicit cancellation is a live contender first: spawned and
        // parked immediately before the `Running -> Cancelling` commit.
        let explicit_registry = deadline_wins_plane.registry.clone();
        let explicit_id = accepted.subagent_id.clone();
        let explicit_cancel = tokio::spawn(async move {
            explicit_registry.cancel(&explicit_id, CancellationReason::UserRequested)
        });
        cancellation_boundary.wait_until_parked();
        assert_eq!(
            deadline_wins_plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("running snapshot")
                .state,
            SubagentState::Running,
            "the explicit contender is parked before its commit"
        );
        assert_eq!(recorded.states(), vec![SubagentState::Running]);

        // Fire the manual deadline: it reaches the same cancellation
        // authority and is allowed to acquire/commit first.
        deadline_wins_plane.monotonic_clock.advance(100);
        let cancelling = wait_for_cancelling(&deadline_wins_plane, &accepted.subagent_id).await;
        assert_eq!(cancelling.state, SubagentState::Cancelling);
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Cancelling],
            "exactly one observable Running -> Cancelling transition"
        );
        assert!(
            matches!(
                child.read_frame().await,
                ParentFrame::Cancel {
                    reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
                }
            ),
            "exactly one effective Cancel frame, with the deadline reason"
        );

        // Release the parked explicit loser: it observes the committed
        // Cancelling and cannot replace the reason or send a second driver
        // command.
        cancellation_boundary.release();
        let losing_explicit =
            tokio::time::timeout(std::time::Duration::from_secs(10), explicit_cancel)
                .await
                .expect("explicit cancel liveness")
                .expect("explicit cancel task")
                .expect("known cancelling child");
        assert_eq!(losing_explicit.state, SubagentState::Cancelling);
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Cancelling],
            "the losing explicit caller produced no transition"
        );

        // The late child result settles one canonical Cancelled preserving
        // the winning deadline reason.
        child
            .send_result(ChildResultStatus::Succeeded, Some("late success"))
            .await;
        let settled = deadline_wins_plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("deadline cancellation settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("the subagent execution deadline expired"),
            "the deadline reason is preserved; UserRequested never replaces it"
        );
        assert_eq!(
            published_terminal_states(&deadline_wins_plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Cancelled],
            "one canonical terminal publication"
        );
        assert_eq!(
            recorded.states(),
            vec![
                SubagentState::Running,
                SubagentState::Cancelling,
                SubagentState::Cancelled
            ]
        );
        // One driver cancellation only: no second Cancel frame ever reached
        // the wire.
        drain_control_frames(&mut child).await;
        drop(child.peer);

        // Ordering 2: explicit cancellation wins; the fired deadline is the
        // parked loser.
        let plane = plane(4);
        let cancellation_boundary = Arc::new(CancellationBoundaryHook::default());
        plane
            .registry
            .install_cancellation_boundary_hook(cancellation_boundary.clone());
        let recorded = recording_observer(&plane);
        let mut child = stage_exit0(&plane);
        let deadline_done = plane
            .registry
            .watch_deadline_completion(&SubagentId::for_conversation(&plane.conversation_id, 1));
        let accepted = start(&plane, &deadline_spec("explicit wins the cancel race", 100)).await;
        assert_eq!(recorded.states(), vec![SubagentState::Running]);
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

        // Fire the manual deadline: the fired deadline task is a live
        // cancellation contender parked immediately before the commit.
        plane.monotonic_clock.advance(100);
        cancellation_boundary.wait_until_parked();
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("running snapshot")
                .state,
            SubagentState::Running,
            "the deadline contender is parked before its commit"
        );
        assert_eq!(recorded.states(), vec![SubagentState::Running]);

        // Explicit cancellation is allowed to commit first (it passes the
        // parked deadline contender).
        let cancelling = plane
            .registry
            .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
            .expect("known child");
        assert_eq!(cancelling.state, SubagentState::Cancelling);
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Cancelling],
            "exactly one observable Running -> Cancelling transition"
        );
        assert!(
            matches!(
                child.read_frame().await,
                ParentFrame::Cancel {
                    reason: Some(CancellationReason::UserRequested)
                }
            ),
            "exactly one effective Cancel frame, with the explicit reason"
        );

        // Release the parked deadline loser: it cannot overwrite the
        // committed UserRequested reason.
        cancellation_boundary.release();
        tokio::time::timeout(std::time::Duration::from_secs(10), deadline_done)
            .await
            .expect("fired deadline liveness")
            .expect("the fired deadline's cancellation call returned");
        assert_eq!(
            recorded.states(),
            vec![SubagentState::Running, SubagentState::Cancelling],
            "the released deadline produced no second transition"
        );

        // The late child result settles one canonical Cancelled preserving
        // the winning UserRequested reason.
        child
            .send_result(ChildResultStatus::Succeeded, Some("late success"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("explicit cancellation settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("requested by the user"),
            "the explicit reason is preserved; the deadline never replaces it"
        );
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Cancelled],
            "one canonical terminal publication"
        );
        assert_eq!(
            recorded.states(),
            vec![
                SubagentState::Running,
                SubagentState::Cancelling,
                SubagentState::Cancelled
            ]
        );
        // Advancing far past the deadline after the winner committed proves
        // the timer cannot fire again; one driver cancellation only.
        plane.monotonic_clock.advance(100_000);
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("cancelled snapshot")
                .state,
            SubagentState::Cancelled
        );
        drain_control_frames(&mut child).await;
        drop(child.peer);
    }

    /// The controlled child is held after `Delegate`, representing an active
    /// model/tool pipeline with no terminal result yet. Deadline expiry sends
    /// the typed cancel through the ordinary driver command channel; the
    /// driver then consumes the late result and performs physical settlement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn deadline_during_delegated_activity_uses_driver_settlement() {
        let plane = plane(4);
        let mut child = stage_exit0(&plane);
        let accepted = start(&plane, &deadline_spec("active model and tool work", 100)).await;
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
        plane.monotonic_clock.advance(100);
        let cancelling = wait_for_cancelling(&plane, &accepted.subagent_id).await;
        assert_eq!(cancelling.state, SubagentState::Cancelling);
        let cancel = child.read_frame().await;
        assert!(matches!(
            cancel,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
            }
        ));
        child
            .send_result(ChildResultStatus::Succeeded, Some("late active result"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("driver physically settled the child");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("the subagent execution deadline expired")
        );
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Cancelled]
        );
        drop(child.peer);
    }

    /// Terminal candidate creation and deadline invalidation are committed
    /// under the registry mutex. The durable terminal event is therefore the
    /// final lifecycle event, and advancing the clock after settlement cannot
    /// resurrect the deadline task.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn deadline_terminal_publication_is_final_and_timer_is_cleaned_up() {
        let plane = plane(4);
        let mut child = stage_exit0(&plane);
        let accepted = start(&plane, &deadline_spec("terminal ordering", 100)).await;
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
        plane.monotonic_clock.advance(100);
        wait_for_cancelling(&plane, &accepted.subagent_id).await;
        assert!(matches!(
            child.read_frame().await,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
            }
        ));
        child
            .send_result(ChildResultStatus::Succeeded, Some("late terminal result"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Cancelled);

        let journal = events(&plane);
        let terminal_positions = journal
            .iter()
            .enumerate()
            .filter_map(|(position, event)| {
                matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                        subagent_id, ..
                    } if *subagent_id == accepted.subagent_id
                )
                .then_some(position)
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal_positions, vec![journal.len() - 1]);
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Cancelled]
        );
        let event_count = journal.len();
        plane
            .monotonic_clock
            .advance(super::super::catalog::MAX_SUBAGENT_EXECUTION_DEADLINE_MS);
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("terminal snapshot")
                .state,
            SubagentState::Cancelled
        );
        assert_eq!(events(&plane).len(), event_count);
        drop(child.peer);
    }

    /// `prepare` owns a copy of the resolved deadline. Mutating the caller's
    /// source specification after preparation cannot change the admitted
    /// launch's firing point.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn prepared_deadline_remains_frozen_after_source_spec_changes() {
        let plane = plane(4);
        let mut child = stage_exit0(&plane);
        let mut source_spec = deadline_spec("frozen deadline", 100);
        let prepared = plane
            .registry
            .prepare(&source_spec, &CancellationSignal::new())
            .await
            .expect("prepared");
        assert_eq!(
            prepared
                .execution_deadline
                .expect("prepared deadline")
                .as_millis(),
            100
        );
        source_spec.resolved.execution_deadline = Some(
            SubagentExecutionDeadline::from_millis(1_000).expect("valid replacement deadline"),
        );
        let accepted = match plane
            .registry
            .commit(prepared, &CancellationSignal::new())
            .await
            .expect("commit")
        {
            SubagentStartOutcome::Accepted(accepted) => accepted,
            SubagentStartOutcome::RolledBack => panic!("commit unexpectedly rolled back"),
        };
        assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
        plane.monotonic_clock.advance(100);
        wait_for_cancelling(&plane, &accepted.subagent_id).await;
        assert!(matches!(
            child.read_frame().await,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
            }
        ));
        child
            .send_result(ChildResultStatus::Succeeded, Some("late frozen result"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("the subagent execution deadline expired")
        );
        drop(child.peer);
    }

    #[tokio::test]
    async fn a_child_lost_to_driver_escalation_after_cancel_settles_cancelled() {
        let plane = plane(4);
        // The child never answers the Cancel frame; the driver escalates
        // (Cancel -> SIGTERM -> SIGKILL) and reaps it.
        let _child = stage_stubborn(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        let _ = plane
            .registry
            .cancel(&accepted.subagent_id, CancellationReason::UserRequested);
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert_eq!(
            settled.detail.as_deref(),
            Some("requested by the user"),
            "driver escalation cannot erase the committed cancellation cause"
        );
        assert_eq!(
            events(&plane)
                .into_iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                            subagent_id,
                            state: SubagentTerminalState::Cancelled,
                            ..
                        } if *subagent_id == accepted.subagent_id
                    )
                })
                .count(),
            1,
            "escalated cancellation has one terminal publication"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_commit_losing_the_cancellation_race_rolls_back_completely() {
        let plane = plane(4);
        let hook = Arc::new(CommitBoundaryHook::default());
        plane.registry.install_commit_boundary_hook(hook.clone());
        let child = stage_stubborn(&plane);
        let pid = child.pid;
        let registry = plane.registry.clone();
        let spec = start_spec("inspect");
        let attempt_cancellation = CancellationSignal::new();
        let committer = {
            let attempt_cancellation = attempt_cancellation.clone();
            tokio::spawn(async move {
                let prepared = registry
                    .prepare(&spec, &CancellationSignal::new())
                    .await
                    .expect("prepared");
                registry.commit(prepared, &attempt_cancellation).await
            })
        };
        hook.wait_until_entered();
        attempt_cancellation.cancel();
        hook.release();
        let outcome = committer.await.expect("committer");
        assert!(matches!(outcome, Ok(SubagentStartOutcome::RolledBack)));
        // No record, no durable trace, no ordinal consumption that recovery
        // would fold.
        assert!(plane.registry.all_snapshots().is_empty());
        assert!(events(&plane).is_empty());
        #[cfg(unix)]
        assert!(matches!(
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits")),
                None
            ),
            Err(nix::errno::Errno::ESRCH)
        ));
    }

    /// The ownership/control handoff race is deterministic: durable
    /// ownership and the Running projection are visible while the driver
    /// command handle is deliberately not installed, cancellation commits in
    /// that exact window, and the resumed handoff forwards the sticky cancel
    /// before Delegate. The real child is then killed and reaped, and its
    /// late/no result cannot overtake the canonical cancellation.
    #[allow(clippy::too_many_lines)]
    async fn control_handoff_cancellation_is_lossless(runtime_drain: bool) {
        let plane = plane(4);
        let hook = Arc::new(ControlHandoffHook::default());
        plane.registry.install_control_handoff_hook(hook.clone());
        let mut child = stage_stubborn(&plane);
        let pid = child.pid;
        let registry = plane.registry.clone();
        let spec = start_spec("inspect");
        let committer = tokio::spawn(async move {
            let prepared = registry
                .prepare(&spec, &CancellationSignal::new())
                .await
                .expect("prepared");
            registry.commit(prepared, &CancellationSignal::new()).await
        });

        hook.wait_until_entered();
        let subagent_id = SubagentId::for_conversation(&plane.conversation_id, 1);
        assert!(events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: ownership_subagent_id,
                ..
            } if *ownership_subagent_id == subagent_id
        )));

        let cancelling = if runtime_drain {
            plane
                .registry
                .cancel_all(CancellationReason::RuntimeShutdown);
            plane
                .registry
                .snapshot(&subagent_id)
                .expect("cancelled record")
        } else {
            let registry = plane.registry.clone();
            tokio::spawn(async move {
                registry
                    .cancel(&subagent_id, CancellationReason::UserRequested)
                    .expect("cancelled record")
            })
            .await
            .expect("cancel task")
        };
        assert_eq!(cancelling.state, SubagentState::Cancelling);
        let expected_reason = if runtime_drain {
            CancellationReason::RuntimeShutdown
        } else {
            CancellationReason::UserRequested
        };

        hook.release();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
            .await
            .expect("commit liveness")
            .expect("committer")
            .expect("commit succeeds after cancellation intent");
        let accepted = match outcome {
            SubagentStartOutcome::Accepted(accepted) => accepted,
            SubagentStartOutcome::RolledBack => panic!("ownership already committed"),
        };
        let ownership_timestamp = plane
            .store
            .read_events(None, 64)
            .expect("events")
            .events
            .iter()
            .find_map(|envelope| {
                matches!(
                    &envelope.event,
                    crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                        subagent_id,
                        ..
                    } if *subagent_id == accepted.subagent_id
                )
                .then_some(envelope.timestamp)
            })
            .expect("ownership timestamp");
        assert_eq!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .expect("accepted snapshot")
                .started_at,
            ownership_timestamp,
            "one ownership commit timestamp feeds event and projection"
        );

        let first = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::ipc::read_parent_frame(&mut child.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame")
        .expect("cancel frame");
        assert!(matches!(
            first,
            ParentFrame::Cancel {
                reason: Some(reason)
            } if reason == expected_reason
        ));
        // No Delegate was sent after cancellation won this handoff frontier:
        // the driver's cancelled-before-start branch never writes it. Drain
        // every remaining control frame (the escalation then EOF after
        // reap) and prove the wire carries nothing else.
        loop {
            let frame = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                super::super::ipc::read_parent_frame(&mut child.peer),
            )
            .await
            .expect("driver control liveness")
            .expect("driver frame");
            match frame {
                Some(ParentFrame::Cancel {
                    reason: Some(reason),
                }) if reason == expected_reason => {}
                Some(ParentFrame::Cancel { reason: None }) => {
                    panic!("a committed cancellation must carry its registry reason")
                }
                Some(ParentFrame::Cancel {
                    reason: Some(reason),
                }) => {
                    panic!("unexpected cancellation reason on the wire: {reason:?}")
                }
                Some(ParentFrame::Delegate(_)) => {
                    panic!("cancellation won the frontier; Delegate must never be sent")
                }
                Some(ParentFrame::Hello(_)) => {
                    panic!("unexpected Hello after Ready")
                }
                Some(ParentFrame::AnchorAccepted(_) | ParentFrame::AnchorRefused(_)) => {
                    panic!("this child offered no nested process unit anchor")
                }
                Some(ParentFrame::InteractionRespond { .. }) => {
                    panic!("the driver test did not offer an interaction response")
                }
                Some(ParentFrame::InteractionProviderAvailable { .. }) => {
                    panic!("the driver test did not offer a provider update")
                }
                Some(ParentFrame::InteractionPublicationAdmissionResult(_)) => {
                    panic!("the driver test did not offer an admission result")
                }
                None => break,
            }
        }
        drop(child.peer);

        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        assert!(!settled.publication_abandoned);
        #[cfg(unix)]
        assert!(
            matches!(
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits")),
                    None
                ),
                Err(nix::errno::Errno::ESRCH)
            ),
            "the direct child was reaped"
        );
        let terminal_states = events(&plane)
            .into_iter()
            .filter_map(|event| match event {
                crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                    subagent_id,
                    state,
                    ..
                } if subagent_id == accepted.subagent_id => Some(state),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminal_states,
            vec![SubagentTerminalState::Cancelled],
            "one canonical cancellation, never a success overtaking it"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_between_ownership_and_control_publication_is_lossless() {
        control_handoff_cancellation_is_lossless(false).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_drain_between_ownership_and_control_publication_is_lossless() {
        control_handoff_cancellation_is_lossless(true).await;
    }

    /// The remaining start-gate edge (Blocker A) is serialized, not
    /// best-effort: while the commit holds the registry mutex at exactly
    /// "command handle installed, gate not yet released", a concurrent
    /// `cancel` provably blocks. Releasing the gate first defines an
    /// already-started child: the driver sends `Delegate` first and the
    /// later cancellation arrives as in-flight cancellation (`Cancel` frame
    /// after `Delegate`), settling one canonical cancelled terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn gate_release_wins_the_start_cancel_arbitration_and_cancel_becomes_in_flight() {
        let plane = plane(4);
        let hook = Arc::new(GateReleaseHook::default());
        plane.registry.install_gate_release_hook(hook.clone());
        let mut child = stage_stubborn(&plane);
        let registry = plane.registry.clone();
        let spec = start_spec("inspect");
        let committer = tokio::spawn(async move {
            let prepared = registry
                .prepare(&spec, &CancellationSignal::new())
                .await
                .expect("prepared");
            registry.commit(prepared, &CancellationSignal::new()).await
        });

        hook.wait_until_entered();
        let subagent_id = SubagentId::for_conversation(&plane.conversation_id, 1);
        assert!(events(&plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: ownership_subagent_id,
                ..
            } if *ownership_subagent_id == subagent_id
        )));

        // A concurrent cancellation is invoked while the commit provably
        // holds the registry mutex at the install-but-not-released edge; it
        // cannot complete until the gate-release section returns.
        let (cancel_started_tx, cancel_started_rx) = std::sync::mpsc::channel();
        let (cancel_done_tx, cancel_done_rx) = std::sync::mpsc::channel();
        let cancel_registry = plane.registry.clone();
        let cancel_id = subagent_id.clone();
        let canceller = std::thread::spawn(move || {
            cancel_started_tx.send(()).expect("cancel-started channel");
            let snapshot = cancel_registry
                .cancel(&cancel_id, CancellationReason::UserRequested)
                .expect("known record");
            cancel_done_tx.send(()).expect("cancel-done channel");
            snapshot
        });
        cancel_started_rx
            .recv()
            .expect("cancel is invoked while the gate release is parked");
        assert!(
            cancel_done_rx.try_recv().is_err(),
            "cancel is provably blocked on the registry mutex held by the parked gate release"
        );

        hook.release();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
            .await
            .expect("commit liveness")
            .expect("committer")
            .expect("commit succeeds");
        let accepted = match outcome {
            SubagentStartOutcome::Accepted(accepted) => accepted,
            SubagentStartOutcome::RolledBack => panic!("ownership already committed"),
        };
        let cancelling = canceller.join().expect("canceller joins");
        assert_eq!(cancelling.state, SubagentState::Cancelling);

        // The gate release won: Delegate is the first parent->child frame,
        // and the committed cancellation arrives after it as in-flight
        // cancellation.
        let first = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::ipc::read_parent_frame(&mut child.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame")
        .expect("delegate frame");
        assert!(
            matches!(first, ParentFrame::Delegate(_)),
            "gate release first means Delegate first"
        );
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::ipc::read_parent_frame(&mut child.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame")
        .expect("cancel frame");
        assert!(matches!(
            second,
            ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested)
            }
        ));

        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Cancelled);
        let terminal_states = events(&plane)
            .into_iter()
            .filter_map(|event| match event {
                crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                    subagent_id,
                    state,
                    ..
                } if subagent_id == accepted.subagent_id => Some(state),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminal_states,
            vec![SubagentTerminalState::Cancelled],
            "one canonical cancellation"
        );
    }

    #[tokio::test]
    async fn the_capacity_bound_is_enforced_at_commit() {
        let plane = plane(1);
        let _first_child = stage_stubborn(&plane);
        let first = start(&plane, &start_spec("first")).await;
        assert_eq!(
            plane
                .registry
                .snapshot(&first.subagent_id)
                .expect("snapshot")
                .state,
            SubagentState::Running
        );
        // prepare stages privately even at capacity; the commit is the
        // linearization point that refuses.
        let _second_child = stage_exit0(&plane);
        let prepared = plane
            .registry
            .prepare(&start_spec("second"), &CancellationSignal::new())
            .await
            .expect("prepared");
        let error = plane
            .registry
            .commit(prepared, &CancellationSignal::new())
            .await
            .expect_err("capacity");
        assert!(matches!(
            error,
            SubagentStartError::CapacityExceeded { max: 1 }
        ));
        // Settle the committed first child (escalate and reap) so the
        // fixture leaks no process.
        let _ = plane
            .registry
            .cancel(&first.subagent_id, CancellationReason::UserRequested);
        plane
            .registry
            .wait_until_settled(&first.subagent_id)
            .await
            .expect("settled");
    }

    #[tokio::test]
    async fn prepare_rejects_an_invalid_task_before_any_spawn() {
        let plane = plane(4);
        let error = plane
            .registry
            .prepare(&start_spec(""), &CancellationSignal::new())
            .await
            .expect_err("empty task");
        assert!(matches!(error, SubagentStartError::InvalidTask { .. }));
        let oversized = "x".repeat(MAX_TASK_BYTES + 1);
        let error = plane
            .registry
            .prepare(&start_spec(&oversized), &CancellationSignal::new())
            .await
            .expect_err("oversized task");
        assert!(matches!(error, SubagentStartError::InvalidTask { .. }));
        let mut oversized_context = start_spec("inspect");
        oversized_context.context = Some("x".repeat(MAX_CONTEXT_PACKAGE_BYTES + 1));
        let error = plane
            .registry
            .prepare(&oversized_context, &CancellationSignal::new())
            .await
            .expect_err("oversized context");
        assert!(matches!(error, SubagentStartError::ContextOversized { .. }));
        assert!(plane.registry.all_snapshots().is_empty());
    }

    #[tokio::test]
    async fn a_terminal_publication_failure_is_retried_then_abandoned() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        // The initial publication plus both bounded retries fail.
        plane.store.arm_fail_accept_times(3);
        child
            .complete(ChildResultStatus::Succeeded, Some("the answer"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("abandoned resolves the wait");
        assert!(settled.publication_abandoned);
        assert_eq!(settled.state, SubagentState::PublishingTerminal);
        // Nothing reached the durable authority.
        assert!(
            plane
                .store
                .select_pending_batch()
                .expect("pending")
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn physical_settlement_failure_keeps_terminal_retry_classification_stable() {
        let plane = plane(4);
        make_clean_git_workspace(&plane);
        plane
            .workspace_settlement_hook
            .fail_next("injected terminal workspace settlement failure");
        let child = stage_stubborn(&plane);
        let mut spec = start_spec("inspect");
        spec.resolved.workspace_policy =
            crate::runtime::subagent::SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            };
        let accepted = start(&plane, &spec).await;
        plane.store.arm_fail_accept_times(3);
        child
            .complete(
                ChildResultStatus::Succeeded,
                Some("success must stay private"),
            )
            .await;
        plane.workspace_settlement_hook.wait_until_entered().await;
        plane.workspace_settlement_hook.release().await;

        let abandoned = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("publication abandoned");
        assert_eq!(abandoned.state, SubagentState::PublishingTerminal);
        assert!(abandoned.publication_abandoned);
        let diagnostic = abandoned.detail.clone().expect("stable diagnostic");
        assert!(diagnostic.contains("physical settlement was not proven"));
        assert!(!diagnostic.contains("success must stay private"));

        assert!(
            plane
                .registry
                .retry_terminal_publication(&accepted.subagent_id)
                .expect("retry")
        );
        let settled = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("settled snapshot");
        assert_eq!(settled.state, SubagentState::Failed);
        assert!(!settled.publication_abandoned);
        assert_eq!(settled.detail.as_deref(), Some(diagnostic.as_str()));
        assert_eq!(
            events(&plane)
                .into_iter()
                .filter(|event| matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                        subagent_id,
                        state: SubagentTerminalState::Failed,
                        ..
                    } if *subagent_id == accepted.subagent_id
                ))
                .count(),
            1,
            "retry publishes the frozen failed classification exactly once"
        );
    }

    /// A physically settled child still owns its capacity while its frozen
    /// terminal candidate is unresolved. Once the identical publication is
    /// durably accepted, the slot is released and the next commit may win.
    #[tokio::test]
    async fn publishing_terminal_retains_capacity_until_durable_settlement() {
        let plane = plane(1);
        let child = stage_exit0(&plane);
        let first = start(&plane, &start_spec("first")).await;
        plane.store.arm_fail_accept_times(3);
        child
            .complete(ChildResultStatus::Succeeded, Some("first answer"))
            .await;
        let unresolved = plane
            .registry
            .wait_until_settled(&first.subagent_id)
            .await
            .expect("abandoned publication is observable");
        assert_eq!(unresolved.state, SubagentState::PublishingTerminal);
        assert!(unresolved.publication_abandoned);

        let _second_child = stage_exit0(&plane);
        let second_prepared = plane
            .registry
            .prepare(&start_spec("second"), &CancellationSignal::new())
            .await
            .expect("private preparation is allowed");
        let error = plane
            .registry
            .commit(second_prepared, &CancellationSignal::new())
            .await
            .expect_err("unresolved terminal settlement retains capacity");
        assert!(matches!(
            error,
            SubagentStartError::CapacityExceeded { max: 1 }
        ));

        assert!(
            plane
                .registry
                .retry_terminal_publication(&first.subagent_id)
                .unwrap()
        );
        let settled = plane
            .registry
            .snapshot(&first.subagent_id)
            .expect("first snapshot");
        assert_eq!(settled.state, SubagentState::Succeeded);
        assert!(settled.settled);
    }

    #[tokio::test]
    async fn an_ambiguous_publication_commit_resolves_exactly_once() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("the answer"))
            .await;
        plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        // A retry of the same correlated publication is an idempotent
        // no-op, never a second message. Rebuild the byte-identical draft:
        // the frozen candidate timestamp is the committed one.
        let first = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("batch");
        assert_eq!(first.items.len(), 1);
        let committed_at = first.items[0]
            .message
            .timestamp
            .expect("terminal notifications carry a timestamp");
        let (draft, event) = super::super::terminal_publication(
            &plane.conversation_id,
            &accepted.subagent_id,
            &accepted.child_agent_id,
            SubagentTerminalState::Succeeded,
            vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "the answer".to_owned(),
                },
            )],
            &crate::events::types::SubagentWorkspaceTerminalResource::None,
            committed_at,
        );
        plane
            .store
            .accept_inbound_with_event(draft, event)
            .expect("idempotent retry");
        let second = plane
            .store
            .select_pending_batch()
            .expect("pending")
            .expect("batch");
        assert_eq!(second.items.len(), 1, "exactly once");
    }

    #[tokio::test]
    async fn the_ordinal_sequence_reseeds_above_the_durable_watermark() {
        let plane = plane(4);
        plane.registry.restore_sequence_watermark(7);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        assert_eq!(
            accepted.subagent_id.as_str(),
            "conv-test-subagent-8",
            "the next ordinal never reissues a durable identity"
        );
        child
            .complete(ChildResultStatus::Succeeded, Some("done"))
            .await;
        plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
    }

    /// A parent generation can die after a child opened its durable store but
    /// before the parent's ownership event committed. The next generation
    /// must preserve that store as an orphaned history boundary and advance
    /// to a fresh child identity instead of reusing it.
    #[tokio::test]
    async fn prepare_skips_an_unpublished_durable_child_identity() {
        let plane = plane(4);
        let stale_id = SubagentId::new("conv-test-subagent-1");
        let stale_store = crate::runtime::subagent::child_conversation_store_path(
            &plane.runtime_root,
            &ConversationId::new(stale_id.as_str()),
        );
        std::fs::create_dir_all(
            stale_store
                .parent()
                .expect("the stale semantic child directory"),
        )
        .expect("stale child directory");
        std::fs::write(&stale_store, b"orphaned durable state").expect("stale durable store");

        let error = plane
            .registry
            .prepare(
                &start_spec("skip stale identity"),
                &CancellationSignal::new(),
            )
            .await
            .expect_err("the next identity should reach the intentionally missing child binary");
        assert!(matches!(error, SubagentStartError::Spawn { .. }));

        let _child = stage_exit0(&plane);
        let prepared = plane
            .registry
            .prepare(&start_spec("fresh identity"), &CancellationSignal::new())
            .await
            .expect("the following identity is fresh");
        assert_eq!(
            prepared.child_conversation_id.as_str(),
            "conv-test-subagent-3"
        );
        prepared.staged.rollback().await.expect("rollback");
        assert_eq!(
            std::fs::read(&stale_store).expect("the orphaned store remains authoritative"),
            b"orphaned durable state"
        );
    }

    #[tokio::test]
    async fn cancel_of_an_unknown_or_terminal_subagent_is_a_noop() {
        let plane = plane(4);
        let unknown = SubagentId::new("conv-test-subagent-99");
        assert!(
            plane
                .registry
                .cancel(&unknown, CancellationReason::UserRequested)
                .is_none()
        );
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("done"))
            .await;
        plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        let after = plane
            .registry
            .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
            .expect("known");
        assert_eq!(after.state, SubagentState::Succeeded);
    }

    /// Issue #178: a live activity update lands in the registry read model
    /// and rides the snapshot, while the lifecycle — the only authority —
    /// is untouched. Stale or reordered revisions are dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_activity_updates_the_snapshot_without_touching_lifecycle() {
        let plane = plane(4);
        let child = stage_stubborn(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;

        let observation = SubagentObservation {
            revision: 1,
            activity: super::super::activity::SubagentActivity::Model {
                request_id: crate::runtime::identity::RequestId::new("req-1"),
                retry: 0,
            },
            counters: super::super::activity::SubagentActivityCounters {
                model_requests: 1,
                ..super::super::activity::SubagentActivityCounters::default()
            },
            ..SubagentObservation::default()
        };
        plane
            .registry
            .apply_activity(&accepted.subagent_id, observation.clone());

        let snapshot = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("snapshot");
        assert_eq!(snapshot.state, SubagentState::Running);
        assert_eq!(snapshot.observation, observation);

        // Stale and reordered revisions are dropped.
        plane
            .registry
            .apply_activity(&accepted.subagent_id, observation.clone());
        plane
            .registry
            .apply_activity(&accepted.subagent_id, SubagentObservation::default());
        let snapshot = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("snapshot");
        assert_eq!(snapshot.observation, observation);

        // Unknown identities are a no-op.
        plane.registry.apply_activity(
            &SubagentId::new("conv-test-subagent-99"),
            observation.clone(),
        );

        let _ = plane
            .registry
            .cancel(&accepted.subagent_id, CancellationReason::UserRequested);
        plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        drop(child);
    }

    /// Issue #178: the redacted execution profile flows from the resolved
    /// specification into the snapshot at commit; settlement resets the
    /// activity to neutral with a bumped revision while keeping the
    /// counters, and post-terminal activity is dropped.
    #[tokio::test]
    async fn settlement_resets_activity_to_neutral_and_drops_late_updates() {
        let plane = plane(4);
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("inspect")).await;

        // The profile was derived from the frozen model authority at start.
        let running = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running snapshot");
        let profile = running.profile.clone().expect("profile at commit");
        assert_eq!(profile.model, "local/model");
        assert!(!profile.reasoning_enabled);
        assert_eq!(running.observation, SubagentObservation::default());

        let observation = SubagentObservation {
            revision: 3,
            activity: super::super::activity::SubagentActivity::Tool {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
                progress: None,
            },
            counters: super::super::activity::SubagentActivityCounters {
                tool_executions: 2,
                ..super::super::activity::SubagentActivityCounters::default()
            },
            ..SubagentObservation::default()
        };
        plane
            .registry
            .apply_activity(&accepted.subagent_id, observation);

        child
            .complete(ChildResultStatus::Succeeded, Some("the answer"))
            .await;
        let settled = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(settled.state, SubagentState::Succeeded);
        assert_eq!(
            settled.observation.activity,
            super::super::activity::SubagentActivity::AwaitingActivity,
            "the terminal snapshot is activity-neutral"
        );
        assert_eq!(
            settled.observation.revision, 4,
            "the settlement reset bumped the revision"
        );
        assert_eq!(settled.observation.counters.tool_executions, 2);
        assert_eq!(settled.profile, running.profile);

        // Post-terminal activity can never resurrect live-ness.
        let late = SubagentObservation {
            revision: 99,
            ..SubagentObservation::default()
        };
        plane.registry.apply_activity(&accepted.subagent_id, late);
        let after = plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("snapshot");
        assert_eq!(after.observation.revision, 4);
        assert_eq!(
            after.observation.activity,
            super::super::activity::SubagentActivity::AwaitingActivity
        );
    }

    // --- The subagent domain's own bounded discovery read model (#180) ---
    //
    // These prove the read model directly against the registry's own record
    // vector, without a live child: the properties under test are ordering,
    // filtering, counting, and bounding, none of which involve a process.
    // The listing type is the subagent domain's own — nothing here names
    // the model-facing `execution` control plane.

    /// Seeds one synthetic record in allocation order, so listing order can
    /// be asserted against a known allocation sequence.
    fn seed_record(registry: &SubagentRegistry, id: &str, lifecycle: SubagentLifecycle) {
        let subagent_id = SubagentId::new(id);
        let mut state = registry.state.lock().expect("registry state");
        let index = state.records.len();
        state.records.push(SubagentRecord {
            subagent_id: subagent_id.clone(),
            child_agent_id: AgentId::new(format!("agent-{id}")),
            child_conversation_id: ConversationId::new(format!("conv-{id}")),
            tool_call_id: ToolCallId::new(format!("call-{id}")),
            agent: SubagentName::parse("reviewer").expect("agent name"),
            definition_digest: serde_json::from_value(serde_json::Value::String(format!(
                "sha256:{}",
                "0".repeat(64)
            )))
            .expect("digest"),
            terminal: SubagentTerminalMode::Normal,
            workspace: WorkspaceSnapshot::shared(std::path::PathBuf::from("/workspace")),
            handoff: None,
            workspace_resource_state: SubagentWorkspaceResourceState::None,
            workspace_disposal: None,
            workspace_unresolved: None,
            lifecycle,
            cancel_reason: None,
            deadline_task: None,
            control: None,
            detail: None,
            observation: SubagentObservation::default(),
            profile: None,
            terminal_workflow_value: None,
            pending_terminal: None,
            publication_abandoned: false,
            notification: NotificationState::None,
            started_at: Utc::now(),
        });
        state.index.insert(subagent_id, index);
    }

    fn listed_ids(listing: &SubagentListing) -> Vec<String> {
        listing
            .snapshots
            .iter()
            .map(|snapshot| snapshot.subagent_id.to_string())
            .collect()
    }

    /// The registry's authoritative order is its own allocation order,
    /// reversed: the most recently started child first.
    #[test]
    fn subagent_listing_is_newest_first_in_the_registrys_allocation_order() {
        let plane = plane(8);
        for id in ["s1", "s2", "s3"] {
            seed_record(&plane.registry, id, SubagentLifecycle::Running);
        }

        let listing = plane.registry.listing(false, 16);

        assert_eq!(listed_ids(&listing), vec!["s3", "s2", "s1"]);
        assert_eq!(listing.matched, 3);
    }

    /// `active_only` is the domain's own lifecycle classification, under
    /// which `PublishingTerminal` is still active.
    #[test]
    fn subagent_listing_active_only_uses_the_domains_own_classification() {
        let plane = plane(8);
        seed_record(&plane.registry, "s1", SubagentLifecycle::Running);
        seed_record(&plane.registry, "s2", SubagentLifecycle::Succeeded);
        seed_record(&plane.registry, "s3", SubagentLifecycle::PublishingTerminal);
        seed_record(&plane.registry, "s4", SubagentLifecycle::Cancelled);

        assert_eq!(
            listed_ids(&plane.registry.listing(true, 16)),
            vec!["s3", "s1"]
        );
        assert_eq!(
            listed_ids(&plane.registry.listing(false, 16)),
            vec!["s4", "s3", "s2", "s1"]
        );
    }

    /// `matched` counts every matching record before the bound, and the
    /// materialized prefix stays finite — so a caller can report truncation
    /// without the registry ever building an unbounded response.
    #[test]
    fn subagent_listing_counts_matches_before_its_materialization_bound() {
        let plane = plane(8);
        for ordinal in 1..=9 {
            let lifecycle = if ordinal % 3 == 0 {
                SubagentLifecycle::Succeeded
            } else {
                SubagentLifecycle::Running
            };
            seed_record(&plane.registry, &format!("s{ordinal}"), lifecycle);
        }

        let all = plane.registry.listing(false, 2);
        assert_eq!(all.matched, 9);
        assert_eq!(listed_ids(&all), vec!["s9", "s8"]);

        let active = plane.registry.listing(true, 3);
        assert_eq!(active.matched, 6);
        assert_eq!(listed_ids(&active), vec!["s8", "s7", "s5"]);

        // A zero bound still reports the whole matching population.
        let none = plane.registry.listing(false, 0);
        assert!(none.snapshots.is_empty());
        assert_eq!(none.matched, 9);
    }

    /// Listing is a pure read: it changes no lifecycle and no observation.
    #[test]
    fn subagent_listing_mutates_neither_lifecycle_nor_observation() {
        let plane = plane(8);
        seed_record(&plane.registry, "s1", SubagentLifecycle::Running);
        let observation = SubagentObservation {
            revision: 7,
            ..SubagentObservation::default()
        };
        plane
            .registry
            .apply_activity(&SubagentId::new("s1"), observation);
        let before = plane.registry.all_snapshots();

        for _ in 0..3 {
            let _ = plane.registry.listing(false, 16);
            let _ = plane.registry.listing(true, 1);
        }

        let after = plane.registry.all_snapshots();
        assert_eq!(before, after);
        assert_eq!(after[0].state, SubagentState::Running);
        assert_eq!(after[0].observation.revision, 7);
    }

    use crate::context::SessionContextPolicy;
}
