use super::{ChildControlDispatcher, ChildControlEvent};
use crate::runtime::identity::ProcessUnitId;
use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolCallId, ToolId};
use crate::runtime::interaction::{
    ApprovalDecision, InteractionAdmissionError, InteractionKind, InteractionOutcome,
    InteractionRef, InteractionRequest, InteractionResponse,
};
use crate::runtime::nested_containment::AnchorError;
use crate::runtime::subagent::activity::SubagentObservation;
use crate::runtime::subagent::ipc::{
    ActivityFrame, ChildFrame, DelegationFrame, DiagnosticFrame,
    InteractionPublicationAdmissionFrame, ParentFrame, ProcessUnitAckFrame,
    ProcessUnitRefusalFrame, read_activity_frame, read_child_frame, write_parent_frame,
};
use crate::runtime::types::CancellationReason;
use crate::tools::types::{ToolInvocationMode, ToolOrigin};
use futures_util::FutureExt;

fn pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
    tokio::net::UnixStream::pair().expect("control socket pair")
}

fn activity(revision: u64) -> ActivityFrame {
    ActivityFrame {
        observation: SubagentObservation {
            revision,
            ..SubagentObservation::default()
        },
    }
}

/// Two units may have offers outstanding at once, and each ACK opens
/// exactly its own gate: routing is by typed identity, never by order.
#[tokio::test]
async fn acknowledgements_route_by_exact_unit_identity() {
    let (mut parent, child) = pair();
    // The observation channel is held open but carries nothing here.
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let authority = dispatcher.anchor_authority();

    let first = ProcessUnitId::new("unit-a");
    let second = ProcessUnitId::new("unit-b");
    let offer_first = tokio::spawn({
        let authority = authority.clone();
        let unit = first.clone();
        async move { authority.offer(unit, 11).await }
    });
    let offer_second = tokio::spawn({
        let authority = authority.clone();
        let unit = second.clone();
        async move { authority.offer(unit, 22).await }
    });

    // Collect both offers before answering, so the answers are
    // deliberately delivered in the opposite order.
    let mut offered = Vec::new();
    for _ in 0..2 {
        match read_child_frame(&mut parent).await.expect("offer") {
            Some(ChildFrame::AnchorOffered(frame)) => offered.push(frame),
            other => panic!("expected an anchor offer, got {other:?}"),
        }
    }
    assert_eq!(offered.len(), 2);

    write_parent_frame(
        &mut parent,
        &ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
            unit_id: second.clone(),
        }),
    )
    .await
    .expect("ack second");
    assert_eq!(
        offer_second.await.expect("join"),
        Ok(()),
        "the second unit's gate opens on its own ACK"
    );
    assert!(
        !offer_first.is_finished(),
        "the first unit's gate stays closed until its own ACK arrives"
    );

    write_parent_frame(
        &mut parent,
        &ParentFrame::AnchorAccepted(ProcessUnitAckFrame { unit_id: first }),
    )
    .await
    .expect("ack first");
    assert_eq!(offer_first.await.expect("join"), Ok(()));
}

