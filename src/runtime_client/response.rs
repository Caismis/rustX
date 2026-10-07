//! Client-neutral completed-response projections over native durable evidence.
//!
//! No state is persisted here. Acceptance identifies content; Attempt completion
//! identifies its closing response. Provider terminals alone never close a tail.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::response::{CompletedResponseProvenance, ResponseOrigin};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEvent;
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{AttemptId, MessageId};

use crate::durable::response::{AttemptEvidence, add_usage, timing};

use super::snapshot::{RuntimeClientTranscriptItem, RuntimeClientTranscriptPage};

/// Derived response view over local execution or inherited lineage provenance.
/// Canonical content remains in the Message Ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedResponseView {
    pub closing_message_id: MessageId,
    /// Original execution owner, including for inherited historical responses.
    pub origin: ResponseOrigin,
    /// Durable completion timestamp; never a browser receipt time.
    pub completed_at: chrono::DateTime<chrono::Utc>,
    /// Immutable first Surface revision containing the closing response.
    pub surface_revision: SurfaceRevision,
    /// Exact adopted ordinary input, when this Attempt has a replayable input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_message_id: Option<MessageId>,
    /// All actual requests in the Attempt, only when every usage is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<crate::durable::response::CompletedResponseTiming>,
    /// Distinct provider-facing models of the Attempt's actual requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// One native process owner shared by live, successful and unsuccessful Turns.
/// Repeated on each exact member so bounded pages are independently resolvable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnProcessView {
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub attempt_id: AttemptId,
    /// Original terminal Journal identity, absent while running or inherited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<crate::runtime::identity::EventId>,
    /// Control sorts immediately before this immutable native transcript position.
    /// No-content terminal Attempts use their terminal event position.
    pub control_cursor: super::snapshot::RuntimeClientTranscriptCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_message_id: Option<MessageId>,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub outcome: TurnProcessOutcome,
    /// Normalized terminal failure, retained at its durable transcript position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<super::event::RuntimeClientAttemptFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub use crate::durable::reading::TurnProcessOutcome;

pub(crate) fn terminal_turn(
    event: crate::events::types::RuntimeEventEnvelope,
    cursor: super::snapshot::RuntimeClientTranscriptCursor,
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
        attempt_id: event.attempt_id.ok_or("terminal Attempt has no identity")?,
        event_id: Some(event.event_id),
        control_cursor: cursor,
        final_message_id: None,
        message_count: 0,
        tool_call_count: 0,
        outcome,
        failure,
        started_at: None,
        ended_at: Some(event.timestamp),
    })
}

/// Whole-conversation execution totals, independent of any transcript window.
/// A lineage child includes its inherited turns' recorded execution, as a
/// `DeepSeek` Harness fork folds its copied prefix; it owns none of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationStatistics {
    /// Harness-style Turn and Step counts: native Attempt starts and Loop turns.
    pub turns: u64,
    pub steps: u64,
    /// Latest native Turn clock; never a browser receipt timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_turn: Option<ConversationTurnClock>,
    /// Summed measured work time, separate from usage coverage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<crate::durable::response::ConversationTiming>,
    pub completed_responses: u64,
    pub model_requests: u64,
    /// Known reported usage. Coverage is explicit; missing reports are not zero.
    pub requests_with_usage: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_usage: Option<ModelUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurnClock {
    pub attempt_id: AttemptId,
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Reads a finite Journal prefix and decorates only exact canonical identities.
/// # Errors
/// Durable failures remain errors rather than silently becoming missing evidence.
pub fn decorate(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
) -> Result<(), ConversationStoreError> {
    decorate_through(store, page, store.presentation_frontier()?)
}

pub(crate) fn decorate_through(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    decorate_projection(store, page, through, None)
}

pub(crate) fn decorate_window(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let messages: Vec<_> = page
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
    let mut attempts: BTreeSet<_> = store.transcript_attempts(&messages)?.into_iter().collect();
    attempts.extend(page.entries.iter().filter_map(|entry| match &entry.item {
        RuntimeClientTranscriptItem::AttemptTerminal { turn } => Some(turn.attempt_id.clone()),
        _ => None,
    }));
    decorate_projection(store, page, through, Some(&attempts))?;
    // Whole-conversation totals have a separate native live projection. A
    // historical window never scans all unrelated Attempts to rebuild it.
    page.statistics = None;
    Ok(())
}

fn decorate_projection(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
    scopes: Option<&BTreeSet<AttemptId>>,
) -> Result<(), ConversationStoreError> {
    for entry in &mut page.entries {
        entry.completed_response = None;
        entry.turn_process = None;
        entry.response_pending = false;
    }
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
            RuntimeClientTranscriptItem::AttemptTerminal { turn } => Some(turn.attempt_id.clone()),
            _ => None,
        })
        .collect();
    let ResponseProjection {
        mut completed,
        processes,
        pending,
        statistics,
        terminals,
    } = project(store, &wanted, &terminal_attempts, through, scopes)?;
    for entry in &mut page.entries {
        if let RuntimeClientTranscriptItem::AttemptTerminal { turn } = &mut entry.item {
            if turn.conversation_id == *store.conversation_id()
                && let Some(owner) = terminals.get(&turn.attempt_id)
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
            if let Some(input) = &response.retry_message_id {
                let replayable = store.load_messages(std::slice::from_ref(input))?.iter().any(|message|
                    matches!(message, MessageBlock::User(user) if user.kind == InboundKind::Message));
                if !replayable {
                    response.retry_message_id = None;
                }
            }
            entry.completed_response = Some(CompletedResponseView {
                surface_revision: store.message_append_revision(&message.id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "Completed response has no canonical Surface append".into(),
                    )
                })?,
                closing_message_id: response.closing_message_id,
                origin: response.origin,
                completed_at: response.completed_at,
                retry_message_id: response.retry_message_id,
                usage: response.usage,
                timing: response.timing,
                models: response.models,
            });
        }
    }
    page.statistics = Some(statistics);
    Ok(())
}

