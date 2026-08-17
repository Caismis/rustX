//! The conversation-owned background execution registry.
//!
//! One conversation owns one authoritative background registry. It exists
//! outside any `AgentExecution`: an attempt receives/clones a handle to it,
//! but detached task handles and records are never owned by a single
//! attempt. The registry is the authoritative state machine of every
//! background execution — never messages, never Agent Status text, never
//! Event Journal text — and it makes cross-conversation access structurally
//! impossible: there is no process-global execution lookup table, and every
//! operation is scoped to the registry's conversation.
//!
//! # Dispatch ownership commit
//!
//! Background dispatch is two-stage. [`ConversationBackgroundRegistry::prepare_dispatch`]
//! validates the invocation, allocates the deterministic `exec_N`
//! [`ToolExecutionId`], creates a private prepared record with its own
//! cancellation resources, and spawns the runner behind a start/commit gate
//! (the runner cannot begin before the gate is released). [`ConversationBackgroundRegistry::commit_dispatch`]
//! is the one deterministic linearization point of background ownership:
//! the registry synchronization boundary is acquired first, the activation
//! gate and the final attempt-cancellation observation happen at that same
//! protected boundary, and only then does the commit happen:
//!
//! - the owning conversation runtime is inactive (Issue #61): the commit is
//!   refused with [`BackgroundDispatchError::ConversationInactive`] and the
//!   prepared dispatch rolls back completely — no published record, no
//!   accepted result, the runner never begins. Once a `ConversationToolRuntime`
//!   is claimed by a `ConversationRuntime`, new background ownership commits
//!   cannot begin before `ConversationRuntime::activate`;
//! - attempt cancellation observable at the boundary: the prepared dispatch
//!   rolls back completely under the same boundary — no published record,
//!   no accepted result, the runner is aborted and never begins;
//! - conversation ownership wins: the record is published as `Starting`,
//!   ownership transfers exactly once, the accepted result is produced, and
//!   a later attempt cancellation can never reclaim the detached execution.
//!
//! There is no unchecked window between the deciding observations and the
//! prepared→owned registry transition.
//!
//! # Conversation runtime ownership transfer
//!
//! A standalone `ConversationToolRuntime` may be claimed by a
//! `ConversationRuntime` only while its background plane is **pristine**,
//! and the claim shares the one registry synchronization boundary with
//! `commit_dispatch` (see
//! [`ConversationBackgroundRegistry::claim_conversation_runtime_inactive`]):
//!
//! ```text
//! standalone ConversationToolRuntime
//!         |
//!         |  conversation-runtime ownership transfer
//!         |  (one registry critical section)
//!         |    1. require no prepared dispatch and no committed record
//!         |    2. claim the coordinator binding
//!         |    3. bind the mailbox runtime-owned with the shared
//!         |       Inactive lifecycle
//!         v
//! ConversationRuntime-owned / inactive
//!         |
//!         |  from this point:
//!         |    background commit -> BackgroundDispatchError::ConversationInactive
//!         v
//! ConversationRuntime::activate()   (the shared lifecycle Inactive -> Active)
//! ```
//!
//! Either a standalone background commit wins the section first (the claim
//! is then refused typed because the registry is no longer pristine), or
//! the transfer wins first (a later commit is refused
//! `ConversationInactive`). A `ConversationRuntime` can therefore never
//! adopt committed or staged background work, and the inactive phase can
//! never inherit a detached semantic transition. There is no adoption
//! protocol: a tool runtime that already owns background work simply
//! cannot become the inactive semantic base of a conversation runtime.
//!
//! # Cancellation-vs-completion race
//!
//! The first registry transition that commits either terminal completion or
//! cancellation intent wins the race. If completion
//! (`Succeeded`/`Failed`/`Cancelled`) commits first, a later cancel is an
//! idempotent no-op returning the terminal snapshot. If cancellation
//! intent commits first (`Starting`/`Running` → `Cancelling`), cancellation
//! owns settlement: the cancellation reason is retained for final
//! settlement, and a later normal executor return cannot overwrite the
//! cancellation winner with `Succeeded` — the stored terminal result is
//! canonicalized to `Cancelled` with the retained reason. Only an explicit
//! runtime/process-control failure after cancellation intent settles as
//! `Failed`.
//!
//! # Terminal inbound publication
//!
//! Every successfully dispatched background execution reaches exactly one
//! terminal registry state, and a terminal transition claims at most one
//! runtime inbound notification: a timestamped
//! [`UserMessageBlock`] with
//! [`UserSource::Runtime`] through the owning
//! [`ConversationInboundMailbox`]. The durable terminal inbound obtains
//! ownership **before** the record becomes terminal (Issue #63, Finding 3):
//! `finish` durably accepts the notification first and only then commits the
//! terminal lifecycle. A durable acceptance failure keeps the record
//! non-terminal and records the failure, so an observable terminal settlement
//! always implies the terminal inbound already committed durably.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::events::{RuntimeEvent, RuntimeEventSink};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::RuntimeClock;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolExecutionId, ToolId};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::types::{CancellationReason, ConversationLifecycle};
use serde::{Deserialize, Serialize};

use crate::tools::artifacts::ArtifactStore;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::limits::bound_tool_progress;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    ToolResultContent,
};
use crate::tools::workspace::Workspace;

/// The one cancellation reason of conversation-owned background cancellation.
///
/// Background cancellation is only ever requested through the conversation
/// control path (`background_task(action = cancel)` or direct registry
/// cancellation), which is a user-requested control action. The registry
/// retains this reason when cancellation intent commits so the canonicalized
/// terminal result always agrees with the registry winner.
const BACKGROUND_CANCEL_REASON: CancellationReason = CancellationReason::UserRequested;

/// The public lifecycle of one background execution.
///
/// Terminal states are absorbing. The allowed public transitions are:
///
/// ```text
/// Starting  → Running
/// Starting  → Cancelling
/// Starting  → Succeeded / Failed
/// Running   → Cancelling
/// Running   → Succeeded / Failed
/// Running   → PublishingTerminal
/// Cancelling → Cancelled
/// PublishingTerminal → Succeeded / Failed / Cancelled
/// ```
///
/// [`BackgroundLifecycle::PublishingTerminal`] is the honest non-terminal
/// state in which the executor has returned its terminal candidate and the
/// registry now owns durable terminal publication; it is never `Running`
/// (the runner has exited) and it retains the settlement candidate until
/// publication reaches a terminal outcome. An internal unpublished prepared
/// state implements dispatch atomicity but never leaks as an accepted
/// execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundLifecycle {
    /// The dispatch committed and the runner is starting.
    Starting,
    /// The runner is executing.
    Running,
    /// Cancellation intent committed and owns settlement.
    Cancelling,
    /// The executor returned its terminal candidate; the registry owns
    /// durable terminal publication, which has not committed yet.
    PublishingTerminal,
    /// The execution succeeded.
    Succeeded,
    /// The execution failed.
    Failed,
    /// The execution was cancelled through the cancellation path.
    Cancelled,
}

/// The read-only observation seam of the background registry.
///
/// A state observer receives the authoritative snapshot of one background
/// execution after every published registry transition (dispatch commit,
/// start, progress, cancellation request, and terminal settlement). The
/// callback fires while the registry synchronization boundary is held, so
/// the observed order is exactly the registry linearization order. An
/// observer must never call back into the registry; the Runtime Client
/// projection (Issue #37) treats each callback as one projection fold
/// under its own synchronization boundary.
pub trait BackgroundObserver: Send + Sync {
    /// Observes one authoritative registry transition snapshot.
    fn on_snapshot(&self, snapshot: &BackgroundExecutionSnapshot);
}

impl BackgroundLifecycle {
    /// Whether this state is terminal (absorbing).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Whether this state is active (non-terminal).
    #[must_use]
    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }

    /// The stable serialized name of the state.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::PublishingTerminal => "publishing_terminal",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The one canonical read-only snapshot of one background execution.
