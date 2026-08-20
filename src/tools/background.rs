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
//! ConversationRuntime::activate()   (the shared lifecycle Inactive -> Running)
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
//!
//! # Terminal settlement ownership (Issue #63)
//!
//! Once a background executor has returned, the registry owns settlement
//! until exactly one stable outcome exists. Retaining the terminal candidate
//! is not enough for runtime-owned work — the runner task itself drives the
//! production settlement continuation, so no runtime-owned execution can
//! leave its production settlement path without terminal publication or an
//! explicit degraded outcome from its owning `ConversationRuntime`:
//!
//! ```text
//! executor returns
//!     -> finish(): durable publication attempt #1
//!         -> success: commit the terminal lifecycle
//!         -> failure: retain the candidate as PublishingTerminal
//!     -> registry-owned bounded retry (attempt #2, same deterministic
//!        `background-terminal:{execution_id}` correlation, exactly-once
//!        even when attempt #1 committed but observed an error)
//!         -> success: commit the terminal lifecycle
//!         -> failure: report the exhausted budget to the owning
//!           ConversationRuntime through the narrow
//!           [`BackgroundDurabilityFailureSink`] seam while the candidate
//!           stays retained and observable
//! ```
//!
//! A standalone registry that has never been claimed by a
//! `ConversationRuntime` has no durability-health owner; after its bounded
//! budget is exhausted it may therefore retain an observable
//! `PublishingTerminal` candidate without a runtime degradation sink.
//!
//! The budget is exactly two publication attempts driven synchronously by
//! the runner: no sleep, no hot loop, no process-global worker, and no
//! generic retry framework.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::durable::inbox::InboundDraft;
use crate::events::{
    BackgroundTerminalState, EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope,
    RuntimeEventSink,
};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::RuntimeClock;
use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
use crate::runtime::identity::{
    ConversationId, EventId, MessageId, ToolCallId, ToolExecutionId, ToolId,
};
use crate::runtime::inbound::ConversationInboundMailbox;
use crate::runtime::types::{CancellationReason, ConversationLifecycle, DurabilityGate};
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

/// The **default** cancellation reason of conversation-owned background
/// cancellation.
///
/// Direct control-path cancellation (`background_task(action = cancel)` or
/// [`ConversationBackgroundRegistry::cancel`]) is a user-requested control
/// action and therefore defaults to this reason. It is not the only possible
/// reason: runtime drain (M9c) requests cancellation of every owned execution
/// through [`ConversationBackgroundRegistry::cancel_with_reason`] with
/// [`CancellationReason::RuntimeShutdown`].
///
/// The authoritative cause store is the record's `cancel_reason`, committed
/// once at the `Starting|Running -> Cancelling` transition and never
/// rewritten, so the first winning reason is absorbing. The registry is the
/// settlement authority and canonicalizes the final terminal result from that
/// stored winner, so the registry winner and the stored result can never
/// disagree.
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
/// registry now owns durable terminal publication; it is never `Running`:
/// the executor has returned and the runner now owns the durable settlement
/// continuation, retaining the settlement candidate until publication
/// reaches a terminal outcome. An internal unpublished prepared
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

/// The narrow durability-failure seam of the background settlement owner
/// (Issue #63).
///
/// Once a background executor has returned, the registry owns settlement
/// until a terminal outcome exists: the runner performs the bounded
/// publication budget itself (the initial attempt inside `finish` plus
/// exactly one registry-owned retry under the same deterministic
/// correlation). When that bounded budget is exhausted, the unresolved
/// terminal candidate stays retained in the explicit non-terminal
/// [`BackgroundLifecycle::PublishingTerminal`] state and the registry
/// reports the failure through this seam, so the owning
/// `ConversationRuntime` enters its explicit durable-failure state. A
/// standalone never-claimed registry may retain the observable candidate
/// without this runtime-owned degraded outcome because no owner exists.
///
/// The sink is invoked by the runner **without** the registry lock held:
/// the runtime-side implementation acquires the coordinator lock, and the
/// lock graph already has a coordinator -> registry edge (the bootstrap
/// handshake), so holding the registry lock across this call could
/// deadlock.
///
/// This call is the runner's **last conversation-facing callback**, and it
/// is ordered *before* the registry publishes `publication_abandoned` (M9c):
/// runtime drain consumes the abandoned fact as settlement, so a drain that
/// observed it while this callback were still runnable could cache a failed
/// shutdown ahead of a real conversation callback. Implementations may
/// therefore assume the caller keeps a lifecycle settlement admission alive
/// across this call.
pub trait BackgroundDurabilityFailureSink: Send + Sync {
    /// The bounded terminal-publication budget of `execution_id` was
    /// exhausted; the terminal candidate remains retained by the registry
    /// in the non-terminal `PublishingTerminal` state.
    fn terminal_publication_failed(&self, execution_id: &ToolExecutionId, diagnostic: String);
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
    /// The durable background-ownership fact could not be committed, so the
    /// detached execution must not begin (Issue #12, M9a).
    ///
    /// The prepared dispatch rolls back completely: the runner is aborted
    /// before its start gate is released, no record is published, and no
    /// external side effect exists. A restart therefore never has to reason
    /// about an execution the durable authority never recorded.
    Durable {
        /// The bounded durable failure diagnostic.
        detail: String,
    },
    /// The owning conversation runtime's durable authority is in the
    /// explicit `DurabilityFailed` state (Issue #63): no new
    /// conversation-owned durable semantic ownership commit may begin until
    /// the runtime is reconstructed. The prepared dispatch rolls back
    /// completely — no record, no runner start, no durable fact.
    DurabilityFailed {
        /// The owning runtime's bounded failure diagnostic.
        detail: String,
    },
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
            Self::Durable { detail } => write!(
                f,
                "the durable background ownership fact could not be committed, so no detached execution was started: {detail}"
            ),
            Self::DurabilityFailed { detail } => write!(
                f,
                "the conversation runtime's durable authority has failed; no new background ownership may begin: {detail}"
            ),
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
    /// The conversation managed tool-output store for detached executors.
    pub tool_output: crate::tools::managed_output::ManagedToolOutput,
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
    /// Set when the runner spent its whole bounded terminal-publication
    /// budget without obtaining durable ownership (Issue #63). The runner
    /// has returned, so this record can no longer produce any external
    /// effect: it has reached its strongest available settlement while
    /// staying explicitly non-terminal, with the candidate retained. Runtime
    /// drain treats it as settled-with-failure evidence instead of waiting
    /// for a terminal state that can never arrive.
    publication_abandoned: bool,
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
    /// The narrow durability-failure seam of the owning conversation
    /// runtime (Issue #63), installed while the runtime is still inactive.
    /// It is invoked by the runner after the bounded terminal-publication
    /// budget is exhausted, never while the registry lock is held.
    failure_sink: Option<Arc<dyn BackgroundDurabilityFailureSink>>,
    /// The owning `ConversationRuntime`'s durability frontier (Issue #60):
    /// a new conversation-owned durable ownership commit must linearize
    /// against the runtime's `DurabilityFailed` commit on this shared gate.
    /// Installed by `ConversationRuntime::new` after the ownership
    /// transfer; a standalone registry has none and commits through the
    /// unbound-mailbox path.
    durability_gate: Option<Arc<DurabilityGate>>,
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
                failure_sink: None,
                durability_gate: None,
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

