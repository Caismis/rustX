//! Client-neutral completed-response projections over native durable evidence.
//!
//! No state is persisted here. Acceptance identifies content; Attempt completion
//! identifies its closing response. Provider terminals alone never close a tail.
#[cfg(test)]
use crate::model::types::ModelUsage;
use crate::observation::statistics::ConversationStatistics;
#[cfg(test)]
use crate::observation::turn::AttemptIdentitySource;
use crate::observation::turn::{CompletedResponseView, ObservedResponseOrigin, TurnProcessView};
#[cfg(test)]
use crate::runtime::identity::{AttemptId, MessageId};
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEvent;
#[cfg(test)]
use crate::message::types::AssistantContentBlock;
use crate::message::types::MessageBlock;
use crate::model::finish::ModelFinishReason;

use crate::observation::transcript::{RuntimeClientTranscriptItem, RuntimeClientTranscriptPage};

#[cfg(test)]
fn inline_attempt(attempt: &AttemptId) -> Option<AttemptId> {
    (attempt.as_str().len() <= 512).then(|| attempt.clone())
}
#[cfg(test)]
fn attempt_source(
    store: &dyn ConversationStore,
    attempt: &AttemptId,
    through: u64,
) -> Result<Option<AttemptIdentitySource>, ConversationStoreError> {
    if inline_attempt(attempt).is_some() {
        return Ok(None);
    }
    store
        .observation_attempt_members(attempt, through)?
        .process
        .and_then(|owner| owner.attempt_id_source)
        .map(Some)
        .ok_or_else(|| {
            ConversationStoreError::Storage("native owner field source unavailable".into())
        })
}

#[cfg(test)]
fn response_locator(
    store: &dyn ConversationStore,
    id: &MessageId,
) -> Result<crate::observation::content::CanonicalContentLocator, ConversationStoreError> {
    store
        .message_header(id)?
        .and_then(|header| header.locator)
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "response canonical locator unavailable".into(),
            )
        })
}
#[cfg(test)]
fn inline_response_id(id: &MessageId) -> Option<MessageId> {
    (id.as_str().len() <= 256).then(|| id.clone())
}

pub use crate::durable::reading::TurnProcessOutcome;

pub(crate) fn terminal_turn(
    event: crate::events::types::RuntimeEventEnvelope,
    cursor: crate::observation::transcript::RuntimeClientTranscriptCursor,
) -> Result<TurnProcessView, String> {
    let failure = match &event.event {
        RuntimeEvent::AttemptFailed { error, .. } => Some(super::projection::client_failure(error)),
        _ => None,
    };
    let outcome = match event.event {
        RuntimeEvent::AttemptCancelled { .. } => TurnProcessOutcome::Cancelled,
        RuntimeEvent::AttemptFailed { .. } => TurnProcessOutcome::Failed,
        RuntimeEvent::AttemptTimedOut { .. } => TurnProcessOutcome::TimedOut,
        RuntimeEvent::AttemptLimitExceeded { .. } => TurnProcessOutcome::LimitExceeded,
        _ => return Err("invalid terminal Attempt reference".into()),
    };
    Ok(TurnProcessView {
        conversation_id: event.conversation_id,
        attempt_id: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 512)
            .cloned(),
        attempt_id_source: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() > 512)
            .map(|id| crate::durable::sqlite::journal_attempt_source(event.sequence, id))
            .transpose()
            .map_err(|_| "terminal owner field unavailable")?,
        event_id: Some(event.event_id),
        control_cursor: cursor,
        final_message_id: None,
        final_message_locator: None,
        final_message_id_deferred: false,
        final_message_id_source: None,
        message_count: 0,
        tool_call_count: 0,
        outcome,
        failure,
        started_at: None,
        ended_at: Some(event.timestamp),
    })
}

/// Independent raw native-page oracle for fixture comparisons.
#[cfg(test)]
pub(crate) fn native_oracle_decorate(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
) -> Result<(), ConversationStoreError> {
    native_oracle_decorate_through(store, page, store.presentation_frontier()?)
}

/// Independent raw native-page oracle at an explicitly selected Journal prefix.
#[cfg(test)]
pub(crate) fn native_oracle_decorate_through(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    decorate_projection(store, page, through, None)
}

pub(crate) fn decorate_through(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    validate_decoration_cut(page, through)?;
    decorate_numeric(store, page, through)
}

/// Read whole-conversation usage independently of runtime/resource initialization.
pub(crate) fn statistics(
    store: &dyn ConversationStore,
    through: u64,
) -> Result<ConversationStatistics, ConversationStoreError> {
    store.observation_statistics(through)
}

pub(crate) fn decorate_window(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    validate_decoration_cut(page, through)?;
    decorate_numeric(store, page, through)?;
    // Whole-conversation totals have a separate native live projection. A
    // historical window never scans all unrelated Attempts to rebuild it.
    page.statistics = None;
    Ok(())
}

