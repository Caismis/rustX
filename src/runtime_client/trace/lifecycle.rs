//! Lifecycle projection: only the mutable vocabulary a refresh transmits.
//!
//! A refresh answers one question about records a client already holds:
//! *what has changed since it loaded them?* The answer is
//! [`TraceLifecycle`] — state, timing, the request's own terminal outcome,
//! the Tool's own outcome, the canonical message it settled into, and that
//! message's artifacts, and native ownership resolved at the current cut.
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
// Two bounded 512-byte location IDs may each expand sixfold under JSON escaping.
const TRACE_LIFECYCLE_BYTES: usize = 65_536;

impl AnchorFacts {
    /// Projects the mutable lifecycle vocabulary of one refreshed record.
    ///
    /// The identity bounds match the summary projection's: an oversized
    /// native identity is omitted whole rather than shortened, because a
    /// shortened identity is a different identity that refers to nothing.
    pub(super) fn lifecycle(
        self,
    ) -> Result<TraceLifecycle, crate::durable::ConversationStoreError> {
        let mut truncated = self.truncated;
        let mut location = self.location;
        if location
            .attempt_id
            .as_ref()
            .is_some_and(|id| !identity_fits(id.as_str()))
        {
            location.attempt_id = None;
            truncated = true;
        }
        if location
            .step_id
            .as_ref()
            .is_some_and(|id| !identity_fits(id.as_str()))
        {
            location.step_id = None;
            truncated = true;
        }
        truncated |= self.native_id.is_some_and(|id| !identity_fits(&id));
        truncated |= self
            .originating_tool_call_id
            .is_some_and(|id| !identity_fits(id.as_str()));
        let request = self.request.map(|request| TraceRequestOutcome {
            failure_kind: request.failure_kind,
            usage: request.usage,
            generation: request.generation,
        });
        let tool = self.tool.map(|tool| TraceToolOutcomeUpdate {
            started: tool.started,
            outcome: tool.outcome,
            detail: tool.detail,
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
            fields: self.fields,
            collections: self.collections,
            id: self.id,
            location,
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
        if encoded_len(&update) <= TRACE_LIFECYCLE_BYTES {
            return Ok(update);
        }
        update.attachments.clear();
        update.message_id = None;
        update.truncated = true;
        if encoded_len(&update) <= TRACE_LIFECYCLE_BYTES {
            return Ok(update);
        }
        if let Some(tool) = &mut update.tool {
            tool.detail = None;
        }
        if encoded_len(&update) > TRACE_LIFECYCLE_BYTES {
            return Err(crate::durable::ConversationStoreError::ObservationCapacityExceeded);
        }
        Ok(update)
    }
}
