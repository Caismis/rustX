//! Native child receipts survive parent process death and failed publication.
//! Every crash is an explicit production gate; no test writes a receipt.
use super::{
    child,
    harness::{CONVERSATION, Lab},
};
use crate::events::types::{AgentActivationAdmissionPhase, RuntimeEvent, SubagentTerminalState};
use crate::runtime::identity::{ConversationId, SubagentId};

fn assert_session_deletion_completes(lab: &Lab) {
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    let mut catalog = lab.catalog();
    let session =
        crate::runtime::identity::SessionId::new("ses_0199c989-03a0-7000-8000-000000000001");
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&session) else {
        panic!("proven physical settlement must permit ordinary Session deletion");
    };
    let work = catalog
        .commit_delete(&session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let record = work.record.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let cleanup = runtime.block_on(work.settle());
    let outcome = catalog.finish_delete(&record, cleanup);
    assert!(
        matches!(outcome, SessionDeleteResult::Deleted { session_id } if session_id == session)
    );
    assert!(!lab.database().exists());
}

#[test]
fn clean_native_settlement_survives_parent_death_before_terminal_commit() {
    let lab = Lab::new();
    let mut parent = lab.spawn(
        child::REAL_SUBAGENT_SETTLED,
        Some("after:subagent_physical_settlement"),
    );
    parent.wait_reached("after:subagent_physical_settlement");
    parent.sigkill();
    let durable = lab.durable();
    let owned = durable
        .journal()
        .into_iter()
        .find_map(|e| match e.event {
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                child_agent_id,
                child_conversation_id,
                ..
            } => Some((subagent_id, child_agent_id, child_conversation_id)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        durable.count_events(|e| matches!(e, RuntimeEvent::SubagentTerminalPublished { .. })),
        0
    );
    let mut reopened = lab.spawn(child::REOPEN_SUBAGENT_SETTLEMENT, None);
    let identity = reopened.wait_note_prefixed("physical-agent:");
    assert_eq!(
        identity,
        format!("physical-agent:{}:{}:{}", owned.1, owned.2, owned.0)
    );
    reopened.wait_note("physical-reopened:idle:shutdown-ok");
    reopened.sigkill();
    assert_eq!(durable.count_events(|e| matches!(e, RuntimeEvent::SubagentTerminalPublished { subagent_id, state: SubagentTerminalState::Interrupted, physical_settlement_proven: false, .. } if *subagent_id == owned.0)), 1);
    assert_eq!(durable.count_events(|e| matches!(e, RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } if *subagent_id == owned.0)), 1);
    assert_eq!(
        durable.count_events(|e| matches!(
            e,
            RuntimeEvent::SubagentTerminalPublished {
                state: SubagentTerminalState::Succeeded,
                ..
            }
        )),
        0,
        "physical proof cannot fabricate the unpublished logical answer"
    );
    // A second real reopen consumes the already committed fact without another
    // terminal or physical-proof append.
    let before = durable.journal().len();
    let mut twice = lab.spawn(child::REOPEN_SUBAGENT_SETTLEMENT, None);
    twice.wait_note("physical-reopened:idle:shutdown-ok");
    twice.sigkill();
    assert_eq!(durable.journal().len(), before);
    drop(durable);
    assert_session_deletion_completes(&lab);
}

#[test]
fn clean_native_settlement_survives_abandoned_parent_terminal_publication() {
    let lab = Lab::new();
    let mut parent = lab.spawn(child::REAL_SUBAGENT_PUBLICATION_FAILURE, None);
    let identity = parent.wait_note_prefixed("physical-publication-failed:");
    parent.sigkill();
    let durable = lab.durable();
    assert_eq!(
        durable.count_events(|e| matches!(e, RuntimeEvent::SubagentTerminalPublished { .. })),
        0
    );
    let mut reopened = lab.spawn(child::REOPEN_SUBAGENT_SETTLEMENT, None);
    assert_eq!(
        reopened.wait_note_prefixed("physical-agent:"),
        identity.replace("physical-publication-failed:", "physical-agent:")
    );
    reopened.wait_note("physical-reopened:idle:shutdown-ok");
    reopened.sigkill();
    assert_eq!(
        durable
            .count_events(|e| matches!(e, RuntimeEvent::SubagentPhysicalSettlementProven { .. })),
        1
    );
    assert_eq!(
        durable.count_events(|e| matches!(
            e,
            RuntimeEvent::SubagentTerminalPublished {
                state: SubagentTerminalState::Interrupted,
                ..
            }
        )),
        1
    );
    drop(durable);
    assert_session_deletion_completes(&lab);
}

#[test]
fn reserved_crash_prefixes_recover_exact_identity_without_reuse() {
    for (gate, nth, reserved_committed) in [
        ("before:event:agent_activation_admission", 1, false),
        ("after:event:agent_activation_admission", 1, true),
        ("after:subagent_staged", 2, true),
    ] {
        let lab = Lab::new();
        let mut parent = lab.spawn_nth(child::REAL_SUBAGENT_RESUME, Some(gate), nth);
        let prior = parent.wait_note_prefixed("physical-agent:");
        parent.wait_reached(gate);
        parent.sigkill();
        let durable = lab.durable();
        let reserved = SubagentId::for_conversation(&ConversationId::new(CONVERSATION), 2);
        assert_eq!(durable.count_events(|e| matches!(e, RuntimeEvent::AgentActivationAdmission { activation_id, phase: AgentActivationAdmissionPhase::Reserved, .. } if *activation_id == reserved)), usize::from(reserved_committed));
        assert_eq!(
            durable.count_events(|e| matches!(e, RuntimeEvent::SubagentOwnershipCommitted { .. })),
            1,
            "the crash precedes second ownership commit at {gate}"
        );
        let mut reopened = lab.spawn(child::REOPEN_SUBAGENT_RESUME, None);
        assert_eq!(reopened.wait_note_prefixed("physical-agent:"), prior);
        let resumed = reopened.wait_note_prefixed("physical-resumed:");
        let next = SubagentId::for_conversation(&ConversationId::new(CONVERSATION), 3);
        assert!(
            resumed.ends_with(&format!(":{next}")),
            "consumed activation 2 cannot be reused even before Reserved: {resumed}"
        );
        reopened.wait_note("physical-reopened:idle:shutdown-ok");
        reopened.sigkill();
        assert_eq!(durable.count_events(|e| matches!(e, RuntimeEvent::AgentActivationAdmission { activation_id, phase: AgentActivationAdmissionPhase::RolledBack { physical_settlement_proven: true }, .. } if *activation_id == reserved)), usize::from(reserved_committed));
        assert_eq!(durable.count_events(|e| matches!(e, RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. } if *subagent_id == reserved)), 0);
    }
}
