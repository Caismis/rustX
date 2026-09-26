// Physical outcome injection occurs at the driver/registry settlement boundary.
// No timing assumptions or sleeps determine the winning terminal generation.
async fn assert_unproven_agent_stops_at_exact_activation(
    registry: &SubagentRegistry,
    agent_id: &AgentId,
    activation_id: &SubagentId,
) {
    let (agent, activation) = registry.agent_snapshot_with_activation(agent_id).unwrap();
    assert_eq!(agent.state, AgentState::Stopping);
    assert_eq!(agent.current_activation.as_ref(), Some(activation_id));
    assert_eq!(&agent.latest_activation, activation_id);
    assert!(!activation.settled);
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
        Err(AgentControlError::Stopping)
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
    for control_failure in [false, true] {
        let plane = plane(4);
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
            !terminal.settled,
            "logical failure is not physical settlement proof"
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
