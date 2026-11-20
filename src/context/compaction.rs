//! The one context-compaction execution pipeline.
//!
//! Automatic compaction inside an [`AgentExecution`](crate::agent::AgentExecution)
//! and idle manual compaction owned by the conversation runtime share this
//! implementation. Planning, summary generation, exact post-summary fit
//! validation, the atomic durable commit, and installation into the hot
//! [`ConversationState`] therefore cannot drift between the two entry points.

use crate::context::checkpoint::{CheckpointCandidate, CheckpointItem, CheckpointMessage};
use crate::model::projected::{
    CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
    SemanticTransformation,
};
use chrono::Utc;

use crate::context::ContextRuntime;
use crate::context::engine::CompactionConstraints;

/// The exact prepared request is the viability check. Keep only a positive
/// arithmetic floor so small summary windows can still be repartitioned.
const MIN_SUMMARY_INPUT_LIMIT: u64 = 1;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::tokens::ProviderObservedInput;
use crate::conversation::ConversationState;
use crate::durable::ConversationStore;
use crate::events::types::RuntimeEventEnvelope;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AttemptId, ConversationId, TurnId};
use crate::tools::types::ModelToolDefinition;

mod install;
pub(crate) use install::{
    ActivationOutcome, ActivationPreview, CurrentInstallation, activate_generated,
    preview_generated,
};

/// Optional execution attribution persisted with the compaction completion.
#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)] // These are three independent identity domains.
pub(crate) struct CompactionAttribution {
    pub(crate) operation_id: crate::runtime::identity::CompactionOperationId,
    pub(crate) attempt_id: Option<AttemptId>,
    pub(crate) turn_id: Option<TurnId>,
}

impl Default for CompactionAttribution {
    fn default() -> Self {
        Self {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                uuid::Uuid::now_v7().to_string(),
            ),
            attempt_id: None,
            turn_id: None,
        }
    }
}

/// The already-committed output of one compaction execution.
#[derive(Debug)]
pub(crate) struct ExecutedCompaction {
    pub(crate) newly_committed: bool,
    pub(crate) persisted_event: RuntimeEventEnvelope,
    /// A degraded frozen carryover to install only after the checkpoint commit.
    pub(crate) carryover_level: Option<crate::model::CarryoverDetailLevel>,
}

/// A compaction pipeline failure, split at the durable commit boundary.
#[derive(Debug)]
pub(crate) enum CompactionExecutionError {
    /// No durable transition occurred; the previous conversation remains
    /// authoritative and may continue normally.
    Context(ContextError),
    /// The durable authority rejected the atomic compaction transition, or
    /// returned a result that violated the transition contract.
    Durable(String),
    /// Another valid activation won the expected-Head compare-and-swap.
    /// No receipt exists for this candidate; the current owner may revalidate.
    HeadChanged,
    /// Commit is confirmed; execution must reconcile or stop without a contradictory failure fact.
    Committed {
        completed: Box<ExecutedCompaction>,
        error: String,
    },
}

impl From<ContextError> for CompactionExecutionError {
    fn from(error: ContextError) -> Self {
        Self::Context(error)
    }
}

/// Owned copy of request-time constraints. A background Summary must not
/// borrow a triggering Attempt's staged inputs or carryover.
struct FrozenConstraints {
    focus: Option<String>,
    must_cover_through: Option<crate::runtime::identity::MessageId>,
    fresh_inbound: Option<crate::runtime::inbound::FreshInboundTurn>,
    protected_context_ids: Vec<crate::runtime::identity::MessageId>,
    staged_request_context: Vec<crate::message::types::MessageBlock>,
    carryover: Option<crate::model::RenderedUnresolvedOutputCarryover>,
    carryover_fallbacks: Vec<super::engine::CompactionCarryover>,
    carryover_anchor: Option<crate::model::RequestOnlyInsertionAnchor>,
    estimate_correction: Option<super::engine::EstimateCorrection>,
}

impl FrozenConstraints {
    fn capture(value: &CompactionConstraints<'_>) -> Self {
        Self {
            focus: value.focus.map(str::to_owned),
            must_cover_through: value.must_cover_through.cloned(),
            fresh_inbound: value.fresh_inbound.cloned(),
            protected_context_ids: value.protected_context_ids.to_vec(),
            staged_request_context: value.staged_request_context.to_vec(),
            carryover: value.carryover.cloned(),
            carryover_fallbacks: value.carryover_fallbacks.to_vec(),
            carryover_anchor: value.carryover_anchor.cloned(),
            estimate_correction: value.estimate_correction,
        }
    }

    fn view(&self, next_carryover: usize) -> CompactionConstraints<'_> {
        let carryover = if next_carryover == 0 {
            self.carryover.as_ref()
        } else {
            self.carryover_fallbacks[next_carryover - 1].value.as_ref()
        };
        CompactionConstraints {
            focus: self.focus.as_deref(),
            must_cover_through: self.must_cover_through.as_ref(),
            fresh_inbound: self.fresh_inbound.as_ref(),
            protected_context_ids: &self.protected_context_ids,
            staged_request_context: &self.staged_request_context,
            carryover,
            carryover_fallbacks: &self.carryover_fallbacks,
            carryover_anchor: if next_carryover > 0 && carryover.is_none() {
                None
            } else {
                self.carryover_anchor.as_ref()
            },
            estimate_correction: self.estimate_correction,
        }
    }
}

