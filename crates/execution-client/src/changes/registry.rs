//! Owns the in-flight and retained observations of one authenticated binding.
use super::{ChangeRecording, ChangesStore};
use crate::process::Operations;
use rustx_execution_protocol::changes::{ChangeReply, ChangeRequest, MAX_CHANGE_BYTES};
use rustx_execution_protocol::process::{OperationId, ProcessOperationKey};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_RECORDS: usize = 32;
const RESULT_CLAIM_TIMEOUT: Duration = Duration::from_secs(60);

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
    Complete {
        summary_claim_until: Option<Instant>,
    },
    Failed,
}

enum Comparison {
    Pending {
        index: usize,
    },
    Ready {
        index: usize,
        reply: ChangeReply,
        claim_until: Instant,
    },
}

impl Comparison {
    fn index(&self) -> usize {
        match *self {
            Self::Pending { index } | Self::Ready { index, .. } => index,
        }
    }

    fn owns_delivery(&self, now: Instant) -> bool {
        match self {
            Self::Pending { .. } => true,
            Self::Ready { claim_until, .. } => now < *claim_until,
        }
    }
}

struct Record {
    phase: Mutex<Phase>,
    scope: Arc<ChangesStore>,
    comparison: Mutex<Option<Comparison>>,
    signal: rustx_process_supervisor::signal::ProcessSignal,
    created: Instant,
    _slot: OwnedSemaphorePermit,
}

impl Record {
    fn cancel(&self) {
        self.signal.cancel();
    }

    fn is_history(&self, now: Instant) -> bool {
        let phase = self.phase.lock().expect("Workspace observation");
        let summary_settled = match *phase {
            Phase::Complete {
                summary_claim_until,
            } => summary_claim_until.is_none_or(|until| now >= until),
            Phase::Failed => true,
            _ => false,
        };
        summary_settled
            && self
                .comparison
                .lock()
                .expect("Workspace comparison")
                .as_ref()
                .is_none_or(|comparison| !comparison.owns_delivery(now))
    }

    fn claim_summary(&self, id: OperationId) -> ChangeReply {
        match &mut *self.phase.lock().expect("Workspace observation") {
            Phase::Finalizing => ChangeReply::Pending,
            Phase::Complete {
                summary_claim_until,
            } => {
                *summary_claim_until = None;
                ChangeReply::Summary(self.scope.summary(&format!("{:032x}", id.as_u128())))
            }
            _ => ChangeReply::Unavailable,
        }
    }

