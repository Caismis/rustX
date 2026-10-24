use super::*;
use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::events::EVENT_SCHEMA_VERSION;
use crate::events::RuntimeEvent;
use crate::events::RuntimeEventEnvelope;
use chrono::TimeZone;

use chrono::{Duration, Utc};
use rusqlite::params;

use crate::durable::ConversationStoreError;
use crate::durable::auxiliary::AuxiliaryWriteFailure;
use crate::events::types::ToolObservationOccurrence;
use crate::message::types::ContentBlockIndex;
use crate::model::auxiliary::{
    AUXILIARY_REQUEST_ID_BYTES_MAX, AuxiliaryAttribution, AuxiliaryAuditCut,
    AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart, AuxiliaryCommitConfirmation,
    AuxiliaryCommitDisposition, AuxiliaryFingerprint, AuxiliaryInputEvidence,
    AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
    AuxiliaryUsageCompleteness,
};
use crate::model::finish::ModelFinishReason;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AttemptId, AuxiliaryCallId, ConversationId, EventId, RequestId, SystemUuidV7Generator, TurnId,
};

struct CompletedFixture {
    store: SqliteConversationStore,
    call: AuxiliaryCallStart,
    request: AuxiliaryRequestStart,
    request_terminal: RuntimeEventEnvelope,
    call_settled: RuntimeEventEnvelope,
    cut: AuxiliaryAuditCut,
}

struct StartedFixture {
    store: SqliteConversationStore,
    call: AuxiliaryCallStart,
    request: AuxiliaryRequestStart,
}

fn fingerprint(byte: char) -> AuxiliaryFingerprint {
    AuxiliaryFingerprint::new(byte.to_string().repeat(64)).unwrap()
}

fn generated_request_id() -> RequestId {
    RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap()
}

fn timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 19, 9, 0, 0)
        .single()
        .unwrap()
}

fn usage() -> ModelUsage {
    ModelUsage {
        input_tokens: Some(11),
        output_tokens: Some(5),
        total_tokens: Some(16),
        details: None,
    }
}

fn make_call_start(call_id: AuxiliaryCallId) -> AuxiliaryCallStart {
    AuxiliaryCallStart {
        call_id,
        purpose: AuxiliaryAttribution::new("summary").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: fingerprint('c'),
        invocation_fingerprint: fingerprint('d'),
        selected_message_ids: Vec::new(),
    }
}

fn make_request_start(
    call_id: AuxiliaryCallId,
    ordinal: u32,
    request_id: RequestId,
) -> AuxiliaryRequestStart {
    AuxiliaryRequestStart {
        call_id,
        ordinal,
        request_id,
        model: "contract-model".into(),
    }
}

fn auxiliary_envelope(
    store: &SqliteConversationStore,
    event_id: String,
    event: RuntimeEvent,
    timestamp: chrono::DateTime<Utc>,
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
        timestamp,
        event,
    }
}

fn request_terminal(
    store: &SqliteConversationStore,
    call_id: AuxiliaryCallId,
    ordinal: u32,
    request_id: RequestId,
    local_completed: bool,
    usage: Option<ModelUsage>,
    timestamp: chrono::DateTime<Utc>,
) -> RuntimeEventEnvelope {
    let event_id = format!(
        "auxiliary-request-{}:{request_id}",
        if local_completed {
            "completed"
        } else {
            "failed"
        }
    );
    let owner = ModelRequestOwner::Auxiliary { call_id, ordinal };
    let event = if local_completed {
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
            error: crate::model::error::ModelError::timeout(
                crate::model::deadline::ModelTimeoutPhase::ResponseStart,
            ),
            usage,
            generation: None,
        }
    };
    auxiliary_envelope(store, event_id, event, timestamp)
}

fn call_terminal(
    store: &SqliteConversationStore,
    call_id: AuxiliaryCallId,
    outcome: AuxiliaryCallOutcome,
    evidence: Option<AuxiliaryRequestEvidence>,
    timestamp: chrono::DateTime<Utc>,
) -> RuntimeEventEnvelope {
    auxiliary_envelope(
        store,
        format!("auxiliary-call-settled:{call_id}"),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            outcome,
            request_evidence: evidence,
        },
        timestamp,
    )
}

