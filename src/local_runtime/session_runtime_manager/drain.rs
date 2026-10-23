//! Process drain and idle reaping of resident runtimes.

use super::Entry;
use super::Flight;
use super::SessionRuntimeManager;
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl SessionRuntimeManager {
    /// Supervise the current residency set without failing fast. To prove all
    /// residency settled, the composer must also settle upstream admissions and
    /// drain a final inventory; this owner has no server lifecycle/protocol gate.
    /// # Panics
    /// Panics if the residency mutex is poisoned.
    pub async fn drain_all_runtimes(&self) -> Vec<String> {
        let readers = {
            let state = self.registry.0.lock().expect("registry mutex");
            (!state.schedule_open).then(|| {
                (
                    state.schedule_drivers.subscribe(),
                    state.schedule_reads.subscribe(),
                )
            })
        };
        if let Some((mut drivers, mut reads)) = readers {
            drivers
                .wait_for(|n| *n == 0)
                .await
                .expect("Scheduler driver owner");
            reads
                .wait_for(|n| *n == 0)
                .await
                .expect("Scheduler reader owner");
        }
        let ids: Vec<_> = self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .entries
            .keys()
            .cloned()
            .collect();
        let results = futures_util::future::join_all(ids.iter().map(|id| async move {
            self.unload(id)
                .await
                .err()
                .map(|error| format!("{id:?}: {error}"))
        }))
        .await;
        let mut failures: Vec<_> = results.into_iter().flatten().collect();
        failures.sort();
        failures
    }

    pub(crate) fn is_empty(&self) -> bool {
        let state = self.registry.0.lock().expect("registry mutex");
        state.entries.is_empty()
            && *state.schedule_reads.borrow() == 0
            && *state.schedule_drivers.borrow() == 0
            && *state.schedule_commits.borrow() == 0
    }

    /// Nonblocking inventory for a composer's forced termination report.
    pub(crate) fn unproven_resources(&self) -> String {
        let Ok(state) = self.registry.0.try_lock() else {
            return "residency snapshot unavailable (admission boundary busy)".into();
        };
        let mut resources: Vec<_> = state
            .entries
            .iter()
            .map(|(id, entry)| {
                let phase = match entry {
                    Entry::StorageOnly(_) => "StorageOnly",
                    Entry::Loading(_) => "Loading",
                    Entry::Loaded(_) => "Loaded",
                    Entry::Unloading { .. } => "Unloading",
                };
                format!("{id:?}: {phase}")
            })
            .collect();
        resources.sort();
        format!(
            "runtimes=[{}], scheduler_drivers={}, schedule_reads={}, schedule_commits={}",
            resources.join(", "),
            *state.schedule_drivers.borrow(),
            *state.schedule_reads.borrow(),
            *state.schedule_commits.borrow()
        )
    }

    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub async fn run_idle_reaper(&self, stop: tokio_util::sync::CancellationToken) {
        loop {
            self.reap_idle();
            let deadline = self.clock.now_millis().saturating_add(1000);
            let waiting = self.clock.wait_until_millis(deadline);
            #[cfg(test)]
            if let Some(observer) = self
                .registry
                .0
                .lock()
                .expect("registry mutex")
                .reaper_waiting
                .clone()
            {
                observer.send_replace(deadline);
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = waiting => {},
            }
        }
    }

    /// A bounded scan. Native probes run outside the registry lock. Only the
    /// short epoch-validated native admission claim runs inside publication.
    /// # Panics
    /// Panics if an internal ownership mutex is poisoned.
    pub fn reap_idle(&self) {
        let candidates: Vec<_> = {
            let state = self.registry.0.lock().expect("registry mutex");
            state
                .entries
                .values()
                .filter_map(|entry| match entry {
                    Entry::Loaded(runtime) => Some(runtime.clone()),
                    _ => None,
                })
                .collect()
        };
        let now = self.clock.now_millis();
        for resident in candidates {
            let activity = resident.activity.load(Ordering::Relaxed);
            let Some(native) = resident.shutdown_runtime() else {
                continue;
            };
            let epoch = native.idle_epoch().ok();
            #[cfg(test)]
            self.probe(&resident.identity.conversation)
                .idle_before_claim
                .enter();
            let mut state = self.registry.0.lock().expect("registry mutex");
            if !matches!(state.entries.get(&resident.identity.conversation), Some(Entry::Loaded(current)) if Arc::ptr_eq(current, &resident))
            {
                continue;
            }
            let mut idle = resident.idle.lock().expect("idle mutex");
            if epoch.is_none()
                || resident.external.load(Ordering::Relaxed) != 0
                || *resident.operations.borrow() != 0
                || resident.activity.load(Ordering::Relaxed) != activity
            {
                *idle = None;
                continue;
            }
            let epoch = epoch.expect("eligible epoch");
            let (since, _, _) = *idle.get_or_insert((now, epoch, activity));
            if idle
                .is_some_and(|(_, previous, operation)| previous != epoch || operation != activity)
            {
                *idle = Some((now, epoch, activity));
                continue;
            }
            if now.saturating_sub(since) < state.policy.idle_grace_ms {
                continue;
            }
            if !native.claim_idle(epoch) {
                *idle = None;
                continue;
            }
            let id = resident.identity.conversation.clone();
            let flight = Flight::new();
            state.entries.insert(
                id.clone(),
                Entry::Unloading {
                    _runtime: resident.clone(),
                    flight: flight.clone(),
                },
            );
            drop(idle);
            drop(state);
            #[cfg(test)]
            self.probe(&id).idle_after_claim.enter();
            self.spawn_unload(id, resident, flight, None);
        }
    }
}
