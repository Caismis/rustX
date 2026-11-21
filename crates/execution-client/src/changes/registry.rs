//! Owns the in-flight and retained observations of one authenticated binding.
use super::{ChangeRecording, ChangesStore};
use crate::process::Operations;
use rustx_execution_protocol::changes::{ChangeReply, ChangeRequest, MAX_CHANGE_BYTES};
use rustx_execution_protocol::process::{OperationId, ProcessOperationKey};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_RECORDS: usize = 32;

pub(crate) struct Changes {
    records: Mutex<BTreeMap<OperationId, Arc<Record>>>,
    scope: Arc<ChangesStore>,
    slots: Arc<Semaphore>,
}

impl Default for Changes {
    fn default() -> Self {
        Self {
            records: Mutex::default(),
            scope: Arc::default(),
            slots: Arc::new(Semaphore::new(MAX_RECORDS)),
        }
    }
}

enum Phase {
    Preparing,
    Ready(Arc<ChangeRecording>),
    Finalizing,
    Complete,
    Failed,
}

struct Record {
    phase: Mutex<Phase>,
    scope: Arc<ChangesStore>,
    comparison: Mutex<Option<(usize, ChangeReply)>>,
    signal: rustx_process_supervisor::signal::ProcessSignal,
    created: std::time::Instant,
    _slot: OwnedSemaphorePermit,
}

impl Record {
    fn cancel(&self) {
        self.signal.cancel();
    }

    fn set_phase(&self, phase: Phase) {
        *self.phase.lock().expect("Workspace observation") = phase;
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        self.signal.cancel();
    }
}

impl Changes {
    pub(crate) fn dispatch(
        &self,
        operations: &Operations,
        workspace: &Path,
        request: ChangeRequest,
    ) -> ChangeReply {
        let key = request.key();
        if !operations.own_key(key) {
            return ChangeReply::Unavailable;
        }
        if let ChangeRequest::Begin { key } = request {
            return self.begin(operations, workspace, &key);
        }
        let id = key.operation;
        if let ChangeRequest::Release { .. } = request {
            if let Some(record) = self.records.lock().expect("Workspace records").remove(&id) {
                self.retire(id, &record);
            }
            return ChangeReply::Accepted;
        }
        let Some(record) = self
            .records
            .lock()
            .expect("Workspace records")
            .get(&id)
            .cloned()
        else {
            return ChangeReply::Unavailable;
        };
        let reply = match request {
            ChangeRequest::Baseline { .. } => {
                match &*record.phase.lock().expect("Workspace observation") {
                    Phase::Preparing => ChangeReply::Pending,
                    Phase::Ready(_) => ChangeReply::BaselineReady,
                    _ => ChangeReply::Unavailable,
                }
            }
            ChangeRequest::Capture { path, .. } => {
                let Some(_active) = operations.admit_auxiliary() else {
                    return ChangeReply::Unavailable;
                };
                Self::capture(workspace, &record, &path)
            }
            ChangeRequest::Finish { .. } => Self::finish(operations, &record),
            ChangeRequest::Summary { .. } => {
                match *record.phase.lock().expect("Workspace observation") {
                    Phase::Finalizing => ChangeReply::Pending,
                    Phase::Complete => ChangeReply::Summary(
                        record.scope.summary(&format!("{:032x}", id.as_u128())),
                    ),
                    _ => ChangeReply::Unavailable,
                }
            }
            ChangeRequest::Compare { index, .. } => Self::compare(operations, &record, index, id),
            ChangeRequest::Comparison { index, .. } => record
                .comparison
                .lock()
                .expect("Workspace comparison")
                .as_ref()
                .filter(|(active, _)| index == *active)
                .map_or(ChangeReply::Unavailable, |(_, reply)| reply.clone()),
            ChangeRequest::Begin { .. } | ChangeRequest::Release { .. } => unreachable!(),
        };
        if serde_json::to_vec(&reply).is_ok_and(|bytes| bytes.len() <= MAX_CHANGE_BYTES) {
            reply
        } else {
            ChangeReply::Unavailable
        }
    }

    fn begin(
        &self,
        operations: &Operations,
        workspace: &Path,
        key: &ProcessOperationKey,
    ) -> ChangeReply {
        let mut records = self.records.lock().expect("Workspace records");
        if records.contains_key(&key.operation) {
            return ChangeReply::Unavailable;
        }
        if self.slots.available_permits() == 0 {
            self.evict_completed(&mut records);
        }
        // Released records keep their slot while a physical worker holds them.
        let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() else {
            return ChangeReply::Unavailable;
        };
        let Ok(root) = operations.workspace_root().descend(Path::new("")) else {
            return ChangeReply::Unavailable;
        };
        let record = Arc::new(Record {
            phase: Mutex::new(Phase::Preparing),
            scope: Arc::clone(&self.scope),
            comparison: Mutex::new(None),
            signal: operations.auxiliary_signal(),
            created: std::time::Instant::now(),
            _slot: slot,
        });
        let id = key.operation;
        let worker = Arc::clone(&record);
        let workspace = workspace.to_path_buf();
        if !operations.start_auxiliary(async move {
            let recording = ChangeRecording::start(
                format!("{:032x}", id.as_u128()),
                workspace,
                Arc::new(root),
                Arc::clone(&worker.scope),
                worker.signal.child(),
            );
            recording.ready().await;
            if recording.failed.load(std::sync::atomic::Ordering::Relaxed)
                || worker.signal.is_cancelled()
            {
                worker.set_phase(Phase::Failed);
            } else {
                worker.set_phase(Phase::Ready(recording));
            }
        }) {
            return ChangeReply::Unavailable;
        }
        records.insert(id, record);
        ChangeReply::Accepted
    }

