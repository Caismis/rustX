use super::*;
use crate::app_server::host::{AppServerHost, HostAdmissionError, ServerLifecycle};
use crate::runtime::monotonic::ManualMonotonicClock;

fn policy(f: &mut Fixture, limit: usize) -> Arc<ManualMonotonicClock> {
    let clock = Arc::new(ManualMonotonicClock::new());
    f.manager.clock = clock.clone();
    let mut state = f.manager.registry.0.lock().unwrap();
    state.policy.max_resident_runtimes = limit;
    state.policy.idle_grace_ms = 100;
    drop(state);
    f.host = AppServerHost::new(
        f.manager.clone(),
        crate::local_runtime::app_server_policy::AppServerPolicy {
            max_resident_runtimes: limit,
            idle_grace_ms: 100,
            ..Default::default()
        },
    );
    clock
}

#[tokio::test]
async fn quota_counts_loading_loaded_unloading_and_single_flight() {
    let mut f = Fixture::new().await;
    policy(&mut f, 1);
    let id = f.id(0).await;
    let probe = f.manager.probe(&id);
    probe.before_compose.arm();
    let first = f.load(0);
    probe.before_compose.entered().await;
    let same = f.load(0);
    probe.joined(2).await;
    assert_eq!(f.manager.diagnostics().loading, 1);
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    probe.before_compose.release();
    let first = first.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &same.await.unwrap().unwrap()));
    assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    probe.before_shutdown.arm();
    let manager = f.manager.clone();
    let unload_id = id.clone();
    let unload = tokio::spawn(async move { manager.unload(&unload_id).await });
    probe.before_shutdown.entered().await;
    assert_eq!(f.manager.diagnostics().unloading, 1);
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    probe.before_shutdown.release();
    unload.await.unwrap().unwrap();
    f.load(1).await.unwrap().unwrap();
    f.close().await;
}

#[tokio::test]
async fn failed_composition_releases_reservation() {
    let mut f = Fixture::new().await;
    policy(&mut f, 1);
    f.manager
        .probe(&f.id(0).await)
        .panic_once
        .store(true, Ordering::SeqCst);
    assert!(f.load(0).await.unwrap().is_err());
    assert_eq!(f.manager.diagnostics().loading, 0);
    f.load(1).await.unwrap().unwrap();
    f.close().await;
}

#[tokio::test]
async fn idle_grace_detach_eviction_cold_resume_and_session_independence() {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    let a = f.load(0).await.unwrap().unwrap();
    let b = f.load(1).await.unwrap().unwrap();
    let live = a.inspect_runtime().unwrap();
    live.submit_inbound(input("idle-history")).unwrap();
    f.gates[0].wait_entered().await;
    let done = live.settlement_signal().notified();
    f.gates[0].release();
    done.await;
    let history = live.historical_canonical_history().unwrap();
    let controller = f.manager.session_controller();
    let settings = controller.read_settings(&f.sessions[0].id).await.unwrap();
    let (_attachment, external) = a.client().attach().unwrap();
    let (_b_attachment, _b_external) = b.client().attach().unwrap();
    f.manager.reap_idle();
    clock.advance(101);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    external.release();
    f.manager.reap_idle();
    clock.advance(99);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    clock.advance(1);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    assert_eq!(
        a.client().submit_inbound(input("late")).unwrap_err(),
        RuntimeManagerError::StaleIncarnation
    );
    f.manager.unload(a.conversation_id()).await.unwrap();
    assert_eq!(
        f.manager.residency(b.conversation_id()),
        ResidencyState::Loaded
    );
    assert_eq!(
        controller.read_session(&f.sessions[0].id).await.unwrap().id,
        f.sessions[0].id
    );
    assert!(
        controller
            .list_sessions(None, 0, 10)
            .await
            .unwrap()
            .sessions
            .iter()
            .any(|row| row.id == f.sessions[0].id)
    );
    assert_eq!(
        controller.read_settings(&f.sessions[0].id).await.unwrap(),
        settings
    );
    let recovered = f.load(0).await.unwrap().unwrap();
    assert_ne!(a.incarnation_id(), recovered.incarnation_id());
    assert_eq!(
        recovered
            .inspect_runtime()
            .unwrap()
            .historical_canonical_history()
            .unwrap(),
        history
    );
    assert_eq!(
        f.provider.request_bodies().len(),
        1,
        "cold resume never resubmits the completed turn"
    );
    f.close().await;
}

