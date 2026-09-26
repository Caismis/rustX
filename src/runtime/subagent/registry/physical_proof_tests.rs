// Physical outcome injection occurs at the driver/registry settlement boundary.
// No timing assumptions or sleeps determine the winning terminal generation.
async fn assert_unproven_agent_stops_at_exact_activation(
    registry: &SubagentRegistry,
    agent_id: &AgentId,
    activation_id: &SubagentId,
) {
    let (agent, activation) = registry.agent_snapshot_with_activation(agent_id).unwrap();
    assert_eq!(agent.state, AgentState::Unavailable);
    assert_eq!(agent.current_activation.as_ref(), Some(activation_id));
    assert_eq!(&agent.latest_activation, activation_id);
    assert!(!activation.is_settled());
    assert_eq!(registry.unproven_settlements(), vec![activation_id.clone()]);
    assert!(matches!(
        registry.wait_agent(agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        registry.interrupt_agent(agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        registry
            .send_message(
                agent_id,
                "must not resume",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new()
            )
            .await,
        Err(AgentControlError::Settlement)
    ));
    assert_eq!(registry.with_goal_idle(|| true), None);
    let workspace = registry.state.lock().unwrap().agents[agent_id]
        .workspace
        .clone();
    assert!(
        workspace.acquire().await.is_err(),
        "physical lease must also remain closed"
    );
}

#[tokio::test]
async fn unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent() {
    #[derive(Default)]
    struct OwnerObserver(Mutex<Vec<(Option<AgentSnapshot>, SubagentSnapshot)>>);
    impl SubagentObserver for OwnerObserver {
        fn on_snapshot(&self, agent: Option<&AgentSnapshot>, activation: &SubagentSnapshot) {
            self.0
                .lock()
                .unwrap()
                .push((agent.cloned(), activation.clone()));
        }
    }
    for control_failure in [false, true] {
        let plane = plane(4);
        let observations = Arc::new(OwnerObserver::default());
        plane
            .registry
            .install_observer_and_agent_snapshots(observations.clone());
        let mut child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("physical proof frontier")).await;
        child.accept_delegate().await;
        plane.registry.settle_from_driver(
            &accepted.subagent_id,
            PhysicalSettlement {
                outcome: if control_failure {
                    PhysicalOutcome::ControlFailure {
                        diagnostic: "unproven control settlement".into(),
                    }
                } else {
                    PhysicalOutcome::Completed(ResultFrame {
                        status: ChildResultStatus::Succeeded,
                        content: Some("not a settled success".into()),
                        diagnostic: None,
                    })
                },
                nested: super::super::anchors::NestedUnitSettlement {
                    contained: Vec::new(),
                    unproven: Vec::new(),
                },
                runtime_root_cleanup_error: (!control_failure)
                    .then(|| "unproven physical root removal".into()),
                candidate: None,
                workspace: crate::runtime::workspace::WorkspaceSettlement::shared(
                    plane
                        .registry
                        .snapshot(&accepted.subagent_id)
                        .unwrap()
                        .workspace,
                ),
            },
        );
        drop(child.peer);
        let terminal = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
        assert_eq!(terminal.state, SubagentState::Failed);
        assert!(
            !terminal.is_settled(),
            "logical failure is not physical settlement proof"
        );
        let fresh = plane
            .registry
            .agent_snapshot_with_activation(&accepted.child_agent_id)
            .unwrap();
        assert!(
            observations
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|(agent, activation)| {
                    agent.as_ref() == Some(&fresh.0) && activation == &fresh.1
                }),
            "live owner publication contains the exact fresh Agent + terminal-unproven activation cut"
        );
        assert_unproven_agent_stops_at_exact_activation(
            &plane.registry,
            &accepted.child_agent_id,
            &accepted.subagent_id,
        )
        .await;
        let restored = SubagentRegistry::new(plane.registry.config.clone());
        restored.restore_agents(plane.store.as_ref()).unwrap();
        assert_unproven_agent_stops_at_exact_activation(
            &restored,
            &accepted.child_agent_id,
            &accepted.subagent_id,
        )
        .await;
        assert_eq!(restored.all_snapshots().len(), 1);
    }
}

#[tokio::test]
async fn poisoned_agent_workspace_is_unavailable_before_reservation_or_wait() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("workspace authority")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    assert!(
        plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap()
            .is_settled()
    );
    {
        let state = plane.registry.state.lock().unwrap();
        state.agents[&accepted.child_agent_id].workspace.poison();
    }
    let before = events(&plane).len();
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Unavailable
    );
    assert!(matches!(
        plane.registry.wait_agent(&accepted.child_agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        plane
            .registry
            .interrupt_agent(&accepted.child_agent_id)
            .await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        plane
            .registry
            .send_message(
                &accepted.child_agent_id,
                "cannot repair by retrying",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new(),
            )
            .await,
        Err(AgentControlError::Settlement)
    ));
    assert_eq!(
        events(&plane).len(),
        before,
        "no reservation or physical activation is admitted"
    );
}

