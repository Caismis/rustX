// Included inside registry::tests so the existing deterministic staged-child
// and physical-settlement fixtures remain the single test substrate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent411_recovery_never_reopens_unproven_physical_containment() {
    for isolated in [false, true] {
        let plane = plane(4);
        if isolated {
            make_clean_git_workspace(&plane);
        }
        let child = stage_with_unresolved_anchor(&plane);
        let mut authority = spec("recover unresolved durable Agent");
        if isolated {
            authority.resolved.workspace_policy =
                crate::runtime::workspace::WorkspacePolicy::GitWorktree {
                    require_clean_parent: true,
                };
        }
        let accepted = start(&plane, &authority).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("unproven answer"))
            .await;
        let terminal = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
        assert_eq!(
            terminal.workspace_resource_state,
            SubagentWorkspaceResourceState::PreservedUnresolved
        );
        let recovered = SubagentRegistry::new(plane.registry.config.clone());
        recovered.restore_agents(plane.store.as_ref()).unwrap();
        let restored = recovered.agent_snapshot(&accepted.child_agent_id).unwrap();
        assert_eq!(restored.state, super::agents::AgentState::Inactive);
        let refused = recovered
            .send_message(&accepted.child_agent_id, "must not restart")
            .await;
        assert!(
            refused.is_err(),
            "unproven containment cannot regain a workspace lease"
        );
        assert_eq!(recovered.list_agents(64).len(), 1);
        assert_eq!(
            recovered.all_snapshots().len(),
            1,
            "no second activation commits"
        );
        assert_eq!(
            recovered
                .agent_snapshot(&accepted.child_agent_id)
                .unwrap()
                .latest_activation,
            accepted.subagent_id
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent411_unreconciled_orphan_cannot_recover_resume_authority() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &spec("owned orphan frontier")).await;
    // The admitted child has not completed. This intentionally bypasses the
    // normal RecoveryPlan reconciliation to prove restore fails closed.
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    let refused = recovered
        .send_message(&accepted.child_agent_id, "must not overlap")
        .await;
    assert!(refused.is_err());
    assert_eq!(recovered.all_snapshots().len(), 1);
    child
        .complete(ChildResultStatus::Succeeded, Some("settled original"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
}
