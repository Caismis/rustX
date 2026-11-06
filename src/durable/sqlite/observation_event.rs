//! Finite witnesses and exact field ranges derived in the existing native J transaction.
use super::{Connection, ConversationStoreError, OptionalExtension, params, sqlite_error, storage};
use crate::durable::observation::{
    ObservationEventField, ObservationEventWitness, ObservationValueChunk, RequestObservationPage,
};
use crate::durable::sqlite::statements::FixedStatements;
use crate::events::types::RuntimeEventEnvelope;
mod inline;
mod storage;
pub(super) fn encode_witness(
    value: &ObservationEventWitness,
) -> Result<Vec<u8>, ConversationStoreError> {
    storage::encode(value)
}
pub(super) type Captured = super::append_facts::Captured<ObservationEventWitness>;
pub(super) struct Prepared {
    pub(super) available: bool,
    pub(super) witness: Option<Captured>,
}
const KIND: &str = "oj";
const TABLE: &str = "state_observation_fields";
fn key(sequence: u64) -> String {
    format!("journal/{sequence}")
}
fn paths(selected: Option<&[&str]>) -> Vec<Vec<String>> {
    selected
        .unwrap_or(&[
            "type",
            "operation_id",
            "tool_call_id",
            "tool_id",
            "progress",
            "error",
            "reason",
            "finish_reason",
            "limit",
            "generation",
            "checkpoint_id",
            "activation_id",
            "tokens_before",
            "estimated_tokens_after",
            "occupancy",
            "tools_completed",
            "result.status",
            "result.duration_ms",
            "result.exit_code",
            "result.truncation",
            "result.managed_output",
            "result.artifacts",
            "result.deliveries",
            "result.content",
        ])
        .iter()
        .map(|path| {
            std::iter::once("event".into())
                .chain(path.split('.').map(str::to_owned))
                .collect()
        })
        .collect()
}
// Only these closed native variants can use the smaller field family.
// Other variants retain the complete existing allowlist and root-page path.
fn selected_paths(event: &crate::events::types::RuntimeEvent) -> Option<&'static [&'static str]> {
    use crate::events::types::RuntimeEvent as Event;
    match event {
        Event::AttemptStarted { .. }
        | Event::AttemptTimedOut { .. }
        | Event::TurnStarted
        | Event::TurnCompleted
        | Event::ModelRequestStarted { .. }
        | Event::ContextContributionEmitted { .. }
        | Event::InboundTurnAdopted { .. }
        | Event::AssistantMessageCommitted { .. }
        | Event::ModelRetryScheduled { .. } => Some(&["type"]),
        Event::AttemptCompleted { .. } => Some(&["type", "finish_reason"]),
        // Owner, identity and usage come from the committing typed fact below;
        // probing their indexed previews again adds no evidence.
        Event::ModelRequestCompleted { .. } => Some(&["type", "finish_reason", "generation"]),
        Event::ModelRequestFailed { .. } => Some(&["type", "error", "generation"]),
        Event::AttemptFailed { .. } => Some(&["type", "error"]),
        Event::AuxiliaryCallStarted { .. } => Some(&[
            "type",
            "call_id",
            "purpose",
            "trigger",
            "business_source",
            "input",
            "semantic_input_fingerprint",
            "invocation_fingerprint",
            "selected_message_ids",
        ]),
        Event::AuxiliaryCallSettled { .. } => {
            Some(&["type", "call_id", "outcome", "request_evidence"])
        }
        Event::ToolExecutionProgress { .. } => {
            Some(&["type", "tool_call_id", "tool_id", "progress"])
        }
        _ => None,
    }
}
#[cfg(test)]
thread_local! {static PREVIEW_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn take_preview_work() -> (usize, usize) {
    PREVIEW_WORK.with(|work| work.replace((0, 0)))
}
fn insert(
    value: &mut serde_json::Map<String, serde_json::Value>,
    path: &[String],
    field: serde_json::Value,
) {
    if path.len() == 1 {
        value.insert(path[0].clone(), field);
        return;
    }
    let target = value
        .entry(path[0].clone())
        .or_insert_with(|| serde_json::json!({}));
    if !target.is_object() {
        *target = serde_json::json!({});
    }
    if let Some(target) = target.as_object_mut() {
        insert(target, &path[1..], field);
    }
}
/// A preview carries semantic discriminants and scalar facts, never a claim
/// that a deferred opaque body has been completely materialized.
fn semantic_preview(value: serde_json::Value) -> (serde_json::Value, bool) {
    fn trim(value: serde_json::Value, nodes: &mut usize) -> (serde_json::Value, bool) {
        if *nodes >= 32 {
            return (serde_json::Value::Null, true);
        }
        *nodes += 1;
        match value {
            serde_json::Value::String(text) if text.len() > 256 => {
                let mut end = 256;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                (serde_json::Value::String(text[..end].to_owned()), true)
            }
            serde_json::Value::Array(items) => {
                let mut changed = items.len() > 8;
                let values = items
                    .into_iter()
                    .take(8)
                    .map(|value| {
                        let (value, trimmed) = trim(value, nodes);
                        changed |= trimmed;
                        value
                    })
                    .collect();
                (serde_json::Value::Array(values), changed)
            }
            serde_json::Value::Object(items) => {
                let mut changed = items.len() > 16;
                let values = items
                    .into_iter()
                    .take(16)
                    .map(|(key, value)| {
                        let (value, trimmed) = trim(value, nodes);
                        changed |= trimmed;
                        (key, value)
                    })
                    .collect();
                (serde_json::Value::Object(values), changed)
            }
            value => (value, false),
        }
    }
    trim(value, &mut 0)
}

fn result_position(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Option<u64>, ConversationStoreError> {
    let (crate::events::types::RuntimeEvent::ToolMessageCommitted { message_id, .. }
    | crate::events::types::RuntimeEvent::AssistantMessageCommitted { message_id }) = &event.event
    else {
        return Ok(None);
    };
    connection
        .query_row_cached(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, error.to_string()))
}
fn prefix(text: &str) -> &str {
    let mut end = text.len().min(256);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
/// Derive's internal tag is emitted first. Stop serialization at its first
/// separator, before any opaque value can be visited or copied.
pub(super) fn native_kind(value: &impl serde::Serialize) -> Result<String, ConversationStoreError> {
    struct Tag(Vec<u8>);
    impl std::io::Write for Tag {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len() + bytes.len() > 128 || bytes.contains(&b',') {
                return Err(std::io::Error::other("native tag captured"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Tag(Vec::with_capacity(128));
    let _ = serde_json::to_writer(&mut writer, value);
    if !writer.0.ends_with(b"}") {
        writer.0.push(b'}');
    }
    let value: serde_json::Value =
        serde_json::from_slice(&writer.0).map_err(|error| storage(error.to_string()))?;
    value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| storage("native tag missing"))
}
fn tool_status(status: &crate::tools::types::ToolExecutionStatus) -> serde_json::Value {
    use crate::tools::types::ToolExecutionStatus as Status;
    match status {
        Status::Success => serde_json::json!({"type":"success"}),
        Status::Failed { error } => serde_json::json!({"type":"failed","error":prefix(error)}),
        Status::Denied { reason } => serde_json::json!({"type":"denied","reason":prefix(reason)}),
        Status::Cancelled { reason, phase } => {
            serde_json::json!({"type":"cancelled","reason":reason,"phase":phase})
        }
        Status::TimedOut => serde_json::json!({"type":"timed_out"}),
        Status::OutcomeUnknown { detail } => {
            serde_json::json!({"type":"outcome_unknown","detail":prefix(detail)})
        }
    }
}
fn model_error(error: &crate::model::error::ModelError) -> serde_json::Value {
    serde_json::json!({"kind":error.kind,"message":prefix(&error.message),"retry_disposition":error.retry_disposition,"retry_after_ms":error.retry_after_ms.map(|value|value.to_string()),"provider_code":error.provider_code.as_deref().map(prefix),"context_overflow":crate::observation::bounded::structural_value(&error.context_overflow),"malformed_tool_proposal":crate::observation::bounded::structural_value(&error.malformed_tool_proposal),"timeout_phase":error.timeout_phase,"generation":crate::observation::bounded::structural_value(&error.generation)})
}
fn terminal_fields(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    value: &mut serde_json::Value,
    execution: Option<&super::observation_execution::Captured>,
    pending: bool,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent as Event;
    identity_fields(event, value)?;
    match &event.event {
        Event::AttemptCompleted { finish_reason, .. }
        | Event::ModelRequestCompleted { finish_reason, .. } => {
            value["finish_reason"] = serde_json::to_value(
                super::observation_execution::finite_finish_reason(finish_reason).0,
            )
            .map_err(|error| storage(error.to_string()))?;
        }
        Event::AttemptCancelled { reason, .. } | Event::WorkflowCancelled { reason, .. } => {
            value["reason"] = serde_json::json!(reason);
        }
        Event::AttemptLimitExceeded { limit, .. } => value["limit"] = serde_json::json!(limit),
        Event::AttemptFailed {
            error: crate::events::types::AttemptFailure::Model { error },
            ..
        } => value["error"] = serde_json::json!({"type":"model","error":model_error(error)}),
        Event::AttemptFailed { .. } => {
            if let Some(attempt) = &event.attempt_id {
                let summary = if pending && let Some(execution) = execution {
                    execution.value.clone()
                } else {
                    super::observation_execution::summary_prepared(
                        connection,
                        attempt,
                        event.sequence,
                        execution,
                    )?
                };
                if let Some(crate::observation::failure::RuntimeClientAttemptFailure::Runtime {
                    error,
                }) = summary.failure
                {
                    value["error"] = serde_json::json!({"type":"runtime","error":error});
                }
            }
        }
        Event::ModelRequestFailed { error, .. } => value["error"] = model_error(error),
        Event::ToolExecutionCompleted { result, .. } => {
            value["result"] = serde_json::json!({"status":tool_status(&result.status),"duration_ms":result.duration_ms.to_string(),"exit_code":result.exit_code,"truncation":crate::observation::bounded::structural_value(&result.truncation),"content":[],"artifacts":[],"deliveries":[]});
        }
        Event::ToolExecutionFailed { error, .. } | Event::CompactionFailed { error, .. } => {
            value["error"] = serde_json::json!(prefix(error));
        }
        Event::WorkflowFailed {
            diagnostic, status, ..
        } => {
            value["diagnostic"] = serde_json::json!(prefix(diagnostic));
            value["status"] = tool_status(status);
        }
        Event::SubagentTerminalPublished {
            state,
            physical_settlement_proven,
            ..
        }
        | Event::SubagentTerminalSettled {
            state,
            physical_settlement_proven,
            ..
        } => {
            value["state"] = serde_json::json!(state);
            value["physical_settlement_proven"] = serde_json::json!(physical_settlement_proven);
        }
        Event::BackgroundTerminalPublished { state, .. } => {
            value["state"] = serde_json::json!(state);
        }
        Event::CompactionCompleted {
            generation,
            checkpoint_id,
            activation_id,
            tokens_before,
            estimated_tokens_after,
            occupancy,
            ..
        } => {
            value["generation"] = serde_json::json!(generation.to_string());
            value["checkpoint_id"] = serde_json::json!(checkpoint_id);
            value["activation_id"] = serde_json::json!(activation_id.0.to_string());
            value["tokens_before"] = crate::observation::bounded::structural_value(tokens_before)
                .unwrap_or(serde_json::Value::Null);
            value["estimated_tokens_after"] = serde_json::json!(estimated_tokens_after.to_string());
            value["occupancy"] = crate::observation::bounded::structural_value(occupancy)
                .unwrap_or(serde_json::Value::Null);
        }
        _ => {}
    }
    Ok(())
}
fn mandatory_capture(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    execution: Option<&super::observation_execution::Captured>,
    pending: Option<&super::append_facts::PendingEvent<'_>>,
) -> Result<(bool, Captured), ConversationStoreError> {
    let event_kind = native_kind(&event.event)?;
    let mut value = serde_json::json!({"type":event_kind});
    terminal_fields(connection, event, &mut value, execution, pending.is_some())?;
    let occupancy =
        super::observation_occupancy::witness_with_pending(connection, event, pending.is_some())?;
    let missing = matches!(
        event.event,
        crate::events::types::RuntimeEvent::CompactionCompleted { .. }
    ) && occupancy.is_none();
    let witness = ObservationEventWitness {
        occupancy,
        native_direct: false,
        trace_total: if let Some(pending) = pending {
            pending_trace_total(connection, event, pending)?
        } else {
            super::optional_display_value(super::observation_statistics::trace_total(
                connection,
                event.sequence,
            ))?
        },
        // The committing row starts unavailable. Its final marker is ORed
        // into this witness by the reader after all auxiliary writes finish.
        source_unavailable: missing,
        transcript_cursor: pending.and_then(|pending| pending.transcript_cursor),
        sequence: event.sequence,
        event_id: (event.event_id.as_str().len() <= 256).then(|| event.event_id.clone()),
        attempt_id: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        turn_id: event
            .turn_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        owner: event
            .event
            .model_request_owner()
            .map(crate::durable::observation::RequestOwnerKind::from),
        auxiliary_call_id: event.event.auxiliary_call_id().cloned(),
        timestamp: event.timestamp,
        event_kind,
        tool_occurrence: event.tool_occurrence.clone(),
        result_ledger_position: result_position(connection, event)?,
        value,
        fields: Vec::new(),
        deferred: true,
    };
    let bytes = encode_witness(&witness)?;
    Ok((
        !missing,
        Captured {
            bytes,
            value: witness,
        },
    ))
}

fn mandatory_witness(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    execution: Option<&super::observation_execution::Captured>,
) -> Result<(bool, Captured), ConversationStoreError> {
    let (available, witness) = mandatory_capture(connection, event, execution, None)?;
    connection
        .prepare_cached("UPDATE events SET observation_witness=?2 WHERE sequence=?1")
        .and_then(|mut statement| statement.execute(params![event.sequence, witness.bytes]))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok((available, witness))
}

/// The layout is observation-only. A failed derived encoding cannot fabricate
/// an execution failure; its exact J source stays typed unavailable instead.
pub(super) fn index_optional(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    native_json: &[u8],
    execution: Option<&super::observation_execution::Captured>,
) -> Result<Prepared, ConversationStoreError> {
    if let Some((available, witness)) = super::optional_display_layout(connection, || {
        #[cfg(test)]
        let _probe =
            super::start_probe::for_event(event, super::AuxiliaryStartProbe::ObservationFields);
        index_with_paths(
            connection,
            event,
            selected_paths(&event.event),
            Some(native_json),
            execution,
        )
    })? {
        return Ok(Prepared {
            available,
            witness: Some(witness),
        });
    }
    // Keep finite own terminal facts when full field indexing fails.
    let witness = super::optional_display_layout(connection, || {
        mandatory_witness(connection, event, execution)
    })?
    .map(|(_, witness)| witness);
    Ok(Prepared {
        available: false,
        witness,
    })
}

/// The original native T assignment is already committed to this transaction.
/// Enriching its derived witness cannot refuse that original terminal fact.
pub(super) fn attach_transcript(
    connection: &Connection,
    sequence: u64,
    cursor: crate::durable::TranscriptCursor,
) -> Result<(), ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row_cached(
            "SELECT observation_witness FROM events WHERE sequence=?1 AND length(observation_witness)<=49152",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?
        .flatten();
    let bytes = bytes.ok_or_else(|| storage("native transcript witness unavailable"))?;
    let mut witness = storage::decode(&bytes)?;
    let updated = storage::attach(&mut witness, cursor)?;
    let rows = connection
        .execute_cached(
            "UPDATE events SET observation_witness=?1 WHERE sequence=?2 AND observation_witness=?3",
            params![updated, sequence, bytes],
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    if rows != 1 {
        return Err(storage("native transcript witness unavailable"));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn index(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    let native_json = serde_json::to_vec(event).map_err(|error| storage(error.to_string()))?;
    index_with_paths(
        connection,
        event,
        selected_paths(&event.event),
        Some(&native_json),
        None,
    )
    .map(|(available, _)| available)
}
#[cfg(test)]
pub(super) fn index_legacy(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    index_with_paths(connection, event, None, None, None).map(|(available, _)| available)
}
#[cfg(test)]
pub(super) fn index_segmented_for_test(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<bool, ConversationStoreError> {
    index_with_paths(connection, event, selected_paths(&event.event), None, None)
        .map(|(available, _)| available)
}
pub(super) struct SourceCapture {
    pub(super) available: bool,
    pub(super) inline_source: Option<Vec<u8>>,
    pub(super) witness: Captured,
}
fn pending_trace_total(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    pending: &super::append_facts::PendingEvent<'_>,
) -> Result<Option<u64>, ConversationStoreError> {
    if super::observation_statistics::is_trace_anchor(&event.event) {
        Ok(pending
            .statistics
            .and_then(|statistics| statistics.trace_total))
    } else {
        super::optional_display_value(super::observation_statistics::trace_total(
            connection,
            event.sequence.saturating_sub(1),
        ))
    }
}

pub(super) fn prepare_pending(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    native_json: &[u8],
    pending: &super::append_facts::PendingEvent<'_>,
) -> Result<Option<SourceCapture>, ConversationStoreError> {
    let captured = super::optional_display_layout(connection, || {
        #[cfg(test)]
        let _probe =
            super::start_probe::for_event(event, super::AuxiliaryStartProbe::ObservationFields);
        capture_with_paths(
            connection,
            event,
            selected_paths(&event.event),
            Some(native_json),
            pending.execution,
            Some(pending),
        )
    })?;
    if captured.is_some() {
        return Ok(captured);
    }
    super::optional_display_layout(connection, || {
        let (_, witness) = mandatory_capture(connection, event, pending.execution, Some(pending))?;
        Ok(SourceCapture {
            available: false,
            inline_source: None,
            witness,
        })
    })
}

fn index_with_paths(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    selected: Option<&[&str]>,
    native_json: Option<&[u8]>,
    execution: Option<&super::observation_execution::Captured>,
) -> Result<(bool, Captured), ConversationStoreError> {
    let capture = capture_with_paths(connection, event, selected, native_json, execution, None)?;
    #[cfg(test)]
    let _probe = super::start_probe::for_event(event, super::AuxiliaryStartProbe::ObservationJson);
    if let Some(inline_source) = &capture.inline_source {
        let rows = connection.prepare_cached("UPDATE events SET observation_source=?2,observation_witness=?3 WHERE sequence=?1 AND CAST(event_json AS BLOB)=?4")
            .and_then(|mut statement| statement.execute(params![event.sequence, inline_source, capture.witness.bytes, native_json]))
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
        if rows != 1 {
            return Err(storage("native event bytes changed before source capture"));
        }
    } else {
        connection
            .prepare_cached("UPDATE events SET observation_witness=?2 WHERE sequence=?1")
            .and_then(|mut statement| {
                statement.execute(params![event.sequence, capture.witness.bytes])
            })
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    }
    Ok((capture.available, capture.witness))
}

// These closed fields have no deferred children. All remaining identity,
// usage, timing and terminal semantics already come from the typed owner below.
fn direct_value(
    event: &RuntimeEventEnvelope,
    selected: Option<&[&str]>,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, ConversationStoreError> {
    use crate::events::types::RuntimeEvent as Event;
    let finish = match (&event.event, selected) {
        (_, Some(["type"])) => None,
        (Event::AttemptCompleted { finish_reason, .. }, Some(["type", "finish_reason"]))
        | (
            Event::ModelRequestCompleted { finish_reason, .. },
            Some(["type", "finish_reason", "generation"]),
        ) => {
            if matches!(finish_reason, crate::model::ModelFinishReason::Other { reason } if reason.len()>256)
            {
                return Ok(None);
            }
            Some(finish_reason)
        }
        _ => return Ok(None),
    };
    let mut value = serde_json::Map::new();
    value.insert("type".into(), native_kind(&event.event)?.into());
    if let Some(finish) = finish {
        value.insert(
            "finish_reason".into(),
            serde_json::to_value(finish).map_err(|error| storage(error.to_string()))?,
        );
    }
    Ok(Some(value))
}

#[cfg(test)]
thread_local! {static CAPTURE_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn take_capture_work() -> (usize, usize) {
    CAPTURE_WORK.with(|work| work.replace((0, 0)))
}

#[allow(clippy::too_many_lines)] // One optional source write and its authenticated finite witness.
fn capture_with_paths(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    selected: Option<&[&str]>,
    native_json: Option<&[u8]>,
    execution: Option<&super::observation_execution::Captured>,
    pending: Option<&super::append_facts::PendingEvent<'_>>,
) -> Result<SourceCapture, ConversationStoreError> {
    let id = key(event.sequence);
    let mut inline_source =
        native_json.and_then(|native| inline::capture_native(&event.event, native));
    let mut fields = Vec::new();
    let direct = inline_source
        .as_ref()
        .map(|_| direct_value(event, selected))
        .transpose()?
        .flatten();
    let value = if let Some(value) = direct {
        #[cfg(test)]
        CAPTURE_WORK.with(|work| {
            let (sources, direct) = work.get();
            work.set((sources, direct + 1));
        });
        value
    } else {
        #[cfg(test)]
        CAPTURE_WORK.with(|work| {
            let (sources, direct) = work.get();
            work.set((sources + 1, direct));
        });
        let source =
            crate::runtime::read_layout::SeekableObservationSource::new(event).map_err(storage)?;
        let inline = if let Some(native) = native_json
            && (inline_source.is_some()
                || source
                    .read(&[], 0, 1)
                    .map_err(|error| storage(error.to_string()))?
                    .readable_end
                    <= inline::MAX_BYTES as u64)
        {
            let source = inline::Source::new(source)?;
            if inline_source.is_none() {
                inline_source = inline::capture_overlay(native, &source.bytes()?)?;
            }
            if inline_source.is_some() {
                Some(source)
            } else {
                super::observation_request::index_state_source(connection, KIND, &id, &source.0)?;
                None
            }
        } else {
            super::observation_request::index_state_source(connection, KIND, &id, &source)?;
            None
        };
        let source = EventFields {
            connection,
            id: &id,
            inline: inline.as_ref(),
        };
        let mut value = serde_json::Map::new();
        if selected.is_none() {
            indexed_root_previews(&source, &mut value, &mut fields)?;
        }
        indexed_field_previews(&source, &mut value, &mut fields, selected)?;
        value
    };
    let event_kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| storage("native event witness missing semantic family"))?
        .to_owned();
    fields.truncate(32);
    let mut semantic = serde_json::Value::Object(value);
    terminal_fields(
        connection,
        event,
        &mut semantic,
        execution,
        pending.is_some(),
    )?;
    let value = semantic
        .as_object()
        .cloned()
        .ok_or_else(|| storage("native witness semantic header unavailable"))?;
    let occupancy =
        super::observation_occupancy::witness_with_pending(connection, event, pending.is_some())?;
    let missing = matches!(
        event.event,
        crate::events::types::RuntimeEvent::CompactionCompleted { .. }
    ) && occupancy.is_none();
    let witness = ObservationEventWitness {
        occupancy,
        native_direct: false,
        trace_total: if let Some(pending) = pending {
            pending_trace_total(connection, event, pending)?
        } else {
            super::optional_display_value(super::observation_statistics::trace_total(
                connection,
                event.sequence,
            ))?
        },
        source_unavailable: missing,
        transcript_cursor: pending.and_then(|pending| pending.transcript_cursor),
        sequence: event.sequence,
        event_id: (event.event_id.as_str().len() <= 256).then(|| event.event_id.clone()),
        attempt_id: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        turn_id: event
            .turn_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 256)
            .cloned(),
        owner: event
            .event
            .model_request_owner()
            .map(crate::durable::observation::RequestOwnerKind::from),
        auxiliary_call_id: event.event.auxiliary_call_id().cloned(),
        timestamp: event.timestamp,
        event_kind,
        tool_occurrence: event.tool_occurrence.clone(),
        result_ledger_position: result_position(connection, event)?,
        value: serde_json::Value::Object(value),
        deferred: !fields.is_empty(),
        fields,
    };
    let bytes = encode_witness(&witness)?;
    Ok(SourceCapture {
        available: !missing,
        inline_source,
        witness: Captured {
            bytes,
            value: witness,
        },
    })
}

fn readable(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<bool, ConversationStoreError> {
    if sequence == 0 || sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "event outside native read cut".into(),
        ));
    }
    // Select the storage shape without even authorizing access to event_json.
    // Large sources use independent bounded segments; CASE in a query naming
    // event_json would still authorize that column while preparing the query.
    connection
        .query_row_cached(
            "SELECT observation_source IS NOT NULL FROM events WHERE sequence=?1 AND observation_kind IS NOT NULL",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?
        .ok_or_else(|| ConversationStoreError::InvalidReference("native event source unavailable".into()))
}
pub(super) fn kind(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<Option<String>, ConversationStoreError> {
    if sequence == 0 || sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "event outside native read cut".into(),
        ));
    }
    let kind: Option<Option<String>> = connection
        .query_row_cached(
            "SELECT observation_kind FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    match kind {
        Some(Some(kind)) => Ok(Some(kind)),
        Some(None) => Err(storage("native event kind header unavailable")),
        None => Ok(None),
    }
}
pub(super) fn witness(
    connection: &Connection,
    sequence: u64,
    through: u64,
) -> Result<Option<ObservationEventWitness>, ConversationStoreError> {
    witness_prepared(connection, sequence, through, None)
}
pub(super) fn witness_prepared(
    connection: &Connection,
    sequence: u64,
    through: u64,
    prepared: Option<&Captured>,
) -> Result<Option<ObservationEventWitness>, ConversationStoreError> {
    if sequence > through {
        return Err(ConversationStoreError::InvalidReference(
            "event outside native read cut".into(),
        ));
    }
    let row: Option<(Option<Vec<u8>>, bool, bool)> = connection
        .prepare_cached("SELECT e.observation_witness,COALESCE(r.direct=1 AND r.ready=1,0),(e.observation_readiness_missing AND e.sequence>(SELECT journal FROM observation_readiness_epoch WHERE id=1)) OR e.observation_unavailable FROM events e LEFT JOIN event_observation_readiness r ON r.sequence=e.sequence WHERE e.sequence=?1")
        .and_then(|mut statement| statement.query_row([sequence], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))
        .optional().map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    let Some((Some(bytes), direct, missing)) = row else {
        return Ok(None);
    };
    decode_witness(&bytes, direct, missing, prepared).map(Some)
}
pub(super) fn decode_witness(
    bytes: &[u8],
    direct: bool,
    missing: bool,
    prepared: Option<&Captured>,
) -> Result<ObservationEventWitness, ConversationStoreError> {
    let mut witness = match prepared.and_then(|capture| capture.reuse(bytes)) {
        Some(witness) => witness,
        None => storage::decode(bytes)?,
    };
    witness.native_direct = direct;
    witness.source_unavailable |= missing;
    Ok(witness)
}

pub(super) fn page(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    if readable(connection, sequence, through)? {
        return inline::Source::load(connection, sequence)?.page(path, offset, limit);
    }
    super::observation_request::page_from(connection, TABLE, &key(sequence), path, offset, limit)
}
pub(super) fn read(
    connection: &Connection,
    sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if readable(connection, sequence, through)? {
        return inline::Source::load(connection, sequence)?.read(path, offset, max_bytes);
    }
    super::observation_request::read_from(
        connection,
        KIND,
        TABLE,
        &key(sequence),
        path,
        offset,
        max_bytes,
    )
}

#[allow(clippy::too_many_lines)] // Closed event identity mapping stays exhaustive in one place.
fn identity_fields(
    event: &RuntimeEventEnvelope,
    value: &mut serde_json::Value,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent as Event;
    // Typed semantic fields are captured from the committing owner, rather
    // than inferred by deserializing a deliberately incomplete preview.
    let identity = |id: &str| {
        if id.len() <= 256 {
            serde_json::Value::String(id.into())
        } else {
            serde_json::Value::Null
        }
    };
    if let Some(owner) = event.event.model_request_owner() {
        value["owner"] = serde_json::json!({
            "type": crate::durable::observation::RequestOwnerKind::from(owner)
        });
    }
    if let Some(call_id) = event.event.auxiliary_call_id() {
        value["call_id"] = identity(call_id.as_str());
    }
    match &event.event {
        Event::AttemptStarted { attempt_id }
        | Event::AttemptCompleted { attempt_id, .. }
        | Event::AttemptCancelled { attempt_id, .. }
        | Event::AttemptFailed { attempt_id, .. }
        | Event::AttemptTimedOut { attempt_id }
        | Event::AttemptLimitExceeded { attempt_id, .. } => {
            value["attempt_id"] = identity(attempt_id.as_str());
        }
        Event::ModelRequestStarted {
            request_id, model, ..
        } => {
            value["request_id"] = identity(request_id.as_str());
            value["model"] = serde_json::json!(prefix(model));
        }
        Event::ModelRequestCompleted {
            request_id,
            usage,
            generation,
            ..
        }
        | Event::ModelRequestFailed {
            request_id,
            usage,
            generation,
            ..
        } => {
            value["request_id"] = identity(request_id.as_str());
            value["usage"] = crate::observation::bounded::structural_value(usage)
                .ok_or_else(|| storage("usage witness exceeds finite header"))?;
            value["generation"] = crate::observation::bounded::structural_value(generation)
                .ok_or_else(|| storage("generation witness exceeds finite header"))?;
        }
        Event::AuxiliaryCallStarted {
            purpose,
            trigger,
            business_source,
            input,
            semantic_input_fingerprint,
            invocation_fingerprint,
            selected_message_ids,
            ..
        } => {
            value["purpose"] =
                serde_json::to_value(purpose).map_err(|error| storage(error.to_string()))?;
            value["trigger"] =
                serde_json::to_value(trigger).map_err(|error| storage(error.to_string()))?;
            value["business_source"] = serde_json::to_value(business_source)
                .map_err(|error| storage(error.to_string()))?;
            value["input"] =
                serde_json::to_value(input).map_err(|error| storage(error.to_string()))?;
            value["semantic_input_fingerprint"] =
                serde_json::json!(semantic_input_fingerprint.as_str());
            value["invocation_fingerprint"] = serde_json::json!(invocation_fingerprint.as_str());
            value["selected_message_ids"] = serde_json::json!(selected_message_ids);
        }
        Event::AuxiliaryCallSettled {
            outcome,
            request_evidence,
            ..
        } => {
            value["request_evidence"] = serde_json::to_value(request_evidence)
                .map_err(|error| storage(error.to_string()))?;
            value["outcome"] =
                serde_json::to_value(outcome).map_err(|error| storage(error.to_string()))?;
        }
        Event::ToolExecutionStarted {
            tool_call_id,
            tool_id,
        }
        | Event::ToolExecutionCompleted {
            tool_call_id,
            tool_id,
            ..
        }
        | Event::ToolExecutionFailed {
            tool_call_id,
            tool_id,
            ..
        }
        | Event::ToolExecutionProgress {
            tool_call_id,
            tool_id,
            ..
        } => {
            value["tool_call_id"] = identity(tool_call_id.as_str());
            value["tool_id"] = identity(tool_id.as_str());
        }
        Event::AssistantMessageCommitted { message_id }
        | Event::ToolMessageCommitted { message_id, .. } => {
            value["message_id"] = identity(message_id.as_str());
        }
        Event::ContextContributionEmitted {
            producer,
            request_id,
            message_id,
            emission,
            logical_step_origin,
        } => {
            value["producer"] = crate::observation::bounded::bounded_bytes(producer, 1024)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
            value["request_id"] = identity(request_id.as_str());
            value["message_id"] = identity(message_id.as_str());
            value["logical_step_origin"] = serde_json::json!(logical_step_origin);
            value["emission"] = crate::observation::bounded::bounded_bytes(emission, 2048)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
        }
        Event::Goal { fact } => {
            value["fact"] = crate::observation::bounded::bounded_bytes(fact, 2048)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
        }
        _ => {}
    }
    Ok(())
}

struct EventFields<'a> {
    connection: &'a Connection,
    id: &'a str,
    inline: Option<&'a inline::Source>,
}
impl EventFields<'_> {
    fn field(&self, path: &[String]) -> Result<(u64, u64, u64, Vec<u8>), ConversationStoreError> {
        match self.inline {
            Some(source) => source.field(path),
            None => super::observation_request::field_from(self.connection, TABLE, self.id, path),
        }
    }
    fn page(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RequestObservationPage, ConversationStoreError> {
        match self.inline {
            Some(source) => source.page(path, offset, limit),
            None => super::observation_request::page_from(
                self.connection,
                TABLE,
                self.id,
                path,
                offset,
                limit,
            ),
        }
    }
}
fn indexed_root_previews(
    source: &EventFields<'_>,
    value: &mut serde_json::Map<String, serde_json::Value>,
    fields: &mut Vec<ObservationEventField>,
) -> Result<(), ConversationStoreError> {
    #[cfg(test)]
    PREVIEW_WORK.with(|work| {
        let (calls, pages) = work.get();
        work.set((calls, pages + 1));
    });
    let root = source.page(&["event".into()], 0, 32)?;
    for field in root.fields {
        let Some(key) = field.key.as_ref().map(|key| key.preview.as_str()) else {
            continue;
        };
        let (preview, trimmed) = semantic_preview(field.value);
        if field.deferred || trimmed {
            fields.push(ObservationEventField {
                path: field.path.clone(),
                encoded_bytes: field.encoded_bytes,
                children: field.children_total,
            });
        }
        if matches!(key, "tool_call_id" | "tool_id" | "checkpoint_id")
            && (field.deferred || trimmed)
        {
            continue;
        }
        value.insert(key.into(), preview);
    }
    Ok(())
}

fn indexed_field_previews(
    source: &EventFields<'_>,
    value: &mut serde_json::Map<String, serde_json::Value>,
    fields: &mut Vec<ObservationEventField>,
    selected: Option<&[&str]>,
) -> Result<(), ConversationStoreError> {
    for path in paths(selected) {
        #[cfg(test)]
        PREVIEW_WORK.with(|work| {
            let (calls, pages) = work.get();
            work.set((calls + 1, pages));
        });
        let Ok((start, end, children, bytes)) = source.field(&path) else {
            continue;
        };
        let decoded: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))?;
        let encoded_bytes = end - start;
        let (decoded, trimmed) = semantic_preview(decoded);
        let exact = encoded_bytes == bytes.len() as u64 && !trimmed;
        if !exact && !fields.iter().any(|field| field.path == path) {
            fields.push(ObservationEventField {
                path: path.clone(),
                encoded_bytes,
                children,
            });
        }
        if path.last().is_some_and(|field| {
            matches!(field.as_str(), "tool_call_id" | "tool_id" | "checkpoint_id")
        }) && (!exact || decoded.as_str().is_some_and(|id| id.len() > 256))
        {
            continue;
        }
        insert(value, &path[1..], decoded);
    }
    Ok(())
}
