//! Finite aggregate read layouts attached to the existing native Journal commit.
//! They grant no execution authority and publish neither events nor cursors.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::reading::TurnExecution;
use crate::durable::response::{add_usage, timing::ActivityFold};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime_client::response::{ConversationStatistics, ConversationTurnClock};
use serde::{Deserialize, Serialize};
#[derive(Default, Serialize, Deserialize)]
struct StatisticsLayout {
    statistics: ConversationStatistics,
    activity: TurnExecution,
    local: LocalStatisticsLayout,
}
#[derive(Default, Serialize, Deserialize)]
struct LocalStatisticsLayout {
    view: crate::runtime_client::agent_statistics::AgentStatistics,
    activity: TurnExecution,
    active_start: Option<u64>,
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(
        value,
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .ok_or_else(|| storage("native aggregate header exceeds bound"))
}
fn add_count(total: &mut u64, part: u64) -> Result<(), ConversationStoreError> {
    *total = total
        .checked_add(part)
        .ok_or_else(|| storage("native statistics count exceeds representation"))?;
    Ok(())
}
fn layout(
    connection: &Connection,
    through: u64,
) -> Result<StatisticsLayout, ConversationStoreError> {
    let missing: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_statistics_holes_idx WHERE observation_statistics_required=1 AND observation_statistics IS NULL AND sequence<=?1)",
        [through], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage("native statistics header unavailable"));
    }
    let bytes:Option<Vec<u8>>=connection.query_row("SELECT observation_statistics FROM events INDEXED BY events_observation_statistics_idx WHERE observation_statistics IS NOT NULL AND sequence<=?1 ORDER BY sequence DESC LIMIT 1",[through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    let bytes = match bytes {
        Some(bytes) => Some(bytes),
        None => connection
            .query_row(
                "SELECT observation_statistics FROM rustx_store WHERE id=1",
                [],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .map_err(|error| storage(error.to_string()))?,
    };
    bytes.map_or_else(
        || {
            let initialized: bool = connection
                .query_row(
                    "SELECT bootstrap_initialized FROM rustx_store WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            if initialized {
                Err(storage("native statistics bootstrap header unavailable"))
            } else {
                Ok(StatisticsLayout::default())
            }
        },
        |bytes| {
            #[cfg(test)]
            READ_WORK.with(|work| {
                let (rows, total, original) = work.get();
                work.set((rows + 1, total + bytes.len(), original));
            });
            serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
        },
    )
}
fn at_cut(
    connection: &Connection,
    through: u64,
) -> Result<StatisticsLayout, ConversationStoreError> {
    let actual: u64 = connection
        .query_row(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if through > actual {
        return Err(ConversationStoreError::InvalidReference(
            "future native statistics cut".into(),
        ));
    }
    layout(connection, through)
}
pub(super) fn read(
    connection: &Connection,
    through: u64,
) -> Result<ConversationStatistics, ConversationStoreError> {
    Ok(at_cut(connection, through)?.statistics)
}
pub(super) fn agent_read(
    connection: &Connection,
    through: u64,
) -> Result<crate::runtime_client::agent_statistics::AgentStatistics, ConversationStoreError> {
    Ok(at_cut(connection, through)?.local.view)
}

pub(super) fn requires_statistics(event: &RuntimeEventEnvelope) -> bool {
    event.attempt_id.is_some()
        && matches!(
            event.event,
            RuntimeEvent::AttemptStarted { .. }
                | RuntimeEvent::TurnStarted
                | RuntimeEvent::ModelRequestStarted { .. }
                | RuntimeEvent::ModelRequestCompleted { .. }
                | RuntimeEvent::ModelRequestFailed { .. }
                | RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::ToolExecutionStarted { .. }
                | RuntimeEvent::ToolExecutionCompleted { .. }
                | RuntimeEvent::ToolExecutionFailed { .. }
                | RuntimeEvent::CompactionStarted { .. }
                | RuntimeEvent::CompactionCompleted { .. }
        )
}

pub(super) fn seed(
    connection: &Connection,
    turns: &[crate::durable::reading::TurnReadingProvenance],
) -> Result<(), ConversationStoreError> {
    let mut state = StatisticsLayout::default();
    let mut activity = ActivityFold::default();
    let mut ttft_requests = 0;
    for turn in turns {
        add_count(&mut state.statistics.turns, 1)?;
        if let Some(execution) = &turn.execution {
            add_count(&mut state.statistics.steps, execution.steps)?;
            add_count(
                &mut state.statistics.model_requests,
                execution.model_requests,
            )?;
            add_count(
                &mut state.statistics.requests_with_usage,
                execution.requests_with_usage,
            )?;
            add_count(
                &mut state.statistics.completed_responses,
                u64::from(execution.completed_response),
            )?;
            if let Some(usage) = &execution.reported_usage {
                add_usage(&mut state.statistics.reported_usage, usage);
            }
            add_count(&mut ttft_requests, execution.ttft_requests)?;
            activity.absorb(execution);
        }
    }
    state.statistics.timing = activity.summary();
    activity.record(&mut state.activity);
    connection
        .execute(
            "UPDATE rustx_store SET observation_statistics=?1 WHERE id=1",
            [encode(&state)?],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn is_trace_anchor(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AttemptStarted { .. }
            | RuntimeEvent::TurnStarted
            | RuntimeEvent::InboundTurnAdopted { .. }
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolExecutionStarted { .. }
            | RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::BackgroundExecutionCommitted { .. }
            | RuntimeEvent::SubagentOwnershipCommitted { .. }
            | RuntimeEvent::WorkflowStarted { .. }
            | RuntimeEvent::InteractionRequested { .. }
    )
}
fn record_trace_anchor(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if is_trace_anchor(&event.event) {
        let total = trace_total(connection, event.sequence.saturating_sub(1))?
            .checked_add(1)
            .ok_or_else(|| storage("native trace count exhausted"))?;
        connection
            .execute(
                "UPDATE events SET observation_trace_total=?1 WHERE sequence=?2",
                params![total, event.sequence],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}
pub(super) fn event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    record_trace_anchor(connection, event)?;
    let Some(attempt) = &event.attempt_id else {
        return Ok(());
    };
    if !requires_statistics(event) {
        return Ok(());
    }
    let mut state = layout(connection, event.sequence.saturating_sub(1))?;
    let mut activity = ActivityFold::default();
    activity.absorb(&state.activity);
    fold_statistics(
        connection,
        event,
        attempt,
        &mut state.statistics,
        &mut activity,
    )?;
    let mut local_activity = ActivityFold::default();
    local_activity.absorb(&state.local.activity);
    fold_statistics(
        connection,
        event,
        attempt,
        &mut state.local.view.statistics,
        &mut local_activity,
    )?;
    state.local.view.statistics.timing = local_activity.summary();
    local_activity.record(&mut state.local.activity);
    record_duration(connection, event, &mut state.local)?;
    state.statistics.timing = activity.summary();
    activity.record(&mut state.activity);
    connection
        .execute(
            "UPDATE events SET observation_statistics=?2 WHERE sequence=?1",
            params![event.sequence, encode(&state)?],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
fn fold_statistics(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    attempt: &crate::runtime::identity::AttemptId,
    statistics: &mut ConversationStatistics,
    activity: &mut ActivityFold,
) -> Result<(), ConversationStoreError> {
    match &event.event {
        RuntimeEvent::AttemptStarted { .. } => {
            add_count(&mut statistics.turns, 1)?;
            statistics.latest_turn = Some(ConversationTurnClock {
                attempt_id: (attempt.as_str().len() <= 512).then(|| attempt.clone()),
                attempt_id_deferred: attempt.as_str().len() > 512,
                attempt_id_source: if attempt.as_str().len() > 512 {
                    Some(crate::runtime_client::snapshot::JournalFieldRef {
                        sequence: event.sequence,
                        path: vec!["attempt_id".into()],
                        encoded_bytes: super::observation_request_summary::serialized_bytes(
                            attempt,
                        )?,
                    })
                } else {
                    None
                },
                started_at: event.timestamp,
                ended_at: None,
            });
        }
        RuntimeEvent::TurnStarted => add_count(&mut statistics.steps, 1)?,
        RuntimeEvent::ModelRequestStarted { .. } => add_count(&mut statistics.model_requests, 1)?,
        RuntimeEvent::ModelRequestCompleted {
            usage, generation, ..
        }
        | RuntimeEvent::ModelRequestFailed {
            usage, generation, ..
        } => {
            if generation
                .as_ref()
                .and_then(
                    crate::model::generation_evidence::GenerationEvidence::time_to_first_output_ms,
                )
                .is_some()
            {
                let mut reading = TurnExecution::default();
                activity.record(&mut reading);
                add_count(&mut reading.ttft_requests, 1)?;
            }
            activity.request(generation.as_ref(), usage.as_ref());
            if let Some(usage) = usage {
                add_count(&mut statistics.requests_with_usage, 1)?;
                add_usage(&mut statistics.reported_usage, usage);
            }
        }
        RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
        | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => {
            record_tool_settlement(connection, event, attempt, tool_call_id, activity)?;
        }
        RuntimeEvent::AttemptCompleted { finish_reason, .. } => {
            if matches!(
                finish_reason,
                crate::model::ModelFinishReason::Stop | crate::model::ModelFinishReason::Refusal
            ) {
                let closing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='assistant_message_committed' AND sequence<?2)",params![attempt.as_str(),event.sequence],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
                add_count(&mut statistics.completed_responses, u64::from(closing))?;
            }
            close_clock(connection, statistics, attempt, event.timestamp)?;
        }
        RuntimeEvent::AttemptCancelled { .. }
        | RuntimeEvent::AttemptFailed { .. }
        | RuntimeEvent::AttemptTimedOut { .. }
        | RuntimeEvent::AttemptLimitExceeded { .. } => {
            close_clock(connection, statistics, attempt, event.timestamp)?;
        }
        _ => {}
    }
    Ok(())
}

fn settle_duration(local: &mut LocalStatisticsLayout, at: chrono::DateTime<chrono::Utc>) {
    if let Some(active) = local.view.duration.active.take() {
        let duration = u64::try_from(
            at.signed_duration_since(active.started_at)
                .num_milliseconds(),
        )
        .unwrap_or(0);
        local.view.duration.settled_ms = local.view.duration.settled_ms.saturating_add(duration);
    }
    local.active_start = None;
}

fn record_duration(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    local: &mut LocalStatisticsLayout,
) -> Result<(), ConversationStoreError> {
    use crate::runtime_client::agent_statistics::AgentActiveInterval;
    if matches!(event.event, RuntimeEvent::AttemptStarted { .. }) {
        if let Some(active) = &local.view.duration.active {
            settle_duration(local, active.observed_at);
        }
        local.active_start = Some(event.sequence);
        local.view.duration.active = Some(AgentActiveInterval {
            started_at: event.timestamp,
            observed_at: event.timestamp,
            running: false,
        });
        return Ok(());
    }
    let Some(start) = local.active_start else {
        return Ok(());
    };
    let same: bool = connection
        .query_row(
            "SELECT attempt_id=?2 FROM events WHERE sequence=?1",
            params![
                start,
                event
                    .attempt_id
                    .as_ref()
                    .map(crate::runtime::identity::AttemptId::as_str)
            ],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if same {
        if let Some(active) = &mut local.view.duration.active {
            active.observed_at = active.observed_at.max(event.timestamp);
        }
        if matches!(
            event.event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
        ) {
            settle_duration(local, event.timestamp);
        }
    }
    Ok(())
}

fn record_tool_settlement(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    attempt: &crate::runtime::identity::AttemptId,
    tool_call_id: &crate::runtime::identity::ToolCallId,
    activity: &mut ActivityFold,
) -> Result<(), ConversationStoreError> {
    // One exact indexed native lifecycle predecessor; no active Tool mirror.
    let mut predecessor = None;
    for kind in [
        "tool_execution_started",
        "tool_execution_completed",
        "tool_execution_failed",
    ] {
        let fact:Option<(u64,String)>=connection.query_row("SELECT sequence,event_timestamp FROM events INDEXED BY events_observation_tool_idx WHERE json_extract(event_json,'$.event.tool_call_id')=?1 AND attempt_id=?2 AND json_extract(event_json,'$.event.type')=?3 AND sequence<?4 ORDER BY sequence DESC LIMIT 1",params![tool_call_id.as_str(),attempt.as_str(),kind,event.sequence],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|error|storage(error.to_string()))?;
        if let Some((sequence, timestamp)) = fact
            && predecessor
                .as_ref()
                .is_none_or(|(previous, _, _): &(u64, &str, String)| *previous < sequence)
        {
            predecessor = Some((sequence, kind, timestamp));
        }
    }
    if let Some((_, "tool_execution_started", timestamp)) = predecessor {
        let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|error| storage(error.to_string()))?
            .with_timezone(&chrono::Utc);
        let span = u64::try_from(
            event
                .timestamp
                .signed_duration_since(timestamp)
                .num_milliseconds(),
        )
        .unwrap_or(0);
        activity.tool_span_ms(span);
    }

    Ok(())
}
fn close_clock(
    connection: &Connection,
    statistics: &mut ConversationStatistics,
    attempt: &crate::runtime::identity::AttemptId,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), ConversationStoreError> {
    if let Some(clock) = statistics.latest_turn.as_mut() {
        let same = if let Some(id) = &clock.attempt_id {
            id == attempt
        } else if let Some(source) = &clock.attempt_id_source {
            connection
                .query_row(
                    "SELECT attempt_id=?2 FROM events WHERE sequence=?1",
                    params![source.sequence, attempt.as_str()],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?
        } else {
            return Err(storage("native turn clock identity evidence unavailable"));
        };
        if same {
            clock.ended_at = Some(at);
        }
    }
    Ok(())
}

pub(super) fn trace_total(
    connection: &Connection,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    let missing:bool=connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_trace_holes_idx WHERE observation_trace_required=1 AND observation_trace_total IS NULL AND sequence<=?1)").map_err(|error| storage(error.to_string()))?.query_row([through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native trace count source unavailable"));
    }
    connection.prepare_cached("SELECT observation_trace_total FROM events INDEXED BY events_observation_trace_total_idx WHERE observation_trace_total IS NOT NULL AND sequence<=?1 ORDER BY sequence DESC LIMIT 1").map_err(|error| storage(error.to_string()))?.query_row([through],|row|row.get(0)).optional().map(|value|value.unwrap_or(0)).map_err(|error|storage(error.to_string()))
}

#[cfg(test)]
thread_local! {
    static READ_WORK: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
}
#[cfg(test)]
pub(super) fn original_read() {
    READ_WORK.with(|work| {
        let (rows, bytes, original) = work.get();
        work.set((rows, bytes, original + 1));
    });
}
#[cfg(test)]
pub(super) fn read_work() -> (usize, usize, usize) {
    READ_WORK.with(|work| work.replace((0, 0, 0)))
}
