//! Client-neutral completed-response projections over native durable evidence.
//!
//! No state is persisted here. Acceptance identifies content; Attempt completion
//! identifies its closing response. Provider terminals alone never close a tail.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEvent;
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelUsage, UsageDetails};
use crate::runtime::identity::{AttemptId, MessageId, RequestId};

use super::snapshot::{RuntimeClientTranscriptItem, RuntimeClientTranscriptPage};

/// A completed execution's exact closing canonical response. This is not history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedResponseView {
    pub closing_message_id: MessageId,
    pub attempt_id: AttemptId,
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
}

/// Whole-conversation execution totals, independent of any transcript window.
/// Forked Conversations start a fresh execution epoch, as native lineage does.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationStatistics {
    pub completed_responses: u64,
    pub model_requests: u64,
    /// Known reported usage. Coverage is explicit; missing reports are not zero.
    pub requests_with_usage: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_usage: Option<ModelUsage>,
}

#[derive(Default)]
struct AttemptEvidence {
    closing: Option<MessageId>,
    last_request: Option<RequestId>,
    requests: u64,
    reports: u64,
    usage: Option<ModelUsage>,
}

fn add_usage(total: &mut Option<ModelUsage>, usage: &ModelUsage) {
    let Some(total) = total.as_mut() else {
        *total = Some(usage.clone());
        return;
    };
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    let sum = |a: Option<u64>, b: Option<u64>| a.zip(b).map(|(a, b)| a.saturating_add(b));
    total.details = total
        .details
        .as_ref()
        .zip(usage.details.as_ref())
        .map(|(a, b)| UsageDetails {
            reasoning_tokens: sum(a.reasoning_tokens, b.reasoning_tokens),
            cached_input_tokens: sum(a.cached_input_tokens, b.cached_input_tokens),
        });
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

#[allow(clippy::too_many_lines)] // One finite ordered fold; provider terminals and canonical acceptance remain distinct.
pub(crate) fn decorate_through(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    for entry in &mut page.entries {
        entry.completed_response = None;
        entry.response_pending = false;
    }
    let wanted: BTreeSet<_> = page
        .entries
        .iter()
        .filter_map(|entry| match &entry.item {
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Assistant(message),
            } => Some(message.id.clone()),
            _ => None,
        })
        .collect();
    let mut attempts: BTreeMap<AttemptId, AttemptEvidence> = BTreeMap::new();
    let mut completed = BTreeMap::new();
    let mut statistics = ConversationStatistics::default();
    let mut after = 0;
    loop {
        let events = store.read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec![
                "attempt_started",
                "model_request_started",
                "model_request_completed",
                "model_request_failed",
                "assistant_message_committed",
                "attempt_completed",
                "attempt_cancelled",
                "attempt_failed",
                "attempt_timed_out",
                "attempt_limit_exceeded",
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
            let Some(id) = event.attempt_id else {
                continue;
            };
            let evidence = attempts.entry(id.clone()).or_default();
            match event.event {
                RuntimeEvent::ModelRequestStarted { request_id, .. } => {
                    evidence.last_request = Some(request_id);
                    evidence.requests += 1;
                    statistics.model_requests += 1;
                }
                RuntimeEvent::ModelRequestCompleted {
                    usage: Some(usage), ..
                }
                | RuntimeEvent::ModelRequestFailed {
                    usage: Some(usage), ..
                } => {
                    evidence.reports += 1;
                    statistics.requests_with_usage += 1;
                    add_usage(&mut evidence.usage, &usage);
                    add_usage(&mut statistics.reported_usage, &usage);
                }
                RuntimeEvent::AssistantMessageCommitted { message_id } => {
                    evidence.closing = Some(message_id);
                }
                RuntimeEvent::AttemptCompleted {
                    finish_reason: ModelFinishReason::Stop | ModelFinishReason::Refusal,
                    ..
                } => {
                    if let Some(closing) = evidence.closing.take() {
                        statistics.completed_responses += 1;
                        if wanted.contains(&closing) {
                            let revision =
                                store.message_append_revision(&closing)?.ok_or_else(|| {
                                    ConversationStoreError::InvalidReference(
                                        "Completed response has no canonical Surface append".into(),
                                    )
                                })?;
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
                                CompletedResponseView {
                                    closing_message_id: closing,
                                    attempt_id: id.clone(),
                                    completed_at: event.timestamp,
                                    surface_revision: revision,
                                    retry_message_id,
                                    usage: (evidence.requests > 0
                                        && evidence.requests == evidence.reports)
                                        .then(|| evidence.usage.take())
                                        .flatten(),
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
                    attempts.remove(&id);
                }
                _ => {}
            }
        }
    }
    let pending: BTreeSet<_> = attempts
        .values()
        .filter_map(|evidence| evidence.closing.as_ref())
        .collect();
    for entry in &mut page.entries {
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
            entry.completed_response = Some(response);
        }
    }
    page.statistics = Some(statistics);
    Ok(())
}

/// Validates a post-response boundary against durable completion and acceptance,
/// independently of whatever metadata a client supplied.
/// # Errors
/// Propagates durable read failures.
pub(crate) fn is_completed_response(
    store: &dyn ConversationStore,
    message: &MessageId,
) -> Result<bool, ConversationStoreError> {
    let through = store.presentation_frontier()?;
    let mut after = 0;
    loop {
        let events = store.read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec!["assistant_message_committed"],
            before: None,
            after,
            ascending: true,
            through,
            limit: 128,
        })?;
        if events.is_empty() {
            return Ok(false);
        }
        for event in events {
            after = event.sequence;
            if !matches!(&event.event, RuntimeEvent::AssistantMessageCommitted { message_id } if message_id == message)
            {
                continue;
            }
            let Some(attempt) = event.attempt_id else {
                return Ok(false);
            };
            let end = store.read_presentation_events(&FactQuery {
                scope: FactScope::Attempt(attempt),
                kinds: vec![
                    "assistant_message_committed",
                    "attempt_completed",
                    "attempt_cancelled",
                    "attempt_failed",
                    "attempt_timed_out",
                    "attempt_limit_exceeded",
                ],
                before: None,
                after,
                ascending: true,
                through,
                limit: 1,
            })?;
            return Ok(matches!(
                end.first().map(|event| &event.event),
                Some(RuntimeEvent::AttemptCompleted {
                    finish_reason: ModelFinishReason::Stop | ModelFinishReason::Refusal,
                    ..
                })
            ));
        }
    }
}

#[cfg(test)]
mod tests;
