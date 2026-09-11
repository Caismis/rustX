use super::*;
use crate::local_runtime::session::deletion::*;
use crate::local_runtime::session::{LineageSeed, SessionNodeId};
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
    let Err(SessionDeleteResult::CommittedDurabilityUncertain { .. }) = catalog
        .commit_delete(&preview.session_id, &preview.target_revision)
        .unwrap()
    else {
        panic!("uncertain outcome");
    };
    assert!(residue(dir.path(), &preview).iter().all(|p| p.exists()));
    assert!(catalog.snapshot(&preview.session_id).is_err());
    let record = catalog.document.deletions[&preview.session_id].clone();
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
    assert_completed_absent(dir.path(), &preview.session_id);
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
    assert_eq!(
        catalog.document.deletions.get(&preview.session_id),
        Some(&work.record)
    );
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
    assert_eq!(
        reopen_catalog(dir.path()).document.deletions[&preview.session_id],
        work.record
    );
    let result = work.run();
    catalog.arm_write_fault_before_rename();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::CommittedCleanupPending { .. }
    ));
    assert_eq!(
        reopen_catalog(dir.path()).document.deletions[&preview.session_id],
        work.record
    );
    let work = catalog.recover_delete(&preview.session_id).unwrap();
    let result = work.run();
    catalog.arm_write_fault_after_rename();
    assert!(matches!(
        catalog.finish_delete(&work.record, result),
        SessionDeleteResult::CommittedDurabilityUncertain { .. }
    ));
    assert_completed_absent(dir.path(), &preview.session_id);
    let mut reopened = reopen_catalog(dir.path());
    assert!(matches!(
        reopened.recover_delete(&preview.session_id),
        Err(SessionDeleteResult::NotFound { .. })
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
        assert_completed_absent(dir.path(), &preview.session_id);
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
fn deletion_rust_types_roundtrip_every_shared_protocol_result_and_reject_paths() {
    let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tui/test/deletion-fixtures.json"
    )))
    .unwrap();
    let mut statuses = BTreeSet::new();
    for fixture in fixtures {
        statuses.insert(fixture["status"].as_str().unwrap().to_owned());
        let result: crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult =
            serde_json::from_value(fixture.clone()).unwrap();
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
    assert!(
        stale
            .rename(&preview.session_id, "resurrect after removal")
            .is_err()
    );
    assert_completed_absent(dir.path(), &preview.session_id);
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
fn deletion_retired_native_domains_reject_stale_allocation_access() {
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
            "retired native domain {conversation}"
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
        SessionDeleteResult::NotFound { .. }
    ));
    assert!(catalog.document.deletions.is_empty());
    assert!(matches!(
        catalog
            .commit_delete(&preview.session_id, "old token")
            .unwrap(),
        Err(SessionDeleteResult::NotFound { .. })
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

fn assert_completed_absent(root: &Path, id: &SessionId) {
    let disk: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("sessions/catalog.json")).unwrap()).unwrap();
    assert!(disk["deletions"].as_object().unwrap().is_empty());
    assert!(disk["sessions"].get(id.as_str()).is_none());
    let mut reopened = reopen_catalog(root);
    assert!(reopened.recover_deletions().is_empty());
    assert!(matches!(
        reopened.delete_preview(id),
        SessionDeleteResult::NotFound { .. }
    ));
}

