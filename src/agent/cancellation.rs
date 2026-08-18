//! Attempt-level cancellation for the agent loop.
//!
//! [`AgentCancellation`] is the deterministic cancellation trigger of one
//! attempt. It wraps the runtime-owned [`CancellationSignal`] (the one
//! generic cancellation primitive shared by the model plane and the tool
//! plane) with the attempt cancellation reason the terminal event must
//! report, and every model invocation of the attempt receives a child signal
//! so one attempt-level cancel terminates the in-flight provider request
//! through the existing adapter contract.
//!
//! It also owns the attempt's one **model-turn start gate** (Issue #12,
//! M9b): the narrow arbitration point that linearizes attempt cancellation
//! against the durable model-request start commit. The gate guarantees that
//! exactly one of "cancellation" and "the durable request-start commit"
//! wins per model turn, with no unsynchronized window between a cancellation
//! observation and the commit: see [`AgentCancellation::arbitrate_model_turn_start`].
//!
//! The loop races tool execution against this signal: once cancellation is
//! observable while a tool is pending, the loop stops starting new work, and
//! every in-flight cancellable foreground execution observes the same signal
//! through its [`ToolExecutionContext`] and physically settles (for example
//! by terminating an owned process group). A committed valid tool-call batch
//! is structurally settled exactly once before the attempt terminal event.
//!
//! [`ToolExecutionContext`]: crate::tools::executor::ToolExecutionContext

use std::sync::{Arc, Mutex};

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::types::CancellationReason;

/// The adjudication of one model-turn start arbitration.
///
/// Exactly one of the two outcomes is possible for one arbitration call:
/// either attempt cancellation linearized first (and the request never
/// starts), or the durable request-start commit linearized first (and the
/// request has durably started).
pub(crate) enum StartAdjudication<T> {
    /// Cancellation linearized before the durable request-start commit:
    /// the model turn never starts. No `RequestSnapshot`, no
    /// `ModelRequestStarted`, no request-scoped context commit, and no
    /// provider invocation exist for this request.
    CancelledBeforeStart,
    /// The durable request-start commit won: the model request has durably
    /// started. Any later cancellation is post-start cancellation of this
    /// already-started request and can never reclassify it as never-started.
    Started(T),
}

/// The per-attempt model-turn start gate (Issue #12, M9b).
///
/// One mutex serializes the only two competing transitions of the
/// cancellation-vs-start race:
///
/// - [`AgentCancellation::cancel`] transitions the signal to cancelled while
///   holding the gate;
/// - [`AgentCancellation::arbitrate_model_turn_start`] checks the signal and
///   runs the durable start commit while holding the gate.
///
/// Whoever enters the gate first wins, deterministically: a cancellation
/// that enters first is observed by every later arbitration; a start commit
/// that entered first has already decided durability before the cancellation
/// can proceed, and the cancellation is then necessarily post-start. The
/// gate is a leaf in the runtime lock graph: the commit closure nests only
/// the store's own lock inside it, and nothing ever acquires this gate while
/// holding the store or coordinator locks except the cancellation path
/// (coordinator → gate), so no lock cycle exists.
#[derive(Debug, Default)]
struct ModelTurnStartGate {
    critical: Mutex<()>,
}

/// The cancellation signal of one agent attempt.
///
/// The handle is cheap to clone and all clones share one underlying signal
/// and one model-turn start gate. Cancelling the signal makes every pending
/// [`AgentCancellation::cancelled`] future and every model-invocation child
/// signal resolve immediately.
#[derive(Clone, Debug)]
pub struct AgentCancellation {
    signal: CancellationSignal,
    reason: CancellationReason,
    start_gate: Arc<ModelTurnStartGate>,
}

impl AgentCancellation {
    /// Creates a new attempt cancellation signal reporting `reason` when
    /// the attempt terminates by cancellation.
    #[must_use]
    pub fn new(reason: CancellationReason) -> Self {
        Self {
            signal: CancellationSignal::new(),
            reason,
            start_gate: Arc::new(ModelTurnStartGate::default()),
        }
    }

