//! Atomic Workflow admission against deterministic candidate facts.
use super::*;

fn cfg274_agent_source(overrides: Option<Value>) -> WorkflowProgram {
    let mut agent = json!({"type":"agent","profile":"reviewer","task":"Review","output":schema(json!({}), &[])});
    if let Some(overrides) = overrides {
        agent["override"] = overrides;
    }
    WorkflowProgram::compile(WorkflowId::parse("review").unwrap(), serde_json::from_value(json!({
        "description":"Static review", "block": {
            "input":schema(json!({}), &[]), "output":schema(json!({}), &[]), "entry":"agent",
            "nodes":{"agent":agent,"done":{"type":"return","output":{"type":"literal","value":{}}}},
            "edges":[{"from":"agent","to":"done"}]
        }
    })).unwrap()).unwrap()
}

fn cfg274_agents(document: &str) -> crate::runtime::subagent::AgentCatalog {
    use crate::runtime::agent_profile::{AgentProfile, AgentProfileKind};
    let document = crate::local_runtime::agent_resources::parse(document).unwrap();
    crate::runtime::subagent::AgentCatalog::new([NamedAgentDefinition::new(
        profile("reviewer"),
        AgentProfile::from_document(&document, AgentProfileKind::Named, Vec::new()).unwrap(),
        "reviewer.toml".into(),
    )
    .unwrap()])
    .unwrap()
}

fn cfg274_admit_child(
    source: WorkflowProgram,
    agents: &crate::runtime::subagent::AgentCatalog,
    available: &crate::capabilities::AvailableToolCatalog,
    availability: &crate::capabilities::CapabilityAvailability,
    skills: &SkillSnapshot,
) -> WorkflowCatalog {
    let mut catalog = WorkflowCatalog::new([source]).unwrap();
    let servers = [(
        crate::runtime::identity::McpServerId::new("github"),
        crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            activation: crate::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: "never-started".into(),
                args: Vec::new(),
                cwd: None,
                environment: BTreeMap::new(),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        },
    )]
    .into();
    catalog.admit(available, availability, skills, agents, &servers);
    catalog
}

fn cfg274_reasons(catalog: &WorkflowCatalog) -> &[WorkflowAdmissionDiagnostic] {
    let WorkflowAdmission::Disabled(reasons) =
        &catalog.entries().values().next().unwrap().admission
    else {
        panic!("expected atomic disable");
    };
    assert!(catalog.enabled_ids().is_empty());
    reasons
}

#[test]
fn cfg274_missing_role_and_required_child_exact_tool_disable_instead_of_suppression() {
    use crate::runtime::agent_profile::{AgentProfileAuthority, AgentScope, resolve_agent_profile};
    let skills = SkillSnapshot::new(Vec::new());
    let available = crate::capabilities::AvailableToolCatalog::default();
    let availability = crate::capabilities::CapabilityAvailability::new();
    let absent = cfg274_admit_child(
        cfg274_agent_source(None),
        &crate::runtime::subagent::AgentCatalog::empty(),
        &available,
        &availability,
        &skills,
    );
    assert!(matches!(
        cfg274_reasons(&absent)[0].reason,
        WorkflowDependencyFailure::Agent(
            crate::runtime::agent_profile::AgentProfileDiagnostic::AgentUnavailable { .. }
        )
    ));
    let agents = cfg274_agents(
        "description='Review'\ninstructions='Review'\n[tools.sources]\ngithub=['missing']",
    );
    let ready = [(
        crate::capabilities::ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new(
            "github",
        )),
        crate::capabilities::CapabilitySourceState::Ready,
    )]
    .into();
    let ordinary = resolve_agent_profile(
        agents.get(&profile("reviewer")).unwrap().profile(),
        &AgentProfileAuthority {
            tools: &available,
            availability: &ready,
            skills: &skills,
            agents: &BTreeSet::new(),
            workflows: &BTreeSet::new(),
            scope: AgentScope::OneShotChild,
        },
    );
    assert!(ordinary.tools.is_empty());
    assert_eq!(ordinary.diagnostics.len(), 1);
    let disabled = cfg274_admit_child(
        cfg274_agent_source(None),
        &agents,
        &available,
        &ready,
        &skills,
    );
    assert_eq!(
        cfg274_reasons(&disabled)[0].reason,
        WorkflowDependencyFailure::Agent(ordinary.diagnostics[0].clone())
    );
    let overridden = cfg274_admit_child(
        cfg274_agent_source(Some(json!({"tools":{"sources":{"github":["missing"]}}}))),
        &cfg274_agents("description='Review'\ninstructions='Review'"),
        &available,
        &ready,
        &skills,
    );
    assert_eq!(cfg274_reasons(&disabled), cfg274_reasons(&overridden));
    let offline = cfg274_admit_child(
        cfg274_agent_source(None),
        &agents,
        &available,
        &availability,
        &skills,
    );
    assert!(matches!(
        cfg274_reasons(&offline)[0].reason,
        WorkflowDependencyFailure::Agent(
            crate::runtime::agent_profile::AgentProfileDiagnostic::Tool(
                crate::capabilities::selection::ToolSelectionError::SourceUnavailable { .. }
            )
        )
    ));
}

