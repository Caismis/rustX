//! CFG-01 filesystem and real native composition regressions (Linux and macOS CI).
#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)] // linear fixture scenarios
use super::launch::*;
use super::{LocalRuntimeDependencies, LocalSessionProduct};
use crate::capabilities::{CapabilitySourceState, ToolSourceId};
use crate::model::ModelCatalog;
use serde_json::json;
use std::path::Path;

struct Fixture {
    root: tempfile::TempDir,
    host: HostEnvironment,
    credentials: crate::credentials::CredentialSnapshot,
    request: LaunchRequest,
}

fn template_source(id: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "examples/local-runtime/workflow-templates/.agents/workflows/{id}.yaml"
    )))
    .unwrap()
}

fn install_workflow(f: &Fixture, text: &str) -> std::path::PathBuf {
    let root = f.host.launch_directory.join(".agents/workflows");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("example.yaml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn cfg237_check_explain_zero_effects_precise_errors_and_authority() {
    use super::diagnostics::Validity;
    let f = Fixture::new();
    f.role(
        false,
        "reviewer",
        json!({"description":"Review","tools":{"builtin":[]},"worktree":{"enabled":false}}),
        "SECRET_ROLE_PROMPT",
    );
    let config = json!({"subagents":{"workflow":["reviewer"]},"workflows":{"main":[]}});
    let original = template_source("typed_agent");
    let scenarios = [
        (original.clone(), config.clone(), Validity::Valid, None),
        (
            original.clone(),
            json!({"workflows":{}}),
            Validity::Invalid,
            Some("block.nodes.summarize.profile"),
        ),
        (
            original.clone(),
            json!({"subagents":{},"workflows":{}}),
            Validity::Invalid,
            Some("block.nodes.summarize.profile"),
        ),
        (
            original.replace("[args, topic]", "[args, missing]"),
            config.clone(),
            Validity::Invalid,
            Some("block.nodes.summarize.input.topic"),
        ),
        (
            original.replace("timeout_ms: 60000", "timeout_ms: 0"),
            config.clone(),
            Validity::Invalid,
            Some("timeout_ms"),
        ),
        (
            original.replace("type: agent", "type: imaginary"),
            config.clone(),
            Validity::Invalid,
            Some("block.nodes.summarize.type"),
        ),
    ];
    for (source, configuration, validity, expected_path) in scenarios {
        f.project(configuration);
        let file = install_workflow(&f, &source);
        for explain in [false, true] {
            let (report, effects) = super::static_effects::measure(|| {
                super::workflow_inspection::inspect(
                    &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
                    explain,
                    &f.request,
                    &f.host,
                )
            });
            assert_eq!(effects, [0; 13]);
            assert_eq!(report.validity, validity, "{:?}", report.diagnostics);
            assert_eq!(
                report.exit_code(),
                if validity == Validity::Invalid { 2 } else { 3 }
            );
            if let Some(path) = expected_path {
                assert_eq!(report.diagnostics[0].file.as_ref(), Some(&file));
                assert_eq!(report.diagnostics[0].path, path);
            } else {
                let projection = report.workflow.as_ref().unwrap();
                assert!(projection.discovered);
                assert!(!projection.configured_main_admission);
                assert_eq!(projection.prospective_main_exposure, Some(false));
                assert_eq!(projection.program.is_some(), explain);
                assert!(projection.execution_admission.starts_with("not_performed"));
            }
            assert!(!report.render(true).contains("SECRET_ROLE_PROMPT"));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
            assert!(!f.resolve_locations_only().runtime_root.exists());
        }
    }
    // Malformed canonical files fail even without selection; untrusted files are not parsed.
    f.project(json!({}));
    install_workflow(&f, "invalid: [");
    let id = crate::runtime::workflow::WorkflowId::parse("example").unwrap();
    assert_eq!(
        super::workflow_inspection::inspect(&id, true, &f.request, &f.host).validity,
        Validity::Invalid
    );
    f.project(config);
    f.trust(TrustAction::Revoke);
    for explain in [false, true] {
        let (report, effects) = super::static_effects::measure(|| {
            super::workflow_inspection::inspect(&id, explain, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        assert_eq!(report.validity, Validity::Incomplete);
        assert!(report.workflow.is_none());
    }
}

#[test]
fn cfg237_graph_paths_reach_diagnostics_with_zero_side_effects() {
    let f = Fixture::new();
    f.role(
        false,
        "reviewer",
        json!({"description":"Review","tools":{"builtin":[]}}),
        "Review.",
    );
    f.project(json!({"subagents":{"workflow":["reviewer"]},"workflows":{}}));
    let original: serde_json::Value = serde_json::to_value(
        serde_yaml::from_str::<crate::runtime::workflow::WorkflowDefinition>(&template_source(
            "parallel_checks",
        ))
        .unwrap(),
    )
    .unwrap();
    for (pointer, value, expected) in [
        ("/description", json!(""), "description"),
        ("/description", json!("x".repeat(4097)), "description"),
        (
            "/block/edges/0/from",
            json!("missing"),
            "block.edges.0.from",
        ),
        ("/block/edges/0/to", json!("missing"), "block.edges.0.to"),
        ("/block/edges/0/port", json!("true"), "block.edges.0.port"),
        (
            "/block/nodes/check_text/branches/clarity/block/edges/0/to",
            json!("missing"),
            "block.nodes.check_text.branches.clarity.block.edges.0.to",
        ),
    ] {
        let mut shape = original.clone();
        *shape.pointer_mut(pointer).unwrap() = value;
        let source = serde_yaml::to_string(&shape).unwrap();
        let file = install_workflow(&f, &source);
        for explain in [false, true] {
            let (report, effects) = super::static_effects::measure(|| {
                super::workflow_inspection::inspect(
                    &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
                    explain,
                    &f.request,
                    &f.host,
                )
            });
            assert_eq!(effects, [0; 13]);
            assert_eq!(report.validity, super::diagnostics::Validity::Invalid);
            assert_eq!(report.exit_code(), 2);
            assert_eq!(report.diagnostics[0].path, expected);
            assert_eq!(report.diagnostics[0].file.as_ref(), Some(&file));
            assert!(report.render(true).contains(expected));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
            assert!(!f.resolve_locations_only().runtime_root.exists());
        }
    }
}

#[test]
fn cfg237_online_schema_unresolved_and_disabled_source_are_distinct() {
    use crate::runtime::workflow::inspection::DependencyState;
    let f = Fixture::new();
    let text = r"description: Inspect a declared external capability.
tools: [{origin: source, source_id: external, name: inspect}]
block:
  input: {type: object, properties: {}, additionalProperties: false}
  output: {type: object, properties: {text: {type: string}}, required: [text], additionalProperties: false}
  entry: inspect
  nodes:
    inspect:
      type: tool
      selector: {origin: source, source_id: external, name: inspect}
      arguments: {type: literal, value: {secret: SECRET_LITERAL}}
      result: {type: text, part: 0}
    done:
      type: return
      output: {type: object, fields: {text: {type: reference, path: [inspect]}}}
  edges: [{from: inspect, to: done}]
";
    install_workflow(&f, text);
    for enabled in [true, false] {
        f.project(json!({"mcp_servers":{"external":{"enabled":enabled,"command":"must-never-spawn"}},"workflows":{}}));
        for explain in [false, true] {
            let (report, effects) = super::static_effects::measure(|| {
                super::workflow_inspection::inspect(
                    &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
                    explain,
                    &f.request,
                    &f.host,
                )
            });
            assert_eq!(effects, [0; 13]);
            assert_eq!(report.validity, super::diagnostics::Validity::Incomplete);
            let state = &report.workflow.as_ref().unwrap().dependencies[0].state;
            if enabled {
                assert!(matches!(state, DependencyState::Unresolved));
            } else {
                assert!(matches!(
                    state,
                    DependencyState::Inert {
                        activation: crate::capabilities::activation::SourceActivation::Disabled
                    }
                ));
            }
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|d| d.path == "block.nodes.inspect.selector")
            );
            assert!(!report.render(true).contains("SECRET_LITERAL"));
            assert_eq!(report.exit_code(), 3);
        }
    }
}

#[test]
fn cfg237_nested_paths_parser_locations_and_compiler_agreement() {
    let f = Fixture::new();
    f.role(
        false,
        "reviewer",
        json!({"description":"Review","tools":{"builtin":[]}}),
        "Review.",
    );
    f.project(json!({"subagents":{"workflow":["reviewer"]},"workflows":{}}));
    let definition: crate::runtime::workflow::WorkflowDefinition =
        serde_yaml::from_str(&template_source("parallel_checks")).unwrap();
    let original = serde_json::to_value(definition).unwrap();
    for (pointer, value, expected) in [
        (
            "/block/nodes/check_text/branches/clarity/block/nodes/assess_clarity/input/text/path",
            json!(["assess_brevity", "passed"]),
            "block.nodes.check_text.branches.clarity.block.nodes.assess_clarity.input.text",
        ),
        (
            "/block/nodes/check_text/branches/clarity/block/nodes/assess_clarity/output/properties/passed",
            json!({"type":"boolean","pattern":"SECRET_SCHEMA"}),
            "block.nodes.check_text.branches.clarity.block.nodes.assess_clarity.output.properties.passed",
        ),
        (
            "/block/nodes/check_text/branches/clarity/block/nodes/return_clarity/output/path",
            json!(["args"]),
            "block.nodes.check_text.branches.clarity.block.nodes.return_clarity.output",
        ),
    ] {
        let mut shape = original.clone();
        *shape.pointer_mut(pointer).unwrap() = value;
        install_workflow(&f, &serde_yaml::to_string(&shape).unwrap());
        let report = super::workflow_inspection::inspect(
            &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
            true,
            &f.request,
            &f.host,
        );
        assert_eq!(report.validity, super::diagnostics::Validity::Invalid);
        assert_eq!(report.diagnostics[0].path, expected);
        assert!(!report.render(true).contains("SECRET_SCHEMA"));
    }
    install_workflow(&f, "description: [\n");
    let report = super::workflow_inspection::inspect(
        &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
        true,
        &f.request,
        &f.host,
    );
    assert!(report.diagnostics[0].line.is_some());
    assert!(report.diagnostics[0].column.is_some());
    install_workflow(&f, &serde_yaml::to_string(&original).unwrap());
    let launch = analyze(&f.request, &f.host).unwrap();
    let id = crate::runtime::workflow::WorkflowId::parse("example").unwrap();
    let report = super::workflow_inspection::inspect(&id, true, &f.request, &f.host);
    let view = report.workflow.unwrap().program.unwrap();
    assert_eq!(
        serde_json::to_value(view).unwrap(),
        serde_json::to_value(launch.workflows.get(&id).unwrap().inspect()).unwrap()
    );
}

#[test]
fn cfg237_workflow_projection_omission_preserves_validity_and_size_bound() {
    let f = Fixture::new();
    f.project(json!({"workflows":{}}));
    install_workflow(&f, &template_source("human_plan"));
    let mut report = super::workflow_inspection::inspect(
        &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
        true,
        &f.request,
        &f.host,
    );
    assert_eq!(report.validity, super::diagnostics::Validity::Valid);
    // Presentation-only pressure on a real compiled projection, not semantic evidence.
    report
        .workflow
        .as_mut()
        .unwrap()
        .program
        .as_mut()
        .unwrap()
        .input_schema = json!({"padding":"x".repeat(super::diagnostics::OUTPUT_LIMIT)});
    for json in [false, true] {
        let output = report.render(json);
        assert!(output.len() < super::diagnostics::OUTPUT_LIMIT);
        assert!(output.contains("projection_omitted"));
    }
    assert_eq!(report.exit_code(), 3);
    let projection: serde_json::Value = serde_json::from_str(&report.render(true)).unwrap();
    assert!(projection["workflow"].is_null());
    assert_eq!(projection["projection_omitted"], true);
    assert_eq!(projection["validity"], "valid");
}

#[test]
fn cfg236_offline_role_provenance_rejections_and_trust_have_zero_effects() {
    let f = Fixture::new();
    let user = f.role(
        true,
        "reviewer",
        json!({"description":"User","tools":{"builtin":["write"]}}),
        "User body",
    );
    let project = f.role(
        false,
        "reviewer",
        json!({"description":"Project","tools":{"builtin":["read"]}}),
        "Project body",
    );
    f.project(json!({"subagents":{"main":[],"workflow":["reviewer"]}}));
    for operation in ["config_check", "config_show"] {
        let ((report, launch), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect(operation, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        let roles = &report.launch.unwrap().roles;
        let role = roles.values().next().unwrap();
        assert_eq!(role.identity.as_str(), "reviewer");
        assert_eq!(role.selected, project);
        assert_eq!(role.overridden, Some(user.clone()));
        assert!(!launch.unwrap().runtime_root.exists());
    }
    std::fs::write(
        &project,
        "---\ndescription: x\nsecretUnsupportedField: SENTINEL\n---\nbody",
    )
    .unwrap();
    let ((report, _), effects) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(effects, [0; 13]);
    assert_eq!(report.validity, super::diagnostics::Validity::Invalid);
    assert_eq!(report.diagnostics[0].file, Some(project));
    assert_eq!(report.diagnostics[0].path, "agents.reviewer");
    assert!(!report.render(true).contains("SENTINEL"));
    f.trust(TrustAction::Revoke);
    let ((report, launch), effects) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(effects, [0; 13]);
    assert!(report.launch.unwrap().roles.is_empty());
    assert!(launch.unwrap().subagents.definitions().next().is_none());
}

#[cfg(unix)]
#[test]
fn cfg236_user_role_authority_resolves_alias_once_for_launch_and_diagnostics() {
    let mut f = Fixture::new();
    let selected = f.role(true, "reviewer", json!({"description":"User"}), "User body");
    f.project(json!({"subagents":{}}));
    let physical_config = f.host.config_directory.clone();
    let alias = f.root.path().join("config-alias");
    std::os::unix::fs::symlink(&physical_config, &alias).unwrap();
    f.host.config_directory = alias.clone();
    let launch = f.resolve();
    assert_eq!(launch.agent_root, physical_config.join("agents"));
    for operation in ["config_check", "config_show"] {
        let ((report, prospective), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect(operation, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        assert_eq!(prospective.unwrap().agent_root, launch.agent_root);
        assert_eq!(
            report
                .launch
                .unwrap()
                .roles
                .values()
                .next()
                .unwrap()
                .selected,
            selected
        );
    }
    // Retargeting the display alias cannot change the pinned source used by reload.
    let replacement = f.root.path().join("replacement");
    std::fs::create_dir(&replacement).unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&replacement, &alias).unwrap();
    let (catalog, _) = super::agent_resources::load(
        &launch.workspace,
        &launch.agent_root,
        &launch.config.subagents,
    )
    .unwrap();
    assert_eq!(
        catalog.definitions().next().unwrap().instructions(),
        "User body"
    );
    // Replacing the physical authority itself must fail, not capture its new target.
    std::fs::rename(&launch.agent_root, f.root.path().join("retired-roles")).unwrap();
    std::fs::write(
        replacement.join("reviewer.toml"),
        "---\ndescription: outside\n---\nOutside",
    )
    .unwrap();
    std::os::unix::fs::symlink(&replacement, &launch.agent_root).unwrap();
    assert!(
        super::agent_resources::load(
            &launch.workspace,
            &launch.agent_root,
            &launch.config.subagents
        )
        .unwrap_err()
        .to_string()
        .contains("outside trusted workspace")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg236_gated_role_reload_cancels_or_publishes_one_complete_generation() {
    let f = Fixture::new();
    f.role(
        false,
        "role",
        json!({"description":"R1", "tools":{"builtin":["read"]}}),
        "R1 body",
    );
    f.project(json!({"subagents":{"main":["role"],"workflow":[]}}));
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let r1 = product.runtime().runtime_resources();
    f.role(
        false,
        "role",
        json!({"description":"R2", "tools":{"builtin":["grep"]}}),
        "R2 body",
    );
    f.project(json!({"subagents":{"main":[],"workflow":["role"]}}));
    let gate = super::agent_resources::test_support::arm(&f.host.launch_directory);
    let runtime = product.runtime().clone();
    let reload = tokio::spawn(async move { runtime.reload_resources().await });
    gate.entered().await; // complete parsed/validated R2, before publication
    assert!(std::sync::Arc::ptr_eq(
        &r1,
        &product.runtime().runtime_resources()
    ));
    reload.abort();
    assert!(reload.await.unwrap_err().is_cancelled());
    assert!(std::sync::Arc::ptr_eq(
        &r1,
        &product.runtime().runtime_resources()
    ));
    drop(gate);
    let gate = super::agent_resources::test_support::arm(&f.host.launch_directory);
    let runtime = product.runtime().clone();
    let reload = tokio::spawn(async move { runtime.reload_resources().await });
    gate.entered().await;
    assert!(std::sync::Arc::ptr_eq(
        &r1,
        &product.runtime().runtime_resources()
    ));
    gate.release();
    reload.await.unwrap().unwrap();
    let r2 = product.runtime().runtime_resources();
    let role = crate::runtime::subagent::SubagentName::parse("role").unwrap();
    assert_eq!(r2.subagents().get(&role).unwrap().instructions(), "R2 body");
    assert!(r2.delegatable_agents().is_empty());
    assert!(r2.subagent_workflow_admission().contains(&role));
    assert_eq!(r1.subagents().get(&role).unwrap().instructions(), "R1 body");
    assert!(r1.delegatable_agents().contains(&role));
    assert!(r1.subagent_workflow_admission().is_empty());
    product.runtime().shutdown().await.unwrap();
}

#[test]
fn cfg271_offline_python_discovery_never_parses_or_prepares_packages() {
    for trusted in [false, true] {
        let f = Fixture::new();
        let root = f.host.launch_directory.join(".agents/tools/foo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("server.py"),
            "raise Exception('must never import')",
        )
        .unwrap();
        std::fs::write(
            root.join("requirements.txt"),
            "--invalid RUSTX_SECRET_SENTINEL_DO_NOT_LEAK",
        )
        .unwrap();
        if !trusted {
            f.trust(TrustAction::Revoke);
        }
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| count.set(0));
        let ((report, launch), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect("config_check", &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| assert_eq!(count.get(), 0));
        assert!(
            !report
                .render(true)
                .contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
        );
        let launch = launch.unwrap();
        assert_eq!(launch.managed_python.packages().len(), usize::from(trusted));
        assert!(!launch.runtime_root.exists());
        if trusted {
            let source = &report.launch.as_ref().unwrap().sources["python:foo"];
            assert!(source.discovered_package);
            assert_eq!(source.readiness, "unresolved");
        }
    }
}

#[test]
fn cfg235_provider_readiness_is_unresolved_without_credential_lookup() {
    let f = Fixture::new();
    let path = f.host.config_directory.join("models.toml");
    let mut model: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&path).unwrap()).unwrap();
    for credential in ["$CFG235_UNSET", "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"] {
        model["providers"]["host"]["api_key"] = json!(credential);
        std::fs::write(&path, toml::to_string_pretty(&model).unwrap()).unwrap();
        for mcp in [false, true] {
            f.project(if mcp {
                json!({"mcp_servers":{"online":{"enabled":true,"url":"http://127.0.0.1:9/mcp"}}})
            } else {
                json!({})
            });
            for operation in ["config_check", "config_show"] {
                let ((report, _), counts) = super::static_effects::measure(|| {
                    super::diagnostics::inspect(operation, &f.request, &f.host)
                });
                assert_eq!(counts, [0; 13]);
                assert_eq!(report.validity, super::diagnostics::Validity::Valid);
                assert_eq!(
                    report.readiness,
                    Some(super::diagnostics::Readiness::Unresolved)
                );
                assert_eq!(report.exit_code(), 3);
                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|d| d.path == "providers.host" && d.category == "unresolved")
                );
                if mcp {
                    assert!(
                        report
                            .diagnostics
                            .iter()
                            .any(|d| d.path == "mcp_servers.online" && d.category == "unresolved")
                    );
                }
                for output in [
                    report.render(false),
                    report.render(true),
                    format!("{report:?}"),
                ] {
                    assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
                    if credential.starts_with('$') {
                        assert!(output.contains("CFG235_UNSET"));
                    }
                }
            }
        }
    }
}

#[test]
fn cfg235_static_check_show_have_zero_effects_and_redacted_outputs() {
    let f = Fixture::new();
    let sentinel = "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK";
    for configuration in [
        json!({}),
        json!({"unknownField": true}),
        json!({"environment":{"DECLARED_LITERAL":sentinel}}),
        json!({"mcp_servers":{"offline":{"enabled":true,"url":"http://127.0.0.1:9/mcp"}}}),
        json!({"mcp_servers":{"disabled":{"enabled":false,"command":"must-never-spawn","args":[sentinel]}}}),
        json!({"mcp_servers":{"unconfigured":{"command":"must-never-spawn"}}}),
    ] {
        f.project(configuration);
        for operation in ["config_check", "config_show"] {
            let ((report, _), counts) = super::static_effects::measure(|| {
                super::diagnostics::inspect(operation, &f.request, &f.host)
            });
            assert_eq!(counts, [0; 13]);
            if let Some(projection) = &report.launch {
                for name in projection.sources.keys() {
                    let diagnostic = report
                        .diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.path == format!("mcp_servers.{name}"))
                        .unwrap();
                    assert!(diagnostic.file.is_some());
                    assert!(!diagnostic.reason.is_empty() && !diagnostic.correction.is_empty());
                    assert!(matches!(diagnostic.classification, "info" | "warning"));
                }
            }
            for output in [
                report.render(false),
                report.render(true),
                format!("{report:?}"),
            ] {
                assert!(!output.contains(sentinel), "{output}");
                assert!(!output.contains("current Session uses"));
            }
            assert!(!f.resolve_locations_only().runtime_root.exists());
        }
    }
    f.project(json!({"mcp_servers":{"untrusted":{"enabled":true,"command":"must-never-spawn"}}}));
    f.trust(TrustAction::Revoke);
    let ((report, launch), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_show", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 13]);
    assert_eq!(report.exit_code(), 3);
    assert!(
        launch
            .unwrap()
            .admit(|| panic!("untrusted admission must never capture credentials"))
            .is_err()
    );
}

#[test]
fn cfg235_prospective_values_and_origins_equal_runtime_resolution() {
    let mut f = Fixture::new();
    f.project(json!({"context":{"reserve_tokens":8192},"environment":{"PRIVATE":"RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"}}));
    f.request.exclude_tools = Some(vec!["read".into()]);
    let prospective = analyze(&f.request, &f.host).unwrap();
    let runtime = f.resolve();
    assert_eq!(prospective.config(), runtime.config());
    assert_eq!(prospective.provenance(), runtime.provenance());
    assert_eq!(prospective.workspace, runtime.workspace);
    assert_eq!(prospective.runtime_root, runtime.runtime_root);
    assert!(
        !prospective
            .selected_tools
            .as_ref()
            .unwrap()
            .contains(&"read".into())
    );
    assert!(!format!("{prospective:?} {runtime:?}").contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
}

#[test]
fn cfg235_oversized_invalid_projection_preserves_authoritative_diagnostics() {
    let f = Fixture::new();
    let package = f.host.launch_directory.join(".agents/tools/foo");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("server.py"), "# inert").unwrap();
    std::fs::write(
        package.join("requirements.txt"),
        "--index-url RUSTX_SECRET_SENTINEL_DO_NOT_LEAK",
    )
    .unwrap();
    f.project(json!({
        "mcp_servers":{"broken":{"enabled":true,"command":"fixture"}},
        "environment": (0..4096).map(|index| (format!("FIELD_{index}"), "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")).collect::<std::collections::BTreeMap<_,_>>()
    }));
    let (mut report, _) = super::diagnostics::inspect("config_show", &f.request, &f.host);
    report.validity = super::diagnostics::Validity::Invalid;
    report.diagnostics.insert(
        0,
        super::diagnostics::Report::failure(
            "config_show",
            Some(f.host.launch_directory.join(".agents/agents/bad.toml")),
            "agents.bad",
            "invalid canonical Agent",
            "repair Agent TOML",
        )
        .diagnostics
        .remove(0),
    );
    assert_bounded_cause(&report, "agents.bad", "invalid");
}

#[test]
fn cfg235_oversized_incomplete_projection_preserves_incomplete_diagnostic() {
    let f = Fixture::new();
    f.user(json!({}));
    f.project(json!({"default_tools": (0..12000).map(|index| format!("unresolved_tool_identity_{index}")).collect::<Vec<_>>(), "environment":{"PRIVATE":"RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"}}));
    let (report, _) = super::diagnostics::inspect("config_show", &f.request, &f.host);
    assert_eq!(report.exit_code(), 3);
    assert!(report.partial.is_some());
    assert_bounded_cause(&report, &report.diagnostics[0].path, "incomplete");
}

fn assert_bounded_cause(report: &super::diagnostics::Report, path: &str, category: &str) {
    let original = report
        .diagnostics
        .iter()
        .find(|d| d.path == path && d.category == category)
        .unwrap();
    let output = report.render(true);
    let human = report.render(false);
    for text in [&output, &human, &format!("{report:?}")] {
        assert!(!text.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
    }
    assert!(output.len() + 1 < super::diagnostics::OUTPUT_LIMIT);
    assert!(human.len() + 1 < super::diagnostics::OUTPUT_LIMIT);
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    let human_value: serde_json::Value =
        serde_json::from_str(human.split_once('\n').unwrap().1).unwrap();
    assert_eq!(value, human_value);
    assert_eq!(value["validity"], category);
    assert_eq!(value["readiness"], "unresolved");
    assert_eq!(value["projection_omitted"], true);
    assert!(value["launch"].is_null() && value["partial"].is_null());
    let diagnostics = value["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), report.diagnostics.len() + 1);
    for (retained, original) in diagnostics.iter().zip(&report.diagnostics) {
        assert_eq!(*retained, serde_json::to_value(original).unwrap());
    }
    let cause = diagnostics
        .iter()
        .find(|d| d["path"] == path && d["category"] == category)
        .unwrap();
    assert_eq!(*cause, serde_json::to_value(original).unwrap());
    assert!(!cause["reason"].as_str().unwrap().is_empty());
    assert!(!cause["correction"].as_str().unwrap().is_empty());
    if category == "invalid" {
        assert_eq!(cause["classification"], "error");
        assert!(cause["file"].is_string());
    }
    assert!(
        diagnostics
            .iter()
            .any(|d| d["category"] == "projection_limit")
    );
}

#[test]
fn cfg235_diagnostics_only_overflow_preserves_first_cause_deterministically() {
    use super::diagnostics::{Diagnostic, OUTPUT_LIMIT, Report};
    let mut report = Report::failure(
        "config_check",
        Some("rustx.toml".into()),
        "python_sources.python:foo",
        "enabled package violates the local package contract",
        "repair server.py and requirements.txt",
    );
    let cause = report.diagnostics.remove(0);
    report.diagnostics = (0..512)
        .map(|index| Diagnostic {
            classification: "info",
            category: "inert",
            file: None,
            path: format!("source.{index}"),
            reason: "diagnostic detail ".repeat(256),
            correction: "leave inert".into(),
            line: None,
            column: None,
        })
        .collect();
    report.diagnostics.insert(100, cause.clone());
    for oversized_cause in [false, true] {
        if oversized_cause {
            report.diagnostics[100]
                .reason
                .push_str(&"\"🦀\n".repeat(100_000));
        }
        let output = report.render(true);
        assert_eq!(output, report.render(true));
        assert!(output.len() + 1 < OUTPUT_LIMIT);
        let human = report.render(false);
        assert!(human.len() + 1 < OUTPUT_LIMIT);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            value,
            serde_json::from_str::<serde_json::Value>(human.split_once('\n').unwrap().1).unwrap()
        );
        assert_eq!(report.exit_code(), 2);
        assert_eq!(value["validity"], "invalid");
        assert_eq!(value["readiness"], "unresolved");
        let diagnostics = value["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics[0]["path"], cause.path);
        assert_eq!(diagnostics[0]["classification"], "error");
        assert!(
            diagnostics[0]["reason"]
                .as_str()
                .unwrap()
                .starts_with(&cause.reason)
        );
        assert_eq!(diagnostics[0]["correction"], cause.correction);
        if !oversized_cause {
            assert_eq!(diagnostics[0], serde_json::to_value(&cause).unwrap());
        }
        for (index, diagnostic) in diagnostics[1..diagnostics.len() - 2].iter().enumerate() {
            assert_eq!(diagnostic["path"], format!("source.{index}"));
        }
        assert_eq!(
            diagnostics[diagnostics.len() - 2]["category"],
            "projection_limit"
        );
        assert_eq!(
            diagnostics.last().unwrap()["category"],
            "diagnostics_truncated"
        );
        assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
    }
}

#[test]
fn cfg235_projection_has_a_structured_size_bound_without_changing_validity() {
    let f = Fixture::new();
    f.project(json!({"environment":(0..4096).map(|index| (format!("FIELD_{index}"), "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")).collect::<std::collections::BTreeMap<_,_>>()}));
    let (report, _) = super::diagnostics::inspect("config_show", &f.request, &f.host);
    assert_eq!(report.exit_code(), 3);
    let output = report.render(true);
    assert!(output.len() < 256 * 1024);
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["projection_omitted"], true);
    assert_eq!(value["validity"], "valid");
    assert_eq!(
        value["diagnostics"][0],
        serde_json::to_value(&report.diagnostics[0]).unwrap()
    );
    assert!(value["launch"].is_null());
    assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
    assert!(report.render(false).len() < 256 * 1024);
}

#[test]
fn cfg235_all_example_layers_use_real_resolver_and_workflow_compiler() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime");
    for (workspace, config, model, expected_workflows) in [
        ("minimal/workspace", None, "minimal/models.toml", 0),
        ("workflow-basic", None, "minimal/models.toml", 1),
        ("workspace", Some("rustx.toml"), "models.toml", 2),
    ] {
        let f = Fixture::new();
        let mut host = f.host.clone();
        host.launch_directory = base.join(workspace);
        std::fs::write(
            host.config_directory.join("models.toml"),
            std::fs::read(base.join(model)).unwrap(),
        )
        .unwrap();
        std::fs::write(
            host.config_directory.join("settings.toml"),
            std::fs::read(base.join("minimal/settings.toml")).unwrap(),
        )
        .unwrap();
        let request = LaunchRequest {
            workspace: Some(host.launch_directory.clone()),
            config: config.map(|path| base.join(path)),
            ..Default::default()
        };
        change_trust(&request, &host, TrustAction::Grant).unwrap();
        let prospective = analyze(&request, &host).unwrap();
        assert_eq!(
            prospective.workflows.definitions().len(),
            expected_workflows
        );
        assert!(
            prospective
                .admit(crate::credentials::CredentialSnapshot::default)
                .is_ok()
        );
    }
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test]
async fn cfg235_probe_verifies_mcp_without_business_calls_and_respects_inert_sources() {
    use super::probes::{ProbeState, execute, plan};
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    let fixture = HttpFixture::start(HttpFixtureControl::new()).await;
    let f = Fixture::new();
    f.project(json!({"mcp_servers":{
        "enabled":{"enabled":true,"url":fixture.endpoint},
        "disabled":{"enabled":false,"command":"must-never-execute"},
        "unconfigured":{"command":"must-never-execute"}
    }}));
    std::fs::create_dir_all(f.host.launch_directory.join(".agents/tools/optional")).unwrap();
    let (report, launch) = super::diagnostics::inspect("doctor", &f.request, &f.host);
    assert_eq!(report.validity, super::diagnostics::Validity::Valid);
    let launch = launch.unwrap();
    let probe_plan = plan(&launch, true);
    let results = execute(
        &launch,
        &probe_plan,
        crate::runtime::CancellationSignal::new(),
    )
    .await;
    let state = |name: &str| {
        results
            .iter()
            .find(|result| result.target == name)
            .unwrap()
            .state
    };
    assert_eq!(state("enabled"), ProbeState::Verified);
    assert_eq!(state("disabled"), ProbeState::Skipped);
    assert_eq!(state("unconfigured"), ProbeState::Skipped);
    assert_eq!(state("python:optional"), ProbeState::Skipped);
    assert_eq!(fixture.control.accepted_calls(), 0);
    assert!(!launch.environment_store_root().exists());
    f.trust(TrustAction::Revoke);
    let untrusted = analyze(&f.request, &f.host).unwrap();
    let plan = plan(&untrusted, true);
    assert!(plan.targets.iter().all(|target| !target.spawn_process
        && !target.network
        && !target.prepare_environment
        && !target.resolve_credentials));
    let results = execute(&untrusted, &plan, crate::runtime::CancellationSignal::new()).await;
    assert_eq!(
        results
            .iter()
            .find(|result| result.target == "enabled")
            .unwrap()
            .state,
        ProbeState::Unavailable
    );
    fixture.shutdown().await;
}

#[test]
fn cfg235_diagnostics_keep_source_field_classification_and_correction() {
    let f = Fixture::new();
    for (configuration, field) in [
        (json!({"unknown_field":true}), "$"),
        (json!({"approval_mode":"full_access"}), "approval_mode"),
        (json!({"subagents":{"main":["missing"]}}), "subagents.main"),
    ] {
        f.project(configuration);
        let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
        assert_eq!(report.exit_code(), 2);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.path, field);
        assert!(diagnostic.file.is_some());
        assert_eq!(diagnostic.classification, "error");
        assert_eq!(diagnostic.category, "invalid");
        assert!(!diagnostic.reason.is_empty());
        assert!(!diagnostic.correction.is_empty());
    }
    std::fs::write(
        f.host.launch_directory.join("rustx.toml"),
        "# document\n[bad",
    )
    .unwrap();
    let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
    assert_eq!(report.diagnostics[0].line, Some(2));
    assert!(report.diagnostics[0].column.is_some());
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test]
async fn cfg235_probe_stdio_timeout_and_cancel_reap_owned_process() {
    use super::probes::{ProbeState, execute, plan};
    use crate::tools::mcp::fixture::{
        FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
    };
    use std::sync::Arc;
    if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
        return;
    }
    for timed_out in [false, true] {
        let f = Fixture::new();
        f.user(json!({"model":{"model":"host/one"}, "mcp_servers":{"owned":{
            "enabled":true, "command":std::env::current_exe().unwrap(),
            "args":fixture_spawn_args("local_runtime::launch_tests::cfg235_probe_stdio_timeout_and_cancel_reap_owned_process"),
            "env":{FIXTURE_MODE_ENV:"1"}
        }}}));
        let launch = analyze(&f.request, &f.host).unwrap();
        let mut plan = plan(&launch, false);
        let pause = Arc::new(crate::tools::mcp::test_sync::ConnectOwnershipPause::default());
        let expire = Arc::new(tokio::sync::Notify::new());
        plan.hooks.ownership_pause = Some(pause.clone());
        plan.hooks.expire = Some(expire.clone());
        let cancellation = crate::runtime::CancellationSignal::new();
        let work = execute(&launch, &plan, cancellation.clone());
        tokio::pin!(work);
        tokio::select! { () = pause.wait_entered() => {}, _ = &mut work => panic!("probe returned before owned pause") }
        let pid =
            nix::unistd::Pid::from_raw(i32::try_from(pause.supervisor_pid().unwrap()).unwrap());
        if timed_out {
            expire.notify_one();
        } else {
            cancellation.cancel();
        }
        assert!(
            futures_util::poll!(&mut work).is_pending(),
            "caller must await physical owner"
        );
        pause.release();
        let results = work.await;
        let result = results
            .iter()
            .find(|result| result.target == "owned")
            .unwrap();
        assert_eq!(
            result.state,
            if timed_out {
                ProbeState::TimedOut
            } else {
                ProbeState::Cancelled
            }
        );
        assert_eq!(
            nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
            Err(nix::errno::Errno::ECHILD)
        );
        assert_eq!(
            nix::sys::signal::kill(pid, None),
            Err(nix::errno::Errno::ESRCH)
        );
        assert!(
            !launch.runtime_root.exists(),
            "no Session or recovery state"
        );
    }
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test]
async fn cfg235_probe_close_is_awaited_and_failed_close_is_not_verified() {
    use super::probes::{ProbeState, execute, plan};
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    use std::sync::Arc;
    let fixture = HttpFixture::start(HttpFixtureControl::new()).await;
    let f = Fixture::new();
    f.project(json!({"mcp_servers":{"owned":{"enabled":true,"url":fixture.endpoint}}}));
    let launch = analyze(&f.request, &f.host).unwrap();
    let mut plan = plan(&launch, false);
    let close = Arc::new(crate::tools::mcp::test_sync::CloseProbe::parking());
    plan.hooks.close = Some(close.clone());
    let cancellation = crate::runtime::CancellationSignal::new();
    let work = execute(&launch, &plan, cancellation.clone());
    tokio::pin!(work);
    tokio::select! { () = close.wait_entered() => {}, _ = &mut work => panic!("returned before close") }
    cancellation.cancel();
    assert!(futures_util::poll!(&mut work).is_pending());
    close.release();
    let results = work.await;
    assert_eq!(results[1].state, ProbeState::Cancelled);
    assert!(results[1].verified.is_none());
    let mut plan = super::probes::plan(&launch, false);
    plan.hooks.close = Some(Arc::new(crate::tools::mcp::test_sync::CloseProbe::failing(
        "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK",
    )));
    let results = execute(&launch, &plan, crate::runtime::CancellationSignal::new()).await;
    assert_eq!(results[1].state, ProbeState::Failed);
    assert!(results[1].verified.is_none());
    assert!(
        !super::probes::render_results(&results, true)
            .contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
    );
    assert_eq!(fixture.control.accepted_calls(), 0);
    fixture.shutdown().await;
}

#[test]
fn cfg235_incomplete_missing_explicit_workflow_and_credential_reference_states() {
    let mut f = Fixture::new();
    let model_path = f.host.config_directory.join("models.toml");
    let mut model: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&model_path).unwrap()).unwrap();
    model["providers"]["host"]["api_key"] = json!("$CFG235_UNSET_CREDENTIAL");
    std::fs::write(&model_path, toml::to_string_pretty(&model).unwrap()).unwrap();
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 13]);
    assert_eq!(
        report.exit_code(),
        3,
        "execution credential and connectivity facts remain unresolved"
    );
    f.request.config = Some("explicit-missing.toml".into());
    let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
    assert_eq!(report.exit_code(), 2);
    assert_eq!(
        report.diagnostics[0].file,
        Some(f.host.launch_directory.join("explicit-missing.toml"))
    );
    f.request.config = None;
    let workflow = f
        .host
        .launch_directory
        .join(".agents/workflows/broken.yaml");
    std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
    std::fs::write(&workflow, "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK: [").unwrap();
    f.project(json!({"workflows":{"main":["broken"]}}));
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 13]);
    assert_eq!(report.exit_code(), 2);
    assert_eq!(report.diagnostics[0].file, Some(workflow));
    assert!(
        !report
            .render(true)
            .contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
    );
    f.project(json!({}));
    let valid = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/local-runtime/workflow-basic/.agents/workflows/read_file.yaml"),
    )
    .unwrap();
    std::fs::write(
        f.host
            .launch_directory
            .join(".agents/workflows/broken.yaml"),
        valid.replace("entry: read", "entry: missing_node"),
    )
    .unwrap();
    f.project(json!({"workflows":{"main":["broken"]}}));
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 13]);
    assert_eq!(
        report.exit_code(),
        2,
        "native compiler rejects the missing entry node"
    );
    assert_eq!(report.diagnostics[0].path, "block.entry");
    f.project(json!({}));
    f.user(json!({}));
    let (report, _) = super::diagnostics::inspect("config_show", &f.request, &f.host);
    assert_eq!(report.exit_code(), 3);
    assert_eq!(report.diagnostics[0].category, "incomplete");
    assert!(report.partial.is_some());
    std::fs::remove_file(model_path).unwrap();
    let (report, _) = super::diagnostics::inspect("config_show", &f.request, &f.host);
    assert_eq!(report.exit_code(), 3);
    assert!(report.partial.is_some());
}

