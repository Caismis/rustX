//! One supervised wake loop over bounded, cancellable native owner operations.
use super::{RuntimeManagerError, SessionRuntimeManager, error};
use crate::runtime::identity::ConversationId;
use crate::scheduler::{
    ScheduleAdmission, ScheduleBinding, ScheduleDriverDiagnostics, ScheduleDueCursor,
    ScheduleOperation, ScheduleOperationResult, ScheduleSourceHealth, ScheduleSummary,
};
use chrono::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use tokio::sync::watch;

pub(super) const READ_MS: u64 = 1000;
const RECONCILE_MS: u64 = 30_000;
const SCAN_BATCH: usize = 64;
const DIRTY_MS: u64 = 1000;
const DISPATCH_MS: u64 = 2000;
const ROUND_OVERHEAD_MS: u64 = 100;
const ACTION_BATCH: usize = 64;

struct DriverPermit(watch::Sender<usize>);
impl Drop for DriverPermit {
    fn drop(&mut self) {
        self.0.send_replace(0);
    }
}
struct Pass {
    pending: VecDeque<ScheduleBinding>,
    started: u64,
    bound: u64,
    visited: usize,
}
struct Indexed {
    binding: ScheduleBinding,
    summary: ScheduleSummary,
    available: bool,
    retry_at: u64,
    // Preserved across header refreshes. Never-served and then oldest-served
    // sources precede recently serviced sources, including stalled activations.
    last_served_at: Option<u64>,
    cursor: Option<ScheduleDueCursor>,
}

fn pass_bound(size: usize) -> u64 {
    let size = u64::try_from(size).unwrap_or(u64::MAX);
    let rounds = size.div_ceil(SCAN_BATCH as u64).max(1);
    READ_MS
        .saturating_add(size.saturating_mul(READ_MS))
        .saturating_add(rounds.saturating_mul(DIRTY_MS + DISPATCH_MS + ROUND_OVERHEAD_MS))
}
fn diagnostic(error: impl std::fmt::Display) -> String {
    error.to_string().chars().take(256).collect()
}
fn expired_history(summary: &ScheduleSummary, now: chrono::DateTime<chrono::Utc>) -> bool {
    summary
        .oldest_completed_utc
        .is_some_and(|at| now.signed_duration_since(at) > Duration::days(30))
}
impl SessionRuntimeManager {
    /// Native discovery health; reading it opens no stores and activates no Agent.
    /// # Panics
    /// Panics if the diagnostics mutex is poisoned.
    #[must_use]
    pub fn schedule_driver_diagnostics(&self) -> ScheduleDriverDiagnostics {
        self.schedule_diagnostics
            .lock()
            .expect("Scheduler diagnostics")
            .clone()
    }

