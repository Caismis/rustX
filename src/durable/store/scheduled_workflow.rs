//! Required Workflow start/claim/effect/settlement transitions.
use crate::durable::ConversationStoreError;
use crate::runtime::workflow::WorkflowNodeInstance;
use crate::runtime::workflow::scheduled::{
    PinnedScheduledWorkflow,
    lifecycle::{
        ClaimedScheduledWorkflow, ScheduledEffectKind, ScheduledEffectOwner, ScheduledWorkflowHead,
        ScheduledWorkflowOutcome, ScheduledWorkflowOwnerRetirement, ScheduledWorkflowQuiescence,
        ScheduledWorkflowRecovery, ScheduledWorkflowSettlement, ScheduledWorkflowStart,
        ScheduledWorkflowStartId,
    },
};
use chrono::{DateTime, Utc};

#[allow(clippy::missing_errors_doc)]
pub trait ScheduledWorkflowStore: Send + Sync + std::fmt::Debug {
    fn scheduled_workflow_get(
        &self,
        id: &ScheduledWorkflowStartId,
    ) -> Result<Option<ScheduledWorkflowStart>, ConversationStoreError>;
    fn scheduled_workflow_pending(
        &self,
        limit: u32,
    ) -> Result<Vec<ScheduledWorkflowHead>, ConversationStoreError>;
    fn scheduled_workflow_claim(
        &self,
        id: &ScheduledWorkflowStartId,
        expected: &PinnedScheduledWorkflow,
        owner: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedScheduledWorkflow>, ConversationStoreError>;
    fn scheduled_workflow_wait(
        &self,
        id: &ScheduledWorkflowStartId,
        reason: &str,
        retry_after: DateTime<Utc>,
    ) -> Result<(), ConversationStoreError>;
    fn scheduled_workflow_reject(
        &self,
        id: &ScheduledWorkflowStartId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, ConversationStoreError>;
    fn scheduled_workflow_cancel(
        &self,
        id: &ScheduledWorkflowStartId,
        now: DateTime<Utc>,
    ) -> Result<Option<ScheduledWorkflowHead>, ConversationStoreError>;
    fn scheduled_workflow_begin_effect(
        &self,
        claim: &ClaimedScheduledWorkflow,
        node: &WorkflowNodeInstance,
        kind: ScheduledEffectKind,
    ) -> Result<(), ConversationStoreError>;
    fn scheduled_workflow_effect_owner(
        &self,
        claim: &ClaimedScheduledWorkflow,
        node: &WorkflowNodeInstance,
        owner: ScheduledEffectOwner,
    ) -> Result<(), ConversationStoreError>;
    fn scheduled_workflow_effect_settled(
        &self,
        claim: &ClaimedScheduledWorkflow,
        node: &WorkflowNodeInstance,
        proven: bool,
        diagnostic: Option<&str>,
    ) -> Result<(), ConversationStoreError>;
    fn scheduled_workflow_recovery(
        &self,
        id: &ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
    ) -> Result<Option<ScheduledWorkflowRecovery>, ConversationStoreError>;
    fn scheduled_workflow_recover_effects(
        &self,
        id: &ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
        retired: &ScheduledWorkflowOwnerRetirement,
    ) -> Result<(), ConversationStoreError>;
    fn scheduled_workflow_blocked(
        &self,
        id: &ScheduledWorkflowStartId,
        reason: &str,
    ) -> Result<(), ConversationStoreError>;
    fn scheduled_workflow_settle(
        &self,
        id: &ScheduledWorkflowStartId,
        outcome: ScheduledWorkflowOutcome,
        proof: ScheduledWorkflowQuiescence,
        result: Option<&serde_json::Value>,
        now: DateTime<Utc>,
    ) -> Result<ScheduledWorkflowSettlement, ConversationStoreError>;
}
