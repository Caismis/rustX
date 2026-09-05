//! Generic foreground Tool-execution liveness deadlines (Issue #204).
//!
//! One admitted foreground tool execution is bounded by at most two
//! deadlines, both owned by the Agent Loop's generic execution lifecycle —
//! never independently by an executor:
//!
//! - the **hard deadline** bounds the total execution lifetime, measured
//!   from the executor-start frontier of the call. It is immutable for the
//!   admitted execution: progress evidence never extends it.
//! - the **idle-liveness deadline** bounds the time an execution may go
//!   without meaningful executor progress evidence. Every progress report
//!   refreshes it. It applies only to executions whose executor declares
//!   [`ToolProgressCapability::Meaningful`]: an executor that cannot produce
//!   honest progress runs with the hard deadline only and must never invent
//!   heartbeat reports.
//!
//! A deadline expiring is cancellation/liveness **intent**, not proof that
//! external execution stopped (Issue #202). The lifecycle requests
//! executor-owned physical cancellation and then awaits the executor's
//! settlement evidence, bounded by [`TOOL_SETTLEMENT_CONFIRMATION`]. The
//! settled evidence selects the canonical status: proven terminal settlement
//! after a deadline winner is [`ToolExecutionStatus::TimedOut`], an
//! executor-proven `OutcomeUnknown` stays unknown, and a normal completion
//! that won the physical race inside the executor survives untouched. When
//! the executor's execution future does not return within the confirmation
//! window, the lifecycle has exhausted the accepted executor-owned
//! settlement mechanism without proving terminality: the canonical result is
//! `OutcomeUnknown` — never `TimedOut`, because "the execution future did
//! not return" is not "physical execution definitely stopped" — and the
//! lifecycle drops the uncooperative future so the Agent Loop stays live.
//!
//! [`ToolExecutionStatus::TimedOut`]: crate::tools::types::ToolExecutionStatus::TimedOut

use std::time::Duration;

/// The hard execution deadline used when a runtime configuration does not
/// provide another value.
pub const DEFAULT_TOOL_HARD_DEADLINE: Duration = Duration::from_mins(2);

/// The bounded window the generic lifecycle awaits an executor's physical
/// settlement evidence after cancellation intent (a deadline winner or
/// attempt cancellation) was delivered to the execution (Issue #204).
///
/// This is not a second execution deadline and its expiry is never
/// `TimedOut`: it bounds only the wait for *settlement evidence* from an
/// executor that was already asked to cancel. Cooperative executors settle
/// promptly inside their own bounded physical ladders (for example Bash's
/// process-group kill, wait, and reap, bounded well below this window). An
/// executor whose execution future still has not returned when the window
/// expires has crossed — or may have crossed — the external-effect frontier
/// without provable terminality, so the lifecycle commits `OutcomeUnknown`
/// and drops the uncooperative future rather than blocking the Agent Loop
/// forever.
pub const TOOL_SETTLEMENT_CONFIRMATION: Duration = Duration::from_secs(30);

/// The immutable execution-liveness policy of one runtime.
///
/// The policy is current launch configuration, never model input, durable
/// state, or executor-owned data. It is copied into the admitted attempt's
/// execution authority at admission, so a running `ToolCall` can never
/// observe a later policy value, and it is inherited by subagent children
/// through their typed startup specification exactly like the model timeout
/// policy (Issue #138). There is deliberately one generic policy for every
/// foreground tool: no executor-specific deadline knobs exist in the Agent
/// Loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolExecutionDeadlinePolicy {
    /// The total maximum execution lifetime of one admitted foreground call,
    /// measured from its executor-start frontier. Progress never extends it.
    pub hard_deadline: Duration,
    /// The maximum time one started execution may go without meaningful
    /// progress evidence; each report starts a fresh idle window. `None`
    /// disables the idle watchdog. The window is additionally gated by the
    /// admitted executor's [`ToolProgressCapability`]: it applies only to
    /// executions whose executor can emit meaningful progress evidence.
    pub idle_liveness: Option<Duration>,
}

impl Default for ToolExecutionDeadlinePolicy {
    fn default() -> Self {
        Self {
            hard_deadline: DEFAULT_TOOL_HARD_DEADLINE,
            idle_liveness: None,
        }
    }
}

impl ToolExecutionDeadlinePolicy {
    /// Creates one finite execution-liveness policy.
    #[must_use]
    pub const fn new(hard_deadline: Duration, idle_liveness: Option<Duration>) -> Self {
        Self {
            hard_deadline,
            idle_liveness,
        }
    }

    /// Whether every configured deadline can make progress.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        !self.hard_deadline.is_zero()
            && match self.idle_liveness {
                Some(idle) => !idle.is_zero(),
                None => true,
            }
    }

    /// The absolute hard deadline of an execution that crossed its
    /// executor-start frontier at `started_millis` on the runtime monotonic
    /// clock. Immutable after start: progress evidence never extends the
    /// total execution lifetime.
    #[must_use]
    pub fn hard_deadline_millis(&self, started_millis: u64) -> u64 {
        deadline_after(started_millis, self.hard_deadline)
    }

    /// The effective idle-liveness window for one admitted execution
    /// (Issue #204): the configured policy window applies exactly when the
    /// admitted executor declared [`ToolProgressCapability::Meaningful`] at
    /// invocation resolution. An executor without meaningful progress
    /// evidence runs under the hard deadline only — the runtime never
    /// imposes an idle timeout it could only satisfy by fabricating
    /// heartbeats.
    #[must_use]
    pub fn effective_idle_liveness(&self, capability: ToolProgressCapability) -> Option<Duration> {
        match (self.idle_liveness, capability) {
            (Some(idle), ToolProgressCapability::Meaningful) => Some(idle),
            _ => None,
        }
    }
}

