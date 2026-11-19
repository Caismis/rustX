//! Context checkpoint vocabulary shared across layers.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::CheckpointId;

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
pub struct ProjectionActivationId(pub u64);

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct CheckpointMessageOccurrence {
    pub checkpoint_id: CheckpointId,
    pub item_index: u32,
}
