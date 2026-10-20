//! Recovery inside one logical model step: transient retries against the
//! latest frozen input, context-overflow compact-and-retry, and the shared
//! semantic corrective-generation budget.

use super::request::PreparedModelTurn;
use super::stream::ModelInvocation;
use super::terminal::Terminal;
use super::{
    AgentExecution, MAX_MODEL_RETRY_DELAY_MS, MAX_SEMANTIC_CORRECTIVE_GENERATIONS_PER_LOGICAL_STEP,
};
use crate::agent::assembly::ModelEventAssembler;
use crate::context::engine::EstimateCorrection;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::generation::GenerationFailure;
use crate::runtime::identity::RequestId;

/// Computes one deterministic transient retry delay. The ordinal passed here
/// is the count of the transient retry (one-based), not the shared actual
/// request ordinal, because an overflow recovery may occur between transient
/// attempts without resetting this exponential sequence.
#[must_use]
pub(super) fn transient_retry_delay_ms(retry_count: u32, retry_after_ms: Option<u64>) -> u64 {
    retry_after_ms.map_or_else(
        || {
            2_000_u64
                .saturating_mul(1_u64 << retry_count.saturating_sub(1).min(2))
                .min(MAX_MODEL_RETRY_DELAY_MS)
        },
        |hint| hint.min(MAX_MODEL_RETRY_DELAY_MS),
    )
}

/// The provider-independent class of one semantic generation anomaly.
///
/// A semantic anomaly is a fact about the *generation*, never about the
/// request that carried it: the physical generation happened and its output
/// is unusable. The three classes below are the complete set, they are
/// recognized from the typed [`ModelErrorKind`] alone, and they share one
/// corrective-generation budget precisely because they are the same kind of
/// fact.
///
/// Nothing here reads a provider message, a provider code, or a finish-reason
/// string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticGenerationAnomaly {
    /// The generation's tool intent could not cross `ToolCall` acceptance.
    MalformedToolProposal,
    /// One generated channel deterministically degenerated into repetition.
    Degenerated,
    /// A generation budget was exhausted before the generation completed.
    BudgetExceeded,
}

impl SemanticGenerationAnomaly {
    /// Classifies one normalized model failure.
    ///
    /// Returns `None` for every transport, request, cancellation, and
    /// context-window failure: those are not defects of the generation and
    /// keep their own existing recovery architecture untouched.
    pub(super) fn classify(error: &ModelError) -> Option<Self> {
        match error.kind {
            ModelErrorKind::MalformedToolProposal => Some(Self::MalformedToolProposal),
            ModelErrorKind::GenerationDegenerated => Some(Self::Degenerated),
            ModelErrorKind::GenerationBudgetExceeded => Some(Self::BudgetExceeded),
            _ => None,
        }
    }

    /// The bounded generic instruction the corrective generation carries.
    ///
    /// It states the failure *class* and what to do instead. It never
    /// contains the discarded output: a repetition loop quoted back to the
    /// model is a few-shot example of the exact failure, and a truncated
    /// answer replayed as context is the same mistake in another form.
    pub(super) const fn instruction(self) -> &'static str {
        match self {
            Self::MalformedToolProposal => {
                "The previous response did not produce a structurally valid tool call \
                 and was discarded. Regenerate this step: to call a tool, emit one \
                 structured tool call with a declared tool name and complete JSON \
                 arguments; otherwise answer without calling a tool."
            }
            Self::Degenerated => {
                "The previous response repeated itself and was discarded before it \
                 was used. Regenerate this step: answer once, directly, and do not \
                 restate the same sentence or the same reasoning again."
            }
            Self::BudgetExceeded => {
                "The previous response did not finish within this step's generation \
                 budget and was discarded. Regenerate this step: think briefly, then \
                 answer completely and concisely within the limit."
            }
        }
    }
}

