use super::*;
use crate::context::asynchronous::{
    CompactionCoordinator, CompactionDelivery, CompactionDisposition,
};
use crate::context::compaction::{CompactionAttribution, capture_compaction};
use crate::context::engine::CompactionConstraints;
use crate::runtime::conversation_runtime::test_support::*;

mod cancellation;
mod idle_fit;
mod supervision;

fn permission_reminder_ids(
    request: &crate::model::ModelRequest,
    mode: crate::runtime::ApprovalMode,
) -> Vec<crate::runtime::identity::MessageId> {
    let content = crate::context::assembly::permission_reminder_content(
        &crate::runtime::permission::permission_reminder_text(mode),
    );
    request
        .messages
        .iter()
        .filter_map(|message| match message.as_canonical() {
            Some(MessageBlock::User(user))
                if user.source == crate::message::types::UserSource::Runtime
                    && user.kind
                        == crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::NativeEnvironment,
                        )
                    && user.content == content =>
            {
                Some(user.id.clone())
            }
            _ => None,
        })
        .collect()
}

async fn start_background(runtime: &ConversationRuntime) -> CompactionAdmission {
    let (admission, operation, captured) =
        capture_background(runtime, &CompactionConstraints::default()).await;
    RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner))
        .start(admission.clone(), captured.unwrap(), operation)
        .unwrap();
    admission
}

async fn capture_background(
    runtime: &ConversationRuntime,
    constraints: &CompactionConstraints<'_>,
) -> (
    CompactionAdmission,
    Box<dyn crate::context::summarizer::SummaryOperation>,
    Result<CapturedCompaction, CompactionExecutionError>,
) {
    let (context, revision, tools, prompt) = {
        let mut state = runtime.inner.lock_state();
        let model = state.model.snapshot().unwrap();
        let resources = state.resources.clone();
        let context = runtime
            .inner
            .context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                runtime.inner.model_timeout_policy,
                &resources,
                Arc::new(runtime.inner.auxiliary.clone()),
            )
            .unwrap()
            .with_runtime_resources(&resources);
        let sections = resources
            .context_assembly()
            .system_sections(&resources.native_system_input())
            .unwrap();
        (
            context,
            state
                .conversation
                .hydrate(runtime.inner.store.as_ref())
                .unwrap()
                .unwrap()
                .revision(),
            resources.capability().tool_registry().model_definitions(),
            render_effective_system_prompt(&sections),
        )
    };
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let admission = coordinator
        .admit(&AttemptId::new("trigger-attempt"))
        .unwrap()
        .unwrap();
    let operation = context
        .summarizer
        .operation(
            crate::context::compaction_budget::CompactionBudget::asynchronous(
                admission.operation_id.clone(),
                runtime.inner.monotonic_clock.clone(),
            ),
            admission.cancellation.child(),
        )
        .unwrap();
    let captured = capture_compaction(
        &context,
        revision,
        &runtime.inner.conversation_id,
        runtime.inner.store.as_ref(),
        &tools,
        None,
        &prompt,
        constraints,
        &admission.cancellation,
        CompactionAttribution {
            operation_id: admission.operation_id.clone(),
            attempt_id: None,
            turn_id: None,
        },
        operation.as_ref(),
    )
    .await;
    (admission, operation, captured)
}

/// A background result which wins an authorized recovery race closes the
/// exact competing blocking operation before its own checkpoint is committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One ordered race proves loser cleanup before the winner's commit.
async fn ready_background_cancels_and_joins_competing_blocking_scope() {
    use crate::model::event::ModelEvent;
    use crate::model::types::ModelProtocol;
    use crate::scripted_suites::support::model::{
        FixtureModel, ScriptedAdapterFactory, fixture_session_model,
    };

    let directory = tempfile::Builder::new()
        .prefix(&"recovery-native-owner-".repeat(6))
        .tempdir()
        .unwrap();
    let (release_background, background_gate) = model_release();
    let mut background = summary_turn_script("covered old context");
    background.insert(1, FakeStep::ParkUntilReleased(background_gate));
    let blocking = vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilCancelled,
    ];
    let mut first = runtime_inbound_message("old-one");
    first.content = text_content(&"old ".repeat(1_000));
    let mut second = runtime_inbound_message("old-two");
    second.content = text_content(&"old ".repeat(1_000));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![background, blocking, one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    runtime.inner.lock_state().model = fixture_session_model(
        &[
            FixtureModel::text("scripted/scripted", ModelProtocol::OpenAiChatCompletions)
                .with_context_window(4_096)
                .with_max_output_tokens(512),
        ],
        "scripted/scripted",
        &ScriptedAdapterFactory::new(adapter),
    );
    let mode = runtime.inner.lock_state().effective_approval_mode;
    runtime.inner.lock_state().last_permission_reminder_mode = Some(mode);
    runtime.activate();
    let background = start_background(&runtime).await;
    let work = runtime.inner.async_work(&background).unwrap();
    work.state.lock().unwrap().force_blocking_competition = true;
    let mut parks = model.parks();
    within_liveness_guard("background Summary park", async {
        parks.wait_for(|count| *count >= 1).await.unwrap();
    })
    .await;
    // Leave headroom for the long native cwd and re-armed permission reminder.
    // The complete old request must still exceed admission while the captured
    // Summary replacement fits. The real blocking start and background commit
    // below independently prove both inequalities.
    let fresh_text = "new ".repeat(500);
    runtime.submit_inbound(text_content(&fresh_text)).unwrap();
    // The admitted blocking scope may still be queued by Auxiliary capacity;
    // its ownership, cancellation and physical join must precede activation.
    within_liveness_guard("competing blocking Summary start", async {
        model
            .streams_started()
            .wait_for(|count| *count >= 2)
            .await
            .unwrap();
    })
    .await;
    release_background.send(true).unwrap();
    let _ = within_liveness_guard("recovered Attempt settlement", async {
        let settled = runtime.child_attempt_settlement(0);
        tokio::pin!(settled);
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                assert!(
                    state.status.diagnostic.is_none(),
                    "the background fixture must fit current admission: {:?}",
                    state.status
                );
            }
            tokio::select! {
                ledger = &mut settled => break ledger,
                () = changed => {}
            }
        }
    })
    .await;
    assert!(
        model.streams_exited_count() >= 2,
        "both Summary scopes must settle"
    );
    let events = runtime.inner.store.read_events(None, 256).unwrap().events;
    let blocking_id = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::CompactionStarted {
                operation_id,
                mode: crate::context::operation::CompactionMode::Blocking,
            } => Some(operation_id.clone()),
            _ => None,
        })
        .expect("blocking competitor started");
    assert_ne!(blocking_id, background.operation_id);
    let cancelled = events
        .iter()
        .position(|event| {
            matches!(&event.event,
                RuntimeEvent::CompactionCancelled { operation_id } if *operation_id == blocking_id
            )
        })
        .unwrap_or_else(|| {
            let status = work.state.lock().unwrap().status.clone();
            let facts: Vec<_> = events.iter().map(|event| &event.event).collect();
            panic!(
                "blocking competitor {blocking_id} did not settle cancelled; background status: {status:?}; durability failure: {:?}; journal: {facts:#?}",
                runtime.inner.durability_failure_diagnostic(),
            )
        });
    let committed = events.iter().position(|event| matches!(&event.event,
        RuntimeEvent::CompactionCompleted { operation_id, .. } if *operation_id == background.operation_id
    )).expect("background operation committed");
    assert!(
        cancelled < committed,
        "blocking scope settlement precedes background installation"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.event, RuntimeEvent::CompactionCompleted { .. }))
            .count(),
        1
    );
    let requests = model.requests();
    let reminder_ids = permission_reminder_ids(&requests[2], mode);
    assert_eq!(
        reminder_ids.len(),
        1,
        "the winning background checkpoint stages one canonical native reminder"
    );
    assert!(
        !requests[2]
            .effective_system_prompt
            .contains("Execution permission mode:")
    );
    let snapshots = runtime
        .inner
        .store
        .read_request_snapshots(None, 8)
        .unwrap()
        .snapshots;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0]
            .contributions
            .iter()
            .filter(|contribution| contribution.producer
                == crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::PermissionReminder,
                ))
            .map(|contribution| contribution.message_id.clone())
            .collect::<Vec<_>>(),
        reminder_ids
    );
    assert!(
        reminder_ids
            .iter()
            .all(|id| snapshots[0].request_context_ids.contains(id))
    );
    assert_eq!(
        requests.len(),
        3,
        "background Summary, blocking Summary, then primary request"
    );
    assert_eq!(
        serde_json::to_string(&requests[2].messages)
            .unwrap()
            .matches(fresh_text.as_str())
            .count(),
        1,
        "the accepted inbound tail remains in the repaired primary request exactly once"
    );
    let history = runtime.inner.store.projected_history().unwrap();
    assert_eq!(
        serde_json::to_string(&history.messages)
            .unwrap()
            .matches(fresh_text.as_str())
            .count(),
        1,
        "the post-capture canonical tail remains selected exactly once"
    );
    runtime.shutdown().await.unwrap();
}