    async fn schedule_until<T>(
        &self,
        deadline: u64,
        work: impl Future<Output = Result<T, RuntimeManagerError>>,
    ) -> Result<T, RuntimeManagerError> {
        tokio::select! {biased;
            ()=self.schedule_stop.cancelled()=>Err(error("Scheduler admission is closed")),
            ()=self.clock.wait_until_millis(deadline)=>Err(error("Scheduler operation budget exceeded")),
            result=work=>result,
        }
    }
    async fn begin_schedule_pass(
        &self,
        index: &mut HashMap<ConversationId, Indexed>,
    ) -> Result<Pass, RuntimeManagerError> {
        let started = self.clock.now_millis();
        let inventory = self
            .schedule_until(started.saturating_add(READ_MS), async {
                Ok(self.sessions.catalog.lock().await.schedule_bindings())
            })
            .await?;
        let live: HashSet<_> = inventory
            .iter()
            .map(|binding| binding.conversation_id.clone())
            .collect();
        index.retain(|id, _| live.contains(id));
        let bound = pass_bound(inventory.len());
        let mut diagnostics = self
            .schedule_diagnostics
            .lock()
            .expect("Scheduler diagnostics");
        diagnostics.sources.retain(|id, _| live.contains(id));
        diagnostics.inventory_error = None;
        diagnostics.inventory_size = inventory.len();
        diagnostics.scanned_in_pass = 0;
        diagnostics.pass_bound_ms = bound;
        diagnostics.discovery_bound_ms = bound.saturating_mul(2).saturating_add(RECONCILE_MS);
        Ok(Pass {
            pending: inventory.into(),
            started,
            bound,
            visited: 0,
        })
    }
    async fn probe_schedule_source(
        &self,
        binding: ScheduleBinding,
        index: &mut HashMap<ConversationId, Indexed>,
    ) {
        let result = self
            .schedule_operation(
                &binding.session_id,
                &binding.node_id,
                ScheduleOperation::Summary,
            )
            .await;
        let mut diagnostics = self
            .schedule_diagnostics
            .lock()
            .expect("Scheduler diagnostics");
        diagnostics.stores_visited = diagnostics.stores_visited.saturating_add(1);
        let health = diagnostics
            .sources
            .entry(binding.conversation_id.clone())
            .or_insert_with(|| ScheduleSourceHealth {
                binding: binding.clone(),
                available: false,
                last_success_utc: None,
                last_generation: None,
                next_due_utc: None,
                pending_targets: 0,
                enabled: 0,
                error: None,
                execution_wait: None,
            });
        match result {
            Ok(ScheduleOperationResult::Summary(summary)) => {
                if index
                    .get(&binding.conversation_id)
                    .is_some_and(|entry| entry.summary.generation > summary.generation)
                {
                    return;
                }
                health.available = true;
                health.error = None;
                health.last_success_utc = Some(self.schedule_clock.now());
                health.last_generation = Some(summary.generation);
                health.next_due_utc = summary.next_due_utc;
                health.pending_targets = summary.pending.len();
                health.enabled = summary.enabled;
                match index.get_mut(&binding.conversation_id) {
                    Some(entry) => {
                        if summary.generation > entry.summary.generation {
                            entry.retry_at = 0;
                        }
                        entry.summary = summary;
                        entry.available = true;
                    }
                    None => {
                        index.insert(
                            binding.conversation_id.clone(),
                            Indexed {
                                binding,
                                summary,
                                available: true,
                                retry_at: 0,
                                last_served_at: None,
                                cursor: None,
                            },
                        );
                    }
                }
            }
            Err(e) => {
                health.available = false;
                health.error = Some(diagnostic(e));
                // An unavailable source retains its last successful native facts.
                if let Some(entry) = index.get_mut(&binding.conversation_id) {
                    entry.available = false;
                }
            }
            _ => unreachable!("typed header probe"),
        }
    }
    async fn scan_schedule_batch(
        &self,
        pass: &mut Pass,
        index: &mut HashMap<ConversationId, Indexed>,
    ) {
        for _ in 0..SCAN_BATCH {
            if self.schedule_stop.is_cancelled() {
                return;
            }
            let Some(binding) = pass.pending.pop_front() else {
                break;
            };
            self.probe_schedule_source(binding, index).await;
            pass.visited += 1;
            self.schedule_diagnostics
                .lock()
                .expect("Scheduler diagnostics")
                .scanned_in_pass = pass.visited;
        }
    }
    async fn refresh_dirty_schedules(
        &self,
        index: &mut HashMap<ConversationId, Indexed>,
        outer_deadline: u64,
    ) {
        let deadline = self
            .clock
            .now_millis()
            .saturating_add(DIRTY_MS)
            .min(outer_deadline);
        for _ in 0..SCAN_BATCH {
            if self.schedule_stop.is_cancelled() || self.clock.now_millis() >= deadline {
                return;
            }
            let binding = {
                self.schedule_dirty
                    .lock()
                    .expect("Scheduler dirty set")
                    .pop_first()
                    .map(|(_, binding)| binding)
            };
            let Some(binding) = binding else {
                break;
            };
            let read = self.probe_schedule_source(binding.clone(), index);
            if self
                .schedule_until(deadline, async {
                    read.await;
                    Ok(())
                })
                .await
                .is_err()
            {
                // Preserve a notification whose bounded read did not finish.
                self.schedule_dirty
                    .lock()
                    .expect("Scheduler dirty set")
                    .entry(binding.conversation_id.clone())
                    .or_insert(binding);
                return;
            }
        }
    }
    fn schedule_wait_reason(&self, id: &ConversationId, reason: Option<String>) {
        if let Some(health) = self
            .schedule_diagnostics
            .lock()
            .expect("Scheduler diagnostics")
            .sources
            .get_mut(id)
        {
            health.execution_wait = reason;
        }
    }
    async fn prune_schedule_source(&self, entry: &mut Indexed, deadline: u64, actions: &mut usize) {
        let binding = &entry.binding;
        if expired_history(&entry.summary, self.schedule_clock.now())
            && *actions < ACTION_BATCH
            && self.clock.now_millis() < deadline
        {
            *actions += 1;
            let result = self
                .schedule_until(
                    deadline,
                    self.schedule_operation(
                        &binding.session_id,
                        &binding.node_id,
                        ScheduleOperation::Prune,
                    ),
                )
                .await;
            match result {
                Ok(ScheduleOperationResult::Pruned { generation, .. }) => {
                    entry.summary.generation = generation;
                    entry.summary.oldest_completed_utc = None;
                }
                Err(e) => self.schedule_wait_reason(&binding.conversation_id, Some(diagnostic(e))),
                _ => unreachable!("typed retention operation"),
            }
        }
    }
    async fn refresh_schedule_activation(
        &self,
        entry: &mut Indexed,
        deadline: u64,
        actions: &mut usize,
    ) {
        let binding = &entry.binding;
        if *actions < ACTION_BATCH && self.clock.now_millis() < deadline {
            let read = self.schedule_operation(
                &binding.session_id,
                &binding.node_id,
                ScheduleOperation::Summary,
            );
            if let Ok(ScheduleOperationResult::Summary(summary)) =
                self.schedule_until(deadline, read).await
            {
                if summary.generation >= entry.summary.generation {
                    entry.summary = summary;
                }
                if let Some(trigger) = entry.summary.ready.first() {
                    *actions += 1;
                    let result = self
                        .schedule_until(deadline, self.activate_scheduled_target(binding, trigger))
                        .await;
                    if let Err(e) = result {
                        self.schedule_wait_reason(
                            &binding.conversation_id,
                            Some(format!(
                                "accepted_waiting_for_activation: {}",
                                diagnostic(e)
                            )),
                        );
                    }
                }
            }
        }
    }
    async fn drive_schedule_source(&self, entry: &mut Indexed, deadline: u64, actions: &mut usize) {
        self.prune_schedule_source(entry, deadline, actions).await;
        let binding = &entry.binding;
        let mut more = false;
        if entry
            .summary
            .next_due_utc
            .is_some_and(|due| due <= self.schedule_clock.now())
            && *actions < ACTION_BATCH
            && self.clock.now_millis() < deadline
        {
            let read = self.schedule_operation(
                &binding.session_id,
                &binding.node_id,
                ScheduleOperation::DuePage {
                    now: self.schedule_clock.now(),
                    after: entry.cursor.clone(),
                },
            );
            if let Ok(ScheduleOperationResult::DuePage(page)) =
                self.schedule_until(deadline, read).await
                && page.generation >= entry.summary.generation
            {
                let mut visited = 0;
                for candidate in &page.candidates {
                    if *actions >= ACTION_BATCH || self.clock.now_millis() >= deadline {
                        break;
                    }
                    entry.cursor = Some(ScheduleDueCursor {
                        due_utc: candidate.due_utc,
                        ordinal: candidate.id.ordinal,
                    });
                    visited += 1;
                    *actions += 1;
                    let work = self.schedule_operation(
                        &binding.session_id,
                        &binding.node_id,
                        ScheduleOperation::Admit {
                            id: candidate.id.clone(),
                            expected_revision: candidate.revision,
                        },
                    );
                    let result = self.schedule_until(deadline, work).await;
                    let reason = match result {
                        Ok(ScheduleOperationResult::Admission(
                            ScheduleAdmission::WaitingForTarget,
                        )) => Some(format!(
                            "waiting_for_target: schedule {}",
                            candidate.id.ordinal
                        )),
                        Ok(ScheduleOperationResult::Admission(
                            ScheduleAdmission::SchedulerCapacity,
                        )) => Some("scheduler_capacity".into()),
                        Ok(ScheduleOperationResult::Admission(_)) => None,
                        Err(e) => {
                            self.schedule_diagnostics
                                .lock()
                                .expect("Scheduler diagnostics")
                                .dispatch_failures += 1;
                            Some(diagnostic(e))
                        }
                        _ => unreachable!("typed occurrence admission"),
                    };
                    self.schedule_wait_reason(&binding.conversation_id, reason);
                }
                more = visited < page.candidates.len() || page.next.is_some();
                if !more {
                    entry.cursor = None;
                }
            }
        }
        self.refresh_schedule_activation(entry, deadline, actions)
            .await;
        entry.retry_at = if more {
            self.clock.now_millis()
        } else {
            self.clock.now_millis().saturating_add(RECONCILE_MS)
        };
    }
    async fn dispatch_schedule_batch(
        &self,
        index: &mut HashMap<ConversationId, Indexed>,
        outer_deadline: u64,
    ) {
        let now = self.clock.now_millis();
        let utc = self.schedule_clock.now();
        let mut ready: Vec<_> = index
            .iter()
            .filter(|(_, entry)| {
                entry.available
                    && entry.retry_at <= now
                    && (entry.summary.next_due_utc.is_some_and(|due| due <= utc)
                        || !entry.summary.ready.is_empty()
                        || expired_history(&entry.summary, utc))
            })
            .map(|(id, entry)| (entry.last_served_at, entry.summary.next_due_utc, id.clone()))
            .collect();
        ready.sort();
        let deadline = now.saturating_add(DISPATCH_MS).min(outer_deadline);
        let mut actions = 0;
        for (_, _, id) in ready.into_iter().take(ACTION_BATCH) {
            if actions >= ACTION_BATCH
                || self.clock.now_millis() >= deadline
                || self.schedule_stop.is_cancelled()
            {
                break;
            }
            let Some(mut entry) = index.remove(&id) else {
                continue;
            };
            entry.last_served_at = Some(self.clock.now_millis());
            self.drive_schedule_source(&mut entry, deadline, &mut actions)
                .await;
            index.insert(id, entry);
        }
    }
    fn finish_schedule_pass(&self, pass: &Pass, index_size: usize) {
        {
            let mut diagnostics = self
                .schedule_diagnostics
                .lock()
                .expect("Scheduler diagnostics");
            diagnostics.passes_completed = diagnostics.passes_completed.saturating_add(1);
            diagnostics.last_pass_ms = self.clock.now_millis().saturating_sub(pass.started);
            diagnostics.indexed_conversations = index_size;
            if diagnostics.last_pass_ms > pass.bound {
                diagnostics.pass_overruns += 1;
            }
        }
    }

