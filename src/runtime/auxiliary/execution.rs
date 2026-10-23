//! One frozen Call's bounded requests and confirmed settlement on its owner thread.

use std::sync::Arc;

use super::authority::RequestStartReservation;
use crate::context::{ContextError, ContextErrorKind};
use crate::durable::auxiliary::{
    AuxiliarySettlementCommit, AuxiliaryWriteFailure, auxiliary_settlement_envelope,
};
use crate::events::types::RuntimeEvent;
use crate::model::auxiliary::{
    AuxiliaryCallOutcome, AuxiliaryRequestEvidence, AuxiliaryUsageCompleteness,
};
use crate::model::{
    AuxiliaryCallStart, ModelError, ModelErrorKind, ModelRequest, ModelRequestOwner,
    ModelTimeoutPolicy, ResolvedModelInvocation,
};

use super::capacity::CallPermit;
use super::executor::{CallExit, CallRecord, ConfirmedExecution, FrozenCall};
use super::stream::{AuxiliaryStreamFold, SealedExecution, local_error, poll_stream_owned};
use super::writer::{AuxiliaryWriter, AuxiliaryWriterClient, RequiredReceipt, RequiredWrite};

struct Execution {
    record: Arc<CallRecord>,
    resources: Arc<super::executor::ExecutionResources>,
    client: AuxiliaryWriterClient,
    start: Arc<AuxiliaryCallStart>,
    request: ModelRequest,
    request_id: crate::runtime::identity::RequestId,
    ordinal: u32,
    dispatch_check: Option<Arc<dyn super::executor::AuxiliaryDispatchCheck>>,
    title_source: Option<crate::durable::auxiliary::AuxiliaryTitleSourceBinding>,
    invocation: ResolvedModelInvocation,
    timeout: ModelTimeoutPolicy,
    generation_safety: crate::model::GenerationSafetyPolicy,
    resource_revision: u64,
    call_started: bool,
    request_started: bool,
    request_start_millis: u64,
    start_allowance: Option<RequestStartReservation>,
    fold: Option<AuxiliaryStreamFold>,
    sealed: Option<SealedExecution>,
    undispatched: Option<ModelError>,
    settlement: Option<Box<AuxiliarySettlementCommit>>,
    failure: Option<ContextError>,
    /// Unknown or invalid required evidence forbids further settlement writes.
    settlement_blocked: bool,
    /// A required storage failure survives a proven rollback and later settlement.
    required_failure: Option<ContextError>,
}

#[allow(clippy::too_many_lines)] // The retained owner contains execution, panic recovery and final physical join.
pub(super) fn execute(
    record: &Arc<CallRecord>,
    permit: Arc<CallPermit>,
    frozen: FrozenCall,
    handle: &tokio::runtime::Handle,
) -> CallExit {
    let resources = record
        .execution_resources()
        .expect("registered execution resources");
    let (writer, client) = match AuxiliaryWriter::start(resources.store.clone(), permit) {
        Ok(writer) => writer,
        Err(error) => {
            return CallExit {
                joined: None,
                result: Err(error),
                durability_failed: false,
            };
        }
    };
    record.install_writer(writer);
    let FrozenCall {
        start,
        request,
        invocation,
        timeout,
        resource_revision,
        dispatch_check,
        title_source,
    } = frozen;
    let mut generation_safety = invocation.generation_safety_policy();
    if let Some(consumer) = record.consumer() {
        generation_safety.max_generated_bytes = generation_safety
            .max_generated_bytes
            .min(consumer.generation_limit());
    }
    let mut execution = Execution {
        record: record.clone(),
        resources,
        client,
        start: Arc::new(start),
        request,
        request_id: record.request_id.clone(),
        ordinal: 0,
        dispatch_check,
        title_source,
        invocation,
        timeout,
        generation_safety,
        resource_revision,
        call_started: false,
        request_started: false,
        request_start_millis: 0,
        start_allowance: None,
        fold: None,
        sealed: None,
        undispatched: None,
        settlement: None,
        failure: None,
        settlement_blocked: false,
        required_failure: None,
    };
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle.block_on(execution.run())
    }));
    match run {
        Ok(Ok(())) => {}
        Ok(Err(error)) => execution.failure = Some(error),
        Err(_) => execution.failure = Some(supervision("auxiliary model execution panicked")),
    }
    // The model future has returned or unwound, but the record still owns all
    // writes and its fold. Recover an abandoned reply without another write.
    execution.recover_pending();
    if !execution.settlement_blocked {
        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle.block_on(execution.settle())
        }));
        match cleanup {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                execution.failure = Some(error);
                execution.settlement_blocked = execution.call_started;
            }
            Err(_) => {
                execution.failure = Some(supervision("auxiliary settlement supervision panicked"));
                execution.settlement_blocked = true;
            }
        }
        execution.recover_pending();
    }
    let joined = record.take_writer().map(AuxiliaryWriter::join);
    if joined.as_ref().is_some_and(|joined| joined.panicked) {
        execution.settlement_blocked = true;
        execution.failure = Some(supervision("required auxiliary writer panicked"));
    }
    let durability_failed = execution.settlement_blocked || execution.required_failure.is_some();
    let result = execution.result();
    CallExit {
        joined,
        result,
        durability_failed,
    }
}