fn evidence(
    request_id: RequestId,
    observed: Option<AuxiliaryObservedProviderTerminal>,
    usage: Option<ModelUsage>,
    completeness: AuxiliaryUsageCompleteness,
    uncertainty: bool,
) -> AuxiliaryRequestEvidence {
    AuxiliaryRequestEvidence {
        request_id,
        observed_provider_terminal: observed,
        usage,
        usage_completeness: completeness,
        remote_execution_uncertainty: uncertainty,
    }
}

fn started_fixture() -> StartedFixture {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let call = make_call_start(AuxiliaryCallId::generate());
    let request = make_request_start(call.call_id.clone(), 0, generated_request_id());
    store
        .commit_auxiliary_call_start(&call, timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(&request, timestamp())
        .unwrap();
    StartedFixture {
        store,
        call,
        request,
    }
}

fn completed_fixture() -> CompletedFixture {
    let started = started_fixture();
    let terminal_usage = usage();
    let request_evidence = evidence(
        started.request.request_id.clone(),
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        }),
        Some(terminal_usage.clone()),
        AuxiliaryUsageCompleteness::Complete,
        false,
    );
    let request_terminal = request_terminal(
        &started.store,
        started.call.call_id.clone(),
        started.request.ordinal,
        started.request.request_id.clone(),
        true,
        Some(terminal_usage),
        timestamp(),
    );
    let call_settled = call_terminal(
        &started.store,
        started.call.call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence),
        timestamp(),
    );
    started
        .store
        .commit_auxiliary_request_settlement(request_terminal.clone(), call_settled.clone())
        .unwrap();
    let cut = started.store.auxiliary_audit_cut().unwrap();
    CompletedFixture {
        store: started.store,
        call: started.call,
        request: started.request,
        request_terminal,
        call_settled,
        cut,
    }
}

fn assert_known_not_committed<T: core::fmt::Debug>(
    result: Result<T, AuxiliaryWriteFailure>,
) -> ConversationStoreError {
    match result {
        Err(AuxiliaryWriteFailure::KnownNotCommitted { error }) => error,
        other => panic!("expected KnownNotCommitted, got {other:?}"),
    }
}

fn assert_current_mandatory_reads_reject(fixture: &CompletedFixture) {
    assert!(
        fixture
            .store
            .load_auxiliary_call(&fixture.call.call_id, fixture.cut)
            .is_err()
    );
    assert!(
        fixture
            .store
            .load_auxiliary_request(&fixture.request.request_id, fixture.cut)
            .is_err()
    );
    assert!(
        fixture
            .store
            .confirm_auxiliary_settlement(&fixture.request_terminal, &fixture.call_settled,)
            .is_err()
    );
}

fn request_start_sequence(fixture: &CompletedFixture) -> i64 {
    fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_sequence FROM auxiliary_requests WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap()
}

fn assert_request_start_confirmation_rejects(fixture: &CompletedFixture) {
    assert!(
        fixture
            .store
            .confirm_auxiliary_request_start(&fixture.request)
            .is_err()
    );
}

fn append_generic_history(store: &SqliteConversationStore, count: usize) {
    for index in 0..count {
        let attempt_id = AttemptId::new(format!("contract-filler-{index}"));
        let mut envelope = auxiliary_envelope(
            store,
            format!("contract-filler-event-{index}"),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
            timestamp(),
        );
        envelope.attempt_id = Some(attempt_id);
        store.append_event(envelope).unwrap();
    }
}

