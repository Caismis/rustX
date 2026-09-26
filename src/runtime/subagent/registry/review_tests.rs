// Review regressions use registry mutex boundaries, control frames and first-poll gates.
#[tokio::test]
async fn admitting_generation_is_interruptible_and_waitable_before_owner_task_runs() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("first")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let mut send = Box::pin(plane.registry.send_message(
        &first.child_agent_id,
        "next",
        AgentActivationOrigin::ClientControl,
        CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut send).is_pending());
    let snapshot = plane
        .registry
        .agent_snapshot(&first.child_agent_id)
        .unwrap();
    assert_eq!(snapshot.state, AgentState::Admitting);
    let generation = snapshot.current_activation.unwrap();
    let mut wait = Box::pin(plane.registry.wait_agent(&first.child_agent_id));
    assert!(futures_util::poll!(&mut wait).is_pending());
    let mut interrupt = Box::pin(plane.registry.interrupt_agent(&first.child_agent_id));
    assert!(futures_util::poll!(&mut interrupt).is_pending());
    assert!(matches!(
        send.await,
        Err(AgentControlError::Start(SubagentStartError::Cancelled))
    ));
    for result in [wait.await.unwrap(), interrupt.await.unwrap()] {
        assert_eq!(result.activation_id, Some(generation.clone()));
        assert!(result.outcome.is_none(), "no execution committed");
    }
    assert_eq!(plane.registry.all_snapshots().len(), 1);
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
}

#[tokio::test]
async fn resume_success_requires_exact_child_canonical_input_acknowledgement() {
    for acknowledge in [false, true] {
        let plane = plane(4);
        let first_child = stage_exit0(&plane);
        let first = start(&plane, &start_spec("creation")).await;
        first_child
            .complete(ChildResultStatus::Succeeded, Some("first"))
            .await;
        plane
            .registry
            .wait_until_settled(&first.subagent_id)
            .await
            .unwrap();
        let mut child = stage_exit0(&plane);
        let message_origin = AgentActivationOrigin::MessageTool {
            tool_call_id: ToolCallId::new("send-call"),
        };
        let mut send = Box::pin(plane.registry.send_message(
            &first.child_agent_id,
            "resume input",
            message_origin.clone(),
            CancellationSignal::new(),
        ));
        assert!(futures_util::poll!(&mut send).is_pending());
        let ParentFrame::Delegate(delegate) = child.read_frame().await else {
            panic!("delegate");
        };
        assert_eq!(delegate.task, "resume input");
        assert!(
            futures_util::poll!(&mut send).is_pending(),
            "ownership and Delegate write are not input acceptance"
        );
        let current = plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap();
        assert_eq!(
            plane
                .registry
                .snapshot(&current.latest_activation)
                .unwrap()
                .origin,
            message_origin
        );
        if acknowledge {
            super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::DelegateAccepted)
                .await
                .unwrap();
            let accepted = send.await.unwrap();
            assert_eq!(accepted.activation_id, current.latest_activation);
            child
                .send_result(ChildResultStatus::Succeeded, Some("done"))
                .await;
        } else {
            // Control loss at the ambiguous frontier: the parent cannot claim success.
            drop(child);
            assert!(matches!(send.await, Err(AgentControlError::Admission(_))));
        }
        plane
            .registry
            .wait_agent(&first.child_agent_id)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn seal_open_restores_same_activation_before_admitting_another_message() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("seal")).await;
    child.accept_delegate().await;
    super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::SealRequested)
        .await
        .unwrap();
    assert!(matches!(child.read_frame().await, ParentFrame::SealGranted));
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Stopping
    );
    assert!(
        plane
            .registry
            .admit_guidance(&first.subagent_id, "late")
            .is_err()
    );
    super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::SealOpen)
        .await
        .unwrap();
    assert!(matches!(
        child.read_frame().await,
        ParentFrame::AdmissionReopened
    ));
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Active
    );
    let (_, answer, _ticket) = plane
        .registry
        .admit_guidance(&first.subagent_id, "same activation")
        .unwrap();
    let ParentFrame::Guidance(guidance) = child.read_frame().await else {
        panic!("guidance");
    };
    super::super::ipc::write_child_frame(
        &mut child.peer,
        &ChildFrame::GuidanceResult(super::super::ipc::GuidanceResultFrame {
            guidance_id: guidance.guidance_id,
            outcome: super::super::ipc::ChildGuidanceOutcome::Accepted,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        answer.await.unwrap(),
        super::super::ipc::ChildGuidanceOutcome::Accepted
    ));
    super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::SealRequested)
        .await
        .unwrap();
    assert!(matches!(child.read_frame().await, ParentFrame::SealGranted));
    child
        .send_result(ChildResultStatus::Succeeded, Some("final"))
        .await;
    let settled = plane
        .registry
        .wait_agent(&first.child_agent_id)
        .await
        .unwrap();
    assert_eq!(settled.activation_id, Some(first.subagent_id));
}

