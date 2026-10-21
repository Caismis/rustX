//! Test-only synchronization for in-crate unit tests.
//!
//! [`ContinuationBoundaryPause`] parks the execution at the turn-continuation
//! boundary — after a completed turn (including every mailbox drain/append
//! of that turn) returned "continue", before the generic
//! cancellation-before-next-turn check — so a unit test can make
//! cancellation observable deterministically between turns, without timing
//! assumptions.
//!
//! The pause signals `reached` through a watch (observed with `wait_for`)
//! and blocks the execution task on a `std` channel until the test
//! releases it, so the controlling test must run on a multi-threaded
//! runtime. This hook exists only under `#[cfg(test)]`.

use std::sync::mpsc;

use tokio::sync::oneshot;
use tokio::sync::watch;

/// A test-only control point at the turn-continuation boundary.
///
/// The execution parks here exactly when a completed turn returned
/// "continue" — the turn is structurally complete and every mailbox
/// drain/append of that turn is done — before the generic
/// cancellation-before-next-turn check runs. A unit test can therefore
/// make cancellation observable deterministically after one turn
/// completed but before another starts, without timing assumptions.
///
/// The pause signals `reached` through a watch (observed with
/// `wait_for`) and blocks the execution task on a `std` channel until
/// the test releases it, so the controlling test must run on a
/// multi-threaded runtime. This hook exists only under `#[cfg(test)]`.
#[derive(Debug)]
pub(super) struct ContinuationBoundaryPause {
    reached: watch::Sender<bool>,
    release: mpsc::Receiver<()>,
}

impl ContinuationBoundaryPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(super) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(false);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Signals that the turn boundary was reached, then blocks until
    /// the test releases the execution.
    pub(super) fn park_at_continuation_boundary(&self) {
        self.reached.send_replace(true);
        let _ = self.release.recv();
    }
}

/// A test-only control point after a transient `ModelRetryScheduled`
/// event is durably committed and after the retry's absolute monotonic
/// deadline is captured, before the backoff wait begins. Cancellation can
/// therefore be placed between those two linearization points without
/// relying on elapsed wall-clock time.
#[derive(Debug)]
pub(crate) struct RetryBackoffPause {
    reached: watch::Sender<u32>,
    release: mpsc::Receiver<()>,
}

impl RetryBackoffPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<u32>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(0);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Increments the scheduled-retry count and waits for the
    /// deterministic test controller to release this backoff.
    pub(super) fn park(&self) {
        self.reached.send_modify(|count| *count += 1);
        let _ = self.release.recv();
    }
}

/// A test-only control point after one provider stream item has completed all
/// Agent-Loop processing. It is used to synchronize exact readiness cuts
/// between provider stream items, publication flushes, and request deadlines.
#[derive(Debug)]
pub(crate) struct ModelStreamItemPause {
    reached: watch::Sender<u32>,
    release: mpsc::Receiver<()>,
}

impl ModelStreamItemPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<u32>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(0);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Announces that one event has been fully processed, then waits for
    /// the test to release the next provider wait.
    pub(super) fn park(&self) {
        self.reached.send_modify(|count| *count += 1);
        let _ = self.release.recv();
    }
}

/// A test-only control point after one rejected physical generation is
/// durably settled and before the logical model step decides whether a
/// corrective generation is authorized. Cancellation can therefore be
/// placed exactly between those two linearization points.
#[derive(Debug)]
pub(crate) struct GenerationAnomalyPause {
    reached: watch::Sender<u32>,
    release: mpsc::Receiver<()>,
}

impl GenerationAnomalyPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<u32>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(0);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Announces one settled rejected generation, then waits for the
    /// test to release the recovery decision.
    pub(super) fn park(&self) {
        self.reached.send_modify(|count| *count += 1);
        let _ = self.release.recv();
    }
}

