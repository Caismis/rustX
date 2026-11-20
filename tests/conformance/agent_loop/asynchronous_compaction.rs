//! Detached Summary generation continues across real provider and Attempt boundaries.
use super::*;
use rustx::context::operation::{CompactionOperationPhase as Phase, CompactionOperationStatus};
use rustx::durable::AuxiliaryStore;
use rustx::events::types::RuntimeEvent as E;
use rustx::model::auxiliary::{
    AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryInputEvidence,
};
use rustx::model::structured::StructuredOutputStrategy;
use rustx::runtime::identity::{AttemptId, CompactionOperationId};

const NEXT_TURN: &str = "async: cancelled later user tail";
const FINAL_TURN: &str = "async: fresh final user tail";

/// Observe actual App Server notifications while retaining Driver's cursor fence.
pub(super) async fn next_status(
    driver: &Driver,
    phase: Phase,
    expected: Option<&CompactionOperationId>,
) -> CompactionOperationStatus {
    fn selected(
        context: &rustx::runtime_client::snapshot::RuntimeClientContextView,
        phase: Phase,
        expected: Option<&CompactionOperationId>,
    ) -> Option<CompactionOperationStatus> {
        if let Some(operation) = context.compaction_operations.iter().find(|item| {
            item.status.phase == phase && expected.is_none_or(|id| &item.status.operation_id == id)
        }) {
            return Some(operation.status.clone());
        }
        if phase == Phase::Ready
            && let Some(operation) = context
                .compaction_operations
                .iter()
                .rev()
                .find(|item| expected.is_none_or(|id| &item.status.operation_id == id))
            && operation.status.phase.is_terminal()
        {
            panic!("compaction terminated before Ready: {:?}", operation.status);
        }
        None
    }
    let snapshot = driver.snapshot().expect("current status snapshot").0;
    if let Some(status) = selected(&snapshot.context, phase, expected) {
        return status;
    }
    let mut receiver = driver.events.lock().await;
    loop {
        let record = match tokio::time::timeout(
            std::time::Duration::from_secs(120),
            receiver.recv(),
        )
        .await
        {
            Ok(Some(record)) => record,
            result => {
                let projected = driver
                    .snapshot()
                    .map(|(snapshot, cursor)| (snapshot.context.compaction_operations, cursor));
                let durable = driver.store.compaction_resource_cut().and_then(|cut| {
                    driver
                        .store
                        .latest_compaction_operations(cut.journal.0)
                        .map(|statuses| (cut.journal.0, statuses))
                });
                panic!(
                    "waiting for {phase:?}: notification={result:?}, projected={projected:?}, durable={durable:?}"
                );
            }
        };
        let NotificationMethod::Event { cursor, event, .. } = record else {
            unreachable!()
        };
        let previous = driver
            .consumed_cursor
            .swap(cursor.get(), std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            cursor.get(),
            previous + 1,
            "native notification cursor is contiguous"
        );
        for fact in event.facts() {
            let context = match fact {
                RuntimeClientEvent::ContextCompactionOperationUpdated { context } => context,
                RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                    &checkpoint.snapshot.context
                }
                _ => continue,
            };
            if let Some(status) = selected(context, phase, expected) {
                return status;
            }
        }
    }
}

