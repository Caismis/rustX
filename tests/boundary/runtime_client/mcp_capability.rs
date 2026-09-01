//! Boundary conformance: capability projection over a real MCP child.
//!
//! The MCP fixture server is this same test binary re-executed in fixture
//! mode as a real stdio child process. The contract under test — MCP origin
//! metadata in the capability projection — only exists once a real stdio
//! transport session to a real server process serves the catalog, so these
//! tests are boundary conformance even though the Runtime Client host side
//! is driven in-process.
//!
//! The transport-independent conformance scenarios (run over in-memory byte
//! pipes) stay in `scripted_suites::runtime_client::conformance`; the
//! managed Python package projection (Issue #174) lives in
//! [`super::python_capability`].
//!
//! The exact libtest name of each test is passed to the fixture child as its
//! re-entry point; renaming a test means updating its spawn string.

use super::super::support;
use super::super::support::runtime_client_conformance as conformance;

/// The MCP capability projection, driven through both transports.
#[cfg(all(unix, feature = "mcp-fixture"))]
mod projection_covers_mcp_origins {
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn direct() {
        super::conformance::capability_projection_covers_mcp_origins(
            &super::conformance::DirectDriverFactory,
            "boundary_suites::runtime_client::mcp_capability::projection_covers_mcp_origins::direct",
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stdio_jsonl() {
        super::conformance::capability_projection_covers_mcp_origins(
            &super::conformance::StdioJsonlDriverFactory,
            "boundary_suites::runtime_client::mcp_capability::projection_covers_mcp_origins::stdio_jsonl",
        )
        .await;
    }
}

/// The MCP origin is projected with its server identity; the MCP fixture
/// server serves the catalog.
#[cfg(all(unix, feature = "mcp-fixture"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete MCP capability fixture
async fn capability_projection_carries_mcp_origin_metadata() {
    use rustx::runtime_client::{RuntimeClientRequest, RuntimeClientResult};
    use rustx::tools::types::ToolOrigin;

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
                    "boundary_suites::runtime_client::mcp_capability::capability_projection_carries_mcp_origin_metadata",
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
