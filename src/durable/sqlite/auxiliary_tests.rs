use super::*;

use chrono::Utc;
use rusqlite::params;

use crate::context::resources::ResourceReadCut;
use crate::durable::auxiliary::AuxiliaryWriteFailure;
use crate::durable::inbox::ConversationStoreError;
use crate::model::auxiliary::{
    AUXILIARY_CALL_START_ENCODED_MAX, AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX,
    AUXILIARY_REQUEST_ID_BYTES_MAX, AUXILIARY_REQUEST_MODEL_BYTES_MAX,
    AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX, AuxiliaryAuditCut, AuxiliaryAuditFact,
    AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryAttribution, AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition,
    AuxiliaryFingerprint, AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal,
    AuxiliaryRequestEvidence, AuxiliaryRequestStart, AuxiliaryRequestTerminal,
    AuxiliaryUsageCompleteness,
};
use crate::model::deadline::ModelTimeoutPhase;
use crate::model::finish::ModelFinishReason;
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AuxiliaryCallId, ConversationId, EventId, MessageId, RequestId, SystemUuidV7Generator,
};

fn fingerprint(byte: char) -> AuxiliaryFingerprint {
    AuxiliaryFingerprint::new(byte.to_string().repeat(64)).unwrap()
}

fn generated_request_id() -> RequestId {
    RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap()
}

fn timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 19, 8, 0, 0)
        .single()
        .unwrap()
}

fn usage(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> ModelUsage {
    ModelUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        details: None,
    }
}

fn call_start(call_id: AuxiliaryCallId) -> AuxiliaryCallStart {
    AuxiliaryCallStart {
        call_id,
        purpose: AuxiliaryAttribution::new("summary").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: fingerprint('a'),
        invocation_fingerprint: fingerprint('b'),
        selected_message_ids: Vec::new(),
    }
}

fn request_start(
    call_id: AuxiliaryCallId,
    ordinal: u32,
    request_id: RequestId,
) -> AuxiliaryRequestStart {
    AuxiliaryRequestStart {
        call_id,
        ordinal,
        request_id,
        model: "auxiliary-model".into(),
    }
}

fn auxiliary_envelope(
    store: &SqliteConversationStore,
    event_id: String,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: timestamp(),
        event,
    }
}

fn request_terminal(
    store: &SqliteConversationStore,
    call_id: AuxiliaryCallId,
    ordinal: u32,
    request_id: RequestId,
    completed: bool,
    usage: Option<ModelUsage>,
) -> RuntimeEventEnvelope {
    let event_id = format!("auxiliary-request-{}:{request_id}", if completed { "completed" } else { "failed" });
    let owner = ModelRequestOwner::Auxiliary { call_id, ordinal };
    let event = if completed {
        RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id,
            finish_reason: ModelFinishReason::Stop,
            usage,
            generation: None,
        }
    } else {
        RuntimeEvent::ModelRequestFailed {
            owner,
            request_id,
            error: crate::model::error::ModelError::timeout(ModelTimeoutPhase::ResponseStart),
            usage,
            generation: None,
        }
    };
    auxiliary_envelope(
        store,
        event_id,
        event,
    )
}

fn failed_terminal_with_encoded_len(
    store: &SqliteConversationStore,
    call_id: AuxiliaryCallId,
    request_id: RequestId,
    encoded_len: usize,
) -> RuntimeEventEnvelope {
    let mut terminal = request_terminal(store, call_id, 0, request_id, false, None);
    let RuntimeEvent::ModelRequestFailed { error, .. } = &mut terminal.event else {
        unreachable!()
    };
    error.message.clear();
    let base_len = serde_json::to_vec(&terminal).unwrap().len();
    assert!(encoded_len >= base_len);
    let RuntimeEvent::ModelRequestFailed { error, .. } = &mut terminal.event else {
        unreachable!()
    };
    error.message = "x".repeat(encoded_len - base_len);
    assert_eq!(serde_json::to_vec(&terminal).unwrap().len(), encoded_len);
    terminal
}

fn call_terminal(
    store: &SqliteConversationStore,
    call_id: AuxiliaryCallId,
    outcome: AuxiliaryCallOutcome,
    request_evidence: Option<AuxiliaryRequestEvidence>,
) -> RuntimeEventEnvelope {
    auxiliary_envelope(
        store,
        format!("auxiliary-call-settled:{call_id}"),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            outcome,
            request_evidence,
        },
    )
}

fn request_evidence(
    request_id: RequestId,
    observed: Option<AuxiliaryObservedProviderTerminal>,
    usage: Option<ModelUsage>,
    completeness: AuxiliaryUsageCompleteness,
    remote_execution_uncertainty: bool,
) -> AuxiliaryRequestEvidence {
    AuxiliaryRequestEvidence {
        request_id,
        observed_provider_terminal: observed,
        usage,
        usage_completeness: completeness,
        remote_execution_uncertainty,
    }
}

fn initialized_store() -> (tempfile::TempDir, SqliteConversationStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &directory.path().join("auxiliary.sqlite"),
    )
    .unwrap();
    store
        .initialize(&[user_message("base", "base")])
        .unwrap();
    (directory, store)
}

fn assert_known_not_committed<T: core::fmt::Debug>(
    result: Result<T, AuxiliaryWriteFailure>,
) -> ConversationStoreError {
    match result {
        Err(AuxiliaryWriteFailure::KnownNotCommitted { error }) => error,
        other => panic!("expected KnownNotCommitted, got {other:?}"),
    }
}

