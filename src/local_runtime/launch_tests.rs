//! CFG-01 filesystem and real native composition regressions (Linux and macOS CI).
#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)] // linear fixture scenarios
use super::configuration::*;
use super::launch::*;
use super::{LocalRuntimeDependencies, LocalSessionClient};
use crate::capabilities::{CapabilitySourceState, ToolSourceId};
use serde_json::json;
use std::path::Path;

struct Fixture {
    root: tempfile::TempDir,
    host: HostEnvironment,
    credentials: crate::credentials::CredentialSnapshot,
    request: LaunchRequest,
}

#[test]
fn cfg279_structured_parameters_check_show_and_project_overlay_are_side_effect_free() {
    let f = Fixture::new();
    f.user(json!({"agent":{"model":{"model":"host/one", "request_params":{"provider":{"order":["a","b"],"allow_fallbacks":true}}}}}));
    let file = f.host.launch_directory.join("rustx.toml");
    for (parameters, expected_path) in [
        (
            "provider = {order = ['c']}\nsecret = 'SECRET_PROVIDER_VALUE'",
            None,
        ),
        (
            "items = [{when = true}, {when = 1979-05-27}]\nsecret = 'SECRET_PROVIDER_VALUE'",
            Some("agent.model.request_params.items[1].when"),
        ),
    ] {
        std::fs::write(
            &file,
            format!(
                "[agent.model]\nmodel = 'host/one'\n[agent.model.request_params]\n{parameters}"
            ),
        )
        .unwrap();
        for operation in ["config_check", "config_show"] {
            let ((report, launch), effects) = super::static_effects::measure(|| {
                super::diagnostics::inspect(operation, &f.request, &f.host)
            });
            assert_eq!(effects, [0; 12]);
            assert!(!report.render(true).contains("SECRET_PROVIDER_VALUE"));
            if let Some(path) = expected_path {
                assert_eq!(report.validity, super::diagnostics::Validity::Invalid);
                assert_eq!(report.diagnostics[0].path, path);
            } else {
                assert_eq!(report.validity, super::diagnostics::Validity::Valid);
                let launch = launch.unwrap();
                assert_eq!(
                    launch.config.initial_model().request_params["provider"],
                    json!({"order":["c"]})
                );
            }
        }
    }
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
    let config = json!({"subagents": {}, "agent": {"workflows": []}});
    let original = template_source("typed_agent");
    let scenarios = [
        (original.clone(), config.clone(), Validity::Valid, None),
        (original.clone(), json!({}), Validity::Valid, None),
        (
            original.clone(),
            json!({"subagents": {}}),
            Validity::Valid,
            None,
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
            assert_eq!(effects, [0; 12]);
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
                assert!(matches!(
                    projection.admission,
                    crate::runtime::capability_inspection::WorkflowInspection::Enabled
                ));
                assert_eq!(projection.program.is_some(), explain);
            }
            assert!(!report.render(true).contains("SECRET_ROLE_PROMPT"));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
            assert!(!f.resolve_locations_only().runtime_root.exists());
        }
    }
    // Explicit inspection reports the malformed resource even when Root does not select it.
    f.project(json!({}));
    install_workflow(&f, "invalid: [");
    let id = crate::runtime::workflow::WorkflowId::parse("example").unwrap();
    assert_eq!(
        super::workflow_inspection::inspect(&id, true, &f.request, &f.host).validity,
        Validity::Invalid
    );
    f.project(config);

    for explain in [false, true] {
        let (report, effects) = super::static_effects::measure(|| {
            super::workflow_inspection::inspect(&id, explain, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 12]);
        assert_eq!(report.validity, Validity::Invalid);
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
    f.project(json!({"subagents": {}}));
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
            assert_eq!(effects, [0; 12]);
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
    use crate::capabilities::selection::{SourceResolutionFailure, ToolSelectionError};
    use crate::runtime::workflow::WorkflowDependencyFailure;
    let f = Fixture::new();
    let text = r"description: Inspect a declared external capability.
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
    f.mcp(false, json!({"external":{"command":"must-never-spawn"}}));
    f.project(json!({}));
    for explain in [false, true] {
        let (report, effects) = super::static_effects::measure(|| {
            super::workflow_inspection::inspect(
                &crate::runtime::workflow::WorkflowId::parse("example").unwrap(),
                explain,
                &f.request,
                &f.host,
            )
        });
        assert_eq!(effects, [0; 12]);
        assert_eq!(report.validity, super::diagnostics::Validity::Incomplete);
        let crate::runtime::capability_inspection::WorkflowInspection::Disabled(facts) =
            &report.workflow.as_ref().unwrap().admission
        else {
            panic!("dependency disables Workflow")
        };
        assert_eq!(facts[0].path, "block.nodes.inspect.selector");
        let WorkflowDependencyFailure::Tool(ToolSelectionError::SourceUnavailable {
            reason, ..
        }) = &facts[0].reason
        else {
            panic!("source reason")
        };
        assert!(matches!(reason, SourceResolutionFailure::Unprepared));
        assert!(!report.render(true).contains("SECRET_LITERAL"));
        assert_eq!(report.exit_code(), 3);
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
    f.project(json!({"subagents": {}}));
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
    f.project(json!({}));
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
fn cfg3_offline_agent_shadowing_has_zero_external_effects() {
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
    f.project(json!({"subagents": {}, "agent": {"agents": []}}));
    for operation in ["config_check", "config_show"] {
        let ((report, launch), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect(operation, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 12]);
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
    assert_eq!(effects, [0; 12]);
    assert_eq!(report.validity, super::diagnostics::Validity::Valid);
    assert!(
        report
            .capabilities
            .as_ref()
            .unwrap()
            .resource_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.file.as_ref() == Some(&project))
    );
    assert!(!report.render(true).contains("SENTINEL"));
    f.project(json!({"agent":{"agents":["reviewer"]}}));
    assert!(analyze(&f.request, &f.host).is_err());
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
    assert_eq!(launch.agent_root, physical_config.join(".agents/agents"));
    for operation in ["config_check", "config_show"] {
        let ((report, prospective), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect(operation, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 12]);
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
    // Retargeting the display alias cannot change the pinned source used by reconciliation.
    let replacement = f.root.path().join("replacement");
    std::fs::create_dir(&replacement).unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&replacement, &alias).unwrap();
    let (catalog, _) =
        super::agent_resources::load_authorized(Some(&launch.workspace), &launch.agent_root)
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
    let (catalog, _) =
        super::agent_resources::load_authorized(Some(&launch.workspace), &launch.agent_root)
            .unwrap();
    assert!(catalog.definitions().next().is_none());
    assert!(
        catalog
            .discovery_diagnostics
            .iter()
            .any(|error| error.to_string().contains("outside workspace boundary"))
    );
}

#[test]
fn cfg3_offline_python_discovery_parses_inert_bytes_without_preparing_packages() {
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
    crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| count.set(0));
    let ((report, launch), effects) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(effects, [0; 12]);
    crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| assert_eq!(count.get(), 1));
    assert!(
        !report
            .render(true)
            .contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
    );
    let launch = launch.unwrap();
    assert_eq!(launch.managed_python.packages().len(), 1);
    assert!(!launch.runtime_root.exists());
    assert!(matches!(
        report.capabilities.as_ref().unwrap().sources[&ToolSourceId::ManagedPython("foo".into())],
        crate::runtime::capability_inspection::SourceInspection::Unprepared
    ));
}

#[test]
fn cfg235_provider_readiness_is_unresolved_without_credential_lookup() {
    let f = Fixture::new();
    let path = f.host.config_directory.join("rustx.toml");
    let mut model: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&path).unwrap()).unwrap();
    for credential in ["$CFG235_UNSET", "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"] {
        model["providers"]["host"]["api_key"] = json!(credential);
        std::fs::write(&path, toml::to_string_pretty(&model).unwrap()).unwrap();
        for mcp in [false, true] {
            f.project(json!({}));
            f.mcp(
                false,
                if mcp {
                    json!({"online":{"url":"http://127.0.0.1:9/mcp"}})
                } else {
                    json!({})
                },
            );
            for operation in ["config_check", "config_show"] {
                let ((report, _), counts) = super::static_effects::measure(|| {
                    super::diagnostics::inspect(operation, &f.request, &f.host)
                });
                assert_eq!(counts, [0; 12]);
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
                    assert!(matches!(
                        report.capabilities.as_ref().unwrap().sources[&ToolSourceId::Mcp(
                            crate::runtime::identity::McpServerId::new("online")
                        )],
                        crate::runtime::capability_inspection::SourceInspection::Unprepared
                    ));
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
            assert_eq!(counts, [0; 12]);
            if let Some(projection) = &report.capabilities {
                for name in projection.sources.keys() {
                    let diagnostic = report
                        .diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.path == format!("tools.sources.{name}"))
                        .unwrap();
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
    f.mcp(false, json!({"untrusted":{"command":"must-never-spawn"}}));
    f.project(json!({}));

    let ((report, launch), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_show", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 12]);
    assert_eq!(report.exit_code(), 3);
    let admitted = launch.unwrap().admit(|| f.credentials.clone()).unwrap();

    assert_eq!(
        admitted.config.mcp_servers.len(),
        1,
        "definitions are retained without activation"
    );
}

#[test]
fn cfg235_prospective_values_and_origins_equal_runtime_resolution() {
    let f = Fixture::new();
    f.project(json!({"context":{"reserve_tokens":8192},"environment":{"PRIVATE":"RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"}}));
    let prospective = analyze(&f.request, &f.host).unwrap();
    let runtime = f.resolve();
    assert_eq!(prospective.config(), runtime.config());
    assert_eq!(prospective.provenance(), runtime.provenance());
    assert_eq!(prospective.workspace, runtime.workspace);
    assert_eq!(prospective.runtime_root, runtime.runtime_root);
    assert!(
        !prospective
            .inspection
            .main
            .as_ref()
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "read")
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
    f.mcp(false, json!({"broken":{"command":"fixture"}}));
    f.project(json!({

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
    f.project(json!({"environment": {"PRIVATE":"RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"}, "agent": {"tools": {"builtin": (0..12000).map(|index| format!("unresolved_tool_identity_{index}")).collect::<Vec<_>>()}}}));
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
    for (directory, expected_workflows) in [
        ("minimal", 0),
        ("workflow-basic", 1),
        ("", 2),
        ("workflow-templates", 4),
    ] {
        let f = Fixture::new();
        let mut host = f.host.clone();
        host.launch_directory = base.join(directory);
        let request = LaunchRequest {
            workspace: Some(host.launch_directory.clone()),
            config: Some(host.launch_directory.join("rustx.toml")),
            ..Default::default()
        };
        let prospective =
            analyze(&request, &host).unwrap_or_else(|error| panic!("example {directory}: {error}"));
        assert_eq!(prospective.workflows.entries().len(), expected_workflows);
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
    f.mcp(
        false,
        json!({
            "enabled":{"url":fixture.endpoint},
            "disabled":{"command":"must-never-execute"},
            "unconfigured":{"command":"must-never-execute"}
        }),
    );
    f.project(json!({"agent":{"tools":{"sources":{"enabled":"all"}}}}));
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
    fixture.shutdown().await;
}

#[test]
fn cfg235_diagnostics_keep_source_field_classification_and_correction() {
    let f = Fixture::new();
    for (configuration, field) in [
        (json!({"unknown_field":true}), "$"),
        (json!({"approval_mode":"invalid"}), "$"),
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
        f.mcp(true, json!({"owned":{
            "command":std::env::current_exe().unwrap(),
            "args":fixture_spawn_args("local_runtime::launch_tests::cfg235_probe_stdio_timeout_and_cancel_reap_owned_process"),
            "env":{FIXTURE_MODE_ENV:"1"}
        }}));
        f.user(
            json!({ "agent": {"model": {"model":"host/one"}, "tools":{"sources":{"owned":"all"}}}}),
        );
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
    f.mcp(false, json!({"owned":{"url":fixture.endpoint}}));
    f.project(json!({"agent":{"tools":{"sources":{"owned":"all"}}}}));
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
    let model_path = f.host.config_directory.join("rustx.toml");
    let mut model: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&model_path).unwrap()).unwrap();
    model["providers"]["host"]["api_key"] = json!("$CFG235_UNSET_CREDENTIAL");
    std::fs::write(&model_path, toml::to_string_pretty(&model).unwrap()).unwrap();
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 12]);
    assert_eq!(
        report.exit_code(),
        3,
        "execution credential and connectivity facts remain unresolved"
    );
    f.request.config = Some(f.host.launch_directory.join("explicit-missing.toml"));
    let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
    assert_eq!(
        report.exit_code(),
        3,
        "the absent bound User source leaves model selection incomplete"
    );
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
    f.project(json!({"agent": {"workflows": ["broken"]}}));
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 12]);
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
    f.project(json!({"agent": {"workflows": ["broken"]}}));
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 12]);
    assert_eq!(
        report.exit_code(),
        2,
        "native compiler rejects the missing entry node"
    );
    assert_eq!(report.diagnostics[0].path, "block.entry");
    f.project(json!({}));
    std::fs::write(&model_path, "").unwrap();
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
    f.mcp(true, json!({"secret":{"url":fixture.endpoint,"sensitive_headers":{"Authorization":"$CFG235_TOKEN"}}}));
    f.user(
        json!({ "agent": {"model": {"model":"host/one"}, "tools":{"sources":{"secret":"all"}}}}),
    );
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
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home")).unwrap();
        std::fs::create_dir_all(&host.config_directory).unwrap();
        let fixture = Self {
            root,
            host,
            credentials: crate::credentials::CredentialSnapshot::default(),
            request: LaunchRequest::default(),
        };
        fixture.user(json!({"agent": {"model": {"model":"host/one"}}}));
        fixture
    }
    fn user(&self, mut value: serde_json::Value) {
        // Test input owns the requested policy; this fixture explicitly supplies
        // its independent Provider and Model definitions in the same document.
        value["providers"] =
            json!({"host": {"base_url":"http://127.0.0.1:9/v1", "api_key":"fixture"}});
        value["models"] = json!(
            ["one", "two"]
                .into_iter()
                .map(|name| (
                    format!("host/{name}"),
                    json!({"provider":"host", "id":name,
                "protocol":"openai_chat_completions", "context_window":128_000,
                "max_output_tokens":4096, "capabilities":{"input_modalities":["text"],
                "output_modalities":["text"],"tool_calls":true,"reasoning":false},
                "compat":{"chat_reasoning_replay":"omit"}})
                ))
                .collect::<std::collections::BTreeMap<_, _>>()
        );
        std::fs::write(
            self.host.config_directory.join("rustx.toml"),
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
            self.host.home_directory.join("rustx/.agents/agents")
        } else {
            self.host.launch_directory.join(".agents/agents")
        };
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{name}.toml"));
        let mut metadata = metadata;
        metadata["instructions"] = body.into();
        std::fs::write(&path, toml::to_string_pretty(&metadata).unwrap()).unwrap();
        path
    }
    fn mcp(&self, user: bool, definitions: serde_json::Value) {
        let root = if user {
            self.host.home_directory.join("rustx/.agents")
        } else {
            self.host.launch_directory.join(".agents")
        };
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("mcp.toml"),
            toml::to_string_pretty(&json!({"mcp_servers":definitions})).unwrap(),
        )
        .unwrap();
    }
    fn project(&self, value: serde_json::Value) {
        std::fs::write(
            self.host.launch_directory.join("rustx.toml"),
            toml::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    }
    fn resolve(&self) -> AdmittedSessionConfig {
        analyze(&self.request, &self.host)
            .unwrap()
            .admit(|| self.credentials.clone())
            .unwrap()
    }
    fn resolve_locations_only(&self) -> SessionLocations {
        resolve_locations(&self.request, &self.host).unwrap().0
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
    let f = Fixture::new();
    for (user, workspace) in [
        (
            json!({"url":"https://user.invalid/mcp","sensitive_headers":{"Authorization":"$USER_SECRET"}}),
            json!({"url":"https://workspace.invalid/mcp"}),
        ),
        (
            json!({"command":"user-server","sensitive_env":{"TOKEN":"$USER_SECRET"}}),
            json!({"command":"workspace-server"}),
        ),
    ] {
        f.mcp(true, json!({"service":user}));
        f.mcp(false, json!({"service":workspace}));
        let launch = f.resolve();
        let bindings =
            super::composition::captured_mcp_bindings(launch.config(), &launch.credentials)
                .unwrap();
        let binding = &bindings[&crate::runtime::identity::McpServerId::new("service")];
        assert!(binding.credentials.environment.is_empty());
        assert!(binding.credentials.headers.is_empty());
        assert!(matches!(
            launch.provenance["mcp_servers.service"],
            Origin::Workspace { .. }
        ));
    }
}

#[tokio::test]
async fn cfg233_provider_binding_uses_launch_snapshot_and_ignores_unused_missing_keys() {
    let mut f = Fixture::new();
    let path = f.host.config_directory.join("rustx.toml");
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
    let product = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(!format!("{launch:?}").contains("CFG233_PROVIDER_SENTINEL"));
    product.runtime().shutdown().await.unwrap();
    drop(product); // Release native catalog ownership before the next launch.
    assert!(
        LocalSessionClient::compose(&f.resolve(), &LocalRuntimeDependencies::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("CAPTURED_KEY")
    );
}

#[tokio::test]
async fn cfg3_selected_missing_credentials_and_connection_failures_reject_root_admission() {
    for source in ["credential", "connection"] {
        let f = Fixture::new();
        f.mcp(true, json!({
            "credential":{"command":"/does/not/exist","sensitive_env":{"TOKEN":"$REQUIRED_KEY"}},
            "connection":{"command":"/does/not/exist"},
            "unused":{"command":"must-never-spawn","sensitive_env":{"TOKEN":"$IGNORED_KEY"}}
        }));
        f.project(json!({"agent":{"tools":{"sources":{source:"all"}}}}));
        let error = LocalSessionClient::compose(&f.resolve(), &LocalRuntimeDependencies::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot be admitted"));
        assert!(!error.to_string().contains("IGNORED_KEY"));
    }
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
            LocalSessionClient::compose(&f.resolve(), &LocalRuntimeDependencies::default())
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
    let f = Fixture::new();
    let package = f.host.launch_directory.join(".agents/tools/discovered");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("server.py"),
        "raise Exception('must not import')",
    )
    .unwrap();
    // No requirements file: inert discovery cannot require package validity.
    f.mcp(true, json!({
        "missing":{"command":"/nonexistent/cfg233","sensitive_env":{"TOKEN":"$UNSET"}},
        "offline":{"url":"http://127.0.0.1:1/mcp","sensitive_headers":{"Authorization":"$UNSET"}}
    }));
    f.user(json!({ "agent": {"model": {"model":"host/one"}}}));
    let launch = f.resolve();
    let product = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
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

    assert_eq!(std::fs::read_dir(&package).unwrap().count(), 1);
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
    f.mcp(true, json!({"authenticated":{"url":format!("http://{endpoint}/mcp"),"sensitive_headers":{"Authorization":"$AUTH"}}}));
    f.user(json!({ "agent": {"model": {"model":"host/one"}, "tools": {"sources":{"authenticated":"all"}}}}));
    let launch = f.resolve();
    let error = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap_err();
    peer.await.unwrap();
    assert!(error.to_string().contains("cannot be admitted"));
    assert!(!format!("{error:?} {launch:?}").contains(SENTINEL));
    assert!(
        !serde_json::to_string(launch.config())
            .unwrap()
            .contains(SENTINEL)
    );
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
    f.mcp(true, json!({"service":{"command":"old-command","args":["old"]},"retained":{"command":"retained"}}));
    f.user(json!({"agent_id": "user", "context": {"reserve_tokens":2000,"keep_recent_tokens":6000}, "environment": {"USER_ENTRY":"one","REPLACED":"old"},  "subagents": {}, "agent": {"model": {"model":"host/one", "reasoning_profile":{"mode":"catalog_default"}}, "tools": {"builtin": ["read","bash"]}}}));
    f.mcp(false, json!({"service":{"command":"new-command"}}));
    f.project(
        json!({"context": {"reserve_tokens":3000}, "environment": {"REPLACED":"new"},  "subagents": {}, "agent": {"model": {"model":"host/two"}, "tools": {"builtin": []}}}),
    );
    let resolved = f.resolve();
    assert!(matches!(
        resolved.provenance["agent.model.reasoning_profile"],
        Origin::Workspace { .. }
    ));
    assert_eq!(resolved.config.agent_id.as_str(), "user");
    assert_eq!(resolved.config.context.reserve_tokens, 3000);
    assert_eq!(
        resolved.config.context.keep_recent_tokens,
        super::config::ContextPolicyDocument::default().keep_recent_tokens
    );
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
    assert_eq!(source.layer, "workspace");
    assert_eq!(
        source.overridden.as_ref().unwrap(),
        &f.host.config_directory.join(".agents/agents/role.toml")
    );
    assert!(matches!(
        resolved.provenance["context.keep_recent_tokens"],
        Origin::Workspace { .. }
    ));
    assert!(matches!(
        resolved.provenance["context.reserve_tokens"],
        Origin::Workspace { .. }
    ));
    f.request.model = Some("host/one".into());
    let cli = f.resolve();
    assert_eq!(cli.session_model().model.to_string(), "host/one");
    assert!(matches!(
        cli.provenance["agent.model.model"],
        Origin::Workspace { .. }
    ));
    f.mcp(false, json!({}));
    f.project(json!({"environment":{},"subagents":{}}));
    let empty = f.resolve();
    assert_eq!(empty.config.environment.len(), 2);
    assert_eq!(empty.config.environment["REPLACED"], "old");
    assert_eq!(empty.config.mcp_servers.len(), 2);
    assert!(empty.config.agent.agents.is_empty());
    assert_eq!(empty.config.agent.tools.builtin, ["read", "bash"]);
}

#[test]
fn optional_explicit_malformed_and_typed_rejections_are_distinct() {
    let mut f = Fixture::new();
    assert_eq!(f.resolve().config.agent_id.as_str(), "rustx");
    f.request.config = Some(f.host.launch_directory.join("missing.toml"));
    assert!(resolve(&f.request, &f.host).is_err());
    f.request.config = None;
    for text in [
        "invalid = [",
        "[skills]\nsources = ['global']",
        "[agent]\nno_builtin_tools = true",
        "[agent.tools]\nbuiltin = ['unknown']",
    ] {
        std::fs::write(f.host.launch_directory.join("rustx.toml"), text).unwrap();
        assert!(resolve(&f.request, &f.host).is_err(), "{text}");
    }
    f.project(
        json!({"agent":{"skills":[]}, "context":{"summary_output_cap":{"mode":"model_limit"}}}),
    );
    assert_eq!(f.resolve().config.context.summary_output_cap, None);
    std::fs::write(
        f.host.launch_directory.join("rustx.toml"),
        " ".repeat(1024 * 1024 + 1),
    )
    .unwrap();
    assert!(resolve(&f.request, &f.host).unwrap_err().contains("1 MiB"));
}

#[test]
fn explicit_user_config_rebinds_only_the_source_document() {
    let mut f = Fixture::new();
    let user_agent = f.role(true, "user", json!({"description":"User"}), "User body");
    let workspace_agent = f.role(
        false,
        "workspace",
        json!({"description":"Workspace"}),
        "Workspace body",
    );
    let before = f.resolve();
    let replacement = f.root.path().join("replacement.toml");
    std::fs::copy(f.host.config_directory.join("rustx.toml"), &replacement).unwrap();
    f.request.config = Some(replacement.clone());
    let after = f.resolve();
    assert_eq!(before.runtime_root, after.runtime_root);
    assert_eq!(before.agent_root, after.agent_root);
    assert_eq!(after.subagents.definitions().count(), 2);
    assert_eq!(
        after.role_sources[&crate::runtime::subagent::SubagentName::parse("user").unwrap()]
            .selected,
        user_agent
    );
    assert_eq!(
        after.role_sources[&crate::runtime::subagent::SubagentName::parse("workspace").unwrap()]
            .selected,
        workspace_agent
    );
    f.request.config = Some("relative.toml".into());
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("absolute")
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

    f.request.workspace = Some(original.workspace.clone());
    assert_eq!(f.resolve().identity, original.identity);
    f.request.workspace = Some(sub);

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
    assert_eq!(locations.runtime_root, original.runtime_root);

    assert_eq!(
        resolve(&f.request, &host).unwrap().workspace,
        std::fs::canonicalize(worktree).unwrap()
    );

    assert!(resolve(&f.request, &f.host).is_ok(), "revocation is scoped");
    let outside = host.launch_directory.join("untrusted.md");
    std::fs::write(&outside, "other worktree").unwrap();
    f.role(
        false,
        "other",
        json!({"description": "other", "agents_md": {"files":[outside]}}),
        "role",
    );
    f.project(json!({"agent":{"agents":["other"]}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside workspace boundary")
    );
}

#[tokio::test]
async fn minimal_native_composition_and_frozen_launch_ignore_later_config_edits() {
    let f = Fixture::new();
    let launch = f.resolve();
    f.user(json!({"agent_id": "edited", "agent": {"model": {"model":"host/two"}}}));
    f.project(json!({"agent": {"model": {"model":"missing/model"}}}));
    let product = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
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
    assert_eq!(launch.config.agent.skills, None);
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
    let product = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    product.runtime().shutdown().await.unwrap();
    drop(product);
    let catalog = launch.runtime_root.join("sessions/catalog.json");
    let before = std::fs::read(&catalog).unwrap();

    assert_eq!(before, std::fs::read(&catalog).unwrap());

    f.project(json!({"agent_id":""}));
    assert!(resolve(&f.request, &f.host).is_err());
    assert_eq!(before, std::fs::read(&catalog).unwrap());
    f.project(json!({"agent": {"skills": ["missing-skill"]}}));
    assert!(
        analyze(&f.request, &f.host).is_err(),
        "selected invalid resources reject prospective admission"
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
    f.user(json!({"agent": {"model": {"model":"one"}}}));
    assert!(
        resolve(&f.request, &f.host).is_err(),
        "unqualified reference is ambiguous"
    );
    f.user(json!({"agent": {"model": {"model":"host/one"}}}));
    let model_path = f.host.config_directory.join("rustx.toml");
    let mut catalog: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&model_path).unwrap()).unwrap();
    catalog["models"]["host/one"]
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
        HostEnvironment::from_paths(f.host.launch_directory.clone(), "relative".into()).is_err()
    );
}

#[cfg(unix)]
#[test]
fn dangling_workspace_source_is_a_read_error() {
    let f = Fixture::new();
    std::os::unix::fs::symlink("missing.toml", f.host.launch_directory.join("rustx.toml")).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("cannot read")
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
    let host = HostEnvironment::from_paths("/".into(), "/unused-host".into()).unwrap();
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



[agent]
agents = ["role", "role"]
"#,
    )
    .unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"context": {"unknown":true}, "agent": {"model": {"model":"host/one"}}}));
    f.project(json!({"context":{"reserve_tokens":7}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
    f.user(json!({"subagents": {}, "agent": {"model": {"model":"host/one"}, "agents": ["role","role"]}}));
    f.project(json!({"subagents":{}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"schema_version": 7, "agent": {"model": {"model":"host/one"}}}));
    f.project(json!({"schema_version":8}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("schema_version 7")
    );
    f.user(json!({"agent": {"model": {"model":"host/one"}}}));
    f.project(
        json!({"subagents": {"definitions": {"role":{"description":"obsolete inline payload"}}}}),
    );
    assert!(resolve(&f.request, &f.host).is_err());
}

#[test]
fn workspace_authors_global_tool_policies_without_granting_root_tools() {
    let f = Fixture::new();
    f.user(json!({"approval_mode":"full_access", "native_tools":{"bash":{"approval":"always", "execution":"background_only"}}, "agent":{"model":{"model":"host/one"}}}));
    f.project(json!({"approval_mode":"policy", "native_tools":{"bash":{"approval":"never"}}}));
    let resolved = f.resolve();
    assert_eq!(
        resolved.config.approval_mode,
        crate::runtime::ApprovalMode::Policy
    );
    assert!(resolved.config.agent.tools.builtin.is_empty());
    let value = serde_json::to_value(resolved.config()).unwrap();
    assert_eq!(value["nativeTools"]["bash"]["approval"], "never");
    assert_ne!(value["nativeTools"]["bash"]["execution"], "background_only");
    assert!(matches!(
        resolved.provenance["native_tools.bash"],
        Origin::Workspace { .. }
    ));
}

#[tokio::test]
async fn selected_workspace_guidance_cannot_escape_its_resource_root() {
    let f = Fixture::new();
    let outside = f.root.path().join("outside.md");
    std::fs::write(&outside, "Outside body").unwrap();
    f.project(json!({"agent":{"agents_md":{"files":[outside]}}}));
    assert!(resolve(&f.request, &f.host).is_err());
    f.project(json!({}));
    f.role(
        true,
        "reader",
        json!({"description":"User reader", "tools":{"builtin":["read"]}}),
        "User instructions",
    );
    let launch = f.resolve();
    assert_eq!(launch.role_sources.values().next().unwrap().layer, "user");
    let product = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert_eq!(
        product
            .runtime()
            .runtime_resources()
            .subagents()
            .definitions()
            .next()
            .unwrap()
            .instructions(),
        "User instructions"
    );
    product.runtime().shutdown().await.unwrap();
}

/// CFG3 binds exactly the captured User home and Workspace Skill roots;
/// Workspace reserves a complete same-name shadow before package validation.
#[tokio::test]
async fn cfg3_launch_freezes_two_skill_roots_with_whole_workspace_shadowing() {
    let f = Fixture::new();
    let user = f.host.home_directory.join("rustx/.agents/skills");
    let workspace = f.host.launch_directory.join(".agents/skills");
    for (root, name, description) in [
        (&user, "user-only", "User only"),
        (&user, "shared", "User shared"),
        (&workspace, "shared", "Workspace shared"),
    ] {
        let package = root.join(name);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nBODY\n"),
        )
        .unwrap();
    }
    f.project(json!({"agent":{"skills":"all"}}));
    let launch = f.resolve();
    let shared = launch
        .inspection
        .skills
        .iter()
        .find(|s| s.name == "shared")
        .unwrap();
    assert_eq!(shared.source, crate::skills::SkillSource::Workspace);
    assert_eq!(shared.shadowed[0].source, crate::skills::SkillSource::User);
    std::fs::write(
        workspace.join("shared/SKILL.md"),
        "malformed edit after capture",
    )
    .unwrap();
    let product = LocalSessionClient::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let resources = product.runtime().runtime_resources();
    let prompt = resources.skill_catalog().unwrap();
    assert!(prompt.contains("Workspace shared"));
    assert!(!prompt.contains("User shared"));
    assert!(prompt.contains(user.to_str().unwrap()));
    assert!(prompt.contains(workspace.to_str().unwrap()));
    assert_eq!(
        resources.root_profile().unwrap().skills,
        ["shared", "user-only"]
    );
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
    f.project(json!({"agent":{"agents_md":{"files":["link/file"]}}}));
    assert!(resolve(&f.request, &f.host).is_err());
    f.project(json!({}));
    std::os::unix::fs::symlink(&other, f.host.launch_directory.join(".agents")).unwrap();
    let candidate = analyze(&f.request, &f.host).unwrap();
    assert!(!candidate.inspection.resource_diagnostics.is_empty());
    std::os::unix::fs::symlink(
        other.join("file"),
        f.host.launch_directory.join("AGENTS.md"),
    )
    .unwrap();
    assert!(
        crate::runtime::load_project_context_files(&f.host.launch_directory)
            .unwrap_err()
            .to_string()
            .contains("outside workspace boundary")
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
            .contains("physical binding changed")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cfg3_mcp_absolute_working_directory_is_captured_without_connection() {
    let f = Fixture::new();
    let outside = f.root.path().join("mcp-working-directory");
    std::fs::create_dir(&outside).unwrap();
    f.mcp(
        false,
        json!({"external":{"command":"must-not-spawn", "cwd":outside}}),
    );
    let (launch, effects) = super::static_effects::measure(|| f.resolve());
    assert_eq!(effects, [0; 12]);
    let bindings =
        super::composition::captured_mcp_bindings(&launch.config, &f.credentials).unwrap();
    let binding = bindings.values().next().unwrap();
    let crate::tools::mcp::McpTransportConfig::Stdio { cwd, .. } = &binding.transport else {
        panic!("stdio")
    };
    assert_eq!(cwd.as_deref(), Some(outside.as_path()));
}

#[test]
fn cfg270_only_toml_names_are_discovered_and_old_documents_are_inert() {
    let f = Fixture::new();
    // Even malformed/host-authority-bearing obsolete files are inert.
    for path in [
        f.host.config_directory.join("settings.jsonc"),
        f.host.config_directory.join("settings.toml"),
        f.host.config_directory.join("models.toml"),
        f.host.config_directory.join("models.jsonc"),
        f.host.launch_directory.join("rustx.jsonc"),
    ] {
        std::fs::write(path, b"{ obsolete malformed configuration").unwrap();
    }
    let launch = f.resolve();
    assert_eq!(launch.config.initial_model().model.to_string(), "host/one");
    std::fs::remove_file(f.host.config_directory.join("rustx.toml")).unwrap();
    let missing = analyze(&f.request, &f.host).unwrap_err();
    assert!(missing.incomplete);
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
                    "mcp.toml" => {
                        let _: super::mcp_resources::McpDocument =
                            crate::toml_authoring::parse(&bytes).unwrap();
                    }
                    "rustx.toml" => {
                        super::configuration::parse_layer(
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

#[test]
fn cfg271_removed_existence_registries_are_unknown_fields_in_each_layer() {
    let f = Fixture::new();
    for field in [
        json!({"python_sources":{}}),
        json!({"subagents": {"definitions": []}}),
        json!({"workflows": {"definitions": []}}),
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
        f.user(json!({"agent": {"model": {"model":"host/one"}}}));
    }
}

#[test]
fn cfg274_obsolete_registration_and_manifest_fields_are_authoring_errors() {
    for text in [
        "[subagents]\nworkflow=['reviewer']",
        "[workflows]\nmain=['review']",
        "[workflows.definitions]\nreview='review.yaml'",
    ] {
        assert!(
            crate::local_runtime::config::CurrentRuntimeConfig::from_toml_slice(text.as_bytes())
                .is_err(),
            "obsolete authoring accepted: {text}"
        );
    }
    let mut value: serde_json::Value =
        serde_yaml::from_str(&template_source("typed_agent")).unwrap();
    value["tools"] = json!([]);
    assert!(serde_json::from_value::<crate::runtime::workflow::WorkflowDefinition>(value).is_err());
}

#[test]
fn cfg275_offline_generation_projects_native_facts_once_without_effects_or_secrets() {
    use crate::runtime::capability_inspection::{SourceInspection, WorkflowInspection};
    use crate::skills::{SkillDiagnostic, SkillSource};
    let f = Fixture::new();
    f.mcp(
        false,
        json!({"optional": {"command": "must-never-spawn", "env": {"TOKEN": "SECRET_SOURCE_ENV"}}}),
    );
    f.project(json!({
        "agent": {"tools": {"builtin": ["read"], "sources": {"optional": ["inspect"]}}, "skills": "all", "workflows": ["example"]}
    }));
    f.role(false, "reviewer", json!({"description":"Review", "tools":{"builtin":["read"],"sources":{"absent":["inspect"]}}, "skills":["review"]}), "SECRET_AGENT_BODY");
    for root in [
        &f.host.home_directory.join("rustx"),
        &f.host.launch_directory,
    ] {
        let path = root.join(".agents/skills/review");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            "---\nname: review\ndescription: Review\n---\nSECRET_SKILL_BODY",
        )
        .unwrap();
    }
    let broken = f.host.launch_directory.join(".agents/skills/broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join("SKILL.md"),
        "---\nname: [SECRET_INVALID_VALUE]\ndescription: bad\n---\n",
    )
    .unwrap();
    install_workflow(
        &f,
        "description: Missing tool\nblock:\n  input: {type: object}\n  output: {type: object, properties: {text: {type: string}}, required: [text], additionalProperties: false}\n  entry: inspect\n  nodes:\n    inspect:\n      type: tool\n      selector: {origin: source, source_id: optional, name: inspect}\n      arguments: {type: literal, value: {}}\n      result: {type: text, part: 0}\n    done:\n      type: return\n      output: {type: object, fields: {text: {type: reference, path: [inspect]}}}\n  edges: [{from: inspect, to: done}]\n",
    );
    let ((report, launch), effects) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_show", &f.request, &f.host)
    });
    assert_eq!(effects, [0; 12]);
    let launch = launch.unwrap_or_else(|| panic!("{}", report.render(true)));
    let facts = &launch.inspection;
    let root = facts.main.as_ref().unwrap();
    assert_eq!(
        root.tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["read"]
    );

    assert!(matches!(
        facts.sources[&ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new("optional"))],
        SourceInspection::Unprepared
    ));
    assert_eq!(root.skills[0].source, SkillSource::Workspace);
    assert_eq!(root.skills[0].shadowed[0].source, SkillSource::User);
    assert!(
        facts
            .skill_diagnostics
            .iter()
            .any(|fact| matches!(fact, SkillDiagnostic::PackageInvalid { .. }))
    );
    assert!(
        facts
            .skill_diagnostics
            .iter()
            .any(|fact| matches!(fact, SkillDiagnostic::Shadowed { .. }))
    );
    assert!(matches!(
        facts.workflows.values().next().unwrap(),
        WorkflowInspection::Disabled(_)
    ));
    let named = facts.agents.values().next().unwrap();
    assert_eq!(named.tools[0].name, "read");
    assert_eq!(named.skills[0].source, SkillSource::Workspace);
    assert_eq!(named.diagnostics.len(), 1);
    let expected = serde_json::to_value(facts).unwrap();
    for _ in 0..3 {
        let ((again, candidate), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect("config_show", &f.request, &f.host)
        });
        assert_eq!(effects, [0; 12]);
        assert_eq!(
            serde_json::to_value(candidate.unwrap().inspection).unwrap(),
            expected
        );
        assert_eq!(again.render(true), report.render(true));
    }
    for secret in [
        "SECRET_SOURCE_ENV",
        "SECRET_AGENT_BODY",
        "SECRET_SKILL_BODY",
        "SECRET_INVALID_VALUE",
    ] {
        assert!(!report.render(true).contains(secret), "{secret}");
        assert!(!report.render(false).contains(secret), "{secret}");
    }
}

#[test]
fn cfg275_committed_end_to_end_example_uses_final_authoring_and_offline_admission() {
    let mut f = Fixture::new();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime");
    f.host.launch_directory = example.clone();
    f.request.config = Some(example.join("rustx.toml"));
    let ((report, launch), effects) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_show", &f.request, &f.host)
    });
    assert_eq!(effects, [0; 12]);
    let launch = launch.unwrap_or_else(|| panic!("{}", report.render(true)));
    assert_eq!(
        launch.config.initial_model().model.to_string(),
        "example/demo-model"
    );
    let main = launch.inspection.main.as_ref().unwrap();
    assert!(main.tools.iter().any(|tool| tool.name == "read"));
    assert_eq!(launch.inspection.workflows.len(), 2);
    assert_eq!(launch.inspection.agents.len(), 4);
}