#[tokio::test]
async fn cfg271_probe_cannot_turn_discovery_into_python_preparation() {
    use crate::runtime::process_runner::{
        CapturedProcessResult, RunnerTestControl, SupervisedCommandSpec, SupervisedProcessRunner,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(Default)]
    struct FailingRunner(AtomicUsize);
    impl SupervisedProcessRunner for FailingRunner {
        fn run(
            &self,
            _: SupervisedCommandSpec,
            _: Option<RunnerTestControl>,
        ) -> futures_util::future::BoxFuture<'_, Result<CapturedProcessResult, String>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK".into()) })
        }
    }
    let f = Fixture::new();
    let package = f.host.launch_directory.join(".agents/tools/optional");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("server.py"), "# never executed by this test\n").unwrap();
    std::fs::write(package.join("requirements.txt"), "").unwrap();
    f.project(json!({}));
    let launch = analyze(&f.request, &f.host).unwrap();
    let runner = Arc::new(FailingRunner::default());
    let store = crate::tools::python::PythonToolStore::with_binaries_and_runner(
        f.root.path().join("recorded-store"),
        "/fixture/uv".into(),
        "/fixture/python3".into(),
        runner.clone(),
    )
    .unwrap();
    for authorized in [false, true, true] {
        let before = runner.0.load(Ordering::SeqCst);
        let mut plan = super::probes::plan(&launch, authorized);
        plan.hooks.python_store = Some(store.clone());
        let results =
            super::probes::execute(&launch, &plan, crate::runtime::CancellationSignal::new()).await;
        let result = results
            .iter()
            .find(|result| result.target == "python:optional")
            .unwrap();
        assert_eq!(runner.0.load(Ordering::SeqCst), before);
        assert_eq!(result.state, super::probes::ProbeState::Skipped);
        assert!(
            !super::probes::render_results(&results, true)
                .contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
        );
        assert!(!launch.runtime_root.exists());
    }
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test]
async fn cfg235_probe_credential_use_and_failures_never_leak_values() {
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    let fixture = HttpFixture::start(HttpFixtureControl::new()).await;
    let f = Fixture::new();
    f.user(json!({"model":{"model":"host/one"},"mcp_servers":{"secret":{"enabled":true,"url":fixture.endpoint,"sensitive_headers":{"Authorization":"$CFG235_TOKEN"}}}}));
    let launch = analyze(&f.request, &f.host).unwrap();
    for present in [false, true] {
        let mut plan = super::probes::plan(&launch, false);
        plan.hooks.credentials = Some(crate::credentials::CredentialSnapshot::new(if present {
            vec![(
                "CFG235_TOKEN".into(),
                "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK".into(),
            )]
        } else {
            vec![]
        }));
        assert!(plan.targets[1].resolve_credentials);
        let results =
            super::probes::execute(&launch, &plan, crate::runtime::CancellationSignal::new()).await;
        assert_eq!(
            results[1].state,
            if present {
                super::probes::ProbeState::Verified
            } else {
                super::probes::ProbeState::Failed
            }
        );
        for output in [
            plan.render(false),
            plan.render(true),
            format!("{plan:?} {results:?}"),
            super::probes::render_results(&results, false),
            super::probes::render_results(&results, true),
        ] {
            assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
        }
    }
    assert_eq!(fixture.control.accepted_calls(), 0);
    assert!(!launch.runtime_root.exists());
    fixture.shutdown().await;
}