#[tokio::test]
async fn operation_wins_idle_and_drain_retains_owned_lease_after_waiter_drop() {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    let a = f.load(0).await.unwrap().unwrap();
    f.manager.reap_idle();
    clock.advance(100);
    let probe = f.manager.probe(a.conversation_id());
    probe.before_operation.arm();
    let receiver = a.client().start_operation(|| async {}).unwrap();
    probe.before_operation.entered().await;
    drop(receiver);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    let b = f.load(1).await.unwrap().unwrap();
    let accepted = f.host.admit_request(std::convert::identity).unwrap();
    f.host.begin_drain();
    assert!(matches!(
        f.host.admit_request(std::convert::identity),
        Err(HostAdmissionError::ServerDraining)
    ));
    let host = f.host.clone();
    let drain = tokio::spawn(async move { host.drain().await });
    probe
        .draining_operations
        .subscribe()
        .wait_for(|value| *value)
        .await
        .unwrap();
    assert!(!drain.is_finished());
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    // B settles even while A and an accepted host request remain pending.
    f.manager.unload(b.conversation_id()).await.unwrap();
    assert_eq!(
        f.manager.residency(b.conversation_id()),
        ResidencyState::Unloaded
    );
    assert!(!drain.is_finished());
    drop(accepted);
    probe.before_operation.release();
    assert!(drain.await.unwrap().is_empty());
    f.host.finish_drain().unwrap();
    assert_eq!(f.host.diagnostics().lifecycle, ServerLifecycle::Terminated);
    f.close().await;
}

