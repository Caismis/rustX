use super::*;
use crate::local_runtime::session::LineageSeed;
use crate::local_runtime::session::deletion::*;
use crate::runtime::local_storage::{ConversationAccess, ProductController, ProductRoot};
use crate::runtime_client::types::{RequestId, RuntimeClientRequest};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

fn fixture() -> (TempDir, SessionCatalog, SessionDeletePreview) {
    let (dir, mut catalog, _) = open_catalog();
    let target = catalog.active_snapshot().unwrap();
    let store = store_for(&catalog, &target.id, &target.active_conversation_id);
    let child_id = child(dir.path(), &store, 1, false);
    let socket =
        crate::runtime::subagent::child_conversation_inspection_socket_path(dir.path(), &child_id);
    drop(std::os::unix::net::UnixListener::bind(socket).unwrap());
    drop(store);
    let next = catalog.prepare_session(&state(), &[]).unwrap();
    catalog
        .publish_session(&next, SessionNodeOrigin::New)
        .unwrap();
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&target.id) else {
        panic!("preview");
    };
    (dir, catalog, preview)
}

fn residue(root: &Path, preview: &SessionDeletePreview) -> Vec<PathBuf> {
    preview
        .scopes
        .iter()
        .map(|s| match s {
            DeletionScope::Node {
                conversation_id, ..
            } => root
                .join("sessions")
                .join(preview.session_id.as_str())
                .join("conversations")
                .join(conversation_id.as_str()),
            DeletionScope::Child {
                conversation_id, ..
            } => root.join("subagents").join(conversation_id.as_str()),
        })
        .collect()
}

#[test]
fn deletion_preview_releases_guards_execute_reacquires_and_rejects_stale() {
    let (dir, mut catalog, preview) = fixture();
    let root = ProductRoot::existing(dir.path()).unwrap();
    let paths = residue(root.root(), &preview);
    let access = ConversationAccess::existing(&root, &paths[0]).unwrap();
    assert!(matches!(
        catalog
            .commit_delete(&preview.session_id, &preview.target_revision)
            .unwrap(),
        Err(SessionDeleteResult::Blocked {
            reason: DeletionBlocker::InUse,
            ..
        })
    ));
    drop(access);
    let original = catalog.document.sessions[&preview.session_id].clone();
    let node = original.nodes.values().next().unwrap();
    let store = store_for(&catalog, &preview.session_id, &node.conversation_id);
    child(dir.path(), &store, 2, false);
    drop(store);
    assert!(matches!(
        catalog
            .commit_delete(&preview.session_id, &preview.target_revision)
            .unwrap(),
        Err(SessionDeleteResult::Stale { .. })
    ));
    assert!(paths.iter().all(|p| p.exists()));
    let current = catalog.active_snapshot().unwrap().id;
    assert!(matches!(
        catalog.delete_preview(&current),
        SessionDeleteResult::Blocked {
            reason: DeletionBlocker::CurrentSession,
            ..
        }
    ));
}

#[test]
fn deletion_before_visibility_preserves_every_source_and_live_authority() {
    let (dir, mut catalog, preview) = fixture();
    let before = fs::read(&catalog.path).unwrap();
    catalog.arm_write_fault_before_rename();
    assert!(matches!(
        catalog.commit_delete(&preview.session_id, &preview.target_revision),
        Err(SessionError::CatalogCommit {
            error: CatalogCommitError::NotCommitted { .. }
        })
    ));
    assert_eq!(before, fs::read(&catalog.path).unwrap());
    assert!(
        residue(dir.path(), &preview)
            .iter()
            .all(|p| p.join("conversation.sqlite").exists())
    );
    assert!(catalog.snapshot(&preview.session_id).is_ok());
    assert!(catalog.document.deletions.is_empty());
}