/// A bounded background prefix cannot delay mandatory recovery when even
/// the smallest valid Summary leaves the complete current input too large.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mandatory_join_rejects_provably_insufficient_frozen_scope() {
    use crate::model::event::ModelEvent;
    use crate::model::types::ModelProtocol;
    use crate::scripted_suites::support::model::{
        FixtureModel, ScriptedAdapterFactory, fixture_session_model,
    };

    let directory = tempfile::tempdir().unwrap();
    let mut first = runtime_inbound_message("old-one");
    first.content = text_content(&"old ".repeat(1_000));
    let mut second = runtime_inbound_message("old-two");
    second.content = text_content(&"old ".repeat(1_000));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilCancelled,
        ]],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    runtime.inner.lock_state().model = fixture_session_model(
        &[
            FixtureModel::text("scripted/scripted", ModelProtocol::OpenAiChatCompletions)
                .with_context_window(4_096)
                .with_max_output_tokens(512),
        ],
        "scripted/scripted",
        &ScriptedAdapterFactory::new(adapter),
    );
    runtime.activate();
    let admission = start_background(&runtime).await;
    let mut parked = model.parked();
    within_liveness_guard("held Summary", async {
        parked.wait_for(|value| *value).await.unwrap();
    })
    .await;
    let (context, request) = {
        let state = runtime.inner.lock_state();
        let model = state.model.snapshot().unwrap();
        let resources = state.resources.clone();
        let context = runtime
            .inner
            .context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                runtime.inner.model_timeout_policy,
                &resources,
                Arc::new(runtime.inner.auxiliary.clone()),
            )
            .unwrap();
        let request = crate::model::ModelRequest {
            prompt_fallback_authorization:
                crate::model::types::PromptFallbackAuthorization::default(),
            response_schema: None,
            response_byte_limit: None,
            images: BTreeMap::default(),
            invocation: model.primary().invocation_config(),
            messages: crate::model::SelectedModelInput::default(),
            tools: resources.capability().tool_registry().model_definitions(),
            effective_system_prompt: String::new(),
            continuation: None,
            continuation_source: None,
        };
        (context, request)
    };
    let mut huge = runtime_inbound_message("fresh-large");
    huge.content = text_content(&"fresh ".repeat(10_000));
    let staged = vec![MessageBlock::User(huge)];
    let constraints = CompactionConstraints {
        staged_request_context: &staged,
        ..CompactionConstraints::default()
    };
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    assert!(
        !coordinator
            .can_join(&admission, &context, &request, &constraints)
            .unwrap()
    );
    assert!(
        !admission.cancellation.is_cancelled(),
        "infeasible waiting must not cancel independent work"
    );
    runtime.shutdown().await.unwrap();
    assert_eq!(model.streams_exited_count(), 1);
}

