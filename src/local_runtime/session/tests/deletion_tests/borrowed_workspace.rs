use super::*;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, SubagentOwnershipKind};
use crate::runtime::identity::AttemptId;
use crate::runtime::workflow::WorkflowRunId;
use crate::runtime::workspace::{
    WorkspaceDisposalSettlement, WorkspaceSettlement, WorkspaceSettlementDisposition,
};

struct Fixture {
    session: SessionId,
    parent: SqliteConversationStore,
    database: std::path::PathBuf,
    run: WorkflowRunId,
    workspace: WorkspaceSnapshot,
    children: Vec<ConversationId>,
}

impl Fixture {
    fn create(root: &std::path::Path, borrowers: u64) -> Self {
        let catalog = SessionCatalog::create(root, &state()).unwrap();
        let (session, node, _) = catalog.active_lineage().unwrap();
        let database = catalog.database_path(&session, &node.conversation_id);
        let parent = store_for(&catalog, &session, &node.conversation_id);
        let run = WorkflowRunId {
            conversation_id: node.conversation_id,
            attempt_id: AttemptId::new("workflow-attempt"),
            invocation: 1,
        };
        let workspace = WorkspaceSnapshot {
            borrowed_from: None,
            logical_workspace: root.join("workspaces/worktrees/candidate"),
            isolation: WorkspaceIsolation::GitWorktree(GitWorktreeSnapshot {
                source_repository_root: root.join("project"),
                repository_relative_workspace: std::path::PathBuf::new(),
                physical_worktree_root: root.join("workspaces/worktrees/candidate"),
                base_commit: "a".repeat(40),
                branch: "rustx/workflow-candidate".into(),
                parent_had_uncommitted_changes: false,
            }),
        };
        let mut fixture = Self {
            session,
            parent,
            database,
            run,
            workspace,
            children: Vec::new(),
        };
        fixture.commit(
            "owned",
            RuntimeEvent::WorkflowWorkspaceOwned {
                run_id: fixture.run.clone(),
                workspace: fixture.workspace.clone(),
            },
        );
        for ordinal in 1..=borrowers {
            fixture.borrow(root, ordinal);
        }
        fixture
    }

    fn commit(&self, phase: &str, event: RuntimeEvent) {
        self.parent
            .append_event(RuntimeEventEnvelope {
                schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                event_id: crate::runtime::workspace::workflow_resource_event_id(&self.run, phase),
                sequence: 0,
                conversation_id: self.run.conversation_id.clone(),
                attempt_id: Some(self.run.attempt_id.clone()),
                turn_id: None,
                timestamp: Utc::now(),
                event,
            })
            .unwrap();
    }

    fn borrow(&mut self, root: &std::path::Path, ordinal: u64) {
        self.borrow_profile(root, ordinal, &format!("sha256:{}", "a".repeat(64)));
    }

    fn borrow_profile(&mut self, root: &std::path::Path, ordinal: u64, profile: &str) {
        let subagent = SubagentId::for_conversation(&self.run.conversation_id, ordinal);
        let child = ConversationId::new(subagent.as_str());
        let mut borrowed = self.workspace.clone();
        borrowed.borrowed_from = Some(self.run.clone());
        self.parent
            .append_event(crate::runtime::subagent::ownership_event(
                &self.run.conversation_id,
                &subagent,
                &AgentId::new(format!("agent-{ordinal}")),
                &child,
                &ToolCallId::new(format!("borrow-call-{ordinal}")),
                &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
                &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
                &serde_json::from_value(serde_json::json!(profile)).unwrap(),
                SubagentOwnershipKind::Workflow,
                &borrowed,
                Utc::now(),
            ))
            .unwrap();
        let database = crate::runtime::subagent::child_conversation_store_path(root, &child);
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        SqliteConversationStore::open(child.clone(), &database)
            .unwrap()
            .initialize(&[])
            .unwrap();
        self.children.push(child);
    }

