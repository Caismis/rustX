//! Finite process facts over the original canonical/Journal membership.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::sqlite::statements::FixedStatements;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(
        value,
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .ok_or_else(|| storage("native member header exceeds bound"))
}
pub(super) fn members(
    connection: &Connection,
    attempt: &crate::runtime::identity::AttemptId,
    through: u64,
) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
    let missing: bool = connection.query_row_cached(
        "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_member_holes_idx WHERE attempt_id=?1 AND sequence<=?2 AND observation_members_required=1 AND observation_members IS NULL)",
        params![attempt.as_str(), through], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage("native member header unavailable"));
    }
    let bytes: Option<Vec<u8>> = connection
        .query_row_cached(
            super::event_heads::MEMBERS,
            params![attempt.as_str(), through],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    bytes.map_or_else(
        || Ok(crate::durable::observation::AttemptMemberCounts::default()),
        |bytes| serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string())),
    )
}
pub(super) fn requires_members(event: &RuntimeEventEnvelope) -> bool {
    event.attempt_id.is_some()
        && matches!(
            event.event,
            RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
        )
}
fn process_event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    previous: Option<crate::durable::observation::MemberProcessHeader>,
) -> Result<crate::durable::observation::MemberProcessHeader, ConversationStoreError> {
    use crate::durable::reading::TurnProcessOutcome as Outcome;
    let attempt = event
        .attempt_id
        .as_ref()
        .ok_or_else(|| storage("member owner absent"))?;
    let mut owner = if let Some(previous) = previous {
        previous
    } else {
        let started: Option<(u64, String)> = connection.query_row_cached(
            "SELECT sequence,event_timestamp FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='attempt_started' AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",
            params![attempt.as_str(), event.sequence], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|error|storage(error.to_string()))?;
        crate::durable::observation::MemberProcessHeader {
            conversation_id: event.conversation_id.clone(),
            attempt_id: (attempt.as_str().len() <= 512).then(|| attempt.clone()),
            attempt_id_source: if attempt.as_str().len() > 512 {
                Some(super::observation_identity::journal(
                    started
                        .as_ref()
                        .map_or(event.sequence, |(sequence, _)| *sequence),
                    attempt,
                )?)
            } else {
                None
            },
            event_id: None,
            outcome: Outcome::Running,
            started_at: started
                .as_ref()
                .map(|(_, timestamp)| {
                    chrono::DateTime::parse_from_rfc3339(timestamp)
                        .map(|value| value.with_timezone(&chrono::Utc))
                })
                .transpose()
                .map_err(|_| storage("native member clock is corrupt"))?,
            ended_at: None,
            native_start: started.as_ref().map(|(sequence, _)| *sequence),
            native_terminal: None,
            owner_anchor: started
                .as_ref()
                .map_or(event.sequence, |(sequence, _)| *sequence),
        }
    };
    let outcome = match event.event {
        RuntimeEvent::AttemptCompleted { .. } => Some(Outcome::Completed),
        RuntimeEvent::AttemptCancelled { .. } => Some(Outcome::Cancelled),
        RuntimeEvent::AttemptFailed { .. } => Some(Outcome::Failed),
        RuntimeEvent::AttemptTimedOut { .. } => Some(Outcome::TimedOut),
        RuntimeEvent::AttemptLimitExceeded { .. } => Some(Outcome::LimitExceeded),
        _ => None,
    };
    if let Some(outcome) = outcome {
        owner.outcome = outcome;
        owner.event_id = (event.event_id.as_str().len() <= 256).then(|| event.event_id.clone());
        owner.ended_at = Some(event.timestamp);
        owner.native_terminal = Some(event.sequence);
    }
    Ok(owner)
}
fn member_identity(
    connection: &Connection,
    conversation: &crate::runtime::identity::ConversationId,
    counts: &mut crate::durable::observation::AttemptMemberCounts,
    message: &crate::runtime::identity::MessageId,
) -> Result<(), ConversationStoreError> {
    use crate::runtime_client::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentRepresentation, ContentSource,
    };
    let position: u64 = connection
        .query_row_cached(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message.as_str()],
            |row| row.get(0),
        )
        .map_err(|_| storage("native member canonical identity unavailable"))?;
    let locator = CanonicalContentLocator::ByLedgerPosition { position };
    let inline = message.as_str().len() <= 256;
    counts.final_message_id = inline.then(|| message.clone());
    counts.final_message_id_deferred = !inline;
    counts.final_message_id_source = if inline {
        None
    } else {
        let source = ContentSource::CanonicalMessageBlock {
            component: CanonicalContentComponent::MessageMetadata,
            locator: locator.clone(),
            block_index: crate::message::types::ContentBlockIndex::new(0),
        };
        Some(
            super::content::descriptor(
                connection,
                conversation,
                &source,
                ContentRepresentation::CanonicalJsonV1,
            )?
            .ok_or_else(|| storage("native member identity source unavailable"))?,
        )
    };
    counts.final_message_locator = Some(locator);
    Ok(())
}
pub(super) fn prepare_event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<
    Option<super::append_facts::Captured<crate::durable::observation::AttemptMemberCounts>>,
    ConversationStoreError,
