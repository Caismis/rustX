use super::*;
use crate::context::checkpoint::{CheckpointItem, CheckpointMessage};
use crate::durable::reading::AuxiliaryAuditSnapshot;
use crate::durable::{CanonicalStore, JournalStore, ProjectionStore};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::message::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::auxiliary::{
    AuxiliaryAttribution, AuxiliaryAuditFact, AuxiliaryBusinessSource, AuxiliaryCallOutcome,
    AuxiliaryCallStart, AuxiliaryFingerprint, AuxiliaryInputEvidence,
    AuxiliaryObservedProviderTerminal, AuxiliaryRequestAudit, AuxiliaryRequestEvidence,
    AuxiliaryRequestStart, AuxiliaryUsageCompleteness,
};
use crate::model::error::ModelErrorKind;
use crate::model::projected::{
    CanonicalMessageRef, DerivedModelMessage, ProjectedMessage, SemanticTransformation,
};
use crate::model::{ModelFinishReason, ModelRequestOwner, ModelUsage};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{MessageId, RequestId};
use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

fn auxiliary_fingerprint(byte: char) -> AuxiliaryFingerprint {
    AuxiliaryFingerprint::new(byte.to_string().repeat(64)).unwrap()
}

fn start_auxiliary_call(
    store: &SqliteConversationStore,
    purpose: AuxiliaryAttribution,
    source: AuxiliaryBusinessSource,
    with_request: bool,
) -> (crate::runtime::identity::AuxiliaryCallId, Option<RequestId>) {
    let call_id = crate::runtime::identity::AuxiliaryCallId::generate();
    if let AuxiliaryBusinessSource::Compaction { operation_id } = &source {
        store
            .append_event(RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: EventId::new(""),
                sequence: 0,
                conversation_id: store.conversation_id().clone(),
                attempt_id: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
                event: RuntimeEvent::CompactionStarted {
                    operation_id: operation_id.clone(),
                },
            })
            .unwrap();
    }
    store
        .commit_auxiliary_call_start(
            &AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose,
                trigger: AuxiliaryAttribution::new("automatic").unwrap(),
                business_source: source,
                input: AuxiliaryInputEvidence::Historical {
                    cut: store.compaction_resource_cut().unwrap(),
                },
                semantic_input_fingerprint: auxiliary_fingerprint('a'),
                invocation_fingerprint: auxiliary_fingerprint('b'),
                selected_message_ids: Vec::new(),
            },
            chrono::Utc::now(),
        )
        .unwrap();
    let request_id = with_request.then(|| {
        let request_id =
            RequestId::generate_auxiliary(&crate::runtime::identity::SystemUuidV7Generator)
                .unwrap();
        store
            .commit_auxiliary_request_start(
                &AuxiliaryRequestStart {
                    call_id: call_id.clone(),
                    ordinal: 0,
                    request_id: request_id.clone(),
                    model: "fixture/summary".into(),
                },
                chrono::Utc::now(),
            )
            .unwrap();
        request_id
    });
    (call_id, request_id)
}

fn start_summary_call(
    store: &SqliteConversationStore,
    source: AuxiliaryBusinessSource,
    with_request: bool,
) -> (crate::runtime::identity::AuxiliaryCallId, Option<RequestId>) {
    start_auxiliary_call(
        store,
        AuxiliaryAttribution::new("summary").unwrap(),
        source,
        with_request,
    )
}

fn settle_summary_call(
    store: &SqliteConversationStore,
    call_id: crate::runtime::identity::AuxiliaryCallId,
    request_id: RequestId,
) {
    settle_summary_call_with_outcome(store, call_id, request_id, AuxiliaryCallOutcome::Completed);
}

fn settle_summary_call_with_outcome(
    store: &SqliteConversationStore,
    call_id: crate::runtime::identity::AuxiliaryCallId,
    request_id: RequestId,
    outcome: AuxiliaryCallOutcome,
) -> crate::durable::AuxiliarySettlementCommit {
    let usage = ModelUsage {
        input_tokens: 40,
        output_tokens: 8,
        total_tokens: 48,
        details: None,
    };
    let owner = ModelRequestOwner::Auxiliary {
        call_id: call_id.clone(),
        ordinal: 0,
    };
    let event = if outcome == AuxiliaryCallOutcome::Completed {
        RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id: request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage.clone()),
            generation: None,
        }
    } else {
        RuntimeEvent::ModelRequestFailed {
            owner,
            request_id: request_id.clone(),
            error: crate::model::error::ModelError {
                kind: if outcome == AuxiliaryCallOutcome::Cancelled {
                    ModelErrorKind::Cancelled
                } else {
                    ModelErrorKind::Authentication
                },
                message: "Summary generation did not complete".into(),
                retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
            usage: Some(usage.clone()),
            generation: None,
        }
    };
    let observed_provider_terminal = match &event {
        RuntimeEvent::ModelRequestCompleted { .. } => {
            Some(AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
            })
        }
        RuntimeEvent::ModelRequestFailed { error, .. } => {
            Some(AuxiliaryObservedProviderTerminal::Failed {
                kind: error.kind.clone(),
            })
        }
        _ => unreachable!(),
    };
    let envelope = |event| {
        crate::durable::auxiliary::auxiliary_settlement_envelope(
            store.conversation_id().clone(),
            chrono::Utc::now(),
            event,
        )
        .unwrap()
    };
    let terminal = envelope(event);
    let settled = envelope(RuntimeEvent::AuxiliaryCallSettled {
        call_id,
        outcome,
        request_evidence: Some(AuxiliaryRequestEvidence {
            request_id,
            observed_provider_terminal,
            usage: Some(usage),
            usage_completeness: AuxiliaryUsageCompleteness::Complete,
            remote_execution_uncertainty: false,
        }),
    });
    store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap()
}

fn summary_reference(
    store: &SqliteConversationStore,
    call_id: crate::runtime::identity::AuxiliaryCallId,
    through: u64,
) -> crate::context::checkpoint::SummaryGenerationRef {
    crate::context::checkpoint::SummaryGenerationRef {
        conversation_id: store.conversation_id().clone(),
        call_id,
        through_journal: through,
    }
}

fn complete_summary_snapshot() -> AuxiliaryAuditSnapshot {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let operation = "operation:summary-audit-bounds";
    let source = AuxiliaryBusinessSource::Compaction {
        operation_id: CompactionOperationId::new(operation),
    };
    let (call_id, request_id) = start_summary_call(&store, source, true);
    settle_summary_call(&store, call_id.clone(), request_id.unwrap());
    let mut checkpoint = candidate(
        &store,
        "summary-audit-bounds",
        vec![summary("bounded audit")],
    );
    checkpoint.summary_generation = Some(summary_reference(
        &store,
        call_id,
        store.auxiliary_audit_cut().unwrap().journal,
    ));
    store
        .commit_projection(commit_input(
            &store,
            operation,
            checkpoint,
            &CancellationSignal::new(),
        ))
        .unwrap();
    let mut audits = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap()
        .auxiliary_audits;
    assert_eq!(audits.len(), 1);
    audits.pop().unwrap()
}

fn update_summary_requests(
    snapshot: &mut AuxiliaryAuditSnapshot,
    mut update: impl FnMut(&mut AuxiliaryRequestAudit),
) {
    for record in &mut snapshot.records {
        if let AuxiliaryAuditFact::RequestStarted(request)
        | AuxiliaryAuditFact::RequestSettled(request) = &mut record.fact
        {
            update(request);
        }
    }
}

fn update_summary_calls(
    snapshot: &mut AuxiliaryAuditSnapshot,
    mut update: impl FnMut(&mut crate::model::auxiliary::AuxiliaryCallAudit),
) {
    for record in &mut snapshot.records {
        if let AuxiliaryAuditFact::CallStarted(call) | AuxiliaryAuditFact::CallSettled(call) =
            &mut record.fact
        {
            update(call);
        }
    }
}

fn assert_fresh_receipt_replays_original(
    original: &ProjectionCommitReceipt,
    mut replayed: ProjectionCommitReceipt,
) {
    assert!(original.newly_committed);
    assert!(!replayed.newly_committed);
    // Only physical delivery differs; compare every original native field.
    replayed.newly_committed = true;
    assert_eq!(&replayed, original);
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    assert_original_publication_replays(&original.completed, &replayed.completed);
}

fn assert_original_publication_replays(
    original: &RuntimeEventEnvelope,
    replayed: &RuntimeEventEnvelope,
) {
    use crate::durable::observation::NativeObservationTransition;
    let original = original
        .observation
        .as_ref()
        .expect("fresh original publication evidence");
    let replayed = replayed
        .observation
        .as_ref()
        .expect("replayed original publication evidence");
    assert_eq!(original.read_cut, replayed.read_cut);
    assert_eq!(original.ready_before, replayed.ready_before);
    assert_eq!(original.ready_t_through, replayed.ready_t_through);
    assert_eq!(original.trace_total, replayed.trace_total);
    assert_eq!(original.source_unavailable, replayed.source_unavailable);
    assert_eq!(original.journal_range, replayed.journal_range);
    assert_eq!(original.failure, replayed.failure);
    assert_eq!(original.failure_deferred, replayed.failure_deferred);
    assert_eq!(original.failure_readable_end, replayed.failure_readable_end);
    assert_eq!(original.failure_sequence, replayed.failure_sequence);
    assert_eq!(
        original.finish_reason_deferred,
        replayed.finish_reason_deferred
    );
    assert!(original.finish_reason.is_none() && replayed.finish_reason.is_none());
    assert!(original.canonical.is_none() && replayed.canonical.is_none());
    assert!(original.pending.is_none() && replayed.pending.is_none());
    let (
        NativeObservationTransition::Single(original),
        NativeObservationTransition::Single(replayed),
    ) = (&original.transition, &replayed.transition)
    else {
        panic!("original single compaction transition")
    };
    assert_eq!(
        serde_json::to_value(original).unwrap(),
        serde_json::to_value(replayed).unwrap()
    );
}