/// A one-shot test-only control point immediately before a provider wait.
/// It lets a controller make several branches ready before the execution
/// constructs and polls the biased arbitration, so simultaneous-ready
/// precedence tests do not depend on executor scheduling.
#[derive(Debug)]
pub(crate) struct ModelArbitrationPause {
    after_items: u32,
    reached: watch::Sender<bool>,
    release: oneshot::Receiver<()>,
}

impl ModelArbitrationPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install(after_items: u32) -> (Self, watch::Receiver<bool>, oneshot::Sender<()>) {
        let (reached, reached_rx) = watch::channel(false);
        let (release_tx, release_rx) = oneshot::channel();
        (
            Self {
                after_items,
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// The provider-stream-item count at which this pause is eligible.
    pub(super) const fn after_items(&self) -> u32 {
        self.after_items
    }

    /// Announces that the arbitration boundary was reached, then waits
    /// until the controller has made the competing branches ready.
    pub(super) async fn wait(self) {
        self.reached.send_replace(true);
        let _ = self.release.await;
    }
}

/// A test-only control point after one `ToolExecutionStarted` fact and
/// before the next sibling is announced. The test can request attempt or
/// runtime cancellation while the exact start frontier is parked.
#[derive(Debug)]
pub(crate) struct ToolStartPause {
    reached: watch::Sender<bool>,
    release: mpsc::Receiver<()>,
}

impl ToolStartPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(false);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Signals the first announced tool start and blocks the loop until
    /// the test releases the frontier.
    pub(super) fn park(&self) {
        self.reached.send_replace(true);
        let _ = self.release.recv();
    }
}

/// A test-only **signal** (no park) published the instant one foreground
/// tool call's hard deadline becomes an absolute monotonic value.
///
/// It exists because `ToolExecutionStarted` is *not* that instant: the
/// start fact is emitted to observers before `run_foreground` reads its
/// executor-start frontier from the clock. A manual-clock test that
/// advanced on the start fact could therefore advance *before* the
/// frontier is read, pushing the absolute deadline out by the same
/// amount and silently never firing it. Waiting for this signal is what
/// makes such a test an ordering rather than a race.
#[derive(Debug)]
pub(crate) struct ToolDeadlineArmedSignal {
    armed: watch::Sender<Option<u64>>,
}

impl ToolDeadlineArmedSignal {
    /// Creates the signal and its observation handle. The handle carries
    /// the absolute hard-deadline value, so a test can assert exactly
    /// which frontier it is about to cross.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<Option<u64>>) {
        let (armed, armed_rx) = watch::channel(None);
        (Self { armed }, armed_rx)
    }

    /// Publishes the absolute hard deadline of the call being started.
    pub(super) fn armed(&self, deadline_millis: u64) {
        self.armed.send_replace(Some(deadline_millis));
    }
}

/// A test-only control point after the foreground cancellation branch
/// won its physical-result arbitration and before the Agent Loop awaits
/// the deliberately late physical future. The test can therefore prove
/// that cancellation settled first, then release the physical operation
/// and verify that its result cannot replace the canonical cancellation.
#[derive(Debug)]
pub(crate) struct ToolCancellationSettlementPause {
    reached: watch::Sender<bool>,
    release: mpsc::Receiver<()>,
}

impl ToolCancellationSettlementPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(false);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Announces that cancellation won the Agent Loop arbitration, then
    /// waits for the test to release the late physical future.
    pub(super) fn park(&self) {
        self.reached.send_replace(true);
        let _ = self.release.recv();
    }
}

/// A test-only control point after the physical-result branch wins the
/// foreground settlement arbitration and before post-arbitration result
/// normalization. The controller can request a later attempt
/// cancellation at that exact point and prove it cannot rewrite the
/// physical winner's cancellation reason.
#[derive(Debug)]
pub(crate) struct ToolPhysicalSettlementPause {
    reached: watch::Sender<bool>,
    release: mpsc::Receiver<()>,
}