fn append_generic_events(store: &SqliteConversationStore, namespace: &str, count: usize) {
    for index in 0..count {
        let attempt_id =
            crate::runtime::identity::AttemptId::new(format!("{namespace}-{index}"));
        let mut envelope = auxiliary_envelope(
            store,
            format!("{namespace}-event-{index}"),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
        );
        envelope.attempt_id = Some(attempt_id);
        store.append_event(envelope).unwrap();
    }
}

#[test]
fn auxiliary_lifecycle_round_trips_through_real_sqlite_and_reopen() {
    let (directory, store) = initialized_store();
    let conversation_id = store.conversation_id().clone();
    let path = directory.path().join("auxiliary.sqlite");
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    let call = call_start(call_id.clone());
    let request = request_start(call_id.clone(), 0, request_id.clone());

    let call_receipt = store
        .commit_auxiliary_call_start(&call, timestamp())
        .unwrap();
    assert_eq!(
        call_receipt.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    let call_only_cut = store.auxiliary_audit_cut().unwrap();
    let request_receipt = store
        .commit_auxiliary_request_start(&request, timestamp())
        .unwrap();
    assert_eq!(
        request_receipt.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    let before_settlement = store.auxiliary_audit_cut().unwrap();

    let terminal_usage = usage(7, 3, 10);
    let evidence = request_evidence(
        request_id.clone(),
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        }),
        Some(terminal_usage.clone()),
        AuxiliaryUsageCompleteness::Complete,
        false,
    );
    let mut terminal = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        Some(terminal_usage.clone()),
    );
    let generation = GenerationEvidence {
        dispatch_after_start_ms: Some(4),
        first_output_ms: Some(9),
        last_output_ms: Some(12),
        terminal_ms: 14,
    };
    if let RuntimeEvent::ModelRequestCompleted {
        generation: stored, ..
    } = &mut terminal.event
    {
        *stored = Some(generation);
    }
    let settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(evidence.clone()),
    );
    let settlement = store
        .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
        .unwrap();
    assert_eq!(
        settlement.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    assert_eq!(settlement.call.outcome, Some(AuxiliaryCallOutcome::Completed));
    let request_audit = settlement.request.as_ref().unwrap();
    assert_eq!(
        request_audit.local_terminal,
        Some(AuxiliaryRequestTerminal::Completed)
    );
    assert_eq!(request_audit.usage, Some(terminal_usage));
    assert_eq!(request_audit.generation, Some(generation));
    assert_eq!(request_audit.request_evidence, Some(evidence));
    let audit = store
        .read_auxiliary_audit(store.auxiliary_audit_cut().unwrap(), None, 32)
        .unwrap();
    assert_eq!(audit.total, 4);
    assert!(matches!(
        audit.records.as_slice(),
        [
            crate::model::auxiliary::AuxiliaryAuditRecord {
                audit_offset: 1,
                fact: AuxiliaryAuditFact::CallStarted(_),
                ..
            },
            crate::model::auxiliary::AuxiliaryAuditRecord {
                audit_offset: 2,
                fact: AuxiliaryAuditFact::RequestStarted(_),
                ..
            },
            crate::model::auxiliary::AuxiliaryAuditRecord {
                audit_offset: 3,
                fact: AuxiliaryAuditFact::RequestSettled(_),
                ..
            },
            crate::model::auxiliary::AuxiliaryAuditRecord {
                audit_offset: 4,
                fact: AuxiliaryAuditFact::CallSettled(_),
                ..
            },
        ]
    ));

    assert_eq!(
        store.load_request_owner(&request_id).unwrap(),
        Some(ModelRequestOwner::Auxiliary {
            call_id: call_id.clone(),
            ordinal: 0,
        })
    );
    assert_eq!(
        store
            .load_auxiliary_call(&call_id, call_only_cut)
            .unwrap()
            .unwrap()
            .settled_sequence,
        None
    );
    assert!(
        store
            .load_auxiliary_request(&request_id, call_only_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .load_auxiliary_request(&request_id, before_settlement)
            .unwrap()
            .unwrap()
            .request_evidence
            .is_none()
    );
    let snapshots: u64 = store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM request_snapshots", [], |row| row.get(0))
        .unwrap();
    assert_eq!(snapshots, 0);

    drop(store);
    let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
    assert!(matches!(
        reopened.confirm_auxiliary_call_start(&call).unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    assert!(matches!(
        reopened
            .confirm_auxiliary_request_start(&request)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    assert!(matches!(
        reopened
            .confirm_auxiliary_settlement(&terminal, &settled)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
}

#[test]
fn historical_input_uses_the_complete_native_cut() {
    let store = store();
    store
        .initialize(&[user_message("base", "complete source cut")])
        .unwrap();
    let cut = store.compaction_resource_cut().unwrap();

    let mut historical = call_start(AuxiliaryCallId::generate());
    historical.input = AuxiliaryInputEvidence::Historical { cut: cut.clone() };
    historical.business_source = AuxiliaryBusinessSource::HumanMessage {
        message_id: MessageId::new("base"),
    };
    historical.selected_message_ids = vec![MessageId::new("base")];
    store
        .commit_auxiliary_call_start(&historical, timestamp())
        .unwrap();

    let before_canonical = store.auxiliary_audit_cut().unwrap();
    store
        .append_canonical(&user_message("late", "canonical-only advancement"))
        .unwrap();
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before_canonical);
    assert_eq!(
        store
            .load_auxiliary_call(&historical.call_id, before_canonical)
            .unwrap()
            .unwrap()
            .input,
        historical.input
    );
    let mut future_ledger = call_start(AuxiliaryCallId::generate());
    future_ledger.input = AuxiliaryInputEvidence::Historical { cut: cut.clone() };
    future_ledger.selected_message_ids = vec![MessageId::new("late")];
    assert!(matches!(
        store.commit_auxiliary_call_start(&future_ledger, timestamp()),
        Err(AuxiliaryWriteFailure::KnownNotCommitted {
            error: ConversationStoreError::InvalidReference(_)
        })
    ));

    let mut temporary_with_history = call_start(AuxiliaryCallId::generate());
    temporary_with_history.selected_message_ids = vec![MessageId::new("base")];
    assert_known_not_committed(
        store.commit_auxiliary_call_start(&temporary_with_history, timestamp()),
    );

    let mut missing_human = call_start(AuxiliaryCallId::generate());
    missing_human.input = AuxiliaryInputEvidence::Historical { cut: cut.clone() };
    missing_human.business_source = AuxiliaryBusinessSource::HumanMessage {
        message_id: MessageId::new("missing"),
    };
    assert_known_not_committed(store.commit_auxiliary_call_start(&missing_human, timestamp()));

    let mut missing_compaction = call_start(AuxiliaryCallId::generate());
    missing_compaction.business_source = AuxiliaryBusinessSource::Compaction {
        operation_id: crate::runtime::identity::CompactionOperationId::new(
            "missing-compaction",
        ),
    };
    assert_known_not_committed(
        store.commit_auxiliary_call_start(&missing_compaction, timestamp()),
    );

    let mut foreign = call_start(AuxiliaryCallId::generate());
    let mut foreign_cut = cut;
    foreign_cut.conversation_id = ConversationId::generate();
    foreign.input = AuxiliaryInputEvidence::Historical { cut: foreign_cut };
    assert_known_not_committed(store.commit_auxiliary_call_start(&foreign, timestamp()));
}

#[test]
fn auxiliary_bounds_are_checked_before_any_durable_allocation() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();

    let before = store.auxiliary_audit_cut().unwrap();
    let event_rows: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    let request_rows: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM auxiliary_requests", [], |row| row.get(0))
        .unwrap();
    let mut oversized_model = request_start(call_id.clone(), 0, generated_request_id());
    oversized_model.model = "m".repeat(AUXILIARY_REQUEST_MODEL_BYTES_MAX + 1);
    assert_known_not_committed(
        store.commit_auxiliary_request_start(&oversized_model, timestamp()),
    );
    let mut oversized_request = request_start(call_id.clone(), 0, generated_request_id());
    oversized_request.request_id =
        RequestId::new("r".repeat(AUXILIARY_REQUEST_ID_BYTES_MAX + 1));
    assert_known_not_committed(
        store.commit_auxiliary_request_start(&oversized_request, timestamp()),
    );
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);

    let mut too_many_refs = call_start(AuxiliaryCallId::generate());
    too_many_refs.selected_message_ids = (0..=AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX)
        .map(|index| MessageId::new(format!("selected-{index}")))
        .collect();
    assert_known_not_committed(store.commit_auxiliary_call_start(&too_many_refs, timestamp()));

    let mut encoded_oversize = call_start(AuxiliaryCallId::generate());
    encoded_oversize.input = AuxiliaryInputEvidence::Historical {
        cut: store.compaction_resource_cut().unwrap(),
    };
    encoded_oversize.selected_message_ids = (0..AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX)
        .map(|index| MessageId::new(format!("{index}-{}", "x".repeat(600))))
        .collect();
    assert!(serde_json::to_vec(&encoded_oversize).unwrap().len() > AUXILIARY_CALL_START_ENCODED_MAX);
    assert_known_not_committed(
        store.commit_auxiliary_call_start(&encoded_oversize, timestamp()),
    );
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        event_rows
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM auxiliary_requests", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        request_rows
    );

    let request_id = generated_request_id();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let before_terminal = store.auxiliary_audit_cut().unwrap();
    let mut oversized_terminal = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        false,
        None,
    );
    if let RuntimeEvent::ModelRequestFailed { error, .. } = &mut oversized_terminal.event {
        error.message = "x".repeat(20_000);
    }
    let settled = call_terminal(
        &store,
        call_id,
        AuxiliaryCallOutcome::Failed,
        Some(request_evidence(
            request_id,
            None,
            None,
            AuxiliaryUsageCompleteness::Unreported,
            true,
        )),
    );
    assert_known_not_committed(
        store.commit_auxiliary_request_settlement(oversized_terminal, settled),
    );
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before_terminal);
}

#[test]
fn terminal_reserved_sequence_bound_is_exact_after_late_allocation() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    append_generic_events(&store, "terminal-bound-sequence", 9);
    let before = store.auxiliary_audit_cut().unwrap();

    let max_sequence_digits = u64::MAX.to_string().len();
    let exact_initial_len =
        AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX - (max_sequence_digits - 1);
    let settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Failed,
        Some(request_evidence(
            request_id.clone(),
            None,
            None,
            AuxiliaryUsageCompleteness::Unreported,
            true,
        )),
    );
    assert_known_not_committed(store.commit_auxiliary_request_settlement(
        failed_terminal_with_encoded_len(
            &store,
            call_id.clone(),
            request_id.clone(),
            exact_initial_len + 1,
        ),
        settled.clone(),
    ));
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);

    let receipt = store
        .commit_auxiliary_request_settlement(
            failed_terminal_with_encoded_len(
                &store,
                call_id,
                request_id,
                exact_initial_len,
            ),
            settled,
        )
        .unwrap();
    let stored = receipt.request_terminal.unwrap();
    assert!(stored.sequence >= 10);
    let allocated_digits = stored.sequence.to_string().len();
    let stored_len = serde_json::to_vec(&stored).unwrap().len();
    assert_eq!(stored_len, exact_initial_len + allocated_digits - 1);
    assert!(stored_len <= AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX);
}

