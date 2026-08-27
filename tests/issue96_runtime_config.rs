//! Issue #96: current runtime configuration is re-composed on resume while
//! intentionally Session-local model state survives.

use std::sync::Arc;

use rustx::capabilities::CapabilitySourceId;
use rustx::local_runtime::composition::{
    LocalRuntimeDependencies, LocalRuntimeError, LocalRuntimePaths, LocalSessionProduct,
};
use rustx::model::catalog::{MapCredentialEnvironment, ModelRef};
use rustx::model::session::SessionModelConfig;
use rustx::runtime::identity::McpServerId;
use rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION;
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};

const MODELS: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:9/v1",
      "apiKey": "$RUSTX_ISSUE96_KEY",
      "models": [
        {
          "id": "model-a",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
          "compat": {"chatReasoningReplay": "omit"}
        },
        {
          "id": "model-b",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
          "compat": {"chatReasoningReplay": "omit"}
        },
        {
          "id": "model-c",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {"inputModalities": ["text"], "outputModalities": ["text"], "toolCalls": true, "reasoning": false},
          "compat": {"chatReasoningReplay": "omit"}
        }
      ]
    }
  }
}"#;

fn paths(root: &std::path::Path, config: &std::path::Path) -> LocalRuntimePaths {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    LocalRuntimePaths {
        models: root.join("models.jsonc"),
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
        credentials: Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE96_KEY".to_owned(),
            "test-only-secret".to_owned(),
        )])),
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
                "type": "stdio",
                "command": "/definitely/missing-rustx-issue96-mcp"
            }
        })
    } else {
        serde_json::json!({})
    };
    serde_json::json!({
        "schemaVersion": 3,
        "agentId": "agent-issue96",
        "model": {"model": model},
        "agentStatus": {
            "time": {"enabled": true, "timezone": timezone},
            "background": {"enabled": true}
        },
        "context": {"reserveTokens": reserve_tokens, "keepRecentTokens": 4096},
        "defaultTools": default_tools,
        "skills": [skills_root],
        "mcpServers": mcp_servers,
        "environment": {"ISSUE96_CURRENT": environment_value}
    })
    .to_string()
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
    let config_path = root.path().join("rustx.jsonc");
    let skills_root = root.path().join("configured-skills");
    write_skill(&skills_root, "old-skill", "Old current resource");
    std::fs::write(root.path().join("models.jsonc"), MODELS).expect("models");
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

    let product = LocalSessionProduct::compose(&startup, &dependencies())
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

    let resumed = LocalSessionProduct::compose(&startup, &dependencies())
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
    assert_eq!(snapshot.tool_registry().names(), vec!["read"]);
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
    assert_eq!(
        capabilities
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read"]
    );
    assert!(!capabilities.available_tools.is_empty());
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
    drop(resumed);
    let fresh = LocalSessionProduct::compose(&startup, &dependencies())
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
    let config_path = root.path().join("rustx.jsonc");
    let skills_root = root.path().join("configured-skills");
    std::fs::create_dir_all(&skills_root).expect("Skill root");
    std::fs::write(root.path().join("models.jsonc"), MODELS).expect("models");
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
    let product = LocalSessionProduct::compose(&startup, &dependencies())
        .await
        .expect("valid config creates the catalog");
    drop(product);

    let invalid = br#"{
        "agentId": "agent-issue96",
        "model": {"model": "local/model-a"},
        "context": {"reserveTokens": 1, "keepRecentTokens": 1},
        "conversationId": "historical"
    }"#;
    std::fs::write(&config_path, invalid).expect("invalid current config");
    let result = LocalSessionProduct::compose(&startup, &dependencies()).await;
    assert!(
        matches!(result, Err(LocalRuntimeError::RuntimeConfig(_))),
        "invalid current config must fail before catalog resume: {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_first_boot_model_does_not_publish_a_poisoned_session() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.jsonc");
    let skills_root = root.path().join("configured-skills");
    std::fs::create_dir_all(&skills_root).expect("Skill root");
    std::fs::write(root.path().join("models.jsonc"), MODELS).expect("models");
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
    let error = LocalSessionProduct::compose(&startup, &dependencies())
        .await
        .expect_err("missing current model must fail before Session publication");
    assert!(matches!(error, LocalRuntimeError::Model(_)));
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
    let product = LocalSessionProduct::compose(&startup, &dependencies())
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
    let config_path = root.path().join("rustx.jsonc");
    std::fs::write(
        root.path().join("models.jsonc"),
        r#"{
  // The provider identity is a local name, never an official endpoint.
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:9/v1",
      "apiKey": "$RUSTX_ISSUE96_KEY",
      "models": [
        {
          "id": "model-a",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {
            "inputModalities": ["text"],
            "outputModalities": ["text"],
            "toolCalls": true,
            "reasoning": false,
          },
          /* Required for this protocol: how prior reasoning is replayed. */
          "compat": {"chatReasoningReplay": "omit"},
        },
      ],
    },
  },
}"#,
    )
    .expect("commented models");
    std::fs::write(
        &config_path,
        r#"{
  "schemaVersion": 3,
  "agentId": "agent-issue96",
  // The default model of a brand-new Session.
  "model": {"model": "local/model-a"},
  "context": {
    "reserveTokens": 11,
    "keepRecentTokens": 4096,
  },
  "defaultTools": ["read"],
  // "mcpServers": {"exa": {"type": "http", "url": "https://mcp.exa.ai/mcp"}},
  "mcpServers": {},
}"#,
    )
    .expect("commented config");
    let startup = paths(root.path(), &config_path);

    let product = LocalSessionProduct::compose(&startup, &dependencies())
        .await
        .expect("JSONC configuration documents must compose");
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
async fn relaxations_beyond_jsonc_still_fail_composition() {
    let root = tempfile::tempdir().expect("root");
    let config_path = root.path().join("rustx.jsonc");
    std::fs::write(root.path().join("models.jsonc"), MODELS).expect("models");
    let startup = paths(root.path(), &config_path);

    std::fs::write(
        &config_path,
        r#"{
  schemaVersion: 3,
  'agentId': 'agent-issue96',
  "model": {"model": "local/model-a"},
  "context": {"reserveTokens": 11, "keepRecentTokens": 4096}
}"#,
    )
    .expect("non-JSONC config");
    let error = LocalSessionProduct::compose(&startup, &dependencies())
        .await
        .expect_err("unquoted keys and single-quoted strings must fail");
    assert!(matches!(error, LocalRuntimeError::RuntimeConfig(_)));
    assert!(
        error.to_string().contains("line 2"),
        "a syntax failure must report where it was detected: {error}"
    );
}
