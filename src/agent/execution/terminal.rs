//! The attempt terminal: its settlement candidate, the failure terminals of
//! every durable and runtime authority, state-machine settlement, and the
//! one terminal Event Journal fact.

use super::{AgentExecution, DurableFailureKind};
use crate::context::error::ContextError;
use crate::conversation::ConversationError;
use crate::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::PublicationStreamId;
use crate::runtime::interaction::InteractionFailure;
use crate::runtime::types::{CancellationReason, RuntimeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalCommitState {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalCarryoverDecision {
    Undecided,
    Replace(Option<PublicationStreamId>),
}

/// The terminal outcome of the whole attempt.
pub(super) enum Terminal {
    Completed { finish_reason: ModelFinishReason },
    Cancelled { reason: CancellationReason },
    Failed { failure: AttemptFailure },
}

impl Terminal {
    /// Projects the execution state-machine settlement candidate without
    /// requiring a durable terminal event to exist.
    pub(super) fn outcome(&self) -> AttemptOutcome {
        match self {
            Self::Completed { finish_reason } => AttemptOutcome::Completed {
                finish_reason: finish_reason.clone(),
            },
            Self::Cancelled { reason } => AttemptOutcome::Cancelled { reason: *reason },
            Self::Failed { failure } => AttemptOutcome::Failed {
                error: failure.clone(),
            },
        }
    }
}

/// A canonical commit failure in the attempt loop.
///
/// The prepare → durable → install canonical-commit seam (Issue #63, Finding
/// 2) has two fallible phases: the in-memory preparation/validation (a
/// [`ConversationError`]) and the durable Message Ledger append (an
/// [`ConversationStoreError`]). The install phase is infallible after both.
#[derive(Debug)]
pub(super) enum CanonicalCommitError {
    Conversation(ConversationError),
    Durable(crate::durable::inbox::ConversationStoreError),
    Interaction(InteractionFailure),
}

impl core::fmt::Display for CanonicalCommitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Conversation(error) => write!(f, "{error}"),
            Self::Durable(error) => write!(f, "{error}"),
            Self::Interaction(error) => write!(f, "{error}"),
        }
    }
}

impl From<ConversationError> for CanonicalCommitError {
    fn from(error: ConversationError) -> Self {
        Self::Conversation(error)
    }
}

