//! One native lineage linearization point, never a destination execution replay.
use super::{Connection, ConversationStoreError, OptionalExtension, decode, seq_to_i64, storage};
use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::durable::reading::LineageReadCut;
use crate::durable::response::{
    AttemptEvidence, CompletedResponseProvenance, ResponseOrigin, add_usage,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{InboundKind, MessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn read(
    connection: &Connection,
    conversation: &ConversationId,
    revision: SurfaceRevision,
) -> Result<LineageReadCut, ConversationStoreError> {
    // This first SELECT linearizes temporal cut C. Selected revision R below
    // owns structure, not execution time; the same R at a newer C may observe
    // newer outcomes. Later commits cannot alter this captured (R, C).
    let read_cut = super::reading::cut(connection, conversation)?;
    let (ids, surface_history) = super::replay_surface_history(connection, revision)?;
    let referenced: BTreeSet<_> = surface_history
        .iter()
        .flat_map(SurfaceOp::message_ids)
        .cloned()
        .collect();
    let canonical: Vec<_> = super::load_canonical_rows(connection)?
        .into_iter()
        .filter(|message| referenced.contains(&crate::conversation::message_id_of(message)))
        .collect();
    let messages = ids
        .iter()
        .map(|id| super::load_message(connection, id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut turns = super::reading::lineage_turns(connection, conversation, read_cut.journal)?;
    // Member presence selects message-backed history. A terminal-only predecessor
    // controls structural prefix membership only AFTER the Journal cut filtered time.
    turns.retain_mut(|turn| {
        if turn.prompt_message_id.as_ref().is_some_and(|id| !referenced.contains(id)) {
            turn.prompt_message_id = None;
        }
        let had_members = !turn.process_message_ids.is_empty();
        turn.process_message_ids
            .retain(|id| referenced.contains(id));
        if had_members {
            return !turn.process_message_ids.is_empty();
        }
        turn.preceding_message_id
            .as_ref()
            .is_none_or(|id| referenced.contains(id))
    });
    let completed_responses = responses(connection, conversation, read_cut.journal, &canonical)?;
    Ok(LineageReadCut {
        read_cut,
        conversation_id: conversation.clone(),
        surface_revision: revision,
        messages,
        canonical,
        surface_history,
        completed_responses,
        turns,
    })
}

#[allow(clippy::too_many_lines)] // One native response evidence fold under the same SQLite snapshot.
fn responses(
    connection: &Connection,
    conversation: &ConversationId,
    through: u64,
    canonical: &[MessageBlock],
) -> Result<Vec<CompletedResponseProvenance>, ConversationStoreError> {
    let wanted: BTreeSet<_> = canonical
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(assistant.id.clone()),
            _ => None,
        })
        .collect();
    let inherited: Option<String> = connection
        .query_row(
            "SELECT response_provenance FROM bootstrap_identity WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let mut completed: BTreeMap<MessageId, CompletedResponseProvenance> = inherited
        .map(|json| decode::<Vec<CompletedResponseProvenance>>(&json, "lineage responses"))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .filter(|response| wanted.contains(&response.closing_message_id))
        .map(|response| (response.closing_message_id.clone(), response))
        .collect();
    let mut attempts: BTreeMap<AttemptId, AttemptEvidence> = BTreeMap::new();
    let mut statement = connection.prepare("SELECT event_json FROM events WHERE sequence<=?1 AND json_extract(event_json,'$.event.type') IN ('attempt_started','assistant_message_committed','model_request_started','model_request_completed','model_request_failed','attempt_completed','attempt_cancelled','attempt_failed','attempt_timed_out','attempt_limit_exceeded') ORDER BY sequence")
        .map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map([seq_to_i64(through)?], |row| row.get::<_, String>(0))
        .map_err(|error| storage(error.to_string()))?;
    for json in rows {
        let event: RuntimeEventEnvelope = decode(
            &json.map_err(|error| storage(error.to_string()))?,
            "lineage response fact",
        )?;
        let Some(id) = event.attempt_id else {
            continue;
        };
        let evidence = attempts.entry(id.clone()).or_default();
        match event.event {
            RuntimeEvent::AttemptStarted { .. } => evidence.started_at = Some(event.timestamp),
            RuntimeEvent::AssistantMessageCommitted { message_id } => {
                evidence.members.insert(message_id.clone());
                evidence.closing = Some(message_id);
            }
            RuntimeEvent::ModelRequestStarted { request_id, .. } => {
                evidence.timing.start(request_id.clone());
                evidence.last_request = Some(request_id);
                evidence.requests += 1;
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
                    add_usage(&mut evidence.usage, &usage);
                }
            }
            RuntimeEvent::AttemptCompleted {
                finish_reason: ModelFinishReason::Stop | ModelFinishReason::Refusal,
                ..
            } => {
                if let Some(closing) = evidence
                    .closing
                    .take()
                    .filter(|closing| wanted.contains(closing))
                {
                    let retry_message_id = if let Some(request) = &evidence.last_request {
                        let snapshot =
                            super::load_request_snapshot(connection, conversation, request)?;
                        super::reconstruct_surface(connection, snapshot.surface_revision)?
                            .iter()
                            .rev()
                            .map(|id| super::load_message(connection, id))
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .find_map(|message| match message {
                                MessageBlock::User(user) if user.kind == InboundKind::Message => {
                                    Some(user.id)
                                }
                                _ => None,
                            })
                    } else {
                        None
                    };
                    completed.insert(
                        closing.clone(),
                        CompletedResponseProvenance {
                            process_message_ids: evidence
                                .members
                                .iter()
                                .filter(|id| wanted.contains(*id))
                                .cloned()
                                .collect(),
                            closing_message_id: closing.clone(),
                            origin: ResponseOrigin {
                                conversation_id: conversation.clone(),
                                attempt_id: id.clone(),
                                closing_message_id: closing,
                            },
                            completed_at: event.timestamp,
                            retry_message_id,
                            timing: evidence
                                .timing
                                .summary(evidence.started_at, event.timestamp),
                            usage: (evidence.requests > 0 && evidence.requests == evidence.reports)
                                .then(|| evidence.usage.take())
                                .flatten(),
                        },
                    );
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
    Ok(canonical
        .iter()
        .filter_map(|message| completed.remove(&crate::conversation::message_id_of(message)))
        .collect())
}