#[test]
fn deletion_completed_worksets_do_not_accumulate_and_native_allocators_advance() {
    let (dir, mut catalog, _) = open_catalog();
    for n in 1..=6 {
        let target = catalog.active_snapshot().unwrap();
        assert_eq!(target.id.as_str(), format!("session-{n}"));
        assert_eq!(
            target.active_conversation_id.as_str(),
            format!("conversation-{n}")
        );
        let next = catalog.prepare_session(&state(), &[]).unwrap();
        assert_eq!(next.node_id.as_str(), format!("node-{}", n + 1));
        catalog
            .publish_session(&next, SessionNodeOrigin::New)
            .unwrap();
        let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&target.id) else {
            panic!("preview")
        };
        let work = catalog
            .commit_delete(&target.id, &preview.target_revision)
            .unwrap()
            .unwrap();
        let disk = reopen_catalog(dir.path());
        assert_eq!(disk.document.deletions.len(), 1);
        assert_eq!(disk.document.deletions[&target.id], work.record);
        let cleaned = work.run();
        assert!(matches!(
            catalog.finish_delete(&work.record, cleaned),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_completed_absent(dir.path(), &target.id);
        catalog = reopen_catalog(dir.path());
    }
}

#[test]
fn catalog_generation_rejects_stale_metadata_and_planned_publication() {
    let (dir, mut catalog, _) = open_catalog();
    let target = catalog.active_snapshot().unwrap();
    let mut stale = catalog.clone();
    let old_plan = catalog.document.clone();
    catalog
        .rename(&target.id, "new authoritative name")
        .unwrap();
    let bytes = fs::read(&catalog.path).unwrap();
    assert!(
        stale
            .rename(&target.id, "lost update")
            .unwrap_err()
            .to_string()
            .contains("stale catalog generation")
    );
    assert!(
        catalog
            .commit(old_plan)
            .unwrap_err()
            .to_string()
            .contains("stale catalog generation")
    );
    assert_eq!(fs::read(&catalog.path).unwrap(), bytes);
    assert_eq!(
        reopen_catalog(dir.path())
            .active_snapshot()
            .unwrap()
            .name
            .as_deref(),
        Some("new authoritative name")
    );
}

#[test]
fn deletion_protocol_projection_is_bounded_for_large_frozen_graphs() {
    use crate::local_runtime::supervisor::project_session_deletion;
    let mut lengths = Vec::new();
    for size in [1, 10, 10_000] {
        let scopes: Vec<_> = (0..size)
            .map(|i| DeletionScope::Child {
                conversation_id: ConversationId::new(format!("conversation-1-subagent-{i}")),
                parent_conversation: ConversationId::new("conversation-1"),
            })
            .chain(std::iter::once(DeletionScope::Node {
                node_id: SessionNodeId::new("node-1"),
                conversation_id: ConversationId::new("conversation-1"),
            }))
            .collect();
        let preview = SessionDeletePreview {
            session_id: SessionId::new("session-1"),
            name: Some("x".repeat(10_000)),
            target_revision: "a".repeat(64),
            scopes: scopes.clone(),
        };
        let projected =
            serde_json::to_value(project_session_deletion(SessionDeleteResult::Preview {
                preview,
            }))
            .unwrap();
        assert_eq!(projected["preview"]["owned_child_count"], size);
        assert_eq!(projected["preview"]["owned_conversation_count"], size + 1);
        assert_eq!(projected["preview"]["owned_node_count"], 1);
        let text = projected.to_string();
        assert!(text.len() < 600);
        assert!(!text.contains("scopes"));
        let record = DeletionRecord {
            session_id: SessionId::new("session-1"),
            target_revision: "a".repeat(64),
            scopes,
        };
        let results = [
            SessionDeleteResult::CommittedCleanupPending {
                record: record.clone(),
                detail: Some("/private/path".repeat(size)),
            },
            SessionDeleteResult::CommittedDurabilityUncertain {
                session_id: record.session_id,
                detail: "/private/path".repeat(size),
            },
            SessionDeleteResult::Blocked {
                session_id: SessionId::new("session-1"),
                reason: DeletionBlocker::Workspace {
                    resources: vec!["private resource".into(); size],
                },
            },
        ];
        let serialized: Vec<_> = results
            .into_iter()
            .map(|r| serde_json::to_string(&project_session_deletion(r)).unwrap())
            .collect();
        for text in &serialized {
            assert!(text.len() < 140);
            assert!(!text.contains("private"));
            assert!(!text.contains("scopes"));
        }
        lengths.push((text.len(), serialized[0].len(), serialized[1].len()));
    }
    assert_eq!(lengths[0].1, lengths[2].1);
    assert_eq!(lengths[0].2, lengths[2].2);
    // Only the decimal count widths may grow, never the descendant collection.
    assert_eq!(lengths[2].0 - lengths[0].0, 8);
}

#[test]
fn deletion_protocol_counts_real_durable_ownership_without_exporting_it() {
    let (dir, mut catalog, _) = open_catalog();
    let target = catalog.active_snapshot().unwrap();
    let store = store_for(&catalog, &target.id, &target.active_conversation_id);
    for ordinal in 1..=128 {
        child(dir.path(), &store, ordinal, false);
    }
    drop(store);
    let next = catalog.prepare_session(&state(), &[]).unwrap();
    catalog
        .publish_session(&next, SessionNodeOrigin::New)
        .unwrap();
    let result = crate::local_runtime::supervisor::project_session_deletion(
        catalog.delete_preview(&target.id),
    );
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["preview"]["owned_node_count"], 1);
    assert_eq!(value["preview"]["owned_child_count"], 128);
    assert_eq!(value["preview"]["owned_conversation_count"], 129);
    assert!(serde_json::to_string(&result).unwrap().len() < 300);
    assert!(value["preview"].get("scopes").is_none());
}