struct ResponseProjection {
    terminals: BTreeMap<AttemptId, TurnProcessView>,
    completed: BTreeMap<MessageId, CompletedResponseProvenance>,
    processes: BTreeMap<MessageId, TurnProcessView>,
    pending: BTreeSet<MessageId>,
    statistics: ConversationStatistics,
}

#[allow(clippy::too_many_lines)] // One shared finite fold; no second Journal scan for lineage provenance.
fn project(
    store: &dyn ConversationStore,
    wanted: &BTreeSet<MessageId>,
    terminal_attempts: &BTreeSet<AttemptId>,
    through: u64,
    selected: Option<&BTreeSet<AttemptId>>,
) -> Result<ResponseProjection, ConversationStoreError> {
    let mut terminals = BTreeMap::new();
    let mut attempts: BTreeMap<AttemptId, AttemptEvidence> = BTreeMap::new();
    let mut completed: BTreeMap<_, _> = store
        .inherited_responses_for(&wanted.iter().cloned().collect::<Vec<_>>())?
        .into_iter()
        .filter(|response| {
            wanted.contains(&response.closing_message_id)
                || response
                    .process_message_ids
                    .iter()
                    .any(|id| wanted.contains(id))
        })
        .map(|response| (response.closing_message_id.clone(), response))
        .collect();
    let mut processes = BTreeMap::new();
    for turn in store.inherited_turns_for(&wanted.iter().cloned().collect::<Vec<_>>())? {
        if turn
            .process_message_ids
            .iter()
            .all(|id| !wanted.contains(id))
        {
            continue;
        }
        let completed_response = completed.values().find(|response| {
            response.origin.conversation_id == turn.id.conversation_id
                && response.origin.attempt_id == turn.id.attempt_id
        });
        if let Some(mut owner) = process_view(
            store,
            &turn.id.attempt_id,
            &turn.process_message_ids.iter().cloned().collect(),
            completed_response.map(|response| response.closing_message_id.clone()),
            turn.started_at,
            turn.ended_at,
            turn.outcome.into(),
            None,
        )? {
            owner.conversation_id = turn.id.conversation_id;
            for id in turn.process_message_ids {
                if wanted.contains(&id) {
                    processes.insert(id, owner.clone());
                }
            }
        }
    }
    let mut statistics = ConversationStatistics::default();
    let mut activity = timing::ActivityFold::default();
    // As a DeepSeek Harness fork folds its copied event prefix, whole-conversation
    // totals begin with the inherited turns' own recorded execution.
    if selected.is_none() {
        for turn in store.load_inherited_turns()? {
            statistics.turns += 1;
            let Some(execution) = turn.execution else {
                continue;
            };
            statistics.steps += execution.steps;
            statistics.model_requests += execution.model_requests;
            statistics.requests_with_usage += execution.requests_with_usage;
            if let Some(usage) = &execution.reported_usage {
                add_usage(&mut statistics.reported_usage, usage);
            }
            statistics.completed_responses += u64::from(execution.completed_response);
            activity.absorb(&execution);
        }
    }
    let scopes = selected.map_or_else(
        || vec![FactScope::All],
        |selected| selected.iter().cloned().map(FactScope::Attempt).collect(),
    );
    for scope in scopes {
        let mut after = 0;
        loop {
            let events = store.read_presentation_events(&FactQuery {
                scope: scope.clone(),
                kinds: vec![
                    "attempt_started",
                    "turn_started",
                    "model_request_started",
                    "model_request_completed",
                    "model_request_failed",
                    "assistant_message_committed",
                    "attempt_completed",
                    "attempt_cancelled",
                    "attempt_failed",
                    "attempt_timed_out",
                    "attempt_limit_exceeded",
                    "tool_execution_started",
                    "tool_execution_completed",
                    "tool_execution_failed",
                ],
                before: None,
                after,
                ascending: true,
                through,
                limit: 128,
            })?;
            if events.is_empty() {
                break;
            }
            for event in events {
                after = event.sequence;
                let Some(id) = event.attempt_id.clone() else {
                    continue;
                };
                match &event.event {
                    RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } => {
                        activity.tool_started(id, tool_call_id.clone(), event.timestamp);
                        continue;
                    }
                    RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                    | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => {
                        activity.tool_settled(id, tool_call_id.clone(), event.timestamp);
                        continue;
                    }
                    _ => {}
                }
                let evidence = attempts.entry(id.clone()).or_default();
                match event.event {
                    RuntimeEvent::AttemptStarted { .. } => {
                        evidence.started_at = Some(event.timestamp);
                        statistics.turns += 1;
                        statistics.latest_turn = Some(ConversationTurnClock {
                            attempt_id: id.clone(),
                            started_at: event.timestamp,
                            ended_at: None,
                        });
                    }
                    RuntimeEvent::TurnStarted => statistics.steps += 1,
                    RuntimeEvent::ModelRequestStarted { request_id, model } => {
                        evidence.request_started(request_id, model);
                        statistics.model_requests += 1;
                    }
                    RuntimeEvent::ModelRequestCompleted {
                        request_id,
                        usage,
                        generation,
                        ..
                    }
                    | RuntimeEvent::ModelRequestFailed {
                        request_id,
                        usage,
                        generation,
                        ..
                    } => {
                        activity.request(generation.as_ref(), usage.as_ref());
                        evidence
                            .timing
                            .terminal(&request_id, generation, usage.as_ref());
                        if let Some(usage) = usage {
                            evidence.reports += 1;
                            statistics.requests_with_usage += 1;
                            add_usage(&mut evidence.usage, &usage);
                            add_usage(&mut statistics.reported_usage, &usage);
                        }
                    }
                    RuntimeEvent::AssistantMessageCommitted { message_id } => {
                        evidence.members.insert(message_id.clone());
                        evidence.closing = Some(message_id);
                    }
                    RuntimeEvent::AttemptCompleted {
                        finish_reason: ModelFinishReason::Stop | ModelFinishReason::Refusal,
                        ..
                    } => {
                        if let Some(clock) = statistics
                            .latest_turn
                            .as_mut()
                            .filter(|clock| clock.attempt_id == id)
                        {
                            clock.ended_at = Some(event.timestamp);
                        }
                        if let Some(closing) = evidence.closing.take() {
                            statistics.completed_responses += 1;
                            if !evidence.members.is_disjoint(wanted)
                                && let Some(process) = process_view(
                                    store,
                                    &id,
                                    &evidence.members,
                                    Some(closing.clone()),
                                    evidence.started_at,
                                    Some(event.timestamp),
                                    TurnProcessOutcome::Completed,
                                    Some(event.event_id.clone()),
                                )?
                            {
                                for member in &evidence.members {
                                    if wanted.contains(member) {
                                        processes.insert(member.clone(), process.clone());
                                    }
                                }
                            }
                            if wanted.contains(&closing) {
                                let retry_message_id = match &evidence.last_request {
                                    Some(id) => {
                                        let request = store.load_request_snapshot(id)?;
                                        store
                                            .load_surface_snapshot(request.surface_revision)?
                                            .iter()
                                            .rev()
                                            .find_map(|message| match message {
                                                MessageBlock::User(user)
                                                    if user.kind == InboundKind::Message =>
                                                {
                                                    Some(user.id.clone())
                                                }
                                                _ => None,
                                            })
                                    }
                                    None => None,
                                };
                                completed.insert(
                                    closing.clone(),
                                    CompletedResponseProvenance {
                                        process_message_ids: evidence
                                            .members
                                            .iter()
                                            .cloned()
                                            .collect(),
                                        closing_message_id: closing.clone(),
                                        origin: ResponseOrigin {
                                            conversation_id: store.conversation_id().clone(),
                                            attempt_id: id.clone(),
                                            closing_message_id: closing,
                                        },
                                        completed_at: event.timestamp,
                                        timing: evidence
                                            .timing
                                            .summary(evidence.started_at, event.timestamp),
                                        retry_message_id,
                                        usage: (evidence.requests > 0
                                            && evidence.requests == evidence.reports)
                                            .then(|| evidence.usage.take())
                                            .flatten(),
                                        models: std::mem::take(&mut evidence.models),
                                    },
                                );
                            }
                        }
                        attempts.remove(&id);
                    }
                    RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptFailed { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. } => {
                        if let Some(clock) = statistics
                            .latest_turn
                            .as_mut()
                            .filter(|clock| clock.attempt_id == id)
                        {
                            clock.ended_at = Some(event.timestamp);
                        }
                        let outcome = match event.event {
                            RuntimeEvent::AttemptCancelled { .. } => {
                                Some(TurnProcessOutcome::Cancelled)
                            }
                            RuntimeEvent::AttemptFailed { .. } => Some(TurnProcessOutcome::Failed),
                            RuntimeEvent::AttemptTimedOut { .. } => {
                                Some(TurnProcessOutcome::TimedOut)
                            }
                            RuntimeEvent::AttemptLimitExceeded { .. } => {
                                Some(TurnProcessOutcome::LimitExceeded)
                            }
                            _ => None,
                        };
                        if let Some(outcome) = outcome {
                            if !terminal_attempts.contains(&id)
                                && evidence.members.is_disjoint(wanted)
                            {
                                attempts.remove(&id);
                                continue;
                            }
                            if let Some(mut process) = process_view(
                                store,
                                &id,
                                &evidence.members,
                                None,
                                evidence.started_at,
                                Some(event.timestamp),
                                outcome,
                                Some(event.event_id.clone()),
                            )? {
                                if let RuntimeEvent::AttemptFailed { error, .. } = &event.event {
                                    process.failure =
                                        Some(super::projection::client_failure(error));
                                }
                                for member in &evidence.members {
                                    if wanted.contains(member) {
                                        processes.insert(member.clone(), process.clone());
                                    }
                                }
                                terminals.insert(id.clone(), process);
                            } else if terminal_attempts.contains(&id) {
                                let cursor = store
                                    .event_transcript_cursor(&event.event_id)?
                                    .ok_or_else(|| {
                                        ConversationStoreError::InvalidReference(
                                            "terminal has no transcript position".into(),
                                        )
                                    })?;
                                let mut owner = terminal_turn(event.clone(), cursor.into())
                                    .map_err(ConversationStoreError::InvalidReference)?;
                                owner.started_at = evidence.started_at;
                                terminals.insert(id.clone(), owner);
                            }
                        }
                        attempts.remove(&id);
                    }
                    _ => {}
                }
            }
        }
    }
    for (id, evidence) in &attempts {
        if evidence.members.is_disjoint(wanted) {
            continue;
        }
        if let Some(process) = process_view(
            store,
            id,
            &evidence.members,
            None,
            evidence.started_at,
            None,
            TurnProcessOutcome::Running,
            None,
        )? {
            for member in &evidence.members {
                if wanted.contains(member) {
                    processes.insert(member.clone(), process.clone());
                }
            }
        }
    }
    statistics.timing = activity.summary();
    let pending: BTreeSet<_> = attempts
        .values()
        .flat_map(|evidence| evidence.members.iter().cloned())
        .collect();
    Ok(ResponseProjection {
        terminals,
        completed,
        processes,
        pending,
        statistics,
    })
}

