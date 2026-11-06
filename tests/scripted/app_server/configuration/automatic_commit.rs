//! Automatic configuration commit by live Sessions: native commits a saved
//! configuration at its own safe boundary with no client action, admitted
//! Attempts keep their capture, real owners defer the commit until they
//! settle, newer desire supersedes staged work, and commit orders against
//! independent admission.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

fn system_prompt(runtime: &crate::runtime::ConversationRuntime) -> String {
    let resources = runtime.runtime_resources();
    resources
        .context_assembly()
        .system_sections(&resources.native_system_input())
        .map(|sections| crate::context::render_effective_system_prompt(&sections))
        .unwrap()
}

async fn select_all_skills(fixture: &Fixture) {
    write(
        fixture,
        0,
        ConfigMutation::Skills {
            authored: Some(crate::runtime::agent_profile::AgentSkillSelection::All),
        },
    )
    .await;
    applied(fixture, 0).await;
}

fn instructions(runtime: &crate::runtime::ConversationRuntime) -> String {
    runtime
        .runtime_resources()
        .configuration()
        .unwrap()
        .config
        .agent
        .instructions
        .clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue383_disabled_extension_keeps_admitted_configuration_until_settlement() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(Some("read"), 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Todo {
                authored: Some(crate::extensions::TodoExtensionDocument { enabled: true }),
            },
        )
        .await;
        applied(&fixture, 0).await;
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .admitted_attempt
            .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Todo {
                authored: Some(crate::extensions::TodoExtensionDocument { enabled: false }),
            },
        )
        .await;
        // Prepared and staged: the running Attempt owns the current generation.
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Capabilities],
            UnitApplication::Ready,
            "{staged:?}"
        );
        assert!(runtime.has_staged_configuration());
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .admitted_attempt
            .unwrap(),
            admitted
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        // Settlement alone commits the staged candidate: no save, click or
        // message drives it.
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied
        );
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 2);
        for body in &requests {
            let request: serde_json::Value = serde_json::from_str(body).unwrap();
            assert!(
                request["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == "todo"),
                "every step of the admitted Attempt retains Todo"
            );
        }
        let change = configuration_changes(&runtime).pop().unwrap();
        assert_eq!(change.tools.removed.names, ["todo"]);
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        let future: serde_json::Value =
            serde_json::from_str(fixture.provider.request_bodies().last().unwrap()).unwrap();
        assert!(
            !future["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "todo")
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t04_every_resident_session_commits_and_t09_policy_and_healthy_rescan_are_noops() {
    let fixture = Fixture::new().await;
    for session in &fixture.sessions {
        fixture.manager.load(&session.id, None).await.unwrap();
    }
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("P2".into()),
        },
    )
    .await;
    let runtimes: Vec<_> = fixture
        .sessions
        .iter()
        .map(|s| fixture.manager.configuration_runtime(&s.id).unwrap())
        .collect();
    for (index, runtime) in runtimes.iter().enumerate() {
        let view = applied(&fixture, index).await;
        assert_eq!(
            view.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert_eq!(instructions(runtime), "P2");
        let changes = configuration_changes(runtime);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].domains,
            [crate::runtime::configuration_commit::ConfigurationDomain::Instructions]
        );
        assert!(changes[0].tools.is_empty());
    }
    write(
        &fixture,
        0,
        ConfigMutation::Approval {
            authored: Some(crate::runtime::ApprovalMode::FullAccess),
        },
    )
    .await;
    applied(&fixture, 0).await;
    applied(&fixture, 1).await;
    for runtime in &runtimes {
        assert_eq!(instructions(runtime), "P2");
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(runtime)
                .unwrap()
                .approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
        // The independently published policy unit records its own fact,
        // without rebinding the Session.
        let changes = configuration_changes(runtime);
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(
            changes[1].domains,
            [crate::runtime::configuration_commit::ConfigurationDomain::ExecutionPolicy]
        );
        assert_eq!(
            changes[1].previous_binding_revision,
            changes[1].binding_revision
        );
    }
    let revision = runtimes[1].runtime_resources().revision();
    let changes = configuration_changes(&runtimes[1]).len();
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[1].clone(),
            },
        )
        .await
        .unwrap();
    applied(&fixture, 1).await;
    assert_eq!(runtimes[1].runtime_resources().revision(), revision);
    assert_eq!(
        configuration_changes(&runtimes[1]).len(),
        changes,
        "an unchanged source records no change"
    );
    assert!(
        fixture.provider.request_bodies().is_empty(),
        "commit invokes no model"
    );
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_newer_desire_supersedes_a_staged_candidate_and_rescan_does_not_reprepare() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("candidate one".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(settled(&fixture, 0).await, staged);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("candidate two".into()),
            },
        )
        .await;
        let newer = settled(&fixture, 0).await;
        assert_ne!(newer.desired, staged.desired);
        assert_eq!(
            newer.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        assert_ne!(instructions(&runtime), "candidate one");
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        assert_eq!(instructions(&runtime), "candidate two");
        assert_eq!(
            configuration_changes(&runtime).len(),
            1,
            "the superseded candidate never committed"
        );
        fixture.close().await;
    })
    .await;
}

#[tokio::test]
async fn t15_unload_load_retains_the_committed_configuration_without_replay() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    let loaded = fixture.manager.load(id, None).await.unwrap();
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("committed before unload".into()),
        },
    )
    .await;
    applied(&fixture, 0).await;
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(instructions(&runtime), "committed before unload");
    assert_eq!(configuration_changes(&runtime).len(), 1);
    fixture.manager.unload(&loaded.conversation).await.unwrap();
    fixture.manager.load(id, None).await.unwrap();
    let reloaded = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(instructions(&reloaded), "committed before unload");
    assert_eq!(
        configuration_changes(&reloaded).len(),
        1,
        "reconstruction never replays a historical change"
    );
    assert!(
        fixture.provider.request_bodies().is_empty(),
        "commit and reload invoke no model"
    );
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t01_new_attempt_captures_automatic_policy_old_attempt_retains_capture() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let captured =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .admitted_attempt
            .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Approval {
                authored: Some(crate::runtime::ApprovalMode::FullAccess),
            },
        )
        .await;
        write(
            &fixture,
            0,
            ConfigMutation::ModelTimeout {
                authored: Some(crate::local_runtime::authoring::TimeoutLayer {
                    response_start_timeout_ms: Some(41_000),
                    stream_idle_timeout_ms: Some(42_000),
                }),
            },
        )
        .await;
        let applied = settled(&fixture, 0).await;
        assert_eq!(
            applied.units[&ApplyUnit::ExecutionPolicy],
            UnitApplication::Applied
        );
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .admitted_attempt
            .unwrap(),
            captured
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        let later = crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
            &runtime,
        )
        .unwrap()
        .admitted_attempt
        .unwrap();
        assert_eq!(
            later.approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
        assert!(later.generation > captured.generation);
        assert_ne!(later.model_timeout, captured.model_timeout);
        fixture.gates[1].release();
        settlement.wait().await;
        fixture.close().await;
    })
    .await;
}

/// A model timeout or a Tool deadline edited alone is an effective
/// execution-policy change: the running Attempt keeps its capture, the next
/// one freezes the successor, and each commit records exactly one fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_and_deadline_edits_alone_each_record_one_execution_policy_fact() {
    bounded(async {
        use crate::local_runtime::configuration::settings::EffectiveConfiguration;
        use crate::runtime::configuration_commit::ConfigurationDomain;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let admitted = |runtime: &crate::runtime::ConversationRuntime| {
            EffectiveConfiguration::adopted(runtime)
                .unwrap()
                .admitted_attempt
                .unwrap()
        };
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let captured = admitted(&runtime);
        write(
            &fixture,
            0,
            ConfigMutation::ModelTimeout {
                authored: Some(crate::local_runtime::authoring::TimeoutLayer {
                    response_start_timeout_ms: Some(41_000),
                    stream_idle_timeout_ms: Some(42_000),
                }),
            },
        )
        .await;
        settled(&fixture, 0).await;
        assert_eq!(admitted(&runtime), captured);
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].domains, [ConfigurationDomain::ExecutionPolicy]);
        fixture.gates[0].release();
        settlement.wait().await;
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        let timed = admitted(&runtime);
        assert_ne!(timed.model_timeout, captured.model_timeout);
        assert_eq!(timed.tool_deadline, captured.tool_deadline);
        write(
            &fixture,
            0,
            ConfigMutation::ToolDeadline {
                authored: Some(crate::local_runtime::authoring::ToolDeadlineLayer {
                    hard_deadline_ms: Some(43_000),
                    idle_liveness_ms: None,
                }),
            },
        )
        .await;
        settled(&fixture, 0).await;
        assert_eq!(admitted(&runtime), timed);
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(changes[1].domains, [ConfigurationDomain::ExecutionPolicy]);
        assert_eq!(
            changes[1].previous_binding_revision,
            changes[1].binding_revision
        );
        fixture.gates[1].release();
        settlement.wait().await;
        let effective = EffectiveConfiguration::adopted(&runtime).unwrap();
        assert_eq!(effective.tool_deadline.hard_deadline_ms, 43_000);
        assert_eq!(Some(effective.model_timeout), timed.model_timeout);
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let before = runtime.runtime_resources();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_prepare.arm();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("superseded candidate".into()),
            },
        )
        .await;
        probe.before_configuration_prepare.entered().await;
        let old = fixture
            .manager
            .configuration_application(id)
            .unwrap()
            .desired;
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        // External source ingestion, including its failure, wins the source
        // fence while old preparation is parked off-side.
        std::fs::write(
            fixture.workspaces[0].join("rustx.toml"),
            "[agent]\nagents=['missing']\n",
        )
        .unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        probe.before_configuration_prepare.release();
        let latest = settled(&fixture, 0).await;
        assert_ne!(latest.desired, old);
        assert!(matches!(
            latest.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
        // The User instructions remain desired and independently valid; they
        // are staged under the newest identity while the Attempt owns the
        // generation.
        assert_eq!(
            latest.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert!(matches!(
            committed.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
        assert_eq!(instructions(&runtime), "superseded candidate");
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_ready_candidate_commits_before_racing_independent_admission() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        // Park the admission pass before it takes the coordinator lock.
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        runtime.install_configuration_admission_gate(gate.clone());
        runtime.submit_inbound(input("request-A")).unwrap();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("committed before admission".into()),
            },
        )
        .await;
        applied(&fixture, 0).await;
        drop(release);
        fixture.gates[0].wait_entered().await;
        // The commit linearized first, so the independent Attempt froze it.
        let admitted =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .admitted_attempt
            .unwrap();
        assert_eq!(admitted.generation, runtime.runtime_resources().revision());
        let request = fixture.provider.request_bodies().pop().unwrap();
        assert!(request.contains("committed before admission"), "{request}");
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        fixture.close().await;
    })
    .await;
}

