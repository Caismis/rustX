//! Restricted live authority shared by every semantic Call of one business owner.
//! Cloning never renews the absolute deadline or physical request allowance.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::context::{ContextError, ContextErrorKind};
use crate::durable::auxiliary::{AuxiliaryRequestStartCommit, AuxiliaryWriteFailure};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::RequestId;
use crate::runtime::monotonic::MonotonicClock;

pub(crate) const CALL_DEADLINE_MILLIS: u64 = 300_000;
pub(crate) const CALL_START_LIMIT: u8 = 3;

#[derive(Debug)]
struct Authority {
    admitted_at: u64,
    clock: Arc<dyn MonotonicClock>,
    limits: Mutex<Limits>,
    starts: Mutex<BTreeMap<RequestId, bool>>,
    evidence: Mutex<BTreeMap<RequestId, crate::model::AuxiliaryRequestEvidence>>,
}

#[derive(Debug)]
struct Limits {
    deadline_millis: u64,
    starts: usize,
    // None for independent calls. Planned compaction has a single admission,
    // and recovery actions expand its physical allowance one slot at a time.
    planned: Option<PlannedWork>,
}

#[derive(Debug)]
struct PlannedWork {
    admitted: Option<usize>,
    recoveries: usize,
}

/// A finite owner-issued allowance. Consumers pass clones to children; only
/// native business admission constructs fresh capacity.
#[derive(Clone, Debug)]
pub struct AuxiliaryAuthority(Arc<Authority>);

impl AuxiliaryAuthority {
    pub(crate) fn new(clock: Arc<dyn MonotonicClock>, limit: u8) -> Self {
        Self::create(clock, usize::from(limit), None)
    }

    pub(crate) fn new_planned(clock: Arc<dyn MonotonicClock>) -> Self {
        Self::create(
            clock,
            0,
            Some(PlannedWork {
                admitted: None,
                recoveries: 0,
            }),
        )
    }

    fn create(clock: Arc<dyn MonotonicClock>, starts: usize, planned: Option<PlannedWork>) -> Self {
        let admitted_at = clock.now_millis();
        Self(Arc::new(Authority {
            admitted_at,
            limits: Mutex::new(Limits {
                deadline_millis: admitted_at.saturating_add(CALL_DEADLINE_MILLIS),
                starts,
                planned,
            }),
            clock,
            starts: Mutex::default(),
            evidence: Mutex::default(),
        }))
    }

