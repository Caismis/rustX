//! One registered Call's single request and required settlement, on its owner thread.

use std::sync::Arc;

use crate::context::compaction_budget::CompactionStartReservation;
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
    client: AuxiliaryWriterClient,
    start: Arc<AuxiliaryCallStart>,
    request: Option<ModelRequest>,
    invocation: ResolvedModelInvocation,
    timeout: ModelTimeoutPolicy,
    resource_revision: u64,
    call_started: bool,
    request_started: bool,
    request_start_millis: u64,
    start_allowance: Option<CompactionStartReservation>,
    fold: Option<AuxiliaryStreamFold>,
    sealed: Option<SealedExecution>,
    undispatched: Option<ModelError>,
    settlement: Option<Box<AuxiliarySettlementCommit>>,
    failure: Option<ContextError>,
    durability_failed: bool,
}

#[allow(clippy::too_many_lines)] // The retained owner contains execution, panic recovery and final physical join.
pub(super) fn execute(
    record: &Arc<CallRecord>,
    permit: Arc<CallPermit>,
    frozen: FrozenCall,
    handle: &tokio::runtime::Handle,
) -> CallExit {
    let (writer, client) = match AuxiliaryWriter::start(record.executor.store.clone(), permit) {
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
    } = frozen;
    let mut execution = Execution {
        record: record.clone(),
        client,
        start: Arc::new(start),
        request: Some(request),
        invocation,
        timeout,
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
        durability_failed: false,
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
    if !execution.durability_failed {
        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle.block_on(execution.settle())
        }));
        match cleanup {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                execution.failure = Some(error);
                execution.durability_failed = execution.call_started;
            }
            Err(_) => {
                execution.failure = Some(supervision("auxiliary settlement supervision panicked"));
                execution.durability_failed = true;
            }
        }
        execution.recover_pending();
    }
    let joined = record.take_writer().map(AuxiliaryWriter::join);
    if joined.as_ref().is_some_and(|joined| joined.panicked) {
        execution.durability_failed = true;
        execution.failure = Some(supervision("required auxiliary writer panicked"));
    }
    let durability_failed = execution.durability_failed;
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
        let request = self.request.as_ref().expect("one frozen input");
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
        self.write(
            RequiredWrite::CallStart(self.start.clone(), self.record.executor.clock.now()),
            true,
        )
        .await?;
        self.record.scope.budget.check(&self.record.cancellation)?;
        let request_start = self
            .record
            .request_start(self.invocation.invocation_config().model);
        self.request_start_millis = self.record.executor.monotonic.now_millis();
        self.write(
            RequiredWrite::RequestStart(Arc::new(request_start), self.record.executor.clock.now()),
            true,
        )
        .await?;
        if let Err(error) = self.record.grant_dispatch() {
            self.undispatched = Some(control_error(&error));
            return Err(error);
        }
        let now = self.record.executor.monotonic.now_millis();
        self.fold = Some(AuxiliaryStreamFold::new(
            self.record.request_id.clone(),
            now,
            self.request_start_millis,
            self.record.scope.budget.deadline_millis(),
            self.timeout,
            self.invocation.generation_safety_policy(),
        ));
        // This is the only adapter entry for this RequestId. A panic or lost
        // acknowledgement unwinds to settlement; the consumed grant is permanent.
        let stream = self.invocation.adapter().stream(
            self.request
                .take()
                .expect("dispatch consumes frozen input once"),
            self.record.cancellation.child(),
        );
        let observed = poll_stream_owned(
            stream,
            self.fold.as_mut().expect("request fold"),
            &self.record.cancellation,
            self.record.executor.monotonic.as_ref(),
        )
        .await;
        let fold = self.fold.take().expect("one request fold");
        self.sealed = Some(match observed {
            Ok(()) => {
                let (sealed, failure) = self.record.seal_success(fold);
                self.failure = failure;
                sealed
            }
            Err(error) => fold.fail(error, self.record.executor.monotonic.now_millis()),
        });
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Preserve start/control/receipt precedence around the exact submitted intent.
    async fn write(&mut self, intent: RequiredWrite, start: bool) -> Result<(), ContextError> {
        let permission = if start {
            Some(tokio::select! {
                biased;
                () = self.record.cancellation.cancelled() => return Err(cancelled()),
                () = self.record.scope.budget.wait_until_deadline() => return Err(deadline()),
                permission = self.record.executor.durability.enter_ownership_commit_async() => {
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
                    self.record.scope.budget.check(&cancellation)?;
                    if let RequiredWrite::RequestStart(request, _) = &intent {
                        self.start_allowance = Some(
                            self.record
                                .scope
                                .budget
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
                () = self.record.scope.budget.wait_until_deadline() => { client.cancel_start(); (written.await, Some(deadline())) },
            }
        } else {
            (written.await, None)
        };
        drop(permission);
        let outcome = outcome?;
        match outcome.as_ref() {
            Ok(receipt) => {
                self.accept_receipt(receipt);
                if self.durability_failed {
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
                self.durability_failed =
                    !start || matches!(failure, AuxiliaryWriteFailure::Unknown { .. });
                if !self.durability_failed
                    && let Some(error) = control
                {
                    return Err(error);
                }
                Err(ContextError::new(
                    if self.durability_failed {
                        ContextErrorKind::AuxiliaryDurabilityFailed
                    } else {
                        ContextErrorKind::AuxiliaryNotStarted
                    },
                    failure.to_string(),
                ))
            }
        }
    }

    fn accept_receipt(&mut self, receipt: &RequiredReceipt) {
        match receipt {
            RequiredReceipt::CallStart(_) => self.call_started = true,
            RequiredReceipt::RequestStart(receipt) => {
                self.request_started = true;
                if let Some(allowance) = self.start_allowance.take()
                    && let Err(error) = allowance.confirm(receipt)
                {
                    self.failure = Some(error);
                    self.durability_failed = true;
                }
            }
            RequiredReceipt::Settlement(receipt) => self.settlement = Some(receipt.clone()),
        }
    }

    fn recover_pending(&mut self) {
        match self.record.pending_write() {
            Some(evidence) => match evidence.outcome.as_ref() {
                Ok(receipt) => self.accept_receipt(receipt),
                Err(AuxiliaryWriteFailure::Unknown { .. }) => {
                    self.durability_failed = true;
                    self.failure = Some(ContextError::new(
                        ContextErrorKind::AuxiliaryDurabilityFailed,
                        "required auxiliary write remains unknown after exact confirmation",
                    ));
                }
                Err(AuxiliaryWriteFailure::KnownNotCommitted { .. }) => {}
            },
            None => {}
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
                self.sealed = Some(fold.fail(error, self.record.executor.monotonic.now_millis()));
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
                    request_id: self.record.request_id.clone(),
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
        let timestamp = self.record.executor.clock.now();
        let envelope = |event| {
            auxiliary_settlement_envelope(
                self.record.executor.store.conversation_id().clone(),
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
                ordinal: 1,
            };
            let usage = evidence.and_then(|evidence| evidence.usage);
            let generation = self.sealed.as_ref().map(|sealed| sealed.generation);
            let event = if let Some(Ok(output)) = self.sealed.as_ref().map(|sealed| &sealed.output)
            {
                RuntimeEvent::ModelRequestCompleted {
                    owner,
                    request_id: self.record.request_id.clone(),
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
                    request_id: self.record.request_id.clone(),
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
        if self.durability_failed {
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
            });
        }
        Err(self
            .failure
            .take()
            .unwrap_or_else(|| not_started("auxiliary request did not dispatch")))
    }
}

fn audit_error(error: &ModelError) -> ModelError {
    let mut error = error.clone();
    error.message = format!("auxiliary model request failed: {:?}", error.kind);
    error.provider_code = None;
    error.retry_disposition = crate::model::ModelRetryDisposition::Never;
    error.retry_after_ms = None;
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