#[test]
fn deletion_uncertain_visibility_never_cleans_and_recovery_confirms_same_record() {
    let (dir, mut catalog, preview) = fixture();
    catalog.arm_write_fault_after_rename();
    let Err(SessionDeleteResult::CommittedDurabilityUncertain { record, .. }) = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
    else {
        panic!("uncertain outcome");
    };
    assert!(residue(dir.path(), &preview).iter().all(|p| p.exists()));
    assert!(catalog.snapshot(&preview.session_id).is_err());
    drop(catalog);
    let mut catalog = reopen_catalog(dir.path());
    assert_eq!(catalog.document.deletions[&preview.session_id], record);
    // A failed recovery publication cannot confer cleanup authority either.
    catalog.arm_write_fault_before_rename();
    assert!(matches!(
        catalog.recover_delete(&preview.session_id),
        Err(SessionDeleteResult::CommittedDurabilityUncertain { .. })
    ));
    assert!(residue(dir.path(), &preview).iter().all(|p| p.exists()));
    assert!(matches!(
        catalog.recover_deletions().as_slice(),
        [SessionDeleteResult::Deleted { .. }]
    ));
    assert!(catalog.recover_deletions().is_empty());
}

#[test]
fn deletion_cleanup_owns_frozen_plan_without_root_guard_and_identity_is_absorbing() {
    let (dir, mut catalog, preview) = fixture();
    let root = ProductRoot::existing(dir.path()).unwrap();
    let paths = residue(dir.path(), &preview);
    let work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    assert_eq!(work.record.phase, DeletionPhase::CleanupPending);
    assert!(matches!(
        catalog
            .commit_delete(&preview.session_id, "ignored")
            .unwrap(),
        Err(SessionDeleteResult::CommittedCleanupPending { .. })
    ));
    assert!(catalog.snapshot(&preview.session_id).is_err());
    for path in &paths {
        assert!(ConversationAccess::existing(&root, path).is_err());
    }
    // A conflicting root snapshot can be acquired AND held for the entire
    // recursive cleanup. This proves no cleanup operation needs a global guard.
    let freeze = root.freeze_ownership().unwrap();
    let cleaned = work.run();
    assert!(cleaned.is_ok());
    drop(freeze);
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        SessionDeleteResult::Deleted { .. }
    ));
    assert!(paths.iter().all(|p| !p.exists()));
    for scope in &preview.scopes {
        if let DeletionScope::Child {
            conversation_id, ..
        } = scope
        {
            assert!(
                !crate::runtime::subagent::child_conversation_inspection_socket_path(
                    dir.path(),
                    conversation_id
                )
                .exists()
            );
        }
    }
    assert!(work.run().is_ok());
    assert!(catalog.recover_deletions().is_empty());
    let DeletionScope::Node {
        node_id,
        conversation_id,
    } = preview
        .scopes
        .iter()
        .find(|s| matches!(s, DeletionScope::Node { .. }))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(
        catalog
            .prepare_session_with_ids(
                &state(),
                preview.session_id.clone(),
                node_id.clone(),
                conversation_id.clone(),
                &LineageSeed::history(vec![])
            )
            .is_err()
    );
    assert!(paths.iter().all(|p| !p.exists()));
    assert!(catalog.active_snapshot().is_ok());
}

#[test]
fn deletion_cleanup_failure_and_final_publication_faults_remain_retryable() {
    let (dir, mut catalog, preview) = fixture();
    let work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let failed = catalog.finish_delete(
        &work.record,
        Err(std::io::Error::other("injected cleanup failure")),
    );
    assert!(matches!(
        failed,
        SessionDeleteResult::CommittedCleanupPending { .. }
    ));
    assert!(residue(dir.path(), &preview).iter().all(|p| p.exists()));
    let result = work.run();
    catalog.arm_write_fault_before_rename();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::CommittedCleanupPending { .. }
    ));
    let work = catalog.recover_delete(&preview.session_id).unwrap();
    let result = work.run();
    catalog.arm_write_fault_after_rename();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::CommittedDurabilityUncertain {
            record: DeletionRecord {
                phase: DeletionPhase::Deleted,
                ..
            },
            ..
        }
    ));
    let mut reopened = reopen_catalog(dir.path());
    assert!(matches!(
        reopened.recover_delete(&preview.session_id),
        Err(SessionDeleteResult::Deleted { .. })
    ));
}

