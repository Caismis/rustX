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

/// Editing only a selected Skill's body keeps its catalog entry byte-identical
/// but replaces its content version: the commit records it as `changed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn body_only_skill_edit_records_one_changed_fact() {
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
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), before + 1, "{changes:?}");
        assert_eq!(changes[before].skills.changed.names, ["guide"]);
        assert!(
            changes[before].skills.added.is_empty() && changes[before].skills.removed.is_empty()
        );
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
                    panic!("one installed Skill: {catalog}");
                };
                assert_eq!(skill["entry"]["name"], "guide");
                skill["version_id"].as_str().unwrap().to_owned()
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
            catalog["tools"][0]["workflow"]["program_digest"],
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