#[test]
fn observed_completion_kind_must_match_local_completed_terminal() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let before = store.auxiliary_audit_cut().unwrap();
    let terminal = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        None,
    );
    let settled = call_terminal(
        &store,
        call_id,
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence(
            request_id,
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Length,
            }),
            None,
            AuxiliaryUsageCompleteness::Unreported,
            false,
        )),
    );
    assert!(matches!(
        assert_known_not_committed(
            store.commit_auxiliary_request_settlement(terminal, settled)
        ),
        ConversationStoreError::InvalidReference(_)
    ));
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
}

#[test]
fn locally_completed_request_requires_observed_completion_without_partial_write() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let before = store.auxiliary_audit_cut().unwrap();
    let counts_before: (i64, i64, i64) = {
        let connection = store.lock().unwrap();
        (
            connection
                .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                .unwrap(),
            connection
                .query_row("SELECT count(*) FROM auxiliary_audit_facts", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT count(*) FROM request_owners", [], |row| row.get(0))
                .unwrap(),
        )
    };

    let error = assert_known_not_committed(store.commit_auxiliary_request_settlement(
        request_terminal(
            &store,
            call_id.clone(),
            0,
            request_id.clone(),
            true,
            None,
        ),
        call_terminal(
            &store,
            call_id.clone(),
            AuxiliaryCallOutcome::Completed,
            Some(request_evidence(
                request_id.clone(),
                None,
                None,
                AuxiliaryUsageCompleteness::Unreported,
                false,
            )),
        ),
    ));
    assert!(
        error
            .to_string()
            .contains("requires an observed provider completion")
    );
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
    let counts_after: (i64, i64, i64) = {
        let connection = store.lock().unwrap();
        (
            connection
                .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                .unwrap(),
            connection
                .query_row("SELECT count(*) FROM auxiliary_audit_facts", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT count(*) FROM request_owners", [], |row| row.get(0))
                .unwrap(),
        )
    };
    assert_eq!(counts_after, counts_before);
    let request = store
        .load_auxiliary_request(&request_id, before)
        .unwrap()
        .unwrap();
    assert_eq!(request.terminal_sequence, None);
    assert_eq!(
        store
            .load_auxiliary_call(&call_id, before)
            .unwrap()
            .unwrap()
            .settled_sequence,
        None
    );
}

#[test]
fn unknown_provider_completion_prose_is_not_duplicated_into_audit() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let private = "PRIVATE_PROVIDER_FINISH";
    let terminal_usage = usage(5, 3, 8);
    let mut terminal = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        Some(terminal_usage.clone()),
    );
    let RuntimeEvent::ModelRequestCompleted {
        finish_reason, ..
    } = &mut terminal.event
    else {
        unreachable!()
    };
    *finish_reason = ModelFinishReason::Other {
        reason: private.into(),
    };
    store
        .commit_auxiliary_request_settlement(
            terminal,
            call_terminal(
                &store,
                call_id,
                AuxiliaryCallOutcome::Completed,
                Some(request_evidence(
                    request_id,
                    Some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Other,
                    }),
                    Some(terminal_usage),
                    AuxiliaryUsageCompleteness::Complete,
                    false,
                )),
            ),
        )
        .unwrap();

    let page = store
        .read_auxiliary_audit(store.auxiliary_audit_cut().unwrap(), None, 32)
        .unwrap();
    assert!(!serde_json::to_string(&page).unwrap().contains(private));
}