/// A refusal fails exactly the refused unit's gate with its reason.
#[tokio::test]
async fn a_refusal_fails_only_that_unit() {
    let (mut parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let authority = dispatcher.anchor_authority();
    let unit = ProcessUnitId::new("unit-a");
    let offer = tokio::spawn({
        let authority = authority.clone();
        let unit = unit.clone();
        async move { authority.offer(unit, 11).await }
    });
    assert!(matches!(
        read_child_frame(&mut parent).await.expect("offer"),
        Some(ChildFrame::AnchorOffered(_))
    ));
    write_parent_frame(
        &mut parent,
        &ParentFrame::AnchorRefused(ProcessUnitRefusalFrame {
            unit_id: unit,
            reason: "duplicate".to_owned(),
        }),
    )
    .await
    .expect("refuse");
    assert_eq!(
        offer.await.expect("join"),
        Err(AnchorError::Refused("duplicate".to_owned()))
    );
}

/// Parent EOF fails every outstanding offer and publishes parent loss,
/// so a preparation racing it settles rather than continuing.
///
/// The observation channel stays **open** the whole time (Issue #178):
/// control-channel EOF alone is the parent-liveness authority, and a
/// live observation transport can never substitute for it.
#[tokio::test]
async fn parent_eof_fails_every_outstanding_offer() {
    let (parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let authority = dispatcher.anchor_authority();
    let handle = dispatcher.handle();
    let offer = tokio::spawn({
        let authority = authority.clone();
        async move { authority.offer(ProcessUnitId::new("unit-a"), 11).await }
    });
    // Drop the parent end only after the offer is provably outstanding.
    let mut parent = parent;
    assert!(matches!(
        read_child_frame(&mut parent).await.expect("offer"),
        Some(ChildFrame::AnchorOffered(_))
    ));
    drop(parent);
    assert_eq!(offer.await.expect("join"), Err(AnchorError::ParentLost));
    handle.parent_lost_signal().await;
    assert!(handle.parent_lost());
}

/// An offer made after parent loss fails immediately: nothing may start.
#[tokio::test]
async fn an_offer_after_parent_loss_fails_immediately() {
    let (parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let handle = dispatcher.handle();
    let authority = dispatcher.anchor_authority();
    drop(parent);
    handle.parent_lost_signal().await;
    assert_eq!(
        authority.offer(ProcessUnitId::new("unit-a"), 11).await,
        Err(AnchorError::ParentLost)
    );
}

/// The child's drain must complete even while other owners still hold
/// outbound senders.
///
/// The nested anchor authority is installed **process-wide** and every
/// live supervised unit lease holds a clone, so "the writer ends when
/// every sender drops" would never happen: the child would hang at exit
/// instead of letting the parent observe its drain. Drain therefore
/// closes the queue explicitly, flushing what is already queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_completes_while_other_owners_still_hold_senders() {
    let (mut parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    // Two independent owners keep senders alive across the drain,
    // exactly as the installed authority and a live unit lease would.
    let authority = dispatcher.anchor_authority();
    let handle = dispatcher.handle();
    handle
        .send_reliable(ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("answer".to_owned()),
                diagnostic: None,
            },
        ))
        .await
        .expect("the terminal result is queued");

    tokio::time::timeout(std::time::Duration::from_secs(10), dispatcher.shutdown())
        .await
        .expect("the drain must complete while other senders are alive");

    // The queued terminal result was flushed before the write half
    // closed, and the parent then observes a clean EOF.
    assert!(matches!(
        read_child_frame(&mut parent).await.expect("result"),
        Some(ChildFrame::Result(_))
    ));
    assert_eq!(read_child_frame(&mut parent).await.expect("eof"), None);
    // The surviving owners are inert rather than dangling: a release on
    // a drained transport is a no-op, not a panic or a hang.
    authority.release(ProcessUnitId::new("unit-a"), 11).await;
    assert_eq!(
        handle
            .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                message: "after drain".to_owned(),
            }))
            .await,
        Err(AnchorError::ParentLost),
        "a send after the drain is refused rather than queued forever"
    );
}

