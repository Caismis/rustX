//! Finite projection-head and committed-compaction descriptors.
use rustx_types::runtime::types::TokenMeasurement;
use serde::{Deserialize, Serialize};

/// Projection ownership is independent of the local execution trajectory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientProjectionHeadView {
    pub checkpoint_id: rustx_types::runtime::identity::CheckpointId,
    pub activation_id: rustx_types::context::checkpoint::ProjectionActivationId,
    /// Insert the historical marker after this native transcript position.
    /// Captured at activation; independent of Ledger and Journal coordinates.
    pub transcript_anchor: super::transcript::RuntimeClientTranscriptCursor,
    pub base_frontier: rustx_types::context::resources::LedgerFrontier,
    pub activation_ledger: rustx_types::context::resources::LedgerFrontier,
    pub origin: rustx_types::context::checkpoint::OriginActivation,
    pub destination_installation: Option<rustx_types::runtime::identity::LineageInstallationId>,
    pub original_items: u32,
    pub derived_items: u32,
    pub summary_preview: Option<crate::observation::trace::TraceText>,
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
    /// Stable native operation that installed this checkpoint.
    pub operation_id: rustx_types::runtime::identity::CompactionOperationId,
    /// The local committed projection generation.
    pub generation: u64,
    /// The independently durable checkpoint identity.
    pub checkpoint_id: rustx_types::runtime::identity::CheckpointId,
    /// The activation coordinate, independent of canonical and Journal order.
    pub activation_id: rustx_types::context::checkpoint::ProjectionActivationId,
    /// The pre-compaction input measurement and its provenance.
    pub tokens_before: TokenMeasurement,
    /// The deterministic estimate of the rebuilt request context.
    pub estimated_tokens_after: u64,
}
