//! Auxiliary-owned request facts in Trace observation.
use super::*;
use crate::durable::observation::{
    ObservationReadCut, RequestOwnerKind, TraceFactHeader, TraceFactKind, TraceFactQuery,
    TraceFactScope,
};
use crate::model::auxiliary::{
    AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryFingerprint, AuxiliaryFinishReason, AuxiliaryInputEvidence,
    AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
    AuxiliaryUsageCompleteness,
};
use crate::model::snapshot::ModelRequestOwner;
use crate::runtime::RequestId;
use crate::runtime::identity::AuxiliaryCallId;
use crate::runtime_client::trace::TraceProjection;
use chrono::TimeZone;

fn trace_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 19, 8, 0, 0)
        .single()
        .unwrap()
}

fn primary_start(
    store: &SqliteConversationStore,
    ordinal: u32,
) -> (RequestSnapshot, RuntimeEventEnvelope) {
    let mut snapshot = progress_request_snapshot(
        &format!("aux-trace-primary-{ordinal}"),
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(store, &mut snapshot, &[]);
    let start = store
        .commit_model_turn_start(&[], &snapshot, trace_time())
        .unwrap()
        .started;
    (snapshot, start)
}

fn primary_terminal(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    completed: bool,
) -> RuntimeEventEnvelope {
    let owner = ModelRequestOwner::Agent {
        identity: snapshot.identity.clone(),
    };
    let event = if completed {
        RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        }
    } else {
        RuntimeEvent::ModelRequestFailed {
            owner,
            request_id: snapshot.request_id.clone(),
            error: crate::model::error::ModelError::timeout(
                crate::model::deadline::ModelTimeoutPhase::ResponseStart,
            ),
            usage: None,
            generation: None,
        }
    };
    let mut terminal = envelope(
        store.conversation_id(),
        &format!("primary-trace-terminal:{}", snapshot.request_id),
        Some(snapshot.identity.attempt_id.clone()),
        event,
    );
    terminal.turn_id = Some(snapshot.identity.turn.clone());
    terminal.timestamp = trace_time() + chrono::Duration::seconds(1);
    store.append_event(terminal).unwrap()
}

fn auxiliary_call(store: &SqliteConversationStore, ordinal: u32) -> AuxiliaryCallStart {
    let fingerprint = AuxiliaryFingerprint::new(format!("{ordinal:064x}")).unwrap();
    let call = AuxiliaryCallStart {
        call_id: AuxiliaryCallId::new(format!("aux_019a0000-0000-7000-8000-{ordinal:012x}")),
        purpose: AuxiliaryAttribution::new("suggested_input").unwrap(),
        trigger: AuxiliaryAttribution::new("conversation_idle").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: fingerprint.clone(),
        invocation_fingerprint: fingerprint,
        selected_message_ids: Vec::new(),
    };
    store
        .commit_auxiliary_call_start(&call, trace_time())
        .unwrap();
    call
}

fn auxiliary_start(
    store: &SqliteConversationStore,
    call: &AuxiliaryCallStart,
) -> (AuxiliaryRequestStart, RuntimeEventEnvelope) {
    let request = AuxiliaryRequestStart {
        call_id: call.call_id.clone(),
        ordinal: 0,
        request_id: RequestId::new(format!("aux-trace-request:{}", call.call_id)),
        model: "auxiliary-trace-model".into(),
    };
    let start = store
        .commit_auxiliary_request_start(&request, trace_time())
        .unwrap()
        .event;
    (request, start)
}

fn auxiliary_settle(
    store: &SqliteConversationStore,
    request: &AuxiliaryRequestStart,
    completed: bool,
) {
    let owner = ModelRequestOwner::Auxiliary {
        call_id: request.call_id.clone(),
        ordinal: request.ordinal,
    };
    let (terminal, observed, outcome) = if completed {
        (
            RuntimeEvent::ModelRequestCompleted {
                owner,
                request_id: request.request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: None,
                generation: None,
            },
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: AuxiliaryFinishReason::Stop,
            }),
            AuxiliaryCallOutcome::Completed,
        )
    } else {
        (
            RuntimeEvent::ModelRequestFailed {
                owner,
                request_id: request.request_id.clone(),
                error: crate::model::error::ModelError::timeout(
                    crate::model::deadline::ModelTimeoutPhase::ResponseStart,
                ),
                usage: None,
                generation: None,
            },
            None,
            AuxiliaryCallOutcome::Failed,
        )
    };
    let terminal = crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        trace_time() + chrono::Duration::seconds(1),
        terminal,
    )
    .unwrap();
    let settled = crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        trace_time() + chrono::Duration::seconds(1),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: request.call_id.clone(),
            outcome,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id: request.request_id.clone(),
                observed_provider_terminal: observed,
                usage: None,
                usage_completeness: AuxiliaryUsageCompleteness::Unreported,
                remote_execution_uncertainty: !completed,
            }),
        },
    )
    .unwrap();
    store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
}