/// Immutable source and invocation inputs shared by generation and its result.
struct CapturedSource {
    history: crate::durable::projection::HistoricalModelInput,
    cut: super::resources::ResourceReadCut,
    historical_items: Vec<CheckpointItem>,
    previous: Option<crate::durable::projection::ProjectionCheckpoint>,
    retired_count: usize,
    attribution: CompactionAttribution,
    engine: super::engine::ContextEngine,
    projection: super::projection::ContextProjection,
    tools: Vec<ModelToolDefinition>,
    effective_system_prompt: String,
    constraints: FrozenConstraints,
    budgets: super::engine::CompactionBudgets,
    primary_model: String,
    maximum_summary_output: u32,
}

/// Frozen finite capture; it carries no permission to replace the Head.
pub(crate) struct CapturedCompaction {
    source: CapturedSource,
    budget: super::compaction_budget::CompactionBudget,
    prepared: Box<dyn super::summarizer::PreparedSummary>,
    chunks: super::chunks::ChunkPlan,
    next_carryover: usize,
    carryover_level: Option<crate::model::CarryoverDetailLevel>,
}

#[allow(dead_code)] // The Session owner inspects the frozen prefix before spawning generation.
impl CapturedCompaction {
    pub(crate) fn source_history(&self) -> &crate::durable::projection::HistoricalModelInput {
        &self.source.history
    }

    pub(crate) fn source_cut(&self) -> &super::resources::ResourceReadCut {
        &self.source.cut
    }

    pub(crate) fn source_checkpoint(
        &self,
    ) -> Option<&crate::durable::projection::ProjectionCheckpoint> {
        self.source.previous.as_ref()
    }

    pub(crate) fn retired_count(&self) -> usize {
        self.source.retired_count
    }
}

/// A locally validated Summary over one captured source. The candidate is
/// evidence of the captured fit only: it is not authorized for async install.
pub(crate) struct GeneratedCompaction {
    source: CapturedSource,
    captured_candidate: CheckpointCandidate,
    summary: super::summarizer::SummaryGeneration,
    completed_coverage: super::chunks::CompletedSourceCoverage,
    after_projection: super::projection::ContextProjection,
    after: u64,
    carryover_level: Option<crate::model::CarryoverDetailLevel>,
}

#[allow(dead_code)] // The Session-scoped installer consumes these in #529.
impl GeneratedCompaction {
    pub(crate) fn source_history(&self) -> &crate::durable::projection::HistoricalModelInput {
        &self.source.history
    }

    pub(crate) fn source_cut(&self) -> &super::resources::ResourceReadCut {
        &self.source.cut
    }

    pub(crate) fn retired_count(&self) -> usize {
        self.source.retired_count
    }

    pub(crate) fn source_checkpoint(
        &self,
    ) -> Option<&crate::durable::projection::ProjectionCheckpoint> {
        self.source.previous.as_ref()
    }

    pub(crate) fn completed_coverage(&self) -> &super::chunks::CompletedSourceCoverage {
        &self.completed_coverage
    }

    /// The candidate was priced against the capture and must be rebuilt for
    /// current installation authority before an asynchronous commit.
    pub(crate) fn captured_candidate(&self) -> &CheckpointCandidate {
        &self.captured_candidate
    }

    pub(crate) fn generated_summary(&self) -> &DerivedModelMessage {
        match &self.captured_candidate.items[0].message {
            CheckpointMessage::Derived(summary) => summary,
            CheckpointMessage::Original(_) => unreachable!("generated Summary is Derived"),
        }
    }

    pub(crate) fn summary_generation(&self) -> &super::summarizer::SummaryGeneration {
        &self.summary
    }
}

/// Standalone Context fixtures own the same operation lifetime explicitly.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
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
    let budget = super::compaction_budget::CompactionBudget::new(
        attribution.operation_id.clone(),
        context.monotonic_clock.clone(),
    );
    budget.check(cancellation)?;
    let append = |event| -> Result<(), CompactionExecutionError> {
        store
            .append_event(crate::events::types::RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                event_id: crate::runtime::identity::EventId::new(""),
                sequence: 0,
                conversation_id: conversation_id.clone(),
                attempt_id: attribution.attempt_id.clone(),
                turn_id: attribution.turn_id.clone(),
                timestamp: Utc::now(),
                event,
            })
            .map(|_| ())
            .map_err(resource_failure)
    };
    append(crate::events::types::RuntimeEvent::CompactionStarted {
        operation_id: attribution.operation_id.clone(),
        mode: crate::context::operation::CompactionMode::Blocking,
    })?;
    let operation = context
        .summarizer
        .operation(budget.clone(), cancellation.child())?;
    let result = execute_compaction_in_operation(
        conversation,
        context,
        conversation_id,
        store,
        tools,
        observed,
        effective_system_prompt,
        constraints,
        cancellation,
        attribution.clone(),
        operation.as_ref(),
        None,
    )
    .await;
    operation.close().await;
    if let Err(CompactionExecutionError::Context(error)) = &result {
        if error.kind == ContextErrorKind::NoEligibleHistory {
            append(crate::events::types::RuntimeEvent::CompactionSkipped {
                operation_id: attribution.operation_id.clone(),
            })?;
        } else if error.kind == ContextErrorKind::Cancelled {
            append(crate::events::types::RuntimeEvent::CompactionCancelled {
                operation_id: attribution.operation_id.clone(),
            })?;
        } else {
            append(crate::events::types::RuntimeEvent::CompactionFailed {
                operation_id: attribution.operation_id.clone(),
                error: error.message.clone(),
            })?;
        }
    }
    result
}

