//! One request's provider-independent fold. No dispatch, persistence or retry.
//!
//! Completion remains provisional until EOF and native outcome arbitration.
//! Valid evidence is retained even if local acceptance subsequently fails.

use futures_util::StreamExt as _;

use crate::message::types::AssistantContentBlock;
use crate::model::assembly::ModelEventAssembler;
use crate::model::auxiliary::{
    AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryUsageCompleteness,
};
use crate::model::generation_evidence::{GenerationEvidence, GenerationTiming};
use crate::model::{
    GenerationGuard, GenerationSafetyPolicy, ModelError, ModelErrorKind, ModelEvent,
    ModelFinishReason, ModelRequestDeadline, ModelRetryDisposition, ModelStream, ModelStreamItem,
    ModelTimeoutPolicy, ModelUsage,
};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::RequestId;
use crate::runtime::monotonic::MonotonicClock;

/// Bounds placeholder allocation even for empty deltas that consume no byte budget.
pub const AUXILIARY_CONTENT_BLOCK_LIMIT: u32 = 4096;

/// Execution output. Context separately decides whether this is a usable Summary.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxiliaryTextOutput {
    pub text: String,
    pub refused: bool,
    pub finish_reason: ModelFinishReason,
}

/// A locally sealed execution, still requiring authoritative durable settlement.
#[derive(Debug, Clone, PartialEq)]
pub struct SealedExecution {
    pub output: Result<AuxiliaryTextOutput, ModelError>,
    pub generation: GenerationEvidence,
    pub evidence: AuxiliaryRequestEvidence,
}

/// EOF still needs the native owner's synchronized success grant. A failure
/// has no usable output and never waits for a second provider item or EOF.
pub enum AuxiliaryStreamExit {
    Eof(Box<AuxiliaryStreamFold>),
    Failed(Box<SealedExecution>),
}

/// Polls an already-dispatched stream in its retained native execution task.
/// This function creates no tasks, writes, permits or dispatch authority.
pub async fn poll_stream(
    stream: ModelStream,
    mut fold: AuxiliaryStreamFold,
    cancellation: &CancellationSignal,
    clock: &dyn MonotonicClock,
) -> AuxiliaryStreamExit {
    match poll_stream_owned(stream, &mut fold, cancellation, clock).await {
        Ok(()) => AuxiliaryStreamExit::Eof(Box::new(fold)),
        Err(error) => AuxiliaryStreamExit::Failed(Box::new(fold.fail(error, clock.now_millis()))),
    }
}

/// The native record keeps the fold outside this future so a provider panic
/// cannot discard already-trusted usage, timing or terminal evidence.
pub(super) async fn poll_stream_owned(
    mut stream: ModelStream,
    fold: &mut AuxiliaryStreamFold,
    cancellation: &CancellationSignal,
    clock: &dyn MonotonicClock,
) -> Result<(), ModelError> {
    loop {
        let next = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Err(local_error(ModelErrorKind::Cancelled, "auxiliary generation cancelled"));
            }
            () = clock.wait_until_millis(fold.deadline_millis()) => {
                let now = clock.now_millis();
                if let Some(error) = fold.expiration(now) {
                    return Err(error);
                }
                continue;
            }
            next = stream.next() => next,
        };
        let Some(item) = next else {
            return Ok(());
        };
        let now = clock.now_millis();
        fold.accept(&item, now)?;
    }
}

/// A single physical request's stream and timing state.
pub struct AuxiliaryStreamFold {
    request_id: RequestId,
    assembler: ModelEventAssembler,
    guard: GenerationGuard,
    timing: GenerationTiming,
    liveness: ModelRequestDeadline,
    absolute_deadline_millis: u64,
    started: bool,
    failure: Option<ModelError>,
    usage: Option<ModelUsage>,
    usage_completeness: AuxiliaryUsageCompleteness,
    provider_terminal: Option<AuxiliaryObservedProviderTerminal>,
}

impl AuxiliaryStreamFold {
    /// Starts at the one consumed native dispatch grant, after confirmed start.
    #[must_use]
    pub fn new(
        request_id: RequestId,
        dispatch_millis: u64,
        request_start_millis: u64,
        absolute_deadline_millis: u64,
        timeout: ModelTimeoutPolicy,
        safety: GenerationSafetyPolicy,
    ) -> Self {
        Self {
            request_id,
            assembler: ModelEventAssembler::new(),
            guard: GenerationGuard::new(safety),
            timing: GenerationTiming::started_at(dispatch_millis, Some(request_start_millis)),
            liveness: ModelRequestDeadline::new(timeout, dispatch_millis),
            absolute_deadline_millis,
            started: false,
            failure: None,
            usage: None,
            usage_completeness: AuxiliaryUsageCompleteness::Unreported,
            provider_terminal: None,
        }
    }

