//! Issue #96: current runtime configuration is re-composed on resume while
//! intentionally Session-local model state survives.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use rustx::capabilities::CapabilitySourceId;
use rustx::local_runtime::composition::{LocalRuntimeDependencies, LocalSessionProduct};
use rustx::model::catalog::{MapCredentialEnvironment, ModelRef};
use rustx::model::session::SessionModelConfig;
use rustx::runtime::identity::McpServerId;
use rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION;
use rustx::runtime_client::settings::{
    EffectiveAgentStatusExtension, EffectiveBackgroundStatus, EffectiveNativeAgentExtensions,
    EffectiveTimeStatus, SettingsBoundary,
};
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};

const MODELS: &str = r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_ISSUE96_KEY"

[[providers.local.models]]
id = "model-a"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"

[[providers.local.models]]
id = "model-b"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"

[[providers.local.models]]
id = "model-c"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"
"#;

fn paths(root: &std::path::Path, config: &std::path::Path) -> LaunchFixture {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    LaunchFixture {
        models: root.join("models.toml"),
        config: config.to_path_buf(),
        skill_paths: Vec::new(),
        no_skills: true,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: rustx::local_runtime::StartupSession::Empty,
        session_name: None,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.join("runtime"),
    }
}

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE96_KEY".to_owned(),
            "test-only-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

fn config_json(
    model: &str,
    reserve_tokens: u64,
    timezone: &str,
    environment_value: &str,
    skills_root: &std::path::Path,
    default_tools: &[&str],
    include_old_mcp: bool,
) -> String {
    let mcp_servers = if include_old_mcp {
        serde_json::json!({
            "old": {
                "enabled": true,
                    "type": "stdio",
                "command": "missing-rustx-issue96-mcp"
            }
        })
    } else {
        serde_json::json!({})
    };
    toml::to_string_pretty(&serde_json::json!({
        "schema_version": 8,
        "agent_id": "agent-issue96",
        "model": {"model": model},
        "extensions": {
            "agent_status": {
                "enabled": true,
                "time": {"enabled": true, "timezone": timezone},
                "background": {"enabled": true}
            }
        },
        "context": {"reserve_tokens": reserve_tokens, "keep_recent_tokens": 4096},
        "default_tools": default_tools,
        "skills": [skills_root],
        "mcp_servers": mcp_servers,
        "environment": {"ISSUE96_CURRENT": environment_value}
    }))
    .unwrap()
}

fn write_skill(root: &std::path::Path, name: &str, description: &str) {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).expect("Skill directory");
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nbody\n"),
    )
    .expect("Skill resource");
}