    fn mutate_borrow(&self, mutate: impl FnOnce(&mut WorkspaceSnapshot)) {
        let event_id = crate::runtime::subagent::subagent_ownership_event_id(
            &SubagentId::for_conversation(&self.run.conversation_id, 1),
        );
        let connection = rusqlite::Connection::open(&self.database).unwrap();
        let json: String = connection
            .query_row(
                "SELECT event_json FROM events WHERE event_id=?1",
                [event_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let mut event: RuntimeEventEnvelope = serde_json::from_str(&json).unwrap();
        let RuntimeEvent::SubagentOwnershipCommitted { workspace, .. } = &mut event.event else {
            panic!("child ownership");
        };
        mutate(workspace);
        connection
            .execute(
                "UPDATE events SET event_json=?1 WHERE event_id=?2",
                rusqlite::params![serde_json::to_string(&event).unwrap(), event_id.as_str()],
            )
            .unwrap();
    }
}

#[test]
fn deletion_borrowed_workspace_process_gate() {
    use std::io::{Read, Write};
    let Some(root) = std::env::var_os("RUSTX_260_BORROW_ROOT") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let _controller = crate::runtime::local_storage::ProductController::acquire(root).unwrap();
    let _fixture = Fixture::create(root, 1);
    println!("BORROW_COMMITTED");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
}

#[test]
#[allow(clippy::too_many_lines)] // One gated process-death and authoritative disposal sequence.
fn deletion_borrowed_workspace_crash_before_child_terminal_disposes_only_workflow() {
    use crate::local_runtime::session_deletion::WorkspaceBlockerState;
    use std::io::{BufRead, BufReader};
    let root = tempfile::tempdir().unwrap();
    let mut process = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "local_runtime::session::tests::deletion_tests::borrowed_workspace::deletion_borrowed_workspace_process_gate", "--nocapture"])
        .env("RUSTX_260_BORROW_ROOT", root.path()).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "child exited before commit gate"
        );
        if line.trim() == "BORROW_COMMITTED" {
            break;
        }
    }
    process.stdout = Some(output.into_inner());
    process.kill().unwrap();
    process.wait().unwrap();
    let catalog = reopen_catalog(root.path());
    let (session, node, _) = catalog.active_lineage().unwrap();
    let parent = store_for(&catalog, &session, &node.conversation_id);
    let events = parent.read_events(None, 100).unwrap().events;
    assert_eq!(
        events.len(),
        2,
        "only Workflow ownership and child borrowing survived; no child terminal"
    );
    let RuntimeEvent::WorkflowWorkspaceOwned { run_id, workspace } = events[0].event.clone() else {
        panic!("workflow ownership");
    };
    let RuntimeEvent::SubagentOwnershipCommitted {
        child_conversation_id,
        ..
    } = &events[1].event
    else {
        panic!("child ownership");
    };
    let target = SessionDeletionPreflight::acquire(root.path(), &session).unwrap();
    assert!(
        target
            .conversations()
            .iter()
            .any(|c| &c.conversation_id == child_conversation_id)
    );
    assert_eq!(target.workspace_blockers().len(), 1);
    assert_eq!(
        target.workspace_blockers()[0].resource_id,
        format!("workflow:{}", serde_json::to_string(&run_id).unwrap())
    );
    let initial = *target.ownership_revision();
    drop(target);
    let database = catalog.database_path(&session, &node.conversation_id);
    let fixture = Fixture {
        session,
        database,
        parent,
        run: run_id,
        workspace,
        children: vec![],
    };
    let tree = fixture.workspace.git_worktree().unwrap();
    let handoff = crate::runtime::workspace::WorkspaceHandoff {
        logical_workspace: fixture.workspace.logical_workspace.clone(),
        physical_worktree_root: tree.physical_worktree_root.clone(),
        branch: tree.branch.clone(),
        base_commit: tree.base_commit.clone(),
        head_commit: tree.base_commit.clone(),
        dirty: true,
    };
    fixture.commit(
        "settled",
        RuntimeEvent::WorkflowWorkspaceSettled {
            run_id: fixture.run.clone(),
            workspace: WorkspaceSettlement {
                snapshot: fixture.workspace.clone(),
                disposition: WorkspaceSettlementDisposition::Retained {
                    handoff: handoff.clone(),
                    cleanup_error: None,
                },
            },
            candidate: Some(crate::runtime::workspace::CandidateReference {
                run: fixture.run.clone(),
                version: 1,
                content: "a".repeat(64),
            }),
            recovery_guard: None,
        },
    );
    fixture.commit(
        "disposal-started",
        RuntimeEvent::WorkflowWorkspaceDisposalStarted {
            run_id: fixture.run.clone(),
            handoff,
        },
    );
    fixture.commit(
        "disposal-worktree-removed",
        RuntimeEvent::WorkflowWorkspaceDisposalSettled {
            run_id: fixture.run.clone(),
            settlement: WorkspaceDisposalSettlement::WorktreeRemoved {
                detail: "branch remains".into(),
            },
        },
    );
    let partial = SessionDeletionPreflight::acquire(root.path(), &fixture.session).unwrap();
    assert_eq!(partial.workspace_blockers().len(), 1);
    assert_eq!(
        partial.workspace_blockers()[0].state,
        WorkspaceBlockerState::BranchOnly
    );
    let partial_revision = *partial.ownership_revision();
    assert_ne!(initial, partial_revision);
    drop(partial);
    fixture.commit(
        "disposal-disposed",
        RuntimeEvent::WorkflowWorkspaceDisposalSettled {
            run_id: fixture.run.clone(),
            settlement: WorkspaceDisposalSettlement::Disposed,
        },
    );
    let disposed = SessionDeletionPreflight::acquire(root.path(), &fixture.session).unwrap();
    assert!(disposed.workspace_blockers().is_empty());
    assert_ne!(&partial_revision, disposed.ownership_revision());
    assert!(
        fixture
            .parent
            .read_events(None, 100)
            .unwrap()
            .events
            .iter()
            .all(|e| !matches!(
                e.event,
                RuntimeEvent::SubagentTerminalPublished { .. }
                    | RuntimeEvent::SubagentTerminalSettled { .. }
            )),
        "Workflow disposal needs no child terminal"
    );
}