    fn evict_completed(&self, records: &mut BTreeMap<OperationId, Arc<Record>>) {
        let oldest = records
            .iter()
            .filter(|(_, record)| {
                Arc::strong_count(record) == 1
                    && matches!(
                        *record.phase.lock().expect("Workspace observation"),
                        Phase::Complete | Phase::Failed
                    )
            })
            .min_by_key(|(_, record)| record.created)
            .map(|(id, _)| *id);
        if let Some(id) = oldest
            && let Some(record) = records.remove(&id)
        {
            self.retire(id, &record);
        }
    }

    fn retire(&self, id: OperationId, record: &Record) {
        record.cancel();
        self.scope.release(&format!("{:032x}", id.as_u128()));
    }

    fn capture(workspace: &Path, record: &Record, path: &Path) -> ChangeReply {
        if path.as_os_str().is_empty() || crate::relative_entry(path).is_err() {
            return ChangeReply::Unavailable;
        }
        let phase = record.phase.lock().expect("Workspace observation");
        if let Phase::Ready(recording) = &*phase {
            recording.capture(&workspace.join(path));
            ChangeReply::Accepted
        } else {
            ChangeReply::Unavailable
        }
    }

    fn finish(operations: &Operations, record: &Arc<Record>) -> ChangeReply {
        let recording = {
            let mut phase = record.phase.lock().expect("Workspace observation");
            let Phase::Ready(recording) = &*phase else {
                return ChangeReply::Unavailable;
            };
            let recording = recording.clone();
            *phase = Phase::Finalizing;
            recording
        };
        let worker = Arc::clone(record);
        if operations.start_auxiliary(async move {
            let saved = recording.record().await.is_ok() && !worker.signal.is_cancelled();
            worker.set_phase(if saved {
                Phase::Complete
            } else {
                Phase::Failed
            });
        }) {
            ChangeReply::Accepted
        } else {
            record.set_phase(Phase::Failed);
            ChangeReply::Unavailable
        }
    }

    fn compare(
        operations: &Operations,
        record: &Arc<Record>,
        index: usize,
        id: OperationId,
    ) -> ChangeReply {
        if !matches!(
            *record.phase.lock().expect("Workspace observation"),
            Phase::Complete
        ) {
            return ChangeReply::Unavailable;
        }
        {
            let mut pending = record.comparison.lock().expect("Workspace comparison");
            if pending
                .as_ref()
                .is_some_and(|(_, reply)| *reply == ChangeReply::Pending)
            {
                return ChangeReply::Unavailable;
            }
            *pending = Some((index, ChangeReply::Pending));
        }
        let worker = Arc::clone(record);
        if operations.start_auxiliary(async move {
            let reply = match worker
                .scope
                .compare(&format!("{:032x}", id.as_u128()), index, &worker.signal)
                .await
            {
                Ok(comparison) => ChangeReply::Comparison { index, comparison },
                Err(_) => ChangeReply::Unavailable,
            };
            *worker.comparison.lock().expect("Workspace comparison") = Some((index, reply));
        }) {
            ChangeReply::Accepted
        } else {
            *record.comparison.lock().expect("Workspace comparison") = None;
            ChangeReply::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_history_releases_capacity_without_evicting_live_observers() {
        let changes = Changes {
            slots: Arc::new(Semaphore::new(3)),
            ..Changes::default()
        };
        let now = std::time::Instant::now();
        let make = |phase, elapsed| {
            Arc::new(Record {
                phase: Mutex::new(phase),
                scope: Arc::clone(&changes.scope),
                comparison: Mutex::new(None),
                signal: rustx_process_supervisor::signal::ProcessSignal::new(),
                created: now + std::time::Duration::from_millis(elapsed),
                _slot: Arc::clone(&changes.slots).try_acquire_owned().unwrap(),
            })
        };
        let live = OperationId::new();
        let compared = OperationId::new();
        let complete = OperationId::new();
        let comparison_holds_slot = make(Phase::Complete, 1);
        let mut records = BTreeMap::from([
            (live, make(Phase::Preparing, 0)),
            (compared, Arc::clone(&comparison_holds_slot)),
            (complete, make(Phase::Complete, 2)),
        ]);
        assert_eq!(changes.slots.available_permits(), 0);
        changes.evict_completed(&mut records);
        assert!(!records.contains_key(&complete));
        assert!(records.contains_key(&live));
        assert!(records.contains_key(&compared));
        assert_eq!(changes.slots.available_permits(), 1);
        drop(comparison_holds_slot);
        changes.evict_completed(&mut records);
        assert_eq!(records.keys().copied().collect::<Vec<_>>(), [live]);
        assert_eq!(changes.slots.available_permits(), 2);
    }
}