#[tokio::test]
async fn finite_workspace_disposal_cannot_delete_a_resumable_agents_workspace() {
    let plane = plane(4);
    make_clean_git_workspace(&plane);
    let child = stage_exit0(&plane);
    let mut spec = start_spec("durable workspace");
    spec.authority.resolved.workspace_policy =
        crate::runtime::workspace::WorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        };
    let first = start(&plane, &spec).await;
    let workspace = plane
        .registry
        .snapshot(&first.subagent_id)
        .unwrap()
        .workspace;
    let file = workspace.logical_workspace.join("agent-owned.txt");
    std::fs::write(&file, "survives every finite activation").unwrap();
    child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    assert!(
        plane
            .registry
            .wait_until_settled(&first.subagent_id)
            .await
            .unwrap()
            .is_settled()
    );
    assert!(matches!(
        plane
            .registry
            .dispose_retained_workspace(&first.subagent_id)
            .await
            .unwrap(),
        SubagentWorkspaceDisposal::NoRetainedWorkspace(_)
    ));
    assert!(file.exists());
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
    let mut child = stage_exit0(&plane);
    let (resumed, _) = tokio::join!(
        plane.registry.send_message(
            &first.child_agent_id,
            "continue",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new()
        ),
        child.accept_delegate(),
    );
    let resumed = resumed.unwrap();
    assert_ne!(resumed.activation_id, first.subagent_id);
    assert_eq!(
        plane
            .registry
            .snapshot(&resumed.activation_id)
            .unwrap()
            .workspace,
        workspace
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "survives every finite activation"
    );
    child
        .send_result(ChildResultStatus::Succeeded, Some("second"))
        .await;
    assert!(
        plane
            .registry
            .wait_until_settled(&resumed.activation_id)
            .await
            .unwrap()
            .is_settled()
    );
    assert!(file.exists());
}

#[tokio::test]
async fn recovered_isolated_agent_workspace_has_no_activation_disposal_handoff() {
    use crate::runtime::subagent::physical_recovery::ChildPhysicalLease;
    let plane = plane(4);
    make_clean_git_workspace(&plane);
    let child = stage_exit0(&plane);
    let mut authority = spec("recover Agent workspace lifetime");
    authority.authority.resolved.workspace_policy =
        crate::runtime::workspace::WorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        };
    let accepted = start(&plane, &authority).await;
    let workspace = plane
        .registry
        .snapshot(&accepted.subagent_id)
        .unwrap()
        .workspace;
    let worktree = workspace
        .git_worktree()
        .unwrap()
        .physical_worktree_root
        .clone();
    plane.store.arm_fail_accept_times(3);
    child
        .complete(ChildResultStatus::Succeeded, Some("unpublished answer"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    // Model the native receipt after the scripted driver's actual reap. The
    // lease must be released before recovery can prove physical containment.
    let spawn = &plane.registry.config.spawn;
    let incarnation = crate::runtime::subagent::child_conversation_store_path(
        spawn.product_root.root(),
        &spawn.session_id,
        &accepted.child_conversation_id,
    )
    .parent()
    .unwrap()
    .join("incarnation-agent-workspace-proof");
    std::fs::create_dir_all(&incarnation).unwrap();
    let lease = ChildPhysicalLease::for_test(
        incarnation,
        accepted.subagent_id.clone(),
        accepted.child_conversation_id.clone(),
    )
    .unwrap();
    lease.publish_quiescent().unwrap();
    drop(lease);
    let evidence =
        crate::runtime::recovery::RecoveryEvidence::reconstruct(plane.store.as_ref()).unwrap();
    crate::runtime::recovery::RecoveryPlan::classify(&evidence)
        .reconcile(plane.store.as_ref(), &SystemClock)
        .unwrap();
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    let (agent, activation) = recovered
        .agent_snapshot_with_activation(&accepted.child_agent_id)
        .unwrap();
    assert_eq!(agent.state, AgentState::Inactive);
    assert_eq!(
        activation.workspace_resource_state,
        SubagentWorkspaceResourceState::None
    );
    assert!(
        activation.handoff.is_none(),
        "inspection never transfers Agent workspace ownership"
    );
    assert!(matches!(
        recovered
            .dispose_retained_workspace(&accepted.subagent_id)
            .await
            .unwrap(),
        SubagentWorkspaceDisposal::NoRetainedWorkspace(_)
    ));
    assert!(worktree.is_dir());
    assert_eq!(
        recovered
            .agent_snapshot_with_activation(&accepted.child_agent_id)
            .unwrap(),
        (agent, activation)
    );
    assert!(!events(&plane).iter().any(|event| matches!(event, crate::events::types::RuntimeEvent::SubagentWorkspaceDisposalStarted { subagent_id, .. } if *subagent_id == accepted.subagent_id)));
}