/// Delegate and Cancel reach the semantic driver as events; the raw
/// stream is never exposed.
#[tokio::test]
async fn semantic_frames_become_control_events() {
    let (mut parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
    write_parent_frame(
        &mut parent,
        &ParentFrame::Delegate(DelegationFrame {
            attachments: Vec::new(),
            task: "inspect".to_owned(),
            context: None,
            interaction_provider_available: false,
        }),
    )
    .await
    .expect("delegate");
    write_parent_frame(
        &mut parent,
        &ParentFrame::Cancel {
            reason: Some(CancellationReason::UserRequested),
        },
    )
    .await
    .expect("cancel");
    assert!(matches!(
        dispatcher.next_event().await,
        Some(ChildControlEvent::Delegate(_))
    ));
    assert_eq!(
        dispatcher.next_event().await,
        Some(ChildControlEvent::Cancel {
            reason: Some(CancellationReason::UserRequested),
        })
    );
}

/// A mismatched admission result is rejected by the child-side transport
/// correlation and cannot authorize a different routed interaction. A
/// later exact response still resolves only its own request.
#[tokio::test]
async fn publication_admission_requires_exact_routed_identity() {
    let (mut parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let handle = dispatcher.handle();
    let first = InteractionRef::new(
        ConversationId::new("conv_b520d0ec-730a-7133-8d51-7de417e31c5a"),
        InteractionId::new("interaction-1"),
    );
    let first_waiter = tokio::spawn({
        let handle = handle.clone();
        let first = first.clone();
        async move { handle.admit_interaction_publication(first).await }
    });
    let first_request = match read_child_frame(&mut parent).await.expect("read admission") {
        Some(ChildFrame::InteractionPublicationAdmissionRequested(request)) => request,
        other => panic!("unexpected admission frame: {other:?}"),
    };
    assert_eq!(first_request.interaction, first);
    let mut mismatched = first_request.clone();
    mismatched.interaction = InteractionRef::new(
        ConversationId::new("conv_b520d0ec-730a-7133-8d51-7de417e31c5a"),
        InteractionId::new("interaction-2"),
    );
    mismatched.admitted = true;
    write_parent_frame(
        &mut parent,
        &ParentFrame::InteractionPublicationAdmissionResult(mismatched),
    )
    .await
    .expect("write mismatched admission");
    assert_eq!(
        first_waiter.await.expect("first waiter"),
        Err(InteractionAdmissionError::ControlLost)
    );

    let second = InteractionRef::new(
        ConversationId::new("conv_b520d0ec-730a-7133-8d51-7de417e31c5a"),
        InteractionId::new("interaction-2"),
    );
    let second_waiter = tokio::spawn({
        let handle = handle.clone();
        let second = second.clone();
        async move { handle.admit_interaction_publication(second).await }
    });
    let second_request = match read_child_frame(&mut parent).await.expect("read admission") {
        Some(ChildFrame::InteractionPublicationAdmissionRequested(request)) => request,
        other => panic!("unexpected admission frame: {other:?}"),
    };
    assert_eq!(second_request.interaction, second);
    write_parent_frame(
        &mut parent,
        &ParentFrame::InteractionPublicationAdmissionResult(InteractionPublicationAdmissionFrame {
            request_id: second_request.request_id,
            interaction: second_request.interaction.clone(),
            admitted: true,
        }),
    )
    .await
    .expect("write exact admission");
    let permit = second_waiter.await.expect("second waiter").expect("permit");
    assert!(permit.matches(&second));

    dispatcher.shutdown().await;
}

/// A proven-terminal release under bounded outbound backpressure is
/// **never silently dropped**: with the queue provably full (the writer
/// is gated, so `OUTBOUND_CAPACITY` completed sends fill it exactly),
/// the release future pends instead of resolving, and once the
/// backpressure clears the exact `AnchorReleased` frame arrives and
/// removes exactly the parent's retained anchor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_proven_terminal_release_survives_bounded_backpressure() {
    use futures_util::FutureExt;

    let (mut parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let gate = super::WriterGate::default();
    let dispatcher = ChildControlDispatcher::start_with_gates(
        child,
        observation_child,
        Some(gate.clone()),
        None,
    );
    let handle = dispatcher.handle();
    let authority = dispatcher.anchor_authority();

    // Fill the bounded outbound queue exactly. The writer is gated
    // before its first receive, so these completed sends are the proof
    // that the queue is now full — no timing is involved.
    for index in 0..super::OUTBOUND_CAPACITY {
        handle
            .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                message: format!("filler {index}"),
            }))
            .await
            .expect("queue capacity remains while filling");
    }

    // The parent retains this exact anchor; the child has just proven
    // the unit physically terminal and releases it under backpressure.
    let unit = ProcessUnitId::new("unit-a");
    let mut retained = crate::runtime::subagent::anchors::RetainedProcessUnits::default();
    retained.retain(unit.clone(), 4242).expect("retained");
    let mut release = Box::pin(authority.release(unit.clone(), 4242));
    assert!(
        release.as_mut().now_or_never().is_none(),
        "a release behind a full queue must wait for capacity; it is never dropped"
    );

    // The backpressure clears: the writer drains the queue in order and
    // the release is delivered — never lost.
    gate.open();
    release.await;
    for _ in 0..super::OUTBOUND_CAPACITY {
        assert!(
            matches!(
                read_child_frame(&mut parent).await.expect("queued frame"),
                Some(ChildFrame::Diagnostic(_))
            ),
            "the queued fillers drain first, in order"
        );
    }
    assert_eq!(
        read_child_frame(&mut parent)
            .await
            .expect("the release frame"),
        Some(ChildFrame::AnchorReleased(
            crate::runtime::subagent::ipc::ProcessUnitAnchorFrame {
                unit_id: unit.clone(),
                pgid: 4242
            }
        )),
        "the exact AnchorReleased arrives once the backpressure clears"
    );
    // The parent applies exactly this release to exactly this anchor.
    assert!(retained.release(&unit, 4242));
    assert!(retained.is_empty());

    dispatcher.shutdown().await;
}

