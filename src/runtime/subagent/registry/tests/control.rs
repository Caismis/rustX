//! Parent control regressions: cancellation no-ops, workflow-owned refusal,
//! message admission order, sealing, and steer ticket ownership.

use super::*;

/// Workflow-owned finite `AgentRuns` never enter the durable native Agent
/// identity domain, so `send_message` rejects them even while running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_registry_authority_refuses_to_steer_a_workflow_owned_child() {
    let plane = plane(4);
    let child = stage_exit0(&plane);
    let accepted = start(
        &plane,
        &workflow_spec(&plane.conversation_id, "produce the node output"),
    )
    .await;
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("record")
            .state,
        SubagentState::Running,
        "an ordinary child in this state would be steerable"
    );

    let refused = plane
        .registry
        .send_message(
            &accepted.child_agent_id,
            "the workflow owns this instruction",
            crate::runtime::subagent::AgentActivationOrigin::ClientControl,
            crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect_err("a Workflow-owned AgentRun is never steerable");
    assert!(matches!(refused, AgentControlError::Unknown(id) if id == accepted.child_agent_id));
    assert_eq!(
        plane
            .registry
            .snapshot(&accepted.subagent_id)
            .expect("record")
            .state,
        SubagentState::Running,
        "the refusal mutates no lifecycle state"
    );

    // The child's own terminal path is untouched by the refusal.
    child
        .complete(
            ChildResultStatus::Succeeded,
            Some(r#"{"summary":"answer"}"#),
        )
        .await;
    let settled = plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        plane.registry.workflow_agent_output(&accepted.subagent_id),
        Some(serde_json::json!({"summary": "answer"})),
        "the frozen Workflow output contract is unchanged"
    );
}

#[tokio::test]
async fn cancel_of_an_unknown_or_terminal_subagent_is_a_noop() {
    let plane = plane(4);
    let unknown = SubagentId::new("conv_5d71cacf-35af-790f-8022-fe16cd1b9fe0-subagent-99");
    assert!(
        plane
            .registry
            .cancel(&unknown, CancellationReason::UserRequested)
            .is_none()
    );
    let child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("inspect")).await;
    child
        .complete(ChildResultStatus::Succeeded, Some("done"))
        .await;
    plane
        .registry
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    let after = plane
        .registry
        .cancel(&accepted.subagent_id, CancellationReason::UserRequested)
        .expect("known");
    assert_eq!(after.state, SubagentState::Succeeded);
}

#[tokio::test]
async fn concurrent_active_messages_follow_owner_admission_sequence() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("messages")).await;
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));
    let mut first = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "one",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    let mut second = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "two",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    assert!(futures_util::poll!(&mut first).is_pending());
    assert!(futures_util::poll!(&mut second).is_pending());
    let ParentFrame::Guidance(one) = child.read_frame().await else {
        panic!("first guidance");
    };
    let ParentFrame::Guidance(two) = child.read_frame().await else {
        panic!("second guidance");
    };
    assert_eq!(one.message, "one");
    assert_eq!(two.message, "two");
    assert_eq!(two.guidance_id, one.guidance_id + 1);
    // Response scheduling cannot alter durable delivery order.
    for message in [two, one] {
        super::super::super::ipc::write_child_frame(
            &mut child.peer,
            &ChildFrame::GuidanceResult(super::super::super::ipc::GuidanceResultFrame {
                guidance_id: message.guidance_id,
                outcome: super::super::super::ipc::ChildGuidanceOutcome::Accepted,
            }),
        )
        .await
        .unwrap();
    }
    assert_eq!(first.await.unwrap().activation_id, accepted.subagent_id);
    assert_eq!(second.await.unwrap().activation_id, accepted.subagent_id);
    child
        .send_result(ChildResultStatus::Succeeded, Some("ordered report"))
        .await;
    plane
        .registry
        .wait_agent(&accepted.child_agent_id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seal_arbitrates_with_message_admission_and_drains_the_winner() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("survey")).await;
    let id = accepted.subagent_id;
    assert!(matches!(child.read_frame().await, ParentFrame::Delegate(_)));

    // Admission is synchronous under the registry mutex. This call is
    // the gate establishing that the message wins before SealRequested.
    let (guidance_id, answer, _ticket) = plane
        .registry
        .admit_guidance(&id, "before seal", &[])
        .unwrap();
    super::super::super::ipc::write_child_frame(&mut child.peer, &ChildFrame::SealRequested)
        .await
        .unwrap();
    let ParentFrame::Guidance(guidance) = child.read_frame().await else {
        panic!("admitted input must precede the grant");
    };
    assert_eq!(guidance.guidance_id, guidance_id);
    assert_eq!(guidance.message, "before seal");
    assert!(matches!(child.read_frame().await, ParentFrame::SealGranted));
    assert_eq!(
        plane.registry.snapshot(&id).unwrap().state,
        SubagentState::Stopping
    );
    assert!(matches!(
        plane.registry.admit_guidance(&id, "after seal", &[]),
        Err(SubagentSteerError::Settled {
            state: SubagentState::Stopping
        })
    ));
    super::super::super::ipc::write_child_frame(
        &mut child.peer,
        &ChildFrame::GuidanceResult(super::super::super::ipc::GuidanceResultFrame {
            guidance_id,
            outcome: super::super::super::ipc::ChildGuidanceOutcome::Accepted,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        answer.await.unwrap(),
        super::super::super::ipc::ChildGuidanceOutcome::Accepted
    ));
}