impl Fixture {
    fn new() -> Self {
        // macOS exposes /var through /private/var; fixture expectations use
        // physical paths just like the resolver, not the temporary-dir alias.
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home"), None, None)
                .unwrap();
        std::fs::create_dir_all(&host.config_directory).unwrap();
        std::fs::write(host.config_directory.join("models.toml"), toml::to_string_pretty(&json!({
            "providers": { "host": { "base_url": "http://127.0.0.1:9/v1", "api_key": "fixture", "models": [
                {"id":"one", "protocol":"openai_chat_completions", "context_window":128_000, "max_output_tokens":4096,
                 "capabilities":{"input_modalities":["text"],"output_modalities":["text"],"tool_calls":true,"reasoning":false},"compat":{"chat_reasoning_replay":"omit"}},
                {"id":"two", "protocol":"openai_chat_completions", "context_window":128_000, "max_output_tokens":4096,
                 "capabilities":{"input_modalities":["text"],"output_modalities":["text"],"tool_calls":true,"reasoning":false},"compat":{"chat_reasoning_replay":"omit"}}
            ]}}
        })).unwrap()).unwrap();
        let fixture = Self {
            root,
            host,
            credentials: crate::credentials::CredentialSnapshot::default(),
            request: LaunchRequest::default(),
        };
        fixture.user(json!({"model":{"model":"host/one"}}));
        fixture.trust(TrustAction::Grant);
        fixture
    }
    fn user(&self, value: serde_json::Value) {
        std::fs::write(
            self.host.config_directory.join("settings.toml"),
            toml::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    }
    fn role(
        &self,
        user: bool,
        name: &str,
        metadata: serde_json::Value,
        body: &str,
    ) -> std::path::PathBuf {
        let root = if user {
            self.host.config_directory.join("agents")
        } else {
            self.host.launch_directory.join(".agents/agents")
        };
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{name}.toml"));
        let mut metadata = metadata;
        metadata["instructions"] = body.into();
        if let Some(value) = metadata.as_object_mut().unwrap().remove("timeoutMs") {
            metadata["timeout_ms"] = value;
        }
        if let Some(value) = metadata.as_object_mut().unwrap().remove("agentsMd") {
            metadata["agents_md"] = value;
        }
        std::fs::write(&path, toml::to_string_pretty(&metadata).unwrap()).unwrap();
        path
    }
    fn project(&self, value: serde_json::Value) {
        std::fs::write(
            self.host.launch_directory.join("rustx.toml"),
            toml::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    }
    fn trust(&self, action: TrustAction) {
        change_trust(&self.request, &self.host, action).unwrap();
    }
    fn resolve(&self) -> ResolvedLaunch {
        analyze(&self.request, &self.host)
            .unwrap()
            .admit(|| self.credentials.clone())
            .unwrap()
    }
    fn resolve_locations_only(&self) -> LaunchLocations {
        resolve_locations(&self.request, &self.host).unwrap().0
    }
}

#[test]
fn exact_selection_validates_non_cli_launch_requests_before_resolution() {
    let f = Fixture::new();
    for request in [
        LaunchRequest {
            tools: Some(vec![]),
            ..Default::default()
        },
        LaunchRequest {
            exclude_tools: Some(vec![]),
            ..Default::default()
        },
        LaunchRequest {
            tools: Some(vec!["read".into(), "read".into()]),
            ..Default::default()
        },
        LaunchRequest {
            exclude_tools: Some(vec!["read".into(), "read".into()]),
            ..Default::default()
        },
        LaunchRequest {
            no_tools: true,
            tools: Some(vec!["read".into()]),
            ..Default::default()
        },
        LaunchRequest {
            no_tools: true,
            exclude_tools: Some(vec!["read".into()]),
            ..Default::default()
        },
        LaunchRequest {
            no_tools: true,
            no_builtin_tools: true,
            ..Default::default()
        },
        LaunchRequest {
            no_builtin_tools: true,
            tools: Some(vec!["read".into()]),
            ..Default::default()
        },
    ] {
        assert!(resolve(&request, &f.host).is_err(), "{request:?}");
    }
}

#[test]
fn cfg271_python_enablement_settings_are_rejected() {
    for project_layer in [false, true] {
        for value in ["enabled", "disabled", "untrusted", "unconfigured"] {
            let f = Fixture::new();
            let mut document = json!({"python_sources":{"python:x":value}});
            if project_layer {
                f.project(document);
            } else {
                document["model"] = json!({"model":"host/one"});
                f.user(document);
            }
            let result = resolve(&f.request, &f.host);
            assert!(
                result.is_err(),
                "{value}, project={project_layer}: {result:?}"
            );
        }
    }
}

#[test]
fn cfg233_whole_source_replacement_never_rebinds_host_credentials() {
    let mut f = Fixture::new();
    f.credentials = crate::credentials::CredentialSnapshot::new([(
        "HOST_SECRET".into(),
        "CFG233_HOST_SECRET_SENTINEL".into(),
    )]);
    for (host, project) in [
        (
            json!({"enabled":true,"url":"https://host.invalid/mcp","sensitive_headers":{"Authorization":"$HOST_SECRET"}}),
            json!({"enabled":true,"url":"https://project.invalid/mcp"}),
        ),
        (
            json!({"enabled":true,"command":"host-server","sensitive_env":{"TOKEN":"$HOST_SECRET"}}),
            json!({"enabled":true,"command":"project-server"}),
        ),
    ] {
        f.user(json!({"model":{"model":"host/one"},"mcp_servers":{"service":host}}));
        f.project(json!({"mcp_servers":{"service":project}}));
        let launch = f.resolve();
        let bindings = super::composition::mcp_bindings_with_authority(
            launch.config(),
            &launch.workspace,
            launch.provenance(),
            &launch.credentials,
        )
        .unwrap();
        let binding = &bindings[&crate::runtime::identity::McpServerId::new("service")];
        assert!(binding.credentials.environment.is_empty());
        assert!(binding.credentials.headers.is_empty());
        assert!(!format!("{launch:?}").contains("CFG233_HOST_SECRET_SENTINEL"));
        for key in ["sensitive_env", "sensitive_headers"] {
            let mut forbidden = project.clone();
            forbidden[key] = json!({"TOKEN":"$HOST_SECRET"});
            f.project(json!({"mcp_servers":{"service":forbidden}}));
            assert!(
                resolve(&f.request, &f.host)
                    .unwrap_err()
                    .contains("host-owned")
            );
        }
    }
}

#[tokio::test]
async fn cfg233_provider_binding_uses_launch_snapshot_and_ignores_unused_missing_keys() {
    let mut f = Fixture::new();
    let path = f.host.config_directory.join("models.toml");
    let mut document: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&path).unwrap()).unwrap();
    document["providers"]["host"]["api_key"] = json!("$CAPTURED_KEY");
    document["providers"]["unused"] = document["providers"]["host"].clone();
    document["providers"]["unused"]["api_key"] = json!("$UNSET_UNUSED_KEY");
    std::fs::write(&path, toml::to_string_pretty(&document).unwrap()).unwrap();
    f.credentials = crate::credentials::CredentialSnapshot::new([(
        "CAPTURED_KEY".into(),
        "CFG233_PROVIDER_SENTINEL".into(),
    )]);
    let launch = f.resolve();
    f.credentials = crate::credentials::CredentialSnapshot::default();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(!format!("{launch:?}").contains("CFG233_PROVIDER_SENTINEL"));
    product.runtime().shutdown().await.unwrap();
    assert!(
        LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("CAPTURED_KEY")
    );
}

