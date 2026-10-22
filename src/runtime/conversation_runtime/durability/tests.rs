use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::durable::inbox::ConversationInboundCapability;
use crate::runtime::conversation_runtime::test_support::*;

/// Issue #63 (Finding 5 / Important 5): a transient select storage
/// failure follows the bounded retry and completes a fresh admission
/// cycle; the pending item is admitted exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transient_select_failure_recovers_and_admits_once() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_b49ab8ad-7952-716b-8f30-a0e5f0375fd7",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_b49ab8ad-7952-716b-8f30-a0e5f0375fd7",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // One select fault: the first admission fails, the bounded retry
    // succeeds.
    store.arm_fail_select_times(1);
    let admission = runtime
        .submit_inbound(text_content("item"))
        .expect("accepted");
    assert_eq!(admission.inbound_sequence.get(), 1);

    // The transient failure is surfaced exactly once as a DurableFailure.
    let observations = await_observation(pending.as_ref(), |o| {
        matches!(o, ConversationObservation::DurableFailure { .. })
    })
    .await;
    assert_eq!(
        observations
            .iter()
            .filter(|o| matches!(o, ConversationObservation::DurableFailure { .. }))
            .count(),
        1,
        "exactly one transient DurableFailure observation"
    );

    // The retry adopts the pending item exactly once and completes the
    // admission cycle (settlement completes).
    runtime.child_attempt_settlement(0).await;
    let ledger = runtime.coordinator_ledger().expect("settled");
    assert!(
        ledger.iter().any(|m| matches!(
            m,
            MessageBlock::User(user) if user.id == admission.message_id
        )),
        "the pending item was adopted exactly once"
    );
    assert!(
        store.load_pending().expect("load pending").is_empty(),
        "no pending item remains after the successful retry"
    );
}

/// Issue #63 (Important 5): a persistent select storage failure moves the
/// runtime into the explicit `DurabilityFailed` state after the bounded
/// retry; pending work remains intact, no attempt is admitted, and a
/// later durable mutation fails typed (no false healthy state).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_select_failure_enters_durability_failed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_2765079b-5cc3-7da8-81c2-537c52f240cc",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_2765079b-5cc3-7da8-81c2-537c52f240cc",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // Arm two select faults: the first fails and re-kicks exactly once;
    // the retry fails again → explicit DurabilityFailed, no hot loop.
    store.arm_fail_select_times(2);
    let admission = runtime
        .submit_inbound(text_content("item"))
        .expect("accepted");
    assert_eq!(admission.inbound_sequence.get(), 1);

    let observations = await_observation(pending.as_ref(), |o| {
        matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "select_pending_batch")
    })
    .await;
    assert!(
        observations
            .iter()
            .any(|o| matches!(o, ConversationObservation::DurabilityFailed { .. })),
        "the persistent failure is surfaced as an explicit degraded state"
    );

    // Pending remains durably intact, no attempt was admitted, and a
    // later mutation requiring durability fails typed.
    assert_eq!(
        store.load_pending().expect("load pending").len(),
        1,
        "the persistent failure left the accepted pending item intact"
    );
    assert!(
        !runtime.has_current_attempt(),
        "no attempt was admitted through a failed selection"
    );
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));
}

/// Issue #63 (retry domain): two independent first failures of
/// *different* durable stages never combine into a false
/// `DurabilityFailed` — the finite admission-cycle budget gives select
/// and adopt independent allowances, so the adopt failure gets its own
/// bounded retry after the select retry already succeeded; the item is
/// then admitted exactly once and the cycle completes normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn select_then_adopt_failures_get_independent_bounded_retries() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_b4d6ae75-9048-7504-8dad-c0ed48da2f01",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_b4d6ae75-9048-7504-8dad-c0ed48da2f01",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // One select fault and one adopt fault: pass 1 consumes the select
    // retry allowance and re-kicks; pass 2 selects successfully (the
    // select allowance remains consumed) but fails the adopt, which
    // consumes adopt's independent allowance; pass 3 adopts successfully
    // and completes the cycle before admitting the item.
    store.arm_fail_select_times(1);
    store.arm_fail_adopt_times(1);
    let admission = runtime
        .submit_inbound(text_content("item"))
        .expect("accepted");
    assert_eq!(admission.inbound_sequence.get(), 1);

    runtime.child_attempt_settlement(0).await;
    let ledger = runtime.coordinator_ledger().expect("settled");
    assert!(
        ledger.iter().any(|m| matches!(
            m,
            MessageBlock::User(user) if user.id == admission.message_id
        )),
        "the pending item was adopted exactly once after the independent retries"
    );
    assert!(
        store.load_pending().expect("load pending").is_empty(),
        "no pending item remains after the successful retries"
    );
    // Every durability observation is published during admission,
    // strictly before the settlement handoff observed above.
    let observations = pending.drain_fixture_facts();
    let transient = observations
        .iter()
        .filter(|o| matches!(o, ConversationObservation::DurableFailure { .. }))
        .count();
    assert_eq!(
        transient, 2,
        "exactly two transient failures (one per operation), never a combined failure"
    );
    assert!(
        !observations
            .iter()
            .any(|o| matches!(o, ConversationObservation::DurabilityFailed { .. })),
        "two independent first failures must not become DurabilityFailed"
    );
}

/// Issue #63 (retry-cycle budget): alternating transient failures cannot
/// keep replacing one another's retry debt. The deterministic ordered
/// fault script forces exactly this admission sequence:
///
/// ```text
/// select fails -> select succeeds -> adopt fails -> select fails again
/// ```
///
/// The second select failure exhausts the select allowance retained from
/// the first operation, so the runtime fails closed before any attempt is
/// admitted. The test uses the real Runtime Client projection to prove
/// the explicit degraded state is observable; the timeout only guards
/// against a deadlocked or non-progressing test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn alternating_select_adopt_failures_exhaust_one_admission_cycle() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_97aa4ab9-a684-7a4a-8846-b45621bf8052",
        ))
        .expect("in-memory store"),
    );
    let (runtime, model) = headless_runtime_over_store(
        &dir,
        "conv_97aa4ab9-a684-7a4a-8846-b45621bf8052",
        store.clone(),
    )
    .await;
    let host = crate::runtime_client::RuntimeClientHost::new(
        crate::runtime_client::RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        },
    )
    .expect("runtime client host");
    let (attachment, _) = host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach runtime client");
    let subscription = attachment
        .subscribe_events(crate::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe runtime client");
    runtime.activate();

    // Activation's empty admission cycle is complete before this script
    // is armed. The pending item therefore drives exactly the four
    // operations named above.
    store.arm_admission_fault_script([
        crate::durable::sqlite::AdmissionFaultOperation::SelectPendingBatch,
        crate::durable::sqlite::AdmissionFaultOperation::AdoptPendingBatch,
        crate::durable::sqlite::AdmissionFaultOperation::SelectPendingBatch,
    ]);
    runtime
        .submit_inbound(text_content("item"))
        .expect("accepted");

    let failure = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match subscription.next().await {
                crate::runtime_client::EventDelivery::Event(event)
                    if event.event.facts().iter().any(|fact| matches!(fact, crate::runtime_client::RuntimeClientEvent::RuntimeDurabilityFailed {
                            operation,
                            ..
                        } if operation == "select_pending_batch")) =>
                {
                    break event;
                }
                crate::runtime_client::EventDelivery::Event(_) => {}
                delivery => panic!("unexpected terminal client delivery: {delivery:?}"),
            }
        }
    })
    .await
    .expect("the alternating fault sequence reaches explicit failure");
    assert!(failure.event.facts().iter().any(
        |fact| matches!(fact, crate::runtime_client::RuntimeClientEvent::RuntimeDurabilityFailed {
            operation,
            ..
        } if operation == "select_pending_batch")
    ));

    // The ordered script was fully consumed, proving the intended
    // deterministic operation sequence rather than a persistent
    // same-operation shortcut.
    assert!(
        store
            .admission_fault_script
            .lock()
            .expect("admission fault script lock")
            .is_empty(),
        "select fail -> select success -> adopt fail -> select fail"
    );

    let (snapshot, snapshot_cursor) = host
        .checkpoint()
        .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        .expect("client snapshot");
    assert_eq!(
        snapshot
            .durability_failure
            .as_ref()
            .map(|failure| failure.operation.as_str()),
        Some("select_pending_batch"),
        "the Runtime Client projection exposes the explicit degraded state"
    );
    assert_eq!(
        store.load_pending().expect("load pending").len(),
        1,
        "the accepted pending item remains durable and intact"
    );
    assert!(
        !runtime.has_current_attempt(),
        "no attempt is admitted through the failed adoption cycle"
    );
    assert!(
        model.requests().is_empty(),
        "the failed admission cycle never reaches the model"
    );
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));
    // Failure closes semantic admission immediately. Durable presentation
    // of the already accepted input may finish afterwards, independently.
    let mut cursor = failure.cursor.get();
    loop {
        let event = match subscription.try_next() {
            crate::runtime_client::EventDelivery::Event(event) => event,
            crate::runtime_client::EventDelivery::Pending => break,
            other => panic!("unexpected delivery: {other:?}"),
        };
        cursor += 1;
        assert_eq!(event.cursor.get(), cursor);
        assert!(event.event.facts().iter().any(|fact| matches!(
            fact,
            crate::runtime_client::RuntimeClientEvent::ReadDomainsUpdated { .. }
        )));
    }
    assert!(
        cursor >= snapshot_cursor.get(),
        "snapshot suffix is fully replayable"
    );
}