    /// The cancellation reason reported by the terminal cancellation event.
    #[must_use]
    pub fn reason(&self) -> CancellationReason {
        self.reason
    }

    /// Requests cancellation of the attempt.
    ///
    /// The cancellation takes the attempt's model-turn start gate before
    /// transitioning the signal, so it linearizes against any in-progress
    /// durable request-start commit: if a start commit's critical section is
    /// running, this blocks until durability has decided (and the
    /// cancellation is then necessarily post-start); if this critical
    /// section runs first, every later arbitration observes the cancelled
    /// signal and the request never starts.
    ///
    /// # Panics
    ///
    /// Panics if the start-gate mutex is poisoned (a panicking critical
    /// section is a defect, not a recoverable state).
    pub fn cancel(&self) {
        {
            let _critical = self
                .start_gate
                .critical
                .lock()
                .expect("model-turn start gate poisoned");
            self.signal.cancel();
        }
    }

    /// The one cancellation-vs-start linearization point of every model
    /// turn of this attempt (Issue #12, M9b).
    ///
    /// The Agent Loop calls this exactly once per actual model request —
    /// the first turn, every tool→model continuation, every recovered
    /// continuation, and every context-overflow retry — with the durable
    /// request-start commit as `commit`. The gate is held across the check
    /// and the commit, so exactly one of cancellation and the durable start
    /// commit can linearize first:
    ///
    /// - cancellation already requested ⇒
    ///   [`StartAdjudication::CancelledBeforeStart`], `commit` never runs,
    ///   and no request-scoped fact becomes durable;
    /// - the durable commit runs and succeeds ⇒
    ///   [`StartAdjudication::Started`], and a cancellation arriving while
    ///   the commit executed (blocked on the gate) is post-start
    ///   cancellation of the now-started request;
    /// - the durable commit fails ⇒ the `Err` propagates, no start fact
    ///   exists, and no start claim is left behind.
    ///
    /// The provider invocation is never part of this arbitration: it may
    /// only happen after [`StartAdjudication::Started`] returned.
    pub(crate) fn arbitrate_model_turn_start<T, E>(
        &self,
        commit: impl FnOnce() -> Result<T, E>,
    ) -> Result<StartAdjudication<T>, E> {
        let _critical = self
            .start_gate
            .critical
            .lock()
            .expect("model-turn start gate poisoned");
        if self.signal.is_cancelled() {
            return Ok(StartAdjudication::CancelledBeforeStart);
        }
        let committed = commit()?;
        Ok(StartAdjudication::Started(committed))
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

    /// The underlying runtime-owned cancellation signal of the attempt.
    ///
    /// Foreground tool executions receive this signal in their execution
    /// context, so attempt cancellation physically reaches cancellable
    /// native foreground work.
    #[must_use]
    pub fn signal(&self) -> CancellationSignal {
        self.signal.clone()
    }

    /// A model-invocation signal cancelled together with this attempt signal.
    #[must_use]
    pub fn model_cancellation(&self) -> CancellationSignal {
        self.signal.child()
    }
}

#[cfg(test)]
mod tests {
    use super::AgentCancellation;
    use crate::runtime::types::CancellationReason;

    /// A fresh signal is not cancelled and resolves only after `cancel`.
    #[tokio::test]
    async fn cancellation_signal_transitions() {
        let signal = AgentCancellation::new(CancellationReason::UserRequested);
        assert!(!signal.is_cancelled());
        assert_eq!(signal.reason(), CancellationReason::UserRequested);
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    /// The model-invocation child signal follows the attempt signal.
    #[tokio::test]
    async fn model_invocation_follows_attempt_cancellation() {
        let signal = AgentCancellation::new(CancellationReason::ParentCancelled);
        let invocation = signal.model_cancellation();
        assert!(!invocation.is_cancelled());
        signal.cancel();
        assert!(invocation.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), invocation.cancelled())
            .await
            .expect("invocation must be cancelled with the attempt");
    }

