//! Bounded observation regressions: event probe.

use crate::durable::ConversationStore;
use crate::durable::sqlite::observation::tests::member::derived_enrichment_proposal;
use crate::durable::sqlite::observation::tests::member::original_success_tool_result;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, InboxStore, JournalStore, ObservationStore, RequestStore};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::AgentStatusModuleId;
use crate::message::types::{AssistantContentBlock, ContributionEmission, MessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{AttemptId, EventId, MessageId, ToolCallId, ToolId};
use chrono::TimeZone;
use chrono::Utc;
use rusqlite::Connection;

#[derive(Debug, PartialEq)]
pub(super) struct EventSourceOracle {
    witness: Vec<u8>,
    fields: Vec<Vec<rusqlite::types::Value>>,
    segments: Vec<Vec<rusqlite::types::Value>>,
    frontiers: Vec<Vec<rusqlite::types::Value>>,
}
pub(super) fn event_source_rows(
    connection: &Connection,
    sql: &str,
    id: &str,
) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = connection.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([id], |row| {
            (0..columns).map(|column| row.get(column)).collect()
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
pub(super) fn event_source_oracle(connection: &Connection, sequence: u64) -> EventSourceOracle {
    let id = format!("journal/{sequence}");
    let oracle = EventSourceOracle {
        witness: connection
            .query_row(
                "SELECT observation_witness FROM events WHERE sequence=?1",
                [sequence],
                |row| row.get(0),
            )
            .unwrap(),
        fields: event_source_rows(
            connection,
            "SELECT * FROM state_observation_fields WHERE source_id=?1 ORDER BY path_json",
            &id,
        ),
        segments: event_source_rows(
            connection,
            "SELECT * FROM content_segments WHERE source_kind='observation_journal' AND source_id=?1 ORDER BY segment",
            &id,
        ),
        frontiers: event_source_rows(
            connection,
            "SELECT * FROM content_frontiers WHERE source_kind='observation_journal' AND source_id=?1",
            &id,
        ),
    };
    assert!(!oracle.fields.is_empty(), "actual event field rows");
    assert!(!oracle.segments.is_empty(), "actual original event bytes");
    assert!(
        !oracle.frontiers.is_empty(),
        "actual original event frontier"
    );
    oracle
}
pub(super) fn rebuild_event_source(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    legacy: bool,
) -> (EventSourceOracle, (usize, usize)) {
    let transaction = connection.unchecked_transaction().unwrap();
    let id = format!("journal/{}", event.sequence);
    for table in [
        "state_observation_fields",
        "content_segments",
        "content_frontiers",
    ] {
        transaction
            .execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [&id])
            .unwrap();
    }
    observation_event::take_preview_work();
    let available = if legacy {
        observation_event::index_legacy(&transaction, event)
    } else {
        observation_event::index(&transaction, event)
    }
    .unwrap();
    assert!(available);
    let work = observation_event::take_preview_work();
    let oracle = event_source_oracle(&transaction, event.sequence);
    transaction.rollback().unwrap();
    (oracle, work)
}
pub(super) fn assert_event_probe_oracle(
    store: &SqliteConversationStore,
    event: &RuntimeEventEnvelope,
    expected_work: (usize, usize),
) {
    let original_journal = store.read_events(None, 64).unwrap().events;
    let original_canonical = store.load_canonical().unwrap();
    let original_cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    let (legacy, legacy_work) = rebuild_event_source(&connection, event, true);
    assert_eq!(legacy_work, (24, 1));
    let (current, work) = rebuild_event_source(&connection, event, false);
    assert_eq!(work, expected_work);
    assert_eq!(
        current, legacy,
        "all witness, field geometry/headers, original segments/frontiers"
    );
    drop(connection);
    assert_eq!(
        store.read_events(None, 64).unwrap().events,
        original_journal
    );
    assert_eq!(store.load_canonical().unwrap(), original_canonical);
    assert_eq!(store.observation_read_cut().unwrap(), original_cut);
}

#[test]
fn t08_t14_t19_event_probe_pruning_preserves_original_progress_sources_and_large_id_deferral() {
    for giant_ids in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("probe-original-owner");
        let (mut proposal, mut call) = derived_enrichment_proposal();
        if giant_ids {
            call.id = ToolCallId::new("original-call水".repeat(80));
            call.tool_id = ToolId::new("original-tool🙂".repeat(80));
            let MessageBlock::Assistant(message) = &mut proposal else {
                unreachable!()
            };
            message.content = vec![AssistantContentBlock::ToolCall(call.clone())];
        }
        let (committed, _) = store
            .append_canonical_with_event(
                &proposal,
                envelope(
                    store.conversation_id(),
                    "probe-proposal",
                    Some(attempt.clone()),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: proposal.id().clone(),
                    },
                ),
            )
            .unwrap();
        assert_event_probe_oracle(&store, &committed, (1, 0));
        let started = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-tool-start",
                Some(attempt.clone()),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &started, (24, 1));
        for (index, message) in [None, Some(String::new()), Some("水🙂".repeat(73))]
            .into_iter()
            .enumerate()
        {
            let progress = crate::tools::types::ToolProgress {
                message,
                completed: Some(0.125),
                total: Some(3.5),
            };
            let progress = store
                .append_event(envelope(
                    store.conversation_id(),
                    &format!("probe-progress-{index}"),
                    Some(attempt.clone()),
                    RuntimeEvent::ToolExecutionProgress {
                        tool_call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        execution_id: None,
                        progress,
                    },
                ))
                .unwrap();
            assert_event_probe_oracle(&store, &progress, (4, 0));
        }
        // An unselected family retains the complete path policy even for
        // original giant text and opaque JSON numeric/property lookalikes.
        let result = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-tool-result",
                Some(attempt),
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call.id,
                    tool_id: call.tool_id,
                    result: original_success_tool_result(),
                },
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &result, (24, 1));
    }
}

