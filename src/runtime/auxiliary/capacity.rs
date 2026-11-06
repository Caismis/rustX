//! Process and Conversation capacity reserved without capacity waits or partial leaks.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Shared bound for physical Calls, required writers and independent retirement.
pub(super) const PROCESS_CALL_LIMIT: usize = 32;

/// The precise nonblocking reservation that refused admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityDomain {
    ProcessTotal,
    ProcessGeneral,
    ConversationTotal,
    ConversationGeneral,
    RequiredWriter,
}

/// No auxiliary work or durable admission was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxiliaryBusy {
    pub domain: CapacityDomain,
}

impl std::fmt::Display for AuxiliaryBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "auxiliary capacity unavailable: {:?}", self.domain)
    }
}

impl std::error::Error for AuxiliaryBusy {}

/// Capacity shared by every participating Conversation in a native process.
#[derive(Debug)]
pub struct ProcessCapacity {
    // Composite reservations and their rollback are indivisible to contenders.
    reservation: Mutex<()>,
    total: Arc<Semaphore>,
    general: Arc<Semaphore>,
    writers: Arc<Semaphore>,
    changed: tokio::sync::watch::Sender<u64>,
}

impl ProcessCapacity {
    /// Creates an isolated process composition with 32 total / 24 general slots.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            reservation: Mutex::new(()),
            total: Arc::new(Semaphore::new(PROCESS_CALL_LIMIT)),
            general: Arc::new(Semaphore::new(24)),
            writers: Arc::new(Semaphore::new(PROCESS_CALL_LIMIT)),
            changed: tokio::sync::watch::channel(0).0,
        })
    }

    pub(crate) fn shared_process() -> Arc<Self> {
        static PROCESS: OnceLock<Arc<ProcessCapacity>> = OnceLock::new();
        PROCESS.get_or_init(Self::new).clone()
    }

    /// Creates one Conversation's 4 total / 3 general capacity domain.
    #[must_use]
    pub fn conversation(self: &Arc<Self>) -> ConversationCapacity {
        ConversationCapacity {
            process: self.clone(),
            total: Arc::new(Semaphore::new(4)),
            general: Arc::new(Semaphore::new(3)),
            #[cfg(test)]
            after_process_general: Arc::default(),
            #[cfg(test)]
            reservation_contended: Arc::default(),
        }
    }
}

/// One Conversation's reservations against its local and process domains.
#[derive(Clone, Debug)]
pub struct ConversationCapacity {
    process: Arc<ProcessCapacity>,
    total: Arc<Semaphore>,
    general: Arc<Semaphore>,
    #[cfg(test)]
    after_process_general: Arc<Mutex<Option<ReservationHook>>>,
    #[cfg(test)]
    reservation_contended: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

#[cfg(test)]
#[derive(Debug)]
struct ReservationHook {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

/// Dropping the release sender also unblocks a held test reservation.
#[cfg(test)]
pub(crate) struct CapacityReservationGate {
    pub(crate) entered: tokio::sync::oneshot::Receiver<()>,
    pub(crate) release: std::sync::mpsc::Sender<()>,
}

impl ConversationCapacity {
    #[cfg(test)]
    pub(crate) fn hold_after_process_general_for_test(&self) -> CapacityReservationGate {
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        *self
            .after_process_general
            .lock()
            .expect("capacity test gate") = Some(ReservationHook {
            entered,
            release: held,
        });
        CapacityReservationGate {
            entered: observed,
            release,
        }
    }

    #[cfg(test)]
    pub(crate) fn reservation_contention_for_test(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (entered, observed) = tokio::sync::oneshot::channel();
        *self
            .reservation_contended
            .lock()
            .expect("capacity contention observer") = Some(entered);
        observed
    }

    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.process.changed.subscribe()
    }
    /// Reserves ordinary auxiliary work without borrowing protected capacity.
    ///
    /// # Errors
    /// Returns the exhausted domain immediately and rolls back partial reservations.
    /// A short process lock serializes the composite check, never capacity waits.
    pub fn reserve_general(&self) -> Result<CallPermit, AuxiliaryBusy> {
        self.reserve(false)
    }

