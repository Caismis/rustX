//! On-demand durable access to the derived transcript read model.
//!
//! `TranscriptHistory` retains only a durable store handle. Every call reads
//! one bounded page from the ordering spine and resolves bodies through their
//! canonical owners; it never becomes a second transcript authority.

use std::sync::Arc;

use crate::durable::{ConversationStore, ConversationStoreError, TranscriptCursor, TranscriptPage};

/// A durable transcript-history read handle for one conversation.
#[derive(Clone)]
pub struct TranscriptHistory {
    store: Arc<dyn ConversationStore>,
}

impl std::fmt::Debug for TranscriptHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TranscriptHistory(durable-read-handle)")
    }
}

/// A transcript page lookup or durable reconstruction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptHistoryError {
    /// The durable ordering spine or one of its canonical owners could not be
    /// read coherently.
    Read(String),
}

impl std::fmt::Display for TranscriptHistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for TranscriptHistoryError {}

impl TranscriptHistory {
    /// Creates a read handle over the conversation store.
    #[must_use]
    pub fn new(store: Arc<dyn ConversationStore>) -> Self {
        Self { store }
    }

    /// Reads one bounded page in chronological order.
    ///
    /// `before` is exclusive. With no cursor, the newest page is returned;
    /// callers pass `next_cursor` unchanged to continue toward older history.
    ///
    /// # Errors
    ///
    /// Returns a read error when the durable ordering spine or one of its
    /// canonical owners cannot be resolved.
    pub fn page(
        &self,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, TranscriptHistoryError> {
        self.store
            .load_transcript_page(before, limit)
            .map_err(|error: ConversationStoreError| {
                TranscriptHistoryError::Read(error.to_string())
            })
    }
}
