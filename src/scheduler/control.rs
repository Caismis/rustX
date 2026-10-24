//! Native request-local model authority over the ordinary `ScheduleStore`.
//! No model argument can establish a binding or a Human request identity.
use super::{
    ScheduleBinding, ScheduleCommand, ScheduleCommandAuthority, ScheduleCommandResult, ScheduleId,
    ScheduleSnapshot,
};
use crate::durable::ConversationStore;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};
use crate::runtime::inbound::ConversationInboundMailbox;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleHumanGrant {
    pub message_id: MessageId,
    pub original_attempt_id: AttemptId,
}

struct BoundControl {
    binding: ScheduleBinding,
    dirty: Arc<Mutex<BTreeMap<ConversationId, ScheduleBinding>>>,
    changed: Arc<tokio::sync::Notify>,
}

struct ControlInner {
    store: Arc<dyn ConversationStore>,
    mailbox: ConversationInboundMailbox,
    clock: Arc<dyn crate::runtime::types::RuntimeClock>,
    bound: OnceLock<BoundControl>,
}

/// Bound only by the native user-Session owner, never by a child composition.
#[derive(Clone)]
pub(crate) struct ScheduleControl(Arc<ControlInner>);

impl ScheduleControl {
    pub(crate) fn new(
        store: Arc<dyn ConversationStore>,
        mailbox: ConversationInboundMailbox,
        clock: Arc<dyn crate::runtime::types::RuntimeClock>,
    ) -> Self {
        Self(Arc::new(ControlInner {
            store,
            mailbox,
            clock,
            bound: OnceLock::new(),
        }))
    }

    pub(crate) fn bind(
        &self,
        binding: ScheduleBinding,
        dirty: Arc<Mutex<BTreeMap<ConversationId, ScheduleBinding>>>,
        changed: Arc<tokio::sync::Notify>,
    ) -> Result<(), String> {
        if self.0.store.conversation_id() != &binding.conversation_id {
            return Err("Schedule control binding belongs to another conversation".into());
        }
        self.0
            .bound
            .set(BoundControl {
                binding,
                dirty,
                changed,
            })
            .map_err(|_| "Schedule control already bound".into())
    }

    pub(crate) fn binding(&self) -> Option<&ScheduleBinding> {
        self.0.bound.get().map(|bound| &bound.binding)
    }

    fn bound(&self) -> Result<&BoundControl, String> {
        self.0
            .bound
            .get()
            .ok_or_else(|| "Schedule control requires a native root Session binding".into())
    }

    pub(crate) fn get(&self, id: &ScheduleId) -> Result<Option<ScheduleSnapshot>, String> {
        self.bound()?;
        self.0.store.schedule_get(id).map_err(|e| e.to_string())
    }

    pub(crate) fn list(&self, after: u64, limit: u32) -> Result<Vec<ScheduleSnapshot>, String> {
        self.bound()?;
        self.0
            .store
            .schedule_list(after, limit)
            .map_err(|e| e.to_string())
    }

    pub(crate) fn command(
        &self,
        grant: &Mutex<Option<ScheduleHumanGrant>>,
        command: &ScheduleCommand,
        preparation: Option<&crate::runtime::subagent::NativeExecutionContext>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<ScheduleCommandResult, String> {
        let bound = self.bound()?;
        // The execution owns this lock. A newer native Human handoff cannot
        // replace the request while an older command crosses its commit gate.
        let grant = grant.lock().expect("Schedule request grant");
        let grant = grant
            .as_ref()
            .ok_or("Schedule mutation requires a live current Human request")?;
        let authority = ScheduleCommandAuthority::HumanRequest {
            message_id: grant.message_id.clone(),
            original_attempt_id: grant.original_attempt_id.clone(),
        };
        if let Some(receipt) = self
            .0
            .store
            .schedule_command_receipt(&bound.binding, command, &authority)
            .map_err(|e| e.to_string())?
        {
            return Ok(receipt);
        }
        let prepared = match &command.mutation {
            super::ScheduleMutation::CreateWorkflow {
                workflow_id,
                arguments,
                ..
            }
            | super::ScheduleMutation::UpdateWorkflow {
                workflow_id,
                arguments,
                ..
            } => Some(
                preparation
                    .ok_or("native Workflow preparation is unavailable")?
                    .prepare_schedule_workflow(bound.binding.clone(), workflow_id, arguments)?,
            ),
            _ => None,
        };
        let result = self
            .0
            .mailbox
            .with_running_commit(|| {
                if cancellation.is_cancelled() {
                    return Err("Schedule mutation cancelled before commit".to_owned());
                }
                self.0
                    .store
                    .schedule_model_command(
                        &bound.binding,
                        command,
                        self.0.clock.as_ref(),
                        &authority,
                        prepared.as_ref(),
                        cancellation,
                    )
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| e.to_string())??;
        if matches!(
            result,
            ScheduleCommandResult::Applied { .. } | ScheduleCommandResult::Deleted { .. }
        ) {
            let mut dirty = bound.dirty.lock().expect("Scheduler dirty set");
            if dirty.len() < 1024 || dirty.contains_key(&bound.binding.conversation_id) {
                dirty.insert(bound.binding.conversation_id.clone(), bound.binding.clone());
            }
            bound.changed.notify_one();
        }
        Ok(result)
    }
}

/// Borrowed from the owning `AgentExecution`, so it cannot survive settlement.
#[derive(Clone)]
pub(crate) struct ScheduleToolContext<'a> {
    pub control: ScheduleControl,
    pub command_id: String,
    pub grant: &'a Mutex<Option<ScheduleHumanGrant>>,
}
