//! Finite immutable evidence prepared by the existing native write transaction.
//! A derived-read failure leaves the semantic commit intact and its receipt absent.
use super::{Arc, Connection, ConversationId};
use crate::durable::inbox::CanonicalObservationBatch;
use crate::durable::observation::{
    NativeObservationBatch, NativeObservationTransition, NativeToolResultCommit,
    ObservationEventWitness,
};
use crate::events::types::RuntimeEventEnvelope;

fn witness(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Option<ObservationEventWitness> {
    super::observation_event::witness(connection, event.sequence, event.sequence)
        .ok()
        .flatten()
}
fn batch(
    connection: &Connection,
    conversation: &ConversationId,
    range: Option<(u64, u64)>,
    canonical: Option<Arc<CanonicalObservationBatch>>,
    transition: NativeObservationTransition,
) -> Option<Arc<NativeObservationBatch>> {
    let summary = match &transition {
        NativeObservationTransition::Single(witness)
            if matches!(
                witness.event_kind.as_str(),
                "attempt_failed" | "attempt_completed"
            ) =>
        {
            super::observation_execution::summary_at_sequence(connection, witness.sequence)
                .ok()
                .flatten()
        }
        _ => None,
    };
    let finish_reason = match &transition {
        NativeObservationTransition::Single(witness)
            if witness.event_kind == "attempt_completed" && witness.deferred =>
        {
            witness
                .fields
                .iter()
                .find(|field| field.path == ["event", "finish_reason"])
                .map(
                    |field| crate::durable::observation::NativeObservationFieldRef {
                        sequence: witness.sequence,
                        path: field.path.clone(),
                        encoded_bytes: field.encoded_bytes,
                    },
                )
        }
        _ => None,
    };
    // Readiness is independently recoverable by its native epoch owner; both
    // indexed flags fence the initial receipt without conflating their repair.
    let source_unavailable = range.is_some_and(|(first,last)| connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_unavailable_idx WHERE sequence>=?1 AND sequence<=?2 AND observation_unavailable=1) OR EXISTS(SELECT 1 FROM events INDEXED BY events_observation_readiness_missing_idx WHERE sequence>=?1 AND sequence<=?2 AND observation_readiness_missing=1)",super::params![first,last],|row|row.get(0)).unwrap_or(true));
    let predecessor = range.map_or(super::journal_prefix(connection).ok()?, |(first, _)| {
        first.saturating_sub(1)
    });
    let read_cut = super::observation::cut(connection, conversation).ok()?;
    Some(Arc::new(NativeObservationBatch {
        ready_before: super::observation_readiness::frontier(connection, 0, predecessor).ok(),
        ready_t_through: super::observation_readiness::transcript_frontier(
            connection,
            0,
            read_cut.read.transcript,
        )
        .ok(),
        trace_total: super::observation_statistics::trace_total(
            connection,
            super::journal_prefix(connection).ok()?,
        )
        .ok(),
        pending: None,
        source_unavailable: source_unavailable
            || canonical
                .as_ref()
                .is_some_and(|canonical| canonical.source_unavailable),
        failure_deferred: summary
            .as_ref()
            .is_some_and(|summary| summary.failure_deferred),
        failure_readable_end: summary
            .as_ref()
            .map_or(0, |summary| summary.failure_readable_end),
        failure_sequence: summary
            .as_ref()
            .and_then(|summary| summary.terminal_sequence),
        finish_reason_deferred: summary
            .as_ref()
            .is_some_and(|summary| summary.finish_reason_deferred),
        failure: summary.and_then(|summary| summary.failure),
        finish_reason,
        read_cut,
        journal_range: range,
        canonical,
        transition,
    }))
}
pub(super) fn single(
    connection: &Connection,
    conversation: &ConversationId,
    event: &RuntimeEventEnvelope,
) -> Option<Arc<NativeObservationBatch>> {
    batch(
        connection,
        conversation,
        Some((event.sequence, event.sequence)),
        None,
        NativeObservationTransition::Single(witness(connection, event)?),
    )
}
pub(super) fn canonical(
    connection: &Connection,
    conversation: &ConversationId,
    event: &RuntimeEventEnvelope,
    canonical: Arc<CanonicalObservationBatch>,
) -> Option<Arc<NativeObservationBatch>> {
    let value = witness(connection, event)?;
    let transition = match event.event {
        crate::events::types::RuntimeEvent::CompactionCompleted { .. } => {
            NativeObservationTransition::Compaction(value)
        }
        crate::events::types::RuntimeEvent::InboundTurnAdopted { .. } => {
            NativeObservationTransition::Adoption(value)
        }
        _ => NativeObservationTransition::Single(value),
    };
    batch(
        connection,
        conversation,
        canonical.journal_range,
        Some(canonical),
        transition,
    )
}
pub(super) fn request_start(
    connection: &Connection,
    conversation: &ConversationId,
    started: &RuntimeEventEnvelope,
    emissions: &[RuntimeEventEnvelope],
    canonical: Option<Arc<CanonicalObservationBatch>>,
    snapshot: &crate::model::snapshot::RequestSnapshot,
    sources: &[crate::durable::inbox::StatusObservationSource],
) -> Option<Arc<NativeObservationBatch>> {
    let contributions = emissions
        .iter()
        .take(8)
        .map(|event| witness(connection, event))
        .collect::<Option<Vec<_>>>()?;
    batch(
        connection,
        conversation,
        Some((
            started.sequence,
            emissions
                .last()
                .map_or(started.sequence, |event| event.sequence),
        )),
        canonical,
        NativeObservationTransition::RequestStart {
            statuses:
                sources
                    .iter()
                    .filter_map(|source| {
                        let contribution = snapshot
                            .contributions
                            .get(usize::try_from(source.contribution_ordinal).ok()?)?;
                        let crate::context::contribution::ContributionPresentation::AgentStatus(
                            status,
                        ) = contribution.presentation.as_ref()?;
                        Some(crate::runtime_client::projection::prepared_status(
                            &snapshot.identity.attempt_id,
                            snapshot.identity.turn.as_str().parse().ok()?,
                            &contribution.message_id,
                            &contribution.opportunities,
                            contribution.post_tool_batch_anchor,
                            status,
                            Some(source),
                        ))
                    })
                    .collect(),
            started: witness(connection, started)?,
            contributions,
            contribution_total: emissions.len() as u64,
        },
    )
}
pub(super) fn positive_status(
    status: &crate::tools::types::ToolExecutionStatus,
) -> crate::tools::types::ToolExecutionStatus {
    use crate::tools::types::ToolExecutionStatus as Status;
    fn prefix(text: &str) -> String {
        let mut end = text.len().min(256);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].into()
    }
    match status {
        Status::Success => Status::Success,
        Status::TimedOut => Status::TimedOut,
        Status::Failed { error } => Status::Failed {
            error: prefix(error),
        },
        Status::Denied { reason } => Status::Denied {
            reason: prefix(reason),
        },
        Status::OutcomeUnknown { detail } => Status::OutcomeUnknown {
            detail: prefix(detail),
        },
        Status::Cancelled { reason, phase } => Status::Cancelled {
            reason: *reason,
            phase: *phase,
        },
    }
}
pub(super) fn tool_results(
    connection: &Connection,
    conversation: &ConversationId,
    events: &[RuntimeEventEnvelope],
    messages: &[crate::message::types::MessageBlock],
    canonical: Arc<CanonicalObservationBatch>,
) -> Option<Arc<NativeObservationBatch>> {
    use crate::runtime_client::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentRepresentation, ContentSource,
    };
    let last = events.last()?;
    let first = events.first()?;
    let latest_assistant = events
        .iter()
        .filter_map(|event| event.tool_occurrence.as_ref())
        .map(|occurrence| occurrence.ledger_position)
        .max();
    let mut loaded_results = Vec::with_capacity(32);
    for (event, message) in events.iter().zip(messages) {
        let Some(occurrence) = event.tool_occurrence.as_ref() else {
            continue;
        };
        if occurrence.tool_ordinal >= 32 || Some(occurrence.ledger_position) != latest_assistant {
            continue;
        }
        let crate::message::types::MessageBlock::Tool(tool) = message else {
            return None;
        };
        let result_ledger_position = super::content::canonical_position(
            connection,
            &CanonicalContentLocator::ByMessageId {
                message_id: tool.id.clone(),
            },
        )
        .ok()?;
        let source = ContentSource::CanonicalMessageBlock {
            locator: CanonicalContentLocator::ByLedgerPosition {
                position: result_ledger_position,
            },
            block_index: crate::message::types::ContentBlockIndex::new(0),
            component: CanonicalContentComponent::MessageMetadata,
        };
        let metadata = super::content::descriptor(
            connection,
            conversation,
            &source,
            ContentRepresentation::CanonicalJsonV1,
        )
        .ok()
        .flatten()?;
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT header_json FROM observation_headers WHERE ledger_position=?1",
                [result_ledger_position],
                |row| row.get(0),
            )
            .ok()?;
        let header: crate::durable::observation::MessageHeader =
            serde_json::from_slice(&bytes).ok()?;
        let result_header = super::observation_tools::result_header(&tool.result);
        loaded_results.push(NativeToolResultCommit {
            occurrence: occurrence.clone(),
            result_ledger_position,
            content_total: result_header.content_total,
            result_deferred: result_header.result_deferred,
            result: result_header.result,
            metadata,
            content: header.content,
        });
    }
    if loaded_results.len() > 32 {
        return None;
    }
    batch(
        connection,
        conversation,
        Some((first.sequence, last.sequence)),
        Some(canonical),
        NativeObservationTransition::ToolResults {
            loaded_results,
            tool_result_count: events.len() as u64,
        },
    )
}
pub(super) fn workflow_terminal(
    connection: &Connection,
    conversation: &ConversationId,
    terminal: &RuntimeEventEnvelope,
    output: &RuntimeEventEnvelope,
) -> Option<Arc<NativeObservationBatch>> {
    batch(
        connection,
        conversation,
        Some((terminal.sequence, output.sequence)),
        None,
        NativeObservationTransition::WorkflowTerminal {
            terminal: Box::new(witness(connection, terminal)?),
            output: Box::new(witness(connection, output)?),
        },
    )
}