#[test]
fn settlement_evidence_preserves_provider_and_local_outcomes() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();

    let evidence = request_evidence(
        request_id.clone(),
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        }),
        None,
        AuxiliaryUsageCompleteness::Unreported,
        true,
    );
    let terminal = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        false,
        None,
    );
    let settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Failed,
        Some(evidence.clone()),
    );
    let receipt = store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
    let request = receipt.request.unwrap();
    assert_eq!(
        request.local_terminal,
        Some(AuxiliaryRequestTerminal::Failed)
    );
    assert_eq!(
        request.normalized_failure_kind,
        Some(crate::model::error::ModelErrorKind::Timeout)
    );
    assert_eq!(request.usage, None);
    assert_eq!(request.request_evidence, Some(evidence));
}

#[test]
fn incomplete_usage_and_contradictory_provider_evidence_are_rejected() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let before = store.auxiliary_audit_cut().unwrap();

    let missing_usage = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        None,
    );
    let missing_usage_settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence(
            request_id.clone(),
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
            }),
            None,
            AuxiliaryUsageCompleteness::Complete,
            false,
        )),
    );
    assert_known_not_committed(
        store.commit_auxiliary_request_settlement(missing_usage, missing_usage_settled),
    );

    let reported = usage(2, 3, 5);
    let unreported_with_usage = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        Some(reported.clone()),
    );
    let unreported_with_usage_settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence(
            request_id.clone(),
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
            }),
            Some(reported),
            AuxiliaryUsageCompleteness::Unreported,
            false,
        )),
    );
    assert_known_not_committed(
        store.commit_auxiliary_request_settlement(
            unreported_with_usage,
            unreported_with_usage_settled,
        ),
    );

    let local_completed = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        None,
    );
    let provider_failed = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence(
            request_id.clone(),
            Some(AuxiliaryObservedProviderTerminal::Failed {
                kind: crate::model::error::ModelErrorKind::Transport,
            }),
            None,
            AuxiliaryUsageCompleteness::Unreported,
            false,
        )),
    );
    assert_known_not_committed(
        store.commit_auxiliary_request_settlement(local_completed, provider_failed),
    );

    let wrong_key = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        None,
    );
    let wrong_key_settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence(
            generated_request_id(),
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
            }),
            None,
            AuxiliaryUsageCompleteness::Unreported,
            false,
        )),
    );
    assert_known_not_committed(
        store.commit_auxiliary_request_settlement(wrong_key, wrong_key_settled),
    );
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
}