impl ToolPhysicalSettlementPause {
    /// Creates the pause and its observation/release handles.
    #[must_use]
    pub(crate) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
        let (reached, reached_rx) = watch::channel(false);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Announces that the physical result won and waits for the test to
    /// release post-arbitration settlement.
    pub(super) fn park(&self) {
        self.reached.send_replace(true);
        let _ = self.release.recv();
    }
}

/// A test-only control point at the one M9b model-turn start boundary.
///
/// Two independent phases cover the deterministic race matrix:
///
/// - `pre_start`: parks immediately before the cancellation-vs-start
///   arbitration, after every fallible preparation step completed. No
///   request-scoped context, Request Snapshot, start fact, or provider
///   request exists while parked, so a cancellation issued while parked
///   provably linearizes before the arbitration.
/// - `pre_commit`: parks **inside** the arbitration critical section
///   (the attempt's start gate is held), immediately before the durable
///   start commit. A cancellation issued while parked provably blocks
///   behind the gate and is therefore post-start once the commit
///   succeeds.
///
/// Each phase counts its parks on a watch channel and blocks the
/// execution task on a `std` channel until the test releases that exact
/// park, so multi-request tests address each park by ordinal. This hook
/// exists only under `#[cfg(test)]`.
#[derive(Debug)]
pub(crate) struct StartBoundaryPause {
    pre_start: Option<PhasePause>,
    pre_commit: Option<PhasePause>,
}

/// One parked phase of the start boundary.
#[derive(Debug)]
struct PhasePause {
    reached: watch::Sender<u32>,
    release: mpsc::Receiver<()>,
    parks: std::sync::atomic::AtomicU32,
}

/// The test-side control of one installed phase.
#[derive(Debug)]
pub(crate) struct PhasePauseControl {
    reached: watch::Receiver<u32>,
    release: mpsc::Sender<()>,
}

impl PhasePauseControl {
    /// Waits until the phase parked the `ordinal`-th time (1-based).
    pub(crate) async fn await_park(&mut self, ordinal: u32) {
        self.reached
            .wait_for(|parks| *parks >= ordinal)
            .await
            .expect("start boundary pause channel stays open");
    }

    /// Releases the currently parked execution.
    pub(crate) fn release(&self) {
        let _ = self.release.send(());
    }
}

impl PhasePause {
    fn install() -> (Self, PhasePauseControl) {
        let (reached, reached_rx) = watch::channel(0_u32);
        let (release_tx, release_rx) = mpsc::channel();
        (
            Self {
                reached,
                release: release_rx,
                parks: std::sync::atomic::AtomicU32::new(0),
            },
            PhasePauseControl {
                reached: reached_rx,
                release: release_tx,
            },
        )
    }

    fn park(&self) {
        // Hand off the worker before announcing this synchronous fixture
        // park, so a control task in its unstealable LIFO slot can run.
        // Both M9 call sites retain their original arbitration boundary.
        tokio::task::block_in_place(|| {
            let parks = self.parks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            self.reached.send_replace(parks);
            let _ = self.release.recv();
        });
    }
}

impl StartBoundaryPause {
    /// Creates the pause with the selected phases and their test-side
    /// controls.
    #[must_use]
    pub(crate) fn install(
        pre_start: bool,
        pre_commit: bool,
    ) -> (Self, Option<PhasePauseControl>, Option<PhasePauseControl>) {
        let (pre_start_pause, pre_start_control) = if pre_start {
            let (pause, control) = PhasePause::install();
            (Some(pause), Some(control))
        } else {
            (None, None)
        };
        let (pre_commit_pause, pre_commit_control) = if pre_commit {
            let (pause, control) = PhasePause::install();
            (Some(pause), Some(control))
        } else {
            (None, None)
        };
        (
            Self {
                pre_start: pre_start_pause,
                pre_commit: pre_commit_pause,
            },
            pre_start_control,
            pre_commit_control,
        )
    }

    /// Parks immediately before the cancellation-vs-start arbitration.
    pub(super) fn park_before_start_arbitration(&self) {
        if let Some(phase) = &self.pre_start {
            phase.park();
        }
    }