fn requests(scope: TraceFactScope, ascending: bool, limit: usize) -> TraceFactQuery {
    TraceFactQuery {
        scope,
        kinds: vec![
            TraceFactKind::ModelRequestStarted,
            TraceFactKind::ModelRequestCompleted,
            TraceFactKind::ModelRequestFailed,
        ],
        before: None,
        after: 0,
        ascending,
        limit,
    }
}

fn sequences(facts: &[TraceFactHeader]) -> Vec<u64> {
    facts.iter().map(|fact| fact.sequence).collect()
}

#[test]
fn all_scope_and_public_timeline_survive_every_auxiliary_request_phase() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, _) = primary_start(&store, 0);
    primary_terminal(&store, &first, false);
    let (second, _) = primary_start(&store, 1);
    primary_terminal(&store, &second, true);
    let baseline = TraceProjection::new(&store)
        .unwrap()
        .page(None, 32)
        .unwrap();
    let baseline_cut = store.observation_read_cut().unwrap();
    let baseline_count = store
        .observation_trace_total(baseline_cut.read.journal)
        .unwrap();
    let query = requests(TraceFactScope::All, true, 32);
    let baseline_facts = store
        .observation_trace_facts(&baseline_cut, &query)
        .unwrap();
    assert_eq!(baseline_facts.len(), 4);
    let assert_unchanged = || {
        let cut = store.observation_read_cut().unwrap();
        assert_eq!(
            store.observation_trace_facts(&cut, &query).unwrap(),
            baseline_facts
        );
        let page = TraceProjection::at_cut(&store, &cut)
            .unwrap()
            .page(None, 32)
            .unwrap();
        assert_eq!(page.records, baseline.records);
        assert_eq!(page.next_cursor, baseline.next_cursor);
        assert_eq!(
            store.observation_trace_total(cut.read.journal).unwrap(),
            baseline_count
        );
    };
    for (ordinal, completed) in [(0, false), (1, true)] {
        let call = auxiliary_call(&store, ordinal);
        assert_unchanged();
        let (request, start) = auxiliary_start(&store, &call);
        let membership: (bool, bool) = store.lock().unwrap().query_row(
            "SELECT observation_trace_fact_required,observation_trace_fact IS NOT NULL FROM events WHERE sequence=?1",
            [start.sequence], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(membership, (false, false));
        assert_unchanged();
        auxiliary_settle(&store, &request, completed);
        assert_unchanged();
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One mixed history verifies all pagination directions and owners.
fn mixed_request_scopes_apply_limits_and_cursors_to_primary_facts() {
    let store = store();
    store.initialize(&[]).unwrap();
    let mut expected = Vec::new();
    let mut starts = Vec::new();
    for ordinal in 0..35 {
        let call = auxiliary_call(&store, ordinal);
        let (auxiliary, _) = auxiliary_start(&store, &call);
        let (primary, start) = primary_start(&store, ordinal);
        expected.push(start.sequence);
        starts.push((primary.request_id.clone(), start.sequence));
        auxiliary_settle(&store, &auxiliary, ordinal % 2 == 0);
        expected.push(primary_terminal(&store, &primary, ordinal % 2 == 0).sequence);
    }
    let cut = store.observation_read_cut().unwrap();
    for scope in [TraceFactScope::All, TraceFactScope::Agent] {
        for ascending in [true, false] {
            for limit in [1, 32] {
                let mut query = requests(scope.clone(), ascending, limit);
                let mut observed = Vec::new();
                loop {
                    let page = store.observation_trace_facts(&cut, &query).unwrap();
                    if page.is_empty() {
                        break;
                    }
                    assert!(
                        page.iter()
                            .all(|fact| fact.owner == Some(RequestOwnerKind::Agent))
                    );
                    assert_eq!(page.len(), limit.min(expected.len() - observed.len()));
                    let last = page.last().unwrap().sequence;
                    observed.extend(sequences(&page));
                    assert!(
                        observed.len() <= expected.len(),
                        "the cursor repeated a page"
                    );
                    if ascending {
                        query.after = last;
                    } else {
                        query.before = Some(last);
                    }
                }
                let mut ordered = expected.clone();
                if !ascending {
                    ordered.reverse();
                }
                assert_eq!(observed, ordered);
            }
        }
    }
    for pair in starts.windows(2) {
        let predecessor = store
            .observation_request_predecessor_at(pair[1].1, cut.read.journal)
            .unwrap()
            .unwrap();
        assert_eq!(predecessor.start_sequence, pair[0].1);
        assert_eq!(predecessor.request_id.as_ref(), Some(&pair[0].0));
    }
    for pair in expected.as_chunks::<2>().0 {
        let facts = store
            .observation_trace_facts(
                &cut,
                &requests(
                    TraceFactScope::Owner {
                        anchor_sequence: pair[0],
                    },
                    true,
                    32,
                ),
            )
            .unwrap();
        assert_eq!(sequences(&facts), pair);
    }
    let mut timeline = TraceProjection::at_cut(&store, &cut)
        .unwrap()
        .page(None, 7)
        .unwrap();
    let mut records = timeline.records.len();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        for record in &timeline.records {
            assert!(
                seen.insert(record.id.clone()),
                "the timeline repeated a record"
            );
        }
        let Some(cursor) = timeline.next_cursor.as_ref() else {
            break;
        };
        timeline = TraceProjection::at_cut(&store, &cut)
            .unwrap()
            .page(Some(cursor), 7)
            .unwrap();
        assert!(!timeline.records.is_empty(), "continuation must advance");
        records += timeline.records.len();
        assert!(records <= 105, "the timeline exceeded its primary anchors");
    }
    assert_eq!(
        u64::try_from(records).unwrap(),
        store.observation_trace_total(cut.read.journal).unwrap()
    );
}

#[test]
fn captured_primary_trace_survives_later_auxiliary_and_primary_terminals() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (primary, primary_started) = primary_start(&store, 0);
    let call = auxiliary_call(&store, 0);
    let (request, _) = auxiliary_start(&store, &call);
    let cut = store.observation_read_cut().unwrap();
    let query = requests(TraceFactScope::All, true, 32);
    let expected = store.observation_trace_facts(&cut, &query).unwrap();
    assert_eq!(sequences(&expected), vec![primary_started.sequence]);
    let baseline = TraceProjection::at_cut(&store, &cut)
        .unwrap()
        .page(None, 32)
        .unwrap();
    auxiliary_settle(&store, &request, true);
    let terminal = primary_terminal(&store, &primary, true);
    let later = store.observation_read_cut().unwrap();
    assert_eq!(
        sequences(&store.observation_trace_facts(&later, &query).unwrap()),
        vec![primary_started.sequence, terminal.sequence]
    );
    assert_eq!(
        store.observation_trace_facts(&cut, &query).unwrap(),
        expected
    );
    assert_eq!(
        TraceProjection::at_cut(&store, &cut)
            .unwrap()
            .page(None, 32)
            .unwrap(),
        baseline
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET observation_trace_fact=NULL WHERE sequence=?1",
            [terminal.sequence],
        )
        .unwrap();
    assert!(store.observation_trace_facts(&later, &query).is_err());
    assert_eq!(
        store.observation_trace_facts(&cut, &query).unwrap(),
        expected
    );
    assert_eq!(
        TraceProjection::at_cut(&store, &cut)
            .unwrap()
            .page(None, 32)
            .unwrap(),
        baseline
    );
}

