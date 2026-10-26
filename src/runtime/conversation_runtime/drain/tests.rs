use crate::durable::JournalStore;
use crate::runtime::conversation_runtime::test_support::*;

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn sibling_mcp_settlement_failures_aggregate_through_one_runtime_drain() {
    use crate::tools::mcp::fixture::{
        FIXTURE_MODE_ENV, FixtureServer, TOOL_PREFIX_ENV, fixture_spawn_args, serve_if_fixture_mode,
    };
    use crate::tools::mcp::test_sync::CloseProbe;

    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }

    let test_name = "runtime::conversation_runtime::drain::tests::sibling_mcp_settlement_failures_aggregate_through_one_runtime_drain";
    let fixture_binding = |tool_prefix: Option<&str>| {
        let mut environment =
            std::collections::BTreeMap::from([(FIXTURE_MODE_ENV.to_owned(), "1".to_owned())]);
        if let Some(tool_prefix) = tool_prefix {
            environment.insert(TOOL_PREFIX_ENV.to_owned(), tool_prefix.to_owned());
        }
        crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture_spawn_args(test_name),
                cwd: None,
                environment,
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        }
    };
    let alpha = crate::runtime::identity::McpServerId::new("sibling-alpha");
    let beta = crate::runtime::identity::McpServerId::new("sibling-beta");
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _) = headless_runtime_with_options(
        &dir,
        Vec::new(),
        None,
        None,
        HeadlessRuntimeOptions {
            mcp_servers: std::collections::BTreeMap::from([
                (alpha.clone(), fixture_binding(None)),
                (beta.clone(), fixture_binding(Some("beta-"))),
            ]),
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();

    let alpha_runtime = runtime
        .inner
        .capability
        .current_mcp_runtime(&alpha)
        .expect("alpha generation A");
    let beta_runtime = runtime
        .inner
        .capability
        .current_mcp_runtime(&beta)
        .expect("beta generation A");
    alpha_runtime.install_close_probe(Arc::new(CloseProbe::failing(
        "sibling alpha physical settlement is unproven",
    )));
    beta_runtime.install_close_probe(Arc::new(CloseProbe::failing(
        "sibling beta physical settlement is unproven",
    )));

    runtime.inner.capability.retire_current_mcp_runtimes();
    let settlement = runtime.inner.capability.settle_ready_mcp_runtimes().await;
    let Err(failures) = settlement else {
        panic!("sibling failures must be reported by ready retirement settlement");
    };
    let message = failures.join("; ");
    assert!(message.contains("sibling alpha physical settlement is unproven"));
    assert!(message.contains("sibling beta physical settlement is unproven"));
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining,
        "the second diagnostic does not start a second lifecycle state machine"
    );
    assert_eq!(runtime.inner.capability.pending_mcp_retirements(), 2);

    let shutdown = runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("both sibling failures must remain final shutdown evidence: {shutdown:?}");
    };
    assert!(detail.contains("sibling alpha physical settlement is unproven"));
    assert!(detail.contains("sibling beta physical settlement is unproven"));
    assert_eq!(runtime.inner.capability.pending_mcp_retirements(), 2);
}

/// Shutdown closes further semantic admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_gates_further_admission() {
    let fixture = headless_fixture().await;
    fixture
        .runtime
        .shutdown()
        .await
        .expect("accepted after activation");
    assert!(matches!(
        fixture.runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::Shutdown)
    ));
}

/// A user cancellation that wins the owning attempt must remain the cause
/// of every runtime-driven interaction settlement. The waiter is parked
/// after observing cancellation but before it can call the coordinator;
/// shutdown therefore has to settle the pending map and propagate the
/// `AgentCancellation` winner itself.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_propagates_existing_user_cancellation_to_pending_interaction() {
    use crate::message::types::ContentBlockIndex;
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::tools::types::{ToolCall, ToolCallStart};

    let definition = ToolDefinition {
        id: ToolId::new("tool-approval-user-first"),
        name: "approval".to_owned(),
        description: "a deterministic approval test tool".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        }),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::default(),
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let tool = FakeTool::new(definition.clone(), success_result("must not run"));
    let tool_calls = tool.calls();
    let tool_started = tool.started();
    let mut registry = ToolRegistry::new();
    tool.register(&mut registry);

    let call_id = ToolCallId::new("call-runtime-user-first");
    let scripts = vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: call_id.clone(),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
            },
        }),
        FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: call_id.clone(),
            arguments_delta: "{\"text\":\"hi\"}".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCall {
                id: call_id,
                tool_id: definition.id,
                name: definition.name,
                arguments: serde_json::json!({"text": "hi"}),
            },
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]];

    let dir = tempfile::tempdir().expect("temp dir");
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let probe = CoordinatorProbe {
        drain_linearization: Some(drain_linearization.clone()),
        ..CoordinatorProbe::default()
    };
    let (runtime, _) = headless_runtime(&dir, scripts, Some(registry), Some(probe)).await;
    runtime.install_test_pre_tool_policy(Arc::new(RuntimeAskPolicy));

    let waiter_gate = Arc::new(InteractionWaitCancellationGate::default());
    waiter_gate.arm();
    runtime
        .inner
        .interaction
        .install_wait_cancellation_gate(waiter_gate.clone());
    let settle_gate = Arc::new(InteractionSettleGate::default());
    settle_gate.arm();
    runtime
        .inner
        .interaction
        .install_settle_gate(settle_gate.clone());

    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: runtime.clone(),
        replay_limit: None,
    })
    .expect("Runtime Client host");
    let (attachment, initialized) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("Runtime Client attachment");
    let cursor = match initialized {
        RuntimeClientResult::Initialized { .. } => attachment.checkpoint().unwrap().cursor,
        other => panic!("unexpected initialization result: {other:?}"),
    };
    let subscription = attachment
        .subscribe_events(cursor)
        .expect("Runtime Client subscription");

    runtime.activate();
    let accepted = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: RequestId::new(1),
        content: text_content("request approval"),
    });
    assert!(matches!(
        accepted.result,
        Some(RuntimeClientResult::InboundAccepted { .. })
    ));

    let pending = loop {
        match subscription.next().await {
            EventDelivery::Event(event) => {
                if let Some(interaction) = event.event.facts().iter().find_map(|fact| {
                    if let RuntimeClientEvent::InteractionPending { interaction } = fact {
                        Some((*interaction).clone())
                    } else {
                        None
                    }
                }) {
                    break interaction;
                }
            }
            EventDelivery::Pending => unreachable!("next never returns Pending"),
            delivery => panic!("pending interaction stream ended: {delivery:?}"),
        }
    };
    let (attempt_id, cancellation) = {
        let state = runtime.inner.state.lock().expect("runtime state lock");
        let current = state
            .current_attempt
            .as_ref()
            .expect("pending interaction has an active attempt");
        (current.attempt_id.clone(), current.cancellation.clone())
    };
    assert_eq!(pending.execution.attempt_id(), Some(&attempt_id));

    // This is the first-winner commit. The waiter observes the signal but
    // is then parked before it can call coordinator.cancel.
    assert!(cancellation.request_cancel(CancellationReason::UserRequested));
    assert_eq!(cancellation.reason(), CancellationReason::UserRequested);
    let waiter_gate_for_wait = waiter_gate.clone();
    tokio::task::spawn_blocking(move || waiter_gate_for_wait.wait_entered())
        .await
        .expect("waiter cancellation gate task");

    // Drain now contends with the already-cancelled attempt. Its own
    // RuntimeShutdown request must lose, and the drain path must use the
    // UserRequested reason when it removes the pending interaction.
    let runtime_for_shutdown = runtime.clone();
    let (shutdown_sender, mut shutdown_receiver) = tokio::sync::oneshot::channel();
    let drain_wait = drain_linearization.notified();
    tokio::spawn(async move {
        let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
    });
    drain_wait.await;
    assert_eq!(cancellation.reason(), CancellationReason::UserRequested);
    assert!(!cancellation.request_cancel(CancellationReason::RuntimeShutdown));
    assert_eq!(cancellation.reason(), CancellationReason::UserRequested);

    let settle_gate_for_wait = settle_gate.clone();
    tokio::task::spawn_blocking(move || settle_gate_for_wait.wait_entered())
        .await
        .expect("drain interaction settle gate task");
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining
    );
    assert_eq!(runtime.inner.interaction.pending_count(), 0);
    assert_eq!(
        shutdown_receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    );

    // Releasing the drain transition only hands the UserRequested result
    // to the still-parked waiter. Quiescence remains impossible until the
    // waiter releases its own cancellation gate and drops the payload.
    settle_gate.release();
    assert_eq!(
        shutdown_receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    );
    waiter_gate.release();
    shutdown_receiver
        .await
        .expect("shutdown completion sender")
        .expect("runtime reaches quiescence");
    assert_eq!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Quiescent
    );
    assert_eq!(runtime.inner.interaction.pending_count(), 0);

    let settled_outcome = loop {
        match subscription.next().await {
            EventDelivery::Event(event) => {
                if let Some(outcome) = event.event.facts().iter().find_map(|fact| {
                    if let RuntimeClientEvent::InteractionSettled {
                        interaction,
                        outcome,
                    } = fact
                        && *interaction == pending.interaction
                    {
                        Some((*outcome).clone())
                    } else {
                        None
                    }
                }) {
                    break outcome;
                }
            }
            EventDelivery::Pending => unreachable!("next never returns Pending"),
            delivery => panic!("settled interaction stream ended: {delivery:?}"),
        }
    };
    assert_eq!(
        settled_outcome,
        InteractionOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    );
    let (snapshot, _) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("post-shutdown snapshot");
    assert!(snapshot.pending_interactions.is_empty());

    let stale = attachment
        .handle_request_async(RuntimeClientRequest::InteractionRespond {
            id: RequestId::new(2),
            interaction: pending.interaction.clone(),
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await;
    assert_eq!(
        stale.error,
        Some(RuntimeClientError::InteractionNotPending {
            interaction: pending.interaction.clone()
        })
    );

    let canonical = runtime
        .inner
        .store
        .load_canonical()
        .expect("canonical tool settlement");
    let tool_messages = canonical
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_messages.len(), 1, "one structural tool result slot");
    assert!(matches!(
        &tool_messages[0].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
        }
    ));
    assert!(!matches!(
        &tool_messages[0].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::RuntimeShutdown,
            phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
        }
    ));
    assert!(tool_calls.borrow().is_empty(), "executor was never invoked");
    assert!(!*tool_started.borrow(), "executor never started");

    let journal = runtime
        .inner
        .store
        .read_events(None, 256)
        .expect("runtime event journal")
        .events;
    assert_eq!(
        journal
            .iter()
            .filter(|envelope| matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::ToolExecutionStarted { .. }
            ))
            .count(),
        0,
        "cancelled-before-start emits no ToolExecutionStarted"
    );
    let attempt_cancel_reasons = journal
        .iter()
        .filter_map(|envelope| match &envelope.event {
            crate::events::types::RuntimeEvent::AttemptCancelled { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        attempt_cancel_reasons,
        vec![CancellationReason::UserRequested]
    );
    assert_eq!(
        settled_outcome,
        InteractionOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        },
        "no RuntimeShutdown interaction terminal exists for this attempt"
    );
}