pub(super) fn pending(
    connection: &Connection,
    conversation: &ConversationId,
    selected: Option<u64>,
    removed: Option<(u64, Option<crate::durable::TranscriptCursor>)>,
    adopted: Option<(u64, u64)>,
) -> Option<Arc<crate::durable::inbox::PendingObservationReceipt>> {
    let read_cut = super::reading::cut(connection, conversation).ok()?;
    let mut page =
        super::observation::pending_page(connection, conversation, &read_cut, None, 32).ok()?;
    for header in &mut page.headers {
        header.message.content.clear();
        header.message.collections.clear();
        header.message.locator = None;
    }
    let mut selected = selected.and_then(|sequence| {
        super::observation::pending_page(
            connection,
            conversation,
            &read_cut,
            Some(sequence.checked_sub(1)?),
            1,
        )
        .ok()?
        .headers
        .into_iter()
        .next()
        .filter(|header| header.sequence == sequence)
    });
    if let Some(header) = &mut selected {
        header.message.content.clear();
        header.message.collections.clear();
        header.message.locator = None;
    }
    Some(Arc::new(crate::durable::inbox::PendingObservationReceipt {
        ready_through: super::observation_readiness::frontier(connection, 0, read_cut.journal).ok(),
        ready_t_through: super::observation_readiness::transcript_frontier(
            connection,
            0,
            read_cut.transcript,
        )
        .ok(),
        total: read_cut.pending_total,
        pending_through: read_cut.pending_through,
        read_cut,
        head: page.headers,
        selected,
        removed_sequence: removed.map(|(sequence, _)| sequence),
        removed_transcript: removed.and_then(|(_, cursor)| cursor),
        adopted_through: adopted.map(|(through, _)| through),
        adopted_count: adopted.map(|(_, count)| count),
    }))
}