#[test]
fn deletion_process_child() {
    let Some(root) = std::env::var_os("RUSTX_255_ROOT") else {
        return;
    };
    let controller = Arc::new(ProductController::acquire(Path::new(&root)).unwrap());
    let mut catalog = reopen_catalog(controller.root());
    catalog.retain_lifecycle(controller);
    let id = SessionId::new("session-1");
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&id) else {
        panic!("preview");
    };
    let work = catalog
        .commit_delete(&id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let result = work.run();
    catalog.finish_delete(&work.record, result);
}

#[test]
fn deletion_process_death_after_commit_and_mid_cleanup_converges_without_discovery() {
    for boundary in ["logical_commit", "cleanup_item"] {
        let (dir, catalog, preview) = fixture();
        drop(catalog);
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_runtime::session::tests::deletion_tests::lifecycle::deletion_process_child",
                "--nocapture",
            ])
            .env("RUSTX_255_ROOT", dir.path())
            .env("RUSTX_255_GATE", boundary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "child exited before boundary"
            );
            if line.trim() == format!("DELETE_GATE:{boundary}") {
                break;
            }
        }
        process.kill().unwrap();
        process.wait().unwrap();
        let mut catalog = reopen_catalog(dir.path());
        assert!(catalog.snapshot(&preview.session_id).is_err());
        assert_eq!(
            catalog.document.deletions[&preview.session_id].scopes,
            preview.scopes
        );
        let paths = residue(dir.path(), &preview);
        assert_eq!(
            paths.iter().filter(|p| p.exists()).count(),
            if boundary == "logical_commit" { 2 } else { 1 }
        );
        // Corrupt unrelated live storage: ownership discovery now fails. The
        // frozen cleanup must still complete without opening any live store.
        let active = catalog.active_snapshot().unwrap();
        fs::write(
            catalog.database_path(&active.id, &active.active_conversation_id),
            b"not sqlite",
        )
        .unwrap();
        assert!(matches!(
            catalog.recover_deletions().as_slice(),
            [SessionDeleteResult::Deleted { .. }]
        ));
        assert!(paths.iter().all(|p| !p.exists()));
        assert!(catalog.recover_deletions().is_empty());
        assert!(reopen_catalog(dir.path()).recover_deletions().is_empty());
    }
}

#[tokio::test]
async fn deletion_recursive_worker_releases_supervisor_mutex_and_never_finishes_early() {
    let (_dir, mut catalog, preview) = fixture();
    let work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    crate::local_runtime::supervisor::assert_deletion_cleanup_releases_catalog(
        catalog,
        work,
        state().model,
    )
    .await;
}