fn model(reference: &str) -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse(reference).expect("model reference"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn resume_recomposes_current_runtime_and_preserves_only_session_model() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    let skills_root = root.path().join("workspace/configured-skills");
    write_skill(&skills_root, "old-skill", "Old current resource");
    std::fs::write(root.path().join("models.toml"), MODELS).expect("models");
    std::fs::write(
        &config_path,
        config_json(
            "local/model-a",
            11,
            "UTC",
            "v1",
            &skills_root,
            &["read"],
            true,
        ),
    )
    .expect("config v1");
    let startup = paths(root.path(), &config_path);
    std::fs::write(
        startup.workspace.join("AGENTS.md"),
        "old project instructions",
    )
    .expect("old project instructions");

    let product = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("initial product");
    assert_eq!(
        product.runtime().runtime_resources().project_instructions(),
        Some("old project instructions")
    );
    assert_eq!(
        product.runtime().model_view().configured.model.to_string(),
        "local/model-a"
    );
    let endpoint = product.endpoint();
    let initialized = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    assert!(matches!(
        initialized.result,
        Some(RuntimeClientResult::Initialized { .. })
    ));
    let model_set = endpoint.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(2),
        config: Box::new(model("local/model-b")),
    });
    assert!(matches!(
        model_set.result,
        Some(RuntimeClientResult::ModelSet { .. })
    ));
    product.runtime().shutdown().await.unwrap();
    drop(endpoint);
    drop(product);

    std::fs::remove_dir_all(skills_root.join("old-skill")).expect("remove old Skill");
    write_skill(&skills_root, "new-skill", "New current resource");
    std::fs::write(
        startup.workspace.join("AGENTS.md"),
        "new project instructions",
    )
    .expect("new project instructions");
    std::fs::write(
        &config_path,
        config_json(
            "local/model-c",
            22,
            "Asia/Shanghai",
            "v2",
            &skills_root,
            &[],
            false,
        ),
    )
    .expect("config v2");

    let resumed = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("resumed product");
    let runtime = resumed.runtime();
    assert_eq!(
        runtime.runtime_resources().project_instructions(),
        Some("new project instructions"),
        "cold resume independently discovers current project resources"
    );
    assert_eq!(
        runtime.model_view().configured.model.to_string(),
        "local/model-b"
    );
    assert_eq!(runtime.context_config().policy.reserve_tokens, 22);
    assert_eq!(
        runtime
            .context_config()
            .status_engine
            .as_ref()
            .expect("the next launch composes the Agent Status extension")
            .config()
            .time
            .timezone,
        Some(chrono_tz::Asia::Shanghai)
    );
    assert_eq!(
        runtime.tool_runtime().environment().authorized_entries(),
        &[("ISSUE96_CURRENT".to_owned(), "v2".to_owned())]
    );
    assert!(
        !runtime
            .capability()
            .availability()
            .contains_key(&CapabilitySourceId::Mcp(McpServerId::new("old")))
    );
    let snapshot = runtime.capability().current_snapshot();
    assert_eq!(
        snapshot
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["new-skill"]
    );
    // Issue #259 regression 1/4: the ordinary selection is empty and stays
    // empty — and the extension-provided `todo` Tool is still active, because
    // ordinary Tool selection is not the authority that composes it.
    assert_eq!(snapshot.tool_registry().names(), vec!["todo"]);
    assert!(
        !snapshot
            .available_tools()
            .definitions()
            .iter()
            .any(|tool| tool.name == "todo"),
        "and it is not an ordinary available capability, so no selector can name it"
    );
    assert!(snapshot.skill_catalog().is_none());
    assert!(!snapshot.available_tools().tools().is_empty());

    let resumed_endpoint = resumed.endpoint();
    let resumed_initialized = resumed_endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(3),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    let initialized_snapshot = match resumed_initialized.result {
        Some(RuntimeClientResult::Initialized { snapshot, .. }) => snapshot,
        other => panic!("initialize returned an unexpected result: {other:?}"),
    };
    assert!(
        initialized_snapshot.messages.iter().all(|message| {
            let json = serde_json::to_string(message).expect("message JSON");
            !json.contains("project instructions") && !json.contains("current resource")
        }),
        "cold resource changes inject no synthetic conversation message"
    );
    let capability_view = resumed_endpoint.handle_request(RuntimeClientRequest::CapabilityGet {
        id: RequestId::new(5),
    });
    let capabilities = match capability_view.result {
        Some(RuntimeClientResult::Capability { capabilities }) => capabilities,
        other => panic!("capability_get returned an unexpected result: {other:?}"),
    };
    // The client view agrees with the runtime: the active set is exactly the
    // extension-provided Tool, and the ordinary available catalog does not
    // contain it (Issue #259).
    assert_eq!(
        capabilities
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["todo"]
    );
    assert!(!capabilities.available_tools.is_empty());
    assert!(
        !capabilities
            .available_tools
            .iter()
            .any(|tool| tool.name == "todo")
    );
    // `/new` over an untouched empty Session is a semantic no-op, so the
    // switch this test fences needs the active Session to own durable user
    // work first. Durable Pending Inbound acceptance is exactly that
    // boundary; the attempt against the unreachable provider then fails and
    // settles on its own.
    let submitted = resumed_endpoint
        .handle_request_async(RuntimeClientRequest::SubmitInbound {
            id: RequestId::new(6),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: "resume work".to_owned(),
                },
            )],
        })
        .await;
    assert!(
        matches!(
            submitted.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ),
        "unexpected SubmitInbound response: {submitted:?}"
    );
    let new_session = resumed_endpoint
        .handle_request_async(RuntimeClientRequest::SessionNew {
            id: RequestId::new(4),
        })
        .await;
    assert!(
        matches!(
            new_session.result,
            Some(RuntimeClientResult::SessionChanged {
                restart_required: true,
                ..
            })
        ),
        "unexpected SessionNew response: {new_session:?}"
    );
    drop(snapshot);
    drop(resumed_endpoint);
    drop(resumed);
    let fresh = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("fresh Session after current default change");
    assert_eq!(
        fresh.runtime().model_view().configured.model.to_string(),
        "local/model-c"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_current_config_is_rejected_even_when_a_catalog_exists() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    let skills_root = root.path().join("workspace/configured-skills");
    std::fs::create_dir_all(&skills_root).expect("Skill root");
    std::fs::write(root.path().join("models.toml"), MODELS).expect("models");
    std::fs::write(
        &config_path,
        config_json(
            "local/model-a",
            11,
            "UTC",
            "v1",
            &skills_root,
            &["read"],
            false,
        ),
    )
    .expect("valid config");
    let startup = paths(root.path(), &config_path);
    let product = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("valid config creates the catalog");
    drop(product);

    let invalid = br#"agent_id = "agent-issue96"
conversation_id = "historical"

[model]
model = "local/model-a"

[context]
reserve_tokens = 1
keep_recent_tokens = 1
"#;
    std::fs::write(&config_path, invalid).expect("invalid current config");
    assert!(startup.try_resolve().unwrap_err().contains("unknown field"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_first_boot_model_does_not_publish_a_poisoned_session() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    let skills_root = root.path().join("workspace/configured-skills");
    std::fs::create_dir_all(&skills_root).expect("Skill root");
    std::fs::write(root.path().join("models.toml"), MODELS).expect("models");
    let startup = paths(root.path(), &config_path);

    std::fs::write(
        &config_path,
        config_json(
            "local/missing",
            11,
            "UTC",
            "invalid-first-boot",
            &skills_root,
            &[],
            false,
        ),
    )
    .expect("invalid first config");
    assert!(
        startup
            .try_resolve()
            .unwrap_err()
            .contains("unknown catalog model")
    );
    assert!(
        !root.path().join("runtime/sessions/catalog.json").exists(),
        "a failed first launch must not publish a root Session"
    );

    std::fs::write(
        &config_path,
        config_json(
            "local/model-a",
            11,
            "UTC",
            "corrected",
            &skills_root,
            &[],
            false,
        ),
    )
    .expect("corrected config");
    let product = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("corrected config must reuse the runtime root");
    assert_eq!(
        product.runtime().model_view().configured.model.to_string(),
        "local/model-a"
    );
    drop(product);

    let catalog = std::fs::read_to_string(startup.runtime_root.join("sessions/catalog.json"))
        .expect("corrected startup published a root Session");
    assert!(catalog.contains("local/model-a"));
    assert!(!catalog.contains("local/missing"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commented_configuration_documents_compose_a_runtime() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    std::fs::write(
        root.path().join("models.toml"),
        r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_ISSUE96_KEY"

[[providers.local.models]]
id = "model-a"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[providers.local.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[providers.local.models.compat]
chat_reasoning_replay = "omit"
"#,
    )
    .expect("commented models");
    std::fs::write(
        &config_path,
        r#"schema_version = 8
agent_id = "agent-issue96"
default_tools = ["read"]

[model]
model = "local/model-a"

[context]
reserve_tokens = 11
keep_recent_tokens = 4096

[mcp_servers]
"#,
    )
    .expect("commented config");
    let startup = paths(root.path(), &config_path);

    let product = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("TOML configuration documents must compose");
    assert_eq!(
        product.runtime().model_view().configured.model.to_string(),
        "local/model-a"
    );
    assert!(
        !product
            .runtime()
            .capability()
            .availability()
            .contains_key(&CapabilitySourceId::Mcp(McpServerId::new("exa"))),
        "a commented-out MCP entry must stay inert"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_toml_fails_before_composition() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    std::fs::write(root.path().join("models.toml"), MODELS).expect("models");
    let startup = paths(root.path(), &config_path);

    std::fs::write(
        &config_path,
        r#"# syntax failure
[model
model = "local/model-a"
"#,
    )
    .expect("non-TOML config");
    let error = startup
        .try_resolve()
        .expect_err("unterminated tables must fail");
    assert!(
        error.clone().contains("line 2"),
        "a syntax failure must report where it was detected: {error}"
    );
}

/// Writes a launch document whose only variable is the closed native Agent
/// Extension composition.
fn extension_config(enabled: bool, timezone: &str) -> String {
    toml::to_string_pretty(&serde_json::json!({
        "schema_version": 8,
        "agent_id": "agent-ext256",
        "model": {"model": "local/model-a"},
        "context": {"reserve_tokens": 11, "keep_recent_tokens": 4096},
        "default_tools": ["read"],
        "extensions": {
            "agent_status": {
                "enabled": enabled,
                "time": {"enabled": true, "timezone": timezone},
                "background": {"enabled": true}
            }
        }
    }))
    .unwrap()
}

/// Attaches one `LocalSessionProduct` endpoint and returns the
/// effective-extension projection its `initialize` snapshot carries.
fn attached_projection(
    endpoint: &rustx::runtime_client::RuntimeClientEndpoint,
    request_id: u64,
) -> Option<EffectiveNativeAgentExtensions> {
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(request_id),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    match response.result {
        Some(RuntimeClientResult::Initialized { snapshot, .. }) => snapshot.effective_extensions,
        other => panic!("initialize returned an unexpected result: {other:?}"),
    }
}

/// Re-reads the projection over an already-attached endpoint. This is the
/// ordinary client read path, so it also proves that nothing about asking
/// again re-resolves the composition.
fn product_projection(
    endpoint: &rustx::runtime_client::RuntimeClientEndpoint,
    request_id: u64,
) -> Option<EffectiveNativeAgentExtensions> {
    let response = endpoint.handle_request(RuntimeClientRequest::SnapshotGet {
        id: RequestId::new(request_id),
    });
    match response.result {
        Some(RuntimeClientResult::Snapshot { snapshot, .. }) => snapshot.effective_extensions,
        other => panic!("snapshot_get returned an unexpected result: {other:?}"),
    }
}

fn composed_timezone(runtime: &rustx::runtime::ConversationRuntime) -> Option<chrono_tz::Tz> {
    runtime
        .context_config()
        .status_engine
        .as_ref()
        .expect("the Agent Status extension is composed")
        .config()
        .time
        .timezone
}

/// Issue #256 regressions 4 and 5.
///
/// The native Agent Extension composition is **launch-scoped**. A resource
/// reload republishes a whole new `RuntimeResourceSnapshot` — proven here by
/// the advanced revision — and still cannot install, remove, or reconfigure
/// the Agent Status extension of the already-composed runtime. Only the next
/// launch resolves the current document through the ordinary resolver, and
/// that restart preserves the existing Session history byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn ext256_reload_cannot_recompose_extensions_but_the_next_launch_does() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    std::fs::write(root.path().join("models.toml"), MODELS).expect("models");
    std::fs::write(&config_path, extension_config(true, "UTC")).expect("config v1");
    let startup = paths(root.path(), &config_path);

    let product = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("initial product");
    let runtime = product.runtime();
    assert_eq!(composed_timezone(runtime), Some(chrono_tz::UTC));
    let r1 = runtime.runtime_resources().revision();
    let endpoint = product.endpoint();
    // The Runtime Client projection of that same launch-frozen composition.
    assert_eq!(
        attached_projection(&endpoint, 1),
        Some(composed(true, Some(chrono_tz::UTC), true))
    );

    // The on-disk document now disables the extension entirely and changes
    // its contributor configuration.
    std::fs::write(&config_path, extension_config(false, "Asia/Shanghai")).expect("config v2");
    runtime
        .reload_resources()
        .await
        .expect("the resource generation republishes");
    let r2 = runtime.runtime_resources().revision();
    assert!(
        r2.get() > r1.get(),
        "the reload really did publish a new resource generation"
    );
    assert_eq!(
        composed_timezone(runtime),
        Some(chrono_tz::UTC),
        "reload cannot uninstall or reconfigure a launch-scoped extension"
    );
    // Regression 4: a published resource generation does not change the
    // existing live effective-extension projection either. The projection
    // has no reload seam at all — it is installed once, from the runtime.
    assert_eq!(
        product_projection(&endpoint, 100),
        Some(composed(true, Some(chrono_tz::UTC), true)),
        "publishing R2 cannot change an already-composed effective projection"
    );

    // Durable Session work, so the restart below has history to preserve.
    // The attempt against the unreachable provider fails and settles itself.
    let submitted = endpoint
        .handle_request_async(RuntimeClientRequest::SubmitInbound {
            id: RequestId::new(2),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: "extension-scoped work".to_owned(),
                },
            )],
        })
        .await;
    assert!(
        matches!(
            submitted.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ),
        "unexpected SubmitInbound response: {submitted:?}"
    );
    // Shutting the runtime down settles the in-flight attempt, so the
    // pre-restart transcript is read at a quiescent, fully linearized point
    // rather than racing the attempt's own durable publication.
    product
        .runtime()
        .shutdown()
        .await
        .expect("the runtime shuts down");
    let before = session_messages(&endpoint, 3);
    assert!(
        !before.is_empty(),
        "the Session owns durable history before the restart"
    );
    // The endpoint retains controller authority; a real restart releases it too.
    drop(endpoint);
    drop(product);

    // Restart/resume is a new launch: it resolves the *current* document
    // through the existing resolver, binding the Session the catalog
    // publishes as active.
    let mut restart = paths(root.path(), &config_path);
    restart.startup_session = rustx::local_runtime::StartupSession::ContinueActive;
    let resumed = LocalSessionProduct::compose(&(restart).resolve(), &dependencies())
        .await
        .expect("resumed product");
    assert!(
        resumed.runtime().context_config().status_engine.is_none(),
        "the next launch composes the current extension set"
    );
    let resumed_endpoint = resumed.endpoint();
    // Regression 5: restart/recompose is what changes the projection, and it
    // reports the *new* launch's composition, not the retired one.
    assert_eq!(
        attached_projection(&resumed_endpoint, 4),
        Some(agent_status_absent()),
        "the restarted launch projects its own extension composition"
    );
    assert_eq!(
        session_messages(&resumed_endpoint, 5),
        before,
        "an extension configuration change rewrites no canonical Session history"
    );
    resumed
        .runtime()
        .shutdown()
        .await
        .expect("resumed runtime shuts down");
    drop(resumed_endpoint);
    drop(resumed);

    // The symmetric direction: a launch that composes nothing cannot have
    // the extension installed into it by a reload.
    std::fs::write(&config_path, extension_config(false, "UTC")).expect("config v3");
    let empty = LocalSessionProduct::compose(&(startup).resolve(), &dependencies())
        .await
        .expect("empty-extension product");
    assert!(empty.runtime().context_config().status_engine.is_none());
    std::fs::write(&config_path, extension_config(true, "Asia/Shanghai")).expect("config v4");
    empty
        .runtime()
        .reload_resources()
        .await
        .expect("the resource generation republishes");
    assert!(
        empty.runtime().context_config().status_engine.is_none(),
        "reload cannot install a launch-scoped extension into a composed runtime"
    );
    assert_eq!(
        attached_projection(&empty.endpoint(), 6),
        Some(agent_status_absent()),
        "and cannot install one into the effective projection either"
    );
}

/// The canonical Session messages the Runtime Client projects for the
/// currently selected Session.
fn session_messages(
    endpoint: &rustx::runtime_client::RuntimeClientEndpoint,
    request_id: u64,
) -> Vec<String> {
    let response = endpoint.handle_request(RuntimeClientRequest::TranscriptPageGet {
        id: RequestId::new(request_id),
        before_cursor: None,
        limit: 64,
    });
    match response.result {
        Some(RuntimeClientResult::TranscriptPage { page }) => page
            .entries
            .iter()
            .map(|entry| serde_json::to_string(&entry.item).expect("transcript item JSON"))
            .collect(),
        other => panic!("transcript_page_get returned an unexpected result: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Issue #256: the Runtime Client effective-extension projection of a root
// Agent runtime.
// ---------------------------------------------------------------------------

/// The frozen effective native Agent Extension composition the Runtime Client
/// projects for one live root runtime.
///
/// This reads the authoritative host snapshot, which is the only external
/// read model of the projection. It is deliberately *not* derived from
/// `runtime().native_extensions()` here: the point of these regressions is
/// that the wire projection agrees with the runtime, so both sides are read
/// independently and compared.
fn projected_extensions(
    runtime: &rustx::local_runtime::composition::LocalConversationRuntime,
) -> Option<EffectiveNativeAgentExtensions> {
    runtime
        .host()
        .snapshot()
        .expect("the host projects its snapshot")
        .0
        .effective_extensions
}

/// A composition with the given Agent Status contributors and the Todo
/// extension composed.
///
/// Todo is composed in every fixture below because none of them authors a
/// `todo` member, and an unauthored member takes the closed document's own
/// default — which since Issue #259 composes Todo. That is exactly the point
/// of the migration: the default is owned by extension composition, not by
/// `defaultTools`, whose fixtures here select only `read`.
fn composed(
    time: bool,
    timezone: Option<chrono_tz::Tz>,
    background: bool,
) -> EffectiveNativeAgentExtensions {
    EffectiveNativeAgentExtensions {
        goal: None,
        agent_status: Some(EffectiveAgentStatusExtension {
            time: EffectiveTimeStatus {
                enabled: time,
                timezone,
            },
            background: EffectiveBackgroundStatus {
                enabled: background,
            },
        }),
        todo: Some(rustx::runtime_client::settings::EffectiveTodoExtension {}),
    }
}

/// The composition with no Agent Status, and Todo composed by default.
///
/// The two members are independent axes: switching Agent Status off says
/// nothing about Todo, and this value is what proves it on the wire.
fn agent_status_absent() -> EffectiveNativeAgentExtensions {
    EffectiveNativeAgentExtensions {
        goal: None,
        agent_status: None,
        todo: Some(rustx::runtime_client::settings::EffectiveTodoExtension {}),
    }
}

/// The composition with no native Agent Extension at all.
fn uncomposed() -> EffectiveNativeAgentExtensions {
    EffectiveNativeAgentExtensions {
        goal: None,
        agent_status: None,
        todo: None,
    }
}

/// Issue #256 regressions 1, 2 and 3, plus the deliberate disagreement with
/// `config show --sources`.
///
/// A live root runtime projects the exact composition it froze at
/// `LocalConversationCore::compose`: the Time enablement, the frozen IANA
/// timezone, and the Background enablement, as authored — not defaults, and
/// not today's disk. A launch that composes no Agent Status projects
/// `agent_status = None` unambiguously, which is a different value from a
/// composed extension with both contributors switched off.
///
/// Enablement is never inferred from Agent Status observations: this runtime
/// has emitted none at all — its composed-status window is provably empty —
/// and the extension still reports as composed.
///
/// Finally, editing the document after launch makes the two configuration
/// surfaces disagree, and that disagreement is the contract: a fresh
/// prospective resolution (the authority behind `config show --sources`)
/// sees the new document while the attached runtime keeps projecting the
/// composition it is actually running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ext256_a_live_root_projects_its_frozen_effective_extension_composition() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.toml");
    std::fs::write(root.path().join("models.toml"), MODELS).expect("models");
    // Every contributor field is deliberately non-default, so a projection
    // that quietly substituted built-in defaults could not pass.
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&serde_json::json!({
            "schema_version": 8,
            "agent_id": "agent-ext256",
            "model": {"model": "local/model-a"},
            "context": {"reserve_tokens": 11, "keep_recent_tokens": 4096},
            "default_tools": ["read"],
            "extensions": {"agent_status": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": false}
            }}
        }))
        .unwrap(),
    )
    .expect("config v1");
    let fixture = paths(root.path(), &config_path);

    let live = rustx::local_runtime::composition::LocalConversationRuntime::compose(
        &(fixture).resolve(),
        &dependencies(),
    )
    .await
    .expect("interactive composition");

    let (snapshot, _) = live.host().snapshot().expect("snapshot");
    assert_eq!(
        snapshot.effective_extensions,
        Some(composed(true, Some(chrono_tz::Asia::Shanghai), false)),
        "the projection is the exact frozen composition, contributor by contributor"
    );
    // One source of truth: the wire projection and the runtime's own frozen
    // composition are the same value, not two independently maintained ones.
    assert_eq!(
        snapshot.effective_extensions,
        Some(EffectiveNativeAgentExtensions::project(
            &live.runtime().native_extensions()
        )),
    );
    // A root composition is launch-frozen, and the lifetime says so.
    assert_eq!(
        snapshot.settings_lifetimes.extensions,
        SettingsBoundary::LaunchCapture
    );
    assert_eq!(
        snapshot.settings_evidence,
        rustx::runtime_client::settings::SettingsEvidence::LiveSession
    );
    // Regression 3: no Agent Status observation exists anywhere in this
    // runtime, and the extension is still reported as composed. Enablement
    // is a composition fact, not an observation fact.
    assert!(
        snapshot.statuses.is_empty(),
        "no status has been composed for any step yet"
    );

    // The deliberate divergence with the prospective configuration surface.
    std::fs::write(
        &config_path,
        toml::to_string_pretty(&serde_json::json!({
            "schema_version": 8,
            "agent_id": "agent-ext256",
            "model": {"model": "local/model-a"},
            "context": {"reserve_tokens": 11, "keep_recent_tokens": 4096},
            "default_tools": ["read"],
            "extensions": {"agent_status": {"enabled": false}}
        }))
        .unwrap(),
    )
    .expect("config v2");
    let prospective = paths(root.path(), &config_path).resolve();
    assert!(
        prospective
            .config()
            .extension_composition()
            .agent_status()
            .is_none(),
        "a fresh prospective resolution reads the edited document"
    );
    assert_eq!(
        projected_extensions(&live),
        Some(composed(true, Some(chrono_tz::Asia::Shanghai), false)),
        "the attached runtime keeps projecting the composition it runs, not the \
         one a next launch would compose"
    );
    live.runtime().shutdown().await.expect("shutdown");
    drop(live);

    // Regression 2: a launch that composes no Agent Status projects absence
    // unambiguously, and absence is not "present with everything off".
    let empty = rustx::local_runtime::composition::LocalConversationRuntime::compose(
        &paths(root.path(), &config_path).resolve(),
        &dependencies(),
    )
    .await
    .expect("interactive composition");
    let projected = projected_extensions(&empty).expect("a live runtime always projects one");
    assert_eq!(projected, agent_status_absent());
    assert!(projected.agent_status.is_none());
    assert!(
        projected.todo.is_some(),
        "disabling Agent Status says nothing about Todo (Issue #259)"
    );
    assert_ne!(projected, uncomposed());
    assert_ne!(
        projected,
        composed(false, None, false),
        "an absent extension is a different fact from a fully disabled one"
    );
    empty.runtime().shutdown().await.expect("shutdown");
}
