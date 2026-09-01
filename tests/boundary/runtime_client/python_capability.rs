//! Boundary conformance: capability projection over a managed Python tool
//! package (Issue #174).
//!
//! A managed Python package is prepared by a real, network-bound `uv`
//! environment build and served by a real `FastMCP` stdio child process.
//! The contract under test — the package's tools projected as MCP-origin
//! tools of the synthesized `python:<folder>` server — only exists once
//! that real build and real child serve the catalog, so these tests are
//! boundary conformance even though the Runtime Client host side is driven
//! in-process. They skip cleanly when `uv` is unavailable.
//!
//! The transport-independent conformance scenarios whose fixtures spawn no
//! real child stay in `scripted_suites::runtime_client::conformance`; the
//! configured-server half of the MCP origin contract lives in
//! [`super::mcp_capability`].

use super::super::support;
use super::super::support::runtime_client_conformance as conformance;

use std::sync::Arc;

use rustx::runtime::identity::ToolId;
use rustx::runtime_client::{RuntimeClientRequest, RuntimeClientResult};
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

/// A capability view covering native + managed Python package (MCP-origin,
/// Issue #174) + Skill origins: the revision, deterministic ordering, origin
/// metadata, and Skill identity/version/name/description/exact virtual
/// location, with no private internals on the wire.
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
                "py_echo".to_owned(),
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

    // The candidate activated a new revision (Skill + Python package content).
    assert!(capabilities.revision.get() >= 1);

    // Deterministic ordering: base registry order, then the managed Python
    // package's MCP tools, all in one deterministic catalog; two reads are
    // identical.
    let names: Vec<&str> = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, vec!["ls", "py_echo", "read"]);
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

    // Origin metadata is correct and typed: the managed Python package
    // (Issue #174) surfaces as an MCP-origin tool of its synthesized
    // `python:<folder>` server.
    assert_eq!(capabilities.tools[0].origin, ToolOrigin::Builtin);
    assert!(matches!(
        &capabilities.tools[1].origin,
        ToolOrigin::Mcp { server_id } if server_id.as_str() == "python:py-echo"
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

/// The Python-origin transport conformance scenario runs outside the
/// macro-generated matrix: it builds ONE fixture (one uv environment build
/// of the pinned `FastMCP` package, completed before any transport
/// connects) and drives both transports sequentially against the same
/// committed generation, instead of paying one concurrent network-bound
/// build per driver. See the scenario's own documentation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capability_projection_covers_python_origins() {
    conformance::capability_projection_covers_python_origins().await;
}