///
/// The snapshot is reused by registry queries, `background_task(status)`,
/// `background_task(cancel)`, Agent Status projection input, and
/// deterministic tests. It never exposes internal task handles or process
/// ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundExecutionSnapshot {
    /// The detached runtime execution identity.
    pub execution_id: ToolExecutionId,
    /// The canonical tool identity.
    pub tool_id: ToolId,
    /// The model-facing tool name.
    pub tool_name: String,
    /// The authoritative lifecycle state.
    pub state: BackgroundLifecycle,
    /// The latest bounded progress snapshot, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
    /// The bounded terminal result, when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolExecutionResult>,
}

/// The outcome of a committed background dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundDispatchOutcome {
    /// Conversation ownership committed: the execution is detached and the
    /// accepted attempt-facing result is produced. The accepted result is
    /// the result of the model-issued tool call, not the final result of the
    /// detached execution.
    Accepted {
        /// The allocated runtime execution identity.
        execution_id: ToolExecutionId,
        /// The accepted result: bounded structured content identifying
        /// `execution_id`, `state`, and the tool.
        result: ToolExecutionResult,
    },
    /// Attempt cancellation won before the ownership commit: the prepared
    /// dispatch was rolled back, no execution is detached, and no accepted
    /// result exists.
    RolledBack,
}

/// A background dispatch failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundDispatchError {
    /// The invocation is not a background invocation.
    NotBackgroundInvocation,
    /// The conversation mailbox of this registry is bound to a
    /// `ConversationRuntime` that has not been activated yet (Issue #61).
    ///
    /// A new background ownership commit cannot begin before the owning
    /// conversation runtime activates: the prepared dispatch is rolled
    /// back completely — no published record, no accepted result, and the
    /// runner never begins. The conversation-bound instance follows the
    /// runtime lifecycle; only a standalone (unclaimed) registry commits
    /// unconditionally.
    ConversationInactive {
        /// The conversation whose runtime has not been activated.
        conversation_id: ConversationId,
    },
    /// The execution sequence space is exhausted.
    SequenceExhausted,
    /// An internal dispatch failure.
    Internal(String),
}

impl core::fmt::Display for BackgroundDispatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotBackgroundInvocation => write!(
                f,
                "only background invocations can be dispatched to the background registry"
            ),
            Self::ConversationInactive { conversation_id } => write!(
                f,
                "conversation {conversation_id} is not activated; a new background ownership commit cannot begin before the owning conversation runtime activates"
            ),
            Self::SequenceExhausted => write!(f, "the execution sequence space is exhausted"),
            Self::Internal(message) => write!(f, "background dispatch failed: {message}"),
        }
    }
}

impl std::error::Error for BackgroundDispatchError {}

/// A prepared but not yet committed background dispatch.
///
/// Between [`ConversationBackgroundRegistry::prepare_dispatch`] and
/// [`ConversationBackgroundRegistry::commit_dispatch`] the runner is parked
/// behind its start gate and the dispatch is private. Dropping the prepared
/// dispatch without committing rolls it back: the runner is aborted and no
/// detached execution exists.
#[derive(Debug)]
pub struct PreparedBackgroundDispatch {
    registry: ConversationBackgroundRegistry,
    execution_id: ToolExecutionId,
    committed: bool,
}

impl Drop for PreparedBackgroundDispatch {
    fn drop(&mut self) {
        if !self.committed {
            self.registry.rollback_prepared(&self.execution_id);
        }
    }
}

/// A failure of the `ConversationToolRuntime -> ConversationRuntime`
/// ownership transfer.
///
/// See [`ConversationBackgroundRegistry::claim_conversation_runtime_inactive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackgroundOwnershipClaimError {
    /// The identity is already bound to a conversation runtime.
    AlreadyClaimed,
    /// The background plane is not pristine: it already holds a prepared
    /// dispatch or a committed execution record.
    NotQuiescent,
}

/// The execution resources of the conversation background registry.
#[derive(Clone)]
pub struct BackgroundResources {
    /// The owning conversation inbound mailbox for terminal notifications.
    pub mailbox: ConversationInboundMailbox,
    /// The conversation workspace for detached executors.
    pub workspace: Workspace,
    /// The conversation artifact store for detached executors.
    pub artifacts: ArtifactStore,
    /// The runtime clock stamping terminal inbound messages.
    pub clock: Arc<dyn RuntimeClock>,
    /// The narrow non-durable execution-fact sink, when attached.
    pub event_sink: Option<Arc<dyn RuntimeEventSink>>,
}

/// The per-execution publication state of the terminal inbound message.
///
/// The durable terminal inbound must obtain ownership **before** the record
/// becomes terminal (Issue #63, Finding 3): `finish` durably accepts the
/// terminal notification first and only then commits the terminal lifecycle
/// with `Published`. A durable acceptance failure keeps the record
/// non-terminal and records `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationState {
    Pending,
    Published,
    Failed,
}

/// One published background record.
struct BackgroundRecord {
    execution_id: ToolExecutionId,
    tool_call_id: ToolCallId,
    tool_id: ToolId,
    tool_name: String,
    lifecycle: BackgroundLifecycle,
    cancellation: CancellationSignal,
    /// The retained cancellation reason when cancellation intent committed
    /// (`Cancelling`): the registry keeps it for final settlement, so the
    /// canonicalized terminal result always agrees with the registry
    /// winner.
    cancel_reason: Option<CancellationReason>,
    progress: Option<ToolProgress>,
    result: Option<ToolExecutionResult>,
    /// The retained terminal candidate while durable terminal publication is
    /// pending (`PublishingTerminal`). This is the settlement owner: after the
    /// executor returns, the registry retains the candidate until publication
    /// reaches a terminal outcome.
    pending_terminal: Option<TerminalCandidate>,
    notification: NotificationState,
}

/// The registry-owned terminal settlement candidate of one execution.
///
/// Once the executor returns, this value is retained by the registry (in
/// [`BackgroundRecord::pending_terminal`]) so a durable publication failure
/// can never lose the executor result or leave a `Running` record with no
/// runner.
#[derive(Clone)]
struct TerminalCandidate {
    /// The terminal lifecycle the candidate settles to.
    settled: BackgroundLifecycle,
    /// The exact terminal result the candidate carries.
    result: ToolExecutionResult,
}

/// One prepared (not yet committed) background dispatch.
struct PreparedRecord {
    record: BackgroundRecord,
    gate: Arc<Notify>,
    runner: tokio::task::JoinHandle<()>,
}

/// The synchronized registry state.
struct BackgroundRegistryState {
    next_execution_sequence: u64,
    prepared: HashMap<ToolExecutionId, PreparedRecord>,
    records: Vec<BackgroundRecord>,
    index: HashMap<ToolExecutionId, usize>,
    /// The read-only state observer, installed by the owning runtime client
    /// boundary (Issue #37). It fires while the registry lock is held.
    observer: Option<Arc<dyn BackgroundObserver>>,
    /// Test-only synchronization hook at the dispatch ownership commit
    /// boundary; never present outside `#[cfg(test)]`.
    #[cfg(test)]
    commit_hook: Option<Arc<test_sync::CommitBoundaryHook>>,
}

/// The conversation-owned authoritative background registry.
///
/// The registry is cheaply cloneable; all clones share one synchronized
/// state machine. Dispatch, settlement, cancellation, and queries all pass
/// through the same synchronization boundary, so no timing assumption is
/// ever made.
pub struct ConversationBackgroundRegistry {
    conversation_id: ConversationId,
    inner: Arc<Mutex<BackgroundRegistryState>>,
    resources: BackgroundResources,
    state_version: tokio::sync::watch::Sender<u64>,
}

impl Clone for ConversationBackgroundRegistry {
    fn clone(&self) -> Self {
        Self {
            conversation_id: self.conversation_id.clone(),
            inner: self.inner.clone(),
            resources: self.resources.clone(),
            state_version: self.state_version.clone(),
        }
    }
}