#[cfg(test)]
fn clear_response_readings(page: &mut RuntimeClientTranscriptPage) {
    for entry in &mut page.entries {
        entry.completed_response = None;
        entry.turn_process = None;
        entry.response_pending = false;
    }
}

#[cfg(test)]
fn decorate_projection(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
    scopes: Option<&BTreeSet<AttemptId>>,
) -> Result<(), ConversationStoreError> {
    if page.source_cut.is_some() {
        validate_decoration_cut(page, through)?;
        return decorate_numeric(store, page, through);
    }
    clear_response_readings(page);
    let wanted: BTreeSet<_> = page
        .entries
        .iter()
        .filter_map(|entry| match &entry.item {
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Assistant(message),
            } => Some(message.id.clone()),
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Tool(message),
            } => Some(message.occurrence.assistant_message_id.clone()),
            _ => None,
        })
        .collect();
    let terminal_attempts = page
        .entries
        .iter()
        .filter_map(|entry| match &entry.item {
            RuntimeClientTranscriptItem::AttemptTerminal { turn } => turn.attempt_id.clone(),
            _ => None,
        })
        .collect();
    let ResponseProjection {
        mut completed,
        processes,
        pending,
        statistics,
        terminals,
        models,
    } = project(store, &wanted, &terminal_attempts, through, scopes)?;
    for entry in &mut page.entries {
        if let RuntimeClientTranscriptItem::AttemptTerminal { turn } = &mut entry.item {
            if turn.conversation_id == *store.conversation_id()
                && let Some(owner) = turn
                    .attempt_id
                    .as_ref()
                    .and_then(|attempt| terminals.get(attempt))
            {
                *turn = owner.clone();
            }
            entry.turn_process = Some(turn.clone());
        }
        let owner = match &entry.item {
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Assistant(message),
            } => Some(&message.id),
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Tool(message),
            } => Some(&message.occurrence.assistant_message_id),
            _ => None,
        };
        if let Some(owner) = owner {
            entry.turn_process = processes.get(owner).cloned();
        }
        entry.response_pending = owner.is_some_and(|id| pending.contains(id));
        if let Some(frontier) = failure_frontier(store, entry, through)? {
            page.collections.push(frontier);
        }
        let RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Assistant(message),
        } = &entry.item
        else {
            continue;
        };
        entry.response_pending = pending.contains(&message.id);
        if message
            .content
            .iter()
            .any(|block| matches!(block, AssistantContentBlock::ToolCall(_)))
        {
            continue;
        }
        if let Some(mut response) = completed.remove(&message.id) {
            if let Some(locator) = &response.retry_locator
                && !store.ordinary_user_message(locator)?
            {
                response.retry_message_id = None;
                response.retry_locator = None;
            }
            page.collections.extend(response_frontiers(
                store,
                page.source_cut.as_ref(),
                entry.cursor.get(),
                through,
                &response,
                models.get(&message.id),
            )?);
            entry.completed_response = Some(response);
        }
    }
    page.statistics = Some(statistics);
    decorate_deferred_responses(store, page, through)?;
    Ok(())
}

fn decorate_numeric(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let cut = page.source_cut.clone().ok_or_else(|| {
        ConversationStoreError::InvalidReference("native decoration cut absent".into())
    })?;
    if cut.read.journal != through {
        return Err(ConversationStoreError::InvalidReference(
            "response decoration differs from native window cut".into(),
        ));
    }
    for entry in &mut page.entries {
        let process = store.observation_transcript_process(&cut, entry.cursor.get())?;
        if let RuntimeClientTranscriptItem::AttemptTerminal { turn } = &mut entry.item
            && let Some(owner) = &process
        {
            *turn = owner.clone();
        }
        entry.response_pending = process
            .as_ref()
            .is_some_and(|owner| owner.outcome == TurnProcessOutcome::Running);
        if let Some(crate::durable::observation::ResponseIdentitySource::Attempt { sequence }) =
            store.response_identity_source(&cut, entry.cursor.get())?
        {
            let summary = store.observation_execution_at(&cut, sequence)?;
            if summary.failure_deferred {
                if summary.failure_readable_end == 0 {
                    return Err(ConversationStoreError::Storage(
                        "native failure layout unavailable".into(),
                    ));
                }
                page.collections
                    .push(crate::observation::cut::CollectionFrontier {
                        domain: format!(
                            "transcript.entries.{}.turn_process.failure",
                            entry.cursor.get()
                        ),
                        revision: through,
                        total: 1,
                        loaded: 0,
                        next_offset: Some(0),
                    });
            }
        }
        entry.turn_process = process;
        entry.completed_response = None;
        let RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Assistant(message),
        } = &entry.item
        else {
            continue;
        };
        if message.content.iter().any(|block| {
            matches!(
                block,
                crate::message::types::AssistantContentBlock::ToolCall(_)
            )
        }) {
            continue;
        }
        if let Some(source) = store.response_identity_source(&cut, entry.cursor.get())?
            && let Some(response) = observed_response(store, &cut, &source)?
        {
            let position = store.message_ledger_position(&message.id)?.ok_or_else(|| {
                ConversationStoreError::Storage("native response locator unavailable".into())
            })?;
            if response.closing_locator
                == (crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                    position,
                })
            {
                page.collections.push(deferred_models_frontier(
                    store,
                    &cut,
                    &source,
                    entry.cursor.get(),
                )?);
                page.collections.extend(response_frontiers(
                    store,
                    Some(&cut),
                    entry.cursor.get(),
                    through,
                    &response,
                    None,
                )?);
                entry.completed_response = Some(response);
            }
        }
    }
    page.statistics = Some(statistics(store, through)?);
    decorate_deferred_responses(store, page, through)
}

