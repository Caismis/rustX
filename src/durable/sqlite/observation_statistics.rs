//! Independent statistical facts and self-contained full historical states.
//! They grant no execution authority and publish neither events nor cursors.
use super::observation_codec as codec;
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
#[cfg(test)]
use crate::durable::CanonicalStore;
use crate::durable::reading::TurnExecution;
use crate::durable::response::{add_usage, timing::ActivityFold};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::observation::statistics::{ConversationStatistics, ConversationTurnClock};
mod contribution;
mod owner;
mod record;
pub(super) use owner::FoldCache;
#[derive(Default)]
struct StatisticsLayout {
    statistics: ConversationStatistics,
    activity: TurnExecution,
    local: LocalStatisticsLayout,
}
#[derive(Default)]
struct LocalStatisticsLayout {
    view: crate::observation::statistics::AgentStatistics,
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
    crate::observation::statistics::AgentDuration,
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
    #[cfg(test)]
    WRITE_WORK.with(|work| {
        let (full, facts) = work.get();
        work.set((full + 1, facts));
    });
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
            view: crate::observation::statistics::AgentStatistics {
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
    Ok(record::layout(connection, through)?.state)
}
fn at_cut(
    connection: &Connection,
    through: u64,
) -> Result<StatisticsLayout, ConversationStoreError> {
    let actual: u64 = connection
        .prepare_cached("SELECT next_event_sequence FROM rustx_store WHERE id=1")
        .and_then(|mut statement| statement.query_row([], |row| row.get(0)))
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
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
/// this transaction. Verify the selected self-contained full record.
pub(super) fn read_validated(
    validated: &super::observation::ValidatedCut<'_, '_>,
) -> Result<ConversationStatistics, ConversationStoreError> {
    Ok(layout(validated.connection()?, validated.expected().read.journal)?.statistics)
}
pub(super) fn agent_read(
    connection: &Connection,
    through: u64,
) -> Result<crate::observation::statistics::AgentStatistics, ConversationStoreError> {
    Ok(at_cut(connection, through)?.local.view)
}

pub(super) fn requires_statistics(event: &RuntimeEventEnvelope) -> bool {
    if event.attempt_id.is_none() {
        return false;
    }
    match &event.event {
        RuntimeEvent::ModelRequestStarted { owner, .. }
        | RuntimeEvent::ModelRequestCompleted { owner, .. }
        | RuntimeEvent::ModelRequestFailed { owner, .. } => matches!(
            owner,
            crate::model::snapshot::ModelRequestOwner::Agent { .. }
        ),
        RuntimeEvent::AttemptStarted { .. }
        | RuntimeEvent::TurnStarted
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
        | RuntimeEvent::CompactionCompleted { .. } => true,
        _ => false,
    }
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
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}
pub(super) fn is_trace_anchor(event: &RuntimeEvent) -> bool {
    if let RuntimeEvent::ModelRequestStarted { owner, .. } = event
        && !matches!(
            owner,
            crate::model::snapshot::ModelRequestOwner::Agent { .. }
        )
    {
        return false;
    }
    super::observation_event::native_kind(event)
        .ok()
        .and_then(|kind| crate::durable::trace::TraceFactKind::parse(&kind))
        .is_some_and(crate::durable::trace::TraceFactKind::is_anchor)
}
pub(super) struct Prepared {
    pub(super) trace_total: Option<u64>,
    pub(super) statistics: Option<super::append_facts::Captured<ConversationStatistics>>,
    fold: Option<(record::Frame, u64)>,
}
impl Prepared {
    pub(super) fn remember(self, cache: &FoldCache) {
        if let (Some((fold, data)), Some(capture)) = (self.fold, self.statistics) {
            cache.stage(fold, data, capture.bytes);
        }
    }
}

pub(super) fn prepare_cached(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    cache: Option<&FoldCache>,
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
        fold: None,
    };
    let Some(attempt) = &event.attempt_id else {
        return Ok(prepared);
    };
    if !requires_statistics(event) {
        return Ok(prepared);
    }
    let through = event.sequence.saturating_sub(1);
    let (data, cached) = match cache {
        Some(cache) => cache.take(connection, through)?,
        None => (0, None),
    };
    let mut frame = match cached {
        Some(frame) => frame,
        None => record::layout(connection, through)?,
    };
    let contribution =
        contribution::Contribution::capture(connection, event, attempt, &frame.state)?;
    contribution.apply(event.sequence, &mut frame.state)?;
    frame.state = normalized_fold(frame.state);
    let bytes = record::capture(event.sequence, &contribution, &frame.state)?;
    let value = frame.state.statistics.clone();
    frame.sequence = event.sequence;
    if cache.is_some() {
        prepared.fold = Some((frame, data));
    }
    prepared.statistics = Some(super::append_facts::Captured { bytes, value });
    Ok(prepared)
}
// These are exactly the fields retained by the private tuple decoder.
fn normalized_fold(mut state: StatisticsLayout) -> StatisticsLayout {
    let shared = shared(&state);
    state.activity = activity_from_fields(activity_fields(&state.activity));
    state.local.activity = activity_from_fields(activity_fields(&state.local.activity));
    state.local.view.occupancy = None;
    if shared {
        state.local.view.statistics.clone_from(&state.statistics);
        state.local.activity.clone_from(&state.activity);
    }
    for statistics in [&mut state.statistics, &mut state.local.view.statistics] {
        if let Some(timing) = &mut statistics.timing {
            timing.output_tokens_per_second = timing
                .output_tokens_per_second
                .filter(|value| value.is_finite());
        }
    }
    state
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
                    .prepare_cached(concat!("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_attempt_kind_idx WHERE attempt_key = ", crate::durable::sqlite::event_owner_key::attempt_parameter!("?1"), " AND json_extract(event_json,'$.event.type')='assistant_message_committed' AND sequence<?2)"))
                    .and_then(|mut statement| statement.query_row(params![attempt.as_str(),event.sequence],|row|row.get(0)))
                    .map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
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
        let fact:Option<(u64,String)>=connection.prepare_cached(concat!("SELECT sequence,event_timestamp FROM events INDEXED BY events_observation_tool_idx WHERE json_extract(event_json,'$.event.tool_call_id')=?1 AND attempt_key = ", crate::durable::sqlite::event_owner_key::attempt_parameter!("?2"), " AND json_extract(event_json,'$.event.type')=?3 AND sequence<?4 ORDER BY sequence DESC LIMIT 1")).and_then(|mut statement| statement.query_row(params![tool_call_id.as_str(),attempt.as_str(),kind,event.sequence],|row|Ok((row.get(0)?,row.get(1)?)))).optional().map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
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
pub(super) fn trace_total(
    connection: &Connection,
    through: u64,
) -> Result<u64, ConversationStoreError> {
    let (missing, total): (bool, Option<u64>) = connection
        .prepare_cached(super::event_heads::TRACE_TOTAL)
        .and_then(|mut statement| {
            statement.query_row([through], |row| Ok((row.get(0)?, row.get(1)?)))
        })
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
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
fn record_read(rows: usize, bytes: usize) {
    READ_WORK.with(|work| {
        let (prior, total, original) = work.get();
        work.set((prior + rows, total + bytes, original));
    });
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
                input_tokens: Some(u64::MAX),
                output_tokens: Some(17),
                total_tokens: Some(u64::MAX),
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
            let json =
                crate::observation::bounded::bounded_bytes(&fields(&state, shared(&state)), 16_384)
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
            if crate::observation::bounded::bounded_bytes(&fields(&state, shared(&state)), 16_384)
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
                crate::observation::bounded::bounded_bytes(&fields(&state, shared(&state)), 16_384)
                    .is_some()
            );
        }
    }

