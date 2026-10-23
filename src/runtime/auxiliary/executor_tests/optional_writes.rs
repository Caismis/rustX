//! A real interrupted optional DML must never escape its original start transaction.
use super::*;
use crate::durable::presentation::JournalObserver;
use crate::durable::sqlite::AuxiliaryStartProbe;
use crate::durable::{ConversationStore, ObservationStore, RequestStore};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::EventId;
use crate::runtime::observation::{ConversationObservation, PendingObservations};

struct ObservedJournal {
    notifications: Mutex<Vec<Option<u64>>>,
    pending: Arc<PendingObservations>,
}
impl JournalObserver for ObservedJournal {
    fn committed(&self, through: Option<u64>) {
        lock(&self.notifications).push(through);
        self.pending.committed(through);
    }
    fn ready(&self, through: u64) {
        self.pending.ready(through);
    }
}

#[allow(clippy::too_many_lines)] // One native cancellation schedule through durable inspection and source drain.
async fn cancelled_optional_start(request_start: bool, probe: AuxiliaryStartProbe) {
    let fixture = fixture(Script::Complete);
    let _close = CloseScopeOnDrop(fixture.scope.clone());
    let ModelInputMessage::Canonical(prior) = &fixture.frozen.request.messages[0] else {
        unreachable!();
    };
    fixture
        .store
        .initialize(std::slice::from_ref(prior))
        .unwrap();
    let history = fixture.store.load_canonical().unwrap();
    let head = fixture.store.load_head().unwrap();
    let transcript = fixture.store.load_transcript_page(None, 32).unwrap();
    let store: Arc<dyn ConversationStore> = fixture.store.clone();
    let pending = Arc::new(PendingObservations::new());
    pending.bind_journal(&store, 0);
    let observer = Arc::new(ObservedJournal {
        notifications: Mutex::new(Vec::new()),
        pending: pending.clone(),
    });
    assert_eq!(fixture.store.observe_journal(observer.clone()).unwrap(), 0);
    let record = fixture.reservation.record.clone();
    let identity = if request_start {
        record.request_id.to_string()
    } else {
        record.call_id.to_string()
    };
    let (entered, release) =
        SqliteConversationStore::hold_auxiliary_start_probe_for_test(identity, probe);
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    // The consumed progress gate is inside the real INSERT/UPDATE, after its
    // optional savepoint and while the exact native start writer owns SQLite.
    assert_eq!(
        *lock(&observer.notifications),
        if request_start { vec![Some(1)] } else { vec![] }
    );
    let cancelled = lock(&record.writer)
        .as_ref()
        .unwrap()
        .observe_start_cancellation_for_test();
    fixture.scope.cancellation.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(10), cancelled)
        .await
        .unwrap()
        .unwrap();
    drop(release);
    let error = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.kind,
        ContextErrorKind::Cancelled,
        "{probe:?}, request_start={request_start}"
    );
    assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
    assert!(fixture.executor.0.resources.durability.failure().is_none());
    assert!(
        fixture
            .store
            .load_request_owner(&record.request_id)
            .unwrap()
            .is_none()
    );
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    let call = fixture
        .store
        .load_auxiliary_call(&record.call_id, cut)
        .unwrap();
    let expected = if request_start {
        assert_eq!(call.unwrap().outcome, Some(AuxiliaryCallOutcome::Cancelled));
        assert!(
            fixture
                .store
                .read_auxiliary_call_requests(&record.call_id, cut, None, 32)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fixture.store.auxiliary_integrity_counts_for_test(),
            (2, [2, 1, 0, 0, 2])
        );
        vec![Some(1), Some(2)]
    } else {
        assert!(call.is_none());
        assert_eq!(
            fixture.store.auxiliary_integrity_counts_for_test(),
            (0, [0; 5])
        );
        vec![]
    };
    assert_eq!(
        *lock(&observer.notifications),
        expected,
        "no rolled-back Journal frontier may be published"
    );
    assert_eq!(fixture.store.load_canonical().unwrap(), history);
    assert_eq!(fixture.store.load_head().unwrap(), head);
    assert_eq!(
        fixture.store.load_transcript_page(None, 32).unwrap(),
        transcript
    );
    let fresh = CancellationSignal::new();
    let allowances = (0..12)
        .map(|_| {
            fixture
                .scope
                .authority
                .reserve_start(
                    crate::runtime::identity::RequestId::new(uuid::Uuid::now_v7().to_string()),
                    &fresh,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        allowances.len(),
        12,
        "the interrupted RequestStart consumes no allowance"
    );
    let scope = AuxiliaryExecutor::scope(
        &CompactionBudget::new(
            CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            fixture.clock.clone(),
        ),
        &fresh,
    );
    drop(
        fixture
            .executor
            .reserve_general(&scope)
            .expect("later active admission remains healthy"),
    );

    // A real later native append reuses the rolled-back sequence. Drain every
    // actual witness through the production source reader, including settlement.
    let appended = fixture
        .store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(uuid::Uuid::now_v7().to_string()),
            sequence: 0,
            conversation_id: fixture.store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            tool_occurrence: None,
            observation: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            },
        })
        .unwrap();
    assert_eq!(appended.sequence, if request_start { 3 } else { 1 });
    fixture
        .store
        .mark_observation_ready(1, appended.sequence)
        .unwrap();
    let events = fixture.store.read_events(None, 32).unwrap().events;
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1..=appended.sequence).collect::<Vec<_>>()
    );
    let drained = pending.drain();
    let facts = drained
        .iter()
        .flat_map(ConversationObservation::facts)
        .filter_map(|fact| {
            if let ConversationObservation::JournalFact(witness) = fact {
                Some(witness)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        facts.iter().map(|fact| fact.sequence).collect::<Vec<_>>(),
        (1..=appended.sequence).collect::<Vec<_>>()
    );
    assert_eq!(facts.last().unwrap().event_kind, "compaction_started");
    if request_start {
        assert_eq!(facts[0].event_kind, "auxiliary_call_started");
        assert_eq!(facts[1].event_kind, "auxiliary_call_settled");
    }
    assert!(!drained.iter().any(|item| matches!(
        item,
        ConversationObservation::JournalBatch { through: None, .. }
    )));
    assert!(pending.drain().iter().all(|item| !matches!(
        item,
        ConversationObservation::JournalBatch { through: None, .. }
    )));
    fixture.executor.drain().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_optional_call_start_preserves_journal_atomicity_and_observation_drain() {
    for probe in [
        AuxiliaryStartProbe::ReadinessInsert,
        AuxiliaryStartProbe::ObservationFields,
        AuxiliaryStartProbe::ObservationJson,
    ] {
        cancelled_optional_start(false, probe).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_optional_request_start_preserves_journal_atomicity_and_observation_drain() {
    for probe in [
        AuxiliaryStartProbe::ReadinessInsert,
        AuxiliaryStartProbe::ObservationFields,
        AuxiliaryStartProbe::ObservationJson,
    ] {
        cancelled_optional_start(true, probe).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn benign_optional_start_layout_failure_keeps_authoritative_execution_healthy() {
    for request_start in [false, true] {
        let fixture = fixture(Script::Complete);
        let _close = CloseScopeOnDrop(fixture.scope.clone());
        fixture
            .store
            .fail_optional_start_layout_for_test(request_start);
        let call_id = fixture.reservation.call_id().clone();
        fixture
            .reservation
            .submit(fixture.frozen)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 1);
        assert!(fixture.executor.0.resources.durability.failure().is_none());
        let events = fixture.store.read_events(None, 32).unwrap().events;
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        let cut = fixture.store.auxiliary_audit_cut().unwrap();
        assert_eq!(
            fixture
                .store
                .load_auxiliary_call(&call_id, cut)
                .unwrap()
                .unwrap()
                .outcome,
            Some(AuxiliaryCallOutcome::Completed)
        );
        fixture.executor.drain().await;
    }
}
