//! Exact finite request/response reading aggregates, derived in the native J transaction.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::observation::{
    AttemptExecutionSummary, AttemptModelName, AttemptModelNamePage, HEADER_PAGE_MAX,
    ObservationValueChunk,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::{AttemptId, RequestId};
use serde::{Deserialize, Serialize};
use sha2::Digest;
#[derive(Default, Serialize, Deserialize)]
struct ExecutionLayout {
    owner_anchor: u64,
    summary: AttemptExecutionSummary,
    first_request_sequence: Option<u64>,
    first_ttft: Option<u64>,
    terminals: u64,
    duration_invalid: bool,
    output_invalid: bool,
    observed_output: bool,
    generation_ms: u64,
    output_tokens: u64,
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(
        value,
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .ok_or_else(|| storage("native execution header exceeds bound"))
}
fn layout(
    connection: &Connection,
    attempt: &AttemptId,
    through: u64,
) -> Result<ExecutionLayout, ConversationStoreError> {
    let bytes:Option<Vec<u8>>=connection.query_row("SELECT observation_execution FROM events INDEXED BY events_observation_execution_idx WHERE observation_execution IS NOT NULL AND attempt_id=?1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![attempt.as_str(),through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    bytes.map_or_else(
        || Ok(ExecutionLayout::default()),
        |bytes| decode_layout(&bytes),
    )
}
pub(super) fn summary_at_sequence(
    connection: &Connection,
    sequence: u64,
) -> Result<Option<AttemptExecutionSummary>, ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT observation_execution FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?
        .flatten();
    bytes
        .map(|bytes| {
            decode_layout(&bytes)
                .map(|layout| layout.summary)
                .map_err(|error| storage(error.to_string()))
        })
        .transpose()
}
pub(super) fn summary(
    connection: &Connection,
    attempt: &AttemptId,
    through: u64,
) -> Result<AttemptExecutionSummary, ConversationStoreError> {
    Ok(layout(connection, attempt, through)?.summary)
}
pub(super) fn required(event: &RuntimeEventEnvelope) -> bool {
    event.attempt_id.is_some()
        && matches!(
            event.event,
            RuntimeEvent::AttemptStarted { .. }
                | RuntimeEvent::ModelRequestStarted { .. }
                | RuntimeEvent::ModelRequestCompleted { .. }
                | RuntimeEvent::ModelRequestFailed { .. }
                | RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
        )
}
pub(super) fn summary_selected(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<AttemptExecutionSummary, ConversationStoreError> {
    Ok(selected_layout(connection, sequence, through)?.summary)
}
fn selected_layout(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<ExecutionLayout, ConversationStoreError> {
    let selected = layout_at(connection, sequence)?;
    let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_execution_holes_idx WHERE observation_execution_required=1 AND observation_execution IS NULL AND sequence>=?1 AND sequence<=?2)",params![selected.owner_anchor,through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native execution header evidence unavailable"));
    }
    let latest:u64=connection.query_row("SELECT sequence FROM events INDEXED BY events_execution_owner_idx WHERE observation_execution_owner=?1 AND observation_execution IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![selected.owner_anchor,through],|row|row.get(0)).map_err(|_|storage("native execution owner header unavailable"))?;
    layout_at(connection, latest)
}
pub(super) fn event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let Some(attempt) = &event.attempt_id else {
        return Ok(());
    };
    if !matches!(
        event.event,
        RuntimeEvent::AttemptStarted { .. }
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
    ) {
        return Ok(());
    }
    let mut state = layout(connection, attempt, event.sequence.saturating_sub(1))?;
    if state.owner_anchor == 0 {
        state.owner_anchor = event.sequence;
    }
    match &event.event {
        RuntimeEvent::AttemptStarted { .. } => {
            state.summary.started_at = Some(event.timestamp);
        }
        RuntimeEvent::ModelRequestStarted {
            request_id, model, ..
        } => {
            request_started(connection, event, attempt, request_id, model, &mut state)?;
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
            request_settled(
                connection,
                event,
                request_id,
                usage.as_ref(),
                generation.as_ref(),
                &mut state,
            )?;
        }
        RuntimeEvent::AssistantMessageCommitted { message_id } => {
            state.summary.closing_message_id =
                (message_id.as_str().len() <= 256).then(|| message_id.clone());
            state.summary.closing_ledger_position = connection
                .query_row(
                    "SELECT position FROM message_ledger WHERE message_id=?1",
                    [message_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(error.to_string()))?;
            state.summary.closing_identity_sequence = Some(event.sequence);
            let _ = super::optional_display_layout(connection, || {
                super::content::index_utf8_field(
                    connection,
                    "response_identity",
                    &format!("journal:{}", event.sequence),
                    0,
                    message_id.as_str(),
                )
            });
        }
        RuntimeEvent::AttemptCompleted { finish_reason, .. } => {
            let (reason, deferred) = finite_finish_reason(finish_reason);
            state.summary.finish_reason = Some(reason);
            state.summary.finish_reason_deferred = deferred;
            state.summary.finish_reason_sequence = Some(event.sequence);
            close(&mut state.summary, event, "attempt_completed");
        }
        RuntimeEvent::AttemptCancelled { .. } => {
            close(&mut state.summary, event, "attempt_cancelled");
        }
        RuntimeEvent::AttemptFailed { error, .. } => {
            attempt_failed(connection, event, error, &mut state.summary);
        }
        RuntimeEvent::AttemptTimedOut { .. } => {
            close(&mut state.summary, event, "attempt_timed_out");
        }
        RuntimeEvent::AttemptLimitExceeded { .. } => {
            close(&mut state.summary, event, "attempt_limit_exceeded");
        }
        _ => {}
    }
    seal_execution(connection, event.sequence, &mut state)
}
fn seal_execution(
    connection: &Connection,
    sequence: u64,
    state: &mut ExecutionLayout,
) -> Result<(), ConversationStoreError> {
    state.summary.models_next_offset = ((state.summary.models.len() as u64)
        < state.summary.models_total)
        .then_some(state.summary.models.len() as u64);
    finish_timing(state);
    while crate::runtime_client::checkpoint::bounded_bytes(
        state,
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .is_none()
    {
        if state.summary.models.pop().is_none() {
            return Err(storage("native execution essentials exceed header bound"));
        }
        state.summary.models_next_offset = Some(state.summary.models.len() as u64);
    }
    connection
        .execute(
            "UPDATE events SET observation_execution=?2,observation_execution_owner=?3 WHERE sequence=?1",
            params![sequence, encode(state)?,state.owner_anchor],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
fn attempt_failed(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    error: &crate::events::types::AttemptFailure,
    summary: &mut AttemptExecutionSummary,
) {
    let normalized = BorrowedFailure(error);
    summary.failure_readable_end = super::optional_display_layout(connection, || {
        super::content::index_attempt_failure(connection, event.sequence, &normalized)
    })
    .unwrap_or(0);
    let (failure, deferred) = crate::runtime_client::event::finite_attempt_failure(error);
    summary.failure = Some(failure);
    summary.failure_deferred = deferred;
    close(summary, event, "attempt_failed");
}
fn request_started(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    attempt: &AttemptId,
    request_id: &RequestId,
    model: &str,
    state: &mut ExecutionLayout,
) -> Result<(), ConversationStoreError> {
    state.summary.requests = state
        .summary
        .requests
        .checked_add(1)
        .ok_or_else(|| storage("request count exhausted"))?;
    state.first_request_sequence.get_or_insert(event.sequence);
    state.summary.last_request_sequence = Some(event.sequence);
    state.summary.last_request_id = (request_id.as_str().len() <= 512).then(|| request_id.clone());
    state.summary.last_request_id_source = (state.summary.last_request_id.is_none()).then(|| {
        request_source(
            event.sequence,
            super::observation_request_summary::serialized_bytes(&request_id.as_str())
                .expect("borrowed string encodes"),
        )
    });
    let model_hash: Vec<u8> = sha2::Sha256::digest(model.as_bytes()).to_vec();
    let known: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM observation_models WHERE attempt_id=?1 AND model_hash=?2)",
            params![attempt.as_str(), model_hash],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if !known {
        let ordinal = state.summary.models_total;
        let end = super::optional_display_layout(connection, || {
            super::content::index_model_name(connection, event.sequence, model)
        })
        .unwrap_or(0);
        connection.execute("INSERT INTO observation_models(attempt_id,ordinal,first_sequence,request_alias,request_encoded_bytes,model_hash,encoded_bytes,owner_anchor) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![attempt.as_str(),ordinal,event.sequence,state.summary.last_request_id.as_ref().map(RequestId::as_str),super::observation_request_summary::serialized_bytes(&request_id.as_str())?,model_hash,end,state.owner_anchor]).map_err(|error|storage(error.to_string()))?;
        state.summary.models_total += 1;
        if ordinal == state.summary.models.len() as u64
            && ordinal < HEADER_PAGE_MAX as u64
            && end > 0
            && end <= 1024
        {
            state.summary.models.push(model.to_owned());
            if crate::runtime_client::checkpoint::bounded_bytes(&state.summary, 12_288).is_none() {
                state.summary.models.pop();
            }
        }
    }

    Ok(())
}
fn request_settled(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    request_id: &RequestId,
    usage: Option<&crate::model::ModelUsage>,
    generation: Option<&crate::model::generation_evidence::GenerationEvidence>,
    state: &mut ExecutionLayout,
) -> Result<(), ConversationStoreError> {
    state.terminals += 1;
    if let Some(usage) = usage {
        state.summary.reports += 1;
        crate::durable::response::add_usage(&mut state.summary.usage, usage);
    }
    let start:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_request_id_kind_idx WHERE json_extract(event_json,'$.event.request_id')=?1 AND json_extract(event_json,'$.event.type')='model_request_started' AND sequence<?2 ORDER BY sequence DESC LIMIT 1",params![request_id.as_str(),event.sequence],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    if state.first_request_sequence.is_some() && state.first_request_sequence == start {
        state.first_ttft = generation.and_then(
            crate::model::generation_evidence::GenerationEvidence::time_to_first_output_ms,
        );
    }
    if let Some(generation) = generation {
        if generation.first_output_ms.is_some() {
            state.observed_output = true;
            if let Some(span) = generation.generation_ms() {
                state.generation_ms = state
                    .generation_ms
                    .checked_add(span)
                    .ok_or_else(|| storage("request timing exhausted"))?;
                match usage.filter(|_| span > 0) {
                    Some(usage) => {
                        state.output_tokens = state
                            .output_tokens
                            .checked_add(usage.output_tokens)
                            .ok_or_else(|| storage("output count exhausted"))?;
                    }
                    None => state.output_invalid = true,
                }
            } else {
                state.duration_invalid = true;
                state.output_invalid = true;
            }
        } else if generation.last_output_ms.is_some() {
            state.duration_invalid = true;
            state.output_invalid = true;
        } else if usage.map(|usage| usage.output_tokens) != Some(0) {
            state.output_invalid = true;
        }
    } else {
        state.duration_invalid = true;
        state.output_invalid = true;
    }

    Ok(())
}
fn close(summary: &mut AttemptExecutionSummary, event: &RuntimeEventEnvelope, kind: &str) {
    summary.ended_at = Some(event.timestamp);
    summary.terminal_event_id =
        (event.event_id.as_str().len() <= 256).then(|| event.event_id.clone());
    summary.terminal_sequence = Some(event.sequence);
    summary.terminal_kind = Some(kind.into());
}
pub(super) fn models(
    connection: &Connection,
    attempt: &AttemptId,
    through: u64,
    offset: u64,
    limit: usize,
) -> Result<AttemptModelNamePage, ConversationStoreError> {
    if limit == 0 || limit > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let summary = summary(connection, attempt, through)?;
    if offset > summary.models_total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let mut statement=connection.prepare("SELECT ordinal,request_alias,request_encoded_bytes,first_sequence,encoded_bytes FROM observation_models WHERE attempt_id=?1 AND ordinal>=?2 AND ordinal<?3 AND first_sequence<=?4 ORDER BY ordinal LIMIT ?5").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![
                attempt.as_str(),
                offset,
                summary.models_total,
                through,
                limit
            ],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut names = Vec::with_capacity(limit);
    for row in rows {
        let (ordinal, request_id, request_encoded_bytes, source_sequence, encoded_bytes) =
            row.map_err(|error| storage(error.to_string()))?;
        if encoded_bytes == 0 {
            return Err(storage("native model name layout unavailable"));
        }
        let request_id = request_id.map(RequestId::new);
        let value = if (2..=1024).contains(&encoded_bytes) {
            Some(
                serde_json::from_slice(
                    &super::content::model_name_range(
                        connection,
                        source_sequence,
                        0,
                        encoded_bytes,
                    )?
                    .bytes,
                )
                .map_err(|error| storage(error.to_string()))?,
            )
        } else {
            None
        };
        names.push(AttemptModelName {
            ordinal,
            request_id_source: request_id
                .is_none()
                .then(|| request_source(source_sequence, request_encoded_bytes)),
            request_id,
            source_sequence,
            encoded_bytes,
            value,
        });
    }
    let next = offset
        .checked_add(names.len() as u64)
        .ok_or_else(|| storage("models offset exhausted"))?;
    Ok(AttemptModelNamePage {
        total: summary.models_total,
        offset,
        names,
        next_offset: (next < summary.models_total).then_some(next),
    })
}
pub(super) fn model_value(
    connection: &Connection,
    attempt: &AttemptId,
    through: u64,
    ordinal: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if max_bytes == 0
        || max_bytes > 65_536
        || offset > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let total = summary(connection, attempt, through)?.models_total;
    let source:Option<(u64,u64)>=connection.query_row("SELECT first_sequence,encoded_bytes FROM observation_models WHERE attempt_id=?1 AND ordinal=?2 AND ordinal<?3 AND first_sequence<=?4",params![attempt.as_str(),ordinal,total,through],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
    let (request, end) = source.ok_or_else(|| {
        ConversationStoreError::InvalidReference("model name source unavailable".into())
    })?;
    if end == 0 {
        return Err(storage("native model name read layout unavailable"));
    }
    if offset > end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let next = offset
        .checked_add(u64::from(max_bytes))
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(end);
    super::content::model_name_range(connection, request, offset, next)
}

pub(super) fn models_at(
    connection: &Connection,
    sequence: u64,
    through: u64,
    offset: u64,
    limit: usize,
) -> Result<AttemptModelNamePage, ConversationStoreError> {
    if limit == 0 || limit > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let state = selected_layout(connection, sequence, through)?;
    let summary = state.summary;
    if offset > summary.models_total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let mut statement=connection.prepare("SELECT ordinal,request_alias,request_encoded_bytes,first_sequence,encoded_bytes FROM observation_models WHERE owner_anchor=?1 AND ordinal>=?2 AND ordinal<?3 AND first_sequence<=?4 ORDER BY ordinal LIMIT ?5").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(
            params![
                state.owner_anchor,
                offset,
                summary.models_total,
                through,
                limit
            ],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .map_err(|error| storage(error.to_string()))?;
    let mut names = Vec::with_capacity(limit);
    for row in rows {
        let (ordinal, request_id, request_encoded_bytes, source_sequence, encoded_bytes) =
            row.map_err(|error| storage(error.to_string()))?;
        if encoded_bytes == 0 {
            return Err(storage("native model name layout unavailable"));
        }
        let request_id = request_id.map(RequestId::new);
        let value = if (2..=1024).contains(&encoded_bytes) {
            Some(
                serde_json::from_slice(
                    &super::content::model_name_range(
                        connection,
                        source_sequence,
                        0,
                        encoded_bytes,
                    )?
                    .bytes,
                )
                .map_err(|error| storage(error.to_string()))?,
            )
        } else {
            None
        };
        names.push(AttemptModelName {
            ordinal,
            request_id_source: request_id
                .is_none()
                .then(|| request_source(source_sequence, request_encoded_bytes)),
            request_id,
            source_sequence,
            encoded_bytes,
            value,
        });
    }
    let next = offset
        .checked_add(names.len() as u64)
        .ok_or_else(|| storage("models offset exhausted"))?;
    Ok(AttemptModelNamePage {
        total: summary.models_total,
        offset,
        names,
        next_offset: (next < summary.models_total).then_some(next),
    })
}
pub(super) fn model_value_at(
    connection: &Connection,
    sequence: u64,
    through: u64,
    ordinal: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if max_bytes == 0
        || max_bytes > 65_536
        || offset > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let state = selected_layout(connection, sequence, through)?;
    let total = state.summary.models_total;
    let source:Option<(u64,u64)>=connection.query_row("SELECT first_sequence,encoded_bytes FROM observation_models WHERE owner_anchor=?1 AND ordinal=?2 AND ordinal<?3 AND first_sequence<=?4",params![state.owner_anchor,ordinal,total,through],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
    let (request, end) = source.ok_or_else(|| {
        ConversationStoreError::InvalidReference("model name source unavailable".into())
    })?;
    if end == 0 {
        return Err(storage("native model name read layout unavailable"));
    }
    if offset > end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let next = offset
        .checked_add(u64::from(max_bytes))
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(end);
    super::content::model_name_range(connection, request, offset, next)
}

struct BorrowedFailure<'a>(&'a crate::events::types::AttemptFailure);
impl schemars::JsonSchema for BorrowedFailure<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RuntimeClientAttemptFailure".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <crate::runtime_client::event::RuntimeClientAttemptFailure as schemars::JsonSchema>::json_schema(generator)
    }
}
impl Serialize for BorrowedFailure<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self.0 {
            crate::events::types::AttemptFailure::Model { error } => {
                let mut value = serializer.serialize_struct("RuntimeClientAttemptFailure", 4)?;
                value.serialize_field("type", "model")?;
                value.serialize_field("kind", &error.kind)?;
                value.serialize_field("message", &error.message)?;
                value.serialize_field("retry_after_ms", &error.retry_after_ms)?;
                value.end()
            }
            crate::events::types::AttemptFailure::Runtime { error } => {
                let mut value = serializer.serialize_struct("RuntimeClientAttemptFailure", 2)?;
                value.serialize_field("type", "runtime")?;
                value.serialize_field("error", error)?;
                value.end()
            }
        }
    }
}
fn request_source(
    sequence: u64,
    encoded_bytes: u64,
) -> crate::runtime_client::snapshot::JournalFieldRef {
    crate::runtime_client::snapshot::JournalFieldRef {
        sequence,
        path: vec!["event".into(), "request_id".into()],
        encoded_bytes,
    }
}
pub(super) fn failure_value(
    connection: &Connection,
    attempt: &AttemptId,
    through: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if max_bytes == 0
        || max_bytes > 65_536
        || offset > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let summary = summary(connection, attempt, through)?;
    let event = summary
        .terminal_sequence
        .filter(|_| summary.failure.is_some())
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference("failure diagnostic source unavailable".into())
        })?;
    if offset > summary.failure_readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(summary.failure_readable_end);
    super::content::attempt_failure_range(connection, event, offset, end)
}

