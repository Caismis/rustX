//! Bounded background-execution vocabulary shared by the registry, the Agent
//! Status projection, and the journal: the lifecycle enum and the read-only
//! execution snapshot.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{ToolExecutionId, ToolId};
use crate::tools::types::{BashPresentation, ToolExecutionResult, ToolProgress};

/// The public lifecycle of one background execution.
///
/// Terminal states are absorbing. The allowed public transitions are:
///
/// ```text
/// Starting  → Running
/// Starting  → Cancelling
/// Starting  → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
/// Running   → Cancelling
/// Running   → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
/// Starting / Running / Cancelling → PublishingTerminal
/// Cancelling → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
/// PublishingTerminal → Succeeded / Failed / Denied / Cancelled / TimedOut / OutcomeUnknown
/// ```
///
/// The terminal vocabulary claims only facts the executor proved: `Failed`
/// is a *known* failure, `Denied` is policy/approval refusal (not `Failed`),
/// `TimedOut` is a proven deadline settlement, and
/// `OutcomeUnknown` means the executor could not prove the external outcome
/// at all. A terminal lifecycle never claims a stronger external outcome
/// than the canonical `ToolExecutionStatus` of the stored result proves —
/// in particular an unknown outcome is never labelled `Failed`, and a
/// cancellation request that could not be confirmed physically never
/// becomes `Cancelled`.
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
#[derive(schemars::JsonSchema)]
pub enum BackgroundLifecycle {
    /// The dispatch committed and the runner is starting.
    Starting,
    /// The runner is executing.
    Running,
    /// Cancellation intent committed and the cancellation was requested.
    /// This is non-terminal: terminal certainty is still pending and the
    /// executor's returned settlement decides the terminal state.
    Cancelling,
    /// The executor returned its terminal candidate; the registry owns
    /// durable terminal publication, which has not committed yet.
    PublishingTerminal,
    /// The execution succeeded.
    Succeeded,
    /// The execution reached a known failure: the executor proved a failed
    /// terminal outcome. `Failed` never means "anything that is not success
    /// or cancelled" — an unproven outcome is [`Self::OutcomeUnknown`].
    Failed,
    /// Invocation was refused by policy or approval authority.
    Denied,
    /// The executor proved physical cancellation settlement; the canonical
    /// terminal result carries the registry-retained cancellation reason and
    /// the phase derived from its logical start frontier.
    Cancelled,
    /// The execution's deadline expired and the executor proved the
    /// terminal settlement (see `ToolExecutionStatus::TimedOut`).
    TimedOut,
    /// The executor returned without being able to prove the external
    /// outcome (see `ToolExecutionStatus::OutcomeUnknown`). This is a
    /// first-class terminal state: the execution is settled as far as this
    /// runtime is concerned, but its external outcome is genuinely unknown
    /// and is never collapsed into [`Self::Failed`].
    OutcomeUnknown,
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
    fn on_snapshot(
        &self,
        snapshot: &BackgroundExecutionSnapshot,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    );
    /// Publishes installed native state together with its exact durable receipt.
    fn on_committed(
        &self,
        snapshot: &BackgroundExecutionSnapshot,
        _committed: &crate::events::types::RuntimeEventEnvelope,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
    ) {
        self.on_snapshot(snapshot, cut, ordinal);
    }
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
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Denied
                | Self::Cancelled
                | Self::TimedOut
                | Self::OutcomeUnknown
        )
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
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

/// The one canonical read-only snapshot of one background execution.
///
/// The snapshot is reused by registry queries, `job_status`,
/// `job_cancel`, Agent Status projection input, and
/// deterministic tests. It never exposes internal task handles or process
/// ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundExecutionSnapshot {
    /// Native Bash presentation, derived from the canonical invocation.
    pub bash: Option<Arc<BashPresentation>>,
    /// The detached runtime execution identity.
    pub execution_id: ToolExecutionId,
    /// The canonical tool identity.
    pub tool_id: Arc<ToolId>,
    /// The model-facing tool name.
    pub tool_name: Arc<str>,
    /// The authoritative lifecycle state.
    pub state: BackgroundLifecycle,
    /// The latest bounded progress snapshot, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
    /// The bounded terminal result, when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Arc<ToolExecutionResult>>,
}
