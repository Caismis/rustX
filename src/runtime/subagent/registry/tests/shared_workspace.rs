//! Issue #462 shared-workspace regressions over the real registry and the
//! staged-child driver. Ordering is established by IPC frames and registry
//! snapshots, never by elapsed time.
use super::*;
use crate::events::types::{AgentActivationAdmissionPhase, RuntimeEvent};

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// B04/B05: ordinary child creation inside a dirty Git workspace binds the
/// child to that exact directory. rustX creates no worktree, branch or copy,
/// and the parent's uncommitted state stays in place for the child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ordinary_child_shares_a_dirty_git_workspace_without_worktrees() {
    let plane = plane(4);
    let workspace = plane.dir.path().join("workspace");
    git(&workspace, &["init", "-q"]);
    std::fs::write(workspace.join("tracked.txt"), "base\n").unwrap();
    git(&workspace, &["add", "."]);
    git(&workspace, &["commit", "-qm", "base"]);
    std::fs::write(workspace.join("tracked.txt"), "uncommitted parent edit\n").unwrap();
    let worktrees = git(&workspace, &["worktree", "list", "--porcelain"]);
    let branches = git(&workspace, &["branch", "--list"]);

    let child = stage_stubborn(&plane);
    let accepted = start(&plane, &start_spec("read the dirty workspace")).await;
    let snapshot = plane.registry.snapshot(&accepted.subagent_id).unwrap();
    assert_eq!(snapshot.workspace, workspace);
    assert_eq!(
        std::fs::read_to_string(snapshot.workspace.join("tracked.txt")).unwrap(),
        "uncommitted parent edit\n",
        "the child binding sees the parent's uncommitted state"
    );
    assert_eq!(
        git(&workspace, &["worktree", "list", "--porcelain"]),
        worktrees
    );
    assert_eq!(git(&workspace, &["branch", "--list"]), branches);

    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert_eq!(
        git(&workspace, &["worktree", "list", "--porcelain"]),
        worktrees
    );
    assert_eq!(git(&workspace, &["branch", "--list"]), branches);
    assert_eq!(
        std::fs::read_to_string(workspace.join("tracked.txt")).unwrap(),
        "uncommitted parent edit\n"
    );
}

/// B11: sibling Agents are not serialized by any workspace lease. Both
/// children are delegated and running in the same workspace before either
/// settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sibling_agents_run_concurrently_in_one_workspace() {
    let plane = plane(4);
    let mut first = stage_stubborn(&plane);
    let mut second = stage_stubborn(&plane);
    let a = start(&plane, &start_spec("first sibling")).await;
    let b = start(&plane, &start_spec("second sibling")).await;
    first.accept_delegate().await;
    second.accept_delegate().await;
    for id in [&a.subagent_id, &b.subagent_id] {
        let snapshot = plane.registry.snapshot(id).unwrap();
        assert_eq!(snapshot.state, SubagentState::Running);
        assert_eq!(snapshot.workspace, plane.dir.path().join("workspace"));
    }
    first
        .send_result(ChildResultStatus::Succeeded, Some("first"))
        .await;
    second
        .send_result(ChildResultStatus::Succeeded, Some("second"))
        .await;
    for id in [&a.subagent_id, &b.subagent_id] {
        assert_eq!(
            plane.registry.wait_until_settled(id).await.unwrap().state,
            SubagentState::Succeeded
        );
    }
}

/// C19: a finite Workflow child is never a durable Agent. Ordinary Agent
/// controls cannot resume it after settlement; its result belongs only to
/// the Workflow node that started it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_child_cannot_be_resumed_through_agent_controls() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(
        &plane,
        &workflow_spec(&plane.conversation_id, "produce the node output"),
    )
    .await;
    child
        .complete(ChildResultStatus::Succeeded, Some(r#"{"summary":"done"}"#))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let before = events(&plane).len();
    assert!(
        plane
            .registry
            .send_message(
                &accepted.child_agent_id,
                "continue as an ordinary Agent",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new(),
            )
            .await
            .is_err(),
        "a Workflow child has no durable continuation authority"
    );
    assert!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .is_none()
    );
    assert_eq!(plane.registry.all_snapshots().len(), 1);
    assert_eq!(events(&plane).len(), before, "the refusal commits nothing");
    assert!(
        plane.store.select_pending_batch().unwrap().is_none(),
        "the Workflow result never enters the parent mailbox"
    );
}

