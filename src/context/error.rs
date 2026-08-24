//! Context-plane errors.
//!
//! The context engine is a deterministic runtime-owned projection of
//! canonical history into bounded model context. Its failures are runtime
//! facts, never provider facts: an error here is reported as a typed
//! [`ContextError`] and converted into a runtime error at the attempt
//! boundary — [`RuntimeError::ContextPreparationFailed`] for failures that
//! occur while preparing model context before compaction starts, and
//! [`RuntimeError::ContextCompactionFailed`] for actual proactive compaction
//! pipeline failures. A context error is never fabricated into a normalized
//! [`ModelError`].
//!
//! [`RuntimeError::ContextPreparationFailed`]: crate::runtime::types::RuntimeError::ContextPreparationFailed
//! [`RuntimeError::ContextCompactionFailed`]: crate::runtime::types::RuntimeError::ContextCompactionFailed
//! [`ModelError`]: crate::model::error::ModelError

use serde::{Deserialize, Serialize};

/// The typed failure of a context-engine operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct ContextError {
    /// The failure class.
    pub kind: ContextErrorKind,
    /// Human-readable diagnostic message.
    pub message: String,
}

/// The failure classes the context engine distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextErrorKind {
    /// The context configuration is impossible: the window does not leave a
    /// positive effective input budget after reserve and output tokens.
    InvalidConfiguration,
    /// The current context cannot fit even after the available complete
    /// messages have been retired: non-retirable request input (the Effective
    /// System Prompt, tool definitions, or the summary request itself)
    /// consumes the whole budget. Compaction cannot fix this; the caller must
    /// fail explicitly.
    CannotFit,
    /// The canonical conversation violates the structural contract: a
    /// `ToolMessageBlock` whose tool call resolves to no requesting
    /// `AssistantMessageBlock`, or a Surface span whose boundary is
    /// inconsistent with the active conversation. Malformed state is
    /// rejected, never guessed around.
    MalformedHistory,
    /// A compaction plan would make no measurable progress: the new summary
    /// would cover no additional canonical unit and the projected estimate
    /// would not strictly decrease. This is the central anti-loop invariant.
    NoProgress,
    /// Summary generation failed (a model-backed summarizer refusal, a tool
    /// request, a model failure, or a scripted fake failure).
    SummaryFailed,
    /// Summary generation was cancelled.
    Cancelled,
    /// Agent Status composition failed: an extension section provider
    /// reported an error. The failure is propagated as a context-preparation
    /// failure; it is never silently downgraded to an absent section.
    StatusFailed,
    /// An unexpected internal context failure.
    Internal,
}

impl ContextError {
    /// Creates a context error.
    #[must_use]
    pub fn new(kind: ContextErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextError, ContextErrorKind};

    /// Context errors round-trip with a stable discriminator.
    #[test]
    fn context_error_round_trip() {
        let error = ContextError::new(
            ContextErrorKind::NoProgress,
            "compaction must make measurable progress",
        );
        let json = serde_json::to_string(&error).expect("serialize error");
        let decoded: ContextError = serde_json::from_str(&json).expect("deserialize error");
        assert_eq!(decoded, error);
        assert_eq!(decoded.kind, ContextErrorKind::NoProgress);
    }
}