#[cfg(test)]
fn failure_frontier(
    store: &dyn ConversationStore,
    entry: &crate::observation::transcript::RuntimeClientTranscriptEntry,
    through: u64,
) -> Result<Option<crate::observation::cut::CollectionFrontier>, ConversationStoreError> {
    let Some(turn) = &entry.turn_process else {
        return Ok(None);
    };
    if turn.conversation_id != *store.conversation_id()
        || turn.outcome != TurnProcessOutcome::Failed
    {
        return Ok(None);
    }
    let Some(attempt) = turn.attempt_id.as_ref() else {
        return Ok(None);
    };
    let summary = store.observation_attempt_summary(attempt, through)?;
    Ok(summary
        .failure_deferred
        .then(|| crate::observation::cut::CollectionFrontier {
            domain: format!(
                "transcript.entries.{}.turn_process.failure",
                entry.cursor.get()
            ),
            revision: through,
            total: 1,
            loaded: 0,
            next_offset: Some(0),
        }))
}

fn response_frontiers(
    store: &dyn ConversationStore,
    cut: Option<&crate::durable::observation::ObservationReadCut>,
    cursor: u64,
    through: u64,
    response: &CompletedResponseView,
    model_counts: Option<&(u64, u64, Option<u64>)>,
) -> Result<Vec<crate::observation::cut::CollectionFrontier>, ConversationStoreError> {
    let mut frontiers = Vec::new();
    if let Some((total, loaded, next_offset)) = model_counts {
        frontiers.push(crate::observation::cut::CollectionFrontier {
            domain: format!("transcript.entries.{cursor}.completed_response.models"),
            revision: through,
            total: *total,
            loaded: *loaded,
            next_offset: *next_offset,
        });
    }
    if !response.deferred_fields.is_empty()
        && let Some(cut) = cut
        && let Some(source) = store.response_identity_source(cut, cursor)?
    {
        frontiers.push(crate::observation::cut::CollectionFrontier {
            domain: format!("transcript.entries.{cursor}.completed_response.identities"),
            revision: through,
            total: store.response_identity_fields(cut, &source)?.len() as u64,
            loaded: 0,
            next_offset: Some(0),
        });
    }
    Ok(frontiers)
}

#[cfg(test)]
struct ResponseProjection {
    terminals: BTreeMap<AttemptId, TurnProcessView>,
    completed: BTreeMap<MessageId, CompletedResponseView>,
    processes: BTreeMap<MessageId, TurnProcessView>,
    pending: BTreeSet<MessageId>,
    statistics: ConversationStatistics,
    models: BTreeMap<MessageId, (u64, u64, Option<u64>)>,
}

/// Resolves only the bounded page's native owners from indexed commit-owned summaries.
#[cfg(test)]
fn project(
    store: &dyn ConversationStore,
    wanted: &BTreeSet<MessageId>,
    terminal_attempts: &BTreeSet<AttemptId>,
    through: u64,
    selected: Option<&BTreeSet<AttemptId>>,
) -> Result<ResponseProjection, ConversationStoreError> {
    let mut projection = ResponseProjection {
        terminals: BTreeMap::new(),
        completed: BTreeMap::new(),
        processes: BTreeMap::new(),
        pending: BTreeSet::new(),
        statistics: store.observation_statistics(through)?,
        models: BTreeMap::new(),
    };
    inherited_projection(store, wanted, through, &mut projection)?;
    let mut owners: BTreeMap<AttemptId, BTreeSet<MessageId>> = BTreeMap::new();
    for message in wanted {
        for attempt in store.transcript_attempts(std::slice::from_ref(message), through)? {
            if selected.is_none_or(|selected| selected.contains(&attempt)) {
                owners.entry(attempt).or_default().insert(message.clone());
            }
        }
    }
    for attempt in terminal_attempts {
        owners.entry(attempt.clone()).or_default();
    }
    for (attempt, members) in owners {
        apply_attempt_summary(store, &attempt, &members, through, &mut projection)?;
    }
    Ok(projection)
}

