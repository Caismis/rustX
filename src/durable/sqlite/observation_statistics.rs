//! Finite aggregate read layouts attached to the existing native Journal commit.
//! They grant no execution authority and publish neither events nor cursors.
use super::observation_codec as codec;
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::reading::TurnExecution;
use crate::durable::response::{add_usage, timing::ActivityFold};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime_client::response::{ConversationStatistics, ConversationTurnClock};
#[derive(Default)]
struct StatisticsLayout {
    statistics: ConversationStatistics,
    activity: TurnExecution,
    local: LocalStatisticsLayout,
}
#[derive(Default)]
struct LocalStatisticsLayout {
    view: crate::runtime_client::agent_statistics::AgentStatistics,
    activity: TurnExecution,
    active_start: Option<u64>,
}
// Native conversation and local-agent totals are identical until a lineage seed
// contributes inherited work. Keep only one copy in that common case. This is
// a private, exact-length storage tuple; public response JSON is unchanged.
type StoredActivity = (Option<u64>, u64, u64, u64, u64, Option<u64>);
type StoredTotals = (
    u64,
    u64,
    Option<ConversationTurnClock>,
    Option<crate::durable::response::ConversationTiming>,
    u64,
    u64,
    u64,
    Option<crate::model::ModelUsage>,
);
type StoredStatistics = (
    StoredTotals,
    StoredActivity,
    crate::runtime_client::agent_statistics::AgentDuration,
    Option<u64>,
    Option<(StoredTotals, StoredActivity)>,
);
fn shared(state: &StatisticsLayout) -> bool {
    state.statistics == state.local.view.statistics && state.activity == state.local.activity
}
fn activity_fields(value: &TurnExecution) -> StoredActivity {
    (
        value.model_ms,
        value.ttft_ms,
        value.ttft_requests,
        value.decode_ms,
        value.decode_tokens,
        value.tool_ms,
    )
}
fn activity_from_fields(value: StoredActivity) -> TurnExecution {
    let (model_ms, ttft_ms, ttft_requests, decode_ms, decode_tokens, tool_ms) = value;
    TurnExecution {
        model_ms,
        ttft_ms,
        ttft_requests,
        decode_ms,
        decode_tokens,
        tool_ms,
        ..TurnExecution::default()
    }
}
fn totals_fields<'a, T: serde::Serialize + 'a>(
    value: &'a ConversationStatistics,
    timing: T,
) -> impl serde::Serialize + 'a {
    (
        value.turns,
        value.steps,
        &value.latest_turn,
        timing,
        value.completed_responses,
        value.model_requests,
        value.requests_with_usage,
        &value.reported_usage,
    )
}
fn totals_from_fields(value: StoredTotals) -> ConversationStatistics {
    let (
        turns,
        steps,
        latest_turn,
        timing,
        completed_responses,
        model_requests,
        requests_with_usage,
        reported_usage,
    ) = value;
    ConversationStatistics {
        turns,
        steps,
        latest_turn,
        timing,
        completed_responses,
        model_requests,
        requests_with_usage,
        reported_usage,
    }
}
fn fields(state: &StatisticsLayout, is_shared: bool) -> impl serde::Serialize + '_ {
    let local = (!is_shared).then(|| {
        (
            totals_fields(
                &state.local.view.statistics,
                &state.local.view.statistics.timing,
            ),
            activity_fields(&state.local.activity),
        )
    });
    (
        totals_fields(&state.statistics, &state.statistics.timing),
        activity_fields(&state.activity),
        &state.local.view.duration,
        state.local.active_start,
        local,
    )
}

