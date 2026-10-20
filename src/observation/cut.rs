//! Exact read cuts, frontiers and content bindings that identify one bounded
//! native observation. A cut is immutable once captured.
use super::content::ContentRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LivePublicationReadCut {
    pub source: super::content::ContentSource,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub block_total: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub tool_total: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalForegroundReadCut {
    pub locator: super::content::CanonicalContentLocator,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReadCut {
    pub canonical_foreground: Option<CanonicalForegroundReadCut>,
    pub live_publication: Option<LivePublicationReadCut>,
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub activation_id: Option<crate::runtime::identity::SubagentId>,
    pub revision: u64,
    pub through: u64,
    /// Immutable durable descriptor frontier captured with this display cut.
    pub source_cut: Option<crate::durable::observation::ObservationReadCut>,
    pub domain_revisions: std::collections::BTreeMap<String, u64>,
    pub collection_cuts:
        std::collections::BTreeMap<String, crate::runtime::identity::ObservationCollectionCut>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentBinding {
    /// Projection-owned structural selection inside `CanonicalJsonV1` bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<Vec<String>>,
    pub path: Vec<String>,
    pub content_ref: ContentRef,
    pub deferred: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectionFrontier {
    pub domain: String,
    pub revision: u64,
    pub total: u64,
    pub loaded: u64,
    pub next_offset: Option<u64>,
}
/// Original JSON key evidence for a field selected by a bounded ordinal locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointKey {
    pub path: Vec<String>,
    pub preview: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub encoded_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointPageItem {
    /// Whether a finite actionable summary includes its complete request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<CheckpointKey>,
    pub offset: u64,
    pub identity: String,
    pub value: Option<serde_json::Value>,
    pub content: Vec<ContentBinding>,
    pub deferred: bool,
}
