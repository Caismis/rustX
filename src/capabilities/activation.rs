//! Source admission precedes every optional external preparation boundary.

use serde::{Deserialize, Serialize};

/// Configuration authors can express intent, never host evaluation results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEnablement {
    /// Request activation, subject to host trust and resource authority.
    Enabled,
    /// Keep the source inert.
    Disabled,
}

/// Effective host-owned authority, not a writable configuration vocabulary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceActivation {
    /// Discovery carries no activation grant.
    #[default]
    Unconfigured,
    /// Explicitly disabled, including for reconnect and child materialization.
    Disabled,
    /// Eligible only after host project trust and resource authority checks.
    Enabled,
    /// Host authority rejected the source.
    Untrusted,
}

impl SourceActivation {
    /// Evaluate declared intent against the host's trust/resource decision.
    /// CFG-01 rejects untrusted launches before composition. Embedded hosts may
    /// represent that rejection as inert availability instead. Neither discovery
    /// nor a capability consumer supplies authority here.
    #[must_use]
    pub const fn evaluate(intent: Option<SourceEnablement>, authority_accepted: bool) -> Self {
        match intent {
            None => Self::Unconfigured,
            Some(SourceEnablement::Disabled) => Self::Disabled,
            Some(SourceEnablement::Enabled) if authority_accepted => Self::Enabled,
            Some(SourceEnablement::Enabled) => Self::Untrusted,
        }
    }
    /// The single admission predicate, checked before side effects.
    ///
    /// # Errors
    /// Returns the inert-state reason unless explicitly enabled.
    pub fn admit(self) -> Result<(), &'static str> {
        match self {
            Self::Enabled => Ok(()),
            Self::Disabled => Err("source is disabled"),
            Self::Unconfigured => Err("source has no explicit activation grant"),
            Self::Untrusted => Err("source is untrusted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg233_host_evaluation_is_separate_from_declarative_enablement() {
        for accepted in [false, true] {
            assert_eq!(
                SourceActivation::evaluate(None, accepted),
                SourceActivation::Unconfigured
            );
            assert_eq!(
                SourceActivation::evaluate(Some(SourceEnablement::Disabled), accepted),
                SourceActivation::Disabled
            );
        }
        assert_eq!(
            SourceActivation::evaluate(Some(SourceEnablement::Enabled), false),
            SourceActivation::Untrusted
        );
        assert_eq!(
            SourceActivation::evaluate(Some(SourceEnablement::Enabled), true),
            SourceActivation::Enabled
        );
    }
}
