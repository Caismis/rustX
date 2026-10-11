use super::*;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::local_runtime::session::deletion::SessionDeleteResult;

fn activation(parent: &SqliteConversationStore, ordinal: u64) -> RuntimeEventEnvelope {
    let mut event = parent
        .read_events(None, 256)
        .unwrap()
        .events
        .into_iter()
        .find(|event| matches!(event.event, RuntimeEvent::SubagentOwnershipCommitted { .. }))
        .unwrap();
    let id = SubagentId::for_conversation(parent.conversation_id(), ordinal);
    if let RuntimeEvent::SubagentOwnershipCommitted {
        subagent_id,
        admitted_authority,
        child_agent_id,
        origin,
        ..
    } = &mut event.event
    {
        *subagent_id = id.clone();
        *admitted_authority = None;
        *origin = crate::runtime::subagent::AgentActivationOrigin::ClientControl;
        parent
            .append_event(crate::runtime::subagent::admission_event(
                parent.conversation_id(),
                child_agent_id,
                &id,
                origin,
                crate::events::types::AgentActivationAdmissionPhase::Reserved,
                Utc::now(),
            ))
            .unwrap();
    }
    event.event_id = crate::runtime::subagent::subagent_ownership_event_id(&id);
    event
}

#[test]
fn resumed_agent_is_one_session_ownership_edge() {
    let (root, catalog, _) = open_catalog();
    let session = first_session(&catalog);
    let (node, _) = catalog.lineage(&session, None).unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let child_id = child(root.path(), &parent, 1, true);
    let before = DeletionTargetSnapshot::inspect(root.path(), &session).unwrap();
    let revision = *before.ownership_revision();
    drop(before);
    parent.append_event(activation(&parent, 2)).unwrap();
    let id = SubagentId::for_conversation(parent.conversation_id(), 2);
    let (draft, terminal) = crate::runtime::subagent::terminal_publication(
        parent.conversation_id(),
        &id,
        &AgentId::new(format!("agent-{child_id}")),
        crate::events::types::SubagentTerminalState::Interrupted,
        vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "fixture physical settlement proven".into(),
            },
        )],
        true,
        Utc::now(),
    );
    parent
        .accept_subagent_terminal(None, draft, terminal)
        .unwrap();
    let resumed = DeletionTargetSnapshot::inspect(root.path(), &session).unwrap();
    assert_eq!(resumed.conversations().len(), 2);
    assert_eq!(
        resumed
            .conversations()
            .iter()
            .filter(|c| c.conversation_id == child_id)
            .count(),
        1
    );
    assert_eq!(
        *resumed.ownership_revision(),
        revision,
        "another settled activation does not change durable ownership"
    );
    drop(resumed);
    assert_eq!(
        crate::local_runtime::session_ownership::conversation_owner(root.path(), &child_id)
            .unwrap(),
        session
    );
}

#[test]
fn activation_replay_rejects_agent_conversation_aliases_and_authority_readmission() {
    for case in [
        "different-agent",
        "different-conversation",
        "readmitted-authority",
    ] {
        let (root, catalog, _) = open_catalog();
        let session = first_session(&catalog);
        let (node, _) = catalog.lineage(&session, None).unwrap();
        let parent = store_for(&catalog, &session, &node.conversation_id);
        child(root.path(), &parent, 1, false);
        let mut next = activation(&parent, 2);
        if let RuntimeEvent::SubagentOwnershipCommitted {
            child_agent_id,
            child_conversation_id,
            ..
        } = &mut next.event
        {
            match case {
                "different-agent" => *child_agent_id = AgentId::new("alias-agent"),
                "different-conversation" => *child_conversation_id = ConversationId::generate(),
                _ => {}
            }
        }
        if case == "readmitted-authority" {
            let (next, authority) = admit_agent(next);
            assert!(
                parent.append_agent_admission(next, &authority).is_err(),
                "frozen credential ownership cannot be readmitted"
            );
            assert_eq!(parent.read_events(None, 256).unwrap().events.len(), 2);
            continue;
        }
        if case == "different-agent" {
            assert!(
                parent.append_event(next).is_err(),
                "reservation cannot authorize another Agent"
            );
            continue;
        }
        parent.append_event(next).unwrap();
        let error = DeletionTargetSnapshot::inspect(root.path(), &session).unwrap_err();
        assert!(
            error.to_string().contains("invalid or ambiguous"),
            "{case}: {error}"
        );
    }
}

