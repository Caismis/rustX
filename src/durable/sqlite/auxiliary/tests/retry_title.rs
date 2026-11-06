//! SQLite transaction winners and exact receipts for retry and native Title sources.
use super::*;
use crate::durable::auxiliary::{AuxiliaryTitleSourceBinding, auxiliary_settlement_envelope};
use crate::durable::{AuxiliaryStore, CanonicalStore, ConversationStore};
use crate::events::RuntimeEvent;
use crate::model::auxiliary::*;
use crate::model::deadline::ModelTimeoutPhase;
use crate::model::{ModelError, ModelRequestOwner};
use crate::runtime::identity::{AuxiliaryCallId, RequestId, SystemUuidV7Generator};
use chrono::Utc;
use std::sync::{Arc, Barrier};

fn call() -> AuxiliaryCallStart {
    AuxiliaryCallStart {
        call_id: AuxiliaryCallId::generate(),
        purpose: AuxiliaryAttribution::new("test").unwrap(),
        trigger: AuxiliaryAttribution::new("test").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
        invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
        selected_message_ids: vec![],
    }
}
fn request(call: &AuxiliaryCallStart, ordinal: u32) -> AuxiliaryRequestStart {
    AuxiliaryRequestStart {
        call_id: call.call_id.clone(),
        ordinal,
        request_id: RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap(),
        model: "test".into(),
    }
}
fn terminal(
    store: &SqliteConversationStore,
    call: &AuxiliaryCallStart,
) -> crate::events::RuntimeEventEnvelope {
    auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        Utc::now(),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: call.call_id.clone(),
            outcome: AuxiliaryCallOutcome::Cancelled,
            request_evidence: None,
        },
    )
    .unwrap()
}
fn retry(
    store: &SqliteConversationStore,
    request: &AuxiliaryRequestStart,
) -> (
    crate::events::RuntimeEventEnvelope,
    crate::events::RuntimeEventEnvelope,
) {
    let error = ModelError::timeout(ModelTimeoutPhase::ResponseStart);
    let failed = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        Utc::now(),
        RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Auxiliary {
                call_id: request.call_id.clone(),
                ordinal: request.ordinal,
            },
            request_id: request.request_id.clone(),
            error: error.clone(),
            usage: None,
            generation: None,
        },
    )
    .unwrap();
    let scheduled = auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        Utc::now(),
        RuntimeEvent::AuxiliaryRetryScheduled {
            call_id: request.call_id.clone(),
            request_id: request.request_id.clone(),
            next_ordinal: request.ordinal + 1,
            delay_ms: 2000,
            request_evidence: AuxiliaryRequestEvidence {
                request_id: request.request_id.clone(),
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Failed {
                    kind: error.kind,
                }),
                usage: None,
                usage_completeness: AuxiliaryUsageCompleteness::Unreported,
                remote_execution_uncertainty: true,
            },
        },
    )
    .unwrap();
    (failed, scheduled)
}
fn title(store: &SqliteConversationStore) -> (AuxiliaryCallStart, AuxiliaryTitleSourceBinding) {
    let source = store.first_human_source().unwrap().unwrap();
    let mut call = call();
    call.business_source = AuxiliaryBusinessSource::HumanMessage {
        message_id: source.message_id.clone(),
    };
    (call, AuxiliaryTitleSourceBinding::new(&source).unwrap())
}
fn disk() -> (tempfile::TempDir, SqliteConversationStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        &directory.path().join("state.db"),
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    (directory, store)
}

