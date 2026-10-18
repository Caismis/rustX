//! Finite display identities and previews over original native turn owners.
use crate::durable::TranscriptCursor;
use crate::durable::reading::ConversationReadCut;
use crate::runtime::identity::{AttemptId, ConversationId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedTurnIdentity {
    pub conversation_id: ConversationId,
    pub attempt_id: Option<AttemptId>,
    pub attempt_id_source: Option<super::response::AttemptIdentitySource>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedTurn {
    pub id: ObservedTurnIdentity,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub ordinal: u64,
    pub cursor: Option<TranscriptCursor>,
    #[schemars(length(max = 50))]
    pub prompt: String,
    #[schemars(length(max = 120))]
    pub response: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedTurnPage {
    pub cut: ConversationReadCut,
    pub total: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(length(max = 64))]
    pub turns: Vec<ObservedTurn>,
}