#[derive(serde::Serialize)]
struct JsonTiming {
    #[serde(skip_serializing_if = "Option::is_none")]
    model_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mean_ttft_ms: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "codec::json_optional_float"
    )]
    output_tokens_per_second: Option<f64>,
}
impl From<&crate::durable::response::ConversationTiming> for JsonTiming {
    fn from(value: &crate::durable::response::ConversationTiming) -> Self {
        Self {
            model_ms: value.model_ms,
            tool_ms: value.tool_ms,
            mean_ttft_ms: value.mean_ttft_ms,
            output_tokens_per_second: value.output_tokens_per_second,
        }
    }
}
fn compact_totals_fields(value: &ConversationStatistics) -> impl serde::Serialize + '_ {
    totals_fields(value, value.timing.as_ref().map(JsonTiming::from))
}
fn compact_fields(state: &StatisticsLayout, is_shared: bool) -> impl serde::Serialize + '_ {
    (
        compact_totals_fields(&state.statistics),
        activity_fields(&state.activity),
        &state.local.view.duration,
        state.local.active_start,
        (!is_shared).then(|| {
            (
                compact_totals_fields(&state.local.view.statistics),
                activity_fields(&state.local.activity),
            )
        }),
    )
}
fn encode(state: &StatisticsLayout) -> Result<Vec<u8>, ConversationStoreError> {
    // Admission uses the original tuple, before JSON's nonfinite-to-null timing
    // conversion. The smaller private encoding cannot admit additional values.
    let is_shared = shared(state);
    codec::encode(
        &fields(state, is_shared),
        &compact_fields(state, is_shared),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
    .map_err(|_| storage("native aggregate header exceeds bound"))
}
fn decode(bytes: &[u8]) -> Result<StatisticsLayout, ConversationStoreError> {
    let (statistics, activity, duration, active_start, local): StoredStatistics =
        codec::decode_json_tuple(bytes, crate::durable::observation::HEADER_ENCODED_MAX)?;
    let statistics = totals_from_fields(statistics);
    let activity = activity_from_fields(activity);
    let (local_statistics, local_activity) = local.map_or_else(
        || (statistics.clone(), activity.clone()),
        |(statistics, activity)| {
            (
                totals_from_fields(statistics),
                activity_from_fields(activity),
            )
        },
    );
    Ok(StatisticsLayout {
        statistics,
        activity,
        local: LocalStatisticsLayout {
            view: crate::runtime_client::agent_statistics::AgentStatistics {
                statistics: local_statistics,
                occupancy: None,
                duration,
            },
            activity: local_activity,
            active_start,
        },
    })
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
    let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_aggregate_holes_idx WHERE observation_aggregate_holes>0 AND observation_aggregate_holes IN (4,5,6,7) AND sequence<=?1)").and_then(|mut statement| statement.query_row([through],|row| row.get(0))).map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage("native statistics header unavailable"));
    }
    let bytes: Option<Vec<u8>> = connection
        .prepare_cached(super::event_heads::STATISTICS)
        .and_then(|mut statement| statement.query_row([through], |row| row.get(0)))
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let bytes = match bytes {
        Some(bytes) => Some(bytes),
        None => connection
            .prepare_cached("SELECT observation_statistics FROM rustx_store WHERE id=1")
            .and_then(|mut statement| {
                statement.query_row([], |row| row.get::<_, Option<Vec<u8>>>(0))
            })
            .map_err(|error| storage(error.to_string()))?,
    };
    bytes.map_or_else(
        || {
            let initialized: bool = connection
                .prepare_cached("SELECT bootstrap_initialized FROM rustx_store WHERE id=1")
                .and_then(|mut statement| statement.query_row([], |row| row.get(0)))
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
            decode(&bytes)
        },
    )
}
fn at_cut(
    connection: &Connection,
    through: u64,
) -> Result<StatisticsLayout, ConversationStoreError> {
    let actual: u64 = connection
        .prepare_cached("SELECT next_event_sequence FROM rustx_store WHERE id=1")
        .and_then(|mut statement| statement.query_row([], |row| row.get(0)))
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
/// The native head and the supplied Journal boundary were already checked in
/// this transaction. Keep layout/hole decoding here, in its original error order.
pub(super) fn read_validated(
    validated: &super::observation::ValidatedCut<'_, '_>,
) -> Result<ConversationStatistics, ConversationStoreError> {
    Ok(layout(validated.connection()?, validated.expected().read.journal)?.statistics)
}
/// Reuse the freshly encoded value only after reading its real stored cell and
/// the same cut/hole evidence as the historical reader. Trigger changes remain
/// visible and are decoded with the current storage codec.
pub(super) fn read_prepared(
    connection: &Connection,
    through: u64,
    prepared: Option<&Prepared>,
) -> Result<ConversationStatistics, ConversationStoreError> {
    let Some(captured) = prepared.and_then(|prepared| prepared.statistics.as_ref()) else {
        return read(connection, through);
    };
    let (actual, bytes, missing): (u64, Option<Vec<u8>>, bool) = connection
        .prepare_cached("SELECT next_event_sequence,(SELECT observation_statistics FROM events WHERE sequence=?1),EXISTS(SELECT 1 FROM events INDEXED BY events_aggregate_holes_idx WHERE observation_aggregate_holes>0 AND observation_aggregate_holes IN (4,5,6,7) AND sequence<=?1) FROM rustx_store WHERE id=1")
        .and_then(|mut statement| statement.query_row([through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))
        .map_err(|error| storage(error.to_string()))?;
    if through > actual {
        return Err(ConversationStoreError::InvalidReference(
            "future native statistics cut".into(),
        ));
    }
    if missing {
        return Err(storage("native statistics header unavailable"));
    }
    match bytes {
        Some(bytes) => captured
            .reuse(&bytes)
            .map_or_else(|| decode(&bytes).map(|layout| layout.statistics), Ok),
        None => layout(connection, through).map(|layout| layout.statistics),
    }
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
    let bytes = encode(&state)?;
    connection
        .prepare_cached("UPDATE rustx_store SET observation_statistics=?1 WHERE id=1")
        .and_then(|mut statement| statement.execute([bytes]))
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
pub(super) struct Prepared {
    pub(super) trace_total: Option<u64>,
    pub(super) statistics: Option<super::append_facts::Captured<ConversationStatistics>>,
}

pub(super) fn prepare(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<Prepared, ConversationStoreError> {
    let trace_total = if is_trace_anchor(&event.event) {
        Some(
            trace_total(connection, event.sequence.saturating_sub(1))?
                .checked_add(1)
                .ok_or_else(|| storage("native trace count exhausted"))?,
        )
    } else {
        None
    };
    let mut prepared = Prepared {
        trace_total,
        statistics: None,
    };
    let Some(attempt) = &event.attempt_id else {
        return Ok(prepared);
    };
    if !requires_statistics(event) {
        return Ok(prepared);
    }
    let mut state = layout(connection, event.sequence.saturating_sub(1))?;
    let was_shared = shared(&state);
    let facts = NativeFacts::read(connection, event, attempt)?;
    let mut activity = ActivityFold::default();
    activity.absorb(&state.activity);
    fold_statistics(
        connection,
        event,
        attempt,
        &facts,
        &mut state.statistics,
        &mut activity,
    )?;
    state.statistics.timing = activity.summary();
    activity.record(&mut state.activity);
    if was_shared {
        state.local.view.statistics.clone_from(&state.statistics);
        state.local.activity.clone_from(&state.activity);
    } else {
        let mut local_activity = ActivityFold::default();
        local_activity.absorb(&state.local.activity);
        fold_statistics(
            connection,
            event,
            attempt,
            &facts,
            &mut state.local.view.statistics,
            &mut local_activity,
        )?;
        state.local.view.statistics.timing = local_activity.summary();
        local_activity.record(&mut state.local.activity);
    }
    record_duration(connection, event, &mut state.local)?;
    let bytes = encode(&state)?;
    prepared.statistics = Some(super::append_facts::Captured {
        bytes,
        value: state.statistics,
    });
    Ok(prepared)
}
#[derive(Default)]
struct NativeFacts {
    completed_response: bool,
    tool_span: Option<u64>,
}
impl NativeFacts {
    fn read(
        connection: &Connection,
        event: &RuntimeEventEnvelope,
        attempt: &crate::runtime::identity::AttemptId,
    ) -> Result<Self, ConversationStoreError> {
        let mut facts = Self::default();
        match &event.event {
            RuntimeEvent::AttemptCompleted {
                finish_reason:
                    crate::model::ModelFinishReason::Stop | crate::model::ModelFinishReason::Refusal,
                ..
            } => {
                facts.completed_response = connection
                    .prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_id=?1 AND json_extract(event_json,'$.event.type')='assistant_message_committed' AND sequence<?2)")
                    .and_then(|mut statement| statement.query_row(params![attempt.as_str(),event.sequence],|row|row.get(0)))
                    .map_err(|error|storage(error.to_string()))?;
            }
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
            | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => {
                facts.tool_span = tool_settlement(connection, event, attempt, tool_call_id)?;
            }
            _ => {}
        }
        Ok(facts)
    }
}
fn fold_statistics(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    attempt: &crate::runtime::identity::AttemptId,
    facts: &NativeFacts,
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
        RuntimeEvent::ToolExecutionCompleted { .. } | RuntimeEvent::ToolExecutionFailed { .. } => {
            if let Some(span) = facts.tool_span {
                activity.tool_span_ms(span);
            }
        }
        RuntimeEvent::AttemptCompleted { .. } => {
            add_count(
                &mut statistics.completed_responses,
                u64::from(facts.completed_response),
            )?;
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
        .prepare_cached("SELECT attempt_id=?2 FROM events WHERE sequence=?1")
        .and_then(|mut statement| {
            statement.query_row(
                params![
                    start,
                    event
                        .attempt_id
                        .as_ref()
                        .map(crate::runtime::identity::AttemptId::as_str)
                ],
                |row| row.get(0),
            )
        })
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

fn tool_settlement(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    attempt: &crate::runtime::identity::AttemptId,
    tool_call_id: &crate::runtime::identity::ToolCallId,
) -> Result<Option<u64>, ConversationStoreError> {
    // One exact indexed native lifecycle predecessor; no active Tool mirror.
    let mut predecessor = None;
    for kind in [
        "tool_execution_started",
        "tool_execution_completed",
        "tool_execution_failed",
    ] {
        let fact:Option<(u64,String)>=connection.prepare_cached("SELECT sequence,event_timestamp FROM events INDEXED BY events_observation_tool_idx WHERE json_extract(event_json,'$.event.tool_call_id')=?1 AND attempt_id=?2 AND json_extract(event_json,'$.event.type')=?3 AND sequence<?4 ORDER BY sequence DESC LIMIT 1").and_then(|mut statement| statement.query_row(params![tool_call_id.as_str(),attempt.as_str(),kind,event.sequence],|row|Ok((row.get(0)?,row.get(1)?)))).optional().map_err(|error|storage(error.to_string()))?;
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
        return Ok(Some(span));
    }

    Ok(None)
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
                .prepare_cached("SELECT attempt_id=?2 FROM events WHERE sequence=?1")
                .and_then(|mut statement| {
                    statement
                        .query_row(params![source.sequence, attempt.as_str()], |row| row.get(0))
                })
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
    let (missing, total): (bool, Option<u64>) = connection
        .prepare_cached(super::event_heads::TRACE_TOTAL)
        .and_then(|mut statement| {
            statement.query_row([through], |row| Ok((row.get(0)?, row.get(1)?)))
        })
        .map_err(|error| storage(error.to_string()))?;
    if missing {
        return Err(storage("native trace count source unavailable"));
    }
    Ok(total.unwrap_or(0))
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

#[cfg(test)]
mod compact_tests {
    use super::*;

    fn timed(rate: f64) -> StatisticsLayout {
        let statistics = ConversationStatistics {
            timing: Some(crate::durable::response::ConversationTiming {
                model_ms: Some(u64::MAX),
                tool_ms: Some(0),
                mean_ttft_ms: Some(3),
                output_tokens_per_second: Some(rate),
            }),
            reported_usage: Some(crate::model::ModelUsage {
                input_tokens: u64::MAX,
                output_tokens: 17,
                total_tokens: u64::MAX,
                details: None,
            }),
            ..ConversationStatistics::default()
        };
        let mut state = StatisticsLayout {
            statistics,
            ..StatisticsLayout::default()
        };
        state.local.view.statistics = state.statistics.clone();
        state
    }

    #[test]
    fn binary_statistics_preserve_tuple_admission_nonfinite_and_full_width_values() {
        for rate in [
            0.0,
            -0.0,
            1.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let state = timed(rate);
            let json = crate::runtime_client::checkpoint::bounded_bytes(
                &fields(&state, shared(&state)),
                16_384,
            )
            .unwrap();
            let binary = encode(&state).unwrap();
            let original: StoredStatistics = serde_json::from_slice(&json).unwrap();
            let restored: StoredStatistics = codec::decode_json_tuple(&binary, 16_384).unwrap();
            assert_eq!(
                serde_json::to_vec(&restored).unwrap(),
                serde_json::to_vec(&original).unwrap()
            );
            let rate_read = decode(&binary)
                .unwrap()
                .statistics
                .timing
                .unwrap()
                .output_tokens_per_second;
            if rate.is_finite() {
                assert_eq!(rate_read.unwrap().to_bits(), rate.to_bits());
            } else {
                assert_eq!(rate_read, None, "JSON nonfinite timing remains null");
            }
        }
        let mut state = timed(1.0);
        let mut lower = 0;
        let mut upper = 20_000;
        while lower + 1 < upper {
            let size = usize::midpoint(lower, upper);
            state.statistics.latest_turn = Some(ConversationTurnClock {
                attempt_id: Some(crate::runtime::identity::AttemptId::new("x".repeat(size))),
                attempt_id_deferred: false,
                attempt_id_source: None,
                started_at: chrono::DateTime::from_timestamp(1_700_000_000, 123).unwrap(),
                ended_at: None,
            });
            if crate::runtime_client::checkpoint::bounded_bytes(
                &fields(&state, shared(&state)),
                16_384,
            )
            .is_some()
            {
                lower = size;
            } else {
                upper = size;
            }
        }
        for size in [lower, upper] {
            state.statistics.latest_turn.as_mut().unwrap().attempt_id =
                Some(crate::runtime::identity::AttemptId::new("x".repeat(size)));
            assert_eq!(
                encode(&state).is_ok(),
                crate::runtime_client::checkpoint::bounded_bytes(
                    &fields(&state, shared(&state)),
                    16_384
                )
                .is_some()
            );
        }
    }

    #[test]
    fn binary_statistics_read_keeps_tuple_byte_boundary_and_seed_capacity() {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        let state = timed(1.0);
        let mut value = serde_json::to_value(fields(&state, shared(&state))).unwrap();
        value[0][7]["ignored"] = serde_json::json!("");
        let base = serde_json::to_vec(&value).unwrap().len();
        for length in [16_383, 16_384, 16_385] {
            value[0][7]["ignored"] = serde_json::json!("x".repeat(length - base));
            assert_eq!(serde_json::to_vec(&value).unwrap().len(), length);
            let bytes = codec::encode_admitted(&value).unwrap();
            assert_eq!(decode(&bytes).is_ok(), length <= 16_384);
        }
        value[0][7]["ignored"] = serde_json::json!(vec![1.0_f64; 3900]);
        assert!(serde_json::to_vec(&value).unwrap().len() < 16_384);
        let bytes = codec::encode_admitted(&value).unwrap();
        assert!(bytes.len() > 16_384 && bytes.len() <= codec::MAX_BYTES);
        let store = SqliteConversationStore::in_memory(
            crate::runtime::identity::ConversationId::generate(),
        )
        .unwrap();
        store.initialize(&[]).unwrap();
        let connection = store.lock().unwrap();
        connection
            .execute(
                "UPDATE rustx_store SET observation_statistics=?1 WHERE id=1",
                [&bytes],
            )
            .unwrap();
        assert_eq!(read(&connection, 0).unwrap(), state.statistics);
        connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json,observation_statistics) VALUES(1,'wide-statistics',1,'test','time','{}',?1)", [&bytes]).unwrap();
        let retained: Vec<u8> = connection
            .query_row(
                "SELECT observation_statistics FROM events WHERE sequence=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(decode(&retained).unwrap().statistics, state.statistics);
        connection
            .execute(
                "UPDATE rustx_store SET observation_statistics=?1 WHERE id=1",
                [serde_json::to_vec(&fields(&state, shared(&state))).unwrap()],
            )
            .unwrap();
        assert!(
            read(&connection, 0).is_err(),
            "schema 86 has no old JSON decoder"
        );
    }

    #[test]
    fn compact_statistics_shared_and_inherited_round_trip_and_shape() {
        for inherited in [false, true] {
            let mut state = StatisticsLayout::default();
            state.statistics.turns = 3;
            state.statistics.model_requests = 11;
            state.statistics.reported_usage = Some(crate::model::ModelUsage {
                input_tokens: u64::MAX,
                output_tokens: 17,
                total_tokens: u64::MAX,
                details: None,
            });
            state.activity.model_ms = Some(101);
            state.activity.tool_ms = Some(29);
            state.activity.ttft_ms = 37;
            state.activity.ttft_requests = 5;
            state.activity.decode_ms = 41;
            state.activity.decode_tokens = 43;
            state.local.view.statistics = state.statistics.clone();
            state.local.activity = state.activity.clone();
            state.local.view.duration.settled_ms = 47;
            state.local.active_start = Some(53);
            if inherited {
                state.local.view.statistics.turns = 1;
                state.local.activity.model_ms = Some(13);
            }
            let old = serde_json::to_vec(&serde_json::json!({
                "statistics": state.statistics,
                "activity": state.activity,
                "local": {
                    "view": state.local.view,
                    "activity": state.local.activity,
                    "active_start": state.local.active_start,
                },
            }))
            .unwrap();
            let bytes = encode(&state).unwrap();
            assert!(bytes.len() * 2 < old.len());
            let actual = decode(&bytes).unwrap();
            assert_eq!(actual.statistics, state.statistics);
            assert_eq!(actual.activity, state.activity);
            assert_eq!(actual.local.view, state.local.view);
            assert_eq!(actual.local.activity, state.local.activity);
            assert_eq!(actual.local.active_start, state.local.active_start);
            let mut value: serde_json::Value = codec::decode(&bytes).unwrap();
            assert_eq!(value[4].is_null(), !inherited);
            value.as_array_mut().unwrap().pop();
            assert!(decode(&codec::encode_admitted(&value).unwrap()).is_err());
            assert!(decode(&serde_json::to_vec(&fields(&state, shared(&state))).unwrap()).is_err());
            assert!(
                decode(&old).is_err(),
                "obsolete duplicated encoding is not a fallback"
            );
        }
    }
}