fn message(n: u64) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("M{n}")),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("content {n}"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}
fn store() -> SqliteConversationStore {
    SqliteConversationStore::in_memory(ConversationId::generate()).unwrap()
}
fn summary(text: &str) -> CheckpointItem {
    CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
                source: UserSource::Runtime,
                kind: crate::model::projected::ProjectedUserKind::Message,
                timestamp: None,
            },
            sources: Vec::new(),
            checkpoint_sources: Vec::new(),
            transformations: vec![SemanticTransformation::Summary],
        })),
        call_sources: Vec::new(),
        result_call: None,
    }
}
fn candidate(
    store: &SqliteConversationStore,
    id: &str,
    mut items: Vec<CheckpointItem>,
) -> CheckpointCandidate {
    // Ordinary fixtures describe transformations of a real immutable source.
    // Negative provenance tests explicitly remove it after candidate assembly.
    let first: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT message_json FROM message_ledger ORDER BY position LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let source: MessageBlock = serde_json::from_str(&first).unwrap();
    for item in &mut items {
        if let CheckpointMessage::Derived(body) = &mut item.message
            && body.sources.is_empty()
            && body.checkpoint_sources.is_empty()
        {
            body.sources
                .push(CanonicalMessageRef::of(store.conversation_id(), &source));
        }
    }
    let cut = store.compaction_resource_cut().unwrap();
    CheckpointCandidate {
        checkpoint_id: CheckpointId::new(id),
        expected_head: store
            .projection_head()
            .unwrap()
            .map(|p| p.candidate.checkpoint_id),
        base_frontier: cut.ledger,
        read_cut: cut,
        items,
        summary_metadata: None,
        summary_generation: None,
    }
}
fn commit_input(
    store: &SqliteConversationStore,
    operation: &str,
    candidate: CheckpointCandidate,
    signal: &CancellationSignal,
) -> ProjectionCommitInput {
    let operation_id = CompactionOperationId::new(operation);
    ProjectionCommitInput {
        grant: signal
            .authorize_compaction_commit(operation_id.clone())
            .expect("commit granted"),
        operation_id,
        candidate,
        owner: store.projection_owner(None).unwrap(),
        tokens_before: TokenMeasurement {
            input_tokens: 100,
            source: TokenMeasurementSource::Estimated,
        },
        estimated_tokens_after: 10,
        occupancy: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
    }
}

#[test]
fn checkpoint_base_excludes_m96_through_m100_and_only_m101_enters_tail() {
    let store = store();
    let messages: Vec<_> = (1..=100).map(message).collect();
    store.initialize(&messages).unwrap();
    let original = |n: usize| CheckpointItem {
        message: CheckpointMessage::Original(CanonicalMessageRef::of(
            store.conversation_id(),
            &messages[n - 1],
        )),
        call_sources: Vec::new(),
        result_call: None,
    };
    let value = candidate(
        &store,
        "P1",
        vec![summary("summary M1–M70"), original(90), original(95)],
    );
    let receipt = store
        .commit_projection(commit_input(
            &store,
            "O1",
            value,
            &CancellationSignal::new(),
        ))
        .unwrap();
    assert_eq!(
        receipt.checkpoint.candidate.base_frontier,
        LedgerFrontier(100)
    );
    assert_eq!(
        store.load_canonical().unwrap(),
        messages,
        "projection commits no canonical summary"
    );
    store.append_canonical(&message(101)).unwrap();
    let history = store.projected_history().unwrap();
    let ids: Vec<_> = history
        .messages
        .iter()
        .filter_map(|m| m.canonical_id().map(ToString::to_string))
        .collect();
    assert_eq!(ids, ["M90", "M95", "M101"]);
    assert!(history.messages[0].canonical_id().is_none());
    assert_eq!(history.cut.ledger, LedgerFrontier(101));
    assert_eq!(history.cut.base_frontier, LedgerFrontier(100));
}

#[test]
fn operation_receipts_are_idempotent_conflicting_and_never_rewind_a_newer_head() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("first")]),
        &CancellationSignal::new(),
    );
    let receipt = store.commit_projection(first.clone()).unwrap();
    assert_fresh_receipt_replays_original(
        &receipt,
        store.commit_projection(first.clone()).unwrap(),
    );
    let second = store
        .commit_projection(commit_input(
            &store,
            "O2",
            candidate(&store, "P2", vec![summary("second")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    // Replay remains the exact historical receipt even after a newer head.
    assert_fresh_receipt_replays_original(
        &receipt,
        store.commit_projection(first.clone()).unwrap(),
    );
    assert_eq!(store.projection_head().unwrap().unwrap(), second.checkpoint);
    let mut conflicting = first;
    conflicting.candidate.items = vec![summary("changed candidate")];
    assert!(
        store
            .commit_projection(conflicting)
            .unwrap_err()
            .to_string()
            .contains("different candidate")
    );
    let events = store.read_events(None, 64).unwrap();
    assert_eq!(
        events
            .events
            .iter()
            .filter(|e| matches!(e.event, RuntimeEvent::CompactionCompleted { .. }))
            .count(),
        2
    );
}

#[test]
fn unrelated_journal_appends_and_post_base_canonical_growth_do_not_stale_a_candidate() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("one")]),
        &CancellationSignal::new(),
    );
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: input.operation_id.clone(),
            },
        })
        .unwrap();
    store.append_canonical(&message(2)).unwrap();
    store.commit_projection(input).unwrap();
    let history = store.projected_history().unwrap();
    assert_eq!(history.cut.base_frontier, LedgerFrontier(1));
    assert_eq!(history.messages[1].canonical_id(), Some(message(2).id()));
}

#[test]
fn cancellation_before_grant_prevents_commit_and_after_grant_preserves_authorized_durability() {
    let before = CancellationSignal::new();
    before.cancel();
    assert!(
        before
            .authorize_compaction_commit(CompactionOperationId::new("cancelled"))
            .is_none()
    );
    let store = std::sync::Arc::new(store());
    store.initialize(&[message(1)]).unwrap();
    let signal = CancellationSignal::new();
    let (granted_tx, granted_rx) = std::sync::mpsc::channel();
    let (commit_tx, commit_rx) = std::sync::mpsc::channel();
    let thread = {
        let store = store.clone();
        let signal = signal.clone();
        std::thread::spawn(move || {
            let input = commit_input(
                &store,
                "O1",
                candidate(&store, "P1", vec![summary("authorized")]),
                &signal,
            );
            granted_tx.send(()).unwrap(); // linearization: shared cancellation gate granted O1
            commit_rx.recv().unwrap();
            store.commit_projection(input).unwrap()
        })
    };
    granted_rx.recv().unwrap();
    signal.cancel();
    commit_tx.send(()).unwrap();
    let receipt = thread.join().unwrap();
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
    assert!(
        store
            .read_events(None, 64)
            .unwrap()
            .events
            .iter()
            .all(|e| !matches!(e.event, RuntimeEvent::CompactionFailed { .. }))
    );
}

#[test]
fn same_ledger_frontier_retains_both_activation_boundaries() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "O1",
            candidate(&store, "P1", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "O2",
            candidate(&store, "P2", vec![summary("second")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let timeline = store
        .projection_timeline(ProjectionActivationId(0), 64)
        .unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0].activation.ledger, timeline[1].activation.ledger);
    assert_ne!(
        timeline[0].activation.activation,
        timeline[1].activation.activation
    );
    let first = store
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(1))
        .unwrap();
    let second = store
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
        .unwrap();
    assert_eq!(
        first.projections.last().unwrap().candidate.checkpoint_id,
        CheckpointId::new("P1")
    );
    assert_eq!(
        second.projections.last().unwrap().candidate.checkpoint_id,
        CheckpointId::new("P2")
    );
}

#[test]
fn missing_head_content_or_original_dependencies_never_fall_back_to_identity() {
    for missing in ["head", "content", "original"] {
        let store = store();
        let original = message(1);
        store.initialize(std::slice::from_ref(&original)).unwrap();
        let item = CheckpointItem {
            message: CheckpointMessage::Original(CanonicalMessageRef::of(
                store.conversation_id(),
                &original,
            )),
            call_sources: Vec::new(),
            result_call: None,
        };
        store
            .commit_projection(commit_input(
                &store,
                "O1",
                candidate(&store, "P1", vec![item]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        let connection = store.lock().unwrap();
        match missing {
            "head" => {
                connection
                    .execute("DELETE FROM projection_head", [])
                    .unwrap();
            }
            "content" => {
                connection
                    .execute("UPDATE projection_checkpoints SET candidate_json='{}'", [])
                    .unwrap();
            }
            _ => {
                connection
                    .execute("UPDATE message_ledger SET message_json='{}'", [])
                    .unwrap();
            }
        }
        drop(connection);
        assert!(
            store.projected_history().is_err(),
            "{missing} must fail explicitly"
        );
    }
}

#[test]
fn unknown_commit_outcome_recovers_the_same_operation_without_reexecution() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("one")]),
        &CancellationSignal::new(),
    );
    store.arm_compaction_fault_script([
        crate::durable::sqlite::CompactionFaultOperation::AfterCommit,
    ]);
    assert!(
        store.commit_projection(input.clone()).is_err(),
        "the transaction committed but its receipt was lost"
    );
    let receipt = store
        .projection_receipt(&input.operation_id, &input.candidate.fingerprint())
        .unwrap()
        .unwrap();
    assert_eq!(store.commit_projection(input).unwrap(), receipt);
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
    assert_eq!(store.read_events(None, 64).unwrap().events.len(), 1);
    assert!(matches!(
        store.projected_history().unwrap().messages[0],
        crate::model::ModelInputMessage::Derived(_)
    ));
}

#[test]
fn execution_owner_transition_fences_a_prepared_checkpoint_grant() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("one")]),
        &CancellationSignal::new(),
    );
    let attempt = AttemptId::new("new-owner");
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt.clone()),
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        })
        .unwrap();
    assert!(
        store
            .commit_projection(input)
            .unwrap_err()
            .to_string()
            .contains("owner changed")
    );
    assert!(store.projection_head().unwrap().is_none());
    assert_eq!(store.load_canonical().unwrap(), [message(1)]);
}

#[test]
fn activation_ledger_is_commit_time_frontier_and_not_checkpoint_base() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "growth",
        candidate(&store, "before-growth", vec![summary("summary")]),
        &CancellationSignal::new(),
    );
    store.append_canonical(&message(2)).unwrap();
    let committed = store.commit_projection(input).unwrap();
    assert_eq!(
        committed.checkpoint.candidate.base_frontier,
        LedgerFrontier(1)
    );
    assert_eq!(committed.checkpoint.activation.ledger, LedgerFrontier(2));
    let earlier = store
        .read_lineage_cut(crate::conversation::SurfaceRevision::new(1))
        .unwrap();
    assert!(
        earlier.projections.is_empty(),
        "a future activation is legitimately ineligible at the earlier canonical boundary"
    );
    assert!(
        store
            .read_projection_lineage_cut(
                LedgerFrontier(1),
                committed.checkpoint.activation.activation
            )
            .is_err()
    );
    let at = store
        .read_projection_lineage_cut(
            LedgerFrontier(2),
            committed.checkpoint.activation.activation,
        )
        .unwrap();
    assert_eq!(at.projections.len(), 1);
    let history = store.projected_history().unwrap();
    assert_eq!(
        history.messages.iter().last().unwrap().canonical_id(),
        Some(&MessageId::new("M2"))
    );
}

