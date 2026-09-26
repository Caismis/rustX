// Included in conversation_runtime::tests: use the real registry, durable
// frontier, lifecycle drain, and child driver rather than a projected fixture.
async fn finish_finite_workflow_child(
    dir: &tempfile::TempDir,
    registry: &crate::runtime::subagent::SubagentRegistry,
    store: &crate::durable::SqliteConversationStore,
    abandon_publication: bool,
) -> crate::runtime::subagent::SubagentAccepted {
    use crate::runtime::subagent::ipc::{
        ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
        write_child_frame,
    };
    use crate::runtime::subagent::{
        ActivationAdmission, AgentActivationOrigin, DurableAgentAuthority,
        InheritedExecutionPolicy, SubagentPublication, SubagentStartOutcome, SubagentStartSpec,
        SubagentTerminalMode,
    };
    let node = crate::runtime::workflow::test_instance("finite_recovery", "child");
    let spec = SubagentStartSpec {
        authority: DurableAgentAuthority {
            execution_policy: InheritedExecutionPolicy::default(),
            resolved: test_resolved_subagent("reviewer"),
            approval_mode: ApprovalMode::Policy,
        },
        admission: ActivationAdmission {
            task: "finite structured child".into(),
            context: None,
            origin: AgentActivationOrigin::Workflow {
                node_id: Box::new(node.clone()),
            },
            terminal: SubagentTerminalMode::WorkflowOutput {
                output_schema: serde_json::json!({"type":"object"}),
                workflow_id: crate::runtime::workflow::WorkflowId::parse("finite_recovery")
                    .unwrap(),
                run_id: node.block.run.clone(),
                node_id: Box::new(node),
            },
        },
    };
    let root = dir.path().join("finite-child");
    let (child, mut peer) = stage_runtime_test_child(&root);
    registry.push_staged_override(child);
    let cancellation = CancellationSignal::new();
    let prepared = registry.prepare(&spec, &cancellation).await.unwrap();
    let SubagentStartOutcome::Accepted(accepted) =
        registry.commit(prepared, &cancellation).await.unwrap()
    else {
        panic!("finite child ownership");
    };
    assert!(matches!(
        read_parent_frame(&mut peer).await.unwrap(),
        Some(ParentFrame::Delegate(_))
    ));
    // Delegate receipt is the ownership gate. No terminal can publish until
    // this peer supplies the result after the transaction fault is armed.
    if abandon_publication {
        store.arm_fail_event_times(3);
    }
    write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("{}".into()),
            diagnostic: None,
        }),
    )
    .await
    .unwrap();
    let settled = registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert!(
        !root.exists(),
        "the live native driver reaped and removed its incarnation"
    );
    assert_eq!(
        settled.settlement.publication,
        if abandon_publication {
            SubagentPublication::Abandoned
        } else {
            SubagentPublication::Committed
        }
    );
    accepted
}

