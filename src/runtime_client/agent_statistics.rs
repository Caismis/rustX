//! Child-owned statistics, independent of transcript windows and live activation.
//! Like Harness's session meters and subagent timing projection, durable facts
//! supply all readings; clients only interpolate a currently running clock.
use super::response::{ConversationStatistics, ConversationTurnClock};
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::response::{add_usage, timing::ActivityFold};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEvent;
use crate::runtime::identity::AttemptId;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentStatistics {
    pub statistics: ConversationStatistics,
    pub occupancy: Option<crate::context::occupancy::ContextOccupancy>,
    pub duration: AgentDuration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDuration {
    /// Sum of this child's closed working intervals, excluding inactive gaps.
    pub settled_ms: u64,
    /// Last interval, if its terminal event has not been observed.
    pub active: Option<AgentActiveInterval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentActiveInterval {
    pub started_at: DateTime<Utc>,
    /// Last durable evidence. Non-running children freeze here, including recovery.
    pub observed_at: DateTime<Utc>,
    /// Authoritative live lifecycle, never inferred from a missing terminal.
    pub running: bool,
}

/// Cache synchronization is per durable Conversation, never per process.
#[derive(Default)]
pub(crate) struct StatisticsCache(
    std::sync::Mutex<
        std::collections::BTreeMap<
            crate::runtime::identity::ConversationId,
            std::sync::Arc<std::sync::Mutex<StatisticsFold>>,
        >,
    >,
);
impl StatisticsCache {
    pub(crate) fn entry(
        &self,
        id: crate::runtime::identity::ConversationId,
    ) -> std::sync::Arc<std::sync::Mutex<StatisticsFold>> {
        std::sync::Arc::clone(
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(id)
                .or_default(),
        )
    }
}

#[derive(Default, Clone)]
pub(crate) struct StatisticsFold {
    through: u64,
    view: AgentStatistics,
    activity: ActivityFold,
    active_attempt: Option<AttemptId>,
    has_assistant: bool,
}
impl StatisticsFold {
    /// Commit the cache only after the complete finite read succeeds. Repeated
    /// reads at the same frontier do no event work; recovery simply replays.
    pub(crate) fn read(
        &mut self,
        store: &dyn ConversationStore,
        through: u64,
        running_since: impl FnOnce() -> Option<DateTime<Utc>>,
    ) -> Result<AgentStatistics, ConversationStoreError> {
        if through != self.through {
            let mut next = if through < self.through {
                Self::default()
            } else {
                self.clone()
            };
            let mut after = next.through;
            loop {
                let events = store.read_presentation_events(&FactQuery {
                    scope: FactScope::All,
                    kinds: vec![
                        "attempt_started",
                        "turn_started",
                        "model_request_started",
                        "model_request_completed",
                        "model_request_failed",
                        "assistant_message_committed",
                        "attempt_completed",
                        "attempt_cancelled",
                        "attempt_failed",
                        "attempt_timed_out",
                        "attempt_limit_exceeded",
                        "tool_execution_started",
                        "tool_execution_completed",
                        "tool_execution_failed",
                        "compaction_started",
                        "compaction_completed",
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
                    next.apply(event.attempt_id, event.timestamp, &event.event);
                }
            }
            next.view.statistics.timing = next.activity.summary();
            next.view.occupancy = crate::context::occupancy::read(store, through)?;
            next.through = through;
            *self = next;
        }
        let mut view = self.view.clone();
        if let Some(active) = &mut view.duration.active {
            active.running = running_since().is_some_and(|since| active.started_at >= since);
        }
        Ok(view)
    }

    fn settle(&mut self, at: DateTime<Utc>) {
        if let Some(active) = self.view.duration.active.take() {
            self.view.duration.settled_ms = self.view.duration.settled_ms.saturating_add(
                u64::try_from(
                    at.signed_duration_since(active.started_at)
                        .num_milliseconds(),
                )
                .unwrap_or(0),
            );
        }
        self.active_attempt = None;
    }

    fn apply(&mut self, id: Option<AttemptId>, at: DateTime<Utc>, event: &RuntimeEvent) {
        if matches!(event, RuntimeEvent::AttemptStarted { .. }) {
            // An abandoned interval must not bridge a process restart or idle gap.
            if let Some(active) = &self.view.duration.active {
                self.settle(active.observed_at);
            }
        } else if id.is_some()
            && id == self.active_attempt
            && let Some(active) = &mut self.view.duration.active
        {
            active.observed_at = active.observed_at.max(at);
        }
        let Some(id) = id else {
            return;
        };
        let stats = &mut self.view.statistics;
        match event {
            RuntimeEvent::AttemptStarted { .. } => {
                self.has_assistant = false;
                stats.turns += 1;
                stats.latest_turn = Some(ConversationTurnClock {
                    attempt_id: id.clone(),
                    started_at: at,
                    ended_at: None,
                });
                self.active_attempt = Some(id);
                self.view.duration.active = Some(AgentActiveInterval {
                    started_at: at,
                    observed_at: at,
                    running: false,
                });
            }
            RuntimeEvent::AssistantMessageCommitted { .. } => self.has_assistant = true,
            RuntimeEvent::TurnStarted => stats.steps += 1,
            RuntimeEvent::ModelRequestStarted { .. } => stats.model_requests += 1,
            RuntimeEvent::ModelRequestCompleted {
                usage, generation, ..
            }
            | RuntimeEvent::ModelRequestFailed {
                usage, generation, ..
            } => {
                self.activity.request(generation.as_ref(), usage.as_ref());
                if let Some(usage) = usage {
                    stats.requests_with_usage += 1;
                    add_usage(&mut stats.reported_usage, usage);
                }
            }
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } => {
                self.activity.tool_started(id, tool_call_id.clone(), at);
            }
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
            | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => {
                self.activity.tool_settled(id, tool_call_id.clone(), at);
            }
            RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. } => {
                if self.has_assistant
                    && matches!(
                        event,
                        RuntimeEvent::AttemptCompleted {
                            finish_reason: crate::model::finish::ModelFinishReason::Stop
                                | crate::model::finish::ModelFinishReason::Refusal,
                            ..
                        }
                    )
                {
                    stats.completed_responses += 1;
                }
                if let Some(clock) = stats
                    .latest_turn
                    .as_mut()
                    .filter(|clock| clock.attempt_id == id)
                {
                    clock.ended_at = Some(at);
                }
                if self.active_attempt.as_ref() == Some(&id) {
                    self.settle(at);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    #[tokio::test(flavor = "current_thread")]
    async fn independent_conversations_do_not_share_the_blocked_fold_lock() {
        use crate::durable::SqliteConversationStore;
        use crate::runtime::identity::ConversationId;
        use std::sync::Arc;
        let cache = Arc::new(StatisticsCache::default());
        let a = ConversationId::generate();
        let b = ConversationId::generate();
        let held = cache.entry(a.clone());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = tokio::task::spawn_blocking(move || {
            let _lock = held.lock().unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.await.unwrap();
        let owner = Arc::clone(&cache);
        let (requested_tx, requested_rx) = tokio::sync::oneshot::channel();
        let waiting_read = tokio::task::spawn_blocking(move || {
            let fold = owner.entry(a);
            requested_tx.send(()).unwrap();
            let _lock = fold.lock().unwrap();
        });
        requested_rx.await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store =
            SqliteConversationStore::open(b.clone(), &directory.path().join("b.sqlite")).unwrap();
        let independent = tokio::task::spawn_blocking(move || {
            let attempt = AttemptId::new("b");
            store
                .append_event(crate::events::types::RuntimeEventEnvelope {
                    schema_version: 1,
                    event_id: crate::runtime::identity::EventId::new("b-start"),
                    sequence: 0,
                    conversation_id: b.clone(),
                    attempt_id: Some(attempt.clone()),
                    turn_id: None,
                    timestamp: Utc::now(),
                    event: RuntimeEvent::AttemptStarted {
                        attempt_id: attempt,
                    },
                })
                .unwrap();
            let through = store.presentation_frontier().unwrap();
            cache
                .entry(b)
                .lock()
                .unwrap()
                .read(&store, through, || None)
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(independent.statistics.turns, 1);
        assert!(!independent.duration.active.unwrap().running);
        assert!(!waiting_read.is_finished());
        release_tx.send(()).unwrap();
        holder.await.unwrap();
        waiting_read.await.unwrap();
    }

    #[test]
    fn timeout_and_abandoned_intervals_never_include_inactive_gaps() {
        let mut fold = StatisticsFold::default();
        let at = |seconds: i64| Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap();
        for (id, start, end, terminal) in [
            ("one", 0, 10, true),
            ("two", 100, 105, false),
            ("three", 200, 207, true),
        ] {
            let id = AttemptId::new(id);
            fold.apply(
                Some(id.clone()),
                at(start),
                &RuntimeEvent::AttemptStarted {
                    attempt_id: id.clone(),
                },
            );
            fold.apply(Some(id.clone()), at(end), &RuntimeEvent::TurnStarted);
            if terminal {
                fold.apply(
                    Some(id.clone()),
                    at(end),
                    &RuntimeEvent::AttemptTimedOut { attempt_id: id },
                );
            }
        }
        assert_eq!(fold.view.duration.settled_ms, 22_000);
        assert!(fold.view.duration.active.is_none());
        assert_eq!(fold.view.statistics.turns, 3);
        assert_eq!(fold.view.statistics.completed_responses, 0);
    }
}