/// Repeated concurrent shutdown calls join one drain completion and
/// publish one lifecycle transition.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_shutdown_is_one_idempotent_drain() {
    let fixture = headless_fixture().await;
    let first = fixture.runtime.clone();
    let second = fixture.runtime.clone();
    let (left, right) = tokio::join!(first.shutdown(), second.shutdown());
    assert_eq!(left, Ok(()));
    assert_eq!(right, Ok(()));
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );
    let third = fixture.runtime.shutdown().await;
    assert_eq!(third, Ok(()));
}

/// Issue #63 (Finding 1): a successful acceptance and shutdown have one
/// total ordering. When acceptance linearizes first — proven by parking
/// `submit_inbound` inside its coordinator-lock critical section while
/// `shutdown` blocks on that same lock — the acceptance succeeds and
/// shutdown follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_linearizes_before_shutdown_when_acceptance_wins() {
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let shutdown_arrival = Arc::new(tokio::sync::Notify::new());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        admission_gate: None,
        settlement_gate: None,
        before_restore_gate: None,
        activation_gate: None,
        manual_compaction_settlement_gate: None,
        submit_gate: Some(gate.clone()),
        submit_arrival: None,
        shutdown_arrival: Some(shutdown_arrival.clone()),
        mcp_failure_drain_gate: None,
        drain_linearization: None,
        start_boundary_pause: None,
        model_arbitration_pause: None,
        tool_start_pause: None,
        drain_supervision: None,
        attempt_exit_gate: None,
        parent_guidance_seal_gate: None,
        background_failure_gate: None,
        subagent_failure_published_gate: None,
        ..CoordinatorProbe::default()
    }))
    .await;
    gate.arm();

    // `submit_inbound` parks inside its critical section: the coordinator
    // lock is held, the shutdown decision was read, but the durable
    // acceptance has not yet committed.
    let submit_runtime = fixture.runtime.clone();
    let submit_task = tokio::task::spawn_blocking(move || {
        submit_runtime
            .submit_inbound(text_content("raced"))
            .expect("acceptance linearized before shutdown")
    });
    let submit_entered = {
        let gate = gate.clone();
        tokio::task::spawn_blocking(move || gate.wait_entered())
    };
    submit_entered
        .await
        .expect("submit parked inside its critical section holding the lock");

    // Shutdown signals that it reached the point immediately before
    // attempting the coordinator lock, then blocks on that lock. The
    // ordering proof is mutex exclusion (submit holds the lock), not a
    // timeout: shutdown cannot complete while submit holds the critical
    // section.
    let shutdown_runtime = fixture.runtime.clone();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let shutdown_task = tokio::spawn(async move {
        shutdown_runtime
            .shutdown()
            .await
            .expect("shutdown reaches quiescence");
        let _ = shutdown_tx.send(());
    });
    shutdown_arrival.notified().await;

    // Release submit: the acceptance commits, then shutdown acquires the
    // lock and linearizes after it.
    let release = {
        let gate = gate.clone();
        tokio::task::spawn_blocking(move || gate.release())
    };
    release.await.expect("release submit");
    let admission = submit_task.await.expect("submit completed");
    assert_eq!(
        admission.inbound_sequence.get(),
        1,
        "the acceptance was the first sequence of the conversation"
    );
    shutdown_task.await.expect("shutdown completed");
    shutdown_rx
        .recv()
        .expect("shutdown completed after submit released the lock");
    // Shutdown is now authoritative: no later acceptance succeeds.
    assert!(matches!(
        fixture.runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::Shutdown)
    ));
}

/// Issue #63 (Finding 1): when shutdown linearizes first, a later submit
/// returns `Shutdown` and commits no pending item and consumes no
/// sequence. Admission is frozen deterministically so the pre-shutdown
/// acceptance stays pending for the exact-count assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_linearizes_before_acceptance_commits_nothing() {
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let shutdown_arrival = Arc::new(tokio::sync::Notify::new());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        admission_gate: Some(admission_gate.clone()),
        settlement_gate: None,
        before_restore_gate: None,
        activation_gate: None,
        manual_compaction_settlement_gate: None,
        submit_gate: None,
        submit_arrival: None,
        shutdown_arrival: Some(shutdown_arrival.clone()),
        mcp_failure_drain_gate: None,
        drain_linearization: None,
        start_boundary_pause: None,
        model_arbitration_pause: None,
        tool_start_pause: None,
        drain_supervision: None,
        attempt_exit_gate: None,
        parent_guidance_seal_gate: None,
        background_failure_gate: None,
        subagent_failure_published_gate: None,
        ..CoordinatorProbe::default()
    }))
    .await;
    // Freeze admission so the worker cannot adopt the pre-shutdown item.
    admission_gate.arm();
    let first = fixture
        .runtime
        .submit_inbound(text_content("before"))
        .expect("pre-shutdown acceptance");
    assert_eq!(first.inbound_sequence.get(), 1);
    admission_gate.wait_entered();
    // Shutdown linearizes first. The worker is still parked at its
    // pre-admission test boundary, so release that boundary only after
    // the explicit shutdown-arrival signal proves drain owns admission.
    let shutdown_runtime = fixture.runtime.clone();
    let shutdown_task = tokio::spawn(async move {
        shutdown_runtime
            .shutdown()
            .await
            .expect("shutdown reaches quiescence");
    });
    shutdown_arrival.notified().await;
    let release = {
        let admission_gate = admission_gate.clone();
        tokio::task::spawn_blocking(move || admission_gate.release())
    };
    release.await.expect("release parked admission");
    shutdown_task.await.expect("shutdown completed");
    assert!(matches!(
        fixture.runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::Shutdown)
    ));

    // The refused acceptance committed no pending item and consumed no
    // sequence: exactly the one pre-shutdown acceptance remains durable.
    let batch = fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .select_pending_batch()
        .expect("select")
        .expect("exactly one pre-shutdown pending item");
    assert_eq!(
        batch.items().len(),
        1,
        "the refused acceptance committed no pending item"
    );
    assert_eq!(
        batch.items()[0].sequence().get(),
        1,
        "the refused acceptance consumed no sequence"
    );
    // Release the frozen admission worker so the test's tokio runtime can
    // shut down: the worker observes the closed wake gate and returns without
    // adopting the pending item.
    admission_gate.release();
}