/// **A dropped steer future disposes its own ticket without
/// synthesizing cancellation** (Issue #193 architecture review blocker
/// 2).
///
/// The interleaving is established deterministically, with no sleep and
/// no reliance on later steer behavior alone:
///
/// 1. the steer future is polled exactly once — admission runs
///    synchronously up to the child answer, arming the ticket and
///    handing the envelope to the driver;
/// 2. the child side proves the envelope was handed over by reading the
///    `Guidance` frame off the real control socket (the ticket is armed
///    strictly before the driver writes that frame);
/// 3. the child withholds its answer, and the steer future is then
///    dropped — caller abandonment. The guard's `Drop` runs
///    synchronously with the drop;
/// 4. the ticket list is observed directly through the authoritative
///    test accessor: it is empty;
/// 5. the lifecycle is still `Running` — no cancellation intent was
///    synthesized (a cancel would have committed `Cancelling`);
/// 6. a later valid steer still works normally through the very same
///    child, and the child's next frame is that steer's `Guidance`
///    envelope — never a synthesized `Cancel`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_steer_future_removes_its_ticket_without_synthesizing_cancellation() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("survey")).await;
    let id = accepted.subagent_id.clone();
    assert!(
        matches!(child.read_frame().await, ParentFrame::Delegate(_)),
        "the committed child is delegated first"
    );

    // (1) Admission: one poll drives the steer through `admit_guidance`
    // and parks it on the child's durable answer.
    let mut steer = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "steer one",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    let () = std::future::poll_fn(|cx| {
        assert!(
            steer.as_mut().poll(cx).is_pending(),
            "the steer cannot settle while the child withholds its answer"
        );
        std::task::Poll::Ready(())
    })
    .await;
    // (2) The envelope is provably on the driver's lane.
    let ParentFrame::Guidance(_) = child.read_frame().await else {
        panic!("the steer routes exactly one guidance envelope to the child");
    };
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        1,
        "the admitted steer holds exactly its own ticket"
    );

    // (3)+(4) The caller abandons the steer future while the child
    // withholds its answer: the guard's `Drop` removes exactly its own
    // ticket, synchronously — proven by the authoritative observation,
    // not by later behavior alone.
    drop(steer);
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        0,
        "an abandoned steer future leaves no ticket behind"
    );
    // (5) No cancellation was synthesized: the lifecycle is still
    // `Running` — a synthesized cancel would have committed
    // `Cancelling` under the same mutex.
    assert_eq!(
        plane.registry.snapshot(&id).expect("record").state,
        SubagentState::Running,
        "dropping a steer future never cancels the child"
    );

    // (6) A later valid steer still works normally through the very
    // same running child.
    let second = tokio::spawn({
        let registry = plane.registry.clone();
        let agent_id = accepted.child_agent_id.clone();
        async move {
            registry
                .send_message(
                    &agent_id,
                    "steer after the abandonment",
                    crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                    crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
        }
    });
    let ParentFrame::Guidance(guidance) = child.read_frame().await else {
        panic!("the child's next frame is the second steer's envelope, never a synthesized Cancel");
    };
    super::super::super::ipc::write_child_frame(
        &mut child.peer,
        &ChildFrame::GuidanceResult(super::super::super::ipc::GuidanceResultFrame {
            guidance_id: guidance.guidance_id,
            outcome: super::super::super::ipc::ChildGuidanceOutcome::Accepted,
        }),
    )
    .await
    .expect("guidance answer");
    let second = second.await.expect("steer task").expect("accepted");
    assert_eq!(second.activation_id, id);
    assert_eq!(second.agent_id, accepted.child_agent_id);
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        0,
        "a committed steer leaves no ticket behind either"
    );
}