impl core::fmt::Debug for ConversationBackgroundRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConversationBackgroundRegistry")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl ConversationBackgroundRegistry {
    /// Creates the background registry of one conversation.
    #[must_use]
    pub fn new(conversation_id: ConversationId, resources: BackgroundResources) -> Self {
        let (state_version, _) = tokio::sync::watch::channel(0);
        Self {
            conversation_id,
            inner: Arc::new(Mutex::new(BackgroundRegistryState {
                next_execution_sequence: 0,
                prepared: HashMap::new(),
                records: Vec::new(),
                index: HashMap::new(),
                observer: None,
                #[cfg(test)]
                commit_hook: None,
            })),
            resources,
            state_version,
        }
    }

    fn notify_state_change(&self) {
        self.state_version.send_modify(|version| {
            *version = version.saturating_add(1);
        });
    }

    /// Installs the test-only synchronization hook at the dispatch
    /// ownership commit boundary. Only available under `#[cfg(test)]`;
    /// never used by production code.
    #[cfg(test)]
    pub(crate) fn install_commit_boundary_hook(&self, hook: Arc<test_sync::CommitBoundaryHook>) {
        let mut state = self.state();
        state.commit_hook = Some(hook);
    }

    /// Installs the observer and captures every retained record snapshot
    /// as one atomic registry section.
    ///
    /// This is the background-registry half of the Issue #61 adapter
    /// bootstrap handshake: because installation and the record snapshot
    /// share the one registry synchronization boundary, a transition
    /// either linearizes before the section (its snapshot is in the
    /// returned seed and no observation was fired — the observer did not
    /// exist yet) or after it (the installed observer fires it into the
    /// bridge queue). No transition can be lost between the seed and the
    /// live observation stream and none can be applied twice.
    pub(crate) fn install_observer_and_snapshots(
        &self,
        observer: Arc<dyn BackgroundObserver>,
    ) -> Vec<BackgroundExecutionSnapshot> {
        let mut state = self.state();
        state.observer = Some(observer);
        state.records.iter().map(snapshot_of).collect()
    }

    /// The one `ConversationToolRuntime -> ConversationRuntime` ownership
    /// transfer, linearized against [`ConversationBackgroundRegistry::commit_dispatch`].
    ///
    /// Under one registry critical section — the same synchronization
    /// boundary `commit_dispatch` uses for its deciding mailbox-lifecycle
    /// observation — this method:
    ///
    /// ```text
    /// 1. requires a pristine background plane: no prepared dispatch and
    ///    no committed execution record;
    /// 2. claims the coordinator binding of the runtime identity;
    /// 3. binds the canonical mailbox runtime-owned with the fresh
    ///    `Inactive` shared lifecycle.
    /// ```
    ///
    /// The three steps share the registry lock with the dispatch ownership
    /// commit, so they serialize: either a standalone background commit
    /// linearizes first (its record is then visible here, and the claim is
    /// refused [`BackgroundOwnershipClaimError::NotQuiescent`]), or this
    /// transfer linearizes first (the mailbox becomes runtime-owned with
    /// the shared lifecycle `Inactive` before this section ends, and a
    /// later `commit_dispatch` observes it and is refused with
    /// [`BackgroundDispatchError::ConversationInactive`]). A conversation
    /// runtime can therefore never be constructed over a tool runtime that
    /// already contains staged or committed background ownership state, and
    /// a standalone commit can never cross the transfer.
    ///
    /// The mailbox lifecycle bind nests the mailbox lock inside the held
    /// registry lock, the same order `commit_dispatch` uses; no mailbox ->
    /// registry edge exists anywhere, so the lock graph stays acyclic.
    ///
    /// The `coordinator_claimed` atomic is the one-time binding of the
    /// tool-runtime identity ([`ConversationToolRuntime`](crate::tools::runtime::ConversationToolRuntime));
    /// the claim commits under this registry section, so a concurrent
    /// second transfer cannot interleave with the quiescence check. The
    /// `lifecycle` is the [`ConversationLifecycle`](crate::runtime::types::ConversationLifecycle)
    /// composed by the `ConversationRuntime` being constructed; activation
    /// (`Inactive -> Active`) is a later, distinct transition that every
    /// runtime-owned semantic boundary observes through this same handle.
    ///
    /// # Errors
    ///
    /// Returns [`BackgroundOwnershipClaimError::AlreadyClaimed`] when the
    /// identity is already bound to a conversation runtime and
    /// [`BackgroundOwnershipClaimError::NotQuiescent`] when the background
    /// plane is not pristine. On either failure nothing is consumed: no
    /// claim, no mailbox transition, no rollback residue.
    pub(crate) fn claim_conversation_runtime_inactive(
        &self,
        coordinator_claimed: &AtomicBool,
        lifecycle: &ConversationLifecycle,
    ) -> Result<(), BackgroundOwnershipClaimError> {
        let state = self.state();
        if !state.prepared.is_empty() || !state.records.is_empty() {
            return Err(BackgroundOwnershipClaimError::NotQuiescent);
        }
        if coordinator_claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BackgroundOwnershipClaimError::AlreadyClaimed);
        }
        self.resources.mailbox.bind_inactive(lifecycle);
        Ok(())
    }

    /// Reverts a claim taken by [`ConversationBackgroundRegistry::claim_conversation_runtime_inactive`]
    /// that a `ConversationRuntime` construction then failed after.
    ///
    /// Under the registry synchronization boundary the mailbox is restored
    /// to its exact previous standalone unbound state and the coordinator
    /// claim is cleared, so a rejected construction leaves no residue and a
    /// fresh claim of the same identity may be attempted again. This is
    /// never called on runtime drop: a successfully constructed
    /// `ConversationRuntime` owns its identity for its lifetime.
    pub(crate) fn release_conversation_runtime_claim(&self, coordinator_claimed: &AtomicBool) {
        let _state = self.state();
        self.resources.mailbox.unbind();
        coordinator_claimed.store(false, Ordering::Release);
    }

    /// Fires the installed observer for one record snapshot while the
    /// registry lock is held.
    fn observe_record(state: &BackgroundRegistryState, index: usize) {
        if let Some(observer) = &state.observer {
            observer.on_snapshot(&snapshot_of(&state.records[index]));
        }
    }

    /// The conversation this registry belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The conversation resources shared with detached runners.
    #[must_use]
    pub fn resources(&self) -> &BackgroundResources {
        &self.resources
    }

    /// Stage one: prepares a background dispatch.
    ///
    /// Validates that the invocation is a background invocation, allocates
    /// the next deterministic `exec_N` execution id under the registry
    /// synchronization boundary, creates the private prepared record with
    /// its own cancellation signal, and spawns the runner behind the
    /// start/commit gate. The runner cannot begin before the gate is
    /// released by [`ConversationBackgroundRegistry::commit_dispatch`].
    ///
    /// # Errors
    ///
    /// Returns [`BackgroundDispatchError::NotBackgroundInvocation`] for a
    /// foreground invocation and
    /// [`BackgroundDispatchError::SequenceExhausted`] when the sequence
    /// space is exhausted.
    pub fn prepare_dispatch(
        &self,
        invocation: &ToolInvocation,
        executor: &Arc<dyn ToolExecutor>,
        environment: ToolEnvironment,
    ) -> Result<PreparedBackgroundDispatch, BackgroundDispatchError> {
        if invocation.mode != ToolInvocationMode::Background {
            return Err(BackgroundDispatchError::NotBackgroundInvocation);
        }
        let mut state = self.state();
        let next = state
            .next_execution_sequence
            .checked_add(1)
            .ok_or(BackgroundDispatchError::SequenceExhausted)?;
        state.next_execution_sequence = next;
        let execution_id = ToolExecutionId::new(format!("exec_{next}"));
        let cancellation = CancellationSignal::new();
        let gate = Arc::new(Notify::new());
        // The effective attempt environment is captured here, at prepare
        // time — strictly before the background ownership commit — and the
        // detached runner retains exactly this captured environment for its
        // whole lifetime. It never queries the conversation's current
        // capability state later.
        let runner = self.spawn_runner(
            execution_id.clone(),
            invocation.clone(),
            executor.clone(),
            cancellation.clone(),
            gate.clone(),
            environment,
        );
        let prepared = PreparedRecord {
            record: BackgroundRecord {
                execution_id: execution_id.clone(),
                tool_call_id: invocation.call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                tool_name: invocation.tool_name.clone(),
                lifecycle: BackgroundLifecycle::Starting,
                cancellation,
                cancel_reason: None,
                progress: None,
                result: None,
                pending_terminal: None,
                notification: NotificationState::Pending,
            },
            gate,
            runner,
        };
        state.prepared.insert(execution_id.clone(), prepared);
        drop(state);
        self.notify_state_change();
        Ok(PreparedBackgroundDispatch {
            registry: self.clone(),
            execution_id,
            committed: false,
        })
    }

    /// Stage two: commits the prepared dispatch (the linearization point).
    ///
    /// The registry synchronization boundary is acquired first; the
    /// activation gate and the final attempt-cancellation observation
    /// happen at that same protected boundary, so there is no unchecked
    /// window between the deciding observations and the prepared→owned
    /// transition.
    ///
    /// The activation gate is the owning conversation runtime's lifecycle:
    /// when this registry's mailbox is bound to an inactive
    /// `ConversationRuntime`, the commit is refused with
    /// [`BackgroundDispatchError::ConversationInactive`] and the prepared
    /// dispatch rolls back completely — no published record, no accepted
    /// result, and the runner is aborted and never begins (Issue #61:
    /// once a conversation tool runtime is claimed by a conversation
    /// runtime, new background ownership commits cannot begin before
    /// `ConversationRuntime::activate`). A standalone registry whose
    /// mailbox is unbound commits unconditionally.
    ///
    /// If the runtime is active and the attempt cancellation is observable
    /// at the boundary, the prepared dispatch rolls back completely under
    /// the boundary — no published record, no accepted result, and the
    /// runner is aborted and never begins. Otherwise conversation
    /// ownership commits exactly once: the record is published as
    /// `Starting`, the runner gate is released, and the accepted
    /// attempt-facing result is produced. No await or cancellation
    /// checkpoint can split the ownership commit from the accepted result.
    ///
    /// # Errors
    ///
    /// Returns [`BackgroundDispatchError::ConversationInactive`] when the
    /// owning conversation runtime has not been activated.
    pub fn commit_dispatch(
        &self,
        mut prepared: PreparedBackgroundDispatch,
        attempt_cancellation: &CancellationSignal,
    ) -> Result<BackgroundDispatchOutcome, BackgroundDispatchError> {
        let mut state = self.state();
        // The activation gate: observed under this registry critical
        // section, so the commit linearizes cleanly against
        // `ConversationRuntime::activate` — a commit that observes the
        // pre-activation state is refused, one that observes the
        // post-activation state is a real post-activation transition. On
        // refusal the prepared handle drops and rolls the dispatch back.
        if self.resources.mailbox.is_bound_inactive() {
            return Err(BackgroundDispatchError::ConversationInactive {
                conversation_id: self.conversation_id.clone(),
            });
        }
        // TEST-ONLY ownership-commit boundary: the registry lock is held and
        // the deciding cancellation observation is next. Tests park here to
        // make the linearization exact.
        #[cfg(test)]
        if let Some(hook) = &state.commit_hook {
            hook.enter();
        }
        if attempt_cancellation.is_cancelled() {
            // The deciding cancellation observation and the rollback share
            // this critical section: the prepared record is removed and the
            // runner aborted here, and the prepared handle's drop semantics
            // are neutralized so no second rollback path exists.
            if let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) {
                prepared_record.runner.abort();
            }
            prepared.committed = true;
            return Ok(BackgroundDispatchOutcome::RolledBack);
        }
        let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) else {
            prepared.committed = true;
            return Ok(BackgroundDispatchOutcome::RolledBack);
        };
        let result = accepted_result(&prepared.execution_id, &prepared_record.record.tool_name);
        let execution_id = prepared.execution_id.clone();
        let next_index = state.records.len();
        state.index.insert(execution_id.clone(), next_index);
        state.records.push(prepared_record.record);
        Self::observe_record(&state, next_index);
        drop(state);
        self.notify_state_change();
        prepared.committed = true;
        prepared_record.gate.notify_one();
        Ok(BackgroundDispatchOutcome::Accepted {
            execution_id,
            result,
        })
    }

    /// Requests cancellation of one execution and returns the canonical
    /// snapshot after processing the request.
    ///
    /// Cancellation is idempotent: for an already-cancelling or terminal
    /// execution the current snapshot is returned unchanged and the state is
    /// never destructively changed. An unknown execution id returns `None`.
    ///
    /// When cancellation intent commits, the cancellation reason is
    /// retained in the record; the registry is the settlement authority and
    /// uses it to canonicalize the final terminal result, so the registry
    /// winner and the stored result can never disagree.
    #[must_use]
    pub fn cancel(&self, execution_id: &ToolExecutionId) -> Option<BackgroundExecutionSnapshot> {
        let mut state = self.state();
        let index = *state.index.get(execution_id)?;
        {
            let record = &mut state.records[index];
            match record.lifecycle {
                BackgroundLifecycle::Starting | BackgroundLifecycle::Running => {
                    record.lifecycle = BackgroundLifecycle::Cancelling;
                    record.cancel_reason = Some(BACKGROUND_CANCEL_REASON);
                    record.cancellation.cancel();
                }
                BackgroundLifecycle::Cancelling
                | BackgroundLifecycle::PublishingTerminal
                | BackgroundLifecycle::Succeeded
                | BackgroundLifecycle::Failed
                | BackgroundLifecycle::Cancelled => {}
            }
        }
        Self::observe_record(&state, index);
        let snapshot = snapshot_of(&state.records[index]);
        drop(state);
        self.notify_state_change();
        Some(snapshot)
    }

    /// The canonical snapshot of one execution, active or terminal.
    #[must_use]
    pub fn snapshot(&self, execution_id: &ToolExecutionId) -> Option<BackgroundExecutionSnapshot> {
        let state = self.state();
        let index = *state.index.get(execution_id)?;
        Some(snapshot_of(&state.records[index]))
    }

    /// The active (Starting/Running/Cancelling) snapshots in execution
    /// allocation order. Terminal executions never appear here.
    #[must_use]
    pub fn active_snapshot(&self) -> Vec<BackgroundExecutionSnapshot> {
        let state = self.state();
        state
            .records
            .iter()
            .filter(|record| record.lifecycle.is_active())
            .map(snapshot_of)
            .collect()
    }

    /// All snapshots (active and terminal) in execution allocation order.
    ///
    /// Terminal records remain queryable for the conversation lifetime.
    #[must_use]
    pub fn all_snapshots(&self) -> Vec<BackgroundExecutionSnapshot> {
        let state = self.state();
        state.records.iter().map(snapshot_of).collect()
    }

    /// The runner-owned settlement boundary of one execution.
    ///
    /// The first registry transition that commits either terminal completion
    /// or cancellation intent wins the race (see the module documentation).
    /// A terminal transition may claim at most one runtime inbound
    /// publication; duplicate settlement calls are idempotent no-ops.
    ///
    /// The durable terminal inbound commits **before** the terminal lifecycle
    /// (Issue #63, Finding 3): the terminal candidate is computed first, the
    /// notification is durably accepted, and only then is the record exposed
    /// as terminal. A durable acceptance failure leaves the record
    /// non-terminal and records the failure, so an observable terminal
    /// settlement always implies the terminal inbound already committed
    /// durably.
    ///
    /// When cancellation intent already owns settlement (`Cancelling`), a
    /// later normal executor return cannot contradict the registry winner:
    /// the stored terminal result is canonicalized to `Cancelled` with the
    /// retained cancellation reason, preserving useful bounded result data
    /// and artifacts where present. Only an explicit runtime/process-control
    /// failure after cancellation intent settles as `Failed`.
    pub fn finish(&self, execution_id: &ToolExecutionId, result: &ToolExecutionResult) {
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        // Issue #63 (Finding 3 + Blocker 2): compute the terminal candidate
        // **without** committing the lifecycle, durably accept the terminal
        // inbound notification, and only then commit the terminal lifecycle.
        // The observable terminal settlement therefore implies the terminal
        // inbound already obtained durable ownership. After the executor
        // returns, the candidate is retained by the registry until settlement
        // reaches a terminal outcome — it never disappears on a publication
        // failure.
        let candidate = {
            let record = &state.records[index];
            if record.lifecycle.is_terminal() {
                return;
            }
            match record.lifecycle {
                BackgroundLifecycle::Starting | BackgroundLifecycle::Running => {
                    match result.status {
                        ToolExecutionStatus::Success => {
                            (BackgroundLifecycle::Succeeded, result.clone())
                        }
                        ToolExecutionStatus::Cancelled { .. } => {
                            (BackgroundLifecycle::Cancelled, result.clone())
                        }
                        ToolExecutionStatus::Failed { .. }
                        | ToolExecutionStatus::TimedOut
                        | ToolExecutionStatus::Interrupted => {
                            (BackgroundLifecycle::Failed, result.clone())
                        }
                    }
                }
                BackgroundLifecycle::Cancelling => {
                    // Cancellation intent already owns settlement. A normal
                    // executor return must not overwrite the cancellation
                    // winner; only an explicit runtime/process-control failure
                    // is represented as Failed.
                    if matches!(result.status, ToolExecutionStatus::Failed { .. }) {
                        (BackgroundLifecycle::Failed, result.clone())
                    } else {
                        let mut canonical = result.clone();
                        canonical.status = ToolExecutionStatus::Cancelled {
                            reason: record.cancel_reason.unwrap_or(BACKGROUND_CANCEL_REASON),
                        };
                        (BackgroundLifecycle::Cancelled, canonical)
                    }
                }
                BackgroundLifecycle::PublishingTerminal
                | BackgroundLifecycle::Succeeded
                | BackgroundLifecycle::Failed
                | BackgroundLifecycle::Cancelled => return,
            }
        };
        let (settled, stored) = candidate;
        // The registry retains the terminal candidate before publication, so
        // a durable acceptance failure cannot lose the executor result and
        // cannot leave a false `Running` state with no runner.
        state.records[index].pending_terminal = Some(TerminalCandidate {
            settled,
            result: stored.clone(),
        });
        let notification = terminal_inbound_message(
            execution_id,
            &state.records[index].tool_name,
            settled,
            &stored.artifacts,
            self.resources.clock.now(),
        );
        // The background terminal notification uses the same durable
        // acceptance owner as every other inbound producer (Issue #63), with
        // a deterministic producer correlation so a retry with the same
        // committed correlation can never publish a duplicate notification.
        // Durable acceptance commits **before** the terminal lifecycle.
        let correlation = format!("background-terminal:{}", execution_id.as_str());
        match self
            .resources
            .mailbox
            .enqueue_correlated(notification, correlation)
        {
            Ok(_) => {
                let record = &mut state.records[index];
                record.lifecycle = settled;
                record.result = Some(stored);
                record.notification = NotificationState::Published;
                record.pending_terminal = None;
            }
            Err(_error) => {
                // The terminal inbound did not obtain durable ownership, so
                // the record must NOT become terminal and must NOT stay
                // `Running` (the runner has exited). It enters the explicit
                // publication-pending state with the candidate retained for a
                // later `retry_terminal_publication`.
                let record = &mut state.records[index];
                record.lifecycle = BackgroundLifecycle::PublishingTerminal;
                record.notification = NotificationState::Failed;
            }
        }
        Self::observe_record(&state, index);
        drop(state);
        self.notify_state_change();
    }

    /// Retries the durable terminal publication of one execution that is in
    /// [`BackgroundLifecycle::PublishingTerminal`], using the retained
    /// terminal candidate and the stable correlation.
    ///
    /// This is the narrow owned retry trigger of the background settlement
    /// owner (Blocker 2): a `PublishingTerminal` record always retains its
    /// candidate and can reach a terminal outcome through this seam without
    /// duplicating the terminal inbound (the correlation is exactly-once).
    #[must_use]
    pub fn retry_terminal_publication(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        let mut state = self.state();
        let index = *state.index.get(execution_id)?;
        let Some(candidate) = state.records[index].pending_terminal.clone() else {
            let snapshot = snapshot_of(&state.records[index]);
            drop(state);
            return Some(snapshot);
        };
        if state.records[index].lifecycle != BackgroundLifecycle::PublishingTerminal {
            let snapshot = snapshot_of(&state.records[index]);
            drop(state);
            return Some(snapshot);
        }
        let notification = terminal_inbound_message(
            execution_id,
            &state.records[index].tool_name,
            candidate.settled,
            &candidate.result.artifacts,
            self.resources.clock.now(),
        );
        let correlation = format!("background-terminal:{}", execution_id.as_str());
        match self
            .resources
            .mailbox
            .enqueue_correlated(notification, correlation)
        {
            Ok(_) => {
                let record = &mut state.records[index];
                record.lifecycle = candidate.settled;
                record.result = Some(candidate.result);
                record.notification = NotificationState::Published;
                record.pending_terminal = None;
            }
            Err(_error) => {
                state.records[index].notification = NotificationState::Failed;
            }
        }
        Self::observe_record(&state, index);
        let snapshot = snapshot_of(&state.records[index]);
        drop(state);
        self.notify_state_change();
        Some(snapshot)
    }

    /// Updates the latest bounded progress snapshot of one execution and
    /// emits the corresponding canonical execution fact through the narrow
    /// event seam, when one is attached.
    ///
    /// Every progress notification is normalized through the one shared
    /// UTF-8-safe bound (`bound_tool_progress`), the same normalization the
    /// foreground path uses. Only the current/latest bounded progress
    /// snapshot is retained; no unbounded progress history exists in the
    /// registry. Progress of a terminal execution is ignored.
    pub fn report_progress(&self, execution_id: &ToolExecutionId, progress: ToolProgress) {
        let bounded = bound_tool_progress(progress);
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        {
            let record = &mut state.records[index];
            if !record.lifecycle.is_active() {
                return;
            }
            record.progress = Some(bounded.clone());
        }
        Self::observe_record(&state, index);
        let record = &state.records[index];
        let event = RuntimeEvent::ToolExecutionProgress {
            tool_call_id: record.tool_call_id.clone(),
            tool_id: record.tool_id.clone(),
            execution_id: Some(execution_id.clone()),
            progress: bounded,
        };
        drop(state);
        if let Some(sink) = &self.resources.event_sink {
            sink.emit(event);
        }
        self.notify_state_change();
    }

    /// The synchronized registry state.
    ///
    /// # Panics
    ///
    /// Panics only if the registry lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    fn state(&self) -> std::sync::MutexGuard<'_, BackgroundRegistryState> {
        self.inner
            .lock()
            .expect("background registry lock poisoned")
    }

    /// The runner-owned start boundary: the published `Starting` record
    /// transitions to `Running` immediately before the executor begins.
    /// A record already claimed by cancellation intent stays `Cancelling`.
    pub fn mark_running(&self, execution_id: &ToolExecutionId) {
        let mut state = self.state();
        let Some(index) = state.index.get(execution_id).copied() else {
            return;
        };
        {
            let record = &mut state.records[index];
            if record.lifecycle == BackgroundLifecycle::Starting {
                record.lifecycle = BackgroundLifecycle::Running;
            } else {
                return;
            }
        }
        Self::observe_record(&state, index);
        drop(state);
        self.notify_state_change();
    }

    /// Waits for one execution to reach an absorbing terminal state using the
    /// registry's exact state-change notification, not scheduler polling.
    pub async fn wait_until_terminal(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        let mut version = self.state_version.subscribe();
        loop {
            if let Some(snapshot) = self.snapshot(execution_id)
                && snapshot.state.is_terminal()
            {
                return Some(snapshot);
            }
            version.changed().await.ok()?;
        }
    }

    /// Rolls a prepared dispatch back: the runner is aborted and the private
    /// record is dropped. No detached execution exists afterwards.
    fn rollback_prepared(&self, execution_id: &ToolExecutionId) {
        let mut state = self.state();
        if let Some(prepared) = state.prepared.remove(execution_id) {
            prepared.runner.abort();
        }
    }

    /// Spawns the gated runner of one background execution.
    fn spawn_runner(
        &self,
        execution_id: ToolExecutionId,
        invocation: ToolInvocation,
        executor: Arc<dyn ToolExecutor>,
        cancellation: CancellationSignal,
        gate: Arc<Notify>,
        environment: ToolEnvironment,
    ) -> tokio::task::JoinHandle<()> {
        let registry = self.clone();
        tokio::spawn(async move {
            gate.notified().await;
            registry.mark_running(&execution_id);
            let reporter = BackgroundProgressReporter {
                registry: registry.clone(),
                execution_id: execution_id.clone(),
            };
            let resources = &registry.resources;
            let context = ToolExecutionContext {
                conversation_id: &registry.conversation_id,
                execution_id: Some(&execution_id),
                cancellation: cancellation.clone(),
                workspace: &resources.workspace,
                progress: &reporter,
                artifacts: &resources.artifacts,
                environment: &environment,
            };
            let result = executor.execute(invocation, context).await;
            registry.finish(&execution_id, &result);
        })
    }
}