#[test]
fn cfg274_replacements_remove_entire_defaults_and_known_goal_disables_child() {
    let agents = cfg274_agents(
        "description='Review'\ninstructions='Review'\nskills=['missing']\n[tools.sources]\ngithub=['missing']\n[extensions.goal]\nenabled=true",
    );
    let available = crate::capabilities::AvailableToolCatalog::default();
    let skills = SkillSnapshot::new(Vec::new());
    let availability = crate::capabilities::CapabilityAvailability::new();
    let disabled = cfg274_admit_child(
        cfg274_agent_source(None),
        &agents,
        &available,
        &availability,
        &skills,
    );
    assert_eq!(cfg274_reasons(&disabled).len(), 3);
    assert!(cfg274_reasons(&disabled).iter().any(|reason| matches!(
        reason.reason,
        WorkflowDependencyFailure::Agent(
            crate::runtime::agent_profile::AgentProfileDiagnostic::ScopeUnsupported { .. }
        )
    )));
    let enabled = cfg274_admit_child(
        cfg274_agent_source(Some(json!({"tools":{},"skills":[],"extensions":{}}))),
        &agents,
        &available,
        &availability,
        &skills,
    );
    let program = enabled
        .executable(&WorkflowId::parse("review").unwrap())
        .unwrap();
    assert_eq!(program.agent_nodes().len(), 1);
    assert!(program.agent_nodes()[0].resolved.is_some());
    // Present tools cannot leave a source member inherited from the default.
    let goal_only = cfg274_admit_child(
        cfg274_agent_source(Some(json!({"tools":{},"skills":[]}))),
        &agents,
        &available,
        &availability,
        &skills,
    );
    assert_eq!(cfg274_reasons(&goal_only).len(), 1);
}