/// Issue #63 (retry domain): a persistent adopt storage failure moves
/// the runtime into the explicit `DurabilityFailed(adopt_pending_batch)`
/// state after its own bounded retry; the pending work remains intact,
/// no attempt is admitted, and a later durable mutation fails typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_adopt_failure_enters_durability_failed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_51dc7c9c-4b76-7f38-861b-49d80370e1b8",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_51dc7c9c-4b76-7f38-861b-49d80370e1b8",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // Two adopt faults: pass 1 selects successfully but fails the adopt
    // (the adopt allowance is consumed and a re-kick is armed); the retry
    // fails the same stage again -> explicit DurabilityFailed, no hot
    // loop.
    store.arm_fail_adopt_times(2);
    let admission = runtime
        .submit_inbound(text_content("item"))
        .expect("accepted");
    assert_eq!(admission.inbound_sequence.get(), 1);

    let observations = await_observation(pending.as_ref(), |o| {
        matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "adopt_pending_batch")
    })
    .await;
    let transient = observations
        .iter()
        .filter(|o| matches!(o, ConversationObservation::DurableFailure { .. }))
        .count();
    assert_eq!(
        transient, 1,
        "exactly one transient failure before the second adopt-stage failure"
    );

    // Pending remains durably intact, no attempt was admitted, and a
    // later durable mutation fails typed.
    assert_eq!(
        store.load_pending().expect("load pending").len(),
        1,
        "the persistent failure left the accepted pending item intact"
    );
    assert!(
        !runtime.has_current_attempt(),
        "no attempt was admitted through a failed adoption"
    );
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));
}

/// Issue #63 (active-attempt durability audit) + Issue #12 (M9b): when
/// the active attempt hits a durable canonical-write failure, the
/// attempt settles failed with the typed durable-store failure AND the
/// coordinator records the durable-authority failure — the runtime
/// never returns to a false `Healthy` state that admits further work as
/// though storage were fine.
///
/// Under M9b the first canonical write of an attempt is the model-turn
/// start transaction (request-scoped context + Request Snapshot +
/// `ModelRequestStarted`); its failure leaves no start fact and no
/// provider invocation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_attempt_durable_failure_degrades_the_runtime() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_503e0440-6f7e-731a-8571-e29416cdf3d5",
        ))
        .expect("in-memory store"),
    );
    let (runtime, model) = headless_runtime_over_store(
        &dir,
        "conv_503e0440-6f7e-731a-8571-e29416cdf3d5",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // The model-turn start transaction of the first turn fails durably.
    store.arm_request_start_fault_script([
        crate::durable::sqlite::RequestStartFaultOperation::BeforeContextAppend,
    ]);
    let admission = runtime
        .submit_inbound(text_content("item"))
        .expect("accepted");
    assert_eq!(admission.inbound_sequence.get(), 1);

    // The coordinator enters the explicit DurabilityFailed state for
    // the active attempt's durable commit failure.
    let observations = await_observation(pending.as_ref(), |o| {
        matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "request_start")
    })
    .await;
    assert!(
        observations.iter().any(|o| matches!(
            o,
            ConversationObservation::Event { event, .. }
                if matches!(
                    event,
                    crate::events::types::RuntimeEvent::AttemptFailed {
                        error: crate::events::types::AttemptFailure::Runtime {
                            error: crate::runtime::types::RuntimeError::DurableStore { .. },
                        },
                        ..
                    }
                )
        )),
        "the attempt settled failed with the typed durable-store failure"
    );

    // No false healthy progress: new durable work is refused typed and
    // no further attempt is admitted.
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));
    assert!(
        model.requests().is_empty(),
        "the durable fault struck the model-turn start transaction, so the \
         attempt failed before its first model request and no provider \
         invocation escaped the failed start"
    );

    // The durable Ledger contains the adopted inbound but never the
    // failed start's request-scoped context: memory and durability
    // stayed consistent (a failed durable commit installed nothing).
    let canonical = store.load_canonical().expect("load canonical");
    assert_eq!(
        canonical.len(),
        1,
        "only the adopted inbound is durable; the failed start committed nothing"
    );
    assert!(
        matches!(&canonical[0], MessageBlock::User(user) if user.id == admission.message_id),
        "the adopted inbound survived intact"
    );
}

/// A terminal Event Journal failure must degrade the owning runtime while
/// leaving the execution settlement candidate and the durable Journal
/// truthful: no observer or local read model may see a terminal event that
/// `SQLite` rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_event_durable_failure_degrades_without_fabricated_terminal() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_8a56d655-7b4e-7b7a-870c-20e586d7d16f",
        ))
        .expect("in-memory store"),
    );
    let (runtime, model) = headless_runtime_over_store(
        &dir,
        "conv_8a56d655-7b4e-7b7a-870c-20e586d7d16f",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    store.arm_fail_next_terminal_event();
    let admission = runtime
        .submit_inbound(text_content("terminal fault"))
        .expect("accepted");
    let observations = await_observation(pending.as_ref(), |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "event_journal"
        )
    })
    .await;

    assert_eq!(
        model.requests().len(),
        1,
        "the model turn completed normally"
    );
    assert!(
        !observations.iter().any(|observation| matches!(
            observation,
            ConversationObservation::Event { event, .. } if is_terminal_event(event)
        )),
        "the rejected terminal candidate never reaches the runtime observer"
    );
    let journal = store.read_events(None, 128).expect("event journal").events;
    assert!(
        !journal
            .iter()
            .any(|envelope| is_terminal_event(&envelope.event)),
        "the failed terminal transaction leaves no durable terminal event"
    );
    assert!(
        store
            .load_canonical()
            .expect("canonical ledger")
            .iter()
            .any(|message| {
                matches!(message, MessageBlock::User(user) if user.id == admission.message_id)
            })
    );
    assert_eq!(store.terminal_event_attempts(), 1);
    assert!(!runtime.has_current_attempt());
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));
}

