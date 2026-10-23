//! Scheduler-owned transitions in the existing conversation transaction domain.
use crate::durable::ConversationStoreError;
use crate::runtime::types::RuntimeClock;
use crate::scheduler::{
    ScheduleAdmission, ScheduleBinding, ScheduleCommand, ScheduleCommandResult, ScheduleId,
    ScheduleSnapshot, ScheduleSummary,
};

#[allow(clippy::missing_errors_doc)]
pub trait ScheduleStore: Send + Sync + std::fmt::Debug {
    fn schedule_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn RuntimeClock,
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
    fn schedule_summary(&self) -> Result<ScheduleSummary, ConversationStoreError>;
    fn admit_scheduled_inbound(
        &self,
        id: &ScheduleId,
        expected_revision: u64,
        clock: &dyn RuntimeClock,
    ) -> Result<ScheduleAdmission, ConversationStoreError>;
}
