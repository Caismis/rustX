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

use crate::runtime::RuntimeClock;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::types::{CancellationReason, DurabilityGate};

use super::catalog::{SubagentDefinitionDigest, SubagentName};
use super::ipc::DelegationFrame;
use super::process::{PhysicalOutcome, PhysicalSettlement, StagedChild, SubagentSpawnPlan};
use super::resolver::ResolvedSubagentSpec;
use super::{
    MAX_CONTEXT_PACKAGE_BYTES, MAX_RESULT_CONTENT_BYTES, MAX_TASK_BYTES, SubagentTerminalState,
    bound_utf8, ownership_event, terminal_publication,
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
    Accepted { started_at: DateTime<Utc> },
    RolledBack,
    Failed(SubagentStartError),
}

/// The canonicalized terminal outcome awaiting publication.
#[derive(Debug, Clone)]
struct TerminalCandidate {
    state: TerminalState,
    /// The bounded result content (succeeded only).
    content: Option<String>,
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
    lifecycle: SubagentLifecycle,
    cancel_reason: Option<CancellationReason>,
    /// The narrow cancellation handle into the driver task — never an OS
    /// process handle.
    control: Option<tokio::sync::mpsc::Sender<super::process::DriverCommand>>,
    /// The bounded terminal result content or diagnostic.
    detail: Option<String>,
    pending_terminal: Option<TerminalCandidate>,
    publication_abandoned: bool,
    notification: NotificationState,
    started_at: DateTime<Utc>,
}

impl SubagentRecord {
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
            state,
            detail: self.detail.clone(),
            publication_abandoned: self.publication_abandoned,
            settled: self.lifecycle.is_terminal() && !self.publication_abandoned,
            started_at: self.started_at,
        }
    }
}

struct RegistryState {
    next_ordinal: u64,
    records: Vec<SubagentRecord>,
    index: HashMap<SubagentId, usize>,
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

/// A consistency snapshot of one subagent child.
///
/// Read-model materialization only: every field is derived from the
/// registry's state machine, never an authority of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The lifecycle state.
    pub state: SubagentState,
    /// The bounded result content (succeeded) or failure/cancellation
    /// detail, once known.
    pub detail: Option<String>,
    /// Whether a terminal publication could not reach the durable
    /// authority and was abandoned.
    pub publication_abandoned: bool,
    /// Whether the child reached a settled state (terminal, publication
    /// not abandoned).
    pub settled: bool,
    /// When the ownership committed.
    pub started_at: DateTime<Utc>,
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
    /// The delegated task.
    pub task: String,
    /// The explicit bounded context package.
    pub context: Option<String>,
    /// The delegating tool call.
    pub tool_call_id: ToolCallId,
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
    task: String,
    context: Option<String>,
    staged: StagedChild,
}

/// The outcome of a successful ownership commit.
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
    /// The tool result the delegating call receives.
    pub result: serde_json::Value,
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
pub trait SubagentObserver: Send + Sync {
    /// Called under the registry lock with each new consistency snapshot;
    /// the implementation must be cheap and nonblocking.
    fn on_snapshot(&self, snapshot: &SubagentSnapshot);
}

