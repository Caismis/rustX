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

/// The floor a shrinking summary input budget never crosses. Below it a
/// summary request could no longer carry a useful span, and the honest
/// outcome is an explicit compaction failure.
const MIN_SUMMARY_INPUT_LIMIT: u64 = 4_096;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::tokens::ProviderObservedInput;
use crate::conversation::ConversationState;
use crate::durable::ConversationStore;
use crate::events::types::RuntimeEventEnvelope;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AttemptId, ConversationId, TurnId};
use crate::tools::types::ModelToolDefinition;

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
        attribution,
        operation.as_ref(),
    )
    .await;
    operation.close().await;
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
) -> Result<ExecutedCompaction, CompactionExecutionError> {
    let budget = operation.budget();
    if budget.operation_id() != &attribution.operation_id {
        return Err(ContextError::new(
            ContextErrorKind::AuxiliaryNotStarted,
            "Summary operation belongs to another Compaction",
        )
        .into());
    }
    budget.check(cancellation)?;
    let mut prepared = Some(operation.prepare()?);
    let owner = store
        .projection_owner(attribution.attempt_id.as_ref())
        .map_err(resource_failure)?;
    let history = store.projected_history().map_err(resource_failure)?;
    let previous = store.projection_head().map_err(resource_failure)?;
    if previous.as_ref().map(|p| &p.candidate.checkpoint_id) != history.cut.checkpoint_id.as_ref() {
        return Err(CompactionExecutionError::Durable(
            "projection changed during compaction capture".into(),
        ));
    }
    let mut resources = crate::context::resources::CompactionResources::capture(
        store,
        crate::context::resources::ResourceLimits::default(),
    )
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
        conversation.revision(),
        history.messages.clone(),
        tools,
        observed,
        effective_system_prompt,
    );
    let mut budgets = context.compaction_budgets;
    budgets.summary_input_limit = budget.summary_input_limit(budgets.summary_input_limit);
    let (retired_count, summary) = loop {
        budget.check(cancellation)?;
        let n = super::planning::summary_prefix(
            &engine,
            &projection.messages,
            effective_system_prompt,
            tools,
            budgets,
            constraints,
            cancellation,
        )
        .await?;
        // Inline provenance is a finite capability. Admit it before invoking
        // the provider rather than generating a candidate we cannot commit.
        let mut refs = std::collections::BTreeSet::new();
        let mut retained_refs = 0usize;
        for (index, item) in history.items.iter().enumerate() {
            if index < n {
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
                ContextErrorKind::Internal,
                "compaction provenance exceeds inline budget before summary invocation",
            )
            .into());
        }
        let request = crate::context::summarizer::SummaryRequest {
            retired: projection.messages.range(0..n).to_vec(),
        };
        let result = prepared
            .take()
            .expect("one reservation per Summary input")
            .summarize(request, Some(resources.cut().clone()))
            .await;
        match result {
            Ok(summary) if !summary.text.trim().is_empty() => break (n, summary),
            Ok(_) => {
                return Err(ContextError::new(
                    ContextErrorKind::SummaryFailed,
                    "summary generation produced no content",
                )
                .into());
            }
            Err(error)
                if error.kind == ContextErrorKind::SummaryInputTooLarge
                    && budgets.summary_input_limit > MIN_SUMMARY_INPUT_LIMIT =>
            {
                budgets.summary_input_limit = budget.shrink_summary_input_limit(
                    budgets.summary_input_limit,
                    MIN_SUMMARY_INPUT_LIMIT,
                    cancellation,
                )?;
                prepared = Some(operation.prepare()?);
            }
            Err(error) => return Err(error.into()),
        }
    };
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
    if let Some(previous) = &previous
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
        sources: sources.into_values().collect(),
        checkpoint_sources: checkpoint_sources.into_values().collect(),
        transformations: vec![SemanticTransformation::Summary],
    };
    let checkpoint_id =
        crate::runtime::identity::CheckpointId::new(uuid::Uuid::now_v7().to_string());
    let mut items = vec![CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(derived)),
        call_sources: Vec::new(),
        result_call: None,
    }];
    for mut item in history.items.into_iter().skip(retired_count) {
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
                    CompactionExecutionError::Durable("retained result crosses retired call".into())
                })?;
        }
        items.push(item);
    }
    let candidate = CheckpointCandidate {
        checkpoint_id,
        expected_head: history.cut.checkpoint_id,
        read_cut: resources.cut().clone(),
        base_frontier: history.cut.ledger,
        items,
        summary_metadata: Some(metadata),
        summary_generation: summary.generation,
    };
    let compiled =
        crate::context::checkpoint::compile_checkpoint(&candidate, |source, body_required| {
            resources
                .dependency(source, body_required)
                .map_err(|e| e.to_string())
        })
        .map_err(|e| {
            CompactionExecutionError::Context(ContextError::new(
                ContextErrorKind::MalformedHistory,
                e,
            ))
        })?;
    let after_projection = engine.build_projection_input(
        conversation.revision(),
        compiled.messages.into(),
        tools,
        None,
        effective_system_prompt,
    );
    let after = engine.estimate_with_staged_context_and_carryover(
        &after_projection,
        constraints.staged_request_context,
        tools,
        constraints.carryover,
        constraints.carryover_anchor,
    );
    let mut limit = engine.soft_input_limit(budgets.primary_output_budget)?;
    if let Some(correction) = constraints.estimate_correction {
        limit = correction.apply(limit);
    }
    if after >= limit {
        return Err(ContextError::new(
            ContextErrorKind::CannotFit,
            "the compacted projection exceeds the soft input limit",
        )
        .into());
    }
    // Progress belongs to historical selection alone. Core-owned staged Context,
    // carryover, prompt/tool overhead and provider observations cannot certify a
    // growing historical projection as a successful compaction.
    let history_before = engine.estimate_model_input(&projection.messages, "", &[]);
    let history_after = engine.estimate_model_input(&after_projection.messages, "", &[]);
    if history_after >= history_before {
        return Err(ContextError::new(
            ContextErrorKind::NoProgress,
            "summary does not decrease historical input",
        )
        .into());
    }
    budget.check(cancellation)?;
    let grant = operation.authorize_commit()?;
    let fingerprint = candidate.fingerprint();
    let owner_for_install = owner.clone();
    let input = crate::durable::projection::ProjectionCommitInput {
        grant,
        operation_id: attribution.operation_id.clone(),
        candidate,
        owner,
        tokens_before: projection.estimated_input,
        estimated_tokens_after: after_projection.estimated_input.input_tokens,
        occupancy: Some(crate::context::occupancy::estimate(
            after,
            engine.config().context_window_tokens,
            &context.primary_model,
            effective_system_prompt,
            tools,
        )),
        turn_id: attribution.turn_id,
        timestamp: Utc::now(),
    };
    let receipt = match store.commit_projection(input) {
        Ok(receipt) => receipt,
        Err(error) => match store.projection_receipt(&attribution.operation_id, &fingerprint) {
            Ok(Some(mut receipt)) => {
                // This is first delivery after an uncertain native commit,
                // unlike an ordinary duplicate operation receipt lookup.
                receipt.newly_committed = true;
                receipt
            }
            Ok(None) => return Err(CompactionExecutionError::Durable(error.to_string())),
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
    };
    let reload_current = || -> Result<ConversationState, String> {
        if store
            .projection_owner(attribution.attempt_id.as_ref())
            .map_err(|e| e.to_string())?
            != owner_for_install
        {
            return Err("execution ownership changed before current-head installation".into());
        }
        let current = store.projected_history().map_err(|e| e.to_string())?;
        let messages: Vec<_> = current
            .messages
            .into_iter()
            .filter_map(|m| m.as_canonical().cloned())
            .collect();
        let ids = messages.iter().map(|m| m.id().clone()).collect();
        ConversationState::from_durable_head(
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

fn resource_failure(error: crate::durable::ConversationStoreError) -> CompactionExecutionError {
    match error {
        crate::durable::ConversationStoreError::ResourceBoundExceeded(message) => {
            ContextError::new(ContextErrorKind::CannotFit, message).into()
        }
        error => CompactionExecutionError::Durable(error.to_string()),
    }
}