impl Execution {
    #[allow(clippy::too_many_lines)] // Frozen preflight, confirmed starts and the one-shot dispatch form one progression.
    async fn run(&mut self) -> Result<(), ContextError> {
        let request = &self.request;
        if !self.timeout.is_positive()
            || request.model().is_empty()
            || request.model().len() > crate::model::AUXILIARY_REQUEST_MODEL_BYTES_MAX
            || request.model().chars().any(char::is_control)
        {
            return Err(not_started(
                "invalid frozen auxiliary model metadata or timeout policy",
            ));
        }
        if self.start.call_id != self.record.call_id
            || request.invocation != self.invocation.invocation_config()
            || !request.tools.is_empty()
            || request.continuation.is_some()
            || request.continuation_source.is_some()
            || self.start.semantic_input_fingerprint.as_str()
                != request.semantic_input_fingerprint()
            || self.start.invocation_fingerprint
                != super::executor::invocation_fingerprint(
                    &request.invocation,
                    self.resource_revision,
                )?
        {
            return Err(not_started(
                "invalid auxiliary frozen ownership or continuation/tool authority",
            ));
        }
        crate::model::adapter::validation::validate_request(
            request,
            self.invocation.adapter().protocol(),
        )
        .map_err(|error| not_started(&error.message))?;
        let intent = self.title_source.as_ref().map_or_else(
            || RequiredWrite::CallStart(self.start.clone(), self.resources.clock.now()),
            |source| {
                RequiredWrite::TitleCallStart(
                    self.start.clone(),
                    source.clone(),
                    self.resources.clock.now(),
                )
            },
        );
        self.write(intent, true).await?;
        crate::runtime::process_death::reach("after:auxiliary_call_start_confirmed");
        loop {
            self.record
                .scope
                .authority
                .check(&self.record.cancellation)?;
            self.check_dispatch().await?;
            let request_start = crate::model::AuxiliaryRequestStart {
                call_id: self.record.call_id.clone(),
                ordinal: self.ordinal,
                request_id: self.request_id.clone(),
                model: self.invocation.invocation_config().model,
            };
            self.request_start_millis = self.resources.monotonic.now_millis();
            self.write(
                RequiredWrite::RequestStart(Arc::new(request_start), self.resources.clock.now()),
                true,
            )
            .await?;
            let observed_at = self.dispatch().await?;
            let retry = self
                .sealed
                .as_ref()
                .and_then(|sealed| sealed.output.as_ref().err())
                .and_then(|error| {
                    retry_delay(
                        error,
                        self.ordinal,
                        observed_at,
                        self.record.scope.authority.deadline_millis(),
                    )
                });
            let Some(delay) = retry else {
                return Ok(());
            };
            if !self.record.scope.authority.has_remaining_start() {
                return Ok(());
            }
            self.record
                .scope
                .authority
                .check(&self.record.cancellation)?;
            self.schedule_retry(delay).await?;
            let eligible = observed_at.saturating_add(delay);
            tokio::select! {
                biased;
                () = self.record.cancellation.cancelled() => return Err(cancelled()),
                () = self.record.scope.authority.wait_until_deadline() => return Err(deadline()),
                () = self.resources.monotonic.wait_until_millis(eligible) => {}
            }
            self.record
                .scope
                .authority
                .check(&self.record.cancellation)?;
            self.ordinal += 1;
            self.record.advance_request(self.ordinal)?;
            self.request_id = crate::runtime::identity::RequestId::new(format!(
                "auxiliary:{}",
                uuid::Uuid::now_v7()
            ));
        }
    }