/// A complete capability candidate whose commit finds the native capability
/// boundary held stays staged and holds back the accepted input's admission:
/// once the boundary is released the worker commits it and resumes exactly that
/// admission, with no further message, and the Attempt captures the successor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_capability_commit_defers_admission_until_the_boundary_releases() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(Some("read"), 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_publish.arm();
        write(
            &fixture,
            0,
            ConfigMutation::Todo {
                authored: Some(crate::extensions::TodoExtensionDocument { enabled: true }),
            },
        )
        .await;
        probe.before_configuration_publish.entered().await;
        // The input is accepted and its admission parked before the lock.
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let admission = gate.arm_scoped();
        runtime.install_configuration_admission_gate(gate.clone());
        runtime.submit_inbound(input("request-A")).unwrap();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        // A concurrent lifecycle commit holds the capability boundary while
        // the complete candidate is staged.
        let (release, held) = std::sync::mpsc::channel();
        runtime.lifecycle_for_test().hold_commit_boundary(held);
        probe.before_configuration_publish.release();
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Capabilities],
            UnitApplication::Ready,
            "{staged:?}"
        );
        assert!(runtime.has_staged_configuration());
        drop(admission);
        while !runtime.configuration_admission_deferred() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(!runtime.has_current_attempt());
        assert!(fixture.provider.request_bodies().is_empty());
        release.send(()).unwrap();
        fixture.gates[0].wait_entered().await;
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied
        );
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1, "admitted exactly once");
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        assert!(
            request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "todo"),
            "the deferred Attempt captured the committed successor"
        );
        assert_eq!(configuration_changes(&runtime).len(), 1);
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        fixture.close().await;
    }))
    .await;
}

/// A runtime that drained itself in place (a failed physical reclamation)
/// while still resident can commit nothing: its application defers to the next
/// allocation instead of preparing again forever, and every other Session
/// still commits the same save.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runtime_closed_in_place_defers_its_application_and_others_still_commit() {
    bounded(async {
        let fixture = Fixture::new().await;
        for session in &fixture.sessions {
            fixture.manager.load(&session.id, None).await.unwrap();
        }
        let id = fixture.sessions[0].id.clone();
        let closed = fixture.manager.configuration_runtime(&id).unwrap();
        let live = fixture
            .manager
            .configuration_runtime(&fixture.sessions[1].id)
            .unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        closed.fail_residency_settlement();
        assert_ne!(
            closed.lifecycle_state(),
            crate::runtime::ConversationLifecycleState::Running
        );
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("after close".into()),
            },
        )
        .await;
        applied(&fixture, 1).await;
        assert_eq!(instructions(&live), "after close");
        let mut changed = fixture.manager.configuration_changes();
        while !fixture.manager.applications.is_deferred(id.as_ref()) {
            changed.changed().await.unwrap();
        }
        let desired = fixture
            .manager
            .configuration_application(&id)
            .unwrap()
            .desired;
        let preparations = probe
            .configuration_preparations
            .load(std::sync::atomic::Ordering::SeqCst);
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .desired,
            desired,
            "no new application identity"
        );
        assert_eq!(
            probe
                .configuration_preparations
                .load(std::sync::atomic::Ordering::SeqCst),
            preparations,
            "no further preparation"
        );
        assert!(configuration_changes(&closed).is_empty());
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// A client attaching to the runtime that already holds a Ready candidate
/// changes nothing: the candidate is neither discarded nor prepared again, and
/// it commits once, as a Source change, at the next safe boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reattaching_the_same_runtime_keeps_its_ready_candidate() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("kept across attach".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        let preparations = probe
            .configuration_preparations
            .load(std::sync::atomic::Ordering::SeqCst);
        fixture.manager.load(&id, None).await.unwrap();
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .desired,
            staged.desired
        );
        assert!(runtime.has_staged_configuration());
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        assert_eq!(instructions(&runtime), "kept across attach");
        assert_eq!(
            probe
                .configuration_preparations
                .load(std::sync::atomic::Ordering::SeqCst),
            preparations
        );
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].reason,
            crate::runtime::configuration_commit::ConfigurationChangeReason::Source
        );
        fixture.close().await;
    })
    .await;
}

/// A rebind that re-prepares for a new allocation also consumes the cold
/// deferral left beside the replaced allocation's marker, so the next attach
/// to that same allocation keeps the re-prepared Ready candidate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reallocation_consumes_its_deferral_and_a_later_attach_keeps_the_candidate() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("after reallocation".into()),
            },
        )
        .await;
        let first = settled(&fixture, 0).await;
        fixture
            .manager
            .applications
            .strand_in_replaced_allocation(id.as_ref());
        fixture.manager.load(&id, None).await.unwrap();
        let mut changed = fixture.manager.configuration_changes();
        let staged = loop {
            let application = fixture.manager.configuration_application(&id).unwrap();
            if application.desired != first.desired
                && application.units.get(&ApplyUnit::Instructions) == Some(&UnitApplication::Ready)
            {
                break application;
            }
            changed.changed().await.unwrap();
        };
        assert!(!fixture.manager.applications.is_deferred(id.as_ref()));
        let preparations = probe
            .configuration_preparations
            .load(std::sync::atomic::Ordering::SeqCst);
        fixture.manager.load(&id, None).await.unwrap();
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .desired,
            staged.desired
        );
        assert_eq!(
            probe
                .configuration_preparations
                .load(std::sync::atomic::Ordering::SeqCst),
            preparations
        );
        assert!(runtime.has_staged_configuration());
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        assert_eq!(instructions(&runtime), "after reallocation");
        assert_eq!(configuration_changes(&runtime).len(), 1);
        fixture.close().await;
    })
    .await;
}

/// A background executor that signals its start, parks until released, and
/// then completes successfully; cancellation still wins while parked.
pub(super) struct GatedBackgroundExecutor {
    started: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Sender<bool>,
}

impl GatedBackgroundExecutor {
    pub(super) fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (release, _release_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                release: release.clone(),
            },
            started_rx,
            release,
        )
    }
}

impl crate::tools::executor::ToolExecutor for GatedBackgroundExecutor {
    fn start<'a>(
        &'a self,
        _invocation: crate::tools::types::ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let started = self.started.clone();
        let mut release = self.release.subscribe();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                started.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("release channel stays open");
                crate::tools::types::ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// Accept one gated background execution directly on the runtime.
fn start_background(
    runtime: &crate::runtime::ConversationRuntime,
    call: &str,
) -> (
    crate::runtime::identity::ToolExecutionId,
    tokio::sync::watch::Receiver<bool>,
    tokio::sync::watch::Sender<bool>,
) {
    let (executor, started, release) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new(call),
        },
        tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
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
        .unwrap();
    let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        background
            .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
            .unwrap()
    else {
        panic!("background dispatch accepted")
    };
    (execution_id, started, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_model_selection_supersedes_a_candidate_blocked_by_background_ownership() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let original = instructions(&runtime);
        let (execution_id, mut started, release) =
            start_background(&runtime, "live-selection-background");
        started.wait_for(|entered| *entered).await.unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("commits after background settles".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );

        let selected = crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        );
        fixture
            .manager
            .set_model(&id, selected.clone())
            .await
            .unwrap();
        assert_eq!(runtime.model_view().configured, selected);
        let background = runtime.tool_runtime().background();
        assert!(
            background.configuration_busy(),
            "selection retains physical background ownership"
        );
        assert_eq!(instructions(&runtime), original);
        // Selection superseded the staged candidate; the source is prepared
        // again over the explicit selection and stays staged.
        let reprepared = settled(&fixture, 0).await;
        assert_ne!(reprepared.desired, staged.desired);
        assert_eq!(
            reprepared.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );

        release.send_replace(true);
        background.wait_until_terminal(&execution_id).await.unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        applied(&fixture, 0).await;
        assert_eq!(instructions(&runtime), "commits after background settles");
        assert_eq!(
            runtime.model_view().configured,
            selected,
            "configuration application never overwrites explicit selection"
        );
        fixture.close().await;
    })
    .await;
}

/// The catalog observer cuts the runtime's binding revision without the
/// lock a staged commit retains its binding under: a commit that lands
/// between that cut and the observer's write keeps its newer revision, so the
/// next model selection is not refused as a conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_catalog_rebase_never_rewinds_a_binding_a_commit_retained() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        // Drive the observer explicitly below. Its initial live pass must not
        // supersede the staged candidate before the test takes its cut.
        let observer = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let observer_release = observer.arm_scoped();
        *fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id)
            .catalog_observer_start
            .lock()
            .unwrap() = Some(observer.clone());
        fixture.manager.load(&id, None).await.unwrap();
        gate_entered(&observer).await;
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let retained = |fixture: &Fixture| {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()
                .get(&id)
                .unwrap()
                .binding_revision
        };
        let (execution_id, mut started, release) =
            start_background(&runtime, "call-rebase-background");
        started.wait_for(|entered| *entered).await.unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("committed between the cut and the write".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        // Ready publishes before the preparation worker's final iteration.
        // Let it exit before holding its lock across the observer cut, so it
        // cannot strand the runtime's commit wake on a blocked Tokio worker.
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        let before = runtime.configuration_cut().binding_revision;
        assert!(runtime.has_staged_configuration());
        assert_eq!(retained(&fixture), before);
        let hook = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let rebase_release = hook.arm_scoped();
        fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .catalog_rebase_cut = Some(hook.clone());
        let incarnation = fixture.manager.resident_incarnation(&id).unwrap();
        let rebase = tokio::task::spawn_blocking({
            let (manager, id, runtime) = (fixture.manager.clone(), id.clone(), runtime.clone());
            move || manager.rebase_runtime_catalog(&id, incarnation, &runtime)
        });
        tokio::task::spawn_blocking({
            let hook = hook.clone();
            move || {
                hook.wait_entered();
            }
        })
        .await
        .unwrap();
        // The observer holds its cut at `before`; the staged commit lands now.
        release.send_replace(true);
        runtime
            .tool_runtime()
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .unwrap();
        // Preparing the background continuation's provider request can need
        // the application lock this observer holds. Observe the commit at
        // the coordinator before releasing that lock, not at the provider.
        while runtime.configuration_cut().binding_revision == before {
            tokio::task::yield_now().await;
        }
        let committed = runtime.configuration_cut().binding_revision;
        assert_eq!(retained(&fixture), committed);
        drop(rebase_release);
        rebase.await.unwrap();
        // The same observer can run again without re-entering the test gate.
        // This also covers a live publication racing the explicit rebase.
        fixture
            .manager
            .rebase_runtime_catalog(&id, incarnation, &runtime);
        assert_eq!(
            retained(&fixture),
            committed,
            "the observer rewound the retained binding to its stale cut"
        );
        drop(observer_release);
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        applied(&fixture, 0).await;
        let selected = crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        );
        fixture
            .manager
            .set_model(&id, selected.clone())
            .await
            .unwrap();
        assert_eq!(runtime.model_view().configured, selected);
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_background_ownership_defers_commit_until_it_settles() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let original = instructions(&runtime);
        let (execution_id, mut started, release) =
            start_background(&runtime, "call-background-gate");
        started.wait_for(|entered| *entered).await.unwrap();
        // Attribution: no foreground attempt and no subagent owns work; the
        // background registry alone owns the generation.
        assert!(
            runtime
                .subagent_registry()
                .is_none_or(|s| !s.configuration_busy())
        );
        assert!(runtime.tool_runtime().background().configuration_busy());
        assert_eq!(
            runtime.idle_epoch(),
            Err(crate::runtime::conversation_runtime::IdleBusyReason::Background)
        );
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("commit after background settles".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        assert_eq!(instructions(&runtime), original);
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        release.send_replace(true);
        let terminal = runtime
            .tool_runtime()
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .unwrap();
        assert!(matches!(
            terminal.state,
            crate::tools::background::BackgroundLifecycle::Succeeded
        ));
        // The execution's own terminal notification is consumed by one
        // continuation attempt; it, not the commit, is the only model traffic
        // settlement causes.
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert_eq!(instructions(&runtime), "commit after background settles");
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations,
            "settlement prepared nothing"
        );
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(
            requests[0].contains("Background execution"),
            "the single request is the background terminal continuation: {}",
            requests[0]
        );
        fixture.close().await;
    })
    .await;
}