/// The MCP failure transition is one coordinator critical section. The
/// failure path is parked after both the latch and `Draining` are
/// published; a competing submit reaches the boundary but cannot cross
/// it until the failure path releases the coordinator lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_failure_and_lifecycle_closure_are_atomic_before_inbound() {
    let failure_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    failure_gate.arm();
    let submit_arrival = Arc::new(tokio::sync::Notify::new());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        submit_arrival: Some(submit_arrival.clone()),
        mcp_failure_drain_gate: Some(failure_gate.clone()),
        ..CoordinatorProbe::default()
    }))
    .await;

    let failure_inner = fixture.runtime.inner.clone();
    let failure = tokio::spawn(async move {
        failure_inner.fence_mcp_settlement_failure("atomic MCP failure".to_owned());
    });
    failure_gate.wait_entered();
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Draining,
        "the lifecycle transition is already published while the failure critical section is parked"
    );

    let submit_runtime = fixture.runtime.clone();
    let (submit_tx, mut submit_rx) = tokio::sync::oneshot::channel();
    let submit = tokio::spawn(async move {
        let _ = submit_tx.send(submit_runtime.submit_inbound(text_content("late inbound")));
    });
    submit_arrival.notified().await;
    assert!(
        matches!(
            submit_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the competing submit reached the boundary but cannot cross the held coordinator lock"
    );

    failure_gate.release();
    failure.await.expect("failure callback task");
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );
    assert_eq!(
        submit_rx.await.expect("submit result"),
        Err(InboundAdmissionError::Shutdown)
    );
    submit.await.expect("submit task");
    assert_eq!(
        fixture
            .runtime
            .tool_runtime()
            .durable_store()
            .load_pending()
            .expect("pending inbox")
            .len(),
        0,
        "the post-failure inbound was never durably accepted"
    );

    let shutdown = fixture.runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("the latched failure must remain shutdown evidence: {shutdown:?}");
    };
    assert!(detail.contains("atomic MCP failure"));
}

/// A worker that is already poised to admit pending work cannot publish a
/// new current attempt after the unified MCP failure transition wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_failure_closes_attempt_admission_before_worker_boundary() {
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let failure_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        admission_gate: Some(admission_gate.clone()),
        mcp_failure_drain_gate: Some(failure_gate.clone()),
        ..CoordinatorProbe::default()
    }))
    .await;

    admission_gate.arm();
    fixture
        .runtime
        .submit_inbound(text_content("pending before failure"))
        .expect("inbound is accepted before the failure wins");
    admission_gate.wait_entered();

    failure_gate.arm();
    let failure_inner = fixture.runtime.inner.clone();
    let failure = tokio::spawn(async move {
        failure_inner.fence_mcp_settlement_failure("attempt admission MCP failure".to_owned());
    });
    failure_gate.wait_entered();
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );
    assert!(
        fixture.model.requests().is_empty(),
        "the provider request count remains zero while admission is closed"
    );

    failure_gate.release();
    failure.await.expect("failure callback task");
    assert!(!fixture.runtime.has_current_attempt());
    admission_gate.release();
    let shutdown = fixture.runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("the latched failure must remain shutdown evidence: {shutdown:?}");
    };
    assert!(detail.contains("attempt admission MCP failure"));
    assert!(!fixture.runtime.has_current_attempt());
    assert!(fixture.model.requests().is_empty());
}

/// Lifecycle Draining remains the generic admission authority for manual
/// compaction after MCP failure publication; no compaction owner is
/// created and no second MCP-specific gate is consulted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_failure_blocks_manual_compaction_through_lifecycle() {
    let fixture = headless_fixture().await;
    fixture
        .runtime
        .inner
        .fence_mcp_settlement_failure("compaction MCP failure".to_owned());
    assert_eq!(
        fixture.runtime.compact_context(None).await,
        Err(crate::runtime::conversation_runtime::ManualCompactionError::Shutdown)
    );
    assert!(!fixture.runtime.has_manual_compaction());

    let shutdown = fixture.runtime.shutdown().await;
    let Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail }) =
        shutdown
    else {
        panic!("the latched failure must remain shutdown evidence: {shutdown:?}");
    };
    assert!(detail.contains("compaction MCP failure"));
}

/// M9c (Fix C): a foreground execution that started **before** any
/// cancellation existed must observe the cause that actually won the
/// race, not the attempt's start-time default.
///
/// Happens-before: the executor asserts its context is not cancelled and
/// only then publishes `started`; the test waits for `started` before
/// calling `shutdown`, so runtime drain is provably the first
/// cancellation of this attempt. The executor then reads the cause from
/// the attempt authority through its context.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreground_executor_observes_the_winning_runtime_shutdown_cause() {
    let (tool, mut started, observed) = CauseProbeTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start before any cancellation",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    within_liveness_guard("runtime shutdown", runtime.shutdown())
        .await
        .expect("drain reaches quiescence");
    assert_eq!(
        *observed.lock().expect("observed cause lock"),
        Some(CancellationReason::RuntimeShutdown),
        "the executor reads the winning cause from the attempt authority"
    );
    let store = runtime.tool_runtime().durable_store();
    assert!(
        store
            .read_events(None, 256)
            .expect("events")
            .events
            .iter()
            .any(|envelope| matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::AttemptCancelled {
                    reason: CancellationReason::RuntimeShutdown,
                    ..
                }
            )),
        "the attempt terminal event agrees with the executor's observation"
    );
}

/// M9c (Fix C, first-winner): a user cancellation that won first stays
/// the absorbing cause; a later runtime drain never relabels it, and the
/// executor reads the same first winner.
///
/// Happens-before: the executor publishes `started` before any
/// cancellation exists; `cancel_current_attempt` then provably wins the
/// first cancellation under the coordinator lock, and only afterwards
/// does `shutdown` request `RuntimeShutdown` on the same handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_cancellation_cause_survives_a_later_runtime_drain() {
    let (tool, mut started, observed) = CauseProbeTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start before any cancellation",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    let observations = await_observation(pending.as_ref(), |observation| {
        matches!(observation, ConversationObservation::AttemptAdmitted { .. })
    })
    .await;
    let attempt_id = observations
        .iter()
        .find_map(|observation| match observation {
            ConversationObservation::AttemptAdmitted { attempt_id, .. } => Some(attempt_id.clone()),
            _ => None,
        })
        .expect("the admitted attempt identity");
    runtime
        .cancel_current_attempt(&attempt_id)
        .expect("user cancellation wins first");

    within_liveness_guard("runtime shutdown", runtime.shutdown())
        .await
        .expect("drain reaches quiescence");
    assert_eq!(
        *observed.lock().expect("observed cause lock"),
        Some(CancellationReason::UserRequested),
        "a later runtime drain cannot relabel the first winning cause"
    );
    let store = runtime.tool_runtime().durable_store();
    assert!(
        store
            .read_events(None, 256)
            .expect("events")
            .events
            .iter()
            .any(|envelope| matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::AttemptCancelled {
                    reason: CancellationReason::UserRequested,
                    ..
                }
            )),
        "the terminal event reports the first winner"
    );
}

/// Issue #204 (case M): cancellation intent is never settlement.
/// Runtime drain cancels the admitted attempt and requests physical
/// cancellation of its foreground execution, but `shutdown` cannot
/// complete while the executor's settlement is still unresolved — the
/// drain barrier awaits the executor's settlement authority driving the
/// operation to its real terminal evidence (guarded against a
/// contract-violating executor by the settlement control-plane guard,
/// which this test never approaches on the wall clock) rather than
/// abandoning the operation.
///
/// This is the runtime quiescence invariant for local ownership: the
/// settle gate models residual rustX-owned local physical cleanup still
/// in flight (a kill/wait/reap or join ladder), and `shutdown()` must
/// never report `Quiescent` while that local cleanup ownership is
/// pending. Only when the settlement plane returns — cleanup completed —
/// may drain cross its barrier.
///
/// Happens-before: `cancel_observed` proves the drain's cancellation
/// request already reached the executor and the executor is now parked
/// on its settle gate, mid-settlement. The shutdown result channel must
/// still be empty at that instant; only releasing the gate lets drain
/// cross its settlement barrier and return.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_executor_physical_settlement() {
    let (tool, mut started, mut cancel_observed, settle_release) = SettleGateProbeTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start before any cancellation",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    let runtime_for_shutdown = runtime.clone();
    let (shutdown_sender, mut shutdown_receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
    });
    within_liveness_guard(
        "the executor to observe the drain's cancellation request",
        cancel_observed.wait_for(|observed| *observed),
    )
    .await
    .expect("cancel-observation channel stays open");
    assert_eq!(
        shutdown_receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty),
        "drain cannot complete while the admitted execution's settlement is unresolved"
    );

    settle_release.send_replace(true);
    within_liveness_guard("runtime shutdown", shutdown_receiver)
        .await
        .expect("shutdown task stays alive")
        .expect("drain reaches quiescence");

    let events = runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("events")
        .events;
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            }
        )),
        "the attempt terminal cancellation event is journaled"
    );
    assert_eq!(
        events
            .iter()
            .filter(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                    if tool_call_id == &ToolCallId::new("call-cause-probe")
            ))
            .count(),
        1,
        "exactly one terminal result is committed for the call"
    );
}