#[tokio::test]
async fn cfg233_enabled_missing_credentials_and_connection_failures_are_source_local() {
    let f = Fixture::new();
    f.user(json!({"tools":{"sources":{"credential":"all","connection":"all","disabled":"all"}},"model":{"model":"host/one"},"mcp_servers":{
        "credential":{"enabled":true,"command":"/does/not/exist","sensitive_env":{"TOKEN":"$REQUIRED_KEY"}},
        "connection":{"enabled":true,"command":"/does/not/exist"},
        "disabled":{"enabled":false,"command":"/does/not/exist","sensitive_env":{"TOKEN":"$IGNORED_KEY"}}
    }}));
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let sources = product.runtime().capability().availability();
    let source = |id| ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new(id));
    let CapabilitySourceState::Unavailable { reason } = &sources[&source("credential")] else {
        panic!("source failure")
    };
    assert!(reason.contains("credential") && reason.contains("env:REQUIRED_KEY"));
    assert!(matches!(
        sources[&source("connection")],
        CapabilitySourceState::Unavailable { .. }
    ));
    assert!(matches!(
        sources[&source("disabled")],
        CapabilitySourceState::Inactive { .. }
    ));
    assert!(!format!("{sources:?}").contains("IGNORED_KEY"));
    product.runtime().shutdown().await.unwrap();
}