/// Issue #518: an external edit of a supported Workspace source reaches the
/// same native path as a Save through the bounded watcher — a notification is
/// only invalidation — and commits automatically, recording one change fact.
/// An edit of an ordinary project file in the same Workspace changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_workspace_edit_commits_through_the_bounded_source_watch() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let before = configuration_changes(&runtime).len();
        let stop = tokio_util::sync::CancellationToken::new();
        let edit = async {
            let mut changed = fixture.manager.configuration_changes();
            std::fs::write(fixture.workspaces[0].join("notes.txt"), "not a source").unwrap();
            let path = fixture.workspaces[0].join("rustx.toml");
            let mut config: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            config.as_table_mut().unwrap().insert(
                "agent".into(),
                toml::Value::try_from(
                    serde_json::json!({"instructions": "from an external editor"}),
                )
                .unwrap(),
            );
            std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
            while instructions(&runtime) != "from an external editor" {
                changed.changed().await.unwrap();
            }
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), edit);
        watched.unwrap();
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), before + 1, "{changes:?}");
        assert!(
            changes[before]
                .domains
                .contains(&crate::runtime::configuration_commit::ConfigurationDomain::Instructions)
        );
        fixture.close().await;
    })
    .await;
}

/// Body edits stay inert; frontmatter edits publish exactly one definition change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_resources_stay_inert_and_frontmatter_changes_are_recorded() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let skill = fixture.workspaces[0].join(".agents/skills/guide");
        std::fs::create_dir_all(&skill).unwrap();
        let frontmatter = "---\nname: guide\ndescription: Project guide\n---\n";
        std::fs::write(skill.join("SKILL.md"), format!("{frontmatter}first body\n")).unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        select_all_skills(&fixture).await;
        let before = configuration_changes(&runtime).len();
        let revision = runtime.runtime_resources().capability().revision();
        let entries = runtime
            .runtime_resources()
            .capability()
            .model_skill_entries()
            .to_vec();
        assert!(
            entries.iter().any(|entry| entry.name == "guide"),
            "{entries:?}"
        );
        std::fs::write(
            skill.join("SKILL.md"),
            format!("{frontmatter}second body\n"),
        )
        .unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        applied(&fixture, 0).await;
        assert_eq!(
            runtime
                .runtime_resources()
                .capability()
                .model_skill_entries(),
            entries.as_slice()
        );
        assert_eq!(configuration_changes(&runtime).len(), before);
        assert_eq!(
            runtime.runtime_resources().capability().revision(),
            revision
        );
        // Only a changed definition, including a preserved metadata field,
        // starts configuration preparation and publishes a new version.
        let frontmatter = "---\nname: guide\ndescription: Project guide\nlicense: MIT\n---\n";
        std::fs::write(skill.join("SKILL.md"), frontmatter).unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        applied(&fixture, 0).await;
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), before + 1, "{changes:?}");
        assert_eq!(changes[before].skills.changed.names, ["guide"]);
        assert!(
            changes[before].skills.added.is_empty() && changes[before].skills.removed.is_empty()
        );
        let frontmatter = frontmatter.replace("license: MIT", "license: Apache-2.0");
        // A third version with no request between them: each fact keeps the
        // version it installed, readable after the file moved on and the
        // runtime that recorded them is gone.
        std::fs::write(skill.join("SKILL.md"), format!("{frontmatter}third body\n")).unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        applied(&fixture, 0).await;
        assert!(fixture.provider.request_bodies().is_empty());
        let store = runtime.tool_runtime().durable_store();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("{frontmatter}current body\n"),
        )
        .unwrap();
        drop(runtime);
        let _ = fixture.manager.drain_all_runtimes().await;
        let catalogs = configuration_catalogs(&store);
        let versions = catalogs[before..]
            .iter()
            .map(|catalog| {
                assert!(catalog["tools"].as_array().unwrap().is_empty(), "{catalog}");
                let [skill] = catalog["skills"].as_array().unwrap().as_slice() else {
                    panic!("one affected Skill: {catalog}");
                };
                assert_eq!(skill["name"], "guide");
                assert_eq!(skill["change"], "changed");
                assert_ne!(skill["before"]["version_id"], skill["after"]["version_id"]);
                skill["after"]["version_id"].as_str().unwrap().to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(versions.len(), 2);
        assert_ne!(versions[0], versions[1]);
        fixture.close().await;
    })
    .await;
}

/// A selected Workflow whose program changes under an identical name,
/// description and input schema is a `changed` Tool: its implementation is
/// the enabled program, and the next call runs the successor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_program_only_change_records_one_changed_fact() {
    bounded(async {
        let fixture = Fixture::with_tool(Some("review")).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let agent = workspace.join(".agents/agents/reviewer.toml");
        let workflow = workspace.join(".agents/workflows/review.yaml");
        std::fs::create_dir_all(agent.parent().unwrap()).unwrap();
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        std::fs::write(&agent, "description='Review'\ninstructions='reviewer'\n").unwrap();
        let program = |task: &str| {
            let schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
            serde_json::to_string(&serde_json::json!({
                "description": "Review",
                "block": {"input": schema, "nodes": {"workflow_start": {"type": "start"},
                    "agent": {"type": "agent", "profile": "reviewer", "task": [{"type": "literal", "value": task}], "output": {"type": "json_schema", "schema": schema}},
                    "done": {"type": "return", "outputs": []}},
                    "edges": [{"from": "workflow_start", "to": "agent"}, {"from": "agent", "to": "done"}]}
            }))
            .unwrap()
        };
        std::fs::write(&workflow, program("Review task A")).unwrap();
        std::fs::write(workspace.join("rustx.toml"), "[agent]\nworkflows=['review']\n").unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let declaration = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .model_definitions()
                .into_iter()
                .find(|definition| definition.name == "review")
                .expect("review offered")
        };
        let identity = |runtime: &crate::runtime::ConversationRuntime| {
            match &runtime.runtime_resources().workflows().entries()
                [&crate::runtime::workflow::WorkflowId::parse("review").unwrap()]
                .admission
            {
                crate::runtime::workflow::WorkflowAdmission::Enabled(program) => {
                    program.tool_identity()
                }
                crate::runtime::workflow::WorkflowAdmission::Disabled(_) => {
                    panic!("review enabled")
                }
            }
        };
        let (before, old_identity) = (declaration(&runtime), identity(&runtime));
        let facts = configuration_changes(&runtime).len();
        std::fs::write(&workflow, program("Review task B")).unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
            )
            .await
            .unwrap();
        applied(&fixture, 0).await;
        assert_eq!(declaration(&runtime), before);
        assert_ne!(identity(&runtime), old_identity);
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 1, "{changes:?}");
        let change = &changes[facts];
        assert_eq!(change.tools.changed.names, ["review"]);
        assert!(change.tools.added.is_empty() && change.tools.removed.is_empty());
        let catalog = configuration_catalogs(&runtime.tool_runtime().durable_store())
            .pop()
            .unwrap();
        assert_eq!(
            catalog["tools"][0]["after"]["workflow"]["program_digest"],
            identity(&runtime).program_digest.as_str()
        );
        let mut done = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        done.wait().await;
        let requests = fixture.provider.request_bodies();
        assert!(requests.iter().any(|body| body.contains("Review task B")), "{requests:?}");
        assert!(!requests.iter().any(|body| body.contains("Review task A")));
        fixture.close().await;
    })
    .await;
}

/// A Tool runs the Named Agents it admits, so an instructions-only edit of
/// an Agent the `review` Workflow froze and `subagent` delegates to changes
/// both Tools, and the next Workflow call runs the edited Agent; an edit of
/// an Agent nothing admits records nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_agent_edits_change_exactly_the_tools_that_run_them() {
    bounded(async {
        let fixture = Fixture::with_tool(Some("review")).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let agents = workspace.join(".agents/agents");
        let workflow = workspace.join(".agents/workflows/review.yaml");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        let profile = |instructions: &str| {
            format!("description='Review'\ninstructions='{instructions}'\n")
        };
        std::fs::write(agents.join("reviewer.toml"), profile("reviewer A")).unwrap();
        std::fs::write(agents.join("unused.toml"), profile("unused A")).unwrap();
        let schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        std::fs::write(&workflow, serde_json::to_string(&serde_json::json!({
            "description": "Review",
            "block": {"input": schema, "nodes": {"workflow_start": {"type": "start"},
                "agent": {"type": "agent", "profile": "reviewer", "task": [{"type": "literal", "value": "Review"}], "output": {"type": "json_schema", "schema": schema}},
                "done": {"type": "return", "outputs": []}},
                "edges": [{"from": "workflow_start", "to": "agent"}, {"from": "agent", "to": "done"}]}
        })).unwrap()).unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\nworkflows=['review']\nagents=['reviewer']\n",
        )
        .unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let declarations = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .model_definitions()
        };
        let before = declarations(&runtime);
        let reconcile = || async {
            fixture
                .manager
                .reconcile_configuration(
                    &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                        directory: workspace.clone(),
                    },
                )
                .await
                .unwrap();
            applied(&fixture, 0).await;
        };
        let facts = configuration_changes(&runtime).len();
        std::fs::write(agents.join("unused.toml"), profile("unused B")).unwrap();
        reconcile().await;
        assert_eq!(configuration_changes(&runtime).len(), facts, "nothing runs unused");
        std::fs::write(agents.join("reviewer.toml"), profile("reviewer B")).unwrap();
        reconcile().await;
        assert_eq!(declarations(&runtime), before, "declarations are unchanged");
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 1, "{changes:?}");
        assert_eq!(changes[facts].tools.changed.names, ["review", "subagent"]);
        assert!(changes[facts].tools.added.is_empty() && changes[facts].tools.removed.is_empty());
        let catalog = configuration_catalogs(&runtime.tool_runtime().durable_store())
            .pop()
            .unwrap();
        for tool in catalog["tools"].as_array().unwrap() {
            assert_ne!(
                tool["before"]["agents"]["reviewer"],
                tool["after"]["agents"]["reviewer"],
                "{tool}"
            );
        }
        let mut done = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        done.wait().await;
        let requests = fixture.provider.request_bodies();
        assert!(requests.iter().any(|body| body.contains("reviewer B")), "{requests:?}");
        assert!(!requests.iter().any(|body| body.contains("reviewer A")));
        fixture.close().await;
    })
    .await;
}

