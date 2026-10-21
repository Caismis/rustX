//! Frozen effective execution profile identity across ownership commit,
//! workflow ownership, and recovery (SUB-258).

use super::*;
use crate::runtime::ToolCallId;

/// Issue #258 — the effective execution-profile identity is a **durable**
/// execution fact.
///
/// A named role is only the child's *default* profile: an authorized
/// invocation override may replace its tools, Skills, or extensions for
/// exactly this child. `definition_digest` cannot express that, so the
/// ownership commit carries the effective profile identity too, and the
/// value the durable fact holds, the value the live projection reports,
/// and the value a terminal record still reports are one value.
#[tokio::test]
async fn sub258_ownership_commit_records_the_frozen_effective_profile_digest() {
    let plane = plane(4);
    let child = stage_exit0(&plane);

    // A specialized child: the frozen contract differs from the role's
    // defaults, so its profile identity differs from a default child's
    // while its definition identity does not.
    let mut spec = start_spec("inspect");
    spec.authority.resolved.extensions = crate::extensions::NativeAgentExtensions::none();
    let expected = spec.authority.resolved.profile_digest();
    assert_ne!(
        expected,
        resolved("explore").profile_digest(),
        "the specialization really did change the effective profile"
    );

    let accepted = start(&plane, &spec).await;
    let committed = events(&plane)
        .into_iter()
        .find_map(|event| match event {
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                definition_digest,
                profile_digest,
                ..
            } if subagent_id == accepted.subagent_id => Some((definition_digest, profile_digest)),
            _ => None,
        })
        .expect("one durable ownership fact");
    assert_eq!(
        committed,
        (
            spec.authority
                .resolved
                .definition_digest
                .as_str()
                .to_owned(),
            expected.as_str().to_owned()
        ),
        "the ownership fact commits both identities exactly as frozen"
    );

    let running = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .expect("running record");
    assert_eq!(
        running.profile_digest,
        expected.as_str(),
        "the live projection reports the committed identity"
    );

    // Terminal settlement is a lifecycle transition, not a re-derivation.
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("terminal settlement");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        settled.profile_digest,
        expected.as_str(),
        "terminal settlement never alters the committed profile identity"
    );
}

/// A Workflow-owned child preserves it identically: ownership kind
/// changes the terminal domain, never the committed identity.
#[tokio::test]
async fn sub258_a_workflow_owned_child_commits_the_same_profile_identity() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let spec = workflow_spec(&plane.conversation_id, "inspect");
    let expected = spec.authority.resolved.profile_digest();

    let accepted = start(&plane, &spec).await;
    let committed = events(&plane)
        .into_iter()
        .find_map(|event| match event {
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                profile_digest,
                ownership,
                ..
            } if subagent_id == accepted.subagent_id => Some((ownership, profile_digest)),
            _ => None,
        })
        .expect("one durable ownership fact");
    assert_eq!(
        committed,
        (
            crate::events::types::SubagentOwnershipKind::Workflow,
            expected.as_str().to_owned()
        )
    );
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("running record")
            .profile_digest,
        expected.as_str()
    );
    drop(child);
}

/// Recovery restores the **committed** identity and derives nothing.
///
/// The durable evidence deliberately names a definition and a profile no
/// live resolution in this process could produce, so a projection that
/// recomputed from the current role definition or the current resource
/// generation could not possibly reproduce them.
#[tokio::test]
async fn sub258_recovery_restores_the_committed_profile_identity_without_recomputing_it() {
    let plane = plane(4);
    let subagent_id = SubagentId::for_conversation(&plane.conversation_id, 7);
    let committed_profile = format!("sha256:{}", "a".repeat(64));
    let committed_definition = format!("sha256:{}", "b".repeat(64));
    let evidence = crate::runtime::recovery::SubagentEvidence {
        subagent_id: subagent_id.clone(),
        child_agent_id: crate::runtime::identity::AgentId::new("agent-recovered"),
        child_conversation_id: crate::runtime::identity::ConversationId::new(
            "conv_5c5feea1-e5d8-7965-8253-00af9a91ea8c",
        ),
        origin: AgentActivationOrigin::CreationTool {
            tool_call_id: ToolCallId::new("call-recovered"),
        },
        agent: "explore".to_owned(),
        definition_digest: committed_definition.clone(),
        profile_digest: committed_profile.clone(),
        ownership: crate::events::types::SubagentOwnershipKind::Normal,
        started_at: chrono::Utc::now(),
        workspace: plane.dir.path().join("workspace"),
    };
    let restored = SubagentRecord::recovered(plane.registry.config.agent_id.clone(), &evidence)
        .expect("the recovery-projected record")
        .snapshot();
    assert_eq!(
        restored.profile_digest, committed_profile,
        "recovery restores exactly what ownership committed"
    );
    assert_eq!(restored.definition_digest, committed_definition);
    assert_ne!(
        restored.profile_digest,
        resolved("explore").profile_digest().as_str(),
        "the restored identity is emphatically not one this process could recompute"
    );
}