/// Plans, summarizes, validates, durably commits, and installs one compaction.
///
/// The caller exclusively owns `conversation` for the whole future. A
/// cancellation or context error installs nothing. Success means the typed
/// checkpoint, activation/head, operation receipt and actual completion fact
/// were committed atomically before the current durable head was installed.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn execute_compaction_in_operation(
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
    operation: &dyn super::summarizer::SummaryOperation,
    progress: Option<&(dyn Fn(RuntimeEventEnvelope) + Send + Sync)>,
) -> Result<ExecutedCompaction, CompactionExecutionError> {
    // Blocking and manual work retain the owner and deadline they admitted.
    let budget = operation.budget();
    if budget.operation_id() != &attribution.operation_id {
        return Err(ContextError::new(
            ContextErrorKind::AuxiliaryNotStarted,
            "Summary operation belongs to another Compaction",
        )
        .into());
    }
    budget.check(cancellation)?;
    let owner = store
        .projection_owner(attribution.attempt_id.as_ref())
        .map_err(resource_failure)?;
    let captured = capture_compaction(
        context,
        conversation.revision(),
        conversation_id,
        store,
        tools,
        observed,
        effective_system_prompt,
        constraints,
        cancellation,
        attribution,
        operation,
    )
    .await?;
    let publish_progress = |envelope| -> Result<(), CompactionExecutionError> {
        let persisted = store.append_event(envelope).map_err(resource_failure)?;
        if let Some(progress) = progress {
            progress(persisted);
        }
        Ok(())
    };
    let generated = generate_compaction(
        captured,
        store,
        operation,
        cancellation,
        &publish_progress,
        &|| Ok(()),
    )
    .await?;
    activate_blocking_generated(
        conversation,
        store,
        operation,
        cancellation,
        owner,
        generated,
    )
}

/// Preserve the admitted #522 owner, deadline and scope at blocking commit.
pub(crate) fn activate_blocking_generated(
    conversation: &mut ConversationState,
    store: &dyn ConversationStore,
    operation: &dyn super::summarizer::SummaryOperation,
    cancellation: &CancellationSignal,
    owner: crate::durable::projection::ProjectionOwner,
    generated: GeneratedCompaction,
) -> Result<ExecutedCompaction, CompactionExecutionError> {
    let prefix_proof = super::coverage::VerifiedPrefixCoverage::seal_current(&generated)?;
    operation.budget().check(cancellation)?;
    let grant = operation.authorize_commit()?;
    let GeneratedCompaction {
        source,
        mut captured_candidate,
        after_projection,
        after,
        carryover_level,
        ..
    } = generated;
    captured_candidate.prefix_coverage = Some(prefix_proof.record().clone());
    let input = crate::durable::projection::ProjectionCommitInput {
        grant,
        operation_id: source.attribution.operation_id,
        candidate: captured_candidate,
        owner,
        prefix_proof: Some(prefix_proof),
        tokens_before: source.projection.estimated_input,
        estimated_tokens_after: after_projection.estimated_input.input_tokens,
        occupancy: Some(crate::context::occupancy::estimate(
            after,
            source.engine.config().context_window_tokens,
            &source.primary_model,
            &source.effective_system_prompt,
            &source.tools,
        )),
        turn_id: source.attribution.turn_id,
        timestamp: Utc::now(),
    };
    commit_compaction(conversation, store, input, carryover_level)
}

