//! Context resource read-cut vocabulary shared across layers.

use serde::{Deserialize, Serialize};

use crate::context::checkpoint::ProjectionActivationId;
use crate::durable::canonical::TranscriptCursor;
use crate::runtime::identity::ConversationId;

/// Maximum occurrences in one detached Compaction resource cut.
pub const COMPACTION_RESOURCE_ITEMS_MAX: usize = 8192;
/// Maximum materialized bytes in one detached Compaction resource cut.
pub const COMPACTION_RESOURCE_BYTES_MAX: usize = 16 * 1024 * 1024;

/// Inclusive canonical commit position, independent of Journal and activation order.
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct LedgerFrontier(pub u64);

/// Inclusive source Journal position, never an activation or Ledger position.
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct JournalFrontier(pub u64);

/// One finite read authority. Pending inbound is not a compaction dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceReadCut {
    pub conversation_id: ConversationId,
    pub ledger: LedgerFrontier,
    pub journal: JournalFrontier,
    pub transcript: TranscriptCursor,
    pub projection_activation: ProjectionActivationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RequestHistoryCut {
    pub ledger: LedgerFrontier,
    pub checkpoint_id: Option<crate::runtime::identity::CheckpointId>,
    pub activation: crate::context::checkpoint::ProjectionActivationId,
    pub base_frontier: LedgerFrontier,
}
