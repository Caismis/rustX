mod borrowed_workspace;
mod workflow_disposal;

use super::*;
use crate::local_runtime::session_deletion::SessionDeletionPreflight;
use crate::runtime::identity::{AgentId, SubagentId};
use crate::runtime::workspace::{GitWorktreeSnapshot, WorkspaceIsolation, WorkspaceSnapshot};

fn child(
    root: &std::path::Path,
    parent: &SqliteConversationStore,
    ordinal: u64,
    isolated: bool,
) -> ConversationId {
    let subagent = SubagentId::for_conversation(parent.conversation_id(), ordinal);
    let id = ConversationId::new(subagent.as_str());
    let workspace = if isolated {
        WorkspaceSnapshot {
            borrowed_from: None,
            logical_workspace: root.join("workspaces/worktrees/retained"),
            isolation: WorkspaceIsolation::GitWorktree(GitWorktreeSnapshot {
                source_repository_root: root.join("external-project"),
                repository_relative_workspace: std::path::PathBuf::new(),
                physical_worktree_root: root.join("workspaces/worktrees/retained"),
                base_commit: "a".repeat(40),
                branch: "rustx/retained".into(),
                parent_had_uncommitted_changes: false,
            }),
        }
    } else {
        WorkspaceSnapshot::shared(root.join("external-project"))
    };
    let event = crate::runtime::subagent::ownership_event(
        parent.conversation_id(),
        &subagent,
        &AgentId::new(format!("agent-{id}")),
        &id,
        &ToolCallId::new(format!("call-{id}")),
        &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        crate::events::types::SubagentOwnershipKind::Normal,
        &workspace,
        Utc::now(),
    );
    parent.append_event(event).unwrap();
    if isolated {
        let tree = workspace.git_worktree().unwrap();
        std::fs::create_dir_all(&tree.physical_worktree_root).unwrap();
        std::fs::write(
            tree.physical_worktree_root.join("user-work.txt"),
            "preserve",
        )
        .unwrap();
        let resource = crate::events::types::SubagentWorkspaceTerminalResource::Retained {
            handoff: crate::runtime::workspace::WorkspaceHandoff {
                logical_workspace: workspace.logical_workspace.clone(),
                physical_worktree_root: tree.physical_worktree_root.clone(),
                branch: tree.branch.clone(),
                base_commit: tree.base_commit.clone(),
                head_commit: tree.base_commit.clone(),
                dirty: true,
            },
        };
        let (draft, event) = crate::runtime::subagent::recovery_terminal_publication(
            parent.conversation_id(),
            &subagent,
            &AgentId::new(format!("agent-{id}")),
            "explore",
            "sha256:definition",
            &resource,
            Utc::now(),
        );
        parent.accept_subagent_terminal(None, draft, event).unwrap();
    }
    let database = crate::runtime::subagent::child_conversation_store_path(root, &id);
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    SqliteConversationStore::open(id.clone(), &database)
        .unwrap()
        .initialize(&[])
        .unwrap();
    id
}

