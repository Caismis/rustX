use super::*;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, SubagentWorkspaceTerminalResource};

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
        ..
    } = &mut event.event
    {
        *subagent_id = id.clone();
        *admitted_authority = None;
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
    let (draft, terminal) = crate::runtime::subagent::recovery_terminal_publication(
        parent.conversation_id(),
        &id,
        &AgentId::new(format!("agent-{child_id}")),
        "explore",
        "sha256:definition",
        &SubagentWorkspaceTerminalResource::None,
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
            next = admit_agent(next);
            assert!(
                parent.append_event(next).is_err(),
                "frozen credential ownership cannot be readmitted"
            );
            assert_eq!(parent.read_events(None, 256).unwrap().events.len(), 1);
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
    let manager = crate::runtime::workspace::WorkspaceManager::new(
        source.path(),
        root.path().join("workspaces"),
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
    let ownership = admit_agent(crate::runtime::subagent::ownership_event(
        &AgentId::new("agent-parent"),
        parent.conversation_id(),
        &allocation,
        &agent,
        &child,
        &ToolCallId::new("delegation"),
        &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        &serde_json::from_value(serde_json::json!(format!("sha256:{}", "a".repeat(64)))).unwrap(),
        crate::events::types::SubagentOwnershipKind::Normal,
        &workspace,
        Utc::now(),
    ));
    parent.append_event(ownership).unwrap();
    let (draft, terminal) = crate::runtime::subagent::recovery_terminal_publication(
        parent.conversation_id(),
        &allocation,
        &agent,
        "explore",
        "sha256:definition",
        &SubagentWorkspaceTerminalResource::None,
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
    use crate::local_runtime::session::deletion::SessionDeleteResult;
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
    use crate::local_runtime::session::deletion::SessionDeleteResult;
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
