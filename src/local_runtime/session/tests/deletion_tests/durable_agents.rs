use super::*;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, SubagentWorkspaceTerminalResource};
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
fn resumed_agent_is_one_session_ownership_edge_and_keeps_workspace_blocker() {
    let (root, catalog, _) = open_catalog();
    let session = first_session(&catalog);
    let (node, _) = catalog.lineage(&session, None).unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let child_id = child(root.path(), &parent, 1, true);
    let before = DeletionTargetSnapshot::inspect(root.path(), &session).unwrap();
    let revision = *before.ownership_revision();
    let blocker = before.workspace_blockers()[0].clone();
    assert!(blocker.resource_id.starts_with("agent:"));
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
        &SubagentWorkspaceTerminalResource::None,
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
    assert_eq!(resumed.workspace_blockers(), &[blocker]);
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
        &SubagentWorkspaceTerminalResource::None,
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

#[tokio::test]
async fn cold_clean_isolated_agent_deletion_cannot_substitute_git_inspection_for_process_proof() {
    let fixture = physical_agent().await;
    let (node, _) = fixture.catalog.lineage(&fixture.session, None).unwrap();
    let parent = store_for(&fixture.catalog, &fixture.session, &node.conversation_id);
    let resumed = activation(&parent, 2);
    parent.append_event(resumed).unwrap();
    let agent_id = AgentId::new("durable-clean-agent");
    publish_unproven_terminal(
        &parent,
        &SubagentId::for_conversation(parent.conversation_id(), 2),
        &agent_id,
    );
    let error = DeletionTargetSnapshot::inspect(fixture.root.path(), &fixture.session).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Agent activation terminal has no proven physical settlement")
    );
    assert!(matches!(
        fixture.catalog.delete_preview(&fixture.session),
        SessionDeleteResult::Blocked { .. }
    ));
    assert!(fixture.worktree.exists());
    assert!(parent.load_agent_authority(&agent_id).is_ok());
    assert!(
        fixture
            .catalog
            .database_path(&fixture.session, &fixture.child)
            .exists()
    );
}

struct PhysicalAgent {
    root: tempfile::TempDir,
    source: tempfile::TempDir,
    catalog: SessionCatalog,
    session: SessionId,
    child: ConversationId,
    worktree: std::path::PathBuf,
}

fn git(directory: &std::path::Path, arguments: &[&str]) {
    let result = std::process::Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

async fn physical_agent() -> PhysicalAgent {
    physical_agent_at_root(None).await
}

async fn physical_agent_at_root(root_alias: Option<&std::path::Path>) -> PhysicalAgent {
    let (root, catalog, _) = open_catalog();
    let source = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-q"]);
    std::fs::write(source.path().join("tracked.txt"), "original\n").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-qm", "base"]);
    let session = first_session(&catalog);
    let (node, _) = catalog.lineage(&session, None).unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let allocation = SubagentId::for_conversation(parent.conversation_id(), 1);
    // Session-owned workspaces use the same canonical storage authority as
    // production admission, including when the product root has an OS alias.
    #[cfg(unix)]
    if let Some(alias) = root_alias {
        std::os::unix::fs::symlink(root.path(), alias).unwrap();
    }
    let identity =
        crate::runtime::local_storage::ProductRoot::existing(root_alias.unwrap_or(root.path()))
            .unwrap();
    let access = crate::runtime::local_storage::ConversationAccess::existing(
        &identity,
        catalog
            .database_path(&session, parent.conversation_id())
            .parent()
            .unwrap(),
    )
    .unwrap();
    let manager = crate::runtime::workspace::WorkspaceManager::for_local_conversation(
        source.path().canonicalize().unwrap(),
        std::sync::Arc::new(access),
    );
    let lease = manager
        .acquire(
            crate::runtime::workspace::WorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            },
            &allocation,
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .unwrap();
    let workspace = lease.snapshot().clone();
    let worktree = workspace.logical_workspace.clone();
    let child = ConversationId::generate();
    let agent = AgentId::new("durable-clean-agent");
    let (ownership, authority) = admit_agent(crate::runtime::subagent::ownership_event(
        &AgentId::new("agent-parent"),
        parent.conversation_id(),
        &allocation,
        &agent,
        &child,
        &crate::runtime::subagent::AgentActivationOrigin::CreationTool {
            tool_call_id: ToolCallId::new("delegation"),
        },
        &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        &serde_json::from_value(serde_json::json!(format!("sha256:{}", "a".repeat(64)))).unwrap(),
        crate::events::types::SubagentOwnershipKind::Normal,
        &workspace,
        Utc::now(),
    ));
    parent
        .append_agent_admission(ownership, &authority)
        .unwrap();
    let (draft, terminal) = crate::runtime::subagent::terminal_publication(
        parent.conversation_id(),
        &allocation,
        &agent,
        crate::events::types::SubagentTerminalState::Interrupted,
        vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "fixture physical settlement proven".into(),
            },
        )],
        &SubagentWorkspaceTerminalResource::None,
        true,
        Utc::now(),
    );
    parent
        .accept_subagent_terminal(None, draft, terminal)
        .unwrap();
    let database =
        crate::runtime::subagent::child_conversation_store_path(root.path(), &session, &child);
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    SqliteConversationStore::open(child.clone(), &database)
        .unwrap()
        .initialize(&[])
        .unwrap();
    // Process retirement drops the live lease; durable ownership still retains
    // the exact Git allocation for the Session cleanup authority.
    drop(lease);
    drop(manager);
    drop(parent);
    PhysicalAgent {
        root,
        source,
        catalog,
        session,
        child,
        worktree,
    }
}

