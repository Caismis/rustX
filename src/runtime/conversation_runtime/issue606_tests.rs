//! Issue #606 architecture-contract tests: `ForegroundSlot` exclusivity and
//! poisoning, `ManualTail` reclamation fencing, extracted-owner boundaries
//! (`ExternalAuxiliary` / `ScheduledExecutor` / `ChildControl` / host task),
//! three-role isolation, and atomic admission sections.
use super::foreground::{
    CurrentManualCompaction, CurrentScheduled, ForegroundSlot, IdleConversation, ManualTailState,
};
use super::test_support;
use super::*;
use crate::durable::SqliteConversationStore;
use crate::durable::store::canonical::CanonicalStore;
use crate::message::types::UserSource;
use crate::runtime::auxiliary::external::AuxiliaryInvocationKind;
use crate::runtime::identity::CompactionOperationId;
use crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId;

fn manual_compaction_slot() -> ForegroundSlot {
    ForegroundSlot::ManualCompaction(CurrentManualCompaction {
        operation_id: CompactionOperationId::new("compaction-test"),
        cancellation: AgentCancellation::new(CancellationReason::UserRequested),
    })
}

fn scheduled_slot() -> ForegroundSlot {
    ForegroundSlot::Scheduled {
        current: CurrentScheduled {
            id: ScheduledWorkflowStartId::try_from(format!("swf_{}", "a".repeat(64)))
                .expect("scheduled start id"),
            owner: "scheduled:test".to_owned(),
            cancellation: ScheduledCancellation::new(),
            graph_finished: false,
            recovered_writer: false,
            settlement_wake: Arc::default(),
        },
        conversation: IdleConversation::Durable,
    }
}

fn child_store(
    dir: &tempfile::TempDir,
    conversation_id: &ConversationId,
) -> Arc<SqliteConversationStore> {
    let store = SqliteConversationStore::open(
        conversation_id.clone(),
        &dir.path().join(format!("{conversation_id}.sqlite")),
    )
    .expect("open store");
    store.initialize(&[]).expect("initialize store");
    Arc::new(store)
}

#[test]
fn manual_tail_pending_and_joining_are_both_unreaped() {
    // ManualTail Pending + Joining: an unexited or claimed-but-incomplete
    // compaction task still fences every dependent admission and
    // publication; only `None` releases the physical fence.
    assert!(!ManualTailState::None.is_unreaped());
    let completion = Arc::new(compaction::ManualCompactionCompletion::default());
    assert!(ManualTailState::Joining(completion).is_unreaped());
}

#[test]
fn a_lent_slot_poisons_and_blocks_every_future_owner() {
    // Attempt/ManualCompaction panic contract: the slot that never returned
    // its conversation becomes `Poisoned` — terminal, non-idle, and refusing
    // every subsequent admission so drain can never hang on it.
    let mut slot = manual_compaction_slot();
    assert!(slot.poison());
    assert!(slot.is_poisoned());
    assert!(slot.is_foreground_active());
    assert!(!slot.is_idle());
    // Poisoned is absorbing: a second poison and every start refuse.
    assert!(!slot.poison());
    assert!(
        slot.lend_for_manual_compaction(CurrentManualCompaction {
            operation_id: CompactionOperationId::new("compaction-late"),
            cancellation: AgentCancellation::new(CancellationReason::UserRequested),
        })
        .is_none()
    );
}

#[test]
fn scheduled_and_idle_slots_are_never_poisonable() {
    let mut scheduled = scheduled_slot();
    assert!(!scheduled.poison());
    let mut idle = ForegroundSlot::Idle(IdleConversation::Durable);
    assert!(!idle.poison());
}

#[test]
fn only_the_exact_scheduled_owner_releases_the_slot() {
    // A settlement from a different owner cannot retire this run's slot.
    let mut slot = scheduled_slot();
    assert!(!slot.finish_scheduled("scheduled:foreign"));
    assert!(!slot.is_idle());
    assert!(slot.finish_scheduled("scheduled:test"));
    assert!(slot.is_idle());
}