#[test]
fn deletion_tree_membership_excludes_independent_fork_clone_and_shared_resources() {
    let (directory, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &session, &conversation);
    let revision = store.load_head().unwrap().revision;
    let source = lineage_at(&store, &conversation, revision);
    let mut expected = BTreeSet::from([conversation]);
    for _ in 0..2 {
        let (prepared, _) = catalog
            .prepare_tree_node_at_user_message(
                &session,
                &state(),
                &source,
                &MessageId::new("source-user-a"),
            )
            .unwrap();
        expected.insert(prepared.conversation_id.clone());
        catalog
            .publish_node(&session, &prepared, node.clone(), SessionNodeOrigin::New)
            .unwrap();
    }
    let clone = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(
            &clone,
            SessionNodeOrigin::Clone {
                source_session: session.clone(),
                source_node: node.clone(),
                source_surface_revision: revision,
            },
        )
        .unwrap();
    let (fork, _) = catalog
        .prepare_fork_session(&state(), &source, &MessageId::new("source-user-a"))
        .unwrap();
    catalog
        .publish_session(
            &fork,
            SessionNodeOrigin::Fork {
                source_session: session.clone(),
                source_node: node,
                source_surface_revision: revision,
                source_user_message: MessageId::new("source-user-a"),
            },
        )
        .unwrap();
    for class in [
        "environments",
        "capabilities",
        "cache",
        "config",
        "credentials",
    ] {
        std::fs::create_dir(directory.path().join(class)).unwrap();
    }
    let preflight = SessionDeletionPreflight::acquire(directory.path(), &session).unwrap();
    assert_eq!(preflight.nodes().len(), 3);
    assert_eq!(
        preflight
            .conversations()
            .iter()
            .map(|c| c.conversation_id.clone())
            .collect::<BTreeSet<_>>(),
        expected
    );
    assert!(preflight.conversations().iter().all(|c| {
        c.private_root.starts_with(
            catalog
                .product
                .root()
                .join("sessions")
                .join(session.as_str()),
        )
    }));
    assert!(SessionDeletionPreflight::acquire(directory.path(), &session).is_err());
    let revision = *preflight.ownership_revision();
    drop(preflight);
    assert_eq!(
        &revision,
        SessionDeletionPreflight::acquire(directory.path(), &session)
            .unwrap()
            .ownership_revision()
    );
}

