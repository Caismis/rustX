//! One Compaction's cumulative physical-start, input-shrink and deadline budget.
//!
//! This is business accounting, not execution admission or dispatch authority.
//! Every re-planning Call shares this value; task or waiter loss cannot refund
//! a physical start whose transaction outcome has not been established.

use std::sync::{Arc, Mutex};

use crate::context::{ContextError, ContextErrorKind};
use crate::durable::auxiliary::{AuxiliaryRequestStartCommit, AuxiliaryWriteFailure};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{CompactionOperationId, RequestId};
use crate::runtime::monotonic::MonotonicClock;

/// Absolute operation eligibility, including preparation and persistence waits.
pub const COMPACTION_DEADLINE_MILLIS: u64 = 300_000;
/// Maximum confirmed or unresolved physical starts across all re-planning.
pub const COMPACTION_START_LIMIT: u8 = 12;
/// Maximum input-budget reductions across all carryover degradation.
pub const COMPACTION_SHRINK_LIMIT: u8 = 3;

#[derive(Debug, Default)]
struct Counts {
    committed: u8,
    reserved: u8,
    shrinks: u8,
}

#[derive(Debug)]
struct Operation {
    id: CompactionOperationId,
    deadline_millis: u64,
    clock: Arc<dyn MonotonicClock>,
    counts: Mutex<Counts>,
}

/// Shared budget frozen once before both Compaction re-planning loops.
#[derive(Clone, Debug)]
pub struct CompactionBudget(Arc<Operation>);

impl CompactionBudget {
    pub(crate) fn same_operation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub(crate) fn uses_clock(&self, clock: &Arc<dyn MonotonicClock>) -> bool {
        Arc::ptr_eq(&self.0.clock, clock)
    }
    pub(crate) fn wait_until_deadline(&self) -> futures_util::future::BoxFuture<'static, ()> {
        self.0.clock.wait_until_millis(self.0.deadline_millis)
    }
    /// Starts one operation's five-minute budget in the supplied clock domain.
    #[must_use]
    pub fn new(id: CompactionOperationId, clock: Arc<dyn MonotonicClock>) -> Self {
        Self(Arc::new(Operation {
            id,
            deadline_millis: clock
                .now_millis()
                .saturating_add(COMPACTION_DEADLINE_MILLIS),
            clock,
            counts: Mutex::default(),
        }))
    }

    /// The business operation shared by every child Call.
    #[must_use]
    pub fn operation_id(&self) -> &CompactionOperationId {
        &self.0.id
    }

    /// The original absolute deadline; cloning never renews eligibility.
    #[must_use]
    pub fn deadline_millis(&self) -> u64 {
        self.0.deadline_millis
    }

    /// Checks operation eligibility before preparation and checkpoint grant.
    ///
    /// # Errors
    /// Returns cancellation or absolute deadline expiry without changing counts.
    pub fn check(&self, cancellation: &CancellationSignal) -> Result<(), ContextError> {
        if cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction cancelled",
            ));
        }
        if self.0.clock.now_millis() >= self.0.deadline_millis {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDeadlineExceeded,
                "the absolute compaction deadline expired",
            ));
        }
        Ok(())
    }

    /// Reserves an allowance immediately before the native start transaction.
    ///
    /// # Errors
    /// Refuses cancellation, expiry or exhaustion, including unresolved writes.
    /// # Panics
    /// A poisoned accounting mutex indicates an internal invariant violation.
    pub fn reserve_start(
        &self,
        request_id: RequestId,
        cancellation: &CancellationSignal,
    ) -> Result<CompactionStartReservation, ContextError> {
        self.check(cancellation)?;
        let mut counts = self.0.counts.lock().expect("compaction budget lock");
        if counts.committed + counts.reserved >= COMPACTION_START_LIMIT {
            return Err(exhausted(
                "compaction exhausted its 12 physical request starts",
            ));
        }
        counts.reserved += 1;
        Ok(CompactionStartReservation {
            budget: self.clone(),
            request_id,
        })
    }

    /// Charges one input shrink to this operation, regardless of re-entry.
    ///
    /// # Errors
    /// Refuses cancellation, expiry or a fourth shrink.
    /// # Panics
    /// A poisoned accounting mutex indicates an internal invariant violation.
    pub fn take_shrink(&self, cancellation: &CancellationSignal) -> Result<(), ContextError> {
        self.check(cancellation)?;
        let mut counts = self.0.counts.lock().expect("compaction budget lock");
        if counts.shrinks >= COMPACTION_SHRINK_LIMIT {
            return Err(exhausted("compaction exhausted its three input shrinks"));
        }
        counts.shrinks += 1;
        Ok(())
    }
}

