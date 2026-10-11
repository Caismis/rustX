//! Issue #462 shared-workspace regressions over the real registry and the
//! staged-child driver. Ordering is established by IPC frames and registry
//! snapshots, never by elapsed time.
use super::*;

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
    assert_eq!(git(&workspace, &["worktree", "list", "--porcelain"]), worktrees);
    assert_eq!(git(&workspace, &["branch", "--list"]), branches);

    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert_eq!(git(&workspace, &["worktree", "list", "--porcelain"]), worktrees);
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
    let accepted = start(&plane, &workflow_spec("produce the node output")).await;
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
    assert!(plane.registry.agent_snapshot(&accepted.child_agent_id).is_none());
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
    assert!(matches!(error, SubagentStartError::Workspace { .. }), "{error:?}");
    assert!(!missing.exists(), "no replacement directory is created");
    assert!(registry.all_snapshots().is_empty());
    assert!(events(&plane).is_empty(), "nothing is committed");
}