/// The semantic-generation recovery state of one logical model step.
///
/// The distinction this type exists to make explicit is **semantic
/// generation** versus **actual request**. A semantic anomaly authorizes one
/// *corrective generation*; an actual provider request is merely one attempt
/// to realize it. A transport failure, a rate limit, a timeout, or a
/// context-overflow recovery inside that corrective generation produces
/// another actual request of the *same* semantic generation, so the
/// corrective hint belongs to the generation and must survive every one of
/// them.
///
/// One budget serves every anomaly class. There is no per-class counter, so
/// no ordering of malformed proposals, degeneration, and budget exhaustion
/// can produce a third semantic generation for the same logical step.
///
/// Lifetime, exactly:
///
/// - **created** by [`Self::authorize`], when a semantic anomaly is observed
///   and the step's budget still allows a corrective generation;
/// - **carried** by every actual request built while it is pending —
///   [`Self::pending_hint`] borrows, it never takes, so no request
///   construction can consume it;
/// - **cleared** by [`Self::resolve`], the moment the corrective generation
///   produces a canonical model outcome;
/// - **discarded** with the replaced logical-step working set
///   (`LogicalStep::begin`) at every logical-model-step boundary, together
///   with the spent budget, so it can never reach a later step.
///
/// The budget lives here too, for the same reason: it is semantic state of
/// the step, not of any one request.
#[derive(Debug, Default)]
pub(super) struct SemanticGenerationRecovery {
    /// Corrective generations already authorized in this logical step.
    corrective_generations: u32,
    /// The bounded corrective hint owned by the corrective generation that
    /// has not yet resolved.
    pub(super) pending_hint: Option<CorrectiveHint>,
}

/// The bounded request-only corrective context of one corrective generation.
#[derive(Debug, Clone)]
pub(super) struct CorrectiveHint {
    /// The generic instruction of the anomaly class.
    pub(super) anomaly: SemanticGenerationAnomaly,
    /// The bounded provider-independent reason recorded with the discarded
    /// generation.
    pub(super) reason: String,
}

impl SemanticGenerationRecovery {
    /// Authorizes the corrective generation of one semantic anomaly, if the
    /// step's budget still allows it, and takes ownership of its hint.
    ///
    /// The reason is already bounded at its construction boundary: a
    /// malformed-proposal diagnostic is bounded by the model layer (see
    /// [`crate::model::MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES`]) and a
    /// generation-safety diagnostic is rendered from a typed all-numeric
    /// fact and can never carry provider output at all. There is one owner
    /// of each bound, and it is not this loop.
    pub(super) fn authorize(&mut self, anomaly: SemanticGenerationAnomaly, reason: String) -> bool {
        if self.corrective_generations >= MAX_SEMANTIC_CORRECTIVE_GENERATIONS_PER_LOGICAL_STEP {
            return false;
        }
        self.corrective_generations += 1;
        self.pending_hint = Some(CorrectiveHint { anomaly, reason });
        true
    }

    /// The corrective hint every actual request realizing the pending
    /// corrective generation must carry.
    pub(super) const fn pending_hint(&self) -> Option<&CorrectiveHint> {
        self.pending_hint.as_ref()
    }

    /// The pending corrective generation reached a canonical model outcome.
    /// Its hint has served its purpose and must not reach the tool-result
    /// continuation that follows.
    pub(super) fn resolve(&mut self) {
        self.pending_hint = None;
    }
}