/// A Named Agent's identity is its resolved execution, not only its
/// definition: a Skill only a delegated child selects changes the `subagent`
/// Tool when its content changes, while a Skill nothing runs changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_only_skill_version_changes_the_tool_that_runs_it() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let agents = workspace.join(".agents/agents");
        let skills = workspace.join(".agents/skills");
        std::fs::create_dir_all(&agents).unwrap();
        let skill = |name: &str, body: &str| {
            std::fs::create_dir_all(skills.join(name)).unwrap();
            std::fs::write(
                skills.join(name).join("SKILL.md"),
                format!(
                    "---\nname: {name}\ndescription: Guide {name}\nlicense: {body}\n---\nbody\n"
                ),
            )
            .unwrap();
        };
        skill("guide", "body A");
        skill("idle", "body A");
        std::fs::write(
            agents.join("reviewer.toml"),
            "description='Review'\ninstructions='Review the task'\nskills=['guide']\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\nagents=['reviewer']\nskills=[]\n",
        )
        .unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let resources = runtime.runtime_resources();
        assert!(resources.capability().model_skill_entries().is_empty());
        let declarations = resources.capability().tool_registry().model_definitions();
        let prompt = system_prompt(&runtime);
        let reconcile = || async {
            fixture
                .manager
                .reconcile_configuration(
                    &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                        directory: workspace.clone(),
                    },
                )
                .await
                .unwrap();
            applied(&fixture, 0).await;
        };
        let facts = configuration_changes(&runtime).len();
        skill("idle", "body B");
        reconcile().await;
        assert_eq!(
            configuration_changes(&runtime).len(),
            facts,
            "nothing runs idle"
        );
        let version = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .skills()
                .packages()
                .iter()
                .find(|package| package.name() == "guide")
                .map(|package| package.version_id().clone())
                .unwrap()
        };
        let before = version(&runtime);
        skill("guide", "body B");
        reconcile().await;
        assert_ne!(version(&runtime), before, "the child runs the new version");
        let resources = runtime.runtime_resources();
        assert!(resources.capability().model_skill_entries().is_empty());
        assert_eq!(
            resources.capability().tool_registry().model_definitions(),
            declarations
        );
        assert_eq!(system_prompt(&runtime), prompt);
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 1, "{changes:?}");
        assert_eq!(changes[facts].tools.changed.names, ["subagent"]);
        assert!(changes[facts].skills.changed.names.is_empty());
        let catalog = configuration_catalogs(&runtime.tool_runtime().durable_store())
            .pop()
            .unwrap();
        let tool = &catalog["tools"][0];
        assert_eq!(tool["name"], "subagent");
        let (old, new) = (
            &tool["before"]["agents"]["reviewer"],
            &tool["after"]["agents"]["reviewer"],
        );
        assert_eq!(
            old["definition"], new["definition"],
            "the definition is unchanged"
        );
        assert_ne!(old["skills"]["guide"], new["skills"]["guide"], "{tool}");
        fixture.close().await;
    })
    .await;
}

/// Invocation overrides read the current inert catalog, even when no profile
/// selects the changed Skill. Existing Attempt captures keep the old catalog;
/// publication neither prepares dependencies nor records an effective change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unselected_skill_catalog_updates_future_overrides_without_preparation_or_facts() {
    bounded(async {
        use crate::runtime::subagent::{
            SubagentInvocationOverride, SubagentName, SubagentResolution,
            SubagentResolutionError, SubagentResolver,
        };
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let agents = workspace.join(".agents/agents");
        let skills = workspace.join(".agents/skills");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.toml"),
            "description='Review'\ninstructions='Review the task'\nskills=[]\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\nagents=['reviewer']\nskills=[]\n",
        )
        .unwrap();
        let skill = |name: &str, description: &str, disabled: bool, pin: &str| {
            std::fs::create_dir_all(skills.join(name)).unwrap();
            std::fs::write(skills.join(name).join("SKILL.md"), format!(
                "---\nname: {name}\ndescription: {description}\ndisable-model-invocation: {disabled}\nmetadata:\n  rustx.python-dependencies: '{{\"rustx-review-must-not-install\":\"{pin}\"}}'\n---\nbody\n"
            )).unwrap();
        };
        skill("idle", "Original", false, "1.0");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let old = runtime.runtime_resources();
        let capability_revision = old.capability().revision();
        let facts = configuration_changes(&runtime).len();
        let resolve = |resources: &crate::runtime::RuntimeResourceSnapshot, name: &str| {
            let agent = SubagentName::parse("reviewer").unwrap();
            let invocation = SubagentInvocationOverride {
                skills: Some(vec![name.into()]),
                ..Default::default()
            };
            let model = runtime.model_state_for_test();
            SubagentResolver::resolve(&SubagentResolution {
                resources,
                agent: &agent,
                attempt_model: model.config(),
                models: &resources.configuration().unwrap().models,
                invocation: Some(&invocation),
            })
        };
        let reconcile = || async {
            fixture.manager.reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
            ).await.unwrap();
            settled(&fixture, 0).await
        };
        assert_eq!(resolve(&old, "idle").unwrap().skills[0].catalog_entry.description, "Original");
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        skill("idle", "Updated", false, "2.0");
        skill("new-skill", "New", false, "1.0");
        let staged = reconcile().await;
        assert_eq!(staged.units[&ApplyUnit::Instructions], UnitApplication::Ready);
        assert!(Arc::ptr_eq(&old, &runtime.runtime_resources()));
        assert_eq!(runtime.capability().current_snapshot().skills(), old.capability().skills());
        skill("idle", "Latest", false, "3.0");
        let latest = reconcile().await;
        assert_ne!(latest.desired, staged.desired);
        assert!(Arc::ptr_eq(&old, &runtime.runtime_resources()));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        let updated = runtime.runtime_resources();
        let idle = resolve(&updated, "idle").unwrap();
        assert_eq!(idle.skills[0].catalog_entry.description, "Latest");
        assert_eq!(idle.skills[0].dependencies.python["rustx-review-must-not-install"], "3.0");
        assert_eq!(resolve(&updated, "new-skill").unwrap().skills[0].catalog_entry.description, "New");
        assert_eq!(resolve(&old, "idle").unwrap().skills[0].catalog_entry.description, "Original");
        assert!(matches!(resolve(&old, "new-skill"), Err(SubagentResolutionError::UnknownSkill { .. })));
        assert_eq!(runtime.capability().current_snapshot().skills(), updated.capability().skills());
        runtime.refresh_runtime_catalogs().await.unwrap();
        assert_eq!(resolve(&runtime.runtime_resources(), "idle").unwrap().skills, idle.skills);
        skill("idle", "Disabled", true, "3.0");
        std::fs::remove_dir_all(skills.join("new-skill")).unwrap();
        reconcile().await;
        applied(&fixture, 0).await;
        let removed = runtime.runtime_resources();
        assert!(matches!(resolve(&removed, "idle"), Err(SubagentResolutionError::SkillNotModelVisible { .. })));
        assert!(matches!(resolve(&removed, "new-skill"), Err(SubagentResolutionError::UnknownSkill { .. })));
        assert_eq!(removed.capability().revision(), capability_revision);
        assert_eq!(configuration_changes(&runtime).len(), facts);
        assert!(removed.capability().python_environment().is_none());
        fixture.manager.unload(&fixture.sessions[0].active_conversation_id).await.unwrap();
        skill("cold-skill", "Added while cold", false, "1.0");
        let mut changes = fixture.manager.configuration_changes();
        fixture.manager.reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: workspace.clone(),
            },
        ).await.unwrap();
        while !fixture.manager.applications.is_deferred(id.as_ref()) {
            changes.changed().await.unwrap();
        }
        assert!(fixture.manager.configuration_runtime(&id).is_none());
        fixture.manager.load(&id, None).await.unwrap();
        applied(&fixture, 0).await;
        let reloaded = fixture.manager.configuration_runtime(&id).unwrap();
        let cold = reloaded.runtime_resources();
        assert_eq!(resolve(&cold, "cold-skill").unwrap().skills[0].catalog_entry.description, "Added while cold");
        assert!(matches!(resolve(&cold, "idle"), Err(SubagentResolutionError::SkillNotModelVisible { .. })));
        assert_eq!(configuration_changes(&reloaded).len(), facts);
        assert!(cold.capability().python_environment().is_none());
        fixture.close().await;
    }).await;
}

/// The Model domain compares the invocations that run, not the Session's
/// client view: a parameter change of the model only a delegated Agent
/// selects and a provider endpoint move under unchanged selections are each
/// one Model fact, while adding a model nothing selects records nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_facts_follow_the_invocations_that_run() {
    bounded(async {
        use crate::local_runtime::configuration::settings::SourceTarget;
        use crate::runtime::configuration_commit::ConfigurationDomain;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let agents = workspace.join(".agents/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.toml"),
            "description='Review'\ninstructions='Review'\n[model]\nmodel='local/b'\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\nagents=['reviewer']\n",
        )
        .unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let view = runtime.model_view();
        let models = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .authored
            .unwrap()
            .models
            .unwrap();
        let facts = configuration_changes(&runtime).len();
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/c".into(),
                authored: Some(models["local/a"].clone()),
            },
        )
        .await;
        applied(&fixture, 0).await;
        assert_eq!(
            configuration_changes(&runtime).len(),
            facts,
            "nothing selects local/c"
        );
        let mut tuned = models["local/b"].clone();
        tuned.request_params = Some(crate::toml_authoring::AuthoredRequestParams(
            serde_json::from_value(serde_json::json!({"temperature": 0.25})).unwrap(),
        ));
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/b".into(),
                authored: Some(tuned),
            },
        )
        .await;
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view, "the Session view is unchanged");
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 1, "{changes:?}");
        assert_eq!(changes[facts].domains, [ConfigurationDomain::Model]);
        write(
            &fixture,
            0,
            ConfigMutation::Provider {
                id: "local".into(),
                authored: Some(
                    crate::local_runtime::configuration::settings::ProviderWrite {
                        base_url: fixture.provider.url("/v2"),
                        credential:
                            crate::local_runtime::configuration::settings::CredentialEdit::Retain,
                    },
                ),
            },
        )
        .await;
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view, "the Session view is unchanged");
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 2, "{changes:?}");
        assert_eq!(changes[facts + 1].domains, [ConfigurationDomain::Model]);
        fixture.close().await;
    })
    .await;
}

/// A direct Model node of an enabled Workflow is a model that runs: a
/// parameter change of the model only it names is one Model fact through
/// the commit path, while adding a model nothing selects records nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_model_node_is_a_model_that_runs() {
    bounded(async {
        use crate::local_runtime::configuration::settings::SourceTarget;
        use crate::runtime::configuration_commit::ConfigurationDomain;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let workflow = workspace.join(".agents/workflows/generate.yaml");
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        let schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        std::fs::write(&workflow, serde_json::to_string(&serde_json::json!({
            "description": "Generate",
            "block": {"input": schema, "nodes": {"workflow_start": {"type": "start"},
                "generate": {"type": "model", "model": {"mode": "explicit", "model": "local/b"},
                    "messages": [{"role": "system", "content": [{"type": "literal", "value": "Return done"}]}, {"role": "user", "content": [{"type": "literal", "value": "Go."}]}],
                    "output": {"type": "text"}},
                "done": {"type": "return", "outputs": []}},
                "edges": [{"from": "workflow_start", "to": "generate"}, {"from": "generate", "to": "done"}]}
        })).unwrap()).unwrap();
        std::fs::write(workspace.join("rustx.toml"), "[agent]\nworkflows=['generate']\n").unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        assert!(
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .any(|tool| tool.name == "generate"),
            "the Workflow is enabled"
        );
        let view = runtime.model_view();
        let models = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .authored
            .unwrap()
            .models
            .unwrap();
        let facts = configuration_changes(&runtime).len();
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/c".into(),
                authored: Some(models["local/a"].clone()),
            },
        )
        .await;
        applied(&fixture, 0).await;
        assert_eq!(configuration_changes(&runtime).len(), facts, "nothing selects local/c");
        let mut tuned = models["local/b"].clone();
        tuned.request_params = Some(crate::toml_authoring::AuthoredRequestParams(
            serde_json::from_value(serde_json::json!({"temperature": 0.25})).unwrap(),
        ));
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/b".into(),
                authored: Some(tuned),
            },
        )
        .await;
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view, "the Session view is unchanged");
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 1, "{changes:?}");
        assert_eq!(changes[facts].domains, [ConfigurationDomain::Model]);
        fixture.close().await;
    })
    .await;
}

