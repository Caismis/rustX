//! CFG-01 filesystem and real native composition regressions (Linux and macOS CI).
#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)] // linear fixture scenarios
use super::launch::*;
use super::{LocalRuntimeDependencies, LocalSessionProduct};
use crate::capabilities::{CapabilitySourceId, CapabilitySourceState};
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
    let config = json!({"subagents":{"definitions":["reviewer"],"workflow":["reviewer"]},"workflows":{"definitions":["example"],"main":[]}});
    let original = template_source("typed_agent");
    let scenarios = [
        (original.clone(), config.clone(), Validity::Valid, None),
        (
            original.clone(),
            json!({"workflows":{"definitions":["example"]}}),
            Validity::Invalid,
            Some("block.nodes.summarize.profile"),
        ),
        (
            original.clone(),
            json!({"subagents":{"definitions":["reviewer"]},"workflows":{"definitions":["example"]}}),
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
                assert!(projection.registered);
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
    // Unregistered files never become registrations; untrusted files are not parsed.
    f.project(json!({}));
    install_workflow(&f, "invalid: [");
    let id = crate::runtime::workflow::WorkflowId::parse("example").unwrap();
    assert_eq!(
        super::workflow_inspection::inspect(&id, true, &f.request, &f.host).diagnostics[0].path,
        "workflows.definitions"
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
    f.project(json!({"subagents":{"definitions":["reviewer"],"workflow":["reviewer"]},"workflows":{"definitions":["example"]}}));
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
tools: [{origin: mcp, server_id: external, name: inspect}]
block:
  input: {type: object, properties: {}, additionalProperties: false}
  output: {type: object, properties: {text: {type: string}}, required: [text], additionalProperties: false}
  entry: inspect
  nodes:
    inspect:
      type: tool
      selector: {origin: mcp, server_id: external, name: inspect}
      arguments: {type: literal, value: {secret: SECRET_LITERAL}}
      result: {type: text, part: 0}
    done:
      type: return
      output: {type: object, fields: {text: {type: reference, path: [inspect]}}}
  edges: [{from: inspect, to: done}]
";
    install_workflow(&f, text);
    for enabled in [true, false] {
        f.project(json!({"mcpServers":{"external":{"enabled":enabled,"command":"must-never-spawn"}},"workflows":{"definitions":["example"]}}));
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
    f.project(json!({"subagents":{"definitions":["reviewer"],"workflow":["reviewer"]},"workflows":{"definitions":["example"]}}));
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
    f.project(json!({"workflows":{"definitions":["example"]}}));
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
    f.project(json!({"subagents":{"definitions":["reviewer"],"main":[],"workflow":["reviewer"]}}));
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
    assert_eq!(report.diagnostics[0].path, "subagents.definitions.reviewer");
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
    f.project(json!({"subagents":{"definitions":["reviewer"]}}));
    let physical_config = f.host.config_directory.clone();
    let alias = f.root.path().join("config-alias");
    std::os::unix::fs::symlink(&physical_config, &alias).unwrap();
    f.host.config_directory = alias.clone();
    let launch = f.resolve();
    assert_eq!(launch.role_root, physical_config.join("subagents"));
    for operation in ["config_check", "config_show"] {
        let ((report, prospective), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect(operation, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        assert_eq!(prospective.unwrap().role_root, launch.role_root);
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
    let (catalog, _) = super::subagent_resources::load(
        &launch.workspace,
        &launch.role_root,
        &launch.config.subagents,
    )
    .unwrap();
    assert_eq!(
        catalog.definitions().next().unwrap().instructions(),
        "User body"
    );
    // Replacing the physical authority itself must fail, not capture its new target.
    std::fs::rename(&launch.role_root, f.root.path().join("retired-roles")).unwrap();
    std::fs::write(
        replacement.join("reviewer.md"),
        "---\ndescription: outside\n---\nOutside",
    )
    .unwrap();
    std::os::unix::fs::symlink(&replacement, &launch.role_root).unwrap();
    assert!(
        super::subagent_resources::load(
            &launch.workspace,
            &launch.role_root,
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
    f.project(json!({"subagents":{"definitions":["role"],"main":["role"],"workflow":[]}}));
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
    f.project(json!({"subagents":{"definitions":["role"],"main":[],"workflow":["role"]}}));
    let gate = super::subagent_resources::test_support::arm(&f.host.launch_directory);
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
    let gate = super::subagent_resources::test_support::arm(&f.host.launch_directory);
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
    assert!(r2.subagent_main_admission().is_empty());
    assert!(r2.subagent_workflow_admission().contains(&role));
    assert_eq!(r1.subagents().get(&role).unwrap().instructions(), "R1 body");
    assert!(r1.subagent_main_admission().contains(&role));
    assert!(r1.subagent_workflow_admission().is_empty());
    product.runtime().shutdown().await.unwrap();
}

fn check_python_package(
    intent: Option<&str>,
    trusted: bool,
    files: Option<(&str, &str)>,
    expected: Option<PythonLocalStatus>,
    parses: usize,
) {
    let f = Fixture::new();
    if let Some((server, requirements)) = files {
        let package = f.host.launch_directory.join(".agents/tools/foo");
        std::fs::create_dir_all(&package).unwrap();
        if !server.is_empty() {
            std::fs::write(package.join("server.py"), server).unwrap();
        }
        std::fs::write(package.join("requirements.txt"), requirements).unwrap();
    }
    if let Some(intent) = intent {
        f.project(json!({"pythonSources":{"python:foo":intent}}));
    }
    if !trusted {
        f.trust(TrustAction::Revoke);
    }
    for operation in ["config_check", "config_show"] {
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| count.set(0));
        let ((report, launch), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect(operation, &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        for output in [
            report.render(false),
            report.render(true),
            format!("{report:?}"),
        ] {
            assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
        }
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| assert_eq!(count.get(), parses));
        let launch = launch.expect("optional source failures retain the prospective launch");
        assert!(!launch.environment_store_root().exists());
        assert!(!launch.runtime_root.exists());
        let source = &report.launch.as_ref().unwrap().sources["python:foo"];
        assert_eq!(source.local_status, expected);
        let invalid = matches!(
            expected,
            Some(PythonLocalStatus::Missing | PythonLocalStatus::Invalid)
        );
        assert_eq!(report.exit_code(), if invalid { 2 } else { 3 });
        assert_eq!(
            source.readiness,
            if invalid {
                "unavailable"
            } else if expected.is_some() {
                "unresolved"
            } else {
                "inert"
            }
        );
        if invalid {
            let diagnostic = report
                .diagnostics
                .iter()
                .find(|d| d.path == "pythonSources.python:foo")
                .unwrap();
            assert_eq!(diagnostic.category, "invalid");
            assert_eq!(diagnostic.classification, "error");
            assert!(diagnostic.file.is_some());
            assert!(
                diagnostic
                    .reason
                    .contains(if expected == Some(PythonLocalStatus::Missing) {
                        "not present locally"
                    } else {
                        "local package contract"
                    })
            );
            assert!(diagnostic.correction.contains("server.py"));
            // Static source failure does not become a global runtime admission failure.
            assert!(
                launch
                    .admit(crate::credentials::CredentialSnapshot::default)
                    .is_ok()
            );
        }
    }
}

#[test]
fn cfg235_enabled_python_package_is_locally_validated_without_preparation() {
    check_python_package(
        Some("enabled"),
        true,
        Some(("# inert server", "")),
        Some(PythonLocalStatus::Valid),
        1,
    );
}
#[test]
fn cfg235_enabled_missing_python_package_is_a_precise_static_source_failure() {
    check_python_package(
        Some("enabled"),
        true,
        None,
        Some(PythonLocalStatus::Missing),
        0,
    );
}
#[test]
fn cfg235_enabled_malformed_python_package_is_a_precise_static_source_failure() {
    check_python_package(
        Some("enabled"),
        true,
        Some(("", "")),
        Some(PythonLocalStatus::Invalid),
        1,
    );
    check_python_package(
        Some("enabled"),
        true,
        Some(("# server", "--index-url RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")),
        Some(PythonLocalStatus::Invalid),
        1,
    );
}
#[test]
fn cfg235_disabled_malformed_python_package_remains_inert() {
    check_python_package(Some("disabled"), true, Some(("", "invalid")), None, 0);
}
#[test]
fn cfg235_unconfigured_malformed_python_package_remains_inert() {
    check_python_package(None, true, Some(("", "invalid")), None, 0);
}
#[test]
fn cfg235_untrusted_python_package_contents_are_not_read() {
    check_python_package(Some("enabled"), false, Some(("", "invalid")), None, 0);
}

#[test]
fn cfg235_provider_readiness_is_unresolved_without_credential_lookup() {
    let f = Fixture::new();
    let path = f.host.config_directory.join("models.jsonc");
    let mut model: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for credential in ["$CFG235_UNSET", "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"] {
        model["providers"]["host"]["apiKey"] = json!(credential);
        std::fs::write(&path, serde_json::to_vec(&model).unwrap()).unwrap();
        for mcp in [false, true] {
            f.project(if mcp {
                json!({"mcpServers":{"online":{"enabled":true,"url":"http://127.0.0.1:9/mcp"}}})
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
                            .any(|d| d.path == "mcpServers.online" && d.category == "unresolved")
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
fn cfg235_python_local_contract_reuses_name_file_and_symlink_validation() {
    for case in ["missing_requirements", "invalid-name", "symlink"] {
        let f = Fixture::new();
        let name = if case == "invalid-name" {
            "bad--name"
        } else {
            "foo"
        };
        let id = format!("python:{name}");
        let package = f.host.launch_directory.join(".agents/tools").join(name);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("server.py"), "# inert").unwrap();
        if case != "missing_requirements" {
            std::fs::write(package.join("requirements.txt"), "").unwrap();
        }
        if case == "symlink" {
            std::os::unix::fs::symlink("server.py", package.join("linked.py")).unwrap();
        }
        f.project(json!({"pythonSources":{id.clone():"enabled"}}));
        let ((report, launch), effects) = super::static_effects::measure(|| {
            super::diagnostics::inspect("config_check", &f.request, &f.host)
        });
        assert_eq!(effects, [0; 13]);
        assert_eq!(report.exit_code(), 2, "{case}");
        assert_eq!(
            report
                .launch
                .as_ref()
                .unwrap_or_else(|| panic!("{case}: {}", report.render(true)))
                .sources[&id]
                .local_status,
            Some(PythonLocalStatus::Invalid)
        );
        assert!(!launch.unwrap().environment_store_root().exists());
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
        json!({"mcpServers":{"offline":{"enabled":true,"url":"http://127.0.0.1:9/mcp"}}}),
        json!({"mcpServers":{"disabled":{"enabled":false,"command":"must-never-spawn","args":[sentinel]}}}),
        json!({"mcpServers":{"unconfigured":{"command":"must-never-spawn"}}}),
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
                        .find(|diagnostic| diagnostic.path == format!("mcpServers.{name}"))
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
    f.project(json!({"mcpServers":{"untrusted":{"enabled":true,"command":"must-never-spawn"}}}));
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
    f.project(json!({"context":{"reserveTokens":8192},"environment":{"PRIVATE":"RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"}}));
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
        "pythonSources":{"python:foo":"enabled"},
        "environment": (0..4096).map(|index| (format!("FIELD_{index}"), "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")).collect::<std::collections::BTreeMap<_,_>>()
    }));
    let (report, _) = super::diagnostics::inspect("config_show", &f.request, &f.host);
    assert_eq!(report.exit_code(), 2);
    assert_bounded_cause(&report, "pythonSources.python:foo", "invalid");
}

#[test]
fn cfg235_oversized_incomplete_projection_preserves_incomplete_diagnostic() {
    let f = Fixture::new();
    f.user(json!({}));
    f.project(json!({"defaultTools": (0..12000).map(|index| format!("unresolved_tool_identity_{index}")).collect::<Vec<_>>(), "environment":{"PRIVATE":"RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"}}));
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
        Some("rustx.jsonc".into()),
        "pythonSources.python:foo",
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
        ("minimal/workspace", None, "minimal/models.jsonc", 0),
        ("workflow-basic", None, "minimal/models.jsonc", 1),
        ("workspace", Some("rustx.jsonc"), "models.jsonc", 2),
    ] {
        let f = Fixture::new();
        let mut host = f.host.clone();
        host.launch_directory = base.join(workspace);
        std::fs::write(
            host.config_directory.join("models.jsonc"),
            std::fs::read(base.join(model)).unwrap(),
        )
        .unwrap();
        std::fs::write(
            host.config_directory.join("settings.jsonc"),
            std::fs::read(base.join("minimal/settings.jsonc")).unwrap(),
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

#[tokio::test]
async fn cfg235_probe_verifies_mcp_without_business_calls_and_respects_inert_sources() {
    use super::probes::{ProbeState, execute, plan};
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    let fixture = HttpFixture::start(HttpFixtureControl::new()).await;
    let f = Fixture::new();
    f.project(json!({"mcpServers":{
        "enabled":{"enabled":true,"url":fixture.endpoint},
        "disabled":{"enabled":false,"command":"must-never-execute"},
        "unconfigured":{"command":"must-never-execute"}
    },"pythonSources":{"python:optional":"enabled"}}));
    let (report, launch) = super::diagnostics::inspect("doctor", &f.request, &f.host);
    assert_eq!(report.validity, super::diagnostics::Validity::Invalid);
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
    assert_eq!(state("python:optional"), ProbeState::Unavailable);
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
        (json!({"unknownField":true}), "unknownField"),
        (json!({"approvalMode":"full_access"}), "approvalMode"),
        (
            json!({"subagents":{"definitions":["missing"]}}),
            "subagents.definitions.missing",
        ),
    ] {
        f.project(configuration);
        let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
        assert_eq!(report.exit_code(), 2);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.path, field);
        assert!(diagnostic.file.is_some());
        assert_eq!(diagnostic.classification, "error");
        assert_eq!(
            diagnostic.category,
            if field == "subagents.definitions.missing" {
                "resource_missing"
            } else {
                "invalid"
            }
        );
        assert!(!diagnostic.reason.is_empty());
        assert!(!diagnostic.correction.is_empty());
    }
    std::fs::write(f.host.launch_directory.join("rustx.jsonc"), "{\n bad").unwrap();
    let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
    assert_eq!(report.diagnostics[0].line, Some(2));
    assert!(report.diagnostics[0].column.is_some());
}

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
        f.user(json!({"model":{"model":"host/one"}, "mcpServers":{"owned":{
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

#[tokio::test]
async fn cfg235_probe_close_is_awaited_and_failed_close_is_not_verified() {
    use super::probes::{ProbeState, execute, plan};
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    use std::sync::Arc;
    let fixture = HttpFixture::start(HttpFixtureControl::new()).await;
    let f = Fixture::new();
    f.project(json!({"mcpServers":{"owned":{"enabled":true,"url":fixture.endpoint}}}));
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
    let model_path = f.host.config_directory.join("models.jsonc");
    let mut model: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&model_path).unwrap()).unwrap();
    model["providers"]["host"]["apiKey"] = json!("$CFG235_UNSET_CREDENTIAL");
    std::fs::write(&model_path, serde_json::to_vec(&model).unwrap()).unwrap();
    let ((report, _), counts) = super::static_effects::measure(|| {
        super::diagnostics::inspect("config_check", &f.request, &f.host)
    });
    assert_eq!(counts, [0; 13]);
    assert_eq!(
        report.exit_code(),
        3,
        "execution credential and connectivity facts remain unresolved"
    );
    f.request.config = Some("explicit-missing.jsonc".into());
    let (report, _) = super::diagnostics::inspect("config_check", &f.request, &f.host);
    assert_eq!(report.exit_code(), 2);
    assert_eq!(
        report.diagnostics[0].file,
        Some(f.host.launch_directory.join("explicit-missing.jsonc"))
    );
    f.request.config = None;
    let workflow = f
        .host
        .launch_directory
        .join(".agents/workflows/broken.yaml");
    std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
    std::fs::write(&workflow, "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK: [").unwrap();
    f.project(json!({"workflows":{"definitions":["broken"],"main":["broken"]}}));
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
    f.project(json!({"workflows":{"definitions":["broken"],"main":["broken"]}}));
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
async fn cfg235_preparation_requires_authorization_and_uses_existing_python_owner() {
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
    f.project(json!({"pythonSources":{"python:optional":"enabled"}}));
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
        if authorized {
            assert!(
                runner.0.load(Ordering::SeqCst) > before,
                "existing Python owner attempted preparation; failed build was fully retired"
            );
            assert_eq!(result.state, super::probes::ProbeState::Failed);
        } else {
            assert_eq!(runner.0.load(Ordering::SeqCst), before);
            assert_eq!(result.state, super::probes::ProbeState::Unavailable);
        }
        assert!(
            !super::probes::render_results(&results, true)
                .contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
        );
        assert!(!launch.runtime_root.exists());
    }
}

#[tokio::test]
async fn cfg235_probe_credential_use_and_failures_never_leak_values() {
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};
    let fixture = HttpFixture::start(HttpFixtureControl::new()).await;
    let f = Fixture::new();
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{"secret":{"enabled":true,"url":fixture.endpoint,"sensitiveHeaders":{"Authorization":"$CFG235_TOKEN"}}}}));
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
        std::fs::write(host.config_directory.join("models.jsonc"), serde_json::to_vec(&json!({
            "providers": { "host": { "baseUrl": "http://127.0.0.1:9/v1", "apiKey": "fixture", "models": [
                {"id":"one", "protocol":"openai_chat_completions", "contextWindow":128_000, "maxOutputTokens":4096,
                 "capabilities":{"inputModalities":["text"],"outputModalities":["text"],"toolCalls":true,"reasoning":false},"compat":{"chatReasoningReplay":"omit"}},
                {"id":"two", "protocol":"openai_chat_completions", "contextWindow":128_000, "maxOutputTokens":4096,
                 "capabilities":{"inputModalities":["text"],"outputModalities":["text"],"toolCalls":true,"reasoning":false},"compat":{"chatReasoningReplay":"omit"}}
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
            self.host.config_directory.join("settings.jsonc"),
            serde_json::to_vec(&value).unwrap(),
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
            self.host.config_directory.join("subagents")
        } else {
            self.host.launch_directory.join(".agents/subagents")
        };
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{name}.md"));
        std::fs::write(&path, format!("---\n{metadata}\n---\n{body}")).unwrap();
        path
    }
    fn project(&self, value: serde_json::Value) {
        std::fs::write(
            self.host.launch_directory.join("rustx.jsonc"),
            serde_json::to_vec(&value).unwrap(),
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
fn cfg233_configuration_accepts_only_declarative_python_enablement() {
    for project_layer in [false, true] {
        for value in ["enabled", "disabled", "untrusted", "unconfigured"] {
            let f = Fixture::new();
            let mut document = json!({"pythonSources":{"python:x":value}});
            if project_layer {
                f.project(document);
            } else {
                document["model"] = json!({"model":"host/one"});
                f.user(document);
            }
            let result = resolve(&f.request, &f.host);
            assert_eq!(
                result.is_ok(),
                matches!(value, "enabled" | "disabled"),
                "{value}, project={project_layer}: {result:?}"
            );
        }
    }
    let f = Fixture::new();
    f.project(json!({"pythonSources":{"python:x":"enabled"}}));
    f.trust(TrustAction::Revoke);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted"),
        "CFG-01 rejects before a running coordinator could project Untrusted"
    );
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
            json!({"enabled":true,"url":"https://host.invalid/mcp","sensitiveHeaders":{"Authorization":"$HOST_SECRET"}}),
            json!({"enabled":true,"url":"https://project.invalid/mcp"}),
        ),
        (
            json!({"enabled":true,"command":"host-server","sensitiveEnv":{"TOKEN":"$HOST_SECRET"}}),
            json!({"enabled":true,"command":"project-server"}),
        ),
    ] {
        f.user(json!({"model":{"model":"host/one"},"mcpServers":{"service":host}}));
        f.project(json!({"mcpServers":{"service":project}}));
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
        for key in ["sensitiveEnv", "sensitiveHeaders"] {
            let mut forbidden = project.clone();
            forbidden[key] = json!({"TOKEN":"$HOST_SECRET"});
            f.project(json!({"mcpServers":{"service":forbidden}}));
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
    let path = f.host.config_directory.join("models.jsonc");
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["providers"]["host"]["apiKey"] = json!("$CAPTURED_KEY");
    document["providers"]["unused"] = document["providers"]["host"].clone();
    document["providers"]["unused"]["apiKey"] = json!("$UNSET_UNUSED_KEY");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
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
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{
        "credential":{"enabled":true,"command":"/does/not/exist","sensitiveEnv":{"TOKEN":"$REQUIRED_KEY"}},
        "connection":{"enabled":true,"command":"/does/not/exist"},
        "disabled":{"enabled":false,"command":"/does/not/exist","sensitiveEnv":{"TOKEN":"$IGNORED_KEY"}}
    }}));
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let sources = product.runtime().capability().availability();
    let source = |id| CapabilitySourceId::Mcp(crate::runtime::identity::McpServerId::new(id));
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
async fn cfg233_missing_declared_python_is_visible_in_composed_source_status() {
    let f = Fixture::new();
    f.project(json!({"pythonSources":{"python:missing":"enabled", "python:optional":"disabled"}}));
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let sources = product.runtime().capability().availability();
    assert!(matches!(
        sources[&CapabilitySourceId::Mcp(crate::runtime::identity::McpServerId::new(
            "python:missing"
        ))],
        CapabilitySourceState::Unavailable { .. }
    ));
    assert_eq!(
        sources[&CapabilitySourceId::Mcp(crate::runtime::identity::McpServerId::new(
            "python:optional"
        ))],
        CapabilitySourceState::Inactive {
            activation: crate::capabilities::activation::SourceActivation::Disabled
        }
    );
    let response = product.endpoint().handle_request(
        crate::runtime_client::RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        },
    );
    let payload = serde_json::to_string(&response).unwrap();
    assert!(payload.contains("python:missing") && payload.contains("not discovered"));
    assert!(!f.host.launch_directory.join(".agents/tools").exists());
    assert!(
        !launch
            .environment_store_root()
            .read_dir()
            .unwrap()
            .any(|entry| entry.unwrap().path().join("python-tools").exists())
    );
    product.runtime().shutdown().await.unwrap();
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
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{
        "missing":{"enabled":false,"command":"/nonexistent/cfg233","sensitiveEnv":{"TOKEN":"$UNSET"}},
        "offline":{"enabled":false,"url":"http://127.0.0.1:1/mcp","sensitiveHeaders":{"Authorization":"$UNSET"}}
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
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{
        "stdio":{"command":"/bin/sh","args":["-c",format!("touch {}", marker.display())],"sensitiveEnv":{"TOKEN":"$UNSET"}},
        "http":{"url":format!("http://{}/mcp", listener.local_addr().unwrap()),"sensitiveHeaders":{"Authorization":"$UNSET"}}
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
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{"authenticated":{"enabled":true,"url":format!("http://{endpoint}/mcp"),"sensitiveHeaders":{"Authorization":"$AUTH"}}}}));
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
    assert_eq!(builtin.provenance["agentId"], Origin::Builtin);
    f.user(json!({"model":{"model":"host/one"}, "agentId":"user", "context":{"reserveTokens":2000,"keepRecentTokens":6000},
        "defaultTools":["read","bash"],"environment":{"USER_ENTRY":"one","REPLACED":"old"},
        "mcpServers":{"service":{"command":"old-command","args":["old"]},"retained":{"command":"retained"}},
        "subagents":{"definitions":["role"]}
    }));
    f.project(
        json!({"model":{"model":"host/two"},"context":{"reserveTokens":3000},"defaultTools":[],
            "environment":{"REPLACED":"new"}, "mcpServers":{"service":{"command":"new-command"}},
            "subagents":{"definitions":["role"]}
        }),
    );
    let resolved = f.resolve();
    assert_eq!(resolved.config.agent_id.as_str(), "user");
    assert_eq!(resolved.config.context.reserve_tokens, 3000);
    assert_eq!(resolved.config.context.keep_recent_tokens, 6000);
    assert!(resolved.config.default_tools.is_empty());
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
        &f.host.config_directory.join("subagents/role.md")
    );
    assert!(matches!(
        resolved.provenance["context.keepRecentTokens"],
        Origin::User { .. }
    ));
    assert!(matches!(
        resolved.provenance["context.reserveTokens"],
        Origin::Project { .. }
    ));
    f.request.model = Some("host/one".into());
    f.request.tools = Some(vec!["read".into(), "bash".into()]);
    f.request.exclude_tools = Some(vec!["read".into()]);
    let cli = f.resolve();
    assert_eq!(cli.config.model.model.to_string(), "host/one");
    assert!(matches!(cli.provenance["model.model"], Origin::Cli { .. }));
    assert!(matches!(cli.provenance["excludeTools"], Origin::Cli { .. }));
    assert_eq!(cli.tools, Some(vec!["read".into(), "bash".into()]));
    f.project(json!({"environment":{},"mcpServers":{},"subagents":{"definitions":[]}}));
    let empty = f.resolve();
    assert!(empty.config.environment.is_empty());
    assert!(empty.config.mcp_servers.is_empty());
    assert!(empty.config.subagents.definitions.is_empty());
    assert_eq!(empty.config.default_tools, ["read", "bash"]);
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
        "runtimeRoot",
        "workspace",
    ] {
        f.project(json!({field:"../../host-authority"}));
        for explicit in [false, true] {
            f.request.config = explicit.then(|| "./rustx.jsonc".into());
            let error = resolve(&f.request, &f.host).unwrap_err();
            assert!(
                error.contains(field) && error.contains("forbidden"),
                "{error}"
            );
        }
    }
}

#[test]
fn optional_explicit_malformed_unknown_and_null_are_distinct() {
    let mut f = Fixture::new();
    assert_eq!(f.resolve().config.agent_id.as_str(), "rustx");
    f.request.config = Some("missing.jsonc".into());
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("cannot read")
    );
    f.request.config = None;
    for value in [
        "{",
        "{\"unknown\":1}",
        "{\"skills\":null}",
        "{\"context\":{\"typo\":1}}",
        "{\"defaultTools\":false}",
    ] {
        std::fs::write(f.host.launch_directory.join("rustx.jsonc"), value).unwrap();
        assert!(resolve(&f.request, &f.host).is_err(), "{value}");
    }
    f.project(json!({"context":{"summaryOutputCap":null},"skills":[]}));
    assert_eq!(f.resolve().config.context.summary_output_cap, None);
    std::fs::remove_file(f.host.config_directory.join("settings.jsonc")).unwrap();
    f.request.model = Some("host/one".into());
    assert!(
        resolve(&f.request, &f.host).is_ok(),
        "optional user settings"
    );
    std::fs::write(
        f.host.launch_directory.join("rustx.jsonc"),
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
    f.user(json!({"model":{"model":"host/one"}, "skills":["user-skills"], "subagents":{"definitions":["user"]}}));
    f.project(json!({"subagents":{"definitions":["user","project"]}}));
    let resolved = f.resolve();
    assert_eq!(
        resolved.config.skills,
        [f.host.config_directory.join("user-skills")]
    );
    f.request.skill_paths = vec!["cli-skill".into()];
    assert_eq!(
        f.resolve().config.skills,
        [f.host.launch_directory.join("cli-skill")]
    );
    for (name, source) in &resolved.role_sources {
        let base = if name.as_str() == "user" {
            f.host.config_directory.join("subagents")
        } else {
            f.host.launch_directory.join(".agents/subagents")
        };
        assert_eq!(source.selected, base.join(format!("{name}.md")));
    }
    f.request.config = Some("replacement.jsonc".into());
    std::fs::write(f.host.launch_directory.join("replacement.jsonc"), "{}").unwrap();
    assert_eq!(
        f.resolve().config.subagents.definitions.len(),
        1,
        "--config replaces the project slot"
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
    std::fs::write(sub.join("rustx.jsonc"), "{}").unwrap();
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
    f.project(json!({"subagents":{"definitions":["other"]}}));
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
    f.user(json!({"model":{"model":"host/two"},"agentId":"edited"}));
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
    assert_eq!(launch.config.model.model.to_string(), "host/one");
    assert_eq!(launch.config.agent_id.as_str(), "rustx");
    assert!(launch.config.mcp_servers.is_empty());
    assert!(launch.config.skills.is_empty());
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
    f.project(json!({"agentId":""}));
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
    let model_path = f.host.config_directory.join("models.jsonc");
    let mut catalog: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&model_path).unwrap()).unwrap();
    catalog["providers"]["host"]["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("protocol");
    std::fs::write(model_path, serde_json::to_vec(&catalog).unwrap()).unwrap();
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
    f.user(json!({"models":"models.jsonc","runtimeRoot":"private","model":{"model":"host/one"}}));
    let user = f.resolve();
    assert_eq!(user.runtime_root, f.host.config_directory.join("private"));
    assert_eq!(
        user.provenance["models"],
        Origin::User {
            document: f.host.config_directory.join("settings.jsonc"),
            base: f.host.config_directory.clone()
        }
    );
    assert!(matches!(
        user.provenance["runtimeRoot"],
        Origin::User { .. }
    ));
    f.request.runtime_root = Some("../cli-state".into());
    f.request.models = Some(f.host.config_directory.join("models.jsonc"));
    let cli = f.resolve();
    assert_eq!(cli.runtime_root, f.root.path().join("cli-state"));
    assert!(matches!(cli.provenance["runtimeRoot"], Origin::Cli { .. }));
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
    f.user(json!({"runtimeRoot":"private","model":null,"context":false}));
    f.project(json!({"trust":true}));
    std::fs::remove_file(f.host.config_directory.join("models.jsonc")).unwrap();
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
    std::os::unix::fs::symlink("missing.jsonc", f.host.launch_directory.join("rustx.jsonc"))
        .unwrap();
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
        f.host.launch_directory.join("rustx.jsonc"),
        r#"{"subagents":{"definitions":["role","role"]}}"#,
    )
    .unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"model":{"model":"host/one"},"context":{"unknown":true}}));
    f.project(json!({"context":{"reserveTokens":7}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
    f.user(json!({"model":{"model":"host/one"},"subagents":{"definitions":["role","role"]}}));
    f.project(json!({"subagents":{"definitions":[]}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"model":{"model":"host/one"},"schemaVersion":7}));
    f.project(json!({"schemaVersion":8}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("schemaVersion 7")
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
    let user = json!({"model":{"model":"host/one"},"approvalMode":"full_access",
        "nativeTools":{"bash":{"approval":"always"}},
        "mcpServers":{"server":{"command":"fixture"}},
        "mcpToolPolicies":{"server":{"approval":"always"}}});
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
        json!({"approvalMode":"full_access"}),
        json!({"nativeTools":{"bash":{"approval":"never"}}}),
        json!({"mcpToolPolicies":{"server":{"approval":"never"}}}),
        json!({"nativeTools":{}}),
        json!({"nativeTools":{"bash":{"execution":"background_only"}}}),
    ] {
        f.project(document);
        // Explicit CLI selection cannot mask a forbidden project declaration.
        f.request.model = Some("host/two".into());
        for explicit in [false, true] {
            f.request.config = explicit.then(|| "rustx.jsonc".into());
            let error = resolve(&f.request, &f.host).unwrap_err();
            assert!(
                error.contains("forbidden") && error.contains("approval authority"),
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
        json!({"subagents":{"definitions":["x"]}})
    };
    for document in [
        role(json!(&resource)),
        json!({"skills":[&other]}),
        role(json!(&resource)),
        json!({"mcpServers":{"x":{"command":&resource}}}),
        json!({"mcpServers":{"x":{"command":"fixture","cwd":&other}}}),
    ] {
        f.project(document);
        let error = resolve(&f.request, &f.host).unwrap_err();
        assert!(error.contains("outside trusted workspace"), "{error}");
    }
    // An external --config is inert input, not a grant for its neighboring files.
    std::fs::write(
        other.join("config.jsonc"),
        role(json!("../B/resource")).to_string(),
    )
    .unwrap();
    f.request.config = Some(other.join("config.jsonc"));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    f.request.config = None;
    f.project(json!({}));
    std::fs::remove_file(f.host.launch_directory.join(".agents/subagents/x.md")).unwrap();
    f.role(true, "x", json!({"description":"host"}), "user-owned bytes");
    f.user(json!({"model":{"model":"host/one"},"subagents":{"definitions":["x"]}}));
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
    f.project(json!({"subagents":{"definitions":["x"]}}));
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
    f.project(json!({"subagents":{"definitions":["x"]}}));
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
    // Removing the offending declaration permits a new candidate: stale launch
    // paths must not become a second resource-generation authority.
    f.project(json!({"subagents":{"definitions":[]}}));
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
        json!({"mcpServers":{"x":{"command":"link/file"}}}),
        json!({"mcpServers":{"x":{"command":"fixture","cwd":"link"}}}),
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
    f.project(json!({"workflows":{"definitions":["escape"]}}));
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
    f.project(json!({"mcpServers":{"x":{"enabled":true,"command":"./server"}}}));
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
    f.project(json!({"mcpServers":{"x":{"command":"./server","resourceWorkspace":other}}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
}