impl<'a> AgentExecution<'a> {
    /// Replays the currently frozen logical-step semantics for another actual
    /// request. No Context Assembly, status generation, `FreshInbound`
    /// admission, contributor evaluation, or live-state sampling occurs here.
    /// The only changing input is the shared actual-request ordinal.
    pub(super) async fn retry_frozen_model_turn(
        &mut self,
        request_ordinal: u32,
        next_request_ordinal: &mut u32,
    ) -> Result<ModelInvocation, Terminal> {
        let input = *self
            .logical_step
            .current_input
            .as_ref()
            .ok_or_else(|| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    "retry has no current frozen input",
                ))
            })?
            .clone();
        let mut snapshot = input.snapshot;
        snapshot.identity.retry_number = request_ordinal;
        snapshot.request_id = snapshot.identity.request_id();
        snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
        for association in &mut snapshot.tool_associations {
            association.call.message.request_id = snapshot.request_id.clone();
            association.result.request_id = snapshot.request_id.clone();
        }
        // A genuine new request introduces no context; same-request recovery
        // retains the original snapshot in the durable start authority.
        snapshot.request_context_ids.clear();
        let prepared = PreparedModelTurn {
            #[cfg(feature = "issue-387-profile")]
            adapter_input_profile: input.adapter_input_profile,
            context: Vec::new(),
            snapshot,
            request: input.request,
            anchor: input.anchor,
            request_identity: input.request_identity,
            estimated_input: input.estimated_input,
        };
        let started = self.start_actual_model_request(prepared, next_request_ordinal)?;
        self.consume_invocation(started).await
    }

    /// Compile an explicitly authorized semantic version, then make it current at start.
    async fn start_recovered_model_turn(
        &mut self,
        request_ordinal: u32,
        next_request_ordinal: &mut u32,
    ) -> Result<ModelInvocation, Terminal> {
        let staged = self.stage_model_turn(request_ordinal, &[])?;
        let prepared = self.finalize_model_turn(&[], staged)?;
        let started = self.start_actual_model_request(prepared, next_request_ordinal)?;
        self.consume_invocation(started).await
    }

    /// The first actual request of the corrective generation authorized by
    /// one semantic generation anomaly.
    ///
    /// The failed physical generation is noncanonical and is discarded
    /// whole: its assembler, its provisional Assistant identity, its
    /// provisional content, and any provider continuation state it carried
    /// are dropped with the failed [`ModelInvocation`], so no rejected
    /// Assistant message, orphan canonical `ToolCall`, `ToolResult`, or
    /// provider continuation fragment can survive into the next request. The
    /// only thing that crosses into the corrective generation is one bounded
    /// provider-independent corrective hint, already installed on the
    /// step-owned [`SemanticGenerationRecovery`] by the caller.
    ///
    /// This is one actual request of the *same* logical step, not a new
    /// semantic decision: it reuses the frozen accepted context, freezes its
    /// own Request Snapshot, and passes through the same
    /// cancellation-vs-start arbitration as every other model request.
    /// Cancellation observable before it begins wins, so a remaining budget
    /// never authorizes another provider request after cancellation.
    ///
    /// If this request fails transiently, or overflows the context window,
    /// the ordinary recovery paths produce further actual requests of this
    /// same corrective generation. They do not re-enter this function and
    /// they do not touch the recovery state, so each of them carries the
    /// identical corrective hint.
    pub(super) async fn regenerate_after_semantic_anomaly(
        &mut self,
        failed_request_id: &RequestId,
        retry_number: u32,
        next_request_ordinal: &mut u32,
    ) -> Result<ModelInvocation, Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled_terminal());
        }
        // The abandoned publication is a durable predecessor of this
        // regeneration: the discarded generation's deltas settle into the
        // publication audit, never into canonical history.
        if let Err(error) = self.settle_publication_audit() {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled_terminal());
        }
        self.emit(RuntimeEvent::ModelRetryScheduled {
            failed_request_id: failed_request_id.clone(),
            retry_number,
            retry_delay_ms: None,
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        self.start_recovered_model_turn(retry_number, next_request_ordinal)
            .await
    }

    /// The bounded compact-and-retry path after a context overflow.
    ///
    /// The compaction must retire the continuation-owning turn completely
    /// (the constraint is passed to the context engine), the pending
    /// continuation is then invalidated, and the retry request uses the
    /// smaller projection with its own deterministic retry-specific
    /// provisional/committed message identity
    /// `{attempt}-agent-{turn}-retry-{retry_number}`.
    ///
    /// The retry is not a new admitted dynamic-context step. It reuses the
    /// already accepted context generation, status fact, Skill system section,
    /// and contributor output; the historical projection and request identity
    /// advance because compaction happened.
    ///
    /// The retry returns the complete retry invocation — provisional
    /// identity, assembler, and terminal together — so a successful retry
    /// replaces the failed invocation wholesale and the failed request's
    /// provisional content and tool calls are never committed or executed.
    ///
    /// If the retry also overflows, no second compaction occurs: the attempt
    /// settles with the second overflow error as its final model failure.
    pub(super) async fn retry_after_overflow(
        &mut self,
        overflow_error: &ModelError,
        failed_request_id: &RequestId,
        retry_number: u32,
        next_request_ordinal: &mut u32,
    ) -> Result<ModelInvocation, Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The abandoned publication is a durable predecessor of this retry.
        // It must settle before compaction, a retry snapshot/start commit, or
        // the second adapter invocation can occur.
        if let Err(error) = self.settle_publication_audit() {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        let effective_system_prompt = self.effective_system_prompt()?;
        let must_cover = self.continuation_owner.clone();
        // ContextWindowExceeded is a rejected provider request, not evidence
        // that a successful model invocation observed the fresh inbound
        // batch. Keep the pending constraint through overflow compaction;
        // the retry reuses the already-admitted context generation without
        // rerunning assembly.
        let fresh = self.pending_fresh_inbound.clone();
        // The provider has just measured what this runtime only estimated.
        // The recovery compaction plans against that measurement, not
        // against the estimate the provider rejected: without this the
        // retry aims at exactly the budget that already failed, and a
        // conversation whose content this estimator under-counts overflows
        // again.
        //
        // The measurement is read from the typed report the model adapter
        // attached when it classified the rejection. The agent loop never
        // reads the provider's prose: an unstructured diagnostic carries
        // unrelated large integers, and one of those mistaken for an input
        // count would produce a correction ratio that shrinks every budget
        // below usefulness. No report means the conservative unquantified
        // correction, which is exactly the honest answer.
        let estimate_correction = self
            .measurements
            .last_request_estimated_input
            .and_then(|estimated| {
                overflow_error
                    .context_overflow
                    .and_then(|report| report.reported_input_tokens)
                    .and_then(|observed| EstimateCorrection::new(estimated, observed))
            })
            .unwrap_or(EstimateCorrection::UNQUANTIFIED);
        match self
            .perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                &effective_system_prompt,
                Some(overflow_error),
                &[],
                Some(estimate_correction),
            )
            .await
        {
            Ok(()) => {}
            Err(terminal) => return Err(terminal),
        }
        // The cancellation check here is a cheap fast path only: it avoids
        // emitting `ModelRetryScheduled` once cancellation is already
        // observable. The retry's actual cancellation arbitration is the
        // same `start_model_turn` gate every other model request uses.
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The successful compaction already invalidated the incompatible
        // opaque provider continuation exactly once, inside
        // `perform_compaction`; this path never clears it a second time.
        self.emit(RuntimeEvent::ModelRetryScheduled {
            failed_request_id: failed_request_id.clone(),
            retry_number,
            retry_delay_ms: None,
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        // The retry is another actual provider request: it stages no new
        // request-scoped context (the original request's start transaction
        // already committed it), freezes its own Request Snapshot, and
        // passes through the one cancellation-vs-start arbitration exactly
        // like the first request of the turn.
        self.start_recovered_model_turn(retry_number, next_request_ordinal)
            .await
    }

    /// Settles one rejected physical generation.
    ///
    /// The typed fact becomes one normalized provider-independent
    /// [`ModelError`] whose diagnostic this runtime authored, exactly one
    /// durable `ModelRequestFailed` is recorded for the request that
    /// produced it, and the error is returned so the caller can send it
    /// through the ordinary logical-step recovery state machine. Nothing
    /// here decides recovery, and nothing here can commit.
    pub(super) fn settle_generation_anomaly(
        &mut self,
        assembler: &ModelEventAssembler,
        failure: GenerationFailure,
    ) -> Result<ModelError, Terminal> {
        let error = ModelError::generation_failure(failure);
        self.settle_model_request_failure(assembler, &error)?;
        #[cfg(test)]
        if let Some(pause) = self
            .pauses
            .generation_anomaly_pause
            .lock()
            .expect("generation anomaly pause lock")
            .as_ref()
        {
            // The rejected generation is durably settled and no corrective
            // generation has been authorized yet. A test can make
            // cancellation observable exactly at that cut.
            pause.park();
        }
        Ok(error)
    }

    /// Settles the failed request's publication before recording a transient
    /// retry schedule. Failed deltas remain in the durable publication audit;
    /// no failed stream can stay open while the next request is admitted.
    pub(super) fn schedule_transient_retry(
        &mut self,
        failed_request_id: RequestId,
        retry_number: u32,
        retry_delay_ms: u64,
    ) -> Result<(), Terminal> {
        if let Err(error) = self.settle_publication_audit() {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled_terminal());
        }
        self.emit(RuntimeEvent::ModelRetryScheduled {
            failed_request_id,
            retry_number,
            retry_delay_ms: Some(retry_delay_ms),
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        Ok(())
    }

    /// Waits for a retry deadline in the runtime-owned monotonic clock. The
    /// deadline is captured by the caller before the schedule commits, so the
    /// durable `ModelRetryScheduled` and the backoff deadline are one atomic
    /// fact. The cancellation branch is intentionally biased so an already
    /// observable cancellation wins before the clock can authorize another
    /// request.
    pub(super) async fn wait_for_retry_backoff(&self, deadline_ms: u64) -> Result<(), Terminal> {
        #[cfg(test)]
        if let Some(pause) = self
            .pauses
            .retry_backoff_pause
            .lock()
            .expect("retry backoff pause lock")
            .as_ref()
        {
            // `schedule_transient_retry` has already committed the schedule;
            // parking here additionally freezes the absolute deadline before
            // the test releases the wait, making manual-clock advancement an
            // exact proof of the delay rather than a timing hint.
            pause.park();
        }
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => Err(self.cancelled_terminal()),
            () = self.monotonic_clock.wait_until_millis(deadline_ms) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