/// The durability-failure reporting seam of the subagent plane.
pub trait SubagentDurabilityFailureSink: Send + Sync {
    /// A terminal publication could not reach the durable authority.
    fn terminal_publication_failed(&self, subagent_id: &SubagentId, diagnostic: &str);
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
    /// The process spawn plan.
    pub spawn: SubagentSpawnPlan,
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
                records: Vec::new(),
                index: HashMap::new(),
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
                staged_overrides: std::collections::VecDeque::new(),
            })),
            state_version: tokio::sync::watch::Sender::new(0),
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
        state.observer = Some(observer);
        snapshots
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
        let ordinal = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let ordinal = state.next_ordinal;
            state.next_ordinal += 1;
            ordinal
        };
        let subagent_id = SubagentId::for_conversation(&self.config.conversation_id, ordinal);
        let child_conversation_id = ConversationId::new(subagent_id.as_str());
        let child_agent_id = AgentId::new(format!("agent-{subagent_id}"));
        let staged = {
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
                        task: spec.task.clone(),
                        context: spec.context.clone(),
                        staged,
                    });
                }
            }
            // Semantic identity may be reused after a pre-commit crash, so
            // reserve its mutable physical namespace independently of the
            // durable ordinal before launching the child.
            let runtime_root = self
                .config
                .spawn
                .allocate_child_runtime_root(&subagent_id)
                .map_err(|error| SubagentStartError::Spawn {
                    detail: error.to_string(),
                })?;
            let child_spec = self.config.spawn.child_spec(
                &subagent_id,
                &child_conversation_id,
                &child_agent_id,
                &self.config.agent_id,
                &spec.resolved,
                &runtime_root,
            );
            super::process::spawn_staged(
                &self.config.spawn,
                &child_spec,
                runtime_root,
                preparation_cancellation,
            )
            .await
            .map_err(|error| match error {
                super::process::SpawnError::Cancelled => SubagentStartError::Cancelled,
                error => SubagentStartError::Spawn {
                    detail: error.to_string(),
                },
            })?
        };
        Ok(PreparedSubagent {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id: spec.tool_call_id.clone(),
            agent: spec.resolved.agent.clone(),
            definition_digest: spec.resolved.definition_digest.clone(),
            task: spec.task.clone(),
            context: spec.context.clone(),
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
            task,
            context,
            staged,
        } = prepared;
        let decision = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let mailbox = self.config.mailbox.clone();
            let clock = self.config.clock.clone();
            let config = &self.config;
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
                        started_at,
                    )) {
                        return Decision::Failed(SubagentStartError::Durability {
                            detail: error.to_string(),
                        });
                    }
                    Decision::Accepted { started_at }
                }) {
                    Ok(decision) => decision,
                    Err(_) => Decision::Failed(SubagentStartError::ConversationInactive),
                };
                if let Decision::Accepted { started_at } = &decision {
                    let record = SubagentRecord {
                        subagent_id: subagent_id.clone(),
                        child_agent_id: child_agent_id.clone(),
                        child_conversation_id: child_conversation_id.clone(),
                        tool_call_id,
                        agent: agent.clone(),
                        definition_digest: definition_digest.clone(),
                        lifecycle: SubagentLifecycle::Running,
                        cancel_reason: None,
                        control: None,
                        detail: None,
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
            Decision::Accepted { .. } => {
                let driver = staged.into_driver(DelegationFrame { task, context });
                let (commands, start_gate, task) = driver.split();
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
                {
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
                    // Sending `Some(reason)` resolves the gate cancelled:
                    // the driver sends that exact reason before Delegate
                    // and never allows child semantic work to begin.
                    // Sending `None` opens the normal gate. The release is
                    // synchronous under the same mutex acquisition, so
                    // start-vs-cancel has exactly one arbitration boundary.
                    let _ = start_gate.send(cancel_before_start);
                }
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
                    result: serde_json::json!({
                        "status": "running",
                        "note": "The child runtime is running asynchronously. Its answer \
                                 arrives as a new turn from the child agent; do not retry \
                                 or poll for it."
                    }),
                }))
            }
        }
    }

    fn clone_for_task(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            state_version: self.state_version.clone(),
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

    /// The consistency snapshots of every known subagent, in ordinal order.
    #[must_use]
    pub fn all_snapshots(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.records.iter().map(SubagentRecord::snapshot).collect()
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
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let &index = state.index.get(subagent_id)?;
        let record = &mut state.records[index];
        if record.lifecycle.is_terminal() || record.publication_abandoned {
            return Some(record.snapshot());
        }
        if matches!(record.lifecycle, SubagentLifecycle::Running) {
            record.lifecycle = SubagentLifecycle::Cancelling;
            record.cancel_reason = Some(reason);
            if let Some(control) = &record.control {
                let _ = control.try_send(super::process::DriverCommand::Cancel { reason });
            }
            publish_snapshot(&mut state, &self.state_version, index);
        }
        Some(state.records[index].snapshot())
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
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return;
        }
        let PhysicalSettlement {
            outcome,
            nested,
            runtime_root_cleanup_error,
        } = settlement;
        // An unproven nested settlement or a failed exact-root cleanup never
        // silently disappears. The child answer remains the semantic
        // candidate, while the physical settlement diagnostic is carried
        // alongside it.
        let settlement_diagnostic = [
            nested.unproven_diagnostic(),
            runtime_root_cleanup_error
                .as_ref()
                .map(|detail| format!("the child physical runtime root was not removed: {detail}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let settlement_diagnostic =
            (!settlement_diagnostic.is_empty()).then_some(settlement_diagnostic.join("; "));
        let physical_settlement_unproven =
            !nested.unproven.is_empty() || runtime_root_cleanup_error.is_some();
        let candidate = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            let record = &mut state.records[index];
            if record.lifecycle.is_terminal() || record.publication_abandoned {
                return;
            }
            let cancelling = matches!(record.lifecycle, SubagentLifecycle::Cancelling);
            // The publication timestamp freezes at canonicalization: every
            // later bounded retry rebuilds the byte-identical draft, so an
            // ambiguous commit resolves as the idempotent correlation
            // retry, never a conflict.
            let timestamp = self.config.clock.now();
            let candidate = match outcome {
                PhysicalOutcome::Completed(frame) => match (cancelling, frame.status) {
                    (false, super::ipc::ChildResultStatus::Succeeded) => TerminalCandidate {
                        state: TerminalState::Succeeded,
                        content: Some(bound_utf8(
                            frame.content.unwrap_or_default(),
                            MAX_RESULT_CONTENT_BYTES,
                        )),
                        diagnostic: None,
                        reason: None,
                        timestamp,
                    },
                    (false, super::ipc::ChildResultStatus::Failed) => TerminalCandidate {
                        state: TerminalState::Failed,
                        content: None,
                        diagnostic: Some(bound_utf8(
                            frame
                                .diagnostic
                                .unwrap_or_else(|| "the child attempt failed".to_owned()),
                            MAX_RESULT_CONTENT_BYTES,
                        )),
                        reason: None,
                        timestamp,
                    },
                    (false, super::ipc::ChildResultStatus::Cancelled) => TerminalCandidate {
                        state: TerminalState::Cancelled,
                        content: None,
                        diagnostic: None,
                        // A child result without a committed parent
                        // cancellation has no semantic reason in its wire
                        // envelope. Do not fabricate UserRequested.
                        reason: None,
                        timestamp,
                    },
                    // Cancellation intent is canonical: a completed frame
                    // after the intent settles as cancelled.
                    (true, _) => TerminalCandidate {
                        state: TerminalState::Cancelled,
                        content: None,
                        diagnostic: None,
                        reason: record.cancel_reason,
                        timestamp,
                    },
                },
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
                    diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                    reason: None,
                    timestamp,
                },
            };
            let candidate = match settlement_diagnostic {
                None => candidate,
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
            record.pending_terminal = Some(candidate.clone());
            record.detail = candidate
                .content
                .clone()
                .or_else(|| candidate.diagnostic.clone())
                .or_else(|| {
                    candidate
                        .reason
                        .map(|reason| reason_text(reason).to_owned())
                });
            candidate
        };
        self.publish_terminal(subagent_id, &candidate);
    }

    /// Attempts the durable terminal publication; on failure, enters
    /// `PublishingTerminal` and schedules the bounded retry.
    fn publish_terminal(&self, subagent_id: &SubagentId, candidate: &TerminalCandidate) {
        let initial_failure = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            let record = &state.records[index];
            let (draft, event) = terminal_publication(
                &self.config.conversation_id,
                subagent_id,
                &record.child_agent_id,
                candidate_state(candidate),
                terminal_blocks(record, candidate),
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
        let (draft, event) = terminal_publication(
            &self.config.conversation_id,
            subagent_id,
            &record.child_agent_id,
            candidate_state(&candidate),
            terminal_blocks(record, &candidate),
            candidate.timestamp,
        );
        match self.config.mailbox.accept_draft_with_event(draft, event) {
            Ok(_) => {
                let record = &mut state.records[index];
                record.lifecycle = match candidate.state {
                    TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                    TerminalState::Failed => SubagentLifecycle::Failed,
                    TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                    TerminalState::Interrupted => SubagentLifecycle::Interrupted,
                };
                record.pending_terminal = None;
                record.publication_abandoned = false;
                record.notification = NotificationState::Delivered;
                publish_snapshot(&mut state, &self.state_version, index);
                Ok(true)
            }
            Err(error) => {
                state.records[index].notification = NotificationState::Failed;
                publish_snapshot(&mut state, &self.state_version, index);
                Err(error.to_string())
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

/// The human-readable cancellation detail.
const fn reason_text(reason: CancellationReason) -> &'static str {
    match reason {
        CancellationReason::UserRequested => "requested by the user",
        CancellationReason::RuntimeShutdown => "the runtime is shutting down",
        CancellationReason::ParentCancelled => "the parent operation was cancelled",
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::SubagentTerminalState;
    use super::super::catalog::SubagentToolSelector;
    use super::super::ipc::{ChildFrame, ChildResultStatus, ParentFrame, ResultFrame};
    use super::*;
    use crate::durable::ConversationStore;
    use crate::runtime::types::{CancellationReason, SystemClock};

    /// A registry over a real (in-memory) durable store with a test seam
    /// for staged children.
    struct TestPlane {
        _dir: tempfile::TempDir,
        registry: SubagentRegistry,
        store: Arc<crate::durable::SqliteConversationStore>,
        conversation_id: ConversationId,
        runtime_root: std::path::PathBuf,
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
        let registry = SubagentRegistry::new(SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-parent"),
            mailbox,
            clock: Arc::new(SystemClock),
            spawn: SubagentSpawnPlan {
                program: std::path::PathBuf::from("/nonexistent/rustx"),
                workspace,
                runtime_root: runtime_root.clone(),
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                agent_status: crate::context::AgentStatusConfig::default(),
                context: SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
            },
            max_active,
        });
        TestPlane {
            _dir: dir,
            registry,
            store,
            conversation_id,
            runtime_root,
        }
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

    /// Stages a scripted child whose process ignores everything and must be
    /// killed; used for cancellation-escalation tests.
    fn stage_stubborn(plane: &TestPlane) -> ScriptedChild {
        stage_process(plane, "trap '' TERM; exec sleep 60")
    }

    fn stage_process(plane: &TestPlane, shell: &str) -> ScriptedChild {
        let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("pair");
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
        let staged = StagedChild::for_test(child, driver_end, child_runtime_root);
        plane.registry.push_staged_override(staged);
        ScriptedChild {
            peer: test_end,
            pid,
        }
    }

    impl ScriptedChild {
        /// Awaits the delegated task and answers with one terminal result.
        async fn complete(mut self, status: ChildResultStatus, content: Option<&str>) {
            let frame = super::super::ipc::read_parent_frame(&mut self.peer)
                .await
                .expect("delegate frame");
            assert!(
                matches!(frame, Some(ParentFrame::Delegate(_))),
                "the committed child is delegated first"
            );
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
            task: task.to_owned(),
            context: None,
            tool_call_id: ToolCallId::new("call-1"),
        }
    }

    fn start_spec(task: &str) -> SubagentStartSpec {
        spec(task)
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
        plane
            .registry
            .push_staged_override(StagedChild::for_test(child, parent, root));
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
        assert_eq!(settled.detail.as_deref(), Some("the answer"));
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

    use crate::context::SessionContextPolicy;
}