/// B21: a missing Session binding fails admission before any allocation,
/// without falling back to another working directory or creating a
/// replacement directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_workspace_binding_fails_without_fallback_or_creation() {
    let plane = plane(4);
    let missing = plane.dir.path().join("deleted-workspace");
    let mut config = plane.registry.config.clone();
    config.workspace = missing.clone();
    let registry = SubagentRegistry::new(config);
    let error = registry
        .prepare(&start_spec("no binding"), &CancellationSignal::new())
        .await
        .expect_err("a missing binding cannot start a child");
    assert!(
        matches!(error, SubagentStartError::Workspace { .. }),
        "{error:?}"
    );
    assert!(!missing.exists(), "no replacement directory is created");
    assert!(registry.all_snapshots().is_empty());
    assert!(events(&plane).is_empty(), "nothing is committed");
}

fn allocations(plane: &TestPlane) -> Vec<(SubagentId, ConversationId)> {
    let spawn = &plane.registry.config.spawn;
    let mut allocations = super::super::super::physical_recovery::consumed_session_allocations(
        &spawn.product_root,
        &spawn.session_id,
    )
    .unwrap();
    allocations.sort();
    allocations
}

/// A1/A2/A3: a bad binding never consumes a physical activation or changes
/// committed history. Restoring the same directory permits the same Agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_workspace_preflight_rejects_missing_and_file_bindings_without_allocation() {
    for regular_file in [false, true] {
        assert_resume_workspace_preflight(regular_file, Vec::new()).await;
        assert_resume_workspace_preflight(
            regular_file,
            vec![crate::message::content::UploadedFileRef {
                batch_id: "admitted-batch".into(),
                name: "input.txt".into(),
            }],
        )
        .await;
    }
}

async fn assert_resume_workspace_preflight(
    regular_file: bool,
    attachments: Vec<crate::message::content::UploadedFileRef>,
) {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("first activation")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("retained history"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let workspace = plane
        .registry
        .snapshot(&first.subagent_id)
        .unwrap()
        .workspace;
    let retained = plane.dir.path().join("retained-workspace");
    std::fs::rename(&workspace, &retained).unwrap();
    if regular_file {
        std::fs::write(&workspace, "not a directory").unwrap();
    }
    let authority = plane.registry.state.lock().unwrap().agents[&first.child_agent_id]
        .authority
        .clone();
    let before_events = events(&plane);
    let before_allocations = allocations(&plane);
    let before_attempts = plane.registry.state.lock().unwrap().allocation_attempts;
    let error = plane
        .registry
        .send_message_with_attachments(
            &first.child_agent_id,
            "rejected resume",
            &attachments,
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            AgentControlError::Start(SubagentStartError::Workspace { .. })
        ),
        "{error:?}"
    );
    assert_eq!(events(&plane), before_events);
    assert_eq!(allocations(&plane), before_allocations);
    assert_eq!(
        plane.registry.state.lock().unwrap().allocation_attempts,
        before_attempts
    );
    assert!(plane.registry.unproven_settlements().is_empty());
    let agent = plane
        .registry
        .agent_snapshot(&first.child_agent_id)
        .unwrap();
    assert_eq!(agent.state, AgentState::Inactive);
    assert_eq!(agent.latest_activation, first.subagent_id);
    assert!(agent.current_activation.is_none());
    if regular_file {
        std::fs::remove_file(&workspace).unwrap();
    }
    std::fs::rename(&retained, &workspace).unwrap();
    let mut child = stage_exit0(&plane);
    let (resumed, delegation) = tokio::join!(
        plane.registry.send_message_with_attachments(
            &first.child_agent_id,
            "restored resume",
            &attachments,
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new()
        ),
        child.accept_delegate(),
    );
    let resumed = resumed.unwrap();
    assert_eq!(delegation.task, "restored resume");
    assert_eq!(delegation.attachments, attachments);
    assert_eq!(resumed.agent_id, first.child_agent_id);
    let snapshot = plane.registry.snapshot(&resumed.activation_id).unwrap();
    assert_eq!(snapshot.workspace, workspace);
    assert_eq!(snapshot.child_conversation_id, first.child_conversation_id);
    {
        let state = plane.registry.state.lock().unwrap();
        assert_eq!(state.agents[&first.child_agent_id].authority, authority);
        assert_eq!(
            state.records[state.index[&first.subagent_id]].profile_digest,
            state.records[state.index[&resumed.activation_id]].profile_digest
        );
    }
    child
        .send_result(ChildResultStatus::Succeeded, Some("second"))
        .await;
    plane
        .registry
        .wait_until_settled(&resumed.activation_id)
        .await
        .unwrap();
    assert!(events(&plane).starts_with(&before_events));
}