/// Issue #204 (drain, confirmed split settlement): runtime drain awaits
/// the executor's settlement AUTHORITY, not the physical completion
/// plane. The fixture's completion plane parks forever; its settlement
/// plane reports `Confirmed` only after a test gate releases. While the
/// gate is closed the shutdown future must still be pending; releasing
/// it lets drain cross the settlement barrier and reach quiescence,
/// with the canonical `Cancelled { RuntimeShutdown }` committed exactly
/// once.
///
/// Happens-before: `cancel_observed` proves the drain's cancellation
/// request already reached the executor and the settlement plane is
/// parked on its gate, mid-settlement — the shutdown result channel is
/// provably empty at that instant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_confirmed_settlement_then_drains() {
    let (tool, mut started, mut cancel_observed, settle_release) = DetachedConfirmGateTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start before any cancellation",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    let runtime_for_shutdown = runtime.clone();
    let (shutdown_sender, mut shutdown_receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
    });
    within_liveness_guard(
        "the settlement plane to observe the drain's cancellation request",
        cancel_observed.wait_for(|observed| *observed),
    )
    .await
    .expect("cancel-observation channel stays open");
    assert_eq!(
        shutdown_receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty),
        "drain cannot complete while the settlement authority is still gated"
    );

    settle_release.send_replace(true);
    within_liveness_guard("runtime shutdown", shutdown_receiver)
        .await
        .expect("shutdown task stays alive")
        .expect("drain reaches quiescence");

    let events = runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("events")
        .events;
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            }
        )),
        "the attempt terminal cancellation event is journaled"
    );
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::ToolExecutionSettlementObserved {
                certainty: crate::tools::deadline::ToolSettlementCertainty::Confirmed,
                ..
            }
        )),
        "the executor's confirmed settlement evidence is journaled"
    );
    let completed = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completed.len(),
        1,
        "exactly one terminal result is committed for the call"
    );
    assert!(
        matches!(
            completed[0].status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            }
        ),
        "the attempt winner owns the canonical reason of the proven cancellation: {:?}",
        completed[0].status
    );
}

/// Issue #204 (drain, unconfirmed split settlement): drain waits for
/// the attempt's canonical settlement — which awaits the executor's
/// settlement authority — and an `Unconfirmed` settlement resolves that
/// wait. The canonical `OutcomeUnknown` is committed and `shutdown`
/// RETURNS with quiescence, which is honest exactly because the
/// executor's `Unconfirmed` carried the local-ownership invariant: all
/// rustX-owned local execution ownership of the call already settled
/// (the fixture spawns no task or process), and only the remote
/// external effect beyond rustX's ownership domain remains uncertain.
/// External uncertainty never blocks local quiescence; local rustX
/// execution would (see the settle-gate cuts above).
///
/// Happens-before: `cancel_observed` proves the drain's cancellation
/// request reached the executor; the settlement plane then reports
/// `Unconfirmed` without any further test input, so drain returning
/// proves it required no physical completion. The terminal canonical
/// state is absorbing: the journal never gains another fact for the
/// call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one linear deterministic scenario
async fn shutdown_drains_past_unconfirmed_settlement() {
    let (tool, mut started, mut cancel_observed) = DetachedUnconfirmedTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start before any cancellation",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    let runtime_for_shutdown = runtime.clone();
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
    });
    within_liveness_guard(
        "the executor to observe the drain's cancellation request",
        cancel_observed.wait_for(|observed| *observed),
    )
    .await
    .expect("cancel-observation channel stays open");
    within_liveness_guard("runtime shutdown", shutdown_receiver)
        .await
        .expect("shutdown task stays alive")
        .expect("drain reaches quiescence once local execution ownership settled");

    let call_id = ToolCallId::new("call-cause-probe");
    let store = runtime.tool_runtime().durable_store();
    let events: Vec<RuntimeEvent> = store
        .read_events(None, 256)
        .expect("events")
        .events
        .into_iter()
        .map(|envelope| envelope.event)
        .collect();
    assert!(
        events.iter().any(|event| matches!(
            event,
            RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            }
        )),
        "the attempt terminal cancellation event is journaled"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolExecutionCancellationRequested {
                cause: crate::tools::deadline::ToolCancellationCause::Attempt(
                    CancellationReason::RuntimeShutdown
                ),
                ..
            }
        )),
        "the drain's physical cancellation request is journaled"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolExecutionSettlementObserved {
                certainty: crate::tools::deadline::ToolSettlementCertainty::Unconfirmed,
                ..
            }
        )),
        "the executor's unconfirmed settlement evidence is journaled"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        )),
        "the executor's settlement authority returned, so no control-plane failure is journaled"
    );
    let completed = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } if tool_call_id == &call_id => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completed.len(),
        1,
        "exactly one terminal result is committed for the call"
    );
    let ToolExecutionStatus::OutcomeUnknown { detail } = &completed[0].status else {
        panic!(
            "unconfirmed post-frontier settlement is OutcomeUnknown, never a confirmed \
             cancellation: {:?}",
            completed[0].status
        );
    };
    assert!(
        detail.contains("external-effect frontier"),
        "the executor's own unconfirmed frontier evidence: {detail}"
    );
    let facts_before_cleanup = drain_call_facts(&events, &call_id);

    // The executor left no rustX-owned local execution behind: the
    // settled terminal state is absorbing and the journal never gains
    // another fact for the call.
    let later: Vec<RuntimeEvent> = store
        .read_events(None, 256)
        .expect("events")
        .events
        .into_iter()
        .map(|envelope| envelope.event)
        .collect();
    assert_eq!(
        drain_call_facts(&later, &call_id),
        facts_before_cleanup,
        "the committed terminal state is absorbing"
    );
}

/// Issue #204 (drain, settlement control-plane failure): a
/// contract-violating executor whose settlement authority never returns
/// cannot block runtime drain forever, and drain never lies about local
/// ownership. The hard deadline fires on the manual clock, the
/// cancellation request provably reaches the executor, the settlement
/// control-plane guard expires, and the lifecycle commits one canonical
/// `OutcomeUnknown` — journaled with the typed
/// `ToolExecutionSettlementControlFailed` fact and NO
/// `ToolExecutionSettlementObserved`, because no executor settlement
/// evidence was ever observed. Before shutdown may report quiescence,
/// the executor's remaining rustX-owned local ownership is consumed by
/// dropping the handle (proven by the operation's drop guard), so
/// `Quiescent` is honest: no rustX-owned local execution survives, and
/// only the external effect frontier — here unprovable by construction —
/// remains unknown.
///
/// Happens-before: `cancel_observed` proves the cancellation request
/// reached the executor before the guard window is exhausted; the
/// settlement waiter is registered before the clock advances, so the
/// committed `OutcomeUnknown` cannot settle unobserved.
#[allow(clippy::too_many_lines)] // one linear deterministic scenario
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_drains_after_settlement_control_plane_failure() {
    let (tool, mut started, mut cancel_observed, operation_dropped) = GuardViolationTool::new();
    let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_b6f1eb3d-33f7-7829-864f-4b9d1335716d");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id.clone(),
        crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        )),
    )
    .expect("tool runtime");
    let coordinator = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id,
            workspace: tool_runtime.workspace().clone(),
            agent_activation: {
                let mut activation = fixture_activation(&tool_runtime);
                activation.profile.tools.builtin = registry
                    .definitions()
                    .into_iter()
                    .filter(|tool| tool.origin.source().is_none())
                    .map(|tool| tool.name.clone())
                    .collect();
                activation
            },
            base_tool_registry: Arc::new(registry),
            extension_tools: tool_runtime.extension_tool_plane(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("coordinator");
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator.commit(candidate).expect("commit");
    let model = Arc::new(FakeModel::new(vec![script, one_turn_script()]));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let config = RuntimeConversationConfig {
        explicit_model: true,
        agent_id: AgentId::new("agent-a"),
        model: scripted_session_model(adapter),
        approval_mode: ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
            std::time::Duration::from_secs(10),
            None,
        ),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(DefaultTokenEstimator),
            status_engine: Some(AgentStatusEngine::default()),
        },
        tool_runtime,
        resources: test_resources(&coordinator),
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    };
    let runtime = ConversationRuntime::with_test_monotonic_clock(
        config,
        clock.clone() as Arc<dyn crate::runtime::MonotonicClock>,
    )
    .expect("runtime composition");
    runtime.activate();
    runtime
        .submit_inbound(text_content("start the foreground tool"))
        .expect("accepted");
    within_liveness_guard(
        "the foreground tool to start at its executor-start frontier",
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("start channel stays open");

    let settled = runtime.settlement_signal().notified();
    tokio::pin!(settled);
    settled.as_mut().enable();
    // t=10_000: the hard deadline fires cancellation intent.
    clock.advance(10_000);
    within_liveness_guard(
        "the executor to observe the cancellation request",
        cancel_observed.wait_for(|observed| *observed),
    )
    .await
    .expect("cancel-observation channel stays open");
    // t=40_000: the settlement control-plane guard (30s from the
    // intent) expires — the executor's settlement authority never
    // returned, a settlement-contract violation with no evidence.
    clock.advance(30_000);
    within_liveness_guard("the admitted attempt to settle", settled).await;

    // The bounded ownership rule of the guard path is real: dropping
    // the handle consumed the executor's remaining local ownership, so
    // drain's quiescence below is honest.
    assert!(
        operation_dropped.load(std::sync::atomic::Ordering::Acquire),
        "the guard path dropped the handle, consuming the executor's remaining local ownership"
    );
    within_liveness_guard("runtime shutdown", runtime.shutdown())
        .await
        .expect("drain reaches quiescence after the control-plane failure");

    let events = runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("events")
        .events;
    assert!(
        events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::ToolExecutionDeadlineFired {
                kind: crate::tools::deadline::ToolDeadlineKind::Hard,
                ..
            }
        )),
        "the hard deadline intent is journaled"
    );
    let control_failures: Vec<&str> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            RuntimeEvent::ToolExecutionSettlementControlFailed {
                tool_call_id,
                reason,
                ..
            } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        control_failures.len(),
        1,
        "exactly one typed settlement control-plane failure fact"
    );
    assert!(
        control_failures[0].contains("settlement control plane did not return"),
        "the lifecycle's contract-violation record: {}",
        control_failures[0]
    );
    assert!(
        !events.iter().any(|envelope| matches!(
            &envelope.event,
            RuntimeEvent::ToolExecutionSettlementObserved { tool_call_id, .. }
                if tool_call_id == &ToolCallId::new("call-cause-probe")
        )),
        "no settlement was ever observed: the executor's authority never returned"
    );
    let completed = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1, "exactly one canonical tool result");
    assert!(
        matches!(
            completed[0].status,
            ToolExecutionStatus::OutcomeUnknown { .. }
        ),
        "a settlement control-plane failure is OutcomeUnknown, never TimedOut: {:?}",
        completed[0].status
    );
}

