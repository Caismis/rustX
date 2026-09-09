//! Source admission precedes every optional external preparation boundary.

use serde::{Deserialize, Serialize};

/// An explicit source decision, independent of Tool selection and project trust.
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
