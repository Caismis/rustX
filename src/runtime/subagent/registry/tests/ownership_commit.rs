//! Two-stage start regressions: preparation validation, the ownership commit
//! linearization point against cancellation and drain, capacity, and ordinals.

use super::*;
use crate::runtime::subagent::MAX_CONTEXT_PACKAGE_BYTES;
use crate::runtime::subagent::MAX_TASK_BYTES;
use crate::runtime::subagent::process::StagedChild;

/// Issue #145 removed the temporary #144 refusal: an externally
/// sourced capability is no longer rejected in `prepare`. The registry
/// stages the child exactly as it does for any other frozen
/// specification, and physical realization (with its own identity
/// verification) happens inside the child, before it answers `Ready`.
#[tokio::test]
async fn an_external_origin_requirement_is_no_longer_refused_by_the_registry() {
    let plane = plane(2);
    let mut spec = spec("inspect");
    let definition = crate::tools::types::ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-get-issue"),
        name: "get_issue".to_owned(),
        description: "issue".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        execution_policy: crate::tools::types::ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: crate::tools::types::ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: crate::tools::types::ToolReplayPolicy::Never,
        origin: crate::tools::types::ToolOrigin::Mcp {
            server_id: crate::runtime::identity::McpServerId::new("github"),
        },
    };
    spec.authority.resolved.tools = vec![
        super::super::super::resolver::ResolvedSubagentTool::Source {
            source_id: crate::capabilities::ToolSourceId::Mcp(
                crate::runtime::identity::McpServerId::new("github"),
            ),
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            identity: crate::tools::mcp::identity::definition_identity(&definition)
                .expect("an MCP definition has an MCP identity"),
            definition,
        },
    ];
    // The staged override consumes `prepare` before any real process is
    // spawned, so this asserts exactly one thing: the registry no longer
    // has a capability-shaped refusal of its own.
    let (parent, _peer) = tokio::net::UnixStream::pair().expect("control pair");
    let (observation_end, _observation_peer) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let child = tokio::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn a staged stand-in");
    let root = std::env::temp_dir().join(format!(
        "rustx-staged-override-{}-{}",
        std::process::id(),
        "external-origin"
    ));
    std::fs::create_dir_all(&root).expect("root");
    plane.registry.push_staged_override(StagedChild::for_test(
        child,
        parent,
        observation_end,
        root,
    ));
    let prepared = plane
        .registry
        .prepare(&spec, &CancellationSignal::new())
        .await
        .expect("an externally sourced capability no longer refuses staging");
    assert!(
        prepared.staged.retained_anchor_count() == 0,
        "a freshly staged child has anchored no nested process unit yet"
    );
    prepared.staged.rollback().await.expect("rollback");
    // The selector vocabulary is unchanged: #145 removed a physical
    // limitation, not a capability model.
    assert_eq!(
        AgentToolSelection::Source {
            source_id: crate::capabilities::ToolSourceId::try_from(String::from("github")).unwrap(),
            name: "get_issue".to_owned(),
        }
        .canonical(),
        "source:github/get_issue"
    );
}

#[tokio::test]
async fn a_child_lost_to_driver_escalation_after_cancel_settles_cancelled() {
    let plane = plane(4);
    // The child never answers the Cancel frame; the driver escalates
    // (Cancel -> SIGTERM -> SIGKILL) and reaps it.
    let _child = stage_stubborn(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    let _ = plane
        .registry
        .cancel(&accepted.subagent_id, CancellationReason::UserRequested);
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_eq!(
        settled.detail.as_deref(),
        Some("requested by the user"),
        "driver escalation cannot erase the committed cancellation cause"
    );
    assert_eq!(
        events(&plane)
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                        subagent_id,
                        state: SubagentTerminalState::Cancelled,
                        ..
                    } if *subagent_id == accepted.subagent_id
                )
            })
            .count(),
        1,
        "escalated cancellation has one terminal publication"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_losing_the_cancellation_race_rolls_back_completely() {
    let plane = plane(4);
    let hook = Arc::new(CommitBoundaryHook::default());
    plane.registry.install_commit_boundary_hook(hook.clone());
    let child = stage_stubborn(&plane);
    let pid = child.pid;
    let registry = plane.registry.clone();
    let spec = start_spec("inspect");
    let attempt_cancellation = CancellationSignal::new();
    let committer = {
        let attempt_cancellation = attempt_cancellation.clone();
        tokio::spawn(async move {
            let prepared = registry
                .prepare(&spec, &CancellationSignal::new())
                .await
                .expect("prepared");
            registry.commit(prepared, &attempt_cancellation).await
        })
    };
    hook.wait_until_entered();
    attempt_cancellation.cancel();
    hook.release();
    let outcome = committer.await.expect("committer");
    assert!(matches!(outcome, Ok(SubagentStartOutcome::RolledBack)));
    // No record, no durable trace, no ordinal consumption that recovery
    // would fold.
    assert!(plane.registry.all_snapshots().is_empty());
    assert!(events(&plane).is_empty());
    #[cfg(unix)]
    assert!(matches!(
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits")),
            None
        ),
        Err(nix::errno::Errno::ESRCH)
    ));
}

