//! Deterministic test seams of the registry boundaries (test builds only).

use super::super::process::StagedChild;
use super::SteerTicketPhase;
use super::SubagentRegistry;
use crate::runtime::identity::SubagentId;
use std::sync::Arc;
use std::sync::PoisonError;

pub(super) type ResumeCleanupHook =
    Box<dyn FnOnce(&super::super::physical_recovery::ParentPhysicalLease) + Send>;

pub(super) struct AllocationTestHook {
    pub(super) entered: tokio::sync::oneshot::Sender<()>,
    pub(super) stage:
        Box<dyn FnOnce(super::super::process::PhysicalChildRuntimeRoot) -> StagedChild + Send>,
}

/// One-shot barriers at the real resume staging and publication boundaries.
pub(crate) struct ResumeTestGates {
    pub staged: tokio::sync::oneshot::Sender<()>,
    pub release_staged: tokio::sync::oneshot::Receiver<()>,
    pub published: tokio::sync::oneshot::Sender<()>,
    pub release_published: tokio::sync::oneshot::Receiver<()>,
}

impl SubagentRegistry {
    pub(crate) fn install_resume_test_gates(&self, gates: ResumeTestGates) {
        self.state.lock().unwrap().resume_test_gates = Some(gates);
    }
}

/// A test-only pause inside the ownership-commit critical section
/// (production is unwired; mirrors the background dispatch hook).
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
#[derive(Debug, Default)]
pub struct GateReleaseHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

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

impl ControlHandoffHook {
    /// Blocks the handoff until [`Self::release`].
    pub fn wait(&self) {
        if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        }) {
            tokio::task::block_in_place(|| self.wait_inner());
        } else {
            self.wait_inner();
        }
    }

    fn wait_inner(&self) {
        let mut state = self.state.lock().expect("subagent handoff hook");
        if matches!(*state, CommitHookState::Released) {
            return;
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CommitHookState {
    #[default]
    Idle,
    Entered,
    Released,
}

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
#[derive(Debug, Default)]
pub struct CancellationBoundaryHook {
    state: std::sync::Mutex<CancellationHookPhase>,
    changed: std::sync::Condvar,
}

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
#[derive(Debug, Default)]
pub struct TerminalAuthorityHook {
    state: std::sync::Mutex<CommitHookState>,
    changed: std::sync::Condvar,
}

impl TerminalAuthorityHook {
    /// Blocks the terminal settlement path until [`Self::release`].
    pub fn wait(&self) {
        // A native driver can reach this exact pre-registry boundary on the
        // Tokio worker that owns IO/timers. Keep the same Condvar gate, but
        // return that reactor core while a multi-thread fixture parks here so
        // real transport acceptance and the test liveness guard can progress.
        if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            )
        }) {
            tokio::task::block_in_place(|| self.wait_blocking());
        } else {
            self.wait_blocking();
        }
    }

    fn wait_blocking(&self) {
        let mut state = self.state.lock().expect("subagent terminal hook");
        if matches!(*state, CommitHookState::Released) {
            return;
        }
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

/// A test-only pause for one composed [`SubagentRegistry::send_message`]
/// acknowledgement at the exact pre-commit edge: the steer caller has
/// already observed the child's durable decision (an `Accepted` answer has
/// advanced its ticket to
/// [`SteerTicketPhase::ChildAcceptedPendingParentCommit`]) and is parked
/// immediately before the registry commit that consumes the ticket and
/// reports the acknowledgement. The parked acknowledgement holds no registry
/// lock and parks with an ordinary async wait (no executor worker is
/// blocked), so the natural terminal authority can settle the child — and
/// clear every outstanding ticket — while the acknowledgement is parked,
/// exactly the interleaving Issue #193's architecture review blocker 1
/// describes. Production has no pause and no equivalent semantic state; the
/// hook exists only to force that deterministic interleaving in the
/// regression.
#[derive(Debug, Default)]
pub struct SteerAcknowledgementHook {
    state: tokio::sync::watch::Sender<SteerAckHookState>,
}

/// The phases of the steer-acknowledgement pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SteerAckHookState {
    /// No steer acknowledgement has reached the boundary yet.
    #[default]
    Armed,
    /// A steer acknowledgement is parked immediately before its commit.
    Entered,
    /// The parked acknowledgement has been released; later arrivals pass
    /// straight through (the pause is spent).
    Released,
}

impl SteerAcknowledgementHook {
    /// Parks the steer acknowledgement until [`Self::release`]: the first
    /// arriving acknowledgement signals entry and waits; arrivals while one
    /// is parked (or after the release) pass straight through.
    pub async fn park(&self) {
        let mut state = self.state.subscribe();
        let phase = *state.borrow();
        if matches!(phase, SteerAckHookState::Armed) {
            self.state.send_replace(SteerAckHookState::Entered);
            while !matches!(*state.borrow_and_update(), SteerAckHookState::Released) {
                if state.changed().await.is_err() {
                    return;
                }
            }
        }
    }