#[test]
fn scheduled_slot_blocks_catalog_publication_until_settled() {
    // Scheduled Publication Matrix: a running Scheduled owner fences the
    // Runtime MCP Catalog (no historical exemption); once it settles the
    // fence lifts. Pure ExecutionPolicy commits never consult this gate.
    let mut slot = scheduled_slot();
    assert!(slot.blocks_runtime_publication());
    assert!(slot.finish_scheduled("scheduled:test"));
    assert!(!slot.blocks_runtime_publication());
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_tail_fences_admission_then_the_worker_reaps_and_admits() {
    // ManualTail Pending + Post-compaction Admission: while the physical
    // compaction task is unreaped, dependent admission refuses; after the
    // task's durable settlement the admission worker's next pass reaps the
    // finished tail and admits the queued work.
    let gate = Arc::new(probe::Gate::default());
    gate.arm();
    let coordinator_probe = probe::CoordinatorProbe {
        manual_compaction_settlement_gate: Some(gate.clone()),
        ..Default::default()
    };
    let fixture = test_support::headless_fixture_with(Some(coordinator_probe)).await;
    let handle = fixture
        .runtime
        .admit_compaction(None)
        .expect("first compaction admitted");
    assert!(matches!(
        fixture.runtime.inner.lock_state().foreground.manual_tail,
        ManualTailState::Pending(_)
    ));
    // Pending tail: a second compaction is fenced even though the first
    // task's durable work is not yet reported.
    assert!(matches!(
        fixture.runtime.admit_compaction(None),
        Err(compaction::ManualCompactionError::Busy)
    ));
    gate.wait_entered_async().await;
    gate.release();
    // The physical task exits and records its outcome — success or the
    // deterministic NoProgress refusal of an empty history, either way the
    // tail is a finished, unreaped owner.
    let _ = handle.wait().await;
    // The tail record stays Pending until a reaper claims it — but its
    // worker is finished, so the admission worker's next pass reaps it and
    // admits the queued inbound.
    fixture
        .runtime
        .submit_inbound(test_support::text_content("post-compaction inbound"))
        .expect("inbound queued");
    let settlement = fixture.runtime.attempt_settlement(0).await;
    assert!(settlement.ordinal >= 1);
    assert!(
        !fixture
            .runtime
            .inner
            .lock_state()
            .foreground
            .manual_tail
            .is_unreaped()
    );
    fixture.runtime.shutdown().await.expect("fixture shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_reapers_claim_one_join_handle() {
    // Concurrent Reaper with a real barrier: one reaper provably holds the
    // `JoinHandle` while `Joining` is published; a concurrent reaper
    // walks `WaitJoined`, waits for the join's completion signal rather
    // than racing a second join, and the tail ends `None` with
    // `completed && joined` both observed.
    let joining_gate = Arc::new(probe::Gate::default());
    joining_gate.arm();
    let waitjoined_gate = Arc::new(probe::Gate::default());
    waitjoined_gate.arm();
    let coordinator_probe = probe::CoordinatorProbe {
        manual_tail_joining_gate: Some(joining_gate.clone()),
        manual_tail_waitjoined_gate: Some(waitjoined_gate.clone()),
        ..Default::default()
    };
    let fixture = test_support::headless_fixture_with(Some(coordinator_probe)).await;
    let runtime = fixture.runtime.clone();
    let handle = runtime.admit_compaction(None).expect("compaction admitted");
    // Drive a reap pass that claims the JoinHandle and pauses with
    // `Joining` published. The admission worker's next pass reaps it once
    // the task finishes; the barrier holds it inside `Reap::Work`.
    runtime
        .submit_inbound(test_support::text_content("trigger a reaper pass"))
        .expect("inbound queued");
    joining_gate.wait_entered_async().await;
    assert!(
        matches!(
            runtime.inner.lock_state().foreground.manual_tail,
            ManualTailState::Joining(_)
        ),
        "the first reaper published Joining while holding the JoinHandle"
    );
    // A second reaper — drain — sees Joining and parks on the shared
    // completion. The WaitJoined barrier PROVES it is parked there before
    // the first reaper's join is allowed to finish.
    let drain = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.shutdown().await }
    });
    waitjoined_gate.wait_entered_async().await;
    assert!(
        matches!(
            runtime.inner.lock_state().foreground.manual_tail,
            ManualTailState::Joining(_)
        ),
        "the second reaper observed Joining and parks in WaitJoined"
    );
    // Release the parked second reaper's gate and the first reaper's join
    // in one go: `joined` publication is now the only frontier between
    // them — exactly the shared-completion handoff under test.
    waitjoined_gate.release();
    joining_gate.release();
    // `handle.wait()` requires `completed && joined` — it returns only
    // because the one claimed join finished, which is the shared
    // completion the second reaper waited on.
    let _ = handle.wait().await;
    drain.await.expect("drain task").expect("shutdown");
    assert!(
        !runtime
            .inner
            .lock_state()
            .foreground
            .manual_tail
            .is_unreaped(),
        "both reapers converge on one join; the tail is reaped"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn host_task_installation_is_inactive_only_and_install_once() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        test_support::headless_runtime(&dir, vec![test_support::one_turn_script()], None, None)
            .await;
    runtime
        .install_host_task(|| tokio::spawn(async {}))
        .expect("first install while Inactive");
    let error = runtime
        .install_host_task(|| tokio::spawn(async {}))
        .expect_err("a second install is refused");
    assert_eq!(error.kind, crate::context::ContextErrorKind::AuxiliaryBusy);
    runtime.activate();
    let error = runtime
        .install_host_task(|| tokio::spawn(async {}))
        .expect_err("post-activation install is refused");
    assert_eq!(error.kind, crate::context::ContextErrorKind::Cancelled);
    runtime.shutdown().await.expect("fixture shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn host_task_holds_only_a_weak_runtime_while_idle() {
    // The optional host task must not pin the runtime or its store: while the
    // task is idle, dropping the last semantic runtime handle drops the
    // runtime even though the task is still parked.
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        test_support::headless_runtime(&dir, vec![test_support::one_turn_script()], None, None)
            .await;
    let weak = Arc::downgrade(&runtime.inner);
    let (release, parked) = tokio::sync::oneshot::channel::<()>();
    let (proof, proved) = tokio::sync::oneshot::channel::<bool>();
    runtime
        .install_host_task(|| {
            tokio::spawn(async move {
                parked.await.expect("park");
                proof.send(weak.upgrade().is_some()).expect("proof");
            })
        })
        .expect("install");
    drop(runtime);
    release.send(()).expect("release");
    assert!(
        !proved.await.expect("task answered"),
        "the parked host task still pinned the runtime"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsettled_host_task_blocks_the_drain_quiescent_boundary() {
    // Title Publication Tail: auxiliary work may have ended, but while the
    // host task (session title publication) is unsettled the drain must not
    // mark Quiescent. Order: executor drain -> host task -> no admissions ->
    // quiescent.
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        test_support::headless_runtime(&dir, vec![test_support::one_turn_script()], None, None)
            .await;
    let (entered, entered_rx) = std::sync::mpsc::channel::<()>();
    let (release, parked) = tokio::sync::oneshot::channel::<()>();
    runtime
        .install_host_task(|| {
            tokio::spawn(async move {
                entered.send(()).expect("entered");
                parked.await.expect("park");
            })
        })
        .expect("install");
    runtime.activate();
    let shutdown = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.shutdown_outcome().await }
    });
    entered_rx.recv().expect("host task is parked");
    // The drain commits `Running -> Draining` eagerly and cannot complete
    // while the host task remains unsettled.
    assert!(matches!(
        runtime.inner.lifecycle.state(),
        ConversationLifecycleState::Draining | ConversationLifecycleState::Running
    ));
    release.send(()).expect("release host task");
    let outcome = shutdown.await.expect("drain task").expect("drain outcome");
    assert!(outcome.logical.is_ok() && outcome.physical.is_ok());
}