#[test]
fn resumed_admission_deletion_requires_exact_generation_settlement_proof() {
    use crate::events::types::AgentActivationAdmissionPhase;
    use crate::runtime::subagent::AgentActivationOrigin;
    for proof in [None, Some(false), Some(true)] {
        let (root, catalog, _) = open_catalog();
        let session = first_session(&catalog);
        let (node, _) = catalog.lineage(&session, None).unwrap();
        let parent = store_for(&catalog, &session, &node.conversation_id);
        let child_id = child(root.path(), &parent, 1, true);
        let agent_id = AgentId::new(format!("agent-{child_id}"));
        assert!(DeletionTargetSnapshot::inspect(root.path(), &session).is_ok());
        let activation_id = SubagentId::for_conversation(parent.conversation_id(), 2);
        let fact = |phase| {
            crate::runtime::subagent::admission_event(
                parent.conversation_id(),
                &agent_id,
                &activation_id,
                &AgentActivationOrigin::ClientControl,
                phase,
                Utc::now(),
            )
        };
        parent
            .append_event(fact(AgentActivationAdmissionPhase::Reserved))
            .unwrap();
        if let Some(physical_settlement_proven) = proof {
            parent
                .append_event(fact(AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven,
                }))
                .unwrap();
        }
        let target = DeletionTargetSnapshot::inspect(root.path(), &session);
        if proof == Some(true) {
            assert!(
                target.is_ok(),
                "proved rollback releases the new admission blocker"
            );
        } else {
            let error = target.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Agent activation admission has no proven physical settlement"),
                "{error}"
            );
        }
    }
}

fn publish_unproven_terminal(
    parent: &SqliteConversationStore,
    activation_id: &SubagentId,
    agent_id: &AgentId,
) {
    let (draft, terminal) = crate::runtime::subagent::recovery_terminal_publication(
        parent.conversation_id(),
        activation_id,
        agent_id,
        "explore",
        "sha256:definition",
        Utc::now(),
    );
    parent
        .accept_subagent_terminal(None, draft, terminal)
        .unwrap();
}

#[test]
fn cold_shared_agent_deletion_retains_unproven_terminal_evidence() {
    let (root, catalog, _) = open_catalog();
    let session = first_session(&catalog);
    let (node, _) = catalog.lineage(&session, None).unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let child_id = child(root.path(), &parent, 1, false);
    let agent_id = AgentId::new(format!("agent-{child_id}"));
    publish_unproven_terminal(
        &parent,
        &SubagentId::for_conversation(parent.conversation_id(), 1),
        &agent_id,
    );
    let error = DeletionTargetSnapshot::inspect(root.path(), &session).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Agent activation terminal has no proven physical settlement")
    );
    assert!(matches!(
        catalog.delete_preview(&session),
        SessionDeleteResult::Blocked { .. }
    ));
    assert!(parent.load_agent_authority(&agent_id).is_ok());
    assert!(catalog.database_path(&session, &child_id).exists());
}

#[test]
fn pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof() {
    use crate::local_runtime::session_deletion::DeletionExclusion;
    use crate::runtime::local_storage::ProductRoot;
    use crate::runtime::subagent::physical_recovery::ParentPhysicalLease;
    let (root, catalog, _) = open_catalog();
    let session = first_session(&catalog);
    let (node, _) = catalog.lineage(&session, None).unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let child_id = child(root.path(), &parent, 1, true);
    let product = ProductRoot::existing(root.path()).unwrap();
    let activation = SubagentId::for_conversation(parent.conversation_id(), 2);
    let owner = ParentPhysicalLease::reserve(&product, &session, &child_id, &activation).unwrap();
    let duplicate = owner.duplicate_lock_for_test().unwrap();
    drop(owner);
    let target = DeletionTargetSnapshot::inspect(root.path(), &session).unwrap();
    let error = DeletionExclusion::acquire(root.path(), &target).unwrap_err();
    assert!(error.to_string().contains("physical activation"), "{error}");
    // Explicitly unlock the test-owned open-file description. Unlike drop,
    // this also releases any transient duplicate inherited by another spawn.
    duplicate.unlock().unwrap();
    drop(duplicate);
    let exclusion = DeletionExclusion::acquire(root.path(), &target).unwrap();
    drop(exclusion);
    assert!(!parent.read_events(None, 256).unwrap().events.iter().any(|event| matches!(
        &event.event, RuntimeEvent::AgentActivationAdmission { activation_id, .. } if activation_id == &activation
    )));
}