    /// Awaits until the steer acknowledgement has reached the pause.
    pub async fn wait_until_entered(&self) {
        let mut state = self.state.subscribe();
        while !matches!(*state.borrow_and_update(), SteerAckHookState::Entered) {
            if state.changed().await.is_err() {
                return;
            }
        }
    }

    /// Releases the parked steer acknowledgement and spends the pause: every
    /// later arrival passes straight through.
    pub fn release(&self) {
        self.state.send_replace(SteerAckHookState::Released);
    }
}

impl SubagentRegistry {
    /// Installs a commit-boundary hook (tests only).
    pub fn install_commit_boundary_hook(&self, hook: Arc<CommitBoundaryHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.commit_hook = Some(hook);
    }

    /// Parks after native Stopping publication, before physical seal permission.
    pub(crate) fn install_seal_boundary_hook(&self, hook: Arc<CommitBoundaryHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.seal_boundary_hook = Some(hook);
    }

    /// Installs the exact test-only pause between durable ownership/record
    /// publication and the gate-release critical section.
    pub fn install_control_handoff_hook(&self, hook: Arc<ControlHandoffHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.control_handoff_hook = Some(hook);
    }

    /// Installs the test-only pause at the exact remaining start-gate edge:
    /// the command handle is installed in the record but the start gate is
    /// not yet released. The pause parks inside the gate-release critical
    /// section while holding the registry mutex.
    pub fn install_gate_release_hook(&self, hook: Arc<GateReleaseHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.gate_release_hook = Some(hook);
    }

    /// Installs the test-only pause for the first live cancellation
    /// contender (deadline expiry or an explicit cancel) at the exact
    /// pre-commit edge of the cancellation linearization point: the
    /// contender has passed settlement admission and is parked immediately
    /// before the registry mutex that commits `Running -> Cancelling`.
    pub fn install_cancellation_boundary_hook(&self, hook: Arc<CancellationBoundaryHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.cancellation_boundary_hook = Some(hook);
    }

    /// Installs the test-only pause for the terminal settlement path at the
    /// exact pre-commit edge of the terminal-authority linearization point:
    /// physical settlement is complete and the path is parked immediately
    /// before the registry mutex that commits the terminal candidate and
    /// `... -> PublishingTerminal`.
    pub fn install_terminal_authority_hook(&self, hook: Arc<TerminalAuthorityHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.terminal_authority_hook = Some(hook);
    }

    /// Installs the test-only pause of one composed [`SubagentRegistry::send_message`]
    /// acknowledgement: the steer's child decision has already reached the
    /// caller (an `Accepted` answer has advanced its ticket), and the path is
    /// parked immediately before the registry commit that consumes the ticket
    /// and reports the acknowledgement. The pause holds no registry lock, so
    /// the natural terminal authority — or a cancellation — can acquire the
    /// same mutex while the acknowledgement is parked. Production has no
    /// pause and no equivalent semantic state; the hook exists only to force
    /// the deterministic natural-terminal-after-acceptance interleaving in
    /// the Issue #193 architecture-review regression.
    pub fn install_steer_acknowledgement_hook(&self, hook: Arc<SteerAcknowledgementHook>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.steer_acknowledgement_hook = Some(hook);
    }

    /// Test-only: registers a one-shot completion latch for this child's
    /// record-owned deadline task. The receiver resolves only after the
    /// fired deadline's cancellation call has fully returned. Must be
    /// called before the child starts: the owning commit claims the latch
    /// when it creates the deadline task.
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

    /// Test-only authoritative observation of one child's outstanding steer
    /// tickets (Issue #193): how many tickets the record currently holds
    /// under the arbitration mutex, in any phase.
    ///
    /// This is the direct observation the ownership regressions assert
    /// against — ticket cleanup is proven here, not merely inferred from
    /// later steer behavior.
    pub fn outstanding_guidance_tickets(&self, subagent_id: &SubagentId) -> usize {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return 0;
        };
        state.records[index].steer_tickets.len()
    }

    /// Test-only authoritative observation of how many of one child's
    /// outstanding steer tickets have already advanced to
    /// [`SteerTicketPhase::ChildAcceptedPendingParentCommit`] (Issue #193):
    /// the child's durable `Accepted` answer reached the registry and only
    /// the parent acknowledgement commit is still owed.
    pub fn accepted_guidance_pending_commit(&self, subagent_id: &SubagentId) -> usize {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return 0;
        };
        state.records[index]
            .steer_tickets
            .iter()
            .filter(|ticket| ticket.phase == SteerTicketPhase::ChildAcceptedPendingParentCommit)
            .count()
    }
}
