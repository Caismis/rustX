//! Native Title ownership survives real process death at its three authorities.
use crate::events::types::RuntimeEvent;
use crate::model::{AuxiliaryCallAudit, AuxiliaryCallOutcome};
use crate::runtime::identity::MessageId;

use super::child;
use super::harness::{Durable, Lab};

fn only_call(durable: &Durable) -> AuxiliaryCallAudit {
    let journal = durable.journal();
    let calls = journal
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::AuxiliaryCallStarted { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1, "one native Title admission");
    durable
        .store()
        .load_auxiliary_call(calls[0], durable.store().auxiliary_audit_cut().unwrap())
        .unwrap()
        .unwrap()
}

#[test]
fn title_call_death_recovery_releases_only_a_confirmed_zero_request_association() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::SESSION_TITLE,
        Some("after:auxiliary_call_start_confirmed"),
    );
    process.wait_reached("after:auxiliary_call_start_confirmed");
    process.sigkill();
    let (session, conversation) = lab.source_lineage();
    let original = {
        let durable = lab.lineage(&session, &conversation);
        let call = only_call(&durable);
        assert!(call.outcome.is_none());
        assert!(
            durable
                .store()
                .read_auxiliary_call_requests(
                    &call.call_id,
                    durable.store().auxiliary_audit_cut().unwrap(),
                    None,
                    3
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            !durable
                .store()
                .first_human_source()
                .unwrap()
                .unwrap()
                .eligible
        );
        call
    };
    for _ in 0..2 {
        let mut reopened = lab.spawn(child::SESSION_TITLE_RECOVER_INERT, None);
        reopened.wait_note("title-reopen:recovered-before-activation");
        reopened.sigkill();
        let durable = lab.lineage(&session, &conversation);
        let call = only_call(&durable);
        assert_eq!(call.call_id, original.call_id);
        assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Cancelled));
        assert!(
            durable
                .store()
                .first_human_source()
                .unwrap()
                .unwrap()
                .eligible
        );
        assert_eq!(
            durable
                .count_events(|event| matches!(event, RuntimeEvent::AuxiliaryCallSettled { .. })),
            1,
            "repeated startup recovery releases exactly once"
        );
    }
    assert!(
        lab.catalog()
            .summary(&session)
            .unwrap()
            .generated_title
            .is_none()
    );
}

#[test]
fn title_confirmed_request_death_preserves_consumption_without_cold_dispatch() {
    let lab = Lab::new();
    let mut process = lab.spawn(
        child::SESSION_TITLE,
        Some("after:auxiliary_request_start_confirmed"),
    );
    process.wait_reached("after:auxiliary_request_start_confirmed");
    process.sigkill();
    let (session, conversation) = lab.source_lineage();
    let original = {
        let durable = lab.lineage(&session, &conversation);
        let call = only_call(&durable);
        assert!(call.outcome.is_none());
        let requests = durable
            .store()
            .read_auxiliary_call_requests(
                &call.call_id,
                durable.store().auxiliary_audit_cut().unwrap(),
                None,
                3,
            )
            .unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].terminal_sequence.is_none());
        assert!(requests[0].request_evidence.is_none());
        assert!(
            !durable
                .store()
                .first_human_source()
                .unwrap()
                .unwrap()
                .eligible
        );
        durable.journal()
    };
    let mut reopened = lab.spawn(child::SESSION_TITLE_REOPEN, None);
    reopened.wait_note("title-reopen:idle:shutdown-ok");
    reopened.sigkill();
    assert_eq!(
        lab.lineage(&session, &conversation).journal(),
        original,
        "restart neither replays the request nor invents terminal evidence"
    );
    assert!(
        lab.catalog()
            .summary(&session)
            .unwrap()
            .generated_title
            .is_none()
    );
}

#[test]
fn title_catalog_visibility_is_atomic_after_model_settlement_and_never_replayed() {
    for (gate, visible) in [
        ("before:publish_generated_title", false),
        ("after:publish_generated_title", true),
    ] {
        let lab = Lab::new();
        let mut process = lab.spawn(child::SESSION_TITLE, Some(gate));
        process.wait_reached(gate);
        process.sigkill();
        let (session, conversation) = lab.source_lineage();
        let (original, call_id) = {
            let durable = lab.lineage(&session, &conversation);
            let call = only_call(&durable);
            assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Completed));
            let requests = durable
                .store()
                .read_auxiliary_call_requests(
                    &call.call_id,
                    durable.store().auxiliary_audit_cut().unwrap(),
                    None,
                    3,
                )
                .unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].request_evidence.is_some());
            assert!(
                !durable
                    .store()
                    .first_human_source()
                    .unwrap()
                    .unwrap()
                    .eligible
            );
            (durable.journal(), call.call_id)
        };
        let summary = lab.catalog().summary(&session).unwrap();
        assert_eq!(summary.generated_title.is_some(), visible);
        assert!(summary.name.is_none());
        if let Some(title) = &summary.generated_title {
            assert_eq!(title.text, "Generated title");
            assert!(
                matches!(&title.provenance, crate::local_runtime::session::SessionTitleProvenance::Local { conversation_id, message_id, call_id: accepted } if conversation_id == &conversation && message_id == &MessageId::new("msg_0199c989-03a0-7000-8000-000000000002") && accepted == &call_id)
            );
        }
        let mut reopened = lab.spawn(child::SESSION_TITLE_REOPEN, None);
        reopened.wait_note("title-reopen:idle:shutdown-ok");
        reopened.sigkill();
        assert_eq!(lab.lineage(&session, &conversation).journal(), original);
        assert_eq!(
            lab.catalog().summary(&session).unwrap().generated_title,
            summary.generated_title,
            "unpersisted output is not replayed into Catalog on restart"
        );
    }
}

#[test]
fn title_timestamp_less_human_after_runtime_preview_wakes_the_native_owner() {
    let lab = Lab::new();
    let mut process = lab.spawn(child::SESSION_TITLE_AFTER_RUNTIME, None);
    process.wait_note("title-after-runtime:published");
    process.sigkill();
    let (session, conversation) = lab.source_lineage();
    let durable = lab.lineage(&session, &conversation);
    let source = durable.store().first_human_source().unwrap().unwrap();
    assert_eq!(source.position, 2);
    assert_eq!(
        source.message_id,
        MessageId::new("msg_0199c989-03a0-7000-8000-000000000003")
    );
    assert!(!source.eligible);
    assert_eq!(
        only_call(&durable).outcome,
        Some(AuxiliaryCallOutcome::Completed)
    );
    assert_eq!(
        lab.catalog()
            .summary(&session)
            .unwrap()
            .generated_title
            .unwrap()
            .text,
        "Generated title"
    );
}
