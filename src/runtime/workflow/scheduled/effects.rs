//! Required effect receipts at the same executor frontiers used by Tool runs.
use super::lifecycle::{
    ClaimedScheduledWorkflow, ScheduledEffectKind, ScheduledEffectOwner, bounded_reason,
};
use crate::durable::ConversationStore;
use crate::runtime::workflow::{WorkflowNodeInstance, WorkflowNodeProgram, WorkflowRunError};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub(crate) struct ScheduledEffects {
    store: Arc<dyn ConversationStore>,
    claim: ClaimedScheduledWorkflow,
    unproven: Mutex<BTreeMap<String, String>>,
}
impl std::fmt::Debug for ScheduledEffects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduledEffects")
            .field("start", &self.claim.start.head.id)
            .finish_non_exhaustive()
    }
}
fn failure(e: impl std::fmt::Display) -> WorkflowRunError {
    WorkflowRunError::InvocationAuthority(bounded_reason(e.to_string()))
}
fn key(n: &WorkflowNodeInstance) -> String {
    serde_json::to_string(n).expect("native node identity")
}
impl ScheduledEffects {
    pub(crate) fn new(store: Arc<dyn ConversationStore>, claim: ClaimedScheduledWorkflow) -> Self {
        Self {
            store,
            claim,
            unproven: Mutex::default(),
        }
    }
    pub(crate) fn kind(node: &WorkflowNodeProgram) -> Option<ScheduledEffectKind> {
        match node {
            WorkflowNodeProgram::Tool { .. } => Some(ScheduledEffectKind::Tool),
            WorkflowNodeProgram::Agent(_) => Some(ScheduledEffectKind::Agent),
            WorkflowNodeProgram::Model(_) => Some(ScheduledEffectKind::Model),
            WorkflowNodeProgram::Review { .. } => Some(ScheduledEffectKind::Review),
            _ => None,
        }
    }
    pub(crate) fn begin(
        &self,
        node: &WorkflowNodeInstance,
        kind: ScheduledEffectKind,
    ) -> Result<(), WorkflowRunError> {
        self.store
            .scheduled_workflow_begin_effect(&self.claim, node, kind)
            .map_err(failure)
    }
    pub(crate) fn owner(
        &self,
        node: &WorkflowNodeInstance,
        owner: ScheduledEffectOwner,
    ) -> Result<(), WorkflowRunError> {
        self.store
            .scheduled_workflow_effect_owner(&self.claim, node, owner)
            .map_err(failure)
    }
    pub(crate) fn unproven(&self, node: &WorkflowNodeInstance, reason: impl AsRef<str>) {
        self.unproven
            .lock()
            .expect("scheduled native proof")
            .insert(key(node), bounded_reason(reason));
    }
    pub(crate) fn finish(&self, node: &WorkflowNodeInstance) -> Result<(), WorkflowRunError> {
        let unknown = self.unproven.lock().expect("scheduled native proof");
        let diagnostic = unknown.get(&key(node));
        self.store
            .scheduled_workflow_effect_settled(
                &self.claim,
                node,
                diagnostic.is_none(),
                diagnostic.map(String::as_str),
            )
            .map_err(failure)
    }
}