/// Read evidence belongs to the request transaction that actually committed its Context.
pub(super) fn status_sources(
    connection: &Connection,
    conversation: &ConversationId,
    snapshot: &crate::model::snapshot::RequestSnapshot,
) -> Vec<crate::durable::inbox::StatusObservationSource> {
    use crate::runtime_client::content::{
        CanonicalContentComponent as Component, CanonicalContentLocator as Locator,
        ContentRepresentation as Rep, ContentSource,
    };
    snapshot.contributions.iter().enumerate().filter_map(|(ordinal, contribution)| {
        if !snapshot.request_context_ids.contains(&contribution.message_id) { return None; }
        contribution.presentation.as_ref()?;
        let position: u64 = connection.query_row("SELECT position FROM message_ledger WHERE message_id=?1",[contribution.message_id.as_str()],|row|row.get(0)).ok()?;
        let locator = Locator::ByLedgerPosition {position};
        let reference = |locator: &Locator, component: Component, representation: Rep| super::content::descriptor(connection,conversation,&ContentSource::CanonicalMessageBlock {component,locator:locator.clone(),block_index:crate::message::types::ContentBlockIndex::new(0)},representation).ok().flatten();
        let target = contribution.opportunities.fresh_inbound.as_ref().and_then(|fresh| connection.query_row("SELECT m.position,t.position FROM message_ledger m LEFT JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=m.message_id WHERE m.message_id=?1",[fresh.target_message_id.as_str()],|row|Ok((row.get::<_,u64>(0)?,row.get::<_,Option<u64>>(1)?))).ok());
        let target_locator=target.map(|(position,_)|Locator::ByLedgerPosition {position});
        Some(crate::durable::inbox::StatusObservationSource {
            contribution_ordinal:ordinal as u64,
            request_id:snapshot.request_id.clone(),
            rendered_ref:reference(&locator,Component::Payload,Rep::Utf8Text),
            target_metadata_ref:target_locator.as_ref().and_then(|target|reference(target,Component::MessageMetadata,Rep::CanonicalJsonV1)),
            locator,
            target_locator,
            target_transcript:target.and_then(|(_,cursor)|cursor.map(crate::durable::TranscriptCursor::new)),
        })
    }).collect()
}