#[test]
fn deletion_rust_types_roundtrip_every_shared_sdk_result_and_reject_paths() {
    let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/sdk/runtime-client/test/deletion-fixtures.json"
    )))
    .unwrap();
    let mut statuses = BTreeSet::new();
    for fixture in fixtures {
        statuses.insert(fixture["status"].as_str().unwrap().to_owned());
        let result: SessionDeleteResult = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(result).unwrap(), fixture);
    }
    assert_eq!(statuses.len(), 7);
    for request in [
        RuntimeClientRequest::SessionDeletePreview {
            id: RequestId::new(1),
            session_id: "session-1".into(),
        },
        RuntimeClientRequest::SessionDelete {
            id: RequestId::new(2),
            session_id: "session-1".into(),
            expected_target_revision: "a".repeat(64),
        },
        RuntimeClientRequest::SessionDeleteRecover {
            id: RequestId::new(3),
            session_id: "session-1".into(),
        },
    ] {
        assert!(request.requires_async());
        assert!(request.session_request().is_some());
        assert_eq!(
            request.is_mutating(),
            request.method() != "session_delete_preview"
        );
        let mut value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<RuntimeClientRequest>(value.clone()).unwrap(),
            request
        );
        value["path"] = serde_json::json!("/untrusted");
        assert!(serde_json::from_value::<RuntimeClientRequest>(value).is_err());
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One source/cascade/survivor end-to-end contract.
fn deletion_tree_and_children_cascade_but_fork_clone_and_external_resources_survive() {
    let (dir, mut catalog, _) = open_catalog();
    let (conversation, session, node) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &session, &conversation);
    let revision = store.load_head().unwrap().revision;
    let source = lineage_at(&store, &conversation, revision);
    child(dir.path(), &store, 1, false);
    let (tree, _) = catalog
        .prepare_tree_node_at_user_message(
            &session,
            &state(),
            &source,
            &MessageId::new("source-user-a"),
        )
        .unwrap();
    catalog
        .publish_node(&session, &tree, node.clone(), SessionNodeOrigin::New)
        .unwrap();
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
    drop(store);
    let mut survivors = Vec::new();
    for prepared in [&clone, &fork] {
        let store = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
        survivors.push(store.load_head().unwrap());
    }
    for class in [
        "environments",
        "cache",
        "config",
        "credentials",
        "external-project",
    ] {
        fs::create_dir_all(dir.path().join(class)).unwrap();
        fs::write(dir.path().join(class).join("keep"), b"preserve").unwrap();
    }
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&session) else {
        panic!("preview");
    };
    assert_eq!(preview.scopes.len(), 3);
    let work = catalog
        .commit_delete(&session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let result = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::Deleted { .. }
    ));
    for (prepared, before) in [&clone, &fork].into_iter().zip(survivors) {
        catalog.select(&prepared.session_id, None).unwrap();
        let store = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
        assert_eq!(store.load_head().unwrap(), before);
        let message = UserMessageBlock {
            id: MessageId::new(format!("later-user-{}", prepared.session_id)),
            content: vec![text("work after source deletion")],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        };
        store
            .append_canonical(&MessageBlock::User(message.clone()))
            .unwrap();
        assert!(
            store
                .load_canonical()
                .unwrap()
                .contains(&MessageBlock::User(message))
        );
        let head = store.load_head().unwrap();
        let lineage = lineage_at(&store, &prepared.conversation_id, head.revision);
        let later = catalog.prepare_clone_session(&state(), &lineage).unwrap();
        catalog
            .publish_session(&later, SessionNodeOrigin::New)
            .unwrap();
    }
    for class in [
        "environments",
        "cache",
        "config",
        "credentials",
        "external-project",
    ] {
        assert_eq!(
            fs::read(dir.path().join(class).join("keep")).unwrap(),
            b"preserve"
        );
    }
}

#[test]
fn deletion_stale_catalog_cannot_resurrect_committed_identity() {
    let (dir, mut catalog, preview) = fixture();
    let mut stale = catalog.clone();
    let work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    assert!(stale.rename(&preview.session_id, "resurrect").is_err());
    assert!(
        reopen_catalog(dir.path())
            .snapshot(&preview.session_id)
            .is_err()
    );
    let result = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::Deleted { .. }
    ));
}

#[test]
fn deletion_cleanup_path_failure_preserves_external_data_and_retries_same_work() {
    let (dir, mut catalog, preview) = fixture();
    let work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let path = residue(dir.path(), &preview).remove(0);
    let saved = path.with_extension("saved");
    fs::rename(&path, &saved).unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("keep"), b"safe").unwrap();
    std::os::unix::fs::symlink(external.path(), &path).unwrap();
    let result = work.run();
    assert!(result.is_err());
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::CommittedCleanupPending { .. }
    ));
    // The bad deletion residue must not prevent access to the active Session.
    let active = catalog.active_snapshot().unwrap();
    let active_path = catalog.database_path(&active.id, &active.active_conversation_id);
    let root = ProductRoot::existing(dir.path()).unwrap();
    assert!(ConversationAccess::existing(&root, active_path.parent().unwrap()).is_ok());
    assert_eq!(fs::read(external.path().join("keep")).unwrap(), b"safe");
    fs::remove_file(&path).unwrap();
    fs::rename(saved, path).unwrap();
    assert!(matches!(
        catalog.recover_deletions().as_slice(),
        [SessionDeleteResult::Deleted { .. }]
    ));
}