/// Transport isolation of the terminal result (Issue #178). The
/// observation writer is gated — the deterministic model of a
/// backpressured observation transport — with 100 activity revisions
/// saturated into the latest-value slot. The terminal `Result`:
///
/// 1. completes its send immediately even with the reliable writer
///    gated (activity consumed no reliable-queue capacity), and
/// 2. crosses the reliable control channel the instant the reliable
///    writer runs, while the observation transport stays stalled.
///
/// Only when the observation stall clears does exactly one activity
/// frame arrive on the observation channel: the latest revision, never
/// an intermediate, and never on the control channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_observation_transport_never_delays_the_terminal_result() {
    use futures_util::FutureExt;

    let (mut parent, child) = pair();
    let (mut observation_parent, observation_child) = pair();
    let writer_gate = super::WriterGate::default();
    let observation_gate = super::WriterGate::default();
    let dispatcher = ChildControlDispatcher::start_with_gates(
        child,
        observation_child,
        Some(writer_gate.clone()),
        Some(observation_gate.clone()),
    );
    let handle = dispatcher.handle();

    // Saturate the disposable lane: every publication overwrites the
    // previous unpublished value in place, synchronously.
    for revision in 1..=100u64 {
        handle.publish_activity(activity(revision));
    }

    // The terminal result never waits on activity capacity: the
    // reliable queue is provably empty, so this send resolves
    // immediately even though both writers are parked.
    let result = crate::runtime::subagent::ipc::ResultFrame {
        status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
        content: Some("the terminal answer".to_owned()),
        diagnostic: None,
    };
    handle
        .send_reliable(ChildFrame::Result(result.clone()))
        .await
        .expect("Result never waits on disposable activity");
    assert!(
        read_child_frame(&mut parent).now_or_never().is_none(),
        "the gated reliable writer has written nothing yet"
    );

    // The observation transport stays stalled: the Result still
    // crosses the reliable control channel immediately and intact.
    writer_gate.open();
    assert_eq!(
        read_child_frame(&mut parent).await.expect("result frame"),
        Some(ChildFrame::Result(result)),
        "the Result arrives while the observation transport is stalled"
    );
    assert!(
        read_activity_frame(&mut observation_parent)
            .now_or_never()
            .is_none(),
        "the stalled observation channel carries nothing"
    );

    // Once the stall clears, exactly one activity frame arrives on the
    // observation channel: the latest revision, never an intermediate.
    observation_gate.open();
    assert_eq!(
        read_activity_frame(&mut observation_parent)
            .await
            .expect("activity frame"),
        Some(activity(100)),
        "the coalesced activity carries only the latest revision"
    );
    assert!(
        read_activity_frame(&mut observation_parent)
            .now_or_never()
            .is_none(),
        "no intermediate revision ever reached the wire"
    );
    assert!(
        read_child_frame(&mut parent).now_or_never().is_none(),
        "activity never touches the reliable control channel"
    );

    dispatcher.shutdown().await;
}

