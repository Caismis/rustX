//! A reserved Call's serialized required writes and actual blocking-worker join.
//!
//! The native Call owner keeps `AuxiliaryWriter` outside the model task. Reply
//! loss never owns cancellation or the worker's join. Settlement and exact
//! confirmation use the original Store rather than the cancellable start scope.

use std::sync::{Arc, Condvar, Mutex, mpsc};

use chrono::{DateTime, Utc};
use tokio::sync::oneshot;

use crate::context::{ContextError, ContextErrorKind};
use crate::durable::auxiliary::{
    AuxiliaryCallStartCommit, AuxiliaryRequestStartCommit, AuxiliarySettlementCommit,
    AuxiliaryWriteFailure,
};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEventEnvelope;
use crate::model::auxiliary::{
    AuxiliaryCallStart, AuxiliaryCommitConfirmation, AuxiliaryRequestStart,
};

use super::capacity::CallPermit;

/// The exact auxiliary operation retained before handing work to a thread.
#[derive(Debug, Clone)]
pub enum RequiredWrite {
    CallStart(Arc<AuxiliaryCallStart>, DateTime<Utc>),
    RequestStart(Arc<AuxiliaryRequestStart>, DateTime<Utc>),
    Settlement(Arc<(RuntimeEventEnvelope, RuntimeEventEnvelope)>),
    CallSettlement(Arc<RuntimeEventEnvelope>),
}

/// A confirmed semantic receipt, without another physical request or write.
#[derive(Debug, Clone)]
pub enum RequiredReceipt {
    CallStart(Box<AuxiliaryCallStartCommit>),
    RequestStart(Box<AuxiliaryRequestStartCommit>),
    Settlement(Box<AuxiliarySettlementCommit>),
}

type WriteOutcome = Arc<Result<RequiredReceipt, AuxiliaryWriteFailure>>;
pub(super) type SubmittedWrite = oneshot::Receiver<WriteOutcome>;

impl RequiredWrite {
    fn commit(
        &self,
        start_store: &dyn ConversationStore,
        settlement_store: &dyn ConversationStore,
    ) -> Result<RequiredReceipt, AuxiliaryWriteFailure> {
        match self {
            Self::CallStart(start, timestamp) => start_store
                .commit_auxiliary_call_start(start, *timestamp)
                .map(|receipt| RequiredReceipt::CallStart(Box::new(receipt))),
            Self::RequestStart(start, timestamp) => start_store
                .commit_auxiliary_request_start(start, *timestamp)
                .map(|receipt| RequiredReceipt::RequestStart(Box::new(receipt))),
            Self::Settlement(pair) => settlement_store
                .commit_auxiliary_request_settlement(pair.0.clone(), pair.1.clone())
                .map(|receipt| RequiredReceipt::Settlement(Box::new(receipt))),
            Self::CallSettlement(terminal) => settlement_store
                .commit_auxiliary_call_settlement(terminal.as_ref().clone())
                .map(|receipt| RequiredReceipt::Settlement(Box::new(receipt))),
        }
    }

    /// Confirms the original operation after its submitted writer has exited.
    ///
    /// # Errors
    /// Returns authoritative Store errors without rewriting or repairing facts.
    pub fn confirm(
        &self,
        store: &dyn ConversationStore,
    ) -> Result<AuxiliaryCommitConfirmation<RequiredReceipt>, ConversationStoreError> {
        Ok(match self {
            Self::CallStart(start, _) => {
                map_confirmation(store.confirm_auxiliary_call_start(start)?, |receipt| {
                    RequiredReceipt::CallStart(Box::new(receipt))
                })
            }
            Self::RequestStart(start, _) => {
                map_confirmation(store.confirm_auxiliary_request_start(start)?, |receipt| {
                    RequiredReceipt::RequestStart(Box::new(receipt))
                })
            }
            Self::Settlement(pair) => map_confirmation(
                store.confirm_auxiliary_settlement(&pair.0, &pair.1)?,
                |receipt| RequiredReceipt::Settlement(Box::new(receipt)),
            ),
            Self::CallSettlement(terminal) => map_confirmation(
                store.confirm_auxiliary_call_settlement(terminal)?,
                |receipt| RequiredReceipt::Settlement(Box::new(receipt)),
            ),
        })
    }
}