/// A result arriving during a started primary request waits for a later
/// boundary and cannot alter the already frozen request or its source head.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One scenario spans publication, the active primary and idle installation.
async fn late_ready_does_not_rewrite_the_running_primary_request() {
    let directory = tempfile::tempdir().unwrap();
    let (release_background, background_gate) = model_release();
    let (release_primary, primary_gate) = model_release();
    let mut background = summary_turn_script("historical fact");
    background.insert(1, FakeStep::ParkUntilReleased(background_gate));
    let mut primary = one_turn_script();
    primary.insert(1, FakeStep::ParkUntilReleased(primary_gate));
    let mut first = runtime_inbound_message("frozen-old-one");
    first.content = text_content(&"old context ".repeat(512));
    let mut second = runtime_inbound_message("frozen-old-two");
    second.content = text_content(&"retained context ".repeat(512));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![background, primary, one_turn_script(), one_turn_script()],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    let mode = runtime.inner.lock_state().effective_approval_mode;
    runtime.inner.lock_state().last_permission_reminder_mode = Some(mode);
    runtime.activate();
    let admission = start_background(&runtime).await;
    let mut parks = model.parks();
    within_liveness_guard("background parked", async {
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    runtime
        .submit_inbound(text_content(
            "fresh input must remain in the primary request",
        ))
        .unwrap();
    within_liveness_guard("primary parked", async {
        model
            .streams_started()
            .wait_for(|count| *count >= 2)
            .await
            .unwrap();
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    let frozen_primary = model.requests()[1].clone();
    assert!(
        !frozen_primary
            .effective_system_prompt
            .contains("Execution permission mode:")
    );
    let work = runtime.inner.async_work(&admission).unwrap();
    let (publication_entered, publication_wait) = tokio::sync::oneshot::channel();
    let (release_publication, publication_release) = std::sync::mpsc::channel();
    *work.ready_publication_gate.lock().unwrap() = Some(ReadyPublicationGate {
        entered: publication_entered,
        release: publication_release,
    });
    release_background.send(true).unwrap();
    within_liveness_guard("Ready status publication boundary", publication_wait)
        .await
        .unwrap();
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let waiting_attempt = AttemptId::new("ready-publication-waiter");
    let waiter_cancel = CancellationSignal::new();
    let mut waiting = Box::pin(coordinator.wait(&admission, &waiting_attempt, &waiter_cancel));
    tokio::select! {
        biased;
        _ = &mut waiting => panic!("unpublished Ready cannot satisfy a foreground waiter"),
        () = tokio::task::yield_now() => {}
    }
    assert_eq!(
        work.state.lock().unwrap().status.phase,
        CompactionOperationPhase::Ready
    );
    waiter_cancel.cancel();
    assert!(waiting.await.is_none());
    let weak = Arc::downgrade(&runtime.inner);
    let selected = tokio::task::spawn_blocking(move || {
        RuntimeCompactionCoordinator(weak).take_ready().is_some()
    });
    assert!(
        !tokio::time::timeout(std::time::Duration::from_secs(3), selected)
            .await
            .expect("selection must not wait for the status writer")
            .unwrap(),
        "unpublished Ready cannot become installation authority"
    );
    assert!(runtime.inner.store.projection_head().unwrap().is_none());
    release_publication.send(()).unwrap();
    within_liveness_guard("background ready while primary is running", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if matches!(state.phase, Phase::Ready(_))
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
                assert!(!matches!(state.phase, Phase::Failed(_) | Phase::Terminal));
            }
            changed.await;
        }
    })
    .await;
    assert!(runtime.inner.store.projection_head().unwrap().is_none());
    assert_eq!(model.requests()[1], frozen_primary);
    release_primary.send(true).unwrap();
    let _ = within_liveness_guard("primary settlement", runtime.child_attempt_settlement(0)).await;
    within_liveness_guard("idle installation", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if work.state.lock().unwrap().status.phase == CompactionOperationPhase::Committed {
                break;
            }
            changed.await;
        }
    })
    .await;
    assert_eq!(
        model.requests().len(),
        2,
        "no primary request was restarted"
    );
    assert_eq!(model.requests()[1], frozen_primary);
    let checkpoint = runtime.inner.store.projection_head().unwrap().unwrap();
    let events = runtime.inner.store.read_events(None, 256).unwrap().events;
    let started = events
        .iter()
        .position(|event| matches!(&event.event, RuntimeEvent::ModelRequestStarted { .. }))
        .unwrap();
    let committed = events.iter().position(|event| matches!(&event.event,
        RuntimeEvent::CompactionCompleted { operation_id, .. } if *operation_id == admission.operation_id)).unwrap();
    assert!(started < committed);
    assert!(checkpoint.candidate.summary_generation.is_some());
    // The actual idle commit re-arms the reminder after the older frozen
    // primary has settled. Only the next started primary consumes it.
    assert_eq!(
        runtime.inner.lock_state().last_permission_reminder_mode,
        None
    );
    runtime
        .submit_inbound(text_content("after idle activation"))
        .unwrap();
    let _ = within_liveness_guard(
        "post-activation primary",
        runtime.child_attempt_settlement(1),
    )
    .await;
    let after_activation = model.requests()[2].clone();
    let reminder_ids = permission_reminder_ids(&after_activation, mode);
    assert_eq!(reminder_ids.len(), 1);
    assert_eq!(
        after_activation.effective_system_prompt,
        frozen_primary.effective_system_prompt
    );
    assert_eq!(
        runtime.inner.lock_state().last_permission_reminder_mode,
        Some(mode)
    );
    runtime
        .submit_inbound(text_content("after reminder consumption"))
        .unwrap();
    let _ = within_liveness_guard("subsequent primary", runtime.child_attempt_settlement(2)).await;
    let subsequent = model.requests()[3].clone();
    assert_eq!(
        subsequent.effective_system_prompt,
        frozen_primary.effective_system_prompt
    );
    assert_eq!(
        permission_reminder_ids(&subsequent, mode),
        reminder_ids,
        "the retained canonical reminder is reused as history without another emission"
    );
    let snapshots = runtime
        .inner
        .store
        .read_request_snapshots(None, 8)
        .unwrap()
        .snapshots;
    assert_eq!(snapshots.len(), 3);
    let reminder_owner = crate::runtime::identity::ContextContributorIdentity::Native(
        crate::runtime::identity::NativeContextContributor::PermissionReminder,
    );
    assert_eq!(
        snapshots[1]
            .contributions
            .iter()
            .filter(|contribution| contribution.producer == reminder_owner)
            .map(|contribution| contribution.message_id.clone())
            .collect::<Vec<_>>(),
        reminder_ids
    );
    assert!(
        snapshots[2]
            .contributions
            .iter()
            .all(|contribution| contribution.producer != reminder_owner),
        "ordinary start does not stage a second reminder"
    );
    runtime.shutdown().await.unwrap();
}

