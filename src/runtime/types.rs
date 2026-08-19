//! Runtime-owned shared semantics: token measurements, cancellation reasons,
//! runtime errors, and the conversation lifecycle.
//!
//! These types are shared by context, tool, model, and event contracts. They
//! are plain runtime-owned data and never reference provider SDK or storage
//! types.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The authoritative lifecycle state of one conversation runtime.
///
/// `Running -> Draining` is the single drain linearization point. The
/// lifecycle is shared by the coordinator, inbound mailbox, background
/// registry, and capability coordinator, so every runtime-owned semantic
/// commit observes the same admission boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationLifecycleState {
    /// Composition has completed, but semantic runtime work cannot begin yet.
    Inactive,
    /// New semantic work may be admitted.
    Running,
    /// New semantic work is closed; already-owned work may settle.
    Draining,
    /// No runtime-owned operation remains capable of a semantic effect.
    Quiescent,
}

impl ConversationLifecycleState {
    const INACTIVE: u8 = 0;
    const RUNNING: u8 = 1;
    const DRAINING: u8 = 2;
    const QUIESCENT: u8 = 3;

    const fn from_raw(raw: u8) -> Self {
        match raw {
            Self::RUNNING => Self::Running,
            Self::DRAINING => Self::Draining,
            Self::QUIESCENT => Self::Quiescent,
            _ => Self::Inactive,
        }
    }
}

#[derive(Debug)]
struct LifecycleInner {
    state: AtomicU8,
    /// Number of concrete runtime-owned subsystem operations that have
    /// crossed an admission boundary and have not yet returned. A drain
    /// waits for this to reach zero before observing quiescence.
    admissions: AtomicUsize,
    /// Serializes the short native commit sections that do not take the
    /// conversation coordinator lock. A drain takes this boundary before
    /// publishing `Draining`; a background ownership or capability commit
    /// takes it before its authoritative swap. This makes their race a
    /// total order instead of relying on a check followed by an unrelated
    /// write.
    commit_boundary: Mutex<()>,
    changed: tokio::sync::Notify,
}

/// One counted runtime lifecycle admission.
///
/// The guard is intentionally private to runtime-owned subsystem seams. It
/// is not a generic shutdown hook: it only proves that one concrete semantic
/// operation remains inside its native ownership boundary.
pub(crate) struct LifecycleAdmission {
    inner: Arc<LifecycleInner>,
}

impl Drop for LifecycleAdmission {
    fn drop(&mut self) {
        self.inner.admissions.fetch_sub(1, Ordering::AcqRel);
        self.inner.changed.notify_waiters();
    }
}

/// The one authoritative lifecycle of a conversation runtime.
///
/// Activation is `Inactive -> Running`. Explicit shutdown performs one
/// `Running -> Draining` transition, after which new semantic admission is
/// refused while already-owned work retains a narrow settlement path.
/// `Quiescent` is published only after all counted subsystem admissions,
/// retained capability-owned processes, and the runtime's higher-level owners
/// have settled.
#[derive(Debug, Clone)]
pub struct ConversationLifecycle {
    inner: Arc<LifecycleInner>,
}

impl Default for ConversationLifecycle {
    fn default() -> Self {
        Self {
            inner: Arc::new(LifecycleInner {
                state: AtomicU8::new(ConversationLifecycleState::INACTIVE),
                admissions: AtomicUsize::new(0),
                commit_boundary: Mutex::new(()),
                changed: tokio::sync::Notify::new(),
            }),
        }
    }
}

impl ConversationLifecycle {
    /// Creates a fresh conversation lifecycle in the `Inactive` state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current lifecycle state.
    #[must_use]
    pub fn state(&self) -> ConversationLifecycleState {
        ConversationLifecycleState::from_raw(self.inner.state.load(Ordering::Acquire))
    }