fn assert_terminal_is_last_owned(
    journal: &[rustx::events::types::RuntimeEventEnvelope],
    id: &AttemptId,
) {
    let owned: Vec<_> = journal
        .iter()
        .filter(|entry| entry.attempt_id.as_ref() == Some(id))
        .collect();
    assert_ne!(owned, [] as [&rustx::events::RuntimeEventEnvelope; 0]);
    assert_eq!(
        owned
            .iter()
            .filter(|entry| matches!(
                entry.event,
                E::AttemptCompleted { .. }
                    | E::AttemptCancelled { .. }
                    | E::AttemptFailed { .. }
                    | E::AttemptTimedOut { .. }
                    | E::AttemptLimitExceeded { .. }
            ))
            .count(),
        1
    );
    assert!(matches!(
        owned.last().unwrap().event,
        E::AttemptCompleted { .. }
            | E::AttemptCancelled { .. }
            | E::AttemptFailed { .. }
            | E::AttemptTimedOut { .. }
            | E::AttemptLimitExceeded { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One real provider and durable lifecycle proof.
async fn detached_summary_outlives_two_attempts_and_installs_only_after_idle() {
    let Some(emulator) = ProviderEmulator::start("asynchronous_compaction").await else {
        return;
    };
    let setup = Setup {
        context_window: 256_000,
        auxiliary: Some(format!("emulator/{SUMMARY_MODEL}")),
        summary_scenario: SummaryScenario::Chunked,
        ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
    };
    let driver = Driver::start(&emulator, &setup).await;
    driver.submit(CHUNKED_PROMPT);
    let _ = tokio::join!(
        emulator.await_gate_with_timeout("async-summary", std::time::Duration::from_secs(300)),
        emulator.await_gate_with_timeout("async-primary", std::time::Duration::from_secs(300))
    );
    let first = driver.submitted_attempt().expect("first Attempt started").1;
    assert!(driver.store.projection_head().unwrap().is_none());
    emulator.release_gate("async-primary").await;
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert!(driver.store.projection_head().unwrap().is_none());
    driver
        .model_set(SessionModelConfig::of(
            ModelRef::parse(&format!("emulator/{SECOND_MODEL}")).unwrap(),
        ))
        .await
        .unwrap();

    driver.submit(NEXT_TURN);
    emulator.await_gate("async-next-primary").await;
    let second = driver
        .submitted_attempt()
        .expect("second Attempt started")
        .1;
    assert_ne!(first, second);
    let before = emulator.requests().await;
    let frozen = before
        .iter()
        .find(|request| request["model"] == SECOND_MODEL)
        .expect("second request")["body"]
        .clone();
    assert!(
        serde_json::to_string(&frozen)
            .unwrap()
            .contains("chunked-call-001")
    );
    emulator.release_gate("async-summary").await;
    let ready = next_status(&driver, Phase::Ready, None).await;
    assert!(
        driver.store.projection_head().unwrap().is_none(),
        "Ready has no installed checkpoint"
    );
    assert_eq!(
        emulator
            .requests()
            .await
            .iter()
            .find(|request| request["model"] == SECOND_MODEL)
            .unwrap()["body"],
        frozen
    );
    driver.cancel_current_attempt().await.unwrap();
    emulator.release_gate("async-next-primary").await;
    let (second_events, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Cancelled { .. }),
        "{outcome:?}"
    );
    let committed = second_events.iter().find_map(|event| match event {
        RuntimeClientEvent::ContextCompactionOperationUpdated { context } => context
            .compaction_operations
            .iter()
            .find(|item| {
                item.status.phase == Phase::Committed
                    && item.status.operation_id == ready.operation_id
            })
            .map(|item| item.status.clone()),
        _ => None,
    });
    let committed = match committed {
        Some(status) => status,
        None => next_status(&driver, Phase::Committed, Some(&ready.operation_id)).await,
    };
    assert_eq!(committed.operation_id, ready.operation_id);
    assert_eq!(committed.admission_generation, ready.admission_generation);
    let head = driver
        .store
        .projection_head()
        .unwrap()
        .expect("idle installation");
    let receipt = driver
        .store
        .projection_receipt(&committed.operation_id, &head.fingerprint)
        .unwrap()
        .expect("one receipt");
    assert_eq!(receipt.checkpoint, head);
    assert_eq!(
        driver
            .store
            .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 32)
            .unwrap()
            .len(),
        1
    );

    driver.submit(FINAL_TURN);
    let (_, outcome) = driver.settle().await;

    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let requests = emulator.requests().await;
    let summaries: Vec<_> = requests
        .iter()
        .filter(|request| request["model"] == SUMMARY_MODEL)
        .collect();
    assert!(summaries.len() > 13);
    assert_eq!(
        summaries[0]["body"]["response_format"]["type"],
        "json_schema"
    );
    for (index, request) in summaries.iter().enumerate() {
        assert_eq!(request["body"]["reasoning_effort"], "medium");
        if index != 0 {
            assert!(request["body"].get("response_format").is_none());
        }
        let body = body_text(request);
        assert!(!body.contains(PRIMARY_REASONING) && !body.contains(SUMMARY_REASONING));
    }
    let final_request = requests
        .iter()
        .rev()
        .find(|request| request["model"] == SECOND_MODEL)
        .unwrap();
    let final_body = body_text(final_request);
    assert_eq!(final_body.matches(NEXT_TURN).count(), 1);
    assert_eq!(final_body.matches(FINAL_TURN).count(), 1);
    assert!(final_body.contains(CHUNKED_SUMMARY));
    assert!(!final_body.contains("chunked-call-001"));
    assert!(!final_body.contains(SUMMARY_REASONING));
    assert!(
        final_body.contains(PRIMARY_REASONING),
        "the new model receives readable reasoning from retained canonical tail, not retired source"
    );

    let mut journal = Vec::new();
    let mut after = None;
    loop {
        let page = driver.store.read_events(after, 128).unwrap();
        if page.events.is_empty() {
            break;
        }
        after = page.next_sequence;
        journal.extend(page.events);
    }
    assert_terminal_is_last_owned(&journal, &first);
    assert_terminal_is_last_owned(&journal, &second);
    let started = journal
        .iter()
        .find(|entry| {
            matches!(&entry.event,
                E::CompactionStarted { operation_id, .. } if operation_id == &ready.operation_id
            )
        })
        .expect("native operation anchor");
    let detail = rpc(
        &driver.connection,
        Method::TraceDetail {
            target: driver.target.clone(),
            record_id: format!("trace:{}", started.sequence),
        },
    )
    .await
    .unwrap();
    let MethodResult::DomainPage { page } = detail else {
        panic!("finite native compaction inspection");
    };
    let inspection = page.items[0].value.as_ref().unwrap();
    assert_eq!(
        inspection["content"]["checkpoint"]["checkpoint_id"],
        serde_json::json!(head.candidate.checkpoint_id)
    );
    assert_eq!(
        inspection["content"]["checkpoint"]["activation_id"],
        head.activation.activation.0.to_string()
    );
    assert!(
        inspection["content"]["checkpoint"]["summary_preview"]["text"]
            .as_str()
            .unwrap()
            .contains(CHUNKED_SUMMARY)
    );
    let statuses: Vec<_> = journal
        .iter()
        .filter_map(|entry| match &entry.event {
            E::CompactionOperationUpdated { status }
                if status.operation_id == ready.operation_id =>
            {
                assert!(entry.attempt_id.is_none() && entry.turn_id.is_none());
                Some(status)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.phase.is_terminal())
            .count(),
        1
    );
    assert_eq!(statuses.last().unwrap().phase, Phase::Committed);
    assert!(
        statuses
            .iter()
            .all(|status| status.triggering_attempt_id.as_ref() == Some(&first))
    );
    assert_eq!(journal.iter().filter(|entry| matches!(&entry.event, E::CompactionCompleted { operation_id, .. } if operation_id == &ready.operation_id)).count(), 1);
    let starts: Vec<_> = journal
        .iter()
        .filter_map(|entry| match &entry.event {
            E::AuxiliaryCallStarted {
                call_id,
                business_source: AuxiliaryBusinessSource::Compaction { operation_id, .. },
                ..
            } if operation_id == &ready.operation_id => Some(call_id),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), summaries.len());
    let accepted: Vec<_> = journal
        .iter()
        .filter_map(|entry| match &entry.event {
            E::CompactionChunkAccepted {
                operation_id,
                accepted_fragments,
                source_ranges,
                generation,
            } if operation_id == &ready.operation_id => {
                Some((*accepted_fragments, source_ranges, generation))
            }
            _ => None,
        })
        .collect();
    assert_eq!(accepted.len() + 1, starts.len());
    let cut = driver.store.auxiliary_audit_cut().unwrap();
    let mut last: Option<(u32, u32)> = None;
    let mut bytes = 0_u64;
    let mut previous = None;
    let mut observed_usage = Vec::with_capacity(starts.len());
    for (index, call_id) in starts.iter().enumerate() {
        let call = driver
            .store
            .load_auxiliary_call(call_id, cut)
            .unwrap()
            .unwrap();
        let request = driver
            .store
            .read_auxiliary_call_requests(call_id, cut, None, 2)
            .unwrap();
        assert_eq!(request.len(), 1);
        assert_eq!(request[0].model, SUMMARY_MODEL);
        observed_usage.push(request[0].usage.clone());
        let AuxiliaryBusinessSource::Compaction { preceding_call, .. } = &call.business_source
        else {
            unreachable!()
        };
        assert_eq!(
            preceding_call.as_ref().map(|item| &item.call_id),
            index.checked_sub(1).map(|i| starts[i])
        );
        let AuxiliaryInputEvidence::Compaction {
            source_ranges,
            previous_summary,
            output_strategy,
            ..
        } = &call.input
        else {
            unreachable!()
        };
        assert_eq!(
            previous_summary.as_ref().map(|item| &item.call_id),
            previous
        );
        if index == 0 {
            assert_eq!(*output_strategy, StructuredOutputStrategy::Native);
            assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Failed));
            continue;
        }
        assert!(
            matches!(output_strategy, StructuredOutputStrategy::PromptFallback { operation_id } if operation_id == &ready.operation_id)
        );
        assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Completed));
        let (ordinal, ranges, generation) = accepted[index - 1];
        assert_eq!(ordinal as usize, index);
        assert_eq!(source_ranges, ranges);
        assert_eq!(&generation.as_ref().unwrap().call_id, *call_id);
        for range in ranges {
            if let Some((prior_index, prior_end)) = last {
                if prior_index == range.projection_index {
                    assert_eq!(range.byte_start, prior_end);
                } else {
                    assert_eq!(range.projection_index, prior_index + 1);
                    assert_eq!(range.byte_start, 0);
                }
            } else {
                assert_eq!((range.projection_index, range.byte_start), (0, 0));
            }
            bytes += u64::from(range.byte_end - range.byte_start);
            last = Some((range.projection_index, range.byte_end));
        }
        previous = Some(*call_id);
    }
    let coverage = head
        .candidate
        .prefix_coverage
        .as_ref()
        .expect("native authenticated prefix coverage");
    assert_eq!(bytes, coverage.rendered_bytes);
    assert_eq!(last.unwrap().0 + 1, coverage.captured_prefix_items);
    let admitted = journal
        .iter()
        .find(|entry| {
            matches!(&entry.event,
                E::CompactionStarted { operation_id, .. } if operation_id == &ready.operation_id
            )
        })
        .unwrap()
        .timestamp;
    let ready_at = journal
        .iter()
        .find(|entry| {
            matches!(&entry.event,
                E::CompactionOperationUpdated { status }
                    if status.operation_id == ready.operation_id && status.phase == Phase::Ready
            )
        })
        .unwrap()
        .timestamp;
    let installed = journal
        .iter()
        .find(|entry| {
            matches!(&entry.event,
                E::CompactionCompleted { operation_id, .. } if operation_id == &ready.operation_id
            )
        })
        .unwrap()
        .timestamp;
    let next_primary = journal
        .iter()
        .find(|entry| {
            entry.timestamp >= admitted
                && matches!(
                    &entry.event,
                    E::ModelRequestStarted {
                        owner: rustx::model::ModelRequestOwner::Agent { .. },
                        ..
                    }
                )
        })
        .unwrap()
        .timestamp;
    let usage_component = |select: fn(&rustx::model::types::ModelUsage) -> Option<u64>| {
        let known: u128 = observed_usage
            .iter()
            .filter_map(|usage| usage.as_ref().and_then(select))
            .map(u128::from)
            .sum();
        let missing = observed_usage
            .iter()
            .filter(|usage| usage.as_ref().and_then(select).is_none())
            .count();
        serde_json::json!({"known_tokens": (missing < observed_usage.len()).then(|| known.to_string()), "requests_without_measurement": missing})
    };
    let occupancy = driver.snapshot().unwrap().0.context.occupancy.map(|value| serde_json::json!({
        "input_tokens": value.input_tokens.to_string(),
        "window_tokens": value.context_window_tokens.to_string(),
        "remaining_window_tokens": value.context_window_tokens.checked_sub(value.input_tokens).map(|tokens| tokens.to_string()),
        "estimated": value.estimated,
    }));
    println!(
        "RUSTX_529_CONTROLLED_METRICS {}",
        serde_json::json!({
            "summary_admission_to_next_primary_start_ms": (next_primary - admitted).num_milliseconds(),
            "summary_admission_to_ready_ms": (ready_at - admitted).num_milliseconds(),
            "ready_to_installation_ms": (installed - ready_at).num_milliseconds(),
            "accepted_fragments": accepted.len(),
            "physical_requests": starts.len(),
            "blocking_operations": journal.iter().filter(|entry| matches!(&entry.event,
                E::CompactionStarted { mode: rustx::context::operation::CompactionMode::Blocking, .. }
            )).count(),
            "input_usage": usage_component(|usage| usage.input_tokens),
            "output_usage": usage_component(|usage| usage.output_tokens),
            "reasoning_usage": usage_component(|usage| usage.details.as_ref().and_then(|details| details.reasoning_tokens)),
            "rejected_native_request_usage": observed_usage.first().cloned().flatten(),
            "latest_primary_occupancy": occupancy,
        })
    );
    emulator.finish().await;
}
