//! The normalized finite Attempt failure. Provider-specific fields never appear.
use crate::model::error::ModelErrorKind;
use crate::runtime::types::RuntimeError;
use serde::{Deserialize, Serialize};

/// The normalized client-visible failure of one attempt.
///
/// This is the external projection of the internal [`AttemptFailure`]
/// ([`crate::events::types::AttemptFailure`]): provider-specific fields
/// (such as the raw provider error code) never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientAttemptFailure {
    /// A model request exhausted its retry policy.
    Model {
        /// The normalized model error kind.
        kind: ModelErrorKind,
        /// The normalized human-readable message.
        message: String,
        /// The retry hint, when the provider reported one.
        retry_after_ms: Option<u64>,
    },
    /// A runtime failure.
    Runtime {
        /// The normalized runtime error.
        error: RuntimeError,
    },
}
/// Borrow only diagnostics into a finite display; original fields retain their J source.
pub(crate) fn finite_attempt_failure(
    failure: &crate::events::types::AttemptFailure,
) -> (RuntimeClientAttemptFailure, bool) {
    fn text(value: &str, deferred: &mut bool) -> String {
        *deferred |= value.chars().nth(256).is_some();
        value.chars().take(256).collect()
    }
    let mut deferred = false;
    let value = match failure {
        crate::events::types::AttemptFailure::Model { error } => {
            RuntimeClientAttemptFailure::Model {
                kind: error.kind.clone(),
                message: text(&error.message, &mut deferred),
                retry_after_ms: error.retry_after_ms,
            }
        }
        crate::events::types::AttemptFailure::Runtime { error } => {
            use crate::runtime::types::RuntimeError as E;
            let error = match error {
                E::Internal { message } => E::Internal {
                    message: text(message, &mut deferred),
                },
                E::InvalidState { message } => E::InvalidState {
                    message: text(message, &mut deferred),
                },
                E::Unsupported { message } => E::Unsupported {
                    message: text(message, &mut deferred),
                },
                E::UnknownTool { name } => E::UnknownTool {
                    name: text(name, &mut deferred),
                },
                E::DurableStore { message } => E::DurableStore {
                    message: text(message, &mut deferred),
                },
                E::ContractViolation { message } => E::ContractViolation {
                    message: text(message, &mut deferred),
                },
                E::ContextPreparationFailed { message } => E::ContextPreparationFailed {
                    message: text(message, &mut deferred),
                },
                E::ContextCompactionFailed { message } => E::ContextCompactionFailed {
                    message: text(message, &mut deferred),
                },
                E::PreStepRejected { reason } => E::PreStepRejected {
                    reason: text(reason, &mut deferred),
                },
                E::PreStepPolicyFailed { message } => E::PreStepPolicyFailed {
                    message: text(message, &mut deferred),
                },
                E::ToolResultObservationFailed { message } => E::ToolResultObservationFailed {
                    message: text(message, &mut deferred),
                },
                E::RestartInterrupted { message } => E::RestartInterrupted {
                    message: text(message, &mut deferred),
                },
                E::DeferredContextRejected { message } => E::DeferredContextRejected {
                    message: text(message, &mut deferred),
                },
            };
            RuntimeClientAttemptFailure::Runtime { error }
        }
    };
    (value, deferred)
}
