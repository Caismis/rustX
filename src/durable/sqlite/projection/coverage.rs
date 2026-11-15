//! Authentication of captured Summary coverage and current retained suffix.

#[cfg(test)]
mod tests;

use super::{
    CheckpointCandidate, CheckpointMessage, Connection, ConversationId, ConversationStoreError,
    ProjectionActivationId, ProjectionCheckpoint, ProjectionCommitInput, ResourceLimits,
    at_journal_cut, auxiliary_lineage, compile_history, invalid,
};
#[cfg(test)]
use super::{
    CheckpointId, CompactionOperationId, JournalFrontier, OriginActivation, ProjectionActivation,
};
use crate::context::checkpoint::CheckpointItem;
use crate::context::coverage::PrefixCoverage;
use crate::durable::projection::HistoricalModelInput;
use crate::model::projected::{CheckpointSourceRef, ProjectedMessage, SemanticTransformation};

fn source(scope: &ConversationId, checkpoint: &ProjectionCheckpoint) -> CheckpointSourceRef {
    CheckpointSourceRef {
        conversation_id: scope.clone(),
        checkpoint_id: checkpoint.candidate.checkpoint_id.clone(),
        content_digest: checkpoint.fingerprint.clone(),
    }
}

fn is_summary(item: &CheckpointItem) -> bool {
    matches!(&item.message, CheckpointMessage::Derived(derived)
        if derived.transformations.contains(&SemanticTransformation::Summary))
}

fn validate_native_shape(checkpoint: &ProjectionCheckpoint) -> Result<(), ConversationStoreError> {
    let items = &checkpoint.candidate.items;
    if checkpoint.candidate.prefix_coverage.is_some()
        && (items.first().is_none_or(|item| !is_summary(item))
            || items.iter().skip(1).any(is_summary))
    {
        return Err(invalid(
            "native coverage checkpoint has invalid Summary shape",
        ));
    }
    Ok(())
}

fn complete_boundary(
    history: &HistoricalModelInput,
    count: usize,
) -> Result<(), ConversationStoreError> {
    if count == 0
        || count > history.items.len()
        || history.tool_associations.iter().any(|association| {
            ((association.call_message as usize) < count)
                != ((association.result_message as usize) < count)
        })
    {
        return Err(invalid(
            "coverage prefix does not end at a complete source boundary",
        ));
    }
    Ok(())
}

/// Authenticate a proposed coverage coordinate against the old capture, not
/// the current Head. The bounded selected projection is loaded at that cut.
fn captured_history(
    connection: &Connection,
    scope: &ConversationId,
    record: &PrefixCoverage,
) -> Result<HistoricalModelInput, ConversationStoreError> {
    let cut = &record.capture_cut;
    super::super::compaction_resources::validate(
        connection,
        scope,
        cut,
        ResourceLimits::default(),
    )?;
    let previous = at_journal_cut(connection, cut.journal.0)?;
    if previous
        .as_ref()
        .map_or(ProjectionActivationId(0), |checkpoint| {
            checkpoint.activation.activation
        })
        != cut.projection_activation
    {
        return Err(invalid(
            "coverage capture disagrees with its historical Projection Head",
        ));
    }
    if let Some(previous) = &previous {
        validate_native_shape(previous)?;
    }
    let history = compile_history(connection, scope, cut, previous.as_ref())?;
    let count = record.captured_prefix_items as usize;
    complete_boundary(&history, count)?;
    let digest = crate::context::coverage::capture_digest(cut, &history.messages, count)
        .map_err(|error| ConversationStoreError::ResourceBoundExceeded(error.message))?;
    if digest != record.capture_digest {
        return Err(invalid(
            "coverage capture does not match its immutable historical prefix",
        ));
    }
    let predecessor_source = previous
        .as_ref()
        .map(|checkpoint| source(scope, checkpoint));
    let predecessor = predecessor_source.as_ref().map(|identity| {
        (
            identity,
            previous
                .as_ref()
                .and_then(|checkpoint| checkpoint.candidate.prefix_coverage.as_ref()),
        )
    });
    let expected = PrefixCoverage::from_capture(
        cut.clone(),
        record.captured_prefix_items,
        digest,
        record.rendered_bytes,
        record.rendered_digest.clone(),
        predecessor,
    )
    .map_err(invalid)?;
    if &expected != record {
        return Err(invalid(
            "coverage root or frontier disagrees with its captured predecessor",
        ));
    }
    Ok(history)
}