/// Routed interaction requests and settlements use the reliable control
/// lane, even while the disposable observation writer is stalled and its
/// latest-value slot is being overwritten. The exact pair identity and
/// both interaction kinds stay intact on the semantic lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one deterministic two-lane transport proof
async fn routed_interactions_are_reliable_and_observation_independent() {
    let (mut parent, child) = pair();
    let (mut observation_parent, observation_child) = pair();
    let observation_gate = super::WriterGate::default();
    let dispatcher = ChildControlDispatcher::start_with_gates(
        child,
        observation_child,
        None,
        Some(observation_gate.clone()),
    );
    let handle = dispatcher.handle();

    for revision in 1..=100u64 {
        handle.publish_activity(activity(revision));
    }

    let approval = InteractionRequest {
        id: InteractionId::new("approval-1"),
        conversation_id: ConversationId::new("conv_b520d0ec-730a-7133-8d51-7de417e31c5a"),
        execution: rustx::runtime::interaction::InteractionExecution::Attempt {
            attempt_id: AttemptId::new("attempt-1"),
            turn: 1,
        },
        kind: InteractionKind::Approval {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("call-1"),
            },
            tool_id: ToolId::new("tool-1"),
            tool_name: "probe".to_owned(),
            origin: ToolOrigin::Builtin,
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"path": "original"}),
            reason: "approval required".to_owned(),
        },
    };
    let questionnaire = InteractionRequest {
        id: InteractionId::new("questionnaire-1"),
        conversation_id: approval.conversation_id.clone(),
        execution: rustx::runtime::interaction::InteractionExecution::Attempt {
            attempt_id: AttemptId::new("attempt-1"),
            turn: 2,
        },
        kind: InteractionKind::Questionnaire {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("questionnaire-call"),
            },
            requester: crate::events::InteractionRequester {
                tool_id: crate::runtime::identity::ToolId::new("tool-ask-user"),
                tool_name: "ask_user".to_owned(),
                origin: crate::tools::types::ToolOrigin::Builtin,
            },
            questionnaire: crate::runtime::interaction::QuestionnaireSpecification {
                questions: vec![crate::runtime::interaction::QuestionSpecification {
                    question: "Which target?".to_owned(),
                    header: "Target".to_owned(),
                    answer: crate::runtime::interaction::AnswerSpecification::SingleChoice(
                        crate::runtime::interaction::SingleChoiceSpecification {
                            options: vec![
                                crate::runtime::interaction::OptionSpecification {
                                    label: "staging".to_owned(),
                                    description: "safe".to_owned(),
                                    preview: None,
                                },
                                crate::runtime::interaction::OptionSpecification {
                                    label: "production".to_owned(),
                                    description: "live".to_owned(),
                                    preview: None,
                                },
                            ],
                            allow_custom: true,
                        },
                    ),
                }],
            },
        },
    };
    let approval_ref = InteractionRef::new(approval.conversation_id.clone(), approval.id.clone());
    let questionnaire_ref = InteractionRef::new(
        questionnaire.conversation_id.clone(),
        questionnaire.id.clone(),
    );
    let approval = crate::observation::interaction::interaction_request_header(
        &approval,
        crate::runtime::interaction::InteractionSource::Primary,
    );
    let questionnaire = crate::observation::interaction::interaction_request_header(
        &questionnaire,
        crate::runtime::interaction::InteractionSource::Primary,
    );
    handle
        .send_reliable(ChildFrame::InteractionRequested(Box::new(approval.clone())))
        .await
        .expect("approval request is reliable");
    handle
        .send_reliable(ChildFrame::InteractionRequested(Box::new(
            questionnaire.clone(),
        )))
        .await
        .expect("questionnaire request is reliable");
    handle
        .send_reliable(ChildFrame::InteractionSettled {
            interaction: approval_ref.clone(),
            outcome: InteractionOutcome::Responded {
                response: InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            },
        })
        .await
        .expect("approval settlement is reliable");
    handle
        .send_reliable(ChildFrame::InteractionSettled {
            interaction: questionnaire_ref.clone(),
            outcome: InteractionOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            },
        })
        .await
        .expect("questionnaire settlement is reliable");

    assert_eq!(
        read_child_frame(&mut parent)
            .await
            .expect("approval request"),
        Some(ChildFrame::InteractionRequested(Box::new(approval))),
    );
    assert_eq!(
        read_child_frame(&mut parent)
            .await
            .expect("questionnaire request"),
        Some(ChildFrame::InteractionRequested(Box::new(questionnaire))),
    );
    assert_eq!(
        read_child_frame(&mut parent)
            .await
            .expect("approval settlement"),
        Some(ChildFrame::InteractionSettled {
            interaction: approval_ref,
            outcome: InteractionOutcome::Responded {
                response: InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            },
        }),
    );
    assert_eq!(
        read_child_frame(&mut parent)
            .await
            .expect("questionnaire settlement"),
        Some(ChildFrame::InteractionSettled {
            interaction: questionnaire_ref,
            outcome: InteractionOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            },
        }),
    );
    assert!(
        read_activity_frame(&mut observation_parent)
            .now_or_never()
            .is_none(),
        "semantic interaction traffic never uses the stalled observation lane"
    );

    observation_gate.open();
    assert_eq!(
        read_activity_frame(&mut observation_parent)
            .await
            .expect("coalesced activity"),
        Some(activity(100)),
    );
    dispatcher.shutdown().await;
}

