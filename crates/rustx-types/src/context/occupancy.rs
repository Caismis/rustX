//! Context occupancy vocabulary shared across layers.

use serde::{Deserialize, Serialize};

/// Latest native context reading for this Conversation, with explicit provenance.
/// Unsent composer text is never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOccupancy {
    /// True for the native rebuilt-context estimate after compaction.
    #[serde(default)]
    pub estimated: bool,
    /// Normalized request input, or the committed rebuilt-context estimate.
    pub input_tokens: u64,
    /// Capacity frozen in the request or compaction's native model snapshot.
    pub context_window_tokens: u64,
    /// Provider-facing historical model, never substituted from current config.
    pub model: String,
    /// Heuristic composition of the same request's input.
    pub breakdown: ContextBreakdown,
}

impl ContextOccupancy {
    /// Rejects a reading whose context window is unknown.
    ///
    /// # Errors
    /// Returns the violation text; the durable store maps it to `InvalidReference`.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.context_window_tokens == 0 {
            return Err("an inherited occupancy reading has no valid context window");
        }
        Ok(())
    }
}

/// Where one request's measured input went, priced by the provider-neutral
/// `ceil(bytes / 4)` estimate. System prompt and Tool definitions are priced
/// heuristically; messages are the remainder of the measured or estimated total.
/// The parts sum to that total unless the component estimates alone exceed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextBreakdown {
    pub system_tokens: u64,
    pub tool_tokens: u64,
    pub message_tokens: u64,
}