    /// Whether new semantic work may currently be admitted.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state() == ConversationLifecycleState::Running
    }

    /// Whether activation has happened, including during or after drain.
    #[must_use]
    pub fn is_activated(&self) -> bool {
        self.state() != ConversationLifecycleState::Inactive
    }

    /// Performs the one `Inactive -> Running` transition.
    ///
    /// Returns `true` for exactly the one call that committed the
    /// transition and `false` for every concurrent or later call, which
    /// makes activation idempotent: a caller that receives `true` may
    /// perform exactly the one-time post-activation work (for example
    /// spawning the admission worker), and a caller that receives `false`
    /// must not.
    ///
    /// # Panics
    ///
    /// Panics only if the lifecycle commit boundary is poisoned, which would
    /// mean a previous lifecycle critical section panicked.
    #[must_use]
    pub fn activate(&self) -> bool {
        let _boundary = self
            .inner
            .commit_boundary
            .lock()
            .expect("lifecycle commit boundary poisoned");
        self.inner
            .state
            .compare_exchange(
                ConversationLifecycleState::INACTIVE,
                ConversationLifecycleState::RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Linearizes the one `Running -> Draining` transition.
    ///
    /// Returns `true` only for the caller that closes semantic admission.
    /// Repeated callers observe the already-draining or quiescent lifecycle
    /// and converge on the same drain operation.
    #[must_use]
    pub(crate) fn begin_drain(&self) -> bool {
        let _boundary = self
            .inner
            .commit_boundary
            .lock()
            .expect("lifecycle commit boundary poisoned");
        self.inner
            .state
            .compare_exchange(
                ConversationLifecycleState::RUNNING,
                ConversationLifecycleState::DRAINING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Runs one concrete non-coordinator semantic commit while holding the
    /// lifecycle commit boundary. The drain transition cannot linearize
    /// between the `Running` observation and the operation's authoritative
    /// commit. The returned admission is also counted so quiescence waits
    /// for the operation's publication handoff.
    pub(crate) fn with_running_commit<T>(
        &self,
        operation: impl FnOnce() -> T,
    ) -> Result<T, ConversationLifecycleState> {
        let _boundary = self
            .inner
            .commit_boundary
            .lock()
            .expect("lifecycle commit boundary poisoned");
        if self.state() != ConversationLifecycleState::Running {
            return Err(self.state());
        }
        self.inner.admissions.fetch_add(1, Ordering::AcqRel);
        let admission = LifecycleAdmission {
            inner: Arc::clone(&self.inner),
        };
        let result = operation();
        drop(admission);
        Ok(result)
    }

    /// Admits one concrete runtime-owned operation and lets that operation
    /// retain the counted admission guard while it publishes its owned
    /// state.
    ///
    /// The operation runs under the same commit boundary as
    /// `Running -> Draining`. This is the stronger form needed when a
    /// publication must be visible to its owner before drain is allowed to
    /// scan and settle already-admitted work: the interaction coordinator
    /// inserts its pending entry while this boundary is held, then retains
    /// the guard until the waiter releases callback authority.
    pub(crate) fn admit_running_commit<T>(
        &self,
        operation: impl FnOnce(LifecycleAdmission) -> T,
    ) -> Result<T, ConversationLifecycleState> {
        let _boundary = self
            .inner
            .commit_boundary
            .lock()
            .expect("lifecycle commit boundary poisoned");
        if self.state() != ConversationLifecycleState::Running {
            return Err(self.state());
        }
        self.inner.admissions.fetch_add(1, Ordering::AcqRel);
        let admission = LifecycleAdmission {
            inner: Arc::clone(&self.inner),
        };
        Ok(operation(admission))
    }

    /// Enters a normal semantic commit boundary.
    ///
    /// The state is checked both before and after incrementing the admission
    /// count. If drain wins between those observations, the operation is
    /// refused and its count is removed before the guard is returned.
    pub(crate) fn try_enter_running(
        &self,
    ) -> Result<LifecycleAdmission, ConversationLifecycleState> {
        if self.state() != ConversationLifecycleState::Running {
            return Err(self.state());
        }
        self.inner.admissions.fetch_add(1, Ordering::AcqRel);
        if self.state() == ConversationLifecycleState::Running {
            return Ok(LifecycleAdmission {
                inner: Arc::clone(&self.inner),
            });
        }
        self.inner.admissions.fetch_sub(1, Ordering::AcqRel);
        self.inner.changed.notify_waiters();
        Err(self.state())
    }

    /// Enters capability preparation during composition or normal runtime
    /// execution. Preparation is allowed while `Inactive` because it is a
    /// composition operation rather than semantic admission; the counted
    /// guard still keeps an activation/drain race from escaping quiescence.
    /// Once `Draining` begins, no new preparation may start.
    pub(crate) fn try_enter_preparation(
        &self,
    ) -> Result<LifecycleAdmission, ConversationLifecycleState> {
        let state = self.state();
        if !matches!(
            state,
            ConversationLifecycleState::Inactive | ConversationLifecycleState::Running
        ) {
            return Err(state);
        }
        self.inner.admissions.fetch_add(1, Ordering::AcqRel);
        let after = self.state();
        if matches!(
            after,
            ConversationLifecycleState::Inactive | ConversationLifecycleState::Running
        ) {
            return Ok(LifecycleAdmission {
                inner: Arc::clone(&self.inner),
            });
        }
        self.inner.admissions.fetch_sub(1, Ordering::AcqRel);
        self.inner.changed.notify_waiters();
        Err(after)
    }

    /// Enters the narrow settlement path of an already-owned operation.
    ///
    /// Settlement is allowed while `Running` or `Draining`, but never after
    /// `Quiescent` (and never during pre-activation composition).
    pub(crate) fn try_enter_settlement(
        &self,
    ) -> Result<LifecycleAdmission, ConversationLifecycleState> {
        // Settlement entry shares the native commit boundary with drain
        // publication. Without this short lock, a settlement caller could
        // observe `Draining` and increment `admissions` after
        // `mark_quiescent` had already checked zero but before its CAS to
        // `Quiescent`, allowing a real callback to begin after quiescence.
        let _boundary = self
            .inner
            .commit_boundary
            .lock()
            .expect("lifecycle commit boundary poisoned");
        let state = self.state();
        if !matches!(
            state,
            ConversationLifecycleState::Running | ConversationLifecycleState::Draining
        ) {
            return Err(state);
        }
        self.inner.admissions.fetch_add(1, Ordering::AcqRel);
        Ok(LifecycleAdmission {
            inner: Arc::clone(&self.inner),
        })
    }

    /// Attempts to publish `Quiescent` after all counted admissions settle.
    #[must_use]
    pub(crate) fn mark_quiescent(&self) -> bool {
        let _boundary = self
            .inner
            .commit_boundary
            .lock()
            .expect("lifecycle commit boundary poisoned");
        if self.inner.admissions.load(Ordering::Acquire) != 0 {
            return false;
        }
        let changed = self
            .inner
            .state
            .compare_exchange(
                ConversationLifecycleState::DRAINING,
                ConversationLifecycleState::QUIESCENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if changed {
            self.inner.changed.notify_waiters();
        }
        changed
    }

    /// Waits until no counted subsystem operation remains in flight.
    pub(crate) async fn wait_for_no_admissions(&self) {
        loop {
            if self.inner.admissions.load(Ordering::Acquire) == 0 {
                return;
            }
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.admissions.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Waits until the lifecycle has reached its terminal quiescent state.
    pub(crate) async fn wait_until_quiescent(&self) {
        loop {
            if self.state() == ConversationLifecycleState::Quiescent {
                return;
            }
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state() == ConversationLifecycleState::Quiescent {
                return;
            }
            notified.await;
        }
    }
}

/// The runtime-owned durability frontier shared with conversation-owned
/// durable semantic ownership commits (Issue #60).
///
/// `ConversationRuntime` owns the authoritative durable-health state
/// (`DurabilityHealth` under the coordinator lock). This gate is the single
/// synchronization frontier through which that fact reaches the
/// conversation-owned registries: it carries the same failed fact at the
/// same commit point (updated by the coordinator's durability-failure
/// commit) and serializes new conversation-owned durable ownership commits
/// (subagent and background) against the `DurabilityFailed` commit, so the
/// two have one deterministic total order:
///
/// - a new ownership commit holds the gate **across** its durable
///   ownership write and record publication — a failure that wins the gate
///   first makes the commit refuse (and its staged child roll back), while
///   an ownership that wins first is already durably owned before the
///   failure can be published;
/// - settlement of already-owned work never acquires the gate: `DurabilityFailed`
///   closes **new** semantic mutation only, never settlement.
///
/// `DurabilityFailed` is deliberately not a lifecycle state: a degraded
/// durable authority and a shutting-down runtime are different dimensions,
/// and this gate keeps them separate.
#[derive(Debug, Default)]
pub(crate) struct DurabilityGate {
    state: Mutex<DurabilityGateState>,
}

#[derive(Debug, Default)]
struct DurabilityGateState {
    /// Whether the owning runtime committed `DurabilityFailed`.
    failed: bool,
    /// The bounded failure diagnostic of that commit.
    diagnostic: Option<String>,
}

impl DurabilityGate {
    /// Creates a fresh, healthy frontier.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Commits the `DurabilityFailed` fact into the shared frontier.
    ///
    /// Called by the owning runtime's durability-failure commit under the
    /// coordinator lock. New ownership commits serialize on this same gate,
    /// so this acquisition is the linearization point against any new
    /// conversation-owned durable ownership commit.
    pub(crate) fn mark_failed(&self, diagnostic: String) {
        let mut state = self.state.lock().expect("durability gate lock poisoned");
        state.failed = true;
        state.diagnostic = Some(diagnostic);
    }

    /// Acquires the ownership-commit permission for one new
    /// conversation-owned durable ownership commit.
    ///
    /// The returned guard must be held across the ownership durable write
    /// and (for the registries) the record publication: while it is held,
    /// the `DurabilityFailed` commit blocks on the same gate, so the two
    /// operations have one total order and no check-then-write window
    /// exists. Returns [`OwnershipCommitRefused`] when the owning runtime
    /// already committed `DurabilityFailed`.
    pub(crate) fn enter_ownership_commit(
        &self,
    ) -> Result<OwnershipCommitGuard<'_>, OwnershipCommitRefused> {
        let state = self.state.lock().expect("durability gate lock poisoned");
        if state.failed {
            return Err(OwnershipCommitRefused {
                diagnostic: state
                    .diagnostic
                    .clone()
                    .unwrap_or_else(|| "the conversation durable authority failed".to_owned()),
            });
        }
        Ok(OwnershipCommitGuard { _gate: state })
    }
}

/// The short-lived permission of one new conversation-owned durable
/// ownership commit.
///
/// Held across the ownership durable write and record publication, it is
/// what gives the `DurabilityFailed` commit and the ownership commit one
/// deterministic total order on the [`DurabilityGate`].
pub(crate) struct OwnershipCommitGuard<'a> {
    _gate: MutexGuard<'a, DurabilityGateState>,
}

/// A new conversation-owned durable ownership commit refused because the
/// owning runtime's durable authority is in the explicit failed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnershipCommitRefused {
    /// The owning runtime's bounded failure diagnostic.
    pub(crate) diagnostic: String,
}

/// A token measurement of a model input, with explicit provenance.
///
/// This is a Layer 0 value contract. The Context Engine owns the accounting
/// behavior that decides when a measurement is valid, but the measurement
/// itself is shared by runtime events, context projections, and the Runtime
/// Client read model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenMeasurement {
    /// The measured or estimated input token count.
    pub input_tokens: u64,
    /// How the measurement was obtained.
    pub source: TokenMeasurementSource,
}

/// Where a [`TokenMeasurement`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenMeasurementSource {
    /// The provider reported usage for exactly this projection
    /// (`ModelUsage.input_tokens` of the completed request). Never
    /// fabricated, never a sum of cumulative snapshots.
    ProviderReported,
    /// A deterministic runtime-owned estimate.
    Estimated,
}

/// Why an operation was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    /// The user requested cancellation of the attempt or its work.
    UserRequested,
    /// The conversation runtime is shutting down.
    RuntimeShutdown,
    /// A parent operation that owns this work was cancelled.
    ParentCancelled,
}

