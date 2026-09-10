use super::*;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::local_storage::{ConversationAccess, ProductRoot};
use crate::runtime::workspace::{
    WorkspaceDisposalHook, WorkspaceDisposalSettlement, WorkspaceManager, WorkspaceOwner,
    WorkspacePolicy,
};
use std::sync::Arc;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn deletion_preflight_first_excludes_workflow_destructive_admission_until_release() {
    disposal_race(true).await;
}

#[tokio::test]
async fn deletion_workflow_disposal_first_excludes_preflight_through_physical_settlement() {
    disposal_race(false).await;
}

#[allow(clippy::too_many_lines)]
async fn disposal_race(preflight_first: bool) {
    let (root, mut catalog, _) = open_catalog();
    let (conversation, session, _) = append_history(&catalog, &source_history());
    let store = store_for(&catalog, &session, &conversation);
    let lineage = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let other = catalog.prepare_clone_session(&state(), &lineage).unwrap();
    catalog
        .publish_session(&other, SessionNodeOrigin::New)
        .unwrap();
    let identity = ProductRoot::existing(root.path()).unwrap();
    let access = ConversationAccess::existing(
        &identity,
        catalog
            .database_path(&session, &conversation)
            .parent()
            .unwrap(),
    )
    .unwrap();
    let store = Arc::new(store.with_lifecycle(Arc::new(access)));
    let source = tempfile::tempdir().unwrap();
    git(source.path(), &["init"]);
    std::fs::write(source.path().join("source"), "baseline").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-m", "baseline"]);
    let mut manager =
        WorkspaceManager::new(std::fs::canonicalize(source.path()).unwrap(), root.path());
    let hook = Arc::new(WorkspaceDisposalHook::new());
    manager.install_disposal_hook(hook.clone());
    let mut node = crate::runtime::workflow::test_instance("disposal", "write");
    node.block.run.conversation_id = conversation.clone();
    let run = node.block.run.clone();
    let scope = manager
        .acquire(
            WorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            },
            &WorkspaceOwner::Workflow(run.clone()),
            &CancellationSignal::new(),
        )
        .await
        .unwrap()
        .retain_for_run(run.clone())
        .await
        .unwrap();
    let writer = scope
        .borrow(node, None, &CancellationSignal::new())
        .await
        .unwrap();
    let checkout = writer.snapshot().logical_workspace.clone();
    let branch = writer.snapshot().git_worktree().unwrap().branch.clone();
    std::fs::write(checkout.join("source"), "retained candidate").unwrap();
    writer.finish(false).await.unwrap();
    let workspace = scope.settle().await;
    let crate::runtime::workspace::WorkspaceSettlementDisposition::Retained { handoff, .. } =
        &workspace.disposition
    else {
        panic!("fixture must retain the Workflow candidate")
    };
    let started = RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: crate::runtime::workspace::workflow_resource_event_id(&run, "disposal-started"),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: Utc::now(),
        event: RuntimeEvent::WorkflowWorkspaceDisposalStarted {
            run_id: run.clone(),
            handoff: handoff.clone(),
        },
    };
    for (phase, event) in [
        (
            "owned",
            RuntimeEvent::WorkflowWorkspaceOwned {
                run_id: run.clone(),
                workspace: workspace.snapshot.clone(),
            },
        ),
        (
            "settled",
            RuntimeEvent::WorkflowWorkspaceSettled {
                run_id: run.clone(),
                workspace,
                candidate: scope.final_reference().await,
                recovery_guard: scope.recovery_guard().await.map(Box::new),
            },
        ),
    ] {
        store
            .append_event(RuntimeEventEnvelope {
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: crate::runtime::workspace::workflow_resource_event_id(&run, phase),
                sequence: 0,
                conversation_id: conversation.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp: Utc::now(),
                event,
            })
            .unwrap();
    }
    let branch_head = git(source.path(), &["rev-parse", &branch]);
    if preflight_first {
        // A separate target isolates the ownership freeze from the live store's
        // ConversationAccess. No conflicting physical operation may cross it.
        let preview = SessionDeletionPreflight::acquire(root.path(), &other.session_id).unwrap();
        assert!(
            manager
                .dispose_workflow_workspace(&*store, &run)
                .await
                .is_err()
        );
        assert!(
            store.append_event(started).is_err(),
            "direct destructive-start publication must also participate in the freeze"
        );
        assert!(checkout.exists());
        assert_eq!(git(source.path(), &["rev-parse", &branch]), branch_head);
        assert!(
            !store
                .read_events(None, 1000)
                .unwrap()
                .events
                .iter()
                .any(|event| matches!(
                    event.event,
                    RuntimeEvent::WorkflowWorkspaceDisposalStarted { .. }
                ))
        );
        drop(preview);
    }
    hook.arm_before_recheck();
    hook.arm_after_worktree_removal();
    let task_store = store.clone();
    let task_run = run.clone();
    let task = tokio::spawn(async move {
        manager
            .dispose_workflow_workspace(&*task_store, &task_run)
            .await
    });
    hook.wait_until_verified().await; // real Started commit, before physical Git mutation
    assert!(
        store
            .read_events(None, 1000)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(
                event.event,
                RuntimeEvent::WorkflowWorkspaceDisposalStarted { .. }
            ))
    );
    assert!(checkout.exists());
    assert_eq!(git(source.path(), &["rev-parse", &branch]), branch_head);
    assert_eq!(
        SessionDeletionPreflight::acquire(root.path(), &other.session_id)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    hook.release().await;
    hook.wait_until_worktree_removed().await;
    assert!(!checkout.exists());
    assert_eq!(
        SessionDeletionPreflight::acquire(root.path(), &other.session_id)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock,
    );
    hook.release_after_worktree_removal().await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        WorkspaceDisposalSettlement::Disposed
    );
    assert!(!checkout.exists());
    assert!(git(source.path(), &["branch", "--list", &branch]).is_empty());
    drop(store); // ordinary target access ends; physical disposal has settled
    let final_preview = SessionDeletionPreflight::acquire(root.path(), &session).unwrap();
    assert!(final_preview.workspace_blockers().is_empty());
}