#[tokio::test]
async fn session_deletion_releases_clean_agent_workspace_and_replays_frozen_cleanup() {
    let mut fixture = physical_agent().await;
    let SessionDeleteResult::Preview { preview } = fixture.catalog.delete_preview(&fixture.session)
    else {
        panic!("clean Agent workspace is deletable")
    };
    let work = fixture
        .catalog
        .commit_delete(&fixture.session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let record = work.record.clone();
    assert_eq!(record.agent_workspaces.len(), 1);
    assert!(
        fixture.worktree.exists(),
        "catalog commit precedes physical settlement"
    );
    work.settle().await.unwrap();
    assert!(!fixture.worktree.exists());
    assert!(
        fixture.source.path().join("tracked.txt").exists(),
        "source repository is never deleted"
    );
    assert!(
        !crate::runtime::subagent::child_conversation_store_path(
            fixture.root.path(),
            &fixture.session,
            &fixture.child
        )
        .exists()
    );
    // Recovery uses the same committed workset, with no missing-history scan.
    let retry = fixture.catalog.recover_delete(&fixture.session).unwrap();
    assert_eq!(retry.record, record);
    retry.settle().await.unwrap();
    assert!(matches!(
        fixture.catalog.finish_delete(&record, Ok(())),
        SessionDeleteResult::Deleted { .. }
    ));
}

#[tokio::test]
async fn session_deletion_never_forces_dirty_agent_workspace_removal() {
    let mut fixture = physical_agent().await;
    std::fs::write(fixture.worktree.join("new-work.txt"), "preserve").unwrap();
    assert!(matches!(
        fixture.catalog.delete_preview(&fixture.session),
        SessionDeleteResult::Blocked { .. }
    ));
    std::fs::remove_file(fixture.worktree.join("new-work.txt")).unwrap();
    let SessionDeleteResult::Preview { preview } = fixture.catalog.delete_preview(&fixture.session)
    else {
        panic!("clean")
    };
    let work = fixture
        .catalog
        .commit_delete(&fixture.session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let record = work.record.clone();
    // Explicit catalog commit is the gate: an external edit wins after the
    // clean workset was frozen, before the physical Git removal boundary.
    std::fs::write(fixture.worktree.join("new-work.txt"), "preserve").unwrap();
    let outcome = work.settle().await;
    assert!(outcome.is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.worktree.join("new-work.txt")).unwrap(),
        "preserve"
    );
    assert!(
        crate::runtime::subagent::child_conversation_store_path(
            fixture.root.path(),
            &fixture.session,
            &fixture.child
        )
        .exists(),
        "ownership history survives incomplete workspace cleanup"
    );
    assert!(matches!(
        fixture.catalog.finish_delete(&record, outcome),
        SessionDeleteResult::CommittedCleanupPending { .. }
    ));
    std::fs::remove_file(fixture.worktree.join("new-work.txt")).unwrap();
    let retry = fixture.catalog.recover_delete(&fixture.session).unwrap();
    retry.settle().await.unwrap();
    assert!(!fixture.worktree.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn session_deletion_preserves_canonical_allocation_through_product_root_alias() {
    let aliases = tempfile::tempdir().unwrap();
    let alias = aliases.path().join("product-root");
    let mut fixture = physical_agent_at_root(Some(&alias)).await;
    assert!(
        fixture
            .worktree
            .starts_with(fixture.root.path().canonicalize().unwrap())
    );
    assert!(!fixture.worktree.starts_with(&alias));
    let SessionDeleteResult::Preview { preview } = fixture.catalog.delete_preview(&fixture.session)
    else {
        panic!("canonical Agent allocation must remain deletable through a root alias")
    };
    let work = fixture
        .catalog
        .commit_delete(&fixture.session, &preview.target_revision)
        .unwrap()
        .unwrap();
    work.settle().await.unwrap();
    assert!(!fixture.worktree.exists());
    fixture
        .catalog
        .recover_delete(&fixture.session)
        .unwrap()
        .settle()
        .await
        .unwrap();
}