/// A normalized runtime-owned execution error.
///
/// `RuntimeError` describes failures of the runtime itself, distinct from
/// normalized model errors (`ModelError`) and tool execution statuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeError {
    /// An unexpected internal failure with no further classification.
    Internal {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The runtime reached a state it should not be in.
    InvalidState {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The requested operation is not supported by this runtime.
    Unsupported {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The model requested a tool that is not present in the attempt's
    /// immutable tool registry. No tool result exists for the request, so
    /// the runtime fails explicitly instead of fabricating one.
    UnknownTool {
        /// The tool name the model called.
        name: String,
    },
    /// The durable canonical authority (the durable Message Ledger /
    /// Pending Inbound Inbox) rejected a required commit of the active
    /// attempt. The attempt settles failed, and the owning conversation
    /// runtime records the durable-authority failure so it cannot return
    /// to a false healthy state and admit further work as though storage
    /// were fine (Issue #63).
    DurableStore {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The canonical model stream violated its contract (for example a
    /// non-terminal event after the terminal event, or a tool-call delta
    /// referencing an unknown call). The runtime rejects the stream
    /// explicitly instead of silently accepting impossible state.
    ContractViolation {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// Context preparation failed while building the model context of a
    /// request **before any compaction started**: an invalid pending
    /// fresh-inbound state discovered during projection/status preparation,
    /// a failing Agent Status section provider, or a projection preparation
    /// failure that is not itself a compaction operation. This is never
    /// mislabeled as a compaction failure.
    ContextPreparationFailed {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// Context compaction failed. This is a runtime-owned context-plane
    /// failure of an actual proactive compaction pipeline: it never
    /// fabricates a provider error, and it is distinct from a normalized
    /// model error even when the two coincide (for example a context
    /// overflow whose recovery compaction failed). Preparation failures that
    /// occur before compaction starts are [`RuntimeError::ContextPreparationFailed`]
    /// instead.
    ContextCompactionFailed {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The attempt-level pre-step policy rejected the proposed model step
    /// (Issue #56). The rejection happens strictly before the admission
    /// linearization point, so no proposed dynamic context became canonical,
    /// no Surface revision advanced because of it, no `RequestSnapshot` was
    /// frozen, and no provider request started.
    PreStepRejected {
        /// The policy's bounded rejection reason.
        reason: String,
    },
    /// The attempt-level pre-step policy itself failed while evaluating the
    /// final immutable proposal batch (Issue #56). Like a rejection, this
    /// settles the attempt before admission, so no partial context admission
    /// exists.
    PreStepPolicyFailed {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The attempt-level tool-result observer failed (Issue #56). The
    /// observation pass runs strictly **after** the owning tool batch reached
    /// structural settlement, so the committed Assistant tool-call message and
    /// its complete canonical `ToolMessage` batch are unaffected; only the
    /// deferred context of the failed observation pass is discarded.
    ToolResultObservationFailed {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The owning runtime process restarted while this attempt was durably
    /// non-terminal, so startup recovery settled it (Issue #12, M9a).
    ///
    /// This error states exactly what rustX knows: the attempt's process-local
    /// execution state is gone and the attempt can never continue. It is
    /// **not** a claim about any external work the attempt had started. When a
    /// model request or a tool execution had already crossed its durable start
    /// commit, that external outcome stays explicitly unknown — recovery
    /// records the indeterminate outcome instead of inventing a provider
    /// failure, and it never resends or re-executes anything.
    RestartInterrupted {
        /// The bounded recovery diagnostic: which durable evidence settled
        /// the attempt and what remained indeterminate.
        message: String,
    },
    /// A tool-result observation pass produced deferred context that violates
    /// the bounded deferred-context contract (Issue #56).
    ///
    /// The check runs at the observer transaction boundary, before anything is
    /// staged, so the whole observation pass is discarded and no partial
    /// deferred state survives. Like every observation failure this happens
    /// after structural settlement, so the committed Assistant tool-call
    /// message keeps its complete canonical `ToolMessage` batch.
    DeferredContextRejected {
        /// Human-readable diagnostic message.
        message: String,
    },
}

/// A runtime-owned UTC clock boundary.
///
/// State-machine code that must stamp deterministic timestamps (for example
/// background terminal inbound messages) goes through this narrow
/// abstraction; no production code calls `Utc::now()` directly in testable
/// state-machine code. Tests use a fixed/scripted clock so snapshots are
/// deterministic.
pub trait RuntimeClock: Send + Sync {
    /// The current UTC instant.
    fn now(&self) -> DateTime<Utc>;
}

/// The production clock: system UTC time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl RuntimeClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CancellationReason, ConversationLifecycle, ConversationLifecycleState, RuntimeClock,
        RuntimeError, SystemClock,
    };

    /// The shared lifecycle has one monotonic activation/drain path, normal
    /// admission closes at drain, and quiescence waits for the last counted
    /// settlement owner to leave its native boundary.
    #[tokio::test]
    async fn lifecycle_closes_admission_before_publishing_quiescence() {
        let lifecycle = ConversationLifecycle::new();
        assert_eq!(lifecycle.state(), ConversationLifecycleState::Inactive);
        assert!(lifecycle.activate());
        assert_eq!(lifecycle.state(), ConversationLifecycleState::Running);

        let admission = lifecycle
            .try_enter_running()
            .expect("running lifecycle admits semantic work");
        assert!(lifecycle.begin_drain());
        assert_eq!(lifecycle.state(), ConversationLifecycleState::Draining);
        assert!(lifecycle.try_enter_running().is_err());
        assert!(lifecycle.try_enter_settlement().is_ok());
        assert!(!lifecycle.mark_quiescent());

        drop(admission);
        lifecycle.wait_for_no_admissions().await;
        assert!(lifecycle.mark_quiescent());
        assert_eq!(lifecycle.state(), ConversationLifecycleState::Quiescent);
        assert!(!lifecycle.begin_drain());
        assert!(lifecycle.try_enter_settlement().is_err());
        lifecycle.wait_until_quiescent().await;
    }

    /// Cancellation reasons use a stable serialized representation.
    #[test]
    fn cancellation_reason_round_trip() {
        let value = CancellationReason::ParentCancelled;
        let json = serde_json::to_string(&value).expect("serialize reason");
        assert_eq!(json, "\"parent_cancelled\"");
        let decoded: CancellationReason = serde_json::from_str(&json).expect("deserialize reason");
        assert_eq!(decoded, value);
    }

    /// The system clock returns a valid UTC instant.
    #[test]
    fn system_clock_reports_utc_instants() {
        let instant = SystemClock.now();
        assert!(instant.timestamp() > 0, "a real UTC instant is reported");
    }

    /// Runtime errors round-trip with an explicit discriminator.
    #[test]
    fn runtime_error_round_trip() {
        let value = RuntimeError::InvalidState {
            message: "attempt already finished".to_owned(),
        };
        let json = serde_json::to_string(&value).expect("serialize error");
        let decoded: RuntimeError = serde_json::from_str(&json).expect("deserialize error");
        assert_eq!(decoded, value);
    }

    /// Tool-resolution and stream-contract errors have stable discriminators.
    #[test]
    fn runtime_error_discriminators_are_stable() {
        let cases = [
            (
                RuntimeError::UnknownTool {
                    name: "missing".to_owned(),
                },
                "unknown_tool",
            ),
            (
                RuntimeError::ContractViolation {
                    message: "event after terminal".to_owned(),
                },
                "contract_violation",
            ),
            (
                RuntimeError::ContextPreparationFailed {
                    message: "status provider failed".to_owned(),
                },
                "context_preparation_failed",
            ),
            (
                RuntimeError::ContextCompactionFailed {
                    message: "no progress".to_owned(),
                },
                "context_compaction_failed",
            ),
            (
                RuntimeError::PreStepRejected {
                    reason: "policy rejected the step".to_owned(),
                },
                "pre_step_rejected",
            ),
            (
                RuntimeError::PreStepPolicyFailed {
                    message: "policy failed".to_owned(),
                },
                "pre_step_policy_failed",
            ),
            (
                RuntimeError::ToolResultObservationFailed {
                    message: "observer failed".to_owned(),
                },
                "tool_result_observation_failed",
            ),
        ];
        for (error, expected) in cases {
            let value = serde_json::to_value(&error).expect("serialize error");
            assert_eq!(value["type"], expected);
            let decoded: RuntimeError = serde_json::from_value(value).expect("deserialize error");
            assert_eq!(decoded, error);
        }
    }
}