#[test]
fn cfg274_all_freezes_exact_source_set_and_recovery_readmits_only_new_catalog() {
    let plane = workflow_test_plane(1);
    let context = workflow_test_context(&plane);
    let agents = cfg274_agents("description='Review'\ninstructions='Review'");
    let source = crate::capabilities::ToolSourceId::Mcp(
        crate::runtime::identity::McpServerId::new("github"),
    );
    let ready = [(source, crate::capabilities::CapabilitySourceState::Ready)].into();
    let available = |names: &[&str]| {
        crate::capabilities::AvailableToolCatalog::metadata(names.iter().map(|name| {
            let mut tool = definition();
            tool.name = (*name).into();
            tool.id = crate::runtime::identity::ToolId::new(format!("github-{name}"));
            tool.origin = ToolOrigin::Mcp {
                server_id: crate::runtime::identity::McpServerId::new("github"),
            };
            tool
        }))
    };
    let program = || cfg274_agent_source(Some(json!({"tools":{"sources":{"github":"all"}}})));
    let skills = SkillSnapshot::new(Vec::new());
    let r1 = cfg274_admit_child(program(), &agents, &available(&["a", "b"]), &ready, &skills);
    let r2 = cfg274_admit_child(program(), &agents, &available(&["c"]), &ready, &skills);
    let frozen = |catalog: &WorkflowCatalog| {
        context
            .bind_workflow_agent(
                catalog
                    .executable(&WorkflowId::parse("review").unwrap())
                    .unwrap()
                    .agent_nodes()[0]
                    .resolved
                    .as_ref()
                    .unwrap(),
            )
            .unwrap()
    };
    assert_eq!(
        frozen(&r1)
            .tools
            .iter()
            .map(crate::runtime::subagent::resolver::ResolvedSubagentTool::name)
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        frozen(&r2)
            .tools
            .iter()
            .map(crate::runtime::subagent::resolver::ResolvedSubagentTool::name)
            .collect::<Vec<_>>(),
        ["c"]
    );
    let lost = cfg274_admit_child(
        program(),
        &agents,
        &available(&[]),
        &crate::capabilities::CapabilityAvailability::new(),
        &skills,
    );
    assert_eq!(cfg274_reasons(&lost).len(), 1);
    assert_eq!(frozen(&r1).tools.len(), 2);
    let recovered = cfg274_admit_child(
        program(),
        &agents,
        &available(&["recovered"]),
        &ready,
        &skills,
    );
    assert_eq!(frozen(&recovered).tools[0].name(), "recovered");
}