/// Freeze one historical cut and its Summary plan. The native caller must be
/// at a structurally complete safe boundary before admitting this capture.
/// The operation and its first reservation must remain with the native owner.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn capture_compaction(
    context: &ContextRuntime,
    revision: crate::conversation::SurfaceRevision,
    conversation_id: &ConversationId,
    store: &dyn ConversationStore,
    tools: &[ModelToolDefinition],
    observed: Option<&ProviderObservedInput>,
    effective_system_prompt: &str,
    constraints: &CompactionConstraints<'_>,
    cancellation: &CancellationSignal,
    attribution: CompactionAttribution,
    operation: &dyn super::summarizer::SummaryOperation,
) -> Result<CapturedCompaction, CompactionExecutionError> {
    let budget = operation.budget();
    if budget.operation_id() != &attribution.operation_id {
        return Err(ContextError::new(
            ContextErrorKind::AuxiliaryNotStarted,
            "Summary operation belongs to another Compaction",
        )
        .into());
    }
    budget.check(cancellation)?;
    let prepared = operation.prepare()?;
    let (history, cut) = store.auxiliary_history().map_err(resource_failure)?;
    let previous = store
        .projection_at_journal_cut(cut.journal.0)
        .map_err(resource_failure)?;
    if &cut.conversation_id != conversation_id
        || history.cut.ledger != cut.ledger
        || history.cut.activation != cut.projection_activation
        || previous.as_ref().map(|p| &p.candidate.checkpoint_id)
            != history.cut.checkpoint_id.as_ref()
    {
        return Err(CompactionExecutionError::Durable(
            "compaction capture is not a consistent complete historical boundary".into(),
        ));
    }
    let historical_items = history
        .checkpoint_items(previous.as_ref())
        .map_err(resource_failure)?;
    let complete = super::engine::assemble_context_input(
        &history.messages,
        constraints.staged_request_context,
        constraints.carryover,
        constraints.carryover_anchor,
    )
    .map_err(|e| ContextError::new(ContextErrorKind::MalformedHistory, e))?;
    let mut engine = context.engine.clone();
    engine
        .prepare_media(&history.messages, &complete, cancellation)
        .await?;
    let projection = engine.build_projection_input(
        revision,
        history.messages.clone(),
        tools,
        observed,
        effective_system_prompt,
    );
    let mut budgets = context.compaction_budgets;
    budgets.summary_input_limit = budget.summary_input_limit(budgets.summary_input_limit);
    let constraints = FrozenConstraints::capture(constraints);
    let mut active_constraints = constraints.view(0);
    let mut next_carryover = 0;
    let mut carryover_level = None;
    let retired_count = loop {
        match super::planning::summary_prefix(
            &engine,
            &projection.messages,
            effective_system_prompt,
            tools,
            budgets,
            &active_constraints,
            cancellation,
        )
        .await
        {
            Ok(count) => break count,
            Err(error)
                if matches!(
                    error.kind,
                    ContextErrorKind::CannotFit | ContextErrorKind::SearchIncomplete
                ) =>
            {
                let Some(level) = advance_carryover(
                    &mut active_constraints,
                    &mut next_carryover,
                    &budget,
                    cancellation,
                )?
                else {
                    return Err(error.into());
                };
                carryover_level = Some(level);
            }
            Err(error) => return Err(error.into()),
        }
    };
    // Inline provenance is a finite capability. Admit it before invoking
    // the provider rather than generating a candidate we cannot commit.
    let mut refs = std::collections::BTreeSet::new();
    let mut retained_refs = 0usize;
    for (index, item) in historical_items.iter().enumerate() {
        if index < retired_count {
            match &item.message {
                crate::context::checkpoint::CheckpointMessage::Original(r) => {
                    refs.insert((r.conversation_id.clone(), r.message_id.clone()));
                }
                crate::context::checkpoint::CheckpointMessage::Derived(d)
                    if !d.transformations.contains(&SemanticTransformation::Summary) =>
                {
                    refs.extend(
                        d.sources
                            .iter()
                            .map(|r| (r.conversation_id.clone(), r.message_id.clone())),
                    );
                }
                crate::context::checkpoint::CheckpointMessage::Derived(_) => {}
            }
        } else {
            retained_refs += item.call_sources.len();
            if let crate::context::checkpoint::CheckpointMessage::Derived(d) = &item.message {
                retained_refs += d.sources.len() + d.checkpoint_sources.len();
            }
        }
    }
    if refs.len() + retained_refs + usize::from(previous.is_some()) > 1024 {
        return Err(ContextError::new(
            ContextErrorKind::CannotFit,
            "compaction provenance exceeds inline budget before summary invocation",
        )
        .into());
    }
    // This is the same already-captured media projection the primary request
    // sees. It renders upload paths without reading the referenced files and
    // preserves every projected occurrence's index for coverage evidence.
    let rendered = engine
        .rendered_input(&projection.messages)
        .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
    let chunks =
        super::chunks::ChunkPlan::new_captured(&history.messages, &rendered, retired_count, &cut)?;
    budget.admit_work(chunks.planned_bound())?;
    Ok(CapturedCompaction {
        source: CapturedSource {
            history,
            cut,
            historical_items,
            previous,
            retired_count,
            attribution,
            engine,
            projection,
            tools: tools.to_vec(),
            effective_system_prompt: effective_system_prompt.to_owned(),
            constraints,
            budgets,
            primary_model: context.primary_model.clone(),
            maximum_summary_output: context.maximum_summary_output,
        },
        budget,
        prepared,
        chunks,
        next_carryover,
        carryover_level,
    })
}

