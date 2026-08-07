//! Runtime-owned shared semantics: cancellation reasons and runtime errors.
//!
//! These types are shared by tool, model, and event contracts. They are plain
//! runtime-owned data and never reference provider SDK or storage types.

use serde::{Deserialize, Serialize};

/// Why an operation was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    /// The user requested cancellation of the attempt or its work.
    UserRequested,
    /// The conversation runtime is shutting down.
    RuntimeShutdown,
    /// A parent operation that owns this work was cancelled.
    ParentCancelled,
}

/// A normalized runtime-owned execution error.
///
/// `RuntimeError` describes failures of the runtime itself, distinct from
/// normalized model errors (`ModelError`) and tool execution statuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeError {
    /// An unexpected internal failure with no further classification.
    Internal {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The runtime reached a state it should not be in.
    InvalidState {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// The requested operation is not supported by this runtime.
    Unsupported {
        /// Human-readable diagnostic message.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{CancellationReason, RuntimeError};

    /// Cancellation reasons use a stable serialized representation.
    #[test]
    fn cancellation_reason_round_trip() {
        let value = CancellationReason::ParentCancelled;
        let json = serde_json::to_string(&value).expect("serialize reason");
        assert_eq!(json, "\"parent_cancelled\"");
        let decoded: CancellationReason = serde_json::from_str(&json).expect("deserialize reason");
        assert_eq!(decoded, value);
    }

    /// Runtime errors round-trip with an explicit discriminator.
    #[test]
    fn runtime_error_round_trip() {
        let value = RuntimeError::InvalidState {
            message: "attempt already finished".to_owned(),
        };
        let json = serde_json::to_string(&value).expect("serialize error");
        let decoded: RuntimeError = serde_json::from_str(&json).expect("deserialize error");
        assert_eq!(decoded, value);
    }
}