    #[test]
    fn binary_statistics_read_keeps_tuple_byte_boundary_and_seed_capacity() {
        use crate::durable::SqliteConversationStore;
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
        let full = record::test_record(1, &bytes);
        connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,event_json,observation_statistics) VALUES(1,'wide-statistics',1,(SELECT conversation_id FROM rustx_store WHERE id=1),'time','{}',?1)", [&full]).unwrap();
        let retained: Vec<u8> = connection
            .query_row(
                "SELECT observation_statistics FROM events WHERE sequence=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE rustx_store SET next_event_sequence=2 WHERE id=1",
                [],
            )
            .unwrap();
        let (_, retained_state) = record::parts(1, &retained).unwrap();
        assert_eq!(decode(retained_state).unwrap().statistics, state.statistics);
        assert_eq!(read(&connection, 1).unwrap(), state.statistics);
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
                input_tokens: Some(u64::MAX),
                output_tokens: Some(17),
                total_tokens: Some(u64::MAX),
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

#[cfg(test)]
mod fold_normalization_tests {
    use super::*;
    #[test]
    fn cached_statistics_state_matches_stored_normalization_including_shared_signed_zero() {
        for rate in [f64::NAN, f64::INFINITY, -0.0, 0.0, 123.5] {
            let mut state = StatisticsLayout::default();
            state.statistics.timing = Some(crate::durable::response::ConversationTiming {
                output_tokens_per_second: Some(rate),
                ..Default::default()
            });
            state.local.view.statistics = state.statistics.clone();
            if rate == 0.0 {
                state
                    .local
                    .view
                    .statistics
                    .timing
                    .as_mut()
                    .unwrap()
                    .output_tokens_per_second = Some(-rate);
            }
            state.activity.steps = 999;
            state.local.activity.steps = 999;
            let bytes = encode(&state).unwrap();
            let decoded = decode(&bytes).unwrap();
            let normalized = normalized_fold(state);
            assert_eq!(
                serde_json::to_vec(&fields(&normalized, shared(&normalized))).unwrap(),
                serde_json::to_vec(&fields(&decoded, shared(&decoded))).unwrap()
            );
            assert_eq!(normalized.activity, decoded.activity);
            assert_eq!(normalized.local.activity, decoded.local.activity);
            assert!(normalized.local.view.occupancy.is_none());
        }
    }
}

#[cfg(test)]
thread_local! {static WRITE_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn write_work() -> (usize, usize) {
    WRITE_WORK.with(|work| work.replace((0, 0)))
}
#[cfg(test)]
pub(super) fn fold_work() -> (usize, usize) {
    owner::work()
}

#[cfg(test)]
pub(super) fn stored_parts(
    sequence: u64,
    bytes: &[u8],
) -> Result<(&[u8], &[u8]), ConversationStoreError> {
    record::parts(sequence, bytes)
}
