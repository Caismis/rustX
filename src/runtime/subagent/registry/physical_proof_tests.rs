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
        workspace.acquire(&CancellationSignal::new()).await.is_err(),
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
    .join("physical-settlement").join(accepted.subagent_id.as_str());
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


#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parked_recovery_probe_does_not_hold_registry_mutex() {
    let plane = plane(4);
    let child = stage_with_unresolved_anchor(&plane);
    let accepted = start(&plane, &spec("unproven recovery probe")).await;
    child.complete(ChildResultStatus::Succeeded, Some("unproven")).await;
    plane.registry.wait_until_settled(&accepted.subagent_id).await.unwrap();
    // Restore without a Tokio executor: this fixture explicitly owns the probe
    // and competing pass below. Otherwise restore starts an independent periodic
    // reconciler whose brief claim lock can race try_lock even while our hook is
    // correctly parked outside the mutex.
    let config = plane.registry.config.clone();
    let store = plane.store.clone();
    let recovered = std::thread::spawn(move || {
        let recovered = SubagentRegistry::new(config);
        recovered.restore_agents(store.as_ref()).unwrap();
        recovered
    }).join().unwrap();
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::sync_channel(0);
    recovered.state.lock().unwrap().recovery_probe_hook = Some(Box::new(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
    }));
    let probing = recovered.clone();
    let task = std::thread::spawn(move || probing.reconcile_recovered_settlements());
    entry.await.unwrap();
    assert!(recovered.state.try_lock().is_ok(), "probe holds its exact claim, not the registry mutex");
    assert_eq!(recovered.list_agents(MAX_AGENT_LIST_LIMIT).matched, 1);
    assert_eq!(recovered.agent_snapshot(&accepted.child_agent_id).unwrap().state, AgentState::Unavailable);
    assert!(matches!(recovered.interrupt_agent(&accepted.child_agent_id).await, Err(AgentControlError::Settlement)));
    // A concurrent pass sees the claim and returns without probing/committing
    // the same activation. No elapsed delay determines either ordering.
    recovered.reconcile_recovered_settlements();
    release.send(()).unwrap();
    task.join().unwrap();
    assert_eq!(recovered.unproven_settlements(), vec![accepted.subagent_id]);
}

#[test]
fn recovered_activation_constructors_preserve_domain_defaults() {
    let plane = plane(4);
    let frozen = spec("recovery constructor").authority.resolved;
    let mut evidence = crate::runtime::recovery::SubagentEvidence {
        subagent_id: SubagentId::for_conversation(&plane.conversation_id, 1),
        child_agent_id: AgentId::new("recovered-agent"),
        child_conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000002"),
        origin: AgentActivationOrigin::ClientControl,
        agent: frozen.agent.as_str().to_owned(),
        definition_digest: frozen.definition_digest.as_str().to_owned(),
        profile_digest: frozen.profile_digest().as_str().to_owned(),
        ownership: SubagentOwnershipKind::Normal,
        started_at: Utc::now(),
        workspace: WorkspaceSnapshot::shared(plane.dir.path().join("workspace")),
    };
    for ownership in [SubagentOwnershipKind::Normal, SubagentOwnershipKind::Workflow] {
        evidence.ownership = ownership;
        let record = SubagentRecord::recovered(AgentId::new("parent"), &evidence).unwrap();
        assert_eq!(record.ownership, ownership);
        assert_eq!(record.delegate_delivery, DelegateDelivery::Started,
            "replay proves neither non-delivery nor canonical input acceptance");
        assert!(!record.physical_settlement_proven);
        assert!(record.terminal.is_none() && record.control.is_none() && record.profile.is_none());
        assert!(record.steer_tickets.is_empty());
        assert_eq!(record.lifecycle, SubagentLifecycle::Interrupted);
        assert_eq!(record.notification, if ownership == SubagentOwnershipKind::Normal {
            NotificationState::Delivered
        } else { NotificationState::None });
    }
}