#[test]
fn unreported_usage_stays_absent_instead_of_becoming_zero() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let committed = store
        .commit_auxiliary_request_settlement(
            request_terminal(
                &store,
                call_id.clone(),
                0,
                request_id.clone(),
                true,
                None,
            ),
            call_terminal(
                &store,
                call_id,
                AuxiliaryCallOutcome::Completed,
                Some(request_evidence(
                    request_id.clone(),
                    Some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                    }),
                    None,
                    AuxiliaryUsageCompleteness::Unreported,
                    false,
                )),
            ),
        )
        .unwrap();
    let request = committed.request.unwrap();
    assert_eq!(request.usage, None);
    assert_eq!(
        request
            .request_evidence
            .as_ref()
            .unwrap()
            .usage_completeness,
        AuxiliaryUsageCompleteness::Unreported
    );
    assert_eq!(
        store
            .load_auxiliary_request(&request_id, store.auxiliary_audit_cut().unwrap())
            .unwrap()
            .unwrap()
            .usage,
        None
    );
}

#[test]
fn final_request_settlement_rolls_back_atomically_and_reports_known_noncommit() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    let before = store.auxiliary_audit_cut().unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_call_settlement_audit
             BEFORE INSERT ON auxiliary_audit_facts
             WHEN NEW.kind='call_settled'
             BEGIN SELECT RAISE(ABORT,'injected settlement failure'); END;",
        )
        .unwrap();
    let terminal = request_terminal(
        &store,
        call_id.clone(),
        0,
        request_id.clone(),
        true,
        None,
    );
    let settled = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence(
            request_id.clone(),
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
            }),
            None,
            AuxiliaryUsageCompleteness::Unreported,
            false,
        )),
    );
    assert_known_not_committed(
        store.commit_auxiliary_request_settlement(terminal.clone(), settled.clone()),
    );
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
    assert!(
        store
            .load_auxiliary_request(&request_id, before)
            .unwrap()
            .unwrap()
            .terminal_sequence
            .is_none()
    );
    assert!(
        store
            .load_auxiliary_call(&call_id, before)
            .unwrap()
            .unwrap()
            .settled_sequence
            .is_none()
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_call_settlement_audit")
        .unwrap();
    store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
}

#[test]
fn failed_commit_stays_unknown_and_never_becomes_negative_confirmation() {
    let (directory, store) = initialized_store();
    let conversation_id = store.conversation_id().clone();
    let path = directory.path().join("auxiliary.sqlite");
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TABLE commit_fault_parent(id INTEGER PRIMARY KEY);
             CREATE TABLE commit_fault_child(
                 id INTEGER PRIMARY KEY,
                 parent_id INTEGER NOT NULL
                     REFERENCES commit_fault_parent(id)
                     DEFERRABLE INITIALLY DEFERRED
             );
             CREATE TEMP TRIGGER inject_commit_failure
             AFTER INSERT ON auxiliary_audit_facts
             BEGIN
                 INSERT INTO commit_fault_child(id,parent_id) VALUES(1,999);
             END;
             PRAGMA defer_foreign_keys=ON;",
        )
        .unwrap();
    let start = call_start(AuxiliaryCallId::generate());
    assert!(matches!(
        store.commit_auxiliary_call_start(&start, timestamp()),
        Err(AuxiliaryWriteFailure::Unknown { .. })
    ));
    drop(store);

    let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
    assert_eq!(reopened.auxiliary_audit_cut().unwrap().journal, 0);
    assert_eq!(
        reopened.confirm_auxiliary_call_start(&start).unwrap(),
        AuxiliaryCommitConfirmation::Unknown
    );
}

#[test]
fn zero_request_ack_loss_is_exactly_confirmable_after_later_history() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    let expected = call_terminal(
        &store,
        call_id.clone(),
        AuxiliaryCallOutcome::Cancelled,
        None,
    );
    let receipt = store
        .commit_auxiliary_call_settlement(expected.clone())
        .unwrap();
    assert!(receipt.request.is_none());
    let stored_sequence = receipt.call.settled_sequence.unwrap();
    drop(receipt);
    append_generic_events(&store, "zero-call-later", 40);
    let before_confirmation = store.auxiliary_audit_cut().unwrap();

    assert!(matches!(
        store
            .confirm_auxiliary_call_settlement(&expected)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    let mut with_sequence = expected.clone();
    with_sequence.sequence = stored_sequence;
    let with_sequence = store
        .confirm_auxiliary_call_settlement(&with_sequence)
        .unwrap();
    assert!(matches!(
        with_sequence,
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    assert_eq!(
        store.auxiliary_audit_cut().unwrap(),
        before_confirmation,
        "confirmation is read-only"
    );
    assert_eq!(
        store
            .confirm_auxiliary_call_settlement(&call_terminal(
                &store,
                AuxiliaryCallId::generate(),
                AuxiliaryCallOutcome::Cancelled,
                None,
            ))
            .unwrap(),
        AuxiliaryCommitConfirmation::Unknown
    );
}

#[test]
fn call_only_settlement_rejects_an_open_request() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, generated_request_id()),
            timestamp(),
        )
        .unwrap();
    let before = store.auxiliary_audit_cut().unwrap();
    assert_known_not_committed(store.commit_auxiliary_call_settlement(call_terminal(
        &store,
        call_id,
        AuxiliaryCallOutcome::Cancelled,
        None,
    )));
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
}

