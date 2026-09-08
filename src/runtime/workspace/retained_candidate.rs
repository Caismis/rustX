//! Identity-only inspection/disposal of durable Workflow resources. Reading
//! these facts reconstructs no execution scope, borrower, or node authority.
use super::{
    WorkspaceDisposalError, WorkspaceDisposalPhase, WorkspaceDisposalSettlement, WorkspaceManager,
    WorkspaceOwner, WorkspaceSettlement, WorkspaceSettlementDisposition, WorkspaceUnresolvedReason,
    workflow_resource_event_id,
};
use crate::durable::ConversationStore;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::workflow::WorkflowRunId;

fn mismatch(detail: impl Into<String>) -> WorkspaceDisposalError {
    WorkspaceDisposalError::OwnershipMismatch {
        detail: detail.into(),
    }
}

struct Facts {
    workspace: WorkspaceSettlement,
    candidate: Option<super::CandidateReference>,
    intent: Option<super::WorkspaceHandoff>,
    phase: WorkspaceDisposalPhase,
    disposed: bool,
}

/// Historical run settlement and the separate, current disposal fact. Neither
/// grants access or recreates an execution lease after restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowWorkspaceInspection {
    pub settlement: WorkspaceSettlement,
    pub disposed: bool,
}

fn read_facts(
    store: &dyn ConversationStore,
    run: &WorkflowRunId,
) -> Result<Facts, WorkspaceDisposalError> {
    if store.conversation_id() != &run.conversation_id {
        return Err(mismatch("wrong conversation workspace authority"));
    }
    let mut cursor = None;
    let mut owned = None;
    let mut terminal = None;
    let mut candidate = None;
    let mut intent = None;
    let mut phase = WorkspaceDisposalPhase::Authorized;
    let mut disposed = false;
    loop {
        let page = store
            .read_events(cursor, 256)
            .map_err(|e| mismatch(e.to_string()))?;
        if page.events.is_empty() {
            break;
        }
        cursor = page.next_sequence;
        for envelope in page.events {
            match envelope.event {
                RuntimeEvent::WorkflowWorkspaceOwned { run_id, workspace } if run_id == *run => {
                    owned = Some(workspace);
                }
                RuntimeEvent::WorkflowWorkspaceSettled {
                    run_id,
                    workspace,
                    candidate: reference,
                } if run_id == *run => {
                    terminal = Some(workspace);
                    candidate = reference;
                }
                RuntimeEvent::WorkflowWorkspaceDisposalStarted { run_id, handoff }
                    if run_id == *run =>
                {
                    intent = Some(handoff);
                }
                RuntimeEvent::WorkflowWorkspaceDisposalSettled { run_id, settlement }
                    if run_id == *run =>
                {
                    match settlement {
                        WorkspaceDisposalSettlement::WorktreeRemoved { .. } => {
                            phase = WorkspaceDisposalPhase::WorktreeRemoved;
                        }
                        WorkspaceDisposalSettlement::Disposed
                        | WorkspaceDisposalSettlement::AlreadyDisposed => {
                            phase = WorkspaceDisposalPhase::PhysicalResourcesRemoved;
                            disposed = true;
                        }
                        WorkspaceDisposalSettlement::NothingRemoved { .. } => {}
                    }
                }
                _ => {}
            }
        }
    }
    let snapshot = owned.ok_or_else(|| mismatch("missing Workflow workspace ownership"))?;
    let workspace = terminal.unwrap_or_else(|| {
        WorkspaceSettlement::unresolved_with_reason(
            snapshot,
            WorkspaceUnresolvedReason::NestedContainment,
            "Workflow owner ended without physical settlement proof",
        )
    });
    Ok(Facts {
        workspace,
        candidate,
        intent,
        phase,
        disposed,
    })
}