/// Issue #60: after the subagent registry exhausts its bounded terminal
/// publication budget, the owning runtime (not the registry) becomes the
/// explicit durability-health owner. A first transient publication fault
/// is retried without degrading the runtime; exhaustion retains the
/// candidate as unresolved and rejects later durable mutations.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn exhausted_subagent_publication_degrades_the_owning_runtime() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_fb18687a-681e-748a-85a6-401be78c689e",
        ))
        .expect("in-memory store"),
    );
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_fb18687a-681e-748a-85a6-401be78c689e",
        store.clone(),
        Some(admission_gate.clone()),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    let _admission_release = admission_gate.arm_scoped();
    runtime
        .submit_inbound(text_content("hold admission worker"))
        .expect("hold inbound");
    within_liveness_guard(
        "subagent runtime admission worker gate",
        tokio::task::spawn_blocking({
            let admission_gate = admission_gate.clone();
            move || admission_gate.wait_entered()
        }),
    )
    .await
    .expect("admission gate task");

    // One transient terminal-publication fault is retried successfully;
    // the owning runtime remains healthy.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("child-one"));
    subagents.push_staged_override(staged);
    let accepted = match subagents
        .commit(
            subagents
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            title: "Explore task".into(),
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            attachments: Vec::new(),
                            task: "first terminal".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-subagent-one"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare first"),
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("commit first")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("first subagent unexpectedly rolled back")
        }
    };
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    store.arm_fail_accept_times(1);
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &crate::runtime::subagent::ipc::ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("first".to_owned()),
                diagnostic: None,
            },
        ),
    )
    .await
    .expect("first result");
    let first = subagents
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("first settles");
    assert_eq!(
        first.state,
        crate::runtime::subagent::SubagentState::Succeeded
    );
    assert!(
        runtime.inner.durability_failure_diagnostic().is_none(),
        "one transient publication fault does not degrade the runtime"
    );

    // The initial publication plus both bounded retries now fail. The
    // worker is still parked, so no unrelated durable acceptance can
    // consume the scripted storage faults.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("child-two"));
    subagents.push_staged_override(staged);
    let accepted = match subagents
        .commit(
            subagents
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            title: "Explore task".into(),
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            attachments: Vec::new(),
                            task: "second terminal".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-subagent-two"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare second"),
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("commit second")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("second subagent unexpectedly rolled back")
        }
    };
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    let publication_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let _publication_release = publication_gate.arm_scoped();
    runtime
        .inner
        .probe
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .subagent_failure_published_gate = Some(publication_gate.clone());
    store.arm_fail_accept_times(3);
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &crate::runtime::subagent::ipc::ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("second".to_owned()),
                diagnostic: None,
            },
        ),
    )
    .await
    .expect("second result");
    let observations = await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "subagent_terminal_publication"
        )
    })
    .await;
    assert_eq!(
        observations
            .iter()
            .filter(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { operation, .. }
                    if operation == "subagent_terminal_publication"
            ))
            .count(),
        1,
        "exhaustion degrades the runtime once"
    );
    tokio::task::spawn_blocking({
        let gate = publication_gate.clone();
        move || gate.wait_entered()
    })
    .await
    .unwrap();
    assert!(
        subagents
            .snapshot(&accepted.subagent_id)
            .unwrap()
            .settlement
            .publication
            != crate::runtime::subagent::SubagentPublication::Abandoned,
        "runtime health publication precedes registry settlement"
    );
    let settled = subagents.wait_until_settled(&accepted.subagent_id);
    tokio::pin!(settled);
    assert!(
        futures_util::poll!(&mut settled).is_pending(),
        "health observation alone cannot complete terminal settlement"
    );
    publication_gate.release();
    let unresolved = settled.await.expect("second settlement");
    assert_eq!(
        unresolved.state,
        crate::runtime::subagent::SubagentState::PublishingTerminal
    );
    assert_eq!(
        unresolved.settlement.publication,
        crate::runtime::subagent::SubagentPublication::Abandoned
    );
    // Issue #178: the successful answer never rides the live read
    // model, not even while its publication is unresolved; the
    // candidate remains observable through the PublishingTerminal
    // lifecycle state itself.
    assert!(unresolved.detail.is_none());
    let pending_items = store
        .select_pending_batch()
        .expect("pending")
        .map(|batch| batch.items)
        .unwrap_or_default();
    assert!(
        !pending_items.iter().any(|item| {
            item.correlation.as_deref()
                == Some(
                    crate::runtime::subagent::terminal_correlation(&accepted.subagent_id).as_str(),
                )
        }),
        "no false terminal inbound was committed"
    );
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));

    admission_gate.release();
    let shutdown = runtime.shutdown().await;
    assert!(matches!(
        shutdown,
        Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement { detail })
            if detail.contains("subagent")
    ));
}

/// DurabilityFailed-first for subagents (Issue #60): once the owning
/// runtime commits `DurabilityFailed` through the real subagent
/// terminal-publication exhaustion path, a new subagent start is
/// refused at the runtime durability frontier — the commit rejects
/// typed, the staged child is torn down conclusively, no
/// `SubagentOwnershipCommitted` fact and no Running record exist, no
/// Delegate is ever sent, and the existing unresolved candidate stays
/// unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn a_durability_failed_runtime_rejects_new_subagent_ownership() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_95189f41-6174-741a-8c26-0134828bfe76",
        ))
        .expect("in-memory store"),
    );
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_95189f41-6174-741a-8c26-0134828bfe76",
        store.clone(),
        Some(admission_gate.clone()),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    let _admission_release = admission_gate.arm_scoped();
    runtime
        .submit_inbound(text_content("hold admission worker"))
        .expect("hold inbound");
    within_liveness_guard(
        "subagent runtime admission worker gate",
        tokio::task::spawn_blocking({
            let admission_gate = admission_gate.clone();
            move || admission_gate.wait_entered()
        }),
    )
    .await
    .expect("admission gate task");

    // One owned child exhausts its bounded terminal-publication budget
    // and degrades the owning runtime through the real sink path.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
    subagents.push_staged_override(staged);
    let owned = match subagents
        .commit(
            subagents
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            title: "Explore task".into(),
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            attachments: Vec::new(),
                            task: "owned child".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-owned"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare owned"),
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("commit owned")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
            panic!("unexpected rollback")
        }
    };
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    store.arm_fail_accept_times(3);
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &crate::runtime::subagent::ipc::ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("owned answer".to_owned()),
                diagnostic: None,
            },
        ),
    )
    .await
    .expect("owned result");
    let observations = await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "subagent_terminal_publication"
        )
    })
    .await;
    assert_eq!(
        observations
            .iter()
            .filter(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            ))
            .count(),
        1,
        "the exhausted budget degraded the runtime exactly once"
    );
    assert!(
        runtime.inner.durability_failure_diagnostic().is_some(),
        "the runtime committed DurabilityFailed"
    );

    // A new subagent start after the failure is refused at the runtime
    // durability frontier: `prepare` still stages privately, `commit`
    // rejects typed at the gate, and the staged child is rolled back
    // conclusively (the returned error is the typed rejection, never a
    // rollback failure).
    let (staged, _rejected_peer) = stage_runtime_test_child(&dir.path().join("rejected-child"));
    subagents.push_staged_override(staged);
    let prepared = subagents
        .prepare(
            &crate::runtime::subagent::SubagentStartSpec {
                authority: crate::runtime::subagent::DurableAgentAuthority {
                    title: "Explore task".into(),
                    execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
                    resolved: test_resolved_subagent("explore"),
                    approval_mode: crate::runtime::ApprovalMode::Policy,
                },
                admission: crate::runtime::subagent::ActivationAdmission {
                    attachments: Vec::new(),
                    task: "rejected after failure".to_owned(),
                    context: None,
                    origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                        tool_call_id: ToolCallId::new("call-rejected"),
                    },
                    terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                },
            },
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("prepare rejected");
    let error = subagents
        .commit(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect_err("ownership refused after DurabilityFailed");
    assert!(
        matches!(
            error,
            crate::runtime::subagent::SubagentStartError::DurabilityFailed { .. }
        ),
        "the typed rejection names the runtime durability failure: {error}"
    );

    // No new durable ownership fact, no new Running record, no
    // Delegate: exactly the one owned child remains, its unresolved
    // candidate unchanged.
    let rejected_id = SubagentId::for_conversation(
        &ConversationId::new("conv_95189f41-6174-741a-8c26-0134828bfe76"),
        2,
    );
    let journal = store.read_events(None, 128).expect("events").events;
    assert!(
        !journal.iter().any(|envelope| matches!(
            &envelope.event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                ..
            } if *subagent_id == rejected_id
        )),
        "no ownership fact for the rejected child"
    );
    let snapshots = subagents.all_snapshots();
    assert_eq!(snapshots.len(), 1, "only the owned child remains");
    assert_eq!(snapshots[0].subagent_id, owned.subagent_id);
    let unresolved = subagents
        .wait_until_settled(&owned.subagent_id)
        .await
        .expect("owned settlement");
    assert_eq!(
        unresolved.state,
        crate::runtime::subagent::SubagentState::PublishingTerminal
    );
    assert_eq!(
        unresolved.settlement.publication,
        crate::runtime::subagent::SubagentPublication::Abandoned
    );
    // Issue #178: the successful answer never rides the live read
    // model; the unresolved candidate is observable through its
    // PublishingTerminal lifecycle state, unchanged.
    assert!(unresolved.detail.is_none());

    admission_gate.release();
}