/// Run the one ordered Summary generator over a detached capture. No mutable
/// Conversation or installation authority enters this phase.
#[allow(clippy::too_many_lines)]
pub(crate) async fn generate_compaction(
    captured: CapturedCompaction,
    store: &dyn ConversationStore,
    operation: &dyn super::summarizer::SummaryOperation,
    cancellation: &CancellationSignal,
    progress: &(dyn Fn(RuntimeEventEnvelope) -> Result<(), CompactionExecutionError> + Send + Sync),
    before_call: &(dyn Fn() -> Result<(), ContextError> + Send + Sync),
) -> Result<GeneratedCompaction, CompactionExecutionError> {
    let CapturedCompaction {
        source,
        budget,
        prepared,
        mut chunks,
        mut next_carryover,
        mut carryover_level,
    } = captured;
    if !budget.same_operation(&operation.budget()) {
        return Err(ContextError::new(
            ContextErrorKind::AuxiliaryNotStarted,
            "Summary generator does not own the captured operation",
        )
        .into());
    }
    budget.check(cancellation)?;
    let mut prepared = Some(prepared);
    let mut resources = super::resources::CompactionResources::at_cut(
        store,
        source.cut.clone(),
        super::resources::ResourceLimits::default(),
    )
    .map_err(resource_failure)?;
    let history = &source.history;
    let previous = &source.previous;
    let retired_count = source.retired_count;
    let attribution = &source.attribution;
    let conversation_id = &source.cut.conversation_id;
    let engine = &source.engine;
    let projection = &source.projection;
    let tools = source.tools.as_slice();
    let effective_system_prompt = source.effective_system_prompt.as_str();
    let constraints = source.constraints.view(next_carryover);
    let mut active_constraints = source.constraints.view(next_carryover);
    let mut budgets = source.budgets;
    let monotone = engine.supports_monotone_compaction(&projection.messages);
    let mut summary: Option<super::summarizer::SummaryGeneration> = None;
    let mut reductions = 0u8;
    let mut accepted_fragments = 0u32;
    let mut output_budget = budgets.summary_output_budget;
    let output_ceiling = |budgets: super::CompactionBudgets| {
        source.maximum_summary_output.min(
            budgets.summary_output_budget.saturating_add(
                u32::try_from(
                    budgets
                        .summary_input_limit
                        .saturating_sub(MIN_SUMMARY_INPUT_LIMIT),
                )
                .unwrap_or(u32::MAX),
            ),
        )
    };
    while !chunks.is_complete() {
        budget.check_start(cancellation)?;
        let estimate = |request: &super::summarizer::SummaryRequest| -> Result<u64, ContextError> {
            operation.preview(request).map(|native| {
                native.map_or_else(
                    || engine.estimate_model_input(&request.model_input().messages.into(), "", &[]),
                    |request| engine.estimate_frozen_request(&request),
                )
            })
        };
        let current_input_limit = summary_input_limit(budgets, output_budget);
        let request = match chunks.next(
            summary.as_ref(),
            constraints.focus,
            Some(output_budget),
            current_input_limit,
            monotone,
            estimate,
        ) {
            Ok(request) => request,
            Err(error)
                if error.kind == ContextErrorKind::CannotFit
                    && summary.is_some()
                    && reductions < 3 =>
            {
                budget.charge_recovery(cancellation)?;
                let previous = summary.as_ref().expect("checked accepted summary");
                let reduction = super::summarizer::SummaryRequest {
                    previous: Some(super::chunks::SummaryCarryover {
                        text: previous.text.clone(),
                        generation: previous.generation.clone(),
                    }),
                    focus: constraints.focus.map(str::to_owned),
                    output_tokens: Some(output_budget),
                    ..Default::default()
                };
                let reduced = reduce_summary(
                    reduction,
                    operation,
                    engine,
                    budgets,
                    &mut output_budget,
                    output_ceiling(budgets),
                    resources.cut(),
                    cancellation,
                    before_call,
                )
                .await?;
                if reduced.text == previous.text {
                    return Err(ContextError::new(
                        ContextErrorKind::NoProgress,
                        "rolling Summary reduction returned unchanged text; no new fragment capacity was created",
                    )
                    .into());
                }
                summary = Some(reduced);
                reductions += 1;
                continue;
            }
            Err(error) if error.kind == ContextErrorKind::CannotFit => {
                let overhead = super::summarizer::SummaryRequest {
                    previous: summary
                        .as_ref()
                        .map(|value| super::chunks::SummaryCarryover {
                            text: value.text.clone(),
                            generation: value.generation.clone(),
                        }),
                    focus: constraints.focus.map(str::to_owned),
                    output_tokens: Some(output_budget),
                    ..Default::default()
                };
                let overhead_estimate = estimate(&overhead)?;
                return Err(ContextError::new(
                    ContextErrorKind::CannotFit,
                    format!("summary input capacity: {}; request without the next source estimate={overhead_estimate}, summary input limit estimate={current_input_limit}; these estimates are not provider usage", error.message),
                ).into());
            }
            Err(error) => return Err(error.into()),
        };
        before_call()?;
        let call = match prepared.take() {
            Some(call) => call,
            None => operation.prepare()?,
        };
        match call
            .summarize(request.clone(), Some(resources.cut().clone()))
            .await
        {
            Ok(accepted) if !accepted.text.trim().is_empty() => {
                chunks.accept(&request)?;
                // Scripted Context consumers have no native physical Call or
                // generation authority. Keep their cursor private rather than
                // publishing a progress fact that cannot be substantiated.
                if accepted.generation.is_some() {
                    accepted_fragments += 1;
                    progress(crate::events::types::RuntimeEventEnvelope {
                        tool_occurrence: None,
                        observation: None,
                        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                        event_id: crate::runtime::identity::EventId::new(""),
                        sequence: 0,
                        conversation_id: conversation_id.clone(),
                        attempt_id: attribution.attempt_id.clone(),
                        turn_id: attribution.turn_id.clone(),
                        timestamp: Utc::now(),
                        event: crate::events::types::RuntimeEvent::CompactionChunkAccepted {
                            operation_id: attribution.operation_id.clone(),
                            accepted_fragments,
                            source_ranges: request
                                .fragment
                                .as_ref()
                                .map_or_else(Vec::new, |fragment| fragment.ranges.clone()),
                            generation: accepted.generation.clone(),
                        },
                    })?;
                }
                summary = Some(accepted);
            }
            Ok(_) => {
                return Err(ContextError::new(
                    ContextErrorKind::SummaryFailed,
                    "summary generation produced no complete accepted content",
                )
                .into());
            }
            Err(error) if error.kind == ContextErrorKind::SummaryFormatChanged => {}
            Err(error)
                if error.kind == ContextErrorKind::SummaryOutputExhausted
                    && output_budget < output_ceiling(budgets) =>
            {
                budget.charge_recovery(cancellation)?;
                output_budget = output_budget.saturating_mul(2).min(output_ceiling(budgets));
                // Same unprocessed source, with more total output and a smaller
                // input fragment. Provider-native reasoning controls are unchanged.
            }
            Err(error)
                if error.kind == ContextErrorKind::SummaryInputTooLarge
                    && current_input_limit > MIN_SUMMARY_INPUT_LIMIT =>
            {
                let rejected_estimate = estimate(&request)?;
                if rejected_estimate <= MIN_SUMMARY_INPUT_LIMIT {
                    return Err(ContextError::new(
                        ContextErrorKind::SummaryInputTooLarge,
                        "the rejected Summary request has no smaller positive input estimate",
                    )
                    .into());
                }
                let additional_output =
                    u64::from(output_budget.saturating_sub(budgets.summary_output_budget));
                // Price the exact failed request and ensure that request no
                // longer satisfies the next preflight, even when it was much
                // smaller than the old nominal input limit.
                budget.charge_recovery(cancellation)?;
                budgets.summary_input_limit = budget.shrink_summary_input_limit(
                    budgets.summary_input_limit,
                    additional_output,
                    MIN_SUMMARY_INPUT_LIMIT,
                    rejected_estimate - 1,
                    cancellation,
                )?;
            }
            Err(error) if error.kind == ContextErrorKind::SummaryInputTooLarge => {
                return Err(ContextError::new(
                    ContextErrorKind::SummaryInputTooLarge,
                    format!("summary input capacity: provider rejected the complete request at summary input limit estimate={current_input_limit}; no smaller positive input limit remains: {}", error.message),
                ).into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    // Only ChunkPlan can mint this witness, after every frozen source range
    // has actually been accepted. Later reductions retain the same coverage.
    let completed_coverage = chunks.completed_coverage()?;
    if usize::try_from(completed_coverage.source_count()).ok() != Some(retired_count) {
        return Err(ContextError::new(
            ContextErrorKind::Internal,
            "completed Summary coverage disagrees with the captured source",
        )
        .into());
    }
    let mut summary = summary.ok_or_else(|| {
        ContextError::new(
            ContextErrorKind::NoProgress,
            "there is no eligible historical material to summarize",
        )
    })?;
    let mut sources = std::collections::BTreeMap::new();
    let mut checkpoint_sources = std::collections::BTreeMap::new();
    let mut includes_previous_summary = false;
    for message in projection.messages.iter().take(retired_count) {
        match message {
            crate::model::ModelInputMessage::Canonical(original) => {
                let source = CanonicalMessageRef::of(conversation_id, original);
                sources.insert(source.message_id.clone(), source);
            }
            crate::model::ModelInputMessage::Derived(derived) => {
                if derived
                    .transformations
                    .contains(&SemanticTransformation::Summary)
                {
                    includes_previous_summary = true;
                } else {
                    for source in &derived.sources {
                        sources.insert(source.message_id.clone(), source.clone());
                    }
                    for source in &derived.checkpoint_sources {
                        checkpoint_sources.insert(source.checkpoint_id.clone(), source.clone());
                    }
                }
            }
            crate::model::ModelInputMessage::RequestOnly(_) => {
                return Err(CompactionExecutionError::Durable(
                    "core-owned input entered historical compaction".into(),
                ));
            }
        }
    }
    if let Some(previous) = previous
        && includes_previous_summary
    {
        let source = CheckpointSourceRef {
            conversation_id: conversation_id.clone(),
            checkpoint_id: previous.candidate.checkpoint_id.clone(),
            content_digest: previous.fingerprint.clone(),
        };
        checkpoint_sources.insert(source.checkpoint_id.clone(), source);
    }
    let ids: Vec<_> = sources.keys().cloned().collect();
    let mut canonical_sources = Vec::new();
    for ids in ids.chunks(64) {
        canonical_sources.extend(
            resources
                .originals(ids)
                .map_err(resource_failure)?
                .rows
                .into_iter()
                .map(|(_, m)| m),
        );
    }
    let physical_metadata =
        crate::context::compaction_metadata::retired_span_metadata(&canonical_sources);
    let inherited_metadata = previous
        .as_ref()
        .filter(|_| includes_previous_summary)
        .and_then(|p| p.candidate.summary_metadata.as_ref());
    let metadata = crate::message::types::CompactionSummaryMetadata::accumulate(
        inherited_metadata,
        physical_metadata.read_files().to_vec(),
        physical_metadata.modified_files().to_vec(),
    );
    let source_values: Vec<_> = sources.into_values().collect();
    let checkpoint_values: Vec<_> = checkpoint_sources.into_values().collect();
    let history_before = engine.estimate_model_input(&projection.messages, "", &[]);
    let (candidate, after_projection, after) = loop {
        let derived = DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: crate::context::compaction_metadata::render_summary_text(
                            &summary.text,
                            &metadata,
                        ),
                    },
                )],
                source: crate::message::types::UserSource::Runtime,
                kind: crate::model::projected::ProjectedUserKind::CompactionContext,
                timestamp: None,
            },
            sources: source_values.clone(),
            checkpoint_sources: checkpoint_values.clone(),
            transformations: vec![SemanticTransformation::Summary],
        };
        let checkpoint_id =
            crate::runtime::identity::CheckpointId::new(uuid::Uuid::now_v7().to_string());
        let mut items = vec![CheckpointItem {
            message: CheckpointMessage::Derived(Box::new(derived)),
            call_sources: Vec::new(),
            result_call: None,
        }];
        for mut item in source.historical_items.iter().skip(retired_count).cloned() {
            if let Some(call) = &mut item.result_call {
                call.message.checkpoint_id = checkpoint_id.clone();
                call.message.item_index = call
                    .message
                    .item_index
                    .checked_sub(u32::try_from(retired_count).map_err(|_| {
                        CompactionExecutionError::Durable("checkpoint occurrence overflow".into())
                    })?)
                    .and_then(|n| n.checked_add(1))
                    .ok_or_else(|| {
                        CompactionExecutionError::Durable(
                            "retained result crosses retired call".into(),
                        )
                    })?;
            }
            items.push(item);
        }
        let candidate = CheckpointCandidate {
            checkpoint_id,
            expected_head: history.cut.checkpoint_id.clone(),
            read_cut: resources.cut().clone(),
            base_frontier: history.cut.ledger,
            prefix_coverage: None,
            items,
            summary_metadata: Some(metadata.clone()),
            summary_generation: summary.generation.clone(),
        };
        let compiled =
            crate::context::checkpoint::compile_checkpoint(&candidate, |source, body_required| {
                resources
                    .dependency(source, body_required)
                    .map_err(resource_failure)
            })?;
        let after_projection = engine.build_projection_input(
            projection.surface_revision,
            compiled.messages.into(),
            tools,
            None,
            effective_system_prompt,
        );
        let mut after = engine.estimate_with_staged_context_and_carryover(
            &after_projection,
            active_constraints.staged_request_context,
            tools,
            active_constraints.carryover,
            active_constraints.carryover_anchor,
        );
        let mut limit = engine.admissible_input_limit(budgets.primary_output_budget)?;
        if let Some(correction) = active_constraints.estimate_correction {
            limit = correction.apply(limit);
        }
        while after > limit {
            let Some(level) = advance_carryover(
                &mut active_constraints,
                &mut next_carryover,
                &budget,
                cancellation,
            )?
            else {
                break;
            };
            carryover_level = Some(level);
            after = engine.estimate_with_staged_context_and_carryover(
                &after_projection,
                active_constraints.staged_request_context,
                tools,
                active_constraints.carryover,
                active_constraints.carryover_anchor,
            );
        }
        let history_after = engine.estimate_model_input(&after_projection.messages, "", &[]);
        if after <= limit && history_after < history_before {
            break (candidate, after_projection, after);
        }
        if reductions >= 3 {
            if after > limit {
                return Err(ContextError::new(
                    ContextErrorKind::CannotFit,
                    format!("final Summary plus protected/fixed request input still exceeds primary admission after bounded reductions; {}",
                        super::planning::primary_fit_evidence(engine, &after_projection.messages, effective_system_prompt, tools, &active_constraints, after, limit)),
                ).into());
            }
            return Err(ContextError::new(
                ContextErrorKind::NoProgress,
                format!("no historical progress after bounded Summary reductions: historical input estimate before={history_before}, after={history_after}; these estimates are not provider usage"),
            ).into());
        }
        budget.charge_recovery(cancellation)?;
        let reduction = super::summarizer::SummaryRequest {
            previous: Some(super::chunks::SummaryCarryover {
                text: summary.text.clone(),
                generation: summary.generation.clone(),
            }),
            focus: active_constraints.focus.map(str::to_owned),
            output_tokens: Some(output_budget),
            ..Default::default()
        };
        let reduced = reduce_summary(
            reduction,
            operation,
            engine,
            budgets,
            &mut output_budget,
            output_ceiling(budgets),
            resources.cut(),
            cancellation,
            before_call,
        )
        .await?;
        if reduced.text == summary.text {
            return Err(ContextError::new(
                ContextErrorKind::NoProgress,
                "final Summary reduction returned unchanged text; no historical progress was made",
            )
            .into());
        }
        summary = reduced;
        reductions += 1;
    };
    budget.check(cancellation)?;
    Ok(GeneratedCompaction {
        source,
        captured_candidate: candidate,
        summary,
        completed_coverage,
        after_projection,
        after,
        carryover_level,
    })
}