#[test]
fn generic_append_cannot_bypass_auxiliary_semantic_transitions() {
    let store = store();
    store.initialize(&[]).unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    let call_start_event = auxiliary_envelope(
        &store,
        format!("auxiliary-call-started:{call_id}"),
        RuntimeEvent::AuxiliaryCallStarted {
            call_id: call_id.clone(),
            purpose: AuxiliaryAttribution::new("summary").unwrap(),
            trigger: AuxiliaryAttribution::new("automatic").unwrap(),
            business_source: AuxiliaryBusinessSource::Independent,
            input: AuxiliaryInputEvidence::Temporary,
            semantic_input_fingerprint: fingerprint('a'),
            invocation_fingerprint: fingerprint('b'),
            selected_message_ids: Vec::new(),
        },
    );
    assert!(
        store
            .append_event(call_start_event)
            .unwrap_err()
            .to_string()
            .contains("specialized")
    );
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    let request_start_event = auxiliary_envelope(
        &store,
        format!("auxiliary-request-started:{request_id}"),
        RuntimeEvent::ModelRequestStarted {
            owner: ModelRequestOwner::Auxiliary {
                call_id: call_id.clone(),
                ordinal: 0,
            },
            request_id: request_id.clone(),
            model: "auxiliary-model".into(),
        },
    );
    assert!(store.append_event(request_start_event).is_err());
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    assert!(
        store
            .append_event(request_terminal(
                &store,
                call_id.clone(),
                0,
                request_id,
                true,
                None,
            ))
            .is_err()
    );
    assert!(
        store
            .append_event(call_terminal(
                &store,
                call_id,
                AuxiliaryCallOutcome::Completed,
                None,
            ))
            .is_err()
    );
}

#[test]
fn dense_audit_offsets_and_total_are_independent_of_journal_sequence() {
    let store = store();
    store.initialize(&[]).unwrap();
    let mut call_ids = Vec::new();
    for batch in 0..33 {
        let call_id = AuxiliaryCallId::generate();
        store
            .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
            .unwrap();
        call_ids.push(call_id);
        append_generic_events(&store, &format!("audit-filler-{batch}"), 2);
    }
    let cut = store.auxiliary_audit_cut().unwrap();
    assert_eq!(store.auxiliary_audit_total(cut).unwrap(), 33);
    assert_ne!(store.auxiliary_audit_total(cut).unwrap(), cut.journal + 1);

    let first = store.read_auxiliary_audit(cut, None, 32).unwrap();
    assert_eq!(first.cut, cut);
    assert_eq!(first.total, 33);
    assert_eq!(first.records.len(), 32);
    assert_eq!(first.records[0].audit_offset, 1);
    assert_eq!(first.records[31].audit_offset, 32);
    assert_eq!(first.next_offset, Some(32));
    assert!(
        first
            .records
            .windows(2)
            .all(|pair| pair[0].journal_sequence < pair[1].journal_sequence)
    );

    let second = store
        .read_auxiliary_audit(cut, first.next_offset, 32)
        .unwrap();
    assert_eq!(second.total, 33);
    assert_eq!(second.records.len(), 1);
    assert_eq!(second.records[0].audit_offset, 33);
    assert_eq!(second.next_offset, None);

    let later = AuxiliaryCallId::generate();
    store
        .commit_auxiliary_call_start(&call_start(later.clone()), timestamp())
        .unwrap();
    assert_eq!(store.read_auxiliary_audit(cut, None, 32).unwrap(), first);
    assert_eq!(store.auxiliary_audit_total(cut).unwrap(), 33);
    assert!(store.load_auxiliary_call(&later, cut).unwrap().is_none());
    assert!(
        store
            .read_auxiliary_audit(
                AuxiliaryAuditCut {
                    journal: u64::MAX,
                },
                None,
                1,
            )
            .is_err()
    );
    assert!(store.read_auxiliary_audit(cut, Some(34), 1).is_err());
    for (index, record) in first
        .records
        .iter()
        .chain(second.records.iter())
        .enumerate()
    {
        assert_eq!(record.audit_offset, index as u64 + 1);
    }
    assert!(store.load_auxiliary_call(&call_ids[0], cut).unwrap().is_some());
}

#[test]
fn empty_auxiliary_audit_reports_zero_not_the_journal_frontier() {
    let store = store();
    store.initialize(&[]).unwrap();
    append_generic_events(&store, "empty-audit", 12);
    let cut = store.auxiliary_audit_cut().unwrap();
    assert!(cut.journal >= 12);
    assert_eq!(store.auxiliary_audit_total(cut).unwrap(), 0);
    let page = store.read_auxiliary_audit(cut, None, 32).unwrap();
    assert_eq!(page.total, 0);
    assert!(page.records.is_empty());
    assert_eq!(page.next_offset, None);
}

