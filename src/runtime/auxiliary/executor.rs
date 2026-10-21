//! Native Call ownership, close-fenced Compaction children and physical joins.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};

use tokio::sync::{Notify, oneshot};

use crate::context::compaction_budget::CompactionBudget;
use crate::context::{ContextError, ContextErrorKind};
use crate::durable::{ConversationStore, auxiliary::AuxiliarySettlementCommit};
use crate::model::{
    AuxiliaryCallStart, AuxiliaryRequestStart, ModelRequest, ModelTimeoutPolicy,
    ResolvedModelInvocation,
};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AuxiliaryCallId, RequestId};
use crate::runtime::monotonic::MonotonicClock;
use crate::runtime::types::{
    ConversationLifecycle, DurabilityFailureCommit, DurabilityGate, DurableOperation,
    LifecycleAdmission, RuntimeClock,
};

use super::capacity::{CallPermit, ConversationCapacity};
use super::stream::{AuxiliaryTextOutput, SealedExecution};
use super::writer::JoinedAuxiliaryWriter;

#[derive(Clone)]
pub(crate) struct AuxiliaryExecutor(Arc<ExecutorInner>);

pub(super) struct ExecutorInner {
    resources: Arc<ExecutionResources>,
    capacity: ConversationCapacity,
    state: Mutex<RegistryState>,
    changed: Notify,
    reaper: mpsc::SyncSender<Arc<CallRecord>>,
}

pub(super) struct ExecutionResources {
    pub store: Arc<dyn ConversationStore>,
    pub clock: Arc<dyn RuntimeClock>,
    pub monotonic: Arc<dyn MonotonicClock>,
    pub lifecycle: ConversationLifecycle,
    pub durability: Arc<DurabilityGate>,
    pub runtime: tokio::runtime::Handle,
}

#[derive(Default)]
struct RegistryState {
    closed: bool,
    calls: BTreeMap<AuxiliaryCallId, Arc<CallRecord>>,
    // New admission closes on the first failure, bounding these by live Calls.
    exited_obligations: Vec<ExitedObligation>,
}

struct ExitedObligation {
    _intent: Arc<super::writer::RequiredWrite>,
    _evidence: Option<super::writer::WriteEvidence>,
    _failure: crate::runtime::types::DurabilityFailure,
}

pub(crate) struct AuxiliaryScope {
    pub(super) budget: CompactionBudget,
    pub(super) cancellation: CancellationSignal,
    state: Mutex<ScopeState>,
    changed: Notify,
}

#[derive(Default)]
struct ScopeState {
    closed: bool,
    children: BTreeSet<AuxiliaryCallId>,
}

#[derive(Default)]
struct RecordState {
    submitted: bool,
    dispatched: bool,
    sealed: bool,
    permit: Option<Arc<CallPermit>>,
    admission: Option<LifecycleAdmission>,
    worker: Option<std::thread::JoinHandle<()>>,
}

pub(super) struct CallRecord {
    pub executor: Arc<ExecutionResources>,
    registry: Weak<ExecutorInner>,
    reaper: mpsc::SyncSender<Arc<CallRecord>>,
    pub scope: Arc<AuxiliaryScope>,
    pub call_id: AuxiliaryCallId,
    pub request_id: RequestId,
    pub cancellation: CancellationSignal,
    state: Mutex<RecordState>,
    completion: Mutex<Option<oneshot::Sender<Result<ConfirmedExecution, ContextError>>>>,
    exit: Mutex<Option<CallExit>>,
    writer: Mutex<Option<super::writer::AuxiliaryWriter>>,
}

/// Prepared native ownership; dropping it before submission unwinds admission.
pub(crate) struct AuxiliaryReservation {
    record: Arc<CallRecord>,
}

pub(crate) struct FrozenCall {
    pub start: AuxiliaryCallStart,
    pub request: ModelRequest,
    pub invocation: ResolvedModelInvocation,
    pub timeout: ModelTimeoutPolicy,
    pub resource_revision: u64,
}