#[test]
fn deletion_allocator_watermarks_advance_past_skipped_orphan_ids() {
    let (dir, mut catalog, _) = open_catalog();
    let orphan = catalog.prepare_session(&state(), &[]).unwrap();
    let target = catalog.prepare_session(&state(), &[]).unwrap();
    assert_eq!(target.session_id.as_str(), "session-3");
    catalog
        .publish_session(&target, SessionNodeOrigin::New)
        .unwrap();
    // An orphan has no published identity and may be discarded, but its gap
    // must never let the allocator rewind past the later published identity.
    fs::remove_dir_all(orphan.database_path.parent().unwrap()).unwrap();
    let survivor = catalog.prepare_session(&state(), &[]).unwrap();
    assert_eq!(survivor.session_id.as_str(), "session-4");
    catalog
        .publish_session(&survivor, SessionNodeOrigin::New)
        .unwrap();
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&target.session_id)
    else {
        panic!("preview")
    };
    let work = catalog
        .commit_delete(&target.session_id, &preview.target_revision)
        .unwrap()
        .unwrap();
    let cleaned = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        SessionDeleteResult::Deleted { .. }
    ));
    assert_completed_absent(dir.path(), &target.session_id);
    let reopened = reopen_catalog(dir.path());
    let next = reopened.prepare_session(&state(), &[]).unwrap();
    assert_eq!(next.session_id.as_str(), "session-5");
    assert_eq!(next.node_id.as_str(), "node-5");
}

#[tokio::test]
async fn deletion_stale_control_response_requires_a_new_preview_token() {
    use crate::local_runtime::supervisor::LocalSessionSupervisor;
    use crate::runtime_client::host::RuntimeClientSessionControl;
    use crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult as Wire;
    use crate::runtime_client::types::{
        RuntimeClientResult, RuntimeClientSessionRequest as Request,
    };
    let (dir, catalog, initial) = fixture();
    let view = catalog.clone();
    let supervisor = LocalSessionSupervisor::new(catalog, state().model);
    let RuntimeClientResult::SessionDeletion {
        result: Wire::Preview { preview: first },
    } = supervisor
        .handle(Request::DeletePreview {
            session_id: initial.session_id.to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("preview A")
    };
    let source = view.snapshot(&initial.session_id).unwrap();
    let store = store_for(&view, &source.id, &source.active_conversation_id);
    child(dir.path(), &store, 2, false);
    drop(store);
    // Real public Session-control calls: repeated execute(A) never returns B.
    for _ in 0..2 {
        let RuntimeClientResult::SessionDeletion { result } = supervisor
            .handle(Request::Delete {
                session_id: source.id.to_string(),
                expected_target_revision: first.target_revision.clone(),
            })
            .await
            .unwrap()
        else {
            panic!("deletion result")
        };
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({"status": "stale", "session_id": source.id.as_str()})
        );
        assert!(reopen_catalog(dir.path()).document.deletions.is_empty());
    }
    let RuntimeClientResult::SessionDeletion {
        result: Wire::Preview { preview: fresh },
    } = supervisor
        .handle(Request::DeletePreview {
            session_id: source.id.to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("new preview B")
    };
    assert_ne!(fresh.target_revision, first.target_revision);
    assert_eq!(fresh.owned_child_count, first.owned_child_count + 1);
    assert!(matches!(
        supervisor
            .handle(Request::Delete {
                session_id: source.id.to_string(),
                expected_target_revision: fresh.target_revision,
            })
            .await
            .unwrap(),
        RuntimeClientResult::SessionDeletion {
            result: Wire::Deleted { .. }
        }
    ));
    assert_completed_absent(dir.path(), &source.id);
}