#[test]
fn deletion_workspace_transition_is_stale_and_preview_blockers_are_typed() {
    let (dir, mut catalog, preview) = fixture();
    let node = catalog.document.sessions[&preview.session_id]
        .nodes
        .values()
        .next()
        .unwrap();
    let store = store_for(&catalog, &preview.session_id, &node.conversation_id);
    child(dir.path(), &store, 2, true);
    drop(store);
    assert!(matches!(
        catalog.delete_preview(&preview.session_id),
        SessionDeleteResult::Blocked {
            reason: DeletionBlocker::Workspace { .. },
            ..
        }
    ));
    assert!(matches!(
        catalog
            .commit_delete(&preview.session_id, &preview.target_revision)
            .unwrap(),
        Err(SessionDeleteResult::Stale { .. })
    ));
    assert!(
        dir.path()
            .join("workspaces/worktrees/retained/user-work.txt")
            .exists()
    );
    assert!(catalog.document.deletions.is_empty());
    let node = catalog.document.sessions[&preview.session_id]
        .nodes
        .values()
        .next()
        .unwrap();
    fs::write(
        catalog.database_path(&preview.session_id, &node.conversation_id),
        b"corrupt",
    )
    .unwrap();
    assert!(matches!(
        catalog.delete_preview(&preview.session_id),
        SessionDeleteResult::Blocked {
            reason: DeletionBlocker::InvalidOwnership { .. },
            ..
        }
    ));
    assert!(matches!(
        catalog.delete_preview(&SessionId::new("unknown")),
        SessionDeleteResult::NotFound { .. }
    ));
}

#[test]
fn deletion_identity_reservation_rejects_cross_allocation_restore() {
    let (dir, mut catalog, preview) = fixture();
    let work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let result = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::Deleted { .. }
    ));
    let root = ProductRoot::existing(dir.path()).unwrap();
    for scope in &preview.scopes {
        let (conversation, alternate) = match scope {
            DeletionScope::Node {
                conversation_id, ..
            } => (
                conversation_id,
                root.root().join("subagents").join(conversation_id.as_str()),
            ),
            DeletionScope::Child {
                conversation_id, ..
            } => (
                conversation_id,
                root.root()
                    .join("sessions/new-session/conversations")
                    .join(conversation_id.as_str()),
            ),
        };
        fs::create_dir_all(&alternate).unwrap();
        assert!(
            ConversationAccess::existing(&root, &alternate).is_err(),
            "reserved {conversation}"
        );
    }
}

#[test]
fn deletion_duplicate_recovery_cannot_regress_terminal_or_mint_a_second_snapshot() {
    let (_dir, mut catalog, preview) = fixture();
    let first = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let duplicate = catalog.recover_delete(&preview.session_id).unwrap();
    assert_eq!(first.record, duplicate.record);
    let cleaned = first.run();
    assert!(matches!(
        catalog.finish_delete(&first.record, cleaned),
        SessionDeleteResult::Deleted { .. }
    ));
    assert!(matches!(
        catalog.finish_delete(
            &duplicate.record,
            Err(std::io::Error::other("delayed worker failure"))
        ),
        SessionDeleteResult::Deleted { .. }
    ));
    assert_eq!(catalog.document.deletions.len(), 1);
    assert!(matches!(
        catalog
            .commit_delete(&preview.session_id, "old token")
            .unwrap(),
        Err(SessionDeleteResult::Deleted { .. })
    ));
}

#[test]
fn deletion_live_ownership_cannot_claim_a_pending_frozen_child() {
    let (dir, mut catalog, preview) = fixture();
    let source = catalog.snapshot(&preview.session_id).unwrap();
    let store = store_for(&catalog, &source.id, &source.active_conversation_id);
    let mut event = store
        .read_events(None, 256)
        .unwrap()
        .events
        .into_iter()
        .find(|event| {
            matches!(
                event.event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
            )
        })
        .unwrap();
    drop(store);
    let _work = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let active = catalog.active_snapshot().unwrap();
    event.conversation_id = active.active_conversation_id.clone();
    event.sequence = 0;
    let store = store_for(&catalog, &active.id, &active.active_conversation_id);
    store.append_event(event).unwrap();
    drop(store);
    let error = SessionDeletionPreflight::acquire(dir.path(), &active.id).unwrap_err();
    assert!(error.to_string().contains("deleted Conversation identity"));
    assert_eq!(
        catalog.document.deletions[&preview.session_id].scopes,
        preview.scopes
    );
}