    /// Installs the durability-failure sink of the owning conversation
    /// runtime (Issue #63).
    ///
    /// `ConversationRuntime::new` installs it while the runtime is still
    /// inactive — and an inactive runtime refuses every background dispatch
    /// commit, so no execution record or runner exists yet and the
    /// installation can never race a settlement.
    pub(crate) fn install_failure_sink(&self, sink: Arc<dyn BackgroundDurabilityFailureSink>) {
        self.state().failure_sink = Some(sink);
    }

    /// Installs the owning runtime's durability frontier (Issue #60).
    ///
    /// A new conversation-owned background ownership commit must linearize
    /// against the runtime's `DurabilityFailed` commit on this shared gate.
    /// `ConversationRuntime::new` installs it after the ownership transfer;
    /// the runtime remains inactive until activation, so no dispatch commit
    /// can race the installation. A standalone registry never has one.
    pub(crate) fn install_durability_gate(&self, gate: Arc<DurabilityGate>) {
        self.state().durability_gate = Some(gate);
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
    /// (`Inactive -> Running`) is a later, distinct transition that every
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
        // Preparation is allowed during inactive composition, but a
        // draining/quiescent runtime cannot create another parked runner.
        // The counted guard remains held through insertion so drain can
        // either observe the prepared record or wait for this operation to
        // finish before aborting the private runner.
        let _admission = self
            .resources
            .mailbox
            .begin_preparation_admission()
            .map_err(|_| BackgroundDispatchError::ConversationInactive {
                conversation_id: self.conversation_id.clone(),
            })?;
        let mut state = self.state();
        let next = state
            .next_execution_sequence
            .checked_add(1)
            .ok_or(BackgroundDispatchError::SequenceExhausted)?;
        state.next_execution_sequence = next;
        let execution_id = ToolExecutionId::background(next);
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
                publication_abandoned: false,
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

    /// Aborts every prepared-but-uncommitted runner when conversation drain
    /// closes the ownership boundary. Prepared work has no durable ownership
    /// fact and its start gate has never been released, so aborting it cannot
    /// reclaim conversation-owned execution or discard a terminal fact.
    pub(crate) fn abort_prepared_for_drain(&self) {
        let mut state = self.state();
        for (_, prepared) in state.prepared.drain() {
            prepared.runner.abort();
        }
        drop(state);
        self.notify_state_change();
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
        // Hold the shared lifecycle admission guard across the prepared
        // registry transfer. This keeps a parked ownership attempt inside
        // the runtime's settlement accounting; the narrower lifecycle
        // commit boundary below gives drain and the actual durable ownership
        // fact a deterministic total order.
        let _admission = self
            .resources
            .mailbox
            .begin_running_admission()
            .map_err(|_| BackgroundDispatchError::ConversationInactive {
                conversation_id: self.conversation_id.clone(),
            })?;
        let mut state = self.state();
        // Runtime durability frontier (Issue #60): a new conversation-owned
        // durable ownership commit must linearize against the owning
        // runtime's `DurabilityFailed` commit on one synchronization
        // boundary. The permission guard is held across the durable
        // ownership write and the record publication below, so a failure
        // that wins the gate first rejects this dispatch (the prepared
        // runner is aborted while still parked behind its gate), and an
        // ownership that wins first is already durably owned before the
        // failure can be published. A standalone registry has no runtime
        // gate and commits through the unbound-mailbox path. The gate
        // handle is copied out of the registry state first: the guard
        // borrows the gate, never the registry state, so the ownership
        // commit below may still mutate the state while the guard is held.
        let durability_gate = state.durability_gate.clone();
        let ownership_permission = durability_gate
            .as_ref()
            .map(|gate| gate.enter_ownership_commit());
        if let Some(Err(refused)) = &ownership_permission {
            // Reject: roll the prepared dispatch back completely — the
            // runner is aborted while still parked behind its start gate —
            // and report the runtime-owned health diagnostic.
            if let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) {
                prepared_record.runner.abort();
            }
            prepared.committed = true;
            drop(state);
            self.notify_state_change();
            return Err(BackgroundDispatchError::DurabilityFailed {
                detail: refused.diagnostic.clone(),
            });
        }
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
        // The hook above parks before the final ownership boundary. The
        // lifecycle commit boundary below serializes the decisive lifecycle
        // read, durable ownership fact, record publication, and runner-gate
        // release against `Running -> Draining`: drain wins first and this
        // closure is refused; the closure wins first and drain follows it.
        let commit_result = self.resources.mailbox.with_running_commit(|| {
            if self.resources.mailbox.is_bound_inactive() {
                if let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) {
                    prepared_record.runner.abort();
                }
                prepared.committed = true;
                return Err(BackgroundDispatchError::ConversationInactive {
                    conversation_id: self.conversation_id.clone(),
                });
            }
            if attempt_cancellation.is_cancelled() {
                // The deciding cancellation observation and the rollback
                // share this critical section: the prepared record is
                // removed and the runner aborted here, and the prepared
                // handle's drop semantics are neutralized so no second
                // rollback path exists.
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
            // Issue #12 (M9a): the durable ownership fact commits **before**
            // the runner's start gate is released, so a detached external
            // side effect can never begin without durable evidence of its
            // `ToolExecutionId`, its owning tool call, and the fact that
            // ownership committed. On a durable failure the dispatch rolls
            // back completely — the runner is aborted while still parked
            // behind its gate — so the crash-recovery fold never has to
            // reason about an execution the store never saw.
            let ownership = background_ownership_event(
                &self.conversation_id,
                &prepared.execution_id,
                &prepared_record.record,
                self.resources.clock.now(),
            );
            if let Err(error) = self
                .resources
                .mailbox
                .commit_background_ownership(ownership)
            {
                prepared_record.runner.abort();
                prepared.committed = true;
                return Err(BackgroundDispatchError::Durable {
                    detail: error.to_string(),
                });
            }
            let result = accepted_result(&prepared.execution_id, &prepared_record.record.tool_name);
            let execution_id = prepared.execution_id.clone();
            let gate = prepared_record.gate.clone();
            let next_index = state.records.len();
            state.index.insert(execution_id.clone(), next_index);
            state.records.push(prepared_record.record);
            Self::observe_record(&state, next_index);
            prepared.committed = true;
            gate.notify_one();
            Ok(BackgroundDispatchOutcome::Accepted {
                execution_id,
                result,
            })
        });
        if commit_result.is_err() {
            // A drain that won before the final boundary leaves the prepared
            // record private. Roll it back before the counted admission is
            // released, so quiescence cannot become visible with a stale
            // prepared runner still owned by this dispatch.
            if let Some(prepared_record) = state.prepared.remove(&prepared.execution_id) {
                prepared_record.runner.abort();
            }
            prepared.committed = true;
        }
        // `ownership_permission` (the gate guard) drops with this scope,
        // after the record publication above: the DurabilityFailed commit
        // and this ownership commit have one total order on the gate.
        drop(state);
        self.notify_state_change();
        match commit_result {
            Err(_) => Err(BackgroundDispatchError::ConversationInactive {
                conversation_id: self.conversation_id.clone(),
            }),
            Ok(result) => result,
        }
    }