/// An exact physical start that remains charged while its outcome is unknown.
/// Dropping this value deliberately leaves its logical reservation in place;
/// it owns no physical worker, connection or process execution capacity.
#[derive(Debug)]
pub struct CompactionStartReservation {
    budget: CompactionBudget,
    request_id: RequestId,
}

impl CompactionStartReservation {
    /// Charges a matching confirmed start without allocating another allowance.
    ///
    /// # Errors
    /// Rejects a receipt for another physical request and retains uncertainty.
    /// # Panics
    /// A poisoned accounting mutex indicates an internal invariant violation.
    pub fn confirm(self, receipt: &AuxiliaryRequestStartCommit) -> Result<(), ContextError> {
        if receipt.request.request_id != self.request_id {
            return Err(ContextError::new(
                ContextErrorKind::Internal,
                "start receipt request mismatch",
            ));
        }
        let mut counts = self.budget.0.counts.lock().expect("compaction budget lock");
        counts.reserved -= 1;
        counts.committed += 1;
        Ok(())
    }

    /// Refunds only a Store-proven noncommit; an Unknown remains reserved.
    ///
    /// # Errors
    /// Returns required-persistence uncertainty when no noncommit was proven.
    /// # Panics
    /// A poisoned accounting mutex indicates an internal invariant violation.
    pub fn reject(self, failure: &AuxiliaryWriteFailure) -> Result<(), ContextError> {
        if !matches!(failure, AuxiliaryWriteFailure::KnownNotCommitted { .. }) {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDurabilityFailed,
                failure.to_string(),
            ));
        }
        self.budget
            .0
            .counts
            .lock()
            .expect("compaction budget lock")
            .reserved -= 1;
        Ok(())
    }
}