#[test]
fn ordinary_projection_preparation_does_not_hydrate_retired_summary_provenance() {
    let store = store();
    let source = (1..=1000).map(message).collect::<Vec<_>>();
    store.initialize(&source).unwrap();
    let mut derived = summary("bounded immutable summary");
    let CheckpointMessage::Derived(body) = &mut derived.message else {
        unreachable!()
    };
    body.sources = source
        .iter()
        .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
        .collect();
    let p = candidate(
        &store,
        "bounded",
        vec![
            derived,
            CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &source[989],
                )),
                call_sources: vec![],
                result_call: None,
            },
        ],
    );
    store
        .commit_projection(commit_input(
            &store,
            "bounded-op",
            p,
            &CancellationSignal::new(),
        ))
        .unwrap();
    store.append_canonical(&message(1001)).unwrap();
    super::super::compaction_resources::take_body_decodes();
    let input = store.projected_history().unwrap();
    assert_eq!(input.messages.len(), 3);
    assert_eq!(
        super::super::compaction_resources::take_body_decodes(),
        2,
        "only the visible Original and post-base Tail are hydrated; 1000 retired provenance facts are scalar integrity reads"
    );
    store.lock().unwrap().execute("UPDATE message_ledger SET message_json=replace(message_json,'content 1','corrupt 1') WHERE message_id='M1'",[]).unwrap();
    assert!(
        store.projected_history().is_err(),
        "provenance-only corruption still fails closed"
    );
}

#[test]
fn dependency_free_checkpoint_rejects_every_future_resource_cut_atomically() {
    for domain in [
        "ledger",
        "journal",
        "transcript",
        "activation",
        "conversation",
    ] {
        let store = store();
        store.initialize(&[message(1)]).unwrap();
        let mut value = candidate(&store, "P-invalid", vec![summary("detached")]);
        let CheckpointMessage::Derived(body) = &mut value.items[0].message else {
            unreachable!()
        };
        body.sources.clear();
        match domain {
            "ledger" => {
                value.read_cut.ledger = LedgerFrontier(100);
                value.base_frontier = LedgerFrontier(100);
            }
            "journal" => value.read_cut.journal = JournalFrontier(100),
            "transcript" => value.read_cut.transcript = crate::durable::TranscriptCursor::new(100),
            "activation" => value.read_cut.projection_activation = ProjectionActivationId(100),
            _ => value.read_cut.conversation_id = ConversationId::generate(),
        }
        let before = store.compaction_resource_cut().unwrap();
        let error = store
            .commit_projection(commit_input(
                &store,
                "invalid",
                value,
                &CancellationSignal::new(),
            ))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("foreign or future compaction Resource Read Cut"),
            "{domain} must fail at the unconditional cut validator, not the later provenance guard: {error}"
        );
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert!(store.projection_head().unwrap().is_none());
        let connection = store.lock().unwrap();
        for table in [
            "projection_checkpoints",
            "projection_activations",
            "compaction_operations",
        ] {
            let count: u64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "partial {table}");
        }
    }
}

#[test]
fn trusted_metadata_sources_share_one_predecode_materialization_budget() {
    for count in [16, 33] {
        let store = store();
        let messages: Vec<_> = (1..=count)
            .map(|n| {
                let mut m = message(n);
                if let MessageBlock::User(u) = &mut m {
                    u.content = vec![UserContentBlock::Text(TextBlock {
                        text: "x".repeat(512 * 1024),
                    })];
                }
                m
            })
            .collect();
        store.initialize(&messages).unwrap();
        let mut item = summary("bounded metadata");
        let CheckpointMessage::Derived(d) = &mut item.message else {
            unreachable!()
        };
        d.sources = messages
            .iter()
            .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
            .collect();
        let mut value = candidate(&store, "metadata", vec![item]);
        value.summary_metadata = Some(crate::message::types::CompactionSummaryMetadata::empty());
        super::super::compaction_resources::take_body_decodes();
        let result = store.commit_projection(commit_input(
            &store,
            "metadata",
            value,
            &CancellationSignal::new(),
        ));
        let decoded = super::super::compaction_resources::take_body_decodes();
        if count == 16 {
            result.unwrap();
            assert_eq!(decoded, 16);
        } else {
            assert!(result.is_err(), "cumulative 16 MiB budget bypassed");
            assert_eq!(decoded, 31, "next body must be rejected before decoding");
            assert!(store.projection_head().unwrap().is_none());
        }
    }
}

#[test]
fn early_lineage_selection_never_hydrates_later_large_checkpoints() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "O1",
            candidate(&store, "P1", vec![summary("early")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    for n in 2..=10 {
        store.append_canonical(&message(n)).unwrap();
        store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                candidate(
                    &store,
                    &format!("P{n}"),
                    vec![summary(&"x".repeat(2 * 1024 * 1024))],
                ),
                &CancellationSignal::new(),
            ))
            .unwrap();
    }
    take_candidate_decodes();
    let early = store
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(1))
        .unwrap();
    assert_eq!(take_candidate_decodes(), 1, "later bodies were not decoded");
    assert_eq!(early.projections.len(), 1);
    assert_eq!(early.projections[0].candidate.checkpoint_id.as_str(), "P1");
    let selected = store
        .read_projection_lineage_cut(LedgerFrontier(10), ProjectionActivationId(1))
        .unwrap();
    assert_eq!(selected.projections.len(), 1);
    assert_eq!(
        take_candidate_decodes(),
        1,
        "explicit activation excludes later same-cut bodies"
    );
    let cut = store
        .read_message_lineage_cut(
            crate::conversation::SurfaceRevision::new(10),
            &MessageId::new("M2"),
            false,
        )
        .unwrap();
    assert_eq!(cut.projections.len(), 1);
    assert_eq!(
        take_candidate_decodes(),
        1,
        "ordinary Fork selects before hydration"
    );
}

#[test]
fn metadata_checkpoint_sources_cannot_reset_the_operation_budget() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let mut sources = Vec::new();
    for n in 1..=3 {
        let mut parent = candidate(
            &store,
            &format!("parent-{n}"),
            vec![summary(&"x".repeat(6 * 1024 * 1024))],
        );
        parent.summary_metadata = Some(crate::message::types::CompactionSummaryMetadata::empty());
        let receipt = store
            .commit_projection(commit_input(
                &store,
                &format!("parent-{n}"),
                parent,
                &CancellationSignal::new(),
            ))
            .unwrap();
        sources.push(crate::model::projected::CheckpointSourceRef {
            conversation_id: store.conversation_id().clone(),
            checkpoint_id: receipt.checkpoint.candidate.checkpoint_id,
            content_digest: receipt.candidate_fingerprint,
        });
    }
    let mut item = summary("cumulative metadata");
    let CheckpointMessage::Derived(d) = &mut item.message else {
        unreachable!()
    };
    d.checkpoint_sources = sources;
    let mut value = candidate(&store, "child", vec![item]);
    value.summary_metadata = Some(crate::message::types::CompactionSummaryMetadata::empty());
    let before = store.compaction_resource_cut().unwrap();
    take_candidate_decodes();
    take_metadata_decodes();
    assert!(
        store
            .commit_projection(commit_input(
                &store,
                "child",
                value,
                &CancellationSignal::new()
            ))
            .is_err()
    );
    assert_eq!(
        take_candidate_decodes(),
        1,
        "only CAS reads the head body; metadata extraction hydrates no retired body"
    );
    assert_eq!(
        take_metadata_decodes(),
        2,
        "third parent refused before metadata decode"
    );
    assert_eq!(store.compaction_resource_cut().unwrap(), before);
}

#[test]
fn inherited_activation_coordinates_are_validated_before_atomic_installation() {
    let source = store();
    let canonical = vec![message(1)];
    source.initialize(&canonical).unwrap();
    let checkpoint = source
        .commit_projection(commit_input(
            &source,
            "origin",
            candidate(&source, "origin", vec![summary("origin")]),
            &CancellationSignal::new(),
        ))
        .unwrap()
        .checkpoint;
    for defect in ["future", "before_base", "identity", "order"] {
        let destination =
            SqliteConversationStore::in_memory(source.conversation_id().clone()).unwrap();
        let mut inherited = checkpoint.clone();
        inherited.activation.destination_installation = Some(
            crate::runtime::identity::LineageInstallationId::new("installed"),
        );
        match defect {
            "future" => inherited.activation.ledger = LedgerFrontier(2),
            "before_base" => inherited.activation.ledger = LedgerFrontier(0),
            "identity" => inherited.activation.checkpoint_id = CheckpointId::new("foreign"),
            _ => inherited.activation.activation = ProjectionActivationId(2),
        }
        let seed = crate::durable::LineageSeed::history(canonical.clone())
            .with_projections(vec![inherited])
            .unwrap();
        assert!(
            destination.initialize_lineage(&seed).is_err(),
            "accepted inconsistent {defect}"
        );
        assert!(destination.projection_head().unwrap().is_none());
        assert!(
            destination.load_canonical().unwrap().is_empty(),
            "installation rolled back"
        );
    }
}

#[test]
fn historical_read_cut_with_inconsistent_activation_is_rejected_without_dependencies() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "first",
            candidate(&store, "first", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let mut second = candidate(&store, "second", vec![summary("second")]);
    second.read_cut.journal = JournalFrontier(0);
    let before = store.compaction_resource_cut().unwrap();
    assert!(
        store
            .commit_projection(commit_input(
                &store,
                "second",
                second,
                &CancellationSignal::new()
            ))
            .is_err()
    );
    assert_eq!(store.compaction_resource_cut().unwrap(), before);
}

#[test]
fn corrupted_eligibility_header_cannot_hide_a_required_historical_projection() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "first",
            candidate(&store, "first", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET ledger_frontier=100 WHERE activation=1",
            [],
        )
        .unwrap();
    assert!(
        store
            .read_lineage_cut(crate::conversation::SurfaceRevision::new(1))
            .is_err()
    );
    assert!(
        store
            .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(1))
            .is_err()
    );
}

