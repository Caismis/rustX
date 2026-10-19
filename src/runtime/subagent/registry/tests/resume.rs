//! Agent resume regressions: reservation exclusivity, staged rollback
//! uncertainty, delivery proof, and activation-exact interruption.

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_cancellation_before_delegate_is_proven_not_delivered() {
    let plane = plane(4);
    let first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first")).await;
    first
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let mut child = stage_exit0(&plane);
    let hook = Arc::new(ControlHandoffHook::default());
    plane.registry.state.lock().unwrap().control_handoff_hook = Some(hook.clone());
    let registry = plane.registry.clone();
    let id = accepted.child_agent_id.clone();
    let sending = tokio::spawn(async move {
        registry
            .send_message(
                &id,
                "never delivered",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new(),
            )
            .await
    });
    // Ownership is durable, but the process driver cannot cross its start gate.
    hook.wait_until_entered();
    let activation = plane
        .registry
        .agent_snapshot(&accepted.child_agent_id)
        .unwrap()
        .current_activation
        .unwrap();
    let mut interrupt = Box::pin(plane.registry.interrupt_agent(&accepted.child_agent_id));
    assert!(futures_util::poll!(&mut interrupt).is_pending());
    hook.release();
    assert!(matches!(
        child.read_frame().await,
        ParentFrame::Cancel { .. }
    ));
    child.send_result(ChildResultStatus::Cancelled, None).await;
    assert!(matches!(
        sending.await.unwrap(),
        Err(AgentControlError::NotDelivered)
    ));
    assert_eq!(interrupt.await.unwrap().activation_id, Some(activation));
}

#[tokio::test]
async fn resumed_delegate_without_acceptance_is_delivery_unknown() {
    let plane = plane(4);
    let first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first")).await;
    first
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let mut child = stage_exit0(&plane);
    let registry = plane.registry.clone();
    let id = accepted.child_agent_id.clone();
    let sending = tokio::spawn(async move {
        registry
            .send_message(
                &id,
                "possibly delivered",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new(),
            )
            .await
    });
    // Reading the actual frame establishes Delegate write before control loss.
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    drop(child);
    assert!(matches!(
        sending.await.unwrap(),
        Err(AgentControlError::DeliveryUnknown)
    ));
}

/// Dedicated subprocess entry: the production `spawn_staged` command starts
/// this through its shell trampoline, inheriting the real authority fd.
#[test]
fn staged_rollback_wire_child() {
    use std::os::fd::AsFd as _;
    if std::env::var_os("RUSTX_STAGED_ROLLBACK_FIXTURE").is_none() {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let stream = std::os::unix::net::UnixStream::from(
            std::io::stdin().as_fd().try_clone_to_owned().unwrap(),
        );
        stream.set_nonblocking(true).unwrap();
        let mut control = tokio::net::UnixStream::from_std(stream).unwrap();
        let Some(ParentFrame::Hello(spec)) =
            super::super::super::ipc::read_parent_frame(&mut control)
                .await
                .unwrap()
        else {
            panic!("Hello");
        };
        // Actual child authority transition, never a manufactured receipt.
        let _owner =
            super::super::super::physical_recovery::ChildPhysicalLease::acquire(&spec).unwrap();
        super::super::super::ipc::write_child_frame(
            &mut control,
            &ChildFrame::AnchorOffered(super::super::super::ipc::ProcessUnitAnchorFrame {
                unit_id: crate::runtime::identity::ProcessUnitId::new("unit-unproven-startup"),
                pgid: i32::MAX,
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            super::super::super::ipc::read_parent_frame(&mut control)
                .await
                .unwrap(),
            Some(ParentFrame::AnchorAccepted(_))
        ));
        super::super::super::ipc::write_child_frame(
            &mut control,
            &ChildFrame::StartupError(super::super::super::ipc::DiagnosticFrame {
                message: "composition failed after owned staging".into(),
            }),
        )
        .await
        .unwrap();
        // The process owner tears us down after StartupError; the offered
        // anchor remains intentionally unprovable, so no epilogue claims proof.
        let _ = super::super::super::ipc::read_parent_frame(&mut control).await;
    });
}