#[test]
fn optional_observation_capture_failure_preserves_authoritative_audit() {
    let store = store();
    store.initialize(&[]).unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_optional_observation
             BEFORE INSERT ON event_observation_readiness
             BEGIN SELECT RAISE(ABORT,'optional observation failure'); END;",
        )
        .unwrap();
    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    let call = store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    store
        .commit_auxiliary_request_settlement(
            request_terminal(
                &store,
                call_id.clone(),
                0,
                request_id.clone(),
                true,
                None,
            ),
            call_terminal(
                &store,
                call_id.clone(),
                AuxiliaryCallOutcome::Completed,
                Some(request_evidence(
                    request_id.clone(),
                    Some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                    }),
                    None,
                    AuxiliaryUsageCompleteness::Unreported,
                    false,
                )),
            ),
        )
        .unwrap();
    let unavailable: i64 = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_readiness_missing FROM events WHERE sequence=?1",
            [i64::try_from(call.event.sequence).unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unavailable, 1);
    assert!(
        store
            .load_auxiliary_request(&request_id, store.auxiliary_audit_cut().unwrap())
            .unwrap()
            .is_some()
    );
}

#[test]
fn auxiliary_completion_never_establishes_primary_p_marker() {
    let store = store();
    store.initialize(&[user_message("base", "base")]).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, timestamp())
        .unwrap();
    let primary_p = |store: &SqliteConversationStore| -> Option<i64> {
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT completed_sequence FROM request_snapshots WHERE request_id=?1",
                [snapshot.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(primary_p(&store), None);

    let call_id = AuxiliaryCallId::generate();
    let request_id = generated_request_id();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &request_start(call_id.clone(), 0, request_id.clone()),
            timestamp(),
        )
        .unwrap();
    store
        .commit_auxiliary_request_settlement(
            request_terminal(
                &store,
                call_id.clone(),
                0,
                request_id.clone(),
                true,
                None,
            ),
            call_terminal(
                &store,
                call_id,
                AuxiliaryCallOutcome::Completed,
                Some(request_evidence(
                    request_id,
                    Some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                    }),
                    None,
                    AuxiliaryUsageCompleteness::Unreported,
                    false,
                )),
            ),
        )
        .unwrap();
    assert_eq!(primary_p(&store), None);
    let counts: (u64, u64) = {
        let connection = store.lock().unwrap();
        (
            connection
                .query_row("SELECT count(*) FROM publication_streams", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT count(*) FROM publication_audits", [], |row| {
                    row.get(0)
                })
                .unwrap(),
        )
    };
    assert_eq!(counts, (0, 0));

    let mut agent_completion = auxiliary_envelope(
        &store,
        "agent-completion".into(),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Agent {
                identity: snapshot.identity.clone(),
            },
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    agent_completion.attempt_id = Some(snapshot.identity.attempt_id.clone());
    agent_completion.turn_id = Some(snapshot.identity.turn.clone());
    store.append_event(agent_completion).unwrap();
    assert_eq!(
        primary_p(&store),
        Some(i64::try_from(store.auxiliary_audit_cut().unwrap().journal).unwrap())
    );
}

#[test]
fn auxiliary_request_cap_does_not_limit_agent_request_identity() {
    let store = store();
    store.initialize(&[user_message("base", "base")]).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, timestamp())
        .unwrap();
    let oversized = format!("request:{}", "x".repeat(600));
    assert!(oversized.len() > AUXILIARY_REQUEST_ID_BYTES_MAX);
    let start_sequence: i64 = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT start_sequence FROM request_owners WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let updated = store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_owners SET request_id=?1
             WHERE owner_kind='agent' AND start_sequence=?2",
            params![oversized, start_sequence],
        )
        .unwrap();
    assert_eq!(updated, 1);
}

