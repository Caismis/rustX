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
    /// Exact retained Assistant members of this finalized Attempt, including
    /// its closing response. Remapped with canonical identities during lineage.
    pub process_message_ids: Vec<MessageId>,
    pub closing_message_id: MessageId,
    pub origin: ResponseOrigin,
    pub completed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_message_id: Option<MessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<CompletedResponseTiming>,
    /// Distinct provider-facing models of the Attempt's actual requests, in
    /// first-request order; never substituted from current configuration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
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

/// Whole-conversation work time. Unlike a completed response's exact
/// aggregate, each figure sums only the requests or Tool executions that
/// measured it, and is absent until the first one did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTiming {
    /// Summed dispatch-to-provider-terminal time of measured requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub model_ms: Option<u64>,
    /// Summed start-to-terminal time of settled foreground Tool executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub tool_ms: Option<u64>,
    /// Mean dispatch-to-first-output time over requests that produced output.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub mean_ttft_ms: Option<u64>,
    /// Output tokens over decode time, summed over requests that report
    /// usage and a positive decode span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_per_second: Option<f64>,
}

pub(crate) mod timing;

use crate::model::types::UsageDetails;
use crate::runtime::identity::RequestId;
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct AttemptEvidence {
    pub(crate) started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) timing: timing::TimingFold,
    pub(crate) closing: Option<MessageId>,
    pub(crate) members: BTreeSet<MessageId>,
    pub(crate) last_request: Option<RequestId>,
    pub(crate) models: Vec<String>,
    pub(crate) requests: u64,
    pub(crate) reports: u64,
    pub(crate) usage: Option<ModelUsage>,
}

impl AttemptEvidence {
    /// Records one actual request start of this Attempt.
    pub(crate) fn request_started(&mut self, request_id: RequestId, model: String) {
        self.timing.start(request_id.clone());
        self.last_request = Some(request_id);
        if !self.models.contains(&model) {
            self.models.push(model);
        }
        self.requests += 1;
    }
}

pub(crate) fn add_usage(total: &mut Option<ModelUsage>, usage: &ModelUsage) {
    let Some(total) = total.as_mut() else {
        *total = Some(usage.clone());
        return;
    };
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    let sum = |a: Option<u64>, b: Option<u64>| a.zip(b).map(|(a, b)| a.saturating_add(b));
    total.details = total
        .details
        .as_ref()
        .zip(usage.details.as_ref())
        .map(|(a, b)| UsageDetails {
            reasoning_tokens: sum(a.reasoning_tokens, b.reasoning_tokens),
            cached_input_tokens: sum(a.cached_input_tokens, b.cached_input_tokens),
        });
}