    /// The next wake includes the absolute deadline even after `Completed`.
    #[must_use]
    pub fn deadline_millis(&self) -> u64 {
        self.liveness
            .deadline_millis()
            .map_or(self.absolute_deadline_millis, |liveness| {
                liveness.min(self.absolute_deadline_millis)
            })
    }

    /// The native owner uses this same check at timer and EOF arbitration.
    #[must_use]
    pub fn expiration(&self, now_millis: u64) -> Option<ModelError> {
        if now_millis >= self.absolute_deadline_millis {
            return Some(local_error(
                ModelErrorKind::Timeout,
                "auxiliary request exceeded its absolute deadline",
            ));
        }
        self.liveness.pending().and_then(|(phase, deadline)| {
            (now_millis >= deadline).then(|| ModelError::timeout(phase))
        })
    }

    /// Accepts one provider item. A returned failure must be sealed promptly;
    /// the owner must not poll the provider again or retry this request.
    ///
    /// # Errors
    /// Rejects malformed order, tools, expired deadlines and unsafe generation.
    /// A genuine `Failed` is returned immediately with its observed evidence.
    pub fn accept(&mut self, item: &ModelStreamItem, now_millis: u64) -> Result<(), ModelError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.accept_item(item, now_millis);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    #[allow(clippy::too_many_lines)] // Validation precedes all trusted evidence updates.
    fn accept_item(&mut self, item: &ModelStreamItem, now_millis: u64) -> Result<(), ModelError> {
        if self.provider_terminal.is_some() {
            return Err(invalid_stream("auxiliary stream item after terminal"));
        }
        if let ModelStreamItem::Progress(_) = item {
            if !self.started {
                return Err(invalid_stream("auxiliary stream progress before Started"));
            }
            if let Some(error) = self.expiration(now_millis) {
                return Err(error);
            }
            self.liveness.observe(item, now_millis);
            return Ok(());
        }
        let ModelStreamItem::Event(event) = item else {
            unreachable!("progress returned above")
        };
        if matches!(
            event,
            ModelEvent::ToolCallStarted { .. }
                | ModelEvent::ToolCallArgumentsDelta { .. }
                | ModelEvent::ToolCallCompleted { .. }
        ) {
            return Err(local_error(
                ModelErrorKind::Unsupported,
                "auxiliary requests have no tool execution authority",
            ));
        }
        let content_index = match event {
            ModelEvent::TextDelta { block_index, .. }
            | ModelEvent::ReasoningDelta { block_index, .. }
            | ModelEvent::RefusalDelta { block_index, .. }
            | ModelEvent::ContinuationState { block_index, .. } => Some(block_index.get()),
            _ => None,
        };
        if content_index.is_some_and(|index| index >= AUXILIARY_CONTENT_BLOCK_LIMIT) {
            return Err(invalid_stream(
                "auxiliary output exceeds the content-block limit",
            ));
        }
        // Refuse an oversized delta before the assembler allocates its copy.
        // Length completion still passes envelope validation so its genuine
        // provider terminal is not replaced by the local guard failure.
        let generation_failure = self.guard.observe(item).or_else(|| {
            if let ModelEvent::Completed { finish_reason, .. } = event {
                GenerationGuard::classify_completion(finish_reason)
            } else {
                None
            }
        });
        if !matches!(event, ModelEvent::Completed { .. })
            && let Some(failure) = generation_failure
        {
            return Err(ModelError::generation_failure(failure));
        }
        self.assembler
            .push_text_only(event)
            .map_err(|error| invalid_stream(&format!("{error:?}")))?;
        // Only accepted envelopes can update usage, terminal or timing.
        self.timing.observe(event, now_millis);
        match event {
            ModelEvent::Started => self.started = true,
            ModelEvent::UsageUpdate { usage } => {
                self.usage = Some(usage.clone());
                self.usage_completeness = AuxiliaryUsageCompleteness::Partial;
            }
            ModelEvent::Completed {
                finish_reason,
                usage,
            } => {
                self.provider_terminal = Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: finish_reason.into(),
                });
                if let Some(usage) = usage {
                    self.usage = Some(usage.clone());
                    self.usage_completeness = AuxiliaryUsageCompleteness::Complete;
                }
            }
            ModelEvent::Failed { error } => {
                if !matches!(
                    error.kind,
                    ModelErrorKind::Cancelled | ModelErrorKind::Timeout | ModelErrorKind::Transport
                ) {
                    self.provider_terminal = Some(AuxiliaryObservedProviderTerminal::Failed {
                        kind: error.kind.clone(),
                    });
                }
            }
            ModelEvent::TextDelta { .. }
            | ModelEvent::ReasoningDelta { .. }
            | ModelEvent::RefusalDelta { .. }
            | ModelEvent::ContinuationState { .. } => {}
            ModelEvent::ToolCallStarted { .. }
            | ModelEvent::ToolCallArgumentsDelta { .. }
            | ModelEvent::ToolCallCompleted { .. } => unreachable!("tools refused above"),
        }
        // Check the old liveness boundary before observing an item that would
        // refresh or remove it. A late terminal remains real provider evidence.
        if let Some(error) = self.expiration(now_millis) {
            return Err(error);
        }
        self.liveness.observe(item, now_millis);
        if let Some(failure) = generation_failure {
            return Err(ModelError::generation_failure(failure));
        }
        if let ModelEvent::Failed { error } = event {
            return Err(error.clone());
        }
        if matches!(
            event,
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                ..
            }
        ) {
            return Err(local_error(
                ModelErrorKind::Unsupported,
                "auxiliary completion requested tools",
            ));
        }
        Ok(())
    }

    /// Seals EOF after the native owner has accepted current authorization.
    /// An empty/refused output can be a valid execution; Context owns rejection.
    #[must_use]
    pub fn finish(mut self, now_millis: u64) -> SealedExecution {
        let output = match self.failure.take().or_else(|| self.expiration(now_millis)) {
            Some(error) => Err(error),
            None => std::mem::take(&mut self.assembler)
                .finish()
                .map_err(|error| invalid_stream(&format!("{error:?}")))
                .map(|assembled| {
                    let mut text = String::new();
                    let mut refused = assembled.finish_reason == ModelFinishReason::Refusal;
                    for block in assembled.content {
                        match block {
                            AssistantContentBlock::Text(block) => text.push_str(&block.text),
                            AssistantContentBlock::Refusal(_) => refused = true,
                            AssistantContentBlock::Reasoning(_)
                            | AssistantContentBlock::ToolCall(_)
                            | AssistantContentBlock::Image(_) => {}
                        }
                    }
                    // Neither reasoning nor provider continuation leaves this fold.
                    AuxiliaryTextOutput {
                        text,
                        refused,
                        finish_reason: assembled.finish_reason,
                    }
                }),
        };
        self.seal(output, now_millis)
    }

    /// Seals one native control, stream or supervision failure. Consumes this
    /// fold so later cancellation/SQL latency cannot rewrite its evidence.
    #[must_use]
    pub fn fail(self, error: ModelError, now_millis: u64) -> SealedExecution {
        self.seal(Err(error), now_millis)
    }

    fn seal(
        self,
        output: Result<AuxiliaryTextOutput, ModelError>,
        now_millis: u64,
    ) -> SealedExecution {
        // A normalized adapter failure does not prove remote termination.
        // Only a completion, or a definite rejection before Started, closes
        // that uncertainty. Local transport/cancellation failures have no
        // invented provider terminal at all.
        let remote_execution_uncertainty = !match &self.provider_terminal {
            Some(AuxiliaryObservedProviderTerminal::Completed { .. }) => true,
            Some(AuxiliaryObservedProviderTerminal::Failed { kind }) if !self.started => matches!(
                kind,
                ModelErrorKind::InvalidRequest
                    | ModelErrorKind::Authentication
                    | ModelErrorKind::RateLimit
                    | ModelErrorKind::ContextWindowExceeded
                    | ModelErrorKind::Unsupported
            ),
            _ => false,
        };
        SealedExecution {
            output,
            generation: self.timing.settle(now_millis),
            evidence: AuxiliaryRequestEvidence {
                request_id: self.request_id,
                remote_execution_uncertainty,
                observed_provider_terminal: self.provider_terminal,
                usage: self.usage,
                usage_completeness: self.usage_completeness,
            },
        }
    }
}

pub(super) fn local_error(kind: ModelErrorKind, message: &str) -> ModelError {
    ModelError {
        kind,
        message: message.into(),
        retry_disposition: ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}

fn invalid_stream(message: &str) -> ModelError {
    local_error(ModelErrorKind::InvalidRequest, message)
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
