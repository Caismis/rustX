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
use crate::model::types::{ModelUsage, UsageDetails};
use crate::runtime::identity::{AttemptId, MessageId, RequestId};

mod timing;

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
}

/// Native ownership of a completed Attempt's process. Destination final address
/// and immutable origin are distinct, including through lineage copies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedProcessView {
    pub origin: ResponseOrigin,
    pub final_message_id: MessageId,
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
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    timing: timing::TimingFold,
    closing: Option<MessageId>,
    members: BTreeSet<MessageId>,
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

pub(crate) fn decorate_through(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    for entry in &mut page.entries {
        entry.completed_response = None;
        entry.completed_process = None;
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
    let ResponseProjection {
        mut completed,
        processes,
        pending,
        statistics,
    } = project(store, &wanted, through)?;
    for entry in &mut page.entries {
        let owner = match &entry.item {
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Assistant(message),
            } => Some(&message.id),
            RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Tool(message),
            } => Some(&message.occurrence.assistant_message_id),
            _ => None,
        };
        entry.completed_process = owner.and_then(|id| processes.get(id)).cloned();
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
            });
        }
    }
    page.statistics = Some(statistics);
    Ok(())
}

struct ResponseProjection {
    completed: BTreeMap<MessageId, CompletedResponseProvenance>,
    processes: BTreeMap<MessageId, CompletedProcessView>,
    pending: BTreeSet<MessageId>,
    statistics: ConversationStatistics,
}

/// Derive the lineage-safe historical facts in one native evidence fold.
/// Only canonical Assistant identities supplied by the lineage owner are selected.
/// # Errors
/// Propagates durable read failures rather than dropping historical facts.
pub(crate) fn lineage_provenance(
    store: &dyn ConversationStore,
    canonical: &[MessageBlock],
) -> Result<Vec<CompletedResponseProvenance>, ConversationStoreError> {
    let wanted = canonical
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(assistant.id.clone()),
            _ => None,
        })
        .collect();
    let mut projection = project(store, &wanted, store.presentation_frontier()?)?;
    Ok(canonical
        .iter()
        .filter_map(|message| {
            projection
                .completed
                .remove(&crate::conversation::message_id_of(message))
        })
        .collect())
}

#[allow(clippy::too_many_lines)] // One shared finite fold; no second Journal scan for lineage provenance.
fn project(
    store: &dyn ConversationStore,
    wanted: &BTreeSet<MessageId>,
    through: u64,
) -> Result<ResponseProjection, ConversationStoreError> {
    let mut attempts: BTreeMap<AttemptId, AttemptEvidence> = BTreeMap::new();
    let mut completed: BTreeMap<_, _> = store
        .load_inherited_responses()?
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
    let mut processes: BTreeMap<_, _> = completed
        .values()
        .flat_map(|response| {
            response
                .process_message_ids
                .iter()
                .filter(|id| wanted.contains(*id))
                .map(|id| {
                    (
                        id.clone(),
                        CompletedProcessView {
                            origin: response.origin.clone(),
                            final_message_id: response.closing_message_id.clone(),
                        },
                    )
                })
        })
        .collect();
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
                RuntimeEvent::AttemptStarted { .. } => {
                    evidence.started_at = Some(event.timestamp);
                }
                RuntimeEvent::ModelRequestStarted { request_id, .. } => {
                    evidence.timing.start(request_id.clone());
                    evidence.last_request = Some(request_id);
                    evidence.requests += 1;
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
                    if wanted.contains(&message_id) {
                        evidence.members.insert(message_id.clone());
                    }
                    evidence.closing = Some(message_id);
                }
                RuntimeEvent::AttemptCompleted {
                    finish_reason: ModelFinishReason::Stop | ModelFinishReason::Refusal,
                    ..
                } => {
                    if let Some(closing) = evidence.closing.take() {
                        statistics.completed_responses += 1;
                        let process = CompletedProcessView {
                            origin: ResponseOrigin {
                                conversation_id: store.conversation_id().clone(),
                                attempt_id: id.clone(),
                                closing_message_id: closing.clone(),
                            },
                            final_message_id: closing.clone(),
                        };
                        for member in &evidence.members {
                            processes.insert(member.clone(), process.clone());
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
                                    process_message_ids: evidence.members.iter().cloned().collect(),
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
        .flat_map(|evidence| evidence.members.iter().cloned())
        .collect();
    Ok(ResponseProjection {
        completed,
        processes,
        pending,
        statistics,
    })
}

#[cfg(test)]
pub(crate) mod tests;