/// M9c (Fix D): the current-attempt **slot** and the attempt **task**
/// are distinct ownership facts. The slot is cleared inside
/// `finish_attempt`, but the task still owes the coordinator its final
/// admission callback, so quiescence must wait for the task itself.
///
/// Happens-before: `attempt_exit_gate` parks the settled attempt task
/// after the coordinator lock is released and the slot is provably empty,
/// and before the final callback runs. `drain_linearization` proves the
/// drain transition committed while the task is parked there, and
/// `mark_quiescent` — the one authority that publishes quiescence — is
/// then invoked directly and must refuse. Releasing the gate is the only
/// thing that lets the task return and shutdown complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attempt_task_exit_belongs_to_the_quiescence_proof() {
    let attempt_exit_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    attempt_exit_gate.arm();
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        attempt_exit_gate: Some(attempt_exit_gate.clone()),
        parent_guidance_seal_gate: None,
        drain_linearization: Some(drain_linearization.clone()),
        ..CoordinatorProbe::default()
    }))
    .await;
    fixture
        .runtime
        .submit_inbound(text_content("one turn"))
        .expect("accepted");
    attempt_exit_gate.wait_entered();
    assert!(
        !fixture.runtime.has_current_attempt(),
        "the current-attempt slot is already clear at the parked exit boundary"
    );

    assert!(
        fixture.runtime.idle_epoch().is_err(),
        "idle residency also waits for the native attempt task exit"
    );

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = fixture.runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    within_liveness_guard("the drain linearization", drain_linearization.notified()).await;
    assert!(
        !fixture.runtime.inner.lifecycle.mark_quiescent(),
        "the attempt task still owes a callback, so quiescence is refused"
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "an empty current-attempt slot is not attempt-task settlement"
    );
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );

    let release = {
        let gate = attempt_exit_gate.clone();
        tokio::task::spawn_blocking(move || gate.release())
    };
    release.await.expect("release the parked attempt task");
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("the attempt task exit completes the quiescence proof");
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );

    // A stale handle to the runtime's own admission callback cannot
    // produce a semantic effect after quiescence.
    let store = fixture.runtime.tool_runtime().durable_store();
    let events_before = store.read_events(None, 256).expect("events").events;
    let canonical_before = store.load_canonical().expect("canonical");
    let inner = fixture
        .runtime
        .weak_inner()
        .upgrade()
        .expect("the test still owns the runtime");
    inner.admit_next_attempt();
    assert!(!fixture.runtime.has_current_attempt());
    assert_eq!(
        store.read_events(None, 256).expect("events").events,
        events_before
    );
    assert_eq!(store.load_canonical().expect("canonical"), canonical_before);
    assert!(matches!(
        fixture.runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::Shutdown)
    ));
}

/// M9c (Fix E / 8.1): the exact `Running -> Draining` linearization, not
/// an arrival hint, is what a competing acceptance must lose to.
///
/// Happens-before: `drain_linearization` fires immediately after the
/// lifecycle CAS commits, while shutdown still holds the coordinator
/// lock. The competing `submit_inbound` is released only after that
/// signal, so it necessarily queues on the coordinator lock and reads the
/// already-published `Draining` state. It commits nothing and consumes no
/// sequence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_linearization_precedes_the_refused_acceptance() {
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let fixture = headless_fixture_with(Some(CoordinatorProbe {
        admission_gate: Some(admission_gate.clone()),
        drain_linearization: Some(drain_linearization.clone()),
        ..CoordinatorProbe::default()
    }))
    .await;
    // Freeze admission so the pre-shutdown item stays pending and the
    // durable acceptance ledger is stable for the assertions below.
    admission_gate.arm();
    let first = fixture
        .runtime
        .submit_inbound(text_content("before"))
        .expect("pre-shutdown acceptance");
    assert_eq!(first.inbound_sequence.get(), 1);
    admission_gate.wait_entered();

    // The competing acceptance is released only after drain has provably
    // linearized.
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let (late_tx, late_rx) = tokio::sync::oneshot::channel();
    let late_runtime = fixture.runtime.clone();
    tokio::spawn(async move {
        release_rx.await.expect("release channel stays open");
        let _ = late_tx.send(late_runtime.submit_inbound(text_content("racing")));
    });

    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = fixture.runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    within_liveness_guard("the drain linearization", drain_linearization.notified()).await;
    assert_eq!(
        fixture.runtime.lifecycle_state(),
        ConversationLifecycleState::Draining,
        "the drain transition is committed before the competing acceptance runs"
    );
    release_tx
        .send(())
        .expect("release the competing acceptance");
    assert!(
        matches!(
            late_rx.await.expect("late acceptance result"),
            Err(InboundAdmissionError::Shutdown)
        ),
        "an acceptance that starts after the drain commit is refused"
    );

    admission_gate.release();
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("drain completes");

    let batch = fixture
        .runtime
        .tool_runtime()
        .mailbox()
        .select_pending_batch()
        .expect("select")
        .expect("exactly one pre-shutdown pending item");
    assert_eq!(
        batch.items().len(),
        1,
        "the refused acceptance committed no pending item"
    );
    assert_eq!(
        batch.items()[0].sequence().get(),
        1,
        "the refused acceptance consumed no sequence"
    );
}

/// M9c: a conversation-owned background execution survives attempt
/// settlement but not conversation lifetime. Drain requests cancellation,
/// waits for the executor and its exactly-once terminal Pending Inbound
/// publication, and only then becomes quiescent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn runtime_shutdown_drains_active_background_and_publishes_terminal_inbound() {
    let dir = tempfile::tempdir().expect("temp dir");
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let (runtime, _model) = runtime_with_model_probe_at(
        &dir,
        "conv_5955a0a5-829c-7229-b261-cf19b4990758",
        Vec::new(),
        vec![one_turn_script()],
        Some(CoordinatorProbe {
            drain_linearization: Some(drain_linearization.clone()),
            ..CoordinatorProbe::default()
        }),
    )
    .await
    .expect("runtime");
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    let (executor, mut started, release) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new("call-m9c-background"),
        },
        tool_id: crate::runtime::identity::ToolId::new("tool-m9c-background"),
        tool_name: "background_gate".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let background = runtime.tool_runtime().background();
    let prepared = background
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        background
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("ownership commit")
    else {
        panic!("active background dispatch must commit ownership");
    };
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("background start channel stays open");

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let result = shutdown_runtime.shutdown().await;
        let _ = done_tx.send(result);
    });
    drain_linearization.notified().await;
    background
        .wait_until_cancelling(&execution_id)
        .await
        .expect("runtime drain requests background cancellation");
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "cancellation intent is not background terminality"
    );

    release.send_replace(true);
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("background drain completes");
    let terminal = background
        .snapshot(&execution_id)
        .expect("terminal background record");
    // The executor raced past the drain's cancellation request and
    // proved success: cancellation intent (including drain intent)
    // cannot overwrite the executor-proven outcome (Issue #202).
    assert_eq!(
        terminal.state,
        crate::tools::background::BackgroundLifecycle::Succeeded
    );
    assert!(matches!(
        terminal.result.as_ref().map(|result| &result.status),
        Some(crate::tools::types::ToolExecutionStatus::Success)
    ));
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );
    let pending_rows = runtime
        .tool_runtime()
        .mailbox()
        .select_pending_batch()
        .expect("pending terminal notification")
        .expect("terminal notification remains pending after admission closure");
    assert_eq!(pending_rows.items().len(), 1);
    assert_eq!(
        runtime
            .tool_runtime()
            .mailbox()
            .select_pending_batch()
            .expect("retry select")
            .expect("same pending row")
            .items()[0]
            .sequence(),
        pending_rows.items()[0].sequence(),
        "terminal publication is exactly once and retains its identity"
    );
    let terminal_events = runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("events")
        .events
        .into_iter()
        .filter(|event| {
            matches!(
                event.event,
                crate::events::types::RuntimeEvent::BackgroundTerminalPublished { .. }
            )
        })
        .count();
    assert_eq!(terminal_events, 1);
}