/// Ready completion between final preparation and the primary start gate
/// cannot change the frozen Snapshot's historical Head.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_ready_before_start_preserves_the_prepared_snapshot() {
    use crate::agent::execution::test_sync::StartBoundaryPause;

    let directory = tempfile::tempdir().unwrap();
    let (release_background, background_gate) = model_release();
    let mut background = summary_turn_script("historical fact");
    background.insert(1, FakeStep::ParkUntilReleased(background_gate));
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.unwrap();
    let mut first = runtime_inbound_message("prepared-old-one");
    first.content = text_content(&"old context ".repeat(512));
    let mut second = runtime_inbound_message("prepared-old-two");
    second.content = text_content(&"retained context ".repeat(512));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![background, one_turn_script()],
        None,
        Some(CoordinatorProbe {
            start_boundary_pause: Some(pause),
            ..CoordinatorProbe::default()
        }),
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    let admission = start_background(&runtime).await;
    let mut parks = model.parks();
    within_liveness_guard("background parked", async {
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    runtime
        .submit_inbound(text_content("prepared fresh input"))
        .unwrap();
    within_liveness_guard("prepared primary start boundary", pre_start.await_park(1)).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    release_background.send(true).unwrap();
    within_liveness_guard("ready before primary start", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if matches!(state.phase, Phase::Ready(_))
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
                assert!(!matches!(state.phase, Phase::Failed(_) | Phase::Terminal));
            }
            changed.await;
        }
    })
    .await;
    assert!(runtime.inner.store.projection_head().unwrap().is_none());
    assert_eq!(
        model.requests().len(),
        1,
        "primary provider has not started"
    );
    pre_start.release();
    let _ = within_liveness_guard("primary settlement", runtime.child_attempt_settlement(0)).await;
    let snapshots = runtime
        .inner
        .store
        .read_request_snapshots(None, 32)
        .unwrap()
        .snapshots;
    assert_eq!(snapshots.len(), 1);
    assert!(
        snapshots[0].history_cut.checkpoint_id.is_none(),
        "the already prepared request retains its original head"
    );
    assert_eq!(model.requests().len(), 2);
    runtime.shutdown().await.unwrap();
}

/// A required Ready-status write failure closes the live Attempt's next-start
/// gate through the real runtime drain, without fabricating a checkpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ready_status_write_failure_drains_a_running_primary() {
    use crate::durable::{CanonicalStore, JournalStore, ProjectionStore};
    use crate::model::event::ModelEvent;

    let directory = tempfile::tempdir().unwrap();
    let conversation_id = ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(conversation_id.clone()).unwrap(),
    );
    let mut first = runtime_inbound_message("failure-old-one");
    first.content = text_content(&"old context ".repeat(512));
    let mut second = runtime_inbound_message("failure-old-two");
    second.content = text_content(&"retained context ".repeat(512));
    store.initialize(&[]).unwrap();
    store.append_canonical(&MessageBlock::User(first)).unwrap();
    store.append_canonical(&MessageBlock::User(second)).unwrap();
    let (release_background, background_gate) = model_release();
    let mut background = summary_turn_script("old fact");
    background.insert(1, FakeStep::ParkUntilReleased(background_gate));
    let primary = vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilCancelled,
    ];
    let (runtime, model) = headless_runtime_over_store_with(
        &directory,
        conversation_id.as_str(),
        store.clone(),
        vec![background, primary],
        None,
    )
    .await;
    runtime.activate();
    let admission = start_background(&runtime).await;
    let mut parks = model.parks();
    within_liveness_guard("background parked", async {
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    runtime
        .submit_inbound(text_content("primary is already in flight"))
        .unwrap();
    within_liveness_guard("primary parked", async {
        model
            .streams_started()
            .wait_for(|count| *count >= 2)
            .await
            .unwrap();
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    let work = runtime.inner.async_work(&admission).unwrap();
    let (entered, wait_entered) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    *work.ready_publication_gate.lock().unwrap() = Some(ReadyPublicationGate {
        entered,
        release: wait_release,
    });
    release_background.send(true).unwrap();
    within_liveness_guard("Ready write boundary", wait_entered)
        .await
        .unwrap();
    store.arm_fail_event_times(1);
    release.send(()).unwrap();
    within_liveness_guard("automatic failure fence", async {
        loop {
            let changed = runtime.inner.lifecycle.changed();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if runtime.inner.durability_gate.admission_failure().is_some()
                && runtime.inner.lifecycle.state() == ConversationLifecycleState::Draining
            {
                break;
            }
            changed.await;
        }
    })
    .await;
    let shutdown = within_liveness_guard("durability-failure drain", runtime.shutdown()).await;
    assert!(matches!(
        shutdown,
        Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { .. })
    ));
    assert!(runtime.inner.durability_failure_diagnostic().is_some());
    assert!(!runtime.has_current_attempt());
    assert_eq!(model.streams_exited_count(), 2);
    assert_eq!(
        model.requests().len(),
        2,
        "no later primary or Summary start may be granted"
    );
    assert!(store.projection_head().unwrap().is_none());
    let events = store.read_events(None, 256).unwrap().events;
    assert!(!events.iter().any(|event| matches!(&event.event,
        RuntimeEvent::CompactionCompleted { operation_id, .. } if *operation_id == admission.operation_id
    )));
}

/// A completed and settled Summary has no compaction-age expiry; fresh
/// installation authority remains necessary after its Auxiliary scope expires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Keep capture, clock expiry and fresh installation in one proof.
async fn accepted_ready_result_can_install_after_original_scope_deadline() {
    use crate::context::compaction::{
        ActivationOutcome, CurrentInstallation, activate_generated, generate_compaction,
    };
    use crate::runtime::MonotonicClock;
    use crate::scripted_suites::support::context::{FakeContextSummarizer, FakeSummaryStep};

    let directory = tempfile::tempdir().unwrap();
    let mut first = runtime_inbound_message("ttl-old-one");
    first.content = text_content(&"historical detail ".repeat(512));
    let mut second = runtime_inbound_message("ttl-old-two");
    second.content = text_content(&"more historical detail ".repeat(512));
    let (runtime, _) = headless_runtime_with_options(
        &directory,
        Vec::new(),
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let (mut context, model, tools, prompt, mut conversation) = {
        let mut state = runtime.inner.lock_state();
        let model = state.model.snapshot().unwrap();
        let resources = state.resources.clone();
        let context = runtime
            .inner
            .context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                runtime.inner.model_timeout_policy,
                &resources,
                Arc::new(runtime.inner.auxiliary.clone()),
            )
            .unwrap();
        let sections = resources
            .context_assembly()
            .system_sections(&resources.native_system_input())
            .unwrap();
        (
            context,
            model,
            resources.capability().tool_registry().model_definitions(),
            render_effective_system_prompt(&sections),
            {
                state
                    .conversation
                    .hydrate(runtime.inner.store.as_ref())
                    .unwrap();
                state.conversation.lend().unwrap()
            },
        )
    };
    context.summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "small accepted summary".into(),
    )]));
    context.monotonic_clock = clock.clone();
    let operation_id = crate::runtime::identity::CompactionOperationId::new("ready-ttl-operation");
    runtime
        .inner
        .publish_compaction_admission(CompactionOperationStatus {
            operation_id: operation_id.clone(),
            admission_generation: 1,
            triggering_attempt_id: None,
            waiting_attempt_id: None,
            phase: CompactionOperationPhase::Generating,
            accepted_fragments: 0,
            diagnostic: None,
        })
        .unwrap();
    let cancellation = CancellationSignal::new();
    let budget = crate::context::compaction_budget::CompactionBudget::asynchronous(
        operation_id.clone(),
        clock.clone(),
    );
    let deadline = budget.deadline_millis();
    let operation = context
        .summarizer
        .operation(budget.clone(), cancellation.child())
        .unwrap();
    let captured = capture_compaction(
        &context,
        conversation.revision(),
        &runtime.inner.conversation_id,
        runtime.inner.store.as_ref(),
        &tools,
        None,
        &prompt,
        &CompactionConstraints::default(),
        &cancellation,
        CompactionAttribution {
            operation_id: operation_id.clone(),
            attempt_id: None,
            turn_id: None,
        },
        operation.as_ref(),
    )
    .await
    .unwrap();
    let generated = generate_compaction(
        captured,
        runtime.inner.store.as_ref(),
        operation.as_ref(),
        &cancellation,
        &|_| Ok(()),
        &|| Ok(()),
    )
    .await
    .unwrap();
    operation.close().await;
    clock.advance(deadline.saturating_add(1));
    assert!(clock.now_millis() > deadline);
    assert_eq!(
        budget.check(&cancellation).unwrap_err().kind,
        ContextErrorKind::AuxiliaryDeadlineExceeded
    );
    let history = runtime.inner.store.projected_history().unwrap();
    let request = crate::model::ModelRequest {
        prompt_fallback_authorization: crate::model::types::PromptFallbackAuthorization::default(),
        response_schema: None,
        response_byte_limit: None,
        images: BTreeMap::default(),
        invocation: model.primary().invocation_config(),
        messages: history.messages,
        tools,
        effective_system_prompt: prompt,
        continuation: None,
        continuation_source: None,
    };
    let constraints = CompactionConstraints::default();
    let result = activate_generated(
        &generated,
        &mut conversation,
        runtime.inner.store.as_ref(),
        &cancellation,
        &CurrentInstallation {
            context: &context,
            request: &request,
            constraints: &constraints,
            attempt_id: None,
            turn_id: None,
            installer_cancellation: None,
        },
    )
    .unwrap();
    assert!(matches!(result, ActivationOutcome::Committed(_)));
    runtime
        .inner
        .lock_state()
        .conversation
        .restore(conversation);
    runtime
        .inner
        .publish_compaction_status(CompactionOperationStatus {
            operation_id,
            admission_generation: 1,
            triggering_attempt_id: None,
            waiting_attempt_id: None,
            phase: CompactionOperationPhase::Committed,
            accepted_fragments: 0,
            diagnostic: None,
        })
        .unwrap();
    runtime.shutdown().await.unwrap();
}