fn exhausted(message: &'static str) -> ContextError {
    ContextError::new(ContextErrorKind::CompactionBudgetExceeded, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    use crate::durable::{AuxiliaryStore, ConversationStoreError, SqliteConversationStore};
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallStart,
        AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition, AuxiliaryFingerprint,
        AuxiliaryInputEvidence, AuxiliaryRequestStart,
    };
    use crate::runtime::identity::{AuxiliaryCallId, ConversationId, SystemUuidV7Generator};
    use crate::runtime::monotonic::ManualMonotonicClock;

    fn budget() -> (
        CompactionBudget,
        Arc<ManualMonotonicClock>,
        CancellationSignal,
    ) {
        let clock = Arc::new(ManualMonotonicClock::new());
        (
            CompactionBudget::new(CompactionOperationId::new("operation"), clock.clone()),
            clock,
            CancellationSignal::new(),
        )
    }

    fn counts(budget: &CompactionBudget) -> (u8, u8, u8) {
        let counts = budget.0.counts.lock().unwrap();
        (counts.committed, counts.reserved, counts.shrinks)
    }

    fn new_request(call_id: AuxiliaryCallId) -> AuxiliaryRequestStart {
        AuxiliaryRequestStart {
            call_id,
            request_id: RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap(),
            ordinal: 0,
            model: "budget-test-model".into(),
        }
    }

    fn start_call(store: &SqliteConversationStore) -> AuxiliaryRequestStart {
        let call = AuxiliaryCallStart {
            call_id: AuxiliaryCallId::generate(),
            purpose: AuxiliaryAttribution::new("summary").unwrap(),
            trigger: AuxiliaryAttribution::new("automatic").unwrap(),
            business_source: AuxiliaryBusinessSource::Independent,
            input: AuxiliaryInputEvidence::Temporary,
            semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
            invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
            selected_message_ids: Vec::new(),
        };
        store
            .commit_auxiliary_call_start(&call, chrono::DateTime::UNIX_EPOCH)
            .unwrap();
        new_request(call.call_id)
    }

    fn commit_and_lose_receipt(store: &SqliteConversationStore, request: &AuxiliaryRequestStart) {
        let receipt = store
            .commit_auxiliary_request_start(request, chrono::DateTime::UNIX_EPOCH)
            .unwrap();
        assert_eq!(
            receipt.disposition,
            AuxiliaryCommitDisposition::NewlyCommitted
        );
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        drop(receiver);
        assert!(
            sender.send(receipt).is_err(),
            "the real commit lost its waiter"
        );
    }

    #[test]
    fn reentry_and_lost_waiter_never_renew_start_allowances() {
        let (budget, _, cancellation) = budget();
        for n in 0..12 {
            let child = budget.clone();
            drop(
                child
                    .reserve_start(RequestId::new(format!("request-{n}")), &cancellation)
                    .unwrap(),
            );
        }
        assert_eq!(
            budget
                .reserve_start(RequestId::new("thirteenth"), &cancellation)
                .unwrap_err()
                .kind,
            ContextErrorKind::CompactionBudgetExceeded
        );
    }

    #[test]
    fn only_proven_noncommit_refunds_an_uncertain_start() {
        let (budget, _, cancellation) = budget();
        let reservation = budget
            .reserve_start(RequestId::new("rolled-back"), &cancellation)
            .unwrap();
        reservation
            .reject(&AuxiliaryWriteFailure::KnownNotCommitted {
                error: ConversationStoreError::InvalidReference("rejected before SQL".into()),
            })
            .unwrap();
        for n in 0..12 {
            let reservation = budget
                .reserve_start(RequestId::new(format!("unknown-{n}")), &cancellation)
                .unwrap();
            assert_eq!(
                reservation
                    .reject(&AuxiliaryWriteFailure::Unknown {
                        error: ConversationStoreError::InvalidReference(
                            "lost acknowledgement".into()
                        )
                    })
                    .unwrap_err()
                    .kind,
                ContextErrorKind::AuxiliaryDurabilityFailed
            );
        }
        assert!(
            budget
                .reserve_start(RequestId::new("overflow"), &cancellation)
                .is_err()
        );
    }

    #[test]
    fn shrink_and_absolute_deadline_survive_cloning() {
        let (budget, clock, cancellation) = budget();
        for _ in 0..3 {
            budget.clone().take_shrink(&cancellation).unwrap();
        }
        assert_eq!(
            budget.take_shrink(&cancellation).unwrap_err().kind,
            ContextErrorKind::CompactionBudgetExceeded
        );
        clock.advance(300_000);
        assert_eq!(budget.clone().deadline_millis(), 300_000);
        assert_eq!(
            budget.check(&cancellation).unwrap_err().kind,
            ContextErrorKind::AuxiliaryDeadlineExceeded
        );
    }

    #[test]
    fn mixed_sqlite_receipts_rollback_unknown_and_waiter_loss_share_one_reentry_budget() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let clock = Arc::new(ManualMonotonicClock::new());
        clock.advance(37_000);
        let budget = CompactionBudget::new(CompactionOperationId::new("mixed"), clock.clone());
        let cancellation = CancellationSignal::new();

        for reentry in 0..3 {
            let child = budget.clone();
            assert!(child.same_operation(&budget));
            assert_eq!(child.deadline_millis(), 337_000);
            child.take_shrink(&cancellation).unwrap();

            // A structurally valid Request for an unstarted Call is rejected
            // inside the real SQLite transaction, not by a fabricated receipt.
            let rolled_back = new_request(AuxiliaryCallId::generate());
            let reservation = child
                .reserve_start(rolled_back.request_id.clone(), &cancellation)
                .unwrap();
            let failure = store
                .commit_auxiliary_request_start(&rolled_back, chrono::DateTime::UNIX_EPOCH)
                .unwrap_err();
            assert!(matches!(
                failure,
                AuxiliaryWriteFailure::KnownNotCommitted { .. }
            ));
            reservation.reject(&failure).unwrap();
            assert_eq!(counts(&child), (2 * reentry, 2 * reentry, reentry + 1));

            let direct = start_call(&store);
            let reservation = child
                .reserve_start(direct.request_id.clone(), &cancellation)
                .unwrap();
            let receipt = store
                .commit_auxiliary_request_start(&direct, chrono::DateTime::UNIX_EPOCH)
                .unwrap();
            reservation.confirm(&receipt).unwrap();

            let recovered = start_call(&store);
            let reservation = child
                .reserve_start(recovered.request_id.clone(), &cancellation)
                .unwrap();
            commit_and_lose_receipt(&store, &recovered);
            let AuxiliaryCommitConfirmation::ConfirmedCommitted(receipt) =
                store.confirm_auxiliary_request_start(&recovered).unwrap()
            else {
                panic!("lost real start receipt must be exactly confirmable");
            };
            assert_eq!(
                receipt.disposition,
                AuxiliaryCommitDisposition::IdempotentReplay
            );
            reservation.confirm(&receipt).unwrap();

            let unknown = start_call(&store);
            let reservation = child
                .reserve_start(unknown.request_id.clone(), &cancellation)
                .unwrap();
            commit_and_lose_receipt(&store, &unknown);
            // The delivery failure is synthetic; the committed start behind
            // the missing receipt is real. Uncertainty must not refund it.
            let failure = AuxiliaryWriteFailure::Unknown {
                error: ConversationStoreError::InvalidReference("start receipt lost".into()),
            };
            assert_eq!(
                reservation.reject(&failure).unwrap_err().kind,
                ContextErrorKind::AuxiliaryDurabilityFailed
            );

            let abandoned = start_call(&store);
            let reservation = child
                .reserve_start(abandoned.request_id.clone(), &cancellation)
                .unwrap();
            commit_and_lose_receipt(&store, &abandoned);
            drop(reservation);
            assert_eq!(
                counts(&child),
                (2 * (reentry + 1), 2 * (reentry + 1), reentry + 1)
            );
            clock.advance(99_999);
        }

        assert_eq!(counts(&budget), (6, 6, 3));
        assert_eq!(clock.now_millis(), 336_997);
        budget.check(&cancellation).unwrap();
        assert_eq!(
            budget
                .reserve_start(RequestId::new("thirteenth"), &cancellation)
                .unwrap_err()
                .kind,
            ContextErrorKind::CompactionBudgetExceeded
        );
        assert_eq!(
            budget.take_shrink(&cancellation).unwrap_err().kind,
            ContextErrorKind::CompactionBudgetExceeded
        );
        clock.advance(3);
        assert_eq!(
            budget.clone().check(&cancellation).unwrap_err().kind,
            ContextErrorKind::AuxiliaryDeadlineExceeded
        );
        assert_eq!(counts(&budget), (6, 6, 3));
    }

    #[test]
    fn simultaneous_reentries_admit_exactly_twelve_starts_and_three_shrinks() {
        let (budget, _, cancellation) = budget();
        let outcomes = std::thread::scope(|scope| {
            let mut releases = Vec::new();
            let workers: Vec<_> = (0..24)
                .map(|index| {
                    let child = budget.clone();
                    let cancellation = cancellation.clone();
                    let (release, ready) = std::sync::mpsc::channel::<Arc<Barrier>>();
                    releases.push(release);
                    std::thread::Builder::new()
                        .spawn_scoped(scope, move || {
                            let start = ready.recv().ok()?;
                            start.wait();
                            Some((
                                child.reserve_start(
                                    RequestId::new(format!("race-{index}")),
                                    &cancellation,
                                ),
                                child.take_shrink(&cancellation),
                            ))
                        })
                        .expect("spawn budget contender")
                })
                .collect();
            // If spawning fails, dropping the unsent channels releases all
            // existing workers before the scoped threads are joined.
            let start = Arc::new(Barrier::new(workers.len() + 1));
            for release in releases {
                release.send(start.clone()).unwrap();
            }
            start.wait();
            workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .expect("budget contender panicked")
                        .expect("budget contender was released")
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            outcomes.iter().filter(|(start, _)| start.is_ok()).count(),
            12
        );
        assert_eq!(
            outcomes.iter().filter(|(_, shrink)| shrink.is_ok()).count(),
            3
        );
        for (start, shrink) in outcomes {
            if let Err(error) = start {
                assert_eq!(error.kind, ContextErrorKind::CompactionBudgetExceeded);
            }
            if let Err(error) = shrink {
                assert_eq!(error.kind, ContextErrorKind::CompactionBudgetExceeded);
            }
        }
        assert_eq!(counts(&budget), (0, 12, 3));
        assert_eq!(budget.deadline_millis(), 300_000);
    }

    #[test]
    fn wrong_real_request_receipt_cannot_confirm_or_refund_the_last_allowance() {
        let (budget, _, cancellation) = budget();
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let unrelated = start_call(&store);
        let receipt = store
            .commit_auxiliary_request_start(&unrelated, chrono::DateTime::UNIX_EPOCH)
            .unwrap();
        for index in 0..11 {
            drop(
                budget
                    .reserve_start(RequestId::new(format!("lost-{index}")), &cancellation)
                    .unwrap(),
            );
        }
        let reservation = budget
            .reserve_start(RequestId::new("twelfth"), &cancellation)
            .unwrap();
        assert_eq!(
            reservation.confirm(&receipt).unwrap_err().kind,
            ContextErrorKind::Internal
        );
        assert_eq!(counts(&budget), (0, 12, 0));
        assert_eq!(
            budget
                .reserve_start(RequestId::new("replacement"), &cancellation)
                .unwrap_err()
                .kind,
            ContextErrorKind::CompactionBudgetExceeded
        );
    }

    #[tokio::test]
    async fn late_confirmation_settles_accounting_without_reopening_the_absolute_deadline() {
        let clock = Arc::new(ManualMonotonicClock::new());
        clock.advance(17);
        let budget = CompactionBudget::new(CompactionOperationId::new("late"), clock.clone());
        let cancellation = CancellationSignal::new();
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let request = start_call(&store);
        let pending = budget
            .reserve_start(request.request_id.clone(), &cancellation)
            .unwrap();
        commit_and_lose_receipt(&store, &request);
        let mut original_wait = budget.wait_until_deadline();
        assert!(futures_util::poll!(original_wait.as_mut()).is_pending());

        clock.advance(299_999);
        let child = budget.clone();
        let mut reentry_wait = child.wait_until_deadline();
        assert!(futures_util::poll!(original_wait.as_mut()).is_pending());
        assert!(futures_util::poll!(reentry_wait.as_mut()).is_pending());
        child.check(&cancellation).unwrap();
        child.take_shrink(&cancellation).unwrap();
        drop(
            child
                .reserve_start(RequestId::new("last-millisecond"), &cancellation)
                .unwrap(),
        );

        clock.advance(1);
        assert!(futures_util::poll!(original_wait.as_mut()).is_ready());
        assert!(futures_util::poll!(reentry_wait.as_mut()).is_ready());
        let AuxiliaryCommitConfirmation::ConfirmedCommitted(receipt) =
            store.confirm_auxiliary_request_start(&request).unwrap()
        else {
            panic!("a late confirmation still identifies the exact committed start");
        };
        pending.confirm(&receipt).unwrap();
        assert_eq!(counts(&budget), (1, 1, 1));
        assert_eq!(child.deadline_millis(), 300_017);
        for error in [
            child.check(&cancellation).unwrap_err(),
            child
                .reserve_start(RequestId::new("expired"), &cancellation)
                .unwrap_err(),
            child.take_shrink(&cancellation).unwrap_err(),
        ] {
            assert_eq!(error.kind, ContextErrorKind::AuxiliaryDeadlineExceeded);
        }
        assert_eq!(counts(&budget), (1, 1, 1));
    }
}