#[test]
fn original_and_checkpoint_only_candidates_validate_their_cut_before_commit() {
    for checkpoint_only in [false, true] {
        let store = store();
        let original = message(1);
        store.initialize(std::slice::from_ref(&original)).unwrap();
        let first = store
            .commit_projection(commit_input(
                &store,
                "first",
                candidate(&store, "first", vec![summary("first")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        let item = if checkpoint_only {
            let mut item = summary("checkpoint-only");
            let CheckpointMessage::Derived(d) = &mut item.message else {
                unreachable!()
            };
            d.checkpoint_sources
                .push(crate::model::projected::CheckpointSourceRef {
                    conversation_id: store.conversation_id().clone(),
                    checkpoint_id: first.checkpoint.candidate.checkpoint_id.clone(),
                    content_digest: first.candidate_fingerprint,
                });
            item
        } else {
            CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &original,
                )),
                call_sources: Vec::new(),
                result_call: None,
            }
        };
        let mut value = candidate(&store, "second", vec![item]);
        value.read_cut.ledger = LedgerFrontier(100);
        value.base_frontier = LedgerFrontier(100);
        let before = store.compaction_resource_cut().unwrap();
        assert!(
            store
                .commit_projection(commit_input(
                    &store,
                    "second",
                    value,
                    &CancellationSignal::new()
                ))
                .is_err()
        );
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert_eq!(store.projection_head().unwrap().unwrap(), first.checkpoint);
    }
}

#[test]
fn historical_selection_rejects_missing_eligible_checkpoint_closure() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first = store
        .commit_projection(commit_input(
            &store,
            "first",
            candidate(&store, "first", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let mut item = summary("second");
    let CheckpointMessage::Derived(d) = &mut item.message else {
        unreachable!()
    };
    d.checkpoint_sources
        .push(crate::model::projected::CheckpointSourceRef {
            conversation_id: store.conversation_id().clone(),
            checkpoint_id: first.checkpoint.candidate.checkpoint_id,
            content_digest: first.candidate_fingerprint,
        });
    store
        .commit_projection(commit_input(
            &store,
            "second",
            candidate(&store, "second", vec![item]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM projection_activations WHERE activation=1", [])
        .unwrap();
    assert!(
        store
            .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
            .is_err()
    );
    assert!(
        store
            .read_lineage_cut(crate::conversation::SurfaceRevision::new(1))
            .is_err()
    );
}

#[test]
fn checkpoint_predecessor_must_match_the_head_visible_at_its_read_cut() {
    for defect in ["zero_activation", "wrong_predecessor", "wrong_journal"] {
        let store = store();
        store.initialize(&[message(1)]).unwrap();
        let first = store
            .commit_projection(commit_input(
                &store,
                "O1",
                candidate(&store, "P1", vec![summary("first")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        if defect != "zero_activation" {
            store
                .commit_projection(commit_input(
                    &store,
                    "O2",
                    candidate(&store, "P2", vec![summary("second")]),
                    &CancellationSignal::new(),
                ))
                .unwrap();
        }
        let mut value = candidate(&store, "invalid", vec![summary("dependency-free")]);
        value.read_cut.projection_activation = if defect == "zero_activation" {
            ProjectionActivationId(0)
        } else {
            ProjectionActivationId(1)
        };
        if defect == "wrong_predecessor" {
            value.read_cut.journal = first.checkpoint.activation.origin.completed_journal;
        }
        let before = store.compaction_resource_cut().unwrap();
        let head = store.projection_head().unwrap();
        let counts = || {
            let connection = store.lock().unwrap();
            [
                "projection_checkpoints",
                "projection_activations",
                "compaction_operations",
                "events",
            ]
            .map(|table| {
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                        r.get::<_, u64>(0)
                    })
                    .unwrap()
            })
        };
        let before_counts = counts();
        assert!(
            store
                .commit_projection(commit_input(
                    &store,
                    "invalid",
                    value,
                    &CancellationSignal::new()
                ))
                .is_err(),
            "accepted incoherent {defect}"
        );
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert_eq!(store.projection_head().unwrap(), head);
        assert_eq!(
            counts(),
            before_counts,
            "no partial checkpoint or completion"
        );
    }
}

#[test]
fn historical_head_validation_preserves_growth_same_ledger_and_old_receipts() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first_input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("first")]),
        &CancellationSignal::new(),
    );
    assert_eq!(first_input.candidate.expected_head, None);
    assert_eq!(
        first_input.candidate.read_cut.projection_activation,
        ProjectionActivationId(0)
    );
    store.append_canonical(&message(2)).unwrap();
    let first = store.commit_projection(first_input.clone()).unwrap();
    let second_input = commit_input(
        &store,
        "O2",
        candidate(&store, "P2", vec![summary("second")]),
        &CancellationSignal::new(),
    );
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: CompactionOperationId::new("unrelated"),
            },
        })
        .unwrap();
    store.append_canonical(&message(3)).unwrap();
    let second = store.commit_projection(second_input).unwrap();
    let third = store
        .commit_projection(commit_input(
            &store,
            "O3",
            candidate(&store, "P3", vec![summary("third")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    assert_eq!(
        second.checkpoint.activation.ledger,
        third.checkpoint.activation.ledger
    );
    assert_ne!(
        second.checkpoint.activation.activation,
        third.checkpoint.activation.activation
    );
    assert_fresh_receipt_replays_original(&first, store.commit_projection(first_input).unwrap());
    assert_eq!(store.projection_head().unwrap().unwrap(), third.checkpoint);
}

#[test]
fn source_free_historical_derived_is_rejected_without_any_durable_mutation() {
    for role in ["user", "assistant", "tool"] {
        let store = store();
        store.initialize(&[message(1)]).unwrap();
        let mut item = summary("invented historical content");
        let CheckpointMessage::Derived(body) = &mut item.message else {
            unreachable!()
        };
        if role == "assistant" {
            body.message = ProjectedMessage::Assistant { content: vec![] };
        } else if role == "tool" {
            body.message = ProjectedMessage::Tool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("absent"),
                tool_id: crate::runtime::identity::ToolId::new("tool-read"),
                result: crate::tools::types::ToolExecutionResult {
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: vec![],
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: vec![],
                    deliveries: vec![],
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            };
        }
        let counts = || {
            let connection = store.lock().unwrap();
            [
                "projection_checkpoints",
                "projection_activations",
                "compaction_operations",
                "events",
            ]
            .map(|table| {
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                        r.get::<_, u64>(0)
                    })
                    .unwrap()
            })
        };
        let before_counts = counts();
        let before = store.compaction_resource_cut().unwrap();
        let mut value = candidate(&store, "invalid", vec![item]);
        let CheckpointMessage::Derived(body) = &mut value.items[0].message else {
            unreachable!()
        };
        body.sources.clear();
        let error = store
            .commit_projection(commit_input(
                &store,
                "invalid",
                value,
                &CancellationSignal::new(),
            ))
            .unwrap_err();
        assert!(
            error.to_string().contains("immutable provenance"),
            "{error}"
        );
        assert_eq!(counts(), before_counts);
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert!(store.projection_head().unwrap().is_none());
        assert_eq!(store.load_canonical().unwrap(), [message(1)]);
    }
}

#[test]
fn parent_only_and_combined_immutable_provenance_commit_and_recover_receipts() {
    use crate::model::projected::CheckpointSourceRef;
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first_input = commit_input(
        &store,
        "parent",
        candidate(&store, "parent", vec![summary("first")]),
        &CancellationSignal::new(),
    );
    let first = store.commit_projection(first_input.clone()).unwrap();
    for (id, canonical) in [("parent-only", false), ("combined", true)] {
        let mut next = candidate(&store, id, vec![summary(id)]);
        let CheckpointMessage::Derived(body) = &mut next.items[0].message else {
            unreachable!()
        };
        if !canonical {
            body.sources.clear();
        }
        body.checkpoint_sources.push(CheckpointSourceRef {
            conversation_id: store.conversation_id().clone(),
            checkpoint_id: first.checkpoint.candidate.checkpoint_id.clone(),
            content_digest: first.candidate_fingerprint.clone(),
        });
        let input = commit_input(&store, id, next, &CancellationSignal::new());
        let receipt = store.commit_projection(input.clone()).unwrap();
        assert_fresh_receipt_replays_original(&receipt, store.commit_projection(input).unwrap());
        assert_eq!(store.projected_history().unwrap().messages.len(), 1);
    }
    let newest = store.projection_head().unwrap();
    assert_fresh_receipt_replays_original(&first, store.commit_projection(first_input).unwrap());
    assert_eq!(store.projection_head().unwrap(), newest);
    assert_eq!(store.load_canonical().unwrap(), [message(1)]);
}

#[test]
fn invalid_immutable_derived_provenance_rolls_back_every_durable_authority() {
    use crate::model::projected::CheckpointSourceRef;
    for kind in [
        "foreign-canonical",
        "missing-canonical",
        "corrupt-canonical",
        "future-canonical",
        "foreign-parent",
        "missing-parent",
        "corrupt-parent",
        "future-parent",
    ] {
        let store = store();
        store.initialize(&[message(1)]).unwrap();
        let mut value = candidate(&store, "child", vec![summary("child")]);
        let old_cut = value.read_cut.clone();
        let parent = store
            .commit_projection(commit_input(
                &store,
                "parent",
                candidate(&store, "parent", vec![summary("parent")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        if kind == "future-canonical" {
            store.append_canonical(&message(2)).unwrap();
        }
        // A future parent is valid now, but was not eligible at the candidate's read cut.
        if kind == "future-parent" {
            value.read_cut = old_cut;
            value.expected_head = Some(parent.checkpoint.candidate.checkpoint_id.clone());
        } else {
            value = candidate(&store, "child", vec![summary("child")]);
        }
        let CheckpointMessage::Derived(body) = &mut value.items[0].message else {
            unreachable!()
        };
        if kind.ends_with("parent") {
            body.sources.clear();
            let mut source = CheckpointSourceRef {
                conversation_id: store.conversation_id().clone(),
                checkpoint_id: parent.checkpoint.candidate.checkpoint_id.clone(),
                content_digest: parent.candidate_fingerprint.clone(),
            };
            match kind {
                "foreign-parent" => source.conversation_id = ConversationId::generate(),
                "missing-parent" => source.checkpoint_id = CheckpointId::new("missing"),
                "corrupt-parent" => source.content_digest = "corrupt".into(),
                _ => {}
            }
            body.checkpoint_sources.push(source);
        } else {
            match kind {
                "foreign-canonical" => body.sources[0].conversation_id = ConversationId::generate(),
                "missing-canonical" => body.sources[0].message_id = MessageId::new("missing"),
                "corrupt-canonical" => body.sources[0].content_digest = "corrupt".into(),
                "future-canonical" => {
                    body.sources[0] = CanonicalMessageRef::of(store.conversation_id(), &message(2));
                    value.base_frontier = LedgerFrontier(1);
                }
                _ => unreachable!(),
            }
        }
        let before = (
            projection_authority_counts(&store),
            store.projection_head().unwrap(),
            store.compaction_resource_cut().unwrap(),
            store.load_canonical().unwrap(),
        );
        assert!(
            store
                .commit_projection(commit_input(
                    &store,
                    "invalid",
                    value,
                    &CancellationSignal::new()
                ))
                .is_err(),
            "{kind}"
        );
        assert_eq!(
            (
                projection_authority_counts(&store),
                store.projection_head().unwrap(),
                store.compaction_resource_cut().unwrap(),
                store.load_canonical().unwrap()
            ),
            before,
            "{kind}"
        );
    }
}

fn projection_authority_counts(store: &SqliteConversationStore) -> [u64; 4] {
    let connection = store.lock().unwrap();
    [
        "projection_checkpoints",
        "projection_activations",
        "compaction_operations",
        "events",
    ]
    .map(|table| {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap()
    })
}

#[test]
#[allow(clippy::too_many_lines)] // One file-backed eligibility, pagination and inherited-closure scenario.
fn published_history_cut_excludes_future_bodies_before_hydration() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("history.sqlite"),
    )
    .unwrap();
    store.initialize(&[message(1)]).unwrap();
    let mut retained_cut = None;
    for n in 1..=6 {
        let value = candidate(&store, &format!("P{n}"), vec![summary("history")]);
        store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                value,
                &CancellationSignal::new(),
            ))
            .unwrap();
        if n == 2 {
            retained_cut =
                Some(crate::runtime_client::trace_observation::history_store_cut(&store).unwrap());
        }
    }
    let retained_cut = retained_cut.unwrap();
    let inherited = ["P1", "P2"]
        .into_iter()
        .map(|id| {
            let mut checkpoint = store.projection_checkpoint(&CheckpointId::new(id)).unwrap();
            checkpoint.activation.destination_installation = Some(
                crate::runtime::identity::LineageInstallationId::new("destination-installation"),
            );
            checkpoint.candidate.read_cut.journal = JournalFrontier(0);
            checkpoint.candidate.read_cut.transcript = crate::durable::TranscriptCursor::new(0);
            checkpoint.fingerprint = checkpoint.candidate.fingerprint();
            checkpoint
        })
        .collect();
    let destination_root = tempfile::tempdir().unwrap();
    let destination = SqliteConversationStore::open(
        store.conversation_id().clone(),
        &destination_root.path().join("inherited.sqlite"),
    )
    .unwrap();
    destination
        .initialize_lineage(
            &crate::durable::LineageSeed::history(vec![message(1)])
                .with_projections(inherited)
                .unwrap(),
        )
        .unwrap();
    // Origin Journal coordinates belong to the source. Installed inherited
    // projections precede destination execution and are visible at destination J0.
    assert_eq!(
        crate::runtime_client::projection_history::page(
            &destination,
            0,
            ProjectionActivationId(0),
            64
        )
        .unwrap()
        .records
        .len(),
        2
    );
    assert!(
        crate::runtime_client::projection_history::detail(
            &destination,
            &crate::runtime_client::trace_observation::history_store_cut(&destination).unwrap(),
            &CheckpointId::new("P2"),
            0,
            1
        )
        .is_ok()
    );
    let second = store
        .projection_checkpoint(&CheckpointId::new("P2"))
        .unwrap();
    let cut = second.activation.origin.completed_journal.0;
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_checkpoints SET candidate_json='not-json' WHERE checkpoint_id='P3'",
            [],
        )
        .unwrap();
    store.lock().unwrap().execute("UPDATE projection_checkpoints SET candidate_json=zeroblob(9000000) WHERE checkpoint_id='P4'", []).unwrap();
    take_candidate_decodes();
    let page =
        crate::runtime_client::projection_history::page(&store, cut, ProjectionActivationId(0), 64)
            .unwrap();
    assert_eq!(page.records.len(), 2);
    assert!(page.next_activation.is_none());
    assert_eq!(
        take_candidate_decodes(),
        2,
        "only eligible bodies are decoded once"
    );
    assert!(
        crate::runtime_client::projection_history::detail(
            &store,
            &retained_cut,
            &CheckpointId::new("P1"),
            0,
            1
        )
        .is_ok()
    );
    assert_eq!(
        take_candidate_decodes(),
        1,
        "detail only hydrates its eligible checkpoint"
    );
    let first =
        crate::runtime_client::projection_history::page(&store, cut, ProjectionActivationId(0), 1)
            .unwrap();
    assert_eq!(first.records.len(), 1);
    let next = crate::runtime_client::projection_history::page(
        &store,
        cut,
        first.next_activation.unwrap(),
        1,
    )
    .unwrap();
    assert_eq!(next.records.len(), 1);
    assert!(next.next_activation.is_none());
    take_candidate_decodes();
    assert!(
        crate::runtime_client::projection_history::detail(
            &store,
            &retained_cut,
            &CheckpointId::new("P3"),
            0,
            1
        )
        .is_err()
    );
    assert_eq!(
        take_candidate_decodes(),
        0,
        "ineligible detail is rejected before decoding"
    );
    store.lock().unwrap().execute("UPDATE projection_checkpoints SET candidate_json=zeroblob(9000000) WHERE checkpoint_id='P3'", []).unwrap();
    assert_eq!(
        crate::runtime_client::projection_history::page(&store, cut, ProjectionActivationId(0), 64)
            .unwrap()
            .records
            .len(),
        2
    );
    assert!(
        crate::runtime_client::projection_history::page(&store, cut, ProjectionActivationId(2), 64)
            .unwrap()
            .records
            .is_empty()
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_checkpoints SET candidate_json='not-json' WHERE checkpoint_id='P1'",
            [],
        )
        .unwrap();
    assert!(
        crate::runtime_client::projection_history::page(&store, cut, ProjectionActivationId(0), 64)
            .is_err()
    );
    drop(store);
    root.close().unwrap();
    assert_eq!(
        crate::runtime_client::projection_history::page(
            &destination,
            0,
            ProjectionActivationId(0),
            64
        )
        .unwrap()
        .records
        .len(),
        2
    );
}

#[test]
fn n1_future_activation_payloads_cannot_poison_a_published_history_cut() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("activation-history.sqlite"),
    )
    .unwrap();
    store.initialize(&[message(1)]).unwrap();
    let mut cut = 0;
    for n in 1..=4 {
        let receipt = store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                candidate(&store, &format!("P{n}"), vec![summary("history")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        if n == 2 {
            cut = receipt.checkpoint.activation.origin.completed_journal.0;
        }
    }
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET activation_json='not-json' WHERE activation=3",
            [],
        )
        .unwrap();
    store.lock().unwrap().execute(
        "UPDATE projection_activations SET activation_json=zeroblob(2097152) WHERE activation=4", [],
    ).unwrap();
    store.lock().unwrap().execute("UPDATE projection_activations SET activation_digest='corrupt-payload-digest' WHERE activation=3",[]).unwrap();
    take_header_work();
    take_candidate_decodes();
    let first = store
        .projection_timeline_at_journal_cut(cut, ProjectionActivationId(0), 1)
        .unwrap();
    assert_eq!(first.records[0].candidate.checkpoint_id.as_str(), "P1");
    assert_eq!(first.next_activation, Some(ProjectionActivationId(1)));
    let second = store
        .projection_timeline_at_journal_cut(cut, ProjectionActivationId(1), 1)
        .unwrap();
    assert_eq!(second.records[0].candidate.checkpoint_id.as_str(), "P2");
    assert_eq!(second.next_activation, None);
    assert_eq!(take_candidate_decodes(), 2);
    assert_eq!(take_header_work().1, 2);
    assert_eq!(
        store
            .projection_at_journal_cut(cut)
            .unwrap()
            .unwrap()
            .candidate
            .checkpoint_id
            .as_str(),
        "P2"
    );
    take_header_work();
    take_candidate_decodes();
    assert!(
        store
            .projection_checkpoint_at_journal_cut(cut, &CheckpointId::new("P3"))
            .is_err()
    );
    assert!(store.projection_head().is_err());
    assert_eq!(take_candidate_decodes(), 0);
    let mut inconsistent = store.compaction_resource_cut().unwrap();
    inconsistent.journal = JournalFrontier(cut);
    take_header_work();
    assert!(
        store
            .compaction_checkpoint_at(
                &inconsistent,
                &CheckpointId::new("P3"),
                ResourceLimits::default()
            )
            .is_err()
    );
    assert_eq!(take_header_work().1, 0);
    assert_eq!(take_candidate_decodes(), 0);
    assert!(
        store
            .projection_checkpoint(&CheckpointId::new("P3"))
            .is_err()
    );
    assert!(
        store
            .projection_timeline_at_journal_cut(
                store.presentation_frontier().unwrap(),
                ProjectionActivationId(0),
                4
            )
            .is_err()
    );
}

fn n1_file_history(count: u64) -> (tempfile::TempDir, SqliteConversationStore, u64) {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("history.sqlite"),
    )
    .unwrap();
    store.initialize(&[message(1)]).unwrap();
    let mut cut = 0;
    for n in 1..=count {
        let receipt = store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                candidate(&store, &format!("P{n}"), vec![summary("history")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        if n == 2 {
            cut = receipt.checkpoint.activation.origin.completed_journal.0;
        }
    }
    (root, store, cut)
}

#[test]
fn n1_header_work_is_bounded_by_the_historical_page_not_future_timeline() {
    let (_root, store, cut) = n1_file_history(4);
    let measured = || {
        take_header_work();
        take_candidate_decodes();
        let page = store
            .projection_timeline_at_journal_cut(cut, ProjectionActivationId(0), 1)
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.next_activation, Some(ProjectionActivationId(1)));
        let work = take_header_work();
        assert_eq!(work.1, 1, "only the requested activation JSON is decoded");
        assert_eq!(take_candidate_decodes(), 1);
        work.0
    };
    let short = measured();
    for n in 5..=132 {
        store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                candidate(&store, &format!("P{n}"), vec![summary("history")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
    }
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET activation_json='not-json' WHERE activation>2",
            [],
        )
        .unwrap();
    assert_eq!(
        measured(),
        short,
        "future growth adds no header validations"
    );
    assert!(
        short <= 8,
        "one page has only constant frontier/header work"
    );
    let plan:String = store.lock().unwrap().query_row(
        "EXPLAIN QUERY PLAN SELECT event_json FROM events INDEXED BY events_kind_idx WHERE json_extract(event_json,'$.event.type')='compaction_completed' AND sequence<=2 ORDER BY sequence DESC LIMIT 1",[],|r|r.get(3),
    ).unwrap();
    assert!(plan.contains("SEARCH") && plan.contains("events_kind_idx"));
}

#[test]
fn n1_eligible_payload_and_selector_corruption_fail_without_hiding_predecessors() {
    for defect in [
        "payload",
        "payload_digest",
        "ledger_exclusion",
        "journal_exclusion",
        "journal_inclusion",
        "checkpoint",
        "missing_predecessor",
        "missing_completion",
        "completion_scope",
    ] {
        let (_root, store, cut) = n1_file_history(4);
        let sql = match defect {
            "payload" => {
                "UPDATE projection_activations SET activation_json='not-json' WHERE activation=1"
            }
            "payload_digest" => {
                "UPDATE projection_activations SET activation_digest=printf('%064d',0) WHERE activation=1"
            }
            "ledger_exclusion" => {
                "UPDATE projection_activations SET ledger_frontier=100 WHERE activation=1"
            }
            "journal_exclusion" => {
                "UPDATE projection_activations SET visible_journal=100 WHERE activation=2"
            }
            "journal_inclusion" => {
                "UPDATE projection_activations SET visible_journal=1 WHERE activation=3"
            }
            "checkpoint" => {
                "UPDATE projection_activations SET checkpoint_id='P4' WHERE activation=1"
            }
            "missing_predecessor" => "DELETE FROM projection_activations WHERE activation=1",
            "missing_completion" => "DELETE FROM events WHERE sequence=2",
            "completion_scope" => {
                "UPDATE events SET event_json=json_set(event_json,'$.conversation_id','foreign') WHERE sequence=2"
            }
            _ => unreachable!(),
        };
        // Duplicate checkpoint identities are rejected at the normalized schema boundary.
        let changed = store.lock().unwrap().execute(sql, []);
        if defect == "checkpoint" {
            assert!(changed.is_err());
            continue;
        }
        changed.unwrap();
        assert!(
            store
                .projection_timeline_at_journal_cut(cut, ProjectionActivationId(0), 2)
                .is_err(),
            "{defect}"
        );
        if defect == "completion_scope" {
            assert!(
                store
                    .projection_timeline_at_journal_cut(cut, ProjectionActivationId(0), 1)
                    .is_err()
            );
        }
        if defect == "ledger_exclusion" {
            assert!(
                store
                    .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
                    .is_err()
            );
        }
    }
    // Even a recomputed selector checksum cannot override a committed Journal coordinate.
    let (_root, store, cut) = n1_file_history(4);
    let connection = store.lock().unwrap();
    let mut header = eligibility_header(&connection, 2).unwrap();
    header.journal = 100;
    connection.execute("UPDATE projection_activations SET visible_journal=100,header_digest=?1 WHERE activation=2",[header_digest(&header).unwrap()]).unwrap();
    drop(connection);
    assert!(
        store
            .projection_timeline_at_journal_cut(cut, ProjectionActivationId(0), 1)
            .is_err()
    );
}

#[test]
fn n1_pagination_is_contiguous_exhausted_and_frozen_across_append() {
    let (root, store, cut) = n1_file_history(2);
    let first = store
        .projection_timeline_at_journal_cut(cut, ProjectionActivationId(0), 1)
        .unwrap();
    assert_eq!(
        first.records[0].activation.activation,
        ProjectionActivationId(1)
    );
    let writer = SqliteConversationStore::open(
        store.conversation_id().clone(),
        &root.path().join("history.sqlite"),
    )
    .unwrap();
    writer
        .commit_projection(commit_input(
            &writer,
            "O3",
            candidate(&writer, "P3", vec![summary("later")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let second = store
        .projection_timeline_at_journal_cut(cut, first.next_activation.unwrap(), 1)
        .unwrap();
    assert_eq!(
        second.records[0].activation.activation,
        ProjectionActivationId(2)
    );
    assert_eq!(second.next_activation, None);
    assert_eq!(
        store
            .projection_timeline_at_journal_cut(cut, ProjectionActivationId(1), 1)
            .unwrap()
            .records,
        second.records
    );
    let empty = store
        .projection_timeline_at_journal_cut(cut, ProjectionActivationId(2), 1)
        .unwrap();
    assert!(empty.records.is_empty());
    assert_eq!(empty.next_activation, None);
    let current = store
        .projection_timeline_at_journal_cut(
            store.presentation_frontier().unwrap(),
            ProjectionActivationId(1),
            2,
        )
        .unwrap();
    assert_eq!(
        current
            .records
            .iter()
            .map(|p| p.activation.activation.0)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(current.next_activation, None);
    assert!(
        current
            .records
            .iter()
            .all(|p| p.activation.ledger == LedgerFrontier(1))
    );
}

#[test]
fn n1_inherited_j0_boundary_cannot_be_hidden_by_rewritten_selector_metadata() {
    let (source_root, source, _) = n1_file_history(2);
    let selected = source
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
        .unwrap();
    let mut inherited = selected.projections;
    for checkpoint in &mut inherited {
        checkpoint.activation.destination_installation = Some(
            crate::runtime::identity::LineageInstallationId::new("installed"),
        );
        checkpoint.candidate.read_cut.journal = JournalFrontier(0);
        checkpoint.candidate.read_cut.transcript = crate::durable::TranscriptCursor::new(0);
        checkpoint.fingerprint = checkpoint.candidate.fingerprint();
    }
    let destination_root = tempfile::tempdir().unwrap();
    let destination = SqliteConversationStore::open(
        source.conversation_id().clone(),
        &destination_root.path().join("destination.sqlite"),
    )
    .unwrap();
    destination
        .initialize_lineage(
            &crate::durable::LineageSeed::history(vec![message(1)])
                .with_projections(inherited)
                .unwrap(),
        )
        .unwrap();
    drop(source);
    source_root.close().unwrap();
    let page = destination
        .projection_timeline_at_journal_cut(0, ProjectionActivationId(0), 2)
        .unwrap();
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.next_activation, None);
    assert!(
        page.records
            .iter()
            .all(|p| p.activation.origin.completed_journal.0 > 0)
    );
    let connection = destination.lock().unwrap();
    let mut header = eligibility_header(&connection, 2).unwrap();
    header.journal = 100;
    header.installation = None;
    connection.execute("UPDATE projection_activations SET visible_journal=100,installation_id=NULL,header_digest=?1 WHERE activation=2",[header_digest(&header).unwrap()]).unwrap();
    drop(connection);
    assert!(
        destination
            .projection_timeline_at_journal_cut(0, ProjectionActivationId(0), 1)
            .is_err()
    );
    assert!(destination.projection_at_journal_cut(0).is_err());
}

#[test]
fn n1_original_detail_does_not_validate_an_unrelated_future_activation_payload() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("original-detail.sqlite"),
    )
    .unwrap();
    store.initialize(&[message(1)]).unwrap();
    let mut cut = None;
    for n in 1..=4 {
        let items = if n == 1 {
            vec![CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &message(1),
                )),
                call_sources: Vec::new(),
                result_call: None,
            }]
        } else {
            vec![summary("history")]
        };
        store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                candidate(&store, &format!("P{n}"), items),
                &CancellationSignal::new(),
            ))
            .unwrap();
        if n == 2 {
            cut =
                Some(crate::runtime_client::trace_observation::history_store_cut(&store).unwrap());
        }
    }
    let cut = cut.unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET activation_json='not-json' WHERE activation=4",
            [],
        )
        .unwrap();
    take_header_work();
    take_candidate_decodes();
    let detail = crate::runtime_client::projection_history::detail(
        &store,
        &cut,
        &CheckpointId::new("P1"),
        0,
        1,
    )
    .unwrap();
    assert_eq!(detail.items.len(), 1);
    assert_eq!(detail.next_item, None);
    assert_eq!(
        take_header_work().1,
        1,
        "only P1 activation content is decoded"
    );
    assert_eq!(take_candidate_decodes(), 1);
}

