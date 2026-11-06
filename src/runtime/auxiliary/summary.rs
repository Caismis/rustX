//! Native Summary adapter; Context retains input construction and acceptance.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

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
    prompt_fallback: Arc<AtomicBool>,
    last_call: Arc<Mutex<Option<crate::model::auxiliary::AuxiliaryCallRef>>>,
}

struct Prepared {
    reservation: AuxiliaryReservation,
    invocation: ResolvedModelInvocation,
    timeout: ModelTimeoutPolicy,
    resource_revision: u64,
    operation_id: crate::runtime::identity::CompactionOperationId,
    scope: Arc<AuxiliaryScope>,
    budget: CompactionBudget,
    prompt_fallback: Arc<AtomicBool>,
    last_call: Arc<Mutex<Option<crate::model::auxiliary::AuxiliaryCallRef>>>,
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
            prompt_fallback: Arc::new(AtomicBool::new(false)),
            last_call: Arc::new(Mutex::new(None)),
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
            prompt_fallback: Arc::new(AtomicBool::new(false)),
            last_call: Arc::new(Mutex::new(None)),
        }))
    }
}

impl SummaryOperation for Operation {
    fn budget(&self) -> CompactionBudget {
        self.budget.clone()
    }
    fn preview(
        &self,
        input: &SummaryRequest,
    ) -> Result<Option<crate::model::ModelRequest>, ContextError> {
        prepared_request(
            &self.invocation,
            input,
            self.prompt_fallback.load(Ordering::Acquire),
            self.budget.operation_id(),
        )
        .map(|(_, request)| Some(request))
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
            scope: self.scope.clone(),
            budget: self.budget.clone(),
            prompt_fallback: self.prompt_fallback.clone(),
            last_call: self.last_call.clone(),
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

/// Preparation is shared by exact preflight and actual frozen execution.
fn prepared_request(
    invocation: &ResolvedModelInvocation,
    input: &SummaryRequest,
    fallback: bool,
    operation_id: &crate::runtime::identity::CompactionOperationId,
) -> Result<(ResolvedModelInvocation, crate::model::ModelRequest), ContextError> {
    let invocation = input
        .output_tokens
        .map_or_else(|| invocation.clone(), |cap| invocation.with_output_cap(cap));
    let format = super::output::AuxiliaryOutputFormat::JsonSchema(
        crate::context::summary_content::response_schema(),
    );
    let request = input.model_request(&invocation);
    let request = if fallback {
        format.prepare_prompt_fallback(request, operation_id.clone())
    } else {
        format.prepare(request)
    }
    .map_err(crate::context::summarizer::summary_execution_failed)?;
    Ok((invocation, request))
}

impl PreparedSummary for Prepared {
    #[allow(clippy::too_many_lines)] // Preserve frozen request, settlement, lineage and fallback acceptance order.
    fn summarize(
        self: Box<Self>,
        input: SummaryRequest,
        cut: Option<ResourceReadCut>,
    ) -> BoxFuture<'static, Result<SummaryGeneration, ContextError>> {
        Box::pin(async move {
            self.budget.check(&self.scope.cancellation)?;
            let fallback = self.prompt_fallback.load(Ordering::Acquire);
            let (invocation, request) =
                prepared_request(&self.invocation, &input, fallback, &self.operation_id)?;
            let fingerprint = |value: String| {
                AuxiliaryFingerprint::new(value)
                    .map_err(|error| ContextError::new(ContextErrorKind::Internal, error))
            };
            let invocation_fingerprint = super::executor::invocation_fingerprint(
                &request.invocation,
                self.resource_revision,
            )?;
            let input_evidence = cut
                .as_ref()
                .map_or(AuxiliaryInputEvidence::Temporary, |cut| {
                    AuxiliaryInputEvidence::Compaction {
                        output_strategy: request
                            .response_schema
                            .as_ref()
                            .expect("prepared summary schema")
                            .strategy
                            .clone(),
                        cut: cut.clone(),
                        source_ranges: input
                            .fragment
                            .as_ref()
                            .map_or_else(Vec::new, |f| f.ranges.clone()),
                        previous_summary: input
                            .previous
                            .as_ref()
                            .and_then(|previous| previous.generation.clone()),
                    }
                });
            let prepared = self.reservation;
            let start = AuxiliaryCallStart {
                call_id: prepared.call_id().clone(),
                purpose: AuxiliaryAttribution::new("summary").expect("bounded native attribution"),
                trigger: AuxiliaryAttribution::new(if fallback {
                    "compaction_prompt_fallback"
                } else {
                    "compaction"
                })
                .expect("bounded native attribution"),
                business_source: if cut.is_some() {
                    AuxiliaryBusinessSource::Compaction {
                        operation_id: self.operation_id.clone(),
                        preceding_call: self.last_call.lock().expect("Summary call chain").clone(),
                    }
                } else {
                    AuxiliaryBusinessSource::Independent
                },
                input: input_evidence,
                semantic_input_fingerprint: fingerprint(request.semantic_input_fingerprint())?,
                invocation_fingerprint,
                selected_message_ids: Vec::new(),
            };
            let request_evidence = request.clone();
            let completed = prepared
                .submit(FrozenCall {
                    start,
                    request,
                    invocation,
                    timeout: self.timeout,
                    resource_revision: self.resource_revision,
                    dispatch_check: None,
                    title_source: None,
                })?
                .await
                .map_err(|_| {
                    ContextError::new(
                        ContextErrorKind::AuxiliarySupervisionFailed,
                        "native auxiliary completion unavailable",
                    )
                })??;
            *self.last_call.lock().expect("Summary call chain") =
                Some(crate::model::auxiliary::AuxiliaryCallRef {
                    conversation_id: completed.settlement.call_settled.conversation_id.clone(),
                    call_id: completed.settlement.call.call_id.clone(),
                    through_journal: completed.settlement.call_settled.sequence,
                });
            tracing::debug!(usage = ?completed.usage, "observed auxiliary Summary usage subtotal");
            let exhausted = completed
                .settlement
                .request
                .as_ref()
                .and_then(|request| request.request_evidence.as_ref())
                .and_then(|evidence| evidence.observed_provider_terminal.as_ref())
                .is_some_and(|terminal| {
                    matches!(
                        terminal,
                        crate::model::auxiliary::AuxiliaryObservedProviderTerminal::Completed {
                            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Length
                        }
                    )
                });
            if exhausted {
                return Err(ContextError::new(
                    ContextErrorKind::SummaryOutputExhausted,
                    "Summary total output was exhausted before complete JSON; reasoning Profile remains unchanged",
                ));
            }
            match completed.output {
                Ok(super::output::AuxiliaryOutput::JsonSchema(value)) => {
                    let text = crate::context::summary_content::render(value).map_err(|error| {
                        ContextError::new(ContextErrorKind::SummaryFailed, error)
                    })?;
                    let generation = crate::context::checkpoint::SummaryGenerationRef {
                        conversation_id: completed.settlement.call_settled.conversation_id.clone(),
                        call_id: completed.settlement.call.call_id.clone(),
                        through_journal: completed.settlement.call_settled.sequence,
                    };
                    Ok(SummaryGeneration {
                        text: accept_summary(text, false)?,
                        generation: Some(generation),
                    })
                }
                Ok(super::output::AuxiliaryOutput::Text(_)) => Err(ContextError::new(
                    ContextErrorKind::Internal,
                    "Summary execution did not honor its structured output contract",
                )),
                Err(error)
                    if cut.is_some()
                        && !fallback
                        && super::output::can_fallback_native_format(&request_evidence, &error) =>
                {
                    self.budget.charge_recovery(&self.scope.cancellation)?;
                    // Only this operation retains the confirmed downgrade.
                    self.prompt_fallback.store(true, Ordering::Release);
                    // Re-price the changed instructions at Context before
                    // another physical start. The same source remains unprocessed.
                    Err(ContextError::new(
                        ContextErrorKind::SummaryFormatChanged,
                        "native Summary format downgraded to a trusted Prompt; reprice this unprocessed fragment",
                    ))
                }
                Err(error) => Err(crate::context::summarizer::summary_execution_failed(error)),
            }
        })
    }
}