/// Provider reports `applied` only for invocations actually installed: a
/// capture that tunes the model only a Workflow Model node names and edits
/// that Workflow's prompt fails capability preparation, so Provider fails
/// too, nothing is recorded, and the runtime keeps the old Workflow.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_preparation_never_reports_an_uninstalled_workflow_model() {
    bounded(async {
        use crate::local_runtime::configuration::settings::SourceTarget;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let workflow = workspace.join(".agents/workflows/generate.yaml");
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        let program = |prompt: &str| {
            let schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
            std::fs::write(&workflow, serde_json::to_string(&serde_json::json!({
                "description": "Generate",
                "block": {"input": schema, "nodes": {"workflow_start": {"type": "start"},
                    "generate": {"type": "model", "model": {"mode": "explicit", "model": "local/b"},
                        "messages": [{"role": "system", "content": [{"type": "literal", "value": prompt}]}, {"role": "user", "content": [{"type": "literal", "value": "Go."}]}],
                        "output": {"type": "text"}},
                    "done": {"type": "return", "outputs": []}},
                    "edges": [{"from": "workflow_start", "to": "generate"}, {"from": "generate", "to": "done"}]}
            })).unwrap()).unwrap();
        };
        program("Prompt A");
        std::fs::write(workspace.join("rustx.toml"), "[agent]\nworkflows=['generate']\n").unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let resources = runtime.runtime_resources();
        let facts = configuration_changes(&runtime).len();
        // One capture: the model only the Workflow names, and its prompt.
        let user = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .path;
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&user).unwrap()).unwrap();
        document["models"]["local/b"]
            .as_table_mut()
            .unwrap()
            .insert("request_params".into(), r#"{"temperature":0.25}"#.into());
        std::fs::write(&user, toml::to_string(&document).unwrap()).unwrap();
        program("Prompt B");
        fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id)
            .fail_configuration_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
        fixture
            .manager
            .reconcile_configuration(&SourceTarget::Workspace {
                directory: workspace.clone(),
            })
            .await
            .unwrap();
        let view = applied(&fixture, 0).await;
        assert!(
            matches!(view.units[&ApplyUnit::Capabilities], UnitApplication::Failed { .. }),
            "{view:?}"
        );
        assert!(
            matches!(view.units[&ApplyUnit::Provider], UnitApplication::Failed { .. }),
            "Provider was reported for a model that was not installed: {view:?}"
        );
        assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()), "nothing installed");
        assert_eq!(configuration_changes(&runtime).len(), facts, "nothing recorded");
        fixture.close().await;
    })
    .await;
}

/// The authorized Tool environment is part of what runs: changing one value
/// changes the `subagent` Tool whose child inherits it and the Session's
/// execution policy, and the retained catalog names the variable, never its
/// value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_environment_value_change_is_recorded_without_its_value() {
    bounded(async {
        use crate::runtime::configuration_commit::ConfigurationDomain;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = fixture.workspaces[0].clone();
        let agents = workspace.join(".agents/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.toml"),
            "description='Review'\ninstructions='Review'\n[tools]\nbuiltin=['bash']\n",
        )
        .unwrap();
        let config = |mode: &str| {
            std::fs::write(
                workspace.join("rustx.toml"),
                format!("[agent]\nagents=['reviewer']\n[environment]\nMODE = \"{mode}\"\n"),
            )
            .unwrap();
        };
        config("value-alpha");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let declarations = runtime
            .runtime_resources()
            .capability()
            .tool_registry()
            .model_definitions();
        let facts = configuration_changes(&runtime).len();
        config("value-bravo");
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
            )
            .await
            .unwrap();
        applied(&fixture, 0).await;
        let resources = runtime.runtime_resources();
        assert_eq!(
            resources
                .capability()
                .effective_environment()
                .authorized_entries(),
            [("MODE".to_owned(), "value-bravo".to_owned())],
            "the child inherits the new value"
        );
        assert_eq!(
            resources.capability().tool_registry().model_definitions(),
            declarations
        );
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), facts + 1, "{changes:?}");
        assert_eq!(changes[facts].tools.changed.names, ["subagent"]);
        assert!(
            changes[facts]
                .domains
                .contains(&ConfigurationDomain::ExecutionPolicy),
            "{changes:?}"
        );
        let catalog = configuration_catalogs(&runtime.tool_runtime().durable_store())
            .pop()
            .unwrap();
        let tool = &catalog["tools"][0];
        assert_eq!(tool["name"], "subagent");
        assert_eq!(
            tool["after"]["agents"]["reviewer"]["environment"],
            serde_json::json!(["MODE"])
        );
        let text = catalog.to_string();
        assert!(
            !text.contains("value-alpha") && !text.contains("value-bravo"),
            "{text}"
        );
        fixture.close().await;
    })
    .await;
}

/// Root Tools run in the whole effective environment, so a new shared Skill
/// Python environment — a new content-addressed root, as a dependency change
/// prepares — is execution policy even with every authorized entry, Tool and
/// selected Skill unchanged, and the fact carries no path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_skill_environment_is_execution_policy() {
    bounded(async {
        use crate::runtime::configuration_commit::{
            ConfigurationChange, ConfigurationChangeReason, ConfigurationDomain,
        };
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let before = runtime.runtime_resources();
        let model = runtime.model_state_for_test();
        let environment = before.capability().effective_environment();
        let prepared = |root: &str| {
            before.with_effective_environment(environment.with_overlay(
                &crate::tools::environment::ToolEnvironmentOverlay::python(std::path::Path::new(
                    root,
                )),
            ))
        };
        let (first, second) = (prepared("/env/python-sha-a"), prepared("/env/python-sha-b"));
        assert_eq!(
            first
                .capability()
                .effective_environment()
                .authorized_entries(),
            environment.authorized_entries()
        );
        let change = |old: &crate::runtime::RuntimeResourceSnapshot,
                      new: &crate::runtime::RuntimeResourceSnapshot| {
            ConfigurationChange::between(
                ConfigurationChangeReason::Source,
                (1, 2),
                (old, &model),
                (new, &model),
            )
        };
        assert!(
            change(&first, &first).is_none(),
            "the same environment is no change"
        );
        let (fact, content) = change(&first, &second).expect("a new environment root changes");
        assert_eq!(fact.domains, [ConfigurationDomain::ExecutionPolicy]);
        assert!(fact.tools.is_empty() && fact.skills.is_empty(), "{fact:?}");
        assert!(
            !content.catalog.contains("python-sha"),
            "{}",
            content.catalog
        );
        fixture.close().await;
    })
    .await;
}

/// A removal keeps what it removed: 17 Skills present since bootstrap, with
/// no request and no earlier fact, all removed at once, stay readable in full
/// from that one fact after the runtime is gone and the files changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removal_retains_every_removed_definition() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let skills = fixture.workspaces[0].join(".agents/skills");
        let names = (0..17)
            .map(|index| format!("skill-{index:02}"))
            .collect::<Vec<_>>();
        for name in &names {
            std::fs::create_dir_all(skills.join(name)).unwrap();
            std::fs::write(
                skills.join(name).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Guide {name}\n---\nbody\n"),
            )
            .unwrap();
        }
        write(
            &fixture,
            0,
            ConfigMutation::Skills {
                authored: Some(crate::runtime::agent_profile::AgentSkillSelection::All),
            },
        )
        .await;
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        assert_eq!(
            runtime
                .runtime_resources()
                .capability()
                .model_skill_entries()
                .len(),
            17
        );
        assert!(
            configuration_changes(&runtime).is_empty(),
            "no earlier fact"
        );
        write(
            &fixture,
            0,
            ConfigMutation::Skills {
                authored: Some(crate::runtime::agent_profile::AgentSkillSelection::Exact(
                    Vec::new(),
                )),
            },
        )
        .await;
        applied(&fixture, 0).await;
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].skills.removed.total, 17);
        assert_eq!(changes[0].skills.removed.names.len(), 16);
        assert!(fixture.provider.request_bodies().is_empty());
        let store = runtime.tool_runtime().durable_store();
        drop(runtime);
        let _ = fixture.manager.drain_all_runtimes().await;
        std::fs::remove_dir_all(&skills).unwrap();
        let [catalog] = configuration_catalogs(&store).try_into().unwrap();
        let removed = catalog["skills"].as_array().unwrap();
        assert_eq!(
            removed
                .iter()
                .map(|skill| skill["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            names
        );
        for skill in removed {
            assert_eq!(skill["change"], "removed");
            assert!(skill["after"].is_null());
            assert_eq!(skill["before"]["entry"]["name"], skill["name"]);
            assert!(skill["before"]["version_id"].as_str().is_some(), "{skill}");
        }
        fixture.close().await;
    })
    .await;
}

/// Every guidance file the loader reads is watched: an external edit of the
/// User `AGENTS.md` and of a Workspace `AGENTS.MD` each commits on its own,
/// with no save, rescan or second edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_guidance_edits_commit_through_the_source_watch() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let user = fixture
            .manager
            .configuration
            .sources
            .home_directory
            .join("rustx/.agents");
        std::fs::create_dir_all(&user).unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let stop = tokio_util::sync::CancellationToken::new();
        let edit = async {
            let mut changed = fixture.manager.configuration_changes();
            std::fs::write(user.join("AGENTS.md"), "user guidance from an editor").unwrap();
            while !system_prompt(&runtime).contains("user guidance from an editor") {
                changed.changed().await.unwrap();
            }
            std::fs::write(
                fixture.workspaces[0].join("AGENTS.MD"),
                "workspace guidance from an editor",
            )
            .unwrap();
            while !system_prompt(&runtime).contains("workspace guidance from an editor") {
                changed.changed().await.unwrap();
            }
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), edit);
        watched.unwrap();
        assert_eq!(configuration_changes(&runtime).len(), 2);
        fixture.close().await;
    })
    .await;
}