/// Issue #83: conversation drain supervises a Workflow-owned native child
/// through the same `SubagentRegistry` boundary. Cancellation is observed
/// at the child control channel, while shutdown remains parked until the
/// child sends its native terminal frame and the registry proves reap and
/// durable settlement.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn runtime_shutdown_waits_for_workflow_child_native_quiescence() {
    let dir = tempfile::tempdir().expect("temp dir");
    let conversation_id = ConversationId::new("conv_0aca8719-5a71-735b-8721-8979eb90371a");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(conversation_id.clone()).expect("store"),
    );
    let (runtime, _model, subagents) =
        headless_runtime_over_store_with_subagents(&dir, conversation_id.as_str(), store, None)
            .await;
    runtime.activate();

    let runtime_root = dir.path().join("subagents");
    std::fs::create_dir_all(&runtime_root).expect("subagent runtime root");
    let (driver_end, mut peer) = tokio::net::UnixStream::pair().expect("IPC pair");
    let (observation_end, _observation_peer) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .expect("staged native child");
    let child_pid = child.id().expect("child pid");
    let child_root = runtime_root.join(format!("test-child-{child_pid}"));
    std::fs::create_dir_all(&child_root).expect("child runtime root");
    subagents.push_staged_override(crate::runtime::subagent::process::StagedChild::for_test(
        child,
        driver_end,
        observation_end,
        child_root.clone(),
    ));

    let workflow_id =
        crate::runtime::workflow::WorkflowId::parse("drain_workflow").expect("workflow id");
    let mut node_instance = crate::runtime::workflow::test_instance("drain_workflow", "review");
    node_instance.block.run.conversation_id = conversation_id.clone();
    let run_id = node_instance.block.run.clone();
    let prepared = subagents
        .prepare(
            &crate::runtime::subagent::SubagentStartSpec {
                authority: crate::runtime::subagent::DurableAgentAuthority {
                    title: "Explore task".into(),
                    execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
                    resolved: test_resolved_subagent("reviewer"),
                    approval_mode: ApprovalMode::Policy,
                },
                admission: crate::runtime::subagent::ActivationAdmission {
                    attachments: Vec::new(),
                    task: "Produce the workflow result.".to_owned(),
                    context: None,
                    origin: crate::runtime::subagent::AgentActivationOrigin::Workflow {
                        node_id: Box::new(node_instance.clone()),
                    },
                    terminal: crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
                        output_schema: serde_json::json!({
                            "type": "object",
                            "properties": {"summary": {"type": "string"}},
                            "required": ["summary"],
                            "additionalProperties": false
                        }),
                        workflow_id,
                        run_id,
                        node_id: Box::new(node_instance),
                    },
                },
            },
            &CancellationSignal::new(),
        )
        .await
        .expect("prepare workflow child");
    let accepted = match subagents
        .commit(prepared, &CancellationSignal::new())
        .await
        .expect("commit workflow child")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("workflow child was cancelled before admission")
        }
    };
    assert!(
        runtime.idle_epoch().is_err(),
        "workflow child ownership prevents idle claim"
    );
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));

    assert_pending_mutation_preserves_child(&runtime, &subagents, &accepted.subagent_id);

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });

    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("cancellation frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Cancel {
            reason: Some(CancellationReason::RuntimeShutdown)
        })
    ));
    assert!(matches!(
        done_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(!runtime.is_quiescent());
    assert_eq!(subagents.unsettled_snapshot().len(), 1);

    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &crate::runtime::subagent::ipc::ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Cancelled,
                content: None,
                diagnostic: None,
            },
        ),
    )
    .await
    .expect("native cancellation result");

    done_rx
        .await
        .expect("shutdown result")
        .expect("workflow child drain");
    assert!(runtime.is_quiescent());
    assert!(subagents.unsettled_snapshot().is_empty());
    assert!(!child_root.exists());
    let snapshot = subagents
        .snapshot(&accepted.subagent_id)
        .expect("retained child snapshot");
    assert_eq!(
        snapshot.state,
        crate::runtime::subagent::SubagentState::Cancelled
    );
    assert!(snapshot.is_settled());
}

#[cfg(unix)]
#[tokio::test]
async fn abandoned_subagent_publication_cannot_hide_unproven_physical_settlement() {
    use crate::runtime::subagent::ipc::{
        ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
        write_child_frame,
    };
    use crate::runtime::subagent::{
        ActivationAdmission, AgentActivationOrigin, DurableAgentAuthority,
        InheritedExecutionPolicy, SubagentStartOutcome, SubagentStartSpec, SubagentTerminalMode,
    };
    let dir = tempfile::tempdir().unwrap();
    let conversation = ConversationId::generate();
    let store =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    let (runtime, _, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        conversation.as_str(),
        store.clone(),
        None,
    )
    .await;
    runtime.activate();
    let (mut staged, mut peer) = stage_runtime_test_child(&dir.path().join("unproven-abandoned"));
    staged.retain_for_test(
        crate::runtime::identity::ProcessUnitId::new("unprovable-unit"),
        i32::MAX,
    );
    subagents.push_staged_override(staged);
    let cancellation = CancellationSignal::new();
    let prepared = subagents
        .prepare(
            &SubagentStartSpec {
                authority: DurableAgentAuthority {
                    title: "Explore task".into(),
                    resolved: test_resolved_subagent("reviewer"),
                    execution_policy: InheritedExecutionPolicy::default(),
                    approval_mode: ApprovalMode::Policy,
                },
                admission: ActivationAdmission {
                    attachments: Vec::new(),
                    task: "unproven child with abandoned publication".into(),
                    context: None,
                    origin: AgentActivationOrigin::CreationTool {
                        tool_call_id: ToolCallId::new("unproven-abandoned-create"),
                    },
                    terminal: SubagentTerminalMode::Normal,
                },
            },
            &cancellation,
        )
        .await
        .unwrap();
    let SubagentStartOutcome::Accepted(admitted) =
        subagents.commit(prepared, &cancellation).await.unwrap()
    else {
        panic!("owned child");
    };
    assert!(matches!(
        read_parent_frame(&mut peer).await.unwrap(),
        Some(ParentFrame::Delegate(_))
    ));
    // The real driver holds an explicitly unprovable retained anchor.
    // Publication fails only after that driver's physical classification.
    store.arm_fail_accept_times(3);
    write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("semantic answer".into()),
            diagnostic: None,
        }),
    )
    .await
    .unwrap();
    let snapshot = subagents
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
    assert_eq!(
        snapshot.settlement.publication,
        crate::runtime::subagent::SubagentPublication::Abandoned
    );
    assert_eq!(
        snapshot.settlement.physical,
        crate::runtime::subagent::SubagentPhysicalSettlement::Unproven
    );
    assert_eq!(subagents.unproven_settlements(), vec![admitted.subagent_id]);
    assert!(
        runtime.settle_child_physical_lifetime(false).await.is_err(),
        "abandoning logical publication cannot invent a physical receipt for the containing child"
    );
    assert!(runtime.shutdown().await.is_err());
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn runtime_shutdown_rejects_unproven_child_physical_settlement() {
    use crate::runtime::subagent::ipc::{
        ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
        write_child_frame,
    };
    use crate::runtime::subagent::{
        ActivationAdmission, AgentActivationOrigin, DurableAgentAuthority,
        InheritedExecutionPolicy, SubagentStartOutcome, SubagentStartSpec, SubagentTerminalMode,
    };
    let dir = tempfile::tempdir().unwrap();
    let conversation = ConversationId::generate();
    let store =
        Arc::new(crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap());
    let (runtime, _, subagents) =
        headless_runtime_over_store_with_subagents(&dir, conversation.as_str(), store, None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: runtime.clone(),
        replay_limit: None,
    })
    .expect("Runtime Client host");
    let (attachment, initialized) = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("Runtime Client attachment");
    let RuntimeClientResult::Initialized { .. } = initialized else {
        panic!("expected initialization");
    };
    let cursor = attachment.checkpoint().unwrap().cursor;
    let subscription = attachment
        .subscribe_events(cursor)
        .expect("subscribe before the owner transition");
    runtime.activate();
    let (mut staged, mut peer) = stage_runtime_test_child(&dir.path().join("unproven-child"));
    staged.retain_for_test(
        crate::runtime::identity::ProcessUnitId::new("unproven-unit"),
        i32::MAX,
    );
    subagents.push_staged_override(staged);
    let prepared = subagents
        .prepare(
            &SubagentStartSpec {
                authority: DurableAgentAuthority {
                    title: "Explore task".into(),
                    resolved: test_resolved_subagent("reviewer"),
                    execution_policy: InheritedExecutionPolicy::default(),
                    approval_mode: ApprovalMode::Policy,
                },
                admission: ActivationAdmission {
                    attachments: Vec::new(),
                    task: "Exercise failed physical settlement".into(),
                    context: None,
                    origin: AgentActivationOrigin::CreationTool {
                        tool_call_id: ToolCallId::new("unproven-create"),
                    },
                    terminal: SubagentTerminalMode::Normal,
                },
            },
            &CancellationSignal::new(),
        )
        .await
        .unwrap();
    let SubagentStartOutcome::Accepted(admitted) = subagents
        .commit(prepared, &CancellationSignal::new())
        .await
        .unwrap()
    else {
        panic!("admission cancelled");
    };
    assert!(matches!(
        read_parent_frame(&mut peer).await.unwrap(),
        Some(ParentFrame::Delegate(_))
    ));
    write_child_frame(
        &mut peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("semantic result".into()),
            diagnostic: None,
        }),
    )
    .await
    .unwrap();
    let terminal = subagents
        .wait_until_settled(&admitted.subagent_id)
        .await
        .unwrap();
    assert!(
        !terminal.is_settled(),
        "semantic termination cannot prove containment"
    );
    // Observe the actual registry -> RuntimeObserver -> PendingObservations
    // -> RuntimeClientHost publication path. This stream receipt is the
    // synchronization boundary: no snapshot read flushes the transition
    // before the subscriber has independently received it.
    let streamed = loop {
        match subscription.next().await {
            EventDelivery::Event(event) => {
                if let Some(agent) = event.event.facts().iter().find_map(|fact| match fact {
                    RuntimeClientEvent::AgentUpdated { agent, .. }
                        if agent.agent_id == admitted.child_agent_id
                            && agent.activation_state.is_terminal() =>
                    {
                        Some((*agent).clone())
                    }
                    RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => checkpoint
                        .snapshot
                        .agents
                        .iter()
                        .find(|agent| {
                            agent.agent_id == admitted.child_agent_id
                                && agent.activation_state.is_terminal()
                        })
                        .cloned()
                        .map(Box::new),
                    _ => None,
                }) {
                    break agent;
                }
            }
            EventDelivery::Pending => unreachable!("next never returns Pending"),
            delivery => panic!("terminal Agent stream ended: {delivery:?}"),
        }
    };
    assert_eq!(
        streamed.state,
        crate::runtime::subagent::AgentState::Unavailable
    );
    let (owner, activation) = subagents
        .agent_snapshot_with_activation(&admitted.child_agent_id)
        .expect("fresh owner snapshot");
    let expected = crate::runtime_client::projection::views::agent_view(&owner, &activation);
    assert_eq!(*streamed, expected);
    let status = attachment.handle_request(RuntimeClientRequest::AgentStatus {
        id: RequestId::new(1),
        agent_id: admitted.child_agent_id.clone(),
    });
    assert_eq!(status.error, None);
    let observed: crate::runtime_client::snapshot::RuntimeClientAgent =
        crate::scripted_suites::common::native_domain_single(
            status.result.expect("native Agent page"),
        );
    assert_eq!(observed, expected);
    assert_eq!(
        host.checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("fresh host snapshot")
            .0
            .agents,
        vec![expected]
    );
    let error = runtime
        .shutdown()
        .await
        .expect_err("unproven owner prevents quiescence");
    assert!(
        matches!(error, crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { ref detail }
        if detail.contains(admitted.subagent_id.as_str()) && detail.contains("physical settlement is unresolved")),
        "{error:?}"
    );
    assert!(
        runtime.settle_child_physical_lifetime(false).await.is_err(),
        "the explicit unresolved physical owner forbids a child recovery receipt",
    );
    assert!(!runtime.is_quiescent());
}

