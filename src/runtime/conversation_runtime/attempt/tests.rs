use crate::runtime::conversation_runtime::test_support::*;

/// Cancelling an unknown attempt identity fails explicitly and never
/// cancels a different attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_unknown_attempt_fails_typed() {
    let fixture = headless_fixture().await;
    assert_eq!(
        fixture
            .runtime
            .cancel_current_attempt(&crate::runtime::identity::AttemptId::new("ghost")),
        Err(CancelAttemptError::NoCurrentAttempt)
    );
}

/// Issue #12 (M9b), cancellation wins before start — through the real
/// runtime control path. The attempt is parked immediately before the
/// model-turn start arbitration; `cancel_current_attempt` fully
/// completes while the execution is parked, so cancellation provably
/// linearized first: no provider request, no `ModelRequestStarted`, no
/// Request Snapshot, and no request-scoped context commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_before_model_turn_start_never_starts_the_request() {
    use crate::agent::execution::test_sync::StartBoundaryPause;
    let dir = tempfile::tempdir().expect("temp dir");
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let (runtime, model) = headless_runtime(
        &dir,
        vec![one_turn_script()],
        None,
        Some(CoordinatorProbe {
            start_boundary_pause: Some(pause),
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    runtime
        .submit_inbound(text_content("answer me"))
        .expect("accepted");

    // The attempt is parked immediately before the cancellation-vs-start
    // arbitration: preparation completed, nothing request-scoped
    // committed, no provider request exists.
    pre_start.await_park(1).await;
    let attempt = AttemptId::for_conversation(
        &ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
        0,
    );
    runtime
        .cancel_current_attempt(&attempt)
        .expect("the parked attempt is cancellable");
    pre_start.release();

    let observations = await_observation(
        pending.as_ref(),
        |o| matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event)),
    )
    .await;
    let terminal_count = observations
        .iter()
        .filter(|o| {
            matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event))
        })
        .count();
    assert_eq!(terminal_count, 1, "the attempt settles exactly once");
    assert!(
        observations.iter().any(|o| matches!(
            o,
            ConversationObservation::Event { event, .. }
                if matches!(event, crate::events::types::RuntimeEvent::AttemptCancelled { .. })
        )),
        "the attempt settles cancelled"
    );

    let store = runtime.tool_runtime().durable_store();
    assert!(
        model.requests().is_empty(),
        "the provider request never started"
    );
    assert!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .is_empty(),
        "no started Request Snapshot exists"
    );
    let journal = store.read_events(None, 128).expect("journal").events;
    assert!(
        !journal.iter().any(|envelope| matches!(
            envelope.event,
            crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
        )),
        "no request-start fact exists"
    );
    assert!(
        matches!(
            journal.last().map(|envelope| &envelope.event),
            Some(crate::events::types::RuntimeEvent::AttemptCancelled { .. })
        ),
        "the cancellation terminal is unique and last"
    );
    let canonical = store.load_canonical().expect("canonical");
    assert_eq!(
        canonical.len(),
        1,
        "only the adopted inbound is canonical: no request-scoped context half-commit"
    );
    assert!(
        matches!(&canonical[0], MessageBlock::User(user) if user.kind == InboundKind::Message),
        "the one canonical message is the user's inbound"
    );
}

/// Issue #12 (M9b), start wins, then cancellation: the durable start
/// commit linearizes first, the provider request is in flight, and the
/// later cancellation settles exactly that started request — it can
/// never be reclassified as never-started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn started_request_then_cancellation_settles_the_in_flight_request() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = vec![
        FakeStep::Emit(crate::model::event::ModelEvent::Started),
        FakeStep::Emit(crate::model::event::ModelEvent::TextDelta {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            text: "partial".to_owned(),
        }),
        FakeStep::ParkUntilCancelled,
    ];
    let (runtime, model) = headless_runtime(&dir, vec![script], None, None).await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    runtime
        .submit_inbound(text_content("answer me"))
        .expect("accepted");

    // The provider request is in flight and parked. The durable start
    // commit therefore already won — the provider is only invoked after
    // a successful start commit.
    let mut parked = model.parked();
    parked
        .wait_for(|is_parked| *is_parked)
        .await
        .expect("parked channel stays open");
    let store = runtime.tool_runtime().durable_store();
    assert_eq!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .len(),
        1,
        "exactly one started Request Snapshot exists before cancellation"
    );
    assert_eq!(
        store
            .read_events(None, 128)
            .expect("journal")
            .events
            .iter()
            .filter(|envelope| matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
            ))
            .count(),
        1,
        "exactly one request-start fact exists before cancellation"
    );

    let attempt = AttemptId::for_conversation(
        &ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
        0,
    );
    runtime
        .cancel_current_attempt(&attempt)
        .expect("the in-flight attempt is cancellable");

    let observations = await_observation(
        pending.as_ref(),
        |o| matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event)),
    )
    .await;
    assert!(
        observations.iter().any(|o| matches!(
            o,
            ConversationObservation::Event { event, .. }
                if matches!(event, crate::events::types::RuntimeEvent::AttemptCancelled { .. })
        )),
        "the in-flight request settles the attempt as cancelled"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no second provider request exists"
    );
    // The terminal event is unique and last; the in-flight request it
    // settles durably started, and the partial streamed content never
    // became a canonical Assistant message.
    let journal = store.read_events(None, 128).expect("journal").events;
    assert!(
        matches!(
            journal.last().map(|envelope| &envelope.event),
            Some(crate::events::types::RuntimeEvent::AttemptCancelled { .. })
        ),
        "the cancellation terminal is unique and last"
    );
    let canonical = store.load_canonical().expect("canonical");
    assert!(
        !canonical
            .iter()
            .any(|message| matches!(message, MessageBlock::Assistant(_))),
        "the cancelled in-flight request commits no assistant message"
    );
}

#[cfg(target_os = "linux")]
mod workspace_changes;