#[test]
fn missing_primary_header_remains_a_read_failure_after_optional_capture_fault() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (primary, started) = primary_start(&store, 0);
    let call = auxiliary_call(&store, 0);
    let (request, _) = auxiliary_start(&store, &call);
    auxiliary_settle(&store, &request, true);
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_primary_trace_capture
         BEFORE UPDATE OF observation_trace_fact ON events
         WHEN NEW.observation_kind='model_request_completed'
           AND NEW.observation_trace_fact_required=1
         BEGIN SELECT RAISE(ABORT,'primary Trace capture failure'); END;",
        )
        .unwrap();
    let terminal = primary_terminal(&store, &primary, true);
    let membership: (bool, bool) = store.lock().unwrap().query_row(
        "SELECT observation_trace_fact_required,observation_trace_fact IS NULL FROM events WHERE sequence=?1",
        [terminal.sequence], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(
        membership,
        (true, true),
        "the optional capture fault must be consumed"
    );
    let cut = store.observation_read_cut().unwrap();
    for scope in [
        TraceFactScope::All,
        TraceFactScope::Agent,
        TraceFactScope::Owner {
            anchor_sequence: started.sequence,
        },
    ] {
        assert!(
            store
                .observation_trace_facts(&cut, &requests(scope, false, 1))
                .is_err()
        );
    }
    assert!(
        TraceProjection::at_cut(&store, &cut)
            .unwrap()
            .page(None, 32)
            .is_err()
    );
    assert!(
        store
            .load_request_owner(&primary.request_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn corrupt_primary_owner_cannot_be_filtered_into_an_empty_successful_page() {
    for remove_owner in [false, true] {
        let store = store();
        store.initialize(&[]).unwrap();
        let (primary, started) = primary_start(&store, 0);
        let call = auxiliary_call(&store, 0);
        let (_, auxiliary_started) = auxiliary_start(&store, &call);
        let cut = store.observation_read_cut().unwrap();
        let changed = if remove_owner {
            store
                .lock()
                .unwrap()
                .execute(
                    "DELETE FROM request_owners WHERE request_id=?1",
                    [primary.request_id.as_str()],
                )
                .unwrap()
        } else {
            store.lock().unwrap().execute(
                "UPDATE request_owners SET owner_kind='auxiliary',attempt_id=NULL,turn_id=NULL,retry_number=NULL,call_id=?1,auxiliary_ordinal=99 WHERE request_id=?2",
                params![call.call_id.as_str(), primary.request_id.as_str()],
            ).unwrap()
        };
        assert_eq!(changed, 1);
        for scope in [TraceFactScope::All, TraceFactScope::Agent] {
            let mut query = requests(scope, true, 1);
            query.before = Some(auxiliary_started.sequence);
            assert!(store.observation_trace_facts(&cut, &query).is_err());
        }
        assert!(
            store
                .observation_request_predecessor_at(started.sequence, cut.read.journal)
                .is_err()
        );
    }
}

fn measured_sql<T>(store: &SqliteConversationStore, read: impl FnOnce() -> T) -> (T, usize) {
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = steps.clone();
    store
        .conn
        .lock()
        .unwrap()
        .progress_handler(
            1,
            Some(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
    let result = read();
    store
        .conn
        .lock()
        .unwrap()
        .progress_handler(0, None::<fn() -> bool>)
        .unwrap();
    (result, steps.load(std::sync::atomic::Ordering::Relaxed))
}

fn measured_request_query(
    store: &SqliteConversationStore,
    cut: &ObservationReadCut,
    query: &TraceFactQuery,
) -> (Vec<TraceFactHeader>, usize) {
    SqliteConversationStore::trace_read_work();
    let (facts, steps) = measured_sql(store, || store.observation_trace_facts(cut, query));
    let facts = facts.unwrap();
    let (rows, bytes) = SqliteConversationStore::trace_read_work();
    assert_eq!(rows, facts.len());
    assert!(bytes <= facts.len() * crate::durable::observation::HEADER_ENCODED_MAX);
    (facts, steps)
}

#[test]
fn primary_trace_query_work_stays_bounded_across_dense_auxiliary_intervals() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (_, primary) = primary_start(&store, 0);
    let mut baselines = Vec::new();
    for population in [2, 64, 512] {
        let from = match population {
            2 => 0,
            64 => 2,
            _ => 64,
        };
        for ordinal in from..population {
            let call = auxiliary_call(&store, ordinal);
            auxiliary_start(&store, &call);
        }
        let cut = store.observation_read_cut().unwrap();
        let mut observed_steps = Vec::new();
        for scope in [TraceFactScope::All, TraceFactScope::Agent] {
            for empty_interval in [false, true] {
                let mut query = requests(scope.clone(), false, 1);
                query.kinds = vec![TraceFactKind::ModelRequestStarted];
                if empty_interval {
                    query.after = primary.sequence;
                }
                let (facts, steps) = measured_request_query(&store, &cut, &query);
                assert_eq!(
                    sequences(&facts),
                    if empty_interval {
                        vec![]
                    } else {
                        vec![primary.sequence]
                    }
                );
                observed_steps.push(steps);
            }
        }
        if baselines.is_empty() {
            baselines = observed_steps;
        } else {
            for (baseline, actual) in baselines.iter().zip(&observed_steps) {
                assert!(
                    *actual <= baseline + 128,
                    "auxiliary population {population}: SQL VM work grew from {baseline} to {actual}"
                );
            }
        }
        if population == 512 {
            // Negative control: the same primary result through an index that
            // includes auxiliary requests must perform population-sized work.
            let (unscoped_result, unscoped_steps) = measured_sql(&store, || {
                store
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT sequence FROM events INDEXED BY events_trace_kind_idx
                     WHERE observation_kind='model_request_started' AND sequence<=?1
                       AND EXISTS(SELECT 1 FROM request_owners o WHERE o.owner_kind='agent'
                         AND o.start_sequence=events.sequence)
                     ORDER BY sequence DESC LIMIT 1",
                        [cut.read.journal],
                        |row| row.get::<_, u64>(0),
                    )
                    .unwrap()
            });
            assert_eq!(unscoped_result, primary.sequence);
            assert!(
                unscoped_steps > baselines[2] * 4,
                "the dense-interval negative control did not exercise enough SQL work"
            );
        }
    }
}
