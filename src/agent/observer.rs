//! The narrow live-observation seam of one agent attempt execution.
//!
//! The Agent Loop remains the execution authority: it owns the attempt
//! state machine, model turns, cancellation observation, tool lifecycle,
//! safe-boundary mailbox drains, canonical message commit, and attempt
//! terminal settlement. The observer seam below is a read-only projection
//! boundary: consumers (for example the Runtime Client projection, Issue
//! #37) observe committed execution facts as they are produced, without
//! becoming a second execution authority.
//!
//! The seam is deliberately broader than the internal [`RuntimeEvent`]
//! vocabulary in exactly one way: canonical message content. The internal
//! committed-message events reference messages by identity only (message
//! content lives in the durable Message Ledger, M8), while an external
//! client projection needs the committed content to repair its read model.
//! [`AgentExecutionObserver::observe_committed`] therefore receives the
//! canonical [`MessageBlock`] at the same commit linearization point where
//! the loop appends it to canonical history; it is never a competing
//! authority, only an observation of the authoritative commit.
//!
//! Agent Status composition is observed through the exact composed
//! [`AgentStatusObservation`]: the observation carries the one structured
//! [`AgentStatus`] the composer produced with its single clock sample, so a
//! client projection can never cause a second composition with a different
//! clock instant.
//!
//! This module defines the seam only; it owns no state and no consumer.

use crate::context::status::AgentStatus;
use crate::events::types::RuntimeEvent;
use crate::message::types::MessageBlock;
use crate::runtime::identity::{AttemptId, MessageId};

/// The structured Agent Status observation of one request preparation.
///
/// The observed status is the exact composed value of the current request
/// preparation (one clock sample, one extension-provider invocation set):
/// the canonical rendered attachment consumed by the model request is
/// derived from this same value, so model-path and client-path status can
/// never diverge through a second composition.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusObservation {
    /// The attempt that composed the status.
    pub attempt_id: AttemptId,
    /// The turn number of the model request being prepared.
    pub turn: u32,
    /// The canonical inbound message the status targets.
    pub target_message_id: MessageId,
    /// The one composed structured status.
    pub status: AgentStatus,
}

/// The live observation seam of one agent attempt execution.
///
/// All callbacks are synchronous, read-only observations of authoritative
/// execution facts at their commit linearization points. An observer must
/// never mutate execution state and must never block the loop indefinitely;
/// the Runtime Client projection treats each callback as one projection
/// fold under its own synchronization boundary.
pub trait AgentExecutionObserver: Sync {
    /// Observes one canonical runtime execution fact at its emission point.
    ///
    /// The attempt-local ordered trace in `AgentExecutionResult.events`
    /// remains the authoritative record; this callback observes the same
    /// fact at the same emission linearization point, so attempt recording
    /// and external observation share one emission path.
    fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent);

    /// Observes one canonical message commit at its commit point.
    ///
    /// The loop calls this exactly once per committed canonical message
    /// (drained inbound user messages, committed agent messages, and
    /// committed tool messages) immediately after the message joined
    /// canonical history. The content observed here is the authoritative
    /// committed content; observers must treat it as read-only.
    fn observe_committed(&self, attempt_id: &AttemptId, block: &MessageBlock);

    /// Observes the one composed Agent Status of a request preparation.
    fn observe_status(&self, observation: &AgentStatusObservation);
}
