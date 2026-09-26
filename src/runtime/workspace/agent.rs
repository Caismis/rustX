//! A durable Agent retains its workspace between finite physical activations.
//! The lease is lent exclusively, never settled or deleted by normal activation
//! completion. Unproven containment poisons later admission.
use super::{
    WorkspaceLease, WorkspaceManager, WorkspaceOwner, WorkspacePolicy, WorkspaceSettlement,
    WorkspaceSettlementDisposition, WorkspaceSettlementError, WorkspaceSnapshot,
    WorkspaceUnresolvedReason, deterministic_worktree_name,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Debug, Clone)]
pub(crate) struct AgentWorkspace {
    lease: Arc<AsyncMutex<Option<WorkspaceLease>>>,
    admitted: Arc<AtomicBool>,
    poisoned: Arc<AtomicBool>,
    snapshot: WorkspaceSnapshot,
    recovery: Option<(
        WorkspaceManager,
        WorkspacePolicy,
        crate::runtime::identity::SubagentId,
    )>,
}

#[derive(Debug)]
pub(crate) struct AgentWorkspaceAccess {
    scope: AgentWorkspace,
    lease: OwnedMutexGuard<Option<WorkspaceLease>>,
}

impl AgentWorkspace {
    pub(crate) fn new(lease: WorkspaceLease) -> Self {
        Self {
            snapshot: lease.snapshot().clone(),
            recovery: None,
            lease: Arc::new(AsyncMutex::new(Some(lease))),
            admitted: Arc::new(AtomicBool::new(false)),
            poisoned: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn recovered(
        manager: WorkspaceManager,
        owner: crate::runtime::identity::SubagentId,
        policy: WorkspacePolicy,
        snapshot: WorkspaceSnapshot,
        poisoned: bool,
    ) -> Self {
        Self {
            lease: Arc::new(AsyncMutex::new(None)),
            admitted: Arc::new(AtomicBool::new(true)),
            poisoned: Arc::new(AtomicBool::new(poisoned)),
            snapshot,
            recovery: Some((manager, policy, owner)),
        }
    }

    pub(crate) fn commit(&self) {
        self.admitted.store(true, Ordering::Release);
    }

    /// Recovery/disposal can only revoke this physical authority, never reset it.
    pub(crate) fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    pub(crate) async fn acquire(&self) -> Result<AgentWorkspaceAccess, String> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err("Agent workspace physical settlement is unresolved".into());
        }
        let mut lease = Arc::clone(&self.lease)
            .try_lock_owned()
            .map_err(|_| "Agent workspace already has an active physical user".to_owned())?;
        // Recheck after taking the lease: a settling user may have poisoned
        // it between the initial check and exclusive acquisition.
        if self.poisoned.load(Ordering::Acquire) {
            return Err("Agent workspace physical settlement is unresolved".into());
        }
        if lease.is_none() {
            let Some((manager, policy, owner)) = &self.recovery else {
                return Err("Agent workspace has been rolled back".into());
            };
            self.snapshot.validate()?;
            let owner = WorkspaceOwner::from(owner);
            if self.snapshot.is_isolated() {
                // Reacquire the exact admitted physical resource. Never select
                // another base, copy parent bytes, or fall back to parent cwd.
                let inspected = WorkspaceManager::inspect_recovered(&self.snapshot);
                let handoff = inspected.handoff().ok_or_else(|| {
                    inspected
                        .error()
                        .unwrap_or("Agent workspace cannot be proven")
                        .to_owned()
                })?;
                manager
                    .verify_retained_workspace(&owner, &self.snapshot, handoff)
                    .await
                    .map_err(|error| error.to_string())?;
                if !manager
                    .active
                    .lock()
                    .expect("workspace ownership")
                    .insert(deterministic_worktree_name(&owner))
                {
                    return Err("Agent workspace already has a live owner".into());
                }
            }
            *lease = Some(WorkspaceLease {
                active_registered: self.snapshot.is_isolated(),
                policy: *policy,
                owner,
                manager: manager.clone(),
                snapshot: self.snapshot.clone(),
                branch_created: false,
                created: self.snapshot.is_isolated(),
            });
        }
        Ok(AgentWorkspaceAccess {
            scope: self.clone(),
            lease,
        })
    }
}

impl AgentWorkspaceAccess {
    pub(crate) fn snapshot(&self) -> &WorkspaceSnapshot {
        &self.scope.snapshot
    }

    pub(crate) fn settle(self) -> WorkspaceSettlement {
        WorkspaceSettlement {
            snapshot: self.scope.snapshot.clone(),
            disposition: WorkspaceSettlementDisposition::AgentRetained,
        }
    }

    pub(crate) async fn rollback(
        mut self,
    ) -> Result<WorkspaceSettlement, WorkspaceSettlementError> {
        if self.scope.admitted.load(Ordering::Acquire) {
            return Ok(self.settle());
        }
        self.lease
            .take()
            .expect("exclusive staged lease")
            .settle_staged()
            .await
    }

    pub(crate) fn unresolved(self, detail: String) -> WorkspaceSettlement {
        self.scope.poisoned.store(true, Ordering::Release);
        WorkspaceSettlement::unresolved_with_reason(
            self.scope.snapshot.clone(),
            WorkspaceUnresolvedReason::NestedContainment,
            detail,
        )
    }
}
