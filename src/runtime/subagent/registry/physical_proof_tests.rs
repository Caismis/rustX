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
}

#[tokio::test]
async fn unproven_control_or_cleanup_terminal_blocks_live_and_recovered_agent() {
    #[derive(Default)]
    struct OwnerObserver(Mutex<Vec<(Option<AgentSnapshot>, SubagentSnapshot)>>);
    impl SubagentObserver for OwnerObserver {
        fn on_snapshot(
            &self,
            agent: Option<&AgentSnapshot>,
            activation: &SubagentSnapshot,
            _cut: crate::runtime::identity::ObservationCollectionCut,
            _ordinal: Option<u64>,
        ) {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parked_recovery_probe_does_not_hold_registry_mutex() {
    let plane = plane(4);
    let child = stage_with_unresolved_anchor(&plane);
    let accepted = start(&plane, &spec("unproven recovery probe")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("unproven"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
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
    })
    .join()
    .unwrap();
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::sync_channel(0);
    recovered.state.lock().unwrap().recovery_probe_hook = Some(Box::new(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
    }));
    let probing = recovered.clone();
    let task = std::thread::spawn(move || probing.reconcile_recovered_settlements());
    entry.await.unwrap();
    assert!(
        recovered.state.try_lock().is_ok(),
        "probe holds its exact claim, not the registry mutex"
    );
    assert_eq!(recovered.list_agents(MAX_AGENT_LIST_LIMIT).matched, 1);
    assert_eq!(
        recovered
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Unavailable
    );
    assert!(matches!(
        recovered.interrupt_agent(&accepted.child_agent_id).await,
        Err(AgentControlError::Settlement)
    ));
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
        workspace: plane.dir.path().join("workspace"),
    };
    for ownership in [
        SubagentOwnershipKind::Normal,
        SubagentOwnershipKind::Workflow,
    ] {
        evidence.ownership = ownership;
        let record = SubagentRecord::recovered(AgentId::new("parent"), &evidence).unwrap();
        assert_eq!(record.ownership, ownership);
        assert_eq!(
            record.delegate_delivery,
            DelegateDelivery::Started,
            "replay proves neither non-delivery nor canonical input acceptance"
        );
        assert!(!record.physical_settlement_proven);
        assert!(record.terminal.is_none() && record.control.is_none() && record.profile.is_none());
        assert!(record.steer_tickets.is_empty());
        assert_eq!(record.lifecycle, SubagentLifecycle::Interrupted);
        assert_eq!(
            record.notification,
            if ownership == SubagentOwnershipKind::Normal {
                NotificationState::Delivered
            } else {
                NotificationState::None
            }
        );
    }
}

#[tokio::test]
async fn post_result_protocol_failure_publishes_once_after_physical_settlement() {
    for duplicate in [true, false] {
        let plane = plane(1);
        let mut child = stage_stubborn(&plane);
        let accepted = start(&plane, &start_spec("terminal protocol failure")).await;
        child.accept_delegate().await;
        child
            .send_result(ChildResultStatus::Succeeded, Some("must not publish"))
            .await;
        assert_eq!(
            super::super::ipc::read_parent_frame(&mut child.peer)
                .await
                .unwrap(),
            None
        );
        assert!(published_terminal_states(&plane, &accepted.subagent_id).is_empty());
        assert!(
            plane
                .registry
                .snapshot(&accepted.subagent_id)
                .unwrap()
                .state
                .is_active()
        );
        assert_eq!(
            plane
                .registry
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.lifecycle.is_active())
                .count(),
            1,
            "Result alone retains counted capacity"
        );
        if duplicate {
            child
                .send_result(ChildResultStatus::Succeeded, Some("duplicate"))
                .await;
        } else {
            super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::DelegateAccepted)
                .await
                .unwrap();
        }
        let pid = child.pid;
        drop(child);
        let terminal = plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
        assert_eq!(terminal.state, SubagentState::Interrupted);
        assert!(terminal.is_settled());
        assert!(matches!(
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                None
            ),
            Err(nix::errno::Errno::ESRCH)
        ));
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Interrupted]
        );
        assert_eq!(
            plane
                .registry
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.lifecycle.is_active())
                .count(),
            0
        );
        assert_eq!(
            plane
                .registry
                .wait_until_settled(&accepted.subagent_id)
                .await
                .unwrap(),
            terminal
        );
        assert_eq!(
            published_terminal_states(&plane, &accepted.subagent_id),
            vec![SubagentTerminalState::Interrupted]
        );
    }
}
