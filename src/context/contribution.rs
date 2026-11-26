//! Contribution vocabulary lives in `rustx-types`; engine plumbing stays
//! here because it notifies the agent execution observer.

pub use rustx_types::context::contribution::*;

/// Engine-side observation notification for a contribution start.
pub(crate) fn observe_contribution(
    start: &ContributionStart,
    observer: &dyn crate::agent::AgentExecutionObserver,
    attempt: &crate::runtime::identity::AttemptId,
    turn: u32,
    source: Option<&crate::durable::StatusObservationSource>,
) {
    observer.observe_contribution_status(start, attempt, turn, source);
}
