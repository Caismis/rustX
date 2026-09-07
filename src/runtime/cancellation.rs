//! RustX-owned cancellation signal.
//!
//! [`CancellationSignal`] is the one runtime-owned cancellation primitive
//! shared by the model plane and the tool plane: model adapter invocations,
//! the compaction summarizer, foreground tool execution, and detached
//! background executions all observe the same underlying signal mechanism.
//! There is exactly one generic cancellation model in the runtime.
//!
//! The signal itself is semantics-free: it does not own an attempt reason,
//! a background lifecycle, or any execution ownership. [`AgentCancellation`]
//! wraps it with the attempt cancellation reason, and the background
//! registry owns its own background cancellation state.
//!
//! [`AgentCancellation`]: crate::agent::cancellation::AgentCancellation

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;

use crate::runtime::types::CancellationReason;

/// Cancellation signal for one operation.
///
/// Cancelling the signal makes every pending [`CancellationSignal::cancelled`]
/// future resolve immediately. Model adapters stop consuming the provider
/// stream, drop the underlying HTTP stream, do not retry, and terminate with
/// `Failed(Cancelled)`; cancellable tool executors settle their external work
/// (for example by terminating an owned process group) and return a
/// normalized cancelled result.
#[derive(Clone, Debug, Default)]
pub struct CancellationSignal {
    token: CancellationToken,
}

impl CancellationSignal {
    /// Creates a new cancellation signal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Requests cancellation of the associated operation.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Creates a child signal: cancelling this signal cancels the child, so
    /// one owner can fan out one signal per operation while all of them
    /// terminate together.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
        }
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// A future that resolves when cancellation is requested.
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }
}

/// The absorbing first-winner cancellation cause of one owned operation.
///
/// The cause is **read from the owner that owns the signal**, at the moment
/// the executor observes cancellation — never copied into a start-time
/// snapshot. This is the attempt/background reason authority: foreground
/// invocations additionally carry their native lifecycle's deadline-or-attempt
/// arbitration through `ExecutionCancellation::native_cause`. Neither authority
/// can relabel its first winner or mutate its ancestor's reason.
pub trait CancellationCause: Send + Sync {
    /// The current winning semantic cause, or the owner's default cause when
    /// no cancellation has been requested yet.
    fn cause(&self) -> CancellationReason;
}

/// A fixed cause with no live authority behind it.
///
/// This exists only for executions that have no attempt and no background
/// record to answer for them (standalone/direct executor invocations and
/// fixtures). It is not a second store for an owned execution: an owned
/// execution always carries its owner's authority.
#[derive(Debug, Clone, Copy)]
struct DetachedCause(CancellationReason);

impl CancellationCause for DetachedCause {
    fn cause(&self) -> CancellationReason {
        self.0
    }
}

/// The cancellation view handed to a tool executor.
///
/// It pairs the runtime cancellation signal with a **live** read of the
/// owning authority's absorbing cause, so an executor that starts before
/// cancellation happens still reports the cause that actually won the race.
/// Each native lifecycle additionally owns its own absorbing arbitration cause;
/// descendant views read that authority without modifying the attempt/background
/// reason. Signal propagation and semantic provenance remain distinct.
#[derive(Clone)]
pub struct ExecutionCancellation {
    signal: CancellationSignal,
    cause: Arc<dyn CancellationCause>,
    native_cause: Option<Arc<NativeCancellationAuthority>>,
    /// A process-local failure marker for semantic control paths that can no
    /// longer safely release the owning execution. It is deliberately
    /// separate from cancellation: a broken interaction route must not be
    /// rewritten as a human cancellation outcome.
    interaction_failure: InteractionFailureSignal,
}

/// The lifecycle that wins a native race publishes its cause before signalling
/// descendants. Unrequested scopes read through to their ancestor authority.
struct NativeCancellationAuthority {
    winner: std::sync::OnceLock<crate::tools::deadline::ToolCancellationCause>,
    parent: ExecutionCancellation,
}

pub(crate) struct ExecutionCancellationTrigger {
    signal: CancellationSignal,
    authority: Arc<NativeCancellationAuthority>,
}

impl ExecutionCancellationTrigger {
    pub(crate) fn cancel(&self, cause: crate::tools::deadline::ToolCancellationCause) {
        // Semantic linearization point. The absorbing cause is visible before
        // any child can observe the locally requested cancellation signal.
        let _ = self.authority.winner.set(cause);
        self.signal.cancel();
    }
}