fn inherited_response_view(
    store: &dyn ConversationStore,
    response: crate::durable::observation::InheritedResponseSummary,
) -> Result<CompletedResponseView, ConversationStoreError> {
    let closing_locator = crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
        position: response.closing_ledger_position,
    };
    let surface_revision = store
        .message_append_revision_by_locator(&closing_locator)?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "inherited closing Surface append unavailable".into(),
            )
        })?;
    let mut deferred_fields = Vec::new();
    if response.closing_message_id.is_none() {
        deferred_fields.push(vec!["closing_message_id".into()]);
    }
    if response.origin.closing_message_id.is_none() {
        deferred_fields.push(vec!["origin".into(), "closing_message_id".into()]);
    }
    if response.retry_message_id.is_none() && response.retry_ledger_position.is_some() {
        deferred_fields.push(vec!["retry_message_id".into()]);
    }
    Ok(CompletedResponseView {
        closing_locator,
        surface_revision,
        retry_locator: response.retry_ledger_position.map(|position| {
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position }
        }),
        closing_message_id: response.closing_message_id,
        retry_message_id: response.retry_message_id,
        deferred_fields,
        origin: ObservedResponseOrigin {
            conversation_id: response.origin.conversation_id,
            attempt_id: response.origin.attempt_id,
            attempt_id_source: response.origin.attempt_id_source,
            closing_message_id: response.origin.closing_message_id,
        },
        completed_at: response.completed_at,
        usage: response.usage,
        timing: response.timing,
        models: response.models,
    })
}

#[cfg(test)]
fn inherited_projection(
    store: &dyn ConversationStore,
    wanted: &BTreeSet<MessageId>,
    through: u64,
    projection: &mut ResponseProjection,
) -> Result<(), ConversationStoreError> {
    let identities: Vec<_> = wanted.iter().cloned().collect();
    let source = store.observation_inherited_native_oracle(&identities)?;
    for header in source.responses {
        let Some(closing) = header.response.closing_message_id.clone() else {
            continue;
        };
        projection.models.insert(
            closing.clone(),
            (
                header.models_total,
                header.response.models.len() as u64,
                header.models_next_offset,
            ),
        );
        projection
            .completed
            .insert(closing, inherited_response_view(store, header.response)?);
    }
    for turn in source.turns {
        if let Some(mut owner) = process_view(
            store,
            through,
            &turn.id.attempt_id,
            &turn.process_message_ids.iter().cloned().collect(),
            turn.started_at,
            turn.ended_at,
            turn.outcome.into(),
            None,
        )? {
            owner.conversation_id = turn.id.conversation_id;
            for message in turn.process_message_ids {
                if wanted.contains(&message) {
                    projection.processes.insert(message, owner.clone());
                }
            }
        }
    }
    Ok(())
}

fn summary_outcome(
    summary: &crate::durable::observation::AttemptExecutionSummary,
) -> TurnProcessOutcome {
    match summary.terminal_kind.as_deref() {
        Some("attempt_completed") => TurnProcessOutcome::Completed,
        Some("attempt_cancelled") => TurnProcessOutcome::Cancelled,
        Some("attempt_failed") => TurnProcessOutcome::Failed,
        Some("attempt_timed_out") => TurnProcessOutcome::TimedOut,
        Some("attempt_limit_exceeded") => TurnProcessOutcome::LimitExceeded,
        _ => TurnProcessOutcome::Running,
    }
}

