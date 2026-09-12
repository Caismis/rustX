use std::path::{Path, PathBuf};

use rustx::local_runtime::CurrentRuntimeConfig;
use rustx::model::catalog::{
    ChatReasoningReplay, MapCredentialEnvironment, ModelCatalog, ModelRef, ReasoningProfileId,
};
use rustx::model::session::SummaryModelPolicy;
use rustx::model::types::ModelProtocol;
use rustx::skills::{SkillDiscovery, SkillDiscoveryConfig};
use rustx::tools::Workspace;
use rustx::tools::types::{ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy};

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime")
}

fn read_example(name: &str) -> Vec<u8> {
    std::fs::read(examples_root().join(name)).expect("committed example file")
}

#[test]
fn committed_configuration_examples_are_commented_toml() {
    for name in ["models.toml", "rustx.toml"] {
        let bytes = read_example(name);
        let text = String::from_utf8(bytes.clone()).expect("committed example is UTF-8");
        assert!(
            text.contains("# "),
            "{name} must explain its fields in place"
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
            "{name} must exercise the TOML reader, not merely be valid JSON"
        );
    }
}

#[test]
fn committed_model_example_uses_the_production_catalog_contract() {
    let catalog = ModelCatalog::from_toml_slice(&read_example("models.toml"))
        .expect("models.toml must parse through ModelCatalog");
    let model_ref = ModelRef::parse("example/demo-model").expect("canonical model reference");
    let model = catalog.model(&model_ref).expect("example model exists");
    assert_eq!(model.protocol, ModelProtocol::OpenAiChatCompletions);
    assert_eq!(model.context_window, 128_000);
    assert_eq!(model.max_output_tokens, 4_096);
    assert!(model.capabilities.tool_calls);
    assert!(model.capabilities.reasoning);
    assert_eq!(
        model.compat.chat_reasoning_replay,
        Some(ChatReasoningReplay::Reasoning)
    );
    assert_eq!(model.request_params["temperature"], serde_json::json!(0.2));

    let reasoning = model.reasoning.as_ref().expect("reasoning profiles");
    assert_eq!(reasoning.default_profile.as_str(), "off");
    assert_eq!(reasoning.profiles.len(), 2);
    let off = ReasoningProfileId::new("off");
    let on = ReasoningProfileId::new("on");
    assert!(!reasoning.profiles[&off].enabled);
    assert!(reasoning.profiles[&on].enabled);
    assert_eq!(
        reasoning.profiles[&on].request_params["reasoning_effort"],
        serde_json::json!("low")
    );

    let resolved = catalog
        .resolve(&MapCredentialEnvironment::new([(
            "RUSTX_EXAMPLE_API_KEY".to_owned(),
            "example-secret".to_owned(),
        )]))
        .expect("example credential reference resolves");
    let provider = resolved
        .provider(&rustx::model::catalog::ProviderId::new("example"))
        .expect("example provider exists");
    assert_eq!(provider.base_url(), "https://api.example.invalid/v1");
    assert_eq!(
        provider.credential_source(),
        rustx::model::catalog::CredentialSourceView::Environment {
            variable: "RUSTX_EXAMPLE_API_KEY".to_owned()
        }
    );
}

#[test]
fn committed_runtime_config_selects_a_catalog_model_and_configures_runtime_policy() {
    let catalog = ModelCatalog::from_toml_slice(&read_example("models.toml"))
        .expect("models.toml must parse through ModelCatalog");
    let config = CurrentRuntimeConfig::from_toml_slice(&read_example("rustx.toml"))
        .expect("rustx.toml must parse through CurrentRuntimeConfig");

    assert_eq!(config.model.model.to_string(), "example/demo-model");
    catalog
        .model(&config.model.model)
        .expect("configured model must exist in the example catalog");
    assert_eq!(
        config.model.reasoning_profile.as_ref().unwrap().as_str(),
        "off"
    );
    assert_eq!(
        config.model.request_params["temperature"],
        serde_json::json!(0.1)
    );
    assert_eq!(config.model.max_output_tokens, Some(2_048));
    assert_eq!(config.model.summary_model, SummaryModelPolicy::Session);
    assert_eq!(config.context.reserve_tokens, 4_096);
    assert_eq!(config.context.keep_recent_tokens, 12_000);
    assert_eq!(config.context.summary_output_cap, Some(1_024));
    assert!(config.extensions.agent_status.time.enabled);
    assert_eq!(
        config.extensions.agent_status.time.timezone,
        Some(chrono_tz::Asia::Tokyo)
    );
    assert!(config.extensions.agent_status.background.enabled);
    assert!(config.mcp_servers.is_empty());
    assert!(config.mcp_tool_policies.is_empty());
    assert_eq!(config.environment["RUSTX_EXAMPLE_MODE"], "local-runtime");
    assert!(config.default_tools.iter().any(|name| name == "subagent"));
    assert_eq!(
        std::fs::read_dir(examples_root().join("workspace/.agents/agents"))
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "toml"))
            .count(),
        4
    );
    assert_eq!(
        config
            .subagents
            .main
            .iter()
            .map(rustx::runtime::subagent::SubagentName::as_str)
            .collect::<Vec<_>>(),
        vec!["navigator"]
    );
    assert_eq!(
        config
            .subagents
            .workflow
            .iter()
            .map(rustx::runtime::subagent::SubagentName::as_str)
            .collect::<Vec<_>>(),
        vec!["reviewer", "planner", "implementer"]
    );
    assert_eq!(
        config
            .workflows
            .main
            .iter()
            .map(rustx::runtime::workflow::WorkflowId::as_str)
            .collect::<Vec<_>>(),
        vec!["parallel_review", "implement_and_review"]
    );

    let host = CurrentRuntimeConfig::from_toml_slice(&read_example("settings.toml")).unwrap();
    let policies = host.native_tools.to_policies();
    assert_eq!(policies.read.execution, ToolExecutionPolicy::ForegroundOnly);
    assert_eq!(policies.read.concurrency, ToolConcurrencyPolicy::Parallel);
    assert_eq!(
        policies.bash.execution,
        ToolExecutionPolicy::ModelSelectable
    );
    assert_eq!(policies.grep.execution, ToolExecutionPolicy::ForegroundOnly);
    assert_eq!(policies.edit.approval, ToolApprovalPolicy::Always);
    assert_eq!(policies.bash.concurrency, ToolConcurrencyPolicy::Sequential);
    assert_eq!(policies.bash.approval, ToolApprovalPolicy::Always);
}

