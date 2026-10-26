//! Scheduler-owned transitions in the existing conversation transaction domain.
use crate::durable::ConversationStoreError;
use crate::runtime::types::RuntimeClock;
use crate::scheduler::{
    ScheduleAdmission, ScheduleBinding, ScheduleCommand, ScheduleCommandResult, ScheduleId,
    ScheduleSnapshot, ScheduleSummary,
};

#[allow(clippy::missing_errors_doc)]
pub trait ScheduleStore: Send + Sync + std::fmt::Debug {
    fn schedule_model_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn RuntimeClock,
        authority: &crate::scheduler::ScheduleCommandAuthority,
        contract: Option<&crate::runtime::workflow::scheduled::PinnedScheduledWorkflow>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<ScheduleCommandResult, ConversationStoreError>;
    fn schedule_command_receipt(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        authority: &crate::scheduler::ScheduleCommandAuthority,
    ) -> Result<Option<ScheduleCommandResult>, ConversationStoreError>;

    fn schedule_workflow_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn RuntimeClock,
        authority: &crate::scheduler::ScheduleCommandAuthority,
        contract: &crate::runtime::workflow::scheduled::PinnedScheduledWorkflow,
    ) -> Result<ScheduleCommandResult, ConversationStoreError>;

    fn schedule_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn RuntimeClock,
        authority: &crate::scheduler::ScheduleCommandAuthority,
    ) -> Result<ScheduleCommandResult, ConversationStoreError>;
    fn schedule_get(
        &self,
        id: &ScheduleId,
    ) -> Result<Option<ScheduleSnapshot>, ConversationStoreError>;
    fn schedule_list(
        &self,
        after_ordinal: u64,
        limit: u32,
    ) -> Result<Vec<ScheduleSnapshot>, ConversationStoreError>;
    fn schedule_due_page(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        after: Option<&crate::scheduler::ScheduleDueCursor>,
    ) -> Result<crate::scheduler::ScheduleDuePage, ConversationStoreError>;
    fn prune_schedules(
        &self,
        clock: &dyn RuntimeClock,
    ) -> Result<(usize, u64), ConversationStoreError>;
    fn schedule_summary(&self) -> Result<ScheduleSummary, ConversationStoreError>;
    fn admit_scheduled_target(
        &self,
        id: &ScheduleId,
        expected_revision: u64,
        clock: &dyn RuntimeClock,
    ) -> Result<ScheduleAdmission, ConversationStoreError>;
}