#[cfg(test)]
fn apply_attempt_summary(
    store: &dyn ConversationStore,
    attempt: &AttemptId,
    members: &BTreeSet<MessageId>,
    through: u64,
    projection: &mut ResponseProjection,
) -> Result<(), ConversationStoreError> {
    let summary = store.observation_attempt_summary(attempt, through)?;
    let outcome = summary_outcome(&summary);
    let completed = matches!(
        summary.finish_reason,
        Some(ModelFinishReason::Stop | ModelFinishReason::Refusal)
    ) && outcome == TurnProcessOutcome::Completed;
    let final_message = completed
        .then(|| summary.closing_message_id.clone())
        .flatten();
    let mut process = process_view(
        store,
        through,
        attempt,
        members,
        summary.started_at,
        summary.ended_at,
        outcome.clone(),
        summary.terminal_event_id.clone(),
    )?;
    if process.is_none() {
        process = terminal_process_fallback(store, attempt, &summary, &outcome)?;
    }
    if outcome == TurnProcessOutcome::Failed {
        if let Some(process) = &mut process {
            process.failure.clone_from(&summary.failure);
        }
        if summary.failure.is_none() {
            return Err(ConversationStoreError::InvalidReference(
                "native terminal failure summary is unavailable".into(),
            ));
        }
    }
    if let Some(process) = process {
        for member in members {
            projection.processes.insert(member.clone(), process.clone());
        }
        if outcome != TurnProcessOutcome::Running {
            projection.terminals.insert(attempt.clone(), process);
        }
    }
    if outcome == TurnProcessOutcome::Running {
        projection.pending.extend(members.iter().cloned());
    }
    if let (Some(closing), Some(completed_at)) = (final_message, summary.ended_at) {
        let retry = summary
            .last_request_sequence
            .map(|request| store.observation_request_retry_at(request, through))
            .transpose()?
            .flatten();
        projection.models.insert(
            closing.clone(),
            (
                summary.models_total,
                summary.models.len() as u64,
                summary.models_next_offset,
            ),
        );
        let retry_locator = summary
            .last_request_sequence
            .map(|request| store.observation_request_retry_locator_at(request, through))
            .transpose()?
            .flatten();
        projection.completed.insert(
            closing.clone(),
            CompletedResponseView {
                closing_message_id: Some(closing.clone()),
                closing_locator: response_locator(store, &closing)?,
                surface_revision: store.message_append_revision(&closing)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "response Surface append unavailable".into(),
                    )
                })?,
                retry_locator: retry_locator.clone(),
                deferred_fields: if retry.is_none() && retry_locator.is_some() {
                    vec![vec!["retry_message_id".into()]]
                } else {
                    Vec::new()
                },
                origin: ObservedResponseOrigin {
                    conversation_id: store.conversation_id().clone(),
                    attempt_id: inline_attempt(attempt),
                    attempt_id_source: attempt_source(store, attempt, through)?,
                    closing_message_id: Some(closing),
                },
                completed_at,
                retry_message_id: retry.as_ref().and_then(inline_response_id),
                usage: (summary.requests > 0 && summary.requests == summary.reports)
                    .then_some(summary.usage)
                    .flatten(),
                timing: summary.timing,
                models: summary.models,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
fn terminal_process_fallback(
    store: &dyn ConversationStore,
    attempt: &AttemptId,
    summary: &crate::durable::observation::AttemptExecutionSummary,
    outcome: &TurnProcessOutcome,
) -> Result<Option<TurnProcessView>, ConversationStoreError> {
    let Some(event) = &summary.terminal_event_id else {
        return Ok(None);
    };
    let through = summary.terminal_sequence.ok_or_else(|| {
        ConversationStoreError::Storage("native terminal member cut unavailable".into())
    })?;
    let counts = store.observation_attempt_members(attempt, through)?;
    let cursor = if let Some(cursor) = counts.first_cursor {
        cursor
    } else {
        let Some(cursor) = store.event_transcript_cursor(event)? else {
            return Ok(None);
        };
        cursor
    };
    Ok(Some(TurnProcessView {
        conversation_id: store.conversation_id().clone(),
        attempt_id: inline_attempt(attempt),
        attempt_id_source: attempt_source(store, attempt, through)?,
        event_id: Some(event.clone()),
        control_cursor: cursor.into(),
        final_message_id: None,
        final_message_locator: None,
        final_message_id_deferred: false,
        final_message_id_source: None,
        message_count: u32::try_from(counts.message_count).map_err(|_| {
            ConversationStoreError::Storage("native member count exceeds protocol".into())
        })?,
        tool_call_count: u32::try_from(counts.tool_call_count).map_err(|_| {
            ConversationStoreError::Storage("native Tool count exceeds protocol".into())
        })?,
        outcome: outcome.clone(),
        failure: None,
        started_at: summary.started_at,
        ended_at: summary.ended_at,
    }))
}

pub(crate) fn observed_response(
    store: &dyn ConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::durable::observation::ResponseIdentitySource,
) -> Result<Option<CompletedResponseView>, ConversationStoreError> {
    use crate::durable::observation::ResponseIdentitySource;
    match source {
        ResponseIdentitySource::Inherited { ordinal } => store
            .inherited_response_summary(cut, *ordinal)?
            .map(|header| inherited_response_view(store, header.response))
            .transpose(),
        ResponseIdentitySource::Attempt { sequence } => {
            let summary = store.observation_execution_at(cut, *sequence)?;
            let owner = store
                .observation_members_at(cut, *sequence)?
                .process
                .ok_or_else(|| {
                    ConversationStoreError::Storage("native response owner unavailable".into())
                })?;
            if summary_outcome(&summary) != TurnProcessOutcome::Completed
                || !matches!(
                    summary.finish_reason,
                    Some(ModelFinishReason::Stop | ModelFinishReason::Refusal)
                )
            {
                return Ok(None);
            }
            let Some(position) = summary.closing_ledger_position else {
                return Ok(None);
            };
            let closing_locator =
                crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position };
            let Some(surface_revision) =
                store.message_append_revision_by_locator(&closing_locator)?
            else {
                return Ok(None);
            };
            let Some(completed_at) = summary.ended_at else {
                return Ok(None);
            };
            let retry_locator = summary
                .last_request_sequence
                .map(|request| {
                    store.observation_request_retry_locator_at(request, cut.read.journal)
                })
                .transpose()?
                .flatten();
            let retry_message_id = summary
                .last_request_sequence
                .map(|request| store.observation_request_retry_at(request, cut.read.journal))
                .transpose()?
                .flatten();
            let mut deferred_fields = Vec::new();
            if summary.closing_message_id.is_none() {
                deferred_fields.push(vec!["closing_message_id".into()]);
                deferred_fields.push(vec!["origin".into(), "closing_message_id".into()]);
            }
            if retry_locator.is_some() && retry_message_id.is_none() {
                deferred_fields.push(vec!["retry_message_id".into()]);
            }
            Ok(Some(CompletedResponseView {
                closing_message_id: summary.closing_message_id.clone(),
                closing_locator,
                retry_message_id,
                retry_locator,
                surface_revision,
                origin: ObservedResponseOrigin {
                    conversation_id: store.conversation_id().clone(),
                    attempt_id: owner.attempt_id,
                    attempt_id_source: owner.attempt_id_source,
                    closing_message_id: summary.closing_message_id,
                },
                deferred_fields,
                completed_at,
                usage: (summary.requests > 0 && summary.requests == summary.reports)
                    .then_some(summary.usage)
                    .flatten(),
                timing: summary.timing,
                models: summary.models,
            }))
        }
    }
}
#[derive(Serialize, JsonSchema)]
struct DeferredResponseFacts {
    turn_process: Option<TurnProcessView>,
    completed_response: Option<CompletedResponseView>,
}
fn validate_decoration_cut(
    page: &RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let cut = page.source_cut.as_ref().ok_or_else(|| {
        ConversationStoreError::InvalidReference("native decoration cut absent".into())
    })?;
    if cut.read.journal != through {
        return Err(ConversationStoreError::InvalidReference(
            "response decoration differs from native window cut".into(),
        ));
    }
    Ok(())
}
fn decorate_deferred_responses(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let Some(cut) = page.source_cut.as_ref() else {
        return Ok(());
    };
    for row in &mut page.deferred_entries {
        let process = store.observation_transcript_process(cut, row.offset)?;
        let source = store.response_identity_source(cut, row.offset)?;
        let response = source
            .as_ref()
            .map(|source| observed_response(store, cut, source))
            .transpose()?
            .flatten()
            .filter(|response| {
                row.value.as_ref().and_then(|value| value.get("locator"))
                    == serde_json::to_value(&response.closing_locator)
                        .ok()
                        .as_ref()
            });
        if process.is_none() && response.is_none() {
            continue;
        }
        if let Some(source) = source.as_ref() {
            page.collections
                .push(deferred_models_frontier(store, cut, source, row.offset)?);
        }
        merge_deferred_process(row, process, response, &mut page.collections)?;
        if let Some(source) = source {
            let fields = store.response_identity_fields(cut, &source)?;
            if fields.iter().any(|field| field.readable_end > 256) {
                page.collections
                    .push(crate::observation::cut::CollectionFrontier {
                        domain: format!(
                            "transcript.entries.{}.completed_response.identities",
                            row.offset
                        ),
                        revision: through,
                        total: fields.len() as u64,
                        loaded: 0,
                        next_offset: Some(0),
                    });
            }
        }
    }
    Ok(())
}