    // Only native Runtime/Compaction code can select this admission class.
    pub(crate) fn reserve_compaction(&self) -> Result<CallPermit, AuxiliaryBusy> {
        self.reserve(true)
    }

    fn reserve(&self, compaction: bool) -> Result<CallPermit, AuxiliaryBusy> {
        // Declared first, so every partial permit drops before this guard on
        // refusal or unwinding. No contender can mistake rollback for Busy.
        // The critical section never waits for permits or physical work.
        let _reservation = self.reservation_guard();
        let process_total = take(&self.process.total, CapacityDomain::ProcessTotal)?;
        let process_general = if compaction {
            None
        } else {
            Some(take(&self.process.general, CapacityDomain::ProcessGeneral)?)
        };
        #[cfg(test)]
        if !compaction {
            let hook = self
                .after_process_general
                .lock()
                .expect("capacity test gate")
                .take();
            if let Some(hook) = hook {
                let _ = hook.entered.send(());
                let _ = hook.release.recv();
            }
        }
        let conversation_total = take(&self.total, CapacityDomain::ConversationTotal)?;
        let conversation_general = if compaction {
            None
        } else {
            Some(take(&self.general, CapacityDomain::ConversationGeneral)?)
        };
        let writer = take(&self.process.writers, CapacityDomain::RequiredWriter)?;
        Ok(CallPermit {
            _writer: writer,
            _conversation_general: conversation_general,
            _conversation_total: conversation_total,
            _process_general: process_general,
            _process_total: process_total,
            _wake: CapacityRelease(self.process.changed.clone()),
        })
    }

    fn reservation_guard(&self) -> MutexGuard<'_, ()> {
        #[cfg(test)]
        match self.process.reservation.try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                if let Some(observer) = self
                    .reservation_contended
                    .lock()
                    .expect("capacity contention observer")
                    .take()
                {
                    let _ = observer.send(());
                }
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {}
        }
        self.process
            .reservation
            .lock()
            .expect("Auxiliary composite capacity reservation")
    }
}

/// Reservations retained by the native owner through execution and confirmation.
/// A consumer's reply/observer handle must never own or release this value.
#[derive(Debug)]
pub struct CallPermit {
    // Declaration order releases in reverse acquisition order.
    _writer: OwnedSemaphorePermit,
    _conversation_general: Option<OwnedSemaphorePermit>,
    _conversation_total: OwnedSemaphorePermit,
    _process_general: Option<OwnedSemaphorePermit>,
    _process_total: OwnedSemaphorePermit,
    // Actual releases only increase availability and need no reservation lock.
    // Notify last, so even a contender refused during a release sees its wake
    // after all actual permits are available.
    _wake: CapacityRelease,
}