pub(super) fn invocation_fingerprint(
    invocation: &crate::model::ModelInvocationConfig,
    resource_revision: u64,
) -> Result<crate::model::AuxiliaryFingerprint, ContextError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(invocation, resource_revision)).map_err(|error| {
        ContextError::new(ContextErrorKind::AuxiliaryNotStarted, error.to_string())
    })?;
    crate::model::AuxiliaryFingerprint::new(format!("{:x}", Sha256::digest(bytes)))
        .map_err(|error| ContextError::new(ContextErrorKind::AuxiliaryNotStarted, error))
}

#[derive(Debug)]
pub(crate) struct ConfirmedExecution {
    pub output: Result<AuxiliaryTextOutput, crate::model::ModelError>,
    pub settlement: Box<AuxiliarySettlementCommit>,
}

pub(super) struct CallExit {
    pub joined: Option<JoinedAuxiliaryWriter>,
    pub result: Result<ConfirmedExecution, ContextError>,
    pub durability_failed: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl AuxiliaryExecutor {
    #[cfg(test)]
    pub(crate) fn for_test(
        store: Arc<dyn ConversationStore>,
        monotonic: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ContextError> {
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        Self::new(
            store,
            Arc::new(crate::runtime::types::SystemClock),
            monotonic,
            lifecycle,
            Arc::new(DurabilityGate::new()),
            super::capacity::ProcessCapacity::new().conversation(),
        )
    }
    pub(crate) fn new(
        store: Arc<dyn ConversationStore>,
        clock: Arc<dyn RuntimeClock>,
        monotonic: Arc<dyn MonotonicClock>,
        lifecycle: ConversationLifecycle,
        durability: Arc<DurabilityGate>,
        capacity: ConversationCapacity,
    ) -> Result<Self, ContextError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            ContextError::new(ContextErrorKind::AuxiliaryNotStarted, error.to_string())
        })?;
        Ok(Self(Arc::new(ExecutorInner {
            resources: Arc::new(ExecutionResources {
                store,
                clock,
                monotonic,
                lifecycle,
                durability,
                runtime,
            }),
            capacity,
            state: Mutex::default(),
            changed: Notify::new(),
            reaper: process_reaper()?,
        })))
    }

    pub(crate) fn scope(
        budget: CompactionBudget,
        cancellation: &CancellationSignal,
    ) -> Arc<AuxiliaryScope> {
        Arc::new(AuxiliaryScope {
            budget,
            cancellation: cancellation.child(),
            state: Mutex::default(),
            changed: Notify::new(),
        })
    }

    pub(crate) fn reserve(
        &self,
        scope: &Arc<AuxiliaryScope>,
    ) -> Result<AuxiliaryReservation, ContextError> {
        self.reserve_class(scope, true)
    }

    #[cfg(test)]
    fn reserve_general(
        &self,
        scope: &Arc<AuxiliaryScope>,
    ) -> Result<AuxiliaryReservation, ContextError> {
        self.reserve_class(scope, false)
    }

    fn reserve_class(
        &self,
        scope: &Arc<AuxiliaryScope>,
        compaction: bool,
    ) -> Result<AuxiliaryReservation, ContextError> {
        if !scope.budget.uses_clock(&self.0.resources.monotonic) {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryNotStarted,
                "auxiliary operation uses another monotonic clock domain",
            ));
        }
        let permit = (if compaction {
            self.0.capacity.reserve_compaction()
        } else {
            self.0.capacity.reserve_general()
        })
        .map_err(|busy| ContextError::new(ContextErrorKind::AuxiliaryBusy, busy.to_string()))?;
        self.0
            .resources
            .durability
            .with_healthy_commit(|| {
                self.0
                    .resources
                    .lifecycle
                    .try_admit_running_commit(|admission| {
                        scope
                            .cancellation
                            .with_active(|| {
                                scope.budget.check(&scope.cancellation)?;
                                let mut parent = lock(&scope.state);
                                let mut registry = lock(&self.0.state);
                                if parent.closed || registry.closed {
                                    return Err(closed());
                                }
                                let call_id = AuxiliaryCallId::generate();
                                let record = Arc::new(CallRecord {
                                    executor: self.0.resources.clone(),
                                    registry: Arc::downgrade(&self.0),
                                    reaper: self.0.reaper.clone(),
                                    scope: scope.clone(),
                                    call_id: call_id.clone(),
                                    request_id: RequestId::new(format!(
                                        "auxiliary:{}",
                                        uuid::Uuid::now_v7()
                                    )),
                                    cancellation: scope.cancellation.child(),
                                    state: Mutex::new(RecordState {
                                        permit: Some(Arc::new(permit)),
                                        admission: Some(admission),
                                        ..RecordState::default()
                                    }),
                                    completion: Mutex::default(),
                                    exit: Mutex::default(),
                                    writer: Mutex::default(),
                                });
                                parent.children.insert(call_id.clone());
                                registry.calls.insert(call_id, record.clone());
                                Ok(AuxiliaryReservation { record })
                            })
                            .ok_or_else(closed)?
                    })
                    .map_err(|refused| match refused {
                        crate::runtime::types::NativeAdmissionRefusal::Busy => ContextError::new(
                            ContextErrorKind::AuxiliaryBusy,
                            "native lifecycle ownership boundary is busy",
                        ),
                        crate::runtime::types::NativeAdmissionRefusal::Closed(state) => {
                            ContextError::new(
                                ContextErrorKind::Cancelled,
                                format!("runtime native admission is {state:?}"),
                            )
                        }
                    })?
            })
            .map_err(|refused| {
                ContextError::new(
                    ContextErrorKind::AuxiliaryDurabilityFailed,
                    refused.diagnostic,
                )
            })?
    }

    pub(crate) fn begin_drain(&self) {
        let calls = {
            let mut state = lock(&self.0.state);
            state.closed = true;
            state.calls.values().cloned().collect::<Vec<_>>()
        };
        for record in calls {
            record.scope.close();
        }
    }

    pub(crate) async fn drain(&self) {
        self.begin_drain();
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if lock(&self.0.state).calls.is_empty() {
                return;
            }
            changed.await;
        }
    }
}