/// Retry status follows a real accepted Auxiliary retry, and remains attached
/// to the same independently owned Compaction operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn physical_summary_retry_publishes_the_same_operation_identity() {
    use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
    use crate::model::event::ModelEvent;
    let directory = tempfile::tempdir().unwrap();
    let mut first = runtime_inbound_message("retry-old-one");
    first.content = text_content(&"old detail ".repeat(512));
    let mut second = runtime_inbound_message("retry-old-two");
    second.content = text_content(&"more old detail ".repeat(512));
    let failure = ModelError {
        kind: ModelErrorKind::Transport,
        message: "fixture transient failure".into(),
        retry_disposition: ModelRetryDisposition::Transient,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    };
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::Failed { error: failure }),
            ],
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::ParkUntilCancelled,
            ],
        ],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    let admission = start_background(&runtime).await;
    let mut parks = model.parks();
    within_liveness_guard("retried physical Summary", async {
        parks.wait_for(|count| *count >= 1).await.unwrap();
    })
    .await;
    let events = runtime.inner.store.read_events(None, 256).unwrap().events;
    assert!(events.iter().any(|event| matches!(&event.event,
        RuntimeEvent::CompactionOperationUpdated { status }
            if status.operation_id == admission.operation_id && status.phase == CompactionOperationPhase::Retrying
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, RuntimeEvent::AuxiliaryRetryScheduled { .. }))
    );
    runtime.shutdown().await.unwrap();
}

/// Exact operation cancellation stops independent Summary work without
/// forwarding cancellation to a simultaneous primary Attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_cancel_targets_only_the_named_independent_operation() {
    use crate::model::event::ModelEvent;
    let directory = tempfile::tempdir().unwrap();
    let (release_primary, primary_gate) = model_release();
    let mut primary = one_turn_script();
    primary.insert(1, FakeStep::ParkUntilReleased(primary_gate));
    let mut first = runtime_inbound_message("cancel-old-one");
    first.content = text_content(&"old context ".repeat(512));
    let mut second = runtime_inbound_message("cancel-old-two");
    second.content = text_content(&"retained context ".repeat(512));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::ParkUntilCancelled,
            ],
            primary,
        ],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    let admission = start_background(&runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    let mut parks = model.parks();
    within_liveness_guard("background parked", async {
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    runtime
        .submit_inbound(text_content("keep the primary running"))
        .unwrap();
    within_liveness_guard("primary parked", async {
        model
            .streams_started()
            .wait_for(|count| *count >= 2)
            .await
            .unwrap();
        parks.wait_for(|value| *value >= 1).await.unwrap();
    })
    .await;
    assert!(
        !runtime
            .cancel_compaction(&crate::runtime::identity::CompactionOperationId::new(
                "stale-operation"
            ))
            .unwrap()
    );
    assert!(!admission.cancellation.is_cancelled());
    assert!(runtime.cancel_compaction(&admission.operation_id).unwrap());
    within_liveness_guard("independent cancellation settlement", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if state.status.phase == CompactionOperationPhase::Cancelled
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
            }
            changed.await;
        }
    })
    .await;
    assert!(runtime.has_current_attempt());
    assert!(
        !runtime
            .inner
            .lock_state()
            .current_attempt
            .as_ref()
            .unwrap()
            .cancellation
            .is_cancelled()
    );
    assert!(!runtime.cancel_compaction(&admission.operation_id).unwrap());
    assert_eq!(model.requests().len(), 2);
    release_primary.send(true).unwrap();
    let _ = within_liveness_guard("primary settlement", runtime.child_attempt_settlement(0)).await;
    assert!(runtime.inner.store.projection_head().unwrap().is_none());
    runtime.shutdown().await.unwrap();
}

