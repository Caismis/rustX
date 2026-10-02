//! Bounded reading projections over existing native Attempt and transcript owners.
use crate::runtime::identity::{AttemptId, ConversationId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TURN_PAGE_MAX: usize = 64;
pub const TURN_PREVIEW_MAX: usize = 240;

/// Origin survives lineage copying; ordinal never participates in identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurnId {
    pub conversation_id: ConversationId,
    pub attempt_id: AttemptId,
}

/// Frozen inclusive Journal/transcript upper bounds plus a semantic mutation epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationReadCut {
    pub conversation_id: ConversationId,
    pub journal: u64,
    pub transcript: u64,
    /// Edits/removals of mutable transcript bodies retire unreconstructible cuts.
    pub mutation_revision: u64,
}

impl ConversationReadCut {
    /// Append-only growth preserves a cut; foreign, future or mutated cuts fail.
    #[must_use]
    pub fn reconstructible_from(&self, current: &Self) -> bool {
        self.conversation_id == current.conversation_id
            && self.journal <= current.journal
            && self.transcript <= current.transcript
            && self.mutation_revision == current.mutation_revision
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurn {
    pub id: ConversationTurnId,
    pub ordinal: usize,
    /// None until native work has a visible member or terminal position.
    pub cursor: Option<super::TranscriptCursor>,
    #[schemars(length(max = 240))]
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurnPage {
    pub cut: ConversationReadCut,
    pub total: usize,
    pub offset: usize,
    #[schemars(length(max = 64))]
    pub turns: Vec<ConversationTurn>,
}

/// A single transcript read vocabulary; every selector replaces a finite window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationWindowAt {
    Latest,
    Older {
        before: super::TranscriptCursor,
        cut: Option<ConversationReadCut>,
    },
    Newer {
        after: super::TranscriptCursor,
        cut: ConversationReadCut,
    },
    Turn {
        id: ConversationTurnId,
        cut: ConversationReadCut,
    },
}

#[derive(Debug)]
pub struct DurableConversationWindow {
    pub cut: ConversationReadCut,
    pub page: super::TranscriptPage,
    pub newer_cursor: Option<super::TranscriptCursor>,
    pub target: Option<ConversationTurnId>,
    pub target_cursor: Option<super::TranscriptCursor>,
}

/// Immutable reading provenance in native start order, separate from finalized responses.
/// Members and predecessor use destination identities after lineage remapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnReadingProvenance {
    pub id: ConversationTurnId,
    pub process_message_ids: Vec<crate::runtime::identity::MessageId>,
    /// Canonical predecessor of a terminal-only location; None precedes all content.
    pub preceding_message_id: Option<crate::runtime::identity::MessageId>,
    pub outcome: TurnProcessOutcome,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnProcessOutcome {
    Running,
    Completed,
    Cancelled,
    Failed,
    TimedOut,
    LimitExceeded,
}