> {
    if !requires_members(event) {
        return Ok(None);
    }
    let Some(attempt) = &event.attempt_id else {
        return Ok(None);
    };
    let mut counts = members(connection, attempt, event.sequence.saturating_sub(1))?;
    counts.process = Some(process_event(connection, event, counts.process.take())?);
    if let RuntimeEvent::AttemptCompleted { finish_reason, .. } = &event.event {
        counts.completed_response = counts.final_message_locator.is_some()
            && matches!(
                finish_reason,
                crate::model::ModelFinishReason::Stop | crate::model::ModelFinishReason::Refusal
            );
        return Ok(Some(super::append_facts::Captured {
            bytes: encode(&counts)?,
            value: counts,
        }));
    }
    let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event else {
        return Ok(Some(super::append_facts::Captured {
            bytes: encode(&counts)?,
            value: counts,
        }));
    };
    counts.completed_response = false;
    let header = super::observation::message(connection, message_id)?
        .ok_or_else(|| storage("committed native message header absent"))?;
    counts.message_count = counts
        .message_count
        .checked_add(1)
        .ok_or_else(|| storage("native process message count exhausted"))?;
    counts.tool_call_count = counts
        .tool_call_count
        .checked_add(header.tool_call_count)
        .ok_or_else(|| storage("native process Tool count exhausted"))?;
    let cursor:Option<u64>=connection.query_row_cached("SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",[message_id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    if let Some(cursor) = cursor {
        counts.first_cursor = Some(
            counts
                .first_cursor
                .map_or(crate::durable::TranscriptCursor::new(cursor), |previous| {
                    crate::durable::TranscriptCursor::new(previous.get().min(cursor))
                }),
        );
    }
    member_identity(connection, &event.conversation_id, &mut counts, message_id)?;
    Ok(Some(super::append_facts::Captured {
        bytes: encode(&counts)?,
        value: counts,
    }))
}
pub(super) fn seed_members(
    connection: &Connection,
    seed: &crate::durable::inbox::LineageSeed,
) -> Result<(), ConversationStoreError> {
    for (ordinal, turn) in seed.turns().iter().enumerate() {
        let mut counts = crate::durable::observation::AttemptMemberCounts {
            process: Some(crate::durable::observation::MemberProcessHeader {
                conversation_id: turn.id.conversation_id.clone(),
                attempt_id: (turn.id.attempt_id.as_str().len() <= 512)
                    .then(|| turn.id.attempt_id.clone()),
                attempt_id_source: if turn.id.attempt_id.as_str().len() > 512 {
                    Some(super::observation_identity::inherited(
                        connection,
                        crate::runtime_client::response::InheritedAttemptIdentityKind::Turn,
                        ordinal as u64,
                    )?)
                } else {
                    None
                },
                event_id: None,
                outcome: turn.outcome.clone().into(),
                started_at: turn.started_at,
                ended_at: turn.ended_at,
                native_start: None,
                native_terminal: None,
                owner_anchor: 0,
            }),
            ..Default::default()
        };
        for id in &turn.process_message_ids {
            let header = super::observation::message(connection, id)?
                .ok_or_else(|| storage("inherited native member header absent"))?;
            counts.message_count = counts
                .message_count
                .checked_add(1)
                .ok_or_else(|| storage("native member count exhausted"))?;
            counts.tool_call_count = counts
                .tool_call_count
                .checked_add(header.tool_call_count)
                .ok_or_else(|| storage("native member Tool count exhausted"))?;
            let cursor:Option<u64>=connection.query_row_cached("SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",[id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            if let Some(cursor) = cursor {
                counts.first_cursor =
                    Some(counts.first_cursor.map_or(
                        crate::durable::TranscriptCursor::new(cursor),
                        |previous| {
                            crate::durable::TranscriptCursor::new(previous.get().min(cursor))
                        },
                    ));
            }
        }
        let response = seed.completed_responses().iter().find(|response| {
            response.origin.conversation_id == turn.id.conversation_id
                && response.origin.attempt_id == turn.id.attempt_id
        });
        if let Some(response) = response {
            let conversation: String = connection
                .query_row(
                    "SELECT conversation_id FROM rustx_store WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            member_identity(
                connection,
                &crate::runtime::identity::ConversationId::new(conversation),
                &mut counts,
                &response.closing_message_id,
            )?;
            counts.completed_response = true;
        }
        connection
            .execute(
                "UPDATE inherited_turn_provenance SET observation_members=?2 WHERE ordinal=?1",
                params![ordinal, encode(&counts)?],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}
pub(super) fn message_members(
    connection: &Connection,
    message_id: &crate::runtime::identity::MessageId,
    through: u64,
) -> Result<Option<crate::durable::observation::AttemptMemberCounts>, ConversationStoreError> {
    let bytes:Option<Option<Vec<u8>>>=connection.query_row("SELECT p.observation_members FROM inherited_message_membership m JOIN inherited_turn_provenance p ON p.ordinal=m.turn_ordinal WHERE m.assistant_message_id=?1",[message_id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    if let Some(bytes) = bytes {
        let bytes = bytes.ok_or_else(|| storage("inherited member header unavailable"))?;
        return serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| storage(error.to_string()));
    }
    let attempt:Option<Option<String>>=connection.query_row("SELECT attempt_id FROM events INDEXED BY events_message_id_kind_idx WHERE json_extract(event_json,'$.event.message_id')=?1 AND json_extract(event_json,'$.event.type')='assistant_message_committed' AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![message_id.as_str(),through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    attempt
        .flatten()
        .map(|attempt| {
            members(
                connection,
                &crate::runtime::identity::AttemptId::new(attempt),
                through,
            )
        })
        .transpose()
}

pub(super) fn at_sequence(
    connection: &Connection,
    sequence: u64,
) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT observation_members FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .flatten();
    decode_members(bytes)
}

fn inherited_terminal_at(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    transcript: u64,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    let reference: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN typeof(reference_id)='text' AND length(CAST(reference_id AS BLOB))<=16 THEN reference_id END FROM transcript_order WHERE position=?1 AND position<=?2 AND reference_kind='inherited_turn'",
        params![transcript, cut.inherited_through.get()], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    let Some(reference) = reference else {
        return Ok(None);
    };
    let reference = reference.ok_or_else(|| storage("inherited terminal ordinal unavailable"))?;
    let ordinal = reference
        .parse::<u64>()
        .ok()
        .filter(|ordinal| {
            ordinal.to_string() == reference
                && ordinal.checked_add(1).is_some_and(|display| {
                    display <= crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
                })
        })
        .ok_or_else(|| storage("invalid inherited terminal ordinal"))?;
    inherited_process(
        connection,
        cut,
        ordinal,
        crate::durable::TranscriptCursor::new(transcript),
    )
}

pub(super) fn transcript_process(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    transcript: u64,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    if transcript > cut.read.transcript {
        return Err(ConversationStoreError::InvalidReference(
            "member beyond native transcript cut".into(),
        ));
    }
    let position: Option<u64> = connection.query_row(
        "SELECT position FROM message_ledger INDEXED BY message_ledger_native_transcript_idx WHERE native_transcript=?1 AND native_transcript IS NOT NULL AND position<=?2",
        params![transcript, cut.ledger_through], |row|row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?;
    let Some(mut position) = position else {
        if let Some(process) = inherited_terminal_at(connection, cut, transcript)? {
            return Ok(Some(process));
        }
        let sequence:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_transcript_idx WHERE observation_transcript=?1 AND observation_transcript IS NOT NULL AND sequence<=?2 AND observation_kind IN ('attempt_failed','attempt_cancelled','attempt_timed_out','attempt_limit_exceeded')",params![transcript,cut.read.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        return sequence
            .map(|sequence| {
                process_view(
                    connection,
                    cut,
                    at_sequence(connection, sequence)?,
                    Some(crate::durable::TranscriptCursor::new(transcript)),
                )
            })
            .transpose()
            .map(Option::flatten);
    };
    let role: Option<String> = connection.query_row(
        "SELECT json_extract(header_json,'$.role') FROM observation_headers INDEXED BY observation_headers_ledger_idx WHERE ledger_position=?1 AND ledger_position IS NOT NULL AND transcript_position=?2 AND journal_position<=?3",
        params![position, transcript, cut.read.journal], |row|row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?;
    let role = role.ok_or_else(|| storage("native member message header unavailable"))?;
    if role == "tool" {
        let owner:Option<u64>=connection.query_row("SELECT observation_ledger FROM canonical_tool_calls INDEXED BY canonical_tool_calls_result_ledger_idx WHERE result_ledger_position=?1 AND result_ledger_position IS NOT NULL",[position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        let Some(owner) = owner else {
            return Err(storage("native Tool member owner unavailable"));
        };
        position = owner;
    } else if role != "assistant" {
        return Ok(None);
    }
    let inherited: Option<Option<u64>> = connection.query_row(
        "SELECT turn_ordinal FROM inherited_message_membership WHERE assistant_ledger_position=?1",
        [position], |row|row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?;
    if transcript <= cut.inherited_through.get() && inherited.is_none() {
        return Err(storage("inherited native member relationship absent"));
    }
    if inherited.is_some() {
        super::observation_membership::validate(connection, position)?;
    }
    let counts = if let Some(ordinal) = inherited {
        let Some(ordinal) = ordinal else {
            return Ok(None);
        };
        super::observation_turn::inherited(connection, ordinal)?;
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT observation_members FROM inherited_turn_provenance WHERE ordinal=?1",
                [ordinal],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        decode_members(bytes)?
    } else {
        let missing: bool = connection.query_row_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_owner_holes_idx WHERE observation_owner_holes>0 AND observation_owner_holes IN (4,5,6,7) AND sequence<=?1)", [cut.read.journal], |row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        if missing {
            return Err(storage("native member header unavailable"));
        }
        let anchor: Option<Option<u64>> = connection.query_row("SELECT observation_member_anchor FROM events INDEXED BY events_observation_assistant_ledger_idx WHERE observation_assistant_ledger=?1 AND observation_assistant_ledger IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1", params![position,cut.read.journal], |row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        let Some(anchor) = anchor.flatten() else {
            return Ok(None);
        };
        let bytes: Option<Vec<u8>> = connection.query_row("SELECT observation_members FROM events INDEXED BY events_observation_member_anchor_idx WHERE observation_member_anchor=?1 AND observation_members IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1", params![anchor,cut.read.journal], |row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        decode_members(bytes)?
    };
    process_view(connection, cut, counts, None)
}
fn decode_members(
    bytes: Option<Vec<u8>>,
) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
    let bytes = bytes.ok_or_else(|| storage("native member header unavailable"))?;
    serde_json::from_slice(&bytes).map_err(|_| storage("native member header corrupt"))
}
pub(super) fn process_view(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    counts: crate::durable::observation::AttemptMemberCounts,
    fallback: Option<crate::durable::TranscriptCursor>,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    process_view_with_failure(cut, counts, fallback, |terminal| {
        Ok(
            super::observation_execution::summary_at_sequence(connection, terminal)?
                .ok_or_else(|| storage("native member terminal header unavailable"))?
                .failure,
        )
    })
}

/// Only the same event's pre-insert builder may supply these immutable facts.
/// Historical member reads still resolve the terminal from its stored bytes.
pub(super) fn pending_process_view(
    cut: &crate::durable::observation::ObservationReadCut,
    counts: crate::durable::observation::AttemptMemberCounts,
    cursor: crate::durable::TranscriptCursor,
    sequence: u64,
    execution: Option<&super::observation_execution::Captured>,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    process_view_with_failure(cut, counts, Some(cursor), |terminal| {
        if terminal != sequence {
            return Err(storage("native member terminal header unavailable"));
        }
        Ok(execution
            .ok_or_else(|| storage("native member terminal header unavailable"))?
            .value
            .failure
            .clone())
    })
}

fn process_view_with_failure(
    cut: &crate::durable::observation::ObservationReadCut,
    counts: crate::durable::observation::AttemptMemberCounts,
    fallback: Option<crate::durable::TranscriptCursor>,
    failure_at: impl FnOnce(
        u64,
    ) -> Result<
        Option<crate::runtime_client::event::RuntimeClientAttemptFailure>,
        ConversationStoreError,
    >,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    use crate::durable::reading::TurnProcessOutcome;
    let owner = counts
        .process
        .ok_or_else(|| storage("native member owner header unavailable"))?;
    if owner.attempt_id.is_none() && owner.attempt_id_source.is_none() {
        return Err(storage("native member owner identity source unavailable"));
    }
    let Some(cursor) = counts.first_cursor.or(fallback) else {
        return Ok(None);
    };
    if cursor.get() > cut.read.transcript {
        return Err(storage("native member seat exceeds cut"));
    }
    let failure = if let (TurnProcessOutcome::Failed, Some(terminal)) =
        (&owner.outcome, owner.native_terminal)
    {
        Some(
            failure_at(terminal)?
                .ok_or_else(|| storage("native member failure header unavailable"))?,
        )
    } else {
        None
    };
    let final_identity = counts.completed_response;
    Ok(Some(crate::runtime_client::response::TurnProcessView {
        conversation_id: owner.conversation_id,
        attempt_id: owner.attempt_id,
        attempt_id_source: owner.attempt_id_source,
        event_id: owner.event_id,
        control_cursor: cursor.into(),
        final_message_id: final_identity.then_some(counts.final_message_id).flatten(),
        final_message_locator: final_identity
            .then_some(counts.final_message_locator)
            .flatten(),
        final_message_id_deferred: final_identity && counts.final_message_id_deferred,
        final_message_id_source: final_identity
            .then_some(counts.final_message_id_source)
            .flatten(),
        message_count: u32::try_from(
            counts
                .message_count
                .saturating_sub(u64::from(final_identity)),
        )
        .map_err(|_| storage("native member count exceeds protocol"))?,
        tool_call_count: u32::try_from(counts.tool_call_count)
            .map_err(|_| storage("native Tool count exceeds protocol"))?,
        outcome: owner.outcome,
        failure,
        started_at: owner.started_at,
        ended_at: owner.ended_at,
    }))
}

pub(super) fn inherited_process(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    ordinal: u64,
    fallback: crate::durable::TranscriptCursor,
) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError> {
    let header = super::observation_turn::inherited(connection, ordinal)?;
    if header.cursor != Some(fallback) {
        return Err(storage(
            "inherited terminal seat differs from native turn header",
        ));
    }
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT observation_members FROM inherited_turn_provenance WHERE ordinal=?1",
            [ordinal],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let counts = decode_members(bytes)?;
    let owner = counts
        .process
        .as_ref()
        .ok_or_else(|| storage("inherited terminal owner unavailable"))?;
    if owner.conversation_id != header.id.conversation_id
        || owner.attempt_id != header.id.attempt_id
        || owner.attempt_id_source != header.id.attempt_id_source
    {
        return Err(storage(
            "inherited terminal owner differs from native turn header",
        ));
    }
    if counts.completed_response
        || counts.message_count != 0
        || counts.tool_call_count != 0
        || counts.first_cursor.is_some()
    {
        return Err(storage(
            "inherited terminal-only member evidence is inconsistent",
        ));
    }
    process_view(connection, cut, counts, Some(fallback))
}