#[test]
fn deletion_borrowed_workspace_missing_workflow_owner_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::create(root.path(), 1);
    rusqlite::Connection::open(&fixture.database)
        .unwrap()
        .execute(
            "DELETE FROM events WHERE event_id=?1",
            [
                crate::runtime::workspace::workflow_resource_event_id(&fixture.run, "owned")
                    .as_str(),
            ],
        )
        .unwrap();
    assert!(SessionDeletionPreflight::acquire(root.path(), &fixture.session).is_err());
}

#[test]
fn deletion_borrowed_workspace_mismatched_workspace_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::create(root.path(), 1);
    fixture.mutate_borrow(|workspace| {
        let WorkspaceIsolation::GitWorktree(tree) = &mut workspace.isolation else {
            panic!("isolated");
        };
        tree.physical_worktree_root = root.path().join("workspaces/worktrees/foreign");
        workspace.logical_workspace = tree.physical_worktree_root.clone();
    });
    assert!(SessionDeletionPreflight::acquire(root.path(), &fixture.session).is_err());
}

#[test]
fn deletion_borrowed_workspace_foreign_or_invalid_run_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::create(root.path(), 1);
    for run in [
        WorkflowRunId {
            conversation_id: ConversationId::new("foreign"),
            ..fixture.run.clone()
        },
        WorkflowRunId {
            invocation: 0,
            ..fixture.run.clone()
        },
        WorkflowRunId {
            attempt_id: AttemptId::new(""),
            ..fixture.run.clone()
        },
    ] {
        fixture.mutate_borrow(|workspace| workspace.borrowed_from = Some(run));
        assert!(SessionDeletionPreflight::acquire(root.path(), &fixture.session).is_err());
    }
}