#[tokio::test]
async fn staged_rollback_uncertainty_survives_prepare_and_durable_admission_rollback() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut plane = plane(4);
    let executable = std::env::current_exe()
        .unwrap()
        .display()
        .to_string()
        .replace('\'', "'\\''");
    let script = plane.dir.path().join("unproven-child.sh");
    std::fs::write(&script, format!(
        "#!/bin/sh\nRUSTX_STAGED_ROLLBACK_FIXTURE=1 exec '{executable}' --exact runtime::subagent::registry::tests::staged_rollback_wire_child --nocapture\n"
    )).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    plane.registry.config.spawn.program = script;
    let first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first")).await;
    first
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let child_store = super::super::super::child_conversation_store_path(
        plane.registry.config.spawn.product_root.root(),
        &plane.registry.config.spawn.session_id,
        &accepted.child_conversation_id,
    );
    std::fs::create_dir_all(child_store.parent().unwrap()).unwrap();
    let _child_store = crate::durable::SqliteConversationStore::open(
        accepted.child_conversation_id.clone(),
        &child_store,
    )
    .unwrap();
    let result = plane
        .registry
        .send_message(
            &accepted.child_agent_id,
            "next",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new(),
        )
        .await;
    assert!(matches!(result, Err(AgentControlError::Settlement)));
    let failed = plane
        .registry
        .agent_snapshot(&accepted.child_agent_id)
        .unwrap();
    let failed_activation = failed.current_activation.as_ref().unwrap();
    assert_ne!(failed_activation, &accepted.subagent_id);
    assert_eq!(failed.conversation_id, accepted.child_conversation_id);
    assert!(
        super::super::super::physical_recovery::prove(
            &plane.registry.config.spawn.product_root,
            &plane.registry.config.spawn.session_id,
            &accepted.child_conversation_id,
            failed_activation,
        )
        .unwrap()
        .is_none(),
        "unproven Running authority cannot become a recovery proof"
    );
    let rollback: Vec<_> = events(&plane)
        .into_iter()
        .filter_map(|event| {
            if let crate::events::types::RuntimeEvent::AgentActivationAdmission {
                phase:
                    crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                        physical_settlement_proven,
                    },
                ..
            } = event
            {
                Some(physical_settlement_proven)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rollback, vec![false]);
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Unavailable
    );
}

#[tokio::test]
async fn drain_cancels_reserved_resume_before_preparation() {
    let plane = plane(4);
    let first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first")).await;
    first
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let mut resume = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "next",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    // One poll reserves the activation; its owner task has not run yet.
    assert!(futures_util::poll!(&mut resume).is_pending());
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    assert!(matches!(
        resume.await,
        Err(AgentControlError::Start(SubagentStartError::Cancelled))
    ));
    assert_eq!(plane.registry.all_snapshots().len(), 1);
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
}

#[tokio::test]
async fn stale_resume_cleanup_cannot_clear_a_newer_reservation() {
    let plane = plane(4);
    let first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first")).await;
    first
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let mut resume = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "next",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut resume).is_pending());
    {
        let mut state = plane.registry.state.lock().unwrap();
        let agent = state.agents.get_mut(&accepted.child_agent_id).unwrap();
        let reserved = agent.resuming.as_ref().unwrap().activation_id.clone();
        // Exact cleanup operation of an older completion, ordered after
        // a newer reservation through the same owner mutex.
        agent.finish_resume(&accepted.subagent_id);
        assert_eq!(agent.resuming.as_ref().unwrap().activation_id, reserved);
    }
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    assert!(resume.await.is_err());
}