/// Rebuild the current retained suffix exactly, adjusting only occurrence
/// coordinates that are local to the newly constructed checkpoint.
fn validate_current_suffix(
    connection: &Connection,
    scope: &ConversationId,
    candidate: &CheckpointCandidate,
    record: &PrefixCoverage,
    current: Option<&ProjectionCheckpoint>,
) -> Result<(), ConversationStoreError> {
    if candidate.base_frontier != candidate.read_cut.ledger {
        return Err(invalid(
            "native coverage must retain the complete installation cut",
        ));
    }
    if let Some(checkpoint) = current {
        validate_native_shape(checkpoint)?;
    }
    let identity = current.map(|checkpoint| source(scope, checkpoint));
    let head = identity.as_ref().map(|source| {
        (
            source,
            current.and_then(|checkpoint| checkpoint.candidate.prefix_coverage.as_ref()),
        )
    });
    let offset = record.retained_offset(head).ok_or_else(|| {
        invalid("native coverage is not a strict compatible advance of current Head")
    })?;
    let current_history = compile_history(connection, scope, &candidate.read_cut, current)?;
    complete_boundary(&current_history, offset)?;
    if candidate.items.len() != 1 + current_history.items.len() - offset {
        return Err(invalid(
            "native coverage candidate omits or duplicates retained history",
        ));
    }
    for (actual, existing) in candidate
        .items
        .iter()
        .skip(1)
        .zip(current_history.items.iter().skip(offset))
    {
        let mut expected = existing.clone();
        if let Some(call) = &mut expected.result_call {
            call.message.checkpoint_id = candidate.checkpoint_id.clone();
            call.message.item_index = call
                .message
                .item_index
                .checked_sub(
                    u32::try_from(offset)
                        .map_err(|_| invalid("coverage suffix offset overflow"))?,
                )
                .and_then(|index| index.checked_add(1))
                .ok_or_else(|| invalid("retained Tool result crosses coverage boundary"))?;
        }
        if actual != &expected || is_summary(actual) {
            return Err(invalid(
                "native coverage retained suffix differs from current history",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_commit(
    connection: &Connection,
    scope: &ConversationId,
    input: &ProjectionCommitInput,
    current: Option<&ProjectionCheckpoint>,
) -> Result<(), ConversationStoreError> {
    let candidate = &input.candidate;
    let (record, proof) = match (&candidate.prefix_coverage, &input.prefix_proof) {
        (None, None) => return Ok(()),
        (Some(record), Some(proof)) => (record, proof),
        _ => {
            return Err(invalid(
                "native coverage requires its sealed completed-generation proof",
            ));
        }
    };
    let Some(CheckpointItem {
        message: CheckpointMessage::Derived(summary),
        call_sources,
        result_call,
    }) = candidate.items.first()
    else {
        return Err(invalid(
            "native coverage candidate must begin with one Derived Summary",
        ));
    };
    if !call_sources.is_empty()
        || result_call.is_some()
        || !matches!(
            &summary.message,
            ProjectedMessage::User {
                source: crate::message::types::UserSource::Runtime,
                kind: crate::model::projected::ProjectedUserKind::CompactionContext,
                ..
            }
        )
        || summary.transformations != [SemanticTransformation::Summary]
    {
        return Err(invalid("native coverage Summary has invalid shape"));
    }
    let generation = candidate.summary_generation.as_ref();
    if !proof.authorizes(record, summary, generation) {
        return Err(invalid(
            "native coverage proof does not bind the candidate Summary and generation",
        ));
    }
    let cut = &record.capture_cut;
    if cut.ledger > candidate.read_cut.ledger
        || cut.journal > candidate.read_cut.journal
        || cut.transcript > candidate.read_cut.transcript
        || cut.projection_activation > candidate.read_cut.projection_activation
    {
        return Err(invalid(
            "native coverage capture is after its installation cut",
        ));
    }
    if let Some(generation) = generation {
        let mut records = ResourceLimits::default().total_items;
        let mut bytes = ResourceLimits::default().total_bytes;
        let call = auxiliary_lineage::progress_call(
            connection,
            scope,
            &input.operation_id,
            generation,
            &mut records,
            &mut bytes,
        )?;
        if !matches!(&call.input, crate::model::auxiliary::AuxiliaryInputEvidence::Compaction { cut: source, .. } if source == cut)
        {
            return Err(invalid(
                "Summary generation does not use the exact coverage capture cut",
            ));
        }
    }
    captured_history(connection, scope, record)?;
    validate_current_suffix(connection, scope, candidate, record, current)
}