#[derive(Debug)]
struct CapacityRelease(tokio::sync::watch::Sender<u64>);
impl Drop for CapacityRelease {
    fn drop(&mut self) {
        self.0
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn take(
    semaphore: &Arc<Semaphore>,
    domain: CapacityDomain,
) -> Result<OwnedSemaphorePermit, AuxiliaryBusy> {
    semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| AuxiliaryBusy { domain })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    // Successful workers return their permits rather than releasing them. Every
    // contender therefore observes the same admission wave, not serial reuse.
    fn contend(
        conversations: Vec<ConversationCapacity>,
        compaction: bool,
    ) -> (Vec<CallPermit>, Vec<CapacityDomain>) {
        std::thread::scope(|scope| {
            let mut releases = Vec::new();
            let workers: Vec<_> = conversations
                .into_iter()
                .map(|conversation| {
                    let (release, ready) = std::sync::mpsc::channel::<Arc<Barrier>>();
                    releases.push(release);
                    std::thread::Builder::new()
                        .spawn_scoped(scope, move || {
                            let start = ready.recv().ok()?;
                            start.wait();
                            Some(if compaction {
                                conversation.reserve_compaction()
                            } else {
                                conversation.reserve_general()
                            })
                        })
                        .expect("spawn capacity contender")
                })
                .collect();
            // No worker enters the fixed-count barrier until every spawn has
            // succeeded. A spawn panic drops these senders, so existing workers
            // exit instead of deadlocking scoped-thread cleanup at the barrier.
            let start = Arc::new(Barrier::new(workers.len() + 1));
            for release in releases {
                release.send(start.clone()).unwrap();
            }
            start.wait();
            let mut admitted = Vec::new();
            let mut refused = Vec::new();
            for worker in workers {
                match worker
                    .join()
                    .expect("capacity contender panicked")
                    .expect("capacity contender was released")
                {
                    Ok(permit) => admitted.push(permit),
                    Err(busy) => refused.push(busy.domain),
                }
            }
            (admitted, refused)
        })
    }

    fn available(conversation: &ConversationCapacity) -> [usize; 5] {
        [
            conversation.process.total.available_permits(),
            conversation.process.general.available_permits(),
            conversation.total.available_permits(),
            conversation.general.available_permits(),
            conversation.process.writers.available_permits(),
        ]
    }

    #[test]
    fn ordinary_work_leaves_one_local_compaction_slot() {
        let process = ProcessCapacity::new();
        let conversation = process.conversation();
        let ordinary: Vec<_> = (0..3)
            .map(|_| conversation.reserve_general().unwrap())
            .collect();
        assert_eq!(
            conversation.reserve_general().unwrap_err().domain,
            CapacityDomain::ConversationGeneral
        );
        let compaction = conversation.reserve_compaction().unwrap();
        assert_eq!(
            conversation.reserve_compaction().unwrap_err().domain,
            CapacityDomain::ConversationTotal
        );
        drop(ordinary);
        drop(compaction);
        assert!(conversation.reserve_general().is_ok());
    }

    #[test]
    fn conversations_share_process_limits_and_reserved_writer_progress() {
        let process = ProcessCapacity::new();
        let conversations: Vec<_> = (0..9).map(|_| process.conversation()).collect();
        let mut permits = Vec::new();
        for conversation in conversations.iter().take(8) {
            for _ in 0..3 {
                permits.push(conversation.reserve_general().unwrap());
            }
        }
        assert_eq!(
            conversations[8].reserve_general().unwrap_err().domain,
            CapacityDomain::ProcessGeneral
        );
        for conversation in conversations.iter().take(8) {
            permits.push(conversation.reserve_compaction().unwrap());
        }
        assert_eq!(
            conversations[8].reserve_compaction().unwrap_err().domain,
            CapacityDomain::ProcessTotal
        );
        drop(permits);
        assert!(conversations[8].reserve_general().is_ok());
    }

    #[test]
    fn repeated_partial_refusal_does_not_leak_process_capacity() {
        let process = ProcessCapacity::new();
        let full = process.conversation();
        let mut permits: Vec<_> = (0..3).map(|_| full.reserve_general().unwrap()).collect();
        for _ in 0..100 {
            assert!(full.reserve_general().is_err());
        }
        let others: Vec<_> = (0..7).map(|_| process.conversation()).collect();
        for conversation in &others {
            for _ in 0..3 {
                permits.push(conversation.reserve_general().unwrap());
            }
        }
        assert_eq!(permits.len(), 24);
        assert!(process.conversation().reserve_general().is_err());
    }

    #[test]
    fn simultaneous_local_contenders_keep_the_fourth_call_and_writer_protected() {
        let process = ProcessCapacity::new();
        let conversation = process.conversation();
        let (general, refused) = contend(vec![conversation.clone(); 16], false);
        assert_eq!(general.len(), 3);
        assert_eq!(refused.len(), 13);
        assert_eq!(refused, vec![CapacityDomain::ConversationGeneral; 13]);
        assert_eq!(available(&conversation), [29, 21, 1, 0, 29]);

        let (protected, refused) = contend(vec![conversation.clone(); 16], true);
        assert_eq!(protected.len(), 1);
        assert_eq!(refused, vec![CapacityDomain::ConversationTotal; 15]);
        assert_eq!(available(&conversation), [28, 21, 0, 0, 28]);

        drop(general);
        assert_eq!(available(&conversation), [31, 24, 3, 3, 31]);
        let (replacement, refused) = contend(vec![conversation.clone(); 3], false);
        assert_eq!(replacement.len(), 3);
        assert!(refused.is_empty());
        drop(protected);
        drop(replacement);
        assert_eq!(available(&conversation), [32, 24, 4, 3, 32]);
    }

    #[test]
    fn simultaneous_process_contenders_preserve_twenty_four_plus_eight_and_recover() {
        let process = ProcessCapacity::new();
        let conversations: Vec<_> = (0..40).map(|_| process.conversation()).collect();
        let (general, refused) = contend(conversations.clone(), false);
        assert_eq!(general.len(), 24);
        assert_eq!(refused.len(), 16);
        assert_eq!(refused, vec![CapacityDomain::ProcessGeneral; 16]);
        assert_eq!(process.total.available_permits(), 8);
        assert_eq!(process.general.available_permits(), 0);
        assert_eq!(process.writers.available_permits(), 8);

        let (protected, refused) = contend(conversations.clone(), true);
        assert_eq!(protected.len(), 8);
        assert_eq!(refused, vec![CapacityDomain::ProcessTotal; 32]);
        assert_eq!(process.total.available_permits(), 0);
        assert_eq!(process.writers.available_permits(), 0);

        // Releasing only the protected class cannot refund a general permit.
        drop(protected);
        let (unexpected, refused) = contend(conversations.clone(), false);
        assert!(unexpected.is_empty());
        assert_eq!(refused.len(), 40);
        assert_eq!(refused, vec![CapacityDomain::ProcessGeneral; 40]);
        assert_eq!(process.total.available_permits(), 8);
        assert_eq!(process.general.available_permits(), 0);
        assert_eq!(process.writers.available_permits(), 8);

        drop(general);
        for conversation in &conversations {
            assert_eq!(available(conversation), [32, 24, 4, 3, 32]);
        }
        let (recovered, refused) = contend(conversations, true);
        assert_eq!(recovered.len(), 32);
        assert_eq!(refused, vec![CapacityDomain::ProcessTotal; 8]);
        drop(recovered);
        assert_eq!(available(&process.conversation()), [32, 24, 4, 3, 32]);
    }

    #[test]
    fn refusal_at_every_acquisition_layer_rolls_back_all_earlier_permits() {
        for (domain, compaction) in [
            (CapacityDomain::ProcessTotal, false),
            (CapacityDomain::ProcessGeneral, false),
            (CapacityDomain::ConversationTotal, false),
            (CapacityDomain::ConversationGeneral, false),
            (CapacityDomain::RequiredWriter, false),
            (CapacityDomain::ProcessTotal, true),
            (CapacityDomain::ConversationTotal, true),
            (CapacityDomain::RequiredWriter, true),
        ] {
            let process = ProcessCapacity::new();
            let conversation = process.conversation();
            let (semaphore, limit) = match domain {
                CapacityDomain::ProcessTotal => (&process.total, 32),
                CapacityDomain::ProcessGeneral => (&process.general, 24),
                CapacityDomain::ConversationTotal => (&conversation.total, 4),
                CapacityDomain::ConversationGeneral => (&conversation.general, 3),
                CapacityDomain::RequiredWriter => (&process.writers, 32),
            };
            // Isolate each layer, including the otherwise unreachable writer
            // shortage, without replacing the production acquisition path.
            let blocker = semaphore.clone().try_acquire_many_owned(limit).unwrap();
            let before = available(&conversation);
            let (admitted, refused) = contend(vec![conversation.clone(); 3], compaction);
            assert!(admitted.is_empty(), "{domain:?}, protected={compaction}");
            assert_eq!(refused, vec![domain; 3]);
            assert_eq!(available(&conversation), before, "rollback at {domain:?}");

            drop(blocker);
            assert_eq!(available(&conversation), [32, 24, 4, 3, 32]);
            let (general, refused) = contend(vec![conversation.clone(); 3], false);
            assert_eq!(general.len(), 3);
            assert!(refused.is_empty());
            let protected = conversation.reserve_compaction().unwrap();
            assert_eq!(available(&conversation), [28, 21, 0, 0, 28]);
            drop(general);
            drop(protected);
            assert_eq!(available(&conversation), [32, 24, 4, 3, 32]);
        }
    }

    async fn observed<T>(receiver: tokio::sync::oneshot::Receiver<T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(10), receiver)
            .await
            .expect("capacity frontier reached")
            .expect("capacity frontier retained its sender")
    }