/// Commit a candidate already validated by the current installation owner.
/// The caller supplies current Head/owner/cancellation authority; generation
/// evidence by itself is never an installation grant.
pub(crate) fn commit_compaction(
    conversation: &mut ConversationState,
    store: &dyn ConversationStore,
    input: crate::durable::projection::ProjectionCommitInput,
    carryover_level: Option<crate::model::CarryoverDetailLevel>,
) -> Result<ExecutedCompaction, CompactionExecutionError> {
    let fingerprint = input.candidate.fingerprint();
    let owner_for_install = input.owner.clone();
    let operation_id = input.operation_id.clone();
    let receipt = match store.commit_projection(input) {
        Ok(receipt) => receipt,
        Err(error) => match store.projection_receipt(&operation_id, &fingerprint) {
            Ok(Some(mut receipt)) => {
                // This is first delivery after an uncertain native commit,
                // unlike an ordinary duplicate operation receipt lookup.
                receipt.newly_committed = true;
                receipt
            }
            Ok(None) => {
                return Err(match error {
                    crate::durable::ConversationStoreError::ProjectionHeadChanged => {
                        CompactionExecutionError::HeadChanged
                    }
                    error => CompactionExecutionError::Durable(error.to_string()),
                });
            }
            Err(lookup) => {
                return Err(CompactionExecutionError::Durable(format!(
                    "uncertain checkpoint commit: {error}; lookup: {lookup}"
                )));
            }
        },
    };
    let completed = ExecutedCompaction {
        newly_committed: receipt.newly_committed,
        persisted_event: receipt.completed,
        carryover_level,
    };
    let reload_current = || -> Result<ConversationState, String> {
        if store
            .projection_owner(owner_for_install.attempt_id.as_ref())
            .map_err(|e| e.to_string())?
            != owner_for_install
        {
            return Err("execution ownership changed before current-head installation".into());
        }
        let current = store.projected_history().map_err(|e| e.to_string())?;
        let messages: Vec<_> = current
            .messages
            .shared_iter()
            .filter(|m| m.as_canonical().is_some())
            .collect();
        let ids = messages
            .iter()
            .map(|m| m.canonical_id().expect("canonical Ledger body").clone())
            .collect();
        ConversationState::from_shared_durable_head(
            messages,
            ids,
            crate::conversation::SurfaceRevision::new(current.cut.ledger.0),
            current.cut.activation.0,
        )
        .map_err(|e| e.to_string())
    };
    // A transient hot read/install failure gets one authoritative reconciliation.
    // Both reads fence ownership and load the current head, never the old receipt.
    let installed = reload_current().or_else(|_| reload_current());
    match installed {
        Ok(current) => {
            *conversation = current;
            Ok(completed)
        }
        Err(error) => Err(CompactionExecutionError::Committed {
            completed: Box::new(completed),
            error,
        }),
    }
}