/// Cancellation after a reaper starts joining must still reach the Journal
/// before that exact operation can be retired from the native registry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_during_reap_join_publishes_terminal_before_retirement() {
    let directory = tempfile::tempdir().unwrap();
    let (release_summary, summary_gate) = model_release();
    let mut script = summary_turn_script("retained historical fact");
    script.insert(1, FakeStep::ParkUntilReleased(summary_gate));
    let mut first = runtime_inbound_message("reap-first");
    first.content = text_content(&"historical source ".repeat(512));
    let mut second = runtime_inbound_message("reap-second");
    second.content = text_content(&"retained source ".repeat(512));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![script],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    // Open native admission without starting the automatic coordinator: this
    // test owns the one reaper and controls its physical join directly.
    assert!(runtime.inner.lifecycle.activate());
    let admission = start_background(&runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    within_liveness_guard("Summary park", async {
        model.parks().wait_for(|count| *count >= 1).await.unwrap();
    })
    .await;
    let (entered, published) = tokio::sync::oneshot::channel();
    let (release_publication, publication_release) = std::sync::mpsc::channel();
    *work.ready_publication_gate.lock().unwrap() = Some(ReadyPublicationGate {
        entered,
        release: publication_release,
    });
    release_summary.send(true).unwrap();
    within_liveness_guard("Ready publication pause", published)
        .await
        .unwrap();
    let reaped = runtime.inner.reap_async_compaction(true);
    tokio::pin!(reaped);
    tokio::select! {
        biased;
        () = &mut reaped => panic!("the parked worker cannot already be joined"),
        () = tokio::task::yield_now() => {}
    }
    assert!(
        work.join.lock().unwrap().is_none(),
        "the reaper claimed physical join ownership"
    );
    runtime.inner.cancel_async_compaction();
    release_publication.send(()).unwrap();
    within_liveness_guard("cancelled worker reaping", reaped).await;
    let cut = runtime.inner.store.compaction_resource_cut().unwrap();
    let statuses = runtime
        .inner
        .store
        .latest_compaction_operations(cut.journal.0)
        .unwrap();
    let status = statuses
        .iter()
        .find(|s| s.operation_id == admission.operation_id)
        .unwrap();
    assert_eq!(
        status.phase,
        CompactionOperationPhase::Cancelled,
        "physical join cannot retire an unpublished terminal cancellation"
    );
    assert!(
        runtime
            .inner
            .async_compaction
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proactive
            .is_none()
    );
    runtime.shutdown().await.unwrap();
}