#[tokio::test]
async fn external_attachment_capacity_is_independent_and_released_on_detach() {
    let mut f = Fixture::new().await;
    policy(&mut f, 2);
    let host = AppServerHost::new(
        f.manager.clone(),
        crate::local_runtime::app_server_policy::AppServerPolicy {
            max_external_attachments: 1,
            ..Default::default()
        },
    );
    let a = f.load(0).await.unwrap().unwrap();
    let b = f.load(1).await.unwrap().unwrap();
    let capacity = host.admit_attachment().unwrap();
    let (_a, external) = a.client().attach().unwrap();
    assert!(matches!(
        host.admit_attachment(),
        Err(HostAdmissionError::AttachmentCapacity)
    ));
    external.release();
    capacity.release();
    let _capacity = host.admit_attachment().unwrap();
    let _b = b.client().attach().unwrap();
    assert_eq!(f.manager.diagnostics().residency_pins, 1);
    assert_eq!(host.diagnostics().external_attachments, 1);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_claim_has_both_attach_and_inbound_winners_at_publication() {
    bounded(async {
        for attachment_wins in [false, true] {
            let mut f = Fixture::new().await;
            let clock = policy(&mut f, 2);
            let a = f.load(0).await.unwrap().unwrap();
            let native = a.inspect_runtime().unwrap();
            while native.idle_epoch().is_err() {
                tokio::task::yield_now().await;
            }
            f.manager.reap_idle();
            clock.advance(100);
            let probe = f.manager.probe(a.conversation_id());
            let guard = probe.idle_before_claim.arm_scoped();
            let manager = f.manager.clone();
            let scan = tokio::task::spawn_blocking(move || manager.reap_idle());
            gate_entered(&probe.idle_before_claim).await;
            let attachment = if attachment_wins {
                Some(a.client().attach().unwrap())
            } else {
                a.client().submit_inbound(input("request-A")).unwrap();
                None
            };
            drop(guard);
            scan.await.unwrap();
            assert_eq!(
                f.manager.residency(a.conversation_id()),
                ResidencyState::Loaded
            );
            drop(attachment);
            f.close().await;
        }
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        f.manager.reap_idle();
        clock.advance(100);
        let probe = f.manager.probe(a.conversation_id());
        let guard = probe.idle_after_claim.arm_scoped();
        let manager = f.manager.clone();
        let scan = tokio::task::spawn_blocking(move || manager.reap_idle());
        gate_entered(&probe.idle_after_claim).await;
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(matches!(
            a.client().attach(),
            Err(RuntimeManagerError::StaleIncarnation)
        ));
        assert_eq!(
            a.client().submit_inbound(input("late")).unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        let reloading = f.load(0);
        probe.joined(2).await;
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        drop(guard);
        scan.await.unwrap();
        let new = reloading.await.unwrap().unwrap();
        assert_ne!(a.incarnation_id(), new.incarnation_id());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn active_turn_after_detach_starts_grace_only_after_settlement() {
    bounded(async {
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        let (attached, external) = a.client().attach().unwrap();
        a.client().submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        attached.attachment.detach();
        external.release();
        clock.advance(1000);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        assert!(f.manager.diagnostics().sessions[0].idle_for_ms.is_none());
        f.gates[0].release();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        f.manager.reap_idle();
        clock.advance(99);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        clock.advance(1);
        f.manager.reap_idle();
        f.manager.unload(a.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn drain_reports_failure_and_still_settles_sibling_without_releasing_failed_slot() {
    let mut f = Fixture::new().await;
    policy(&mut f, 2);
    let a = f.load(0).await.unwrap().unwrap();
    let b = f.load(1).await.unwrap().unwrap();
    a.inspect_runtime().unwrap().fail_residency_settlement();
    assert!(f.manager.unload(a.conversation_id()).await.is_err());
    let c = f
        .manager
        .sessions
        .create_session(SessionPersistentState::from_input(
            &SessionConfigInput::new(f.workspaces[0].clone()),
        ))
        .await
        .unwrap()
        .session;
    assert_eq!(
        f.manager.load(&c.id, None).await.unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    let failures = f.host.drain().await;
    assert_eq!(failures.len(), 1);
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    assert_eq!(
        f.manager.residency(b.conversation_id()),
        ResidencyState::Unloaded
    );
    assert_eq!(f.host.diagnostics().lifecycle, ServerLifecycle::Draining);
    assert_eq!(f.host.diagnostics().shutdown_failures, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_inbound_before_attempt_admission_prevents_idle_claim() {
    bounded(async {
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        native.install_admission_gate(gate.clone());
        let guard = gate.arm_scoped();
        a.client().submit_inbound(input("request-A")).unwrap();
        gate_entered(&gate).await;
        assert!(!native.has_current_attempt());
        assert!(native.idle_epoch().is_err());
        clock.advance(1000);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        drop(guard);
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn inactive_goal_can_idle_unload_and_policy_is_user_only() {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    std::fs::write(
        f.workspaces[0].join("rustx.toml"),
        "[agent.plugins.goal]\nenabled = true\n",
    )
    .unwrap();
    let a = f.load(0).await.unwrap().unwrap();
    f.manager.reap_idle();
    clock.advance(1000);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    std::fs::write(
        f.workspaces[1].join("rustx.toml"),
        "[app_server]\nmax_resident_runtimes = 2\n",
    )
    .unwrap();
    assert!(
        f.load(1)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("process")
    );
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_distinct_loads_cannot_oversubscribe_one_slot() {
    let mut f = Fixture::new().await;
    policy(&mut f, 1);
    let (a, b) = tokio::join!(f.load(0), f.load(1));
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(RuntimeManagerError::ResidencyCapacity)))
            .count(),
        1
    );
    assert_eq!(f.manager.diagnostics().loaded, 1);
    f.close().await;
}

#[tokio::test]
async fn one_reaper_timer_wakes_from_manual_clock_and_unloads_automatically() {
    bounded(async {
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        let (waiting, mut observed) = watch::channel(0);
        f.manager.registry.0.lock().unwrap().reaper_waiting = Some(waiting);
        let stop = tokio_util::sync::CancellationToken::new();
        let worker_stop = stop.clone();
        let manager = f.manager.clone();
        let worker = tokio::spawn(async move { manager.run_idle_reaper(worker_stop).await });
        observed
            .wait_for(|deadline| *deadline == 1000)
            .await
            .unwrap();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        clock.advance(1000);
        observed
            .wait_for(|deadline| *deadline == 2000)
            .await
            .unwrap();
        assert_ne!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        f.manager.unload(a.conversation_id()).await.unwrap();
        stop.cancel();
        worker.await.unwrap();
        f.close().await;
    })
    .await;
}

async fn goal_fixture() -> (
    Fixture,
    Arc<ManualMonotonicClock>,
    Arc<ManagedRuntime>,
    ConversationRuntime,
) {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    std::fs::write(
        f.workspaces[0].join("rustx.toml"),
        "[agent.plugins.goal]\nenabled = true\n",
    )
    .unwrap();
    let a = f.load(0).await.unwrap().unwrap();
    let native = a.inspect_runtime().unwrap();
    while native.idle_epoch().is_err() {
        tokio::task::yield_now().await;
    }
    (f, clock, a, native)
}

/// Issue #351: residency follows durable Goal semantics, not an activation
/// bit. A composed Goal that is durably Active with budget remaining owns
/// future autonomous work, so idle eviction must not race its continuation;
/// every durable transition out of Active releases that ownership at the
/// same commit that changes the phase.
#[tokio::test]
async fn active_goal_owns_residency_until_a_durable_phase_transition() {
    bounded(async {
        use crate::goal::{GoalControl, GoalMutation};
        for mutation in [
            GoalMutation::Pause,
            GoalMutation::Block {
                reason: "waiting".into(),
            },
            GoalMutation::Complete,
        ] {
            let (f, clock, a, native) = goal_fixture().await;
            // No await between the control commit and the probe: the real
            // admission worker has not consumed its wake, so durable Active
            // state alone is what pins residency here.
            let goal = native
                .control_goal(GoalControl::Create {
                    objective: "test ownership".into(),
                    budget: 2,
                })
                .unwrap();
            let created = goal.current.unwrap();
            assert_eq!(created.phase, crate::goal::GoalPhase::Active);
            assert!(!native.has_current_attempt());
            assert_eq!(
                native.idle_epoch(),
                Err(crate::runtime::conversation_runtime::IdleBusyReason::AutonomousExtension)
            );
            f.manager.reap_idle();
            clock.advance(1000);
            f.manager.reap_idle();
            assert_eq!(
                f.manager.residency(a.conversation_id()),
                ResidencyState::Loaded
            );
            let view = native
                .control_goal(GoalControl::Mutate {
                    expected: created.reference,
                    mutation,
                })
                .unwrap();
            assert_ne!(
                view.current.unwrap().phase,
                crate::goal::GoalPhase::Active,
                "the durable transition is the only thing that released ownership"
            );
            assert!(native.idle_epoch().is_ok());
            f.manager.reap_idle();
            clock.advance(99);
            f.manager.reap_idle();
            assert_eq!(
                f.manager.residency(a.conversation_id()),
                ResidencyState::Loaded
            );
            clock.advance(1);
            f.manager.reap_idle();
            assert_eq!(
                f.manager.residency(a.conversation_id()),
                ResidencyState::Unloading
            );
            f.manager.unload(a.conversation_id()).await.unwrap();
            f.close().await;
        }
    })
    .await;
}

/// Issue #351 recovery: a durably Active Goal is restored Active, so the
/// reopened runtime owns its continuation and cannot be evicted out from
/// under it. Pausing it — the explicit user act — releases residency.
#[tokio::test]
async fn a_reopened_active_goal_owns_residency_until_it_is_paused() {
    bounded(async {
        let (f, clock, a, native) = goal_fixture().await;
        native
            .control_goal(crate::goal::GoalControl::Create {
                objective: "persisted goal".into(),
                budget: 2,
            })
            .unwrap();
        let paused = native
            .control_goal(crate::goal::GoalControl::Mutate {
                expected: native
                    .goal_view()
                    .unwrap()
                    .unwrap()
                    .current
                    .unwrap()
                    .reference,
                mutation: crate::goal::GoalMutation::Pause,
            })
            .unwrap();
        assert_eq!(
            paused.current.unwrap().phase,
            crate::goal::GoalPhase::Paused
        );
        f.manager.unload(a.conversation_id()).await.unwrap();

        // Reopening a Paused Goal owns nothing and may idle-unload.
        let recovered = f.load(0).await.unwrap().unwrap();
        let runtime = recovered.inspect_runtime().unwrap();
        let view = runtime.goal_view().unwrap().unwrap();
        let goal = view.current.unwrap();
        assert_eq!(goal.phase, crate::goal::GoalPhase::Paused);
        while runtime.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        // Resuming through the ordinary typed control restores durable
        // authorization, with no second "arm" operation, and residency
        // ownership returns with it.
        let resumed = runtime
            .control_goal(crate::goal::GoalControl::Mutate {
                expected: goal.reference,
                mutation: crate::goal::GoalMutation::Resume,
            })
            .unwrap();
        let resumed = resumed.current.unwrap();
        assert_eq!(resumed.phase, crate::goal::GoalPhase::Active);
        assert_eq!(
            runtime.idle_epoch(),
            Err(crate::runtime::conversation_runtime::IdleBusyReason::AutonomousExtension)
        );
        f.manager.reap_idle();
        clock.advance(1000);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(recovered.conversation_id()),
            ResidencyState::Loaded
        );
        runtime
            .control_goal(crate::goal::GoalControl::Mutate {
                expected: resumed.reference,
                mutation: crate::goal::GoalMutation::Pause,
            })
            .unwrap();
        while runtime.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        f.manager.reap_idle();
        clock.advance(100);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(recovered.conversation_id()),
            ResidencyState::Unloading
        );
        f.manager.unload(recovered.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}

/// Issue #351: the resume-vs-idle-claim race has two deterministic winners
/// and no activation bit on either side. A committed resume advances the
/// runtime's activity token, so a claim holding the older epoch loses; a
/// claim that wins first closes admission, so the later resume is refused by
/// the ordinary lifecycle and the durable phase is unchanged.
#[tokio::test]
async fn goal_resume_and_idle_claim_have_both_winner_orders() {
    bounded(async {
        use crate::goal::{GoalControl, GoalMutation};
        for resume_wins in [true, false] {
            let (f, _, a, native) = goal_fixture().await;
            let goal = native
                .control_goal(GoalControl::Create {
                    objective: "race".into(),
                    budget: 1,
                })
                .unwrap();
            let paused = native
                .control_goal(GoalControl::Mutate {
                    expected: goal.current.unwrap().reference,
                    mutation: GoalMutation::Pause,
                })
                .unwrap()
                .current
                .unwrap();
            // Drive both native commit orders without yielding to the worker.
            // The epoch is exactly the token the manager uses at idle claim.
            let epoch = native.idle_epoch().unwrap();
            let resume = GoalControl::Mutate {
                expected: paused.reference.clone(),
                mutation: GoalMutation::Resume,
            };
            if resume_wins {
                let view = native.control_goal(resume).unwrap();
                assert_eq!(
                    view.current.unwrap().phase,
                    crate::goal::GoalPhase::Active,
                    "resume alone restores continuation eligibility"
                );
                assert!(!native.has_current_attempt());
                assert!(
                    !native.claim_idle(epoch),
                    "the committed resume invalidated the probed epoch"
                );
            } else {
                assert!(native.claim_idle(epoch));
                assert!(
                    native
                        .control_goal(resume)
                        .unwrap_err()
                        .contains("accepts no inbound")
                );
                assert_eq!(
                    native.goal_view().unwrap().unwrap().current.unwrap(),
                    paused,
                    "a refused control never rewrites durable phase"
                );
            }
            f.manager.unload(a.conversation_id()).await.unwrap();
            f.close().await;
        }
    })
    .await;
}

#[tokio::test]
async fn host_admission_commit_accounts_for_requests_connections_and_attachments() {
    bounded(async {
        let f = Fixture::new().await;
        let request = f.host.admit_request(std::convert::identity).unwrap();
        let connection = f.host.admit_connection(true).unwrap();
        let attachment = f.host.admit_attachment().unwrap();
        f.host.begin_drain();
        f.host.begin_drain();
        assert!(matches!(
            f.host.admit_request(std::convert::identity),
            Err(HostAdmissionError::ServerDraining)
        ));
        assert!(matches!(
            f.host.admit_attachment(),
            Err(HostAdmissionError::ServerDraining)
        ));
        assert!(f.host.admit_connection(true).is_none());
        assert!(f.host.finish_drain().is_err());
        // A request admitted before the host commit may still load. The lower
        // manager has no duplicate server gate. Drain must include this slot.
        let host = f.host.clone();
        let mut drain = Box::pin(host.drain());
        assert!(futures_util::poll!(&mut drain).is_pending());
        let a = f.load(0).await.unwrap().unwrap();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        drop(request);
        assert!(drain.await.is_empty());
        assert!(f.host.finish_drain().is_err());
        drop(connection);
        assert!(f.host.finish_drain().is_err());
        drop(attachment);
        f.host.finish_drain().unwrap();
        let snapshot = f.host.diagnostics();
        assert_eq!(snapshot.lifecycle, ServerLifecycle::Terminated);
        assert_eq!(snapshot.loaded + snapshot.loading + snapshot.unloading, 0);
        assert_eq!(snapshot.external_attachments, 0);
        assert_eq!(snapshot.transport.websocket_connections, 0);
        assert_eq!(snapshot.transport.connection_refusals, 1);
        assert_eq!(snapshot.admission_refusals["server_draining"], 3);
        f.close().await;
    })
    .await;
}

#[test]
fn residency_owner_does_not_depend_on_app_server() {
    let source = include_str!("../../../src/local_runtime/session_runtime_manager.rs");
    assert!(!source.contains("crate::app_server"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_request_and_runtime_lease_commit_before_drain_as_one_admission() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let probe = f.manager.probe(a.conversation_id());
        probe.before_operation.arm();
        let gap = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let parked = gap.arm_scoped();
        let host = f.host.clone();
        let gate = gap.clone();
        let client = a.client();
        let admission = tokio::task::spawn_blocking(move || {
            host.admit_request(|owner| {
                // The actual synchronous callback used by protocol dispatch:
                // host request counted, runtime operation not admitted yet.
                gate.enter();
                client.start_operation(move || async move {
                    let _owner = owner;
                    291
                })
            })
            .unwrap()
            .unwrap()
        });
        gate_entered(&gap).await;
        assert!(
            f.host.admission_boundary_is_held(),
            "drain cannot commit in the former admission gap"
        );
        let host = f.host.clone();
        let draining = tokio::spawn(async move { host.drain().await });
        drop(parked);
        let response = admission.await.unwrap();
        probe.before_operation.entered().await;
        probe
            .draining_operations
            .subscribe()
            .wait_for(|draining| *draining)
            .await
            .unwrap();
        assert!(!draining.is_finished());
        let called = std::sync::atomic::AtomicBool::new(false);
        assert!(matches!(
            f.host
                .admit_request(|_| called.store(true, Ordering::SeqCst)),
            Err(HostAdmissionError::ServerDraining)
        ));
        assert!(!called.load(Ordering::SeqCst));
        probe.before_operation.release();
        assert_eq!(response.await.unwrap(), 291);
        assert!(draining.await.unwrap().is_empty());
        f.host.finish_drain().unwrap();
        f.close().await;
    })
    .await;
}