/// Replacing a watched Skill directory at the same path re-registers its
/// watch before the replacement is captured: an edit landing right after that
/// capture, before the watch loop runs again, still commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_skill_directory_keeps_feeding_the_source_watch() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let agents = fixture.workspaces[0].join(".agents");
        let skill = |root: &std::path::Path, name: &str, body: &str| {
            let directory = root.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Guide {name}\n---\n{body}\n"),
            )
            .unwrap();
        };
        skill(&agents.join("skills"), "first", "first body");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        select_all_skills(&fixture).await;
        let skills = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .model_skill_entries()
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(skills(&runtime), ["first"]);
        let stop = tokio_util::sync::CancellationToken::new();
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let edit = async {
            let mut changed = fixture.manager.configuration_changes();
            skill(&agents.join("skills-next"), "second", "one");
            std::fs::rename(agents.join("skills"), agents.join("skills-old")).unwrap();
            std::fs::rename(agents.join("skills-next"), agents.join("skills")).unwrap();
            // The replacement's capture has read; this edit lands before the
            // watch loop runs again and must still be observed.
            captured.entered().await;
            skill(&agents.join("skills"), "third", "three");
            captured.release();
            while skills(&runtime) != ["second", "third"] {
                changed.changed().await.unwrap();
            }
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), edit);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// A platform rescan names no path, so it recaptures every known source: a
/// Skill added to Workspace A while notifications were lost is committed
/// although only Workspace B's document was notified, and a repeated rescan
/// records nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rescan_recaptures_sources_whose_notifications_were_lost() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 2).await;
        let id = fixture.sessions[0].id.clone();
        let skills_root = fixture.workspaces[0].join(".agents/skills");
        let skill = |name: &str| {
            let directory = skills_root.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Guide {name}\n---\nbody\n"),
            )
            .unwrap();
        };
        skill("first");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        select_all_skills(&fixture).await;
        let skills = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .model_skill_entries()
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(skills(&runtime), ["first"]);
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let rescan =
            || notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan);
        let observe = async {
            watch_settled(&fixture).await;
            // Added before the watch starts, so no platform notification names it.
            skill("second");
            while !fixture.manager.notify_source_watch(rescan()) {
                tokio::task::yield_now().await;
            }
            fixture.manager.notify_source_watch(
                notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(fixture.workspaces[1].join("rustx.toml")),
            );
            captured.entered().await;
            applied(&fixture, 0).await;
            assert_eq!(skills(&runtime), ["first", "second"], "the rescan was lost");
            let facts = configuration_changes(&runtime).len();
            captured.release();
            captured.parked(0).await;
            captured.arm();
            assert!(fixture.manager.notify_source_watch(rescan()));
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert_eq!(configuration_changes(&runtime).len(), facts);
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// On a case-folding volume a notification names a source in the case it
/// was created in, not the fixed names the loaders open: an ordinary edit of
/// `.Agents/Skills/guide/SKILL.md`, the only notification, still commits the
/// new Skill version with exactly one fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_case_variant_notification_commits_the_source_the_loaders_read() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let agents = fixture.workspaces[0].join(".agents");
        let skill = |description: &str| {
            let directory = agents.join("skills/guide");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: guide\ndescription: {description}\n---\nbody\n"),
            )
            .unwrap();
        };
        skill("Version A");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        select_all_skills(&fixture).await;
        let description = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .model_skill_entries()
                .iter()
                .map(|entry| entry.description.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(description(&runtime), ["Version A"]);
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        let facts = configuration_changes(&runtime).len();
        injected_only(&fixture);
        fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_case_aliases = true;
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            // Edited before the watch starts: the injected event is the only one.
            skill("Version B");
            let edit =
                notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(fixture.workspaces[0].join(".Agents/Skills/guide/SKILL.md"));
            while !fixture.manager.notify_source_watch(edit.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                description(&runtime),
                ["Version B"],
                "the edit was not routed"
            );
            assert_eq!(configuration_changes(&runtime).len(), facts + 1);
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// A User Skill inside a known Workspace stays a User source: its edit,
/// notified in another case of the User root, refreshes a Session in another
/// cwd that consumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_skill_inside_a_workspace_refreshes_every_consumer() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let home = fixture.workspaces[0].parent().unwrap().join("home");
        let guide = home.join("rustx/.agents/skills/guide");
        let skill = |description: &str| {
            std::fs::create_dir_all(&guide).unwrap();
            std::fs::write(
                guide.join("SKILL.md"),
                format!("---\nname: guide\ndescription: {description}\n---\nbody\n"),
            )
            .unwrap();
        };
        skill("Version A");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        select_all_skills(&fixture).await;
        let description = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .capability()
                .model_skill_entries()
                .iter()
                .map(|entry| entry.description.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(description(&runtime), ["Version A"]);
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        // The Skill's directory is also a known Workspace, whose canonical
        // path on a case-folding volume keeps `RustX`.
        fixture.manager.applications.lock().record_source(
            &home.join("RustX/.agents/skills/guide"),
            &Err("not a Session's workspace".into()),
        );
        injected_only(&fixture);
        fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_case_aliases = true;
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            skill("Version B");
            let edit =
                notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(home.join("RustX/.agents/skills/guide/SKILL.md"));
            while !fixture.manager.notify_source_watch(edit.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                description(&runtime),
                ["Version B"],
                "the User consumer was not refreshed"
            );
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// A selected instruction file outside the fixed sources is a captured
/// input: its edit, the only notification, reaches the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edited_selected_instruction_file_reaches_the_resident_runtime() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let policy = fixture.workspaces[0].join("docs/policy.md");
        std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
        std::fs::write(&policy, "Policy version A").unwrap();
        let config = fixture.workspaces[0].join("rustx.toml");
        let mut document = std::fs::read_to_string(&config).unwrap();
        document.push_str("\n[agent.agents_md]\nfiles = [\"docs/policy.md\"]\n");
        std::fs::write(&config, document).unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let policy_text = |runtime: &crate::runtime::ConversationRuntime| {
            runtime
                .runtime_resources()
                .project_instructions()
                .unwrap_or_default()
                .to_owned()
        };
        assert!(policy_text(&runtime).contains("Policy version A"));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            // Edited before the watch starts: the injected event is the only one.
            std::fs::write(&policy, "Policy version B").unwrap();
            let edit =
                notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(policy.clone());
            while !fixture.manager.notify_source_watch(edit.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(
                policy_text(&runtime).contains("Policy version B"),
                "the edit was not routed: {}",
                policy_text(&runtime)
            );
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

fn pending_notifications(fixture: &Fixture) -> std::sync::Arc<super::super::source_watch::Pending> {
    fixture
        .manager
        .registry
        .0
        .lock()
        .unwrap()
        .source_watch_signals
        .clone()
        .unwrap()
}

/// A flood of notifications while a capture is slow stays bounded: the
/// watch holds at most a fixed number of paths, then one rescan, which still
/// reaches an edit no notification named; and the watch stops while the
/// flood goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notification_flood_stays_bounded_and_rescans_into_the_edit() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let workspace = fixture.workspaces[0].clone();
        let observe = async {
            watch_settled(&fixture).await;
            let config = workspace.join("rustx.toml");
            assert!(fixture.manager.notify_source_watch(modified(&config)));
            // The pass is parked after its capture: everything below arrives
            // while the watch is busy.
            captured.entered().await;
            let mut document = std::fs::read_to_string(&config).unwrap();
            document.push_str("\n[agent]\ninstructions = \"reached by the rescan\"\n");
            std::fs::write(&config, document).unwrap();
            let pending = pending_notifications(&fixture);
            let bound = super::super::source_watch::PENDING_PATHS;
            for index in 0..4 * bound {
                fixture
                    .manager
                    .notify_source_watch(modified(&workspace.join(format!("noise-{index}"))));
                fixture
                    .manager
                    .notify_source_watch(modified(&workspace.join("noise-0")));
                assert!(pending.held().0 <= bound, "{:?}", pending.held());
            }
            assert_eq!(
                pending.held(),
                (0, true),
                "the flood saturated into a rescan"
            );
            captured.release();
            captured.parked(0).await;
            while instructions(&runtime) != "reached by the rescan" {
                applied(&fixture, 0).await;
                tokio::task::yield_now().await;
            }
            // Stopping is not starved by notifications that keep arriving.
            let flooding = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let flood = tokio::spawn({
                let (manager, flooding, workspace) =
                    (fixture.manager.clone(), flooding.clone(), workspace.clone());
                async move {
                    let mut index = 0_usize;
                    while flooding.load(std::sync::atomic::Ordering::SeqCst) {
                        manager.notify_source_watch(modified(
                            &workspace.join(format!("churn-{index}")),
                        ));
                        index = index.wrapping_add(1);
                        tokio::task::yield_now().await;
                    }
                }
            });
            stop.cancel();
            (flood, flooding)
        };
        let (watched, (flood, flooding)) =
            tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        flooding.store(false, std::sync::atomic::Ordering::SeqCst);
        flood.await.unwrap();
        fixture.close().await;
    })
    .await;
}

/// Run the source watch with no platform watches: only injected
/// notifications arrive, so an edit is observed exactly as the test says.
fn injected_only(fixture: &Fixture) {
    fixture
        .manager
        .registry
        .0
        .lock()
        .unwrap()
        .source_watch_platform = false;
}

fn settled_passes(fixture: &Fixture) -> tokio::sync::watch::Receiver<u64> {
    fixture
        .manager
        .registry
        .0
        .lock()
        .unwrap()
        .source_watch_settled
        .subscribe()
}

/// Wait until the running source watch has settled its first pass, every
/// input it found captured again, and the worker is idle.
async fn watch_settled(fixture: &Fixture) {
    settled_passes(fixture)
        .wait_for(|passes| *passes > 0)
        .await
        .unwrap();
    while !fixture.manager.applications.lock().worker_idle() {
        tokio::task::yield_now().await;
    }
}

/// Text of the resident runtime's project instructions.
fn project_text(runtime: &crate::runtime::ConversationRuntime) -> String {
    runtime
        .runtime_resources()
        .project_instructions()
        .unwrap_or_default()
        .to_owned()
}

/// Select `file` (relative to the first Workspace) as a root instruction file.
fn select_instruction_file(fixture: &Fixture, file: &str) {
    let config = fixture.workspaces[0].join("rustx.toml");
    let mut document = std::fs::read_to_string(&config).unwrap();
    std::fmt::Write::write_fmt(
        &mut document,
        format_args!("\n[agent.agents_md]\nfiles = [{file:?}]\n"),
    )
    .unwrap();
    std::fs::write(&config, document).unwrap();
}

fn modified(path: &std::path::Path) -> notify::Event {
    notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
        .add_path(path.to_path_buf())
}

/// A newly selected instruction file whose first capture fails stays an
/// input: fixing only that file, through its own notification, recovers the
/// same resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_newly_selected_file_that_fails_its_first_capture_recovers_from_its_own_edit() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            let policy = fixture.workspaces[0].join("docs/new-policy.md");
            std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
            std::fs::write(&policy, b"\xff\xfe not UTF-8").unwrap();
            select_instruction_file(&fixture, "docs/new-policy.md");
            let config = modified(&fixture.workspaces[0].join("rustx.toml"));
            while !fixture.manager.notify_source_watch(config.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(!project_text(&runtime).contains("Policy fixed"));
            assert!(
                fixture
                    .manager
                    .applications
                    .lock()
                    .source_inputs()
                    .iter()
                    .any(|(_, file)| file == &policy),
                "the failed input is not observed"
            );
            // The pass after this capture watches the failed input and captures
            // it again, still invalid; only then is it fixed, so its own
            // notification is what recovers.
            let passes = *settled_passes(&fixture).borrow();
            captured.release();
            captured.parked(0).await;
            settled_passes(&fixture)
                .wait_for(|settled| *settled > passes)
                .await
                .unwrap();
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(!project_text(&runtime).contains("Policy fixed"));
            captured.arm();
            std::fs::write(&policy, "Policy fixed").unwrap();
            assert!(fixture.manager.notify_source_watch(modified(&policy)));
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(
                project_text(&runtime).contains("Policy fixed"),
                "not recovered: {}",
                project_text(&runtime)
            );
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// A selected instruction file that is an absolute link is observed through
/// its final target: editing only the target, through its own notification,
/// reaches the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edited_absolute_link_target_reaches_the_resident_runtime() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = std::fs::canonicalize(&fixture.workspaces[0]).unwrap();
        let target = workspace.join("policies/main.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        std::fs::write(&target, "Policy version A").unwrap();
        std::os::unix::fs::symlink(&target, workspace.join("docs/policy.md")).unwrap();
        select_instruction_file(&fixture, "docs/policy.md");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        assert!(project_text(&runtime).contains("Policy version A"));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            std::fs::write(&target, "Policy version B").unwrap();
            while !fixture.manager.notify_source_watch(modified(&target)) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(
                project_text(&runtime).contains("Policy version B"),
                "the target edit was not routed: {}",
                project_text(&runtime)
            );
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// Re-point `link` to `to`, notify only the link itself, and require the
/// resident runtime of the first Session, already loaded with "version 1",
/// to reach "version 2".
async fn a_repointed_link_reaches_the_resident_runtime(
    fixture: &Fixture,
    link: &std::path::Path,
    to: &std::path::Path,
) {
    let id = fixture.sessions[0].id.clone();
    let runtime = fixture.manager.configuration_runtime(&id).unwrap();
    assert!(project_text(&runtime).contains("version 1"));
    while !fixture.manager.applications.lock().worker_idle() {
        tokio::task::yield_now().await;
    }
    injected_only(fixture);
    let captured = fixture
        .manager
        .registry
        .0
        .lock()
        .unwrap()
        .source_watch_captured
        .clone();
    captured.arm();
    let stop = tokio_util::sync::CancellationToken::new();
    let observe = async {
        watch_settled(fixture).await;
        let staged = link.with_extension("next");
        std::os::unix::fs::symlink(to, &staged).unwrap();
        std::fs::rename(&staged, link).unwrap();
        while !fixture.manager.notify_source_watch(modified(link)) {
            tokio::task::yield_now().await;
        }
        captured.entered().await;
        applied(fixture, 0).await;
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        assert!(
            project_text(&runtime).contains("version 2"),
            "the re-pointed link was not routed: {}",
            project_text(&runtime)
        );
        captured.release();
        stop.cancel();
    };
    let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
    watched.unwrap();
}

/// Two versioned policy directories, each holding `policy.md`.
fn versioned_policies(directory: &std::path::Path) {
    for version in ["1", "2"] {
        let path = directory.join(format!("v{version}"));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("policy.md"), format!("Policy version {version}")).unwrap();
    }
}

/// An absolute directory link that passes through another link keeps that
/// link observed: re-pointing only it reaches the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repointed_intermediate_link_reaches_the_resident_runtime() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let workspace = std::fs::canonicalize(&fixture.workspaces[0]).unwrap();
        versioned_policies(&workspace);
        std::fs::create_dir_all(workspace.join("links")).unwrap();
        std::os::unix::fs::symlink("../v1", workspace.join("links/current")).unwrap();
        std::os::unix::fs::symlink(workspace.join("links/current"), workspace.join("docs"))
            .unwrap();
        select_instruction_file(&fixture, "docs/policy.md");
        fixture
            .manager
            .load(&fixture.sessions[0].id, None)
            .await
            .unwrap();
        a_repointed_link_reaches_the_resident_runtime(
            &fixture,
            &workspace.join("links/current"),
            std::path::Path::new("../v2"),
        )
        .await;
        fixture.close().await;
    })
    .await;
}