fn summary_input_limit(budgets: super::CompactionBudgets, output_budget: u32) -> u64 {
    budgets.summary_input_limit.saturating_sub(u64::from(
        output_budget.saturating_sub(budgets.summary_output_budget),
    ))
}

/// Shorter visible content must not reduce the unchanged Profile's reasoning
/// allowance. Every physical retry keeps the original operation lifetime and
/// reprices the complete request before reserving its next Call.
#[allow(clippy::too_many_arguments)] // Both stages retain the same frozen request and accounting context.
async fn reduce_summary(
    mut request: super::summarizer::SummaryRequest,
    operation: &dyn super::summarizer::SummaryOperation,
    engine: &super::ContextEngine,
    budgets: super::CompactionBudgets,
    output_budget: &mut u32,
    output_ceiling: u32,
    cut: &super::resources::ResourceReadCut,
    cancellation: &CancellationSignal,
    before_call: &(dyn Fn() -> Result<(), ContextError> + Send + Sync),
) -> Result<super::summarizer::SummaryGeneration, ContextError> {
    let budget = operation.budget();
    loop {
        budget.check_start(cancellation)?;
        request.output_tokens = Some(*output_budget);
        let limit = summary_input_limit(budgets, *output_budget);
        let estimate = operation.preview(&request)?.map_or_else(
            || engine.estimate_model_input(&request.model_input().messages.into(), "", &[]),
            |preview| engine.estimate_frozen_request(&preview),
        );
        if estimate > limit {
            return Err(ContextError::new(
                ContextErrorKind::CannotFit,
                format!(
                    "summary input capacity: Summary reduction request estimate={estimate}, summary input limit estimate={limit}; no source was discarded; estimates are not provider usage"
                ),
            ));
        }
        before_call()?;
        match operation
            .prepare()?
            .summarize(request.clone(), Some(cut.clone()))
            .await
        {
            Err(error) if error.kind == ContextErrorKind::SummaryFormatChanged => {}
            Err(error)
                if error.kind == ContextErrorKind::SummaryOutputExhausted
                    && *output_budget < output_ceiling =>
            {
                budget.charge_recovery(cancellation)?;
                *output_budget = output_budget.saturating_mul(2).min(output_ceiling);
            }
            Ok(summary) if summary.text.trim().is_empty() => {
                return Err(ContextError::new(
                    ContextErrorKind::SummaryFailed,
                    "Summary reduction returned empty content",
                ));
            }
            result => return result,
        }
    }
}