fn snapshot_of(record: &BackgroundRecord) -> BackgroundExecutionSnapshot {
    BackgroundExecutionSnapshot {
        execution_id: record.execution_id.clone(),
        tool_id: record.tool_id.clone(),
        tool_name: record.tool_name.clone(),
        state: record.lifecycle,
        progress: record.progress.clone(),
        result: record.result.clone().or_else(|| {
            record
                .pending_terminal
                .as_ref()
                .map(|candidate| candidate.result.clone())
        }),
    }
}

/// The deterministic accepted result of a successful background dispatch.
fn accepted_result(execution_id: &ToolExecutionId, tool_name: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json {
            value: serde_json::json!({
                "execution_id": execution_id.as_str(),
                "state": "starting",
                "tool": tool_name,
            }),
        }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

/// The timestamped compact terminal inbound message of one settlement.
///
/// The message contains a compact deterministic terminal summary; full
/// output is never dumped into the inbound message (detailed inspection
/// remains `background_task(status)`). Artifact references are included
/// where useful.
fn terminal_inbound_message(
    execution_id: &ToolExecutionId,
    tool_name: &str,
    state: BackgroundLifecycle,
    artifacts: &[crate::message::content::FileReference],
    timestamp: chrono::DateTime<chrono::Utc>,
) -> UserMessageBlock {
    let mut content = vec![UserContentBlock::Text(TextBlock {
        text: format!(
            "Background execution {} ({tool_name}) settled: {}",
            execution_id.as_str(),
            state.name()
        ),
    })];
    for artifact in artifacts {
        content.push(UserContentBlock::File(artifact.clone()));
    }
    UserMessageBlock {
        id: MessageId::new(format!("background-{}-terminal", execution_id.as_str())),
        content,
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    }
}

/// The background progress reporter handed to detached executors: it
/// updates the registry's latest progress snapshot and emits the
/// corresponding canonical execution fact.
struct BackgroundProgressReporter {
    registry: ConversationBackgroundRegistry,
    execution_id: ToolExecutionId,
}

impl ProgressReporter for BackgroundProgressReporter {
    fn report(&self, progress: ToolProgress) {
        self.registry.report_progress(&self.execution_id, progress);
    }
}

/// Test-only synchronization for the dispatch ownership commit boundary.
///
/// [`CommitBoundaryHook::enter`] is called by `commit_dispatch` while the
/// registry synchronization lock is held, immediately before the deciding
/// attempt-cancellation observation. It signals `entered` and parks the
/// calling thread until the test calls `proceed`, so a test can prove the
/// exact linearization: cancellation made observable after `entered` but
/// before `proceed` is necessarily observed at the protected boundary and
/// rolls the prepared dispatch back; a commit released without
/// interruption is never reclaimable by a later attempt cancellation.
///
/// All synchronization is `std` (mutex + condvar) because the commit
/// boundary is a `std` mutex critical section; the parking blocks the OS
/// thread, so the race tests run on a multi-threaded runtime. This module
/// exists only under `#[cfg(test)]`.
#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::{Condvar, Mutex};

    /// The two-phase gate of the commit boundary.
    #[derive(Debug, Default)]
    pub(crate) struct CommitBoundaryHook {
        state: Mutex<HookState>,
        condvar: Condvar,
    }

    #[derive(Debug, Default)]
    struct HookState {
        entered: bool,
        proceed: bool,
    }

    impl CommitBoundaryHook {
        /// Signals that the commit boundary was entered (the registry lock
        /// is held and the deciding cancellation observation is next), then
        /// blocks until [`CommitBoundaryHook::proceed`].
        pub(crate) fn enter(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            state.entered = true;
            self.condvar.notify_all();
            while !state.proceed {
                state = self.condvar.wait(state).expect("commit hook wait poisoned");
            }
        }

        /// Blocks until the commit boundary was entered.
        pub(crate) fn wait_entered(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            while !state.entered {
                state = self.condvar.wait(state).expect("commit hook wait poisoned");
            }
        }

        /// Releases a parked commit boundary.
        pub(crate) fn proceed(&self) {
            let mut state = self.state.lock().expect("commit hook lock poisoned");
            state.proceed = true;
            self.condvar.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;
    use tokio::sync::watch;

    use super::test_sync::CommitBoundaryHook;
    use super::{
        BACKGROUND_CANCEL_REASON, BackgroundDispatchOutcome, BackgroundLifecycle,
        BackgroundResources, ConversationBackgroundRegistry,
    };
    use crate::durable::inbox::InboundStore;
    use crate::events::RecordingEventSink;
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolExecutionId, ToolId};
    use crate::runtime::inbound::ConversationInboundMailbox;
    use crate::runtime::types::CancellationReason;
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
    use crate::tools::types::{
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    };
    use crate::tools::workspace::Workspace;

    fn success() -> ToolExecutionResult {
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
        }
    }

    fn background_invocation(tool: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new(format!("tool-{tool}")),
            tool_name: tool.to_owned(),
            mode: ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        }
    }

    struct TestRegistry {
        _dir: tempfile::TempDir,
        registry: ConversationBackgroundRegistry,
        mailbox: ConversationInboundMailbox,
    }

    fn registry(conversation_id: &str) -> TestRegistry {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        let conversation = ConversationId::new(conversation_id);
        let mailbox = ConversationInboundMailbox::new(conversation.clone());
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox: mailbox.clone(),
                workspace: Workspace::new(&workspace_root).expect("workspace"),
                artifacts: ArtifactStore::new(conversation, &artifacts).expect("artifacts"),
                clock: Arc::new(crate::runtime::SystemClock),
                event_sink: None,
            },
        );
        TestRegistry {
            _dir: dir,
            registry,
            mailbox,
        }
    }

    /// A background registry over an explicit file-backed durable store, so a
    /// test can inject acceptance faults and reopen the database.
    struct FileRegistry {
        _dir: tempfile::TempDir,
        registry: ConversationBackgroundRegistry,
        store: Arc<crate::durable::SqliteInboundStore>,
        store_path: std::path::PathBuf,
    }

    impl FileRegistry {
        fn store_path(&self) -> &std::path::Path {
            &self.store_path
        }
    }

    fn file_registry(conversation_id: &str) -> FileRegistry {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let conversation = ConversationId::new(conversation_id);
        let store_path = artifacts.join("inbound.db");
        let store = Arc::new(
            crate::durable::SqliteInboundStore::open(conversation.clone(), &store_path)
                .expect("store"),
        );
        let mailbox = ConversationInboundMailbox::over_store(store.clone());
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox,
                workspace: Workspace::new(&workspace_root).expect("workspace"),
                artifacts: ArtifactStore::new(conversation, &artifacts).expect("artifacts"),
                clock: Arc::new(crate::runtime::SystemClock),
                event_sink: None,
            },
        );
        FileRegistry {
            _dir: dir,
            registry,
            store,
            store_path,
        }
    }

    /// An executor that waits for durable release state and then returns a
    /// fixed result, deliberately ignoring the cancellation signal.
    struct IgnoreCancellationExecutor {
        started: watch::Sender<bool>,
        release: watch::Sender<bool>,
        result: ToolExecutionResult,
    }

    impl IgnoreCancellationExecutor {
        fn new(result: ToolExecutionResult) -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
            let (started, started_rx) = watch::channel(false);
            let (release, _release_rx) = watch::channel(false);
            (
                Self {
                    started,
                    release: release.clone(),
                    result,
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for IgnoreCancellationExecutor {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            let started = self.started.clone();
            let mut release = self.release.subscribe();
            let result = self.result.clone();
            Box::pin(async move {
                started.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("release channel stays open");
                result
            })
        }
    }

    fn prepare(
        fixture: &TestRegistry,
        executor: &Arc<dyn ToolExecutor>,
    ) -> super::PreparedBackgroundDispatch {
        fixture
            .registry
            .prepare_dispatch(
                &background_invocation("bash"),
                executor,
                ToolEnvironment::new(),
            )
            .expect("prepare")
    }

    /// A dispatch commit on a registry whose mailbox is bound to an
    /// inactive conversation runtime is refused typed (Issue #61): no
    /// published record, no accepted result, and the runner never begins —
    /// the prepared dispatch rolls back completely. The shared lifecycle's
    /// activation transition restores normal dispatch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_is_refused_while_the_owning_runtime_is_inactive() {
        let fixture = registry("conv-bg-gated");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        // Claim the registry's mailbox exactly as
        // `ConversationRuntime::new` does: the ownership transfer binds
        // the mailbox with a fresh Inactive shared lifecycle.
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        fixture.mailbox.bind_inactive(&lifecycle);

        let prepared = prepare(&fixture, &executor);
        let refused = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect_err("a runtime-owned commit before activation is refused");
        assert_eq!(
            refused,
            super::BackgroundDispatchError::ConversationInactive {
                conversation_id: ConversationId::new("conv-bg-gated"),
            }
        );
        assert_eq!(
            fixture.registry.all_snapshots().len(),
            0,
            "the refused commit published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never began");

        // Activation (the shared lifecycle transition) restores normal
        // dispatch.
        assert!(lifecycle.activate(), "the first activation wins");
        let prepared = prepare(&fixture, &executor);
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("dispatch commits after activation")
        else {
            panic!("accepted");
        };
        await_test_started(&mut started, "the post-activation runner starts").await;
        release.send_replace(true);
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the post-activation execution settles normally"
        );
    }

    /// Cancellation observable at the ownership-commit boundary rolls the
    /// prepared dispatch back: no published record, no accepted result, and
    /// the runner never begins. The test parks the commit exactly between
    /// lock acquisition and the deciding cancellation observation, so the
    /// race is proven without timing assumptions.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_observable_at_the_commit_boundary_rolls_back() {
        let fixture = registry("conv-bg");
        let (executor, mut started, _release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let attempt_cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let hook = Arc::new(CommitBoundaryHook::default());
        fixture.registry.install_commit_boundary_hook(hook.clone());

        let registry = fixture.registry.clone();
        let attempt_for_task = attempt_cancellation.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            registry
                .commit_dispatch(prepared, &attempt_for_task)
                .expect("the commit returns an outcome")
        });
        // The commit is parked inside its critical section: the deciding
        // cancellation observation is next. The hook interactions run on
        // the blocking pool so no tokio worker thread is ever blocked.
        let entered = {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.wait_entered())
        };
        entered.await.expect("commit boundary entered");
        attempt_cancellation.cancel();
        let proceed = {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.proceed())
        };
        proceed.await.expect("commit boundary released");
        let outcome = commit_task.await.expect("commit task returns an outcome");
        assert_eq!(
            outcome,
            BackgroundDispatchOutcome::RolledBack,
            "cancellation observable at the boundary means no accepted result"
        );
        assert_eq!(
            fixture.registry.all_snapshots().len(),
            0,
            "no detached execution is published"
        );
        let started_outcome = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            started.wait_for(|started| *started),
        )
        .await;
        assert!(
            !matches!(started_outcome, Ok(Ok(_))),
            "the rolled-back runner must never begin"
        );
    }

    /// Ownership committed at the boundary is never reclaimable: after the
    /// commit completes, a later attempt cancellation cannot stop the
    /// conversation-owned runner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_wins_and_later_attempt_cancellation_cannot_reclaim() {
        let fixture = registry("conv-bg");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let attempt_cancellation = crate::runtime::cancellation::CancellationSignal::new();
        let hook = Arc::new(CommitBoundaryHook::default());
        fixture.registry.install_commit_boundary_hook(hook.clone());

        let registry = fixture.registry.clone();
        let attempt_for_task = attempt_cancellation.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            registry
                .commit_dispatch(prepared, &attempt_for_task)
                .expect("the commit returns an outcome")
        });
        // Release the boundary immediately: ownership commits while the
        // attempt cancellation is still fresh. The hook interactions run
        // on the blocking pool so no tokio worker thread is ever blocked.
        let boundary = {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || {
                hook.wait_entered();
                hook.proceed();
            })
        };
        boundary.await.expect("commit boundary released");
        let outcome = commit_task.await.expect("commit task returns an outcome");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("expected accepted");
        };
        // Attempt cancellation after the commit cannot reclaim the work.
        attempt_cancellation.cancel();
        await_test_started(&mut started, "the conversation-owned runner still starts").await;
        release.send_replace(true);
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the conversation-owned execution settles normally after the commit"
        );
    }

    /// Cancellation winner consistency: cancellation commits while the
    /// executor runs; the executor ignores cancellation and returns
    /// `Success`; the registry canonicalizes the stored terminal result to
    /// `Cancelled` with the retained reason, and exactly one terminal
    /// inbound publication exists.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_winner_canonicalizes_the_terminal_result() {
        let fixture = registry("conv-bg");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let outcome = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("the commit returns an outcome");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_test_started(&mut started, "runner started").await;
        // Cancellation wins in the registry while the executor is running.
        let cancelling = fixture.registry.cancel(&execution_id).expect("cancel");
        assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
        // The executor ignores cancellation and returns Success.
        release.send_replace(true);
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Cancelled,
            "the registry cancellation winner owns settlement"
        );
        let result = terminal.result.expect("terminal result");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: BACKGROUND_CANCEL_REASON,
            },
            "the stored terminal result agrees with the registry winner"
        );
        let batch = fixture
            .mailbox
            .select_pending_batch()
            .expect("select")
            .expect("one terminal batch");
        assert_eq!(
            batch.items().len(),
            1,
            "exactly one terminal inbound publication"
        );
        let _ = fixture.mailbox.adopt_pending_batch(&batch).expect("adopt");
        assert!(
            fixture
                .mailbox
                .select_pending_batch()
                .expect("select")
                .is_none()
        );
    }

    /// Issue #63 (Blocker 2, Test 1): when the real runner's single
    /// settlement call hits a durable terminal-inbound acceptance failure, the
    /// registry retains the terminal candidate in the explicit
    /// `PublishingTerminal` state — never `Running` with a lost result and no
    /// settlement owner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_publication_failure_retains_candidate_as_publishing_terminal() {
        let fixture = file_registry("conv-bg-fault");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = fixture
            .registry
            .prepare_dispatch(
                &background_invocation("bash"),
                &executor,
                ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("commit");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_test_started(&mut started, "runner started").await;

        // Arm the acceptance fault and release the real runner: its single
        // settlement call (inside `finish`) fails the durable acceptance.
        fixture.store.arm_fail_next_accept_commit();
        release.send_replace(true);

        // The runner invokes finish exactly once; the resulting state is the
        // explicit publication-pending state with the candidate retained.
        let snapshot = wait_for_state(
            &fixture,
            &execution_id,
            BackgroundLifecycle::PublishingTerminal,
        )
        .await;
        assert_eq!(snapshot.state, BackgroundLifecycle::PublishingTerminal);
        assert_ne!(
            snapshot.state,
            BackgroundLifecycle::Running,
            "the record must not fake Running after the runner has exited"
        );
        let result = snapshot
            .result
            .expect("the retained terminal candidate is not lost");
        assert_eq!(result.status, ToolExecutionStatus::Success);
        // No durable acceptance committed (the fault rolled the transaction
        // back).
        assert!(
            fixture.store.load_pending().expect("load").is_empty(),
            "the failed acceptance left no durable pending record"
        );
        assert!(
            fixture.store.load_canonical().expect("load").is_empty(),
            "the failed acceptance left no durable canonical record"
        );
    }

    /// Issue #63 (Blocker 2, Test 2 + 3): the retained candidate is
    /// finalized by the narrow owned retry — durable terminal inbound exactly
    /// once, terminal lifecycle exactly once, and the same correlation
    /// resolves to the same acceptance.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_terminal_publication_reaches_terminal_exactly_once() {
        let fixture = file_registry("conv-bg-retry");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = fixture
            .registry
            .prepare_dispatch(
                &background_invocation("bash"),
                &executor,
                ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("commit");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_test_started(&mut started, "runner started").await;

        fixture.store.arm_fail_next_accept_commit();
        release.send_replace(true);
        let pending = wait_for_state(
            &fixture,
            &execution_id,
            BackgroundLifecycle::PublishingTerminal,
        )
        .await;
        assert_eq!(pending.state, BackgroundLifecycle::PublishingTerminal);

        // The narrow owned retry finalizes the retained candidate.
        let terminal = fixture
            .registry
            .retry_terminal_publication(&execution_id)
            .expect("record");
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        assert!(terminal.result.is_some());

        // Durable terminal inbound exactly once, under the stable correlation.
        let items = fixture.store.load_pending().expect("load");
        assert_eq!(items.len(), 1, "exactly one durable terminal inbound");
        let expected_correlation = format!("background-terminal:{}", execution_id.as_str());
        assert_eq!(
            items[0].correlation.as_deref(),
            Some(expected_correlation.as_str()),
            "the same correlation resolves to the same acceptance"
        );

        // A second retry is an idempotent no-op: no duplicate delivery.
        let again = fixture
            .registry
            .retry_terminal_publication(&execution_id)
            .expect("record");
        assert_eq!(again.state, BackgroundLifecycle::Succeeded);
        assert_eq!(
            fixture.store.load_pending().expect("load").len(),
            1,
            "no duplicate pending delivery is manufactured"
        );
    }

    /// Issue #63 (Finding 3): successful terminal settlement implies the
    /// terminal inbound already obtained durable ownership, and that durable
    /// delivery survives a reopen (process restart).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn successful_terminal_settlement_implies_durable_inbound_ownership() {
        let fixture = file_registry("conv-bg-durable");
        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = fixture
            .registry
            .prepare_dispatch(
                &background_invocation("bash"),
                &executor,
                ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("commit");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_test_started(&mut started, "runner started").await;
        release.send_replace(true);
        let terminal = fixture
            .registry
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        // The durable terminal inbound is owned before the terminal registry
        // state: it is durably pending, awaiting safe-boundary adoption.
        assert_eq!(
            fixture.store.load_pending().expect("load").len(),
            1,
            "exactly one durable terminal delivery"
        );
        // A second connection over the same database file (the process
        // restart boundary) observes the same durable delivery.
        let reopened = crate::durable::SqliteInboundStore::open(
            ConversationId::new("conv-bg-durable"),
            fixture.store_path(),
        )
        .expect("reopen");
        assert_eq!(
            reopened.load_pending().expect("load").len(),
            1,
            "the durable terminal delivery survives restart"
        );
    }

    /// Oversized multibyte progress cannot panic or strand the execution:
    /// the shared UTF-8-safe bound normalizes the message and the execution
    /// still reaches its terminal state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_multibyte_progress_cannot_panic_or_strand() {
        struct ProgressThenDone;
        impl ToolExecutor for ProgressThenDone {
            fn execute<'a>(
                &'a self,
                _invocation: ToolInvocation,
                context: ToolExecutionContext<'a>,
            ) -> BoxFuture<'a, ToolExecutionResult> {
                let message = format!("{}😀", "x".repeat(1024));
                context.progress.report(ToolProgress {
                    message: Some(message),
                    completed: Some(1.0),
                    total: Some(2.0),
                });
                Box::pin(async move { success() })
            }
        }
        let sink = Arc::new(RecordingEventSink::new());
        let sink_dyn: Arc<dyn crate::events::RuntimeEventSink> = sink.clone();
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        let conversation = ConversationId::new("conv-bg");
        let mailbox = ConversationInboundMailbox::new(conversation.clone());
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox: mailbox.clone(),
                workspace: Workspace::new(&workspace_root).expect("workspace"),
                artifacts: ArtifactStore::new(conversation, &artifacts).expect("artifacts"),
                clock: Arc::new(crate::runtime::SystemClock),
                event_sink: Some(sink_dyn),
            },
        );
        let fixture = TestRegistry {
            _dir: dir,
            registry,
            mailbox,
        };
        let executor: Arc<dyn ToolExecutor> = Arc::new(ProgressThenDone);
        let prepared = fixture
            .registry
            .prepare_dispatch(
                &background_invocation("bash"),
                &executor,
                ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = fixture
            .registry
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("the commit returns an outcome");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the oversized progress must not strand the execution"
        );
        let progress = terminal.progress.expect("progress snapshot");
        let message = progress.message.expect("message");
        assert!(
            message.len() <= crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES,
            "the snapshot message is bounded"
        );
        assert_eq!(progress.completed, Some(1.0));
        assert_eq!(progress.total, Some(2.0));
        let progress_events = sink
            .as_ref()
            .events()
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    crate::events::RuntimeEvent::ToolExecutionProgress { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(progress_events.len(), 1);
    }

    const TEST_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    async fn await_test_started(started: &mut watch::Receiver<bool>, description: &'static str) {
        tokio::time::timeout(
            TEST_LIVENESS_GUARD,
            started.wait_for(|is_started| *is_started),
        )
        .await
        .unwrap_or_else(|_| panic!("{description}: start wait exceeded liveness guard"))
        .expect("start channel stays open");
    }

    async fn wait_for_terminal(
        fixture: &TestRegistry,
        execution_id: &ToolExecutionId,
    ) -> super::BackgroundExecutionSnapshot {
        tokio::time::timeout(
            TEST_LIVENESS_GUARD,
            fixture.registry.wait_until_terminal(execution_id),
        )
        .await
        .expect("terminal wait exceeded liveness guard")
        .expect("execution record")
    }

    /// Waits (with a liveness guard only) until one execution reaches an
    /// exact non-terminal state, polling the authoritative snapshot. The
    /// proof is the exact state, never a sleep.
    async fn wait_for_state(
        fixture: &FileRegistry,
        execution_id: &ToolExecutionId,
        want: BackgroundLifecycle,
    ) -> super::BackgroundExecutionSnapshot {
        tokio::time::timeout(TEST_LIVENESS_GUARD, async {
            loop {
                if let Some(snapshot) = fixture.registry.snapshot(execution_id)
                    && snapshot.state == want
                {
                    return snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("state wait exceeded liveness guard")
    }

    /// The unused-reason guard: `BACKGROUND_CANCEL_REASON` is the
    /// conversation-owned cancellation reason.
    #[test]
    fn background_cancel_reason_is_user_requested() {
        assert_eq!(BACKGROUND_CANCEL_REASON, CancellationReason::UserRequested);
    }
}
