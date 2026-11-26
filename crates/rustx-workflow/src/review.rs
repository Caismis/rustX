//! Human-review node vocabulary.

use serde::{Deserialize, Serialize};

/// The maximum number of review actions one Human node may declare.
pub const MAX_REVIEW_ACTIONS: usize = rustx_types::events::interaction::MAX_CHOICE_OPTIONS;
/// The reserved port emitted when a review times out.
pub const REVIEW_TIMEOUT_PORT: &str = "timeout";

/// One button the person may press, and the exit it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReviewAction {
    pub id: String,
    pub title: String,
}