    async fn check_dispatch(&self) -> Result<(), ContextError> {
        let Some(check) = &self.dispatch_check else {
            return Ok(());
        };
        tokio::select! {
            biased;
            () = self.record.cancellation.cancelled() => Err(cancelled()),
            () = self.record.scope.authority.wait_until_deadline() => Err(deadline()),
            checked = check.check() => checked,
        }
    }

    async fn dispatch(&mut self) -> Result<u64, ContextError> {
        #[cfg(test)]
        self.record.wait_dispatch_gate_for_test(false);
        crate::runtime::process_death::reach("after:auxiliary_request_start_confirmed");
        if let Err(error) = self
            .check_dispatch()
            .await
            .and_then(|()| self.record.grant_dispatch())
        {
            self.undispatched = Some(control_error(&error));
            return Err(error);
        }
        #[cfg(test)]
        self.record.wait_dispatch_gate_for_test(true);
        crate::runtime::process_death::reach("after:auxiliary_dispatch_granted");
        let now = self.resources.monotonic.now_millis();
        self.fold = Some(
            AuxiliaryStreamFold::new(
                self.request_id.clone(),
                now,
                self.request_start_millis,
                self.record.scope.authority.deadline_millis(),
                self.timeout,
                self.generation_safety,
            )
            .with_visible_limit(self.record.output_limit())
            .with_consumer(self.record.consumer()),
        );
        if let Some(consumer) = self.record.consumer() {
            consumer.attempt(&self.request_id, self.ordinal);
        }
        // This is the only adapter entry for this RequestId. A panic or lost
        // acknowledgement unwinds to settlement; the consumed grant is permanent.
        let stream = self.invocation.adapter().stream(
            // Selected input retains immutable roots; captured image bytes are
            // copied exactly. No retry reselects history or rereads live media.
            self.request.clone(),
            self.record.cancellation.child(),
        );
        let observed = poll_stream_owned(
            stream,
            self.fold.as_mut().expect("request fold"),
            &self.record.cancellation,
            self.resources.monotonic.as_ref(),
        )
        .await;
        if let Some(consumer) = self.record.consumer() {
            consumer.stopped();
        }
        let fold = self.fold.take().expect("one request fold");
        let observed_at = fold
            .failure_observed_millis()
            .unwrap_or_else(|| self.resources.monotonic.now_millis());
        self.sealed = Some(match observed {
            Ok(()) => {
                let (sealed, failure) = self
                    .record
                    .seal_success(fold, self.resources.monotonic.as_ref());
                self.failure = failure;
                sealed
            }
            Err(error) => fold.fail(error, self.resources.monotonic.now_millis()),
        });
        Ok(observed_at)
    }