/// **Terminal settlement disposes every outstanding steer reservation**
/// (Issue #193 architecture review blocker 2).
///
/// The steer is admitted and parked with its ticket armed, the child is
/// deliberately **not** answered and the steer future is deliberately
/// **not** dropped, and then the child's terminal authority commits:
///
/// 1. the terminal record provably contains no outstanding guidance
///    arbitration state (direct authoritative observation of the
///    settled record);
/// 2. polling the still-held steer future afterwards completes
///    deterministically — the driver already dropped its transport
///    waiter on exit, and the commit observes the cleared ticket — with
///    the deterministic child-side refusal, never an optimistic
///    acceptance;
/// 3. the lifecycle stays terminal and absorbing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_settlement_clears_outstanding_steer_reservations() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("survey")).await;
    let id = accepted.subagent_id.clone();
    assert!(
        matches!(child.read_frame().await, ParentFrame::Delegate(_)),
        "the committed child is delegated first"
    );

    // Admit a steer and park it with the ticket armed; the child
    // withholds its answer and the future is deliberately kept alive.
    let mut steer = Box::pin(plane.registry.send_message(
        &accepted.child_agent_id,
        "never answered",
        crate::runtime::subagent::AgentActivationOrigin::ClientControl,
        crate::runtime::cancellation::CancellationSignal::new(),
    ));
    let () = std::future::poll_fn(|cx| {
        assert!(steer.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let ParentFrame::Guidance(_) = child.read_frame().await else {
        panic!("the steer routes exactly one guidance envelope");
    };
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        1,
        "the outstanding reservation is armed when the terminal commits"
    );

    // The child settles without ever answering: its terminal authority
    // commits and must invalidate every remaining ticket.
    child
        .send_result(ChildResultStatus::Succeeded, Some("done"))
        .await;
    drop(child);
    let settled = plane
        .registry
        .wait_until_settled(&id)
        .await
        .expect("terminal settlement");
    assert_eq!(
        settled.state,
        SubagentState::Succeeded,
        "the child settles through its ordinary terminal path"
    );
    // (1) The terminal record is clean: no live steering arbitration
    // state survives terminal authority.
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        0,
        "a terminal record never retains outstanding steering reservations"
    );

    // A write without an acknowledgement is ambiguous, even after settlement.
    let outcome = std::future::poll_fn(|cx| steer.as_mut().poll(cx)).await;
    assert!(matches!(outcome, Err(AgentControlError::DeliveryUnknown)));
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        0,
        "the refused steer leaves no ticket behind"
    );
}