#[tokio::test]
async fn finite_workflow_committed_physical_proof_survives_runtime_reopen() {
    use crate::events::types::SubagentOwnershipKind;
    use crate::runtime::subagent::{AgentControlError, SubagentState};
    let dir = tempfile::tempdir().unwrap();
    let conversation = ConversationId::generate();
    let store =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    let (runtime, _, registry) = headless_runtime_over_store_with_subagents(
        &dir,
        conversation.as_str(),
        store.clone(),
        None,
    )
    .await;
    runtime.activate();
    let accepted = finish_finite_workflow_child(&dir, &registry, store.as_ref(), false).await;
    assert_eq!(registry.with_goal_idle(|| true), Some(true));
    runtime.shutdown().await.unwrap();
    drop(runtime);
    drop(registry);

    let (reopened, model, registry) =
        headless_runtime_over_store_with_subagents(&dir, conversation.as_str(), store, None).await;
    let snapshot = registry.snapshot(&accepted.subagent_id).unwrap();
    assert_eq!(snapshot.ownership, SubagentOwnershipKind::Workflow);
    assert_eq!(snapshot.state, SubagentState::Succeeded);
    assert!(snapshot.is_settled());
    assert_eq!(registry.with_goal_idle(|| true), Some(true));
    assert!(
        registry
            .list_agents(crate::runtime::subagent::MAX_AGENT_LIST_LIMIT)
            .agents
            .is_empty()
    );
    assert!(matches!(
        registry.wait_agent(&accepted.child_agent_id).await,
        Err(AgentControlError::Unknown(_))
    ));
    reopened.activate();
    assert!(
        model.requests().is_empty(),
        "finite history grants no resumed turn"
    );
    reopened.shutdown().await.unwrap();
    assert!(reopened.is_quiescent());
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One lease boundary covers live owner exclusion and both shutdown outcomes.
async fn finite_workflow_recovery_tracks_physical_obligation_until_exact_owner_proof() {
    use crate::events::types::{RuntimeEvent, SubagentOwnershipKind, SubagentTerminalState};
    use crate::runtime::subagent::physical_recovery::ChildPhysicalLease;
    use crate::runtime::subagent::{
        AgentControlError, SubagentPhysicalSettlement, SubagentPublication, SubagentState,
    };
    for publish_receipt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap(),
        );
        let (runtime, _, registry) = headless_runtime_over_store_with_subagents(
            &dir,
            conversation.as_str(),
            store.clone(),
            None,
        )
        .await;
        runtime.activate();
        let accepted = finish_finite_workflow_child(&dir, &registry, store.as_ref(), true).await;
        assert!(
            runtime.shutdown().await.is_err(),
            "the first owner lost its terminal publication"
        );
        drop(runtime);
        drop(registry);
        let incarnation = crate::runtime::subagent::child_conversation_store_path(
            &dir.path().join("subagents"),
            &crate::runtime::identity::SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
            &accepted.child_conversation_id,
        )
        .parent()
        .unwrap()
        .join("incarnation-finite-recovery");
        std::fs::create_dir_all(&incarnation).unwrap();
        // The real original driver has already reaped. This fixture retains
        // the exact native receipt writer lease to prove a receipt alone can
        // never permit overlap with a surviving physical incarnation.
        let lease = ChildPhysicalLease::for_test(
            incarnation,
            accepted.subagent_id.clone(),
            accepted.child_conversation_id.clone(),
        )
        .unwrap();
        if publish_receipt {
            lease.publish_quiescent().unwrap();
        }
        tokio::time::pause();
        let (reopened, model, registry) = headless_runtime_over_store_with_subagents(
            &dir,
            conversation.as_str(),
            store.clone(),
            None,
        )
        .await;
        let recovered = registry.snapshot(&accepted.subagent_id).unwrap();
        assert_eq!(recovered.ownership, SubagentOwnershipKind::Workflow);
        assert_eq!(recovered.state, SubagentState::Interrupted);
        assert_eq!(
            recovered.settlement.publication,
            SubagentPublication::Committed
        );
        assert_eq!(
            recovered.settlement.physical,
            SubagentPhysicalSettlement::Unproven
        );
        assert_eq!(
            registry.with_goal_idle(|| true),
            None,
            "an occupied incarnation lease excludes proof even with a receipt"
        );
        assert_eq!(
            registry.unproven_settlements(),
            std::slice::from_ref(&accepted.subagent_id)
        );
        assert!(
            registry
                .list_agents(crate::runtime::subagent::MAX_AGENT_LIST_LIMIT)
                .agents
                .is_empty()
        );
        assert!(matches!(
            registry.wait_agent(&accepted.child_agent_id).await,
            Err(AgentControlError::Unknown(_))
        ));
        assert!(model.requests().is_empty());
        let drained = Arc::new(tokio::sync::Notify::new());
        *reopened.inner.probe.lock().unwrap() = Some(CoordinatorProbe {
            drain_linearization: Some(drained.clone()),
            ..CoordinatorProbe::default()
        });
        reopened.activate();
        let shutdown = tokio::spawn({
            let runtime = reopened.clone();
            async move { runtime.shutdown().await }
        });
        drained.notified().await;
        assert_eq!(
            reopened.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
        assert!(!shutdown.is_finished());
        drop(lease);
        registry.reconcile_recovered_settlements();
        let events = store.read_events(None, 128).unwrap().events;
        assert!(events.iter().any(|event| matches!(
            event.event,
            RuntimeEvent::SubagentTerminalSettled {
                state: SubagentTerminalState::Interrupted,
                physical_settlement_proven: false,
                ..
            }
        )));
        let proofs = events.iter().filter(|event| matches!(&event.event, RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } if subagent_id == &accepted.subagent_id)).count();
        assert_eq!(proofs, usize::from(publish_receipt));
        if publish_receipt {
            assert_eq!(registry.with_goal_idle(|| true), Some(true));
            shutdown.await.unwrap().unwrap();
            assert!(reopened.is_quiescent());
            let terminal = registry.snapshot(&accepted.subagent_id).unwrap();
            assert_eq!(
                terminal.state,
                SubagentState::Interrupted,
                "physical proof cannot manufacture the lost logical answer"
            );
            assert!(terminal.is_settled());
        } else {
            assert_eq!(
                registry.with_goal_idle(|| true),
                None,
                "a free lease without a quiescence receipt proves no descendant containment"
            );
            let error = shutdown.await.unwrap().unwrap_err();
            assert!(matches!(
                error,
                super::ShutdownError::RuntimeOwnedSettlement { detail }
                    if detail.contains("physical settlement is unresolved")
            ));
            assert!(!reopened.is_quiescent());
        }
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "recovery never reattaches or replays the finite activation"
        );
        assert!(model.requests().is_empty());
        drop(reopened);
        drop(registry);
        tokio::time::resume();
    }
}
