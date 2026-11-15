use super::*;
use crate::context::operation::{
    CompactionMode, CompactionOperationPhase as Phase, CompactionOperationStatus,
};
use crate::durable::AuxiliaryStore;
use crate::observation::trace::TraceCompactionOutcome as Outcome;
use crate::runtime::identity::CompactionOperationId;

fn admit(store: &SqliteConversationStore, generation: u64) -> CompactionOperationStatus {
    let id = CompactionOperationId::new(format!("trace-async-{generation}"));
    let status = CompactionOperationStatus {
        operation_id: id.clone(),
        admission_generation: generation,
        triggering_attempt_id: None,
        waiting_attempt_id: None,
        phase: Phase::Generating,
        accepted_fragments: 0,
        diagnostic: None,
    };
    let mut started = event(
        store,
        E::CompactionStarted {
            operation_id: id,
            mode: CompactionMode::Proactive,
        },
        i64::try_from(generation).expect("fixture generation fits i64"),
    );
    started.attempt_id = None;
    started.turn_id = None;
    started.event_id = EventId::new(format!("trace-async-start-{generation}"));
    let mut update = started.clone();
    update.event_id = EventId::new(format!("trace-async-status-{generation}"));
    update.event = E::CompactionOperationUpdated {
        status: status.clone(),
    };
    store.append_compaction_admission(started, update).unwrap();
    status
}

fn update(store: &SqliteConversationStore, status: &CompactionOperationStatus, phase: Phase) {
    let mut status = status.clone();
    status.phase = phase;
    let mut update = event(store, E::CompactionOperationUpdated { status }, 100);
    update.attempt_id = None;
    update.turn_id = None;
    store.append_event(update).unwrap();
}