/// Ownership-first for subagents (Issue #60): a child whose ownership
/// committed while the runtime was healthy survives a later
/// `DurabilityFailed` commit — it is not retroactively reclaimed, and
/// it keeps its full settlement authority (cancel, escalate, reap,
/// durable terminal publication).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owned_subagent_survives_a_later_durability_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
        store.clone(),
        None,
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // Ownership commits first, while the runtime is healthy.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
    subagents.push_staged_override(staged);
    let accepted = match subagents
        .commit(
            subagents
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            title: "Explore task".into(),
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            attachments: Vec::new(),
                            task: "owned".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-owned"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare"),
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("commit")
    {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => panic!("accepted"),
    };
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));

    assert!(
        runtime.idle_epoch().is_err(),
        "an owned Subagent prevents idle residency even without a parent attempt"
    );

    assert_pending_mutation_preserves_child(&runtime, &subagents, &accepted.subagent_id);

    // The runtime then commits DurabilityFailed; the already-owned
    // child is not retroactively reclaimed.
    runtime.force_durability_failure_for_test(
        DurableOperation::EventJournal,
        "synthetic durability failure",
    );
    await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    })
    .await;
    let snapshot = subagents
        .snapshot(&accepted.subagent_id)
        .expect("owned snapshot");
    assert_eq!(
        snapshot.state,
        crate::runtime::subagent::SubagentState::Running,
        "ownership survives the later failure"
    );

    // The owned child keeps its settlement authority: cancel reaches
    // the driver, the process is escalated and reaped, and the durable
    // terminal publication still succeeds (settlement is not
    // new-mutation authority).
    let _ = subagents.cancel(&accepted.subagent_id, CancellationReason::UserRequested);
    let settled = subagents
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(
        settled.state,
        crate::runtime::subagent::SubagentState::Cancelled
    );
}

/// The exact ownership-vs-health race for subagents (Issue #60): the
/// ownership commit is parked inside its authoritative critical section
/// (holding the runtime durability frontier across its durable write
/// and record publication) while the `DurabilityFailed` commit is
/// invoked concurrently; the failure fact waits on the frontier without blocking the coordinator,
/// the ownership completes first, and only then does the failure
/// linearize. The owned child survives with its settlement authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn subagent_ownership_racing_a_durability_failure_linearizes_on_the_runtime_frontier() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_aed9850e-213c-7f34-8a3f-fb60527fc81f",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_aed9850e-213c-7f34-8a3f-fb60527fc81f",
        store.clone(),
        None,
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // The ownership commit is parked inside its authoritative critical
    // section (the CommitBoundaryHook), provably holding the runtime
    // durability frontier across the durable write and record
    // publication.
    let hook = Arc::new(crate::runtime::subagent::CommitBoundaryHook::default());
    subagents.install_commit_boundary_hook(hook.clone());
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("racing-child"));
    subagents.push_staged_override(staged);
    let commit_registry = subagents.clone();
    let committer = tokio::spawn(async move {
        let prepared = commit_registry
            .prepare(
                &crate::runtime::subagent::SubagentStartSpec {
                    authority: crate::runtime::subagent::DurableAgentAuthority {
                        title: "Explore task".into(),
                        execution_policy:
                            crate::runtime::subagent::InheritedExecutionPolicy::default(),
                        resolved: test_resolved_subagent("explore"),
                        approval_mode: crate::runtime::ApprovalMode::Policy,
                    },
                    admission: crate::runtime::subagent::ActivationAdmission {
                        attachments: Vec::new(),
                        task: "racing".to_owned(),
                        context: None,
                        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("call-racing"),
                        },
                        terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                    },
                },
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("prepare");
        commit_registry
            .commit(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking({
            let hook = hook.clone();
            move || hook.wait_until_entered()
        }),
    )
    .await
    .expect("commit-boundary liveness")
    .expect("commit-boundary entered");

    // The DurabilityFailed commit is now invoked concurrently: it takes
    // the coordinator lock and records its intent behind the frontier held by
    // the parked ownership commit.
    let (failed_started_tx, failed_started_rx) = std::sync::mpsc::channel();
    let (failed_done_tx, failed_done_rx) = std::sync::mpsc::channel();
    let failing_runtime = runtime.clone();
    let failure_thread = std::thread::spawn(move || {
        failed_started_tx.send(()).expect("failure-started channel");
        failing_runtime.force_durability_failure_for_test(
            DurableOperation::EventJournal,
            "synthetic durability failure",
        );
        failed_done_tx.send(()).expect("failure-done channel");
    });
    failed_started_rx
        .recv()
        .expect("failure is invoked while the ownership is parked");
    let failure_registered = failed_done_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .is_ok();
    let failure_published = pending.drain_fixture_facts().iter().any(|observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    });

    // Capture both facts while parked, then release before asserting so
    // a regression reports its failure instead of stranding the worker.
    hook.release();
    assert!(
        failure_registered,
        "failure registration returns without parking the coordinator while its committed fact waits for ownership exit"
    );
    assert!(
        !failure_published,
        "the health failure has not linearized while the ownership commit holds the frontier"
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
        .await
        .expect("commit liveness")
        .expect("committer")
        .expect("ownership wins the frontier");
    let accepted = match outcome {
        crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
        crate::runtime::subagent::SubagentStartOutcome::RolledBack => panic!("accepted"),
    };
    await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    })
    .await;
    failure_thread.join().expect("failure thread joins");

    // The owned child exists with a durable ownership fact and keeps
    // its settlement authority.
    let snapshot = subagents
        .snapshot(&accepted.subagent_id)
        .expect("owned snapshot");
    assert_eq!(
        snapshot.state,
        crate::runtime::subagent::SubagentState::Running
    );
    let journal = store.read_events(None, 128).expect("events").events;
    assert!(
        journal.iter().any(|envelope| matches!(
            &envelope.event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                ..
            } if *subagent_id == accepted.subagent_id
        )),
        "the ownership durable fact committed before the failure"
    );
    let _ = subagents.cancel(&accepted.subagent_id, CancellationReason::UserRequested);
    let settled = subagents
        .wait_until_settled(&accepted.subagent_id)
        .await
        .expect("settled");
    assert_eq!(
        settled.state,
        crate::runtime::subagent::SubagentState::Cancelled
    );
    // The wire carried Delegate first, then the in-flight Cancel.
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("driver frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("driver frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Cancel {
            reason: Some(crate::runtime::types::CancellationReason::UserRequested),
        })
    ));
}

/// DurabilityFailed-first for background (Issue #60): once the owning
/// runtime commits `DurabilityFailed`, a new background ownership
/// commit is refused at the runtime durability frontier — the prepared
/// runner is aborted, no record and no durable fact exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durability_failed_runtime_rejects_new_background_ownership() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_a2701eac-eb5d-78a0-8494-e6888fb54e26",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_a2701eac-eb5d-78a0-8494-e6888fb54e26",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // The runtime commits DurabilityFailed first.
    runtime.force_durability_failure_for_test(
        DurableOperation::EventJournal,
        "synthetic durability failure",
    );
    await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    })
    .await;

    // A new background ownership commit is refused at the runtime
    // frontier: the prepared runner is aborted, no committed record and
    // no durable ownership fact exist.
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> =
        Arc::new(GatedBackgroundExecutor::new().0);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("call-rejected-bg"),
        },
        tool_id: ToolId::new("tool-bash"),
        tool_name: "bash".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let error = runtime
        .tool_runtime()
        .background()
        .commit_dispatch(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .expect_err("background ownership refused after DurabilityFailed");
    assert!(
        matches!(
            error,
            crate::tools::background::BackgroundDispatchError::DurabilityFailed { .. }
        ),
        "the typed rejection names the runtime durability failure: {error}"
    );
    assert!(
        runtime
            .tool_runtime()
            .background()
            .all_snapshots()
            .is_empty(),
        "no committed background record"
    );
    assert!(
        store
            .read_events(None, 128)
            .expect("events")
            .events
            .iter()
            .all(|envelope| !matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::BackgroundExecutionCommitted { .. }
            )),
        "no background ownership durable fact"
    );
}

