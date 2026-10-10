/// Real driver frames and native registry transitions fence the numerical Fold;
/// no lifecycle label or elapsed sleep stands in for execution evidence.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep the gated execution and durable settlement sequence together.
async fn agent_duration_follows_driver_evidence_through_stopping_and_durable_terminal() {
    use super::super::ipc::{read_parent_frame, write_child_frame};
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::{AttemptId, EventId};
    use crate::runtime_client::agent_statistics::StatisticsFold;
    for status in [
        ChildResultStatus::Succeeded,
        ChildResultStatus::Cancelled,
        ChildResultStatus::Failed,
    ] {
        let plane = plane(4);
        let mut child = stage_stubborn(&plane);
        let accepted = start(&plane, &start_spec("duration")).await;
        let (_, activation) = plane
            .registry
            .agent_snapshot_with_activation(&accepted.child_agent_id)
            .unwrap();
        let base = activation.started_at;
        let store = crate::durable::SqliteConversationStore::in_memory(
            accepted.child_conversation_id.clone(),
        )
        .unwrap();
        let attempt = AttemptId::new("duration-attempt");
        let append = |seconds, event| {
            store
                .append_event(RuntimeEventEnvelope {
                    schema_version: 1,
                    event_id: EventId::new(uuid::Uuid::now_v7().to_string()),
                    sequence: 0,
                    conversation_id: accepted.child_conversation_id.clone(),
                    attempt_id: Some(attempt.clone()),
                    turn_id: Some(crate::runtime::identity::TurnId::new("1")),
                    timestamp: base + chrono::Duration::seconds(seconds),
                    event,
                })
                .unwrap()
        };
        append(
            0,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        );
        append(3, RuntimeEvent::TurnStarted);
        let mut fold = StatisticsFold::default();
        let read = |fold: &mut StatisticsFold| {
            fold.read(&store, store.presentation_frontier().unwrap(), || {
                plane.registry.execution_started_at(&accepted.subagent_id)
            })
            .unwrap()
        };
        let unconfirmed = read(&mut fold).duration.active.unwrap().running;
        let mut changes = plane.registry.state_version.subscribe();
        child.accept_delegate().await;
        while plane
            .registry
            .execution_started_at(&accepted.subagent_id)
            .is_none()
        {
            changes.changed().await.unwrap();
        }
        assert_eq!(
            plane
                .registry
                .agent_snapshot(&accepted.child_agent_id)
                .unwrap()
                .state,
            AgentState::Active
        );
        assert!(read(&mut fold).duration.active.unwrap().running);
        if status == ChildResultStatus::Cancelled {
            plane
                .registry
                .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
                .unwrap();
            assert!(matches!(
                child.read_frame().await,
                ParentFrame::Cancel { .. }
            ));
        } else {
            // Same control stream: SealGranted proves admission has closed.
            write_child_frame(&mut child.peer, &ChildFrame::SealRequested)
                .await
                .unwrap();
            assert!(matches!(child.read_frame().await, ParentFrame::SealGranted));
        }
        assert_eq!(
            plane
                .registry
                .agent_snapshot(&accepted.child_agent_id)
                .unwrap()
                .state,
            AgentState::Stopping
        );
        let active = read(&mut fold);
        let interval = active.duration.active.unwrap();
        assert!(
            interval.running,
            "Stopping closes admission, not working duration"
        );
        assert!(
            !unconfirmed,
            "unacknowledged delegation is not live execution evidence"
        );
        assert_eq!(interval.started_at, base);
        assert_eq!(interval.observed_at, base + chrono::Duration::seconds(3));
        assert_eq!(
            (base + chrono::Duration::seconds(5) - interval.started_at).num_milliseconds(),
            5_000
        );
        assert_eq!(active.duration.settled_ms, 0);
        let terminal = match status {
            ChildResultStatus::Succeeded => RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: crate::model::finish::ModelFinishReason::Stop,
            },
            ChildResultStatus::Cancelled => RuntimeEvent::AttemptCancelled {
                attempt_id: attempt.clone(),
                reason: CancellationReason::UserRequested,
            },
            ChildResultStatus::Failed => RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: crate::events::types::AttemptFailure::Runtime {
                    error: crate::runtime::types::RuntimeError::Internal {
                        message: "native failure".into(),
                    },
                },
            },
        };
        append(7, terminal);
        // Durable attempt settlement wins even while the process still exists.
        assert!(
            plane
                .registry
                .execution_started_at(&accepted.subagent_id)
                .is_some()
        );
        let terminal = read(&mut fold);
        assert_eq!(terminal.duration.settled_ms, 7_000);
        assert!(terminal.duration.active.is_none());
        assert_eq!(terminal, read(&mut fold));
        child
            .send_result(
                status,
                (status == ChildResultStatus::Succeeded).then_some("done"),
            )
            .await;
        assert!(
            read_parent_frame(&mut child.peer).await.unwrap().is_none(),
            "driver half-close follows result consumption"
        );
        assert!(
            plane
                .registry
                .execution_started_at(&accepted.subagent_id)
                .is_none(),
            "cleanup cannot prolong the execution lease"
        );
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(child.pid).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
        plane
            .registry
            .wait_agent(&accepted.child_agent_id)
            .await
            .unwrap();
        assert_eq!(terminal, read(&mut fold));
    }
}