/// Transport isolation of containment ownership (Issue #178): with the
/// observation transport stalled and the activity lane saturated, a
/// proven-terminal `release` resolves immediately — the reliable lane
/// has capacity by construction — and the exact `AnchorReleased` frame
/// crosses the control channel while the observation transport stays
/// stalled, releasing exactly the parent's retained anchor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_observation_transport_never_delays_a_containment_release() {
    use futures_util::FutureExt;

    let (mut parent, child) = pair();
    let (mut observation_parent, observation_child) = pair();
    let observation_gate = super::WriterGate::default();
    let dispatcher = ChildControlDispatcher::start_with_gates(
        child,
        observation_child,
        None,
        Some(observation_gate.clone()),
    );
    let handle = dispatcher.handle();
    let authority = dispatcher.anchor_authority();

    // Saturate the disposable lane.
    for revision in 1..=100u64 {
        handle.publish_activity(activity(revision));
    }

    // The parent retains this exact anchor; the child has just proven
    // the unit physically terminal. The release resolves immediately
    // and crosses the control channel even though the observation
    // transport never accepts another byte.
    let unit = ProcessUnitId::new("unit-a");
    let mut retained = crate::runtime::subagent::anchors::RetainedProcessUnits::default();
    retained.retain(unit.clone(), 4242).expect("retained");
    authority.release(unit.clone(), 4242).await;
    assert_eq!(
        read_child_frame(&mut parent).await.expect("release frame"),
        Some(ChildFrame::AnchorReleased(
            crate::runtime::subagent::ipc::ProcessUnitAnchorFrame {
                unit_id: unit.clone(),
                pgid: 4242
            }
        )),
        "the exact AnchorReleased arrives while the observation transport is stalled"
    );
    assert!(
        read_activity_frame(&mut observation_parent)
            .now_or_never()
            .is_none(),
        "the stalled observation channel carries nothing"
    );
    // The parent applies exactly this release to exactly this anchor.
    assert!(retained.release(&unit, 4242));
    assert!(retained.is_empty());

    // Once the stall clears, the coalesced activity arrives on the
    // observation channel only.
    observation_gate.open();
    assert_eq!(
        read_activity_frame(&mut observation_parent)
            .await
            .expect("activity frame"),
        Some(activity(100))
    );
    assert!(
        read_child_frame(&mut parent).now_or_never().is_none(),
        "activity never touches the reliable control channel"
    );

    dispatcher.shutdown().await;
}

