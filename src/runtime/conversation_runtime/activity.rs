//! Lightweight observations of existing native owners. These observations never
//! authorize execution, cancellation, settlement or residency.
use super::{ConversationRuntime, RuntimeInner};
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub(super) struct TaskActivity(watch::Sender<usize>);
impl Default for TaskActivity {
    fn default() -> Self {
        Self(watch::channel(0).0)
    }
}
impl TaskActivity {
    pub(super) fn pulse(&self) {
        self.0.send_modify(|_| {});
    }
    pub(super) fn lease(&self) -> TaskActivityLease {
        self.0.send_modify(|count| *count += 1);
        TaskActivityLease(self.0.clone())
    }
}
pub(super) struct TaskActivityLease(watch::Sender<usize>);
impl Drop for TaskActivityLease {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Independent facts from native owners, not a state machine.
pub(crate) struct RuntimeActivity {
    pub active: bool,
    pub waiting_on_approval: bool,
    pub waiting_on_user_input: bool,
    pub system_error: bool,
}

/// Receivers retain scalars, not runtimes or history. A manager observer may
/// hold these across retirement without pinning any native execution owner.
pub(crate) struct ActivityChanges {
    tasks: watch::Receiver<usize>,
    interactions: watch::Receiver<()>,
    background: watch::Receiver<u64>,
    children: Option<watch::Receiver<u64>>,
    auxiliary: watch::Receiver<()>,
    child_recovery: Option<watch::Receiver<bool>>,
}
impl ActivityChanges {
    pub(crate) async fn changed(&mut self) -> bool {
        tokio::select! {
            result = self.tasks.changed() => result.is_ok(),
            result = self.interactions.changed() => result.is_ok(),
            result = self.background.changed() => result.is_ok(),
            result = async { match &mut self.children { Some(changes) => { return changes.changed().await.is_ok(); }, None => std::future::pending::<bool>().await } } => result,
            result = async {match &mut self.child_recovery {Some(changes)=>changes.changed().await.is_ok(),None=>std::future::pending::<bool>().await}} => result,
            result = self.auxiliary.changed() => result.is_ok(),
        }
    }
}
impl RuntimeInner {
    fn try_activity(&self) -> Option<RuntimeActivity> {
        // Never take the residency registry while holding any native lock.
        let state = self.state.try_lock().ok()?;
        let (foreground, system_error) = {
            (
                state.current_attempt.is_some()
                    || state.scheduled.is_some()
                    || state.manual_compaction.is_some(),
                self.durability_gate.try_is_failed()? || state.mcp_settlement_failure.is_some(),
            )
        };
        #[cfg(test)]
        self.enter_probe_gate(|probe| probe.activity_capture_gate.clone());
        let tasks = *self.activity.0.borrow();
        // Hold these native leaf owners together, in this one documented
        // order. Independent BG/Sub/Aux handoffs cannot produce a mixed idle
        // cut. No callback performs I/O or re-enters another native owner.
        let mut activity = self
            .interaction
            .with_activity_waits(|waits| {
                self.tool_runtime
                    .background()
                    .with_activity(|background| {
                        let read_children = |children, child_fault, approval, user_input| {
                            self.auxiliary.with_activity(|auxiliary| RuntimeActivity {
                                active: foreground
                                    || tasks != 0
                                    || background
                                    || children
                                    || auxiliary
                                    || waits.0
                                    || waits.1
                                    || approval
                                    || user_input,
                                waiting_on_approval: waits.0 || approval,
                                waiting_on_user_input: waits.1 || user_input,
                                system_error: system_error || child_fault,
                            })
                        };
                        match &self.subagents {
                            Some(children) => children.with_activity(read_children).flatten(),
                            None => read_children(false, false, false, false),
                        }
                    })
                    .flatten()
            })
            .flatten()?;
        // Gate failure is absorbing and may be requested by an auxiliary
        // settlement without taking the coordinator. Read it after releasing
        // the leaf locks: reserve_class takes Gate -> Aux, so Aux -> Gate
        // would invert that order. A fault followed by retirement must never
        // become an idle observation.
        activity.system_error |= self.durability_gate.try_is_failed()?;
        Some(activity)
    }
}
impl ConversationRuntime {
    #[cfg(test)]
    pub(crate) fn with_activity_coordinator_lock_for_test(&self, read: impl FnOnce()) {
        let state = self.inner.lock_state();
        read();
        drop(state);
    }
    pub(crate) fn activity(&self) -> Option<RuntimeActivity> {
        self.inner.try_activity()
    }
    /// An observer recovery worker waits outside Coordinator, residency and
    /// per-Thread capture locks. Even metadata holders that emit no activity
    /// notification release these native mutexes; no timer or polling needed.
    pub(crate) fn wait_activity_ready(&self) {
        drop(self.inner.lock_state());
        let _ = self.inner.durability_gate.admission_failure();
        self.inner.interaction.wait_activity_ready();
        self.inner.tool_runtime.background().wait_activity_ready();
        if let Some(children) = &self.inner.subagents {
            children.wait_activity_ready();
        }
        self.inner.auxiliary.wait_activity_ready();
    }
    pub(crate) fn activity_changes(&self) -> ActivityChanges {
        ActivityChanges {
            tasks: self.inner.activity.0.subscribe(),
            interactions: self
                .inner
                .pending
                .get()
                .expect("native observation installed")
                .activity_changes(),
            background: self.inner.tool_runtime.background().activity_changes(),
            children: self
                .inner
                .subagents
                .as_ref()
                .map(crate::runtime::subagent::SubagentRegistry::activity_changes),
            auxiliary: self.inner.auxiliary.activity_changes(),
            child_recovery: self
                .inner
                .subagents
                .as_ref()
                .map(crate::runtime::subagent::SubagentRegistry::recovery_activity_changes),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::conversation_runtime::test_support::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn thread_status_fault_crossing_native_retirement_never_reports_idle() {
        let dir = tempfile::tempdir().unwrap();
        let id = "conv_113cba28-d6b2-70f4-8f62-8f8007e37ace";
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(id)).unwrap(),
        );
        let gate = Arc::new(Gate::default());
        let (runtime, _) = headless_runtime_over_store_with(
            &dir,
            id,
            store,
            vec![one_turn_script()],
            Some(CoordinatorProbe {
                activity_capture_gate: Some(gate.clone()),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        assert!(!runtime.activity().unwrap().system_error);
        let release = gate.arm_scoped();
        let reader = {
            let runtime = runtime.clone();
            tokio::task::spawn_blocking(move || runtime.activity())
        };
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .unwrap();
        }
        // Auxiliary retirement can close this absorbing frontier without the
        // coordinator. The status reader already observed healthy admission.
        runtime.inner.durability_gate.commit_failure(
            DurableOperation::SelectPendingBatch,
            "settlement failed".into(),
        );
        drop(release);
        let activity = reader.await.unwrap().unwrap();
        assert!(activity.system_error);
        assert!(!activity.active);
        assert!(runtime.shutdown().await.is_err());
    }
}