/// M9c: the foreground Bash path composes its existing physical process
/// proof into runtime quiescence. Drain requests cancellation while the
/// supervised process is owned; the test observes the real process-group
/// terminal frame, parks before the direct supervisor-child reap, and
/// proves shutdown cannot complete until that native settlement handoff
/// is released.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn runtime_shutdown_waits_for_foreground_process_terminality() {
    use crate::runtime::process_runner::RunnerLifecycleHook;
    use crate::runtime::types::ConversationLifecycleState;
    use crate::tools::executor::ToolRegistry;
    use crate::tools::native::{BashTestControl, BashTool};
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
    };

    let control = BashTestControl::new().hold_terminal_event();
    let process_lifecycle: RunnerLifecycleHook = control.runner_control().lifecycle.clone();
    let terminal_hold = control
        .terminal_hold()
        .expect("terminal hold is armed")
        .clone();
    let definition = ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-bash"),
        name: "bash".to_owned(),
        description: "run one supervised bash command".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "minLength": 1},
                "timeout": {"type": "integer", "minimum": 1}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let mut registry = ToolRegistry::new();
    registry
        .register(
            definition.clone(),
            Arc::new(BashTool::with_test_control(control.clone())),
        )
        .expect("test Bash registration");
    let call_id = crate::runtime::identity::ToolCallId::new("call-m9c-process");
    let first_request = vec![
        FakeStep::Emit(crate::model::event::ModelEvent::Started),
        FakeStep::Emit(crate::model::event::ModelEvent::ToolCallStarted {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCallStart {
                id: call_id.clone(),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
            },
        }),
        FakeStep::Emit(crate::model::event::ModelEvent::ToolCallArgumentsDelta {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call_id: call_id.clone(),
            arguments_delta: r#"{"command":"sleep 30"}"#.to_owned(),
        }),
        FakeStep::Emit(crate::model::event::ModelEvent::ToolCallCompleted {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call: crate::tools::types::ToolCall {
                id: call_id,
                tool_id: definition.id,
                name: definition.name,
                arguments: serde_json::json!({"command": "sleep 30"}),
            },
        }),
        FakeStep::Emit(crate::model::event::ModelEvent::Completed {
            finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ];
    let dir = tempfile::tempdir().expect("temp dir");
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let (runtime, _model) = headless_runtime(
        &dir,
        vec![first_request, one_turn_script()],
        Some(registry),
        Some(CoordinatorProbe {
            drain_linearization: Some(drain_linearization.clone()),
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("run the supervised process"))
        .expect("accepted");

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        process_lifecycle.await_ownership_established(),
    )
    .await
    .expect("the foreground process reaches physical ownership");

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    drain_linearization.notified().await;

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        terminal_hold.await_held(),
    )
    .await
    .expect("TERM/KILL and process-group terminality are observed");
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "group terminality without direct-child reap is not runtime quiescence"
    );
    assert!(
        control
            .recorded_signals()
            .iter()
            .any(|signal| signal.signal == "SIGTERM" && signal.emitted),
        "runtime cancellation reaches the owned process group"
    );

    terminal_hold.release();
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("foreground process drain completes");
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );
}

