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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_tool_opportunity_preserves_presence_through_json() {
        for post_tool_batch in [None, Some(PostToolBatchOpportunity {})] {
            let opportunities = ContributionOpportunities {
                fresh_inbound: None,
                post_tool_batch,
            };
            let json = serde_json::to_value(&opportunities).unwrap();
            assert_eq!(
                json["post_tool_batch"].is_object(),
                post_tool_batch.is_some()
            );
            assert_eq!(
                serde_json::from_value::<ContributionOpportunities>(json).unwrap(),
                opportunities
            );
        }
    }
}
