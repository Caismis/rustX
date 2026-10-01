//! Bounded reading projections over existing native Attempt and transcript owners.
use crate::conversation::SurfaceRevision;
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

/// Exact native read authority. Appends, compaction and pending mutations retire it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationReadCut {
    pub conversation_id: ConversationId,
    pub journal: u64,
    pub transcript: u64,
    pub surface_revision: SurfaceRevision,
    /// Pending population changes on removal; an admission also advances the
    /// transcript frontier. Existing native CAS revisions increase on edits.
    pub pending_count: u64,
    pub pending_revision: u64,
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