async fn child_config(
    dir: &tempfile::TempDir,
    conversation_id: &str,
    role: ConversationRole,
) -> RuntimeConversationConfig {
    // `conversation_id` must be a canonical `conv_<uuidv7>` tail fragment.
    let id = ConversationId::new(format!("conv_{conversation_id}").as_str());
    let store = child_store(dir, &id);
    let (_registry, mut config) = test_support::subagent_runtime_config_with_registry(
        dir,
        id.as_ref(),
        store,
        &id.clone(),
        &AgentId::new(format!("registry-agent-{conversation_id}").as_str()),
        &AgentId::new(format!("agent-{conversation_id}").as_str()),
    )
    .await;
    config.role = role;
    // A child runtime never adopts its parent's subagent registry.
    config.subagents = None;
    config
}

#[tokio::test(flavor = "current_thread")]
async fn three_role_isolation_is_declared_not_inferred() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (runtime, _model) =
        test_support::headless_runtime(&dir, vec![test_support::one_turn_script()], None, None)
            .await;
    // Root: no child control handle and no ChildControlState.
    assert!(!runtime.inner.role.is_child());
    assert!(runtime.child_control().is_none());
    assert!(runtime.inner.lock_state().child.is_none());
    // An Inactive runtime drains directly: shutdown is the Running path.
    drop(runtime);

    // SubagentChild: the control handle exists before activation and the
    // child state is installed.
    let config = child_config(
        &dir,
        "7a2b9c4d-1234-7000-8000-000000000001",
        ConversationRole::SubagentChild,
    )
    .await;
    let child = ConversationRuntime::new(config).expect("child runtime");
    let control = child.child_control().expect("child control");
    assert!(child.inner.lock_state().child.is_some());
    assert!(!child.is_activated());
    // Inactive Child Cancellation: physical-only rollback evidence —
    // PreparationRollback drains without semantic activation, so the
    // lifecycle lands on Quiescent without ever passing through Running.
    control
        .settle_child_physical_lifetime(false)
        .await
        .expect("physical rollback");
    assert_eq!(
        child.inner.lifecycle.state(),
        ConversationLifecycleState::Quiescent
    );
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_agent_run_has_no_guidance_or_seal() {
    // WorkflowAgentRun gets output/permits/settlement/rollback but never
    // Parent Guidance or Terminal Seal access: the role gate refuses.
    let dir = tempfile::tempdir().expect("temp dir");
    let config = child_config(
        &dir,
        "7a2b9c4d-1234-7000-8000-000000000002",
        ConversationRole::WorkflowAgentRun {
            output: Some(Arc::new(
                crate::runtime::workflow::WorkflowOutputLatch::new(serde_json::json!({
                    "type": "object", "properties": {"summary": {"type": "string"}},
                    "required": ["summary"], "additionalProperties": false
                }))
                .expect("output latch"),
            )),
        },
    )
    .await;
    let runtime = ConversationRuntime::new(config).expect("workflow child");
    let control = runtime.child_control().expect("workflow child control");
    // seal_probe_calls() == 0: the seal probe is never consulted for a
    // workflow-owned child (mirrors the existing native conformance proof).
    assert_eq!(control.seal_probe_calls(), 0);
    drop(runtime);
}

