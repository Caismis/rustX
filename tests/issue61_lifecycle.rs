//! Issue #61: the production composition lifecycle.
//!
//! The real local runtime composition is split into a shared semantic
//! assembly ([`LocalConversationCore`]) and two final paths: the
//! interactive runtime (core + `RuntimeClientHost`) and the headless
//! runtime (core, no `RuntimeClientHost`). These tests prove against the
//! **real** production composition — the real catalog, current runtime config,
//! provider bindings, tool plane, capability plane, and context pieces —
//! that:
//!
//! - interactive and headless resolve the same semantic composition
//!   (identity, session model, tool runtime ownership, capability
//!   revision, context policy, workspace/artifact boundaries) and differ
//!   only in the Runtime Client adapter (Test F);
//! - a headless production runtime executes a real turn through the same
//!   `AgentExecution` path with no Runtime Client host ever constructed
//!   (Test G);
//! - the interactive production path still builds and runs over the same
//!   semantic composition (Test H).

mod common;

use std::sync::Arc;

use common::provider_emulator::ProviderEmulator;
use rustx::local_runtime::composition::{
    HeadlessConversationRuntime, LocalConversationRuntime, LocalRuntimeDependencies,
    LocalRuntimePaths,
};
use rustx::message::content::TextBlock;
use rustx::message::types::UserContentBlock;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::model::session::SessionModelView;
use rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION;

const CREDENTIAL_VARIABLE: &str = "RUSTX_CONFORMANCE_KEY";
const CREDENTIAL_VALUE: &str = "conformance-secret";
const CHAT_MODEL: &str = "chat-model";

/// A catalog whose credential comes from the environment. The base URL is
/// unreachable: the composition tests never invoke a provider, so startup
/// succeeds exactly as in production.
const MODELS_JSON: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "https://local.fixture.invalid/v1",
      "apiKey": "$RUSTX_CONFORMANCE_KEY",
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
          "compat": {"chatReasoningReplay": "omit"},
          "requestParams": {"temperature": 0.3}
        }
      ]
    }
  }
}"#;

const RUNTIME_CONFIG_JSON: &str = r#"{
  "agentId": "agent-lifecycle",
  "model": {"model": "local/composed-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
  "nativeTools": {"bash": {"execution": "model_selectable", "concurrency": "sequential"}},
  "environment": {"RUSTX_FIXTURE": "1"}
}"#;

/// Writes the startup files into a temporary root and returns the explicit
/// paths.
fn startup(root: &std::path::Path, models: &str, config: &str) -> LocalRuntimePaths {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let models_path = root.join("models.jsonc");
    let config_path = root.join("rustx.jsonc");
    std::fs::write(&models_path, models).expect("models.jsonc");
    std::fs::write(&config_path, config).expect("rustx.jsonc");
    LocalRuntimePaths {
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

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            CREDENTIAL_VARIABLE.to_owned(),
            CREDENTIAL_VALUE.to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}

fn submit_content(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })]
}

/// The semantic projection of one composed runtime, restricted to the
/// pieces that must be identical across the interactive and headless
/// paths. Runtime Client-specific state is deliberately absent.
struct SemanticProjection {
    conversation_id: String,
    agent_id: String,
    model: SessionModelView,
    context_policy: rustx::context::SessionContextPolicy,
    workspace_root: std::path::PathBuf,
    artifacts_root: std::path::PathBuf,
    mailbox_conversation: String,
    background_conversation: String,
    capability_revision: u64,
}

fn semantic_projection(
    conversation_id: &rustx::runtime::identity::ConversationId,
    agent_id: &rustx::runtime::identity::AgentId,
    model: SessionModelView,
    context_policy: rustx::context::SessionContextPolicy,
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    capability: &rustx::capabilities::CapabilityCoordinator,
) -> SemanticProjection {
    SemanticProjection {
        conversation_id: conversation_id.as_str().to_owned(),
        agent_id: agent_id.as_str().to_owned(),
        model,
        context_policy,
        workspace_root: tool_runtime.workspace().root().to_path_buf(),
        artifacts_root: tool_runtime.artifacts().root().to_path_buf(),
        mailbox_conversation: tool_runtime.mailbox().conversation_id().as_str().to_owned(),
        background_conversation: tool_runtime
            .background()
            .conversation_id()
            .as_str()
            .to_owned(),
        capability_revision: capability.current_snapshot().revision().get(),
    }
}