/// **A natural terminal after the child's durable acceptance never
/// erases the parent acknowledgement** (Issue #193 architecture review
/// blocker 1).
///
/// This is the exact interleaving the blocker names: the child durably
/// accepts the guidance and its `GuidanceResult(Accepted)` reaches the
/// parent-side driver, the parent steer future has **not yet** run its
/// registry acknowledgement commit, and the child then completes
/// naturally. The natural terminal authority clears the steer ticket;
/// when the acknowledgement resumes it must still report `accepted` — a
/// scheduling delay of the parent future cannot retroactively erase a
/// durable semantic fact that already committed in the child.
///
/// Every synchronization point is a barrier, never a sleep:
///
/// 1. steer admission completed — the steer is polled once, and the
///    child side reads the `Guidance` envelope off the real control
///    socket (the ticket is armed strictly before the driver writes the
///    frame);
/// 2. child durable acceptance completed **and observed by the
///    registry** — the child answers `Accepted`, and the steer
///    acknowledgement hook parks the steer immediately after its ticket
///    advanced to `ChildAcceptedPendingParentCommit`, provably before
///    the acknowledgement commit;
/// 3. while the parent acknowledgement is parked, the natural terminal
///    authority commits — the child reports its terminal, the settled
///    record is observed, and the terminal record provably holds no
///    live ticket state (cleanup removed the accepted-pending ticket);
/// 4. only afterwards the parked acknowledgement resumes and commits:
///    the steer returns `accepted`, and no stale steer arbitration
///    state remains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn natural_terminal_after_child_acceptance_does_not_erase_the_acknowledgement() {
    let plane = plane(4);
    let mut child = stage_exit0(&plane);
    let accepted = start(&plane, &start_spec("survey")).await;
    let id = accepted.subagent_id.clone();
    assert!(
        matches!(child.read_frame().await, ParentFrame::Delegate(_)),
        "the committed child is delegated first"
    );

    // Park the steer acknowledgement at the exact pre-commit edge, after
    // the child's durable decision has been recorded against its ticket.
    let hook = Arc::new(SteerAcknowledgementHook::default());
    plane
        .registry
        .install_steer_acknowledgement_hook(hook.clone());
    let steered = tokio::spawn({
        let registry = plane.registry.clone();
        let agent_id = accepted.child_agent_id.clone();
        async move {
            registry
                .send_message(
                    &agent_id,
                    "accepted before the natural terminal",
                    crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                    crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
        }
    });

    // (1) Admission completed: the envelope is provably on the driver's
    // lane (the ticket is armed strictly before the driver writes the
    // frame).
    let ParentFrame::Guidance(guidance) = child.read_frame().await else {
        panic!("the steer routes exactly one guidance envelope to the child");
    };
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        1,
        "the admitted steer holds exactly its own ticket"
    );
    assert_eq!(
        plane.registry.accepted_guidance_pending_commit(&id),
        0,
        "before the child answers, the ticket is AwaitingChildDecision"
    );

    // (2) The child durably accepts; the steer caller observes the
    // answer, advances its ticket, and parks at the acknowledgement
    // boundary — deterministically, with the acknowledgement commit
    // still owed.
    super::super::super::ipc::write_child_frame(
        &mut child.peer,
        &ChildFrame::GuidanceResult(super::super::super::ipc::GuidanceResultFrame {
            guidance_id: guidance.guidance_id,
            outcome: super::super::super::ipc::ChildGuidanceOutcome::Accepted,
        }),
    )
    .await
    .expect("guidance answer");
    hook.wait_until_entered().await;
    assert_eq!(
        plane.registry.accepted_guidance_pending_commit(&id),
        1,
        "the registry knows the child's Accepted answer: the ticket is \
             ChildAcceptedPendingParentCommit while the parent acknowledgement is parked"
    );
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        1,
        "the parked acknowledgement still owns its ticket"
    );

    // (3) The child completes naturally while the acknowledgement is
    // parked: the terminal authority commits and clears the
    // accepted-pending ticket (lifecycle cleanup — the acceptance is
    // preserved by the caller-held child outcome through the commit).
    child
        .send_result(ChildResultStatus::Succeeded, Some("done"))
        .await;
    drop(child);
    let settled = plane
        .registry
        .wait_until_settled(&id)
        .await
        .expect("natural terminal settlement");
    assert_eq!(settled.state, SubagentState::Succeeded);
    assert_eq!(
        published_terminal_states(&plane, &id),
        vec![SubagentTerminalState::Succeeded],
        "the natural terminal result is valid and exactly once"
    );
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        0,
        "natural terminal cleanup leaves the settled record with no live \
             steering arbitration state — but must not erase the acceptance"
    );

    // (4) Only now does the parent acknowledgement resume and commit:
    // the ticket is gone and no cancellation ever committed, and the
    // caller-held child outcome is `Accepted`, so the steer is still
    // reported accepted — never transformed into a refusal by the
    // natural terminal.
    hook.release();
    let accepted = steered.await.expect("steer task").expect("accepted");
    assert_eq!(
        accepted.activation_id, id,
        "the acknowledgement names the very child that durably accepted"
    );
    assert_eq!(
        plane.registry.outstanding_guidance_tickets(&id),
        0,
        "the committed acknowledgement leaves no stale steer state"
    );
}