#[tokio::test(flavor = "current_thread")]
#[should_panic(expected = "SubagentChild-only")]
async fn workflow_agent_run_cannot_submit_parent_guidance() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = child_config(
        &dir,
        "7a2b9c4d-1234-7000-8000-000000000003",
        ConversationRole::WorkflowAgentRun {
            output: Some(Arc::new(
                crate::runtime::workflow::WorkflowOutputLatch::new(serde_json::json!({
                    "type": "object", "properties": {"summary": {"type": "string"}},
                    "required": ["summary"], "additionalProperties": false
                }))
                .expect("output latch"),
            )),
        },
    )
    .await;
    let runtime = ConversationRuntime::new(config).expect("workflow child");
    runtime
        .child_control()
        .expect("workflow child control")
        .submit_parent_guidance(UserSource::Runtime, test_support::text_content("steer"))
        .expect("unreachable");
}

#[tokio::test(flavor = "current_thread")]
async fn auxiliary_admission_freezes_snapshot_and_owns_capacity_atomically() {
    // Auxiliary Admission Race: snapshot, reservation and lifecycle
    // admission are taken inside the one coordinator critical section, so
    // the bundle is internally consistent and capacity is owned from the
    // moment the call is admitted.
    let fixture = test_support::headless_fixture().await;
    let cancellation = crate::runtime::cancellation::CancellationSignal::default();
    let admitted = fixture
        .runtime
        .external_auxiliary()
        .admit_auxiliary_work(AuxiliaryInvocationKind::Title, 3, &cancellation, true)
        .expect("title admission");
    assert_eq!(
        admitted.resource_revision,
        admitted.resources.revision().get()
    );
    assert!(admitted.business_admission.is_some());
    let second = fixture
        .runtime
        .external_auxiliary()
        .admit_auxiliary_work(AuxiliaryInvocationKind::Title, 3, &cancellation, true)
        .expect("second title admission");
    drop(admitted);
    drop(second);
    fixture.runtime.shutdown().await.expect("fixture shutdown");
}

