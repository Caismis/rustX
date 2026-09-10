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
        c.private_root
            .starts_with(directory.path().join("sessions").join(session.as_str()))
    }));
    assert!(crate::runtime::local_storage::LocalStorageGuard::writer(directory.path()).is_err());
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
        &path,
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
        &path,
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
    let _authority = crate::runtime::local_storage::LocalStorageGuard::writer(root).unwrap();
    let catalog = SessionCatalog::create(root, &state()).unwrap();
    let (session, node, _) = catalog.active_lineage().unwrap();
    let store = store_for(&catalog, &session, &node.conversation_id);
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
        crate::runtime::local_storage::LocalStorageGuard::writer(root.path()).unwrap(),
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