impl AuxiliaryScope {
    pub(crate) fn authorize_commit(
        &self,
    ) -> Result<crate::runtime::cancellation::CompactionCommitGrant, ContextError> {
        self.cancellation
            .authorize_compaction_commit_with(self.budget.operation_id().clone(), || {
                let state = lock(&self.state);
                !state.closed
                    && state.children.is_empty()
                    && self.budget.check(&self.cancellation).is_ok()
            })
            .ok_or_else(|| {
                self.budget
                    .check(&self.cancellation)
                    .err()
                    .unwrap_or_else(closed)
            })
    }
    pub(crate) fn close(&self) {
        self.cancellation.cancel_with(|| {
            lock(&self.state).closed = true;
        });
        self.changed.notify_waiters();
    }

    pub(crate) async fn wait_closed(&self) {
        self.close();
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if lock(&self.state).children.is_empty() {
                return;
            }
            changed.await;
        }
    }
}

impl Drop for AuxiliaryReservation {
    fn drop(&mut self) {
        let mut state = lock(&self.record.state);
        if state.submitted {
            return;
        }
        // No execution or writer exists before submission. The Context
        // preparation future has exited before this reservation is dropped.
        let permit = state.permit.take();
        let admission = state.admission.take();
        drop(state);
        drop(permit);
        drop(admission);
        self.record.retire();
    }
}

impl AuxiliaryReservation {
    pub(crate) fn call_id(&self) -> &AuxiliaryCallId {
        &self.record.call_id
    }