#[tokio::test]
async fn cfg271_missing_empty_and_unprepared_python_allow_native_startup() {
    for populated in [None, Some(false), Some(true)] {
        let f = Fixture::new();
        let root = f.host.launch_directory.join(".agents/tools");
        if let Some(populated) = populated {
            std::fs::create_dir_all(&root).unwrap();
            if populated {
                std::fs::create_dir(root.join("unprepared")).unwrap();
            }
        }
        let product =
            LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
                .await
                .unwrap();
        assert_eq!(
            product
                .runtime()
                .runtime_resources()
                .managed_python_catalog()
                .packages()
                .len(),
            usize::from(populated == Some(true))
        );
        assert!(
            product
                .runtime()
                .capability()
                .availability()
                .iter()
                .filter(|(id, _)| matches!(id, ToolSourceId::ManagedPython(_)))
                .all(|(_, state)| *state == CapabilitySourceState::Unprepared)
        );
        product.runtime().shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn cfg233_native_composition_keeps_disabled_and_discovered_resources_inert() {
    let mut f = Fixture::new();
    f.request.no_tools = true;
    let package = f.host.launch_directory.join(".agents/tools/discovered");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("server.py"),
        "raise Exception('must not import')",
    )
    .unwrap();
    // No requirements file: inert discovery cannot require package validity.
    f.user(json!({"model":{"model":"host/one"},"mcp_servers":{
        "missing":{"enabled":false,"command":"/nonexistent/cfg233","sensitive_env":{"TOKEN":"$UNSET"}},
        "offline":{"enabled":false,"url":"http://127.0.0.1:1/mcp","sensitive_headers":{"Authorization":"$UNSET"}}
    }}));
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(
        !launch
            .environment_store_root()
            .read_dir()
            .unwrap()
            .any(|entry| entry.unwrap().path().join("python-tools").exists())
    );
    assert_eq!(std::fs::read_dir(&package).unwrap().count(), 1);
    product.runtime().shutdown().await.unwrap();
    f.trust(TrustAction::Revoke);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
    assert_eq!(std::fs::read_dir(&package).unwrap().count(), 1);
}

#[tokio::test]
async fn cfg233_shared_connect_gate_rejects_all_inert_states_without_spawn_or_network() {
    use crate::capabilities::activation::SourceActivation;
    use crate::tools::mcp::{McpInvalidationState, McpServerRuntime};
    let f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let marker = f.root.path().join("spawn-marker");
    f.user(json!({"model":{"model":"host/one"},"mcp_servers":{
        "stdio":{"command":"/bin/sh","args":["-c",format!("touch {}", marker.display())],"sensitive_env":{"TOKEN":"$UNSET"}},
        "http":{"url":format!("http://{}/mcp", listener.local_addr().unwrap()),"sensitive_headers":{"Authorization":"$UNSET"}}
    }}));
    let launch = f.resolve();
    let workspace = crate::tools::Workspace::new(&launch.workspace).unwrap();
    for decision in [
        SourceActivation::Disabled,
        SourceActivation::evaluate(
            Some(crate::capabilities::activation::SourceEnablement::Enabled),
            false,
        ),
        SourceActivation::Unconfigured,
    ] {
        for (id, mut binding) in launch.config.mcp_bindings().unwrap() {
            binding.activation = decision;
            let error = McpServerRuntime::connect(
                &id,
                &binding,
                &workspace,
                std::sync::Arc::new(McpInvalidationState::default()),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(decision.admit().unwrap_err()));
            assert!(
                !error.to_string().contains("UNSET"),
                "activation precedes credential resolution"
            );
        }
    }
    assert!(!marker.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn cfg233_http_credential_failure_redacts_peer_echo_and_configuration() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const SENTINEL: &str = "CFG233_HTTP_SECRET_BEARER_62df";
    let mut f = Fixture::new();
    f.credentials = crate::credentials::CredentialSnapshot::new([("AUTH".into(), SENTINEL.into())]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        assert!(String::from_utf8(request).unwrap().contains(SENTINEL));
        let body = format!("credential rejected: {SENTINEL}");
        socket.write_all(format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    f.user(json!({"tools":{"sources":{"authenticated":"all"}},"model":{"model":"host/one"},"mcp_servers":{"authenticated":{"enabled":true,"url":format!("http://{endpoint}/mcp"),"sensitive_headers":{"Authorization":"$AUTH"}}}}));
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    peer.await.unwrap();
    let sources = product.runtime().capability().availability();
    assert!(matches!(
        sources.values().next(),
        Some(CapabilitySourceState::Unavailable { .. })
    ));
    assert!(!format!("{sources:?} {launch:?}").contains(SENTINEL));
    assert!(
        !serde_json::to_string(launch.config())
            .unwrap()
            .contains(SENTINEL)
    );
    let response = product.endpoint().handle_request(
        crate::runtime_client::RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        },
    );
    let payload = serde_json::to_string(&response).unwrap();
    assert!(payload.contains("authenticated") && payload.contains("unavailable"));
    assert!(!payload.contains(SENTINEL));
    product.runtime().shutdown().await.unwrap();
}

#[test]
fn precedence_absence_empty_and_whole_entries_keep_provenance() {
    let mut f = Fixture::new();
    f.role(
        true,
        "role",
        json!({"description":"old","skills":["old"]}),
        "User body",
    );
    f.role(false, "role", json!({"description":"new"}), "Project body");
    let builtin = f.resolve();
    assert_eq!(builtin.config.agent_id.as_str(), "rustx");
    assert_eq!(builtin.config.context.reserve_tokens, 1024);
    assert_eq!(
        builtin.config.native_tools.to_policies(),
        crate::tools::NativeToolPolicies::default()
    );
    assert_eq!(builtin.provenance["agent_id"], Origin::Builtin);
    f.user(json!({"model":{"model":"host/one", "reasoning_profile":{"mode":"catalog_default"}}, "agent_id":"user", "context":{"reserve_tokens":2000,"keep_recent_tokens":6000},
        "default_tools":["read","bash"],"environment":{"USER_ENTRY":"one","REPLACED":"old"},
        "mcp_servers":{"service":{"command":"old-command","args":["old"]},"retained":{"command":"retained"}},
        "subagents":{}
    }));
    f.project(
        json!({"model":{"model":"host/two"},"context":{"reserve_tokens":3000},"default_tools":[],
            "environment":{"REPLACED":"new"}, "mcp_servers":{"service":{"command":"new-command"}},
            "subagents":{}
        }),
    );
    let resolved = f.resolve();
    assert_eq!(
        resolved.settings_view().reasoning_origin,
        crate::runtime_client::settings::SettingOrigin::User {
            document: f
                .host
                .config_directory
                .join("settings.toml")
                .display()
                .to_string(),
        },
    );
    assert_eq!(resolved.config.agent_id.as_str(), "user");
    assert_eq!(resolved.config.context.reserve_tokens, 3000);
    assert_eq!(resolved.config.context.keep_recent_tokens, 6000);
    assert!(resolved.config.agent.tools.builtin.is_empty());
    assert_eq!(resolved.config.environment.len(), 2);
    assert_eq!(resolved.config.environment["REPLACED"], "new");
    let service =
        &resolved.config.mcp_servers[&crate::runtime::identity::McpServerId::new("service")];
    assert!(service.args.is_empty(), "whole service replacement");
    let role = resolved.subagents.definitions().next().unwrap();
    assert!(role.skills().is_empty(), "whole role replacement");
    assert_eq!(role.instructions(), "Project body");
    let source =
        &resolved.role_sources[&crate::runtime::subagent::SubagentName::parse("role").unwrap()];
    assert_eq!(source.layer, "project");
    assert_eq!(
        source.overridden.as_ref().unwrap(),
        &f.host.config_directory.join("agents/role.toml")
    );
    assert!(matches!(
        resolved.provenance["context.keep_recent_tokens"],
        Origin::User { .. }
    ));
    assert!(matches!(
        resolved.provenance["context.reserve_tokens"],
        Origin::Project { .. }
    ));
    f.request.model = Some("host/one".into());
    f.request.tools = Some(vec!["read".into(), "bash".into()]);
    f.request.exclude_tools = Some(vec!["read".into()]);
    let cli = f.resolve();
    assert_eq!(cli.config.initial_model().model.to_string(), "host/one");
    assert!(matches!(cli.provenance["model.model"], Origin::Cli { .. }));
    assert!(matches!(
        cli.provenance["exclude_tools"],
        Origin::Cli { .. }
    ));
    assert_eq!(cli.tools, Some(vec!["read".into(), "bash".into()]));
    f.project(json!({"environment":{},"mcp_servers":{},"subagents":{}}));
    let empty = f.resolve();
    assert!(empty.config.environment.is_empty());
    assert!(empty.config.mcp_servers.is_empty());
    assert!(empty.config.agent.agents.is_empty());
    assert_eq!(empty.config.agent.tools.builtin, ["read", "bash"]);
}

#[test]
fn forbidden_authority_fails_through_discovery_and_relative_config_override() {
    let mut f = Fixture::new();
    for field in [
        "models",
        "providers",
        "credentials",
        "trust",
        "trusted",
        "trustStore",
        "stateDirectory",
        "runtime_root",
        "workspace",
    ] {
        f.project(json!({field:"../../host-authority"}));
        for explicit in [false, true] {
            f.request.config = explicit.then(|| "./rustx.toml".into());
            let error = resolve(&f.request, &f.host).unwrap_err();
            assert!(
                error.contains(field)
                    && (error.contains("forbidden") || error.contains("unknown field")),
                "{error}"
            );
        }
    }
}

#[test]
fn optional_explicit_malformed_and_typed_rejections_are_distinct() {
    let mut f = Fixture::new();
    assert_eq!(f.resolve().config.agent_id.as_str(), "rustx");
    f.request.config = Some("missing.toml".into());
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("cannot read")
    );
    f.request.config = None;
    std::fs::write(f.host.launch_directory.join("rustx.toml"), "[unterminated").unwrap();
    let malformed = analyze(&f.request, &f.host).unwrap_err();
    assert!(malformed.diagnostic.reason.contains("malformed TOML"));
    for (text, expected) in [
        ("unknown = 1", "unknown field `unknown`"),
        ("[context]\ntypo = 1", "unknown field `typo`"),
        ("default_tools = false", "expected a sequence"),
        ("skills = 'null'", "expected a sequence"),
    ] {
        text.parse::<toml_edit::DocumentMut>()
            .expect("syntactically valid TOML");
        std::fs::write(f.host.launch_directory.join("rustx.toml"), text).unwrap();
        let error = resolve(&f.request, &f.host).unwrap_err();
        assert!(error.contains(expected), "{text}: {error}");
    }
    f.project(json!({"context":{"summary_output_cap":{"mode":"model_limit"}},"skills":[]}));
    assert_eq!(f.resolve().config.context.summary_output_cap, None);
    std::fs::remove_file(f.host.config_directory.join("settings.toml")).unwrap();
    f.request.model = Some("host/one".into());
    assert!(
        resolve(&f.request, &f.host).is_ok(),
        "optional user settings"
    );
    std::fs::write(
        f.host.launch_directory.join("rustx.toml"),
        " ".repeat(1024 * 1024 + 1),
    )
    .unwrap();
    assert!(resolve(&f.request, &f.host).unwrap_err().contains("1 MiB"));
}

#[test]
fn relative_paths_keep_their_document_and_cli_bases() {
    let mut f = Fixture::new();
    std::fs::write(f.host.config_directory.join("user.md"), "User role").unwrap();
    for file in ["project.md", "instructions.md"] {
        std::fs::write(f.host.launch_directory.join(file), "Project instructions").unwrap();
    }
    for path in [
        f.host.config_directory.join("user-skills"),
        f.host.launch_directory.join("cli-skill"),
    ] {
        std::fs::create_dir(&path).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Path fixture\n---\nInstructions\n"),
        )
        .unwrap();
    }
    f.role(true, "user", json!({"description":"user"}), "User role");
    f.role(
        false,
        "project",
        json!({"description":"project","agentsMd":{"files":["instructions.md"]}}),
        "Project role",
    );
    f.user(json!({"model":{"model":"host/one"}, "skills":["user-skills"], "subagents":{}}));
    f.project(json!({"subagents":{}}));
    let resolved = f.resolve();
    assert_eq!(
        resolved.skill_paths,
        [f.host.config_directory.join("user-skills")]
    );
    f.request.skill_paths = vec!["cli-skill".into()];
    assert_eq!(
        f.resolve().skill_paths,
        [f.host.launch_directory.join("cli-skill")]
    );
    for (name, source) in &resolved.role_sources {
        let base = if name.as_str() == "user" {
            f.host.config_directory.join("agents")
        } else {
            f.host.launch_directory.join(".agents/agents")
        };
        assert_eq!(source.selected, base.join(format!("{name}.toml")));
    }
    f.request.config = Some("replacement.toml".into());
    std::fs::write(f.host.launch_directory.join("replacement.toml"), "").unwrap();
    assert_eq!(
        f.resolve().subagents.definitions().count(),
        2,
        "--config selects settings; canonical resources remain discovered"
    );
}

#[test]
fn workspace_boundaries_subdirectories_non_git_nested_and_explicit() {
    let mut f = Fixture::new();
    let original = f.resolve();
    f.project(json!({}));
    let sub = f.host.launch_directory.join("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    f.host.launch_directory = sub.clone();
    assert_eq!(f.resolve().identity, original.identity);
    assert_eq!(f.resolve().runtime_root, original.runtime_root);
    std::fs::write(sub.join("rustx.toml"), "").unwrap();
    let (_, nested) = resolve_locations(&f.request, &f.host).unwrap();
    assert_ne!(nested, original.identity);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
    f.request.workspace = Some(original.workspace.clone());
    assert_eq!(f.resolve().identity, original.identity);
    f.request.workspace = Some(sub);
    f.trust(TrustAction::Grant);
    assert_eq!(f.resolve().identity, nested);
}

#[cfg(unix)]
#[test]
#[allow(clippy::items_after_statements)] // test-local Git assertion helper
fn canonical_symlink_and_real_git_worktree_identities_are_stable_and_separate() {
    let f = Fixture::new();
    let original = f.resolve();
    let alias = f.root.path().join("alias");
    std::os::unix::fs::symlink(&original.workspace, &alias).unwrap();
    let mut host = f.host.clone();
    host.launch_directory = alias;
    assert_eq!(
        resolve(&f.request, &host).unwrap().identity,
        original.identity
    );
    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    git(&original.workspace, &["init", "--quiet"]);
    git(
        &original.workspace,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
            "--quiet",
        ],
    );
    let worktree = f.root.path().join("worktree");
    git(
        &original.workspace,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );
    host.launch_directory = worktree.clone();
    let (locations, identity) = resolve_locations(&f.request, &host).unwrap();
    assert_ne!(identity, original.identity);
    assert_ne!(locations.runtime_root, original.runtime_root);
    assert!(
        resolve(&f.request, &host)
            .unwrap_err()
            .contains("not trusted")
    );
    change_trust(&f.request, &host, TrustAction::Grant).unwrap();
    assert_eq!(
        resolve(&f.request, &host).unwrap().workspace,
        std::fs::canonicalize(worktree).unwrap()
    );
    change_trust(&f.request, &host, TrustAction::Revoke).unwrap();
    assert!(resolve(&f.request, &host).is_err());
    assert!(resolve(&f.request, &f.host).is_ok(), "revocation is scoped");
    let outside = host.launch_directory.join("untrusted.md");
    std::fs::write(&outside, "other worktree").unwrap();
    f.role(
        false,
        "other",
        json!({"description":"other","agentsMd":{"files":[outside]}}),
        "role",
    );
    f.project(json!({"subagents":{}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
}

#[tokio::test]
async fn minimal_native_composition_and_frozen_launch_ignore_later_config_edits() {
    let f = Fixture::new();
    let launch = f.resolve();
    f.user(json!({"model":{"model":"host/two"},"agent_id":"edited"}));
    f.project(json!({"model":{"model":"missing/model"}}));
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(
        product
            .runtime()
            .runtime_resources()
            .subagents()
            .names()
            .is_empty()
    );
    assert_eq!(launch.config.initial_model().model.to_string(), "host/one");
    assert_eq!(launch.config.agent_id.as_str(), "rustx");
    assert!(launch.config.mcp_servers.is_empty());
    assert!(launch.config.agent.skills.is_empty());
    product.runtime().shutdown().await.unwrap();
    assert!(
        resolve(&f.request, &f.host).is_err(),
        "a new launch sees the edit"
    );
}

#[tokio::test]
async fn resolution_and_composition_failures_preserve_published_session_selection() {
    let f = Fixture::new();
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    product.runtime().shutdown().await.unwrap();
    drop(product);
    let catalog = launch.runtime_root.join("sessions/catalog.json");
    let before = std::fs::read(&catalog).unwrap();
    f.trust(TrustAction::Revoke);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
    assert_eq!(before, std::fs::read(&catalog).unwrap());
    f.trust(TrustAction::Grant);
    f.project(json!({"agent_id":""}));
    assert!(resolve(&f.request, &f.host).is_err());
    assert_eq!(before, std::fs::read(&catalog).unwrap());
    f.project(json!({"skills":["missing-skill"]}));
    assert!(
        analyze(&f.request, &f.host).is_err(),
        "static resource failures precede composition"
    );
    assert_eq!(before, std::fs::read(&catalog).unwrap());
}

#[test]
fn no_model_or_protocol_is_guessed_and_host_paths_are_validated() {
    let f = Fixture::new();
    f.user(json!({}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unambiguous")
    );
    f.user(json!({"model":{"model":"one"}}));
    assert!(
        resolve(&f.request, &f.host).is_err(),
        "unqualified reference is ambiguous"
    );
    f.user(json!({"model":{"model":"host/one"}}));
    let model_path = f.host.config_directory.join("models.toml");
    let mut catalog: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&model_path).unwrap()).unwrap();
    catalog["providers"]["host"]["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("protocol");
    std::fs::write(model_path, toml::to_string_pretty(&catalog).unwrap()).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("protocol")
    );
    assert!(
        HostEnvironment::from_paths(
            f.host.launch_directory.clone(),
            "/home/test".into(),
            Some("relative".into()),
            None
        )
        .is_err()
    );
}

#[test]
fn host_state_and_catalog_overrides_keep_document_and_cli_origins() {
    let mut f = Fixture::new();
    f.user(json!({"models":"models.toml","runtime_root":"private","model":{"model":"host/one"}}));
    let user = f.resolve();
    assert_eq!(user.runtime_root, f.host.config_directory.join("private"));
    assert_eq!(
        user.provenance["models"],
        Origin::User {
            document: f.host.config_directory.join("settings.toml"),
            base: f.host.config_directory.clone()
        }
    );
    assert!(matches!(
        user.provenance["runtime_root"],
        Origin::User { .. }
    ));
    f.request.runtime_root = Some("../cli-state".into());
    f.request.models = Some(f.host.config_directory.join("models.toml"));
    let cli = f.resolve();
    assert_eq!(cli.runtime_root, f.root.path().join("cli-state"));
    assert!(matches!(cli.provenance["runtime_root"], Origin::Cli { .. }));
    f.request.runtime_root = Some(f.host.state_directory.clone());
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("trust authority")
    );
}

#[test]
fn inspection_reads_only_the_host_state_reference_without_activation_or_writes() {
    let f = Fixture::new();
    f.trust(TrustAction::Revoke);
    f.user(json!({"runtime_root":"private","model":false,"context":false}));
    f.project(json!({"trust":true}));
    std::fs::remove_file(f.host.config_directory.join("models.toml")).unwrap();
    let locations = resolve_inspection_locations(&f.request, &f.host).unwrap();
    assert_eq!(
        locations.runtime_root,
        f.host.config_directory.join("private")
    );
    assert!(!locations.runtime_root.exists());
    assert!(resolve(&f.request, &f.host).is_err());
}

#[cfg(unix)]
#[test]
fn dangling_optional_files_and_project_redirected_trust_are_errors() {
    let f = Fixture::new();
    std::os::unix::fs::symlink("missing.toml", f.host.launch_directory.join("rustx.toml")).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("cannot read")
    );
    f.trust(TrustAction::Revoke);
    std::fs::remove_dir(f.host.state_directory.join("trust")).unwrap();
    std::os::unix::fs::symlink(
        &f.host.launch_directory,
        f.host.state_directory.join("trust"),
    )
    .unwrap();
    assert!(
        change_trust(&f.request, &f.host, TrustAction::Grant)
            .unwrap_err()
            .contains("outside")
    );
}

#[cfg(unix)]
#[test]
fn workspace_identity_has_exact_unix_byte_sha256_format() {
    use std::os::unix::ffi::OsStrExt;
    // Fixed canonical-path byte inputs at the pure hashing boundary. Golden
    // digests were independently calculated with sha256sum, not the helper.
    // Non-UTF-8 vectors also run on macOS without requiring its filesystem to
    // create filenames that it may reject.
    let vectors: &[(&[u8], &str)] = &[
        (
            b"/",
            "8a5edab282632443219e051e4ade2d1d5bbc671c781051bf1437897cbdfea0f1",
        ),
        (
            b"/rustx/workspace",
            "abe7db53f659b7fae2dfcc44469d33ab6b996505a4a32f09adbaef6d3525c1bd",
        ),
        (
            b"/rustx/project-\xff",
            "9617e4cce0d8db9dfec2c04014c1fc341a8479ed6812ecc04bc1cc749411cee8",
        ),
        (
            b"/rustx/project-\xfe",
            "59894cb99c81922900c2c3c7d55ee8fbb0c8e4ddff59a475915a1c597e4a8284",
        ),
    ];
    for (bytes, expected) in vectors {
        let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
        assert_eq!(workspace_identity(path), *expected);
    }
    // Exercise the real resolution/canonicalization boundary too. Unix root
    // has known canonical bytes on both supported platforms; no tempdir,
    // configuration, trust, or state writes participate in this assertion.
    let host = HostEnvironment::from_paths("/".into(), "/unused-host".into(), None, None).unwrap();
    let request = LaunchRequest {
        workspace: Some("/".into()),
        ..Default::default()
    };
    let (locations, identity) = resolve_locations(&request, &host).unwrap();
    assert_eq!(locations.workspace, Path::new("/"));
    assert_eq!(identity, vectors[0].1);
}

// Linux filesystems accept arbitrary non-NUL bytes. macOS filesystem APIs may
// reject invalid UTF-8; its canonical alias/worktree contract is tested above.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_workspace_identity_is_not_lossy() {
    use std::os::unix::ffi::OsStringExt;
    let mut f = Fixture::new();
    let first = f
        .root
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'a', 0xff]));
    let second = f
        .root
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'a', 0xfe]));
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    f.request.workspace = Some(first);
    let (_, first_identity) = resolve_locations(&f.request, &f.host).unwrap();
    f.request.workspace = Some(second);
    let (_, second_identity) = resolve_locations(&f.request, &f.host).unwrap();
    assert_ne!(first_identity, second_identity);
}

