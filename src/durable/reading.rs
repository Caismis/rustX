//! Bounded reading projections over existing native Attempt and transcript owners.
use crate::runtime::identity::{AttemptId, ConversationId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TURN_PAGE_MAX: usize = 64;
/// One rail-card line of the turn's opening human prompt.
pub const TURN_PROMPT_PREVIEW_MAX: usize = 50;
/// Three rail-card lines of the turn's final response.
pub const TURN_RESPONSE_PREVIEW_MAX: usize = 120;

/// Origin survives lineage copying; ordinal never participates in identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurnId {
    pub conversation_id: ConversationId,
    pub attempt_id: AttemptId,
}

/// Inclusive Journal/transcript bounds a read was captured at, plus its semantic mutation epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationReadCut {
    pub conversation_id: ConversationId,
    pub journal: u64,
    pub transcript: u64,
    /// Advances on every edit/removal of a mutable pending transcript body.
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
    /// The turn's first human prompt; empty when none was adopted for it.
    #[schemars(length(max = 50))]
    pub prompt: String,
    /// The turn's final text-bearing response; empty until the turn settles.
    #[schemars(length(max = 120))]
    pub response: String,
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

/// Bounded windows positioned by native Turn identity or by a captured read cut.
/// Append-only live growth preserves a cut; mutation invalidates it.
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
    /// The canonical human prompt that opened the turn, when one was retained.
    pub prompt_message_id: Option<crate::runtime::identity::MessageId>,
    pub outcome: InheritedTurnOutcome,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The origin Attempt's execution totals at the lineage cut. A seed written
    /// before this evidence existed has none, and contributes no totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<TurnExecution>,
}

/// Additive execution evidence of one native Attempt through a lineage cut.
///
/// As a `DeepSeek` Harness fork folds its copied event prefix, a lineage child's
/// whole-conversation statistics and context occupancy include the turns it
/// inherited. These are the Attempt's own facts, never destination execution:
/// they own no request, Tool, settlement or live clock.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnExecution {
    /// Logical model steps (`TurnStarted`).
    pub steps: u64,
    pub model_requests: u64,
    pub requests_with_usage: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_usage: Option<crate::model::types::ModelUsage>,
    /// Whether the Attempt closed with a finalized response.
    pub completed_response: bool,
    /// Summed request generation work, as `ActivityFold` measures it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_ms: Option<u64>,
    pub ttft_ms: u64,
    pub ttft_requests: u64,
    pub decode_ms: u64,
    pub decode_tokens: u64,
    /// Summed settled foreground Tool time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_ms: Option<u64>,
    /// The context reading after the Attempt's last request, absent when that
    /// request reported none or a later compaction invalidated it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occupancy: Option<crate::context::occupancy::ContextOccupancy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnProcessOutcome {
    Running,
    IncompleteAtCut,
    Completed,
    Cancelled,
    Failed,
    TimedOut,
    LimitExceeded,
}

/// Immutable inherited reading meaning; source execution can never remain live here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InheritedTurnOutcome {
    IncompleteAtCut,
    Completed,
    Cancelled,
    Failed,
    TimedOut,
    LimitExceeded,
}

impl From<InheritedTurnOutcome> for TurnProcessOutcome {
    fn from(outcome: InheritedTurnOutcome) -> Self {
        match outcome {
            InheritedTurnOutcome::IncompleteAtCut => Self::IncompleteAtCut,
            InheritedTurnOutcome::Completed => Self::Completed,
            InheritedTurnOutcome::Cancelled => Self::Cancelled,
            InheritedTurnOutcome::Failed => Self::Failed,
            InheritedTurnOutcome::TimedOut => Self::TimedOut,
            InheritedTurnOutcome::LimitExceeded => Self::LimitExceeded,
        }
    }
}

/// One durable lineage authority (R, C). `surface_revision` R selects structure;
/// invocation-time `read_cut` C selects temporal execution-derived facts, acquired
/// by the first read in one backend snapshot transaction. The same R at a newer C
/// may retain a newer outcome; later source activity cannot change captured (R, C).
#[derive(Debug, Clone, PartialEq)]
pub struct LineageReadCut {
    pub read_cut: ConversationReadCut,
    pub conversation_id: ConversationId,
    pub surface_revision: crate::conversation::SurfaceRevision,
    pub messages: Vec<crate::message::types::MessageBlock>,
    pub canonical: Vec<crate::message::types::MessageBlock>,
    pub surface_history: Vec<crate::conversation::SurfaceOp>,
    pub completed_responses: Vec<super::response::CompletedResponseProvenance>,
    pub turns: Vec<TurnReadingProvenance>,
}