impl std::fmt::Debug for ExecutionCancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionCancellation")
            .field("is_cancelled", &self.is_cancelled())
            .field("reason", &self.reason())
            .finish()
    }
}

impl ExecutionCancellation {
    /// Binds one runtime cancellation signal to the authority that owns its
    /// semantic cause.
    #[must_use]
    pub fn new(signal: CancellationSignal, cause: Arc<dyn CancellationCause>) -> Self {
        Self {
            signal,
            cause,
            native_cause: None,
            interaction_failure: InteractionFailureSignal::default(),
        }
    }

    /// Attaches the owning attempt's process-local interaction-failure
    /// marker. The marker is shared by cloned execution views and never
    /// becomes a durable or routable authority.
    #[must_use]
    pub(crate) fn with_interaction_failure(
        mut self,
        interaction_failure: InteractionFailureSignal,
    ) -> Self {
        self.interaction_failure = interaction_failure;
        self
    }

    /// A view over a signal with no owning attempt/background authority.
    #[must_use]
    pub fn detached(signal: CancellationSignal, reason: CancellationReason) -> Self {
        Self {
            signal,
            cause: Arc::new(DetachedCause(reason)),
            native_cause: None,
            interaction_failure: InteractionFailureSignal::default(),
        }
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.signal.is_cancelled()
    }

    /// A future that resolves when cancellation is requested.
    pub async fn cancelled(&self) {
        self.signal.cancelled().await;
    }

    /// The authority's current winning semantic cause.
    ///
    /// Read this at settlement time, when cancellation is observable: before
    /// the cancellation race has happened it necessarily reports the owner's
    /// default cause, which is a prediction, not an outcome.
    #[must_use]
    pub fn reason(&self) -> CancellationReason {
        self.cause.cause()
    }

    /// Native cancellation provenance, including ancestor deadline intent.
    /// `reason()` remains the attempt/background reason for physical adapters;
    /// native owners must use this view when classifying execution outcomes.
    #[must_use]
    pub(crate) fn native_cause(&self) -> crate::tools::deadline::ToolCancellationCause {
        self.native_cause.as_ref().map_or_else(
            || crate::tools::deadline::ToolCancellationCause::Attempt(self.reason()),
            |authority| {
                authority
                    .winner
                    .get()
                    .copied()
                    .unwrap_or_else(|| authority.parent.native_cause())
            },
        )
    }

    pub(crate) fn native_status(
        &self,
        phase: crate::tools::types::ToolCancellationPhase,
    ) -> crate::tools::types::ToolExecutionStatus {
        match self.native_cause() {
            crate::tools::deadline::ToolCancellationCause::Attempt(reason) => {
                crate::tools::types::ToolExecutionStatus::Cancelled { reason, phase }
            }
            crate::tools::deadline::ToolCancellationCause::Deadline(_) => {
                crate::tools::types::ToolExecutionStatus::TimedOut
            }
        }
    }

    /// Derives a cancellation signal for subordinate work.
    ///
    /// Cancellation propagates from the owning operation into this child, but
    /// cancelling the returned signal cannot cancel the owner. This is the
    /// only signal capability exposed by the execution observation view: a
    /// `ToolExecutor` can supervise a process or nested runner without
    /// acquiring the Agent Loop attempt's cancellation authority.
    #[must_use]
    pub fn child_signal(&self) -> CancellationSignal {
        self.signal.child()
    }

    /// Derives one per-execution child view together with its owner-side
    /// trigger (Issue #204).
    ///
    /// The child reads a lifecycle-owned absorbing native cause, falling through
    /// to its ancestor until this lifecycle wins. Publishing the cause precedes
    /// signalling, so nested deadlines never masquerade as attempt reasons.
    /// The trigger cannot change the attempt authority. Physical adapters may
    /// still read `reason()`; native terminal arbitration uses `native_cause()`.
    #[must_use]
    pub(crate) fn child_execution(&self) -> (ExecutionCancellationTrigger, ExecutionCancellation) {
        let signal = self.signal.child();
        let authority = Arc::new(NativeCancellationAuthority {
            winner: std::sync::OnceLock::new(),
            parent: self.clone(),
        });
        let view = Self {
            signal: signal.clone(),
            cause: Arc::clone(&self.cause),
            native_cause: Some(authority.clone()),
            interaction_failure: self.interaction_failure.clone(),
        };
        (ExecutionCancellationTrigger { signal, authority }, view)
    }

    /// Marks the owning attempt as unable to continue because its semantic
    /// interaction control path failed. This does not request cancellation or
    /// choose an interaction outcome.
    pub(crate) fn mark_interaction_failure(&self) {
        self.interaction_failure.mark();
    }
}