    /// Parks inside the arbitration critical section, immediately
    /// before the durable start commit (the start gate is held).
    pub(super) fn park_before_start_commit(&self) {
        if let Some(phase) = &self.pre_commit {
            phase.park();
        }
    }
}

/// Every test-only synchronization point of one execution. Each one is
/// installed before the execution runs and lives exactly as long as it. The
/// `Mutex` keeps an execution with an installed pause `Sync` (several pauses
/// hold a `std::sync::mpsc::Receiver`), so host-driven attempt tasks can be
/// spawned across threads in tests.
#[derive(Default)]
pub(super) struct ExecutionPauses {
    /// Parks after `CompactionStarted`, before native Summary admission.
    pub(super) compaction_admission:
        std::sync::Mutex<Option<std::sync::Arc<crate::runtime::conversation_runtime::Gate>>>,
    /// Test-only control point parked at the turn-continuation boundary:
    /// after a completed turn (and all its mailbox drain/append work)
    /// returned "continue", before the generic cancellation check of the
    /// next model turn; never present outside `#[cfg(test)]`. The `Mutex`
    /// keeps an execution with an installed pause `Sync` (the pause holds
    /// a `std::sync::mpsc::Receiver`), so host-driven attempt tasks can be
    /// spawned across threads in tests.
    pub(super) continuation_pause: std::sync::Mutex<Option<ContinuationBoundaryPause>>,
    /// Test-only control point at the one M9b model-turn start boundary:
    /// immediately before the cancellation-vs-start arbitration, and inside
    /// the arbitration critical section immediately before the durable
    /// start commit. No request-scoped context, Request Snapshot, start
    /// fact, or provider request exists while the first phase is parked;
    /// while the second phase is parked the arbitration holds the start
    /// gate, so a concurrent cancellation provably blocks behind it.
    pub(super) start_boundary_pause: std::sync::Mutex<Option<StartBoundaryPause>>,
    /// Test-only control point after `ModelRetryScheduled` commits and after
    /// the retry deadline is captured, before the monotonic backoff wait
    /// begins. It makes the cancellation/backoff ordering observable without
    /// sleeping.
    pub(super) retry_backoff_pause: std::sync::Mutex<Option<RetryBackoffPause>>,
    /// Test-only control point after one provider stream item has been
    /// classified and fully processed. It makes exact provider/deadline and
    /// publication/deadline cuts observable without scheduler timing.
    pub(super) model_stream_item_pause: std::sync::Mutex<Option<ModelStreamItemPause>>,
    /// Test-only control point immediately before one provider arbitration.
    /// The controller can make multiple branches ready while the execution is
    /// parked, then release it to prove the explicit biased precedence.
    pub(super) model_arbitration_pause: std::sync::Mutex<Option<ModelArbitrationPause>>,
    /// Test-only control point after a rejected physical generation is
    /// durably settled and before the logical step decides whether a
    /// corrective generation is authorized. It makes the
    /// cancellation-versus-generation-anomaly race an exact interleaving
    /// rather than a likely one.
    pub(super) generation_anomaly_pause: std::sync::Mutex<Option<GenerationAnomalyPause>>,
    /// Test-only control point after a foreground tool-start fact and before
    /// the next sibling's start frontier advances. This makes cancellation
    /// during a parallel batch deterministic without changing production
    /// scheduling.
    pub(super) tool_start_pause: std::sync::Mutex<Option<ToolStartPause>>,
    /// Test-only signal published when one foreground call's hard deadline
    /// becomes an absolute monotonic value, which is strictly after the
    /// `ToolExecutionStarted` fact. A manual-clock test uses it to advance
    /// the clock across a frontier that provably already exists.
    pub(super) tool_deadline_armed: std::sync::Mutex<Option<ToolDeadlineArmedSignal>>,
    /// Test-only control point after cancellation wins the foreground
    /// physical-result arbitration and before the late physical future is
    /// awaited. It makes the cancellation linearization observable without
    /// changing production scheduling or adding a runtime event.
    pub(super) tool_cancellation_settlement_pause:
        std::sync::Mutex<Option<ToolCancellationSettlementPause>>,
    /// Test-only control point immediately after the physical-result branch
    /// wins foreground settlement arbitration. It lets a test request attempt
    /// cancellation after the physical result is authoritative, before any
    /// post-arbitration normalization runs.
    pub(super) tool_physical_settlement_pause:
        std::sync::Mutex<Option<ToolPhysicalSettlementPause>>,
}