/// Whether an executor can emit meaningful progress evidence for one
/// execution (Issue #204).
///
/// This is explicit typed executor state, declared by the executor itself
/// and frozen into the admitted invocation at resolution: it is never
/// inferred from whether progress happened yet, from the executor's name or
/// origin, or from timing. The capability gates only the idle-liveness
/// watchdog; the hard deadline applies to every admitted execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProgressCapability {
    /// The executor has no meaningful progress protocol. Its executions run
    /// under the hard deadline only, even when the runtime policy configures
    /// an idle-liveness window; the executor must never fabricate heartbeat
    /// reports to satisfy a watchdog it cannot honestly inform.
    None,
    /// The executor can emit semantically meaningful liveness evidence
    /// through the execution's progress reporter (for example an MCP
    /// executor forwarding genuine remote progress notifications), so a
    /// configured idle-liveness window applies to its executions.
    Meaningful,
}

/// The generic liveness deadline that fired for one started execution.
///
/// The kind is observational evidence: it records *which* intent fired, not
/// the settlement outcome. The canonical outcome is selected from the
/// executor's settlement evidence at the terminal result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDeadlineKind {
    /// The total execution-lifetime bound expired.
    Hard,
    /// The idle-liveness bound expired without progress evidence.
    Idle,
}

/// The absolute monotonic-clock deadline `duration` after `now_millis`,
/// saturating at the representable maximum.
pub(crate) fn deadline_after(now_millis: u64, duration: Duration) -> u64 {
    let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    now_millis.saturating_add(millis)
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TOOL_HARD_DEADLINE, TOOL_SETTLEMENT_CONFIRMATION, ToolDeadlineKind,
        ToolExecutionDeadlinePolicy, ToolProgressCapability, deadline_after,
    };
    use std::time::Duration;

    fn policy() -> ToolExecutionDeadlinePolicy {
        ToolExecutionDeadlinePolicy::new(
            Duration::from_millis(100),
            Some(Duration::from_millis(10)),
        )
    }

    /// The default is a finite hard deadline without an idle watchdog.
    #[test]
    fn default_policy_is_hard_only() {
        let policy = ToolExecutionDeadlinePolicy::default();
        assert_eq!(policy.hard_deadline, DEFAULT_TOOL_HARD_DEADLINE);
        assert_eq!(policy.idle_liveness, None);
        assert!(policy.is_positive());
    }

    /// Zero deadlines are rejected by the policy's own validity rule.
    #[test]
    fn zero_deadlines_are_not_positive() {
        assert!(
            !ToolExecutionDeadlinePolicy::new(Duration::ZERO, None).is_positive(),
            "a zero hard deadline can never bound an execution"
        );
        assert!(
            !ToolExecutionDeadlinePolicy::new(Duration::from_millis(1), Some(Duration::ZERO))
                .is_positive(),
            "a zero idle deadline would fire at every observation"
        );
    }

    /// The settlement-confirmation window is finite and comfortably exceeds
    /// the bounded physical settlement ladders of cooperative executors.
    #[test]
    fn settlement_confirmation_is_finite_and_generous() {
        assert!(TOOL_SETTLEMENT_CONFIRMATION >= Duration::from_secs(10));
        assert!(TOOL_SETTLEMENT_CONFIRMATION <= Duration::from_mins(5));
    }

    /// The hard deadline is measured from the executor-start frontier and
    /// never moves.
    #[test]
    fn hard_deadline_starts_at_the_executor_start_frontier() {
        assert_eq!(policy().hard_deadline_millis(40), 140);
        assert_eq!(
            deadline_after(u64::MAX - 1, Duration::from_millis(5)),
            u64::MAX
        );
    }

    /// The idle-liveness window applies only to executors that declared
    /// meaningful progress evidence (Issue #204, capability gating).
    #[test]
    fn effective_idle_liveness_is_capability_gated() {
        let policy = policy();
        assert_eq!(
            policy.effective_idle_liveness(ToolProgressCapability::Meaningful),
            Some(Duration::from_millis(10)),
            "configured idle policy plus meaningful progress capability enables the watchdog"
        );
        assert_eq!(
            policy.effective_idle_liveness(ToolProgressCapability::None),
            None,
            "an executor without meaningful progress runs hard-deadline-only"
        );
        let hard_only = ToolExecutionDeadlinePolicy::default();
        assert_eq!(
            hard_only.effective_idle_liveness(ToolProgressCapability::Meaningful),
            None,
            "an absent idle policy never enables the watchdog"
        );
        assert_eq!(
            hard_only.effective_idle_liveness(ToolProgressCapability::None),
            None
        );
    }

    /// The deadline kind vocabulary round-trips through its durable serde
    /// representation.
    #[test]
    fn deadline_kind_round_trips() {
        for kind in [ToolDeadlineKind::Hard, ToolDeadlineKind::Idle] {
            let json = serde_json::to_string(&kind).expect("serialize kind");
            let decoded: ToolDeadlineKind = serde_json::from_str(&json).expect("deserialize kind");
            assert_eq!(decoded, kind);
        }
    }
}