#[test]
fn committed_echo_package_is_discovered_by_production_python_discovery() {
    let workspace_path = examples_root().join("workspace");
    let workspace = Workspace::new(&workspace_path).expect("example workspace");
    let discovered = rustx::tools::python::discover_python_packages(&workspace)
        .expect("example tool packages must be discoverable");
    assert_eq!(discovered.len(), 1);
    let echo = &discovered[0];
    assert_eq!(echo.server_id.as_str(), "python:echo");
    let package = echo
        .outcome
        .as_ref()
        .expect("the committed echo package must be valid");
    assert_eq!(package.name, "echo");
    let file_names: Vec<&str> = package
        .files
        .iter()
        .map(|(path, _)| path.to_str().expect("UTF-8 package path"))
        .collect();
    assert!(file_names.contains(&"server.py"));
    assert!(file_names.contains(&"requirements.txt"));
    // The example declares no dependencies; rustX pins FastMCP itself.
    assert!(package.requirements.is_empty());
}

#[test]
fn committed_example_skill_is_found_by_project_agents_discovery() {
    let workspace_path = examples_root().join("workspace");
    let workspace = Workspace::new(&workspace_path).expect("example workspace");
    let packages = SkillDiscovery::with_config(
        &workspace,
        SkillDiscoveryConfig {
            automatic_roots: vec![workspace_path.join(".agents/skills")],
            explicit_paths: Vec::new(),
        },
    )
    .discover()
    .expect("example Skill must be discoverable");
    assert_eq!(
        packages
            .iter()
            .map(rustx::skills::SkillPackage::name)
            .collect::<Vec<_>>(),
        vec!["review-guidance"]
    );
    assert!(packages[0].description().contains("bounded"));
}

#[test]
fn every_shipped_workflow_is_discovered_and_compiles() {
    use rustx::runtime::workflow::{WorkflowDefinition, WorkflowProgram};
    let config = CurrentRuntimeConfig::from_toml_slice(&read_example("rustx.toml")).unwrap();
    let profiles = config.subagents.workflow.iter().cloned().collect();
    let directory = examples_root().join("workspace/.agents/workflows");
    let mut paths = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), 2);
    for path in paths {
        let id = rustx::runtime::workflow::WorkflowId::parse(
            path.file_stem().unwrap().to_str().unwrap(),
        )
        .unwrap();
        let definition: WorkflowDefinition =
            serde_yaml::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        WorkflowProgram::compile(id.clone(), definition, &profiles)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
    for profile in &config.subagents.workflow {
        assert!(
            examples_root()
                .join(format!("workspace/.agents/agents/{profile}.toml"))
                .is_file()
        );
    }
}

#[test]
fn reference_authoring_errors_fail_before_execution() {
    use rustx::runtime::workflow::{WorkflowDefinition, WorkflowId, WorkflowProgram};
    let config = CurrentRuntimeConfig::from_toml_slice(&read_example("rustx.toml")).unwrap();
    let profiles = config.subagents.workflow.iter().cloned().collect();
    let source: serde_json::Value = serde_yaml::from_slice(&read_example(
        "workspace/.agents/workflows/implement_and_review.yaml",
    ))
    .unwrap();
    let cases = [
        (
            "/block/nodes/plan/input/brief/path",
            serde_json::json!(["args", "missing"]),
        ),
        (
            "/block/nodes/plan/profile",
            serde_json::json!("unavailable_profile"),
        ),
        (
            "/block/nodes/repair/body/nodes/check/selector/name",
            serde_json::json!("unadmitted_tool"),
        ),
        (
            "/block/nodes/repair/body/nodes/implement/input/plan/path",
            serde_json::json!(["plan"]),
        ),
        ("/block/nodes/repair/max_iterations", serde_json::json!(0)),
        (
            "/block/nodes/plan/input/brief/path",
            serde_json::json!(["repair"]),
        ),
        (
            "/block/nodes/repair/body/entry",
            serde_json::json!("missing_node"),
        ),
    ];
    for (pointer, value) in cases {
        let mut invalid = source.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        let definition: WorkflowDefinition = serde_json::from_value(invalid).unwrap();
        let error = WorkflowProgram::compile(
            WorkflowId::parse("implement_and_review").unwrap(),
            definition,
            &profiles,
        )
        .unwrap_err();
        let diagnostic = error.to_string();
        let expected_node = if pointer.ends_with("/entry") {
            "missing_node"
        } else if pointer.contains("/check/") {
            "check"
        } else if pointer.contains("/implement/") {
            "implement"
        } else if pointer.ends_with("max_iterations") {
            "repair"
        } else {
            "plan"
        };
        assert!(
            diagnostic.contains(expected_node),
            "{pointer}: {diagnostic}"
        );

        assert!(
            !diagnostic.is_empty() && diagnostic.len() < 2048,
            "{pointer}: {diagnostic}"
        );
    }
}