    #[allow(clippy::too_many_lines)] // Register the real join before releasing the native startup gate.
    pub(crate) fn submit(
        self,
        frozen: FrozenCall,
    ) -> Result<oneshot::Receiver<Result<ConfirmedExecution, ContextError>>, ContextError> {
        let handle = self.record.executor.runtime.clone();
        let record = self.record.clone();
        let mut state = lock(&record.state);
        if state.submitted {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliarySupervisionFailed,
                "Call already submitted",
            ));
        }
        record.scope.budget.check(&record.cancellation)?;
        let (sender, receiver) = oneshot::channel();
        let (release, admitted) = mpsc::sync_channel(1);
        *lock(&record.completion) = Some(sender);
        let owned = record.clone();
        let worker = std::thread::Builder::new()
            .name("auxiliary-owner".into())
            .spawn(move || {
                // The native record owns this thread's actual join before it can
                // create a writer, commit a fact or enter an adapter.
                if admitted.recv().is_err() {
                    return;
                }
                let permit = lock(&owned.state)
                    .permit
                    .as_ref()
                    .expect("registered Call permit")
                    .clone();
                let mut exit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::execution::execute(&owned, permit, frozen, &handle)
                }))
                .unwrap_or_else(|_| CallExit {
                    joined: None,
                    result: Err(ContextError::new(
                        ContextErrorKind::AuxiliarySupervisionFailed,
                        "native auxiliary supervision panicked",
                    )),
                    durability_failed: true,
                });
                // Even an unexpected panic cannot drop a live writer's physical
                // ownership. The native record retains it outside execution.
                if exit.joined.is_none() {
                    exit.joined = owned
                        .take_writer()
                        .map(super::writer::AuxiliaryWriter::join);
                }
                *lock(&owned.exit) = Some(exit);
                let reaper = owned.reaper.clone();
                // At most 32 admitted Calls can await reaping. This bounded
                // cleanup channel is never a provider execution waiting queue.
                let _ = reaper.send(owned);
            })
            .map_err(|error| {
                ContextError::new(
                    ContextErrorKind::AuxiliaryNotStarted,
                    format!("cannot start auxiliary owner: {error}"),
                )
            })?;
        state.worker = Some(worker);
        state.submitted = true;
        drop(state);
        release
            .send(())
            .expect("native owner waits for registered join");
        Ok(receiver)
    }
}

impl CallRecord {
    pub(super) fn install_writer(&self, writer: super::writer::AuxiliaryWriter) {
        *lock(&self.writer) = Some(writer);
    }
    pub(super) fn take_writer(&self) -> Option<super::writer::AuxiliaryWriter> {
        lock(&self.writer).take()
    }
    pub(super) fn pending_write(&self) -> Option<super::writer::WriteEvidence> {
        lock(&self.writer)
            .as_ref()
            .and_then(super::writer::AuxiliaryWriter::wait_pending)
    }
    pub(super) fn request_start(&self, model: String) -> AuxiliaryRequestStart {
        AuxiliaryRequestStart {
            call_id: self.call_id.clone(),
            ordinal: 0,
            request_id: self.request_id.clone(),
            model,
        }
    }

    pub(super) fn grant_dispatch(&self) -> Result<(), ContextError> {
        self.active_grant(|| {
            let mut state = lock(&self.state);
            if state.dispatched || state.sealed {
                return Err(ContextError::new(
                    ContextErrorKind::AuxiliarySupervisionFailed,
                    "auxiliary dispatch already consumed",
                ));
            }
            state.dispatched = true;
            Ok(())
        })
    }

    pub(super) fn seal_success(
        &self,
        fold: super::stream::AuxiliaryStreamFold,
    ) -> (SealedExecution, Option<ContextError>) {
        let mut fold = Some(fold);
        match self.active_grant(|| {
            lock(&self.state).sealed = true;
            Ok(fold
                .take()
                .expect("one success grant")
                .finish(self.executor.monotonic.now_millis()))
        }) {
            Ok(sealed) => (sealed, None),
            Err(error) => {
                let sealed = fold.take().expect("refused grant retains fold").fail(
                    super::execution::control_error(&error),
                    self.executor.monotonic.now_millis(),
                );
                (sealed, Some(error))
            }
        }
    }