    /// Admit one finite compaction plan. Re-entry can only reuse, not expand it.
    pub(crate) fn admit_work(&self, bound: usize) -> Result<(), ContextError> {
        use crate::context::compaction_budget::{
            COMPACTION_MAX_DEADLINE_MILLIS, COMPACTION_PER_START_MILLIS, MAX_PLANNED_SUMMARY_STARTS,
        };
        if bound == 0 || bound > MAX_PLANNED_SUMMARY_STARTS {
            return Err(ContextError::new(
                ContextErrorKind::CompactionBudgetExceeded,
                "compaction plan exceeds its bounded work allowance",
            ));
        }
        let mut limits = self.0.limits.lock().expect("auxiliary limits lock");
        if self.0.clock.now_millis() >= limits.deadline_millis {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDeadlineExceeded,
                "the original auxiliary operation deadline expired",
            ));
        }
        let Some(work) = &mut limits.planned else {
            return Err(ContextError::new(
                ContextErrorKind::Internal,
                "independent auxiliary authority cannot admit compaction work",
            ));
        };
        if let Some(existing) = work.admitted {
            return if bound <= existing {
                Ok(())
            } else {
                Err(ContextError::new(
                    ContextErrorKind::CompactionBudgetExceeded,
                    "compaction re-entry cannot expand its admitted work plan",
                ))
            };
        }
        work.admitted = Some(bound);
        limits.starts = bound + work.recoveries;
        let extra = u64::try_from(bound - 1)
            .unwrap_or(u64::MAX)
            .saturating_mul(COMPACTION_PER_START_MILLIS);
        let duration = CALL_DEADLINE_MILLIS
            .saturating_add(extra)
            .min(COMPACTION_MAX_DEADLINE_MILLIS);
        limits.deadline_millis = self.0.admitted_at.saturating_add(duration);
        Ok(())
    }

    /// Charge one exceptional recovery; independent calls retain their own retry policy.
    pub(crate) fn charge_recovery(
        &self,
        cancellation: &CancellationSignal,
    ) -> Result<(), ContextError> {
        self.check(cancellation)?;
        let mut limits = self.0.limits.lock().expect("auxiliary limits lock");
        if self.0.clock.now_millis() >= limits.deadline_millis {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDeadlineExceeded,
                "the original auxiliary operation deadline expired",
            ));
        }
        let Some(work) = &mut limits.planned else {
            return Ok(());
        };
        if work.recoveries >= crate::context::compaction_budget::COMPACTION_RECOVERY_LIMIT {
            return Err(ContextError::new(
                ContextErrorKind::CompactionBudgetExceeded,
                "compaction exhausted its bounded recovery allowance",
            ));
        }
        work.recoveries += 1;
        if work.admitted.is_some() {
            limits.starts += 1;
        }
        Ok(())
    }

    pub(crate) fn same_allowance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn uses_clock(&self, clock: &Arc<dyn MonotonicClock>) -> bool {
        Arc::ptr_eq(&self.0.clock, clock)
    }

    pub(crate) fn deadline_millis(&self) -> u64 {
        self.0
            .limits
            .lock()
            .expect("auxiliary limits lock")
            .deadline_millis
    }

    pub(crate) fn wait_until_deadline(&self) -> futures_util::future::BoxFuture<'static, ()> {
        let authority = self.clone();
        Box::pin(async move {
            loop {
                let deadline = authority.deadline_millis();
                authority.0.clock.wait_until_millis(deadline).await;
                if authority.0.clock.now_millis() >= authority.deadline_millis() {
                    return;
                }
            }
        })
    }

    pub(crate) fn check(&self, cancellation: &CancellationSignal) -> Result<(), ContextError> {
        if cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "auxiliary operation cancelled",
            ));
        }
        if self.0.clock.now_millis() >= self.deadline_millis() {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDeadlineExceeded,
                "the original auxiliary operation deadline expired",
            ));
        }
        Ok(())
    }

    /// Allow preparatory Call reservation before a compaction plan exists.
    /// This never grants a physical start; `reserve_start` still enforces
    /// the admitted limit at the durable start boundary.
    pub(crate) fn can_reserve_call(&self) -> bool {
        let limits = self.0.limits.lock().expect("auxiliary limits lock");
        if limits
            .planned
            .as_ref()
            .is_some_and(|work| work.admitted.is_none())
        {
            return true;
        }
        self.0
            .starts
            .lock()
            .expect("auxiliary allowance lock")
            .len()
            < limits.starts
    }

    pub(crate) fn has_remaining_start(&self) -> bool {
        self.remaining_starts() > 0
    }

    pub(crate) fn remaining_starts(&self) -> usize {
        let limits = self.0.limits.lock().expect("auxiliary limits lock");
        limits.starts.saturating_sub(
            self.0
                .starts
                .lock()
                .expect("auxiliary allowance lock")
                .len(),
        )
    }

    pub(crate) fn reserve_start(
        &self,
        request_id: RequestId,
        cancellation: &CancellationSignal,
    ) -> Result<RequestStartReservation, ContextError> {
        self.check(cancellation)?;
        let limits = self.0.limits.lock().expect("auxiliary limits lock");
        let mut starts = self.0.starts.lock().expect("auxiliary allowance lock");
        if self.0.clock.now_millis() >= limits.deadline_millis {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDeadlineExceeded,
                "the original auxiliary operation deadline expired",
            ));
        }
        if !starts.contains_key(&request_id) {
            if starts.len() >= limits.starts {
                return Err(ContextError::new(
                    ContextErrorKind::CompactionBudgetExceeded,
                    "auxiliary operation exhausted its physical request allowance",
                ));
            }
            starts.insert(request_id.clone(), false);
        }
        Ok(RequestStartReservation {
            authority: self.clone(),
            request_id,
        })
    }

    pub(crate) fn observe(&self, evidence: &crate::model::AuxiliaryRequestEvidence) {
        let starts = self.0.starts.lock().expect("auxiliary allowance lock");
        if starts.get(&evidence.request_id) == Some(&true) {
            self.0
                .evidence
                .lock()
                .expect("auxiliary usage lock")
                .entry(evidence.request_id.clone())
                .or_insert_with(|| evidence.clone());
        }
    }

    pub(crate) fn usage_report(&self) -> ObservedAuxiliaryUsage {
        use crate::model::AuxiliaryUsageCompleteness;
        let starts = self.0.starts.lock().expect("auxiliary allowance lock");
        let evidence = self.0.evidence.lock().expect("auxiliary usage lock");
        let reports: Vec<_> = evidence.values().collect();
        let sum = |select: fn(&crate::model::ModelUsage) -> Option<u64>| {
            let values: Vec<_> = reports
                .iter()
                .map(|report| report.usage.as_ref().and_then(select))
                .collect();
            let present = values.iter().filter(|value| value.is_some()).count();
            let total = values
                .into_iter()
                .flatten()
                .try_fold(None, |total: Option<u64>, value| {
                    total.unwrap_or(0).checked_add(value).map(Some)
                });
            (
                total.flatten(),
                total.is_none() || (present > 0 && present != reports.len()),
            )
        };
        let (input_tokens, input_incomplete) = sum(|usage| usage.input_tokens);
        let (output_tokens, output_incomplete) = sum(|usage| usage.output_tokens);
        let (total_tokens, total_incomplete) = sum(|usage| usage.total_tokens);
        let (reasoning_tokens, reasoning_incomplete) = sum(|usage| {
            usage
                .details
                .as_ref()
                .and_then(|details| details.reasoning_tokens)
        });
        let (cached_input_tokens, cached_incomplete) = sum(|usage| {
            usage
                .details
                .as_ref()
                .and_then(|details| details.cached_input_tokens)
        });
        ObservedAuxiliaryUsage {
            input_tokens,
            output_tokens,
            total_tokens,
            reasoning_tokens,
            cached_input_tokens,
            incomplete: input_incomplete
                || output_incomplete
                || total_incomplete
                || reasoning_incomplete
                || cached_incomplete
                || evidence.len() != starts.len()
                || reports.iter().any(|report| {
                    report.usage_completeness != AuxiliaryUsageCompleteness::Complete
                        || report
                            .usage
                            .as_ref()
                            .is_none_or(|usage| !usage.is_complete())
                }),
            remote_execution_uncertainty: evidence.len() != starts.len()
                || reports
                    .iter()
                    .any(|report| report.remote_execution_uncertainty),
        }
    }

    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let starts = self.0.starts.lock().unwrap();
        let committed = starts.values().filter(|confirmed| **confirmed).count();
        (committed, starts.len() - committed)
    }
}

