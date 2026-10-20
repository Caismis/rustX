//! Finite projection-head and committed-compaction descriptors.
use crate::runtime::types::TokenMeasurement;
use serde::{Deserialize, Serialize};

/// Projection ownership is independent of the local execution trajectory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientProjectionHeadView {
    pub checkpoint_id: crate::runtime::identity::CheckpointId,
    pub activation_id: crate::context::checkpoint::ProjectionActivationId,
    pub base_frontier: crate::context::resources::LedgerFrontier,
    pub activation_ledger: crate::context::resources::LedgerFrontier,
    pub origin: crate::context::checkpoint::OriginActivation,
    pub destination_installation: Option<crate::runtime::identity::LineageInstallationId>,
    pub original_items: u32,
    pub derived_items: u32,
    pub summary_preview: Option<super::trace::TraceText>,
}
/// Public metadata for one committed compaction.
///
/// Every field is derived from an actual local checkpoint commit. The
/// independent projection disclosure carries summary content; canonical
/// execution rows never acquire a fabricated summary identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientCompactionView {
    /// The local committed projection generation.
    pub generation: u64,
    /// The independently durable checkpoint identity.
    pub checkpoint_id: crate::runtime::identity::CheckpointId,
    /// The activation coordinate, independent of canonical and Journal order.
    pub activation_id: crate::context::checkpoint::ProjectionActivationId,
    /// The pre-compaction input measurement and its provenance.
    pub tokens_before: TokenMeasurement,
    /// The deterministic estimate of the rebuilt request context.
    pub estimated_tokens_after: u64,
}
