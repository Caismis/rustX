//! Canonical durable cursors shared across layers.

use serde::{Deserialize, Serialize};

/// A durable transcript cursor.
///
/// This cursor belongs to the transcript ordering spine. It is deliberately
/// distinct from the Runtime Client observation cursor, the Event Journal
/// sequence, and the inbound mailbox sequence.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct TranscriptCursor(u64);

impl TranscriptCursor {
    /// Creates a cursor from its durable ordering position.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the durable ordering position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for TranscriptCursor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}
