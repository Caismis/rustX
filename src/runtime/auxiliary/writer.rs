//! A reserved Call's serialized required writes and actual blocking-worker join.
//!
//! The native Call owner keeps `AuxiliaryWriter` outside the model task. Reply
//! loss never owns cancellation or the worker's join. Settlement and exact
//! confirmation use the original Store rather than the cancellable start scope.

use std::sync::{Arc, Mutex, mpsc};

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
    permit: CallPermit,
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
    pub evidence: Option<WriteEvidence>,
    pub panicked: bool,
    _permit: CallPermit,
}

impl AuxiliaryWriter {
    /// Starts at most one physical worker for an already-reserved Call.
    ///
    /// # Errors
    /// Reports `NotStarted` if the operating system cannot create the worker.
    /// # Panics
    /// A poisoned owner mutex indicates an internal invariant violation in the worker.
    pub fn start(
        store: Arc<dyn ConversationStore>,
        permit: CallPermit,
    ) -> Result<(Self, AuxiliaryWriterClient), ContextError> {
        let (commands, incoming) = mpsc::sync_channel(1);
        let state = Arc::new(Mutex::new(WriterState::default()));
        let start_store = store.auxiliary_writer();
        let client = AuxiliaryWriterClient {
            commands: commands.clone(),
            state: state.clone(),
            start_store: start_store.clone(),
        };
        let thread_state = state.clone();
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
                                .expect("auxiliary write owner")
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
                    let outcome = Arc::new(match written {
                        Err(AuxiliaryWriteFailure::Unknown { error }) => {
                            match intent.confirm(store.as_ref()) {
                                Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(receipt)) => {
                                    Ok(receipt)
                                }
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
                    });
                    let mut shared = thread_state.lock().expect("auxiliary write owner");
                    shared.completed = Some(WriteEvidence {
                        intent,
                        outcome: outcome.clone(),
                    });
                    shared.pending = false;
                    drop(shared);
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
            },
            client,
        ))
    }

    /// Joins the real thread. Run only in the native retained blocking-join job.
    /// Outstanding completion/confirmation is not cancelled to shorten this wait.
    ///
    /// # Panics
    /// A poisoned owner mutex indicates an internal invariant violation.
    pub fn join(self) -> JoinedAuxiliaryWriter {
        self.state.lock().expect("auxiliary write owner").closed = true;
        let _ = self.commands.send(Command::Stop);
        let panicked = self.worker.join().is_err();
        let evidence = self
            .state
            .lock()
            .expect("auxiliary write owner")
            .completed
            .clone();
        JoinedAuxiliaryWriter {
            evidence,
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
        let intent = Arc::new(intent);
        let (reply, completed) = oneshot::channel();
        {
            let mut state = self.state.lock().expect("auxiliary write owner");
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
        completed.await.map_err(|_| {
            ContextError::new(
                ContextErrorKind::AuxiliarySupervisionFailed,
                "required write completion unavailable; native owner must join and confirm",
            )
        })
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
}