/// A4: the parked off-lock preflight has the sole reservation. Interrupt
/// captures it, cancels before allocation, and cannot affect a later winner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_workspace_preflight_arbitrates_competitors_and_preallocation_interrupt() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("first")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let before_events = events(&plane);
    let before_allocations = allocations(&plane);
    let before_attempts = plane.registry.state.lock().unwrap().allocation_attempts;
    let gate = Arc::new(CommitBoundaryHook::default());
    plane.registry.state.lock().unwrap().resume_preflight_hook = Some(gate.clone());
    let mut sending = Box::pin(plane.registry.send_message(
        &first.child_agent_id,
        "cancelled winner",
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut sending).is_pending());
    gate.wait_until_entered();
    // This lookup also proves preflight does not retain the registry mutex.
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Admitting
    );
    assert!(matches!(
        plane
            .registry
            .send_message(
                &first.child_agent_id,
                "competitor",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new()
            )
            .await,
        Err(AgentControlError::Stopping)
    ));
    let mut interrupt = Box::pin(plane.registry.interrupt_agent(&first.child_agent_id));
    assert!(futures_util::poll!(&mut interrupt).is_pending());
    gate.release();
    assert!(matches!(
        sending.await,
        Err(AgentControlError::Start(SubagentStartError::Cancelled))
    ));
    assert_eq!(events(&plane), before_events);
    assert_eq!(allocations(&plane), before_allocations);
    assert_eq!(
        plane.registry.state.lock().unwrap().allocation_attempts,
        before_attempts
    );
    assert!(plane.registry.unproven_settlements().is_empty());
    // Hold the first interruption response until a later activation is live.
    let mut child = stage_exit0(&plane);
    let (resumed, _) = tokio::join!(
        plane.registry.send_message(
            &first.child_agent_id,
            "next winner",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new()
        ),
        child.accept_delegate(),
    );
    let resumed = resumed.unwrap();
    let cancelled = interrupt.await.unwrap();
    assert_ne!(cancelled.activation_id, Some(resumed.activation_id.clone()));
    assert!(cancelled.outcome.is_none());
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .current_activation,
        Some(resumed.activation_id.clone())
    );
    child
        .send_result(ChildResultStatus::Succeeded, Some("next"))
        .await;
    plane
        .registry
        .wait_until_settled(&resumed.activation_id)
        .await
        .unwrap();
}

/// The second check is still authoritative if the directory disappears after
/// preflight and physical allocation. Only the proven rollback releases it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_workspace_preflight_does_not_replace_execution_time_validation() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("first")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let before_events = events(&plane);
    let before_allocations = allocations(&plane);
    let before_attempts = plane.registry.state.lock().unwrap().allocation_attempts;
    let gate = Arc::new(CommitBoundaryHook::default());
    plane.registry.state.lock().unwrap().reserved_commit_hook = Some(gate.clone());
    let mut sending = Box::pin(plane.registry.send_message(
        &first.child_agent_id,
        "workspace disappears after preflight",
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut sending).is_pending());
    gate.wait_until_entered();
    assert_eq!(allocations(&plane).len(), before_allocations.len() + 1);
    let workspace = plane
        .registry
        .snapshot(&first.subagent_id)
        .unwrap()
        .workspace;
    std::fs::remove_dir_all(&workspace).unwrap();
    gate.release();
    assert!(matches!(
        sending.await,
        Err(AgentControlError::Start(
            SubagentStartError::Workspace { .. }
        ))
    ));
    let after = events(&plane);
    assert!(after.starts_with(&before_events));
    let appended = &after[before_events.len()..];
    assert!(matches!(appended, [
        RuntimeEvent::AgentActivationAdmission { activation_id: reserved, phase: AgentActivationAdmissionPhase::Reserved, .. },
        RuntimeEvent::AgentActivationAdmission { activation_id: rolled_back, phase: AgentActivationAdmissionPhase::RolledBack {physical_settlement_proven: true}, .. }
    ] if reserved == rolled_back));
    assert_eq!(
        plane.registry.state.lock().unwrap().allocation_attempts,
        before_attempts,
        "no child preparation allocation"
    );
    assert!(plane.registry.unproven_settlements().is_empty());
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
}