    #[tokio::test]
    async fn rollback_is_invisible_to_a_competing_conversation_at_the_process_limit() {
        let process = ProcessCapacity::new();
        let full = process.conversation();
        let mut retained: Vec<_> = (0..3).map(|_| full.reserve_general().unwrap()).collect();
        let others: Vec<_> = (0..7).map(|_| process.conversation()).collect();
        for index in 0..20 {
            retained.push(others[index / 3].reserve_general().unwrap());
        }
        assert_eq!(retained.len(), 23);
        let contender = process.conversation();
        let changes = contender.changes();
        let CapacityReservationGate { entered, release } =
            full.hold_after_process_general_for_test();
        let lock_blocked = contender.reservation_contention_for_test();
        let refusing = std::thread::spawn(move || full.reserve_general());
        observed(entered).await;
        assert_eq!(process.general.available_permits(), 0);

        let competing = contender.clone();
        let (finished, result) = tokio::sync::oneshot::channel();
        let admitted = std::thread::spawn(move || {
            let _ = finished.send(competing.reserve_general());
        });
        // B actually encountered the process reservation lock while A held
        // the last general permit; no scheduling delay stands in for ordering.
        observed(lock_blocked).await;
        assert!(!changes.has_changed().unwrap());
        release.send(()).unwrap();
        assert_eq!(
            refusing.join().unwrap().unwrap_err().domain,
            CapacityDomain::ConversationGeneral
        );
        let replacement = observed(result).await.unwrap();
        admitted.join().unwrap();

        // None of the original 23 owners released. B uses A's rollback slot,
        // without a false Busy result or a synthetic capacity notification.
        assert_eq!(available(&contender), [8, 0, 3, 2, 8]);
        assert!(!changes.has_changed().unwrap());
        drop(replacement);
        assert!(changes.has_changed().unwrap());
        assert_eq!(available(&contender), [9, 1, 4, 3, 9]);
        drop(retained);
        assert_eq!(available(&contender), [32, 24, 4, 3, 32]);
    }