#[test]
fn retries_commit_atomically_preserve_each_request_evidence_and_confirm_exact_receipts() {
    let (_directory, store) = disk();
    let call = call();
    store
        .commit_auxiliary_call_start(&call, Utc::now())
        .unwrap();
    let first = request(&call, 0);
    store
        .commit_auxiliary_request_start(&first, Utc::now())
        .unwrap();
    let (failed, scheduled) = retry(&store, &first);
    let mut invalid = scheduled.clone();
    if let RuntimeEvent::AuxiliaryRetryScheduled { next_ordinal, .. } = &mut invalid.event {
        *next_ordinal = 2;
    }
    assert!(
        store
            .commit_auxiliary_retry_settlement(failed.clone(), invalid)
            .is_err()
    );
    assert!(
        store
            .load_auxiliary_request(&first.request_id, store.auxiliary_audit_cut().unwrap())
            .unwrap()
            .unwrap()
            .terminal_sequence
            .is_none()
    );
    let before = store.auxiliary_audit_cut().unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER reject_retry_index BEFORE INSERT ON auxiliary_retries BEGIN SELECT RAISE(ABORT,'injected second-fact failure'); END;").unwrap();
    assert!(matches!(
        store.commit_auxiliary_retry_settlement(failed.clone(), scheduled.clone()),
        Err(crate::durable::AuxiliaryWriteFailure::KnownNotCommitted { .. })
    ));
    assert_eq!(
        store.auxiliary_audit_cut().unwrap(),
        before,
        "a failed retry index insert rolls back its preceding request terminal and both Journal events"
    );
    assert!(
        store
            .load_auxiliary_request(&first.request_id, before)
            .unwrap()
            .unwrap()
            .terminal_sequence
            .is_none()
    );
    assert!(matches!(
        store
            .confirm_auxiliary_retry_settlement(&failed, &scheduled)
            .unwrap(),
        AuxiliaryCommitConfirmation::Unknown
    ));
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_retry_index;")
        .unwrap();
    let committed = store
        .commit_auxiliary_retry_settlement(failed.clone(), scheduled.clone())
        .unwrap();
    assert_eq!(
        committed.retry_scheduled.sequence,
        committed.request_terminal.sequence + 1
    );
    for _ in 0..3 {
        let AuxiliaryCommitConfirmation::ConfirmedCommitted(receipt) = store
            .confirm_auxiliary_retry_settlement(&failed, &scheduled)
            .unwrap()
        else {
            panic!("exact committed retry must confirm")
        };
        assert_eq!(receipt.request_terminal, committed.request_terminal);
        assert_eq!(receipt.retry_scheduled, committed.retry_scheduled);
    }
    let mut conflicting = scheduled.clone();
    if let RuntimeEvent::AuxiliaryRetryScheduled { delay_ms, .. } = &mut conflicting.event {
        *delay_ms += 1;
    }
    assert!(
        store
            .confirm_auxiliary_retry_settlement(&failed, &conflicting)
            .is_err(),
        "a receipt for the same IDs cannot confirm different retry evidence"
    );
    let second = request(&call, 1);
    store
        .commit_auxiliary_request_start(&second, Utc::now())
        .unwrap();
    let (second_failed, second_scheduled) = retry(&store, &second);
    store
        .commit_auxiliary_retry_settlement(second_failed, second_scheduled)
        .unwrap();
    store
        .commit_auxiliary_call_settlement(terminal(&store, &call))
        .unwrap();
    let cut = store.auxiliary_audit_cut().unwrap();
    for request in [&first, &second] {
        assert!(
            store
                .load_auxiliary_request(&request.request_id, cut)
                .unwrap()
                .unwrap()
                .request_evidence
                .unwrap()
                .remote_execution_uncertainty
        );
    }
    let audit = store.read_auxiliary_audit(cut, None, 32).unwrap();
    assert_eq!(audit.total, 8);
    assert_eq!(
        audit
            .records
            .iter()
            .filter(|record| matches!(record.fact, AuxiliaryAuditFact::RetryScheduled(_)))
            .count(),
        2
    );
    assert!(
        store
            .commit_auxiliary_request_start(&request(&call, 2), Utc::now())
            .is_err()
    );
}

#[test]
fn last_call_start_slot_has_one_real_concurrent_transaction_winner() {
    let (directory, store) = disk();
    let call = call();
    store
        .commit_auxiliary_call_start(&call, Utc::now())
        .unwrap();
    for ordinal in 0..2 {
        let request = request(&call, ordinal);
        store
            .commit_auxiliary_request_start(&request, Utc::now())
            .unwrap();
        let (failed, retry) = retry(&store, &request);
        store
            .commit_auxiliary_retry_settlement(failed, retry)
            .unwrap();
    }
    let contender = SqliteConversationStore::open(
        store.conversation_id().clone(),
        &directory.path().join("state.db"),
    )
    .unwrap();
    let gate = Arc::new(Barrier::new(2));
    let other_gate = gate.clone();
    let second = request(&call, 2);
    let thread = std::thread::spawn(move || {
        other_gate.wait();
        contender
            .commit_auxiliary_request_start(&second, Utc::now())
            .is_ok()
    });
    let final_start = request(&call, 2);
    gate.wait();
    let won = store
        .commit_auxiliary_request_start(&final_start, Utc::now())
        .is_ok();
    assert_ne!(won, thread.join().unwrap());
    assert_eq!(
        store
            .read_auxiliary_call_requests(
                &call.call_id,
                store.auxiliary_audit_cut().unwrap(),
                None,
                32
            )
            .unwrap()
            .len(),
        3
    );
    assert!(
        store
            .commit_auxiliary_request_start(&request(&call, 3), Utc::now())
            .is_err()
    );
}