fn commit(
    store: &dyn ConversationStore,
    run: &WorkflowRunId,
    phase: &str,
    event: RuntimeEvent,
) -> Result<(), WorkspaceDisposalError> {
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: workflow_resource_event_id(run, phase),
            sequence: 0,
            conversation_id: run.conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event,
        })
        .map(|_| ())
        .map_err(|e| WorkspaceDisposalError::Git {
            operation: "durable resource settlement".into(),
            detail: e.to_string(),
        })
}

impl WorkspaceManager {
    /// Reads retained/unresolved facts without restoring execution authority.
    ///
    /// # Errors
    /// Fails for a missing run lease, wrong conversation, or unreadable store.
    pub fn inspect_workflow_workspace(
        store: &dyn ConversationStore,
        run: &WorkflowRunId,
    ) -> Result<WorkflowWorkspaceInspection, WorkspaceDisposalError> {
        let facts = read_facts(store, run)?;
        Ok(WorkflowWorkspaceInspection {
            disposed: facts.disposed
                || matches!(
                    facts.workspace.disposition,
                    WorkspaceSettlementDisposition::Removed
                ),
            settlement: facts.workspace,
        })
    }

    /// Explicitly discards the exact retained candidate selected by run identity.
    /// Neither paths nor caller-supplied Git facts are accepted as authority.
    ///
    /// # Errors
    /// Refuses missing, active, unresolved-containment or mismatched ownership;
    /// reports physical/durable failures without changing run terminal facts.
    pub async fn dispose_workflow_workspace(
        &self,
        store: &dyn ConversationStore,
        run: &WorkflowRunId,
    ) -> Result<WorkspaceDisposalSettlement, WorkspaceDisposalError> {
        let _disposal = self.disposal_lock.lock().await;
        let owner = WorkspaceOwner::Workflow(run.clone());
        self.require_released(&owner)?;
        let facts = read_facts(store, run)?;
        if matches!(
            facts.workspace.disposition,
            WorkspaceSettlementDisposition::Removed
        ) {
            return Ok(WorkspaceDisposalSettlement::AlreadyDisposed);
        }
        let snapshot = &facts.workspace.snapshot;
        let handoff = if let Some(handoff) = facts.intent {
            handoff
        } else {
            let handoff = match &facts.workspace.disposition {
                WorkspaceSettlementDisposition::Retained { handoff, .. } => handoff.clone(),
                WorkspaceSettlementDisposition::PreservedUnresolved {
                    reason: WorkspaceUnresolvedReason::PhysicalSettlement,
                    ..
                } => self.verify_unresolved_workspace(&owner, snapshot).await?,
                _ => return Err(mismatch("candidate has unresolved physical containment")),
            };
            self.verify_retained_workspace(&owner, snapshot, &handoff)
                .await?;
            commit(
                store,
                run,
                "disposal-started",
                RuntimeEvent::WorkflowWorkspaceDisposalStarted {
                    run_id: run.clone(),
                    handoff: handoff.clone(),
                },
            )?;
            handoff
        };
        let result = self
            .dispose_authorized_workspace_inner(
                &owner,
                snapshot,
                &handoff,
                facts.phase,
                true,
                facts.candidate.as_ref(),
            )
            .await?;
        if facts.disposed {
            return Ok(WorkspaceDisposalSettlement::AlreadyDisposed);
        }
        let phase = match &result {
            WorkspaceDisposalSettlement::NothingRemoved { .. } => return Ok(result),
            WorkspaceDisposalSettlement::WorktreeRemoved { .. }
                if facts.phase == WorkspaceDisposalPhase::WorktreeRemoved =>
            {
                return Ok(result);
            }
            WorkspaceDisposalSettlement::WorktreeRemoved { .. } => "disposal-worktree-removed",
            WorkspaceDisposalSettlement::Disposed
            | WorkspaceDisposalSettlement::AlreadyDisposed => "disposal-disposed",
        };
        commit(
            store,
            run,
            phase,
            RuntimeEvent::WorkflowWorkspaceDisposalSettled {
                run_id: run.clone(),
                settlement: result.clone(),
            },
        )?;
        Ok(result)
    }
}