/// One shared, absorbing marker for a failed semantic interaction control
/// path. It is process-local execution state, not durable interaction
/// authority and not a second lifecycle/cancellation state machine.
#[derive(Clone, Debug, Default)]
pub(crate) struct InteractionFailureSignal {
    failed: Arc<AtomicBool>,
}

impl InteractionFailureSignal {
    pub(crate) fn mark(&self) {
        self.failed.store(true, Ordering::Release);
    }

    #[must_use]
    pub(crate) fn is_marked(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationSignal;

    /// A fresh cancellation signal is not cancelled and resolves `cancelled`
    /// only after `cancel` is invoked.
    #[tokio::test]
    async fn cancellation_signal_transitions() {
        let signal = CancellationSignal::new();
        assert!(!signal.is_cancelled());
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    /// Clones share the same underlying signal.
    #[tokio::test]
    async fn cloned_signals_share_cancellation() {
        let first = CancellationSignal::new();
        let second = first.clone();
        first.cancel();
        second.cancelled().await;
        assert!(second.is_cancelled());
    }

    /// `cancelled` resolves even if cancellation arrived before it was
    /// awaited, which is how a stream loop notices an early cancellation.
    #[tokio::test]
    async fn cancelled_after_the_fact_resolves_immediately() {
        let signal = CancellationSignal::new();
        signal.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.cancelled())
            .await
            .expect("cancelled must resolve without waiting");
    }

    /// The execution view reads its authority dynamically: an executor that
    /// started before the cancellation race still reports the winning cause.
    #[tokio::test]
    async fn execution_cancellation_reads_the_authority_dynamically() {
        use super::{CancellationCause, ExecutionCancellation};
        use crate::runtime::types::CancellationReason;
        use std::sync::{Arc, Mutex};

        struct Authority {
            default: CancellationReason,
            winner: Mutex<Option<CancellationReason>>,
        }
        impl CancellationCause for Authority {
            fn cause(&self) -> CancellationReason {
                self.winner
                    .lock()
                    .expect("winner lock")
                    .unwrap_or(self.default)
            }
        }
        let authority = Arc::new(Authority {
            default: CancellationReason::UserRequested,
            winner: Mutex::new(None),
        });
        let signal = CancellationSignal::new();
        let view = ExecutionCancellation::new(signal.clone(), authority.clone());
        // The view is taken before the cancellation race: it must not freeze
        // the default cause.
        assert_eq!(view.reason(), CancellationReason::UserRequested);
        *authority.winner.lock().expect("winner lock") = Some(CancellationReason::RuntimeShutdown);
        signal.cancel();
        view.cancelled().await;
        assert_eq!(view.reason(), CancellationReason::RuntimeShutdown);
    }

    /// An execution view derives one-way cancellation for subordinate work:
    /// the owner reaches the child, while the child cannot reach the owner or
    /// alter the owner's live cause.
    #[test]
    fn execution_child_cancellation_is_downward_only() {
        use super::{CancellationCause, ExecutionCancellation};
        use crate::runtime::types::CancellationReason;
        use std::sync::{Arc, Mutex};

        struct Authority {
            default: CancellationReason,
            winner: Mutex<Option<CancellationReason>>,
        }
        impl CancellationCause for Authority {
            fn cause(&self) -> CancellationReason {
                self.winner
                    .lock()
                    .expect("winner lock")
                    .unwrap_or(self.default)
            }
        }

        let parent = CancellationSignal::new();
        let authority = Arc::new(Authority {
            default: CancellationReason::UserRequested,
            winner: Mutex::new(None),
        });
        let view = ExecutionCancellation::new(parent.clone(), authority.clone());
        let child = view.child_signal();

        child.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        assert!(!view.is_cancelled());
        assert_eq!(view.reason(), CancellationReason::UserRequested);

        *authority.winner.lock().expect("winner lock") = Some(CancellationReason::RuntimeShutdown);
        let propagated_child = view.child_signal();
        parent.cancel();
        assert!(view.is_cancelled());
        assert!(propagated_child.is_cancelled());
        assert_eq!(view.reason(), CancellationReason::RuntimeShutdown);
    }

    /// A child signal is cancelled when its parent is cancelled, so one
    /// owner-level signal can govern every operation of an owner.
    #[tokio::test]
    async fn child_signals_follow_parent_cancellation() {
        let parent = CancellationSignal::new();
        let child = parent.child();
        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), child.cancelled())
            .await
            .expect("child must be cancelled with its parent");
    }
}
