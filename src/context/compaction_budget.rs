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
    use crate::durable::ConversationStoreError;
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
}
