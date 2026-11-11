//! The available binding new Sessions use: preparation failures and retries,
//! cold Sessions outside the residency budget, workflow-only agent rebuilds,
//! healthy registration and process restarts.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t09_t13_t15_new_sessions_use_available_during_preparation_failure_and_retry() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let before = fixture
            .manager
            .configuration_runtime(id)
            .unwrap()
            .runtime_resources();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_prepare.arm();
        let source = fixture
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
        document["agent"]["model"]["model"] = "local/b".into();
        std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
        write_guidance(&fixture, "available after success".into()).await;
        probe.before_configuration_prepare.entered().await;
        let create = || {
            fixture.manager.create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
        };
        let during = create().await.unwrap().session.id;
        let retained = |id: &SessionId| {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[id]
                .clone()
        };
        assert_eq!(
            guidance(&retained(&during).project_context_files),
            guidance(before.project_context_files())
        );
        assert!(fixture.manager.configuration_runtime(&during).is_none());
        probe.fail_configuration_once.store(true, Ordering::SeqCst);
        probe.before_configuration_prepare.release();
        let failed = settled(&fixture, 0).await;
        assert!(matches!(
            failed.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { .. }
        ));
        let after_failure = create().await.unwrap().session.id;
        assert_eq!(
            guidance(&retained(&after_failure).project_context_files),
            guidance(&retained(&during).project_context_files)
        );
        assert_eq!(
            retained(&after_failure).session_model(),
            retained(&during).session_model()
        );
        assert!(retained(&after_failure).same_capabilities(&retained(&during)));
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let success = applied(&fixture, 0).await;
        assert_eq!(
            success.desired.input_revision,
            failed.desired.input_revision
        );
        let after_success = create().await.unwrap().session.id;
        assert_eq!(
            guidance(&retained(&after_success).project_context_files),
            "available after success"
        );
        assert_eq!(
            retained(&after_success).session_model().model.to_string(),
            "local/b"
        );
        assert_eq!(
            retained(&during).session_model().model.to_string(),
            "local/a"
        );
        assert_eq!(
            retained(&after_failure).session_model().model.to_string(),
            "local/a"
        );
        assert_ne!(
            guidance(&retained(&after_failure).project_context_files),
            "available after success"
        );
        assert_ne!(
            guidance(&retained(&during).project_context_files),
            "available after success"
        );
        fixture.manager.load(&after_failure, None).await.unwrap();
        assert_eq!(
            guidance(
                fixture
                    .manager
                    .configuration_runtime(&after_failure)
                    .unwrap()
                    .runtime_resources()
                    .project_context_files()
            ),
            guidance(before.project_context_files())
        );
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t06_t15_configuration_save_keeps_cold_sessions_outside_residency_budget() {
    let fixture = Fixture::with_session_count(None, 0).await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(
            fixture
                .manager
                .create_session(SessionPersistentState {
                    cwd: fixture.workspaces[0].clone(),
                    model: None,
                })
                .await
                .unwrap()
                .session
                .id,
        );
    }
    // Persist the small process limit so source reconciliation keeps it.
    let source = fixture
        .manager
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
        .await
        .unwrap();
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
    document.as_table_mut().unwrap().insert(
        "app_server".into(),
        toml::toml! { max_resident_runtimes = 1 }.into(),
    );
    std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
    let before = fixture.manager.registry.0.lock().unwrap().entries.len();
    write_guidance(&fixture, "cold desired context".into()).await;
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if ids
            .iter()
            .all(|id| fixture.manager.applications.is_deferred(id.as_ref()))
        {
            break;
        }
        changed.changed().await.unwrap();
    }
    assert_eq!(fixture.manager.process_policy().max_resident_runtimes, 1);
    assert_eq!(
        fixture.manager.registry.0.lock().unwrap().entries.len(),
        before
    );
    for id in &ids {
        assert!(fixture.manager.configuration_runtime(id).is_none());
        let view = fixture.manager.configuration_application(id).unwrap();
        assert_eq!(
            view.units[&ApplyUnit::ExecutionPolicy],
            UnitApplication::Applied
        );
        assert_eq!(
            view.units[&ApplyUnit::Instructions],
            UnitApplication::Preparing
        );
        assert!(
            !view
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Failed { .. }))
        );
    }
    let subsequent = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap();
    assert_eq!(
        guidance(
            &fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&subsequent.session.id]
                .project_context_files
        ),
        "cold desired context"
    );
    assert!(
        fixture
            .manager
            .configuration_runtime(&subsequent.session.id)
            .is_none()
    );
    fixture.manager.load(&ids[0], None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(&ids[0]).unwrap();
    // The resident Session commits; the cold ones stay unloaded.
    loop {
        if fixture
            .manager
            .configuration_application(&ids[0])
            .unwrap()
            .units
            .values()
            .all(|unit| !matches!(unit, UnitApplication::Preparing | UnitApplication::Ready))
        {
            break;
        }
        changed.changed().await.unwrap();
    }
    assert_eq!(
        fixture.manager.registry.0.lock().unwrap().entries.len(),
        before + 1
    );
    assert_eq!(
        guidance(runtime.runtime_resources().project_context_files()),
        "cold desired context"
    );
    // It composed from the retained binding and committed the cold desire on
    // top of it, as one recorded reallocation.
    let facts = configuration_changes(&runtime);
    let [change] = facts.as_slice() else {
        panic!("one reallocation fact: {facts:?}");
    };
    assert_eq!(
        change.reason,
        crate::runtime::configuration_commit::ConfigurationChangeReason::Reallocation
    );
    assert!(
        change
            .domains
            .contains(&crate::runtime::configuration_commit::ConfigurationDomain::Instructions),
        "{change:?}"
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t09_t15_new_session_during_preparation_keeps_available_binding_after_success() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let before = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[id]
            .clone();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_prepare.arm();
        write_guidance(&fixture, "P2 available".into()).await;
        probe.before_configuration_prepare.entered().await;
        let create = || {
            fixture.manager.create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
        };
        let during = create().await.unwrap().session.id;
        let retained = |id: &SessionId| {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[id]
                .clone()
        };
        assert_eq!(
            retained(&during).component_revisions,
            before.component_revisions
        );
        assert_eq!(retained(&during).session_model(), before.session_model());
        assert_eq!(
            guidance(&retained(&during).project_context_files),
            guidance(&before.project_context_files)
        );
        probe.before_configuration_prepare.release();
        applied(&fixture, 0).await;
        let after = create().await.unwrap().session.id;
        assert_eq!(
            guidance(&retained(&after).project_context_files),
            "P2 available"
        );
        assert_eq!(
            retained(&during).component_revisions,
            before.component_revisions
        );
        let loaded = fixture.manager.load(&during, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&during).unwrap();
        let history = loaded
            .inspect_runtime()
            .unwrap()
            .historical_canonical_history()
            .unwrap();
        // Residency prepares the desired source again and commits it without
        // a client; canonical history is untouched.
        session_applied(&fixture, &during).await;
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()),
            "P2 available"
        );
        assert_eq!(
            loaded
                .inspect_runtime()
                .unwrap()
                .historical_canonical_history()
                .unwrap(),
            history
        );
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t09_available_default_preparation_is_independent_of_retained_session_selection() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let old = runtime.runtime_resources();
    let source = fixture
        .manager
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
        .await
        .unwrap();
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
    document["agent"]["model"]["model"] = "local/b".into();
    document["models"].as_table_mut().unwrap().remove("local/a");
    // Require real capability construction as well as new provider/default
    // preparation; the old Session cannot bind this catalog to its selection.
    document["agent"]["tools"]["builtin"] =
        toml::Value::try_from(vec!["read", "grep"]).unwrap();
    std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
    // The removed selection blocks neither the source generation nor the
    // catalog: the Provider unit publishes at once, reporting the Session's
    // selection unavailable, and the real capability change prepared against
    // that published binding commits automatically.
    let committed = applied(&fixture, 0).await;
    assert!(
        committed
            .units
            .values()
            .all(|unit| *unit == UnitApplication::Applied),
        "{committed:?}"
    );
    let view = runtime.model_view();
    assert_eq!(view.configured.model.to_string(), "local/a");
    assert!(view.unavailable.unwrap().contains("local/a"));
    let references = |runtime: &crate::runtime::conversation_runtime::ConversationRuntime| {
        runtime
            .model_catalog()
            .models
            .iter()
            .map(|model| model.model.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(references(&runtime), ["local/b"]);
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap();
    fixture
        .manager
        .load(&created.session.id, None)
        .await
        .unwrap();
    let new = fixture
        .manager
        .configuration_runtime(&created.session.id)
        .unwrap();
    assert_eq!(new.model_view().configured.model.to_string(), "local/b");
    let resources = new.runtime_resources();
    assert_eq!(
        resources
            .capability()
            .tool_registry()
            .definitions()
            .into_iter()
            .find(|tool| tool.name == "read")
            .unwrap()
            .execution_policy,
        crate::tools::types::ToolExecutionPolicy::BackgroundOnly
    );
    // The commit published the capabilities; the old Session keeps its
    // configured selection, still unavailable, and the published catalog.
    let view = runtime.model_view();
    assert_eq!(view.configured.model.to_string(), "local/a");
    assert!(view.unavailable.unwrap().contains("local/a"));
    assert!(view.effective.is_some());
    assert_eq!(references(&runtime), ["local/b"]);
    assert!(!Arc::ptr_eq(
        old.capability().tool_registry(),
        runtime.runtime_resources().capability().tool_registry()
    ));
    fixture.close().await;
}

async fn workflow_only_agent_change(model_change: bool) {
    Box::pin(bounded(async {
        let mut phases = FixturePhase::new("workflow-only Agent configuration");
        let fixture = Fixture::with_tool(Some("review")).await;
        phases.enter("configure workflow");
        let id = &fixture.sessions[0].id;
        let workspace = &fixture.workspaces[0];
        let agent = workspace.join(".agents/agents/reviewer.toml");
        let workflow = workspace.join(".agents/workflows/review.yaml");
        std::fs::create_dir_all(agent.parent().unwrap()).unwrap();
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        let profile = |instructions: &str, model: &str| format!("description='Review'\ninstructions='{instructions}'\n[model]\nmodel='local/{model}'\n");
        std::fs::write(&agent, profile("reviewer A", "a")).unwrap();
        let schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        std::fs::write(&workflow, serde_json::to_string(&serde_json::json!({
            "description":"Review", "block": {"input":schema,"nodes": {"workflow_start":{"type":"start"},"agent":{"type":"agent","profile":"reviewer","task":[{"type":"literal","value":"Review"}],"output":{"type":"json_schema","schema":schema}},
                     "done":{"type":"return","outputs":[]}},"edges": [{"from":"workflow_start","to":"agent"},{"from":"agent","to":"done"}],}
        })).unwrap()).unwrap();
        std::fs::write(workspace.join("rustx.toml"), "[agent]\nworkflows=['review']\n").unwrap();
        let loaded = fixture.manager.load(id, None).await.unwrap();
        let live = loaded.inspect_runtime().unwrap();
        let before = live.runtime_resources();
        let retained_before = fixture.manager.sessions.configuration_bindings.lock().unwrap()[id].clone();
        assert!(before.configuration().unwrap().config.agent.agents.is_empty());
        let probe = fixture.manager.probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        std::fs::write(agent.parent().unwrap().join("unused.toml"), profile("unrelated", "b")).unwrap();
        fixture.manager.reconcile_configuration(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: fixture.workspaces[0].clone() }).await.unwrap();
        settled(&fixture, 0).await;
        assert_eq!(probe.configuration_preparations.load(Ordering::SeqCst), preparations);
        assert!(Arc::ptr_eq(&before, &live.runtime_resources()));
        phases.enter("old Workflow admission");
        live.submit_inbound(input("request-A old workflow")).unwrap();
        fixture.gates[0].wait_entered().await;
        let probe = fixture.manager.probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        phases.enter("publish Agent edit");
        let source = fixture.manager.source_settings(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: workspace.clone() }).await.unwrap();
        let authored = if model_change { profile("reviewer A", "b") } else { profile("reviewer B", "a") };
        fixture.manager.write_source_settings(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: workspace.clone() }, source.agents.iter().find(|entry| entry.source.path == agent).unwrap().source.revision.clone(), SourceMutation::Agent {
                name: crate::runtime::subagent::SubagentName::parse("reviewer").unwrap(),
                authored: Some(crate::local_runtime::agent_resources::parse(&authored).unwrap()),
            }).await.unwrap();
        assert_eq!(settled(&fixture, 0).await.units[&ApplyUnit::Capabilities], UnitApplication::Ready);
        assert!(probe.configuration_preparations.load(Ordering::SeqCst) > preparations);
        assert_eq!(live.runtime_resources().capability().revision(), before.capability().revision());
        let mut done = SettlementReceiptWait::capture(&live);
        phases.enter("old Workflow settlement");
        fixture.gates[0].release();
        done.wait().await;
        applied(&fixture, 0).await;
        assert_ne!(live.runtime_resources().capability().revision(), before.capability().revision());
        let requests = fixture.provider.request_bodies();
        assert!(requests.iter().any(|body| body.contains("reviewer A")), "{requests:?}");
        assert!(!requests.iter().any(|body| body.contains("reviewer B")));
        let old_child = requests.iter().map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
            .find(|body| body["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "workflow_output")).unwrap();
        assert_eq!(old_child["model"], "a");
        let old_count = requests.len();
        let mut done = SettlementReceiptWait::capture(&live);
        phases.enter("new Workflow settlement");
        live.submit_inbound(input("request-A new workflow")).unwrap();
        done.wait().await;
        let requests = fixture.provider.request_bodies();
        let child = requests[old_count..].iter().map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
            .find(|body| body["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "workflow_output")).expect("real Workflow Agent request");
        assert_eq!(child["model"], if model_change { "b" } else { "a" });
        assert!(child.to_string().contains(if model_change { "reviewer A" } else { "reviewer B" }));
        let retained = fixture.manager.sessions.configuration_bindings.lock().unwrap()[id].clone();
        assert_eq!(retained.same_provider(&retained_before), !model_change);
        // The same dependency closure protects the physical profile selected
        // during immutable capture, even though Root does not expose it.
        let moved = workspace.parent().unwrap().join("outside-agents");
        std::fs::rename(agent.parent().unwrap(), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, agent.parent().unwrap()).unwrap();
        assert!(retained.validate_resource_authority().is_err());
        phases.enter("fixture shutdown");
        fixture.close().await;
        phases.finish();
    })).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t05_t09_workflow_only_agent_content_rebuilds_frozen_execution() {
    workflow_only_agent_change(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t05_t09_workflow_only_agent_model_rebuilds_frozen_execution() {
    workflow_only_agent_change(true).await;
}

#[tokio::test]
async fn t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let probe = fixture
        .manager
        .probe(&fixture.sessions[0].active_conversation_id);
    std::fs::write(fixture.workspaces[0].join("rustx.toml"),
        "approval_mode='full_access'\n[model_timeout_policy]\nresponse_start_timeout_ms=12345\nstream_idle_timeout_ms=23456\n[tool_deadline_policy]\nhard_deadline_ms=34567\n[subagents]\nmax_concurrent=3\n").unwrap();
    // Save publishes policy independently; Instructions then explicitly adopt.
    write_guidance(&fixture, "healthy N+1".into()).await;
    settled(&fixture, 0).await;
    let independent =
        crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime)
            .unwrap();
    assert_eq!(
        independent.approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    assert_eq!(
        independent.document.approval_mode,
        Some(independent.approval_mode)
    );
    assert!(matches!(
        independent.provenance["approval_mode"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert_eq!(
        independent
            .document
            .model_timeout_policy
            .as_ref()
            .unwrap()
            .response_start_timeout_ms,
        Some(12345)
    );
    assert_eq!(
        independent
            .document
            .tool_deadline_policy
            .as_ref()
            .unwrap()
            .hard_deadline_ms,
        Some(34567)
    );
    assert_eq!(
        independent
            .document
            .subagents
            .as_ref()
            .unwrap()
            .max_concurrent,
        Some(3)
    );
    let healthy = applied(&fixture, 0).await;
    assert!(
        healthy
            .units
            .values()
            .all(|unit| *unit == UnitApplication::Applied)
    );
    let resources = runtime.runtime_resources();
    let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap()
        .session;
    let second = &created.id;
    let adopted = fixture
        .manager
        .sessions
        .configuration_bindings
        .lock()
        .unwrap()[second]
        .clone();
    assert_eq!(guidance(&adopted.project_context_files), "healthy N+1");
    assert!(!fixture.manager.applications.is_deferred(second.as_str()));
    assert!(fixture.manager.configuration_application(second).is_none());
    let second_probe = fixture.manager.probe(&created.active_conversation_id);
    assert_eq!(
        second_probe
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    fixture.manager.load(second, None).await.unwrap();
    let second_runtime = fixture.manager.configuration_runtime(second).unwrap();
    let second_resources = second_runtime.runtime_resources();
    let view = crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
        &second_runtime,
    )
    .unwrap();
    assert_eq!(view.approval_mode, crate::runtime::ApprovalMode::FullAccess);
    assert_eq!(adopted.config.approval_mode, view.approval_mode);
    assert_eq!(view.document.approval_mode, Some(view.approval_mode));
    assert_eq!(view.model_timeout.response_start_timeout_ms, 12345);
    assert_eq!(
        view.document
            .model_timeout_policy
            .as_ref()
            .unwrap()
            .response_start_timeout_ms,
        Some(12345)
    );
    assert_eq!(view.model_timeout.stream_idle_timeout_ms, 23456);
    assert_eq!(
        view.document
            .model_timeout_policy
            .as_ref()
            .unwrap()
            .stream_idle_timeout_ms,
        Some(23456)
    );
    assert_eq!(view.tool_deadline.hard_deadline_ms, 34567);
    assert_eq!(
        view.document
            .tool_deadline_policy
            .as_ref()
            .unwrap()
            .hard_deadline_ms,
        Some(34567)
    );
    assert_eq!(view.child_capacity.max_concurrent, 3);
    assert_eq!(
        view.document.subagents.as_ref().unwrap().max_concurrent,
        Some(3)
    );
    for field in [
        "approval_mode",
        "model_timeout_policy.response_start_timeout_ms",
        "model_timeout_policy.stream_idle_timeout_ms",
        "tool_deadline_policy.hard_deadline_ms",
        "subagents.max_concurrent",
    ] {
        assert!(
            matches!(
                view.provenance[field],
                crate::local_runtime::configuration::Origin::Workspace { .. }
            ),
            "{field}"
        );
        assert_eq!(view.provenance[field], adopted.provenance[field]);
        assert_eq!(
            view.provenance[field],
            resources.configuration().unwrap().authored.provenance[field]
        );
    }
    for unit in [
        ApplyUnit::ExecutionPolicy,
        ApplyUnit::SharedCapacity,
        ApplyUnit::Instructions,
    ] {
        assert_eq!(
            adopted.component_revisions[&unit],
            second_resources
                .configuration()
                .unwrap()
                .authored
                .component_revisions[&unit]
        );
    }
    assert!(fixture.manager.configuration_application(second).is_none());
    assert_eq!(
        second_probe
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        probe.configuration_preparations.load(Ordering::SeqCst),
        preparations
    );
    assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
    fixture.manager.load(second, None).await.unwrap();
    assert!(Arc::ptr_eq(
        &second_resources,
        &second_runtime.runtime_resources()
    ));
    assert_eq!(
        second_probe
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    fixture.close().await;
}

#[tokio::test]
async fn t06_t09_t16_process_restart_alone_does_not_defer_new_session() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let mut policy = fixture.manager.process_policy();
    policy.shutdown_deadline_ms += 1;
    write(
        &fixture,
        0,
        ConfigMutation::AppServer {
            authored: Some(policy),
        },
    )
    .await;
    assert_eq!(
        settled(&fixture, 0).await.units[&ApplyUnit::ProcessBindings],
        UnitApplication::ProcessRestart
    );
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap()
        .session;
    assert!(
        !fixture
            .manager
            .applications
            .is_deferred(created.id.as_str())
    );
    assert!(
        fixture
            .manager
            .configuration_application(&created.id)
            .is_none()
    );
    fixture.manager.load(&created.id, None).await.unwrap();
    assert!(
        fixture
            .manager
            .configuration_application(&created.id)
            .is_none()
    );
    assert_eq!(
        fixture
            .manager
            .probe(&created.active_conversation_id)
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    fixture.close().await;
}