/// Test F — the interactive and headless production compositions share one
/// semantic assembly: same conversation identity, same agent, same session
/// model configuration, same context policy, same tool runtime ownership
/// (workspace, artifacts, mailbox, background), and the same committed
/// capability revision. They differ only in the Runtime Client adapter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interactive_and_headless_share_one_semantic_composition() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(root.path(), MODELS_JSON, RUNTIME_CONFIG_JSON);
    let dependencies = dependencies();

    let interactive = LocalConversationRuntime::compose(&paths, &dependencies)
        .await
        .expect("the interactive composition succeeds");
    let headless = HeadlessConversationRuntime::compose(&paths, &dependencies)
        .await
        .expect("the headless composition succeeds");

    // Both final paths are already active.
    assert!(interactive.runtime().is_activated());
    assert!(headless.runtime().is_activated());

    // The semantic composition resolves identically on both paths.
    let interactive_projection = semantic_projection(
        interactive.runtime().conversation_id(),
        interactive.runtime().agent_id(),
        interactive.runtime().model_view(),
        interactive.runtime().context_config().policy,
        interactive.tool_runtime(),
        interactive.capability(),
    );
    let headless_projection = semantic_projection(
        headless.runtime().conversation_id(),
        headless.runtime().agent_id(),
        headless.runtime().model_view(),
        headless.runtime().context_config().policy,
        headless.tool_runtime(),
        headless.capability(),
    );
    assert_eq!(
        interactive_projection.conversation_id, headless_projection.conversation_id,
        "one conversation identity"
    );
    assert_eq!(
        interactive_projection.agent_id,
        headless_projection.agent_id
    );
    assert_eq!(
        interactive_projection.model, headless_projection.model,
        "the session model configuration resolves identically"
    );
    assert_eq!(
        interactive_projection.context_policy, headless_projection.context_policy,
        "the context policy is the shared session policy"
    );
    assert_eq!(
        interactive_projection.workspace_root, headless_projection.workspace_root,
        "the workspace boundary is the same"
    );
    assert_eq!(
        interactive_projection.artifacts_root, headless_projection.artifacts_root,
        "the artifact boundary is the same"
    );
    assert_eq!(
        interactive_projection.mailbox_conversation, headless_projection.mailbox_conversation,
        "the canonical mailbox belongs to the same conversation"
    );
    assert_eq!(
        interactive_projection.background_conversation, headless_projection.background_conversation,
        "the background registry belongs to the same conversation"
    );
    assert_eq!(
        interactive_projection.capability_revision, headless_projection.capability_revision,
        "the committed startup capability revision is the same"
    );

    // The one protocol-shaped difference: the interactive runtime owns a
    // Runtime Client host, the headless runtime owns none.
    assert!(
        interactive.tool_runtime().is_runtime_client_bound(),
        "the interactive runtime bound its Runtime Client"
    );
    assert!(
        !headless.tool_runtime().is_runtime_client_bound(),
        "no Runtime Client host was ever constructed for the headless runtime"
    );
    assert!(
        !headless.capability().is_runtime_client_bound(),
        "the headless runtime claimed no Runtime Client capability binding"
    );
    // The interactive runtime's protocol surface still speaks for the same
    // conversation.
    let (_attachment, result) = interactive
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    assert!(matches!(
        result,
        rustx::runtime_client::types::RuntimeClientResult::Initialized { .. }
    ));
}

