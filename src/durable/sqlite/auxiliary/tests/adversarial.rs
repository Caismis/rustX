use super::*;
use crate::durable::store::canonical::CanonicalStore;
use crate::durable::store::journal::JournalStore;
use crate::durable::store::projection::ProjectionStore;
use crate::events::RuntimeEvent;
use crate::events::RuntimeEventEnvelope;
use crate::model::ModelFinishReason;
use crate::runtime::EventId;
use crate::runtime::RequestId;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, TimeZone, Utc};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;

use crate::durable::auxiliary::{AuxiliaryWriteFailure, auxiliary_settlement_envelope};
use crate::model::auxiliary::{
    AUXILIARY_AUDIT_PAGE_LIMIT_MAX, AuxiliaryAttribution, AuxiliaryAuditCut, AuxiliaryAuditFact,
    AuxiliaryAuditRecord, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition, AuxiliaryFingerprint,
    AuxiliaryFinishReason, AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal,
    AuxiliaryRequestEvidence, AuxiliaryRequestStart, AuxiliaryRequestTerminal,
    AuxiliaryUsageCompleteness,
};
use crate::model::deadline::ModelTimeoutPhase;
use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::{ModelUsage, UsageDetails};
use crate::runtime::identity::{AuxiliaryCallId, SystemUuidV7Generator};

fn test_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 19, 11, 0, 0).unwrap()
}

fn fresh_call() -> AuxiliaryCallStart {
    AuxiliaryCallStart {
        call_id: AuxiliaryCallId::generate(),
        purpose: AuxiliaryAttribution::new("input-suggestion").unwrap(),
        trigger: AuxiliaryAttribution::new("composer-idle").unwrap(),
        business_source: AuxiliaryBusinessSource::Business {
            namespace: AuxiliaryAttribution::new("composer").unwrap(),
            reference: AuxiliaryAttribution::new("draft-42").unwrap(),
        },
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
        invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
        selected_message_ids: Vec::new(),
    }
}

fn fresh_request(call: &AuxiliaryCallStart) -> AuxiliaryRequestStart {
    AuxiliaryRequestStart {
        call_id: call.call_id.clone(),
        ordinal: 0,
        request_id: RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap(),
        model: "adversarial-contract-model".into(),
    }
}

fn observed_usage() -> ModelUsage {
    ModelUsage {
        input_tokens: Some(17),
        output_tokens: Some(7),
        total_tokens: Some(24),
        details: Some(UsageDetails {
            reasoning_tokens: Some(3),
            cached_input_tokens: Some(5),
        }),
    }
}

fn completed_pair(
    store: &SqliteConversationStore,
    request: &AuxiliaryRequestStart,
) -> (RuntimeEventEnvelope, RuntimeEventEnvelope) {
    let request_terminal = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        test_time() + Duration::milliseconds(100),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary {
                call_id: request.call_id.clone(),
                ordinal: request.ordinal,
            },
            request_id: request.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(observed_usage()),
            generation: Some(GenerationEvidence {
                dispatch_after_start_ms: Some(2),
                first_output_ms: Some(5),
                last_output_ms: Some(8),
                terminal_ms: 13,
            }),
        },
    )
    .unwrap();
    let call_settled = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        test_time() + Duration::milliseconds(101),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: request.call_id.clone(),
            outcome: AuxiliaryCallOutcome::Completed,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id: request.request_id.clone(),
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::Stop,
                }),
                usage: Some(observed_usage()),
                usage_completeness: AuxiliaryUsageCompleteness::Complete,
                remote_execution_uncertainty: false,
            }),
        },
    )
    .unwrap();
    (request_terminal, call_settled)
}

fn cancelled_call(
    store: &SqliteConversationStore,
    call: &AuxiliaryCallStart,
) -> RuntimeEventEnvelope {
    auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        test_time() + Duration::milliseconds(100),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: call.call_id.clone(),
            outcome: AuxiliaryCallOutcome::Cancelled,
            request_evidence: None,
        },
    )
    .unwrap()
}

fn start_request(
    store: &SqliteConversationStore,
    call: &AuxiliaryCallStart,
    request: &AuxiliaryRequestStart,
) {
    store
        .commit_auxiliary_call_start(call, test_time())
        .unwrap();
    store
        .commit_auxiliary_request_start(request, test_time())
        .unwrap();
}