pub(super) fn failure_at(
    connection: &Connection,
    sequence: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if max_bytes == 0
        || max_bytes > 65_536
        || offset > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let summary = layout_at(connection, sequence)?.summary;
    let event = summary
        .terminal_sequence
        .filter(|_| summary.failure.is_some())
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference("failure diagnostic source unavailable".into())
        })?;
    if offset > summary.failure_readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(summary.failure_readable_end);
    super::content::attempt_failure_range(connection, event, offset, end)
}

pub(super) fn finite_finish_reason(
    reason: &crate::model::ModelFinishReason,
) -> (crate::model::ModelFinishReason, bool) {
    if let crate::model::ModelFinishReason::Other { reason } = reason {
        let mut end = reason.len().min(256);
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        (
            crate::model::ModelFinishReason::Other {
                reason: reason[..end].into(),
            },
            end < reason.len(),
        )
    } else {
        (reason.clone(), false)
    }
}

fn finish_timing(state: &mut ExecutionLayout) {
    if let Some(end) = state.summary.ended_at {
        let total_duration_ms = state
            .summary
            .started_at
            .filter(|start| *start <= end)
            .and_then(|start| {
                end.signed_duration_since(start)
                    .num_milliseconds()
                    .try_into()
                    .ok()
            });
        let duration = (state.terminals == state.summary.requests
            && !state.duration_invalid
            && state.observed_output)
            .then_some(state.generation_ms);
        #[allow(clippy::cast_precision_loss)]
        let output_tokens_per_second = duration
            .filter(|duration| *duration > 0 && !state.output_invalid)
            .map(|duration| state.output_tokens as f64 * 1000.0 / duration as f64);
        let timing = crate::durable::response::CompletedResponseTiming {
            total_duration_ms,
            ttft_ms: state.first_ttft,
            generation_ms: duration,
            output_tokens_per_second,
        };
        state.summary.timing = (timing
            != crate::durable::response::CompletedResponseTiming::default())
        .then_some(timing);
    }
}

fn layout_at(
    connection: &Connection,
    sequence: u64,
) -> Result<ExecutionLayout, ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT observation_execution FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    decode_layout(&bytes.ok_or_else(|| storage("native execution header unavailable"))?)
}

fn decode_layout(bytes: &[u8]) -> Result<ExecutionLayout, ConversationStoreError> {
    if bytes.len() > crate::durable::observation::HEADER_ENCODED_MAX {
        return Err(storage("native execution header exceeds bound"));
    }
    serde_json::from_slice(bytes).map_err(|_| storage("native execution header corrupt"))
}
