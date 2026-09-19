//! Immutable response provenance carried by native lineage bootstrap.
//!
//! These facts recognize finalized inherited history. They neither authorize
//! execution recovery nor claim the destination ran the origin's Attempt.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::ModelUsage;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};

/// Original execution owner, unchanged through arbitrarily deep lineage copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResponseOrigin {
    pub conversation_id: ConversationId,
    pub attempt_id: AttemptId,
    /// Provenance only; never a destination content address.
    pub closing_message_id: MessageId,
}

/// Lineage-safe projection of authoritative finalized response evidence.
/// Content and Retry addresses belong to the destination; origin does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedResponseProvenance {
    pub closing_message_id: MessageId,
    pub origin: ResponseOrigin,
    pub completed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_message_id: Option<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<CompletedResponseTiming>,
}

/// Historical product timing derived from native lifecycle and generation evidence.
/// Missing evidence stays absent; these are not destination execution facts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedResponseTiming {
    /// Authoritative successful Attempt completion minus its start timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total_duration_ms: Option<u64>,
    /// First actual request's adapter-dispatch-to-first-output duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub ttft_ms: Option<u64>,
    /// Sum of output-producing requests' first-output-to-provider-terminal spans.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub generation_ms: Option<u64>,
    /// Fully covered output usage divided by fully covered positive generation work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_per_second: Option<f64>,
}