#[test]
fn current_head_rejects_stale_missing_and_nonexistent_pointers_without_changing_history() {
    for mutation in [
        "UPDATE projection_head SET activation=1",
        "UPDATE projection_head SET activation=99",
        "DELETE FROM projection_head",
    ] {
        let (_root, store, cut) = n1_file_history(2);
        assert_eq!(
            store
                .projection_head()
                .unwrap()
                .unwrap()
                .activation
                .activation
                .0,
            2
        );
        store
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys=OFF; {mutation}; PRAGMA foreign_keys=ON;"
            ))
            .unwrap();
        let historical = store
            .projection_checkpoint_at_journal_cut(cut, &CheckpointId::new("P1"))
            .unwrap();
        assert_eq!(historical.activation.activation.0, 1);
        assert!(store.projection_head().is_err(), "{mutation}");
        assert!(store.projected_history().is_err(), "{mutation}");
    }
}

#[test]
fn timeline_complete_record_budget_returns_a_partial_page_before_decoding_the_next_record() {
    let (_root, store, _) = n1_file_history(2);
    let first = store
        .projection_checkpoint(&CheckpointId::new("P1"))
        .unwrap();
    let second = store
        .projection_checkpoint(&CheckpointId::new("P2"))
        .unwrap();
    let first_bytes = serde_json::to_vec(&first).unwrap().len();
    let second_bytes = serde_json::to_vec(&second).unwrap().len();
    let connection = store.lock().unwrap();
    let raw_second: usize = connection.query_row(
        "SELECT octet_length(candidate_json)+octet_length(activation_json)+octet_length(occupancy_json) FROM projection_checkpoints p JOIN projection_activations a USING(checkpoint_id) WHERE a.activation=2", [], |r| r.get(0)).unwrap();
    assert!(raw_second < second_bytes);
    take_candidate_decodes();
    let page = timeline_page(
        &connection,
        ProjectionActivationId(0),
        2,
        None,
        first_bytes + second_bytes - 1,
    )
    .unwrap();
    assert_eq!(page, vec![first.clone()]);
    assert_eq!(
        take_candidate_decodes(),
        1,
        "the deferred record is not decoded"
    );
    assert_eq!(
        timeline_page(
            &connection,
            ProjectionActivationId(1),
            2,
            None,
            second_bytes
        )
        .unwrap(),
        vec![second]
    );
    assert_eq!(
        timeline_page(&connection, ProjectionActivationId(0), 2, None, first_bytes).unwrap(),
        vec![first]
    );
    assert!(
        timeline_page(
            &connection,
            ProjectionActivationId(0),
            2,
            None,
            first_bytes - 1
        )
        .is_err()
    );
    assert_eq!(
        timeline_page(
            &connection,
            ProjectionActivationId(0),
            2,
            None,
            first_bytes + second_bytes
        )
        .unwrap()
        .len(),
        2
    );
    assert!(
        timeline_page(&connection, ProjectionActivationId(2), 2, None, 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn checkpoint_fingerprints_are_admitted_before_owned_materialization_and_body_decode() {
    for corrupt in [
        "x".repeat(2 * 1024 * 1024),
        "a".repeat(63),
        "z".repeat(64),
        "A".repeat(64),
        format!("{}\0{}", "a".repeat(32), "a".repeat(31)),
        "0".repeat(64),
    ] {
        let (_root, store, _) = n1_file_history(2);
        store.lock().unwrap().execute("UPDATE projection_checkpoints SET candidate_fingerprint=?1 WHERE checkpoint_id='P2'", [&corrupt]).unwrap();
        FINGERPRINT_MATERIALIZATIONS.with(|n| n.set(0));
        take_candidate_decodes();
        assert!(store.projection_head().is_err());
        let materializations = FINGERPRINT_MATERIALIZATIONS.with(|n| n.replace(0));
        if corrupt.len() != 64
            || !corrupt
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            assert_eq!(
                materializations, 0,
                "invalid digest encoding must not become an owned String"
            );
            assert_eq!(take_candidate_decodes(), 0);
        } else {
            assert_eq!(materializations, 1);
            assert_eq!(
                take_candidate_decodes(),
                1,
                "well-formed but wrong digests need content verification"
            );
        }
    }
    let (_root, store, _) = n1_file_history(2);
    assert!(store.projection_head().unwrap().is_some());
}

#[test]
fn inherited_publication_then_local_commit_has_one_authoritative_current_head() {
    let (source_root, source, _) = n1_file_history(2);
    let mut inherited = source
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
        .unwrap()
        .projections;
    for checkpoint in &mut inherited {
        checkpoint.activation.destination_installation = Some(
            crate::runtime::identity::LineageInstallationId::new("installed"),
        );
        checkpoint.candidate.read_cut.journal = JournalFrontier(0);
        checkpoint.candidate.read_cut.transcript = crate::durable::TranscriptCursor::new(0);
        checkpoint.fingerprint = checkpoint.candidate.fingerprint();
    }
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        source.conversation_id().clone(),
        &root.path().join("destination.sqlite"),
    )
    .unwrap();
    store
        .initialize_lineage(
            &crate::durable::LineageSeed::history(vec![message(1)])
                .with_projections(inherited)
                .unwrap(),
        )
        .unwrap();
    drop(source);
    source_root.close().unwrap();
    assert_eq!(
        store
            .projection_head()
            .unwrap()
            .unwrap()
            .activation
            .activation
            .0,
        2
    );
    let local = store
        .commit_projection(commit_input(
            &store,
            "local",
            candidate(&store, "P3", vec![summary("local")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    assert_eq!(local.checkpoint.activation.activation.0, 3);
    assert_eq!(store.projection_head().unwrap().unwrap(), local.checkpoint);
    assert!(
        local
            .checkpoint
            .activation
            .destination_installation
            .is_none()
    );
    assert_eq!(
        store
            .projection_at_journal_cut(0)
            .unwrap()
            .unwrap()
            .activation
            .activation
            .0,
        2
    );
    store
        .lock()
        .unwrap()
        .execute("UPDATE projection_head SET activation=2", [])
        .unwrap();
    assert!(store.projection_head().is_err());
    assert_eq!(
        store
            .projection_at_journal_cut(0)
            .unwrap()
            .unwrap()
            .activation
            .activation
            .0,
        2
    );
}

#[test]
fn bounded_record_pages_preserve_same_ledger_order_and_frozen_cut_across_append() {
    let (root, store, cut) = n1_file_history(2);
    let records = store
        .projection_timeline(ProjectionActivationId(0), 2)
        .unwrap();
    let budget = records
        .iter()
        .map(|p| serde_json::to_vec(p).unwrap().len())
        .max()
        .unwrap();
    let selection = Some((
        LedgerFrontier(1),
        JournalFrontier(cut),
        ProjectionActivationId(2),
    ));
    let first = timeline_page(
        &store.lock().unwrap(),
        ProjectionActivationId(0),
        64,
        selection,
        budget,
    )
    .unwrap();
    assert_eq!(first, records[..1]);
    let writer = SqliteConversationStore::open(
        store.conversation_id().clone(),
        &root.path().join("history.sqlite"),
    )
    .unwrap();
    writer
        .commit_projection(commit_input(
            &writer,
            "O3",
            candidate(&writer, "P3", vec![summary("later")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let second = timeline_page(
        &store.lock().unwrap(),
        first.last().unwrap().activation.activation,
        64,
        selection,
        budget,
    )
    .unwrap();
    assert_eq!(second, records[1..]);
    assert_eq!(
        timeline_page(
            &store.lock().unwrap(),
            ProjectionActivationId(1),
            64,
            selection,
            budget
        )
        .unwrap(),
        second
    );
    assert!(
        timeline_page(
            &store.lock().unwrap(),
            ProjectionActivationId(2),
            64,
            selection,
            budget
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        first
            .into_iter()
            .chain(second)
            .map(|p| p.activation.activation.0)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        store
            .projection_head()
            .unwrap()
            .unwrap()
            .activation
            .activation
            .0,
        3
    );
}

#[test]
fn receipt_and_parent_provenance_fingerprints_share_preallocation_admission() {
    let (_root, store, _) = n1_file_history(2);
    let parent = store
        .projection_checkpoint(&CheckpointId::new("P1"))
        .unwrap();
    let second = store
        .projection_checkpoint(&CheckpointId::new("P2"))
        .unwrap();
    let mut child = candidate(&store, "P3", vec![summary("from parent")]);
    let CheckpointMessage::Derived(body) = &mut child.items[0].message else {
        unreachable!()
    };
    body.sources.clear();
    body.checkpoint_sources
        .push(crate::model::projected::CheckpointSourceRef {
            conversation_id: store.conversation_id().clone(),
            checkpoint_id: parent.candidate.checkpoint_id,
            content_digest: parent.fingerprint,
        });
    validate_current_dependencies(&store.lock().unwrap(), store.conversation_id(), &child).unwrap();
    for value in ["f".repeat(2 * 1024 * 1024), "a".repeat(63), "z".repeat(64)] {
        store.lock().unwrap().execute("UPDATE projection_checkpoints SET candidate_fingerprint=?1 WHERE checkpoint_id='P1'", [&value]).unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE compaction_operations SET candidate_fingerprint=?1 WHERE operation_id='O2'",
                [&value],
            )
            .unwrap();
        FINGERPRINT_MATERIALIZATIONS.with(|n| n.set(0));
        assert!(
            validate_current_dependencies(&store.lock().unwrap(), store.conversation_id(), &child)
                .is_err()
        );
        assert!(
            store
                .projection_receipt(&CompactionOperationId::new("O2"), &second.fingerprint)
                .is_err()
        );
        assert_eq!(FINGERPRINT_MATERIALIZATIONS.with(|n| n.replace(0)), 0);
    }
    store.lock().unwrap().execute("UPDATE compaction_operations SET candidate_fingerprint=?1,receipt_digest=zeroblob(2097152) WHERE operation_id='O2'", [&second.fingerprint]).unwrap();
    assert!(
        store
            .projection_receipt(&CompactionOperationId::new("O2"), &second.fingerprint)
            .is_err()
    );
}

#[test]
fn current_head_validation_is_bounded_by_publication_not_timeline_length() {
    let (_root, store, _) = n1_file_history(66);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET activation_json='not-json' WHERE activation=1",
            [],
        )
        .unwrap();
    take_header_work();
    take_candidate_decodes();
    assert_eq!(
        store
            .projection_head()
            .unwrap()
            .unwrap()
            .activation
            .activation
            .0,
        66
    );
    let (headers, activations) = take_header_work();
    assert!(headers <= 4);
    assert_eq!(activations, 1);
    assert_eq!(take_candidate_decodes(), 1);
    store
        .lock()
        .unwrap()
        .execute("UPDATE projection_head SET activation=1", [])
        .unwrap();
    assert!(store.projection_head().is_err());
    assert_eq!(take_candidate_decodes(), 0);
    assert_eq!(
        take_header_work().1,
        0,
        "the pointer mismatch fails before either candidate body"
    );
}

#[test]
fn summary_generation_dependency_accepts_success_after_resource_capture_and_preserves_old_audit_cut()
 {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("summary-open-at-cut.sqlite"),
    )
    .unwrap();
    store.initialize(&[message(1)]).unwrap();
    let mut checkpoint = candidate(&store, "summary-open-at-cut", vec![summary("history")]);
    let operation = CompactionOperationId::new("operation:summary-open-at-cut");
    let (call_id, request_id) = start_summary_call(
        &store,
        AuxiliaryBusinessSource::Compaction {
            operation_id: operation.clone(),
        },
        true,
    );
    let request_id = request_id.unwrap();
    let historical_through = store.auxiliary_audit_cut().unwrap().journal;
    settle_summary_call(&store, call_id.clone(), request_id.clone());
    let through = store.auxiliary_audit_cut().unwrap().journal;
    checkpoint.summary_generation = Some(summary_reference(&store, call_id.clone(), through));
    let mut without_dependency = checkpoint.clone();
    without_dependency.summary_generation = None;
    assert_ne!(
        checkpoint.fingerprint(),
        without_dependency.fingerprint(),
        "the named generation participates in checkpoint identity"
    );
    assert!(
        checkpoint.read_cut.journal.0 <= through,
        "the checkpoint resource cut predates its auxiliary generation"
    );
    let receipt = store
        .commit_projection(ProjectionCommitInput {
            grant: CancellationSignal::new()
                .authorize_compaction_commit(operation.clone())
                .unwrap(),
            operation_id: operation,
            candidate: checkpoint.clone(),
            owner: store.projection_owner(None).unwrap(),
            tokens_before: TokenMeasurement {
                input_tokens: 100,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 10,
            occupancy: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
        })
        .unwrap();
    assert_eq!(
        receipt.checkpoint.candidate.summary_generation,
        checkpoint.summary_generation
    );
    let historical = store
        .load_auxiliary_call(
            &call_id,
            crate::model::auxiliary::AuxiliaryAuditCut {
                journal: historical_through,
            },
        )
        .unwrap()
        .unwrap();
    assert!(historical.settled_sequence.is_none());
    assert!(historical.outcome.is_none());
    assert_eq!(
        store
            .load_auxiliary_request(
                &request_id,
                crate::model::auxiliary::AuxiliaryAuditCut {
                    journal: historical_through,
                },
            )
            .unwrap()
            .unwrap()
            .terminal_sequence,
        None
    );
    let lineage = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(lineage.auxiliary_audits.len(), 1);
    assert_eq!(lineage.auxiliary_audits[0].records.len(), 4);
    assert_eq!(
        lineage.auxiliary_audits[0].completeness,
        crate::durable::reading::AuxiliaryAuditCompleteness::CompleteAtCut
    );
}

#[test]
fn local_checkpoint_rejects_unconfirmed_or_unsuccessful_summary_without_partial_commit() {
    for status in [
        "open",
        "failed",
        "cancelled",
        "completed_after_cut",
        "physical_terminal_before_call_settlement",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::open(
            ConversationId::generate(),
            &root.path().join("summary-admission.sqlite"),
        )
        .unwrap();
        store.initialize(&[message(1)]).unwrap();
        store
            .commit_projection(commit_input(
                &store,
                "preceding-operation",
                candidate(&store, "preceding", vec![summary("retained head")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        let mut checkpoint = candidate(&store, "candidate", vec![summary("generated")]);
        let operation_id = CompactionOperationId::new("summary-operation");
        let (call_id, request_id) = start_summary_call(
            &store,
            AuxiliaryBusinessSource::Compaction {
                operation_id: operation_id.clone(),
            },
            true,
        );
        let started_cut = store.auxiliary_audit_cut().unwrap();
        let outcome = match status {
            "open" => None,
            "failed" => Some(AuxiliaryCallOutcome::Failed),
            "cancelled" => Some(AuxiliaryCallOutcome::Cancelled),
            _ => Some(AuxiliaryCallOutcome::Completed),
        };
        let settled = outcome.map(|outcome| {
            settle_summary_call_with_outcome(&store, call_id.clone(), request_id.unwrap(), outcome)
        });
        let current_cut = store.auxiliary_audit_cut().unwrap();
        let through = match status {
            "completed_after_cut" => started_cut.journal,
            "physical_terminal_before_call_settlement" => {
                settled
                    .as_ref()
                    .unwrap()
                    .request_terminal
                    .as_ref()
                    .unwrap()
                    .sequence
            }
            _ => current_cut.journal,
        };
        checkpoint.summary_generation = Some(summary_reference(&store, call_id.clone(), through));
        let before = (
            projection_authority_counts(&store),
            store.projection_head().unwrap(),
            store.compaction_resource_cut().unwrap(),
            store.load_auxiliary_call(&call_id, current_cut).unwrap(),
            store.load_canonical().unwrap(),
        );
        let error = store
            .commit_projection(commit_input(
                &store,
                operation_id.as_str(),
                checkpoint,
                &CancellationSignal::new(),
            ))
            .unwrap_err();
        assert!(
            error.to_string().contains("successfully completed Call"),
            "{status}: {error}"
        );
        assert_eq!(
            (
                projection_authority_counts(&store),
                store.projection_head().unwrap(),
                store.compaction_resource_cut().unwrap(),
                store.load_auxiliary_call(&call_id, current_cut).unwrap(),
                store.load_canonical().unwrap(),
            ),
            before,
            "{status}: rejected Summary dependency changed durable authority"
        );
    }
}

#[test]
fn inherited_summary_audit_enforces_native_record_and_metadata_bounds() {
    for model_bytes in [513, 20_000] {
        let mut snapshot = complete_summary_snapshot();
        update_summary_requests(&mut snapshot, |request| {
            request.model = "x".repeat(model_bytes);
        });
        assert!(super::auxiliary_lineage::validate_snapshot_facts(&snapshot).is_err());
    }
    let mut snapshot = complete_summary_snapshot();
    update_summary_calls(&mut snapshot, |call| {
        call.selected_message_ids = vec![MessageId::new("x".repeat(9_000))];
    });
    assert!(matches!(
        super::auxiliary_lineage::validate_snapshot_facts(&snapshot),
        Err(ConversationStoreError::ResourceBoundExceeded(_))
    ));
}

#[test]
fn inherited_summary_audit_requires_final_evidence_and_compatible_outcome() {
    let snapshot = complete_summary_snapshot();
    assert!(super::auxiliary_lineage::validate_snapshot_facts(&snapshot).is_ok());

    let mut missing = snapshot.clone();
    update_summary_requests(&mut missing, |request| {
        request.request_evidence = None;
    });
    assert!(
        super::auxiliary_lineage::validate_snapshot_facts(&missing)
            .unwrap_err()
            .to_string()
            .contains("request evidence")
    );

    for observed in [
        None,
        Some(AuxiliaryObservedProviderTerminal::Failed {
            kind: ModelErrorKind::Authentication,
        }),
    ] {
        let mut invalid = snapshot.clone();
        update_summary_requests(&mut invalid, |request| {
            request
                .request_evidence
                .as_mut()
                .expect("completed fixture evidence")
                .observed_provider_terminal = observed.clone();
        });
        assert!(
            super::auxiliary_lineage::validate_snapshot_facts(&invalid)
                .unwrap_err()
                .to_string()
                .contains("observed completion")
        );
    }

    let mut failed_call = snapshot.clone();
    let AuxiliaryAuditFact::CallSettled(call) = &mut failed_call.records.last_mut().unwrap().fact
    else {
        panic!("completed fixture Call settlement")
    };
    call.outcome = Some(AuxiliaryCallOutcome::Failed);
    assert!(
        super::auxiliary_lineage::validate_snapshot_facts(&failed_call)
            .unwrap_err()
            .to_string()
            .contains("outcome disagrees")
    );

    let mut changed_model = snapshot;
    let AuxiliaryAuditFact::RequestSettled(request) = &mut changed_model.records[2].fact else {
        panic!("completed fixture request terminal")
    };
    request.model.push_str("-changed");
    assert!(
        super::auxiliary_lineage::validate_snapshot_facts(&changed_model)
            .unwrap_err()
            .to_string()
            .contains("start and terminal disagree")
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Exercise each invalid prerequisite against the same native transaction contract.
fn summary_generation_dependency_rejects_missing_wrong_and_future_prerequisites() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("summary-invalid.sqlite"),
    )
    .unwrap();
    store.initialize(&[message(1)]).unwrap();

    let missing = candidate(&store, "summary-missing", vec![summary("missing")]);
    let operation = CompactionOperationId::new("operation:summary-missing");
    let (missing_call, _) = start_summary_call(
        &store,
        AuxiliaryBusinessSource::Compaction {
            operation_id: operation.clone(),
        },
        false,
    );
    let mut missing = missing;
    missing.summary_generation = Some(summary_reference(
        &store,
        missing_call,
        store.auxiliary_audit_cut().unwrap().journal,
    ));
    assert!(
        store
            .commit_projection(ProjectionCommitInput {
                grant: CancellationSignal::new()
                    .authorize_compaction_commit(operation.clone())
                    .unwrap(),
                operation_id: operation,
                candidate: missing,
                owner: store.projection_owner(None).unwrap(),
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 10,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap_err()
            .to_string()
            .contains("physical request")
    );

    let wrong_operation = CompactionOperationId::new("operation:summary-wrong-operation");
    let (call_id, request_id) = start_summary_call(
        &store,
        AuxiliaryBusinessSource::Compaction {
            operation_id: CompactionOperationId::new("operation:different"),
        },
        true,
    );
    settle_summary_call(&store, call_id.clone(), request_id.unwrap());
    let mut wrong = candidate(
        &store,
        "summary-wrong-operation",
        vec![summary("wrong operation")],
    );
    wrong.summary_generation = Some(summary_reference(
        &store,
        call_id.clone(),
        store.auxiliary_audit_cut().unwrap().journal,
    ));
    assert!(
        store
            .commit_projection(ProjectionCommitInput {
                grant: CancellationSignal::new()
                    .authorize_compaction_commit(wrong_operation.clone())
                    .unwrap(),
                operation_id: wrong_operation,
                candidate: wrong,
                owner: store.projection_owner(None).unwrap(),
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 10,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap_err()
            .to_string()
            .contains("business source")
    );

    let future_operation = CompactionOperationId::new("operation:summary-future");
    let mut future = candidate(&store, "summary-future", vec![summary("future")]);
    future.summary_generation = Some(summary_reference(
        &store,
        call_id,
        store.auxiliary_audit_cut().unwrap().journal + 1,
    ));
    assert!(
        store
            .commit_projection(ProjectionCommitInput {
                grant: CancellationSignal::new()
                    .authorize_compaction_commit(future_operation.clone())
                    .unwrap(),
                operation_id: future_operation,
                candidate: future,
                owner: store.projection_owner(None).unwrap(),
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 10,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap_err()
            .to_string()
            .contains("outside current native authority")
    );

    let wrong_purpose_operation = CompactionOperationId::new("operation:summary-wrong-purpose");
    let (wrong_purpose_call, wrong_purpose_request) = start_auxiliary_call(
        &store,
        AuxiliaryAttribution::new("btw").unwrap(),
        AuxiliaryBusinessSource::Compaction {
            operation_id: wrong_purpose_operation.clone(),
        },
        true,
    );
    settle_summary_call(
        &store,
        wrong_purpose_call.clone(),
        wrong_purpose_request.unwrap(),
    );
    let mut wrong_purpose = candidate(
        &store,
        "summary-wrong-purpose",
        vec![summary("wrong purpose")],
    );
    wrong_purpose.summary_generation = Some(summary_reference(
        &store,
        wrong_purpose_call,
        store.auxiliary_audit_cut().unwrap().journal,
    ));
    assert!(
        store
            .commit_projection(ProjectionCommitInput {
                grant: CancellationSignal::new()
                    .authorize_compaction_commit(wrong_purpose_operation.clone())
                    .unwrap(),
                operation_id: wrong_purpose_operation,
                candidate: wrong_purpose,
                owner: store.projection_owner(None).unwrap(),
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 10,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap_err()
            .to_string()
            .contains("must name a Summary Call")
    );
}

mod trace_preview_tests;

#[test]
fn append_retains_checkpoint_original_dependency_validation() {
    let store = store();
    let messages: Vec<_> = (1..=3).map(message).collect();
    store.initialize(&messages).unwrap();
    let original = CheckpointItem {
        message: CheckpointMessage::Original(CanonicalMessageRef::of(
            store.conversation_id(),
            &messages[0],
        )),
        call_sources: Vec::new(),
        result_call: None,
    };
    store
        .commit_projection(commit_input(
            &store,
            "append-check",
            candidate(&store, "append-head", vec![original]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store.append_canonical(&message(4)).unwrap();
    {
        let connection = store.lock().unwrap();
        let full = super::super::canonical::ledger::load_head(&connection).unwrap();
        assert_eq!(
            full.active_message_ids,
            vec![MessageId::new("M1"), MessageId::new("M4")]
        );
        assert_eq!(
            super::super::canonical::ledger::append_head(&connection).unwrap(),
            (full.revision, full.compaction_generation)
        );
        // Deliberate external corruption of the Original dependency. The
        // candidate and activation remain unchanged and must not be reblessed.
        connection.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM message_ledger WHERE message_id='M1'; PRAGMA foreign_keys=ON").unwrap();
        assert!(
            matches!(super::super::canonical::ledger::append_head(&connection), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("Original dependency"))
        );
    }
    assert!(store.append_canonical(&message(5)).is_err());
    assert!(
        !super::super::canonical::ledger::ledger_message_exists(
            &store.lock().unwrap(),
            &MessageId::new("M5")
        )
        .unwrap()
    );
}