    fn schedule_deadline(&self, index: &HashMap<ConversationId, Indexed>, next_scan: u64) -> u64 {
        let now = self.clock.now_millis();
        let utc = self.schedule_clock.now();
        let mut deadline = next_scan.min(now.saturating_add(RECONCILE_MS));
        for entry in index.values().filter(|entry| entry.available) {
            if let Some(due) = entry.summary.next_due_utc {
                let due =
                    now.saturating_add(u64::try_from((due - utc).num_milliseconds()).unwrap_or(0));
                deadline = deadline.min(due.max(entry.retry_at));
            }
            if !entry.summary.ready.is_empty() || expired_history(&entry.summary, utc) {
                deadline = deadline.min(entry.retry_at);
            }
        }
        deadline
    }
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
        let mut index = HashMap::new();
        let mut pass: Option<Pass> = None;
        let mut next_scan = 0;
        loop {
            if self.schedule_stop.is_cancelled() {
                return Ok(());
            }
            if pass.is_none() && self.clock.now_millis() >= next_scan {
                match self.begin_schedule_pass(&mut index).await {
                    Ok(started) => pass = Some(started),
                    Err(e) => {
                        self.schedule_diagnostics
                            .lock()
                            .expect("Scheduler diagnostics")
                            .inventory_error = Some(diagnostic(e));
                        next_scan = self.clock.now_millis().saturating_add(RECONCILE_MS);
                    }
                }
            }
            if let Some(current) = pass.as_mut() {
                self.scan_schedule_batch(current, &mut index).await;
            }
            if pass.as_ref().is_some_and(|pass| pass.pending.is_empty()) {
                self.finish_schedule_pass(&pass.take().expect("completed pass"), index.len());
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

                next_scan = self.clock.now_millis().saturating_add(RECONCILE_MS);
            }
            let outer_deadline = if pass.is_some() { u64::MAX } else { next_scan };
            self.refresh_dirty_schedules(&mut index, outer_deadline)
                .await;
            self.dispatch_schedule_batch(&mut index, outer_deadline)
                .await;
            if pass.is_some() {
                tokio::task::yield_now().await;
                continue;
            }
            let deadline = self.schedule_deadline(&index, next_scan);
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
            tokio::select! {()=self.schedule_stop.cancelled()=>return Ok(()),()=self.schedule_changed.notified()=>{},()=wait=>{}}
        }
    }
}
