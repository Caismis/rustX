use std::path::{Path, PathBuf};

use rustx::local_runtime::LocalSessionConfig;
use rustx::local_runtime::config::McpTransportDocument;
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef, ReasoningProfileId};
use rustx::model::session::SummaryModelPolicy;
use rustx::model::types::ModelProtocol;
use rustx::tools::Workspace;
use rustx::tools::python::PythonToolDiscovery;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy};

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime")
}

fn read_example(name: &str) -> Vec<u8> {
    std::fs::read(examples_root().join(name)).expect("committed example file")
}

#[test]
fn committed_model_example_uses_the_production_catalog_contract() {
    let catalog = ModelCatalog::from_json_slice(&read_example("models.json"))
        .expect("models.json must parse through ModelCatalog");
    let model_ref = ModelRef::parse("example/demo-model").expect("canonical model reference");
    let model = catalog.model(&model_ref).expect("example model exists");
    assert_eq!(model.protocol, ModelProtocol::OpenAiChatCompletions);
    assert_eq!(model.context_window, 128_000);
    assert_eq!(model.max_output_tokens, 4_096);
    assert!(model.capabilities.tool_calls);
    assert!(model.capabilities.reasoning);
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
fn committed_baseline_session_selects_a_catalog_model_and_configures_runtime_policy() {
    let catalog = ModelCatalog::from_json_slice(&read_example("models.json"))
        .expect("models.json must parse through ModelCatalog");
    let session = LocalSessionConfig::from_json_slice(&read_example("session.json"))
        .expect("session.json must parse through LocalSessionConfig");

    assert_eq!(session.model.model.to_string(), "example/demo-model");
    catalog
        .model(&session.model.model)
        .expect("session model must exist in the example catalog");
    assert_eq!(
        session.model.reasoning_profile.as_ref().unwrap().as_str(),
        "off"
    );
    assert_eq!(
        session.model.request_params["temperature"],
        serde_json::json!(0.1)
    );
    assert_eq!(session.model.max_output_tokens, Some(2_048));
    assert_eq!(session.model.summary_model, SummaryModelPolicy::Session);
    assert_eq!(session.context.reserve_tokens, 4_096);
    assert_eq!(session.context.keep_recent_tokens, 12_000);
    assert_eq!(session.context.summary_output_cap, Some(1_024));
    assert_eq!(session.mcp_servers.len(), 0);
    assert_eq!(session.environment["RUSTX_EXAMPLE_MODE"], "local-runtime");

    let policies = session.native_tools.to_policies();
    assert_eq!(policies.read.execution, ToolExecutionPolicy::ForegroundOnly);
    assert_eq!(policies.read.concurrency, ToolConcurrencyPolicy::Parallel);
    assert_eq!(
        policies.bash.execution,
        ToolExecutionPolicy::ModelSelectable
    );
    assert_eq!(policies.bash.concurrency, ToolConcurrencyPolicy::Sequential);
}

#[test]
fn committed_mcp_session_exposes_both_current_transport_shapes_and_policies() {
    let session = LocalSessionConfig::from_json_slice(&read_example("session.with-mcp.json"))
        .expect("session.with-mcp.json must parse through LocalSessionConfig");
    assert_eq!(session.mcp_servers.len(), 2);

    let stdio = &session.mcp_servers[0];
    assert_eq!(stdio.server_id.as_str(), "example-stdio");
    match &stdio.transport {
        McpTransportDocument::Stdio {
            program,
            args,
            cwd,
            environment,
        } => {
            assert_eq!(program, "replace-with-real-mcp-server");
            assert_eq!(args, &["--stdio"]);
            assert_eq!(cwd.as_deref(), Some(Path::new(".")));
            assert_eq!(environment["MCP_EXAMPLE_MODE"], "replace-me");
        }
        McpTransportDocument::StreamableHttp { .. } => panic!("expected stdio transport"),
    }
    let stdio_policy = stdio.policy.to_policy();
    assert_eq!(stdio_policy.execution, ToolExecutionPolicy::ModelSelectable);
    assert_eq!(stdio_policy.concurrency, ToolConcurrencyPolicy::Sequential);

    let http = &session.mcp_servers[1];
    assert_eq!(http.server_id.as_str(), "example-http");
    match &http.transport {
        McpTransportDocument::StreamableHttp { endpoint, headers } => {
            assert_eq!(endpoint, "https://mcp.example.invalid/mcp");
            assert_eq!(headers["Authorization"], "Bearer replace-me");
        }
        McpTransportDocument::Stdio { .. } => panic!("expected streamable HTTP transport"),
    }
    let http_policy = http.policy.to_policy();
    assert_eq!(http_policy.execution, ToolExecutionPolicy::ForegroundOnly);
    assert_eq!(http_policy.concurrency, ToolConcurrencyPolicy::Parallel);
}

#[test]
fn committed_echo_tool_is_found_by_production_python_discovery() {
    let workspace_path = examples_root().join("workspace");
    let workspace = Workspace::new(&workspace_path).expect("example workspace");
    let packages = PythonToolDiscovery::new(&workspace)
        .discover()
        .expect("example Python package must be discoverable");
    assert_eq!(packages.len(), 1);

    let package = &packages[0];
    assert_eq!(package.name, "echo");
    assert_eq!(
        package.description,
        "Return the message supplied to the example tool."
    );
    assert_eq!(package.entrypoint, "tool:main");
    assert_eq!(
        package.input_schema,
        serde_json::json!({
            "type": "object",
            "required": ["message"],
            "properties": {"message": {"type": "string"}},
            "additionalProperties": false
        })
    );
    assert_eq!(
        package.policy.execution,
        ToolExecutionPolicy::ForegroundOnly
    );
    assert_eq!(
        package.policy.concurrency,
        ToolConcurrencyPolicy::Sequential
    );
    for required in [
        "TOOL.toml",
        "input.schema.json",
        "pyproject.toml",
        "uv.lock",
        "tool.py",
    ] {
        assert!(
            package
                .files
                .iter()
                .any(|(path, _)| path == Path::new(required)),
            "discovered package must include {required}"
        );
    }
}