#[test]
fn zero_start_release_exact_replay_and_first_start_consumption_survive_reopen() {
    let (directory, store) = disk();
    store
        .append_canonical(&user_message("first", "native source"))
        .unwrap();
    let (first, binding) = title(&store);
    store
        .commit_auxiliary_title_call_start(&first, &binding, Utc::now())
        .unwrap();
    let zero = terminal(&store, &first);
    store
        .commit_auxiliary_call_settlement(zero.clone())
        .unwrap();
    let (second, binding) = title(&store);
    store
        .commit_auxiliary_title_call_start(&second, &binding, Utc::now())
        .unwrap();
    store
        .commit_auxiliary_call_settlement(zero.clone())
        .unwrap();
    assert!(matches!(
        store.confirm_auxiliary_call_settlement(&zero).unwrap(),
        AuxiliaryCommitConfirmation::ConfirmedCommitted(_)
    ));
    assert!(!store.first_human_source().unwrap().unwrap().eligible);
    assert!(
        store
            .commit_auxiliary_request_start(&request(&first, 0), Utc::now())
            .is_err()
    );
    store
        .commit_auxiliary_request_start(&request(&second, 0), Utc::now())
        .unwrap();
    drop(store);
    let reopened = SqliteConversationStore::open(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        &directory.path().join("state.db"),
    )
    .unwrap();
    reopened
        .recover_auxiliary_title_source(&binding, Utc::now())
        .unwrap();
    assert!(!reopened.first_human_source().unwrap().unwrap().eligible);
    let (new_call, source) = title(&reopened);
    assert!(
        reopened
            .commit_auxiliary_title_call_start(&new_call, &source, Utc::now())
            .is_err()
    );
}

#[test]
fn first_start_racing_zero_request_settlement_has_one_transaction_winner() {
    let (directory, store) = disk();
    store
        .append_canonical(&user_message("first", "native source"))
        .unwrap();
    let (call, binding) = title(&store);
    store
        .commit_auxiliary_title_call_start(&call, &binding, Utc::now())
        .unwrap();
    let contender = SqliteConversationStore::open(
        store.conversation_id().clone(),
        &directory.path().join("state.db"),
    )
    .unwrap();
    let gate = Arc::new(Barrier::new(2));
    let other_gate = gate.clone();
    let zero = terminal(&store, &call);
    let thread = std::thread::spawn(move || {
        other_gate.wait();
        contender.commit_auxiliary_call_settlement(zero).is_ok()
    });
    gate.wait();
    let started = store
        .commit_auxiliary_request_start(&request(&call, 0), Utc::now())
        .is_ok();
    assert_ne!(started, thread.join().unwrap());
    assert_eq!(
        store.first_human_source().unwrap().unwrap().eligible,
        !started
    );
}

#[test]
fn concurrent_native_admissions_share_one_association_and_restart_releases_only_zero_start() {
    let (directory, store) = disk();
    store
        .append_canonical(&user_message("first", "native source"))
        .unwrap();
    let (first, binding) = title(&store);
    let (second, other_binding) = title(&store);
    let contender = SqliteConversationStore::open(
        store.conversation_id().clone(),
        &directory.path().join("state.db"),
    )
    .unwrap();
    let gate = Arc::new(Barrier::new(2));
    let other_gate = gate.clone();
    let thread = std::thread::spawn(move || {
        other_gate.wait();
        contender
            .commit_auxiliary_title_call_start(&second, &other_binding, Utc::now())
            .is_ok()
    });
    gate.wait();
    let admitted = store
        .commit_auxiliary_title_call_start(&first, &binding, Utc::now())
        .is_ok();
    assert_ne!(admitted, thread.join().unwrap());
    assert!(!store.first_human_source().unwrap().unwrap().eligible);
    drop(store);
    let reopened = SqliteConversationStore::open(
        binding.conversation_id.clone(),
        &directory.path().join("state.db"),
    )
    .unwrap();
    reopened
        .recover_auxiliary_title_source(&binding, Utc::now())
        .unwrap();
    assert!(reopened.first_human_source().unwrap().unwrap().eligible);
}
