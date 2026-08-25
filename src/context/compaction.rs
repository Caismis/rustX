//! The one context-compaction execution pipeline.
//!
//! Automatic compaction inside an [`AgentExecution`](crate::agent::AgentExecution)
//! and idle manual compaction owned by the conversation runtime share this
//! implementation. Planning, summary generation, exact post-summary fit
//! validation, the atomic durable commit, and installation into the hot
//! [`ConversationState`] therefore cannot drift between the two entry points.

use chrono::Utc;

use crate::context::ContextRuntime;
use crate::context::engine::CompactionConstraints;

/// The bounded number of summary-budget shrinks one compaction may attempt
/// after the summary model rejected its request as too large.
const MAX_SUMMARY_SHRINKS: u32 = 3;

/// The floor a shrinking summary input budget never crosses. Below it a
/// summary request could no longer carry a useful span, and the honest
/// outcome is an explicit compaction failure.
const MIN_SUMMARY_INPUT_LIMIT: u64 = 4_096;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::tokens::ProviderObservedInput;
use crate::conversation::ConversationState;
use crate::durable::{CompactionCommitInput, ConversationStore, TranscriptCursor};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::MessageBlock;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AttemptId, ConversationId, TurnId};
use crate::tools::types::ModelToolDefinition;

/// Optional execution attribution persisted with the compaction completion.
#[derive(Debug, Clone, Default)]
pub(crate) struct CompactionAttribution {
    pub(crate) attempt_id: Option<AttemptId>,
    pub(crate) turn_id: Option<TurnId>,
}

/// The already-committed output of one compaction execution.
pub(crate) struct ExecutedCompaction {
    pub(crate) summary_block: MessageBlock,
    pub(crate) persisted_event: RuntimeEventEnvelope,
    pub(crate) transcript_cursor: TranscriptCursor,
}

/// A compaction pipeline failure, split at the durable commit boundary.
pub(crate) enum CompactionExecutionError {
    /// No durable transition occurred; the previous conversation remains
    /// authoritative and may continue normally.
    Context(ContextError),
    /// The durable authority rejected the atomic compaction transition, or
    /// returned a result that violated the transition contract.
    Durable(String),
}

impl From<ContextError> for CompactionExecutionError {
    fn from(error: ContextError) -> Self {
        Self::Context(error)
    }
}

/// Plans, summarizes, validates, durably commits, and installs one compaction.
///
/// The caller exclusively owns `conversation` for the whole future. A
/// cancellation or context error installs nothing. Success means the summary
/// Ledger fact, Surface replacement, checkpoint, and `CompactionCompleted`
/// event were committed atomically before the hot state was updated.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn execute_compaction(
    conversation: &mut ConversationState,
    context: &ContextRuntime,
    conversation_id: &ConversationId,
    store: &dyn ConversationStore,
    tools: &[ModelToolDefinition],
    observed: Option<&ProviderObservedInput>,
    effective_system_prompt: &str,
    constraints: &CompactionConstraints<'_>,
    cancellation: &CancellationSignal,
    attribution: CompactionAttribution,
) -> Result<ExecutedCompaction, CompactionExecutionError> {
    if cancellation.is_cancelled() {
        return Err(cancelled("compaction cancelled before it began").into());
    }
    let projection =
        context
            .engine
            .build_projection(conversation, tools, observed, effective_system_prompt)?;
    let mut budgets = context.compaction_budgets;
    // The summary input budget is derived from a deterministic estimate, and
    // an estimate can be wrong. When the summary model itself rejects the
    // assembled request as too large, that rejection is the authoritative
    // measurement: the compaction replans the same transition against a
    // halved summary input budget instead of abandoning the conversation to
    // the overflow that triggered it. Each shrink strictly reduces the
    // budget, so the loop terminates.
    let mut shrinks: u32 = 0;
    let (plan, summary) = loop {
        let plan = context.engine.plan_compaction(
            conversation,
            &projection,
            tools,
            budgets,
            constraints,
        )?;
        let summary_request = plan.summary_request();
        let summary = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Err(cancelled("compaction cancelled while summarizing").into());
            }
            result = context
                .summarizer
                .summarize(summary_request, cancellation.child()) => result,
        };
        match summary {
            Ok(summary) => break (plan, summary),
            Err(error)
                if error.kind == ContextErrorKind::SummaryInputTooLarge
                    && shrinks < MAX_SUMMARY_SHRINKS
                    && budgets.summary_input_limit > MIN_SUMMARY_INPUT_LIMIT =>
            {
                shrinks += 1;
                budgets.summary_input_limit =
                    (budgets.summary_input_limit / 2).max(MIN_SUMMARY_INPUT_LIMIT);
            }
            Err(error) => return Err(error.into()),
        }
    };
    if cancellation.is_cancelled() {
        return Err(cancelled("compaction cancelled before the semantic commit").into());
    }

    let (prepared, projection) =
        context
            .engine
            .prepare_compaction(conversation, conversation_id, &plan, &summary, tools)?;
    let summary_block = prepared.summary_block();
    let exact_after = context.engine.estimate_with_staged_context(
        &projection,
        constraints.staged_request_context,
        tools,
    );
    // The exact post-compaction fit is checked against the same corrected
    // budget the plan was built with: after a provider has proven this
    // runtime's estimate optimistic, a plan that only fits the uncorrected
    // limit is not a plan that fits.
    let mut soft_limit = context
        .engine
        .soft_input_limit(budgets.primary_output_budget)?;
    if let Some(correction) = constraints.estimate_correction {
        soft_limit = correction.apply(soft_limit);
    }
    let fits = exact_after < soft_limit;
    if !fits {
        return Err(ContextError::new(
            ContextErrorKind::CannotFit,
            "the compacted surface still exceeds the soft input limit",
        )
        .into());
    }

    let expected_revision = prepared.expected_revision();
    let (durable_revision, durable_generation, persisted_event, transcript_cursor) = store
        .commit_compaction(CompactionCommitInput {
            summary: prepared.summary().clone(),
            span: prepared.span().clone(),
            expected_revision,
            tokens_before: plan.estimated_before,
            estimated_tokens_after: projection.estimated_input.input_tokens,
            attempt_id: attribution.attempt_id,
            turn_id: attribution.turn_id,
            timestamp: Utc::now(),
        })
        .map_err(|error| CompactionExecutionError::Durable(error.to_string()))?;
    if durable_revision != expected_revision.next() {
        return Err(CompactionExecutionError::Durable(
            "durable compaction returned an unexpected Surface revision".to_owned(),
        ));
    }
    let record = conversation.install_prepared_compaction(prepared);
    debug_assert_eq!(record.generation, durable_generation);
    debug_assert!(matches!(
        persisted_event.event,
        RuntimeEvent::CompactionCompleted { .. }
    ));
    Ok(ExecutedCompaction {
        summary_block,
        persisted_event,
        transcript_cursor,
    })
}

fn cancelled(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::Cancelled, message)
}
