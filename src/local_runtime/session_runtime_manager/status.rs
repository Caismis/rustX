//! Process-local projection of native residency and activity. The finite cache
//! contains resident identities only; cold history never enters it.
use super::{Entry, RuntimeIncarnationId, RuntimeRegistry, SessionRuntimeManager};
use crate::runtime::identity::ConversationId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadRuntimeStatus {
    NotLoaded,
    Idle,
    Active { approval: bool, user_input: bool },
    SystemError,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadStatusObservation {
    pub thread_id: ConversationId,
    pub status: ThreadRuntimeStatus,
    pub incarnation: Option<RuntimeIncarnationId>,
    pub revision: u64,
}
#[derive(Debug, Clone)]
pub(crate) enum ThreadStatusEvent {
    Changed(ThreadStatusObservation),
    ResyncRequired,
}
#[derive(Debug, Default)]
struct Projection {
    unavailable: BTreeSet<ConversationId>,
    revision: u64,
    current: BTreeMap<ConversationId, ThreadStatusObservation>,
}
#[derive(Debug)]
pub(super) struct StatusPublisher {
    projection: Mutex<Projection>,
    pub(super) wake: tokio::sync::Notify,
    events: broadcast::Sender<ThreadStatusEvent>,
    started: std::sync::OnceLock<()>,
}
impl Default for StatusPublisher {
    fn default() -> Self {
        Self {
            projection: Mutex::default(),
            wake: tokio::sync::Notify::new(),
            events: broadcast::channel(256).0,
            started: std::sync::OnceLock::new(),
        }
    }
}

fn observation_unavailable(message: &str) -> super::RuntimeManagerError {
    super::RuntimeManagerError::Client(
        crate::runtime_client::types::RuntimeClientError::ObservationUnavailable {
            message: message.into(),
        },
    )
}

impl StatusPublisher {
    /// Residency publications never enter a runtime lock. Loaded activity is
    /// captured by that runtime's observer or by an explicitly addressed read.
    fn refresh_residency(&self, registry: &RuntimeRegistry) {
        let registry = registry.0.lock().expect("registry mutex");
        let mut projection = self.projection.lock().expect("status projection");
        let removed: Vec<_> = projection
            .current
            .keys()
            .chain(projection.unavailable.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|id| {
                matches!(
                    registry.entries.get(*id),
                    None | Some(Entry::StorageOnly(_))
                )
            })
            .cloned()
            .collect();
        for id in removed {
            self.publish(&mut projection, id, ThreadRuntimeStatus::NotLoaded, None);
        }
        for (id, entry) in registry.entries.iter() {
            let (incarnation, status) = match entry {
                Entry::Loading(_) => (
                    None,
                    ThreadRuntimeStatus::Active {
                        approval: false,
                        user_input: false,
                    },
                ),
                // Loaded is observed by its installed native observer. Its
                // scheduling delay is not evidence of active execution.
                Entry::StorageOnly(_) | Entry::Loaded(_) => continue,
                Entry::Unloading {
                    _runtime: runtime,
                    flight,
                } => (
                    Some(runtime.identity.incarnation),
                    if matches!(
                        *flight.result.borrow(),
                        Some(super::Outcome::RetirementUnproven(_))
                    ) {
                        ThreadRuntimeStatus::SystemError
                    } else {
                        ThreadRuntimeStatus::Active {
                            approval: false,
                            user_input: false,
                        }
                    },
                ),
            };
            self.publish(&mut projection, id.clone(), status, incarnation);
        }
    }
    fn capture(
        &self,
        registry: &RuntimeRegistry,
        id: &ConversationId,
    ) -> Result<ThreadStatusObservation, super::RuntimeManagerError> {
        // Only replacement of this addressed identity can invalidate a capture.
        // Finite retries return an explicit unavailable read rather than an
        // invented idle value under sustained residency churn.
        for _ in 0..2 {
            let candidate = {
                let registry = registry.0.lock().expect("registry mutex");
                match registry.entries.get(id) {
                    Some(Entry::Loaded(runtime)) => Some(runtime.clone()),
                    _ => None,
                }
            };
            let capture_guard = candidate
                .as_ref()
                .and_then(|resident| resident.status_read.try_lock().ok());
            let native = candidate
                .as_ref()
                .and_then(|resident| resident.try_observation_runtime());
            let activity = capture_guard.as_ref().and_then(|_| {
                native
                    .as_ref()
                    .and_then(crate::runtime::conversation_runtime::ConversationRuntime::activity)
            });
            let registry = registry.0.lock().expect("registry mutex");
            let (status, incarnation) = match (registry.entries.get(id), candidate.as_ref()) {
                (Some(Entry::Loaded(current)), Some(observed))
                    if Arc::ptr_eq(current, observed) =>
                {
                    let Some(activity) = activity else {
                        let mut projection = self.projection.lock().expect("status projection");
                        if projection.unavailable.insert(id.clone()) {
                            let _ = self.events.send(ThreadStatusEvent::ResyncRequired);
                        }
                        drop(projection);
                        drop(registry);
                        drop(capture_guard);
                        observed.status_wake.notify_one();
                        return Err(observation_unavailable(
                            "Native activity observation unavailable",
                        ));
                    };
                    let status = if activity.system_error {
                        ThreadRuntimeStatus::SystemError
                    } else if activity.active {
                        ThreadRuntimeStatus::Active {
                            approval: activity.waiting_on_approval,
                            user_input: activity.waiting_on_user_input,
                        }
                    } else {
                        ThreadRuntimeStatus::Idle
                    };
                    (status, Some(current.identity.incarnation))
                }
                (Some(Entry::Loaded(_)), _) | (_, Some(_)) => continue,
                (Some(Entry::Loading(_)), None) => (
                    ThreadRuntimeStatus::Active {
                        approval: false,
                        user_input: false,
                    },
                    None,
                ),
                (
                    Some(Entry::Unloading {
                        _runtime: runtime,
                        flight,
                    }),
                    None,
                ) => (
                    if matches!(
                        *flight.result.borrow(),
                        Some(super::Outcome::RetirementUnproven(_))
                    ) {
                        ThreadRuntimeStatus::SystemError
                    } else {
                        ThreadRuntimeStatus::Active {
                            approval: false,
                            user_input: false,
                        }
                    },
                    Some(runtime.identity.incarnation),
                ),
                (None | Some(Entry::StorageOnly(_)), None) => {
                    (ThreadRuntimeStatus::NotLoaded, None)
                }
            };
            let mut projection = self.projection.lock().expect("status projection");
            self.publish(&mut projection, id.clone(), status, incarnation);
            return Ok(ThreadStatusObservation {
                thread_id: id.clone(),
                status,
                incarnation,
                revision: projection.revision,
            });
        }
        Err(observation_unavailable(
            "Thread changed during status capture; reread current state",
        ))
    }
    fn publish(
        &self,
        projection: &mut Projection,
        thread_id: ConversationId,
        status: ThreadRuntimeStatus,
        incarnation: Option<RuntimeIncarnationId>,
    ) {
        let recovering = projection.unavailable.remove(&thread_id);
        if !recovering
            && (projection
                .current
                .get(&thread_id)
                .is_some_and(|old| old.status == status && old.incarnation == incarnation)
                || status == ThreadRuntimeStatus::NotLoaded
                    && !projection.current.contains_key(&thread_id))
        {
            return;
        }
        projection.revision = projection
            .revision
            .checked_add(1)
            .expect("status publication sequence exhausted");
        let event = ThreadStatusObservation {
            thread_id: thread_id.clone(),
            status,
            incarnation,
            revision: projection.revision,
        };
        if status == ThreadRuntimeStatus::NotLoaded {
            projection.current.remove(&thread_id);
        } else {
            projection.current.insert(thread_id, event.clone());
        }
        let _ = self.events.send(ThreadStatusEvent::Changed(event));
    }
}
impl SessionRuntimeManager {
    pub(crate) fn status_notifications(&self) -> broadcast::Receiver<ThreadStatusEvent> {
        self.status.events.subscribe()
    }
    pub(crate) fn start_status_observer(&self) {
        if self.status.started.set(()).is_err() {
            return;
        }
        let registry = Arc::downgrade(&self.registry);
        let mut entries = self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .entries
            .changed
            .subscribe();
        let status = self.status.clone();
        tokio::spawn(async move {
            loop {
                let changed = status.wake.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                let publisher = status.clone();
                if tokio::task::spawn_blocking(move || publisher.refresh_residency(&registry))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::select! { ()=&mut changed=>{}, result=entries.changed()=>if result.is_err(){return;} }
            }
        });
    }
    pub(super) fn prepare_status_observer(
        &self,
        resident: &Arc<super::ResidentRuntime>,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let runtime = resident.shutdown_runtime()?;
        let mut changes = runtime.activity_changes();
        let wake = resident.status_wake.clone();
        let resident = Arc::downgrade(resident);
        let registry = Arc::downgrade(&self.registry);
        let status = self.status.clone();
        let mut entries = self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .entries
            .changed
            .subscribe();
        Some(async move {
            loop {
                let Some(current_resident) = resident.upgrade() else {
                    return;
                };
                let Some(current_registry) = registry.upgrade() else {
                    return;
                };
                let current = matches!(current_registry.0.lock().expect("registry mutex").entries.get(current_resident.identity.conversation_id()),Some(Entry::Loaded(value)) if Arc::ptr_eq(value,&current_resident));
                drop(current_registry);
                drop(current_resident);
                if !current {
                    return;
                }
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                let Some(runtime) = resident.upgrade() else {
                    return;
                };
                let id = runtime.identity.conversation_id().clone();
                drop(runtime);
                let publisher = status.clone();
                match tokio::task::spawn_blocking(move || publisher.capture(&registry, &id)).await {
                    Err(_) => return,
                    Ok(Err(_)) => {
                        // capture has already published unavailable and released
                        // every native/registry/capture lock. Await availability
                        // separately so pure RPCs fail promptly and execution
                        // can finish the lock holder. This worker is the retry
                        // wake even when that holder changed no activity fact.
                        let Some(recovering) = resident.upgrade() else {
                            return;
                        };
                        if tokio::task::spawn_blocking(move || {
                            drop(
                                recovering
                                    .status_read
                                    .lock()
                                    .expect("native status capture"),
                            );
                            if let Some(native) = recovering.shutdown_runtime() {
                                native.wait_activity_ready();
                            }
                        })
                        .await
                        .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    Ok(Ok(_)) => {}
                }
                tokio::select! { ()=wake.notified()=>{}, open=changes.changed()=>if !open{return;}, result=entries.changed()=>if result.is_err(){return;} }
            }
        })
    }
    async fn capture_thread_statuses(
        &self,
        ids: Vec<ConversationId>,
    ) -> Result<
        Vec<Result<ThreadStatusObservation, super::RuntimeManagerError>>,
        super::RuntimeManagerError,
    > {
        if ids.len() > 64 {
            return Err(super::error("Thread status read limit must be at most 64"));
        }
        let registry = self.registry.clone();
        let status = self.status.clone();
        tokio::task::spawn_blocking(move || {
            ids.into_iter()
                .map(|id| status.capture(&registry, &id))
                .collect()
        })
        .await
        .map_err(super::error)
    }
    pub(crate) async fn thread_statuses(
        &self,
        ids: Vec<ConversationId>,
    ) -> Result<Vec<ThreadStatusObservation>, super::RuntimeManagerError> {
        self.capture_thread_statuses(ids)
            .await?
            .into_iter()
            .collect()
    }
    /// Catalog metadata remains readable if one resident's observation is busy.
    pub(crate) async fn thread_status_proofs(
        &self,
        ids: Vec<ConversationId>,
    ) -> Result<Vec<Option<ThreadStatusObservation>>, super::RuntimeManagerError> {
        Ok(self
            .capture_thread_statuses(ids)
            .await?
            .into_iter()
            .map(Result::ok)
            .collect())
    }
    pub(crate) async fn loaded_thread_statuses(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<ThreadStatusObservation>, Option<usize>), super::RuntimeManagerError> {
        if limit == 0 || limit > 64 {
            return Err(super::error("Thread page limit must be 1..64"));
        }
        let (ids, next) = {
            let registry = self.registry.0.lock().expect("registry mutex");
            let mut ids: Vec<_> = registry
                .entries
                .iter()
                .filter(|(_, entry)| !matches!(entry, Entry::StorageOnly(_)))
                .map(|(id, _)| id.clone())
                .collect();
            ids.sort();
            let next = offset.saturating_add(limit);
            (
                ids.iter().skip(offset).take(limit).cloned().collect(),
                (next < ids.len()).then_some(next),
            )
        };
        Ok((self.thread_statuses(ids).await?, next))
    }
}

/// The registry's only entry publication seam. Every native lifecycle mutation
/// wakes projection readers after installing its new in-memory entry.
#[derive(Debug)]
pub(super) struct ResidencyEntries {
    entries: std::collections::HashMap<ConversationId, Entry>,
    generation: u64,
    changed: watch::Sender<()>,
}
impl Default for ResidencyEntries {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            generation: 0,
            changed: watch::channel(()).0,
        }
    }
}
impl std::ops::Deref for ResidencyEntries {
    type Target = std::collections::HashMap<ConversationId, Entry>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
impl ResidencyEntries {
    pub(super) fn insert(&mut self, id: ConversationId, entry: Entry) -> Option<Entry> {
        let old = self.entries.insert(id, entry);
        self.published();
        old
    }
    pub(super) fn remove(&mut self, id: &ConversationId) -> Option<Entry> {
        let old = self.entries.remove(id);
        self.published();
        old
    }
    fn published(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("residency generation exhausted");
        self.changed.send_replace(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thread_status_publication_deduplicates_and_bounds_slow_consumer_delivery() {
        let publisher = StatusPublisher::default();
        let mut slow = publisher.events.subscribe();
        let id = ConversationId::generate();
        let mut projection = publisher.projection.lock().unwrap();
        publisher.publish(&mut projection, id.clone(), ThreadRuntimeStatus::Idle, None);
        for _ in 0..1_000 {
            publisher.publish(&mut projection, id.clone(), ThreadRuntimeStatus::Idle, None);
        }
        assert!(matches!(
            slow.try_recv().unwrap(),
            ThreadStatusEvent::Changed(ThreadStatusObservation {
                status: ThreadRuntimeStatus::Idle,
                ..
            })
        ));
        assert!(matches!(
            slow.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        for i in 0..300 {
            publisher.publish(
                &mut projection,
                id.clone(),
                if i % 2 == 0 {
                    ThreadRuntimeStatus::Active {
                        approval: false,
                        user_input: false,
                    }
                } else {
                    ThreadRuntimeStatus::Idle
                },
                None,
            );
        }
        assert!(matches!(
            slow.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        assert_eq!(projection.current.len(), 1);
        publisher.publish(&mut projection, id, ThreadRuntimeStatus::NotLoaded, None);
        assert!(
            projection.current.is_empty(),
            "retirement keeps no tombstone per historical Thread"
        );
    }
}