fn merge_deferred_process(
    row: &mut crate::observation::cut::CheckpointPageItem,
    process: Option<TurnProcessView>,
    response: Option<CompletedResponseView>,
    frontiers: &mut [crate::observation::cut::CollectionFrontier],
) -> Result<(), ConversationStoreError> {
    let mut facts = DeferredResponseFacts {
        turn_process: process,
        completed_response: response,
    };
    let facts = loop {
        if let Some(value) = crate::observation::bounded::structural_value(&facts) {
            break value;
        }
        let models = facts
            .completed_response
            .as_mut()
            .map(|response| &mut response.models);
        if let Some(models) = models.filter(|models| !models.is_empty()) {
            models.pop();
        } else {
            return Err(ConversationStoreError::Storage(
                "native process display exceeds bound".into(),
            ));
        }
    };
    let value = row
        .value
        .as_mut()
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            ConversationStoreError::Storage("native process descriptor unavailable".into())
        })?;
    let facts = facts
        .as_object()
        .expect("typed process facts are an object");
    value.extend(
        facts
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    // Keep the original encoded descriptor. Its UInt64 fields are already in
    // wire form, so decoding it into a native DTO would lose that representation.
    while crate::observation::bounded::bounded_bytes(value, 14_336).is_none() {
        if !trim_descriptor_call(value, frontiers)? {
            let models = value
                .get_mut("completed_response")
                .and_then(|response| response.get_mut("models"))
                .and_then(serde_json::Value::as_array_mut);
            if let Some(models) = models.filter(|models| !models.is_empty()) {
                models.pop();
            } else {
                return Err(ConversationStoreError::Storage(
                    "native process descriptor exceeds bound".into(),
                ));
            }
        }
    }
    if value
        .get("completed_response")
        .is_some_and(|response| !response.is_null())
        && let Some(frontier) = frontiers.iter_mut().find(|frontier| {
            frontier.domain
                == format!(
                    "transcript.entries.{}.completed_response.models",
                    row.offset
                )
        })
    {
        frontier.loaded = value
            .get("completed_response")
            .and_then(|response| response.get("models"))
            .and_then(serde_json::Value::as_array)
            .map_or(0, |models| models.len() as u64);
        frontier.next_offset = (frontier.loaded < frontier.total).then_some(frontier.loaded);
    }
    Ok(())
}