#[tokio::test]
async fn agent_duration_freezes_at_durable_evidence_after_control_loss() {
    use super::super::ipc::{read_parent_frame, write_child_frame};
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::{AttemptId, EventId, TurnId};
    use crate::runtime_client::agent_statistics::StatisticsFold;
    use tokio::io::AsyncWriteExt;
    let plane = plane(4);
    let mut child = stage_stubborn(&plane);
    let accepted = start(&plane, &start_spec("lost-duration")).await;
    let (_, activation) = plane
        .registry
        .agent_snapshot_with_activation(&accepted.child_agent_id)
        .unwrap();
    let base = activation.started_at;
    let store =
        crate::durable::SqliteConversationStore::in_memory(accepted.child_conversation_id.clone())
            .unwrap();
    let attempt = AttemptId::new("abandoned");
    for (seconds, event) in [
        (
            0,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ),
        (3, RuntimeEvent::TurnStarted),
    ] {
        store
            .append_event(RuntimeEventEnvelope {
                schema_version: 1,
                event_id: EventId::new(format!("event-{seconds}")),
                sequence: 0,
                conversation_id: accepted.child_conversation_id.clone(),
                attempt_id: Some(attempt.clone()),
                turn_id: Some(TurnId::new("1")),
                timestamp: base + chrono::Duration::seconds(seconds),
                event,
            })
            .unwrap();
    }
    child.accept_delegate().await;
    write_child_frame(&mut child.peer, &ChildFrame::SealRequested)
        .await
        .unwrap();
    assert!(matches!(child.read_frame().await, ParentFrame::SealGranted));
    let mut fold = StatisticsFold::default();
    let cut = store.presentation_frontier().unwrap();
    assert!(
        fold.read(&store, cut, || plane
            .registry
            .execution_started_at(&accepted.subagent_id))
            .unwrap()
            .duration
            .active
            .unwrap()
            .running
    );
    child.peer.shutdown().await.unwrap(); // EOF is unknown execution, never a semantic terminal.
    assert!(read_parent_frame(&mut child.peer).await.unwrap().is_none());
    let frozen = fold
        .read(&store, cut, || {
            plane.registry.execution_started_at(&accepted.subagent_id)
        })
        .unwrap();
    let interval = frozen.duration.active.as_ref().unwrap();
    assert!(!interval.running);
    assert_eq!(
        (interval.observed_at - interval.started_at).num_milliseconds(),
        3_000
    );
    assert_eq!(frozen.duration.settled_ms, 0);
    assert_eq!(
        frozen,
        fold.read(&store, cut, || None).unwrap(),
        "recovery cannot revive the abandoned interval"
    );
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.pid).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    );
    plane
        .registry
        .wait_agent(&accepted.child_agent_id)
        .await
        .unwrap();
    assert_eq!(
        frozen,
        fold.read(&store, cut, || plane
            .registry
            .execution_started_at(&accepted.subagent_id))
            .unwrap()
    );
}
