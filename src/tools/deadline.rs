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
//!   refreshes it. Executors that cannot produce honest progress run with
//!   the hard deadline only; they must never invent heartbeat reports.
//!
//! A deadline expiring is cancellation/liveness **intent**, not proof that
//! external execution stopped (Issue #202). The lifecycle requests
//! executor-owned physical cancellation and classifies the settled result by
//! its evidence: proven terminal settlement after a deadline winner is
//! [`ToolExecutionStatus::TimedOut`], an executor-proven `OutcomeUnknown`
//! stays unknown, and a normal completion that won the physical race inside
//! the executor survives untouched. Dropping an executor future is never
//! settlement.
//!
//! [`ToolExecutionStatus::TimedOut`]: crate::tools::types::ToolExecutionStatus::TimedOut

use std::time::Duration;

/// The hard execution deadline used when a runtime configuration does not
/// provide another value.
pub const DEFAULT_TOOL_HARD_DEADLINE: Duration = Duration::from_mins(2);

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
    /// disables the idle watchdog, which is the honest configuration for
    /// executors that cannot produce progress evidence.
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

/// The execution-local liveness state of one started foreground call.
///
/// The state is pure monotonic-clock arithmetic: the owning Agent Loop reads
/// the runtime clock, feeds progress observations, and owns all waiting and
/// winner arbitration itself, mirroring
/// [`crate::model::deadline::ModelRequestDeadline`]. The executor-start
/// frontier is the one clock-reading frontier of both deadlines: the hard
/// deadline of an admitted call is fixed the moment the lifecycle admits the
/// invocation to its executor, and queueing/scheduling time before that
/// frontier is attempt lifecycle, never execution lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolExecutionLiveness {
    policy: ToolExecutionDeadlinePolicy,
    started_millis: u64,
    last_progress_millis: u64,
}

impl ToolExecutionLiveness {
    /// Starts both deadlines at the executor-start frontier.
    #[must_use]
    pub const fn new(policy: ToolExecutionDeadlinePolicy, now_millis: u64) -> Self {
        Self {
            policy,
            started_millis: now_millis,
            last_progress_millis: now_millis,
        }
    }

    /// Records meaningful executor progress evidence.
    ///
    /// Progress refreshes the idle-liveness deadline only. It can never
    /// extend the hard deadline: the total execution lifetime is fixed at
    /// the start frontier.
    pub const fn observe_progress(&mut self, now_millis: u64) {
        self.last_progress_millis = now_millis;
    }

    /// The absolute hard deadline of this execution. Immutable after start.
    #[must_use]
    pub fn hard_deadline_millis(&self) -> u64 {
        deadline_after(self.started_millis, self.policy.hard_deadline)
    }

    /// The absolute idle deadline from the latest progress evidence, when
    /// the policy enables the idle watchdog.
    #[must_use]
    pub fn idle_deadline_millis(&self) -> Option<u64> {
        self.policy
            .idle_liveness
            .map(|idle| deadline_after(self.last_progress_millis, idle))
    }
}

fn deadline_after(now_millis: u64, duration: Duration) -> u64 {
    let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    now_millis.saturating_add(millis)
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TOOL_HARD_DEADLINE, ToolDeadlineKind, ToolExecutionDeadlinePolicy,
        ToolExecutionLiveness,
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

    /// Both deadlines are measured from the executor-start frontier.
    #[test]
    fn deadlines_start_at_the_executor_start_frontier() {
        let liveness = ToolExecutionLiveness::new(policy(), 40);
        assert_eq!(liveness.hard_deadline_millis(), 140);
        assert_eq!(liveness.idle_deadline_millis(), Some(50));
    }

    /// Progress refreshes the idle deadline but never the hard deadline.
    #[test]
    fn progress_refreshes_idle_only() {
        let mut liveness = ToolExecutionLiveness::new(policy(), 0);
        liveness.observe_progress(90);
        assert_eq!(liveness.idle_deadline_millis(), Some(100));
        assert_eq!(
            liveness.hard_deadline_millis(),
            100,
            "progress must never extend the total execution lifetime"
        );
        liveness.observe_progress(99);
        assert_eq!(liveness.idle_deadline_millis(), Some(109));
        assert_eq!(liveness.hard_deadline_millis(), 100);
    }

    /// Without an idle policy the watchdog is absent, not faked.
    #[test]
    fn hard_only_policy_has_no_idle_deadline() {
        let mut liveness = ToolExecutionLiveness::new(ToolExecutionDeadlinePolicy::default(), 7);
        assert_eq!(liveness.idle_deadline_millis(), None);
        liveness.observe_progress(50);
        assert_eq!(liveness.idle_deadline_millis(), None);
        assert_eq!(
            liveness.hard_deadline_millis(),
            7 + u64::try_from(DEFAULT_TOOL_HARD_DEADLINE.as_millis()).expect("fits")
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