fn deferred_models_frontier(
    store: &dyn ConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::durable::observation::ResponseIdentitySource,
    cursor: u64,
) -> Result<crate::observation::cut::CollectionFrontier, ConversationStoreError> {
    use crate::durable::observation::ResponseIdentitySource;
    let (total, loaded, next_offset) = match source {
        ResponseIdentitySource::Attempt { sequence } => {
            let summary = store.observation_execution_at(cut, *sequence)?;
            (
                summary.models_total,
                summary.models.len() as u64,
                summary.models_next_offset,
            )
        }
        ResponseIdentitySource::Inherited { ordinal } => {
            let header = store
                .inherited_response_summary(cut, *ordinal)?
                .ok_or_else(|| {
                    ConversationStoreError::Storage("native response header unavailable".into())
                })?;
            (
                header.models_total,
                header.response.models.len() as u64,
                header.models_next_offset,
            )
        }
    };
    Ok(crate::observation::cut::CollectionFrontier {
        domain: format!("transcript.entries.{cursor}.completed_response.models"),
        revision: cut.read.journal,
        total,
        loaded,
        next_offset,
    })
}
fn trim_descriptor_call(
    value: &mut serde_json::Map<String, serde_json::Value>,
    frontiers: &mut [crate::observation::cut::CollectionFrontier],
) -> Result<bool, ConversationStoreError> {
    let locator = value
        .get("locator")
        .cloned()
        .map(serde_json::from_value::<crate::observation::content::CanonicalContentLocator>)
        .transpose()
        .map_err(|_| ConversationStoreError::Storage("native descriptor locator corrupt".into()))?;
    let calls = value
        .get_mut("tool_calls")
        .and_then(serde_json::Value::as_array_mut);
    let Some(calls) = calls.filter(|calls| !calls.is_empty()) else {
        return Ok(false);
    };
    let removed = calls.pop().expect("nonempty bounded Tool prefix");
    let offset = removed
        .pointer("/occurrence/block_index")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ConversationStoreError::Storage("native Tool occurrence unavailable".into())
        })?;
    let domain = format!(
        "messages.{}.tools",
        locator
            .ok_or_else(|| ConversationStoreError::Storage(
                "native descriptor locator absent".into()
            ))?
            .key()
    );
    let frontier = frontiers
        .iter_mut()
        .find(|frontier| frontier.domain == domain)
        .ok_or_else(|| {
            ConversationStoreError::Storage("native Tool frontier unavailable".into())
        })?;
    frontier.loaded = calls.len() as u64;
    frontier.next_offset = Some(offset);
    if let Some(collections) = value
        .get_mut("collections")
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in collections {
            if item.get("domain").and_then(serde_json::Value::as_str) == Some(domain.as_str()) {
                item["loaded"] = serde_json::Value::String(frontier.loaded.to_string());
                item["next_offset"] = serde_json::Value::String(offset.to_string());
            }
        }
    }
    Ok(true)
}