fn advance_carryover<'a>(
    constraints: &mut CompactionConstraints<'a>,
    next: &mut usize,
    budget: &super::compaction_budget::CompactionBudget,
    cancellation: &CancellationSignal,
) -> Result<Option<crate::model::CarryoverDetailLevel>, ContextError> {
    let alternatives: &'a [super::engine::CompactionCarryover] = constraints.carryover_fallbacks;
    let Some(alternative) = alternatives.get(*next) else {
        return Ok(None);
    };
    let valid_level = alternative.level != crate::model::CarryoverDetailLevel::Full
        && (*next == 0 || alternatives[*next - 1].level < alternative.level);
    let valid_value = match (constraints.carryover, &alternative.value, alternative.level) {
        (Some(_), None, crate::model::CarryoverDetailLevel::Omitted) => true,
        (Some(current), Some(value), level)
            if level != crate::model::CarryoverDetailLevel::Omitted =>
        {
            current.degraded(level) == *value
        }
        _ => false,
    };
    if !valid_level || !valid_value {
        return Err(ContextError::new(
            ContextErrorKind::InvalidConfiguration,
            "carryover alternatives must be frozen, ordered degradations of the admitted value",
        ));
    }
    budget.charge_recovery(cancellation)?;
    *next += 1;
    constraints.carryover = alternative.value.as_ref();
    if constraints.carryover.is_none() {
        constraints.carryover_anchor = None;
    }
    Ok(Some(alternative.level))
}

impl From<crate::context::checkpoint::CheckpointError> for CompactionExecutionError {
    fn from(error: crate::context::checkpoint::CheckpointError) -> Self {
        match error {
            crate::context::checkpoint::CheckpointError::Invalid(message) => {
                ContextError::new(ContextErrorKind::MalformedHistory, message).into()
            }
            crate::context::checkpoint::CheckpointError::Capacity(message) => {
                ContextError::new(ContextErrorKind::CannotFit, message).into()
            }
        }
    }
}

pub(crate) fn resource_failure(
    error: crate::durable::ConversationStoreError,
) -> CompactionExecutionError {
    match error {
        crate::durable::ConversationStoreError::ResourceBoundExceeded(message) => {
            ContextError::new(ContextErrorKind::CannotFit, message).into()
        }
        error => CompactionExecutionError::Durable(error.to_string()),
    }
}