#[tokio::test]
async fn concurrent_resume_reserves_exactly_one_activation_before_preparation() {
    let plane = plane(4);
    let first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first")).await;
    first
        .complete(ChildResultStatus::Succeeded, Some("first report"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    let mut second = stage_exit0(&plane);
    let mut winner = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "resume winner",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    // On this current-thread executor the first poll commits reservation,
    // then parks on its spawned preparation. That task cannot run until
    // this test yields; the losing contender deterministically sees it.
    assert!(futures_util::poll!(&mut winner).is_pending());
    assert!(matches!(
        plane
            .registry
            .send_message(
                &accepted.child_agent_id,
                "resume loser",
                crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                crate::runtime::cancellation::CancellationSignal::new()
            )
            .await,
        Err(AgentControlError::Stopping)
    ));
    let mut admission_wait = Box::pin(plane.registry.wait_agent(&accepted.child_agent_id));
    assert!(futures_util::poll!(&mut admission_wait).is_pending());
    second.accept_delegate().await;
    let resumed = winner.await.unwrap();
    assert_ne!(resumed.activation_id, accepted.subagent_id);
    {
        let state = plane.registry.state.lock().unwrap();
        assert_eq!(
            state
                .records
                .iter()
                .filter(|record| record.child_agent_id == accepted.child_agent_id
                    && record.lifecycle.is_active())
                .count(),
            1
        );
        assert_eq!(state.records.len(), 2);
    }
    second
        .send_result(ChildResultStatus::Succeeded, Some("second report"))
        .await;
    drop(second);
    assert!(admission_wait.await.unwrap().outcome.is_some());
    plane
        .registry
        .wait_agent(&accepted.child_agent_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn interrupt_settles_only_captured_activation_and_agent_resumes() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("interrupt")).await;
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    let mut interrupt = Box::pin(plane.registry.interrupt_agent(&accepted.child_agent_id));
    assert!(futures_util::poll!(&mut interrupt).is_pending());
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Stopping
    );
    assert!(matches!(
        plane
            .registry
            .send_message(
                &accepted.child_agent_id,
                "too late",
                crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                crate::runtime::cancellation::CancellationSignal::new()
            )
            .await,
        Err(AgentControlError::Stopping)
    ));
    assert!(matches!(
        child.read_frame().await,
        ParentFrame::Cancel { .. }
    ));
    child
        .send_result(
            ChildResultStatus::Succeeded,
            Some("losing normal completion"),
        )
        .await;
    let result = interrupt.await.unwrap();
    assert_eq!(result.activation_id, Some(accepted.subagent_id));
    assert_eq!(result.outcome.unwrap().state, SubagentState::Cancelled);
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
    let mut next = stage_exit0(&plane);
    let (resumed, _) = tokio::join!(
        plane.registry.send_message(
            &accepted.child_agent_id,
            "continue",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new()
        ),
        next.accept_delegate(),
    );
    assert!(resumed.unwrap().resumed);
    next.send_result(ChildResultStatus::Succeeded, Some("resumed report"))
        .await;
    drop(next);
    plane
        .registry
        .wait_agent(&accepted.child_agent_id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_resume_preserves_identity_and_wait_captures_only_one_activation() {
    let plane = plane(4);
    let mut first = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("first activation")).await;
    assert!(matches!(first.read_frame().await, ParentFrame::Delegate(_)));
    let agent_id = accepted.child_agent_id;
    let conversation_id = accepted.child_conversation_id;
    assert_eq!(
        plane.registry.agent_snapshot(&agent_id).unwrap().state,
        AgentState::Active
    );
    let mut waiting = Box::pin(plane.registry.wait_agent(&agent_id));
    assert!(futures_util::poll!(&mut waiting).is_pending());
    first
        .send_result(ChildResultStatus::Succeeded, Some("first report"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
    assert_eq!(
        plane.registry.agent_snapshot(&agent_id).unwrap().state,
        AgentState::Inactive
    );
    assert!(
        plane
            .registry
            .wait_agent(&agent_id)
            .await
            .unwrap()
            .activation_id
            .is_none()
    );
    let mut second = stage_exit0(&plane);
    let (resumed, delegate) = tokio::join!(
        plane.registry.send_message(
            &agent_id,
            "second activation",
            AgentActivationOrigin::ClientControl,
            CancellationSignal::new()
        ),
        second.accept_delegate(),
    );
    let resumed = resumed.unwrap();
    assert!(resumed.resumed);
    assert_ne!(resumed.activation_id, accepted.subagent_id);
    let current = plane.registry.agent_snapshot(&agent_id).unwrap();
    assert_eq!(current.conversation_id, conversation_id);
    assert_eq!(current.agent_id, agent_id);
    assert_eq!(current.state, AgentState::Active);
    assert_eq!(
        current.current_activation,
        Some(resumed.activation_id.clone())
    );
    assert_eq!(
        plane
            .registry
            .list_agents(MAX_AGENT_LIST_LIMIT)
            .agents
            .len(),
        1
    );
    // Resume occurs before the original waiter is polled again. It must
    // complete from its captured activation even while the next is active.
    let result = futures_util::poll!(&mut waiting);
    assert!(matches!(result, std::task::Poll::Ready(Ok(AgentWaitResult {
        activation_id: Some(ref id), ..
    })) if id == &accepted.subagent_id));
    assert_eq!(delegate.task, "second activation");
    second
        .send_result(ChildResultStatus::Succeeded, Some("second report"))
        .await;
    plane.registry.wait_agent(&agent_id).await.unwrap();
    assert_eq!(
        plane.registry.agent_snapshot(&agent_id).unwrap().state,
        AgentState::Inactive
    );
}
