//! Narrow Agent-to-Session handoff for independent compaction work.
//!
//! This interface has no projection-installation authority. A ready result is
//! consumed only by the current Agent boundary or idle coordinator.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use super::compaction::{CapturedCompaction, CompactionExecutionError, GeneratedCompaction};
use super::summarizer::SummaryOperation;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AttemptId, CompactionOperationId};

#[derive(Clone)]
pub(crate) struct CompactionAdmission {
    pub operation_id: CompactionOperationId,
    pub generation: u64,
    /// Owned by the Session, never a child of the triggering Attempt.
    pub cancellation: CancellationSignal,
}

pub(crate) struct ReadyCompaction {
    pub admission: CompactionAdmission,
    pub generated: Arc<GeneratedCompaction>,
}

pub(crate) enum CompactionDelivery {
    Ready(ReadyCompaction),
    Failed(super::ContextError),
}

/// The installed result or a non-mutating disposition of one selected result.
pub(crate) enum CompactionDisposition {
    Committed,
    Superseded,
    Incomparable,
    Cancelled,
    /// Retain the result for a later boundary whose complete input may differ.
    Defer(Option<String>),
    Failed(String),
}

pub(crate) trait CompactionCoordinator: Send + Sync {
    /// Reserve the single Session proactive slot and its independent signal.
    fn admit(
        &self,
        trigger: &AttemptId,
    ) -> Result<Option<CompactionAdmission>, CompactionExecutionError>;
    /// Transfer the captured plan and complete operation tail into runtime ownership.
    fn start(
        &self,
        admission: CompactionAdmission,
        captured: CapturedCompaction,
        operation: Box<dyn SummaryOperation>,
    ) -> Result<(), CompactionExecutionError>;
    /// Settle a capture which could not be dispatched.
    fn fail_admission(
        &self,
        admission: &CompactionAdmission,
        error: &CompactionExecutionError,
    ) -> Result<(), super::ContextError>;
    /// One finite consumption cut. A selected result retains its registry slot.
    fn take_ready(&self) -> Option<CompactionDelivery>;
    /// Select the currently useful operation without transferring its ownership.
    fn active(&self) -> Option<CompactionAdmission>;
    /// A continuation outside the frozen retirement span cannot be repaired
    /// by waiting for this operation.
    fn can_join(
        &self,
        admission: &CompactionAdmission,
        context: &super::ContextRuntime,
        request: &crate::model::ModelRequest,
        constraints: &super::engine::CompactionConstraints<'_>,
    ) -> Result<bool, CompactionExecutionError>;
    /// Wait only for the selected operation. Attempt cancellation detaches the waiter.
    fn wait<'a>(
        &'a self,
        admission: &'a CompactionAdmission,
        attempt: &'a AttemptId,
        cancellation: &'a CancellationSignal,
    ) -> BoxFuture<'a, Option<CompactionDelivery>>;
    fn settle(
        &self,
        admission: &CompactionAdmission,
        outcome: CompactionDisposition,
    ) -> Result<(), super::ContextError>;
    /// A foreground commit may prove a captured prefix obsolete.
    fn head_changed(&self);
}