#[test]
fn deletion_nested_durable_children_restart_and_workspace_blocker() {
    let (directory, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let child_id = child(directory.path(), &parent, 1, true);
    let child_store = SqliteConversationStore::open(
        child_id.clone(),
        &crate::runtime::subagent::child_conversation_store_path(directory.path(), &child_id),
    )
    .unwrap();
    let grandchild = child(directory.path(), &child_store, 1, false);
    // Unreferenced child storage is not an ownership edge.
    let unrelated = ConversationId::new("unrelated-child");
    let path =
        crate::runtime::subagent::child_conversation_store_path(directory.path(), &unrelated);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    SqliteConversationStore::open(unrelated.clone(), &path)
        .unwrap()
        .initialize(&[])
        .unwrap();
    drop((catalog, parent, child_store));
    let preflight = SessionDeletionPreflight::acquire(directory.path(), &session).unwrap();
    assert_eq!(
        preflight
            .conversations()
            .iter()
            .map(|c| c.conversation_id.clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([node.conversation_id, child_id, grandchild])
    );
    assert_eq!(preflight.workspace_blockers().len(), 1);
    let worktree = &preflight.workspace_blockers()[0]
        .workspace
        .git_worktree()
        .unwrap()
        .physical_worktree_root;
    assert!(
        preflight
            .conversations()
            .iter()
            .all(|c| !worktree.starts_with(&c.private_root))
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("user-work.txt")).unwrap(),
        "preserve"
    );
}

#[test]
fn deletion_unknown_and_missing_lookup_never_creates_state() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        SessionCatalog::open_existing(directory.path())
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    assert!(
        SessionDeletionPreflight::acquire(directory.path(), &SessionId::new("unknown")).is_err()
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    let catalog = SessionCatalog::create(directory.path(), &state()).unwrap();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let database = catalog.database_path(&session, &node.conversation_id);
    std::fs::remove_dir_all(database.parent().unwrap()).unwrap();
    let before = std::fs::read(directory.path().join("sessions/catalog.json")).unwrap();
    assert!(SessionDeletionPreflight::acquire(directory.path(), &session).is_err());
    assert!(!database.parent().unwrap().exists());
    assert_eq!(
        before,
        std::fs::read(directory.path().join("sessions/catalog.json")).unwrap()
    );
    assert!(
        SqliteConversationStore::open_existing(
            ConversationId::new("unknown"),
            &directory.path().join("unknown.sqlite")
        )
        .is_err()
    );
    assert!(!directory.path().join("unknown.sqlite").exists());
}

#[test]
fn deletion_tampered_catalog_and_symlink_escape_fail_closed() {
    let (directory, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let database = catalog.database_path(&session, &node.conversation_id);
    let outside = tempfile::tempdir().unwrap();
    std::fs::rename(database.parent().unwrap(), outside.path().join("lineage")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("lineage"), database.parent().unwrap()).unwrap();
    assert!(SessionDeletionPreflight::acquire(directory.path(), &session).is_err());
    assert!(outside.path().join("lineage/conversation.sqlite").exists());
    std::fs::remove_file(database.parent().unwrap()).unwrap();
    std::fs::rename(outside.path().join("lineage"), database.parent().unwrap()).unwrap();
    let catalog_path = directory.path().join("sessions/catalog.json");
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&catalog_path).unwrap()).unwrap();
    doc["sessions"][session.as_str()]["nodes"][node.id.as_str()]["conversation_id"] =
        serde_json::json!("../../escape");
    std::fs::write(catalog_path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(SessionDeletionPreflight::acquire(directory.path(), &session).is_err());
}

#[test]
fn deletion_live_inspection_blocks_and_stale_marker_is_reusable() {
    let (directory, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let id = child(directory.path(), &parent, 1, false);
    let path = crate::runtime::subagent::child_conversation_inspection_liveness_path(
        directory.path(),
        &id,
    );
    let lease = crate::local_runtime::live_inspection::LiveConversationInspectionLease::acquire(
        directory.path(),
        &id,
    )
    .unwrap();
    assert_eq!(
        SessionDeletionPreflight::acquire(directory.path(), &session)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(lease);
    assert_eq!(
        crate::local_runtime::live_inspection::probe_liveness(&path).unwrap(),
        Some(false)
    );
    drop(SessionDeletionPreflight::acquire(directory.path(), &session).unwrap());
    let _next = crate::local_runtime::live_inspection::LiveConversationInspectionLease::acquire(
        directory.path(),
        &id,
    )
    .unwrap();
}

#[test]
fn deletion_process_writer_gate() {
    use std::io::{Read, Write};
    let Some(root) = std::env::var_os("RUSTX_254_OWNERSHIP_ROOT") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let _authority = crate::runtime::local_storage::ProductController::acquire(root).unwrap();
    let catalog = SessionCatalog::create(root, &state()).unwrap();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let store = store_for(&catalog, &session, &node.conversation_id);
    let identity = crate::runtime::local_storage::ProductRoot::existing(root).unwrap();
    let _access = crate::runtime::local_storage::ConversationAccess::existing(
        &identity,
        catalog
            .database_path(&session, &node.conversation_id)
            .parent()
            .unwrap(),
    )
    .unwrap();
    let child_id = child(root, &store, 1, false);
    let child_store = SqliteConversationStore::open(
        child_id.clone(),
        &crate::runtime::subagent::child_conversation_store_path(root, &child_id),
    )
    .unwrap();
    child(root, &child_store, 1, false);
    let _dirty = if std::env::var_os("RUSTX_254_HOT_JOURNAL").is_some() {
        let connection =
            rusqlite::Connection::open(catalog.database_path(&session, &node.conversation_id))
                .unwrap();
        connection.execute_batch("PRAGMA cache_size=1; BEGIN IMMEDIATE; UPDATE events SET event_json=zeroblob(2000000);").unwrap();
        Some(connection)
    } else {
        None
    };
    println!("OWNERSHIP_COMMITTED");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
}

#[test]
fn deletion_cross_process_parent_death_preserves_nested_ownership() {
    use std::io::{BufRead, BufReader};
    let root = tempfile::tempdir().unwrap();
    let mut process = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "local_runtime::session::tests::deletion_tests::deletion_process_writer_gate",
            "--nocapture",
        ])
        .env("RUSTX_254_OWNERSHIP_ROOT", root.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        if line.trim() == "OWNERSHIP_COMMITTED" {
            break;
        }
    }
    assert!(SessionDeletionPreflight::acquire(root.path(), &SessionId::new("session-1")).is_err());
    process.kill().unwrap();
    process.wait().unwrap();
    let preflight =
        SessionDeletionPreflight::acquire(root.path(), &SessionId::new("session-1")).unwrap();
    assert_eq!(preflight.conversations().len(), 3);
    assert_eq!(
        preflight
            .conversations()
            .iter()
            .filter(|c| c.parent_conversation.is_some())
            .count(),
        2
    );
}

#[test]
fn deletion_duplicate_and_cyclic_child_identity_fail_closed() {
    let (root, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let id = child(root.path(), &parent, 1, false);
    let child_store = SqliteConversationStore::open(
        id.clone(),
        &crate::runtime::subagent::child_conversation_store_path(root.path(), &id),
    )
    .unwrap();
    let subagent = SubagentId::for_conversation(&id, 1);
    let mut event = crate::runtime::subagent::ownership_event(
        &id,
        &subagent,
        &AgentId::new("agent-cycle"),
        &node.conversation_id,
        &ToolCallId::new("cycle"),
        &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        crate::events::types::SubagentOwnershipKind::Normal,
        &WorkspaceSnapshot::shared(root.path().join("external")),
        Utc::now(),
    );
    child_store.append_event(event.clone()).unwrap();
    assert!(SessionDeletionPreflight::acquire(root.path(), &session).is_err());
    // Simulate a tampered durable child reference without following or creating it.
    if let crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
        child_conversation_id,
        ..
    } = &mut event.event
    {
        *child_conversation_id = ConversationId::new("../../outside");
    }
    let path = crate::runtime::subagent::child_conversation_store_path(root.path(), &id);
    drop(child_store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let json: String = connection
        .query_row("SELECT event_json FROM events LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Keep the stored envelope shape and modify its typed payload.
    value["event"] = serde_json::to_value(event.event).unwrap();
    connection
        .execute(
            "UPDATE events SET event_json=?1",
            [serde_json::to_string(&value).unwrap()],
        )
        .unwrap();
    assert!(SessionDeletionPreflight::acquire(root.path(), &session).is_err());
    assert!(!root.path().join("outside").exists());
}

#[test]
fn deletion_hot_journal_reads_are_nonmutating_and_startup_recovers_owned_stores() {
    use std::io::{BufRead, BufReader};
    let root = tempfile::tempdir().unwrap();
    let mut process = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "local_runtime::session::tests::deletion_tests::deletion_process_writer_gate",
            "--nocapture",
        ])
        .env("RUSTX_254_OWNERSHIP_ROOT", root.path())
        .env("RUSTX_254_HOT_JOURNAL", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        if line.trim() == "OWNERSHIP_COMMITTED" {
            break;
        }
    }
    process.kill().unwrap();
    process.wait().unwrap();
    let database = root
        .path()
        .join("sessions/session-1/conversations/conversation-1/conversation.sqlite");
    let journal = database.with_file_name("conversation.sqlite-journal");
    let before = std::fs::read(&database).unwrap();
    let journal_before = std::fs::read(&journal).unwrap();
    assert!(SessionDeletionPreflight::acquire(root.path(), &SessionId::new("session-1")).is_err());
    assert_eq!(before, std::fs::read(&database).unwrap());
    assert_eq!(journal_before, std::fs::read(&journal).unwrap());
    let writer = std::sync::Arc::new(
        crate::runtime::local_storage::ProductController::acquire(root.path()).unwrap(),
    );
    let catalog = SessionCatalog::open_existing(root.path()).unwrap().unwrap();
    catalog.recover_storage(&writer).unwrap();
    drop((catalog, writer));
    let preflight =
        SessionDeletionPreflight::acquire(root.path(), &SessionId::new("session-1")).unwrap();
    assert_eq!(preflight.conversations().len(), 3);
}

#[test]
fn deletion_wal_metadata_is_rejected_before_sqlite_can_create_sidecars() {
    let (root, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let database = catalog.database_path(&session, &node.conversation_id);
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    drop(connection);
    let before = std::fs::read(&database).unwrap();
    let entries = || {
        let mut names: Vec<_> = std::fs::read_dir(database.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before_names = entries();
    assert_eq!(
        before_names,
        [std::ffi::OsString::from("conversation.sqlite")]
    );
    assert!(SessionDeletionPreflight::acquire(root.path(), &session).is_err());
    assert_eq!(before_names, entries());
    assert_eq!(before, std::fs::read(database).unwrap());
}

fn revision(root: &std::path::Path, session: &SessionId) -> [u8; 32] {
    *SessionDeletionPreflight::acquire(root, session)
        .unwrap()
        .ownership_revision()
}

fn activity(store: &SqliteConversationStore, ordinal: u64) {
    let attempt = crate::runtime::identity::AttemptId::new(format!("ordinary-{ordinal}"));
    store
        .append_event(crate::events::types::RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(format!("ordinary-{ordinal}")),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt.clone()),
            turn_id: None,
            timestamp: Utc::now(),
            event: crate::events::types::RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        })
        .unwrap();
}

#[test]
fn deletion_revision_ignores_unrelated_metadata_and_irrelevant_execution_history() {
    let (root, mut catalog, _) = open_catalog();
    let (conversation, target, node) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &target, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let prepared = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(&prepared, SessionNodeOrigin::New)
        .unwrap();
    let other = prepared.session_id.clone();
    let other_store = store_for(&catalog, &other, &prepared.conversation_id);
    let expected = revision(root.path(), &target);
    activity(&other_store, 1);
    other_store
        .append_canonical(&user("unrelated-user", "unrelated text"))
        .unwrap();
    assert_eq!(
        revision(root.path(), &target),
        expected,
        "unrelated ordinary execution and message content are not ownership"
    );
    other_store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("unrelated-assistant"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("unrelated-call"),
                tool_id: ToolId::new("tool-test"),
                name: "test_tool".into(),
                arguments: serde_json::json!({}),
            })],
        }))
        .unwrap();
    other_store
        .append_event(crate::events::types::RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new("unrelated-tool-start"),
            sequence: 0,
            conversation_id: other_store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event: crate::events::types::RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("unrelated-call"),
                tool_id: ToolId::new("tool-test"),
            },
        })
        .unwrap();
    assert_eq!(
        revision(root.path(), &target),
        expected,
        "unrelated assistant/tool activity is not target ownership"
    );
    catalog.rename(&other, "unrelated label").unwrap();
    assert_eq!(
        revision(root.path(), &target),
        expected,
        "unrelated presentation metadata is not target ownership"
    );
    catalog.select(&target, Some(&node)).unwrap();
    assert_eq!(
        revision(root.path(), &target),
        expected,
        "selection does not change membership"
    );
    activity(&store, 2);
    activity(&store, 3);
    assert_eq!(
        revision(root.path(), &target),
        expected,
        "target event timestamps, identities, sequences and history do not change ownership"
    );
    let bytes = std::fs::read(root.path().join("sessions/catalog.json")).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    std::fs::write(
        root.path().join("sessions/catalog.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    assert_eq!(
        revision(root.path(), &target),
        expected,
        "catalog encoding order/whitespace is not semantic authority"
    );
}