/// Shared source-owner regressions. CLI fixtures only author isolated inputs;
/// every assertion below resolves through the same long-lived manager.
mod session_resolution {
    #[tokio::test]
    async fn durable_omission_uses_current_sources_and_removed_explicit_model_fails_closed() {
        let f = super::Fixture::new();
        let (manager, input) = f.request.session_input(&f.host).unwrap();
        let root = tempfile::tempdir().unwrap();
        let catalog =
            crate::local_runtime::session_controller::SessionController::open(root.path()).unwrap();
        let omitted = crate::local_runtime::session::SessionPersistentState::from_input(&input);
        let a = catalog.create_session(omitted).await.unwrap().session;
        let mut explicit =
            crate::local_runtime::session::SessionPersistentState::from_input(&input);
        explicit.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("host/one").unwrap(),
        ));
        let b = catalog.create_session(explicit).await.unwrap().session;
        drop(catalog);
        let catalog =
            crate::local_runtime::session_controller::SessionController::open(root.path()).unwrap();
        let a = catalog.read_settings(&a.id).await.unwrap().1;
        let b = catalog.read_settings(&b.id).await.unwrap().1;
        f.user(serde_json::json!({"agent":{"model":{"model":"host/two"}}}));
        assert_eq!(
            manager
                .resolve_session(&a.input())
                .unwrap()
                .session_model()
                .model
                .to_string(),
            "host/two"
        );
        assert_eq!(
            manager
                .resolve_session(&b.input())
                .unwrap()
                .session_model()
                .model
                .to_string(),
            "host/one"
        );
        let path = f.host.config_directory.join("rustx.toml");
        let bytes = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, bytes.replace("one", "removed")).unwrap();
        let ((), effects) = crate::local_runtime::static_effects::measure(|| {
            assert!(manager.resolve_session(&b.input()).is_err());
        });
        assert_eq!(effects, [0; 12]);
    }
    use super::*;
    use crate::model::{catalog::ModelRef, session::SessionModelConfig};

    #[test]
    fn one_manager_isolates_explicit_cwd_and_captured_resources() {
        let f = Fixture::new();
        let (manager, input_a) = f.request.session_input(&f.host).unwrap();
        let b = f.root.path().join("second");
        std::fs::create_dir_all(b.join(".agents/skills/only-b")).unwrap();
        std::fs::write(b.join("rustx.toml"), "[agent.model]\nmodel = 'host/two'\n").unwrap();
        std::fs::write(b.join("AGENTS.md"), "B guidance").unwrap();
        std::fs::write(
            b.join(".agents/skills/only-b/SKILL.md"),
            "---\nname: only-b\ndescription: B resource\n---\nB body\n",
        )
        .unwrap();
        std::fs::write(input_a.cwd.join("AGENTS.md"), "A guidance").unwrap();
        let a = manager.resolve_session(&input_a).unwrap();
        let b = manager
            .resolve_session(&SessionConfigInput::new(b))
            .unwrap();
        assert_eq!(a.session_model().model.to_string(), "host/one");
        assert_eq!(b.session_model().model.to_string(), "host/two");
        assert_eq!(a.project_context_files[0].content, "A guidance");
        assert_eq!(b.project_context_files[0].content, "B guidance");
        assert!(a.skill_provenance().is_empty());
        assert_eq!(b.skill_provenance().len(), 1);
        assert_ne!(a.workspace, b.workspace);
        assert_eq!(a.runtime_root, b.runtime_root);
        assert_eq!(
            manager.resolve_session(&input_a).unwrap().config(),
            a.config()
        );
    }

    #[test]
    fn fresh_sources_change_only_later_snapshots_and_omission_stays_omission() {
        let f = Fixture::new();
        let (manager, input) = f.request.session_input(&f.host).unwrap();
        let a = manager.resolve_session(&input).unwrap();
        f.user(json!({"agent":{"model":{"model":"host/two", "request_params":{"temperature":0.3}}, "skills": []}}));
        let b = manager.resolve_session(&input).unwrap();
        assert_eq!(a.session_model().model.to_string(), "host/one");
        assert_eq!(b.session_model().model.to_string(), "host/two");
        assert!(a.input.model.is_none());
        assert!(b.input.model.is_none());
        assert!(input.model.is_none());
        assert!(!a.skill_sources.is_empty());
        assert_eq!(
            a.skill_sources, b.skill_sources,
            "visibility never relocates collection roots"
        );
        let catalog = f.host.config_directory.join("rustx.toml");
        let bytes = std::fs::read_to_string(&catalog).unwrap();
        std::fs::write(&catalog, bytes.replace("128000", "64000")).unwrap();
        let c = manager.resolve_session(&input).unwrap();
        assert_ne!(format!("{:?}", b.models), format!("{:?}", c.models));
        assert_eq!(b.session_model(), c.session_model());
    }

    #[test]
    fn explicit_session_model_is_whole_state_and_beats_current_defaults() {
        let f = Fixture::new();
        f.user(json!({"agent":{"model":{"model":"host/one", "request_params":{"temperature":0.3}, "max_output_tokens":{"mode":"limit","tokens":1024}}}}));
        let (manager, mut input) = f.request.session_input(&f.host).unwrap();
        let omitted = manager.resolve_session(&input).unwrap();
        assert!(!omitted.session_model().request_params.is_empty());
        let mut selected = SessionModelConfig::of(ModelRef::parse("host/two").unwrap());
        selected.max_output_tokens = Some(2048);
        input.model = Some(selected.clone());
        let explicit = manager.resolve_session(&input).unwrap();
        assert_eq!(explicit.session_model(), &selected);
        assert!(explicit.session_model().request_params.is_empty());
        f.user(
            json!({"agent":{"model":{"model":"host/one", "request_params":{"temperature":0.8}}}}),
        );
        assert_eq!(
            manager.resolve_session(&input).unwrap().session_model(),
            &selected
        );
        input
            .model
            .as_mut()
            .unwrap()
            .request_params
            .insert("messages".into(), json!([]));
        let (invalid, effects) =
            super::super::static_effects::measure(|| manager.resolve_session(&input));
        assert!(invalid.is_err());
        assert_eq!(effects, [0; 12]);
    }

    #[test]
    fn invalid_sources_and_disabled_external_sources_have_zero_effects() {
        let f = Fixture::new();
        let (manager, input) = f.request.session_input(&f.host).unwrap();
        for source in [
            "secret_unknown = 'secret-value'",
            "invalid = [",
            "[skills]\nsources = ['unknown']",
            "[agent.model]\nmodel = 'host/absent'",
        ] {
            std::fs::write(f.host.config_directory.join("rustx.toml"), source).unwrap();
            let (result, effects) =
                super::super::static_effects::measure(|| manager.resolve_session(&input));
            assert!(result.is_err());
            assert_eq!(effects, [0; 12]);
        }
        f.mcp(
            true,
            json!({"disabled":{"type":"stdio", "command":"/never-execute", "enabled":false}}),
        );
        f.user(json!({"agent":{"model":{"model":"host/one"}}}));
        let (result, effects) =
            super::super::static_effects::measure(|| manager.resolve_session(&input));
        result.unwrap();
        assert_eq!(effects, [0; 12]);

        let prospective = manager.resolve_session(&input).unwrap();
        let admitted = prospective.admit(|| f.credentials.clone()).unwrap();

        assert!(admitted.project_context_files.is_empty());
    }

    #[test]
    fn resolver_has_no_ambient_cwd_or_environment_mutation_and_cli_delegates() {
        let source = include_str!("configuration.rs");
        for forbidden in [
            "current_dir(",
            "set_current_dir(",
            "set_var(",
            "remove_var(",
            "HostEnvironment",
            "LaunchRequest",
        ] {
            assert!(
                !source.contains(forbidden),
                "shared owner must not contain {forbidden}"
            );
        }
        let f = Fixture::new();
        let (manager, _) = f.request.session_input(&f.host).unwrap();
        let (result, effects) = super::super::static_effects::measure(|| {
            manager.resolve_session(&SessionConfigInput::new(".".into()))
        });
        assert!(result.is_err());
        assert_eq!(effects, [0; 12]);
        let cli = include_str!("launch.rs");
        assert!(cli.contains("manager.resolve_session(&input)"));
        assert!(!cli.contains("merged.overlay"));
        assert!(!cli.contains("resolve_agent_profile"));
    }

    #[tokio::test]
    async fn captured_guidance_and_skills_survive_source_edits_before_composition() {
        let f = Fixture::new();
        let (manager, input) = f.request.session_input(&f.host).unwrap();
        std::fs::write(input.cwd.join("AGENTS.md"), "Captured guidance").unwrap();
        let skill = input.cwd.join(".agents/skills/frozen");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: frozen\ndescription: Captured Skill\n---\nbody\n",
        )
        .unwrap();
        let a = manager.resolve_session(&input).unwrap();
        std::fs::remove_dir_all(&skill).unwrap();
        std::fs::write(input.cwd.join("AGENTS.md"), "Later guidance").unwrap();
        let b = manager.resolve_session(&input).unwrap();
        assert_eq!(a.project_context_files[0].content, "Captured guidance");
        assert_eq!(b.project_context_files[0].content, "Later guidance");
        let admitted = a.admit(|| f.credentials.clone()).unwrap();
        let product = LocalSessionClient::compose(&admitted, &LocalRuntimeDependencies::default())
            .await
            .unwrap();
        let resources = product.runtime().runtime_resources();
        assert_eq!(
            resources.project_context_files()[0].content,
            "Captured guidance"
        );
        assert_eq!(resources.inspection().skills.len(), 1);
        assert!(b.skill_provenance().is_empty());
        drop(product);
        assert_eq!(
            admitted.project_context_files[0].content,
            "Captured guidance"
        );
    }
    #[test]
    fn session_input_and_provenance_redact_provider_native_values() {
        let f = Fixture::new();
        let (manager, mut input) = f.request.session_input(&f.host).unwrap();
        let mut selected = SessionModelConfig::of(ModelRef::parse("host/one").unwrap());
        selected
            .request_params
            .insert("custom".into(), json!("session-secret-value"));
        input.model = Some(selected);
        let prospective = manager.resolve_session(&input).unwrap();
        for text in [
            format!("{input:?}"),
            format!("{prospective:?}"),
            serde_json::to_string(prospective.provenance()).unwrap(),
        ] {
            assert!(!text.contains("session-secret-value"));
        }
    }

    #[tokio::test]
    async fn root_explicit_instruction_capture_survives_edit_and_fresh_resolution_refreshes() {
        let f = Fixture::new();
        let file = f.host.launch_directory.join("root-instructions.md");
        std::fs::write(&file, "instruction A").unwrap();
        f.project(json!({"agent":{"agents_md":{"files":["root-instructions.md"]}}}));
        let (manager, input) = f.request.session_input(&f.host).unwrap();
        let a = manager.resolve_session(&input).unwrap();
        let inspection_a = a.inspection.clone();
        assert_eq!(a.root_agent_project_files[0].content, "instruction A");
        std::fs::write(&file, "instruction B").unwrap();
        let b = manager.resolve_session(&input).unwrap();
        assert_eq!(b.root_agent_project_files[0].content, "instruction B");
        // Removal cannot cause a semantic reread failure at initial composition.
        std::fs::remove_file(&file).unwrap();
        let admitted_a = a.admit(|| f.credentials.clone()).unwrap();
        let product_a =
            LocalSessionClient::compose(&admitted_a, &LocalRuntimeDependencies::default())
                .await
                .unwrap();
        let resources_a = product_a.runtime().runtime_resources();
        assert_eq!(
            resources_a
                .root_profile()
                .unwrap()
                .project_instructions
                .files[0]
                .content,
            "instruction A"
        );
        assert_eq!(resources_a.inspection(), &inspection_a);
        let old_files = resources_a
            .root_profile()
            .unwrap()
            .project_instructions
            .files
            .clone();
        drop(resources_a);
        drop(product_a);
        let inspection_b = b.inspection.clone();
        let admitted_b = b.admit(|| f.credentials.clone()).unwrap();
        let product_b =
            LocalSessionClient::compose(&admitted_b, &LocalRuntimeDependencies::default())
                .await
                .unwrap();
        let resources_b = product_b.runtime().runtime_resources();
        assert_eq!(
            resources_b
                .root_profile()
                .unwrap()
                .project_instructions
                .files[0]
                .content,
            "instruction B"
        );
        assert_eq!(resources_b.inspection(), &inspection_b);
        assert_eq!(old_files[0].content, "instruction A");
    }

    #[test]
    fn whole_session_model_provenance_covers_every_field_and_catalog_default_choice() {
        use crate::model::catalog::ReasoningProfileId;
        use crate::model::session::SummaryModelPolicy;
        let f = Fixture::new();
        let path = f.host.config_directory.join("rustx.toml");
        let mut catalog: serde_json::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &mut catalog["models"]["host/one"];
        entry["capabilities"]["reasoning"] = json!(true);
        entry["reasoning"] = json!({"default_profile":"off", "profiles":{"off":{"enabled":false},"on":{"enabled":true,"request_params":{"reasoning_effort":"high"}}}});
        std::fs::write(path, toml::to_string_pretty(&catalog).unwrap()).unwrap();
        let (manager, mut input) = f.request.session_input(&f.host).unwrap();
        let mut model = SessionModelConfig::of(ModelRef::parse("host/one").unwrap());
        model.reasoning_profile = Some(ReasoningProfileId::parse("on").unwrap());
        model
            .request_params
            .insert("custom".into(), json!("secret-model-parameter"));
        model.max_output_tokens = Some(1024);
        model.summary_model = SummaryModelPolicy::Explicit {
            model: ModelRef::parse("host/two").unwrap(),
            reasoning_profile: None,
            request_params: serde_json::Map::default(),
            max_output_tokens: Some(512),
        };
        for selected in [
            model,
            SessionModelConfig::of(ModelRef::parse("host/one").unwrap()),
        ] {
            input.model = Some(selected.clone());
            let prospective = manager.resolve_session(&input).unwrap();
            assert_eq!(prospective.session_model(), &selected);
            assert_eq!(prospective.input.model.as_ref(), Some(&selected));
            assert_eq!(
                prospective.config().initial_model().model.to_string(),
                "host/one"
            );
            assert!(matches!(
                prospective.provenance()["agent.model"],
                Origin::User { .. }
            ));
            for text in [
                format!("{input:?}"),
                format!("{prospective:?}"),
                serde_json::to_string(prospective.provenance()).unwrap(),
            ] {
                assert!(!text.contains("secret-model-parameter"));
            }
        }
    }
}