/// A User instruction file outside every Workspace keeps a link among its
/// parents observed: re-pointing only that link reaches the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repointed_parent_link_of_an_external_user_input_reaches_the_resident_runtime() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let shared = std::fs::canonicalize(fixture.workspaces[0].parent().unwrap())
            .unwrap()
            .join("shared");
        versioned_policies(&shared);
        std::os::unix::fs::symlink("v1", shared.join("current")).unwrap();
        let config = fixture.manager.configuration.sources.config_path.clone();
        let mut document = std::fs::read_to_string(&config).unwrap();
        std::fmt::Write::write_fmt(
            &mut document,
            format_args!(
                "\n[agent.agents_md]\nfiles = [{:?}]\n",
                shared.join("current/policy.md")
            ),
        )
        .unwrap();
        std::fs::write(&config, document).unwrap();
        fixture
            .manager
            .load(&fixture.sessions[0].id, None)
            .await
            .unwrap();
        a_repointed_link_reaches_the_resident_runtime(
            &fixture,
            &shared.join("current"),
            std::path::Path::new("v2"),
        )
        .await;
        fixture.close().await;
    })
    .await;
}

/// A newly selected instruction file whose authorization fails before it
/// is read (its directory is a regular file) stays an input: repairing only
/// the directory, through the directory's own notification, recovers the
/// same resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_selected_file_beneath_a_non_directory_recovers_when_the_directory_is_repaired() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            let docs = fixture.workspaces[0].join("docs");
            let policy = docs.join("policy.md");
            std::fs::write(&docs, "not a directory").unwrap();
            select_instruction_file(&fixture, "docs/policy.md");
            let config = modified(&fixture.workspaces[0].join("rustx.toml"));
            while !fixture.manager.notify_source_watch(config.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(context_failed(&fixture), "the capture did not fail");
            assert!(
                fixture
                    .manager
                    .applications
                    .lock()
                    .source_inputs()
                    .iter()
                    .any(|(_, file)| file == &policy),
                "the unauthorized input is not observed"
            );
            // Let the failed capture's handling finish entirely, including
            // the pass that watches its input, before repairing anything.
            let passes = *settled_passes(&fixture).borrow();
            captured.release();
            captured.parked(0).await;
            settled_passes(&fixture)
                .wait_for(|settled| *settled > passes)
                .await
                .unwrap();
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(!project_text(&runtime).contains("Policy repaired"));
            captured.arm();
            std::fs::remove_file(&docs).unwrap();
            std::fs::create_dir(&docs).unwrap();
            std::fs::write(&policy, "Policy repaired").unwrap();
            assert!(fixture.manager.notify_source_watch(modified(&docs)));
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(
                project_text(&runtime).contains("Policy repaired"),
                "not recovered: {}",
                project_text(&runtime)
            );
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// An input the watcher learns of only from a capture is captured again
/// once it is watched: an edit after that capture read it, before its watch
/// existed, still reaches the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_before_a_new_input_is_watched_is_captured_once_it_is() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            let policy = fixture.workspaces[0].join("docs/policy.md");
            std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
            std::fs::write(&policy, "Policy version A").unwrap();
            select_instruction_file(&fixture, "docs/policy.md");
            let config = modified(&fixture.workspaces[0].join("rustx.toml"));
            while !fixture.manager.notify_source_watch(config.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(project_text(&runtime).contains("Policy version A"));
            // Edited while the watcher is held after the capture that found
            // the file and before it watches the file's directory.
            std::fs::write(&policy, "Policy version B").unwrap();
            captured.release();
            while !project_text(&runtime).contains("Policy version B") {
                tokio::task::yield_now().await;
            }
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// Replacing the directory that holds a captured input with a pre-filled
/// one notifies only the directory; that notification alone captures the
/// new content into the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_input_directory_recaptures_its_readers() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let docs = fixture.workspaces[0].join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("policy.md"), "Policy version A").unwrap();
        select_instruction_file(&fixture, "docs/policy.md");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        assert!(project_text(&runtime).contains("Policy version A"));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            let staged = fixture.workspaces[0].join("staged");
            std::fs::create_dir_all(&staged).unwrap();
            std::fs::write(staged.join("policy.md"), "Policy version B").unwrap();
            std::fs::rename(&docs, fixture.workspaces[0].join("old")).unwrap();
            std::fs::rename(&staged, &docs).unwrap();
            let moved = notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Name(notify::event::RenameMode::To),
            ))
            .add_path(docs.clone());
            while !fixture.manager.notify_source_watch(moved.clone()) {
                tokio::task::yield_now().await;
            }
            captured.entered().await;
            applied(&fixture, 0).await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(
                project_text(&runtime).contains("Policy version B"),
                "the replacement was not captured: {}",
                project_text(&runtime)
            );
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

fn context_failed(fixture: &Fixture) -> bool {
    fixture
        .manager
        .applications
        .lock()
        .test_desired_source(&fixture.workspaces[0])
        .is_some_and(|capture| {
            capture
                .as_ref()
                .is_ok_and(|capture| capture.context.is_err())
        })
}

/// A Named Agent's newly selected instruction file that fails its first
/// capture is an input although the diagnostic names the Agent's TOML: fixing
/// only that file, through its own notification, recovers the capture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_agent_file_that_fails_its_first_capture_recovers_from_its_own_edit() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let helper = fixture.workspaces[0].join(".agents/agents/helper.toml");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, "description = 'Help'\ninstructions = 'Assist'\n").unwrap();
        let config = fixture.workspaces[0].join("rustx.toml");
        let mut document = std::fs::read_to_string(&config).unwrap();
        document.push_str("\n[agent]\nagents = [\"helper\"]\n");
        std::fs::write(&config, document).unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        assert!(!context_failed(&fixture));
        let file = fixture.workspaces[0].join("docs/helper.md");
        injected_only(&fixture);
        let captured = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .source_watch_captured
            .clone();
        captured.arm();
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, b"\xff\xfe not UTF-8").unwrap();
            std::fs::write(
                &helper,
                "description = 'Help'\ninstructions = 'Assist'\n[agents_md]\nfiles = ['docs/helper.md']\n",
            )
            .unwrap();
            assert!(fixture.manager.notify_source_watch(modified(&helper)));
            captured.entered().await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(context_failed(&fixture), "the invalid file did not fail the capture");
            assert!(
                fixture
                    .manager
                    .applications
                    .lock()
                    .source_inputs()
                    .iter()
                    .any(|(_, input)| input == &file),
                "the file the capture failed at is not an input"
            );
            let passes = *settled_passes(&fixture).borrow();
            captured.release();
            captured.parked(0).await;
            settled_passes(&fixture)
                .wait_for(|settled| *settled > passes)
                .await
                .unwrap();
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(context_failed(&fixture));
            captured.arm();
            std::fs::write(&file, "Helper policy").unwrap();
            assert!(fixture.manager.notify_source_watch(modified(&file)));
            captured.entered().await;
            while !fixture.manager.applications.lock().worker_idle() {
                tokio::task::yield_now().await;
            }
            assert!(!context_failed(&fixture), "the fixed file did not recover");
            captured.release();
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// The first pass treats every input it finds as new: an edit after the
/// capture that read it and before the watch started, with no notification
/// at all, reaches the resident runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_before_the_watch_starts_is_captured_by_its_first_pass() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let policy = fixture.workspaces[0].join("docs/policy.md");
        std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
        std::fs::write(&policy, "Policy version A").unwrap();
        select_instruction_file(&fixture, "docs/policy.md");
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        assert!(project_text(&runtime).contains("Policy version A"));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        std::fs::write(&policy, "Policy version B").unwrap();
        injected_only(&fixture);
        let stop = tokio_util::sync::CancellationToken::new();
        let observe = async {
            watch_settled(&fixture).await;
            while !project_text(&runtime).contains("Policy version B") {
                tokio::task::yield_now().await;
            }
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), observe);
        watched.unwrap();
        fixture.close().await;
    })
    .await;
}

/// An execution-policy update that really published after an earlier
/// unrecorded commit is retained: it survives unload/load, and the healthy
/// replacement allocation recovers the Session's application view.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_published_policy_after_an_unrecorded_commit_survives_reload() {
    bounded(async {
        use crate::local_runtime::configuration::settings::EffectiveConfiguration;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let loaded = fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        runtime.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("published, not recorded".into()),
            },
        )
        .await;
        let view = applied(&fixture, 0).await;
        assert!(matches!(
            view.units[&ApplyUnit::Instructions],
            UnitApplication::Uncertain { .. }
        ));
        write(
            &fixture,
            0,
            ConfigMutation::ModelTimeout {
                authored: Some(crate::local_runtime::authoring::TimeoutLayer {
                    response_start_timeout_ms: Some(41_000),
                    stream_idle_timeout_ms: Some(42_000),
                }),
            },
        )
        .await;
        applied(&fixture, 0).await;
        let timeout = EffectiveConfiguration::adopted(&runtime)
            .unwrap()
            .model_timeout;
        assert_eq!(timeout.response_start_timeout_ms, 41_000);
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        drop(runtime);
        let _ = fixture.manager.unload(&loaded.conversation).await;
        let mut changed = fixture.manager.configuration_changes();
        changed.mark_unchanged();
        fixture.manager.load(&id, None).await.unwrap();
        let reloaded = fixture.manager.configuration_runtime(&id).unwrap();
        let effective = EffectiveConfiguration::adopted(&reloaded).unwrap();
        assert_eq!(effective.model_timeout, timeout);
        assert_eq!(instructions(&reloaded), "published, not recorded");
        let view = applied(&fixture, 0).await;
        assert!(
            view.units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Uncertain { .. })),
            "a healthy replacement recovers the view: {view:?}"
        );
        // Observers waiting on the old view learn of the recovery.
        assert!(changed.has_changed().unwrap());
        assert!(*changed.borrow_and_update() >= view.version, "{view:?}");
        fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// Loading is one state switch: while the publication of a healthy