/// The catalog for the emulator-driven tests, mirroring the issue 47
/// conformance shapes.
fn emulator_models_json(emulator: &ProviderEmulator) -> String {
    let window: u64 = 128_000;
    serde_json::json!({
        "providers": {
            "emulator": {
                "baseUrl": emulator.openai_base_url(),
                "apiKey": format!("${CREDENTIAL_VARIABLE}"),
                "models": [
                    {
                        "id": CHAT_MODEL,
                        "protocol": "openai_chat_completions",
                        "contextWindow": window,
                        "maxOutputTokens": 1024,
                        "capabilities": {
                            "inputModalities": ["text"],
                            "outputModalities": ["text"],
                            "toolCalls": true,
                            "reasoning": true
                        },
                        "compat": {"chatReasoningReplay": "omit"},
                    },
                ],
            },
        },
    })
    .to_string()
}

fn emulator_session_json() -> String {
    serde_json::json!({
        "agentId": "agent-headless",
        "model": {"model": format!("emulator/{CHAT_MODEL}")},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
    })
    .to_string()
}

/// Test G — the production headless turn: the real headless composition
/// publishes ordinary inbound, runs the real `AgentExecution` through the
/// real provider boundary, and settles — with no Runtime Client host ever
/// constructed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_production_turn_runs_without_any_client_host() {
    let Some(emulator) = ProviderEmulator::start("openai_chat_streamed_turn").await else {
        return;
    };
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(
        root.path(),
        &emulator_models_json(&emulator),
        &emulator_session_json(),
    );
    let runtime = HeadlessConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("the headless composition succeeds");
    assert!(
        !runtime.tool_runtime().is_runtime_client_bound(),
        "the headless runtime has no Runtime Client host"
    );

    // Publish one ordinary inbound turn through the runtime's own boundary.
    runtime
        .runtime()
        .submit_inbound(submit_content("conformance: turn one"))
        .expect("inbound accepted");

    // Deterministic settlement: the runtime's settlement signal fires once
    // when the authoritative state returns to the coordinator.
    runtime.runtime().settlement_signal().notified().await;

    // The real provider path ran exactly once and the attempt settled.
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 1, "exactly one provider request");
    assert_eq!(
        requests[0]["model"],
        serde_json::json!(CHAT_MODEL),
        "the real resolved binding drove the request"
    );
    assert_eq!(
        common::request_snapshots(&runtime.runtime().request_history()).len(),
        1,
        "the settled attempt retained its request facts"
    );
    emulator.finish().await;
}

/// Test H — the interactive production path continues to build over the
/// same semantic composition and drive a real turn through the Runtime
/// Client host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interactive_production_turn_still_builds_over_the_same_composition() {
    let Some(emulator) = ProviderEmulator::start("openai_chat_streamed_turn").await else {
        return;
    };
    let root = tempfile::tempdir().expect("temp root");
    let paths = startup(
        root.path(),
        &emulator_models_json(&emulator),
        &emulator_session_json(),
    );
    let runtime = LocalConversationRuntime::compose(&paths, &dependencies())
        .await
        .expect("the interactive composition succeeds");
    assert!(
        runtime.tool_runtime().is_runtime_client_bound(),
        "the interactive runtime bound its Runtime Client"
    );
    assert!(runtime.runtime().is_activated());

    runtime
        .host()
        .submit_inbound(submit_content("conformance: turn one"))
        .expect("inbound accepted");
    let (attachment, _) = runtime
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let (_, cursor) = runtime.host().snapshot().expect("snapshot");
    let subscription = attachment.subscribe_events(cursor).expect("subscribe");
    let mut settled = false;
    while let rustx::runtime_client::host::EventDelivery::Event(published) =
        tokio::time::timeout(std::time::Duration::from_secs(30), subscription.next())
            .await
            .expect("the observation stream stays open")
    {
        if matches!(
            published.event,
            rustx::runtime_client::RuntimeClientEvent::AttemptSettled { .. }
        ) {
            settled = true;
            break;
        }
    }
    assert!(settled, "the interactive turn settled through the host");
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 1);
    emulator.finish().await;
}