/// The ownership/control handoff race is deterministic: durable
/// ownership and the Running projection are visible while the driver
/// command handle is deliberately not installed, cancellation commits in
/// that exact window, and the resumed handoff forwards the sticky cancel
/// before Delegate. The real child is then killed and reaped, and its
/// late/no result cannot overtake the canonical cancellation.
#[allow(clippy::too_many_lines)]
async fn control_handoff_cancellation_is_lossless(runtime_drain: bool) {
    let plane = plane(4);
    let hook = Arc::new(ControlHandoffHook::default());
    plane.registry.install_control_handoff_hook(hook.clone());
    let mut child = stage_stubborn(&plane);
    let pid = child.pid;
    let registry = plane.registry.clone();
    let spec = start_spec("inspect");
    let committer = tokio::spawn(async move {
        let prepared = registry
            .prepare(&spec, &CancellationSignal::new())
            .await
            .expect("prepared");
        registry.commit(prepared, &CancellationSignal::new()).await
    });

    hook.wait_until_entered();
    let subagent_id = SubagentId::for_conversation(&plane.conversation_id, 1);
    assert!(events(&plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: ownership_subagent_id,
            ..
        } if *ownership_subagent_id == subagent_id
    )));

    let cancelling = if runtime_drain {
        plane
            .registry
            .cancel_all(CancellationReason::RuntimeShutdown);
        plane
            .registry
            .snapshot(&subagent_id)
            .expect("cancelled record")
    } else {
        let registry = plane.registry.clone();
        tokio::spawn(async move {
            registry
                .cancel(&subagent_id, CancellationReason::UserRequested)
                .expect("cancelled record")
        })
        .await
        .expect("cancel task")
    };
    // Release the deterministic handoff gate after capturing the state,
    // before assertions, so a failing invariant cannot strand a worker.
    hook.release();
    assert_eq!(cancelling.state, SubagentState::Cancelling);
    let expected_reason = if runtime_drain {
        CancellationReason::RuntimeShutdown
    } else {
        CancellationReason::UserRequested
    };

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
        .await
        .expect("commit liveness")
        .expect("committer")
        .expect("commit succeeds after cancellation intent");
    let accepted = match outcome {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("ownership already committed"),
    };
    let ownership_timestamp = plane
        .store
        .read_events(None, 64)
        .expect("events")
        .events
        .iter()
        .find_map(|envelope| {
            matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id,
                    ..
                } if *subagent_id == accepted.subagent_id
            )
            .then_some(envelope.timestamp)
        })
        .expect("ownership timestamp");
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("accepted snapshot")
            .started_at,
        ownership_timestamp,
        "one ownership commit timestamp feeds event and projection"
    );

    let first = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        super::super::super::ipc::read_parent_frame(&mut child.peer),
    )
    .await
    .expect("driver control liveness")
    .expect("driver frame")
    .expect("cancel frame");
    assert!(matches!(
        first,
        ParentFrame::Cancel {
            reason: Some(reason)
        } if reason == expected_reason
    ));
    // No Delegate was sent after cancellation won this handoff frontier:
    // the driver's cancelled-before-start branch never writes it. Drain
    // every remaining control frame (the escalation then EOF after
    // reap) and prove the wire carries nothing else.
    loop {
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            super::super::super::ipc::read_parent_frame(&mut child.peer),
        )
        .await
        .expect("driver control liveness")
        .expect("driver frame");
        match frame {
            Some(ParentFrame::Cancel {
                reason: Some(reason),
            }) if reason == expected_reason => {}
            Some(ParentFrame::Cancel { reason: None }) => {
                panic!("a committed cancellation must carry its registry reason")
            }
            Some(ParentFrame::Cancel {
                reason: Some(reason),
            }) => {
                panic!("unexpected cancellation reason on the wire: {reason:?}")
            }
            Some(ParentFrame::Delegate(_)) => {
                panic!("cancellation won the frontier; Delegate must never be sent")
            }
            Some(
                ParentFrame::Hello(_) | ParentFrame::SealGranted | ParentFrame::AdmissionReopened,
            ) => {
                panic!("unexpected handshake frame after cancellation")
            }
            Some(ParentFrame::AnchorAccepted(_) | ParentFrame::AnchorRefused(_)) => {
                panic!("this child offered no nested process unit anchor")
            }
            Some(ParentFrame::InteractionRespond { .. }) => {
                panic!("the driver test did not offer an interaction response")
            }
            Some(ParentFrame::InteractionProviderAvailable { .. }) => {
                panic!("the driver test did not offer a provider update")
            }
            Some(ParentFrame::Guidance(_)) => {
                panic!("the driver test did not steer this child")
            }
            Some(ParentFrame::InteractionPublicationAdmissionResult(_)) => {
                panic!("the driver test did not offer an admission result")
            }
            None => break,
        }
    }
    drop(child.peer);

    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    assert_ne!(
        settled.settlement.publication,
        SubagentPublication::Abandoned
    );
    #[cfg(unix)]
    assert!(
        matches!(
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits")),
                None
            ),
            Err(nix::errno::Errno::ESRCH)
        ),
        "the direct child was reaped"
    );
    let terminal_states = events(&plane)
        .into_iter()
        .filter_map(|event| match event {
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                state,
                ..
            } if subagent_id == accepted.subagent_id => Some(state),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminal_states,
        vec![SubagentTerminalState::Cancelled],
        "one canonical cancellation, never a success overtaking it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_between_ownership_and_control_publication_is_lossless() {
    control_handoff_cancellation_is_lossless(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_drain_between_ownership_and_control_publication_is_lossless() {
    control_handoff_cancellation_is_lossless(true).await;
}

/// The remaining start-gate edge (Blocker A) is serialized, not
/// best-effort: while the commit holds the registry mutex at exactly
/// "command handle installed, gate not yet released", a concurrent
/// `cancel` provably blocks. Releasing the gate first defines an
/// already-started child: the driver sends `Delegate` first and the
/// later cancellation arrives as in-flight cancellation (`Cancel` frame
/// after `Delegate`), settling one canonical cancelled terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn gate_release_wins_the_start_cancel_arbitration_and_cancel_becomes_in_flight() {
    let plane = plane(4);
    let hook = Arc::new(GateReleaseHook::default());
    plane.registry.install_gate_release_hook(hook.clone());
    let mut child = stage_stubborn(&plane);
    let registry = plane.registry.clone();
    let spec = start_spec("inspect");
    let committer = tokio::spawn(async move {
        let prepared = registry
            .prepare(&spec, &CancellationSignal::new())
            .await
            .expect("prepared");
        registry.commit(prepared, &CancellationSignal::new()).await
    });

    hook.wait_until_entered();
    let subagent_id = SubagentId::for_conversation(&plane.conversation_id, 1);
    assert!(events(&plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: ownership_subagent_id,
            ..
        } if *ownership_subagent_id == subagent_id
    )));

    // A concurrent cancellation is invoked while the commit provably
    // holds the registry mutex at the install-but-not-released edge; it
    // cannot complete until the gate-release section returns.
    let (cancel_started_tx, cancel_started_rx) = std::sync::mpsc::channel();
    let (cancel_done_tx, cancel_done_rx) = std::sync::mpsc::channel();
    let cancel_registry = plane.registry.clone();
    let cancel_id = subagent_id.clone();
    let canceller = std::thread::spawn(move || {
        cancel_started_tx.send(()).expect("cancel-started channel");
        let snapshot = cancel_registry
            .cancel(&cancel_id, CancellationReason::UserRequested)
            .expect("known record");
        cancel_done_tx.send(()).expect("cancel-done channel");
        snapshot
    });
    cancel_started_rx
        .recv()
        .expect("cancel is invoked while the gate release is parked");
    assert!(
        cancel_done_rx.try_recv().is_err(),
        "cancel is provably blocked on the registry mutex held by the parked gate release"
    );

    hook.release();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
        .await
        .expect("commit liveness")
        .expect("committer")
        .expect("commit succeeds");
    let accepted = match outcome {
        SubagentStartOutcome::Accepted(accepted) => accepted,
        SubagentStartOutcome::RolledBack => panic!("ownership already committed"),
    };
    let cancelling = canceller.join().expect("canceller joins");
    assert_eq!(cancelling.state, SubagentState::Cancelling);

    // The gate release won: Delegate is the first parent->child frame,
    // and the committed cancellation arrives after it as in-flight
    // cancellation.
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        super::super::super::ipc::read_parent_frame(&mut child.peer),
    )
    .await
    .expect("driver control liveness")
    .expect("driver frame")
    .expect("delegate frame");
    assert!(
        matches!(first, ParentFrame::Delegate(_)),
        "gate release first means Delegate first"
    );
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        super::super::super::ipc::read_parent_frame(&mut child.peer),
    )
    .await
    .expect("driver control liveness")
    .expect("driver frame")
    .expect("cancel frame");
    assert!(matches!(
        second,
        ParentFrame::Cancel {
            reason: Some(CancellationReason::UserRequested)
        }
    ));

    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Cancelled);
    let terminal_states = events(&plane)
        .into_iter()
        .filter_map(|event| match event {
            crate::events::types::RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                state,
                ..
            } if subagent_id == accepted.subagent_id => Some(state),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminal_states,
        vec![SubagentTerminalState::Cancelled],
        "one canonical cancellation"
    );
}