/// Ownership-first for background (Issue #60): an execution whose
/// ownership committed while the runtime was healthy survives a later
/// `DurabilityFailed` commit and keeps its settlement authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owned_background_execution_survives_a_later_durability_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_e24a334b-c1b2-7efc-8376-f688cf4206bf",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_e24a334b-c1b2-7efc-8376-f688cf4206bf",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // Ownership commits first while the runtime is healthy.
    let (executor, mut started, release) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let execution_id = commit_background(&runtime, &executor, "call-owned-bg");
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("runner start wait exceeded liveness guard")
    .expect("start channel stays open");

    // The runtime then commits DurabilityFailed; the already-owned
    // execution is not reclaimed.
    runtime.force_durability_failure_for_test(
        DurableOperation::EventJournal,
        "synthetic durability failure",
    );
    await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    })
    .await;
    let snapshot = runtime
        .background_status(&execution_id)
        .expect("owned execution");
    assert_eq!(
        snapshot.state,
        crate::tools::background::BackgroundLifecycle::Running,
        "ownership survives the later failure"
    );

    // Settlement still works: the runner completes and its terminal
    // publication is durably accepted (settlement is not new-mutation
    // authority).
    release.send_replace(true);
    runtime
        .tool_runtime()
        .background()
        .wait_until_settled(&execution_id)
        .await;
    let terminal = runtime
        .background_status(&execution_id)
        .expect("settled execution");
    assert_eq!(
        terminal.state,
        crate::tools::background::BackgroundLifecycle::Succeeded
    );
}

/// The exact ownership-vs-health race for background (Issue #60): the
/// background ownership commit is parked inside its authoritative
/// critical section (holding the runtime durability frontier) while the
/// `DurabilityFailed` commit is invoked concurrently; the failure
/// provably blocks on the frontier, the ownership completes first, and
/// only then does the failure linearize. The owned execution survives
/// and settles normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn background_ownership_racing_a_durability_failure_linearizes_on_the_runtime_frontier() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_80c3ae25-956f-72bf-8e7b-a38546997137",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_80c3ae25-956f-72bf-8e7b-a38546997137",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // The background ownership commit is parked inside its
    // authoritative critical section (the dispatch CommitBoundaryHook),
    // provably holding the runtime durability frontier across the
    // durable write and record publication.
    let background = runtime.tool_runtime().background().clone();
    let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
    background.install_commit_boundary_hook(hook.clone());
    let (executor, mut started, release) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("call-racing-bg"),
        },
        tool_id: ToolId::new("tool-bash"),
        tool_name: "bash".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = background
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let commit_background_registry = background.clone();
    let committer = tokio::task::spawn_blocking(move || {
        commit_background_registry.commit_dispatch(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
    });
    {
        let hook = hook.clone();
        tokio::task::spawn_blocking(move || hook.wait_entered())
            .await
            .expect("the background commit entered its ownership boundary");
    }

    // The DurabilityFailed commit is now invoked concurrently: it takes
    // the coordinator lock and records its intent behind the frontier held by
    // the parked ownership commit.
    let (failed_started_tx, failed_started_rx) = std::sync::mpsc::channel();
    let (failed_done_tx, failed_done_rx) = std::sync::mpsc::channel();
    let failing_runtime = runtime.clone();
    let failure_thread = std::thread::spawn(move || {
        failed_started_tx.send(()).expect("failure-started channel");
        failing_runtime.force_durability_failure_for_test(
            DurableOperation::EventJournal,
            "synthetic durability failure",
        );
        failed_done_tx.send(()).expect("failure-done channel");
    });
    failed_started_rx
        .recv()
        .expect("failure is invoked while the ownership is parked");
    assert!(
        failed_done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .is_ok(),
        "failure registration returns without parking the coordinator while its committed fact waits for ownership exit"
    );
    assert!(
        !pending
            .drain_fixture_facts()
            .iter()
            .any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
        "the health failure has not linearized while the ownership commit holds the frontier"
    );

    // Release the ownership commit: it completes its durable write and
    // record publication first, then the failure commit linearizes.
    {
        let hook = hook.clone();
        tokio::task::spawn_blocking(move || hook.proceed())
            .await
            .expect("the commit boundary was released");
    }
    let outcome = committer
        .await
        .expect("commit outcome")
        .expect("ownership wins the frontier");
    let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("accepted");
    };
    await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    })
    .await;
    failure_thread.join().expect("failure thread joins");

    // The runner-owned start boundary transitions the durable ownership
    // record from Starting to Running immediately before executor start.
    // Wait on that explicit watch rather than sampling a scheduler race.
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("runner start wait exceeded liveness guard")
    .expect("start channel stays open");

    // The owned execution exists with a durable ownership fact and
    // keeps its settlement authority.
    let snapshot = runtime
        .background_status(&execution_id)
        .expect("owned execution");
    assert_eq!(
        snapshot.state,
        crate::tools::background::BackgroundLifecycle::Running
    );
    let journal = store.read_events(None, 128).expect("events").events;
    assert!(
        journal.iter().any(|envelope| matches!(
            &envelope.event,
            crate::events::types::RuntimeEvent::BackgroundExecutionCommitted {
                execution_id: committed_id,
                ..
            } if *committed_id == execution_id
        )),
        "the ownership durable fact committed before the failure"
    );
    release.send_replace(true);
    runtime
        .tool_runtime()
        .background()
        .wait_until_settled(&execution_id)
        .await;
    let terminal = runtime
        .background_status(&execution_id)
        .expect("settled execution");
    assert_eq!(
        terminal.state,
        crate::tools::background::BackgroundLifecycle::Succeeded
    );
}

/// Issue #63 (Blocker 2, owning-runtime level): when the background
/// settlement owner's bounded terminal-publication budget is exhausted,
/// the owning runtime is placed into the explicit `DurabilityFailed`
/// state through the narrow failure seam installed at construction —
/// the unresolved candidate stays retained and observable in the
/// registry, and the runtime never claims false healthy progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn exhausted_background_publication_degrades_the_owning_runtime() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_829c626d-2692-7096-81f2-912d7383be41",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_829c626d-2692-7096-81f2-912d7383be41",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // Dispatch one background execution through the authoritative
    // registry of the runtime's tool runtime.
    let (executor, mut started, release) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new("call-1"),
        },
        tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        runtime
            .tool_runtime()
            .background()
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect("commit")
    else {
        panic!("accepted");
    };
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        started.wait_for(|is_started| *is_started),
    )
    .await
    .expect("runner start wait exceeded liveness guard")
    .expect("start channel stays open");

    // Arm exactly the bounded publication budget (two acceptance
    // faults), then release the runner: both production publication
    // attempts fail, and the runner reports the exhausted budget
    // through the failure seam.
    store.arm_fail_accept_times(2);
    release.send_replace(true);
    let observations = await_observation(pending.as_ref(), |o| {
        matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "background_terminal_publication")
    })
    .await;
    let failures = observations
        .iter()
        .filter(|o| matches!(o, ConversationObservation::DurabilityFailed { .. }))
        .count();
    assert_eq!(
        failures, 1,
        "the exhausted budget degraded the runtime exactly once — no hot loop"
    );

    // The unresolved terminal candidate stays retained and observable;
    // no false terminal publication exists; the runtime refuses new
    // durable work typed.
    let snapshot = runtime
        .background_status(&execution_id)
        .expect("execution record");
    assert_eq!(
        snapshot.state,
        crate::tools::background::BackgroundLifecycle::PublishingTerminal
    );
    assert!(
        snapshot.result.is_some(),
        "the unresolved terminal candidate remains retained and observable"
    );
    assert!(
        store.load_pending().expect("load pending").is_empty(),
        "no false durable terminal inbound committed"
    );
    assert!(matches!(
        runtime.submit_inbound(text_content("late")),
        Err(InboundAdmissionError::DurabilityFailed { .. })
    ));

    // M9c: shutdown does not wait forever for the intentionally retained
    // PublishingTerminal record. The owner has entered its explicit
    // durability-failure state, so shutdown returns a typed settlement
    // failure and the runtime remains Draining rather than claiming
    // false quiescence.
    let shutdown = runtime.shutdown().await;
    assert!(matches!(
        shutdown,
        Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
            detail
        }) if detail.contains("terminal publication")
    ));
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );
}