pub(super) fn probe_generation() -> crate::model::generation_evidence::GenerationEvidence {
    crate::model::generation_evidence::GenerationEvidence {
        dispatch_after_start_ms: Some(1),
        first_output_ms: Some(2),
        last_output_ms: Some(3),
        terminal_ms: 4,
    }
}
#[test]
fn t08_t14_t19_event_probe_pruning_preserves_native_request_terminals_and_optional_generation() {
    for failed in [false, true] {
        for generation in [None, Some(probe_generation())] {
            let store = store();
            store
                .initialize(&[user_message("probe-input", "native input")])
                .unwrap();
            let mut snapshot =
                test_request_snapshot(store.load_head().unwrap().revision, invocation());
            freeze_test_input(&store, &mut snapshot, &[]);
            let started = store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .unwrap()
                .started;
            assert_event_probe_oracle(&store, &started, (1, 0));
            let finish_reason = ModelFinishReason::Other {
                reason: "original 水🙂\\\"\n".repeat(2000),
            };
            let mut error = crate::model::error::ModelError::timeout(
                crate::model::deadline::ModelTimeoutPhase::ResponseStart,
            );
            error.message = "original timeout 水🙂\\\"\n".repeat(2000);
            let kind = if failed {
                RuntimeEvent::ModelRequestFailed {
                    request_id: snapshot.request_id.clone(),
                    error,
                    usage: None,
                    generation,
                }
            } else {
                RuntimeEvent::ModelRequestCompleted {
                    request_id: snapshot.request_id.clone(),
                    finish_reason,
                    usage: None,
                    generation,
                }
            };
            let mut terminal = envelope(
                store.conversation_id(),
                "probe-provider-terminal",
                Some(snapshot.identity.attempt_id.clone()),
                kind,
            );
            terminal.turn_id = Some(snapshot.identity.turn.clone());
            let terminal = store.append_event(terminal).unwrap();
            assert_event_probe_oracle(&store, &terminal, (3, 0));
            if failed {
                let mut retry = terminal.clone();
                retry.event_id = EventId::new("probe-retry");
                retry.event = RuntimeEvent::ModelRetryScheduled {
                    failed_request_id: snapshot.request_id.clone(),
                    retry_number: 1,
                    retry_delay_ms: Some(2),
                };
                let retry = store.append_event(retry).unwrap();
                assert_event_probe_oracle(&store, &retry, (1, 0));
            }
            assert_eq!(
                store.load_request_snapshot(&snapshot.request_id).unwrap(),
                snapshot
            );
        }
    }
}

#[test]
fn t08_t14_t19_event_probe_pruning_preserves_attempt_and_adoption_sources() {
    for failed in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let accepted = store.accept_inbound(draft("native adopted 水🙂")).unwrap();
        let adoption = store
            .adopt_pending_batch(accepted.sequence, None)
            .unwrap()
            .adoption
            .unwrap();
        assert_event_probe_oracle(&store, &adoption, (1, 0));
        let attempt = AttemptId::new("probe-attempt");
        let start = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-attempt-start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &start, (1, 0));
        for (index, kind) in [RuntimeEvent::TurnStarted, RuntimeEvent::TurnCompleted]
            .into_iter()
            .enumerate()
        {
            let mut event = envelope(
                store.conversation_id(),
                &format!("probe-turn-{index}"),
                Some(attempt.clone()),
                kind,
            );
            event.turn_id = Some(crate::runtime::identity::TurnId::new("1"));
            let turn = store.append_event(event).unwrap();
            assert_event_probe_oracle(&store, &turn, (1, 0));
        }
        let kind = if failed {
            let mut error = crate::model::error::ModelError::timeout(
                crate::model::deadline::ModelTimeoutPhase::ResponseStart,
            );
            error.message = "native failure水🙂".repeat(2000);
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: crate::events::types::AttemptFailure::Model { error },
            }
        } else {
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: ModelFinishReason::Other {
                    reason: "native finish水🙂".repeat(2000),
                },
            }
        };
        let terminal = store
            .append_event(envelope(
                store.conversation_id(),
                "probe-attempt-terminal",
                Some(attempt),
                kind,
            ))
            .unwrap();
        assert_event_probe_oracle(&store, &terminal, (2, 0));
    }
}

#[test]
fn t08_t14_t19_event_probe_pruning_preserves_contribution_and_timeout_sources() {
    let store = store();
    store
        .initialize(&[user_message("probe-base", "native base")])
        .unwrap();
    let message_id = MessageId::new("probe-status");
    let status = agent_status_message(
        message_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    let mut snapshot = status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        message_id,
        vec![ContributionEmission {
            key: "native-probe".into(),
            fingerprint: "native-fingerprint".into(),
        }],
    );
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&status));
    let started = store
        .commit_model_turn_start(&[status], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(started.contribution_emissions.len(), 1);
    assert_event_probe_oracle(&store, &started.started, (1, 0));
    assert_event_probe_oracle(&store, &started.contribution_emissions[0], (1, 0));
    let attempt = AttemptId::new("probe-timeout");
    let started = store
        .append_event(envelope(
            store.conversation_id(),
            "probe-timeout-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    assert_event_probe_oracle(&store, &started, (1, 0));
    let timeout = store
        .append_event(envelope(
            store.conversation_id(),
            "probe-timeout-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    assert_event_probe_oracle(&store, &timeout, (1, 0));
}