fn map_confirmation<T>(
    value: AuxiliaryCommitConfirmation<T>,
    wrap: impl FnOnce(T) -> RequiredReceipt,
) -> AuxiliaryCommitConfirmation<RequiredReceipt> {
    match value {
        AuxiliaryCommitConfirmation::ConfirmedCommitted(receipt) => {
            AuxiliaryCommitConfirmation::ConfirmedCommitted(wrap(receipt))
        }
        AuxiliaryCommitConfirmation::ConfirmedNotCommitted => {
            AuxiliaryCommitConfirmation::ConfirmedNotCommitted
        }
        AuxiliaryCommitConfirmation::Unknown => AuxiliaryCommitConfirmation::Unknown,
    }
}

/// Retained operation evidence survives a dropped completion receiver.
#[derive(Debug, Clone)]
pub struct WriteEvidence {
    pub intent: Arc<RequiredWrite>,
    pub outcome: Arc<Result<RequiredReceipt, AuxiliaryWriteFailure>>,
}

#[derive(Default)]
struct WriterState {
    closed: bool,
    pending: bool,
    intent: Option<Arc<RequiredWrite>>,
    completed: Option<WriteEvidence>,
    #[cfg(test)]
    after_commit: Option<(oneshot::Sender<()>, mpsc::Receiver<()>)>,
    #[cfg(test)]
    before_confirmation: Option<(oneshot::Sender<()>, mpsc::Receiver<()>)>,
}

enum Command {
    Write(
        Arc<RequiredWrite>,
        oneshot::Sender<Arc<Result<RequiredReceipt, AuxiliaryWriteFailure>>>,
    ),
    Stop,
}

/// Retained by the native Call owner, never its model future or reply receiver.
#[must_use = "the native owner must join required work before releasing the Call"]
pub struct AuxiliaryWriter {
    commands: mpsc::SyncSender<Command>,
    worker: std::thread::JoinHandle<()>,
    state: Arc<Mutex<WriterState>>,
    permit: Arc<CallPermit>,
    completed: Arc<Condvar>,
}

/// A bounded command/control view. Dropping this handle never cancels work.
#[derive(Clone)]
pub struct AuxiliaryWriterClient {
    commands: mpsc::SyncSender<Command>,
    state: Arc<Mutex<WriterState>>,
    start_store: Arc<dyn ConversationStore>,
}

/// Actual worker exit, while permits remain retained for obligation resolution.
#[must_use = "resolve required obligations or perform native durability handoff before release"]
pub struct JoinedAuxiliaryWriter {
    /// Retained even if a worker exits before it can construct completion evidence.
    pub intent: Option<Arc<RequiredWrite>>,
    pub evidence: Option<WriteEvidence>,
    pub panicked: bool,
    _permit: Arc<CallPermit>,
}

