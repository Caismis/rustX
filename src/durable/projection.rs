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
    pub tool_associations: crate::model::occurrence::ToolAssociationHistory,
    pub cut: RequestHistoryCut,
    pub messages: crate::model::SelectedModelInput,
    pub input_manifest: crate::model::SelectedInputManifest,
    pub(crate) checkpoint_fingerprint: Option<std::sync::Arc<str>>,
    pub(crate) materialized_bytes: usize,
}

impl HistoricalModelInput {
    /// Materialize checkpoint items only for an explicit compaction. Ordinary
    /// requests already own these references in their immutable input manifest.
    ///
    /// # Errors
    /// Returns an invalid-reference error for a different checkpoint, changed
    /// checkpoint content, or a noncanonical Tail occurrence.
    pub fn checkpoint_items(
        &self,
        checkpoint: Option<&ProjectionCheckpoint>,
    ) -> Result<
        Vec<crate::context::checkpoint::CheckpointItem>,
        crate::durable::ConversationStoreError,
    > {
        use crate::context::checkpoint::{CheckpointItem, CheckpointMessage};
        use crate::model::projected::FrozenModelInput;
        if checkpoint.map(|p| &p.candidate.checkpoint_id) != self.cut.checkpoint_id.as_ref()
            || checkpoint.map_or(crate::context::checkpoint::ProjectionActivationId(0), |p| {
                p.activation.activation
            }) != self.cut.activation
            || checkpoint.map_or(crate::context::resources::LedgerFrontier(0), |p| {
                p.candidate.base_frontier
            }) != self.cut.base_frontier
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "compaction items differ from the captured projection cut".into(),
            ));
        }
        if checkpoint.map(|p| p.fingerprint.as_str()) != self.checkpoint_fingerprint.as_deref()
            || checkpoint.is_some_and(|p| p.candidate.fingerprint() != p.fingerprint)
        {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "compaction checkpoint content differs from the captured history".into(),
            ));
        }
        let mut items = checkpoint.map_or_else(Vec::new, |p| p.candidate.items.clone());
        if items.len() > self.input_manifest.len() {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "compaction items exceed the selected input".into(),
            ));
        }
        for frozen in self.input_manifest.iter().skip(items.len()) {
            let FrozenModelInput::Original(reference) = frozen else {
                return Err(crate::durable::ConversationStoreError::InvalidReference(
                    "historical Tail item is not canonical".into(),
                ));
            };
            items.push(CheckpointItem {
                message: CheckpointMessage::Original(reference.clone()),
                call_sources: Vec::new(),
                result_call: None,
            });
        }
        Ok(items)
    }
}