    /// Reseeds the deterministic `exec_N` allocator above every ordinal that
    /// already entered durable authority (Issue #12, M9a).
    ///
    /// The registry's execution counter is process-local, so a restart would
    /// otherwise mint `exec_1` a second time for a different logical
    /// execution. Startup recovery folds the durable
    /// `BackgroundExecutionCommitted` facts and installs the watermark here,
    /// while the runtime is still inactive and no dispatch can commit. The
    /// allocator only ever moves forward.
    ///
    /// # Panics
    ///
    /// Panics only if the registry lock is poisoned.
    pub(crate) fn restore_execution_sequence(&self, highest_durable_ordinal: u64) {
        let mut state = self.state();
        state.next_execution_sequence = state.next_execution_sequence.max(highest_durable_ordinal);
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
        self.cancel_with_reason(execution_id, BACKGROUND_CANCEL_REASON)
    }

    /// Requests cancellation with the cause owned by the caller. The first
    /// cancellation transition remains the absorbing registry winner; later
    /// requests are idempotent and cannot rewrite its cause.
    pub(crate) fn cancel_with_reason(
        &self,
        execution_id: &ToolExecutionId,
        reason: CancellationReason,
    ) -> Option<BackgroundExecutionSnapshot> {
        // Cancellation of an already-owned execution is a settlement/control
        // mutation during drain, but a stale caller must not mutate the
        // registry or publish an observation after quiescence. Standalone
        // registries have no lifecycle and retain their existing behavior.
        let Ok(_settlement) = self.resources.mailbox.begin_settlement_admission() else {
            return self.snapshot(execution_id);
        };
        let mut state = self.state();
        let index = *state.index.get(execution_id)?;
        {
            let record = &mut state.records[index];
            match record.lifecycle {
                BackgroundLifecycle::Starting | BackgroundLifecycle::Running => {
                    record.lifecycle = BackgroundLifecycle::Cancelling;
                    record.cancel_reason = Some(reason);
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

    /// The non-terminal (Starting/Running/Cancelling/PublishingTerminal)
    /// snapshots in execution allocation order. Terminal executions never
    /// appear here.
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
    ///
    /// This is durable publication attempt #1 of the production settlement
    /// continuation; [`ConversationBackgroundRegistry::settle_terminal`]
    /// drives the bounded retry and the explicit failure report.
    pub fn finish(&self, execution_id: &ToolExecutionId, result: &ToolExecutionResult) {
        // A committed runner may finish after the runtime has entered
        // `Draining`; this narrow guard keeps its durable terminal inbound,
        // observer callback, and terminal registry transition inside the
        // runtime's settlement boundary. A stale callback after quiescence is
        // refused before it can mutate anything.
        let Ok(_settlement) = self.resources.mailbox.begin_settlement_admission() else {
            return;
        };
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
                        ToolExecutionStatus::Denied { .. }
                        | ToolExecutionStatus::Failed { .. }
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
                    if matches!(
                        result.status,
                        ToolExecutionStatus::Denied { .. } | ToolExecutionStatus::Failed { .. }
                    ) {
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
        // cannot leave a false `Running` state after the executor has
        // returned.
        state.records[index].pending_terminal = Some(TerminalCandidate {
            settled,
            result: stored.clone(),
        });
        let notification = terminal_inbound_message(
            execution_id,
            &state.records[index].tool_name,
            settled,
            &stored,
            self.resources.clock.now(),
        );
        // The background terminal notification uses the same durable
        // acceptance owner as every other inbound producer (Issue #63), with
        // a deterministic producer correlation so a retry with the same
        // committed correlation can never publish a duplicate notification.
        // Durable acceptance commits **before** the terminal lifecycle.
        let correlation = format!("background-terminal:{}", execution_id.as_str());
        let event_id = EventId::new(format!("background-terminal-event:{execution_id}"));
        let event = background_terminal_event(
            &self.conversation_id,
            &event_id,
            &notification,
            execution_id,
            settled,
        );
        let draft = inbound_draft(notification, correlation);
        match self.resources.mailbox.accept_draft_with_event(draft, event) {
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
                // `Running` after the executor has returned; the runner now
                // continues only as the registry-owned settlement
                // continuation, which performs the bounded retry immediately
                // after this call returns.
                let record = &mut state.records[index];
                record.lifecycle = BackgroundLifecycle::PublishingTerminal;
                record.notification = NotificationState::Failed;
            }
        }
        Self::observe_record(&state, index);
        drop(state);
        self.notify_state_change();
    }

    /// The production settlement continuation of one returned executor
    /// (Issue #63): the runner drives the bounded terminal-publication
    /// budget itself, so no runtime-owned execution can leave its settlement
    /// path without terminal publication or an explicit degraded outcome.
    /// A standalone never-claimed registry may retain the candidate in
    /// `PublishingTerminal` after the bounded budget is exhausted because it
    /// has no owning `ConversationRuntime` to degrade.
    ///
    /// Attempt #1 runs inside [`ConversationBackgroundRegistry::finish`];
    /// exactly one registry-owned retry follows under the same
    /// deterministic correlation (`background-terminal:{execution_id}`),
    /// which resolves exactly-once even when attempt #1 committed durably
    /// but observed an error. When the bounded budget is exhausted, the
    /// terminal candidate remains retained in the explicit non-terminal
    /// `PublishingTerminal` state and the failure is reported to the
    /// owning conversation runtime through the installed
    /// [`BackgroundDurabilityFailureSink`]. There is no sleep, no hot loop,
    /// and no further attempt after the budget is spent.
    ///
    /// The failure report commits **before** the abandoned settlement fact
    /// (M9c): the sink callback is real semantic runtime work, so
    /// `publication_abandoned` — the fact runtime drain consumes as this
    /// owner's settlement — becomes observable only once that callback has
    /// returned and no conversation callback authority remains.
    fn settle_terminal(&self, execution_id: &ToolExecutionId, result: &ToolExecutionResult) {
        self.finish(execution_id, result);
        let Some(snapshot) = self.retry_terminal_publication(execution_id) else {
            return;
        };
        if snapshot.state != BackgroundLifecycle::PublishingTerminal {
            return;
        }
        // The bounded publication budget is exhausted. This record has
        // reached its strongest available settlement: it will be able to act
        // no further, but it is explicitly not terminal.
        //
        // M9c settlement linearization. Reporting the failure is itself a
        // real conversation-facing callback of this runner: the sink upgrades
        // the owning `ConversationRuntime`, takes the coordinator lock,
        // mutates durability health and may publish a `DurabilityFailed`
        // observation. `publication_abandoned` is the settlement fact runtime
        // drain consumes (`unsettled_snapshot` / `wait_until_settled`), so it
        // must never become observable while that callback can still run —
        // otherwise drain could stop waiting on this owner, aggregate the
        // abandoned evidence and cache a failed shutdown *before* the runner
        // finished calling back into the conversation. The failure report
        // therefore commits before the abandoned fact:
        //
        //   publication retries exhausted
        //     -> failure sink callback begins
        //     -> failure sink callback completes
        //     -> publication_abandoned commit
        //     -> waiters notified
        //     -> zero remaining conversation callback authority
        //
        // The whole continuation runs under one settlement admission that is
        // released only after the abandoned fact is published, so neither
        // `mark_quiescent` (successful shutdown) nor `wait_for_no_admissions`
        // (failed shutdown, which leaves the lifecycle `Draining` and cannot
        // rely on admission refusal) can complete while the callback is live.
        let Ok(_settlement) = self.resources.mailbox.begin_settlement_admission() else {
            // Settlement is refused only after `Quiescent`, which drain
            // cannot publish while this record is still an unsettled
            // `PublishingTerminal` owner. Nothing may call back into the
            // conversation from here.
            return;
        };
        // The sink acquires the coordinator lock and the lock graph already
        // has a coordinator -> registry edge (the bootstrap handshake), so
        // the sink is invoked only after every registry lock acquisition
        // above has been released — never while the registry lock is held.
        let sink = self.state().failure_sink.clone();
        if let Some(sink) = sink {
            sink.terminal_publication_failed(
                execution_id,
                format!(
                    "the durable terminal publication of background execution {execution_id} failed persistently"
                ),
            );
        }
        // The last conversation-facing callback of this runner has returned.
        // Publishing the abandoned fact is what lets runtime drain stop
        // waiting on *this* record without abandoning any other owner; from
        // here the execution owns no remaining callback authority.
        self.mark_publication_abandoned(execution_id);
    }

    /// The bounded retry of the durable terminal publication of one
    /// execution that is in [`BackgroundLifecycle::PublishingTerminal`],
    /// using the retained terminal candidate and the stable correlation.
    ///
    /// This is attempt #2 of the runner-owned settlement continuation
    /// ([`ConversationBackgroundRegistry::settle_terminal`]), never an
    /// external polling API: a `PublishingTerminal` record always retains
    /// its candidate and reaches a terminal outcome through this seam
    /// without duplicating the terminal inbound (the correlation is
    /// exactly-once). For an already-terminal record it is an idempotent
    /// no-op returning the current snapshot.
    #[must_use]
    fn retry_terminal_publication(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        let Ok(_settlement) = self.resources.mailbox.begin_settlement_admission() else {
            return self.snapshot(execution_id);
        };
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
            &candidate.result,
            self.resources.clock.now(),
        );
        let correlation = format!("background-terminal:{}", execution_id.as_str());
        let event_id = EventId::new(format!("background-terminal-event:{execution_id}"));
        let event = background_terminal_event(
            &self.conversation_id,
            &event_id,
            &notification,
            execution_id,
            candidate.settled,
        );
        let draft = inbound_draft(notification, correlation);
        match self.resources.mailbox.accept_draft_with_event(draft, event) {
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

    /// Records that one execution spent its whole bounded terminal-publication
    /// budget and completed its last conversation-facing failure callback;
    /// only the durable terminal fact is missing.
    ///
    /// This is the settlement commit of the runner's continuation: it is
    /// invoked only after [`BackgroundDurabilityFailureSink::terminal_publication_failed`]
    /// has returned, so once `publication_abandoned` is observable the
    /// execution owns no remaining conversation callback authority — no
    /// failure-sink callback, no observer callback, no Pending Inbound
    /// attempt, no durability-health mutation, no terminal retry and no
    /// semantic registry mutation can follow it.
    fn mark_publication_abandoned(&self, execution_id: &ToolExecutionId) {
        {
            let mut state = self.state();
            let Some(index) = state.index.get(execution_id).copied() else {
                return;
            };
            if state.records[index].lifecycle != BackgroundLifecycle::PublishingTerminal {
                return;
            }
            state.records[index].publication_abandoned = true;
        }
        self.notify_state_change();
    }

    /// The active executions that runtime drain must still supervise.
    ///
    /// A record whose durable terminal publication was abandoned is excluded:
    /// the abandoned fact is published only after the runner exhausted
    /// durable terminal publication *and* completed every remaining
    /// conversation-facing failure callback, so it owns no callback authority
    /// and can produce no further external effect — re-cancelling and
    /// re-awaiting it would spin forever. It remains explicit non-terminal
    /// evidence through
    /// [`ConversationBackgroundRegistry::abandoned_publications`].
    #[must_use]
    pub(crate) fn unsettled_snapshot(&self) -> Vec<BackgroundExecutionSnapshot> {
        let state = self.state();
        state
            .records
            .iter()
            .filter(|record| record.lifecycle.is_active() && !record.publication_abandoned)
            .map(snapshot_of)
            .collect()
    }

    /// The executions whose durable terminal publication was abandoned, in
    /// allocation order. Each one is settlement evidence that prevents the
    /// owning runtime from claiming successful quiescence.
    #[must_use]
    pub(crate) fn abandoned_publications(&self) -> Vec<ToolExecutionId> {
        let state = self.state();
        state
            .records
            .iter()
            .filter(|record| record.publication_abandoned)
            .map(|record| record.execution_id.clone())
            .collect()
    }

    /// Waits until one execution reaches its strongest available settlement:
    /// a terminal lifecycle, or an explicitly abandoned durable terminal
    /// publication. Either fact implies the execution owns no remaining
    /// conversation callback authority — the abandoned fact is committed only
    /// after the runner's last conversation-facing failure callback returned.
    ///
    /// Unlike a terminal-only wait this can never strand runtime drain, and
    /// unlike a global durability-health check it never reports one record's
    /// failure as another record's settlement.
    pub(crate) async fn wait_until_settled(&self, execution_id: &ToolExecutionId) {
        let mut version = self.state_version.subscribe();
        loop {
            {
                let state = self.state();
                let Some(index) = state.index.get(execution_id).copied() else {
                    return;
                };
                let record = &state.records[index];
                if record.lifecycle.is_terminal() || record.publication_abandoned {
                    return;
                }
            }
            if version.changed().await.is_err() {
                return;
            }
        }
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
        let Ok(_settlement) = self.resources.mailbox.begin_settlement_admission() else {
            return;
        };
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

    /// Waits for one execution to enter cancellation settlement using the
    /// registry's state-change notification. This narrow seam is used by
    /// runtime supervision tests to distinguish cancellation intent from the
    /// later terminal transition without polling.
    #[cfg(test)]
    pub(crate) async fn wait_until_cancelling(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        let mut version = self.state_version.subscribe();
        loop {
            if let Some(snapshot) = self.snapshot(execution_id)
                && snapshot.state == BackgroundLifecycle::Cancelling
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
            // The record — not a start-time copy — is the absorbing cause
            // authority of this execution. The runner starts before any
            // cancellation exists, so the executor must read the winner
            // when it observes cancellation.
            let cause = Arc::new(BackgroundCancellationCause {
                registry: registry.clone(),
                execution_id: execution_id.clone(),
            });
            let context = ToolExecutionContext {
                conversation_id: &registry.conversation_id,
                execution_id: Some(&execution_id),
                cancellation: ExecutionCancellation::new(cancellation.clone(), cause),
                workspace: &resources.workspace,
                progress: &reporter,
                artifacts: &resources.artifacts,
                tool_output: &resources.tool_output,
                environment: &environment,
            };
            let result = executor.execute(invocation, context).await;
            registry.settle_terminal(&execution_id, &result);
        })
    }
}

/// The conversation background registry record is the absorbing
/// cancellation-cause authority of one detached execution.
///
/// The registry commits the cause at the one `Starting|Running -> Cancelling`
/// transition and never rewrites it, so this view is a live read of that one
/// store. It is deliberately not a second cause store: an execution that has
/// not been cancelled reports the conversation-owned default.
struct BackgroundCancellationCause {
    registry: ConversationBackgroundRegistry,
    execution_id: ToolExecutionId,
}

impl crate::runtime::cancellation::CancellationCause for BackgroundCancellationCause {
    fn cause(&self) -> CancellationReason {
        let state = self.registry.state();
        state
            .index
            .get(&self.execution_id)
            .and_then(|index| state.records[*index].cancel_reason)
            .unwrap_or(BACKGROUND_CANCEL_REASON)
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

/// The timestamped terminal inbound message of one settlement.
///
/// The message carries a compact deterministic terminal summary plus a
/// **bounded** model-visible textual projection of the terminal result
/// (see [`terminal_result_projection`]): the model receives the bounded
/// result — including the advisory absolute spill locator and the
/// Read/Grep continuation instruction when the result spilled — inside
/// ordinary canonical text. Full oversized output is never dumped into the
/// inbound message: the bounded canonical text remains replayable even if
/// the auxiliary spill file later disappears, and detailed inspection
/// remains `background_task(status)`. Genuine semantic artifact
/// references publish as their own `UserContentBlock::File` blocks; a
/// textual result never becomes a File block.
fn terminal_inbound_message(
    execution_id: &ToolExecutionId,
    tool_name: &str,
    state: BackgroundLifecycle,
    result: &ToolExecutionResult,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> UserMessageBlock {
    let mut text = format!(
        "Background execution {} ({tool_name}) settled: {}",
        execution_id.as_str(),
        state.name()
    );
    if let Some(projection) = terminal_result_projection(result) {
        text.push_str("\n\nResult:\n");
        text.push_str(&projection);
    }
    let mut content = vec![UserContentBlock::Text(TextBlock { text })];
    for artifact in &result.artifacts {
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

/// The deterministic bounded textual projection of one terminal tool
/// result for the canonical background terminal inbound message.
///
/// Text blocks publish verbatim, JSON blocks publish compactly serialized
/// (the same model-facing representation a provider adapter produces), and
/// file/image content blocks publish as a short textual mention — genuine
/// semantic artifacts publish separately as `UserContentBlock::File`
/// blocks, so the textual projection stays text-only. The projection of a
/// spilled result contains the exact absolute `full_output` locator and
/// the Read/Grep continuation instruction from the bounded result itself.
///
/// The projection becomes canonical conversation state, so it is bounded
/// as one payload to [`MAX_MODEL_TOOL_RESULT_BYTES`](crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES)
/// with an explicit truncation marker: an unbounded executor result can
/// never dump unbounded text into canonical inbound history. A `None`
/// result (no status detail and no content) projects to nothing.
fn terminal_result_projection(result: &ToolExecutionResult) -> Option<String> {
    /// The explicit marker appended when the projection crosses its bound.
    const PROJECTION_TRUNCATED_MARKER: &str = "\n...[terminal result projection truncated]";
    let mut parts: Vec<String> = Vec::new();
    match &result.status {
        ToolExecutionStatus::Failed { error } => parts.push(format!("Error: {error}")),
        ToolExecutionStatus::Denied { reason } => parts.push(format!("Denied: {reason}")),
        ToolExecutionStatus::Success
        | ToolExecutionStatus::Cancelled { .. }
        | ToolExecutionStatus::TimedOut
        | ToolExecutionStatus::Interrupted => {}
    }
    for block in &result.content {
        parts.push(match block {
            ToolResultContent::Text(text) => text.text.clone(),
            ToolResultContent::Json { value } => serde_json::to_string(value)
                .unwrap_or_else(|_| "<unserializable JSON result>".to_owned()),
            ToolResultContent::File(reference) => format!(
                "[file artifact: {}]",
                reference
                    .name
                    .clone()
                    .unwrap_or_else(|| reference.artifact_id.as_str().to_owned())
            ),
            ToolResultContent::Image(_) => "[image content]".to_owned(),
        });
    }
    if parts.is_empty() {
        return None;
    }
    let bound = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES;
    let mut projection = String::new();
    for part in parts {
        if !projection.is_empty() {
            projection.push('\n');
        }
        if projection.len() + part.len() + PROJECTION_TRUNCATED_MARKER.len() > bound {
            let budget = bound
                .saturating_sub(projection.len())
                .saturating_sub(PROJECTION_TRUNCATED_MARKER.len());
            projection.push_str(&crate::tools::limits::bound_utf8_text(part, budget));
            projection.push_str(PROJECTION_TRUNCATED_MARKER);
            return Some(projection);
        }
        projection.push_str(&part);
    }
    Some(projection)
}

fn inbound_draft(notification: UserMessageBlock, correlation: String) -> InboundDraft {
    let timestamp = notification
        .timestamp
        .expect("background terminal notifications carry a timestamp");
    InboundDraft {
        message_id: Some(notification.id.clone()),
        source: notification.source,
        kind: notification.kind,
        content: notification.content,
        timestamp,
        correlation: Some(correlation),
    }
}

fn background_terminal_event(
    conversation_id: &ConversationId,
    event_id: &EventId,
    notification: &UserMessageBlock,
    execution_id: &ToolExecutionId,
    state: BackgroundLifecycle,
) -> RuntimeEventEnvelope {
    background_publication_event(
        conversation_id,
        event_id,
        notification,
        execution_id,
        match state {
            BackgroundLifecycle::Succeeded => BackgroundTerminalState::Succeeded,
            BackgroundLifecycle::Failed => BackgroundTerminalState::Failed,
            BackgroundLifecycle::Cancelled => BackgroundTerminalState::Cancelled,
            BackgroundLifecycle::Starting
            | BackgroundLifecycle::Running
            | BackgroundLifecycle::Cancelling
            | BackgroundLifecycle::PublishingTerminal => {
                unreachable!("only terminal background states are published")
            }
        },
    )
}

fn background_publication_event(
    conversation_id: &ConversationId,
    event_id: &EventId,
    notification: &UserMessageBlock,
    execution_id: &ToolExecutionId,
    state: BackgroundTerminalState,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: event_id.clone(),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: notification
            .timestamp
            .expect("background terminal notifications carry a timestamp"),
        event: RuntimeEvent::BackgroundTerminalPublished {
            execution_id: execution_id.clone(),
            message_id: notification.id.clone(),
            state,
        },
    }
}

/// The durable ownership fact of one detached execution (Issue #12, M9a).
///
/// The fact carries exactly the identity a restart needs to answer "which
/// `ToolExecutionId` existed, for which `ToolCall`/tool, and was ownership
/// committed?" — never the invocation arguments, the environment, or any
/// executor state, all of which are process-local runtime state.
///
/// The envelope carries no attempt identity: a committed background execution
/// deliberately outlives the attempt that dispatched it, so binding the fact
/// to that attempt's durable lifecycle would make the attempt's own terminal
/// contradict a still-open background execution.
fn background_ownership_event(
    conversation_id: &ConversationId,
    execution_id: &ToolExecutionId,
    record: &BackgroundRecord,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(format!("background-committed-event:{execution_id}")),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::BackgroundExecutionCommitted {
            execution_id: execution_id.clone(),
            tool_call_id: record.tool_call_id.clone(),
            tool_id: record.tool_id.clone(),
            tool_name: record.tool_name.clone(),
        },
    }
}

/// The recovery-generated terminal publication of one detached execution that
/// was durably owned but never settled before the process restarted (Issue
/// #12, M9a).
///
/// The identity contract is deliberately **identical** to the live settlement
/// path — the same `MessageId` and the same producer correlation — so a live
/// publication and a recovery publication are mutually exclusive by
/// construction: whichever commits first owns the one notification, and the
/// other is either refused by the durable `background:{execution_id}`
/// lifecycle terminal or resolved as an idempotent correlation retry.
///
/// The published state is [`BackgroundTerminalState::Interrupted`], never
/// `Failed`: the old task/process did not survive the restart and its actual
/// external outcome is unknown. Nothing is relaunched.
pub(crate) fn recovery_terminal_publication(
    conversation_id: &ConversationId,
    execution_id: &ToolExecutionId,
    tool_name: &str,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    let notification = UserMessageBlock {
        id: MessageId::new(format!("background-{}-terminal", execution_id.as_str())),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Background execution {} ({tool_name}) was interrupted by a runtime restart: \
                 its actual outcome is unknown and it was not restarted.",
                execution_id.as_str()
            ),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    };
    let event = background_publication_event(
        conversation_id,
        &EventId::new(format!("background-terminal-event:{execution_id}")),
        &notification,
        execution_id,
        BackgroundTerminalState::Interrupted,
    );
    let correlation = format!("background-terminal:{}", execution_id.as_str());
    (inbound_draft(notification, correlation), event)
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
    use crate::durable::inbox::ConversationStore;
    use crate::events::{RecordingEventSink, RuntimeEvent};
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
                artifacts: ArtifactStore::new(conversation.clone(), &artifacts).expect("artifacts"),
                tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                    conversation,
                    artifacts.join("tool-output"),
                )
                .expect("managed tool output"),
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
        store: Arc<crate::durable::SqliteConversationStore>,
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
            crate::durable::SqliteConversationStore::open(conversation.clone(), &store_path)
                .expect("store"),
        );
        let mailbox = ConversationInboundMailbox::over_store(store.clone());
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox,
                workspace: Workspace::new(&workspace_root).expect("workspace"),
                artifacts: ArtifactStore::new(conversation.clone(), &artifacts).expect("artifacts"),
                tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                    conversation,
                    artifacts.join("tool-output"),
                )
                .expect("managed tool output"),
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

    /// M9c: the runtime drain wins before the background ownership boundary.
    /// The deterministic hook parks the real registry commit after its first
    /// lifecycle observation; drain then closes the shared lifecycle, so the
    /// second observation rolls the prepared dispatch back and never releases
    /// the runner's start gate.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_drain_wins_background_ownership_boundary() {
        let fixture = registry("conv-bg-drain-first");
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        fixture.mailbox.bind_inactive(&lifecycle);
        assert!(lifecycle.activate(), "the runtime lifecycle is running");

        let (executor, started, _release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let hook = Arc::new(CommitBoundaryHook::default());
        fixture.registry.install_commit_boundary_hook(hook.clone());

        let registry = fixture.registry.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            registry.commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
        });
        tokio::task::spawn_blocking({
            let hook = hook.clone();
            move || hook.wait_entered()
        })
        .await
        .expect("commit boundary waiter");

        assert!(lifecycle.begin_drain(), "drain wins the boundary race");
        hook.proceed();
        let result = commit_task
            .await
            .expect("commit task")
            .expect_err("drain refuses the new ownership commit");
        assert_eq!(
            result,
            super::BackgroundDispatchError::ConversationInactive {
                conversation_id: ConversationId::new("conv-bg-drain-first"),
            }
        );
        lifecycle.wait_for_no_admissions().await;
        assert!(fixture.registry.all_snapshots().is_empty());
        assert!(
            !*started.borrow(),
            "the runner never crossed its start gate"
        );
        assert_eq!(
            lifecycle.state(),
            crate::runtime::types::ConversationLifecycleState::Draining
        );
    }

    /// M9c: the background ownership boundary wins before drain. The commit
    /// returns its durable ownership result first; drain then treats the
    /// record as conversation-owned, requests runtime cancellation, and
    /// waits for the existing registry terminal/publication state machine.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_ownership_boundary_wins_runtime_drain() {
        let fixture = registry("conv-bg-ownership-first");
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        fixture.mailbox.bind_inactive(&lifecycle);
        assert!(lifecycle.activate(), "the runtime lifecycle is running");

        let (executor, mut started, release) = IgnoreCancellationExecutor::new(success());
        let executor: Arc<dyn ToolExecutor> = Arc::new(executor);
        let prepared = prepare(&fixture, &executor);
        let hook = Arc::new(CommitBoundaryHook::default());
        fixture.registry.install_commit_boundary_hook(hook.clone());

        let registry = fixture.registry.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            registry
                .commit_dispatch(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .expect("ownership commit")
        });
        let boundary = tokio::task::spawn_blocking({
            let hook = hook.clone();
            move || {
                hook.wait_entered();
                hook.proceed();
            }
        });
        boundary.await.expect("commit boundary waiter");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            commit_task.await.expect("commit task")
        else {
            panic!("ownership commit wins before drain");
        };

        assert!(
            lifecycle.begin_drain(),
            "drain follows the ownership commit"
        );
        await_test_started(&mut started, "owned background runner starts").await;
        let cancelling = fixture
            .registry
            .cancel_with_reason(&execution_id, CancellationReason::RuntimeShutdown)
            .expect("owned execution remains cancellable during drain");
        assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
        release.send_replace(true);
        let terminal = wait_for_terminal(&fixture, &execution_id).await;
        assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);
        lifecycle.wait_for_no_admissions().await;
        assert_eq!(
            fixture
                .mailbox
                .select_pending_batch()
                .expect("select")
                .expect("terminal inbound")
                .items()
                .len(),
            1,
            "terminal publication remains durably pending after drain closes adoption"
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

    /// A test observer collecting every published snapshot in order, so a
    /// regression can prove the exact transition sequence (including the
    /// intermediate non-terminal publication-pending state).
    #[derive(Default)]
    struct CollectingObserver {
        snapshots: std::sync::Mutex<Vec<super::BackgroundExecutionSnapshot>>,
    }

    impl super::BackgroundObserver for CollectingObserver {
        fn on_snapshot(&self, snapshot: &super::BackgroundExecutionSnapshot) {
            self.snapshots
                .lock()
                .expect("observer lock")
                .push(snapshot.clone());
        }
    }

    /// A test durability-failure sink recording every exhausted-budget
    /// report, so a regression can await the production degradation signal
    /// deterministically.
    #[derive(Default)]
    struct RecordingFailureSink {
        calls: std::sync::Mutex<Vec<(ToolExecutionId, String)>>,
        notify: tokio::sync::Notify,
    }

    impl super::BackgroundDurabilityFailureSink for RecordingFailureSink {
        fn terminal_publication_failed(&self, execution_id: &ToolExecutionId, diagnostic: String) {
            self.calls
                .lock()
                .expect("sink lock")
                .push((execution_id.clone(), diagnostic));
            self.notify.notify_one();
        }
    }

    /// Issue #63 (Blocker 2, Tests 1+2): when the real runner's first
    /// durable terminal-inbound publication fails, the **production**
    /// settlement continuation — never a test-only manual retry — performs
    /// the bounded registry-owned retry: the record passes through the
    /// explicit `PublishingTerminal` state and reaches its terminal state
    /// exactly once, with exactly one durable terminal inbound under the
    /// stable deterministic correlation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // One settlement path, asserted end to end.
    async fn first_publication_failure_is_retried_by_the_production_runner() {
        let fixture = file_registry("conv-bg-retry");
        let observer = Arc::new(CollectingObserver::default());
        fixture
            .registry
            .install_observer_and_snapshots(observer.clone());
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

        // Arm exactly one acceptance fault, then release the real runner:
        // publication attempt #1 fails durably, and the runner's own
        // bounded retry (attempt #2) must finalize the settlement.
        fixture.store.arm_fail_accept_times(1);
        release.send_replace(true);

        // The production path itself advances the record to terminal; no
        // test code drives the retry.
        let terminal = tokio::time::timeout(
            TEST_LIVENESS_GUARD,
            fixture.registry.wait_until_terminal(&execution_id),
        )
        .await
        .expect("terminal wait exceeded liveness guard")
        .expect("execution record");
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        assert!(terminal.result.is_some(), "the terminal result is stored");

        // The transition sequence proves attempt #1 failed into the
        // explicit publication-pending state and the production retry
        // committed the terminal lifecycle.
        let states: Vec<BackgroundLifecycle> = observer
            .snapshots
            .lock()
            .expect("observer lock")
            .iter()
            .map(|snapshot| snapshot.state)
            .collect();
        let publishing = states
            .iter()
            .filter(|state| **state == BackgroundLifecycle::PublishingTerminal)
            .count();
        assert_eq!(
            publishing, 1,
            "exactly one failed publication attempt entered PublishingTerminal: {states:?}"
        );
        let succeeded = states
            .iter()
            .filter(|state| **state == BackgroundLifecycle::Succeeded)
            .count();
        assert_eq!(succeeded, 1, "terminal exactly once: {states:?}");
        let publishing_at = states
            .iter()
            .position(|state| *state == BackgroundLifecycle::PublishingTerminal)
            .expect("the publication-pending state was observed");
        let succeeded_at = states
            .iter()
            .position(|state| *state == BackgroundLifecycle::Succeeded)
            .expect("the terminal state was observed");
        assert!(
            publishing_at < succeeded_at,
            "the retry commits the terminal lifecycle after the failed attempt: {states:?}"
        );

        // Durable terminal inbound exactly once, under the stable
        // correlation: the retry resolved the same acceptance.
        let items = fixture.store.load_pending().expect("load");
        assert_eq!(items.len(), 1, "exactly one durable terminal inbound");
        let expected_correlation = format!("background-terminal:{}", execution_id.as_str());
        assert_eq!(
            items[0].correlation.as_deref(),
            Some(expected_correlation.as_str()),
            "the bounded retry used the same deterministic correlation"
        );
        let events = fixture.store.read_events(None, 10).expect("events").events;
        // The durable background lifecycle is exactly two facts: the
        // ownership commit that preceded the external side effect (Issue #12,
        // M9a) and the one terminal publication that closes it.
        assert!(
            matches!(
                &events[0].event,
                RuntimeEvent::BackgroundExecutionCommitted {
                    execution_id: event_execution,
                    ..
                } if event_execution == &execution_id
            ),
            "the ownership commit precedes the execution: {:?}",
            events[0].event
        );
        let published: Vec<&RuntimeEvent> = events
            .iter()
            .map(|envelope| &envelope.event)
            .filter(|event| matches!(event, RuntimeEvent::BackgroundTerminalPublished { .. }))
            .collect();
        assert_eq!(
            published.len(),
            1,
            "terminal publication fact is exactly once"
        );
        assert!(matches!(
            published[0],
            RuntimeEvent::BackgroundTerminalPublished {
                execution_id: event_execution,
                message_id,
                ..
            } if event_execution == &execution_id && message_id == &items[0].message_id
        ));
    }

    /// Issue #63 (Blocker 2, Test 3): when the bounded publication budget
    /// (attempt #1 in `finish` plus the one registry-owned retry) is
    /// exhausted, the terminal candidate remains retained in the explicit
    /// non-terminal `PublishingTerminal` state, no false terminal
    /// publication exists, and the registry reports the failure through the
    /// narrow durability-failure seam exactly once — no hot loop and no
    /// further publication attempts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exhausted_publication_budget_retains_candidate_and_reports_failure() {
        let fixture = file_registry("conv-bg-fault");
        let observer = Arc::new(CollectingObserver::default());
        fixture
            .registry
            .install_observer_and_snapshots(observer.clone());
        let sink = Arc::new(RecordingFailureSink::default());
        fixture
            .registry
            .install_failure_sink(sink.clone() as Arc<dyn super::BackgroundDurabilityFailureSink>);
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

        // Arm exactly two acceptance faults: the full bounded publication
        // budget of the production settlement continuation.
        fixture.store.arm_fail_accept_times(2);
        release.send_replace(true);

        // The runner reports the exhausted budget through the failure seam
        // after attempt #2 — deterministically, no polling.
        tokio::time::timeout(TEST_LIVENESS_GUARD, sink.notify.notified())
            .await
            .expect("the exhausted budget report exceeded the liveness guard");

        // The candidate remains retained in the explicit non-terminal
        // state; no false terminal publication exists.
        let snapshot = fixture.registry.snapshot(&execution_id).expect("record");
        assert_eq!(snapshot.state, BackgroundLifecycle::PublishingTerminal);
        assert_ne!(
            snapshot.state,
            BackgroundLifecycle::Running,
            "the record must not fake Running after the executor has returned"
        );
        let result = snapshot
            .result
            .expect("the retained terminal candidate is not lost");
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert!(
            fixture.store.load_pending().expect("load").is_empty(),
            "no durable pending record committed"
        );
        assert!(
            fixture.store.load_canonical().expect("load").is_empty(),
            "no durable canonical record committed"
        );

        // Bounded: exactly two publication attempts (two `PublishingTerminal`
        // transitions) and exactly one exhaustion report — the sink fires
        // after the last attempt, so no further attempt can still be in
        // flight; there is no hot loop.
        let states: Vec<BackgroundLifecycle> = observer
            .snapshots
            .lock()
            .expect("observer lock")
            .iter()
            .map(|snapshot| snapshot.state)
            .collect();
        let publishing = states
            .iter()
            .filter(|state| **state == BackgroundLifecycle::PublishingTerminal)
            .count();
        assert_eq!(
            publishing, 2,
            "exactly the bounded budget of two publication attempts ran: {states:?}"
        );
        assert!(
            !states.iter().any(|state| state.is_terminal()),
            "no false terminal publication: {states:?}"
        );
        let calls = sink.calls.lock().expect("sink lock").len();
        assert_eq!(calls, 1, "the exhaustion was reported exactly once");
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
        let reopened = crate::durable::SqliteConversationStore::open(
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
                artifacts: ArtifactStore::new(conversation.clone(), &artifacts).expect("artifacts"),
                tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                    conversation,
                    artifacts.join("tool-output"),
                )
                .expect("managed tool output"),
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

    /// The unused-reason guard: `BACKGROUND_CANCEL_REASON` is the
    /// conversation-owned cancellation reason.
    #[test]
    fn background_cancel_reason_is_user_requested() {
        assert_eq!(BACKGROUND_CANCEL_REASON, CancellationReason::UserRequested);
    }
}