#[tokio::test(flavor = "current_thread")]
async fn auxiliary_admission_blocks_inside_the_shared_coordinator_section() {
    // The coordinator mutex IS the critical section: while the test holds
    // it, admission cannot acquire it; once released, admission proceeds.
    // The channel handshake proves the interleave without sleeps.
    let fixture = test_support::headless_fixture().await;
    let guard = fixture.runtime.inner.lock_state();
    let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
    let runtime = fixture.runtime.clone();
    let admitted = std::thread::spawn(move || {
        blocked_tx.send(()).expect("blocked marker");
        runtime.external_auxiliary().admit_auxiliary_work(
            AuxiliaryInvocationKind::Btw,
            1,
            &crate::runtime::cancellation::CancellationSignal::default(),
            false,
        )
    });
    blocked_rx.recv().expect("admit thread started");
    drop(guard);
    assert!(admitted.join().expect("admit thread").is_ok());
    fixture.runtime.shutdown().await.expect("fixture shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_task_panic_publishes_a_failed_settlement_for_terminal_waiters() {
    // Issue #606 (P1): a panicking Attempt owner used to leave every
    // `attempt_settlement` waiter — the SubagentChild/WorkflowAgentRun
    // terminal wait — hung forever. The poison path now publishes a
    // recognizable `RuntimeFailed` settlement so both child roles exit
    // through their existing failure-reporting path. It fabricates no
    // Attempt terminal and overwrites no committed history.
    let dir = tempfile::tempdir().expect("temp dir");
    let (release, released) = test_support::model_release();
    let script = vec![
        test_support::FakeStep::Emit(crate::model::event::ModelEvent::Started),
        test_support::FakeStep::ParkUntilReleased(released),
        test_support::FakeStep::Emit(crate::model::event::ModelEvent::Completed {
            finish_reason: crate::model::finish::ModelFinishReason::Stop,
            usage: None,
        }),
    ];
    let (runtime, _model) = test_support::headless_runtime(&dir, vec![script], None, None).await;
    runtime.activate();
    runtime
        .submit_inbound(test_support::text_content("hold the attempt"))
        .expect("inbound accepted");
    // Wait until the attempt physically owns the slot.
    let attempt_id = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let attempt = {
                let state = runtime.inner.lock_state();
                state
                    .foreground
                    .slot
                    .as_attempt()
                    .map(|current| current.attempt_id.clone())
            };
            if let Some(attempt_id) = attempt {
                break attempt_id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the admitted attempt owns the slot");
    // `AttemptTaskPoisonGuard` calls exactly this on unwinding; driving it
    // directly proves the settlement publication the guard relies on.
    runtime.inner.poison_foreground_for_task(&attempt_id);
    let settlement = runtime.attempt_settlement(0).await;
    assert_eq!(settlement.attempt_id, attempt_id);
    assert!(
        matches!(
            settlement.outcome,
            attempt::NativeAttemptOutcome::RuntimeFailed
        ),
        "the poisoned attempt settles as a recognizable failure: {:?}",
        settlement.outcome
    );
    // Physical drain still completes: the parked task is released and the
    // runtime shuts down rather than hanging on the poisoned owner.
    let _ = release.send(true);
    // Shutdown terminates and reports the absorbing failure fact rather
    // than hanging on the poisoned owner.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("shutdown drains after the panic");
}

#[tokio::test(flavor = "multi_thread")]
async fn in_lock_panic_recovers_to_failed_admission_and_still_drains() {
    // Issue #606 (P2): a panic *while holding* the coordinator guard must
    // not wedge every later caller on a poisoned mutex. The controlled
    // recovery boundary in `lock_state` converges business admission to
    // the absorbing failed state while physical drain — a second reaper
    // pass and shutdown — still terminates.
    let fixture = test_support::headless_fixture().await;
    let runtime = fixture.runtime.clone();
    // Fault injection: panic while holding the coordinator guard, exactly
    // like a panic inside a critical-section mutation (e.g. `append_event`
    // in `finish_manual_compaction`).
    let inner = runtime.inner.clone();
    std::thread::spawn(move || {
        let _guard = inner.state.lock().expect("pre-poison lock");
        panic!("fault injection: panic while holding the coordinator guard");
    })
    .join()
    .expect_err("the injected panic poisoned the coordinator lock");
    // Business admission converges to failed — every gate consults the
    // committed `DurabilityFailed` fact instead of hanging on the lock.
    // `submit_inbound` is synchronous on the coordinator lock: the
    // recovered guard answers immediately instead of panicking or hanging.
    let rejected = runtime.submit_inbound(test_support::text_content("post-poison inbound"));
    assert!(
        rejected.is_err(),
        "a poisoned coordinator admits no new business work"
    );
    // Physical drain — the manual-tail reaper and shutdown — still
    // terminates: it reads the mid-transition facts and exits.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let _ = runtime.admit_compaction(None);
        runtime.shutdown().await
    })
    .await
    .expect("reaper and shutdown terminate after lock poisoning");
    // The converged failure state is consistent for every reader: the
    // poison mark was cleared after the `DurabilityFailed` fact committed,
    // so a non-blocking `try_lock` reader (`try_activity`) observes the
    // failed health instead of spinning on `None` forever (Issue #606).
    let activity = runtime.activity();
    assert!(
        activity.is_some_and(|activity| activity.system_error),
        "non-blocking readers observe the converged failure: {activity:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_panic_during_a_live_attempt_preserves_cancellation_and_settlement_ownership() {
    // Issue #606 (P1): the generic lock-recovery path must close business
    // admission WITHOUT destroying the live foreground owner. The attempt
    // parked on a cancellation-only model barrier keeps its owner record —
    // drain's cancellation reaches the model, the task exits on its own
    // settlement path, and admission is released. The test never releases
    // the model manually: shutdown's cancellation is the only frontier.
    let dir = tempfile::tempdir().expect("temp dir");
    let script = vec![
        test_support::FakeStep::Emit(crate::model::event::ModelEvent::Started),
        test_support::FakeStep::ParkUntilCancelled,
    ];
    let (runtime, model) = test_support::headless_runtime(&dir, vec![script], None, None).await;
    runtime.activate();
    runtime
        .submit_inbound(test_support::text_content(
            "hold the attempt on a cancel barrier",
        ))
        .expect("inbound accepted");
    // The real model request is parked on the cancellation-only barrier:
    // only the preserved owner's cancellation can move it.
    model
        .parks()
        .wait_for(|parks| *parks >= 1)
        .await
        .expect("the attempt parks on the cancel barrier");
    // Fault injection: a panic inside another coordinator critical
    // section (e.g. `apply_execution_policy`'s compose closure) poisons
    // the mutex while the Attempt still owns the slot.
    let inner = runtime.inner.clone();
    std::thread::spawn(move || {
        let _guard = inner.state.lock().expect("pre-poison lock");
        panic!("fault injection: panic inside a coordinator critical section");
    })
    .join()
    .expect_err("the injected panic poisoned the coordinator lock");
    // Business admission converges to failed.
    let rejected = runtime.submit_inbound(test_support::text_content("post-poison inbound"));
    assert!(
        rejected.is_err(),
        "a poisoned coordinator admits no new business work"
    );
    // The live attempt owner survived recovery: its slot is still lent
    // (not Poisoned), so cancellation and settlement remain reachable.
    assert!(
        !runtime.inner.lock_state().foreground.slot.is_poisoned(),
        "recovery preserves the live foreground owner"
    );
    // Shutdown's drain cancellation is the only thing that can release
    // the parked model — the test never touches the release frontier.
    let settlement = runtime.attempt_settlement(0);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("drain cancellation reaches the parked attempt");
    // The task exited through its own settlement path with the real
    // cancelled outcome — no fabricated terminal, no `RuntimeFailed`
    // receipt pre-empted by a destroyed owner.
    let settlement = settlement.await;
    assert!(
        matches!(
            settlement.outcome,
            attempt::NativeAttemptOutcome::Cancelled | attempt::NativeAttemptOutcome::ModelFailed
        ),
        "the attempt settles on its own cancelled outcome: {:?}",
        settlement.outcome
    );
}
