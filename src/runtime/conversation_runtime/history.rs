//! Durable history reads over the conversation store: request history,
//! bounded transcript pages, and lineage-copy Surface/canonical reads.

use super::*;

impl ConversationRuntime {
    /// Returns a durable read handle for historical Request Snapshots.
    ///
    /// The returned value is a read-only handle over the durable request-fact
    /// authority; it does not retain another collection or create another
    /// conversation/transcript authority.
    #[must_use]
    pub fn request_history(&self) -> RequestHistory {
        RequestHistory::new(self.inner.store.clone())
    }

    /// Reads one bounded page of the derived transcript.
    ///
    /// `before` is an exclusive durable transcript cursor. With no cursor the
    /// newest page is returned; passing `next_cursor` walks older history.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the ordering spine or one of its
    /// canonical owners cannot be read coherently.
    pub fn transcript_page(
        &self,
        before: Option<crate::durable::TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        self.inner.store.load_transcript_page(before, limit)
    }

    /// Selects the current committed Surface head and materializes that exact
    /// revision. The head read is the clone linearization point; a later
    /// append or compaction creates a later revision and cannot mutate this
    /// selected historical meaning.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the head or selected Surface
    /// facts cannot be read.
    pub fn historical_head_snapshot(
        &self,
    ) -> Result<(SurfaceRevision, Vec<MessageBlock>), ConversationStoreError> {
        let head = self.inner.store.load_head()?;
        let messages = self.inner.store.load_surface_snapshot(head.revision)?;
        Ok((head.revision, messages))
    }

    /// Reads this conversation's complete durable canonical history, in
    /// Ledger commit order.
    ///
    /// This is the other half of a lineage copy. A Surface snapshot says what
    /// the model can currently see; this says what the conversation durably
    /// *is*, retired facts included. The two differ exactly when a compaction
    /// has run, and a copy that carried only the first half would inherit a
    /// compacted conversation's meaning and an uncompacted one's meaning
    /// differently — see [`crate::durable::LineageSeed`].
    ///
    /// The read races nothing it needs: every canonical row is immutable once
    /// committed, and the caller cuts this history at the exact Surface
    /// revision it selected, so facts committed afterwards are excluded by
    /// the cut rather than by the timing of this read.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when canonical history cannot be read.
    pub fn historical_canonical_history(
        &self,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.inner.store.load_canonical()
    }

    /// Reconstructs one retained provider-neutral request from its durable
    /// snapshot, exact historical Surface revision, and keyed Ledger bodies.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, crate::runtime::request_history::RequestHistoryError> {
        RequestHistory::new(self.inner.store.clone()).reconstruct(identity)
    }

    /// Select the final canonical Assistant commit owned by this exact attempt.
    /// Terminal observation can precede coordinator restoration, so both the
    /// ownership reference and content come from committed storage.
    pub(crate) fn durable_final_assistant(
        &self,
        attempt: &AttemptId,
    ) -> Option<crate::message::types::AssistantMessageBlock> {
        use crate::durable::presentation::{FactQuery, FactScope};
        let facts = self
            .inner
            .store
            .read_presentation_events(&FactQuery {
                scope: FactScope::Attempt(attempt.clone()),
                kinds: vec!["assistant_message_committed"],
                before: None,
                after: 0,
                ascending: false,
                through: self.inner.store.presentation_frontier().ok()?,
                limit: 1,
            })
            .ok()?;
        let RuntimeEvent::AssistantMessageCommitted { message_id } = &facts.first()?.event else {
            return None;
        };
        let mut messages = self
            .inner
            .store
            .load_messages(std::slice::from_ref(message_id))
            .ok()?;
        match messages.pop()? {
            MessageBlock::Assistant(assistant) => Some(assistant),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn durable_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.inner.store.load_canonical().ok()
    }
}
