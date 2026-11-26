//! Bounded native observations of independent compaction ownership.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{AttemptId, CompactionOperationId};

/// Maximum normal physical starts admitted by one bounded source plan.
pub const MAX_PLANNED_SUMMARY_STARTS: usize = 256;
/// Maximum exceptional recovery actions, shared by all re-entry and retries.
pub const COMPACTION_RECOVERY_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactionMode {
    Blocking,
    /// Recovery from an actual provider context-overflow rejection.
    Overflow,
    Manual,
    Proactive,
}

/// Generation readiness and projection commitment are distinct states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactionOperationPhase {
    Generating,
    Retrying,
    Ready,
    ForegroundWaiting,
    Committed,
    Failed,
    Cancelled,
    Superseded,
    NoEffect,
}

impl CompactionOperationPhase {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Committed | Self::Failed | Self::Cancelled | Self::Superseded | Self::NoEffect
        )
    }
}

/// One runtime-admitted operation. Attribution never grants Attempt ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactionOperationStatus {
    pub operation_id: CompactionOperationId,
    pub admission_generation: u64,
    pub triggering_attempt_id: Option<AttemptId>,
    pub waiting_attempt_id: Option<AttemptId>,
    pub phase: CompactionOperationPhase,
    pub accepted_fragments: u32,
    pub diagnostic: Option<String>,
}

impl CompactionOperationStatus {
    /// Status metadata is small enough to publish independently of source bodies.
    /// # Errors
    /// Invalid identity, fragment count, or an oversized diagnostic is refused.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.operation_id.as_str().is_empty()
            || self.admission_generation == 0
            || self.accepted_fragments > 256
            || self
                .diagnostic
                .as_ref()
                .is_some_and(|text| text.len() > 1024)
        {
            return Err("invalid bounded compaction operation status");
        }
        Ok(())
    }
}

/// Native accepted work for the currently active Compaction operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientCompactionProgress {
    pub operation_id: crate::runtime::identity::CompactionOperationId,
    /// The native trigger fixed at operation admission.
    pub mode: crate::context::operation::CompactionMode,
    pub accepted_fragments: u32,
    pub source_ranges: Vec<crate::model::auxiliary::AuxiliarySourceRange>,
    pub generation: Option<crate::context::checkpoint::SummaryGenerationRef>,
}