#[test]
fn deletion_revision_changes_for_target_nodes_children_and_nested_children() {
    let (root, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &session, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let initial = revision(root.path(), &session);
    let (prepared, _) = catalog
        .prepare_tree_node_at_user_message(
            &session,
            &state(),
            &source,
            &MessageId::new("source-user-a"),
        )
        .unwrap();
    catalog
        .publish_node(&session, &prepared, node, SessionNodeOrigin::New)
        .unwrap();
    let with_node = revision(root.path(), &session);
    assert_ne!(
        initial, with_node,
        "a target node adds a cleanup allocation"
    );
    let id = child(root.path(), &store, 1, false);
    let with_child = revision(root.path(), &session);
    assert_ne!(with_node, with_child, "a durable child adds owned scope");
    let nested_parent = SqliteConversationStore::open(
        id.clone(),
        &crate::runtime::subagent::child_conversation_store_path(root.path(), &id),
    )
    .unwrap();
    child(root.path(), &nested_parent, 1, false);
    assert_ne!(
        with_child,
        revision(root.path(), &session),
        "nested ownership is part of target scope"
    );
}

#[test]
fn deletion_revision_tracks_retained_and_partial_and_complete_disposal() {
    use crate::events::types::SubagentWorkspaceDisposalSettlement;
    let (root, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let initial = revision(root.path(), &session);
    child(root.path(), &parent, 1, true);
    let preflight = SessionDeletionPreflight::acquire(root.path(), &session).unwrap();
    let retained = *preflight.ownership_revision();
    assert_ne!(initial, retained);
    let workspace = &preflight.workspace_blockers()[0].workspace;
    let tree = workspace.git_worktree().unwrap();
    let handoff = crate::runtime::workspace::WorkspaceHandoff {
        logical_workspace: workspace.logical_workspace.clone(),
        physical_worktree_root: tree.physical_worktree_root.clone(),
        branch: tree.branch.clone(),
        base_commit: tree.base_commit.clone(),
        head_commit: tree.base_commit.clone(),
        dirty: true,
    };
    drop(preflight);
    let id = SubagentId::for_conversation(&node.conversation_id, 1);
    parent
        .commit_subagent_workspace_disposal_intent(
            crate::runtime::subagent::workspace_disposal_started_event(
                &node.conversation_id,
                &id,
                &handoff,
                Utc::now(),
            ),
        )
        .unwrap();
    assert_eq!(
        revision(root.path(), &session),
        retained,
        "intent alone removes no blocker or allocation"
    );
    parent
        .commit_subagent_workspace_disposal_settlement(
            crate::runtime::subagent::workspace_disposal_settled_event(
                &node.conversation_id,
                &id,
                &handoff,
                SubagentWorkspaceDisposalSettlement::WorktreeRemoved,
                Utc::now(),
            ),
        )
        .unwrap();
    let branch_only = revision(root.path(), &session);
    assert_ne!(
        retained, branch_only,
        "partial disposal leaves only a branch blocker"
    );
    parent
        .commit_subagent_workspace_disposal_settlement(
            crate::runtime::subagent::workspace_disposal_settled_event(
                &node.conversation_id,
                &id,
                &handoff,
                SubagentWorkspaceDisposalSettlement::Disposed,
                Utc::now(),
            ),
        )
        .unwrap();
    let disposed = SessionDeletionPreflight::acquire(root.path(), &session).unwrap();
    assert!(disposed.workspace_blockers().is_empty());
    assert_ne!(
        &branch_only,
        disposed.ownership_revision(),
        "complete disposal clears the blocker"
    );
}

#[test]
fn deletion_target_child_access_blocks_but_unrelated_child_access_does_not() {
    use crate::runtime::local_storage::{ConversationAccess, ProductRoot};
    let (root, mut catalog, _) = open_catalog();
    let (conversation, target, _) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &target, &conversation);
    let target_child = child(root.path(), &store, 1, false);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let other = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(&other, SessionNodeOrigin::New)
        .unwrap();
    let other_store = store_for(&catalog, &other.session_id, &other.conversation_id);
    let unrelated = child(root.path(), &other_store, 1, false);
    let identity = ProductRoot::existing(root.path()).unwrap();
    let _unrelated = ConversationAccess::existing(
        &identity,
        crate::runtime::subagent::child_conversation_store_path(identity.root(), &unrelated)
            .parent()
            .unwrap(),
    )
    .unwrap();
    assert!(SessionDeletionPreflight::acquire(root.path(), &target).is_ok());
    let target_access = ConversationAccess::existing(
        &identity,
        crate::runtime::subagent::child_conversation_store_path(identity.root(), &target_child)
            .parent()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        SessionDeletionPreflight::acquire(root.path(), &target)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(target_access);
    assert!(SessionDeletionPreflight::acquire(root.path(), &target).is_ok());
}

#[test]
fn deletion_snapshot_freezes_native_ownership_commits_but_not_ordinary_events() {
    use crate::runtime::local_storage::{ConversationAccess, ProductRoot};
    let (root, mut catalog, _) = open_catalog();
    let (conversation, target, _) = append_history(&catalog, &source_history());
    let source_store = store_for(&catalog, &target, &conversation);
    let source = lineage_at(
        &source_store,
        &conversation,
        source_store.load_head().unwrap().revision,
    );
    let other = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(&other, SessionNodeOrigin::New)
        .unwrap();
    let identity = ProductRoot::existing(root.path()).unwrap();
    let access = std::sync::Arc::new(
        ConversationAccess::existing(
            &identity,
            catalog
                .database_path(&other.session_id, &other.conversation_id)
                .parent()
                .unwrap(),
        )
        .unwrap(),
    );
    let store =
        store_for(&catalog, &other.session_id, &other.conversation_id).with_lifecycle(access);
    let snapshot = SessionDeletionPreflight::acquire(root.path(), &target).unwrap();
    activity(&store, 42);
    let id = SubagentId::for_conversation(&other.conversation_id, 1);
    let event = crate::runtime::subagent::ownership_event(
        &other.conversation_id,
        &id,
        &AgentId::new("agent"),
        &ConversationId::new(id.as_str()),
        &ToolCallId::new("call"),
        &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        crate::events::types::SubagentOwnershipKind::Normal,
        &WorkspaceSnapshot::shared(root.path().join("external")),
        Utc::now(),
    );
    assert!(
        store.append_event(event.clone()).is_err(),
        "no graph transition can race the retained ownership freeze"
    );
    assert!(
        catalog
            .rename(&other.session_id, "blocked publication")
            .is_err()
    );
    drop(snapshot);
    store.append_event(event).unwrap();
}

#[test]
fn deletion_cross_session_child_claim_is_ambiguous_not_a_revision_change() {
    let (root, mut catalog, _) = open_catalog();
    let (conversation, target, _) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &target, &conversation);
    let owned_child = child(root.path(), &store, 1, false);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let other = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(&other, SessionNodeOrigin::New)
        .unwrap();
    let other_store = store_for(&catalog, &other.session_id, &other.conversation_id);
    let id = SubagentId::for_conversation(&other.conversation_id, 1);
    let event = crate::runtime::subagent::ownership_event(
        &other.conversation_id,
        &id,
        &AgentId::new("agent"),
        &owned_child,
        &ToolCallId::new("call"),
        &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        crate::events::types::SubagentOwnershipKind::Normal,
        &WorkspaceSnapshot::shared(root.path().join("external")),
        Utc::now(),
    );
    other_store.append_event(event).unwrap();
    assert!(
        SessionDeletionPreflight::acquire(root.path(), &target).is_err(),
        "a foreign ownership claim must fail closed, not merely yield a different token"
    );
}

#[test]
fn deletion_detached_private_stores_and_writers_retain_target_access() {
    use crate::runtime::local_storage::{ConversationAccess, ProductRoot};
    let (root, catalog, _) = open_catalog();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let allocation = catalog
        .database_path(&session, &node.conversation_id)
        .parent()
        .unwrap()
        .to_path_buf();
    let identity = ProductRoot::existing(root.path()).unwrap();
    let access = std::sync::Arc::new(ConversationAccess::existing(&identity, &allocation).unwrap());
    let artifacts =
        crate::tools::artifacts::ArtifactStore::new(node.conversation_id.clone(), &allocation)
            .unwrap()
            .with_lifecycle(Some(access.clone()));
    let output = crate::tools::managed_output::ManagedToolOutput::new(
        node.conversation_id.clone(),
        allocation.join("tool-output"),
    )
    .unwrap()
    .with_lifecycle(Some(access.clone()));
    let writer = artifacts
        .open_writer(&artifacts.create_artifact().unwrap())
        .unwrap();
    drop(access);
    drop(artifacts);
    assert!(SessionDeletionPreflight::acquire(root.path(), &session).is_err());
    drop(output);
    assert!(
        SessionDeletionPreflight::acquire(root.path(), &session).is_err(),
        "the streaming writer independently retains target access"
    );
    drop(writer);
    assert!(SessionDeletionPreflight::acquire(root.path(), &session).is_ok());
}

#[test]
fn deletion_target_process_gate() {
    use crate::runtime::local_storage::{ConversationAccess, ProductRoot};
    use std::io::{Read, Write};
    let Some(root) = std::env::var_os("RUSTX_260_TARGET_ROOT") else {
        return;
    };
    let root = ProductRoot::existing(std::path::Path::new(&root)).unwrap();
    let _owner: Box<dyn std::any::Any> = if let Ok(id) = std::env::var("RUSTX_260_TARGET_CHILD") {
        let database = crate::runtime::subagent::child_conversation_store_path(
            root.root(),
            &ConversationId::new(id),
        );
        Box::new(ConversationAccess::existing(&root, database.parent().unwrap()).unwrap())
    } else {
        Box::new(
            SessionDeletionPreflight::acquire(
                root.root(),
                &SessionId::new(std::env::var("RUSTX_260_TARGET_SESSION").unwrap()),
            )
            .unwrap(),
        )
    };
    println!("TARGET_ACQUIRED");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
}

#[test]
fn deletion_cross_process_target_child_unrelated_child_and_destructive_conflict() {
    use std::io::{BufRead, BufReader};
    let (root, mut catalog, _) = open_catalog();
    let (conversation, target, _) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &target, &conversation);
    let target_child = child(root.path(), &store, 1, false);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let other = catalog.prepare_clone_session(&state(), &source).unwrap();
    catalog
        .publish_session(&other, SessionNodeOrigin::New)
        .unwrap();
    let other_store = store_for(&catalog, &other.session_id, &other.conversation_id);
    let unrelated_child = child(root.path(), &other_store, 1, false);
    for (key, value, blocks) in [
        ("RUSTX_260_TARGET_CHILD", unrelated_child.as_str(), false),
        ("RUSTX_260_TARGET_CHILD", target_child.as_str(), true),
        ("RUSTX_260_TARGET_SESSION", target.as_str(), true),
    ] {
        let mut process = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_runtime::session::tests::deletion_tests::deletion_target_process_gate",
                "--nocapture",
            ])
            .env("RUSTX_260_TARGET_ROOT", root.path())
            .env(key, value)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "target owner exited before gate"
            );
            if line.trim() == "TARGET_ACQUIRED" {
                break;
            }
        }
        process.stdout = Some(output.into_inner());
        let result = SessionDeletionPreflight::acquire(root.path(), &target);
        if blocks {
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        } else {
            assert_eq!(
                result.unwrap().conversations().len(),
                2,
                "unrelated live child does not block complete B scope"
            );
        }
        process.kill().unwrap();
        process.wait().unwrap();
        assert!(
            SessionDeletionPreflight::acquire(root.path(), &target).is_ok(),
            "kernel authority is released after process death"
        );
    }
}