/// replacement is parked before it attaches to the Session's application,
/// `load()` has not returned; once it returns, the application already
/// belongs to the replacement and observers were notified.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_load_returns_only_after_its_runtime_owns_the_application() {
    // A parked publication blocks a worker thread: release it however this
    // test ends.
    struct Release(std::sync::Arc<AsyncGate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let loaded = fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        runtime.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("published, not recorded".into()),
            },
        )
        .await;
        let uncertain = |fixture: &Fixture| {
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Uncertain { .. }))
        };
        applied(&fixture, 0).await;
        assert!(uncertain(&fixture));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        drop(runtime);
        let _ = fixture.manager.unload(&loaded.conversation).await;
        let gate = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .resident_attach
            .clone();
        gate.arm();
        let _release = Release(gate.clone());
        let mut changed = fixture.manager.configuration_changes();
        changed.mark_unchanged();
        let manager = fixture.manager.clone();
        let reload = {
            let id = id.clone();
            tokio::spawn(async move { manager.load(&id, None).await.map(|_| ()) })
        };
        gate.entered().await;
        // Parked before the attach: the resident is not reported loaded.
        tokio::task::yield_now().await;
        assert!(
            !reload.is_finished(),
            "load() reported a resident it had not attached"
        );
        assert!(uncertain(&fixture));
        gate.release();
        reload.await.unwrap().unwrap();
        // No wait: the returned load already owns the application.
        assert!(
            !uncertain(&fixture),
            "{:?}",
            fixture.manager.configuration_application(&id)
        );
        assert!(changed.has_changed().unwrap());
        let view = fixture.manager.configuration_application(&id).unwrap();
        assert!(*changed.borrow_and_update() >= view.version, "{view:?}");
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// A replaced allocation's late attach never takes the application back:
/// A's publication parks after taking its runtime handle and before the
/// application lock; meanwhile A is unloaded, B loads, and B publishes a
/// commit it cannot record. When A resumes, the application stays B's and
/// stays uncertain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_allocation_never_reattaches_late() {
    // A parked publication blocks a worker thread: release it however this
    // test ends.
    struct Release(std::sync::Arc<AsyncGate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let gate = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .resident_attach
            .clone();
        gate.arm();
        let _release = Release(gate.clone());
        let first = {
            let (manager, id) = (fixture.manager.clone(), id.clone());
            tokio::spawn(async move { manager.load(&id, None).await.map(|_| ()) })
        };
        gate.entered().await;
        // A is resident but parked before it attaches.
        let a = fixture.manager.configuration_runtime(&id).unwrap();
        let conversation = a.conversation_id().clone();
        drop(a);
        let _ = fixture.manager.unload(&conversation).await;
        // B passes the gate, which parks only the first publication.
        fixture.manager.load(&id, None).await.unwrap();
        let b = fixture.manager.configuration_runtime(&id).unwrap();
        b.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("published by B, not recorded".into()),
            },
        )
        .await;
        let uncertain = |fixture: &Fixture| {
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Uncertain { .. }))
        };
        applied(&fixture, 0).await;
        assert!(uncertain(&fixture));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        gate.release();
        let _ = first.await.unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        assert!(
            uncertain(&fixture),
            "the replaced A took the application back: {:?}",
            fixture.manager.configuration_application(&id)
        );
        assert_eq!(instructions(&b), "published by B, not recorded");
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// The configuration worker never hands an application to the allocation it
/// looked up once that allocation is gone: A could not record a commit; a
/// comment-only edit's preparation looks A up and parks before the
/// application lock; meanwhile A is unloaded and a healthy B loads, with
/// nothing staged or deferred for its rebind to prepare again. When the
/// worker resumes, the edit is prepared for B: no unit reports A's
/// uncertainty or a policy failure from the stopped A.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_preparation_never_attaches_the_allocation_it_outlived() {
    // A parked worker never finishes the edit: release it however this
    // test ends.
    struct Release(std::sync::Arc<AsyncGate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let loaded = fixture.manager.load(&id, None).await.unwrap();
        let a = fixture.manager.configuration_runtime(&id).unwrap();
        a.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("published by A, not recorded".into()),
            },
        )
        .await;
        let uncertain = |fixture: &Fixture| {
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Uncertain { .. }))
        };
        applied(&fixture, 0).await;
        assert!(uncertain(&fixture));
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        drop(a);
        let gate = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .configuration_resident
            .clone();
        gate.arm();
        let _release = Release(gate.clone());
        let first = fixture.manager.configuration_application(&id).unwrap();
        let document = fixture.workspaces[0].join("rustx.toml");
        let mut text = std::fs::read_to_string(&document).unwrap_or_default();
        text.push_str("\n# comment only\n");
        std::fs::write(&document, text).unwrap();
        let reconcile = {
            let manager = fixture.manager.clone();
            let directory = fixture.workspaces[0].clone();
            tokio::spawn(async move {
                manager
                    .reconcile_configuration(
                        &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                            directory,
                        },
                    )
                    .await
            })
        };
        // The worker holds A's handle, before the application lock.
        gate.entered().await;
        let _ = fixture.manager.unload(&loaded.conversation).await;
        fixture.manager.load(&id, None).await.unwrap();
        let b = fixture.manager.configuration_runtime(&id).unwrap();
        gate.release();
        reconcile.await.unwrap().unwrap();
        while !fixture.manager.applications.lock().worker_idle() {
            tokio::task::yield_now().await;
        }
        let view = applied(&fixture, 0).await;
        assert_ne!(view.desired, first.desired, "the edit is a new application");
        assert!(
            view.units.values().all(|unit| !matches!(
                unit,
                UnitApplication::Uncertain { .. } | UnitApplication::Failed { .. }
            )),
            "the stopped A owns the application: {view:?}"
        );
        assert_eq!(instructions(&b), "published by A, not recorded");
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// The allocation's own failure decides uncertainty, not the order its
/// settlement arrives in: a comment-only edit captured after the runtime
/// failed to record a commit, but before that settlement reached the
/// application owner, still reports the commit uncertain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_unrecorded_settlement_is_never_reported_applied() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let settlement = fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .configuration_settlement
            .clone();
        settlement.arm();
        runtime.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("published, not recorded".into()),
            },
        )
        .await;
        // Native published B and failed to record it; its report is parked.
        settlement.entered().await;
        assert_eq!(instructions(&runtime), "published, not recorded");
        assert!(runtime.durability_failure().is_some());
        let first = fixture.manager.configuration_application(&id).unwrap();
        let document = fixture.workspaces[0].join("rustx.toml");
        let mut text = std::fs::read_to_string(&document).unwrap_or_default();
        text.push_str("\n# comment only\n");
        std::fs::write(&document, text).unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let uncertain = |view: &crate::local_runtime::configuration::application::ConfigurationApplication| {
            matches!(
                &view.units[&ApplyUnit::Instructions],
                UnitApplication::Uncertain { diagnostic } if diagnostic.contains("fault injected")
            )
        };
        let view = applied(&fixture, 0).await;
        assert_ne!(view.desired, first.desired, "the edit is a new application");
        assert!(uncertain(&view), "{view:?}");
        assert!(
            view.units.values().all(|unit| *unit != UnitApplication::Applied),
            "{view:?}"
        );
        // The superseded report arrives late and changes nothing reported.
        let mut changed = fixture.manager.configuration_changes();
        changed.mark_unchanged();
        settlement.release();
        changed.changed().await.unwrap();
        assert!(uncertain(&fixture.manager.configuration_application(&id).unwrap()));
        assert!(configuration_changes(&runtime).is_empty());
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// The allocation's configuration health is its own fact, not the first
/// failure its durability gate kept: a commit the runtime could not record
/// after an earlier, unrelated durable failure stays uncertain through a
/// later comment-only edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unrecorded_commit_behind_an_earlier_durability_failure_stays_uncertain() {
    bounded(async {
        use crate::local_runtime::configuration::settings::EffectiveConfiguration;
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        runtime.force_durability_failure_for_test(
            crate::runtime::types::DurableOperation::EventJournal,
            "journal unavailable",
        );
        runtime.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::ModelTimeout {
                authored: Some(crate::local_runtime::authoring::TimeoutLayer {
                    response_start_timeout_ms: Some(41_000),
                    stream_idle_timeout_ms: Some(42_000),
                }),
            },
        )
        .await;
        let unrecorded = |view: &crate::local_runtime::configuration::application::ConfigurationApplication| {
            matches!(
                &view.units[&ApplyUnit::ExecutionPolicy],
                UnitApplication::Uncertain { diagnostic } if diagnostic.contains("fault injected")
            )
        };
        let view = applied(&fixture, 0).await;
        assert!(unrecorded(&view), "{view:?}");
        assert_eq!(
            EffectiveConfiguration::adopted(&runtime)
                .unwrap()
                .model_timeout
                .response_start_timeout_ms,
            41_000,
            "the timeout is published"
        );
        let document = fixture.workspaces[0].join("rustx.toml");
        let mut text = std::fs::read_to_string(&document).unwrap();
        text.push_str("\n# comment only\n");
        std::fs::write(&document, text).unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let after = applied(&fixture, 0).await;
        assert_ne!(after.desired, view.desired, "the edit is a new application");
        assert!(unrecorded(&after), "{after:?}");
        assert!(
            after.units.values().all(|unit| *unit != UnitApplication::Applied),
            "{after:?}"
        );
        assert!(configuration_changes(&runtime).is_empty(), "nothing recorded");
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}

/// A commit whose change fact cannot be recorded is uncertain: the generation
/// is already published, nothing is recorded, admission is fenced by the
/// absorbing durability failure, and nothing is applied again or reported as
/// the previous configuration kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unrecorded_live_commit_is_uncertain_and_fences_admission() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        runtime.fail_next_configuration_record();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("published, not recorded".into()),
            },
        )
        .await;
        let view = applied(&fixture, 0).await;
        assert!(
            matches!(
                &view.units[&ApplyUnit::Instructions],
                UnitApplication::Uncertain { diagnostic } if diagnostic.contains("fault injected")
            ),
            "{view:?}"
        );
        assert_eq!(instructions(&runtime), "published, not recorded");
        assert!(configuration_changes(&runtime).is_empty());
        assert!(runtime.durability_failure().is_some());
        let desired = view.desired.clone();
        assert!(
            runtime.submit_inbound(input("request-A")).is_err(),
            "admission is fenced"
        );
        runtime.admit_now_for_test();
        assert!(!runtime.has_current_attempt());
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .desired,
            desired,
            "the uncertain commit is never applied again"
        );
        // A comment-only edit is a new source revision whose capture equals
        // the published generation; it settles without clearing uncertainty.
        let document = fixture.workspaces[0].join("rustx.toml");
        let mut text = std::fs::read_to_string(&document).unwrap_or_default();
        text.push_str("\n# comment only\n");
        std::fs::write(&document, text).unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let view = applied(&fixture, 0).await;
        assert_ne!(view.desired, desired, "the edit is a new application");
        assert!(
            view.units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Applied)),
            "{view:?}"
        );
        assert!(
            matches!(
                &view.units[&ApplyUnit::Instructions],
                UnitApplication::Uncertain { diagnostic } if diagnostic.contains("fault injected")
            ),
            "{view:?}"
        );
        assert!(configuration_changes(&runtime).is_empty());
        let _ = fixture.manager.drain_all_runtimes().await;
    })
    .await;
}