/// Full native membership determines counts and seat, never the requested page.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn process_view(
    store: &dyn ConversationStore,
    through: u64,
    attempt_id: &AttemptId,
    members: &BTreeSet<MessageId>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    outcome: TurnProcessOutcome,
    event_id: Option<crate::runtime::identity::EventId>,
) -> Result<Option<TurnProcessView>, ConversationStoreError> {
    let Some(member) = members.iter().next() else {
        return Ok(None);
    };
    let Some(counts) = store.observation_member_counts(member, through)? else {
        return Ok(None);
    };
    let message_count = counts
        .message_count
        .saturating_sub(u64::from(counts.completed_response));
    let message_count = u32::try_from(message_count).map_err(|_| {
        ConversationStoreError::InvalidReference(
            "native process message count exceeds protocol range".into(),
        )
    })?;
    let tool_call_count = u32::try_from(counts.tool_call_count).map_err(|_| {
        ConversationStoreError::InvalidReference(
            "native process tool count exceeds protocol range".into(),
        )
    })?;
    let control = counts.first_cursor;
    let source = attempt_source(store, attempt_id, through)?;
    Ok(control.map(|cursor| TurnProcessView {
        conversation_id: store.conversation_id().clone(),
        attempt_id: inline_attempt(attempt_id),
        attempt_id_source: source,
        event_id,
        control_cursor: cursor.into(),
        final_message_id: counts
            .completed_response
            .then_some(counts.final_message_id)
            .flatten(),
        final_message_locator: counts
            .completed_response
            .then_some(counts.final_message_locator)
            .flatten(),
        final_message_id_deferred: counts.completed_response && counts.final_message_id_deferred,
        final_message_id_source: counts
            .completed_response
            .then_some(counts.final_message_id_source)
            .flatten(),
        message_count,
        tool_call_count,
        outcome,
        failure: None,
        started_at,
        ended_at,
    }))
}

pub(crate) fn identity_selection(domain: &str) -> Option<u64> {
    domain
        .strip_prefix("transcript.entries.")?
        .strip_suffix(".completed_response.identities")?
        .parse()
        .ok()
}
pub(crate) fn identity_page(
    store: &dyn ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    cursor: u64,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, super::RuntimeClientError> {
    use super::RuntimeClientError;
    let source_cut = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if limit == 0 || limit > 32 || cursor > source_cut.read.transcript {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid response identity page bounds".into(),
        });
    }
    let source = store
        .response_identity_source(source_cut, cursor)
        .map_err(RuntimeClientError::observation_store_error)?
        .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
    let fields = store
        .response_identity_fields(source_cut, &source)
        .map_err(RuntimeClientError::observation_store_error)?;
    let total = fields.len() as u64;
    let start = usize::try_from(offset)
        .ok()
        .filter(|offset| *offset <= fields.len())
        .ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "response identity page exceeds frontier".into(),
        })?;
    let end = start.saturating_add(limit).min(fields.len());
    let mut items = Vec::new();
    for (index, field) in fields[start..end].iter().enumerate() {
        let value = if field.readable_end <= 256 {
            let bytes = store
                .response_identity_read(source_cut, &source, &field.path, 0, 256)
                .map_err(RuntimeClientError::observation_store_error)?;
            Some(serde_json::Value::String(
                String::from_utf8(bytes.bytes).map_err(|_| {
                    RuntimeClientError::ObservationUnavailable {
                        message: "native response identity bytes are unavailable".into(),
                    }
                })?,
            ))
        } else {
            None
        };
        items.push(crate::observation::cut::CheckpointPageItem {
            offset: offset + index as u64,
            identity: field.path.join("."),
            deferred: value.is_none(),
            value,
            body_complete: None,
            key: None,
            content: Vec::new(),
        });
    }
    Ok(super::checkpoint::CheckpointPage {
        read_cut: cut.clone(),
        cursor: super::RuntimeClientCursor::new(cut.revision),
        domain: domain.into(),
        total,
        items,
        next_offset: (end < fields.len()).then_some(end as u64),
        collections: Vec::new(),
    })
}
pub(crate) fn identity_read(
    store: &dyn ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    cursor: u64,
    item_offset: u64,
    range: (u64, u32),
) -> Result<super::checkpoint::CheckpointValueChunk, super::RuntimeClientError> {
    use super::RuntimeClientError;
    use base64::Engine as _;
    let source_cut = cut
        .source_cut
        .as_ref()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if cursor > source_cut.read.transcript {
        return Err(RuntimeClientError::InvalidRequest {
            message: "response identity exceeds transcript frontier".into(),
        });
    }
    let source = store
        .response_identity_source(source_cut, cursor)
        .map_err(RuntimeClientError::observation_store_error)?
        .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
    let fields = store
        .response_identity_fields(source_cut, &source)
        .map_err(RuntimeClientError::observation_store_error)?;
    let field = usize::try_from(item_offset)
        .ok()
        .and_then(|index| fields.get(index))
        .ok_or_else(|| RuntimeClientError::InvalidRequest {
            message: "response identity field exceeds frontier".into(),
        })?;
    let chunk = store
        .response_identity_read(source_cut, &source, &field.path, range.0, range.1)
        .map_err(RuntimeClientError::observation_store_error)?;
    Ok(super::checkpoint::CheckpointValueChunk {
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: Vec::new(),
        offset: range.0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
        representation: super::checkpoint::CheckpointValueRepresentation::Utf8Text,
    })
}

#[cfg(test)]
pub(crate) mod tests;