    async fn schedule_retry(&mut self, delay_ms: u64) -> Result<(), ContextError> {
        let sealed = self.sealed.as_ref().expect("failed request before retry");
        let failure = sealed.output.as_ref().expect_err("only failures retry");
        let timestamp = self.resources.clock.now();
        let envelope = |event| {
            auxiliary_settlement_envelope(
                self.resources.store.conversation_id().clone(),
                timestamp,
                event,
            )
            .map_err(|error| {
                ContextError::new(
                    ContextErrorKind::AuxiliaryDurabilityFailed,
                    error.to_string(),
                )
            })
        };
        let terminal = envelope(RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Auxiliary {
                call_id: self.record.call_id.clone(),
                ordinal: self.ordinal,
            },
            request_id: self.request_id.clone(),
            error: audit_error(failure),
            usage: sealed.evidence.usage.clone(),
            generation: Some(sealed.generation),
        })?;
        let retry = envelope(RuntimeEvent::AuxiliaryRetryScheduled {
            call_id: self.record.call_id.clone(),
            request_id: self.request_id.clone(),
            next_ordinal: self.ordinal + 1,
            delay_ms,
            request_evidence: sealed.evidence.clone(),
        })?;
        self.write(
            RequiredWrite::RetrySettlement(Arc::new((terminal, retry))),
            false,
        )
        .await
    }

    #[allow(clippy::too_many_lines)] // Preserve start/control/receipt precedence around the exact submitted intent.
    async fn write(&mut self, intent: RequiredWrite, start: bool) -> Result<(), ContextError> {
        let permission = if start {
            Some(tokio::select! {
                biased;
                () = self.record.cancellation.cancelled() => return Err(cancelled()),
                () = self.record.scope.authority.wait_until_deadline() => return Err(deadline()),
                permission = self.resources.durability.enter_ownership_commit_async() => {
                    permission.map_err(|refused| ContextError::new(ContextErrorKind::AuxiliaryDurabilityFailed, refused.diagnostic))?
                }
            })
        } else {
            None
        };
        let client = self.client.clone();
        let submitted = if start {
            let cancellation = self.record.cancellation.clone();
            cancellation
                .with_active(|| {
                    self.record.scope.authority.check(&cancellation)?;
                    if let RequiredWrite::RequestStart(request, _) = &intent {
                        self.start_allowance = Some(
                            self.record
                                .scope
                                .authority
                                .reserve_start(request.request_id.clone(), &cancellation)?,
                        );
                    }
                    client.submit(intent)
                })
                .ok_or_else(cancelled)??
        } else {
            client.submit(intent)?
        };
        let written = async move {
            submitted.await.map_err(|_| {
                supervision("required write reply lost; native owner must recover exact evidence")
            })
        };
        tokio::pin!(written);
        let (outcome, control) = if start {
            tokio::select! {
                biased;
                result = &mut written => (result, None),
                () = self.record.cancellation.cancelled() => { client.cancel_start(); (written.await, Some(cancelled())) },
                () = self.record.scope.authority.wait_until_deadline() => { client.cancel_start(); (written.await, Some(deadline())) },
            }
        } else {
            (written.await, None)
        };
        drop(permission);
        let outcome = outcome?;
        match outcome.as_ref() {
            Ok(receipt) => {
                self.accept_receipt(receipt);
                if self.settlement_blocked {
                    Err(self
                        .failure
                        .clone()
                        .unwrap_or_else(|| supervision("invalid required write receipt")))
                } else if let Some(error) = control {
                    if self.request_started {
                        self.undispatched = Some(control_error(&error));
                    }
                    Err(error)
                } else {
                    Ok(())
                }
            }
            Err(failure) => {
                if let Some(allowance) = self.start_allowance.take() {
                    let _ = allowance.reject(failure);
                }
                let error = self.record_write_failure(failure, start);
                if error.kind != ContextErrorKind::AuxiliaryDurabilityFailed
                    && let Some(control) = control
                {
                    return Err(control);
                }
                Err(error)
            }
        }
    }

    fn accept_receipt(&mut self, receipt: &RequiredReceipt) {
        match receipt {
            RequiredReceipt::CallStart(_) => {
                self.call_started = true;
                if let Some(consumer) = self.record.consumer() {
                    consumer.registered();
                }
            }
            RequiredReceipt::RequestStart(receipt) => {
                self.request_started = true;
                if let Some(allowance) = self.start_allowance.take()
                    && let Err(error) = allowance.confirm(receipt)
                {
                    self.failure = Some(error);
                    self.settlement_blocked = true;
                }
            }
            RequiredReceipt::RetrySettlement(receipt) => {
                if let Some(evidence) = &receipt.request.request_evidence {
                    self.record.scope.authority.observe(evidence);
                }
                if receipt.request.request_id == self.request_id {
                    self.request_started = false;
                    self.sealed = None;
                    self.undispatched = None;
                }
            }
            RequiredReceipt::Settlement(receipt) => {
                if let Some(evidence) = receipt
                    .request
                    .as_ref()
                    .and_then(|request| request.request_evidence.as_ref())
                {
                    self.record.scope.authority.observe(evidence);
                }
                self.settlement = Some(receipt.clone());
            }
        }
    }

    fn record_write_failure(
        &mut self,
        failure: &AuxiliaryWriteFailure,
        start: bool,
    ) -> ContextError {
        let uncertain = matches!(failure, AuxiliaryWriteFailure::Unknown { .. });
        let required = !start
            || uncertain
            || matches!(
                failure.error(),
                crate::durable::ConversationStoreError::Storage(_)
                    | crate::durable::ConversationStoreError::TransactionInvalid(_)
            );
        self.settlement_blocked |= !start || uncertain;
        let error = ContextError::new(
            if required {
                ContextErrorKind::AuxiliaryDurabilityFailed
            } else if matches!(
                failure.error(),
                crate::durable::ConversationStoreError::OperationCancelled
            ) {
                ContextErrorKind::Cancelled
            } else {
                ContextErrorKind::AuxiliaryNotStarted
            },
            failure.to_string(),
        );
        if required {
            self.required_failure.get_or_insert_with(|| error.clone());
        }
        error
    }

    fn recover_pending(&mut self) {
        if let Some(evidence) = self.record.pending_write() {
            match evidence.outcome.as_ref() {
                Ok(receipt) => self.accept_receipt(receipt),
                Err(failure) => {
                    if let Some(allowance) = self.start_allowance.take() {
                        let _ = allowance.reject(failure);
                    }
                    let start = matches!(
                        evidence.intent.as_ref(),
                        RequiredWrite::CallStart(..)
                            | RequiredWrite::TitleCallStart(..)
                            | RequiredWrite::RequestStart(..)
                    );
                    let error = self.record_write_failure(failure, start);
                    self.failure.get_or_insert(error);
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)] // One exact paired Request/Call settlement is constructed and submitted once.
    async fn settle(&mut self) -> Result<(), ContextError> {
        if !self.call_started || self.settlement.is_some() {
            return Ok(());
        }
        if self.request_started && self.sealed.is_none() {
            let error = self.undispatched.clone().unwrap_or_else(|| {
                local_error(
                    ModelErrorKind::InvalidRequest,
                    "auxiliary execution did not complete under native supervision",
                )
            });
            if let Some(fold) = self.fold.take() {
                self.sealed = Some(fold.fail(error, self.resources.monotonic.now_millis()));
            } else {
                self.undispatched = Some(error);
            }
        }
        let failure = self
            .sealed
            .as_ref()
            .and_then(|sealed| sealed.output.as_ref().err())
            .or(self.undispatched.as_ref());
        let outcome = if self
            .sealed
            .as_ref()
            .is_some_and(|sealed| sealed.output.is_ok())
        {
            AuxiliaryCallOutcome::Completed
        } else if failure.is_some_and(|error| error.kind == ModelErrorKind::Cancelled)
            || self.record.cancellation.is_cancelled() && !self.request_started
        {
            AuxiliaryCallOutcome::Cancelled
        } else {
            AuxiliaryCallOutcome::Failed
        };
        let evidence = if self.request_started {
            Some(self.sealed.as_ref().map_or_else(
                || AuxiliaryRequestEvidence {
                    request_id: self.request_id.clone(),
                    observed_provider_terminal: None,
                    usage: None,
                    usage_completeness: AuxiliaryUsageCompleteness::Unreported,
                    remote_execution_uncertainty: false,
                },
                |sealed| sealed.evidence.clone(),
            ))
        } else {
            None
        };
        let timestamp = self.resources.clock.now();
        let envelope = |event| {
            auxiliary_settlement_envelope(
                self.resources.store.conversation_id().clone(),
                timestamp,
                event,
            )
            .map_err(|error| {
                ContextError::new(
                    ContextErrorKind::AuxiliaryDurabilityFailed,
                    error.to_string(),
                )
            })
        };
        let call = envelope(RuntimeEvent::AuxiliaryCallSettled {
            call_id: self.record.call_id.clone(),
            outcome,
            request_evidence: evidence.clone(),
        })?;
        let intent = if self.request_started {
            let owner = ModelRequestOwner::Auxiliary {
                call_id: self.record.call_id.clone(),
                ordinal: self.ordinal,
            };
            let usage = evidence.and_then(|evidence| evidence.usage);
            let generation = self.sealed.as_ref().map(|sealed| sealed.generation);
            let event = if let Some(Ok(output)) = self.sealed.as_ref().map(|sealed| &sealed.output)
            {
                RuntimeEvent::ModelRequestCompleted {
                    owner,
                    request_id: self.request_id.clone(),
                    finish_reason: match &output.finish_reason {
                        crate::model::ModelFinishReason::Other { .. } => {
                            crate::model::ModelFinishReason::Other {
                                reason: "other".into(),
                            }
                        }
                        reason => reason.clone(),
                    },
                    usage,
                    generation,
                }
            } else {
                RuntimeEvent::ModelRequestFailed {
                    owner,
                    request_id: self.request_id.clone(),
                    error: audit_error(failure.expect("unsuccessful request owns a local failure")),
                    usage,
                    generation,
                }
            };
            RequiredWrite::Settlement(Arc::new((envelope(event)?, call)))
        } else {
            RequiredWrite::CallSettlement(Arc::new(call))
        };
        self.write(intent, false).await
    }

    fn result(mut self) -> Result<ConfirmedExecution, ContextError> {
        if let Some(error) = self.required_failure.take() {
            return Err(error);
        }
        if self.settlement_blocked {
            return Err(self.failure.take().unwrap_or_else(|| {
                ContextError::new(
                    ContextErrorKind::AuxiliaryDurabilityFailed,
                    "auxiliary settlement requires native durability handoff",
                )
            }));
        }
        if let (Some(sealed), Some(settlement)) = (self.sealed.take(), self.settlement.take()) {
            if let Some(error) = self.failure.take() {
                return Err(error);
            }
            return Ok(ConfirmedExecution {
                output: sealed.output,
                settlement,
                usage: self.record.scope.authority.usage_report(),
            });
        }
        Err(self
            .failure
            .take()
            .unwrap_or_else(|| not_started("auxiliary request did not dispatch")))
    }
}

/// Eligibility is based on the failure's monotonic observation, before any
/// settlement latency. Provider waits are respected exactly, never shortened.
fn retry_delay(error: &ModelError, ordinal: u32, observed_at: u64, deadline: u64) -> Option<u64> {
    if ordinal + 1 >= u32::from(super::authority::CALL_START_LIMIT)
        || error.retry_disposition != crate::model::ModelRetryDisposition::Transient
        || !matches!(
            error.kind,
            ModelErrorKind::RateLimit
                | ModelErrorKind::Timeout
                | ModelErrorKind::Transport
                | ModelErrorKind::ProviderError
        )
    {
        return None;
    }
    let local = 2_000_u64.saturating_mul(1_u64.checked_shl(ordinal).unwrap_or(u64::MAX));
    let delay = local.max(error.retry_after_ms.unwrap_or(0));
    (delay <= 60_000
        && observed_at
            .checked_add(delay)
            .is_some_and(|eligible| eligible < deadline))
    .then_some(delay)
}

fn audit_error(error: &ModelError) -> ModelError {
    let mut error = error.clone();
    error.message = format!("auxiliary model request failed: {:?}", error.kind);
    error.provider_code = None;
    error
}

pub(super) fn control_error(error: &ContextError) -> ModelError {
    local_error(
        match error.kind {
            ContextErrorKind::AuxiliaryDeadlineExceeded => ModelErrorKind::Timeout,
            ContextErrorKind::Cancelled => ModelErrorKind::Cancelled,
            _ => ModelErrorKind::Unsupported,
        },
        &error.message,
    )
}

fn not_started(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::AuxiliaryNotStarted, message)
}
fn supervision(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::AuxiliarySupervisionFailed, message)
}
fn cancelled() -> ContextError {
    ContextError::new(
        ContextErrorKind::Cancelled,
        "auxiliary preparation cancelled",
    )
}
fn deadline() -> ContextError {
    ContextError::new(
        ContextErrorKind::AuxiliaryDeadlineExceeded,
        "auxiliary preparation deadline expired",
    )
}