impl AuxiliaryWriter {
    /// One-shot test rendezvous after a real transaction returns successfully.
    /// Disconnecting its release simulates loss of the committed acknowledgement.
    #[cfg(test)]
    pub(super) fn hold_next_commit_for_test(&self) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
        let (entered, observed) = oneshot::channel();
        let (release, held) = mpsc::channel();
        let mut state = self.state.lock().unwrap();
        assert!(state.after_commit.is_none());
        state.after_commit = Some((entered, held));
        (observed, release)
    }

    /// Keeps the existing worker's exact-confirmation path active after SQL exit.
    #[cfg(test)]
    pub(super) fn hold_confirmation_for_test(&self) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
        let (entered, observed) = oneshot::channel();
        let (release, held) = mpsc::channel();
        let mut state = self.state.lock().unwrap();
        assert!(state.before_confirmation.is_none());
        state.before_confirmation = Some((entered, held));
        (observed, release)
    }

    /// Starts at most one physical worker for an already-reserved Call.
    ///
    /// # Errors
    /// Reports `NotStarted` if the operating system cannot create the worker.
    /// # Panics
    /// A poisoned owner mutex indicates an internal invariant violation in the worker.
    #[allow(clippy::too_many_lines)] // Required writes and their exact confirmation stay on this one reserved worker.
    pub fn start(
        store: Arc<dyn ConversationStore>,
        permit: impl Into<Arc<CallPermit>>,
    ) -> Result<(Self, AuxiliaryWriterClient), ContextError> {
        let permit = permit.into();
        let (commands, incoming) = mpsc::sync_channel(1);
        let state = Arc::new(Mutex::new(WriterState::default()));
        let start_store = store.auxiliary_writer();
        let client = AuxiliaryWriterClient {
            commands: commands.clone(),
            state: state.clone(),
            start_store: start_store.clone(),
        };
        let thread_state = state.clone();
        let completed = Arc::new(Condvar::new());
        let thread_completed = completed.clone();
        let worker = std::thread::Builder::new()
            .name("auxiliary-write".into())
            .spawn(move || {
                while let Ok(command) = incoming.recv() {
                    let Command::Write(intent, reply) = command else {
                        break;
                    };
                    let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let written = intent.commit(start_store.as_ref(), store.as_ref());
                        #[cfg(test)]
                        if written.is_ok() {
                            let gate = thread_state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .after_commit
                                .take();
                            if let Some((entered, release)) = gate {
                                let _ = entered.send(());
                                release
                                    .recv_timeout(std::time::Duration::from_secs(10))
                                    .expect("write test gate released");
                            }
                        }
                        written
                    }))
                    .unwrap_or_else(|_| {
                        Err(AuxiliaryWriteFailure::Unknown {
                            error: ConversationStoreError::Storage(
                                "auxiliary write task panicked".into(),
                            ),
                        })
                    });
                    // The synchronous transaction call/unwind has finished before
                    // an exact read is attempted. There is no replacement write.
                    let outcome = Arc::new(
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match written {
                            Err(AuxiliaryWriteFailure::Unknown { error }) => {
                                #[cfg(test)]
                                {
                                    let gate = thread_state
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .before_confirmation
                                        .take();
                                    if let Some((entered, release)) = gate {
                                        let _ = entered.send(());
                                        release
                                            .recv_timeout(std::time::Duration::from_secs(10))
                                            .expect("confirmation test gate released");
                                    }
                                }
                                match intent.confirm(store.as_ref()) {
                                    Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
                                        receipt,
                                    )) => Ok(receipt),
                                    Ok(AuxiliaryCommitConfirmation::ConfirmedNotCommitted) => {
                                        Err(AuxiliaryWriteFailure::KnownNotCommitted { error })
                                    }
                                    Ok(AuxiliaryCommitConfirmation::Unknown) => {
                                        Err(AuxiliaryWriteFailure::Unknown { error })
                                    }
                                    Err(confirmation) => Err(AuxiliaryWriteFailure::Unknown {
                                        error: ConversationStoreError::Storage(format!(
                                            "{error}; exact confirmation: {confirmation}"
                                        )),
                                    }),
                                }
                            }
                            other => other,
                        }))
                        .unwrap_or_else(|_| {
                            Err(AuxiliaryWriteFailure::Unknown {
                                error: ConversationStoreError::Storage(
                                    "auxiliary exact confirmation panicked".into(),
                                ),
                            })
                        }),
                    );
                    let mut shared = thread_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    shared.completed = Some(WriteEvidence {
                        intent,
                        outcome: outcome.clone(),
                    });
                    shared.pending = false;
                    drop(shared);
                    thread_completed.notify_all();
                    let _ = reply.send(outcome);
                }
            })
            .map_err(|error| {
                ContextError::new(
                    ContextErrorKind::AuxiliaryNotStarted,
                    format!("cannot start auxiliary writer: {error}"),
                )
            })?;
        Ok((
            Self {
                commands,
                worker,
                state,
                permit,
                completed,
            },
            client,
        ))
    }

    /// Recovers an abandoned reply only on the native owner thread. The real
    /// serialized write and confirmation finish before their evidence is read.
    pub fn wait_pending(&self) -> Option<WriteEvidence> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.pending && !self.worker.is_finished() {
            state = self
                .completed
                .wait_timeout(state, std::time::Duration::from_millis(50))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        state
            .completed
            .as_ref()
            .filter(|evidence| {
                !state.pending
                    && state
                        .intent
                        .as_ref()
                        .is_some_and(|intent| Arc::ptr_eq(intent, &evidence.intent))
            })
            .cloned()
    }

    /// Joins the real thread. Run only in the native retained blocking-join job.
    /// Outstanding completion/confirmation is not cancelled to shorten this wait.
    ///
    /// # Panics
    /// A poisoned owner mutex indicates an internal invariant violation.
    pub fn join(self) -> JoinedAuxiliaryWriter {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
        let _ = self.commands.send(Command::Stop);
        let panicked = self.worker.join().is_err();
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        JoinedAuxiliaryWriter {
            intent: state.intent.clone(),
            evidence: state.completed.clone(),
            panicked,
            _permit: self.permit,
        }
    }
}

