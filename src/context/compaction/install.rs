//! Current-authority admission of an independently generated Summary.

use std::cmp::Ordering;

use super::{
    AttemptId, CancellationSignal, CheckpointCandidate, CheckpointItem, CheckpointMessage,
    CheckpointSourceRef, CompactionConstraints, CompactionExecutionError, ContextError,
    ContextErrorKind, ContextRuntime, ConversationState, ConversationStore, ExecutedCompaction,
    GeneratedCompaction, TurnId, Utc, commit_compaction, resource_failure,
};
use crate::context::resources::{CompactionResources, ResourceLimits};
use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

/// Inputs owned by the current safe-boundary or idle-maintenance caller.
pub(crate) struct CurrentInstallation<'a> {
    pub context: &'a ContextRuntime,
    /// The exact current request semantics, including fixed prompt and tools.
    pub request: &'a crate::model::ModelRequest,
    pub constraints: &'a CompactionConstraints<'a>,
    pub attempt_id: Option<&'a AttemptId>,
    pub turn_id: Option<TurnId>,
    /// Current Attempt cancellation is separate from the operation's gate.
    pub installer_cancellation: Option<&'a CancellationSignal>,
}

pub(crate) enum ActivationOutcome {
    Committed(Box<ExecutedCompaction>),
    Superseded,
    Incomparable,
    Inadmissible(ContextError),
}

pub(crate) enum ActivationPreview {
    Eligible(Box<PreparedInstallation>),
    Superseded,
    Incomparable,
    Inadmissible(ContextError),
}

pub(crate) struct PreparedInstallation {
    candidate: CheckpointCandidate,
    proof: super::super::coverage::VerifiedPrefixCoverage,
    after: u64,
    tokens_before: TokenMeasurement,
    occupancy: super::super::occupancy::ContextOccupancy,
}

/// Validate without a commit grant so a foreground owner can decide whether
/// a competing ready result is useful before cancelling its own generator.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn preview_generated(
    generated: &GeneratedCompaction,
    store: &dyn ConversationStore,
    current: &CurrentInstallation<'_>,
) -> Result<ActivationPreview, CompactionExecutionError> {
    let proof = super::super::coverage::VerifiedPrefixCoverage::seal(generated)?;
    let coverage = proof.record().clone();
    let (history, cut) = store.auxiliary_history().map_err(resource_failure)?;
    let head = store
        .projection_at_journal_cut(cut.journal.0)
        .map_err(resource_failure)?;
    if history.cut.ledger != cut.ledger
        || history.cut.activation != cut.projection_activation
        || head.as_ref().map(|value| &value.candidate.checkpoint_id)
            != history.cut.checkpoint_id.as_ref()
    {
        return Err(CompactionExecutionError::Durable(
            "current installation cut is inconsistent".into(),
        ));
    }
    let head_source = head.as_ref().map(|value| CheckpointSourceRef {
        conversation_id: cut.conversation_id.clone(),
        checkpoint_id: value.candidate.checkpoint_id.clone(),
        content_digest: value.fingerprint.clone(),
    });
    let head_view = head_source.as_ref().map(|source| {
        (
            source,
            head.as_ref()
                .and_then(|value| value.candidate.prefix_coverage.as_ref()),
        )
    });
    match coverage.compare_to_head(head_view) {
        Some(Ordering::Less | Ordering::Equal) => return Ok(ActivationPreview::Superseded),
        None => return Ok(ActivationPreview::Incomparable),
        Some(Ordering::Greater) => {}
    }
    let Some(retained_offset) = coverage.retained_offset(head_view) else {
        return Ok(ActivationPreview::Incomparable);
    };
    if retained_offset > history.items.len() {
        return Ok(ActivationPreview::Incomparable);
    }
    let checkpoint_id =
        crate::runtime::identity::CheckpointId::new(uuid::Uuid::now_v7().to_string());
    let mut items = vec![CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(generated.generated_summary().clone())),
        call_sources: Vec::new(),
        result_call: None,
    }];
    for mut item in history.items.iter().skip(retained_offset).cloned() {
        if let Some(call) = &mut item.result_call {
            let Some(index) = call
                .message
                .item_index
                .checked_sub(u32::try_from(retained_offset).map_err(|_| {
                    CompactionExecutionError::Durable("checkpoint occurrence overflow".into())
                })?)
                .and_then(|index| index.checked_add(1))
            else {
                return Ok(ActivationPreview::Incomparable);
            };
            call.message.checkpoint_id = checkpoint_id.clone();
            call.message.item_index = index;
        }
        items.push(item);
    }
    let candidate = CheckpointCandidate {
        checkpoint_id,
        expected_head: history.cut.checkpoint_id.clone(),
        read_cut: cut.clone(),
        base_frontier: history.cut.ledger,
        prefix_coverage: Some(coverage.clone()),
        items,
        summary_metadata: generated.captured_candidate.summary_metadata.clone(),
        summary_generation: generated.summary.generation.clone(),
    };
    let mut resources = CompactionResources::at_cut(store, cut, ResourceLimits::default())
        .map_err(resource_failure)?;
    let compiled =
        super::super::checkpoint::compile_checkpoint(&candidate, |source, body_required| {
            resources
                .dependency(source, body_required)
                .map_err(|error| error.to_string())
        })
        .map_err(|error| {
            CompactionExecutionError::Durable(format!(
                "current Summary candidate cannot compile: {error}"
            ))
        })?;
    let retained: crate::model::SelectedModelInput = compiled.messages.into();
    let count_original = |input: &crate::model::SelectedModelInput,
                          id: &crate::runtime::identity::MessageId| {
        input
            .iter()
            .filter(|message| message.canonical_id() == Some(id))
            .count()
    };
    if current.constraints.fresh_inbound.is_some_and(|fresh| {
        fresh
            .message_ids()
            .iter()
            .any(|id| count_original(&retained, id) != 1)
    }) || current
        .constraints
        .protected_context_ids
        .iter()
        .any(|id| count_original(&history.messages, id) > 0 && count_original(&retained, id) != 1)
    {
        return Ok(ActivationPreview::Inadmissible(ContextError::new(
            ContextErrorKind::CannotFit,
            "the generated Summary would retire protected current input",
        )));
    }
    if current.constraints.must_cover_through.is_some_and(|id| {
        count_original(&history.messages, id) != 1 || count_original(&retained, id) != 0
    }) {
        return Ok(ActivationPreview::Inadmissible(ContextError::new(
            ContextErrorKind::CannotFit,
            "the generated Summary does not cover the current continuation",
        )));
    }
    let engine = &current.context.engine;
    let historical_before = engine.estimate_model_input(&history.messages, "", &[]);
    let historical_after = engine.estimate_model_input(&retained, "", &[]);
    let assembled = super::super::engine::assemble_context_input(
        &retained,
        current.constraints.staged_request_context,
        current.constraints.carryover,
        current.constraints.carryover_anchor,
    )
    .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
    let mut request = current.request.clone();
    request.messages = engine
        .rendered_input(&assembled)
        .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
    request.continuation = None;
    request.continuation_source = None;
    let after = engine.estimate_frozen_request(&request);
    let mut limit =
        engine.admissible_input_limit(current.context.compaction_budgets.primary_output_budget)?;
    if let Some(correction) = current.constraints.estimate_correction {
        limit = correction.apply(limit);
    }
    if after > limit || historical_after >= historical_before {
        return Ok(ActivationPreview::Inadmissible(ContextError::new(
            if after > limit {
                ContextErrorKind::CannotFit
            } else {
                ContextErrorKind::NoProgress
            },
            format!(
                "the generated Summary does not fit the complete current request or reduce history (estimated input {after}, limit {limit})"
            ),
        )));
    }
    Ok(ActivationPreview::Eligible(Box::new(
        PreparedInstallation {
            candidate,
            proof,
            after,
            tokens_before: TokenMeasurement {
                input_tokens: engine.estimate_frozen_request(current.request),
                source: TokenMeasurementSource::Estimated,
            },
            occupancy: super::super::occupancy::estimate(
                after,
                engine.config().context_window_tokens,
                &current.context.primary_model,
                &request.effective_system_prompt,
                &request.tools,
            ),
        },
    )))
}