/// Every reader of the runtime's durability failure observes the **one
/// authoritative absorbing fact** (Issue #60 single-source-of-truth):
/// after the real subagent terminal-publication exhaustion commits
/// `DurabilityFailed`, the gate's fact, the published observation, the
/// coordinator's fail-closed rejections (`submit_inbound`, `model_set`),
/// the subagent and background ownership refusals, and the shutdown
/// diagnostic all report the same operation and diagnostic. There is no
/// second failed-state storage anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn every_reader_of_the_durability_failure_observes_the_single_authoritative_fact() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_042447de-8629-7a98-887b-c9fa571c7cc9",
        ))
        .expect("in-memory store"),
    );
    let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
        &dir,
        "conv_042447de-8629-7a98-887b-c9fa571c7cc9",
        store.clone(),
        Some(admission_gate.clone()),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    admission_gate.arm();
    runtime
        .submit_inbound(text_content("hold admission worker"))
        .expect("hold inbound");
    within_liveness_guard(
        "subagent runtime admission worker gate",
        tokio::task::spawn_blocking({
            let admission_gate = admission_gate.clone();
            move || admission_gate.wait_entered()
        }),
    )
    .await
    .expect("admission gate task");

    // The real production failure path: one owned child exhausts its
    // bounded terminal-publication budget.
    let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
    subagents.push_staged_override(staged);
    let outcome = subagents
        .commit(
            subagents
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            title: "Explore task".into(),
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            attachments: Vec::new(),
                            task: "owned".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-owned"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare"),
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("commit");
    assert!(matches!(
        outcome,
        crate::runtime::subagent::SubagentStartOutcome::Accepted(_)
    ));
    assert!(matches!(
        crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
            .await
            .expect("delegate frame"),
        Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
    ));
    store.arm_fail_accept_times(3);
    crate::runtime::subagent::ipc::write_child_frame(
        &mut peer,
        &crate::runtime::subagent::ipc::ChildFrame::Result(
            crate::runtime::subagent::ipc::ResultFrame {
                status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                content: Some("answer".to_owned()),
                diagnostic: None,
            },
        ),
    )
    .await
    .expect("result");
    let observations = await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "subagent_terminal_publication"
        )
    })
    .await;

    // The one authoritative fact: stored in the runtime-owned gate, and
    // only there.
    let failure = runtime
        .inner
        .durability_gate
        .failure()
        .expect("the authoritative absorbing fact");
    assert_eq!(
        failure.operation,
        DurableOperation::SubagentTerminalPublication
    );
    assert!(!failure.diagnostic.is_empty());
    let observed = observations
        .iter()
        .find_map(|observation| match observation {
            ConversationObservation::DurabilityFailed {
                operation,
                diagnostic,
            } => Some((operation.as_str(), diagnostic.as_str())),
            _ => None,
        })
        .expect("the DurabilityFailed observation");
    assert_eq!(observed.0, failure.operation.as_str());
    assert_eq!(observed.1, failure.diagnostic.as_str());
    assert_eq!(
        runtime.inner.durability_failure_diagnostic().as_deref(),
        Some(failure.diagnostic.as_str())
    );

    // The coordinator's fail-closed rejections read the same fact.
    match runtime.submit_inbound(text_content("late")) {
        Err(InboundAdmissionError::DurabilityFailed { message }) => {
            assert_eq!(message, failure.diagnostic);
        }
        other => {
            panic!("submit_inbound must refuse with the authoritative diagnostic: {other:?}")
        }
    }
    match runtime.model_set(crate::model::session::SessionModelConfig::of(
        serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
    )) {
        Err(ModelUpdateError::DurabilityFailed { message }) => {
            assert_eq!(message, failure.diagnostic);
        }
        other => panic!("model_set must refuse with the authoritative diagnostic: {other:?}"),
    }

    // The detached ownership planes read the same fact.
    let (staged, _rejected_peer) = stage_runtime_test_child(&dir.path().join("rejected-child"));
    subagents.push_staged_override(staged);
    let prepared = subagents
        .prepare(
            &crate::runtime::subagent::SubagentStartSpec {
                authority: crate::runtime::subagent::DurableAgentAuthority {
                    title: "Explore task".into(),
                    execution_policy: crate::runtime::subagent::InheritedExecutionPolicy::default(),
                    resolved: test_resolved_subagent("explore"),
                    approval_mode: crate::runtime::ApprovalMode::Policy,
                },
                admission: crate::runtime::subagent::ActivationAdmission {
                    attachments: Vec::new(),
                    task: "rejected".to_owned(),
                    context: None,
                    origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                        tool_call_id: ToolCallId::new("call-rejected"),
                    },
                    terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                },
            },
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
        .expect("prepare");
    match subagents
        .commit(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        )
        .await
    {
        Err(crate::runtime::subagent::SubagentStartError::DurabilityFailed { detail }) => {
            assert_eq!(detail, failure.diagnostic);
        }
        other => {
            panic!("subagent ownership must refuse with the authoritative diagnostic: {other:?}")
        }
    }
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> =
        Arc::new(GatedBackgroundExecutor::new().0);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("call-rejected-bg"),
        },
        tool_id: ToolId::new("tool-bash"),
        tool_name: "bash".into(),
        mode: crate::tools::types::ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let prepared = runtime
        .tool_runtime()
        .background()
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    match runtime.tool_runtime().background().commit_dispatch(
        prepared,
        &crate::runtime::cancellation::CancellationSignal::new(),
    ) {
        Err(crate::tools::background::BackgroundDispatchError::DurabilityFailed { detail }) => {
            assert_eq!(detail, failure.diagnostic);
        }
        other => {
            panic!("background ownership must refuse with the authoritative diagnostic: {other:?}")
        }
    }

    admission_gate.release();
}

/// The absorbing winner is never replaced (Issue #60 single
/// source-of-truth): the first committed durability failure stays the
/// authoritative fact — a second failure with a *different* operation
/// and diagnostic neither rewrites the stored operation/diagnostic nor
/// publishes a second `DurabilityFailed` observation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_durability_failure_never_replaces_the_absorbing_first() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_ab84aa64-dcd1-7c9b-8c79-ca11323ae1a6",
        ))
        .expect("in-memory store"),
    );
    let (runtime, _model) = headless_runtime_over_store(
        &dir,
        "conv_ab84aa64-dcd1-7c9b-8c79-ca11323ae1a6",
        store.clone(),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    // Failure A commits first through the single production commit seam.
    runtime.force_durability_failure_for_test(
        DurableOperation::EventJournal,
        "first absorbing failure",
    );
    let observations = await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )
    })
    .await;
    assert_eq!(
        observations
            .iter()
            .filter(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            ))
            .count(),
        1,
        "the first commit publishes exactly one DurabilityFailed observation"
    );
    let failure = runtime
        .inner
        .durability_gate
        .failure()
        .expect("the authoritative absorbing fact");
    assert_eq!(failure.operation, DurableOperation::EventJournal);
    assert_eq!(failure.diagnostic, "first absorbing failure");

    // Failure B arrives later through the same seam, with a genuinely
    // different operation and diagnostic: the guard observes the
    // absorbing winner and neither rewrites the fact nor publishes a
    // second observation.
    runtime.force_durability_failure_for_test(
        DurableOperation::SubagentTerminalPublication,
        "second failure must not win",
    );
    let failure = runtime
        .inner
        .durability_gate
        .failure()
        .expect("the authoritative absorbing fact");
    assert_eq!(
        failure.operation,
        DurableOperation::EventJournal,
        "the first winner stays the absorbing operation"
    );
    assert_eq!(
        failure.diagnostic, "first absorbing failure",
        "the first winner stays the absorbing diagnostic"
    );
    assert!(
        !pending
            .drain_fixture_facts()
            .iter()
            .any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
        "no second DurabilityFailed observation is published"
    );
    assert_eq!(
        runtime.inner.durability_failure_diagnostic().as_deref(),
        Some("first absorbing failure"),
        "the shutdown diagnostic reads the same absorbing fact"
    );
    // The single authoritative fact is still exactly one: there is no
    // second storage to disagree.
    assert_eq!(
        runtime.inner.durability_gate.failure().map(|f| f.operation),
        Some(DurableOperation::EventJournal)
    );
}