// Capture every main-database table, including native observation layouts,
// allocators, implicit row identities and SQLite's own schema/sequence tables.
// A Journal frontier alone cannot expose an orphaned owner row, rewritten
// receipt or partially captured view. Typed SQLite values retain
// NULL/text/blob/integer distinctions; sorting rows makes this independent of
// query plans while preserving duplicate-row multiplicity.
fn durable_state(store: &SqliteConversationStore) -> BTreeMap<String, Vec<Vec<String>>> {
    let connection = store.lock().unwrap();
    let table_names = connection
        .prepare(
            "SELECT name,wr FROM pragma_table_list
             WHERE schema='main' AND type!='view' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    table_names
        .into_iter()
        .map(|(name, without_rowid)| {
            let quoted = name.replace('"', "\"\"");
            let columns = if without_rowid { "*" } else { "rowid,*" };
            let mut statement = connection
                .prepare(&format!("SELECT {columns} FROM \"{quoted}\""))
                .unwrap();
            let column_count = statement.column_count();
            let mut rows = statement
                .query_map([], |row| {
                    (0..column_count)
                        .map(|column| {
                            row.get::<_, Value>(column)
                                .map(|value| format!("{value:?}"))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows.sort();
            (name, rows)
        })
        .collect()
}

fn known_rejection<T: core::fmt::Debug>(result: &Result<T, AuxiliaryWriteFailure>) {
    assert!(
        matches!(result, Err(AuxiliaryWriteFailure::KnownNotCommitted { .. })),
        "expected a proven noncommit, got {result:?}"
    );
}

fn arm_late_failure(store: &SqliteConversationStore, trigger_clause: &str) -> Arc<AtomicUsize> {
    let seen = Arc::new(AtomicUsize::new(0));
    let observed = seen.clone();
    let connection = store.lock().unwrap();
    connection
        .create_scalar_function(
            "auxiliary_adversarial_fault_seen",
            0,
            FunctionFlags::SQLITE_UTF8,
            move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(1_i64)
            },
        )
        .unwrap();
    // RAISE(FAIL) retains earlier statement writes until the semantic writer
    // explicitly rolls back. The out-of-database probe survives that rollback
    // and proves this exact fault was consumed.
    connection
        .execute_batch(&format!(
            "CREATE TEMP TRIGGER auxiliary_adversarial_late_failure
             {trigger_clause}
             BEGIN
                 SELECT auxiliary_adversarial_fault_seen();
                 SELECT RAISE(FAIL,'consumed auxiliary late-write fault');
             END;"
        ))
        .unwrap();
    seen
}

#[derive(Clone, Copy, Debug)]
enum SemanticWrite {
    CallStart,
    RequestStart,
    FinalRequest,
    ZeroRequestFinal,
}

#[test]
#[allow(clippy::too_many_lines)]
fn consumed_late_write_faults_roll_back_every_durable_surface_and_allow_exact_retry() {
    let cases = [
        (
            SemanticWrite::CallStart,
            "AFTER INSERT ON auxiliary_audit_facts WHEN NEW.kind='call_started'",
        ),
        (
            SemanticWrite::RequestStart,
            "AFTER INSERT ON request_owners",
        ),
        (
            SemanticWrite::RequestStart,
            "AFTER INSERT ON auxiliary_requests",
        ),
        (
            SemanticWrite::RequestStart,
            "AFTER INSERT ON auxiliary_audit_facts WHEN NEW.kind='request_started'",
        ),
        (
            SemanticWrite::FinalRequest,
            "AFTER UPDATE OF terminal_sequence ON request_owners",
        ),
        (
            SemanticWrite::FinalRequest,
            "AFTER INSERT ON auxiliary_audit_facts WHEN NEW.kind='request_settled'",
        ),
        (
            SemanticWrite::FinalRequest,
            "AFTER UPDATE OF settled_sequence ON auxiliary_calls",
        ),
        (
            SemanticWrite::FinalRequest,
            "AFTER INSERT ON auxiliary_audit_facts WHEN NEW.kind='call_settled'",
        ),
        (
            SemanticWrite::ZeroRequestFinal,
            "AFTER INSERT ON auxiliary_audit_facts WHEN NEW.kind='call_settled'",
        ),
    ];
    for (write, trigger_clause) in cases {
        let store = store();
        store
            .initialize(&[user_message("kept", "preserve canonical state")])
            .unwrap();
        let call = fresh_call();
        let request = fresh_request(&call);
        if !matches!(write, SemanticWrite::CallStart) {
            store
                .commit_auxiliary_call_start(&call, test_time())
                .unwrap();
        }
        if matches!(write, SemanticWrite::FinalRequest) {
            store
                .commit_auxiliary_request_start(&request, test_time())
                .unwrap();
        }
        let pair = completed_pair(&store, &request);
        let zero_terminal = cancelled_call(&store, &call);
        let before = durable_state(&store);
        let cut = store.auxiliary_audit_cut().unwrap();
        let consumed = arm_late_failure(&store, trigger_clause);
        match write {
            SemanticWrite::CallStart => {
                known_rejection(&store.commit_auxiliary_call_start(&call, test_time()));
            }
            SemanticWrite::RequestStart => {
                known_rejection(&store.commit_auxiliary_request_start(&request, test_time()));
            }
            SemanticWrite::FinalRequest => known_rejection(
                &store.commit_auxiliary_request_settlement(pair.0.clone(), pair.1.clone()),
            ),
            SemanticWrite::ZeroRequestFinal => {
                known_rejection(&store.commit_auxiliary_call_settlement(zero_terminal.clone()));
            }
        }
        assert_eq!(
            consumed.load(Ordering::SeqCst),
            1,
            "{write:?}: {trigger_clause}"
        );
        assert_eq!(durable_state(&store), before, "{write:?}: {trigger_clause}");
        assert_eq!(store.auxiliary_audit_cut().unwrap(), cut);

        // A fresh read has no evidence of a never-recorded rollback receipt.
        // Only the original write result above establishes KnownNotCommitted.
        match write {
            SemanticWrite::CallStart => assert_eq!(
                store.confirm_auxiliary_call_start(&call).unwrap(),
                AuxiliaryCommitConfirmation::Unknown
            ),
            SemanticWrite::RequestStart => assert_eq!(
                store.confirm_auxiliary_request_start(&request).unwrap(),
                AuxiliaryCommitConfirmation::Unknown
            ),
            SemanticWrite::FinalRequest => assert_eq!(
                store
                    .confirm_auxiliary_settlement(&pair.0, &pair.1)
                    .unwrap(),
                AuxiliaryCommitConfirmation::Unknown
            ),
            SemanticWrite::ZeroRequestFinal => assert_eq!(
                store
                    .confirm_auxiliary_call_settlement(&zero_terminal)
                    .unwrap(),
                AuxiliaryCommitConfirmation::Unknown
            ),
        }
        assert_eq!(
            durable_state(&store),
            before,
            "confirmation changed durable state"
        );
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER auxiliary_adversarial_late_failure")
            .unwrap();

        let (disposition, new_sequence) = match write {
            SemanticWrite::CallStart => {
                let receipt = store
                    .commit_auxiliary_call_start(&call, test_time())
                    .unwrap();
                (receipt.disposition, receipt.event.sequence)
            }
            SemanticWrite::RequestStart => {
                let receipt = store
                    .commit_auxiliary_request_start(&request, test_time())
                    .unwrap();
                (receipt.disposition, receipt.event.sequence)
            }
            SemanticWrite::FinalRequest => {
                let receipt = store
                    .commit_auxiliary_request_settlement(pair.0, pair.1)
                    .unwrap();
                assert_eq!(
                    receipt.request_terminal.as_ref().unwrap().sequence,
                    cut.journal + 1
                );
                (receipt.disposition, receipt.call_settled.sequence)
            }
            SemanticWrite::ZeroRequestFinal => {
                let receipt = store
                    .commit_auxiliary_call_settlement(zero_terminal)
                    .unwrap();
                (receipt.disposition, receipt.call_settled.sequence)
            }
        };
        assert_eq!(disposition, AuxiliaryCommitDisposition::NewlyCommitted);
        let expected_growth = if matches!(write, SemanticWrite::FinalRequest) {
            2
        } else {
            1
        };
        assert_eq!(new_sequence, cut.journal + expected_growth);
        assert_eq!(
            consumed.load(Ordering::SeqCst),
            1,
            "the fault remained disarmed"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn call_start_replay_and_confirmation_bind_every_frozen_semantic_coordinate() {
    let store = store();
    store
        .initialize(&[
            user_message("first", "first canonical input"),
            user_message("second", "second canonical input"),
            user_message("third", "third canonical input"),
        ])
        .unwrap();
    let mut original = fresh_call();
    original.input = AuxiliaryInputEvidence::Historical {
        cut: store.compaction_resource_cut().unwrap(),
    };
    original.selected_message_ids = vec![MessageId::new("first"), MessageId::new("second")];
    let committed = store
        .commit_auxiliary_call_start(&original, test_time())
        .unwrap();
    let mut variants = Vec::new();
    let mut changed = original.clone();
    changed.purpose = AuxiliaryAttribution::new("another-consumer").unwrap();
    variants.push(("purpose", changed));
    let mut changed = original.clone();
    changed.trigger = AuxiliaryAttribution::new("explicit-request").unwrap();
    variants.push(("trigger", changed));
    let mut changed = original.clone();
    if let AuxiliaryBusinessSource::Business { namespace, .. } = &mut changed.business_source {
        *namespace = AuxiliaryAttribution::new("another-domain").unwrap();
    }
    variants.push(("business namespace", changed));
    let mut changed = original.clone();
    if let AuxiliaryBusinessSource::Business { reference, .. } = &mut changed.business_source {
        *reference = AuxiliaryAttribution::new("draft-43").unwrap();
    }
    variants.push(("business reference", changed));
    let mut changed = original.clone();
    changed.business_source = AuxiliaryBusinessSource::Independent;
    variants.push(("business source kind", changed));
    let mut changed = original.clone();
    changed.input = AuxiliaryInputEvidence::Historical {
        cut: store.compaction_resource_cut().unwrap(),
    };
    assert_ne!(
        changed.input, original.input,
        "the positive control advances the native cut"
    );
    variants.push(("complete historical cut", changed));
    let mut changed = original.clone();
    changed.semantic_input_fingerprint = AuxiliaryFingerprint::new("c".repeat(64)).unwrap();
    variants.push(("semantic fingerprint", changed));
    let mut changed = original.clone();
    changed.invocation_fingerprint = AuxiliaryFingerprint::new("d".repeat(64)).unwrap();
    variants.push(("invocation fingerprint", changed));
    let mut changed = original.clone();
    changed.selected_message_ids.reverse();
    variants.push(("selected input order", changed));
    let mut changed = original.clone();
    changed.selected_message_ids.pop();
    variants.push(("selected input omission", changed));
    let mut changed = original.clone();
    changed.selected_message_ids.push(MessageId::new("third"));
    variants.push(("selected input addition", changed));

    for (name, changed) in variants {
        // The changed payload is valid for a new Call. Consequently, rejecting
        // it under the original ID proves identity binding, not malformed input.
        let mut independent = changed.clone();
        independent.call_id = AuxiliaryCallId::generate();
        assert_eq!(
            store
                .commit_auxiliary_call_start(&independent, test_time())
                .unwrap()
                .disposition,
            AuxiliaryCommitDisposition::NewlyCommitted,
            "positive control: {name}"
        );
        let before = durable_state(&store);
        known_rejection(&store.commit_auxiliary_call_start(&changed, test_time()));
        assert!(
            store.confirm_auxiliary_call_start(&changed).is_err(),
            "{name}"
        );
        assert_eq!(
            durable_state(&store),
            before,
            "conflicting {name} changed the store"
        );
    }

    let before = durable_state(&store);
    let replay = store
        .commit_auxiliary_call_start(&original, test_time() + Duration::days(1))
        .unwrap();
    assert_eq!(
        replay.disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(
        replay.event, committed.event,
        "the first durable timestamp remains authoritative"
    );
    assert_eq!(replay.call, committed.call);
    let AuxiliaryCommitConfirmation::ConfirmedCommitted(confirmed) =
        store.confirm_auxiliary_call_start(&original).unwrap()
    else {
        panic!("the original frozen Call must remain confirmable");
    };
    assert_eq!(confirmed, replay);
    assert_eq!(durable_state(&store), before);
}

#[test]
fn request_start_replay_preserves_start_receipt_after_settlement_and_rejects_owner_changes() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call = fresh_call();
    let request = fresh_request(&call);
    store
        .commit_auxiliary_call_start(&call, test_time())
        .unwrap();
    let committed = store
        .commit_auxiliary_request_start(&request, test_time())
        .unwrap();
    let (terminal, settled) = completed_pair(&store, &request);
    store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
    let other_call = fresh_call();
    store
        .commit_auxiliary_call_start(&other_call, test_time())
        .unwrap();
    let mut variants = Vec::new();
    let mut changed = request.clone();
    changed.model = "another-model".into();
    variants.push(("model", changed));
    let mut changed = request.clone();
    changed.call_id = other_call.call_id;
    variants.push(("Call owner", changed));
    let mut changed = request.clone();
    changed.ordinal = 1;
    variants.push(("physical ordinal", changed));
    let before = durable_state(&store);
    for (name, changed) in variants {
        known_rejection(&store.commit_auxiliary_request_start(&changed, test_time()));
        assert!(
            store.confirm_auxiliary_request_start(&changed).is_err(),
            "{name}"
        );
        assert_eq!(
            durable_state(&store),
            before,
            "conflicting {name} changed durable state"
        );
    }
    let replay = store
        .commit_auxiliary_request_start(&request, test_time() + Duration::days(1))
        .unwrap();
    assert_eq!(
        replay.disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(replay.event, committed.event);
    assert_eq!(
        replay.request, committed.request,
        "start receipts cannot acquire future terminal fields"
    );
    assert_eq!(
        store.confirm_auxiliary_request_start(&request).unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(replay)
    );
    assert_eq!(durable_state(&store), before);
}

fn call_evidence(envelope: &mut RuntimeEventEnvelope) -> &mut AuxiliaryRequestEvidence {
    match &mut envelope.event {
        RuntimeEvent::AuxiliaryCallSettled {
            request_evidence: Some(evidence),
            ..
        } => evidence,
        other => panic!("expected a settled Call with request evidence, got {other:?}"),
    }
}

fn replace_usage(
    terminal: &mut RuntimeEventEnvelope,
    settled: &mut RuntimeEventEnvelope,
    replacement: Option<ModelUsage>,
) {
    match &mut terminal.event {
        RuntimeEvent::ModelRequestCompleted { usage, .. }
        | RuntimeEvent::ModelRequestFailed { usage, .. } => usage.clone_from(&replacement),
        other => panic!("expected a physical terminal, got {other:?}"),
    }
    call_evidence(settled).usage = replacement;
}

fn positive_settlement_control(
    store: &SqliteConversationStore,
    expected_terminal: &RuntimeEventEnvelope,
    expected_settled: &RuntimeEventEnvelope,
) {
    let call = fresh_call();
    let request = fresh_request(&call);
    start_request(store, &call, &request);
    let mut terminal = expected_terminal.clone();
    terminal.event_id = match &mut terminal.event {
        RuntimeEvent::ModelRequestCompleted {
            owner, request_id, ..
        } => {
            *owner = ModelRequestOwner::Auxiliary {
                call_id: call.call_id.clone(),
                ordinal: 0,
            };
            request_id.clone_from(&request.request_id);
            EventId::new(format!("auxiliary-request-completed:{request_id}"))
        }
        RuntimeEvent::ModelRequestFailed {
            owner, request_id, ..
        } => {
            *owner = ModelRequestOwner::Auxiliary {
                call_id: call.call_id.clone(),
                ordinal: 0,
            };
            request_id.clone_from(&request.request_id);
            EventId::new(format!("auxiliary-request-failed:{request_id}"))
        }
        other => panic!("expected a request terminal, got {other:?}"),
    };
    let mut settled = expected_settled.clone();
    settled.event_id = EventId::new(format!("auxiliary-call-settled:{}", call.call_id));
    if let RuntimeEvent::AuxiliaryCallSettled {
        call_id,
        request_evidence,
        ..
    } = &mut settled.event
    {
        call_id.clone_from(&call.call_id);
        request_evidence.as_mut().unwrap().request_id = request.request_id;
    }
    assert_eq!(
        store
            .commit_auxiliary_request_settlement(terminal, settled)
            .unwrap()
            .disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn exact_completed_receipts_reject_self_consistent_payload_changes_with_successful_controls() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call = fresh_call();
    let request = fresh_request(&call);
    start_request(&store, &call, &request);
    let (terminal, settled) = completed_pair(&store, &request);
    let committed = store
        .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
        .unwrap();
    let mut variants = Vec::new();
    let mut changed = terminal.clone();
    changed.timestamp += Duration::milliseconds(1);
    variants.push(("request timestamp", changed, settled.clone()));
    let mut changed = settled.clone();
    changed.timestamp += Duration::milliseconds(1);
    variants.push(("Call timestamp", terminal.clone(), changed));
    for name in [
        "input tokens",
        "output tokens",
        "total tokens",
        "reasoning tokens",
        "cached tokens",
    ] {
        let mut changed_usage = observed_usage();
        match name {
            "input tokens" => {
                changed_usage.input_tokens = changed_usage.input_tokens.map(|value| value + 1);
            }
            "output tokens" => {
                changed_usage.output_tokens = changed_usage.output_tokens.map(|value| value + 1);
            }
            "total tokens" => {
                changed_usage.total_tokens = changed_usage.total_tokens.map(|value| value + 1);
            }
            "reasoning tokens" => {
                changed_usage.details.as_mut().unwrap().reasoning_tokens = Some(4);
            }
            "cached tokens" => {
                changed_usage.details.as_mut().unwrap().cached_input_tokens = Some(6);
            }
            _ => unreachable!(),
        }
        let mut changed_terminal = terminal.clone();
        let mut changed_settled = settled.clone();
        replace_usage(
            &mut changed_terminal,
            &mut changed_settled,
            Some(changed_usage),
        );
        variants.push((name, changed_terminal, changed_settled));
    }
    let mut changed_terminal = terminal.clone();
    let mut changed_settled = settled.clone();
    replace_usage(&mut changed_terminal, &mut changed_settled, None);
    call_evidence(&mut changed_settled).usage_completeness = AuxiliaryUsageCompleteness::Unreported;
    variants.push(("unreported usage", changed_terminal, changed_settled));
    let mut changed = settled.clone();
    call_evidence(&mut changed).usage_completeness = AuxiliaryUsageCompleteness::Partial;
    variants.push(("usage completeness", terminal.clone(), changed));
    let mut changed = settled.clone();
    call_evidence(&mut changed).remote_execution_uncertainty = true;
    variants.push(("remote uncertainty", terminal.clone(), changed));
    for name in [
        "dispatch bridge",
        "first output",
        "last output",
        "terminal offset",
    ] {
        let mut changed = terminal.clone();
        if let RuntimeEvent::ModelRequestCompleted {
            generation: Some(generation),
            ..
        } = &mut changed.event
        {
            match name {
                "dispatch bridge" => generation.dispatch_after_start_ms = Some(3),
                "first output" => generation.first_output_ms = Some(6),
                "last output" => generation.last_output_ms = Some(9),
                "terminal offset" => generation.terminal_ms = 14,
                _ => unreachable!(),
            }
        }
        variants.push((name, changed, settled.clone()));
    }
    let mut changed_terminal = terminal.clone();
    let mut changed_settled = settled.clone();
    if let RuntimeEvent::ModelRequestCompleted { finish_reason, .. } = &mut changed_terminal.event {
        *finish_reason = ModelFinishReason::Length;
    }
    call_evidence(&mut changed_settled).observed_provider_terminal =
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: AuxiliaryFinishReason::Length,
        });
    variants.push((
        "matching completion kinds",
        changed_terminal,
        changed_settled,
    ));

    for (name, changed_terminal, changed_settled) in variants {
        positive_settlement_control(&store, &changed_terminal, &changed_settled);
        let before = durable_state(&store);
        known_rejection(&store.commit_auxiliary_request_settlement(
            changed_terminal.clone(),
            changed_settled.clone(),
        ));
        assert!(
            store
                .confirm_auxiliary_settlement(&changed_terminal, &changed_settled)
                .is_err(),
            "self-consistent conflicting {name} was confirmed"
        );
        assert_eq!(
            durable_state(&store),
            before,
            "conflicting {name} changed durable state"
        );
    }

    let before = durable_state(&store);
    let replay = store
        .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
        .unwrap();
    assert_eq!(
        replay.disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(replay.request, committed.request);
    assert_eq!(replay.call, committed.call);
    assert_eq!(replay.request_terminal, committed.request_terminal);
    assert_eq!(replay.call_settled, committed.call_settled);
    assert_eq!(
        store
            .confirm_auxiliary_settlement(&terminal, &settled)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(replay)
    );
    assert_eq!(durable_state(&store), before);
}

#[test]
#[allow(clippy::too_many_lines)]
fn failed_receipts_bind_local_failure_provider_evidence_and_call_outcome_independently() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call = fresh_call();
    let request = fresh_request(&call);
    start_request(&store, &call, &request);
    let (mut terminal, mut settled) = completed_pair(&store, &request);
    terminal = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        terminal.timestamp,
        RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Auxiliary {
                call_id: call.call_id.clone(),
                ordinal: 0,
            },
            request_id: request.request_id.clone(),
            error: ModelError::timeout(ModelTimeoutPhase::ResponseStart),
            usage: Some(observed_usage()),
            generation: None,
        },
    )
    .unwrap();
    if let RuntimeEvent::AuxiliaryCallSettled { outcome, .. } = &mut settled.event {
        *outcome = AuxiliaryCallOutcome::Failed;
    }
    call_evidence(&mut settled).observed_provider_terminal = None;
    call_evidence(&mut settled).usage_completeness = AuxiliaryUsageCompleteness::Partial;
    call_evidence(&mut settled).remote_execution_uncertainty = true;
    store
        .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
        .unwrap();
    let mut variants = Vec::new();
    for name in [
        "failure kind",
        "diagnostic",
        "retry disposition",
        "retry delay",
    ] {
        let mut changed = terminal.clone();
        if let RuntimeEvent::ModelRequestFailed { error, .. } = &mut changed.event {
            match name {
                "failure kind" => {
                    error.kind = ModelErrorKind::Transport;
                    error.timeout_phase = None;
                }
                "diagnostic" => error.message.push_str("; another diagnostic"),
                "retry disposition" => error.retry_disposition = ModelRetryDisposition::Never,
                "retry delay" => error.retry_after_ms = Some(123),
                _ => unreachable!(),
            }
        }
        variants.push((name, changed, settled.clone()));
    }
    let mut changed = settled.clone();
    if let RuntimeEvent::AuxiliaryCallSettled { outcome, .. } = &mut changed.event {
        *outcome = AuxiliaryCallOutcome::Cancelled;
    }
    variants.push(("Call cancellation", terminal.clone(), changed));
    let mut changed = settled.clone();
    call_evidence(&mut changed).remote_execution_uncertainty = false;
    variants.push(("remote certainty", terminal.clone(), changed));
    let mut changed = settled.clone();
    call_evidence(&mut changed).observed_provider_terminal =
        Some(AuxiliaryObservedProviderTerminal::Failed {
            kind: ModelErrorKind::RateLimit,
        });
    variants.push(("observed provider failure", terminal.clone(), changed));
    let mut changed = settled.clone();
    call_evidence(&mut changed).observed_provider_terminal =
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: AuxiliaryFinishReason::Stop,
        });
    variants.push((
        "observed completion rejected locally",
        terminal.clone(),
        changed,
    ));
    for (name, changed_terminal, changed_settled) in variants {
        positive_settlement_control(&store, &changed_terminal, &changed_settled);
        let before = durable_state(&store);
        known_rejection(&store.commit_auxiliary_request_settlement(
            changed_terminal.clone(),
            changed_settled.clone(),
        ));
        assert!(
            store
                .confirm_auxiliary_settlement(&changed_terminal, &changed_settled)
                .is_err(),
            "{name}"
        );
        assert_eq!(
            durable_state(&store),
            before,
            "conflicting {name} changed durable state"
        );
    }
}

fn read_all_pages(
    store: &SqliteConversationStore,
    cut: AuxiliaryAuditCut,
    limit: usize,
    expected_total: usize,
) -> Vec<AuxiliaryAuditRecord> {
    let mut after = None;
    let mut records: Vec<AuxiliaryAuditRecord> = Vec::new();
    loop {
        let page = store.read_auxiliary_audit(cut, after, limit).unwrap();
        assert_eq!(page.cut, cut);
        assert_eq!(page.total, u64::try_from(expected_total).unwrap());
        let remaining = expected_total - records.len();
        assert_eq!(page.records.len(), remaining.min(limit));
        let expected_next = if remaining > limit {
            Some(u64::try_from(records.len() + page.records.len()).unwrap())
        } else {
            None
        };
        assert_eq!(page.next_offset, expected_next);
        for record in &page.records {
            assert_eq!(
                record.audit_offset,
                u64::try_from(records.len() + 1).unwrap()
            );
            assert!(record.journal_sequence <= cut.journal);
            if let Some(previous) = records.last() {
                assert!(previous.journal_sequence < record.journal_sequence);
            }
            records.push(record.clone());
        }
        match page.next_offset {
            Some(next) => {
                assert!(next > after.unwrap_or(0), "a nonterminal page must advance");
                after = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(records.len(), expected_total);
    let exhausted = store
        .read_auxiliary_audit(cut, Some(u64::try_from(expected_total).unwrap()), limit)
        .unwrap();
    assert!(exhausted.records.is_empty());
    assert_eq!(exhausted.total, u64::try_from(expected_total).unwrap());
    assert_eq!(exhausted.next_offset, None);
    records
}

#[test]
#[allow(clippy::too_many_lines)]
fn multipage_historical_audit_stays_exact_when_interleaved_calls_settle_in_reverse_order() {
    const CALLS: usize = 20;
    let store = store();
    store
        .initialize(&[user_message("base", "frozen primary input")])
        .unwrap();
    let mut requests = Vec::new();
    let mut expected_open = Vec::new();
    for index in 0..CALLS {
        let call = fresh_call();
        let request = fresh_request(&call);
        let call_receipt = store
            .commit_auxiliary_call_start(&call, test_time())
            .unwrap();
        expected_open.push(AuxiliaryAuditRecord {
            audit_offset: u64::try_from(expected_open.len() + 1).unwrap(),
            journal_sequence: call_receipt.event.sequence,
            fact: AuxiliaryAuditFact::CallStarted(call_receipt.call),
        });
        let attempt = AttemptId::new(format!("interleaved-attempt-{index}"));
        store
            .append_event(envelope(
                store.conversation_id(),
                &format!("interleaved-start-{index}"),
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt,
                },
            ))
            .unwrap();
        let request_receipt = store
            .commit_auxiliary_request_start(&request, test_time())
            .unwrap();
        expected_open.push(AuxiliaryAuditRecord {
            audit_offset: u64::try_from(expected_open.len() + 1).unwrap(),
            journal_sequence: request_receipt.event.sequence,
            fact: AuxiliaryAuditFact::RequestStarted(request_receipt.request),
        });
        requests.push(request);
    }
    let cut = store.auxiliary_audit_cut().unwrap();
    assert_eq!(
        store.auxiliary_audit_total(cut).unwrap(),
        u64::try_from(CALLS * 2).unwrap()
    );
    assert!(cut.journal > u64::try_from(CALLS * 2).unwrap());
    for record in &expected_open {
        match &record.fact {
            AuxiliaryAuditFact::CallStarted(call) => {
                assert_eq!(call.purpose.as_str(), "input-suggestion");
                assert_eq!(call.settled_sequence, None);
                assert_eq!(call.outcome, None);
            }
            AuxiliaryAuditFact::RequestStarted(request) => {
                assert_eq!(request.ordinal, 0);
                assert_eq!(request.terminal_sequence, None);
                assert_eq!(request.local_terminal, None);
                assert_eq!(request.generation, None);
                assert_eq!(request.usage, None);
                assert_eq!(request.request_evidence, None);
            }
            other => panic!("only starts exist at the captured cut, got {other:?}"),
        }
    }
    for limit in [1, 2, 3, 7, 31, AUXILIARY_AUDIT_PAGE_LIMIT_MAX] {
        assert_eq!(read_all_pages(&store, cut, limit, CALLS * 2), expected_open);
    }

    let mut expected_settled = expected_open.clone();
    for request in requests.iter().rev() {
        let (terminal, settled) = completed_pair(&store, request);
        let receipt = store
            .commit_auxiliary_request_settlement(terminal, settled)
            .unwrap();
        let terminal_sequence = receipt.request_terminal.as_ref().unwrap().sequence;
        let intermediate_cut = AuxiliaryAuditCut {
            journal: terminal_sequence,
        };
        let intermediate = store
            .load_auxiliary_request(&request.request_id, intermediate_cut)
            .unwrap()
            .unwrap();
        assert_eq!(
            intermediate.local_terminal,
            Some(AuxiliaryRequestTerminal::Completed)
        );
        assert_eq!(intermediate.usage, Some(observed_usage()));
        assert_eq!(
            intermediate.request_evidence, None,
            "Call evidence follows its own Journal coordinate"
        );
        assert_eq!(
            store
                .load_auxiliary_call(&request.call_id, intermediate_cut)
                .unwrap()
                .unwrap()
                .outcome,
            None
        );
        let request_audit = receipt.request.unwrap();
        assert!(request_audit.request_evidence.is_some());
        for record in &mut expected_settled {
            match &mut record.fact {
                AuxiliaryAuditFact::CallStarted(call) if call.call_id == request.call_id => {
                    *call = receipt.call.clone();
                }
                AuxiliaryAuditFact::RequestStarted(audit)
                    if audit.request_id == request.request_id =>
                {
                    *audit = request_audit.clone();
                }
                _ => {}
            }
        }
        expected_settled.push(AuxiliaryAuditRecord {
            audit_offset: u64::try_from(expected_settled.len() + 1).unwrap(),
            journal_sequence: terminal_sequence,
            fact: AuxiliaryAuditFact::RequestSettled(request_audit),
        });
        expected_settled.push(AuxiliaryAuditRecord {
            audit_offset: u64::try_from(expected_settled.len() + 1).unwrap(),
            journal_sequence: receipt.call_settled.sequence,
            fact: AuxiliaryAuditFact::CallSettled(receipt.call),
        });
    }
    let settled_cut = store.auxiliary_audit_cut().unwrap();
    store
        .append_canonical(&user_message("later", "canonical-only advancement"))
        .unwrap();
    assert_eq!(store.auxiliary_audit_cut().unwrap(), settled_cut);
    let later = fresh_call();
    store
        .commit_auxiliary_call_start(&later, test_time())
        .unwrap();
    let before_reads = durable_state(&store);
    for limit in [1, 2, 3, 7, 31, AUXILIARY_AUDIT_PAGE_LIMIT_MAX] {
        assert_eq!(read_all_pages(&store, cut, limit, CALLS * 2), expected_open);
        assert_eq!(
            read_all_pages(&store, settled_cut, limit, CALLS * 4),
            expected_settled
        );
    }
    assert!(
        store
            .load_auxiliary_call(&later.call_id, settled_cut)
            .unwrap()
            .is_none()
    );
    assert!(store.read_auxiliary_audit(cut, None, 0).is_err());
    assert!(
        store
            .read_auxiliary_audit(cut, None, AUXILIARY_AUDIT_PAGE_LIMIT_MAX + 1)
            .is_err()
    );
    assert!(store.read_auxiliary_audit(cut, Some(u64::MAX), 1).is_err());
    assert_eq!(
        durable_state(&store),
        before_reads,
        "bounded historical inspection is read-only"
    );
}

fn measured_sql_steps<T>(store: &SqliteConversationStore, read: impl FnOnce() -> T) -> (T, usize) {
    let steps = Arc::new(AtomicUsize::new(0));
    let observed = steps.clone();
    store
        .conn
        .lock()
        .unwrap()
        .progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
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
    (result, steps.load(Ordering::Relaxed))
}

fn unbounded_historical_page(
    store: &SqliteConversationStore,
    cut: AuxiliaryAuditCut,
    after_offset: u64,
) -> Vec<(u64, u64)> {
    store
        .lock()
        .unwrap()
        .prepare(
            "SELECT sequence,audit_offset FROM auxiliary_audit_facts
             INDEXED BY auxiliary_audit_offset_idx
             WHERE audit_offset>?1 AND sequence<=?2
             ORDER BY audit_offset LIMIT 3",
        )
        .unwrap()
        .query_map(rusqlite::params![after_offset, cut.journal], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn old_final_and_empty_audit_pages_bound_sql_work_across_later_call_growth() {
    let store = store();
    store.initialize(&[]).unwrap();
    let empty_cut = store.auxiliary_audit_cut().unwrap();
    for _ in 0..3 {
        store
            .commit_auxiliary_call_start(&fresh_call(), test_time())
            .unwrap();
    }
    let old_cut = store.auxiliary_audit_cut().unwrap();
    let cases = [
        ("last visible record", old_cut, Some(2)),
        ("already exhausted cursor", old_cut, Some(3)),
        ("captured empty audit", empty_cut, None),
    ];
    let expected = cases.map(|(_, cut, after)| store.read_auxiliary_audit(cut, after, 2).unwrap());
    assert_eq!(expected[0].records.len(), 1);
    assert!(expected[1].records.is_empty());
    assert!(expected[2].records.is_empty());
    assert!(expected.iter().all(|page| page.next_offset.is_none()));
    let mut baseline = Vec::new();
    let mut unbounded_baseline = Vec::new();
    let mut appended = 0;
    for population in [0, 32, 256, 1024] {
        for _ in appended..population {
            store
                .commit_auxiliary_call_start(&fresh_call(), test_time())
                .unwrap();
        }
        appended = population;
        for (index, (name, cut, after)) in cases.iter().enumerate() {
            let (actual, steps) = measured_sql_steps(&store, || {
                store.read_auxiliary_audit(*cut, *after, 2).unwrap()
            });
            assert_eq!(
                actual, expected[index],
                "{name}: later population {population}"
            );
            assert!(steps > 0, "the SQLite VM work probe must be consumed");
            if population == 0 {
                baseline.push(steps);
            } else {
                assert!(
                    steps <= baseline[index] + 128,
                    "{name}: {population} later Calls grew SQL work from {} to {steps}",
                    baseline[index]
                );
            }

            // The same answers with only the Journal filter have to scan the
            // later dense-offset suffix to prove EOF. That query is the
            // negative control for a finite output hiding unbounded work.
            let (unbounded, unbounded_steps) = measured_sql_steps(&store, || {
                unbounded_historical_page(&store, *cut, after.unwrap_or(0))
            });
            let coordinates: Vec<_> = expected[index]
                .records
                .iter()
                .map(|record| (record.journal_sequence, record.audit_offset))
                .collect();
            assert_eq!(unbounded, coordinates);
            if population == 0 {
                unbounded_baseline.push(unbounded_steps);
            } else if population == 1024 {
                assert!(
                    unbounded_steps >= unbounded_baseline[index] + 1024,
                    "{name}: the unbounded negative control did not inspect the later suffix"
                );
            }
        }
    }
}
