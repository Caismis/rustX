//! Idle-work ownership and drain: what keeps the conversation busy, the Goal
//! idle claim, runtime-wide cancellation, and waiting for settlement.

use super::RegistryState;
use super::SubagentLifecycle;
use super::SubagentRecord;
use super::SubagentRegistry;
use super::agents;
use super::observation::SubagentPublication;
use super::observation::SubagentSnapshot;
use crate::runtime::ConversationInboundMailbox;
use crate::runtime::identity::SubagentId;
use crate::runtime::types::CancellationReason;
use std::sync::PoisonError;

struct GoalIdleClaim<'a>(&'a SubagentRegistry);

impl Drop for GoalIdleClaim<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(state.goal_idle_committing);
        state.goal_idle_committing = false;
        self.0.state_version.send_modify(|version| *version += 1);
    }
}

impl RegistryState {
    fn owns_idle_work(&self) -> bool {
        self.goal_idle_committing
            || !self.recovery_pending.is_empty()
            || !self.ownership_committing.is_empty()
            || self.agents.values().any(|agent| agent.resuming.is_some())
            || self.records.iter().any(|record| {
                (record.lifecycle.is_terminal() && !record.physical_settlement_proven)
                    || record.lifecycle.is_active()
                    || matches!(record.lifecycle, SubagentLifecycle::PublishingTerminal)
            })
    }
}

impl SubagentRegistry {
    /// In-memory ownership read for residency. Physical reconciliation belongs
    /// outside the caller's coordinator lock, never inside this predicate.
    pub(crate) fn owns_idle_work(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .owns_idle_work()
    }

    /// An exact idle claim excludes new ownership/resume admission while the
    /// Goal's durable frontier commits. Reads and controls retain mutex access;
    /// no filesystem or durable append runs inside the registry critical section.
    pub(crate) fn with_goal_idle<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        self.reconcile_recovered_settlements();
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.owns_idle_work() {
                return None;
            }
            // Linearization: a new owner must observe this claim and wait;
            // any previously selected owner made the idle check fail above.
            state.goal_idle_committing = true;
        }
        let _claim = GoalIdleClaim(self);
        Some(operation())
    }

    #[cfg(test)]
    pub(crate) fn fresh_with_mailbox_for_test(&self, mailbox: ConversationInboundMailbox) -> Self {
        let mut config = self.config.clone();
        config.mailbox = mailbox;
        Self::new(config)
    }

    /// Exact generations with unresolved physical ownership after terminal,
    /// failed staging or recovery (including allocations before Reserved).
    /// Draining reports these instead of retrying a finished driver.
    pub(crate) fn unproven_settlements(&self) -> Vec<SubagentId> {
        self.reconcile_recovered_settlements();
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut targets: Vec<_> = state
            .records
            .iter()
            .filter(|record| {
                (record.lifecycle.is_terminal() || record.publication_abandoned)
                    && !record.physical_settlement_proven
            })
            .map(|record| record.subagent_id.clone())
            .chain(state.agents.values().filter_map(|agent| {
                agent
                    .resuming
                    .as_ref()
                    .filter(|reservation| {
                        matches!(
                            *reservation.completion.borrow(),
                            agents::AdmissionSettlement::Failed,
                        )
                    })
                    .map(|reservation| reservation.activation_id.clone())
            }))
            .chain(state.recovery_pending.iter().cloned())
            .collect();
        targets.sort();
        targets.dedup();
        targets
    }

    /// The subagents whose terminal publication was abandoned.
    #[must_use]
    pub fn abandoned_publications(&self) -> Vec<SubagentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .filter(|record| record.publication_abandoned)
            .map(SubagentRecord::snapshot)
            .collect()
    }

    /// Whether any terminal notification still owes observable delivery
    /// work (drain observability).
    #[must_use]
    pub fn has_unresolved_delivery_work(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .records
            .iter()
            .any(|record| record.notification.has_pending_delivery())
    }

    /// Cancels every active subagent (runtime drain).
    pub fn cancel_all(&self, reason: CancellationReason) {
        let ids: Vec<SubagentId> = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for agent in state.agents.values() {
                if let Some(reservation) = &agent.resuming {
                    reservation.cancellation.cancel();
                }
            }
            for cancellation in state.capacity_waiters.values() {
                cancellation.cancel();
            }
            for (cancellation, committed_reason) in state.ownership_committing.values_mut() {
                cancellation.cancel();
                committed_reason.get_or_insert(reason);
            }
            state
                .records
                .iter()
                .filter(|record| {
                    matches!(
                        record.lifecycle,
                        SubagentLifecycle::Starting
                            | SubagentLifecycle::Running
                            | SubagentLifecycle::Stopping
                            | SubagentLifecycle::Cancelling
                    )
                })
                .map(|record| record.subagent_id.clone())
                .collect()
        };
        for id in ids {
            let _ = self.cancel(&id, reason);
        }
    }

    /// Waits for the exact activation's terminal decision or abandoned publication.
    /// Inspect the snapshot's separate publication and physical settlement facts;
    /// returning a terminal decision is not a physical-settlement acknowledgement.
    pub async fn wait_until_settled(&self, subagent_id: &SubagentId) -> Option<SubagentSnapshot> {
        let mut rx = self.state_version.subscribe();
        loop {
            let snapshot = self.snapshot(subagent_id)?;
            if snapshot.state.is_terminal()
                || snapshot.settlement.publication == SubagentPublication::Abandoned
            {
                return Some(snapshot);
            }
            if rx.changed().await.is_err() {
                return None;
            }
        }
    }

    /// Resolves with the newest snapshot of `subagent_id` once `predicate`
    /// holds for it; `None` if the record does not exist. Driven by the
    /// registry state-version watch — no polling.
    pub async fn wait_for_snapshot(
        &self,
        subagent_id: &SubagentId,
        predicate: impl Fn(&SubagentSnapshot) -> bool,
    ) -> Option<SubagentSnapshot> {
        let mut rx = self.state_version.subscribe();
        loop {
            let snapshot = self.snapshot(subagent_id)?;
            if predicate(&snapshot) {
                return Some(snapshot);
            }
            if rx.changed().await.is_err() {
                return None;
            }
        }
    }
}