/// Full native membership determines counts and seat, never the requested page.
#[allow(clippy::too_many_arguments)]
fn process_view(
    store: &dyn ConversationStore,
    attempt_id: &AttemptId,
    members: &BTreeSet<MessageId>,
    final_message_id: Option<MessageId>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    outcome: TurnProcessOutcome,
    event_id: Option<crate::runtime::identity::EventId>,
) -> Result<Option<TurnProcessView>, ConversationStoreError> {
    let mut control = None;
    let mut message_count = 0;
    let mut tool_call_count = 0;
    for message in store.load_messages(&members.iter().cloned().collect::<Vec<_>>())? {
        let MessageBlock::Assistant(message) = message else {
            continue;
        };
        if let Some(cursor) = store.message_transcript_cursor(&message.id)? {
            control = Some(
                control.map_or(cursor, |previous: crate::durable::TranscriptCursor| {
                    previous.min(cursor)
                }),
            );
        }
        if Some(&message.id) != final_message_id.as_ref() {
            message_count += 1;
        }
        tool_call_count += u32::try_from(
            message
                .content
                .iter()
                .filter(|block| matches!(block, AssistantContentBlock::ToolCall(_)))
                .count(),
        )
        .map_err(|_| ConversationStoreError::InvalidReference("too many process tools".into()))?;
    }
    Ok(control.map(|cursor| TurnProcessView {
        conversation_id: store.conversation_id().clone(),
        attempt_id: attempt_id.clone(),
        event_id,
        control_cursor: cursor.into(),
        final_message_id,
        message_count,
        tool_call_count,
        outcome,
        failure: None,
        started_at,
        ended_at,
    }))
}

#[cfg(test)]
pub(crate) mod tests;