/// Observed subtotal only. Missing reports/fields remain absent and remote
/// inference uncertainty is independent of local completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedAuxiliaryUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub incomplete: bool,
    pub remote_execution_uncertainty: bool,
}

/// An unknown transaction keeps its reservation even if this receipt owner drops.
#[derive(Debug)]
pub struct RequestStartReservation {
    authority: AuxiliaryAuthority,
    request_id: RequestId,
}

impl RequestStartReservation {
    /// Confirms the exact request; repeated confirmation never charges twice.
    /// # Errors
    /// Rejects another request's receipt without refunding uncertainty.
    /// # Panics
    /// A poisoned accounting lock or missing native reservation is an invariant violation.
    pub fn confirm(self, receipt: &AuxiliaryRequestStartCommit) -> Result<(), ContextError> {
        if receipt.request.request_id != self.request_id {
            return Err(ContextError::new(
                ContextErrorKind::Internal,
                "start receipt request mismatch",
            ));
        }
        let mut starts = self
            .authority
            .0
            .starts
            .lock()
            .expect("auxiliary allowance lock");
        *starts
            .get_mut(&self.request_id)
            .expect("reserved exact request") = true;
        Ok(())
    }

    /// Releases only a confirmed noncommit. Unknown stays reserved.
    /// # Errors
    /// Returns unresolved required persistence for an unknown write.
    /// # Panics
    /// A poisoned accounting lock indicates an invariant violation.
    pub fn reject(self, failure: &AuxiliaryWriteFailure) -> Result<(), ContextError> {
        if !matches!(failure, AuxiliaryWriteFailure::KnownNotCommitted { .. }) {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDurabilityFailed,
                failure.to_string(),
            ));
        }
        let mut starts = self
            .authority
            .0
            .starts
            .lock()
            .expect("auxiliary allowance lock");
        if starts.get(&self.request_id) == Some(&false) {
            starts.remove(&self.request_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AuxiliaryRequestEvidence, AuxiliaryUsageCompleteness, ModelUsage, UsageDetails,
    };
    use crate::runtime::monotonic::ManualMonotonicClock;

    #[test]
    fn planned_call_reservation_does_not_authorize_an_unadmitted_start() {
        let authority = AuxiliaryAuthority::new_planned(Arc::new(ManualMonotonicClock::new()));
        let cancel = CancellationSignal::new();
        assert!(authority.can_reserve_call());
        assert!(
            authority
                .reserve_start(RequestId::new("unadmitted"), &cancel)
                .is_err()
        );
        authority.admit_work(1).unwrap();
        assert!(authority.can_reserve_call());
        drop(
            authority
                .reserve_start(RequestId::new("admitted"), &cancel)
                .unwrap(),
        );
        assert!(!authority.can_reserve_call());
    }

    #[test]
    fn preplan_recoveries_are_charged_without_authorizing_physical_starts() {
        let clock = Arc::new(ManualMonotonicClock::new());
        let authority = AuxiliaryAuthority::new_planned(clock.clone());
        let child = authority.clone();
        let cancel = CancellationSignal::new();
        for _ in 0..crate::context::compaction_budget::COMPACTION_RECOVERY_LIMIT {
            child.charge_recovery(&cancel).unwrap();
        }
        assert!(authority.charge_recovery(&cancel).is_err());
        assert_eq!(authority.remaining_starts(), 0);
        assert!(
            authority
                .reserve_start(RequestId::new("before-plan"), &cancel)
                .is_err()
        );
        clock.advance(1000);
        authority.admit_work(2).unwrap();
        assert_eq!(
            authority.remaining_starts(),
            2 + crate::context::compaction_budget::COMPACTION_RECOVERY_LIMIT
        );
        assert_eq!(
            authority.deadline_millis(),
            CALL_DEADLINE_MILLIS + crate::context::compaction_budget::COMPACTION_PER_START_MILLIS
        );
        child.admit_work(2).unwrap();
        assert_eq!(
            child.remaining_starts(),
            2 + crate::context::compaction_budget::COMPACTION_RECOVERY_LIMIT
        );
        assert!(child.charge_recovery(&cancel).is_err());
        for index in 0..(2 + crate::context::compaction_budget::COMPACTION_RECOVERY_LIMIT) {
            drop(
                child
                    .reserve_start(RequestId::new(format!("start-{index}")), &cancel)
                    .unwrap(),
            );
        }
        assert!(
            authority
                .reserve_start(RequestId::new("overflow"), &cancel)
                .is_err()
        );
    }

    #[test]
    fn independent_calls_keep_three_starts_and_five_minute_deadline() {
        let clock = Arc::new(ManualMonotonicClock::new());
        let authority = AuxiliaryAuthority::new(clock.clone(), CALL_START_LIMIT);
        let cancel = CancellationSignal::new();
        for index in 0..3 {
            authority.charge_recovery(&cancel).unwrap();
            drop(
                authority
                    .reserve_start(RequestId::new(format!("independent-{index}")), &cancel)
                    .unwrap(),
            );
        }
        assert!(!authority.has_remaining_start());
        assert!(
            authority
                .reserve_start(RequestId::new("fourth"), &cancel)
                .is_err()
        );
        assert_eq!(authority.deadline_millis(), CALL_DEADLINE_MILLIS);
        clock.advance(CALL_DEADLINE_MILLIS);
        assert_eq!(
            authority.check(&cancel).unwrap_err().kind,
            ContextErrorKind::AuxiliaryDeadlineExceeded
        );
    }

    #[tokio::test]
    async fn waiter_created_before_admission_observes_the_admitted_deadline() {
        let clock = Arc::new(ManualMonotonicClock::new());
        let authority = AuxiliaryAuthority::new_planned(clock.clone());
        let mut waiter = authority.wait_until_deadline();
        assert!(futures_util::poll!(waiter.as_mut()).is_pending());
        authority.admit_work(2).unwrap();
        clock.advance(CALL_DEADLINE_MILLIS);
        assert!(futures_util::poll!(waiter.as_mut()).is_pending());
        clock.advance(crate::context::compaction_budget::COMPACTION_PER_START_MILLIS);
        assert!(futures_util::poll!(waiter.as_mut()).is_ready());
    }

    #[test]
    fn observed_subtotals_preserve_partial_fields_uncertainty_and_exact_request_identity() {
        let authority = AuxiliaryAuthority::new(Arc::new(ManualMonotonicClock::new()), 3);
        for (id, input, reasoning, complete, uncertain) in [
            (
                "partial",
                None,
                Some(3),
                AuxiliaryUsageCompleteness::Partial,
                true,
            ),
            (
                "complete",
                Some(7),
                None,
                AuxiliaryUsageCompleteness::Complete,
                false,
            ),
        ] {
            let request_id = RequestId::new(id);
            authority
                .0
                .starts
                .lock()
                .unwrap()
                .insert(request_id.clone(), true);
            let evidence = AuxiliaryRequestEvidence {
                request_id,
                observed_provider_terminal: None,
                usage: Some(ModelUsage {
                    input_tokens: input,
                    output_tokens: Some(2),
                    total_tokens: input.map(|input| input + 2),
                    details: Some(UsageDetails {
                        reasoning_tokens: reasoning,
                        cached_input_tokens: None,
                    }),
                }),
                usage_completeness: complete,
                remote_execution_uncertainty: uncertain,
            };
            authority.observe(&evidence);
            authority.observe(&evidence);
        }
        let observed = authority.usage_report();
        assert_eq!(observed.input_tokens, Some(7));
        assert_eq!(observed.output_tokens, Some(4));
        assert_eq!(observed.total_tokens, Some(9));
        assert_eq!(observed.reasoning_tokens, Some(3));
        assert_eq!(observed.cached_input_tokens, None);
        assert!(observed.incomplete);
        assert!(observed.remote_execution_uncertainty);
    }
}