    #[test]
    fn locally_full_refusals_do_not_generate_capacity_self_wakes() {
        let process = ProcessCapacity::new();
        let conversation = process.conversation();
        let retained: Vec<_> = (0..3)
            .map(|_| conversation.reserve_general().unwrap())
            .collect();
        let changes = conversation.changes();
        for _ in 0..100 {
            assert_eq!(
                conversation.reserve_general().unwrap_err().domain,
                CapacityDomain::ConversationGeneral
            );
            assert!(!changes.has_changed().unwrap());
            assert_eq!(available(&conversation), [29, 21, 1, 0, 29]);
        }
        drop(retained);
        assert!(changes.has_changed().unwrap());
    }

    #[tokio::test]
    async fn actual_release_notifies_without_waiting_for_a_composite_reservation() {
        let process = ProcessCapacity::new();
        let full = process.conversation();
        let retained: Vec<_> = (0..3).map(|_| full.reserve_general().unwrap()).collect();
        let other = process.conversation();
        let physical = other.reserve_general().unwrap();
        let mut changes = other.changes();
        let CapacityReservationGate { entered, release } =
            full.hold_after_process_general_for_test();
        let refusing = std::thread::spawn(move || full.reserve_general());
        observed(entered).await;

        let (released, observed_release) = tokio::sync::oneshot::channel();
        let retiring = std::thread::spawn(move || {
            drop(physical);
            let _ = released.send(());
        });
        observed(observed_release).await;
        retiring.join().unwrap();
        assert!(changes.has_changed().unwrap());
        assert_eq!(available(&other), [28, 20, 4, 3, 29]);
        let _ = changes.borrow_and_update();

        release.send(()).unwrap();
        assert_eq!(
            refusing.join().unwrap().unwrap_err().domain,
            CapacityDomain::ConversationGeneral
        );
        assert_eq!(available(&other), [29, 21, 4, 3, 29]);
        assert!(!changes.has_changed().unwrap());
        drop(retained);
        assert_eq!(available(&other), [32, 24, 4, 3, 32]);
    }
}