impl AgentExecution<'_> {
    /// The `AttemptFailed` terminal of a durable-authority failure: the
    /// durable Message Ledger / Pending Inbound Inbox rejected a required
    /// durable operation of the active attempt. The failure is recorded on
    /// the execution so the settled result carries it to the coordinator
    /// (Issue #63): after an active-attempt durable failure the runtime
    /// must not return to a false healthy state.
    pub(super) fn durable_failure_terminal(
        &mut self,
        context: &str,
        error: &dyn core::fmt::Display,
    ) -> Terminal {
        let message = format!("{context}: {error}");
        self.durable_failure_kind = Some(DurableFailureKind::CanonicalCommit);
        self.durable_failure = Some(message.clone());
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::DurableStore { message },
            },
        }
    }

    pub(super) fn durable_failure_terminal_from_state(&self) -> Option<Terminal> {
        self.durable_failure
            .clone()
            .map(|message| Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            })
    }

    /// Converts an already-recorded Event Journal failure into the canonical
    /// commit error used by the tool-batch path. This keeps the active attempt
    /// from continuing into a semantic write after an event could not be
    /// persisted.
    pub(super) fn durable_failure_commit_error(&self) -> Option<CanonicalCommitError> {
        self.durable_failure.clone().map(|message| {
            CanonicalCommitError::Durable(crate::durable::inbox::ConversationStoreError::Storage(
                message,
            ))
        })
    }

    /// Maps one canonical commit failure to its honest terminal: a durable
    /// Message Ledger failure is a durable-authority failure (recorded for
    /// the coordinator), while an in-memory validation failure is a
    /// contract violation and never touches the durability record.
    pub(super) fn commit_failure_terminal(
        &mut self,
        context: &str,
        error: CanonicalCommitError,
    ) -> Terminal {
        match error {
            CanonicalCommitError::Durable(error) => self.durable_failure_terminal(context, &error),
            CanonicalCommitError::Conversation(error) => Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: format!("{context}: {error}"),
                    },
                },
            },
            CanonicalCommitError::Interaction(error) => Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::Internal {
                        message: format!("{context}: {error}"),
                    },
                },
            },
        }
    }

    /// Converts a failed reliable interaction/control boundary into the
    /// existing failed-attempt supervision outcome. It is intentionally not
    /// cancellation and does not create a human-visible interaction result.
    pub(super) fn interaction_failure_terminal() -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::Internal {
                    message: "the reliable interaction control path failed".to_owned(),
                },
            },
        }
    }

    /// The `AttemptFailed` terminal of a context-plane failure that occurred
    /// while preparing model context **before any compaction began**: an
    /// invalid pending fresh-inbound state discovered during status
    /// preparation or projection preparation that is not itself a compaction
    /// operation. Agent Status module failures are isolated by the closed
    /// engine before this boundary. These are never mislabeled as compaction
    /// failures: [`RuntimeError::ContextCompactionFailed`] is reserved for an
    /// actual proactive compaction pipeline failure.
    pub(super) fn context_failure_terminal(error: &ContextError) -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContextPreparationFailed {
                    message: error.message.clone(),
                },
            },
        }
    }

    /// The attempt's cancellation terminal, sampled at an observation
    /// checkpoint.
    pub(super) fn cancelled_terminal(&self) -> Terminal {
        Terminal::Cancelled {
            reason: self.cancellation.reason(),
        }
    }

    /// The terminal of a deferred batch rejected at the transaction boundary.
    pub(super) fn deferred_rejected(message: String) -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::DeferredContextRejected { message },
            },
        }
    }

    /// Settles the execution state machine for the computed terminal
    /// outcome, immediately before the terminal event is emitted.
    pub(super) fn settle(&mut self, terminal: &Terminal) {
        if let Terminal::Cancelled { reason } = terminal
            && let Some(workflow_output) = self.workflow_output.as_ref()
        {
            let _ = workflow_output.cancel(*reason);
        }
        let settlement = match terminal {
            Terminal::Completed { .. } => self.state.complete(),
            Terminal::Cancelled { .. } | Terminal::Failed { .. } => self.state.fail(),
        };
        settlement.expect("the execution state machine must accept the settlement");
    }

    pub(super) fn emit_terminal(&mut self, terminal: &Terminal) {
        let event = match terminal {
            Terminal::Completed { finish_reason } => RuntimeEvent::AttemptCompleted {
                attempt_id: self.request.attempt_id.clone(),
                finish_reason: finish_reason.clone(),
            },
            Terminal::Cancelled { reason } => RuntimeEvent::AttemptCancelled {
                attempt_id: self.request.attempt_id.clone(),
                reason: *reason,
            },
            Terminal::Failed { failure } => RuntimeEvent::AttemptFailed {
                attempt_id: self.request.attempt_id.clone(),
                error: failure.clone(),
            },
        };
        debug_assert!(!self.terminal_emitted, "exactly one terminal event");
        if matches!(
            self.logical_step.terminal_commit_state,
            TerminalCommitState::Blocked
        ) {
            // Publication evidence or source selection did not reach the
            // producer boundary. Leaving the attempt terminal event absent is
            // deliberate: recovery must finish the same semantic transition
            // from the durable prefix.
            return;
        }
        self.terminal_emitted = true;
        let envelope = self.event_envelope(event);
        let persisted = match std::mem::replace(
            &mut self.logical_step.terminal_carryover_decision,
            TerminalCarryoverDecision::Undecided,
        ) {
            TerminalCarryoverDecision::Replace(source) => self
                .store
                .commit_attempt_terminal_with_carryover(envelope, source),
            TerminalCarryoverDecision::Undecided => self.store.append_event(envelope),
        };
        match persisted {
            Ok(envelope) => self.record_persisted_event(envelope),
            Err(error) => {
                // The execution state machine has settled, but the durable
                // Event Journal has not. The uncommitted candidate must never
                // enter the persisted-event projection or observer stream.
                self.durable_failure_kind = Some(DurableFailureKind::EventJournal);
                self.durable_failure = Some(format!(
                    "terminal event could not be persisted before publication: {error}"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests;