#[test]
fn duplicate_role_definitions_and_invalid_lower_layer_cannot_be_hidden() {
    let f = Fixture::new();
    std::fs::write(
        f.host.launch_directory.join("rustx.toml"),
        r#"[subagents]
main = ["role", "role"]"#,
    )
    .unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"model":{"model":"host/one"},"context":{"unknown":true}}));
    f.project(json!({"context":{"reserve_tokens":7}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
    f.user(json!({"model":{"model":"host/one"},"subagents":{"main":["role","role"]}}));
    f.project(json!({"subagents":{}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"model":{"model":"host/one"},"schema_version":7}));
    f.project(json!({"schema_version":8}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("schema_version 7")
    );
    f.user(json!({"model":{"model":"host/one"}}));
    f.project(
        json!({"subagents":{"definitions":{"role":{"description":"obsolete inline payload"}}}}),
    );
    assert!(resolve(&f.request, &f.host).is_err());
}

#[test]
fn project_trust_never_grants_tool_approval_authority() {
    let mut f = Fixture::new();
    let user = json!({"model":{"model":"host/one"},"approval_mode":"full_access",
        "native_tools":{"bash":{"approval":"always"}},
        "mcp_servers":{"server":{"command":"fixture"}},
        "mcp_tool_policies":{"server":{"approval":"always"}}});
    f.user(user);
    let host = f.resolve();
    assert_eq!(
        host.config.approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    let serialized = serde_json::to_value(host.config()).unwrap();
    assert_eq!(serialized["nativeTools"]["bash"]["approval"], "always");
    assert_eq!(
        serialized["mcpToolPolicies"]["server"]["approval"],
        "always"
    );
    for document in [
        json!({"approval_mode":"full_access"}),
        json!({"native_tools":{"bash":{"approval":"never"}}}),
        json!({"mcp_tool_policies":{"server":{"approval":"never"}}}),
        json!({"native_tools":{}}),
        json!({"native_tools":{"bash":{"execution":"background_only"}}}),
    ] {
        f.project(document);
        // Explicit CLI selection cannot mask a forbidden project declaration.
        f.request.model = Some("host/two".into());
        for explicit in [false, true] {
            f.request.config = explicit.then(|| "rustx.toml".into());
            let error = resolve(&f.request, &f.host).unwrap_err();
            assert!(
                error.contains("forbidden") && error.contains("host-owned authority"),
                "{error}"
            );
        }
    }
}

#[tokio::test]
async fn project_resource_authority_rejects_every_declared_escape_but_preserves_host_paths() {
    let mut f = Fixture::new();
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    let resource = other.join("resource");
    std::fs::write(&resource, "untrusted B bytes").unwrap();
    let role = |path: serde_json::Value| {
        f.role(
            false,
            "x",
            json!({"description":"x","agentsMd":{"files":[path]}}),
            "role",
        );
        json!({"subagents":{}})
    };
    for document in [
        role(json!(&resource)),
        json!({"skills":[&other]}),
        role(json!(&resource)),
        json!({"mcp_servers":{"x":{"command":&resource}}}),
        json!({"mcp_servers":{"x":{"command":"fixture","cwd":&other}}}),
    ] {
        f.project(document);
        let error = resolve(&f.request, &f.host).unwrap_err();
        assert!(error.contains("outside trusted workspace"), "{error}");
    }
    // An external --config is inert input, not a grant for its neighboring files.
    std::fs::write(
        other.join("config.toml"),
        toml::to_string_pretty(&role(json!("../B/resource"))).unwrap(),
    )
    .unwrap();
    f.request.config = Some(other.join("config.toml"));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    f.request.config = None;
    f.project(json!({}));
    std::fs::remove_file(f.host.launch_directory.join(".agents/agents/x.toml")).unwrap();
    f.role(true, "x", json!({"description":"host"}), "user-owned bytes");
    f.user(json!({"model":{"model":"host/one"},"subagents":{}}));
    let host = f.resolve();
    assert_eq!(host.role_sources.values().next().unwrap().layer, "user");
    let skill = other.join("outside");
    std::fs::create_dir(&skill).unwrap();
    f.request.skill_paths = vec![skill];
    std::fs::write(
        f.request.skill_paths[0].join("SKILL.md"),
        "---\nname: outside\ndescription: Host resource\n---\nHost instructions\n",
    )
    .unwrap();
    assert!(matches!(
        f.resolve().provenance["skills"],
        Origin::Cli { .. }
    ));
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let resources = product.runtime().runtime_resources();
    assert_eq!(
        resources
            .subagents()
            .get(&crate::runtime::subagent::SubagentName::parse("x").unwrap())
            .unwrap()
            .instructions(),
        "user-owned bytes"
    );
    assert!(resources.skill_catalog().unwrap().contains("outside"));
    product.runtime().shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn project_symlink_escape_is_rechecked_before_composition_and_reload() {
    let f = Fixture::new();
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("instructions.md"), "B MUST NEVER BE ADMITTED").unwrap();
    let source = f.role(false, "x", json!({"description":"x"}), "trusted A");
    f.project(json!({"subagents":{}}));
    let launch = f.resolve();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(other.join("instructions.md"), &source).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    assert!(
        LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    std::fs::remove_file(&source).unwrap();
    f.role(false, "x", json!({"description":"x"}), "trusted A");
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let before = product.runtime().runtime_resources();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(other.join("instructions.md"), &source).unwrap();
    assert!(
        product
            .runtime()
            .reload_resources()
            .await
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    let after = product.runtime().runtime_resources();
    assert_eq!(before.revision(), after.revision());
    assert_eq!(before.capability_revision(), after.capability_revision());
    assert!(std::sync::Arc::ptr_eq(&before, &after));
    assert_eq!(
        after
            .subagents()
            .get(&crate::runtime::subagent::SubagentName::parse("x").unwrap())
            .unwrap()
            .instructions(),
        "trusted A"
    );
    f.project(json!({"subagents":{}}));
    assert!(
        product
            .runtime()
            .reload_resources()
            .await
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    assert_eq!(
        product.runtime().runtime_resources().revision(),
        before.revision()
    );
    // Removing the canonical resource permits a complete new candidate.
    std::fs::remove_file(&source).unwrap();
    f.project(json!({"subagents":{}}));
    product.runtime().reload_resources().await.unwrap();
    product.runtime().shutdown().await.unwrap();
}

#[cfg(unix)]
#[test]
fn project_directory_symlinks_cannot_authorize_builtin_or_declared_resources() {
    let f = Fixture::new();
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("file"), "B").unwrap();
    std::os::unix::fs::symlink(&other, f.host.launch_directory.join("link")).unwrap();
    for document in [
        json!({"skills":["link"]}),
        json!({"mcp_servers":{"x":{"command":"link/file"}}}),
        json!({"mcp_servers":{"x":{"command":"fixture","cwd":"link"}}}),
    ] {
        f.project(document);
        assert!(
            resolve(&f.request, &f.host)
                .unwrap_err()
                .contains("outside trusted workspace")
        );
    }
    f.project(json!({}));
    std::os::unix::fs::symlink(&other, f.host.launch_directory.join(".agents")).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    std::os::unix::fs::symlink(
        other.join("file"),
        f.host.launch_directory.join("AGENTS.md"),
    )
    .unwrap();
    assert!(
        crate::runtime::load_project_context_files(&f.host.launch_directory)
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    std::fs::remove_file(f.host.launch_directory.join(".agents")).unwrap();
    std::fs::remove_file(f.host.launch_directory.join("AGENTS.md")).unwrap();
    let launch = f.resolve();
    std::fs::rename(&f.host.launch_directory, f.root.path().join("retired-A")).unwrap();
    std::os::unix::fs::symlink(&other, &f.host.launch_directory).unwrap();
    assert!(
        launch
            .validate_resource_authority()
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_workflow_symlink_rejects_reload_without_reading_external_yaml() {
    let f = Fixture::new();
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let before = product.runtime().runtime_resources();
    let other = f.root.path().join("B.yaml");
    std::fs::write(&other, "THIS IS NOT YAML: [").unwrap();
    let directory = f.host.launch_directory.join(".agents/workflows");
    std::fs::create_dir_all(&directory).unwrap();
    std::os::unix::fs::symlink(&other, directory.join("escape.yaml")).unwrap();
    f.project(json!({"workflows":{}}));
    let error = product
        .runtime()
        .reload_resources()
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("outside trusted workspace"),
        "authority rejection must precede YAML parsing: {error}"
    );
    assert!(std::sync::Arc::ptr_eq(
        &before,
        &product.runtime().runtime_resources()
    ));
    product.runtime().shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn frozen_mcp_binding_rechecks_project_authority_on_every_connect() {
    use crate::tools::mcp::{McpInvalidationState, McpServerBinding, McpServerRuntime};
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let program = f.host.launch_directory.join("server");
    std::fs::write(&program, "initial project resource").unwrap();
    f.project(json!({"mcp_servers":{"x":{"enabled":true,"command":"./server"}}}));
    let launch = f.resolve();
    let bindings = super::composition::mcp_bindings_with_authority(
        launch.config(),
        &launch.workspace,
        launch.provenance(),
        &launch.credentials,
    )
    .unwrap();
    let id = crate::runtime::identity::McpServerId::new("x");
    let binding = &bindings[&id];
    assert_eq!(binding.resource_workspace.as_ref(), Some(&launch.workspace));
    // The same binding is serialized into admitted child inputs and retained
    // by reconnect authority; no rediscovery is needed to enforce the root.
    let frozen: McpServerBinding =
        serde_json::from_slice(&serde_json::to_vec(binding).unwrap()).unwrap();
    assert_eq!(&frozen, binding);
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    let sentinel = other.join("started");
    std::fs::write(
        other.join("server"),
        format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
    )
    .unwrap();
    std::fs::set_permissions(other.join("server"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_file(&program).unwrap();
    std::os::unix::fs::symlink(other.join("server"), &program).unwrap();
    let workspace = crate::tools::workspace::Workspace::new(&launch.workspace).unwrap();
    for _ in 0..2 {
        let error = McpServerRuntime::connect(
            &id,
            &frozen,
            &workspace,
            std::sync::Arc::new(McpInvalidationState::default()),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("outside trusted workspace"),
            "{error}"
        );
        assert!(!sentinel.exists());
    }
    f.project(json!({"mcp_servers":{"x":{"command":"./server","resourceWorkspace":other}}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
}

#[test]
fn cfg270_only_toml_names_are_discovered_and_old_documents_are_inert() {
    let f = Fixture::new();
    // Even malformed/host-authority-bearing obsolete files are inert.
    for path in [
        f.host.config_directory.join("settings.jsonc"),
        f.host.config_directory.join("models.jsonc"),
        f.host.launch_directory.join("rustx.jsonc"),
    ] {
        std::fs::write(path, b"{ obsolete malformed configuration").unwrap();
    }
    let launch = f.resolve();
    assert_eq!(launch.config.initial_model().model.to_string(), "host/one");
    std::fs::remove_file(f.host.config_directory.join("settings.toml")).unwrap();
    let missing_selection = analyze(&f.request, &f.host).unwrap_err();
    assert!(missing_selection.incomplete);
    assert!(
        missing_selection
            .diagnostic
            .correction
            .contains("settings.toml")
    );
    std::fs::remove_file(f.host.config_directory.join("models.toml")).unwrap();
    let missing_catalog = analyze(&f.request, &f.host).unwrap_err();
    assert!(missing_catalog.incomplete);
    assert!(
        missing_catalog
            .diagnostic
            .file
            .unwrap()
            .ends_with("models.toml")
    );
    let nested = f.host.launch_directory.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("rustx.jsonc"), b"{}").unwrap();
    // An obsolete ancestor marker cannot claim a workspace either.
    let child = nested.join("child");
    std::fs::create_dir(&child).unwrap();
    let mut host = f.host.clone();
    host.launch_directory = child.clone();
    assert_eq!(
        resolve_locations(&LaunchRequest::default(), &host)
            .unwrap()
            .0
            .workspace,
        child
    );
}

#[test]
fn cfg270_every_checked_in_toml_example_uses_its_production_authoring_owner() {
    fn visit(directory: &Path) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(&path);
            } else if path.extension().is_some_and(|ext| ext == "toml") {
                let bytes = std::fs::read(&path).unwrap();
                match path.file_name().unwrap().to_str().unwrap() {
                    "models.toml" => {
                        ModelCatalog::from_toml_slice(&bytes)
                            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    }
                    "settings.toml" | "rustx.toml" => {
                        super::launch::parse_layer(
                            &path,
                            &bytes,
                            path.file_name().unwrap() == "rustx.toml",
                        )
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    }
                    _ if path.parent().unwrap().file_name().unwrap() == "agents" => {
                        crate::local_runtime::agent_resources::parse(
                            std::str::from_utf8(&bytes).unwrap(),
                        )
                        .unwrap();
                    }
                    other => panic!("unowned TOML example {other}"),
                }
            }
        }
    }
    visit(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cfg271_all_catalogs_publish_together_and_failed_candidates_publish_nothing() {
    let f = Fixture::new();
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let before = product.runtime().runtime_resources();
    let workspace = &f.host.launch_directory;
    let agent = f.role(
        false,
        "added",
        json!({"description":"Added Agent"}),
        "Frozen instructions",
    );
    let skills = workspace.join(".agents/skills/added");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(
        skills.join("SKILL.md"),
        "---\nname: added\ndescription: Added Skill\n---\nFrozen Skill\n",
    )
    .unwrap();
    let package = workspace.join(".agents/tools/added");
    std::fs::create_dir_all(&package).unwrap();
    let workflow = install_workflow(&f, "invalid: [");
    assert!(product.runtime().reload_resources().await.is_err());
    assert!(std::sync::Arc::ptr_eq(
        &before,
        &product.runtime().runtime_resources()
    ));
    // Use a shipped native-only program; no Agent/source materialization demand.
    std::fs::write(&workflow, "description: Return a literal\nblock:\n  input: {type: object, properties: {}, additionalProperties: false}\n  output: {type: object, properties: {}, additionalProperties: false}\n  entry: done\n  nodes:\n    done:\n      type: return\n      output: {type: literal, value: {}}\n").unwrap();
    let gate = super::agent_resources::test_support::arm(workspace);
    let runtime = product.runtime().clone();
    let mut reload = tokio::spawn(async move { runtime.reload_resources().await });
    tokio::select! { () = gate.entered() => {}, result = &mut reload => panic!("candidate failed before publication gate: {result:?}") }
    assert!(std::sync::Arc::ptr_eq(
        &before,
        &product.runtime().runtime_resources()
    ));
    gate.release();
    reload.await.unwrap().unwrap();
    drop(gate);
    let added = product.runtime().runtime_resources();
    assert_eq!(before.subagents().len(), 0);
    assert!(before.managed_python_catalog().packages().is_empty());
    assert_eq!(added.subagents().len(), 1);
    assert_eq!(added.managed_python_catalog().packages().len(), 1);
    assert_eq!(added.capability().skills().packages().len(), 1);
    assert_eq!(added.workflows().definitions().len(), 1);
    for path in [&agent, &skills.join("SKILL.md"), &workflow] {
        let bytes = std::fs::read(path).unwrap();
        std::fs::write(path, "malformed: [").unwrap();
        assert!(product.runtime().reload_resources().await.is_err());
        assert!(std::sync::Arc::ptr_eq(
            &added,
            &product.runtime().runtime_resources()
        ));
        std::fs::write(path, bytes).unwrap();
    }
    std::fs::remove_file(agent).unwrap();
    std::fs::remove_file(workflow).unwrap();
    std::fs::remove_dir_all(skills).unwrap();
    std::fs::remove_dir(package).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &added,
        &product.runtime().runtime_resources()
    ));
    product.runtime().reload_resources().await.unwrap();
    let removed = product.runtime().runtime_resources();
    assert!(removed.subagents().is_empty());
    assert!(removed.workflows().definitions().is_empty());
    assert!(removed.capability().skills().packages().is_empty());
    assert!(removed.managed_python_catalog().packages().is_empty());
    assert_eq!(added.subagents().len(), 1);
    assert_eq!(added.workflows().definitions().len(), 1);
    assert_eq!(added.capability().skills().packages().len(), 1);
    assert_eq!(added.managed_python_catalog().packages().len(), 1);
    product.runtime().shutdown().await.unwrap();
}

#[test]
fn cfg271_removed_existence_registries_are_unknown_fields_in_each_layer() {
    let f = Fixture::new();
    for field in [
        json!({"python_sources":{}}),
        json!({"subagents":{"definitions":[]}}),
        json!({"workflows":{"definitions":[]}}),
    ] {
        f.project(field.clone());
        assert!(
            analyze(&f.request, &f.host)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        f.project(json!({}));
        let mut user = field;
        user["model"] = json!({"model":"host/one"});
        f.user(user);
        assert!(
            analyze(&f.request, &f.host)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        f.user(json!({"model":{"model":"host/one"}}));
    }
}