#[test]
fn compaction_status_is_exact_per_operation_and_historical_cut() {
    let store = store("conv_859b7b18-8b48-7e15-85a2-ffb3f37e1db0");
    let first = admit(&store, 1);
    update(&store, &first, Phase::Ready);
    let ready_cut = store.observation_read_cut().unwrap();
    let original = TraceProjection::at_cut(&store, &ready_cut)
        .unwrap()
        .page(None, 32)
        .unwrap()
        .records
        .remove(0);
    assert_eq!(
        original.compaction.as_ref().unwrap().mode,
        CompactionMode::Proactive
    );
    assert_eq!(
        original
            .compaction
            .as_ref()
            .unwrap()
            .status
            .as_ref()
            .unwrap()
            .phase,
        Phase::Ready
    );
    assert_eq!(original.compaction.as_ref().unwrap().outcome, None);
    assert_ne!(original.state, TraceState::Completed);
    let before = TraceProjection::at_cut(&store, &ready_cut).unwrap();
    let anchor = before
        .inspection_anchor(original.position.sequence().unwrap())
        .unwrap();
    assert!(matches!(
        before.inspection_head(&anchor).unwrap().content,
        TraceInspectionContent::Compaction {
            checkpoint: None,
            ..
        }
    ));
    update(&store, &first, Phase::NoEffect);
    for (generation, phase) in [
        (2, Phase::Cancelled),
        (3, Phase::Superseded),
        (4, Phase::Failed),
    ] {
        let status = admit(&store, generation);
        update(&store, &status, phase);
    }
    let current = TraceProjection::new(&store).unwrap();
    let page = current.page(None, 32).unwrap();
    for (id, expected) in [
        ("trace-async-1", Outcome::NoEffect),
        ("trace-async-2", Outcome::Cancelled),
        ("trace-async-3", Outcome::Superseded),
        ("trace-async-4", Outcome::Failed),
    ] {
        let record = page
            .records
            .iter()
            .find(|record| record.native_id.as_deref() == Some(id))
            .unwrap();
        assert_eq!(record.compaction.as_ref().unwrap().outcome, Some(expected));
    }
    let refreshed = current
        .refresh(std::slice::from_ref(&original.position), None)
        .unwrap();
    assert_eq!(
        refreshed[0].compaction.as_ref().unwrap().outcome,
        Some(Outcome::NoEffect)
    );
    let still_ready = TraceProjection::at_cut(&store, &ready_cut)
        .unwrap()
        .page(None, 32)
        .unwrap()
        .records
        .remove(0);
    assert_eq!(
        still_ready, original,
        "later statuses never alter an earlier source cut"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "Keep the call, cut, and request paging assertions in one ordered scenario"
)]
fn compaction_inspection_pages_exact_calls_and_physical_request_identity() {
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallRef,
        AuxiliaryCallStart, AuxiliaryFingerprint, AuxiliaryFinishReason, AuxiliaryInputEvidence,
        AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
        AuxiliaryUsageCompleteness,
    };
    use crate::runtime::identity::AuxiliaryCallId;
    let store = store("conv_89f89d63-8c0f-7092-b4ca-381cd2c2bbf0");
    let status = admit(&store, 1);
    let projection = TraceProjection::new(&store).unwrap();
    let anchor = projection
        .page(None, 32)
        .unwrap()
        .records
        .remove(0)
        .position
        .sequence()
        .unwrap();
    let fingerprint = AuxiliaryFingerprint::new("a".repeat(64)).unwrap();
    let make_call = |name: &str, preceding_call| AuxiliaryCallStart {
        call_id: AuxiliaryCallId::new(name),
        purpose: AuxiliaryAttribution::new("summary").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Compaction {
            operation_id: status.operation_id.clone(),
            preceding_call,
        },
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: fingerprint.clone(),
        invocation_fingerprint: fingerprint.clone(),
        selected_message_ids: Vec::new(),
    };
    let first = make_call("aux_019a0000-0000-7000-8000-000000000001", None);
    store
        .commit_auxiliary_call_start(&first, timestamp(2))
        .unwrap();
    let mut invocation = crate::model::auxiliary::fixture_invocation_identity();
    invocation.profile = Some(crate::model::catalog::ModelProfileId::parse("precise").unwrap());
    let request = AuxiliaryRequestStart {
        call_id: first.call_id.clone(),
        ordinal: 0,
        request_id: RequestId::new("req_019a0000-0000-7000-8000-000000000001"),
        model: "wire-model".into(),
        invocation: invocation.clone(),
    };
    store
        .commit_auxiliary_request_start(&request, timestamp(3))
        .unwrap();
    let usage = ModelUsage {
        input_tokens: Some(11),
        output_tokens: Some(5),
        total_tokens: Some(16),
        details: None,
    };
    let terminal = crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        timestamp(4),
        E::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Auxiliary {
                call_id: first.call_id.clone(),
                ordinal: 0,
            },
            request_id: request.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage.clone()),
            generation: None,
        },
    )
    .unwrap();
    let settled = crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        timestamp(4),
        E::AuxiliaryCallSettled {
            call_id: first.call_id.clone(),
            outcome: AuxiliaryCallOutcome::Completed,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id: request.request_id.clone(),
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::Stop,
                }),
                usage: Some(usage),
                usage_completeness: AuxiliaryUsageCompleteness::Complete,
                remote_execution_uncertainty: false,
            }),
        },
    )
    .unwrap();
    let receipt = store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
    let old_cut = TraceProjection::new(&store).unwrap().history_cut().unwrap();
    let second = make_call(
        "aux_019a0000-0000-7000-8000-000000000002",
        Some(AuxiliaryCallRef {
            conversation_id: store.conversation_id().clone(),
            call_id: first.call_id.clone(),
            through_journal: receipt.call.settled_sequence.unwrap(),
        }),
    );
    store
        .commit_auxiliary_call_start(&second, timestamp(5))
        .unwrap();
    let cut = TraceProjection::new(&store).unwrap().history_cut().unwrap();
    let domain = format!("trace.records.{anchor}.detail.calls");
    let old =
        crate::runtime_client::trace_observation::history_page(&store, &old_cut, &domain, 0, 1)
            .unwrap();
    assert_eq!(old.total, 1);
    let first_page =
        crate::runtime_client::trace_observation::history_page(&store, &cut, &domain, 0, 1)
            .unwrap();
    assert_eq!(first_page.total, 2);
    assert_eq!(first_page.items[0].identity, first.call_id.to_string());
    let next = first_page
        .next_offset
        .expect("advancing exclusive Call cursor");
    let second_page =
        crate::runtime_client::trace_observation::history_page(&store, &cut, &domain, next, 1)
            .unwrap();
    assert_eq!(second_page.items[0].identity, second.call_id.to_string());
    assert_eq!(
        second_page.items[0].value.as_ref().unwrap()["preceding_call"]["call_id"],
        first.call_id.to_string()
    );
    assert!(second_page.next_offset.is_none());
    let requests = format!("{domain}.{}.requests", first.call_id);
    let requests_page =
        crate::runtime_client::trace_observation::history_page(&store, &cut, &requests, 0, 1)
            .unwrap();
    let item = requests_page.items[0].value.as_ref().unwrap();
    assert_eq!(item["model"], "wire-model");
    assert_eq!(item["invocation"]["provider"], "fixture-provider");
    assert_eq!(item["invocation"]["model_ref"], "fixture/logical-model");
    assert_eq!(item["invocation"]["profile"], "precise");
    let evidence: crate::model::auxiliary::AuxiliaryRequestAudit =
        crate::observation::bounded::decode_domain_value(item.clone()).unwrap();
    assert_eq!(evidence.invocation, invocation);
    assert_eq!(
        evidence.usage.as_ref().and_then(|usage| usage.total_tokens),
        Some(16)
    );
    assert!(
        !evidence
            .request_evidence
            .unwrap()
            .remote_execution_uncertainty
    );
    assert!(
        crate::runtime_client::trace_observation::history_page(&store, &cut, &domain, next + 1, 1)
            .is_err()
    );
}
