//! One supervised wake loop; its index and notifications are rebuildable hints.
use super::{RuntimeManagerError, SessionRuntimeManager, error};
use crate::runtime::identity::ConversationId;
use crate::scheduler::{
    ScheduleBinding, ScheduleDriverDiagnostics, ScheduleOperation, ScheduleOperationResult,
    ScheduleState, ScheduleSummary,
};
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::watch;

struct DriverPermit(watch::Sender<usize>);
impl Drop for DriverPermit {
    fn drop(&mut self) {
        self.0.send_replace(0);
    }
}
#[derive(Clone)]
struct Indexed {
    binding: ScheduleBinding,
    summary: ScheduleSummary,
    retry_at: u64,
    cursor: usize,
}
impl SessionRuntimeManager {
    /// Inspect discovery health without loading a store or Agent.
    /// # Panics
    /// Panics if the native diagnostics mutex is poisoned.
    #[must_use]
    pub fn schedule_driver_diagnostics(&self) -> ScheduleDriverDiagnostics {
        self.schedule_diagnostics
            .lock()
            .expect("Scheduler diagnostics")
            .clone()
    }
    #[allow(clippy::too_many_lines)] // One bounded wake/scan/dispatch supervision loop.
    pub(crate) async fn run_schedule_driver(&self) -> Result<(), RuntimeManagerError> {
        let _permit = {
            let state = self.registry.0.lock().expect("registry mutex");
            if !state.schedule_open {
                return Ok(());
            }
            if *state.schedule_drivers.borrow() != 0 {
                return Err(error("Scheduler driver already running"));
            }
            state.schedule_drivers.send_replace(1);
            DriverPermit(state.schedule_drivers.clone())
        };
        let mut index: HashMap<ConversationId, Indexed> = HashMap::new();
        let mut scan: Option<VecDeque<ScheduleBinding>> = None;
        let mut next_scan = 0;
        let mut pass_start = 0;
        loop {
            if self.schedule_stop.is_cancelled() {
                return Ok(());
            }
            let now = self.clock.now_millis();
            if scan.is_none() && now >= next_scan {
                let inventory = self.sessions.catalog.lock().await.schedule_bindings();
                let live: HashSet<_> = inventory
                    .iter()
                    .map(|binding| binding.conversation_id.clone())
                    .collect();
                index.retain(|id, _| live.contains(id));
                let mut diagnostics = self
                    .schedule_diagnostics
                    .lock()
                    .expect("Scheduler diagnostics");
                diagnostics.unavailable.retain(|id, _| live.contains(id));
                diagnostics.inventory_size = inventory.len();
                drop(diagnostics);
                scan = Some(inventory.into());
                pass_start = now;
            }
            if let Some(inventory) = scan.as_mut() {
                for _ in 0..64 {
                    let Some(binding) = inventory.pop_front() else {
                        break;
                    };
                    let result = tokio::select! {()=self.schedule_stop.cancelled()=>return Ok(()),result=self.schedule_operation(&binding.session_id,&binding.node_id,ScheduleOperation::Summary)=>result};
                    let mut diagnostics = self
                        .schedule_diagnostics
                        .lock()
                        .expect("Scheduler diagnostics");
                    diagnostics.stores_visited = diagnostics.stores_visited.saturating_add(1);
                    match result {
                        Ok(ScheduleOperationResult::Summary(summary)) => {
                            diagnostics.unavailable.remove(&binding.conversation_id);
                            let entry = index
                                .entry(binding.conversation_id.clone())
                                .or_insert_with(|| Indexed {
                                    binding: binding.clone(),
                                    summary: summary.clone(),
                                    retry_at: 0,
                                    cursor: 0,
                                });
                            if summary.generation >= entry.summary.generation {
                                entry.summary = summary;
                                entry.binding = binding;
                            }
                        }
                        Err(e) => {
                            diagnostics.unavailable.insert(
                                binding.conversation_id.clone(),
                                e.to_string().chars().take(256).collect(),
                            );
                            index.remove(&binding.conversation_id);
                        }
                        _ => unreachable!("typed summary"),
                    }
                }
                if inventory.is_empty() {
                    scan = None;
                    next_scan = self.clock.now_millis().saturating_add(30_000);
                    {
                        let mut diagnostics = self
                            .schedule_diagnostics
                            .lock()
                            .expect("Scheduler diagnostics");
                        diagnostics.passes_completed =
                            diagnostics.passes_completed.saturating_add(1);
                        diagnostics.last_pass_ms =
                            self.clock.now_millis().saturating_sub(pass_start);
                        diagnostics.indexed_conversations = index.len();
                    }
                    #[cfg(test)]
                    {
                        let gate = self
                            .registry
                            .0
                            .lock()
                            .expect("registry mutex")
                            .schedule_scan_done
                            .clone();
                        gate.park().await;
                    }
                }
            }
            let utc = self.schedule_clock.now();
            let now = self.clock.now_millis();
            let mut ready: Vec<_> = index
                .iter()
                .filter(|(_, entry)| {
                    entry.retry_at <= now
                        && (entry.summary.next_due_utc.is_some_and(|due| due <= utc)
                            || !entry.summary.pending.is_empty())
                })
                .map(|(id, entry)| (id.clone(), entry.clone()))
                .collect();
            ready.sort_by(|(left, a), (right, b)| {
                a.summary
                    .next_due_utc
                    .cmp(&b.summary.next_due_utc)
                    .then_with(|| left.cmp(right))
            });
            for (id, mut entry) in ready.into_iter().take(64) {
                let work = self.drive_schedule_entry(&mut entry);
                tokio::select! {()=self.schedule_stop.cancelled()=>return Ok(()),()=work=>{}}
                entry.retry_at = self.clock.now_millis().saturating_add(30_000);
                index.insert(id, entry);
            }
            if scan.is_some() {
                tokio::task::yield_now().await;
                continue;
            }
            let now = self.clock.now_millis();
            let utc = self.schedule_clock.now();
            let mut deadline = next_scan.min(now.saturating_add(30_000));
            for entry in index.values() {
                let due = entry.summary.next_due_utc.map(|due| {
                    now.saturating_add(u64::try_from((due - utc).num_milliseconds()).unwrap_or(0))
                });
                if let Some(due) = due {
                    deadline = deadline.min(due.max(entry.retry_at));
                }
                if !entry.summary.pending.is_empty() {
                    deadline = deadline.min(entry.retry_at);
                }
            }
            let wait = self.clock.wait_until_millis(deadline);
            #[cfg(test)]
            if let Some(observer) = self
                .registry
                .0
                .lock()
                .expect("registry mutex")
                .schedule_waiting
                .clone()
            {
                observer.send_replace(deadline);
            }
            tokio::select! {()=self.schedule_stop.cancelled()=>return Ok(()),()=self.schedule_changed.notified()=>{next_scan=0;},()=wait=>{}}
        }
    }
    async fn drive_schedule_entry(&self, entry: &mut Indexed) {
        let binding = &entry.binding;
        if entry
            .summary
            .next_due_utc
            .is_some_and(|due| due <= self.schedule_clock.now())
        {
            let mut schedules = Vec::new();
            let mut after = 0;
            // At most 256 nonterminal plus 200 retained terminal definitions.
            for _ in 0..5 {
                let Ok(ScheduleOperationResult::List(page)) = self
                    .schedule_operation(
                        &binding.session_id,
                        &binding.node_id,
                        ScheduleOperation::List {
                            after_ordinal: after,
                            limit: 100,
                        },
                    )
                    .await
                else {
                    return;
                };
                let Some(last) = page.last() else {
                    break;
                };
                after = last.id.ordinal;
                let complete = page.len() < 100;
                schedules.extend(page.into_iter().filter(|schedule| {
                    schedule.state == ScheduleState::Enabled
                        && schedule
                            .next_due_utc
                            .is_some_and(|due| due <= self.schedule_clock.now())
                }));
                if complete {
                    break;
                }
            }
            schedules.sort_by(|a, b| {
                a.next_due_utc
                    .cmp(&b.next_due_utc)
                    .then_with(|| a.id.ordinal.cmp(&b.id.ordinal))
            });
            if !schedules.is_empty() {
                let count = schedules.len();
                schedules.rotate_left(entry.cursor % count);
                entry.cursor = (entry.cursor + 64) % count;
            }
            for schedule in schedules.into_iter().take(64) {
                if self.schedule_stop.is_cancelled() {
                    return;
                }
                let result = self
                    .schedule_operation(
                        &binding.session_id,
                        &binding.node_id,
                        ScheduleOperation::Admit {
                            id: schedule.id,
                            expected_revision: schedule.revision,
                        },
                    )
                    .await;
                if let Err(e) = result {
                    tracing::warn!(%e,"scheduled admission refused");
                    break;
                }
            }
        }
        if let Ok(ScheduleOperationResult::Summary(summary)) = self
            .schedule_operation(
                &binding.session_id,
                &binding.node_id,
                ScheduleOperation::Summary,
            )
            .await
        {
            entry.summary = summary;
            if !entry.summary.pending.is_empty() {
                // Summary verifies native Scheduler correlation and the exact pending target.
                // A conflicting resident node is never evicted or selected by this operation.
                let _ = self
                    .activate_scheduled_target(binding, &entry.summary.pending[0])
                    .await;
            }
        }
    }
}