/// Observation transport loss is diagnostics-only (Issue #178): with
/// the parent end of the observation channel dropped, activity
/// publication stays synchronous and infallible and the observation
/// writer quietly ends, while the reliable control channel keeps every
/// guarantee — an anchor offer/ack round-trip and the terminal result
/// both complete — and the child never mistakes observation EOF for
/// parent loss.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn observation_transport_loss_is_diagnostics_only() {
    let (mut parent, child) = pair();
    let (observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let handle = dispatcher.handle();
    let authority = dispatcher.anchor_authority();

    // The observation transport is gone. Publication is unaffected.
    drop(observation_parent);
    for revision in 1..=10u64 {
        handle.publish_activity(activity(revision));
    }

    // The reliable channel is untouched: an anchor offer round-trips...
    let offer = tokio::spawn({
        let authority = authority.clone();
        async move { authority.offer(ProcessUnitId::new("unit-a"), 11).await }
    });
    assert!(matches!(
        read_child_frame(&mut parent).await.expect("offer"),
        Some(ChildFrame::AnchorOffered(_))
    ));
    write_parent_frame(
        &mut parent,
        &ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
            unit_id: ProcessUnitId::new("unit-a"),
        }),
    )
    .await
    .expect("ack");
    assert_eq!(offer.await.expect("join"), Ok(()));

    // ...and the terminal result arrives.
    handle
        .send_reliable(ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("the terminal answer".to_owned()),
                diagnostic: None,
            },
        ))
        .await
        .expect("the terminal result is queued");
    assert!(matches!(
        read_child_frame(&mut parent).await.expect("result"),
        Some(ChildFrame::Result(_))
    ));

    // Observation EOF is never parent loss.
    assert!(
        !handle.parent_lost(),
        "observation transport loss is not lifecycle evidence"
    );

    dispatcher.shutdown().await;
}

/// Destroying a held anchor's owner — by plain drop or by task abort —
/// is **not** terminal proof: no `AnchorReleased` may be emitted, and
/// the parent's retained anchor (its catastrophic containment authority
/// for that exact unit) must survive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unproven_owner_loss_never_releases_the_retained_anchor() {
    let (mut parent, child) = pair();
    let (_observation_parent, observation_child) = pair();
    let dispatcher = ChildControlDispatcher::start(child, observation_child);
    let authority = dispatcher.anchor_authority();
    let handle = dispatcher.handle();

    // The parent-side view: both offered anchors are ACKed and
    // retained. Each lease comes from the real `retain_with` path a
    // supervised-unit owner uses.
    let mut retained = crate::runtime::subagent::anchors::RetainedProcessUnits::default();
    let mut acknowledge = async |unit: &ProcessUnitId, pgid: i32| {
        let pending = tokio::spawn({
            let authority = authority.clone();
            let unit = unit.clone();
            async move {
                crate::runtime::nested_containment::retain_with(unit, pgid, Some(authority)).await
            }
        });
        assert!(matches!(
            read_child_frame(&mut parent).await.expect("offer frame"),
            Some(ChildFrame::AnchorOffered(_))
        ));
        retained.retain(unit.clone(), pgid).expect("retained");
        write_parent_frame(
            &mut parent,
            &ParentFrame::AnchorAccepted(ProcessUnitAckFrame {
                unit_id: unit.clone(),
            }),
        )
        .await
        .expect("ack");
        pending
            .await
            .expect("offer task")
            .expect("acknowledged lease")
    };

    // 1. A held lease destroyed by plain owner drop.
    let dropped = acknowledge(&ProcessUnitId::new("unit-drop"), 11).await;
    drop(dropped);

    // 2. A held lease destroyed by owner task abort mid-park.
    let aborted = acknowledge(&ProcessUnitId::new("unit-abort"), 22).await;
    let owner = tokio::spawn(async move {
        let _held = aborted;
        std::future::pending::<()>().await;
    });
    owner.abort();
    let _ = owner.await;

    // The wire proves the silence: frames are totally ordered, so any
    // manufactured release would arrive before this trailing frame.
    handle
        .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
            message: "after both owner losses".to_owned(),
        }))
        .await
        .expect("trailing frame queued");
    assert_eq!(
        read_child_frame(&mut parent).await.expect("trailing frame"),
        Some(ChildFrame::Diagnostic(DiagnosticFrame {
            message: "after both owner losses".to_owned()
        })),
        "no AnchorReleased was manufactured by drop or abort"
    );

    // The parent still retains both exact anchors: catastrophic
    // containment authority for both units remains available.
    assert_eq!(retained.len(), 2);
    assert!(!retained.release(&ProcessUnitId::new("unit-drop"), 99));
    assert!(!retained.release(&ProcessUnitId::new("unit-abort"), 99));

    dispatcher.shutdown().await;
}