/// M9c (Blocker A): a recorded durability failure is an error **fact**,
/// never permission to stop supervising a sibling owner. One background
/// execution exhausts its bounded terminal-publication budget while a
/// provider turn is still parked; drain must keep supervising the live
/// provider and may return its aggregated settlement failure only after
/// the Agent Loop has crossed its explicit settlement barrier.
///
/// Happens-before: the model-arbitration pause holds the started provider
/// stream immediately before the next provider/cancellation arbitration.
/// `drain_supervision` then fires only from inside the drain task, so
/// observing it proves drain reached supervision *with the durability
/// failure already recorded* instead of short-circuiting; the
/// current-attempt slot is still occupied at that instant, and the
/// shutdown result channel is still empty. Releasing the exact
/// arbitration barrier lets the already-requested cancellation settle the
/// attempt, and only then does shutdown return.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn durability_failure_never_abandons_a_live_provider_turn() {
    use crate::agent::execution::test_sync::ModelArbitrationPause;

    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_d2d949a2-cff1-7341-8c06-4df386858aaa",
        ))
        .expect("in-memory store"),
    );
    let (_release_tx, release_rx) = crate::scripted_suites::support::fake::model_release();
    let script = vec![
        FakeStep::Emit(crate::model::event::ModelEvent::Started),
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
    let (model_pause, mut model_pause_reached, model_pause_release) =
        ModelArbitrationPause::install(1);
    let drain_supervision = Arc::new(tokio::sync::Notify::new());
    let (runtime, _model) = headless_runtime_over_store_with_policy(
        &dir,
        "conv_d2d949a2-cff1-7341-8c06-4df386858aaa",
        store.clone(),
        vec![script],
        Some(CoordinatorProbe {
            drain_supervision: Some(drain_supervision.clone()),
            model_arbitration_pause: Some(model_pause),
            ..CoordinatorProbe::default()
        }),
        crate::model::ModelTimeoutPolicy::new(
            std::time::Duration::from_mins(5),
            std::time::Duration::from_mins(5),
        ),
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
    // The provider has emitted Started and the Agent Loop now owns its
    // open stream, but the next provider/cancellation arbitration is
    // held behind an exact test barrier. Shutdown can therefore request
    // cancellation without racing the attempt to settle before drain
    // begins supervision.
    model_pause_reached
        .wait_for(|is_reached| *is_reached)
        .await
        .expect("model arbitration pause channel stays open");

    // A *different* owner records the durability failure: one background
    // execution spends its whole bounded terminal-publication budget.
    let (executor, mut started, release_background) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let execution_id = commit_background(&runtime, &executor, "call-degrade");
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("start channel stays open");
    store.arm_fail_accept_times(2);
    release_background.send_replace(true);
    await_observation(pending.as_ref(), |observation| {
        matches!(observation, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "background_terminal_publication")
    })
    .await;
    assert_eq!(
        runtime
            .background_status(&execution_id)
            .expect("record")
            .state,
        crate::tools::background::BackgroundLifecycle::PublishingTerminal
    );

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    // Drain reached supervision *despite* the recorded durability
    // failure. The old fail-fast drain returned before this point.
    within_liveness_guard(
        "drain to park on the live provider turn",
        drain_supervision.notified(),
    )
    .await;
    assert!(
        runtime.has_current_attempt(),
        "the provider turn is still runtime-owned when drain begins supervising"
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "a sibling's durability failure must not end supervision of a live provider turn"
    );
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining
    );

    // Release the exact post-Started arbitration. The already-requested
    // cancellation then wins the next Agent Loop arbitration while the
    // fake provider stream is still pending, and the attempt settles.
    let _ = model_pause_release.send(());
    let shutdown = done_rx.await.expect("shutdown result channel");
    assert!(
        matches!(
            &shutdown,
            Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                detail
            }) if detail.contains("terminal publication")
        ),
        "the unresolved terminal publication is honest settlement evidence: {shutdown:?}"
    );
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining,
        "unproven terminality never publishes Quiescent"
    );
    assert!(
        !runtime.has_current_attempt(),
        "the supervised provider turn settled before shutdown returned"
    );
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
        "the supervised attempt reached its terminal cancellation"
    );
}

/// M9c (Blocker A / 4.1): one background record's failed terminal
/// publication must not release the supervisor from a *sibling*
/// background execution that is still physically running.
///
/// Happens-before: the failing record's `DurabilityFailed` observation is
/// awaited first, so the failure is provably recorded before shutdown
/// starts. `drain_supervision` then proves drain entered supervision with
/// that failure already known while the sibling is still active, and the
/// sibling's own explicit release is the only thing that lets shutdown
/// return.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn durability_failure_never_abandons_a_sibling_background_execution() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_d498b02e-ce4d-7e66-8b25-a4466a0e3a8b",
        ))
        .expect("in-memory store"),
    );
    let drain_supervision = Arc::new(tokio::sync::Notify::new());
    let (runtime, _model) = headless_runtime_over_store_with(
        &dir,
        "conv_d498b02e-ce4d-7e66-8b25-a4466a0e3a8b",
        store.clone(),
        vec![one_turn_script()],
        Some(CoordinatorProbe {
            drain_supervision: Some(drain_supervision.clone()),
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();

    let (failing, mut failing_started, release_failing) = GatedBackgroundExecutor::new();
    let failing: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(failing);
    let failing_id = commit_background(&runtime, &failing, "call-failing");
    let (sibling, mut sibling_started, release_sibling) = GatedBackgroundExecutor::new();
    let sibling: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(sibling);
    let sibling_id = commit_background(&runtime, &sibling, "call-sibling");
    failing_started
        .wait_for(|is_started| *is_started)
        .await
        .expect("start channel stays open");
    sibling_started
        .wait_for(|is_started| *is_started)
        .await
        .expect("start channel stays open");

    // Exactly the failing record's bounded publication budget.
    store.arm_fail_accept_times(2);
    release_failing.send_replace(true);
    await_observation(pending.as_ref(), |observation| {
        matches!(observation, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "background_terminal_publication")
    })
    .await;

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    within_liveness_guard(
        "drain to park on the sibling background execution",
        drain_supervision.notified(),
    )
    .await;
    assert_eq!(
        runtime
            .background_status(&sibling_id)
            .expect("sibling record")
            .state,
        crate::tools::background::BackgroundLifecycle::Cancelling,
        "the sibling received drain cancellation and is still owned"
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the failed record must not release the supervisor from the live sibling"
    );

    release_sibling.send_replace(true);
    let shutdown = done_rx.await.expect("shutdown result channel");
    assert!(
        matches!(
            &shutdown,
            Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                detail
            }) if detail.contains("terminal publication")
        ),
        "the failed record is still reported: {shutdown:?}"
    );
    // The sibling was supervised to its own terminal boundary and its
    // terminal publication went through the one Pending Inbound path.
    assert!(
        runtime
            .background_status(&sibling_id)
            .expect("sibling record")
            .state
            .is_terminal(),
        "the sibling reached its terminal state under supervision"
    );
    assert_eq!(
        runtime
            .background_status(&failing_id)
            .expect("failing record")
            .state,
        crate::tools::background::BackgroundLifecycle::PublishingTerminal,
        "the unresolved candidate stays explicit, never fabricated terminal"
    );
    let pending_items = store.load_pending().expect("pending inbound");
    assert!(
        pending_items
            .iter()
            .any(|item| { format!("{item:?}").contains(sibling_id.as_str()) }),
        "the supervised sibling published its terminal inbound durably"
    );
    assert!(
        !pending_items
            .iter()
            .any(|item| format!("{item:?}").contains(failing_id.as_str())),
        "no false terminal inbound exists for the unresolved record"
    );
}

