//! Context capacity and planning budgets derived from the current model.

use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};

/// The runtime-owned context configuration of one attempt.
///
/// The soft input limit of one request is derived explicitly and checked:
///
/// ```text
/// admissible_input_limit = context_window_tokens - reserve_tokens - max_output_tokens
/// ```
///
/// Automatic compaction triggers at 90% of the admissible input limit.
/// Admission accepts equality at the hard limit. Impossible capacities are rejected; no fallback
/// constant is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextConfig {
    /// The model context window in tokens.
    pub context_window_tokens: u64,
    /// Tokens permanently reserved out of the model context window.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation content kept uncompressed. This is a
    /// token target, never a message count target.
    pub keep_recent_tokens: u64,
}

/// The budgets compaction planning must keep distinct.
///
/// The primary model's output budget determines how much input the next
/// primary request may carry. The frozen summary invocation's output budget
/// is a reservation for the summary that will be generated while applying
/// the plan, and its **input** limit bounds how large a Surface span may be
/// serialized into one summary request: an arbitrarily large span can never
/// become an impossible summary-model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionBudgets {
    /// The primary invocation's effective maximum output tokens.
    pub primary_output_budget: u32,
    /// The frozen/capped summary invocation's effective maximum output
    /// tokens.
    pub summary_output_budget: u32,
    /// The frozen summary invocation's effective input limit: the largest
    /// selected span estimate one summary request may carry.
    ///
    /// This budget is never scaled by an [`EstimateCorrection`]. A
    /// correction is the ratio between one *primary* request's estimate and
    /// what the provider counted for that same request — a measurement of
    /// that request, not a calibration of a tokenizer. The two requests do
    /// not carry the same material: the summary request has no tools, no
    /// Agent Status, no Skill catalog, and no provider continuation, and a
    /// stored continuation alone can put six figures of provider-counted
    /// input behind the primary request that the summary request will never
    /// send. Reusing the ratio would shrink a summary budget on evidence
    /// about a different request — possibly to `CannotFit` — even when both
    /// requests go to the same model. A summary request that really is too
    /// large is rejected by the summary model, and the bounded shrink loop
    /// around this plan replans against *that* measurement.
    pub summary_input_limit: u64,
}

impl CompactionBudgets {
    /// Creates the budgets for one admitted attempt.
    #[must_use]
    pub const fn new(
        primary_output_budget: u32,
        summary_output_budget: u32,
        summary_input_limit: u64,
    ) -> Self {
        Self {
            primary_output_budget,
            summary_output_budget,
            summary_input_limit,
        }
    }
}

impl ContextConfig {
    /// Derives the soft input limit of one request with the given
    /// runtime-resolved output budget, using checked arithmetic.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::InvalidConfiguration`] when the window
    /// leaves no positive effective input budget.
    pub fn admissible_input_limit(&self, max_output_tokens: u32) -> Result<u64, ContextError> {
        let output = u64::from(max_output_tokens);
        if self.context_window_tokens <= self.reserve_tokens {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                format!(
                    "context_window_tokens {} must exceed reserve_tokens {}",
                    self.context_window_tokens, self.reserve_tokens
                ),
            ));
        }
        let remaining = self.context_window_tokens - self.reserve_tokens;
        if remaining <= output {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                format!(
                    "context_window_tokens {} must exceed reserve_tokens {} + max_output_tokens {}",
                    self.context_window_tokens, self.reserve_tokens, output
                ),
            ));
        }
        Ok(remaining - output)
    }
}
