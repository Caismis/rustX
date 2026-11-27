//! Logical-step contribution facts and immutable presentation data.

use serde::{Deserialize, Serialize};

use crate::context::status::AgentStatus;
use crate::durable::canonical::TranscriptCursor;
use crate::message::types::{ContextKind, ContributionEmission};
use crate::runtime::identity::MessageId;
use rustx_identity::ContextContributorIdentity;

/// Finite opportunities offered by an already scheduled logical step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionOpportunities {
    pub fresh_inbound: Option<FreshInboundOpportunity>,
    pub post_tool_batch: Option<PostToolBatchOpportunity>,
}

impl ContributionOpportunities {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.fresh_inbound.is_none() && self.post_tool_batch.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshInboundOpportunity {
    pub target_message_id: MessageId,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostToolBatchOpportunity {}

/// Closed typed presentation vocabulary; it carries no execution authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContributionPresentation {
    AgentStatus(AgentStatus),
}

/// One accepted contribution, bound to its canonical identity at staging.
/// Retries retain this value and select it by their actual Surface membership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributionStart {
    pub message_id: MessageId,
    pub producer: ContextContributorIdentity,
    pub metadata: ContextKind,
    pub presentation: Option<ContributionPresentation>,
    pub emissions: Vec<ContributionEmission>,
    pub opportunities: ContributionOpportunities,
    pub post_tool_batch_anchor: Option<TranscriptCursor>,
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
