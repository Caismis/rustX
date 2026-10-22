//! Native Summary adapter; Context retains input construction and acceptance.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use crate::context::compaction_budget::CompactionBudget;
use crate::context::resources::ResourceReadCut;
use crate::context::summarizer::{
    PreparedSummary, SummaryBackend, SummaryGeneration, SummaryOperation, SummaryRequest,
    accept_summary,
};
use crate::context::{ContextError, ContextErrorKind};
use crate::model::{
    AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallStart, AuxiliaryFingerprint,
    AuxiliaryInputEvidence, ModelTimeoutPolicy, ResolvedModelInvocation,
};
use crate::runtime::cancellation::CancellationSignal;

use super::executor::{AuxiliaryExecutor, AuxiliaryReservation, AuxiliaryScope, FrozenCall};

struct Operation {
    executor: AuxiliaryExecutor,
    scope: Arc<AuxiliaryScope>,
    budget: CompactionBudget,
    first: Mutex<Option<AuxiliaryReservation>>,
    invocation: ResolvedModelInvocation,
    timeout: ModelTimeoutPolicy,
    resource_revision: u64,
}

struct Prepared {
    reservation: AuxiliaryReservation,
    invocation: ResolvedModelInvocation,
    timeout: ModelTimeoutPolicy,
    resource_revision: u64,
    operation_id: crate::runtime::identity::CompactionOperationId,
}

struct PreAdmitted {
    executor: AuxiliaryExecutor,
    scope: Arc<AuxiliaryScope>,
    budget: CompactionBudget,
    first: Mutex<Option<AuxiliaryReservation>>,
}

impl AuxiliaryExecutor {
    /// Manual admission reserves before resolving the idle model/resources;
    /// only the eventual native business task consumes this preparation.
    pub(crate) fn pre_admit_summary(
        &self,
        budget: CompactionBudget,
        cancellation: &CancellationSignal,
    ) -> Result<Arc<dyn SummaryBackend>, ContextError> {
        let scope = Self::scope(&budget, cancellation);
        let first = self.reserve(&scope)?;
        Ok(Arc::new(PreAdmitted {
            executor: self.clone(),
            scope,
            budget,
            first: Mutex::new(Some(first)),
        }))
    }
}

impl SummaryBackend for PreAdmitted {
    fn operation(
        &self,
        invocation: ResolvedModelInvocation,
        timeout: ModelTimeoutPolicy,
        resource_revision: u64,
        budget: CompactionBudget,
        _cancellation: CancellationSignal,
    ) -> Result<Box<dyn SummaryOperation>, ContextError> {
        if !budget.same_operation(&self.budget) {
            return Err(ContextError::new(
                ContextErrorKind::AuxiliarySupervisionFailed,
                "manual preparation belongs to another Compaction",
            ));
        }
        let first = self
            .first
            .lock()
            .expect("native manual preparation")
            .take()
            .ok_or_else(|| {
                ContextError::new(
                    ContextErrorKind::AuxiliarySupervisionFailed,
                    "manual preparation already transferred",
                )
            })?;
        Ok(Box::new(Operation {
            executor: self.executor.clone(),
            scope: self.scope.clone(),
            budget: self.budget.clone(),
            first: Mutex::new(Some(first)),
            invocation,
            timeout,
            resource_revision,
        }))
    }
}

impl Drop for PreAdmitted {
    fn drop(&mut self) {
        self.scope.close();
    }
}

impl SummaryBackend for AuxiliaryExecutor {
    fn operation(
        &self,
        invocation: ResolvedModelInvocation,
        timeout: ModelTimeoutPolicy,
        resource_revision: u64,
        budget: CompactionBudget,
        cancellation: CancellationSignal,
    ) -> Result<Box<dyn SummaryOperation>, ContextError> {
        let scope = Self::scope(&budget, &cancellation);
        let first = self.reserve(&scope)?;
        Ok(Box::new(Operation {
            executor: self.clone(),
            scope,
            budget,
            first: Mutex::new(Some(first)),
            invocation,
            timeout,
            resource_revision,
        }))
    }
}

impl SummaryOperation for Operation {
    fn budget(&self) -> CompactionBudget {
        self.budget.clone()
    }
    fn authorize_commit(
        &self,
    ) -> Result<crate::runtime::cancellation::CompactionCommitGrant, ContextError> {
        self.scope.authorize_commit(&self.budget)
    }
    fn prepare(&self) -> Result<Box<dyn PreparedSummary>, ContextError> {
        let reservation = match self.first.lock().expect("Summary first reservation").take() {
            Some(first) => first,
            None => self.executor.reserve(&self.scope)?,
        };
        Ok(Box::new(Prepared {
            reservation,
            invocation: self.invocation.clone(),
            timeout: self.timeout,
            resource_revision: self.resource_revision,
            operation_id: self.budget.operation_id().clone(),
        }))
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.first.lock().expect("Summary first reservation").take();
            self.scope.wait_closed().await;
        })
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.scope.close();
    }
}

impl PreparedSummary for Prepared {
    fn summarize(
        self: Box<Self>,
        input: SummaryRequest,
        cut: Option<ResourceReadCut>,
    ) -> BoxFuture<'static, Result<SummaryGeneration, ContextError>> {
        Box::pin(async move {
            let request = input.model_request(&self.invocation);
            let fingerprint = |value: String| {
                AuxiliaryFingerprint::new(value)
                    .map_err(|error| ContextError::new(ContextErrorKind::Internal, error))
            };
            let invocation_fingerprint = super::executor::invocation_fingerprint(
                &request.invocation,
                self.resource_revision,
            )?;
            let start = AuxiliaryCallStart {
                call_id: self.reservation.call_id().clone(),
                purpose: AuxiliaryAttribution::new("summary").expect("native bounded attribution"),
                trigger: AuxiliaryAttribution::new("compaction")
                    .expect("native bounded attribution"),
                business_source: if cut.is_some() {
                    AuxiliaryBusinessSource::Compaction {
                        operation_id: self.operation_id,
                    }
                } else {
                    AuxiliaryBusinessSource::Independent
                },
                input: cut.map_or(AuxiliaryInputEvidence::Temporary, |cut| {
                    AuxiliaryInputEvidence::Historical { cut }
                }),
                semantic_input_fingerprint: fingerprint(request.semantic_input_fingerprint())?,
                invocation_fingerprint,
                selected_message_ids: Vec::new(),
            };
            let completed = self
                .reservation
                .submit(FrozenCall {
                    start,
                    request,
                    invocation: self.invocation,
                    timeout: self.timeout,
                    resource_revision: self.resource_revision,
                    dispatch_check: None,
                    output_bytes_limit: None,
                    title_source: None,
                })?
                .await
                .map_err(|_| {
                    ContextError::new(
                        ContextErrorKind::AuxiliarySupervisionFailed,
                        "native auxiliary completion unavailable",
                    )
                })??;
            tracing::debug!(usage = ?completed.usage, "observed auxiliary Summary usage subtotal");
            let output = completed
                .output
                .map_err(crate::context::summarizer::summary_execution_failed)?;
            let text = accept_summary(output.text, output.refused)?;
            let generation = crate::context::checkpoint::SummaryGenerationRef {
                conversation_id: completed.settlement.call_settled.conversation_id.clone(),
                call_id: completed.settlement.call.call_id.clone(),
                through_journal: completed.settlement.call_settled.sequence,
            };
            Ok(SummaryGeneration {
                text,
                generation: Some(generation),
            })
        })
    }
}
