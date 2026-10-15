//! Immutable checkpoint content, activation lineage and idempotent local receipts.
use crate::context::checkpoint::{CheckpointCandidate, ProjectionActivation};
use crate::context::occupancy::ContextOccupancy;
use crate::events::types::RuntimeEventEnvelope;
use crate::runtime::identity::{AttemptId, CompactionOperationId, TurnId};
use crate::runtime::types::TokenMeasurement;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A fence in the existing execution Journal, not another execution state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionOwner {
    pub attempt_id: Option<AttemptId>,
    pub transition_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionCheckpoint {
    pub candidate: CheckpointCandidate,
    pub fingerprint: String,
    pub activation: ProjectionActivation,
    pub occupancy: Option<ContextOccupancy>,
}

/// One bounded historical page selected under a single native read snapshot.
pub struct ProjectionTimelinePage {
    pub records: Vec<ProjectionCheckpoint>,
    pub next_activation: Option<crate::context::checkpoint::ProjectionActivationId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionCommitReceipt {
    /// Only a fresh native transaction publishes its observation receipt.
    #[serde(skip)]
    pub newly_committed: bool,
    pub operation_id: CompactionOperationId,
    pub candidate_fingerprint: String,
    pub checkpoint: ProjectionCheckpoint,
    pub completed: RuntimeEventEnvelope,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionCommitInput {
    pub grant: crate::runtime::cancellation::CompactionCommitGrant,
    pub operation_id: CompactionOperationId,
    pub candidate: CheckpointCandidate,
    pub owner: ProjectionOwner,
    pub tokens_before: TokenMeasurement,
    pub estimated_tokens_after: u64,
    pub occupancy: Option<ContextOccupancy>,
    pub turn_id: Option<TurnId>,
    pub timestamp: DateTime<Utc>,
}

/// Frozen pre-start history. Request-start Context is intentionally outside this cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestHistoryCut {
    pub ledger: crate::context::resources::LedgerFrontier,
    pub checkpoint_id: Option<crate::runtime::identity::CheckpointId>,
    pub activation: crate::context::checkpoint::ProjectionActivationId,
    pub base_frontier: crate::context::resources::LedgerFrontier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalModelInput {
    pub tool_associations: Vec<crate::model::occurrence::HistoricalToolAssociation>,
    pub cut: RequestHistoryCut,
    pub messages: Vec<crate::model::ModelInputMessage>,
    pub items: Vec<crate::context::checkpoint::CheckpointItem>,
}