#[cfg(test)]
mod retry_policy_tests {
    use super::*;

    fn transient(kind: ModelErrorKind) -> ModelError {
        ModelError {
            kind,
            message: "temporary".into(),
            retry_disposition: crate::model::ModelRetryDisposition::Transient,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        }
    }

    #[test]
    fn auxiliary_policy_tightens_waits_and_never_relabels_local_control_or_domain_rejection() {
        let mut error = transient(ModelErrorKind::Transport);
        assert_eq!(retry_delay(&error, 0, 100, 300_000), Some(2_000));
        assert_eq!(retry_delay(&error, 1, 100, 300_000), Some(4_000));
        assert_eq!(retry_delay(&error, 2, 100, 300_000), None);
        error.retry_after_ms = Some(60_000);
        assert_eq!(retry_delay(&error, 0, 100, 300_000), Some(60_000));
        error.retry_after_ms = Some(60_001);
        assert_eq!(retry_delay(&error, 0, 100, 300_000), None);
        error.retry_after_ms = None;
        assert_eq!(retry_delay(&error, 0, 298_000, 300_000), None);
        assert_eq!(retry_delay(&error, 0, u64::MAX - 100, u64::MAX), None);
        for kind in [
            ModelErrorKind::InvalidRequest,
            ModelErrorKind::Authentication,
            ModelErrorKind::Cancelled,
            ModelErrorKind::Unsupported,
            ModelErrorKind::GenerationBudgetExceeded,
            ModelErrorKind::GenerationDegenerated,
            ModelErrorKind::ContextWindowExceeded,
            ModelErrorKind::MalformedToolProposal,
        ] {
            assert_eq!(retry_delay(&transient(kind), 0, 0, 300_000), None);
        }
        assert_eq!(
            retry_delay(&control_error(&deadline()), 0, 0, 300_000),
            None
        );
    }
}