#[tokio::test]
async fn agent_listing_is_newest_admission_first_and_reports_more_than_sixty_four() {
    let plane = plane(4);
    let mut identities = Vec::new();
    for _ in 0..70 {
        let child = stage_exit0(&plane);
        let accepted = start(&plane, &start_spec("listed Agent")).await;
        child
            .complete(ChildResultStatus::Succeeded, Some("done"))
            .await;
        plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
        identities.push(accepted.child_agent_id);
    }
    let listing = plane.registry.list_agents(usize::MAX);
    assert_eq!(listing.matched, 70);
    assert_eq!(listing.agents.len(), 64);
    assert_eq!(
        listing
            .agents
            .into_iter()
            .map(|a| a.agent_id)
            .collect::<Vec<_>>(),
        identities.into_iter().rev().take(64).collect::<Vec<_>>()
    );
    let empty = plane.registry.list_agents(0);
    assert_eq!(empty.matched, 70);
    assert!(empty.agents.is_empty());
}

#[tokio::test]
async fn fixed_resume_identity_conflict_attempts_exactly_once() {
    let plane = plane(4);
    let first_child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("initial")).await;
    first_child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    {
        let mut state = plane.registry.state.lock().unwrap();
        state.allocation_failure = Some(
            super::super::process::SpawnError::ConversationIdentityInUse {
                conversation_id: first.child_conversation_id.clone(),
                path: plane.runtime_root.clone(),
            },
        );
    }
    let result = plane
        .registry
        .send_message(
            &first.child_agent_id,
            "resume",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new(),
        )
        .await;
    assert!(
        matches!(result, Err(AgentControlError::Start(SubagentStartError::Spawn { detail })) if detail == "reserved activation identity conflicts with existing allocation")
    );
    let state = plane.registry.state.lock().unwrap();
    assert_eq!(
        state.allocation_attempts, 1,
        "fixed identity is never retried"
    );
    assert_eq!(state.records.len(), 1);
    assert!(state.agents[&first.child_agent_id].resuming.is_none());
}

#[tokio::test]
async fn caller_cancellation_before_resume_commit_rolls_back_owned_reservation() {
    let plane = plane(4);
    let first_child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("initial")).await;
    first_child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let signal = CancellationSignal::new();
    let mut send = Box::pin(plane.registry.send_message(
        &first.child_agent_id,
        "cancelled input",
        AgentActivationOrigin::ClientControl,
        signal.child(),
    ));
    assert!(futures_util::poll!(&mut send).is_pending());
    assert_eq!(
        plane.registry.with_goal_idle(|| true),
        None,
        "a durable reservation blocks autonomous Goal admission before ownership commits"
    );
    let mut wait = Box::pin(plane.registry.wait_agent(&first.child_agent_id));
    assert!(futures_util::poll!(&mut wait).is_pending());
    signal.cancel();
    drop(send); // The domain owner must still finish rollback after the Tool stops waiting.
    let settled = wait.await.unwrap();
    assert!(settled.activation_id.is_some());
    assert!(settled.outcome.is_none());
    assert_eq!(plane.registry.all_snapshots().len(), 1);
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
    assert_eq!(
        plane.registry.with_goal_idle(|| true),
        Some(true),
        "proven rollback releases the exact Goal ownership frontier"
    );
}

#[tokio::test]
async fn resume_control_loss_before_delegate_never_reports_input_accepted() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("initial")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let child = stage_exit0(&plane);
    drop(child); // The owned staged peer is gone before ownership/start-gate publication.
    let result = plane
        .registry
        .send_message(
            &first.child_agent_id,
            "never delivered",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new(),
        )
        .await;
    assert!(matches!(result, Err(AgentControlError::Admission(_))));
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&first.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_resume_rollback_retains_stopping_generation_and_fails_controls_closed() {
    let plane = plane(4);
    let first_child = stage_exit0(&plane);
    let first = start(&plane, &start_spec("initial")).await;
    first_child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .unwrap();
    let child = stage_with_unresolved_anchor(&plane);
    let hook = Arc::new(CommitBoundaryHook::default());
    plane.registry.install_commit_boundary_hook(hook.clone());
    let signal = CancellationSignal::new();
    let send = tokio::spawn({
        let registry = plane.registry.clone();
        let id = first.child_agent_id.clone();
        let signal = signal.child();
        async move {
            registry
                .send_message(
                    &id,
                    "unproven",
                    AgentActivationOrigin::ClientControl,
                    signal,
                )
                .await
        }
    });
    tokio::task::spawn_blocking({
        let hook = hook.clone();
        move || hook.wait_until_entered()
    })
    .await
    .unwrap();
    signal.cancel();
    hook.release();
    // Pre-commit rollback physically contains the staged child; it has no
    // committed driver and therefore publishes no Agent control frame.
    drop(child);
    assert!(matches!(
        send.await.unwrap(),
        Err(AgentControlError::Start(
            SubagentStartError::Rollback { .. }
        ))
    ));
    let snapshot = plane
        .registry
        .agent_snapshot(&first.child_agent_id)
        .unwrap();
    assert_eq!(snapshot.state, AgentState::Stopping);
    assert_eq!(
        plane.registry.with_goal_idle(|| true),
        None,
        "failed physical rollback retains the Goal ownership obligation"
    );
    assert_ne!(snapshot.current_activation, Some(first.subagent_id));
    assert!(matches!(
        plane.registry.wait_agent(&first.child_agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        plane.registry.interrupt_agent(&first.child_agent_id).await,
        Err(AgentControlError::Settlement)
    ));
    assert!(matches!(
        plane
            .registry
            .send_message(
                &first.child_agent_id,
                "must reject",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new()
            )
            .await,
        Err(AgentControlError::Stopping)
    ));
}