/// Revalidate under fresh authority and commit through the native atomic path.
pub(crate) fn activate_generated(
    generated: &GeneratedCompaction,
    conversation: &mut ConversationState,
    store: &dyn ConversationStore,
    operation_cancellation: &CancellationSignal,
    current: &CurrentInstallation<'_>,
) -> Result<ActivationOutcome, CompactionExecutionError> {
    let operation_id = generated.source.attribution.operation_id.clone();
    for _ in 0..2 {
        let prepared = match preview_generated(generated, store, current)? {
            ActivationPreview::Eligible(prepared) => *prepared,
            ActivationPreview::Superseded => return Ok(ActivationOutcome::Superseded),
            ActivationPreview::Incomparable => return Ok(ActivationOutcome::Incomparable),
            ActivationPreview::Inadmissible(error) => {
                return Ok(ActivationOutcome::Inadmissible(error));
            }
        };
        let owner = store
            .projection_owner(current.attempt_id)
            .map_err(resource_failure)?;
        let grant = match current.installer_cancellation {
            Some(installer) => installer
                .with_active(|| {
                    operation_cancellation.authorize_compaction_commit(operation_id.clone())
                })
                .flatten(),
            None => operation_cancellation.authorize_compaction_commit(operation_id.clone()),
        }
        .ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction installation cancelled",
            )
        })?;
        let input = crate::durable::projection::ProjectionCommitInput {
            grant,
            operation_id: operation_id.clone(),
            candidate: prepared.candidate,
            owner,
            prefix_proof: Some(prepared.proof),
            tokens_before: prepared.tokens_before,
            estimated_tokens_after: prepared.after,
            occupancy: Some(prepared.occupancy),
            turn_id: current.turn_id.clone(),
            timestamp: Utc::now(),
        };
        match commit_compaction(conversation, store, input, None) {
            Err(CompactionExecutionError::HeadChanged) => {}
            Ok(completed) => return Ok(ActivationOutcome::Committed(Box::new(completed))),
            Err(error) => return Err(error),
        }
    }
    Err(CompactionExecutionError::HeadChanged)
}
