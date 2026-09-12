//! Issue #42: the Rust-side local runtime composition owner.
//!
//! One process owns one conversation session, and that session owns exactly
//! one `ConversationToolRuntime` identity, one `CapabilityCoordinator` over
//! the same conversation and workspace, one committed initial capability
//! revision, and one `RuntimeClientHost`. There is deliberately no second
//! tool plane in these tests: the assertions run against the real composed
//! runtime.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use rustx::local_runtime::composition::{LocalConversationRuntime, LocalRuntimeDependencies};
use rustx::model::ModelProtocol;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION;
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};

/// A catalog whose credential comes from the environment, exercising the
/// startup credential-resolution path.
const MODELS_TOML: &str = r#"[providers.local]
base_url = "https://local.fixture.invalid/v1"
api_key = "$RUSTX_TEST_MODEL_KEY"

[[providers.local.models]]
id = "composed-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
request_params_json = "{\"temperature\": 0.3}"

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"
"#;

const RUNTIME_CONFIG_TOML: &str = r#"agent_id = "agent-composed"

[model]
model = "local/composed-model"

[context]
reserve_tokens = 1024
keep_recent_tokens = 8192

[native_tools.bash]
execution = "model_selectable"
concurrency = "sequential"

[environment]
RUSTX_FIXTURE = "1"
"#;

/// Writes the startup files into a temporary root and returns the explicit
/// paths.
fn startup(root: &std::path::Path, models: &str, config: &str) -> LaunchFixture {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let models_path = root.join("models.toml");
    let config_path = root.join("rustx.toml");
    std::fs::write(&models_path, models).expect("models.toml");
    crate::launch_fixture::write_documents(&config_path, config, &["native_tools"]);
    LaunchFixture {
        models: models_path,
        config: config_path,
        skill_paths: Vec::new(),
        no_skills: false,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: rustx::local_runtime::StartupSession::Empty,
        session_name: None,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.join("private"),
    }
}

/// Composition dependencies with an explicit credential environment. Model
/// bindings are constructed by production from the catalog's protocol and
/// endpoint; this test exercises startup without invoking a model turn.
fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_TEST_MODEL_KEY".to_owned(),
            "composed-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

/// The real composition owns exactly one of each semantic owner, and the
/// initial capability candidate is committed before anything can serve
/// protocol input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn composition_owns_one_conversation_domain() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(root.path(), MODELS_TOML, RUNTIME_CONFIG_TOML);
    let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies())
        .await
        .expect("composition succeeds");

    // One conversation identity, shared by the tool runtime, the capability
    // coordinator, and the host.
    let conversation = runtime.tool_runtime().conversation_id().clone();
    assert_eq!(conversation.as_str(), "conversation-standalone");
    assert_eq!(runtime.host().conversation_id(), &conversation);
    let capability = runtime.capability().current_snapshot();
    assert_eq!(capability.conversation_id(), &conversation);
    assert_eq!(
        capability.workspace_root(),
        runtime.tool_runtime().workspace().root(),
        "the coordinator anchors on the same workspace as the tool runtime"
    );

    // The initial capability candidate was committed *before* the host was
    // constructed, so the very first thing a protocol client can observe —
    // the `initialize` snapshot — already carries the active revision and the
    // composed tool catalog. An uncommitted candidate would leave it empty.
    let (_attachment, result) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let RuntimeClientResult::Initialized { snapshot, .. } = result else {
        panic!("initialize returns the snapshot");
    };
    assert_eq!(
        snapshot.capabilities.revision,
        capability.revision(),
        "the protocol view is the committed active revision"
    );
    assert!(
        !snapshot.capabilities.tools.is_empty(),
        "the initial capability set is committed before serving"
    );

    // The base registry really contains the native tool plane, including the
    // runtime intrinsic bound to *this* conversation's background registry.
    let names: Vec<&str> = snapshot
        .capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    for expected in ["execution", "read", "write", "edit", "glob", "grep", "bash"] {
        assert!(
            names.contains(&expected),
            "the native tool {expected} must be composed: {names:?}"
        );
    }

    // The configured per-tool policy reached the registered definition.
    let bash = snapshot
        .capabilities
        .tools
        .iter()
        .find(|tool| tool.name == "bash")
        .expect("bash is registered");
    assert_eq!(
        bash.execution_policy,
        rustx::tools::types::ToolExecutionPolicy::ModelSelectable,
        "the session's native tool policy is honoured"
    );
    // The runtime intrinsic keeps its fixed policy regardless of configuration.
    let background = snapshot
        .capabilities
        .tools
        .iter()
        .find(|tool| tool.name == "execution")
        .expect("execution is registered");
    assert_eq!(
        background.execution_policy,
        rustx::tools::types::ToolExecutionPolicy::ForegroundOnly
    );

    // `execution` dispatches into *this* conversation's background
    // registry: the composed registry and the host's projection agree.
    assert!(
        runtime
            .tool_runtime()
            .background()
            .all_snapshots()
            .is_empty(),
        "a freshly composed conversation has no background executions"
    );
    assert_eq!(snapshot.background.len(), 0);

    // The session model resolved through the catalog, credential and all.
    assert_eq!(
        snapshot
            .model
            .as_ref()
            .unwrap()
            .configured
            .model
            .to_string(),
        "local/composed-model"
    );
    assert_eq!(
        snapshot.model.as_ref().unwrap().effective.context_window,
        128_000
    );
    assert_eq!(
        snapshot.model.as_ref().unwrap().effective.protocol,
        ModelProtocol::OpenAiChatCompletions
    );
    assert_eq!(
        snapshot.model.as_ref().unwrap().effective.request_params["temperature"],
        serde_json::json!(0.3)
    );
    let serialized = serde_json::to_string(&snapshot).expect("serialize");
    assert!(
        !serialized.contains("composed-secret"),
        "the resolved credential never reaches a client-visible value"
    );
}