#[test]
fn audit_fact_call_request_ordinal_and_kind_mutations_are_rejected_independently() {
    let fixture = completed_fixture();
    let foreign_call = AuxiliaryCallId::generate();
    fixture
        .store
        .commit_auxiliary_call_start(&make_call_start(foreign_call.clone()), timestamp())
        .unwrap();
    let sequence = request_start_sequence(&fixture);
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_audit_facts SET call_id=?1
             WHERE sequence=?2 AND kind='request_started'",
            params![foreign_call.as_str(), sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    let sequence = request_start_sequence(&fixture);
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_audit_facts SET request_id=?1
             WHERE sequence=?2 AND kind='request_started'",
            params![generated_request_id().as_str(), sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    let sequence = request_start_sequence(&fixture);
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_audit_facts SET ordinal=1
             WHERE sequence=?1 AND kind='request_started'",
            [sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    let sequence = request_start_sequence(&fixture);
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_audit_facts
             SET kind='call_started',request_id=NULL,ordinal=NULL
             WHERE sequence=?1 AND kind='request_started'",
            [sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);
}

#[test]
fn owner_identity_and_terminal_mutations_are_rejected_independently() {
    let fixture = completed_fixture();
    let foreign_call = AuxiliaryCallId::generate();
    fixture
        .store
        .commit_auxiliary_call_start(&make_call_start(foreign_call.clone()), timestamp())
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_owners SET call_id=?1 WHERE request_id=?2",
            params![foreign_call.as_str(), fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    let call_settlement_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT sequence FROM auxiliary_audit_facts
             WHERE kind='call_settled' AND call_id=?1",
            [fixture.call.call_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_owners SET terminal_sequence=?1 WHERE request_id=?2",
            params![
                call_settlement_sequence,
                fixture.request.request_id.as_str()
            ],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_owners SET terminal_kind='failed' WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    let call_start_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1",
            [fixture.call.call_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_owners SET start_sequence=?1 WHERE request_id=?2",
            params![call_start_sequence, fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM request_owners WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
    assert_request_start_confirmation_rejects(&fixture);
}

#[test]
fn request_and_call_index_mutations_are_rejected_independently() {
    let fixture = completed_fixture();
    let request_start_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_sequence FROM auxiliary_requests WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_requests SET terminal_sequence=?1 WHERE request_id=?2",
            params![request_start_sequence, fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_requests SET terminal_kind='failed' WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    let foreign_call = AuxiliaryCallId::generate();
    fixture
        .store
        .commit_auxiliary_call_start(&make_call_start(foreign_call.clone()), timestamp())
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_requests SET call_id=?1 WHERE request_id=?2",
            params![foreign_call.as_str(), fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM auxiliary_requests WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_calls SET outcome='failed' WHERE call_id=?1",
            [fixture.call.call_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
}

#[test]
fn missing_mandatory_audit_rows_are_errors_not_historical_absence() {
    let fixture = completed_fixture();
    let call_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1",
            [fixture.call.call_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM auxiliary_audit_facts
             WHERE sequence=?1 AND kind='call_started'",
            [call_sequence],
        )
        .unwrap();
    assert!(
        fixture
            .store
            .load_auxiliary_call(&fixture.call.call_id, fixture.cut)
            .is_err()
    );
    assert!(
        fixture
            .store
            .confirm_auxiliary_call_start(&fixture.call)
            .is_err()
    );

    let fixture = completed_fixture();
    let request_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_sequence FROM auxiliary_requests WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM auxiliary_audit_facts
             WHERE sequence=?1 AND kind='request_started'",
            [request_sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    let terminal_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT terminal_sequence FROM auxiliary_requests WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM auxiliary_audit_facts
             WHERE sequence=?1 AND kind='request_settled'",
            [terminal_sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    let settlement_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT settled_sequence FROM auxiliary_calls WHERE call_id=?1",
            [fixture.call.call_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM auxiliary_audit_facts
             WHERE sequence=?1 AND kind='call_settled'",
            [settlement_sequence],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);
}

#[test]
fn journal_payload_mutations_disagreeing_with_owner_rows_are_rejected() {
    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET event_id='forged-request-start'
             WHERE sequence=(SELECT started_sequence FROM auxiliary_requests
                             WHERE request_id=?1)",
            [fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events
             SET event_json=json_set(event_json,'$.event.owner.ordinal',1)
             WHERE sequence=(SELECT terminal_sequence FROM auxiliary_requests
                             WHERE request_id=?1)",
            [fixture.request.request_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    let replacement = generated_request_id();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events
             SET event_json=json_set(event_json,'$.event.request_id',?1)
             WHERE sequence=(SELECT settled_sequence FROM auxiliary_calls
                             WHERE call_id=?2)",
            params![replacement.as_str(), fixture.call.call_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events
             SET event_json=json_remove(event_json,'$.event.request_evidence')
             WHERE sequence=(SELECT settled_sequence FROM auxiliary_calls
                             WHERE call_id=?1)",
            [fixture.call.call_id.as_str()],
        )
        .unwrap();
    assert_current_mandatory_reads_reject(&fixture);

    let fixture = completed_fixture();
    let request_sequence: i64 = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_sequence FROM auxiliary_requests WHERE request_id=?1",
            [fixture.request.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let mut json: String = fixture
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [request_sequence],
            |row| row.get(0),
        )
        .unwrap();
    json.push_str(&" ".repeat(20_000));
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET event_json=?1 WHERE sequence=?2",
            params![json, request_sequence],
        )
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .load_auxiliary_request(&fixture.request.request_id, fixture.cut),
        Err(ConversationStoreError::ResourceBoundExceeded(_))
    ));
}

#[test]
fn canonical_terminal_envelope_shape_is_exact() {
    let started = started_fixture();
    let terminal_usage = usage();
    let request_evidence = evidence(
        started.request.request_id.clone(),
        Some(AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        }),
        Some(terminal_usage.clone()),
        AuxiliaryUsageCompleteness::Complete,
        false,
    );
    let terminal = request_terminal(
        &started.store,
        started.call.call_id.clone(),
        0,
        started.request.request_id.clone(),
        true,
        Some(terminal_usage),
        timestamp(),
    );
    let settled = call_terminal(
        &started.store,
        started.call.call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(request_evidence),
        timestamp(),
    );

    let mut nonzero = terminal.clone();
    nonzero.sequence = 1;
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(nonzero, settled.clone()),
    );

    let mut wrong_schema = terminal.clone();
    wrong_schema.schema_version += 1;
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(wrong_schema, settled.clone()),
    );

    let mut wrong_event_id = terminal.clone();
    wrong_event_id.event_id = EventId::new("caller-owned-request-terminal");
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(wrong_event_id, settled.clone()),
    );

    let mut wrong_conversation = terminal.clone();
    wrong_conversation.conversation_id = ConversationId::generate();
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(wrong_conversation, settled.clone()),
    );

    let mut with_attempt = terminal.clone();
    with_attempt.attempt_id = Some(AttemptId::new("not-an-auxiliary-attempt"));
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(with_attempt, settled.clone()),
    );

    let mut with_turn = terminal.clone();
    with_turn.turn_id = Some(TurnId::new("not-an-auxiliary-turn"));
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(with_turn, settled.clone()),
    );

    let mut with_tool = terminal.clone();
    with_tool.tool_occurrence = Some(ToolObservationOccurrence {
        tool_ordinal: 1,
        ledger_position: 1,
        block_index: ContentBlockIndex::new(0),
    });
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_request_settlement(with_tool, settled),
    );
}

#[test]
fn exact_settlement_replay_checks_sequence_timestamp_and_payload() {
    let started = started_fixture();
    let terminal = request_terminal(
        &started.store,
        started.call.call_id.clone(),
        0,
        started.request.request_id.clone(),
        true,
        None,
        timestamp(),
    );
    let settled = call_terminal(
        &started.store,
        started.call.call_id.clone(),
        AuxiliaryCallOutcome::Completed,
        Some(evidence(
            started.request.request_id.clone(),
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
            }),
            None,
            AuxiliaryUsageCompleteness::Unreported,
            false,
        )),
        timestamp(),
    );
    let receipt = started
        .store
        .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
        .unwrap();
    assert_eq!(
        receipt.disposition,
        AuxiliaryCommitDisposition::NewlyCommitted
    );
    let stored_request_sequence = receipt.request_terminal.as_ref().unwrap().sequence;
    let stored_call_sequence = receipt.call_settled.sequence;
    drop(receipt);
    append_generic_history(&started.store, 40);

    assert!(matches!(
        started
            .store
            .confirm_auxiliary_settlement(&terminal, &settled)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    let mut sequenced_request = terminal.clone();
    sequenced_request.sequence = stored_request_sequence;
    let mut sequenced_call = settled.clone();
    sequenced_call.sequence = stored_call_sequence;
    assert!(matches!(
        started
            .store
            .confirm_auxiliary_settlement(&sequenced_request, &sequenced_call)
            .unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));

    let mut wrong_sequence = terminal.clone();
    wrong_sequence.sequence = stored_request_sequence + 100;
    assert!(
        started
            .store
            .confirm_auxiliary_settlement(&wrong_sequence, &settled)
            .is_err()
    );

    let mut wrong_timestamp = settled.clone();
    wrong_timestamp.timestamp += Duration::seconds(1);
    assert!(
        started
            .store
            .confirm_auxiliary_settlement(&terminal, &wrong_timestamp)
            .is_err()
    );

    let mut wrong_finish = terminal.clone();
    if let RuntimeEvent::ModelRequestCompleted { finish_reason, .. } = &mut wrong_finish.event {
        *finish_reason = ModelFinishReason::Length;
    }
    assert!(
        started
            .store
            .confirm_auxiliary_settlement(&wrong_finish, &settled)
            .is_err()
    );

    let before = started.store.auxiliary_audit_cut().unwrap();
    assert_eq!(
        started
            .store
            .commit_auxiliary_request_settlement(terminal.clone(), settled.clone())
            .unwrap()
            .disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    assert_eq!(started.store.auxiliary_audit_cut().unwrap(), before);
}

#[test]
fn call_only_terminal_rejects_request_evidence_and_unallocated_sequence_rules_hold() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let call = make_call_start(AuxiliaryCallId::generate());
    store
        .commit_auxiliary_call_start(&call, timestamp())
        .unwrap();
    let mut settled = call_terminal(
        &store,
        call.call_id.clone(),
        AuxiliaryCallOutcome::Cancelled,
        Some(evidence(
            generated_request_id(),
            None,
            None,
            AuxiliaryUsageCompleteness::Unreported,
            true,
        )),
        timestamp(),
    );
    assert_known_not_committed(store.commit_auxiliary_call_settlement(settled.clone()));
    settled.event = RuntimeEvent::AuxiliaryCallSettled {
        call_id: call.call_id.clone(),
        outcome: AuxiliaryCallOutcome::Cancelled,
        request_evidence: None,
    };
    settled.sequence = 1;
    assert_known_not_committed(store.commit_auxiliary_call_settlement(settled.clone()));
    settled.sequence = 0;
    let receipt = store
        .commit_auxiliary_call_settlement(settled.clone())
        .unwrap();
    let stored_sequence = receipt.call_settled.sequence;
    let mut exact = settled;
    exact.sequence = stored_sequence;
    assert!(matches!(
        store.confirm_auxiliary_call_settlement(&exact).unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    exact.timestamp += Duration::milliseconds(1);
    assert!(store.confirm_auxiliary_call_settlement(&exact).is_err());
}

#[test]
fn request_cardinality_and_call_terminal_absorbing_rules_are_enforced() {
    let started = started_fixture();
    let before = started.store.auxiliary_audit_cut().unwrap();
    assert_known_not_committed(started.store.commit_auxiliary_request_start(
        &make_request_start(started.call.call_id.clone(), 0, generated_request_id()),
        timestamp(),
    ));
    assert_known_not_committed(started.store.commit_auxiliary_request_start(
        &make_request_start(started.call.call_id.clone(), 1, generated_request_id()),
        timestamp(),
    ));
    assert_known_not_committed(
        started
            .store
            .commit_auxiliary_call_settlement(call_terminal(
                &started.store,
                started.call.call_id.clone(),
                AuxiliaryCallOutcome::Cancelled,
                None,
                timestamp(),
            )),
    );
    assert_eq!(started.store.auxiliary_audit_cut().unwrap(), before);

    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let call = make_call_start(AuxiliaryCallId::generate());
    store
        .commit_auxiliary_call_start(&call, timestamp())
        .unwrap();
    let terminal = call_terminal(
        &store,
        call.call_id.clone(),
        AuxiliaryCallOutcome::Cancelled,
        None,
        timestamp(),
    );
    store
        .commit_auxiliary_call_settlement(terminal.clone())
        .unwrap();
    assert_eq!(
        store
            .commit_auxiliary_call_settlement(terminal.clone())
            .unwrap()
            .disposition,
        AuxiliaryCommitDisposition::IdempotentReplay
    );
    let mut conflicting = terminal;
    if let RuntimeEvent::AuxiliaryCallSettled { outcome, .. } = &mut conflicting.event {
        *outcome = AuxiliaryCallOutcome::Failed;
    }
    assert_known_not_committed(store.commit_auxiliary_call_settlement(conflicting));
}

#[test]
fn required_schema_indexes_and_constraints_are_current() {
    assert_eq!(SQLITE_SCHEMA_VERSION, 81);
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for index in [
        "auxiliary_requests_open_idx",
        "auxiliary_requests_request_idx",
        "auxiliary_requests_terminal_idx",
        "auxiliary_audit_offset_idx",
        "auxiliary_audit_call_idx",
        "auxiliary_audit_request_idx",
        "auxiliary_audit_sequence_idx",
        "auxiliary_calls_started_idx",
        "auxiliary_calls_settled_idx",
        "request_owners_source_idx",
        "events_auxiliary_call_id_kind_idx",
        "events_auxiliary_fact_type_idx",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type='index' AND name=?1",
                    [index],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1,
            "{index}"
        );
    }
    let index_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type='index' AND name='auxiliary_audit_offset_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(index_sql.to_ascii_uppercase().contains("UNIQUE"));
    let open_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type='index' AND name='auxiliary_requests_open_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(open_sql.to_ascii_uppercase().contains("UNIQUE"));
    assert!(open_sql.to_ascii_uppercase().contains("WHERE"));
}

#[test]
fn auxiliary_request_id_cap_is_owner_scoped() {
    let fixture = completed_fixture();
    let oversized = "r".repeat(AUXILIARY_REQUEST_ID_BYTES_MAX + 1);
    assert!(
        fixture
            .store
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_owners SET request_id=?1 WHERE request_id=?2",
                params![oversized, fixture.request.request_id.as_str()],
            )
            .is_err()
    );
}

#[test]
fn ownerless_terminal_footprint_is_rejected_at_open_without_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ownerless.sqlite");
    let conversation_id = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let call = make_call_start(AuxiliaryCallId::generate());
    let request = make_request_start(call.call_id.clone(), 0, generated_request_id());
    store
        .commit_auxiliary_call_start(&call, timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_start(&request, timestamp())
        .unwrap();
    store
        .commit_auxiliary_request_settlement(
            request_terminal(
                &store,
                call.call_id.clone(),
                0,
                request.request_id.clone(),
                true,
                None,
                timestamp(),
            ),
            call_terminal(
                &store,
                call.call_id,
                AuxiliaryCallOutcome::Completed,
                Some(evidence(
                    request.request_id,
                    Some(AuxiliaryObservedProviderTerminal::Completed {
                        finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                    }),
                    None,
                    AuxiliaryUsageCompleteness::Unreported,
                    false,
                )),
                timestamp(),
            ),
        )
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events
             SET event_json=json_remove(event_json,'$.event.owner')
             WHERE json_extract(event_json,'$.event.type')
                   IN ('model_request_completed','model_request_failed')",
            [],
        )
        .unwrap();
    drop(store);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::IncompatibleSchema(detail))
            if detail.contains("explicit typed ownership")
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn missing_required_index_is_rejected_without_schema_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing-index.sqlite");
    let conversation_id = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch("DROP INDEX auxiliary_audit_offset_idx")
        .unwrap();
    drop(store);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::IncompatibleSchema(detail))
            if detail.contains("auxiliary_audit_offset_idx")
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn falsely_named_indexes_are_rejected_without_schema_rewrite() {
    for (expected_name, corruption) in [
        (
            "auxiliary_audit_sequence_idx",
            "DROP INDEX auxiliary_audit_sequence_idx;
             CREATE INDEX auxiliary_audit_sequence_idx
                 ON auxiliary_audit_facts(kind,audit_offset);",
        ),
        (
            "auxiliary_audit_request_idx",
            "DROP INDEX auxiliary_audit_request_idx;
             CREATE INDEX auxiliary_audit_request_idx
                 ON auxiliary_audit_facts(call_id,sequence);",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wrong-index.sqlite");
        let conversation_id = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store.lock().unwrap().execute_batch(corruption).unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::IncompatibleSchema(detail))
                if detail.contains(expected_name)
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn auxiliary_frontier_queries_use_bounded_index_seeks() {
    use crate::durable::sqlite::auxiliary::audit::{
        AUXILIARY_AUDIT_FRONTIER_SQL, AUXILIARY_AUTHORITATIVE_FRONTIER_SQL,
    };

    let fixture = completed_fixture();
    let connection = fixture.store.lock().unwrap();
    let journal = i64::try_from(fixture.cut.journal).unwrap();
    let explain = |sql: &str| {
        connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([journal], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    let authoritative = explain(AUXILIARY_AUTHORITATIVE_FRONTIER_SQL);
    assert!(
        authoritative
            .iter()
            .filter(|detail| detail.contains("events_auxiliary_fact_type_idx"))
            .count()
            >= 5,
        "{authoritative:?}"
    );
    assert!(
        !authoritative
            .iter()
            .any(|detail| detail.starts_with("SCAN events")),
        "{authoritative:?}"
    );

    let derived = explain(AUXILIARY_AUDIT_FRONTIER_SQL);
    assert!(
        derived
            .iter()
            .any(|detail| detail.contains("auxiliary_audit_sequence_idx")),
        "{derived:?}"
    );
    assert!(
        !derived
            .iter()
            .any(|detail| detail.starts_with("SCAN auxiliary_audit_facts")),
        "{derived:?}"
    );
}

#[test]
fn missing_and_conflicting_audit_records_fail_closed() {
    let fixture = completed_fixture();
    fixture
        .store
        .lock()
        .unwrap()
        .execute("DELETE FROM auxiliary_audit_facts WHERE audit_offset=2", [])
        .unwrap();
    assert!(
        fixture
            .store
            .read_auxiliary_audit(fixture.cut, None, 32)
            .is_err()
    );
    assert!(
        fixture
            .store
            .read_auxiliary_audit(fixture.cut, Some(2), 32)
            .is_err()
    );

    let fixture = completed_fixture();
    let foreign_call = AuxiliaryCallId::generate();
    fixture
        .store
        .commit_auxiliary_call_start(&make_call_start(foreign_call.clone()), timestamp())
        .unwrap();
    fixture
        .store
        .lock()
        .unwrap()
        .execute(
            "UPDATE auxiliary_audit_facts SET call_id=?1 WHERE audit_offset=1",
            [foreign_call.as_str()],
        )
        .unwrap();
    assert!(
        fixture
            .store
            .read_auxiliary_audit(fixture.cut, None, 32)
            .is_err()
    );
}

#[test]
fn deleting_the_latest_open_audit_fact_is_detected_by_total_and_read() {
    for with_request in [false, true] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        let call = make_call_start(AuxiliaryCallId::generate());
        let request = make_request_start(call.call_id.clone(), 0, generated_request_id());
        store
            .commit_auxiliary_call_start(&call, timestamp())
            .unwrap();
        if with_request {
            store
                .commit_auxiliary_request_start(&request, timestamp())
                .unwrap();
        }
        let cut = store.auxiliary_audit_cut().unwrap();
        let latest_offset: i64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT audit_offset FROM auxiliary_audit_facts
                 WHERE sequence<=?1 ORDER BY sequence DESC LIMIT 1",
                [i64::try_from(cut.journal).unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM auxiliary_audit_facts WHERE audit_offset=?1",
                [latest_offset],
            )
            .unwrap();

        assert!(matches!(
            store.auxiliary_audit_total(cut),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(matches!(
            store.read_auxiliary_audit(cut, None, 32),
            Err(ConversationStoreError::InvalidReference(_))
        ));
    }
}

#[test]
fn pre_integration_main_and_auxiliary_versions_are_rejected_without_rewrite() {
    for version in [71, 72, 73, 74] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-integration.sqlite");
        let conversation_id = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version=?1 WHERE id=1",
                [version],
            )
            .unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();
        assert!(
            matches!(SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch { stored, expected: SQLITE_SCHEMA_VERSION })
                if stored == version)
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