    fn claim_comparison(&self, index: usize) -> ChangeReply {
        let mut comparison = self.comparison.lock().expect("Workspace comparison");
        match comparison.as_ref() {
            Some(Comparison::Pending { index: active }) if index == *active => ChangeReply::Pending,
            Some(Comparison::Ready { index: active, .. }) if index == *active => {
                let Some(Comparison::Ready { reply, .. }) = comparison.take() else {
                    unreachable!("the ready result is locked");
                };
                reply
            }
            _ => ChangeReply::Unavailable,
        }
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
            ChangeRequest::Summary { .. } => record.claim_summary(id),
            ChangeRequest::Compare { index, .. } => Self::compare(operations, &record, index, id),
            ChangeRequest::Comparison { index, .. } => record.claim_comparison(index),
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
            created: Instant::now(),
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
        let now = Instant::now();
        let oldest = records
            .iter()
            .filter(|(_, record)| Arc::strong_count(record) == 1 && record.is_history(now))
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
                Phase::Complete {
                    summary_claim_until: Some(Instant::now() + RESULT_CLAIM_TIMEOUT),
                }
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
            Phase::Complete { .. }
        ) {
            return ChangeReply::Unavailable;
        }
        {
            let mut pending = record.comparison.lock().expect("Workspace comparison");
            if let Some(comparison) = pending.as_ref()
                && comparison.owns_delivery(Instant::now())
            {
                return if comparison.index() == index {
                    ChangeReply::Accepted
                } else {
                    ChangeReply::Unavailable
                };
            }
            *pending = Some(Comparison::Pending { index });
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
            *worker.comparison.lock().expect("Workspace comparison") = Some(Comparison::Ready {
                index,
                reply,
                claim_until: Instant::now() + RESULT_CLAIM_TIMEOUT,
            });
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

    fn retain_summary(
        changes: &Changes,
        finished: OperationId,
        scratch: &Path,
    ) -> rustx_execution_protocol::changes::ChangesSummary {
        let summary = rustx_execution_protocol::changes::ChangesSummary {
            files: vec![rustx_execution_protocol::changes::ChangedFile {
                path: "/workspace/changed.bin".into(),
                display: "changed.bin".into(),
                added: 0,
                deleted: 0,
                binary: true,
                oversized: false,
            }],
            total: 1,
            added: 0,
            deleted: 0,
        };
        changes.scope.records.lock().unwrap().insert(
            format!("{:032x}", finished.as_u128()),
            Arc::new(super::super::Recorded {
                summary: summary.clone(),
                sources: vec![super::super::FileSource::Captured(
                    super::super::capture::Capture::Missing,
                    super::super::capture::Capture::Missing,
                )],
                repository: None,
                trees: None,
                _directory: Arc::new(super::super::SnapshotDirectory::create(scratch).unwrap()),
                _capture_directory: None,
            }),
        );
        summary
    }

    #[test]
    fn saturated_history_protects_unclaimed_summary_and_comparison() {
        let changes = Changes {
            slots: Arc::new(Semaphore::new(4)),
            ..Changes::default()
        };
        let now = Instant::now();
        let claim_until = now + RESULT_CLAIM_TIMEOUT;
        let make = |phase, comparison, serial| {
            Arc::new(Record {
                phase: Mutex::new(phase),
                scope: Arc::clone(&changes.scope),
                comparison: Mutex::new(comparison),
                signal: rustx_process_supervisor::signal::ProcessSignal::new(),
                created: now + Duration::from_millis(serial),
                _slot: Arc::clone(&changes.slots).try_acquire_owned().unwrap(),
            })
        };
        let live = OperationId::new();
        let compared = OperationId::new();
        let finished = OperationId::new();
        let history = OperationId::new();
        let scratch = tempfile::tempdir().unwrap();
        let summary = retain_summary(&changes, finished, scratch.path());
        let exact = ChangeReply::Comparison {
            index: 3,
            comparison: Some(rustx_execution_protocol::changes::FileComparison {
                hunks: Vec::new(),
                binary: true,
                oversized: false,
                truncated: false,
            }),
        };
        let mut records = BTreeMap::from([
            (live, make(Phase::Preparing, None, 0)),
            (
                compared,
                make(
                    Phase::Complete {
                        summary_claim_until: None,
                    },
                    Some(Comparison::Ready {
                        index: 3,
                        reply: exact.clone(),
                        claim_until,
                    }),
                    1,
                ),
            ),
            (
                finished,
                make(
                    Phase::Complete {
                        summary_claim_until: Some(claim_until),
                    },
                    None,
                    2,
                ),
            ),
            (
                history,
                make(
                    Phase::Complete {
                        summary_claim_until: None,
                    },
                    None,
                    3,
                ),
            ),
        ]);
        // Workers are gone. A new Begin at capacity must reclaim only the
        // delivered history, leaving both cross-process claims intact.
        assert_eq!(changes.slots.available_permits(), 0);
        assert!(!records[&finished].is_history(now));
        assert!(records[&finished].is_history(claim_until));
        assert!(!records[&compared].is_history(now));
        assert!(records[&compared].is_history(claim_until));
        changes.evict_completed(&mut records);
        let competing_begin = make(Phase::Preparing, None, 4);
        assert!(!records.contains_key(&history));
        changes.evict_completed(&mut records);
        assert_eq!(changes.slots.available_permits(), 0);
        assert_eq!(
            records[&compared].claim_comparison(2),
            ChangeReply::Unavailable
        );
        assert_eq!(records[&compared].claim_comparison(3), exact);
        assert_eq!(
            records[&finished].claim_summary(finished),
            ChangeReply::Summary(Some(summary))
        );
        changes.evict_completed(&mut records);
        changes.evict_completed(&mut records);
        assert_eq!(records.keys().copied().collect::<Vec<_>>(), [live]);
        assert_eq!(changes.slots.available_permits(), 2);
        drop(competing_begin);
    }
    #[test]
    fn comparison_retry_claims_the_ready_result_without_a_new_worker() {
        use rustx_execution_protocol::process::ServerIncarnation;
        use rustx_execution_protocol::{
            Admission, ClientInstanceId, EnvironmentId, Generation, PROTOCOL_VERSION, SessionId,
        };

        let workspace = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let admission = Admission {
            version: PROTOCOL_VERSION,
            server_incarnation: ServerIncarnation::generate(),
            session: SessionId::new("session"),
            environment: EnvironmentId::new("environment"),
            instance: ClientInstanceId::new("client"),
            generation: Generation::try_from(1_u64).unwrap(),
            workspace: workspace.path().canonicalize().unwrap(),
        };
        let operations = Operations::new(admission).unwrap();
        let changes = Changes::default();
        let id = OperationId::new();
        let record = Arc::new(Record {
            phase: Mutex::new(Phase::Complete {
                summary_claim_until: None,
            }),
            scope: Arc::clone(&changes.scope),
            comparison: Mutex::new(None),
            signal: operations.auxiliary_signal(),
            created: Instant::now(),
            _slot: Arc::clone(&changes.slots).try_acquire_owned().unwrap(),
        });
        retain_summary(&changes, id, scratch.path());

        assert_eq!(
            Changes::compare(&operations, &record, 0, id),
            ChangeReply::Accepted
        );
        assert_eq!(operations.settle(Duration::from_secs(5)), 0);
        let Some(Comparison::Ready {
            claim_until: original_deadline,
            ..
        }) = *record.comparison.lock().unwrap()
        else {
            panic!("the original worker already completed before the caller retries")
        };
        // Any additional worker would now produce an empty result. The
        // registered delivery must return the original populated result.
        changes.scope.release(&format!("{:032x}", id.as_u128()));
        assert_eq!(
            Changes::compare(&operations, &record, 1, id),
            ChangeReply::Unavailable
        );
        assert_eq!(
            Changes::compare(&operations, &record, 0, id),
            ChangeReply::Accepted
        );
        assert_eq!(operations.settle(Duration::from_secs(5)), 0);
        assert!(matches!(*record.comparison.lock().unwrap(),
            Some(Comparison::Ready { claim_until, .. }) if claim_until == original_deadline
        ));
        assert_eq!(
            record.claim_comparison(0),
            ChangeReply::Comparison {
                index: 0,
                comparison: Some(rustx_execution_protocol::changes::FileComparison {
                    hunks: Vec::new(),
                    binary: true,
                    oversized: false,
                    truncated: false,
                }),
            }
        );
    }
}
