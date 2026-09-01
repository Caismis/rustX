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
fn committed_configuration_examples_are_commented_jsonc() {
    for name in ["models.jsonc", "rustx.jsonc"] {
        let bytes = read_example(name);
        let text = String::from_utf8(bytes.clone()).expect("committed example is UTF-8");
        assert!(
            text.contains("//"),
            "{name} must explain its fields in place"
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
            "{name} must exercise the JSONC reader, not merely be valid JSON"
        );
    }
}

#[test]
fn committed_model_example_uses_the_production_catalog_contract() {
    let catalog = ModelCatalog::from_jsonc_slice(&read_example("models.jsonc"))
        .expect("models.jsonc must parse through ModelCatalog");
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
    let catalog = ModelCatalog::from_jsonc_slice(&read_example("models.jsonc"))
        .expect("models.jsonc must parse through ModelCatalog");
    let config = CurrentRuntimeConfig::from_jsonc_slice(&read_example("rustx.jsonc"))
        .expect("rustx.jsonc must parse through CurrentRuntimeConfig");

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
    assert!(config.agent_status.time.enabled);
    assert_eq!(
        config.agent_status.time.timezone,
        Some(chrono_tz::Asia::Tokyo)
    );
    assert!(config.agent_status.background.enabled);
    assert!(config.mcp_servers.is_empty());
    assert!(config.mcp_tool_policies.is_empty());
    assert_eq!(config.environment["RUSTX_EXAMPLE_MODE"], "local-runtime");
    assert!(config.default_tools.iter().any(|name| name == "subagent"));
    assert_eq!(config.subagents.definitions.len(), 2);
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
        vec!["reviewer"]
    );
    assert_eq!(
        config
            .workflows
            .definitions
            .iter()
            .map(rustx::runtime::workflow::WorkflowId::as_str)
            .collect::<Vec<_>>(),
        vec!["review_pr", "parallel_review"]
    );
    assert_eq!(config.workflows.main, config.workflows.definitions);

    let policies = config.native_tools.to_policies();
    assert_eq!(policies.read.execution, ToolExecutionPolicy::ForegroundOnly);
    assert_eq!(policies.read.concurrency, ToolConcurrencyPolicy::Parallel);
    assert_eq!(
        policies.bash.execution,
        ToolExecutionPolicy::ModelSelectable
    );
    assert_eq!(policies.grep.execution, ToolExecutionPolicy::BackgroundOnly);
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