#[test]
fn deletion_borrowed_workspace_multiple_children_share_one_disposal_owner() {
    let root = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::create(root.path(), 1);
    let before = revision(root.path(), &fixture.session);
    fixture.borrow(root.path(), 2);
    fixture.borrow(root.path(), 3);
    let target = SessionDeletionPreflight::acquire(root.path(), &fixture.session).unwrap();
    assert_ne!(
        &before,
        target.ownership_revision(),
        "new child Conversations change owned cleanup scope"
    );
    assert_eq!(target.conversations().len(), 4);
    for child in &fixture.children {
        assert_eq!(
            target
                .conversations()
                .iter()
                .filter(|c| &c.conversation_id == child)
                .count(),
            1
        );
    }
    assert_eq!(target.workspace_blockers().len(), 1);
    assert!(
        target.workspace_blockers()[0]
            .resource_id
            .starts_with("workflow:")
    );
}

#[test]
fn deletion_overridden_borrowers_keep_profile_identity_out_of_ownership_revision() {
    use crate::runtime::subagent::{ResolvedSubagentSpec, SubagentInvocationOverride};
    let root = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::create(root.path(), 0);
    let mut spec = ResolvedSubagentSpec {
        agent: crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
        definition_digest: serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
        execution_deadline: None,
        workspace_policy: crate::runtime::workspace::WorkspacePolicy::SharedWorkspace,
        instructions: "inspect".into(),
        model: crate::model::frozen::test_frozen_model_spec(
            serde_json::from_value(serde_json::json!("local/model")).unwrap(),
        ),
        tools: Vec::new(),
        skills: Vec::new(),
        project_instructions: Vec::new(),
        materialization:
            crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
        extensions: crate::extensions::NativeAgentExtensions::none(),
    };
    let profiles: Vec<_> = [
        serde_json::json!({"extensions": {}}),
        serde_json::json!({"extensions": {"agentStatus": {"time": {"enabled": true}}}}),
    ]
    .into_iter()
    .map(|value| {
        let invocation: SubagentInvocationOverride = serde_json::from_value(value).unwrap();
        spec.extensions = invocation.extensions.unwrap().resolve();
        spec.profile_digest()
    })
    .collect();
    assert_ne!(profiles[0], profiles[1]);
    fixture.borrow_profile(root.path(), 1, profiles[0].as_str());
    let first = revision(root.path(), &fixture.session);
    fixture.borrow_profile(root.path(), 2, profiles[1].as_str());
    let target = SessionDeletionPreflight::acquire(root.path(), &fixture.session).unwrap();
    assert_ne!(
        &first,
        target.ownership_revision(),
        "adding a child changes ownership"
    );
    assert_eq!(target.conversations().len(), 3);
    for child in &fixture.children {
        assert!(
            target
                .conversations()
                .iter()
                .any(|c| &c.conversation_id == child)
        );
    }
    assert_eq!(target.workspace_blockers().len(), 1);
    assert!(
        target.workspace_blockers()[0]
            .resource_id
            .starts_with("workflow:")
    );
    let before = *target.ownership_revision();
    drop(target);
    let events = fixture.parent.read_events(None, 256).unwrap().events;
    let mut children = 0;
    for mut event in events {
        if let RuntimeEvent::SubagentOwnershipCommitted {
            definition_digest,
            profile_digest,
            workspace,
            ..
        } = &mut event.event
        {
            assert_eq!(definition_digest, spec.definition_digest.as_str());
            assert_eq!(profile_digest, profiles[children].as_str());
            assert_eq!(workspace.borrowed_from.as_ref(), Some(&fixture.run));
            // Vary only valid capability metadata, retaining the exact resource graph.
            *profile_digest = profiles[1 - children].as_str().to_owned();
            children += 1;
            let connection = rusqlite::Connection::open(&fixture.database).unwrap();
            connection
                .execute(
                    "UPDATE events SET event_json=?1 WHERE event_id=?2",
                    rusqlite::params![
                        serde_json::to_string(&event).unwrap(),
                        event.event_id.as_str()
                    ],
                )
                .unwrap();
        }
    }
    assert_eq!(children, 2);
    assert_eq!(
        revision(root.path(), &fixture.session),
        before,
        "capability metadata alone never changes the deletion target"
    );
}