/// The runtime-private roots are disjoint from the model-visible workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_private_roots_stay_disjoint_from_the_workspace() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(root.path(), MODELS_TOML, RUNTIME_CONFIG_TOML);
    assert!(!paths.artifacts_root().starts_with(&paths.workspace));
    assert!(!paths.environment_store_root().starts_with(&paths.workspace));
    assert_ne!(paths.artifacts_root(), paths.environment_store_root());

    let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies())
        .await
        .expect("composition succeeds");
    let workspace_root = runtime.tool_runtime().workspace().root().to_path_buf();
    let artifacts = std::fs::canonicalize(paths.artifacts_root()).expect("artifact root exists");
    assert!(
        !artifacts.starts_with(&workspace_root),
        "the artifact root must never live inside the model-visible workspace"
    );

    // Composing with an artifact root *inside* the workspace is rejected by
    // the existing ownership check.
    let overlapping = LaunchFixture {
        runtime_root: paths.workspace.join("private"),
        ..paths
    };
    assert!(overlapping.try_resolve().unwrap_err().contains("disjoint"));
}

/// Every startup configuration failure is surfaced before any runtime exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_configuration_failures_are_explicit() {
    let root = tempfile::tempdir().expect("temp root");

    // A missing catalog file.
    let paths = startup(root.path(), MODELS_TOML, RUNTIME_CONFIG_TOML);
    let missing = LaunchFixture {
        models: root.path().join("absent.json"),
        ..paths.clone()
    };
    assert!(missing.try_resolve().unwrap_err().contains("cannot read"));

    // A catalog without an explicit base URL.
    let no_base = MODELS_TOML.replace("base_url = \"https://local.fixture.invalid/v1\"", "");
    let paths = startup(&root.path().join("no-base"), &no_base, RUNTIME_CONFIG_TOML);
    assert!(paths.try_resolve().unwrap_err().contains("base_url"));

    // An unresolved environment credential names only the variable.
    let paths = startup(
        &root.path().join("no-env"),
        MODELS_TOML,
        RUNTIME_CONFIG_TOML,
    );
    let error = LocalConversationRuntime::compose(
        &(paths).resolve(),
        &LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::default())),
            ..LocalRuntimeDependencies::default()
        },
    )
    .await
    .expect_err("an unresolved credential fails startup");
    assert!(error.to_string().contains("RUSTX_TEST_MODEL_KEY"));
    assert!(!error.to_string().contains("composed-secret"));

    // A session selecting a model the catalog does not declare.
    let bad_config = RUNTIME_CONFIG_TOML.replace("local/composed-model", "local/absent-model");
    let paths = startup(&root.path().join("bad-model"), MODELS_TOML, &bad_config);
    assert!(
        paths
            .try_resolve()
            .unwrap_err()
            .contains("unknown catalog model")
    );

    // A current runtime config with an unknown field.
    let bad_config = format!("future_knob = true\n{RUNTIME_CONFIG_TOML}");
    let paths = startup(&root.path().join("bad-config"), MODELS_TOML, &bad_config);
    assert!(paths.try_resolve().unwrap_err().contains("unknown field"));
}

/// The composed runtime serves the real Runtime Client endpoint, and the
/// endpoint is derived from the one host rather than a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_endpoint_speaks_for_the_one_composed_host() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(root.path(), MODELS_TOML, RUNTIME_CONFIG_TOML);
    let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies())
        .await
        .expect("composition succeeds");

    let endpoint = runtime.endpoint();
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    let Some(RuntimeClientResult::Initialized {
        conversation_id,
        agent_id,
        ..
    }) = response.result
    else {
        panic!("the endpoint initializes: {response:?}");
    };
    assert_eq!(conversation_id.as_str(), "conversation-standalone");
    assert_eq!(agent_id.as_str(), "agent-composed");

    // The Runtime Client protocol admits at most one attachment: a second
    // endpoint over the same host is rejected rather than silently evicting
    // the first.
    let second = runtime.endpoint();
    let response = second.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    assert!(
        matches!(
            response.error,
            Some(rustx::runtime_client::RuntimeClientError::AttachmentInUse { .. })
        ),
        "{response:?}"
    );

    // The model catalog is reachable through the protocol, so a client never
    // reads models.toml itself.
    let response = endpoint.handle_request(RuntimeClientRequest::ModelCatalogGet {
        id: RequestId::new(2),
    });
    let Some(RuntimeClientResult::ModelCatalog { catalog }) = response.result else {
        panic!("model_catalog_get succeeds: {response:?}");
    };
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].model.to_string(), "local/composed-model");
    assert_eq!(
        catalog.models[0].credential_source,
        rustx::model::CredentialSourceView::Environment {
            variable: "RUSTX_TEST_MODEL_KEY".to_owned()
        },
        "the credential source kind is safe to expose; the value never is"
    );
}
