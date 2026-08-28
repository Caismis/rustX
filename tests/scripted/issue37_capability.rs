//! Issue #37: capability/tool/Skill inspection of Runtime Client Protocol
//! v6.
//!
//! The active capability projection must carry the revision, the
//! deterministic tool catalog with origin metadata for native, MCP, and
//! Python tools, and the deterministic Skill catalog (identity, version,
//! name, description) — without executors, environment paths, or private
//! dependency internals on the wire.

use super::support;

use std::sync::Arc;

use rustx::runtime::identity::ToolId;
use rustx::runtime_client::{RuntimeClientRequest, RuntimeClientResult};
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

/// A capability view covering native + Python + Skill origins: the
/// revision, deterministic ordering, origin metadata, and Skill
/// identity/version/name/description/exact virtual location, with no private
/// internals on the wire.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete capability fixture
async fn capability_projection_covers_native_python_and_skills() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    if uv.is_none() {
        eprintln!("uv unavailable; capability Python origin not exercised");
        return;
    }
    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder("conv-37-cap")
        .tools({
            let mut base = rustx::tools::executor::ToolRegistry::new();
            let definition = ToolDefinition {
                id: ToolId::new("tool-ls"),
                name: "ls".to_owned(),
                description: "list files".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
                execution_policy: ToolExecutionPolicy::ForegroundOnly,
                concurrency_policy: ToolConcurrencyPolicy::Sequential,
                approval_policy: rustx::tools::types::ToolApprovalPolicy::Never,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Builtin,
            };
            base.register(
                definition.clone(),
                Arc::new(support::fake::FakeTool::new(
                    definition,
                    support::fake::success_result("listed"),
                )),
            )
            .expect("register base tool");
            base
        })
        .native_tools()
        .tool_activation(rustx::capabilities::ToolActivationPolicy {
            tools: Some(vec![
                "ls".to_owned(),
                "py-echo".to_owned(),
                "read".to_owned(),
            ]),
            ..rustx::capabilities::ToolActivationPolicy::default()
        })
        .workspace_fixture(|workspace| {
            support::runtime_client_fixture::write_python_package(
                workspace,
                "py-echo",
                "Echoes arguments",
            );
            support::runtime_client_fixture::write_skill(
                workspace,
                "skill-readme",
                "Reads the README",
            );
        })
        .build()
        .await;
    let host = fixture.host.clone();
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(1),
    });
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability result");
    };

    // The candidate activated a new revision (Skill + Python content).
    assert!(capabilities.revision.get() >= 1);

    // Deterministic ordering: base registry order, then discovered Python
    // tools, all in one deterministic catalog; two reads are identical.
    let names: Vec<&str> = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, vec!["ls", "py-echo", "read"]);
    let second = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(2),
    });
    let Some(RuntimeClientResult::Capability {
        capabilities: second_view,
    }) = second.result
    else {
        panic!("capability result");
    };
    assert_eq!(second_view, capabilities, "deterministic ordering");

    // Origin metadata is correct and typed.
    assert_eq!(capabilities.tools[0].origin, ToolOrigin::Builtin);
    assert!(matches!(
        &capabilities.tools[1].origin,
        ToolOrigin::Python { tool_version_id } if !tool_version_id.as_str().is_empty()
    ));

    // Skill identity/version/name/description/host location.
    let location = support::runtime_client_fixture::skill_location(
        fixture.runtime.tool_runtime().workspace().root(),
        "skill-readme",
    );
    assert_eq!(capabilities.skills.len(), 1);
    let skill = &capabilities.skills[0];
    assert_eq!(skill.name, "skill-readme");
    assert_eq!(skill.description, "Reads the README");
    assert_eq!(skill.location, location);
    assert_eq!(skill.id.as_str(), "skill-readme");
    assert!(
        skill.version_id.as_str().starts_with("sha256:"),
        "the Skill version is the deterministic content hash"
    );

    // No executor, environment path, package-manager, or dependency
    // internals ever appear on the wire.
    let json = serde_json::to_string(&capabilities).expect("serialize capabilities");
    for forbidden in ["\"executor\":", "/skill-env", "uv.lock", "pyproject"] {
        assert!(
            !json.contains(forbidden),
            "the wire projection must not leak {forbidden:?}: {json}"
        );
    }
    assert!(json.contains(&location));
}

/// The MCP origin is projected with its server identity; the MCP fixture
/// server serves the catalog.
#[cfg(all(unix, feature = "mcp-fixture"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete MCP capability fixture
async fn capability_projection_covers_mcp_origins() {
    if rustx::tools::mcp::fixture::serve_if_fixture_mode(
        rustx::tools::mcp::fixture::FixtureServer::from_env(),
    )
    .await
    {
        return;
    }
    let mcp_bindings = rustx::tools::mcp::McpServerBindings::from([(
        rustx::runtime::identity::McpServerId::new("fixture"),
        rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: rustx::tools::mcp::fixture::fixture_spawn_args(
                    "scripted_suites::issue37_capability::capability_projection_covers_mcp_origins",
                ),
                cwd: None,
                environment: std::collections::BTreeMap::from([(
                    rustx::tools::mcp::fixture::FIXTURE_MODE_ENV.to_owned(),
                    "1".to_owned(),
                )]),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        },
    )]);
    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder("conv-37-mcp")
        .mcp_servers(mcp_bindings)
        .build()
        .await;
    let host = fixture.host.clone();
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(1),
    });
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability result");
    };
    assert!(capabilities.revision.get() >= 1);
    let mcp_tools: Vec<_> = capabilities
        .tools
        .iter()
        .filter(|tool| matches!(tool.origin, ToolOrigin::Mcp { .. }))
        .collect();
    assert_eq!(mcp_tools.len(), 3, "echo, mutate, slow");
    let names: Vec<&str> = mcp_tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, vec!["echo", "mutate", "slow"]);
    for tool in mcp_tools {
        assert!(matches!(
            &tool.origin,
            ToolOrigin::Mcp { server_id } if server_id.as_str() == "fixture"
        ));
    }
    // The wire projection carries no MCP SDK objects or transport data.
    let json = serde_json::to_string(&capabilities).expect("serialize");
    for forbidden in ["transport", "rmcp", "executor"] {
        assert!(
            !json.contains(forbidden),
            "the wire projection must not leak {forbidden:?}"
        );
    }
}
