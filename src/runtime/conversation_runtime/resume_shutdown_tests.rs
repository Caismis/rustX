// Included in conversation_runtime::tests to use the real lifecycle owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn shutdown_waits_for_reserved_resume_physical_and_durable_rollback() {
    use crate::events::types::{AgentActivationAdmissionPhase, RuntimeEvent};
    use crate::runtime::subagent::ipc::{
        ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
        write_child_frame,
    };
    use crate::runtime::subagent::{
        ActivationAdmission, AgentActivationOrigin, AgentControlError, AgentState,
        DurableAgentAuthority, InheritedExecutionPolicy, ResumeTestGates, SubagentStartError,
        SubagentStartOutcome, SubagentStartSpec, SubagentTerminalMode,
    };
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
    let drained = Arc::new(tokio::sync::Notify::new());
    *runtime.inner.probe.lock().unwrap() = Some(CoordinatorProbe {
        drain_linearization: Some(drained.clone()),
        ..CoordinatorProbe::default()
    });
    runtime.activate();
    let spec = SubagentStartSpec {
        authority: DurableAgentAuthority {
            execution_policy: InheritedExecutionPolicy::default(),
            resolved: test_resolved_subagent("explore"),
            approval_mode: ApprovalMode::Policy,
        },
        admission: ActivationAdmission {
            task: "initial".into(),
            context: None,
            origin: AgentActivationOrigin::CreationTool {
                tool_call_id: ToolCallId::new("initial"),
            },
            terminal: SubagentTerminalMode::Normal,
        },
    };
    let (child, mut peer) = stage_runtime_test_child(&dir.path().join("first-child"));
    registry.push_staged_override(child);
    let prepared = registry
        .prepare(&spec, &CancellationSignal::new())
        .await
        .unwrap();
    let SubagentStartOutcome::Accepted(first) = registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .unwrap()
    else {
        panic!("initial ownership");
    };
    assert!(matches!(
        read_parent_frame(&mut peer).await.unwrap(),
        Some(ParentFrame::Delegate(_))
    ));
    write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("done".into()),
            diagnostic: None,
        }),
    )
    .await
    .unwrap();
    assert!(
        registry
            .wait_until_settled(&first.subagent_id)
            .await
            .unwrap()
            .is_settled()
    );
    assert_eq!(
        registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );

    let staged_root = dir.path().join("reserved-child");
    let (child, mut reserved_peer) = stage_runtime_test_child(&staged_root);
    registry.push_staged_override(child);
    let (staged, staged_rx) = tokio::sync::oneshot::channel();
    let (release_staged, stage_release) = tokio::sync::oneshot::channel();
    let (published, published_rx) = tokio::sync::oneshot::channel();
    let (release_published, publication_release) = tokio::sync::oneshot::channel();
    registry.install_resume_test_gates(ResumeTestGates {
        staged,
        release_staged: stage_release,
        published,
        release_published: publication_release,
    });
    let sender = tokio::spawn({
        let registry = registry.clone();
        let agent = first.child_agent_id.clone();
        async move {
            registry
                .send_message(
                    &agent,
                    "reserved input",
                    AgentActivationOrigin::ClientControl,
                    CancellationSignal::new(),
                )
                .await
        }
    });
    staged_rx.await.unwrap();
    let owner = registry.agent_snapshot(&first.child_agent_id).unwrap();
    assert_eq!(owner.state, AgentState::Admitting);
    let reserved = owner.current_activation.unwrap();
    assert!(store.read_events(None, 128).unwrap().events.iter().any(|event| matches!(
        &event.event, RuntimeEvent::AgentActivationAdmission { activation_id, phase: AgentActivationAdmissionPhase::Reserved, .. } if activation_id == &reserved
    )));
    assert!(
        staged_root.exists(),
        "the reservation owns a staged physical child"
    );
    let shutdown = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.shutdown().await }
    });
    drained.notified().await;
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );
    assert!(!shutdown.is_finished());
    release_staged.send(()).unwrap();
    published_rx.await.unwrap();
    // This gate is after rollback + durable publication and before admission Drop.
    assert!(
        !staged_root.exists(),
        "physical rollback removed its exact incarnation"
    );
    assert!(
        read_parent_frame(&mut reserved_peer)
            .await
            .unwrap()
            .is_none(),
        "no Delegate reached the rolled-back child"
    );
    let events = store.read_events(None, 128).unwrap().events;
    assert_eq!(events.iter().filter(|event| matches!(
        &event.event, RuntimeEvent::AgentActivationAdmission { activation_id, phase: AgentActivationAdmissionPhase::RolledBack { physical_settlement_proven: true }, .. } if activation_id == &reserved
    )).count(), 1);
    assert!(!events.iter().any(|event| matches!(
        &event.event, RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. } if subagent_id == &reserved
    )));
    assert_eq!(
        registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive,
        "clean rollback never poisons Agent authority"
    );
    assert!(
        !runtime.inner.lifecycle.mark_quiescent(),
        "the live admission guard prevents Quiescent even after the rollback event commits"
    );
    assert!(!shutdown.is_finished());
    release_published.send(()).unwrap();
    assert!(matches!(
        sender.await.unwrap(),
        Err(AgentControlError::Start(
            SubagentStartError::Cancelled | SubagentStartError::ConversationInactive
        ))
    ));
    shutdown.await.unwrap().unwrap();
    assert!(runtime.is_quiescent());
    drop(peer);
    drop(runtime);
    drop(registry);

    let (reopened, _, registry) =
        headless_runtime_over_store_with_subagents(&dir, conversation.as_str(), store, None).await;
    assert_eq!(
        registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
    reopened.activate();
    let (child, mut peer) = stage_runtime_test_child(&dir.path().join("reopened-child"));
    registry.push_staged_override(child);
    let send = registry.send_message(
        &first.child_agent_id,
        "after reopen",
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    );
    let (accepted, ()) = tokio::join!(send, async {
        assert!(matches!(
            read_parent_frame(&mut peer).await.unwrap(),
            Some(ParentFrame::Delegate(_))
        ));
        write_child_frame(&mut peer, &ChildFrame::DelegateAccepted)
            .await
            .unwrap();
    });
    let accepted = accepted.unwrap();
    assert_ne!(accepted.activation_id, reserved);
    assert_ne!(accepted.activation_id, first.subagent_id);
    assert_eq!(
        registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .conversation_id,
        first.child_conversation_id
    );
    write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("done again".into()),
            diagnostic: None,
        }),
    )
    .await
    .unwrap();
    assert!(
        registry
            .wait_until_settled(&accepted.activation_id)
            .await
            .unwrap()
            .is_settled()
    );
    reopened.shutdown().await.unwrap();
}
