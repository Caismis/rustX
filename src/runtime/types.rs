//! Runtime-owned shared semantics: token measurements, cancellation reasons,
//! and runtime errors.
//!
//! These types are shared by context, tool, model, and event contracts. They
//! are plain runtime-owned data and never reference provider SDK or storage
//! types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A token measurement of a model input, with explicit provenance.
///
/// This is a Layer 0 value contract. The Context Engine owns the accounting
/// behavior that decides when a measurement is valid, but the measurement
/// itself is shared by runtime events, context projections, and the Runtime
/// Client read model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenMeasurement {
    /// The measured or estimated input token count.
    pub input_tokens: u64,
    /// How the measurement was obtained.
    pub source: TokenMeasurementSource,
}

/// Where a [`TokenMeasurement`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenMeasurementSource {
    /// The provider reported usage for exactly this projection
    /// (`ModelUsage.input_tokens` of the completed request). Never
    /// fabricated, never a sum of cumulative snapshots.
    ProviderReported,
    /// A deterministic runtime-owned estimate.
    Estimated,
}

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
    /// The model requested a tool that is not present in the attempt's
    /// immutable tool registry. No tool result exists for the request, so
    /// the runtime fails explicitly instead of fabricating one.
    UnknownTool {
        /// The tool name the model called.
        name: String,
    },
    /// The canonical model stream violated its contract (for example a
    /// non-terminal event after the terminal event, or a tool-call delta
    /// referencing an unknown call). The runtime rejects the stream
    /// explicitly instead of silently accepting impossible state.
    ContractViolation {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// Context preparation failed while building the model context of a
    /// request **before any compaction started**: an invalid pending
    /// fresh-inbound state discovered during projection/status preparation,
    /// a failing Agent Status section provider, or a projection preparation
    /// failure that is not itself a compaction operation. This is never
    /// mislabeled as a compaction failure.
    ContextPreparationFailed {
        /// Human-readable diagnostic message.
        message: String,
    },
    /// Context compaction failed. This is a runtime-owned context-plane
    /// failure of an actual proactive compaction pipeline: it never
    /// fabricates a provider error, and it is distinct from a normalized
    /// model error even when the two coincide (for example a context
    /// overflow whose recovery compaction failed). Preparation failures that
    /// occur before compaction starts are [`RuntimeError::ContextPreparationFailed`]
    /// instead.
    ContextCompactionFailed {
        /// Human-readable diagnostic message.
        message: String,
    },
}

/// A runtime-owned UTC clock boundary.
///
/// State-machine code that must stamp deterministic timestamps (for example
/// background terminal inbound messages) goes through this narrow
/// abstraction; no production code calls `Utc::now()` directly in testable
/// state-machine code. Tests use a fixed/scripted clock so snapshots are
/// deterministic.
pub trait RuntimeClock: Send + Sync {
    /// The current UTC instant.
    fn now(&self) -> DateTime<Utc>;
}

/// The production clock: system UTC time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl RuntimeClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::{CancellationReason, RuntimeClock, RuntimeError, SystemClock};

    /// Cancellation reasons use a stable serialized representation.
    #[test]
    fn cancellation_reason_round_trip() {
        let value = CancellationReason::ParentCancelled;
        let json = serde_json::to_string(&value).expect("serialize reason");
        assert_eq!(json, "\"parent_cancelled\"");
        let decoded: CancellationReason = serde_json::from_str(&json).expect("deserialize reason");
        assert_eq!(decoded, value);
    }

    /// The system clock returns a valid UTC instant.
    #[test]
    fn system_clock_reports_utc_instants() {
        let instant = SystemClock.now();
        assert!(instant.timestamp() > 0, "a real UTC instant is reported");
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

    /// Tool-resolution and stream-contract errors have stable discriminators.
    #[test]
    fn runtime_error_discriminators_are_stable() {
        let cases = [
            (
                RuntimeError::UnknownTool {
                    name: "missing".to_owned(),
                },
                "unknown_tool",
            ),
            (
                RuntimeError::ContractViolation {
                    message: "event after terminal".to_owned(),
                },
                "contract_violation",
            ),
            (
                RuntimeError::ContextPreparationFailed {
                    message: "status provider failed".to_owned(),
                },
                "context_preparation_failed",
            ),
            (
                RuntimeError::ContextCompactionFailed {
                    message: "no progress".to_owned(),
                },
                "context_compaction_failed",
            ),
        ];
        for (error, expected) in cases {
            let value = serde_json::to_value(&error).expect("serialize error");
            assert_eq!(value["type"], expected);
            let decoded: RuntimeError = serde_json::from_value(value).expect("deserialize error");
            assert_eq!(decoded, error);
        }
    }
}