    fn active_grant<T>(
        &self,
        grant: impl FnOnce() -> Result<T, ContextError>,
    ) -> Result<T, ContextError> {
        // Only the native owner thread can wait for lifecycle ownership;
        // it never retains the durability mutex during that wait.
        self.executor
            .lifecycle
            .with_running_commit(|| {
                self.executor
                    .durability
                    .with_healthy_commit(|| {
                        self.cancellation
                            .with_active(|| {
                                self.scope.budget.check(&self.cancellation)?;
                                let parent = lock(&self.scope.state);
                                if parent.closed {
                                    return Err(closed());
                                }
                                grant()
                            })
                            .ok_or_else(closed)?
                    })
                    .map_err(|refused| {
                        ContextError::new(
                            ContextErrorKind::AuxiliaryDurabilityFailed,
                            refused.diagnostic,
                        )
                    })?
            })
            .map_err(|_| closed())?
    }

    fn retire(&self) {
        if let Some(registry) = self.registry.upgrade() {
            lock(&registry.state).calls.remove(&self.call_id);
            registry.changed.notify_waiters();
        }
        lock(&self.scope.state).children.remove(&self.call_id);
        self.scope.changed.notify_waiters();
    }

    fn reap(&self) {
        let worker = lock(&self.state)
            .worker
            .take()
            .expect("registered native join");
        let panicked = worker.join().is_err();
        let mut exit = lock(&self.exit).take().unwrap_or_else(|| CallExit {
            joined: None,
            result: Err(ContextError::new(
                ContextErrorKind::AuxiliarySupervisionFailed,
                "native auxiliary owner exited without cleanup evidence",
            )),
            durability_failed: true,
        });
        if panicked {
            exit.durability_failed = true;
            if exit.result.is_ok() {
                exit.result = Err(ContextError::new(
                    ContextErrorKind::AuxiliarySupervisionFailed,
                    "native auxiliary owner panicked after execution",
                ));
            }
        }
        if exit.durability_failed {
            let diagnostic = exit.result.as_ref().err().map_or_else(
                || "auxiliary native supervision failed".into(),
                |error| error.message.clone(),
            );
            let failure = self
                .executor
                .durability
                .commit_failure(DurableOperation::EventJournal, diagnostic);
            let failure = match failure {
                DurabilityFailureCommit::Pending(failure)
                | DurabilityFailureCommit::Committed(failure)
                | DurabilityFailureCommit::AlreadyFailed(failure) => failure,
            };
            if let Some(joined) = exit.joined.as_ref()
                && let Some(intent) = &joined.intent
                && let Some(registry) = self.registry.upgrade()
            {
                lock(&registry.state)
                    .exited_obligations
                    .push(ExitedObligation {
                        _intent: intent.clone(),
                        _evidence: joined.evidence.clone(),
                        _failure: failure,
                    });
            }
        }
        // The thread, writer and exact confirmation have exited. Unresolved
        // intent is now under the native failed-durability owner above.
        drop(exit.joined.take());
        drop(lock(&self.state).permit.take());
        drop(lock(&self.state).admission.take());
        self.retire();
        if let Some(completion) = lock(&self.completion).take() {
            let _ = completion.send(exit.result);
        }
    }
}

fn process_reaper() -> Result<mpsc::SyncSender<Arc<CallRecord>>, ContextError> {
    struct ProcessReaper {
        sender: mpsc::SyncSender<Arc<CallRecord>>,
        _worker: std::thread::JoinHandle<()>,
    }
    static REAPER: OnceLock<Mutex<Option<ProcessReaper>>> = OnceLock::new();
    let mut installed = lock(REAPER.get_or_init(Mutex::default));
    if let Some(reaper) = installed.as_ref() {
        return Ok(reaper.sender.clone());
    }
    let (sender, incoming) = mpsc::sync_channel::<Arc<CallRecord>>(32);
    let worker = std::thread::Builder::new()
        .name("auxiliary-reaper".into())
        .spawn(move || {
            while let Ok(record) = incoming.recv() {
                record.reap();
            }
        })
        .map_err(|error| {
            ContextError::new(
                ContextErrorKind::AuxiliaryNotStarted,
                format!("cannot start auxiliary reaper: {error}"),
            )
        })?;
    *installed = Some(ProcessReaper {
        sender: sender.clone(),
        _worker: worker,
    });
    Ok(sender)
}

fn closed() -> ContextError {
    ContextError::new(
        ContextErrorKind::Cancelled,
        "auxiliary parent or runtime admission closed",
    )
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