use super::AgentExecution;

impl AgentExecution<'_> {
    pub(crate) fn install_compaction_admission_gate(
        &mut self,
        gate: std::sync::Arc<crate::runtime::conversation_runtime::Gate>,
    ) {
        *self.pauses.compaction_admission.lock().unwrap() = Some(gate);
    }

    /// Installs the test-only start-boundary pause (Issue #12, M9b
    /// deterministic race tests).
    #[cfg(test)]
    pub(crate) fn install_start_boundary_pause(&mut self, pause: StartBoundaryPause) {
        *self
            .pauses
            .start_boundary_pause
            .lock()
            .expect("start boundary pause lock") = Some(pause);
    }

    /// Installs the deterministic retry-backoff synchronization hook.
    #[cfg(test)]
    pub(crate) fn install_retry_backoff_pause(&mut self, pause: RetryBackoffPause) {
        *self
            .pauses
            .retry_backoff_pause
            .lock()
            .expect("retry backoff pause lock") = Some(pause);
    }

    /// Installs the deterministic post-provider-stream-item synchronization hook.
    #[cfg(test)]
    pub(crate) fn install_model_stream_item_pause(&mut self, pause: ModelStreamItemPause) {
        *self
            .pauses
            .model_stream_item_pause
            .lock()
            .expect("model stream item pause lock") = Some(pause);
    }

    /// Installs the deterministic pre-arbitration synchronization hook.
    #[cfg(test)]
    pub(crate) fn install_model_arbitration_pause(&mut self, pause: ModelArbitrationPause) {
        *self
            .pauses
            .model_arbitration_pause
            .lock()
            .expect("model arbitration pause lock") = Some(pause);
    }

    /// Installs the deterministic rejected-generation synchronization hook.
    #[cfg(test)]
    pub(crate) fn install_generation_anomaly_pause(&mut self, pause: GenerationAnomalyPause) {
        *self
            .pauses
            .generation_anomaly_pause
            .lock()
            .expect("generation anomaly pause lock") = Some(pause);
    }

    /// Installs the test-only foreground tool-start pause for one attempt.
    #[cfg(test)]
    pub(crate) fn install_tool_start_pause(&mut self, pause: ToolStartPause) {
        *self
            .pauses
            .tool_start_pause
            .lock()
            .expect("tool start pause lock") = Some(pause);
    }

    /// Installs the test-only foreground tool hard-deadline arming signal.
    #[cfg(test)]
    pub(crate) fn install_tool_deadline_armed_signal(&mut self, signal: ToolDeadlineArmedSignal) {
        *self
            .pauses
            .tool_deadline_armed
            .lock()
            .expect("tool deadline armed signal lock") = Some(signal);
    }

    /// Installs the deterministic foreground cancellation-settlement pause.
    #[cfg(test)]
    pub(crate) fn install_tool_cancellation_settlement_pause(
        &mut self,
        pause: ToolCancellationSettlementPause,
    ) {
        *self
            .pauses
            .tool_cancellation_settlement_pause
            .lock()
            .expect("tool cancellation settlement pause lock") = Some(pause);
    }

    /// Installs the deterministic physical-result settlement pause.
    #[cfg(test)]
    pub(crate) fn install_tool_physical_settlement_pause(
        &mut self,
        pause: ToolPhysicalSettlementPause,
    ) {
        *self
            .pauses
            .tool_physical_settlement_pause
            .lock()
            .expect("tool physical settlement pause lock") = Some(pause);
    }
}