#[tokio::test]
async fn cfg274_disabled_direct_start_runs_zero_nodes_tools_and_children() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let mut source = program_definition();
    let WorkflowNodeDefinition::Tool { selector, .. } =
        source.block.nodes.get_mut("check").unwrap()
    else {
        panic!()
    };
    *selector = crate::capabilities::selection::ExactToolSelector::Builtin {
        name: "missing".into(),
    };
    // A child is the entry node; the unavailable Tool occurs later.
    source.block.nodes.insert("first_child".into(), serde_json::from_value(json!({"type":"agent","profile":"reviewer","task":"Review","output":schema(json!({}), &[])})).unwrap());
    source.block.edges.push(WorkflowEdgeDefinition {
        from: "first_child".into(),
        to: "check".into(),
        port: None,
    });
    source.block.entry = "first_child".into();
    let source = Arc::new(compile_test(source).unwrap());
    let context = context.with_test_workflow(&source);
    let before = plane.registry.listing(false, 100);
    let expected = serde_json::to_value(cfg274_reasons(context.resources().workflows())).unwrap();
    for call in ["first", "second"] {
        let (_, cancellation) = workflow_cancellation();
        let error = runtime
            .run_foreground(
                source.id().clone(),
                ToolCallId::new(call),
                context.clone(),
                json!({"passed":true}),
                cancellation,
            )
            .await
            .unwrap_err();
        let WorkflowRunError::Disabled { diagnostics, .. } = error else {
            panic!("must retain admission reason")
        };
        assert_eq!(serde_json::to_value(diagnostics).unwrap(), expected);
    }
    let expected_status = context
        .resources()
        .workflows()
        .executable(source.id())
        .unwrap_err()
        .execution_status();
    let mut registrations = ToolRegistry::new();
    crate::tools::native::register_workflow_tools(
        &mut registrations,
        &runtime,
        context.resources().workflows(),
    )
    .unwrap();
    assert!(
        registrations.names().is_empty(),
        "disabled sources contribute no executable registrations"
    );
    let (_, cancellation) = workflow_cancellation();
    let (native_result, _) = run_outer(runtime.clone(), source, context, cancellation).await;
    assert_eq!(
        native_result.status, expected_status,
        "native and direct starts retain the same disabled reason"
    );
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(
        format!("{:?}", plane.registry.listing(false, 100)),
        format!("{before:?}")
    );
    assert!(runtime.read_model.snapshot().runs.is_empty());
    assert!(
        !plane
            .store
            .read_events(None, 128)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(
                event.event,
                RuntimeEvent::WorkflowStarted { .. } | RuntimeEvent::WorkflowNodeSettled { .. }
            ))
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One merged-catalog selection and lazy binding contract.
fn cfg274_child_skills_use_merged_catalog_explicitly_and_remain_lazy() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    for (root, name, description, extra) in [
        (home.join(".agents/skills"), "review", "global", ""),
        (
            workspace_root.join(".agents/skills"),
            "review",
            "workspace",
            "",
        ),
        (
            home.join(".agents/skills"),
            "root-only-visible",
            "visible",
            "",
        ),
        (
            workspace_root.join(".agents/skills"),
            "excluded",
            "excluded",
            "\ndisable-model-invocation: true",
        ),
    ] {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: {description}{extra}\n---\nSECRET_LAZY_BODY\n"
            ),
        )
        .unwrap();
    }
    let invalid = workspace_root.join(".agents/skills/invalid");
    std::fs::create_dir_all(&invalid).unwrap();
    std::fs::write(invalid.join("SKILL.md"), "invalid front matter").unwrap();
    let workspace = crate::tools::workspace::Workspace::new(&workspace_root).unwrap();
    let skills = SkillSnapshot::from_discovery(
        crate::skills::SkillDiscovery::with_config(
            &workspace,
            crate::skills::SkillDiscoveryConfig {
                automatic: crate::skills::automatic_skill_roots(
                    Some(&home),
                    workspace.root(),
                    &crate::skills::default_automatic_sources(),
                ),
                explicit_paths: Vec::new(),
            },
        )
        .discover()
        .unwrap(),
    );
    let available = crate::capabilities::AvailableToolCatalog::default();
    let availability = crate::capabilities::CapabilityAvailability::new();
    let agents = cfg274_agents("description='Review'\ninstructions='Review'");
    let plane = workflow_test_plane(1);
    let context = workflow_test_context(&plane);
    let frozen = |catalog: &WorkflowCatalog| {
        context
            .bind_workflow_agent(
                catalog
                    .executable(&WorkflowId::parse("review").unwrap())
                    .unwrap()
                    .agent_nodes()[0]
                    .resolved
                    .as_ref()
                    .unwrap(),
            )
            .unwrap()
    };
    let child = cfg274_admit_child(
        cfg274_agent_source(None),
        &agents,
        &available,
        &availability,
        &skills,
    );
    assert!(
        frozen(&child).skills.is_empty(),
        "root automatic visibility must not enter a named child"
    );
    let selected = cfg274_admit_child(
        cfg274_agent_source(Some(json!({"skills":["review"]}))),
        &agents,
        &available,
        &availability,
        &skills,
    );
    let selected = frozen(&selected);
    assert_eq!(selected.skills.len(), 1);
    assert_eq!(selected.skills[0].catalog_entry.description, "workspace");
    assert!(!format!("{selected:?}").contains("SECRET_LAZY_BODY"));
    for name in ["absent", "invalid", "excluded"] {
        let catalog = cfg274_admit_child(
            cfg274_agent_source(Some(json!({"skills":[name]}))),
            &agents,
            &available,
            &availability,
            &skills,
        );
        assert!(
            matches!(&cfg274_reasons(&catalog)[0].reason, WorkflowDependencyFailure::Agent(crate::runtime::agent_profile::AgentProfileDiagnostic::SkillUnavailable { name: selected }) if selected == name)
        );
    }
}

#[tokio::test]
async fn cfg274_enabled_run_freezes_generation_across_dependency_loss() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    let release = Arc::new(tokio::sync::Notify::new());
    Arc::get_mut(&mut probe).unwrap().release = Some(release.clone());
    let source = program();
    let r1 = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    )
    .with_test_workflow(&source);
    assert!(
        r1.resources()
            .capability()
            .tool_registry()
            .names()
            .is_empty()
    );
    let mut started = probe.started.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let running = tokio::spawn({
        let runtime = runtime.clone();
        let source = source.clone();
        let context = r1.clone();
        async move {
            runtime
                .run_foreground(
                    source.id().clone(),
                    ToolCallId::new("r1"),
                    context,
                    json!({"passed":true}),
                    cancellation,
                )
                .await
        }
    });
    started.wait_for(|count| *count == 1).await.unwrap();
    let r2 = workflow_test_context(&plane).with_test_workflow(&source);
    assert_eq!(cfg274_reasons(r2.resources().workflows()).len(), 1);
    assert!(r1.resources().workflows().executable(source.id()).is_ok());
    release.notify_one();
    assert_eq!(running.await.unwrap().unwrap(), json!({"passed":true}));
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    // Recovery admits a new frozen program without mutating R2's decision.
    let r3 = context(&plane, probe, crate::agent::AttemptLifecycle::default())
        .with_test_workflow(&source);
    assert!(r3.resources().workflows().executable(source.id()).is_ok());
    assert!(r2.resources().workflows().executable(source.id()).is_err());
}

