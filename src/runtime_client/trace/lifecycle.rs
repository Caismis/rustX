//! Lifecycle projection: only the mutable vocabulary a refresh transmits.
//!
//! A refresh answers one question about records a client already holds:
//! *what has changed since it loaded them?* The answer is
//! [`TraceLifecycle`] — state, timing, the request's own terminal outcome,
//! the Tool's own outcome, the canonical message it settled into, and that
//! message's artifacts.
//!
//! Immutable historical presentation is deliberately absent, and so are the
//! reads that would produce it. A refresh covers up to
//! [`super::TRACE_RECORD_LIMIT`] records; resolving each one's System Prompt
//! predecessor and joining its canonical Context out of the Ledger would be
//! hundreds of historical joins whose results this response does not carry,
//! and it would make ordinary lifecycle repair unavailable whenever an
//! unrelated presentation join failed. Neither is acceptable, so neither
//! happens: this module starts from [`super::anchor::AnchorFacts`] and never
//! reaches into [`super::record`].

use super::anchor::AnchorFacts;
use super::bounds::{encoded_len, identity_fits};
use super::types::{TraceLifecycle, TraceRequestOutcome, TraceToolOutcomeUpdate};

/// Encoded-byte ceiling of one lifecycle update.
///
/// A refresh shares one frame with the ordinary snapshot, so a single
/// update leaves room for it. The ceiling is over lifecycle bytes alone: an
/// immutable presentation block can no longer push a record's mutable
/// outcome off its own update, because it is not projected here at all.
const TRACE_LIFECYCLE_BYTES: usize = 1024;

impl AnchorFacts {
    /// Projects the mutable lifecycle vocabulary of one refreshed record.
    ///
    /// The identity bounds match the summary projection's: an oversized
    /// native identity is omitted whole rather than shortened, because a
    /// shortened identity is a different identity that refers to nothing.
    pub(super) fn lifecycle(self) -> TraceLifecycle {
        let mut truncated = self.truncated;
        // A native identity a summary row would have to omit is reported as
        // omitted here too, even though a lifecycle update does not carry
        // it: a client must not see the same record described as complete by
        // one response and partial by the other.
        truncated |= self
            .location
            .attempt_id
            .is_some_and(|id| !identity_fits(id.as_str()));
        truncated |= self
            .location
            .step_id
            .is_some_and(|id| !identity_fits(id.as_str()));
        truncated |= self.native_id.is_some_and(|id| !identity_fits(&id));
        truncated |= self
            .originating_tool_call_id
            .is_some_and(|id| !identity_fits(id.as_str()));
        let request = self.request.and_then(|request| {
            // The summary row drops a request block whose own identities do
            // not fit; its lifecycle says the same thing, so a client is
            // never told about an outcome it cannot attribute.
            if identity_fits(request.frozen.request_id.as_str())
                && identity_fits(request.frozen.provisional_message_id.as_str())
            {
                Some(TraceRequestOutcome {
                    failure_kind: request.failure_kind,
                    usage: request.usage,
                    generation: request.generation,
                })
            } else {
                truncated = true;
                None
            }
        });
        let tool = self.tool.and_then(|tool| {
            if identity_fits(tool.call_id.as_str()) && identity_fits(tool.tool_id.as_str()) {
                Some(TraceToolOutcomeUpdate {
                    started: tool.started,
                    outcome: tool.outcome,
                    detail: tool.detail,
                })
            } else {
                truncated = true;
                None
            }
        });
        let mut message_id = self.message_id;
        if message_id
            .as_ref()
            .is_some_and(|id| !identity_fits(id.as_str()))
        {
            message_id = None;
            truncated = true;
        }
        let mut attachments = self.attachments;
        let references = attachments.len();
        attachments.retain(|reference| identity_fits(reference.artifact_id.as_str()));
        truncated |= references != attachments.len();
        let mut update = TraceLifecycle {
            id: self.id,
            state: self.state,
            timing: self.timing,
            request,
            tool,
            message_id,
            attachments,
            truncated,
        };
        // Oversized optional references become visibly partial; they never
        // justify omitting a loaded active record's lifecycle.
        if encoded_len(&update) > TRACE_LIFECYCLE_BYTES {
            update.attachments.clear();
            update.message_id = None;
            update.truncated = true;
        }
        update
    }
}