#[tokio::test]
async fn the_capacity_bound_is_enforced_at_commit() {
    let plane = plane(1);
    let _first_child = stage_stubborn(&plane);
    let first = start(&plane, &start_spec("first")).await;
    assert_eq!(
        plane
            .registry
            .snapshot(&first.subagent_id)
            .expect("snapshot")
            .state,
        SubagentState::Running
    );
    // prepare stages privately even at capacity; the commit is the
    // linearization point that refuses.
    let _second_child = stage_exit0(&plane);
    let prepared = plane
        .registry
        .prepare(&start_spec("second"), &CancellationSignal::new())
        .await
        .expect("prepared");
    let error = plane
        .registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .expect_err("capacity");
    assert!(matches!(
        error,
        SubagentStartError::CapacityExceeded { max: 1 }
    ));
    // Settle the committed first child (escalate and reap) so the
    // fixture leaks no process.
    let _ = plane
        .registry
        .cancel(&first.subagent_id, CancellationReason::UserRequested);
    plane
        .registry
        .wait_until_settled(&first.subagent_id)
        .await
        .expect("settled");
}

#[tokio::test]
async fn prepare_rejects_an_invalid_task_before_any_spawn() {
    let plane = plane(4);
    let error = plane
        .registry
        .prepare(&start_spec(""), &CancellationSignal::new())
        .await
        .expect_err("empty task");
    assert!(matches!(error, SubagentStartError::InvalidTask { .. }));
    let oversized = "x".repeat(MAX_TASK_BYTES + 1);
    let error = plane
        .registry
        .prepare(&start_spec(&oversized), &CancellationSignal::new())
        .await
        .expect_err("oversized task");
    assert!(matches!(error, SubagentStartError::InvalidTask { .. }));
    let mut oversized_context = start_spec("inspect");
    oversized_context.admission.context = Some("x".repeat(MAX_CONTEXT_PACKAGE_BYTES + 1));
    let error = plane
        .registry
        .prepare(&oversized_context, &CancellationSignal::new())
        .await
        .expect_err("oversized context");
    assert!(matches!(error, SubagentStartError::ContextOversized { .. }));
    assert!(plane.registry.all_snapshots().is_empty());
}