#[test]
fn cfg274_model_exposure_requires_agent_selection_and_enabled_admission() {
    let plane = workflow_test_plane(1);
    let source = program();
    let context = context(
        &plane,
        Probe::new(ToolExecutionStatus::Success),
        crate::agent::AttemptLifecycle::default(),
    )
    .with_test_workflow(&source);
    let catalog = context.resources().workflows();
    let workflow =
        crate::tools::native::workflow_definition(catalog.executable(source.id()).unwrap());
    let mut root_registry = ToolRegistry::new();
    crate::tools::native::register_workflow_tools(
        &mut root_registry,
        &workflow_runtime(&plane),
        catalog,
    )
    .unwrap();
    assert!(matches!(
        root_registry.preflight(&crate::tools::types::ToolCall {
            id: ToolCallId::new("main-cannot-call-internal"),
            tool_id: definition().id,
            name: "check".into(),
            arguments: json!({"passed":true,"label":"main"}),
        }),
        Err(crate::tools::executor::ToolPreflightError::UnknownTool { .. })
    ));
    let ordinary = definition();
    let definitions = vec![&workflow, &ordinary];
    let mut activation = crate::capabilities::AgentActivation {
        profile: crate::local_runtime::agent_resources::parse("workflows=['test_workflow']")
            .unwrap(),
        admitted_workflows: catalog.enabled_ids(),
        ..Default::default()
    };
    let skills = SkillSnapshot::new(Vec::new());
    let availability = crate::capabilities::CapabilityAvailability::new();
    let select = |activation: &crate::capabilities::AgentActivation| {
        crate::capabilities::select_definitions(&definitions, activation, &skills, &availability)
            .unwrap()
    };
    assert_eq!(select(&activation), [&workflow]);
    assert!(
        !select(&activation).contains(&&ordinary),
        "Workflow selection cannot grant its internal Tool directly"
    );
    activation.no_direct_tools = true;
    assert_eq!(
        select(&activation),
        [&workflow],
        "direct Tool restrictions cannot remove independently admitted Workflow dispatch"
    );
    activation.no_direct_tools = false;
    activation.profile.workflows.clear();
    assert!(
        select(&activation).is_empty(),
        "enabled is not synonymous with model exposure"
    );
    activation.profile.workflows.push(source.id().clone());
    activation.admitted_workflows.clear();
    assert!(
        select(&activation).is_empty(),
        "disabled is suppressed for this Agent"
    );
    assert!(
        catalog.get(source.id()).is_some(),
        "inspection retains discovery"
    );
}

#[test]
fn cfg275_disabled_workflow_metadata_never_becomes_an_ordinary_agent_tool() {
    let source = program();
    let metadata = crate::tools::native::workflow_definition(&source);
    let mut profile = crate::local_runtime::config::AgentProfileDocument::default();
    profile.tools.builtin.push(metadata.name.clone());
    let resolved = crate::capabilities::inspect_profile(
        &[&metadata],
        &crate::capabilities::AgentActivation {
            profile,
            ..Default::default()
        },
        &SkillSnapshot::new(Vec::new()),
        &crate::capabilities::CapabilityAvailability::new(),
    )
    .unwrap();
    assert!(resolved.tools.is_empty());
    assert!(matches!(
        resolved.diagnostics.as_slice(),
        [crate::runtime::agent_profile::AgentProfileDiagnostic::Tool(
            crate::capabilities::selection::ToolSelectionError::UnknownCapability { .. }
        )]
    ));
}
