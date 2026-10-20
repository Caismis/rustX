//! Finite committed-delivery heads at their native canonical source.
use super::content::CanonicalContentLocator;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientDeliveryItem {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub index: u64,
    pub locator: CanonicalContentLocator,
    pub file: Option<crate::tools::session_files::SessionFileReference>,
    pub metadata_deferred: bool,
    pub encoded_bytes: u64,
    pub metadata_domain: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientDeliveryHead {
    pub domain: String,
    pub total: u64,
    #[schemars(length(max = 8))]
    pub items: Vec<RuntimeClientDeliveryItem>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
pub(crate) fn frontier(head: &RuntimeClientDeliveryHead) -> super::cut::CollectionFrontier {
    super::cut::CollectionFrontier {
        domain: head.domain.clone(),
        revision: 1,
        total: head.total,
        loaded: u64::try_from(head.items.len()).expect("bounded delivery head"),
        next_offset: head.next_offset,
    }
}