#[tokio::test]
async fn app286_cold_resume_retains_one_settings_revision_through_resolution() {
    use super::session::{SessionCatalog, SessionPersistentState};
    use super::session_controller::SessionController;
    use crate::model::{ModelRef, session::SessionModelConfig};
    let f = Fixture::new();
    f.project(json!({"agent":{"tools":{"builtin":["read"]}}}));
    let launch = f.resolve();
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    let mut a = SessionPersistentState::from_input(&launch.input);
    a.model = Some(SessionModelConfig::of(ModelRef::parse("host/two").unwrap()));
    let session = controller.create_session(a.clone()).await.unwrap().session;
    let mut b = a.clone();
    b.model = Some(SessionModelConfig::of(ModelRef::parse("host/one").unwrap()));
    drop(controller);
    let gate = super::composition::cold_resume_test_support::arm(&launch.runtime_root);
    let dependencies = LocalRuntimeDependencies {
        startup_session: super::StartupSession::Select {
            session: session.id.clone(),
            node: None,
        },
        ..Default::default()
    };
    let worker_launch = launch.clone();
    let resume =
        tokio::spawn(
            async move { LocalSessionClient::compose(&worker_launch, &dependencies).await },
        );
    gate.entered().await; // A captured; resolver has not run yet.
    // The attempted B publisher cannot acquire native ownership. Short catalog
    // reads remain available while the resolver is parked; no metadata lock spans it.
    assert!(SessionController::open(&launch.runtime_root).is_err());
    let observed = SessionCatalog::open_existing(&launch.runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(observed.lineage(&session.id, None).unwrap().1, a);
    assert_eq!(observed.settings_revision(&session.id).unwrap(), 0);
    assert_eq!(observed.snapshot(&session.id).unwrap(), session);
    gate.release();
    let product = resume.await.unwrap().unwrap();
    assert_eq!(product.runtime().model_config(), a.model.clone().unwrap());
    let resources = product.runtime().runtime_resources();
    let tool_names: Vec<_> = resources
        .inspection()
        .main
        .as_ref()
        .unwrap()
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(tool_names, ["read"]);
    drop(resources);
    product.runtime().shutdown().await.unwrap();
    drop(product);
    drop(observed);
    // B becomes admissible only after the A owner exits. Its subsequent cold
    // load resolves the complete new revision, including the non-model selection.
    f.project(json!({"agent":{"tools":{"builtin":["glob"]}}}));
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    assert_eq!(
        controller
            .replace_settings(&session.id, 0, b.clone())
            .await
            .unwrap(),
        1
    );
    drop(controller);
    let product = LocalSessionClient::compose(
        &launch,
        &LocalRuntimeDependencies {
            startup_session: super::StartupSession::Select {
                session: session.id.clone(),
                node: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(product.runtime().model_config(), b.model.unwrap());
    let resources = product.runtime().runtime_resources();
    let tool_names: Vec<_> = resources
        .inspection()
        .main
        .as_ref()
        .unwrap()
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(tool_names, ["glob"]);
    product.runtime().shutdown().await.unwrap();
}

#[tokio::test]
async fn app286_cold_node_routes_do_not_publish_graph_focus() {
    use super::session::{SessionCatalog, SessionPersistentState};
    use super::session_controller::SessionController;
    use crate::durable::ConversationStore;
    use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
    use crate::runtime::identity::MessageId;
    let f = Fixture::new();
    let launch = f.resolve();
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    let a = controller
        .create_session(SessionPersistentState::from_input(&launch.input))
        .await
        .unwrap()
        .session;
    let access = controller.acquire_session(&a.id, None).await.unwrap();
    let store = crate::durable::SqliteConversationStore::open(
        a.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    let boundary = MessageId::new("route-boundary");
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: boundary.clone(),
            content: vec![],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    let b = controller
        .branch_session_node(&a.id, &a.active_node, revision, &boundary)
        .await
        .unwrap()
        .session;
    let default = controller
        .set_current_node(&a.id, &a.active_node)
        .await
        .unwrap();
    let catalog_path = launch.runtime_root.join("sessions/catalog.json");
    let bytes = std::fs::read(&catalog_path).unwrap();
    drop(store);
    drop(access);
    drop(controller);
    // Independent cold clients, including the TUI's explicit-default route.
    for node in [
        Some(b.active_node.clone()),
        Some(a.active_node.clone()),
        Some(b.active_node.clone()),
        None,
    ] {
        let expected_conversation = if node.as_ref() == Some(&b.active_node) {
            &b.active_conversation_id
        } else {
            &a.active_conversation_id
        };
        let product = LocalSessionClient::compose(
            &launch,
            &LocalRuntimeDependencies {
                startup_session: super::StartupSession::Select {
                    session: a.id.clone(),
                    node,
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(product.runtime().conversation_id(), expected_conversation);
        let response = crate::runtime_client::host::RuntimeClientSessionControl::handle(
            product.supervisor().as_ref(),
            crate::runtime_client::types::RuntimeClientSessionRequest::Get,
        )
        .await
        .unwrap();
        let crate::runtime_client::types::RuntimeClientResult::Session { session: view } = response
        else {
            panic!("Session Get projection")
        };
        assert_eq!(&view.active_conversation_id, expected_conversation);
        let durable_default = &default.active_node;
        assert_eq!(
            &product.supervisor().current().await.unwrap().active_node,
            durable_default
        );

        assert_eq!(product.supervisor().current().await.unwrap(), default);
        assert_eq!(std::fs::read(&catalog_path).unwrap(), bytes);
        let catalog = SessionCatalog::open_existing(&launch.runtime_root)
            .unwrap()
            .unwrap();
        assert_eq!(catalog.snapshot(&a.id).unwrap(), default);
        assert_eq!(
            catalog.list_page(None, 0, 32).unwrap().sessions[0].active_node,
            a.active_node
        );
        // The two routing projections may differ while both snapshots still
        // report A as the durable graph default.
        let route_a = product
            .supervisor()
            .select(a.id.clone(), Some(a.active_node.clone()))
            .await
            .unwrap();
        let route_b = product
            .supervisor()
            .select(a.id.clone(), Some(b.active_node.clone()))
            .await
            .unwrap();
        assert_eq!(route_a.session, route_b.session);
        assert_ne!(route_a.node.id, route_b.node.id);
        product.runtime().shutdown().await.unwrap();
    }
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    assert_eq!(controller.read_session(&a.id).await.unwrap(), default);
    let changed = controller
        .set_current_node(&a.id, &b.active_node)
        .await
        .unwrap();
    assert_eq!(changed.active_node, b.active_node);
    assert_ne!(std::fs::read(&catalog_path).unwrap(), bytes);
    drop(controller);
    let catalog = SessionCatalog::open_existing(&launch.runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(catalog.snapshot(&a.id).unwrap(), changed);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn app286_session_wire_results_compare_the_installed_route_without_rebinding() {
    use super::session::SessionPersistentState;
    use super::session_controller::SessionController;
    use crate::durable::ConversationStore;
    use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
    use crate::runtime::identity::MessageId;
    use crate::runtime_client::host::RuntimeClientSessionControl;
    use crate::runtime_client::types::{
        RuntimeClientResult, RuntimeClientSessionRequest as Request,
    };

    let f = Fixture::new();
    let launch = f.resolve();
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    let original = controller
        .create_session(SessionPersistentState::from_input(&launch.input))
        .await
        .unwrap()
        .session;
    let access = controller
        .acquire_session(&original.id, None)
        .await
        .unwrap();
    let store = crate::durable::SqliteConversationStore::open(
        original.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    let message_id = MessageId::new("reattach-boundary");
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: message_id.clone(),
            content: vec![],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .unwrap();
    let surface_revision = store.load_head().unwrap().revision;
    drop(store);
    drop(access);
    drop(controller);
    let product = LocalSessionClient::compose(
        &launch,
        &LocalRuntimeDependencies {
            startup_session: super::StartupSession::Select {
                session: original.id.clone(),
                node: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let attachment = product.supervisor();
    let runtime = product.runtime();
    let lifecycle = runtime.lifecycle_state();
    let history = runtime.historical_head_snapshot().unwrap();
    let canonical = runtime.historical_canonical_history().unwrap();
    let model = runtime.model_config();
    for request in [
        Request::New,
        Request::Clone,
        Request::Fork {
            surface_revision,
            message_id: message_id.clone(),
        },
        Request::TreeBranch {
            surface_revision,
            message_id,
        },
    ] {
        let tree_branch = matches!(request, Request::TreeBranch { .. });
        let RuntimeClientResult::SessionChanged {
            session: target,
            restart_required,
            ..
        } = attachment.handle(request).await.unwrap()
        else {
            panic!("route-changing result")
        };
        assert!(restart_required);
        assert_ne!(
            target.active_conversation_id,
            original.active_conversation_id
        );
        assert_eq!(target.id == original.id, tree_branch);
        // Both another Session and another node in this Session require replacement.
        let RuntimeClientResult::SessionChanged {
            restart_required, ..
        } = attachment
            .handle(Request::Select {
                session_id: target.id,
                node_id: Some(target.active_node),
            })
            .await
            .unwrap()
        else {
            panic!("select result")
        };
        assert!(restart_required);
        // Selecting the installed route is still a no-op, even after tree branch
        // published a different durable graph default.
        for node_id in [Some(original.active_node.clone()), None] {
            let RuntimeClientResult::SessionChanged {
                restart_required, ..
            } = attachment
                .handle(Request::Select {
                    session_id: original.id.clone(),
                    node_id: node_id.clone(),
                })
                .await
                .unwrap()
            else {
                panic!("same Session select result")
            };
            assert_eq!(restart_required, tree_branch && node_id.is_none());
        }
        for request in [
            Request::Get,
            Request::Name("renamed".into()),
            Request::Tree {
                node_offset: 0,
                history_offset: 0,
                limit: 32,
            },
        ] {
            let view = match attachment.handle(request).await.unwrap() {
                RuntimeClientResult::Session { session }
                | RuntimeClientResult::SessionTree { session, .. } => session,
                RuntimeClientResult::SessionChanged {
                    session,
                    restart_required,
                    ..
                } => {
                    assert!(!restart_required);
                    session
                }
                other => panic!("unexpected metadata result: {other:?}"),
            };
            assert_eq!(view.id, original.id);
            assert_eq!(view.active_conversation_id, original.active_conversation_id);
        }
        assert_eq!(runtime.conversation_id(), &original.active_conversation_id);
        assert_eq!(runtime.lifecycle_state(), lifecycle, "no drain or shutdown");
        assert_eq!(runtime.historical_head_snapshot().unwrap(), history);
        assert_eq!(runtime.historical_canonical_history().unwrap(), canonical);
        assert_eq!(runtime.model_config(), model);
    }
    runtime.shutdown().await.unwrap();
}

/// Issue #386: Session display-projection repair is Session-owned and reads the
/// Session's **root** lineage, whatever node a launch selects. Arming the live
/// publisher is the only root-runtime-specific half.
///
/// The Session below has a renderable root subject, a missing catalog
/// projection, and a branch whose own first ordinary message is different. A
/// launch that resumes straight onto that branch must repair the Session to the
/// *root's* subject, must not arm a publisher, must not touch canonical
/// history, and must write nothing at all the second time.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn startup_on_a_branch_repairs_the_projection_from_the_root_without_arming() {
    use super::session::{SessionCatalog, SessionPersistentState};
    use super::session_controller::SessionController;
    use crate::durable::ConversationStore;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;

    fn text_user(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }
    fn store_of(
        access: &super::session_controller::SessionAccess,
    ) -> crate::durable::SqliteConversationStore {
        crate::durable::SqliteConversationStore::open(
            access.node.conversation_id.clone(),
            &access.database_path,
        )
        .unwrap()
    }

    let f = Fixture::new();
    let launch = f.resolve();
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    let session = controller
        .create_session(SessionPersistentState::from_input(&launch.input))
        .await
        .unwrap()
        .session;
    let root_node = session.active_node.clone();
    let root_access = controller
        .acquire_session(&session.id, Some(&root_node))
        .await
        .unwrap();
    let root_store = store_of(&root_access);
    let boundary = MessageId::new("root-subject-a");
    root_store
        .append_canonical(&text_user("root-subject-a", "root subject A"))
        .unwrap();
    let revision = root_store.load_head().unwrap().revision;
    // The branch is cut *before* the root's first boundary, so it retains no
    // root message at all and can be given a first message of its own.
    let branch = controller
        .branch_session_node(&session.id, &session.active_node, revision, &boundary)
        .await
        .unwrap()
        .session;
    let branch_node = branch.active_node.clone();
    let branch_access = controller
        .acquire_session(&session.id, Some(&branch_node))
        .await
        .unwrap();
    let branch_conversation = branch_access.node.conversation_id.clone();
    let branch_store = store_of(&branch_access);
    branch_store
        .append_canonical(&text_user("branch-subject-z", "branch subject Z"))
        .unwrap();
    assert_eq!(
        controller
            .read_session_summary(&session.id)
            .await
            .unwrap()
            .preview,
        None,
        "the Session starts with a missing projection"
    );
    let root_canonical = root_store.load_canonical().unwrap();
    let branch_canonical = branch_store.load_canonical().unwrap();
    // The branch must genuinely disagree with the root, or the regression could
    // pass by reading the wrong lineage.
    assert_ne!(root_canonical, branch_canonical);
    drop(branch_store);
    drop(branch_access);
    drop(root_store);
    drop(root_access);
    drop(controller);

    let catalog_path = launch.runtime_root.join("sessions/catalog.json");
    let product = LocalSessionClient::compose(
        &launch,
        &LocalRuntimeDependencies {
            startup_session: super::StartupSession::Select {
                session: session.id.clone(),
                node: Some(branch_node.clone()),
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        product.runtime().conversation_id(),
        &branch_conversation,
        "the launch really composed the branch"
    );
    assert!(
        super::session_display_projection::display_projection_probe(&session.id).is_none(),
        "a branch runtime is never the projection subject, so no publisher is armed"
    );
    product.runtime().shutdown().await.unwrap();
    drop(product);

    let catalog = SessionCatalog::open_existing(&launch.runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(
        catalog.summary(&session.id).unwrap().preview.as_deref(),
        Some("root subject A"),
        "repair derives the Session's root subject, never the composed branch's"
    );
    let repaired_generation = catalog.document_generation();
    let repaired_snapshot = catalog.snapshot(&session.id).unwrap();
    let repaired_revision = catalog.settings_revision(&session.id).unwrap();
    let repaired_bytes = std::fs::read(&catalog_path).unwrap();
    drop(catalog);

    // Canonical history is untouched by display repair, on both lineages.
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    let root_access = controller
        .acquire_session(&session.id, Some(&root_node))
        .await
        .unwrap();
    assert_eq!(
        store_of(&root_access).load_canonical().unwrap(),
        root_canonical
    );
    let branch_access = controller
        .acquire_session(&session.id, Some(&branch_node))
        .await
        .unwrap();
    assert_eq!(
        store_of(&branch_access).load_canonical().unwrap(),
        branch_canonical
    );
    drop(branch_access);
    drop(root_access);
    drop(controller);

    // Write-level idempotence: reopening the same branch repairs nothing, so
    // the catalog file, its generation, `updated_at` and `settings_revision`
    // are all byte-for-byte what the first repair left.
    let product = LocalSessionClient::compose(
        &launch,
        &LocalRuntimeDependencies {
            startup_session: super::StartupSession::Select {
                session: session.id.clone(),
                node: Some(branch_node.clone()),
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    product.runtime().shutdown().await.unwrap();
    drop(product);
    assert_eq!(std::fs::read(&catalog_path).unwrap(), repaired_bytes);
    let catalog = SessionCatalog::open_existing(&launch.runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(catalog.document_generation(), repaired_generation);
    assert_eq!(catalog.snapshot(&session.id).unwrap(), repaired_snapshot);
    assert_eq!(
        catalog.settings_revision(&session.id).unwrap(),
        repaired_revision
    );
}

/// Issue #386: an unrenderable first root message is a settled `None`. Neither
/// the branch's own renderable text nor a later root message may manufacture a
/// replacement projection, on any composition path.
#[tokio::test]
async fn startup_never_manufactures_a_projection_for_an_unrenderable_root_subject() {
    use super::session::{SessionCatalog, SessionPersistentState};
    use super::session_controller::SessionController;
    use crate::durable::ConversationStore;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;

    fn user(id: &str, content: Vec<UserContentBlock>) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    let f = Fixture::new();
    let launch = f.resolve();
    let controller = SessionController::open(&launch.runtime_root).unwrap();
    let session = controller
        .create_session(SessionPersistentState::from_input(&launch.input))
        .await
        .unwrap()
        .session;
    let access = controller.acquire_session(&session.id, None).await.unwrap();
    let store = crate::durable::SqliteConversationStore::open(
        session.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    store
        .append_canonical(&user(
            "unrenderable-first",
            vec![UserContentBlock::Image(
                crate::message::content::ImageReference {
                    artifact_id: crate::runtime::identity::ArtifactId::new("artifact-1"),
                    alt: None,
                },
            )],
        ))
        .unwrap();
    // A later root message is renderable, and is still not the subject.
    let boundary = MessageId::new("later-root-text");
    store
        .append_canonical(&user(
            "later-root-text",
            vec![UserContentBlock::Text(TextBlock {
                text: "a later root message is not the subject".into(),
            })],
        ))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    let branch = controller
        .branch_session_node(&session.id, &session.active_node, revision, &boundary)
        .await
        .unwrap()
        .session;
    let branch_node = branch.active_node.clone();
    let branch_access = controller
        .acquire_session(&session.id, Some(&branch_node))
        .await
        .unwrap();
    crate::durable::SqliteConversationStore::open(
        branch_access.node.conversation_id.clone(),
        &branch_access.database_path,
    )
    .unwrap()
    .append_canonical(&user(
        "branch-text",
        vec![UserContentBlock::Text(TextBlock {
            text: "branch text is not the subject".into(),
        })],
    ))
    .unwrap();
    let _ = &branch;
    drop(branch_access);
    drop(store);
    drop(access);
    drop(controller);

    let catalog_path = launch.runtime_root.join("sessions/catalog.json");
    let bytes = std::fs::read(&catalog_path).unwrap();
    for node in [Some(branch_node.clone()), None] {
        let product = LocalSessionClient::compose(
            &launch,
            &LocalRuntimeDependencies {
                startup_session: super::StartupSession::Select {
                    session: session.id.clone(),
                    node,
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            super::session_display_projection::display_projection_probe(&session.id).is_none(),
            "a settled None is never re-armed, on the root or on a branch"
        );
        product.runtime().shutdown().await.unwrap();
        drop(product);
        assert_eq!(
            std::fs::read(&catalog_path).unwrap(),
            bytes,
            "an unrenderable subject writes nothing at all"
        );
    }
    let catalog = SessionCatalog::open_existing(&launch.runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(catalog.summary(&session.id).unwrap().preview, None);
}
