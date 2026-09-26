// Included in registry::tests; the owner fixture supplies gated physical settlement.
#[tokio::test]
async fn agent_bootstrap_ignores_activation_record_iteration_order() {
    let plane = plane(4);
    let first_child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("bootstrap one Agent")).await;
    first_child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let mut latest = first.subagent_id.clone();
    for message in ["second", "third"] {
        let mut child = stage_exit0(&plane);
        let signal = CancellationSignal::new();
        let (accepted, _) = tokio::join!(
            plane.registry.send_message(
                &first.child_agent_id,
                message,
                AgentActivationOrigin::ClientControl,
                signal.clone()
            ),
            child.accept_delegate(),
        );
        latest = accepted.unwrap().activation_id;
        child
            .send_result(ChildResultStatus::Succeeded, Some(message))
            .await;
        plane.registry.wait_until_settled(&latest).await.unwrap();
    }
    // Reproduce recovery's nonchronological finite record order: 1, 3, 2.
    // The authoritative Agent still names activation 3.
    {
        let mut state = plane.registry.state.lock().unwrap();
        state.records.swap(1, 2);
        let index = state
            .records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.subagent_id.clone(), index))
            .collect();
        state.index = index;
    }
    let observed = Arc::new(RecordingObserver::default());
    let owners = plane
        .registry
        .install_observer_and_agent_snapshots(observed.clone());
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].0.agent_id, first.child_agent_id);
    assert_eq!(owners[0].0.latest_activation, latest);
    assert_eq!(owners[0].1.subagent_id, latest);
    assert_eq!(owners[0].0.state, AgentState::Inactive);
    assert_eq!(
        observed.0.lock().unwrap().len(),
        1,
        "bootstrap emits only exact latest activation"
    );
    let mut fourth_child = stage_exit0(&plane);
    let signal = CancellationSignal::new();
    let (fourth, _) = tokio::join!(
        plane.registry.send_message(
            &first.child_agent_id,
            "fourth",
            AgentActivationOrigin::ClientControl,
            signal.clone()
        ),
        fourth_child.accept_delegate(),
    );
    let fourth = fourth.unwrap();
    let owner = plane
        .registry
        .agent_snapshot(&first.child_agent_id)
        .unwrap();
    assert_eq!(owner.current_activation, Some(fourth.activation_id.clone()));
    assert_ne!(fourth.activation_id, latest);
    fourth_child
        .send_result(ChildResultStatus::Succeeded, Some("fourth"))
        .await;
    plane
        .registry
        .wait_until_settled(&fourth.activation_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn proven_child_loss_has_identical_resume_authority_before_and_after_reopen() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let admitted = start(&plane, &start_spec("unexpected child loss")).await;
    child.accept_delegate().await;
    // EOF after exact delegation acceptance, with the staged process exiting 0,
    // proves direct-child containment without inventing a semantic result.
    drop(child);
    let lost = plane
        .registry
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
    assert_eq!(lost.state, SubagentState::Interrupted);
    assert!(events(&plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentTerminalPublished {
            state: SubagentTerminalState::Interrupted,
            physical_settlement_proven: true,
            ..
        }
    )));
    let recovered = SubagentRegistry::new(plane.registry.config.clone());
    recovered.restore_sequence_watermark(1);
    recovered.restore_agents(plane.store.as_ref()).unwrap();
    // The same committed settlement grants both resident and recovered owners
    // workspace admission; this inspection performs no activation side effect.
    for registry in [&plane.registry, &recovered] {
        let workspace = registry.state.lock().unwrap().agents[&admitted.child_agent_id]
            .workspace
            .clone();
        workspace
            .acquire()
            .await
            .expect("proven settlement admits another physical lease")
            .settle();
    }
    let recovered_plane = TestPlane {
        registry: recovered,
        ..plane
    };
    let mut next = stage_exit0(&recovered_plane);
    let signal = CancellationSignal::new();
    let (resumed, _) = tokio::join!(
        recovered_plane.registry.send_message(
            &admitted.child_agent_id,
            "continue after proven loss",
            AgentActivationOrigin::ClientControl,
            signal.clone()
        ),
        next.accept_delegate(),
    );
    let resumed = resumed.unwrap();
    assert_eq!(resumed.agent_id, admitted.child_agent_id);
    next.send_result(ChildResultStatus::Succeeded, Some("recovered report"))
        .await;
    recovered_plane
        .registry
        .wait_until_settled(&resumed.activation_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn domain_tool_metadata_matches_registered_authority() {
    use crate::tools::native::{NativeToolPolicies, NativeToolResources};
    let plane = plane(4);
    let artifacts = plane.dir.path().join("tool-artifacts");
    let background = crate::tools::background::ConversationBackgroundRegistry::new(
        plane.conversation_id.clone(),
        crate::tools::background::BackgroundResources {
            mailbox: plane.registry.config.mailbox.clone(),
            workspace: crate::tools::workspace::Workspace::new(plane.dir.path().join("workspace"))
                .unwrap(),
            artifacts: crate::tools::artifacts::ArtifactStore::new(
                plane.conversation_id.clone(),
                &artifacts,
            )
            .unwrap(),
            tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                plane.conversation_id.clone(),
                artifacts.join("output"),
            )
            .unwrap(),
            clock: Arc::new(SystemClock),
            event_sink: None,
        },
    );
    let catalog = super::super::AgentCatalog::empty();
    for owns_agents in [false, true] {
        let policies = NativeToolPolicies::default();
        let metadata = crate::tools::native::definitions(policies, owns_agents.then_some(&catalog));
        let mut executable = crate::tools::executor::ToolRegistry::new();
        crate::tools::native::register_native_tools(
            &mut executable,
            NativeToolResources {
                background: background.clone(),
                subagents: owns_agents.then(|| plane.registry.clone()),
                subagent_catalog: catalog.clone(),
            },
            policies,
        )
        .unwrap();
        assert_eq!(
            metadata
                .into_iter()
                .map(|(definition, _)| definition)
                .collect::<Vec<_>>(),
            executable.definitions()
        );
        let names = executable.names();
        assert_eq!(names.contains(&"send_message"), owns_agents);
        assert!(names.contains(&"job_wait"));
        // Both native and Workflow children share the frozen child registration
        // boundary, which must reject every domain control lacking its owner.
        for definition in executable
            .definitions()
            .into_iter()
            .filter(|d| crate::tools::native::is_domain_control(&d.name))
        {
            let mut child = crate::tools::executor::ToolRegistry::new();
            assert!(
                crate::tools::native::register_subagent_child_tools(&mut child, &[definition])
                    .is_err()
            );
            assert!(child.definitions().is_empty());
        }
    }
}