#[test]
fn deletion_alias_startup_authors_one_canonical_session_allocation() {
    use crate::runtime::local_storage::{ConversationAccess, ProductController, ProductRoot};
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("product");
    std::fs::create_dir(&real).unwrap();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let controller = ProductController::acquire(&alias).unwrap();
    let identity = ProductRoot::existing(&real).unwrap();
    assert_eq!(controller.root(), identity.root());
    assert_eq!(
        ProductController::acquire(&real).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let catalog = SessionCatalog::create(&alias, &state()).unwrap();
    assert_eq!(catalog.root, identity.root().join("sessions"));
    let (session, node, _) = catalog.active_lineage().unwrap();
    let database = catalog.database_path(&session, &node.conversation_id);
    assert!(database.starts_with(identity.root()));
    let access = ConversationAccess::existing(&identity, database.parent().unwrap()).unwrap();
    assert_eq!(
        SessionDeletionPreflight::acquire(&alias, &session)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(access);
    let preflight = SessionDeletionPreflight::acquire(&alias, &session).unwrap();
    assert_eq!(preflight.conversations()[0].database, database);
    assert!(
        ConversationAccess::existing(
            &ProductRoot::existing(&alias).unwrap(),
            database.parent().unwrap()
        )
        .is_err()
    );
    let revision = *preflight.ownership_revision();
    drop(preflight);
    assert_eq!(
        *SessionDeletionPreflight::acquire(&real, &session)
            .unwrap()
            .ownership_revision(),
        revision
    );
    drop(controller);
    assert!(ProductController::acquire(&real).is_ok());
}