impl AuxiliaryWriterClient {
    /// Executes one exact intent; overlapping or closed submissions are refused.
    ///
    /// # Errors
    /// Reports supervision failure if the native worker cannot return evidence.
    /// # Panics
    /// A poisoned owner mutex indicates an internal invariant violation.
    pub async fn execute(
        &self,
        intent: RequiredWrite,
    ) -> Result<Arc<Result<RequiredReceipt, AuxiliaryWriteFailure>>, ContextError> {
        self.submit(intent)?.await.map_err(|_| {
            ContextError::new(
                ContextErrorKind::AuxiliarySupervisionFailed,
                "required write completion unavailable; native owner must join and confirm",
            )
        })
    }

    /// Nonblocking submission under the native cancellation/grant boundary.
    pub(super) fn submit(&self, intent: RequiredWrite) -> Result<SubmittedWrite, ContextError> {
        let intent = Arc::new(intent);
        let (reply, completed) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.closed
                || state.pending
                || state.completed.as_ref().is_some_and(|evidence| {
                    matches!(
                        evidence.outcome.as_ref(),
                        Err(AuxiliaryWriteFailure::Unknown { .. })
                    )
                })
            {
                return Err(ContextError::new(
                    ContextErrorKind::AuxiliarySupervisionFailed,
                    "required writes must be serialized by their native Call owner",
                ));
            }
            state.pending = true;
            state.intent = Some(intent.clone());
            if self
                .commands
                .try_send(Command::Write(intent, reply))
                .is_err()
            {
                state.pending = false;
                return Err(ContextError::new(
                    ContextErrorKind::AuxiliarySupervisionFailed,
                    "required write worker unavailable",
                ));
            }
        }
        Ok(completed)
    }

    /// Cancels only preparation/start writes; existing settlement stays usable.
    pub fn cancel_start(&self) {
        self.start_store.cancel_auxiliary_writer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::SqliteConversationStore;
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryFingerprint,
        AuxiliaryInputEvidence,
    };
    use crate::runtime::auxiliary::capacity::ProcessCapacity;
    use crate::runtime::identity::{AuxiliaryCallId, ConversationId};

    fn call_start() -> Arc<AuxiliaryCallStart> {
        Arc::new(AuxiliaryCallStart {
            call_id: AuxiliaryCallId::generate(),
            purpose: AuxiliaryAttribution::new("summary").unwrap(),
            trigger: AuxiliaryAttribution::new("test").unwrap(),
            business_source: AuxiliaryBusinessSource::Independent,
            input: AuxiliaryInputEvidence::Temporary,
            semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
            invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
            selected_message_ids: Vec::new(),
        })
    }

    fn cancelled(
        store: &dyn ConversationStore,
        start: &AuxiliaryCallStart,
    ) -> RuntimeEventEnvelope {
        crate::durable::auxiliary::auxiliary_settlement_envelope(
            store.conversation_id().clone(),
            Utc::now(),
            crate::events::types::RuntimeEvent::AuxiliaryCallSettled {
                call_id: start.call_id.clone(),
                outcome: AuxiliaryCallOutcome::Cancelled,
                request_evidence: None,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn cancellation_of_start_scope_does_not_cancel_owned_settlement() {
        let store =
            Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
        let capacity = ProcessCapacity::new().conversation();
        let (owner, client) =
            AuxiliaryWriter::start(store.clone(), capacity.reserve_general().unwrap()).unwrap();
        let start = call_start();
        let receipt = client
            .execute(RequiredWrite::CallStart(start.clone(), Utc::now()))
            .await
            .unwrap();
        assert!(matches!(
            receipt.as_ref(),
            Ok(RequiredReceipt::CallStart(_))
        ));
        client.cancel_start();
        let terminal = Arc::new(cancelled(store.as_ref(), &start));
        let receipt = client
            .execute(RequiredWrite::CallSettlement(terminal.clone()))
            .await
            .unwrap();
        assert!(matches!(
            receipt.as_ref(),
            Ok(RequiredReceipt::Settlement(_))
        ));
        let joined = tokio::task::spawn_blocking(move || owner.join())
            .await
            .unwrap();
        assert!(!joined.panicked);
        assert!(matches!(
            RequiredWrite::CallSettlement(terminal)
                .confirm(store.as_ref())
                .unwrap(),
            AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
        ));
        drop(joined);
    }

    #[tokio::test]
    async fn dropped_waiter_does_not_release_a_committed_but_unacknowledged_write() {
        let store =
            Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
        let capacity = ProcessCapacity::new().conversation();
        let (owner, client) =
            AuxiliaryWriter::start(store.clone(), capacity.reserve_general().unwrap()).unwrap();
        let other_permits: Vec<_> = (0..2)
            .map(|_| capacity.reserve_general().unwrap())
            .collect();
        let start = call_start();
        let submitted = start.clone();
        let (entered, committed) = oneshot::channel();
        let (release, held) = mpsc::channel();
        client.state.lock().unwrap().after_commit = Some((entered, held));
        let waiter = tokio::spawn(async move {
            client
                .execute(RequiredWrite::CallStart(submitted, Utc::now()))
                .await
        });
        committed.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            store
                .load_auxiliary_call(&start.call_id, store.auxiliary_audit_cut().unwrap())
                .unwrap()
                .is_some(),
            "the real transaction committed before receipt loss"
        );
        assert!(capacity.reserve_general().is_err());
        let join = tokio::task::spawn_blocking(move || owner.join());
        release.send(()).unwrap();
        let joined = join.await.unwrap();
        assert!(!joined.panicked);
        assert!(matches!(
            joined.evidence.as_ref().unwrap().outcome.as_ref(),
            Ok(RequiredReceipt::CallStart(_))
        ));
        assert!(
            capacity.reserve_general().is_err(),
            "worker exit alone does not resolve the Call obligation"
        );
        let terminal = cancelled(store.as_ref(), &start);
        store.commit_auxiliary_call_settlement(terminal).unwrap();
        drop(joined);
        assert!(capacity.reserve_general().is_ok());
        drop(other_permits);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn confirmation_retains_capacity_and_exact_intent_after_sql_exit_and_waiter_loss() {
        for confirmation_panics in [false, true] {
            let store =
                Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
            let capacity = ProcessCapacity::new().conversation();
            let (owner, client) =
                AuxiliaryWriter::start(store.clone(), capacity.reserve_general().unwrap()).unwrap();
            let other_permits: Vec<_> = (0..3)
                .map(|_| capacity.reserve_compaction().unwrap())
                .collect();
            let start = call_start();
            let submitted = start.clone();
            let (entered, committed) = oneshot::channel();
            let (lose_ack, held_ack) = mpsc::channel();
            let (confirming, confirmation_entered) = oneshot::channel();
            let (release_confirmation, held_confirmation) = mpsc::channel();
            {
                let mut state = client.state.lock().unwrap();
                state.after_commit = Some((entered, held_ack));
                state.before_confirmation = Some((confirming, held_confirmation));
            }
            let waiter = tokio::spawn(async move {
                client
                    .execute(RequiredWrite::CallStart(submitted, Utc::now()))
                    .await
            });
            committed.await.unwrap();
            // The real transaction committed, then its acknowledgement was lost
            // by unwinding outside SQL. Only the exact read may resolve it.
            drop(lose_ack);
            confirmation_entered.await.unwrap();
            assert!(
                store
                    .load_auxiliary_call(&start.call_id, store.auxiliary_audit_cut().unwrap())
                    .unwrap()
                    .is_some(),
                "a separate real Store read proves the transaction lease returned"
            );
            waiter.abort();
            assert!(waiter.await.unwrap_err().is_cancelled());
            let joining = tokio::task::spawn_blocking(move || owner.join());
            assert!(
                capacity.reserve_compaction().is_err(),
                "active confirmation still owns the lane and Call"
            );
            if confirmation_panics {
                drop(release_confirmation);
            } else {
                release_confirmation.send(()).unwrap();
            }
            let joined = joining.await.unwrap();
            assert!(
                !joined.panicked,
                "confirmation failure is retained evidence, not lost worker state"
            );
            let intent = joined
                .intent
                .as_ref()
                .expect("exact operation survives confirmation failure");
            assert!(
                matches!(intent.as_ref(), RequiredWrite::CallStart(actual, _) if actual.as_ref() == start.as_ref())
            );
            let evidence = joined.evidence.as_ref().unwrap();
            if confirmation_panics {
                assert!(matches!(
                    evidence.outcome.as_ref(),
                    Err(AuxiliaryWriteFailure::Unknown { .. })
                ));
            } else {
                assert!(matches!(
                    evidence.outcome.as_ref(),
                    Ok(RequiredReceipt::CallStart(_))
                ));
            }
            assert!(
                capacity.reserve_compaction().is_err(),
                "physical exit alone cannot resolve the durable obligation"
            );
            assert!(matches!(
                intent.confirm(store.as_ref()).unwrap(),
                AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
            ));
            store
                .commit_auxiliary_call_settlement(cancelled(store.as_ref(), &start))
                .unwrap();
            drop(joined);
            assert!(capacity.reserve_compaction().is_ok());
            drop(other_permits);
        }
    }
}