/// A waiting Attempt may detach, and a selected result may be returned to the
/// same Session-owned operation. Shutdown still joins the real Summary call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One operation crosses configuration, Attempts, waiter handoff and drain.
async fn detached_waiter_preserves_ready_identity_and_shutdown_joins() {
    let directory = tempfile::tempdir().unwrap();
    let (release, receiver) = model_release();
    let mut summary = summary_turn_script("retained historical fact");
    summary.insert(1, FakeStep::ParkUntilReleased(receiver));
    let mut first = runtime_inbound_message("history-first");
    first.content = text_content(&"first historical material ".repeat(512));
    let mut second = runtime_inbound_message("history-second");
    second.content = text_content(&"second historical material ".repeat(512));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![
            summary,
            vec![
                FakeStep::Emit(crate::model::event::ModelEvent::Started),
                FakeStep::ParkUntilCancelled,
            ],
            one_turn_script(),
        ],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    let (context, revision, tools, prompt) = {
        let mut state = runtime.inner.lock_state();
        let model = state.model.snapshot().unwrap();
        let resources = state.resources.clone();
        let context = runtime
            .inner
            .context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                runtime.inner.model_timeout_policy,
                &resources,
                Arc::new(runtime.inner.auxiliary.clone()),
            )
            .unwrap()
            .with_runtime_resources(&resources);
        let sections = resources
            .context_assembly()
            .system_sections(&resources.native_system_input())
            .unwrap();
        (
            context,
            state
                .conversation
                .hydrate(runtime.inner.store.as_ref())
                .unwrap()
                .unwrap()
                .revision(),
            resources.capability().tool_registry().model_definitions(),
            render_effective_system_prompt(&sections),
        )
    };
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let trigger = AttemptId::for_conversation(&runtime.inner.conversation_id, 0);
    let admission = coordinator.admit(&trigger).unwrap().unwrap();
    let operation = context
        .summarizer
        .operation(
            crate::context::compaction_budget::CompactionBudget::asynchronous(
                admission.operation_id.clone(),
                runtime.inner.monotonic_clock.clone(),
            ),
            admission.cancellation.child(),
        )
        .unwrap();
    let captured = capture_compaction(
        &context,
        revision,
        &runtime.inner.conversation_id,
        runtime.inner.store.as_ref(),
        &tools,
        None,
        &prompt,
        &CompactionConstraints::default(),
        &admission.cancellation,
        CompactionAttribution {
            operation_id: admission.operation_id.clone(),
            attempt_id: None,
            turn_id: None,
        },
        operation.as_ref(),
    )
    .await
    .unwrap();
    coordinator
        .start(admission.clone(), captured, operation)
        .unwrap();
    let mut parked = model.parked();
    within_liveness_guard("background Summary park", async {
        parked.wait_for(|parked| *parked).await.unwrap();
    })
    .await;

    // The Summary has a frozen invocation and source cut, but it must not
    // block an automatically staged configuration. Its physical work still
    // counts for ordinary idle/residency and shutdown.
    let frozen_request = model.requests().into_iter().next().unwrap();
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let replacement = crate::scripted_suites::support::model::fixture_session_model(
        &[crate::scripted_suites::support::model::FixtureModel::text(
            "scripted/reconfigured",
            crate::model::types::ModelProtocol::OpenAiChatCompletions,
        )
        .with_max_output_tokens(512)
        .with_profiles(
            "focused",
            serde_json::json!({
                "focused": {"requestParams": {"temperature": 0.1}}
            }),
        )],
        "scripted/reconfigured",
        &crate::scripted_suites::support::model::ScriptedAdapterFactory::new(adapter),
    );
    let (prepared, baseline, prior_revision) = {
        let state = runtime.inner.lock_state();
        let config = crate::local_runtime::config::CurrentRuntimeConfig::from_toml_slice(
            b"agent_id='agent-a'\n[agent.model]\nmodel='scripted/reconfigured'\n",
        )
        .unwrap();
        let configuration = crate::runtime::resources::RuntimeConfiguration {
            config: Arc::new(config),
            models: replacement.registry().unwrap().clone(),
            resource_definitions: Vec::new(),
            resource_diagnostics: Vec::new(),
            authored: crate::local_runtime::configuration::generation::AuthoredConfiguration {
                component_revisions: std::collections::BTreeMap::new(),
                source_revisions: std::collections::BTreeMap::new(),
                effective:
                    crate::local_runtime::configuration::settings::SourceDocumentView::default(),
                provenance: std::collections::BTreeMap::new(),
            },
        };
        (
            crate::runtime::configuration_commit::PreparedConfiguration {
                owner: runtime.inner.configuration_owner.clone(),
                baseline: state.binding_revision,
                preview: Arc::new(state.resources.with_configuration(configuration)),
                model: replacement,
                capability: None,
                resources: None,
            },
            state.binding_revision,
            state.resources.revision(),
        )
    };
    assert!(runtime.idle_epoch().is_err());
    let (settled, outcome) = tokio::sync::oneshot::channel();
    runtime.stage_configuration(crate::runtime::configuration_commit::StagedConfiguration {
        prepared,
        desired: crate::runtime::configuration_commit::DesiredApplication::new(
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
            1,
        ),
        reason: crate::runtime::configuration_commit::ConfigurationChangeReason::Source,
        retain: Box::new(|_| {}),
        settled: Box::new(move |outcome| {
            Box::pin(async move {
                let _ = settled.send(outcome);
            })
        }),
    });
    assert_eq!(
        within_liveness_guard("automatic configuration commit", outcome)
            .await
            .unwrap(),
        crate::runtime::configuration_commit::StagedOutcome::Committed
    );
    assert_eq!(runtime.inner.lock_state().binding_revision, baseline + 1);
    assert_ne!(
        runtime.inner.lock_state().resources.revision(),
        prior_revision
    );
    {
        let state = runtime.inner.lock_state();
        let current = state.model.snapshot().unwrap();
        assert_ne!(
            current.primary().invocation_config().model,
            frozen_request.invocation.model
        );
        assert_eq!(
            current
                .primary()
                .profile()
                .map(crate::model::catalog::ModelProfileId::as_str),
            Some("focused")
        );
    }
    assert_eq!(
        model.requests()[0],
        frozen_request,
        "configuration commit cannot mutate the admitted Summary invocation"
    );

    // A real Attempt carrying the triggering identity is cancelled while the
    // independently owned Summary remains parked in its original scope.
    runtime
        .submit_inbound(text_content("continue while the Summary is running"))
        .unwrap();
    let mut parks = model.parks();
    within_liveness_guard("trigger Attempt model park", async {
        model
            .streams_started()
            .wait_for(|count| *count >= 2)
            .await
            .unwrap();
        parks.wait_for(|count| *count >= 1).await.unwrap();
    })
    .await;
    runtime.cancel_current_attempt(&trigger).unwrap();
    let _ = within_liveness_guard(
        "trigger Attempt settlement",
        runtime.child_attempt_settlement(0),
    )
    .await;
    assert!(!runtime.has_current_attempt());
    assert!(!admission.cancellation.is_cancelled());
    assert_eq!(
        coordinator.active().unwrap().operation_id,
        admission.operation_id
    );
    runtime
        .submit_inbound(text_content("a later Attempt completes"))
        .unwrap();
    let _ = within_liveness_guard(
        "later Attempt settlement",
        runtime.child_attempt_settlement(1),
    )
    .await;
    assert!(!admission.cancellation.is_cancelled());

    // Prevent idle maintenance from consuming Ready while the test controls
    // the selected-result handoff below.
    runtime.inner.lock_state().manual_compaction = Some(CurrentManualCompaction {
        operation_id: crate::runtime::identity::CompactionOperationId::new("test-maintenance"),
        cancellation: AgentCancellation::new(CancellationReason::RuntimeShutdown),
    });

    let waiting = AttemptId::new("waiting-attempt");
    let waiter_cancel = CancellationSignal::new();
    let waiter = coordinator.wait(&admission, &waiting, &waiter_cancel);
    tokio::pin!(waiter);
    tokio::select! {
        biased;
        _ = &mut waiter => panic!("held Summary cannot already be ready"),
        () = tokio::task::yield_now() => {}
    }
    waiter_cancel.cancel();
    assert!(waiter.await.is_none());
    assert!(!admission.cancellation.is_cancelled());
    assert_eq!(
        coordinator.active().unwrap().operation_id,
        admission.operation_id
    );

    // If the competing blocking operation finishes first, its selection
    // future is dropped. That drop must release only the waiting attribution.
    let other_attempt = AttemptId::new("blocking-winner");
    let other_cancel = CancellationSignal::new();
    let mut competing_wait = Box::pin(coordinator.wait(&admission, &other_attempt, &other_cancel));
    tokio::select! {
        biased;
        _ = &mut competing_wait => panic!("held Summary cannot already be ready"),
        () = tokio::task::yield_now() => {}
    }
    drop(competing_wait);
    let work = runtime.inner.async_work(&admission).unwrap();
    assert!(
        work.state
            .lock()
            .unwrap()
            .status
            .waiting_attempt_id
            .is_none()
    );
    assert!(!admission.cancellation.is_cancelled());

    release.send(true).unwrap();
    within_liveness_guard("independent Summary ready", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if matches!(state.phase, Phase::Ready(_))
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
                assert!(!matches!(state.phase, Phase::Failed(_) | Phase::Terminal));
            }
            changed.await;
        }
    })
    .await;
    let Some(CompactionDelivery::Ready(first)) = coordinator.take_ready() else {
        panic!("ready result")
    };
    let identity = Arc::as_ptr(&first.generated);
    coordinator
        .settle(&first.admission, CompactionDisposition::Defer(None))
        .unwrap();
    let Some(CompactionDelivery::Ready(second)) = coordinator.take_ready() else {
        panic!("returned result")
    };
    assert_eq!(second.admission.operation_id, admission.operation_id);
    assert_eq!(Arc::as_ptr(&second.generated), identity);
    coordinator
        .settle(&second.admission, CompactionDisposition::Defer(None))
        .unwrap();

    runtime.inner.cancel_async_compaction();
    runtime.inner.lock_state().manual_compaction = None;
    runtime.shutdown().await.unwrap();
    assert_eq!(model.streams_exited_count(), 3);
    let mut events = Vec::new();
    let mut after = None;
    loop {
        let page = runtime.inner.store.read_events(after, 128).unwrap();
        if page.events.is_empty() {
            break;
        }
        after = page.events.last().map(|event| event.sequence);
        events.extend(page.events);
    }
    assert!(events.iter().any(|event| matches!(&event.event,
        RuntimeEvent::CompactionOperationUpdated { status }
            if status.operation_id == admission.operation_id && status.phase == CompactionOperationPhase::Cancelled
    )));
}