/// M9c (settlement linearization): a background terminal-publication
/// failure is not logically settled until the runner has completed its
/// **last conversation-facing failure callback**. `publication_abandoned`
/// is the fact runtime drain consumes as this owner's settlement, so it
/// must never become observable while the failure sink can still call
/// back into the conversation — otherwise drain could aggregate the
/// abandoned evidence and cache a failed shutdown *before* the runner
/// published its `DurabilityFailed` observation.
///
/// This regression races shutdown against the failure sink itself, which
/// the sibling-background regression deliberately does not: there the
/// `DurabilityFailed` observation is awaited *before* shutdown starts, so
/// the callback is already complete.
///
/// Happens-before: `background_failure_gate` parks the runner inside
/// `BackgroundFailureSink::terminal_publication_failed`, before the
/// coordinator lock and before the durability-health mutation. While it
/// is parked, the callback has provably started and provably not
/// returned. `drain_linearization` then proves `Running -> Draining`
/// committed and `drain_supervision` proves the drain task is committed
/// to awaiting this exact owner — yet neither shutdown caller may return.
/// Releasing the gate is the only thing that lets the failure become
/// observable, then the abandoned fact, then the shutdown failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn abandoned_publication_never_precedes_the_last_failure_callback() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_414701e8-d767-76d9-88db-c5d4e270cf81",
        ))
        .expect("in-memory store"),
    );
    let background_failure_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    background_failure_gate.arm();
    let drain_linearization = Arc::new(tokio::sync::Notify::new());
    let drain_supervision = Arc::new(tokio::sync::Notify::new());
    let (runtime, _model) = headless_runtime_over_store_with(
        &dir,
        "conv_414701e8-d767-76d9-88db-c5d4e270cf81",
        store.clone(),
        vec![one_turn_script()],
        Some(CoordinatorProbe {
            background_failure_gate: Some(background_failure_gate.clone()),
            subagent_failure_published_gate: None,
            drain_linearization: Some(drain_linearization.clone()),
            drain_supervision: Some(drain_supervision.clone()),
            ..CoordinatorProbe::default()
        }),
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .expect("bridge");
    runtime.activate();
    let background = runtime.tool_runtime().background().clone();

    let (executor, mut started, release_executor) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let execution_id = commit_background(&runtime, &executor, "call-abandon-order");
    started
        .wait_for(|is_started| *is_started)
        .await
        .expect("start channel stays open");

    // Exactly the bounded publication budget: durable attempt #1 (inside
    // `finish`) and durable attempt #2 (the one registry-owned retry).
    store.arm_fail_accept_times(2);
    release_executor.send_replace(true);

    // (5)+(6) The runner entered its last conversation-facing callback
    // and is parked *inside* it. The park is the callback boundary
    // itself, so from here the callback has provably started and
    // provably not returned.
    background_failure_gate.wait_entered();
    let before_release = pending.drain_fixture_facts();
    assert!(
        !before_release.iter().any(|observation| matches!(
            observation,
            ConversationObservation::DurabilityFailed { .. }
        )),
        "the parked callback has not completed, so it published nothing"
    );
    assert!(
        runtime.inner.durability_failure_diagnostic().is_none(),
        "the parked callback has not mutated durability health yet"
    );
    assert_eq!(
        runtime
            .background_status(&execution_id)
            .expect("record")
            .state,
        crate::tools::background::BackgroundLifecycle::PublishingTerminal,
        "the record retains its unresolved terminal candidate"
    );
    assert!(
        background.abandoned_publications().is_empty(),
        "the abandoned settlement fact must not precede the failure callback"
    );

    // (7) Shutdown races the parked failure sink.
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let shutdown_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = done_tx.send(shutdown_runtime.shutdown().await);
    });
    within_liveness_guard(
        "the drain transition to linearize",
        drain_linearization.notified(),
    )
    .await;
    within_liveness_guard(
        "drain to park on the failing background execution",
        drain_supervision.notified(),
    )
    .await;

    // A second, concurrent caller must join the same pending drain rather
    // than start a competing one.
    let (second_tx, mut second_rx) = tokio::sync::oneshot::channel();
    let second_runtime = runtime.clone();
    tokio::spawn(async move {
        let _ = second_tx.send(second_runtime.shutdown().await);
    });

    // (8) Give both callers every scheduling opportunity to finish
    // wrongly, then prove neither did.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    let completion = runtime
        .inner
        .drain
        .get()
        .expect("the one shared drain completion exists")
        .clone();
    assert!(
        !completion
            .completed
            .load(std::sync::atomic::Ordering::Acquire),
        "the shared drain completion must not be filled while the callback is live"
    );
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "shutdown must not return while the runner still owns a callback"
    );
    assert!(
        matches!(
            second_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the concurrent caller joins the same pending drain"
    );
    assert!(
        background.abandoned_publications().is_empty(),
        "the record has not crossed its abandoned settlement boundary"
    );
    assert!(
        !pending
            .drain_fixture_facts()
            .iter()
            .any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
        "the parked callback still published nothing"
    );
    assert_eq!(
        runtime.lifecycle_state(),
        ConversationLifecycleState::Draining,
        "a live owned callback can never publish Quiescent"
    );

    // (9)+(10) Releasing the callback is the only thing that can order
    // the failure, then the abandoned fact, then the shutdown failure.
    background_failure_gate.release();
    await_observation(pending.as_ref(), |observation| {
        matches!(observation, ConversationObservation::DurabilityFailed { operation, .. }
            if operation == "background_terminal_publication")
    })
    .await;
    let shutdown = within_liveness_guard("the supervised shutdown to return", done_rx)
        .await
        .expect("shutdown result channel");
    assert!(
        matches!(
            &shutdown,
            Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                detail
            }) if detail.contains("terminal publication")
        ),
        "the abandoned publication is honest settlement evidence: {shutdown:?}"
    );
    assert_eq!(
        runtime.settle_child_physical_lifetime(false).await,
        Ok(()),
        "the native runner and callback settled; publication failure cannot erase physical proof",
    );
    assert_eq!(
        background.abandoned_publications(),
        vec![execution_id.clone()],
        "the abandoned fact is observable only after the callback returned"
    );
    assert!(
        runtime.inner.durability_failure_diagnostic().is_some(),
        "the failure callback committed before the abandoned fact"
    );
    let second = within_liveness_guard("the joined shutdown to return", second_rx)
        .await
        .expect("second shutdown result channel");
    assert!(
        matches!(
            &second,
            Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                detail
            }) if detail.contains("terminal publication")
        ),
        "the joined caller observes the same supervised failure: {second:?}"
    );

    // (11) After the cached failure exists, that runner owns nothing.
    let events_after = store.read_events(None, 1024).expect("events").events.len();
    let pending_after = store.load_pending().expect("pending inbound").len();
    let record_after = runtime.background_status(&execution_id).expect("record");
    let health_after = runtime.inner.durability_failure_diagnostic();
    let _ = pending.drain_fixture_facts();
    assert!(
        runtime
            .submit_inbound(text_content("after shutdown"))
            .is_err(),
        "a stale inbound handle is refused after the drain decided"
    );
    // A stale settlement handle of the same execution is a local no-op.
    background.finish(
        &execution_id,
        &crate::tools::types::ToolExecutionResult {
            deliveries: Vec::new(),
            status: crate::tools::types::ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    );
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert!(
        pending.drain_fixture_facts().is_empty(),
        "no conversation observation follows the settled runner"
    );
    assert_eq!(
        store.read_events(None, 1024).expect("events").events.len(),
        events_after,
        "no event journal effect follows the settled runner"
    );
    assert_eq!(
        store.load_pending().expect("pending inbound").len(),
        pending_after,
        "no Pending Inbound acceptance follows the settled runner"
    );
    assert_eq!(
        runtime
            .background_status(&execution_id)
            .expect("record")
            .state,
        record_after.state,
        "no background state mutation follows the settled runner"
    );
    assert_eq!(
        runtime.inner.durability_failure_diagnostic(),
        health_after,
        "no durability-health mutation follows the settled runner"
    );

    // (12) The cached failure is honest: the original supervision had
    // already completed before `DrainCompletion` was filled.
    let repeated = within_liveness_guard("the cached shutdown failure", runtime.shutdown()).await;
    assert!(
        matches!(
            &repeated,
            Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                detail
            }) if detail.contains("terminal publication")
        ),
        "a later caller observes the cached supervised failure: {repeated:?}"
    );
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert!(
        pending.drain_fixture_facts().is_empty(),
        "the cached failure needs no fresh supervision and triggers no callback"
    );
    assert_eq!(
        store.read_events(None, 1024).expect("events").events.len(),
        events_after,
        "the repeated shutdown produces no durable effect"
    );
}