    /// Foreground tool executions receive the attempt's underlying signal.
    #[tokio::test]
    async fn tool_executions_share_the_attempt_signal() {
        let signal = AgentCancellation::new(CancellationReason::UserRequested);
        let tool_signal = signal.signal();
        assert!(!tool_signal.is_cancelled());
        signal.cancel();
        assert!(tool_signal.is_cancelled());
    }

    /// Cancellation that linearized before the arbitration wins: the commit
    /// closure never runs.
    #[tokio::test]
    async fn cancellation_before_arbitration_never_commits() {
        let signal = AgentCancellation::new(CancellationReason::UserRequested);
        signal.cancel();
        let arbitration = signal.arbitrate_model_turn_start(|| -> Result<u32, &str> {
            panic!("the start commit never runs once cancellation won")
        });
        assert!(matches!(
            arbitration,
            Ok(super::StartAdjudication::CancelledBeforeStart)
        ));
    }

    /// A successful commit linearizes the start; a cancellation that raced
    /// the commit (blocked on the gate) is post-start cancellation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn committed_start_wins_over_concurrent_cancellation() {
        let signal = AgentCancellation::new(CancellationReason::UserRequested);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (cancel_called_tx, cancel_called_rx) = std::sync::mpsc::channel::<()>();
        let (cancel_returned_tx, cancel_returned_rx) = std::sync::mpsc::channel::<()>();
        // The canceller provably calls `cancel()` only while the start side
        // holds the gate: it waits for the "entered" signal first, so its
        // `cancel()` blocks on the gate for the whole critical section.
        let canceller = std::thread::spawn({
            let signal = signal.clone();
            move || {
                entered_rx.recv().expect("entered channel stays open");
                cancel_called_tx
                    .send(())
                    .expect("cancel-called channel stays open");
                signal.cancel();
                cancel_returned_tx
                    .send(())
                    .expect("cancel-return channel stays open");
            }
        });
        // The arbitration runs on its own thread: its commit closure parks
        // inside the gate until the test releases it.
        let arbiter = std::thread::spawn({
            let signal = signal.clone();
            move || {
                signal.arbitrate_model_turn_start(move || -> Result<u32, &str> {
                    // Inside the gate: signal the test, park until released,
                    // then commit.
                    entered_tx.send(()).expect("entered channel stays open");
                    release_rx.recv().expect("release channel stays open");
                    Ok(7)
                })
            }
        });
        // Wait until the cancellation is provably in flight (its `cancel()`
        // is blocked on the gate the parked arbitration holds).
        cancel_called_rx
            .recv()
            .expect("cancel-called channel stays open");
        assert!(
            cancel_returned_rx.try_recv().is_err(),
            "the concurrent cancellation is still blocked on the gate"
        );
        release_tx.send(()).expect("release channel stays open");
        let arbitration = arbiter.join().expect("arbiter joins");
        assert!(matches!(
            arbitration,
            Ok(super::StartAdjudication::Started(7))
        ));
        canceller.join().expect("canceller joins");
        cancel_returned_rx
            .recv()
            .expect("the cancellation completed after the commit");
        assert!(signal.is_cancelled());
    }

    /// A failed commit leaves no start claim behind: a later arbitration
    /// still observes cancellation honestly.
    #[tokio::test]
    async fn failed_commit_leaves_no_start_claim() {
        let signal = AgentCancellation::new(CancellationReason::UserRequested);
        let arbitration =
            signal.arbitrate_model_turn_start(|| -> Result<u32, &str> { Err("boom") });
        assert!(matches!(arbitration, Err("boom")));
        assert!(!signal.is_cancelled());
        // A later commit may still succeed (the failure claimed nothing).
        let arbitration = signal.arbitrate_model_turn_start(|| -> Result<u32, &str> { Ok(3) });
        assert!(matches!(
            arbitration,
            Ok(super::StartAdjudication::Started(3))
        ));
    }
}