#[tokio::test]
async fn admission_receipts_release_observation_frontier_after_owner_installation() {
    use crate::runtime::observation::{ConversationObservation, PendingObservations};
    struct AdmissionObserver(Arc<PendingObservations>);
    impl SubagentObserver for AdmissionObserver {
        fn observe_agent(&self, snapshot: &AgentSnapshot) {
            self.0.push(ConversationObservation::Agent {
                snapshot: Box::new(snapshot.clone()),
            });
        }
        fn observe_agent_committed(&self, snapshot: &AgentSnapshot, sequence: u64) {
            self.0.push(ConversationObservation::Published {
                journal_sequence: sequence,
                observation: Box::new(ConversationObservation::Agent {
                    snapshot: Box::new(snapshot.clone()),
                }),
            });
        }
        fn on_snapshot(&self, _snapshot: &SubagentSnapshot) {}
    }
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("admission observation cut")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("first"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let queue = Arc::new(PendingObservations::new());
    plane.store.observe_journal(queue.clone()).unwrap();
    plane
        .registry
        .install_observer_and_agent_snapshots(Arc::new(AdmissionObserver(queue.clone())));
    queue.drain();
    plane.registry.state.lock().unwrap().allocation_failure = Some(
        super::super::process::SpawnError::ConversationIdentityInUse {
            conversation_id: first.child_conversation_id.clone(),
            path: plane.runtime_root.clone(),
        },
    );
    assert!(
        plane
            .registry
            .send_message(
                &first.child_agent_id,
                "rollback",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new()
            )
            .await
            .is_err()
    );
    let frontier = plane.store.presentation_frontier().unwrap();
    let batches = queue.drain();
    let states = batches
        .iter()
        .flat_map(|batch| {
            let ConversationObservation::JournalBatch {
                through,
                observations,
            } = batch
            else {
                panic!("journal batch")
            };
            assert_eq!(
                *through,
                Some(frontier),
                "all reserved and rollback receipts are published"
            );
            observations
                .iter()
                .filter_map(|observation| match observation {
                    ConversationObservation::Agent { snapshot } => {
                        Some((snapshot.state, snapshot.current_activation.clone()))
                    }
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    assert!(states.contains(&(
        AgentState::Admitting,
        Some(SubagentId::for_conversation(&plane.conversation_id, 2))
    )));
    assert!(states.contains(&(AgentState::Inactive, None)));
    assert!(queue.drain().is_empty());
}
