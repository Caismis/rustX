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
    deadline_millis: u64,
    clock: Arc<dyn MonotonicClock>,
    limit: u8,
    starts: Mutex<BTreeMap<RequestId, bool>>,
    evidence: Mutex<BTreeMap<RequestId, crate::model::AuxiliaryRequestEvidence>>,
}

/// A finite owner-issued allowance. Consumers pass clones to children; only
/// native business admission constructs fresh capacity.
#[derive(Clone, Debug)]
pub struct AuxiliaryAuthority(Arc<Authority>);

impl AuxiliaryAuthority {
    pub(crate) fn new(clock: Arc<dyn MonotonicClock>, limit: u8) -> Self {
        Self(Arc::new(Authority {
            deadline_millis: clock.now_millis().saturating_add(CALL_DEADLINE_MILLIS),
            clock,
            limit,
            starts: Mutex::default(),
            evidence: Mutex::default(),
        }))
    }

    pub(crate) fn same_allowance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn uses_clock(&self, clock: &Arc<dyn MonotonicClock>) -> bool {
        Arc::ptr_eq(&self.0.clock, clock)
    }

    pub(crate) fn deadline_millis(&self) -> u64 {
        self.0.deadline_millis
    }

    pub(crate) fn wait_until_deadline(&self) -> futures_util::future::BoxFuture<'static, ()> {
        self.0.clock.wait_until_millis(self.0.deadline_millis)
    }

    pub(crate) fn check(&self, cancellation: &CancellationSignal) -> Result<(), ContextError> {
        if cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "auxiliary operation cancelled",
            ));
        }
        if self.0.clock.now_millis() >= self.0.deadline_millis {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliaryDeadlineExceeded,
                "the original auxiliary operation deadline expired",
            ));
        }
        Ok(())
    }

    pub(crate) fn has_remaining_start(&self) -> bool {
        self.0
            .starts
            .lock()
            .expect("auxiliary allowance lock")
            .len()
            < usize::from(self.0.limit)
    }

    pub(crate) fn reserve_start(
        &self,
        request_id: RequestId,
        cancellation: &CancellationSignal,
    ) -> Result<RequestStartReservation, ContextError> {
        self.check(cancellation)?;
        let mut starts = self.0.starts.lock().expect("auxiliary allowance lock");
        if !starts.contains_key(&request_id) {
            if starts.len() >= usize::from(self.0.limit) {
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
    pub(crate) fn counts(&self) -> (u8, u8) {
        let starts = self.0.starts.lock().unwrap();
        let committed = u8::try_from(starts.values().filter(|confirmed| **confirmed).count())
            .expect("bounded confirmed starts");
        (
            committed,
            u8::try_from(starts.len()).expect("bounded total starts") - committed,
        )
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