#[tokio::test]
async fn the_ordinal_sequence_reseeds_above_the_durable_watermark() {
    let plane = plane(4);
    plane.registry.restore_sequence_watermark(7);
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    assert_eq!(
        accepted.subagent_id.as_str(),
        "conv_5d71cacf-35af-790f-8022-fe16cd1b9fe0-subagent-8",
        "the next ordinal never reissues a durable identity"
    );
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
}

/// A parent generation can die after a child opened its durable store but
/// before the parent's ownership event committed. The next generation
/// must preserve that store as an orphaned history boundary and advance
/// to a fresh child identity instead of reusing it.
#[tokio::test]
async fn prepare_skips_an_unpublished_durable_child_identity() {
    #[derive(Debug)]
    struct Identities(std::sync::Mutex<std::collections::VecDeque<uuid::Uuid>>);
    impl crate::runtime::identity::UuidV7Generator for Identities {
        fn next_uuid(&self) -> uuid::Uuid {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .expect("finite allocation sequence")
        }
    }
    let ids = [
        "01900000-0000-7000-8000-000000000011",
        "01900000-0000-7000-8000-000000000012",
        "01900000-0000-7000-8000-000000000013",
    ]
    .map(|id| uuid::Uuid::parse_str(id).unwrap());
    let mut plane = plane(4);
    plane.registry =
        plane
            .registry
            .with_identity_generator(Arc::new(Identities(std::sync::Mutex::new(
                ids.into_iter().collect(),
            ))));
    let stale_id = ConversationId::from_uuid(ids[0]).unwrap();
    let stale_store = crate::runtime::subagent::child_conversation_store_path(
        &plane.runtime_root,
        &plane.registry.config.spawn.session_id,
        &stale_id,
    );
    std::fs::create_dir_all(
        stale_store
            .parent()
            .expect("the stale semantic child directory"),
    )
    .expect("stale child directory");
    std::fs::write(&stale_store, b"orphaned durable state").expect("stale durable store");

    let error = plane
        .registry
        .prepare(
            &start_spec("skip stale identity"),
            &CancellationSignal::new(),
        )
        .await
        .expect_err("the next identity should reach the intentionally missing child binary");
    assert!(matches!(error, SubagentStartError::Spawn { .. }));

    let _child = stage_exit0(&plane);
    let prepared = plane
        .registry
        .prepare(&start_spec("fresh identity"), &CancellationSignal::new())
        .await
        .expect("the following identity is fresh");
    assert_eq!(
        prepared.child_conversation_id,
        ConversationId::from_uuid(ids[2]).unwrap()
    );
    prepared.staged.rollback().await.expect("rollback");
    assert_eq!(
        std::fs::read(&stale_store).expect("the orphaned store remains authoritative"),
        b"orphaned durable state"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn goal_idle_append_releases_registry_mutex_and_excludes_ownership_commit() {
    let plane = plane(4);
    let goal = crate::goal::GoalDomain::new(plane.store.clone());
    goal.write(crate::goal::GoalWrite::Create {
        objective: "one durable idle continuation".into(),
        budget: 1,
        origin: crate::goal::GoalOrigin::RuntimeControl,
    })
    .unwrap()
    .unwrap();
    let child = stage_exit0(&plane);
    let cancellation = CancellationSignal::new();
    let prepared = plane
        .registry
        .prepare(
            &start_spec("must wait for the idle frontier"),
            &cancellation,
        )
        .await
        .unwrap();
    let hook = Arc::new(CommitBoundaryHook::default());
    let idle = std::thread::spawn({
        let registry = plane.registry.clone();
        let hook = hook.clone();
        move || {
            registry.with_goal_idle(|| {
                hook.wait();
                crate::goal::GoalRoundDriver::request(&goal, &registry.config.mailbox, Utc::now())
            })
        }
    });
    // This is the real Goal callback after it captured an idle claim and
    // before its SQLite request. Unrelated controls still take the mutex.
    hook.wait_until_entered();
    assert!(plane.registry.all_snapshots().is_empty());
    assert!(
        plane
            .registry
            .list_agents(MAX_AGENT_LIST_LIMIT)
            .agents
            .is_empty()
    );
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    assert_eq!(plane.registry.with_goal_idle(|| true), None);
    let mut committing = Box::pin(plane.registry.commit(prepared, &cancellation));
    assert!(futures_util::poll!(&mut committing).is_pending());
    assert!(
        plane
            .registry
            .state
            .lock()
            .unwrap()
            .ownership_committing
            .is_empty()
    );
    assert!(!events(&plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    )));
    hook.release();
    assert!(idle.join().unwrap().unwrap().unwrap());
    let SubagentStartOutcome::Accepted(accepted) = committing.await.unwrap() else {
        panic!("released idle claim permits the exact waiting owner");
    };
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ownership_commit_pause_keeps_registry_controls_available() {
    let plane = plane(1);
    let mut child = stage_exit0(&plane);
    let hook = Arc::new(CommitBoundaryHook::default());
    plane.registry.state.lock().unwrap().commit_hook = Some(hook.clone());
    let registry = plane.registry.clone();
    let starting = tokio::spawn(async move {
        let cancellation = CancellationSignal::new();
        let prepared = registry
            .prepare(&start_spec("cancel before durability"), &cancellation)
            .await
            .unwrap();
        registry.commit(prepared, &cancellation).await
    });
    // Capacity and exact ID are reserved; the durable writer is parked
    // outside the registry mutex and before canonical ownership exists.
    hook.wait_until_entered();
    assert!(plane.registry.all_snapshots().is_empty());
    assert_eq!(
        plane
            .registry
            .state
            .lock()
            .unwrap()
            .ownership_committing
            .len(),
        1
    );
    plane
        .registry
        .cancel_all(CancellationReason::RuntimeShutdown);
    hook.release();
    assert!(matches!(
        starting.await.unwrap(),
        Ok(SubagentStartOutcome::RolledBack)
    ));
    assert!(events(&plane).is_empty());
    assert!(
        super::super::super::ipc::read_parent_frame(&mut child.peer)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        plane
            .registry
            .state
            .lock()
            .unwrap()
            .ownership_committing
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserved_commit_does_not_lock_registry_and_interrupt_captures_exact_reservation() {
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
    let hook = Arc::new(CommitBoundaryHook::default());
    plane.registry.state.lock().unwrap().reserved_commit_hook = Some(hook.clone());
    let registry = plane.registry.clone();
    let id = accepted.child_agent_id.clone();
    let sending = tokio::spawn(async move {
        registry
            .send_message(
                &id,
                "next",
                AgentActivationOrigin::ClientControl,
                CancellationSignal::new(),
            )
            .await
    });
    // The real owner has installed its durable physical authority and is
    // parked exactly before appending Reserved, with no registry lock.
    hook.wait_until_entered();
    let snapshot = plane
        .registry
        .agent_snapshot(&accepted.child_agent_id)
        .unwrap();
    assert_eq!(snapshot.state, AgentState::Admitting);
    let activation = snapshot.current_activation.unwrap();
    assert_ne!(activation, accepted.subagent_id);
    let mut interrupt = Box::pin(plane.registry.interrupt_agent(&accepted.child_agent_id));
    assert!(futures_util::poll!(&mut interrupt).is_pending());
    assert_eq!(plane.registry.list_agents(MAX_AGENT_LIST_LIMIT).matched, 1);
    hook.release();
    assert!(matches!(
        sending.await.unwrap(),
        Err(AgentControlError::Start(SubagentStartError::Cancelled))
    ));
    assert_eq!(
        interrupt.await.unwrap().activation_id,
        Some(activation.clone())
    );
    let admissions: Vec<_> = events(&plane)
        .into_iter()
        .filter_map(|event| {
            if let crate::events::types::RuntimeEvent::AgentActivationAdmission {
                activation_id,
                phase,
                ..
            } = event
                && activation_id == activation
            {
                Some(phase)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        admissions,
        vec![
            crate::events::types::AgentActivationAdmissionPhase::Reserved,
            crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                physical_settlement_proven: true
            },
        ]
    );
    assert_eq!(
        plane
            .registry
            .agent_snapshot(&accepted.child_agent_id)
            .unwrap()
            .state,
        AgentState::Inactive
    );
}
