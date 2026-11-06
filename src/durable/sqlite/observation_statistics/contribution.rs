//! Independent, bounded statistical facts. Historical reads never open `event_json`.
use super::{
    ActivityFold, Connection, ConversationStatistics, ConversationStoreError,
    ConversationTurnClock, NativeFacts, RuntimeEvent, RuntimeEventEnvelope, StatisticsLayout,
    TurnExecution, add_count, add_usage, params, settle_duration, shared, storage,
};
use crate::model::ModelUsage;
use crate::model::generation_evidence::GenerationEvidence;
use crate::runtime::identity::AttemptId;

const OBSERVE: u8 = 0;
const START: u8 = 1;
const STEP: u8 = 2;
const REQUEST: u8 = 3;
const OUTCOME: u8 = 4;
const TOOL: u8 = 5;
const COMPLETE: u8 = 6;
const TERMINAL: u8 = 7;

// The second slot is the encoded size of a deferred ID. Timestamp, row
// coordinate, fixed path and presence flags are derived rather than repeated.
type StoredIdentity = (Option<AttemptId>, Option<u64>);
pub(super) type Stored = (
    u8,
    (i64, u32),
    Option<StoredIdentity>,
    u8,
    bool,
    Option<ModelUsage>,
    Option<GenerationEvidence>,
    Option<u64>,
);
pub(super) struct Contribution {
    kind: u8,
    at: chrono::DateTime<chrono::Utc>,
    clock: Option<ConversationTurnClock>,
    same: u8,
    completed: bool,
    usage: Option<ModelUsage>,
    generation: Option<GenerationEvidence>,
    tool_ms: Option<u64>,
}
impl Contribution {
    pub(super) fn capture(
        connection: &Connection,
        event: &RuntimeEventEnvelope,
        attempt: &AttemptId,
        state: &StatisticsLayout,
    ) -> Result<Self, ConversationStoreError> {
        let facts = NativeFacts::read(connection, event, attempt)?;
        let kind = match &event.event {
            RuntimeEvent::AttemptStarted { .. } => START,
            RuntimeEvent::TurnStarted => STEP,
            RuntimeEvent::ModelRequestStarted { .. } => REQUEST,
            RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. } => OUTCOME,
            RuntimeEvent::ToolExecutionCompleted { .. }
            | RuntimeEvent::ToolExecutionFailed { .. } => TOOL,
            RuntimeEvent::AttemptCompleted { .. } => COMPLETE,
            RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. } => TERMINAL,
            _ => OBSERVE,
        };
        let mut same = 0;
        if matches!(kind, COMPLETE | TERMINAL) {
            if clock_matches(connection, &state.statistics, attempt)? {
                same |= 1;
            }
            if clock_matches(connection, &state.local.view.statistics, attempt)? {
                same |= 2;
            }
        }
        if kind != START
            && let Some(start) = state.local.active_start
        {
            let matches: bool = connection
                .prepare_cached("SELECT attempt_id=?2 FROM events WHERE sequence=?1")
                .and_then(|mut statement| {
                    statement.query_row(params![start, attempt.as_str()], |row| row.get(0))
                })
                .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
            if matches {
                same |= 4;
            }
        }
        let clock = if kind == START {
            Some(ConversationTurnClock {
                attempt_id: (attempt.as_str().len() <= 512).then(|| attempt.clone()),
                attempt_id_deferred: attempt.as_str().len() > 512,
                attempt_id_source: if attempt.as_str().len() > 512 {
                    Some(crate::observation::tool::JournalFieldRef {
                        sequence: event.sequence,
                        path: vec!["attempt_id".into()],
                        encoded_bytes: super::super::observation_request_summary::serialized_bytes(
                            attempt,
                        )?,
                    })
                } else {
                    None
                },
                started_at: event.timestamp,
                ended_at: None,
            })
        } else {
            None
        };
        let (usage, generation) = match &event.event {
            RuntimeEvent::ModelRequestCompleted {
                usage, generation, ..
            }
            | RuntimeEvent::ModelRequestFailed {
                usage, generation, ..
            } => (usage.clone(), *generation),
            _ => (None, None),
        };
        Ok(Self {
            kind,
            at: event.timestamp,
            clock,
            same,
            completed: facts.completed_response,
            usage,
            generation,
            tool_ms: facts.tool_span,
        })
    }
    pub(super) fn fields(&self) -> impl serde::Serialize + '_ {
        (
            self.kind,
            (self.at.timestamp(), self.at.timestamp_subsec_nanos()),
            self.clock.as_ref().map(|clock| {
                (
                    &clock.attempt_id,
                    clock
                        .attempt_id_source
                        .as_ref()
                        .map(|source| source.encoded_bytes),
                )
            }),
            self.same,
            self.completed,
            &self.usage,
            &self.generation,
            self.tool_ms,
        )
    }
    pub(super) fn restore(stored: Stored, sequence: u64) -> Result<Self, ConversationStoreError> {
        let (kind, (seconds, nanos), identity, same, completed, usage, generation, tool_ms) =
            stored;
        let at = chrono::DateTime::from_timestamp(seconds, nanos)
            .ok_or_else(|| storage("statistics contribution time invalid"))?;
        let clock = identity.map(|(attempt_id, encoded_bytes)| ConversationTurnClock {
            attempt_id,
            attempt_id_deferred: encoded_bytes.is_some(),
            attempt_id_source: encoded_bytes.map(|encoded_bytes| {
                crate::observation::tool::JournalFieldRef {
                    sequence,
                    path: vec!["attempt_id".into()],
                    encoded_bytes,
                }
            }),
            started_at: at,
            ended_at: None,
        });
        if kind > TERMINAL
            || same > 7
            || (kind == START) != clock.is_some()
            || (kind != OUTCOME && (usage.is_some() || generation.is_some()))
            || (kind != TOOL && tool_ms.is_some())
            || (kind != COMPLETE && completed)
            || (!matches!(kind, COMPLETE | TERMINAL) && same & 3 != 0)
            || (kind == START && same != 0)
        {
            return Err(storage("statistics contribution shape invalid"));
        }
        if let Some(clock) = &clock {
            let bounded = clock
                .attempt_id
                .as_ref()
                .is_none_or(|id| id.as_str().len() <= 512);
            let identity = matches!(
                (
                    &clock.attempt_id,
                    &clock.attempt_id_source,
                    clock.attempt_id_deferred
                ),
                (Some(_), None, false) | (None, Some(_), true)
            );
            if !bounded
                || !identity
                || clock
                    .attempt_id_source
                    .as_ref()
                    .is_some_and(|source| source.encoded_bytes <= 512)
            {
                return Err(storage("statistics contribution clock invalid"));
            }
        }
        Ok(Self {
            kind,
            at,
            clock,
            same,
            completed,
            usage,
            generation,
            tool_ms,
        })
    }
    pub(super) fn terminal(&self) -> bool {
        matches!(self.kind, COMPLETE | TERMINAL)
    }
    fn totals(
        &self,
        statistics: &mut ConversationStatistics,
        activity: &mut TurnExecution,
        close: u8,
    ) -> Result<(), ConversationStoreError> {
        let mut fold = ActivityFold::default();
        fold.absorb(activity);
        match self.kind {
            START => {
                add_count(&mut statistics.turns, 1)?;
                statistics.latest_turn.clone_from(&self.clock);
            }
            STEP => add_count(&mut statistics.steps, 1)?,
            REQUEST => add_count(&mut statistics.model_requests, 1)?,
            OUTCOME => {
                if self
                    .generation
                    .as_ref()
                    .and_then(GenerationEvidence::time_to_first_output_ms)
                    .is_some()
                {
                    let mut reading = TurnExecution::default();
                    fold.record(&mut reading);
                    add_count(&mut reading.ttft_requests, 1)?;
                }
                fold.request(self.generation.as_ref(), self.usage.as_ref());
                if let Some(usage) = &self.usage {
                    add_count(&mut statistics.requests_with_usage, 1)?;
                    add_usage(&mut statistics.reported_usage, usage);
                }
            }
            TOOL => {
                if let Some(span) = self.tool_ms {
                    fold.tool_span_ms(span);
                }
            }
            COMPLETE => add_count(
                &mut statistics.completed_responses,
                u64::from(self.completed),
            )?,
            _ => {}
        }
        if self.same & close != 0
            && let Some(clock) = &mut statistics.latest_turn
        {
            clock.ended_at = Some(self.at);
        }
        statistics.timing = fold.summary();
        fold.record(activity);
        Ok(())
    }
    pub(super) fn apply(
        &self,
        sequence: u64,
        state: &mut StatisticsLayout,
    ) -> Result<(), ConversationStoreError> {
        let was_shared = shared(state);
        self.totals(&mut state.statistics, &mut state.activity, 1)?;
        if was_shared && self.same & 1 == (self.same & 2) >> 1 {
            state.local.view.statistics.clone_from(&state.statistics);
            state.local.activity.clone_from(&state.activity);
        } else {
            self.totals(
                &mut state.local.view.statistics,
                &mut state.local.activity,
                2,
            )?;
        }
        if self.kind == START {
            if let Some(at) = state
                .local
                .view
                .duration
                .active
                .as_ref()
                .map(|active| active.observed_at)
            {
                settle_duration(&mut state.local, at);
            }
            state.local.active_start = Some(sequence);
            state.local.view.duration.active =
                Some(crate::observation::statistics::AgentActiveInterval {
                    started_at: self.at,
                    observed_at: self.at,
                    running: false,
                });
        } else if self.same & 4 != 0 {
            if let Some(active) = &mut state.local.view.duration.active {
                active.observed_at = active.observed_at.max(self.at);
            }
            if self.terminal() {
                settle_duration(&mut state.local, self.at);
            }
        }
        Ok(())
    }
}
fn clock_matches(
    connection: &Connection,
    statistics: &ConversationStatistics,
    attempt: &AttemptId,
) -> Result<bool, ConversationStoreError> {
    let Some(clock) = &statistics.latest_turn else {
        return Ok(false);
    };
    if let Some(id) = &clock.attempt_id {
        return Ok(id == attempt);
    }
    let source = clock
        .attempt_id_source
        .as_ref()
        .ok_or_else(|| storage("native turn clock identity evidence unavailable"))?;
    connection
        .prepare_cached("SELECT attempt_id=?2 FROM events WHERE sequence=?1")
        .and_then(|mut statement| {
            statement.query_row(params![source.sequence, attempt.as_str()], |row| row.get(0))
        })
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))
}
