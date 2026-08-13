//! Issue #42: the Rust-side local runtime composition owner.
//!
//! One process owns one conversation session, and that session owns exactly
//! one `ConversationToolRuntime` identity, one `CapabilityCoordinator` over
//! the same conversation and workspace, one committed initial capability
//! revision, and one `RuntimeClientHost`. There is deliberately no second
//! tool plane in these tests: the assertions run against the real composed
//! runtime.

mod common;

use std::sync::Arc;

use rustx::local_runtime::composition::{
    LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimeError, LocalRuntimePaths,
};
use rustx::model::ModelProtocol;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1;
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};

/// A catalog whose credential comes from the environment, exercising the
/// startup credential-resolution path.
const MODELS_JSON: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "https://local.fixture.invalid/v1",
      "apiKey": "$RUSTX_TEST_MODEL_KEY",
      "models": [
        {
          "id": "composed-model",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 4096,
          "capabilities": {
            "inputModalities": ["text"],
            "outputModalities": ["text"],
            "toolCalls": true,
            "reasoning": false
          },
          "requestParams": {"temperature": 0.3}
        }
      ]
    }
  }
}"#;

const SESSION_JSON: &str = r#"{
  "conversationId": "conv-composed",
  "agentId": "agent-composed",
  "model": {"model": "local/composed-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
  "nativeTools": {"bash": {"execution": "model_selectable", "concurrency": "sequential"}},
  "environment": {"RUSTX_FIXTURE": "1"}
}"#;

/// Writes the startup files into a temporary root and returns the explicit
/// paths.
fn startup(root: &std::path::Path, models: &str, session: &str) -> LocalRuntimePaths {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let models_path = root.join("models.json");
    let session_path = root.join("session.json");
    std::fs::write(&models_path, models).expect("models.json");
    std::fs::write(&session_path, session).expect("session.json");
    LocalRuntimePaths {
        models: models_path,
        session: session_path,
        workspace,
        runtime_root: root.join("private"),
    }
}

/// Composition dependencies with an explicit credential environment. Model
/// bindings are constructed by production from the catalog's protocol and
/// endpoint; this test exercises startup without invoking a model turn.
fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_TEST_MODEL_KEY".to_owned(),
            "composed-secret".to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}

/// The real composition owns exactly one of each semantic owner, and the
/// initial capability candidate is committed before anything can serve
/// protocol input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn composition_owns_one_conversation_domain() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(root.path(), MODELS_JSON, SESSION_JSON);
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("composition succeeds");

    // One conversation identity, shared by the tool runtime, the capability
    // coordinator, and the host.
    let conversation = runtime.tool_runtime().conversation_id().clone();
    assert_eq!(conversation.as_str(), "conv-composed");
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
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
    for expected in [
        "background_task",
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "bash",
    ] {
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
        .find(|tool| tool.name == "background_task")
        .expect("background_task is registered");
    assert_eq!(
        background.execution_policy,
        rustx::tools::types::ToolExecutionPolicy::ForegroundOnly
    );

    // `background_task` dispatches into *this* conversation's background
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
        snapshot.model.configured.model.to_string(),
        "local/composed-model"
    );
    assert_eq!(snapshot.model.effective.context_window, 128_000);
    assert_eq!(
        snapshot.model.effective.protocol,
        ModelProtocol::OpenAiChatCompletions
    );
    assert_eq!(
        snapshot.model.effective.request_params["temperature"],
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
    let paths = startup(root.path(), MODELS_JSON, SESSION_JSON);
    assert!(!paths.artifacts_root().starts_with(&paths.workspace));
    assert!(!paths.environment_store_root().starts_with(&paths.workspace));
    assert_ne!(paths.artifacts_root(), paths.environment_store_root());

    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
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
    let overlapping = LocalRuntimePaths {
        runtime_root: paths.workspace.join("private"),
        ..paths
    };
    let error = LocalConversationRuntime::compose(&overlapping, &dependencies())
        .await
        .expect_err("overlapping storage is rejected");
    assert!(
        matches!(error, LocalRuntimeError::ToolRuntime { .. }),
        "{error:?}"
    );
}

/// Every startup configuration failure is surfaced before any runtime exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_configuration_failures_are_explicit() {
    let root = tempfile::tempdir().expect("temp root");

    // A missing catalog file.
    let paths = startup(root.path(), MODELS_JSON, SESSION_JSON);
    let missing = LocalRuntimePaths {
        models: root.path().join("absent.json"),
        ..paths.clone()
    };
    assert!(matches!(
        LocalConversationRuntime::compose(&missing, &dependencies())
            .await
            .expect_err("a missing catalog fails"),
        LocalRuntimeError::Io { .. }
    ));

    // A catalog without an explicit base URL.
    let no_base = MODELS_JSON.replace("\"baseUrl\": \"https://local.fixture.invalid/v1\",", "");
    let paths = startup(&root.path().join("no-base"), &no_base, SESSION_JSON);
    assert!(matches!(
        LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect_err("a provider without baseUrl fails"),
        LocalRuntimeError::Catalog(_)
    ));

    // An unresolved environment credential names only the variable.
    let paths = startup(&root.path().join("no-env"), MODELS_JSON, SESSION_JSON);
    let error = LocalConversationRuntime::compose(
        &paths,
        &LocalRuntimeDependencies {
            credentials: Arc::new(MapCredentialEnvironment::default()),
            ..LocalRuntimeDependencies::default()
        },
    )
    .await
    .expect_err("an unresolved credential fails startup");
    assert!(error.to_string().contains("RUSTX_TEST_MODEL_KEY"));
    assert!(!error.to_string().contains("composed-secret"));

    // A session selecting a model the catalog does not declare.
    let bad_session = SESSION_JSON.replace("local/composed-model", "local/absent-model");
    let paths = startup(&root.path().join("bad-model"), MODELS_JSON, &bad_session);
    assert!(matches!(
        LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect_err("an unknown model fails startup"),
        LocalRuntimeError::Model(_)
    ));

    // A session config with an unknown field.
    let bad_session = SESSION_JSON.replace(
        "\"agentId\": \"agent-composed\",",
        "\"agentId\": \"agent-composed\", \"futureKnob\": true,",
    );
    let paths = startup(&root.path().join("bad-session"), MODELS_JSON, &bad_session);
    assert!(matches!(
        LocalConversationRuntime::compose(&paths, &dependencies())
            .await
            .expect_err("an unknown session field fails startup"),
        LocalRuntimeError::Session(_)
    ));
}

/// The composed runtime serves the real Runtime Client endpoint, and the
/// endpoint is derived from the one host rather than a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_endpoint_speaks_for_the_one_composed_host() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(root.path(), MODELS_JSON, SESSION_JSON);
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("composition succeeds");

    let endpoint = runtime.endpoint();
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized {
        conversation_id,
        agent_id,
        ..
    }) = response.result
    else {
        panic!("the endpoint initializes: {response:?}");
    };
    assert_eq!(conversation_id.as_str(), "conv-composed");
    assert_eq!(agent_id.as_str(), "agent-composed");

    // v1 admits at most one attachment: a second endpoint over the same host
    // is rejected rather than silently evicting the first.
    let second = runtime.endpoint();
    let response = second.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    assert!(
        matches!(
            response.error,
            Some(rustx::runtime_client::RuntimeClientError::AttachmentInUse { .. })
        ),
        "{response:?}"
    );

    // The model catalog is reachable through the protocol, so a client never
    // reads models.json itself.
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