/// A capture that was never transferred cannot outlive its triggering
/// Attempt's completed execution or retain the Session's proactive slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settled_trigger_releases_its_abandoned_capture() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, model) = headless_runtime(&directory, vec![one_turn_script()], None, None).await;
    runtime.activate();
    let trigger = AttemptId::for_conversation(&runtime.inner.conversation_id, 0);
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let admission = coordinator.admit(&trigger).unwrap().unwrap();
    let work = runtime.inner.async_work(&admission).unwrap();

    runtime
        .submit_inbound(text_content("finish the trigger"))
        .unwrap();
    within_liveness_guard("trigger settlement", runtime.child_attempt_settlement(0)).await;
    within_liveness_guard("abandoned capture status", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if state.status.phase == CompactionOperationPhase::Cancelled
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
            }
            changed.await;
        }
    })
    .await;
    assert!(admission.cancellation.is_cancelled());
    assert_eq!(model.requests().len(), 1, "no Summary worker was started");
    within_liveness_guard("orphan-free shutdown", runtime.shutdown())
        .await
        .unwrap();
    assert!(runtime.inner.async_work(&admission).is_none());
}

/// An unrelated Attempt cannot retire a capture; shutdown owns the final
/// no-worker cleanup after foreground ownership has returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_releases_abandoned_capture_after_unrelated_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, model) = headless_runtime(&directory, vec![one_turn_script()], None, None).await;
    runtime.activate();
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let admission = coordinator
        .admit(&AttemptId::new("another-trigger"))
        .unwrap()
        .unwrap();
    let work = runtime.inner.async_work(&admission).unwrap();

    runtime
        .submit_inbound(text_content("unrelated turn"))
        .unwrap();
    within_liveness_guard("unrelated settlement", runtime.child_attempt_settlement(0)).await;
    assert!(matches!(work.state.lock().unwrap().phase, Phase::Capturing));
    assert!(!admission.cancellation.is_cancelled());
    within_liveness_guard("abandoned capture drain", runtime.shutdown())
        .await
        .unwrap();
    let state = work.state.lock().unwrap();
    assert_eq!(state.status.phase, CompactionOperationPhase::Cancelled);
    assert_eq!(state.published.as_ref(), Some(&state.status));
    drop(state);
    assert!(work.joined.load(Ordering::Acquire));
    assert!(work.lifecycle.lock().unwrap().is_none());
    assert!(runtime.inner.async_work(&admission).is_none());
    assert_eq!(model.requests().len(), 1, "no Summary worker was started");
}

/// The manager probes and claims native idleness before it starts the shared
/// shutdown owner. Ready results do not pin residency, but their abandonment
/// must be recorded before the old writer reports quiescence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Keep generation, idle claim and durable retirement in one ordered proof.
async fn idle_claim_retires_deferred_ready_and_supervises_generation() {
    use crate::model::types::ModelProtocol;
    use crate::scripted_suites::support::model::{
        FixtureModel, ScriptedAdapterFactory, fixture_session_model,
    };

    let directory = tempfile::tempdir().unwrap();
    let (release, gate) = model_release();
    let mut summary = summary_turn_script("captured context");
    summary.insert(1, FakeStep::ParkUntilReleased(gate));
    let mut first = runtime_inbound_message("idle-old-one");
    first.content = text_content(&"old history ".repeat(512));
    let mut second = runtime_inbound_message("idle-old-two");
    second.content = text_content(&"more history ".repeat(512));
    let (runtime, model) = headless_runtime_with_options(
        &directory,
        vec![summary],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(first), MessageBlock::User(second)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    let idle_before = within_liveness_guard("initial idle epoch", async {
        loop {
            if let Ok(epoch) = runtime.idle_epoch() {
                break epoch;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let admission = start_background(&runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    within_liveness_guard("active Summary parked", async {
        model.parks().wait_for(|count| *count >= 1).await.unwrap();
    })
    .await;
    assert!(runtime.idle_epoch().is_err());
    assert!(!runtime.claim_idle(idle_before));
    assert!(!admission.cancellation.is_cancelled());
    assert_eq!(model.streams_exited_count(), 0);

    // Hold the idle slot while the physical worker settles, then change the
    // current model so this previously valid capture cannot fit now.
    runtime.inner.lock_state().manual_compaction = Some(CurrentManualCompaction {
        operation_id: crate::runtime::identity::CompactionOperationId::new("idle-test-slot"),
        cancellation: AgentCancellation::new(CancellationReason::RuntimeShutdown),
    });
    release.send(true).unwrap();
    within_liveness_guard("Summary worker joined", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if work.joined.load(Ordering::Acquire) {
                break;
            }
            changed.await;
        }
    })
    .await;
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    {
        let mut state = runtime.inner.lock_state();
        state.model = fixture_session_model(
            &[
                FixtureModel::text("scripted/small", ModelProtocol::OpenAiChatCompletions)
                    .with_context_window(1_024)
                    .with_max_output_tokens(128),
            ],
            "scripted/small",
            &ScriptedAdapterFactory::new(adapter),
        );
        state.manual_compaction = None;
    }
    runtime.inner.install_ready_async_when_idle().await;
    within_liveness_guard("ineligible Ready result deferred", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if matches!(state.phase, Phase::Ready(_))
                    && state.status.diagnostic.is_some()
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
                assert!(!matches!(state.phase, Phase::Failed(_) | Phase::Terminal));
            }
            changed.await;
        }
    })
    .await;

    within_liveness_guard("claim deferred idle result", async {
        loop {
            if let Ok(epoch) = runtime.idle_epoch()
                && runtime.claim_idle(epoch)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let outcome = within_liveness_guard("idle-claimed shutdown", runtime.shutdown_outcome())
        .await
        .unwrap();
    outcome.logical.unwrap();
    outcome.physical.unwrap();
    assert!(admission.cancellation.is_cancelled());
    assert!(runtime.inner.async_work(&admission).is_none());
    assert!(runtime.inner.store.projection_head().unwrap().is_none());
    assert_eq!(model.requests().len(), 1, "shutdown starts no new Summary");

    // Read the old Store directly: no cold reopen or restart reconciliation
    // may be needed to discover the terminal operation outcome.
    let events = runtime.inner.store.read_events(None, 128).unwrap().events;
    let terminals: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::CompactionOperationUpdated { status }
                if status.operation_id == admission.operation_id && status.phase.is_terminal() =>
            {
                Some(status.phase)
            }
            _ => None,
        })
        .collect();
    assert_eq!(terminals, [CompactionOperationPhase::Cancelled]);
    let again = runtime.shutdown_outcome().await.unwrap();
    again.logical.unwrap();
    again.physical.unwrap();
    assert_eq!(
        runtime.inner.store.read_events(None, 128).unwrap().events,
        events
    );
}
