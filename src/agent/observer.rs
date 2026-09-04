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
//! content lives in the durable Message Ledger), while an external
//! client projection needs the committed content to repair its read model.
//! [`AgentExecutionObserver::observe_committed`] therefore receives the
//! canonical [`MessageBlock`] at the same commit linearization point where
//! the loop appends it to canonical history; it is never a competing
//! authority, only an observation of the authoritative commit.
//!
//! Agent Status is observed only after the canonical model-turn-start commit:
//! the observation carries the exact accepted generation and its committed
//! status-message identity, so a client projection can never observe a
//! prepared-but-cancelled status or cause a second composition. The
//! observation also carries the placement fact its opportunity froze, because
//! the linearization point that *determines* where a status belongs is
//! earlier than the point at which it is observed, and everything in between
//! — inbound acceptance in particular — is an independent durable boundary.
//!
//! This module defines the seam only; it owns no state and no consumer.

use crate::context::status::{AgentStatus, AgentStatusOpportunitySet};
use crate::durable::TranscriptCursor;
use crate::events::types::RuntimeEvent;
use crate::message::types::MessageBlock;
use crate::publication::{PublicationAudit, PublicationFrame, PublicationStreamStart};
use crate::runtime::identity::{AttemptId, ToolCallId, ToolId};
use crate::tools::types::ToolProgress;

/// The structured Agent Status observation of one committed model turn.
///
/// The observed status is the exact accepted value admitted as the canonical
/// Agent Status User context message. It is emitted after model-turn-start
/// arbitration and after that message's `observe_committed` callback.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusObservation {
    /// The attempt that composed the status.
    pub attempt_id: AttemptId,
    /// The turn number of the model request being prepared.
    pub turn: u32,
    /// The canonical Agent Status User message described by this observation.
    pub status_message_id: crate::runtime::identity::MessageId,
    /// The opportunity set that made this generation eligible.
    pub opportunities: AgentStatusOpportunitySet,
    /// The durable transcript position frozen when the `PostToolBatch`
    /// opportunity in [`opportunities`](Self::opportunities) was established:
    /// the canonical `ToolResult` batch's own position, read once at that
    /// commit and never re-read afterwards.
    ///
    /// This is the placement fact of a `PostToolBatch` composition, and it is
    /// deliberately frozen by the semantic owner rather than derived later.
    /// Inbound acceptance is an independent durable boundary that may commit
    /// between this freeze and `observe_status`; a consumer that instead read
    /// "the newest durable position" when it folded this observation would
    /// place the status after an unrelated inbound turn it has nothing to do
    /// with.
    ///
    /// `None` when this composition had no `PostToolBatch` opportunity, or
    /// when the batch that established one committed no visible transcript
    /// item. A `FreshInbound` composition is placed by the exact inbound
    /// message identity its opportunity carries, which is a stronger fact
    /// than a position, so it needs nothing here.
    pub post_tool_batch_anchor: Option<TranscriptCursor>,
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
    /// The durable Event Journal remains the historical authority; this
    /// callback observes the committed fact at the same publication
    /// linearization point, so live projections do not need an attempt-local
    /// duplicate journal.
    fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent);

    /// Observes one canonical message commit at its commit point.
    ///
    /// The loop calls this exactly once per committed canonical message
    /// (drained inbound User messages, committed Assistant messages, and
    /// committed tool messages) immediately after the message joined
    /// canonical history. The content observed here is the authoritative
    /// committed content; observers must treat it as read-only.
    fn observe_committed(
        &self,
        attempt_id: &AttemptId,
        block: &MessageBlock,
        transcript_cursor: Option<TranscriptCursor>,
    );

    /// Observes the one composed Agent Status of a request preparation.
    fn observe_status(&self, observation: &AgentStatusObservation);

    /// Observes one publication stream opening (Issue #108).
    ///
    /// The observation carries the frozen stream identity: the attempt,
    /// turn, request, and provisional message the stream is pinned to. A
    /// projection uses it to open its in-flight read model.
    fn observe_publication_opened(&self, attempt_id: &AttemptId, start: &PublicationStreamStart);

    /// Observes one publication frame **after** its durable commit.
    ///
    /// This is the user-facing release point. The loop calls it only once the
    /// frame's staging transaction — or, for the final frames, the atomic
    /// terminal transaction — has committed, so no semantic output can reach
    /// a Runtime Client that rustX has not durably committed for release.
    fn observe_publication(&self, attempt_id: &AttemptId, frame: &PublicationFrame);

    /// Observes one publication stream settling as an audit.
    ///
    /// The audit is an upper bound on what may have been displayed, never
    /// proof of perception, and its tool-call entries are model proposals
    /// that were never authorized or executed. A projection must be able to
    /// present them as such and must never treat them as canonical Message
    /// Ledger history or execution facts.
    fn observe_publication_settled(
        &self,
        attempt_id: &AttemptId,
        audit: &PublicationAudit,
        transcript_cursor: TranscriptCursor,
    );

    /// Observes one LIVE (not yet durable) foreground tool progress report
    /// (Issue #178). Unlike `observe_event` this is not a committed canonical
    /// fact: it is a disposable, latest-value, read-only observation emitted
    /// while the tool still executes. The durable `ToolExecutionProgress`
    /// facts still commit at batch settlement; this observation never enters
    /// the Event Journal, never blocks the loop, and must never be treated as
    /// execution evidence. Default: ignored.
    fn observe_tool_progress(
        &self,
        _attempt_id: &AttemptId,
        _tool_call_id: &ToolCallId,
        _tool_id: &ToolId,
        _progress: &ToolProgress,
    ) {
    }
}