/// M9c: the Agent Loop remains the foreground batch owner while runtime
/// drain cancels it. A deterministic tool-start frontier parks after the
/// first parallel sibling; drain wins there, the second sibling is never
/// announced or invoked, and the first started sibling still settles
/// before runtime quiescence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn runtime_shutdown_closes_parallel_foreground_start_frontier() {
    use crate::agent::execution::test_sync::ToolStartPause;
    use crate::tools::executor::ToolRegistry;
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
    };

    let definition = |id: &str, name: &str| ToolDefinition {
        id: crate::runtime::identity::ToolId::new(id),
        name: name.to_owned(),
        description: String::new(),
        input_schema: serde_json::json!({"type": "object"}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Parallel,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let (tool_a, started_a, _release_a) = GatedBackgroundExecutor::new();
    let (tool_b, started_b, _release_b) = GatedBackgroundExecutor::new();
    let mut registry = ToolRegistry::new();
    registry
        .register(definition("tool-a", "alpha"), Arc::new(tool_a))
        .expect("tool A registration");
    registry
        .register(definition("tool-b", "beta"), Arc::new(tool_b))
        .expect("tool B registration");

    let call_a = crate::scripted_suites::support::fake::ScriptedCall {
        id: "call-a",
        tool_id: "tool-a",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let call_b = crate::scripted_suites::support::fake::ScriptedCall {
        id: "call-b",
        tool_id: "tool-b",
        name: "beta",
        arguments: serde_json::json!({}),
    };
    let mut script = vec![FakeStep::Emit(crate::model::event::ModelEvent::Started)];
    script.extend(
        crate::scripted_suites::support::fake::tool_call_events(0, &call_a)
            .into_iter()
            .map(FakeStep::Emit),
    );
    script.extend(
        crate::scripted_suites::support::fake::tool_call_events(1, &call_b)
            .into_iter()
            .map(FakeStep::Emit),
    );
    script.push(FakeStep::Emit(crate::model::event::ModelEvent::Completed {
        finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
        usage: None,
    }));

    let dir = tempfile::tempdir().expect("temp dir");
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let (tool_start_pause, mut tool_start_reached, tool_start_release) = ToolStartPause::install();
    let (runtime, _model) = headless_runtime(
        &dir,
        vec![script],
        Some(registry),
        Some(CoordinatorProbe {
            drain_linearization: Some(drain_linearization.clone()),
            tool_start_pause: Some(tool_start_pause),
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("run parallel tools"))
        .expect("accepted");
    tool_start_reached
        .wait_for(|reached| *reached)
        .await
        .expect("tool-start gate stays open");

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    drain_linearization.notified().await;
    assert!(matches!(
        done_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    tool_start_release
        .send(())
        .expect("release the first start frontier");
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("parallel foreground work settles");

    assert!(
        !*started_a.borrow(),
        "drain prevents the first physical poll"
    );
    assert!(!*started_b.borrow(), "the second sibling never executes");
    let events = runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("event journal")
        .events;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                crate::events::types::RuntimeEvent::ToolExecutionStarted { .. }
            ))
            .count(),
        1,
        "only the first parallel sibling crossed the start frontier"
    );
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );
}

/// M9c: runtime drain uses the same M9b start gate as explicit attempt
/// cancellation. Drain wins while the turn is parked before arbitration,
/// so the attempt settles with the runtime-owned cause and no provider or
/// durable request-start fact exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_shutdown_before_model_turn_start_never_starts_the_request() {
    use crate::agent::execution::test_sync::StartBoundaryPause;

    let dir = tempfile::tempdir().expect("temp dir");
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let (runtime, model) = headless_runtime(
        &dir,
        vec![one_turn_script()],
        None,
        Some(CoordinatorProbe {
            drain_linearization: Some(drain_linearization.clone()),
            start_boundary_pause: Some(pause),
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    runtime.activate();
    runtime
        .submit_inbound(text_content("shutdown before start"))
        .expect("accepted");
    pre_start.await_park(1).await;

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    drain_linearization.notified().await;
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "drain waits for the parked attempt rather than treating cancellation as settlement"
    );

    pre_start.release();
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("runtime reaches quiescence");
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );
    assert!(model.requests().is_empty(), "provider request never starts");

    let store = runtime.tool_runtime().durable_store();
    assert!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .is_empty(),
        "runtime cancellation before arbitration creates no request snapshot"
    );
    let journal = store.read_events(None, 128).expect("journal").events;
    assert!(
        !journal.iter().any(|envelope| matches!(
            envelope.event,
            crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
        )),
        "runtime cancellation before arbitration creates no start fact"
    );
    assert!(matches!(
        journal.last().map(|envelope| &envelope.event),
        Some(crate::events::types::RuntimeEvent::AttemptCancelled {
            reason: CancellationReason::RuntimeShutdown,
            ..
        })
    ));
}

/// M9c: a provider request that crossed the durable start boundary keeps
/// runtime drain pending until the native request future settles. The
/// release handle is then exercised again after quiescence to prove that
/// a stale callback cannot create a late semantic effect.
///
/// # Why the live turn is held at an exact barrier
///
/// `begin_drain` requests cancellation of the current attempt inside the
/// coordinator critical section, and the Agent Loop's provider
/// arbitration is `stream.next()` first, *attempt cancellation second*.
/// A provider stream that is merely pending therefore lets the attempt
/// settle as cancelled at any moment after that request — including
/// before this test task is polled again. Reading `Draining` or an empty
/// shutdown channel right after `drain_linearization` was consequently
/// an assertion about an instant nothing ordered, and it observed
/// `Quiescent` whenever the attempt happened to settle first.
///
/// [`ModelArbitrationPause`] holds the attempt **inside** its stream loop
/// — after the two provider items are fully processed and before the next
/// provider/cancellation arbitration is constructed — so the attempt
/// provably *cannot* settle while the barrier holds. Every fact this test
/// reads while it holds is therefore stable rather than raced:
/// `has_current_attempt()`, the lifecycle state, and the shutdown channel
/// all describe a runtime that is structurally unable to move. The
/// `drain_supervision` signal is the stronger claim on top of that: drain
/// registered its settlement waiter while the slot was still occupied, so
/// supervision is committed to this exact owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn runtime_shutdown_waits_for_started_model_settlement_and_blocks_late_effects() {
    use crate::agent::execution::test_sync::ModelArbitrationPause;

    let dir = tempfile::tempdir().expect("temp dir");
    let (release_tx, release_rx) = crate::scripted_suites::support::fake::model_release();
    let script = vec![
        FakeStep::Emit(crate::model::event::ModelEvent::Started),
        FakeStep::Emit(crate::model::event::ModelEvent::TextDelta {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            text: "partial".to_owned(),
        }),
        FakeStep::ParkUntilReleased(release_rx),
        FakeStep::Emit(crate::model::event::ModelEvent::Failed {
            error: crate::model::error::ModelError {
                kind: crate::model::error::ModelErrorKind::Cancelled,
                message: "provider settled cancellation".to_owned(),
                retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
        }),
    ];
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let drain_supervision = Arc::new(tokio::sync::Notify::new());
    // Armed after the two scripted provider items — `Started` and the
    // text delta — so the attempt parks with a durably started request it
    // still owns, and before the arbitration that would let the drain's
    // cancellation settle it.
    let (model_pause, mut model_pause_reached, model_pause_release) =
        ModelArbitrationPause::install(2);
    let (runtime, model) = headless_runtime(
        &dir,
        vec![script],
        None,
        Some(CoordinatorProbe {
            drain_linearization: Some(drain_linearization.clone()),
            drain_supervision: Some(drain_supervision.clone()),
            model_arbitration_pause: Some(model_pause),
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
        .submit_inbound(text_content("park the provider"))
        .expect("accepted");

    // The provider stream is open and owned by the attempt, and the
    // attempt is held at the exact arbitration barrier: it cannot settle
    // until this test releases it, whatever cancellation is requested
    // meanwhile.
    model_pause_reached
        .wait_for(|is_reached| *is_reached)
        .await
        .expect("model arbitration pause channel stays open");
    let store = runtime.tool_runtime().durable_store();
    assert_eq!(
        model.requests().len(),
        1,
        "provider crossed the start boundary"
    );
    assert_eq!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .len(),
        1,
        "the durable request-start boundary is committed"
    );

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let result = shutdown_runtime.shutdown().await;
        let _ = done_tx.send(result);
    });
    within_liveness_guard("the drain linearization", drain_linearization.notified()).await;
    // Drain then parks on this exact owner: the waiter is registered and
    // the current-attempt slot is still occupied. A drain that had
    // short-circuited could never reach this signal.
    within_liveness_guard(
        "drain to park on the started provider turn",
        drain_supervision.notified(),
    )
    .await;
    assert!(
        runtime.has_current_attempt(),
        "the started provider turn is still runtime-owned while drain supervises it"
    );
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining,
        "drain linearized before the provider was released"
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "cancellation request is not provider settlement"
    );

    // The provider emulator deliberately ignores cancellation until this
    // explicit release. This is the physical settlement proof: releasing
    // it first, and the arbitration barrier only afterwards, means the
    // attempt's next arbitration observes the provider's own terminal
    // event rather than a stream that is merely pending.
    release_tx.send_replace(true);
    let _ = model_pause_release.send(());
    done_rx
        .await
        .expect("shutdown result channel")
        .expect("runtime reaches quiescence");
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Quiescent
    );
    assert!(matches!(
        store
            .read_events(None, 256)
            .expect("terminal events")
            .events
            .last()
            .map(|envelope| &envelope.event),
        Some(crate::events::types::RuntimeEvent::AttemptCancelled {
            reason: CancellationReason::RuntimeShutdown,
            ..
        })
    ));
    let requests_before = model.requests().len();
    let events_before = store.read_events(None, 256).expect("events").events;
    let canonical_before = store.load_canonical().expect("canonical");
    let head_before = store.load_head().expect("head");
    let pending_before = store.load_pending().expect("pending inbound");
    let revision_before = runtime.capability().current_snapshot().revision();
    let background_before = runtime.tool_runtime().background().active_snapshot().len();

    // The stale callback source must be proven *gone*, not merely
    // observed to do nothing: every model invocation stream owner has
    // left, so no task remains that could still read this watch channel.
    within_liveness_guard(
        "every model stream owner to exit",
        model
            .streams_exited()
            .wait_for(|exited| *exited >= requests_before as u64),
    )
    .await
    .expect("stream-exit channel stays open");

    // A stale release/callback handle retained by the old owner is now a
    // no-op: it cannot start another provider request or append a fact.
    release_tx.send_replace(false);
    release_tx.send_replace(true);
    // Give the runtime's executor a real scheduling opportunity: any
    // task that *could* still act would run before this join returns.
    tokio::task::yield_now().await;
    tokio::spawn(async {})
        .await
        .expect("a scheduling opportunity for any surviving owner");

    // A stale admission callback handle is refused too.
    let inner = runtime
        .weak_inner()
        .upgrade()
        .expect("the test still owns the runtime");
    inner.admit_next_attempt();
    assert!(
        !runtime.has_current_attempt(),
        "attempt admission is closed"
    );

    assert_eq!(model.requests().len(), requests_before, "provider requests");
    assert_eq!(
        store.read_events(None, 256).expect("events").events,
        events_before,
        "quiescence is a terminal ownership boundary"
    );
    assert_eq!(store.load_canonical().expect("canonical"), canonical_before);
    assert_eq!(store.load_head().expect("head"), head_before);
    assert_eq!(
        store.load_pending().expect("pending inbound"),
        pending_before
    );
    assert_eq!(
        runtime.capability().current_snapshot().revision(),
        revision_before,
        "capability revisions"
    );
    assert_eq!(
        runtime.tool_runtime().background().active_snapshot().len(),
        background_before,
        "background ownership"
    );
    assert!(matches!(
        runtime.submit_inbound(text_content("post-quiescence")),
        Err(InboundAdmissionError::Shutdown)
    ));
    assert_eq!(
        store.load_pending().expect("pending inbound"),
        pending_before,
        "the refused acceptance consumed no sequence"
    );
}

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
            title: "Explore task".into(),
            execution_policy: InheritedExecutionPolicy::default(),
            resolved: test_resolved_subagent("explore"),
            approval_mode: ApprovalMode::Policy,
        },
        admission: ActivationAdmission {
            attachments: Vec::new(),
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