#[test]
fn auxiliary_request_cannot_reuse_an_agent_request_owner() {
    let store = store();
    store
        .initialize(&[user_message("base", "base")])
        .unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, timestamp())
        .unwrap();
    let call_id = AuxiliaryCallId::generate();
    store
        .commit_auxiliary_call_start(&call_start(call_id.clone()), timestamp())
        .unwrap();
    let before = store.auxiliary_audit_cut().unwrap();
    assert_known_not_committed(store.commit_auxiliary_request_start(
        &request_start(call_id, 0, snapshot.request_id.clone()),
        timestamp(),
    ));
    assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
    assert_eq!(
        store.load_request_owner(&snapshot.request_id).unwrap(),
        Some(ModelRequestOwner::Agent {
            identity: snapshot.identity,
        })
    );
    assert!(
        store
            .load_auxiliary_request(
                &snapshot.request_id,
                store.auxiliary_audit_cut().unwrap(),
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn schema_70_is_rejected_without_rewriting_the_database() {
    let (directory, store) = initialized_store();
    let conversation_id = store.conversation_id().clone();
    let path = directory.path().join("auxiliary.sqlite");
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE rustx_store SET schema_version=70 WHERE id=1",
            [],
        )
        .unwrap();
    drop(store);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 70,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn temporary_input_fact_contains_no_input_bytes() {
    let store = store();
    store.initialize(&[]).unwrap();
    let mut start = call_start(AuxiliaryCallId::generate());
    start.input = AuxiliaryInputEvidence::Temporary;
    let receipt = store
        .commit_auxiliary_call_start(&start, timestamp())
        .unwrap();
    let encoded = serde_json::to_string(&receipt.event).unwrap();
    assert!(!encoded.contains("temporary-secret-prompt"));
    assert!(encoded.contains("\"mode\":\"temporary\""));
}

#[test]
fn resource_read_cut_is_part_of_the_stored_call_identity() {
    let store = store();
    store.initialize(&[user_message("base", "base")]).unwrap();
    let cut: ResourceReadCut = store.compaction_resource_cut().unwrap();
    let mut start = call_start(AuxiliaryCallId::generate());
    start.input = AuxiliaryInputEvidence::Historical { cut: cut.clone() };
    start.selected_message_ids = vec![MessageId::new("base")];
    let receipt = store
        .commit_auxiliary_call_start(&start, timestamp())
        .unwrap();
    assert_eq!(receipt.call.input, AuxiliaryInputEvidence::Historical { cut });
    assert!(matches!(
        receipt.event.event,
        RuntimeEvent::AuxiliaryCallStarted { .. }
    ));
}

#[test]
fn business_sources_may_follow_the_frozen_input_cut() {
    let store = store();
    store
        .initialize(&[user_message("base", "frozen input")])
        .unwrap();
    let input_cut = store.compaction_resource_cut().unwrap();
    store
        .append_canonical(&user_message("trigger", "later business cause"))
        .unwrap();
    let operation_id = crate::runtime::identity::CompactionOperationId::new("later-operation");
    let operation = store
        .append_event(auxiliary_envelope(
            &store,
            "later-operation-start".into(),
            RuntimeEvent::CompactionStarted {
                operation_id: operation_id.clone(),
            },
        ))
        .unwrap();
    assert!(operation.sequence > input_cut.journal.0);
    for source in [
        AuxiliaryBusinessSource::HumanMessage {
            message_id: MessageId::new("trigger"),
        },
        AuxiliaryBusinessSource::Compaction { operation_id },
    ] {
        let mut start = call_start(AuxiliaryCallId::generate());
        start.input = AuxiliaryInputEvidence::Historical {
            cut: input_cut.clone(),
        };
        start.selected_message_ids = vec![MessageId::new("base")];
        start.business_source = source;
        let committed = store
            .commit_auxiliary_call_start(&start, timestamp())
            .unwrap();
        assert_eq!(committed.call.input, start.input);
        assert_eq!(committed.call.business_source, start.business_source);
        assert!(matches!(
            store.confirm_auxiliary_call_start(&start).unwrap(),
            AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
        ));
        let read = store
            .load_auxiliary_call(&start.call_id, store.auxiliary_audit_cut().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(read, committed.call);

        // The same later message is still invalid as selected historical input.
        start.call_id = AuxiliaryCallId::generate();
        start.selected_message_ids.push(MessageId::new("trigger"));
        assert_known_not_committed(store.commit_auxiliary_call_start(&start, timestamp()));
    }
}

#[test]
fn consumer_defined_attribution_uses_the_shared_store_lifecycle() {
    use crate::durable::auxiliary::auxiliary_settlement_envelope;
    let store = store();
    let mut start = call_start(AuxiliaryCallId::generate());
    start.purpose = AuxiliaryAttribution::new("translation").unwrap();
    start.trigger = AuxiliaryAttribution::new("document_revision").unwrap();
    start.business_source = AuxiliaryBusinessSource::Business {
        namespace: AuxiliaryAttribution::new("document.translation").unwrap(),
        reference: AuxiliaryAttribution::new("document-42:revision-7").unwrap(),
    };
    let encoded = serde_json::to_string(&start).unwrap();
    assert_eq!(
        serde_json::from_str::<AuxiliaryCallStart>(&encoded).unwrap(),
        start
    );
    store
        .commit_auxiliary_call_start(&start, timestamp())
        .unwrap();
    let request = request_start(start.call_id.clone(), 0, generated_request_id());
    store
        .commit_auxiliary_request_start(&request, timestamp())
        .unwrap();
    let terminal = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        timestamp(),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary {
                call_id: start.call_id.clone(),
                ordinal: 0,
            },
            request_id: request.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    )
    .unwrap();
    let settled = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        timestamp(),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: start.call_id.clone(),
            outcome: AuxiliaryCallOutcome::Completed,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id: request.request_id,
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                }),
                usage: None,
                usage_completeness: AuxiliaryUsageCompleteness::Unreported,
                remote_execution_uncertainty: false,
            }),
        },
    )
    .unwrap();
    let receipt = store
        .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
        .unwrap();
    assert_eq!(receipt.call.purpose, start.purpose);
    assert_eq!(receipt.call.trigger, start.trigger);
    assert_eq!(receipt.call.business_source, start.business_source);
    assert!(matches!(
        store
            .confirm_auxiliary_settlement(&terminal, &settled)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    let replay = store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
    assert_eq!(
        replay.disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(
        store
            .read_auxiliary_audit(store.auxiliary_audit_cut().unwrap(), None, 32)
            .unwrap()
            .total,
        4
    );
}

#[test]
fn settlement_constructor_rejects_primary_and_nonterminal_events() {
    use crate::durable::auxiliary::auxiliary_settlement_envelope;
    let store = store();
    let owner = ModelRequestOwner::Agent {
        identity: crate::model::RequestIdentity {
            attempt_id: crate::runtime::identity::AttemptId::new("attempt"),
            turn: crate::runtime::identity::TurnId::new("turn"),
            retry_number: 0,
        },
    };
    for event in [
        RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id: RequestId::new("primary"),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
        RuntimeEvent::TurnStarted,
    ] {
        assert!(
            auxiliary_settlement_envelope(store.conversation_id().clone(), timestamp(), event)
                .is_err()
        );
    }
}
