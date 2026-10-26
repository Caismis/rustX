//! Boundary conformance: capability projection over a managed Python tool
//! package (Issue #174).
//!
//! A managed Python package is prepared by a real, network-bound `uv`
//! environment build and served by a real `FastMCP` stdio child process.
//! The contract under test — the package's Tools projected with Managed Python
//! source provenance — only exists once
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
use rustx::runtime_client::RuntimeClientRequest;
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

/// A capability view covering native + Managed Python + Skill origins: the revision, deterministic ordering, origin
/// metadata, and Skill identity/version/name/description/exact virtual
/// location, with no private internals on the wire.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one complete capability fixture
async fn capability_projection_covers_native_python_and_skills() {
    if super::super::common::provider_emulator::required_uv("the capability Python origin")
        .is_none()
    {
        return;
    }
    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder(
        "conv_47c7c667-7657-7a48-aa3b-c9f889124fae",
    )
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
    .agent_activation(rustx::capabilities::AgentActivation {
        profile: {
            let mut profile = rustx::capabilities::AgentActivation::default().profile;
            profile.tools.builtin = vec!["ls".to_owned(), "read".to_owned()];
            profile.skills = Some(rustx::runtime::agent_profile::AgentSkillSelection::Exact(
                vec!["skill-readme".into()],
            ));
            profile
        },
        ..rustx::capabilities::AgentActivation::default()
    })
    .workspace_fixture(|workspace| {
        support::runtime_client_fixture::write_python_package(
            workspace,
            "py-echo",
            "Echoes arguments",
        );
        support::runtime_client_fixture::write_skill(workspace, "skill-readme", "Reads the README");
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
    let rustx::runtime_client::RuntimeClientResult::DomainPage { page } =
        response.result.expect("successful native domain read")
    else {
        panic!("bounded capability domain page");
    };
    let cut = page.read_cut;
    let capabilities = complete_capabilities(&attachment, &cut);

    // The candidate activated a new revision (Skill + Python package content).
    assert!(capabilities.revision.get() >= 1);

    // Deterministic ordering: base registry order, then the managed Python
    // package's ordinary Tools, then the one native Resource helper group:
    // this fixture selects the FastMCP source with source-wide trust, and the
    // peer advertises Resources even without any registered resource. Two
    // reads of the same generation remain identical.
    let names: Vec<&str> = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "ls",
            "read",
            "py_echo",
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource",
        ]
    );
    let second_view = complete_capabilities(&attachment, &cut);
    assert_eq!(second_view, capabilities, "deterministic ordering");

    // Origin metadata is correct and typed: the managed Python package
    // surfaces with its canonical Managed Python package provenance.
    assert_eq!(capabilities.tools[0].origin, ToolOrigin::Builtin);
    assert!(
        capabilities.tools[3..]
            .iter()
            .all(|tool| tool.origin == ToolOrigin::Builtin)
    );
    assert!(matches!(
        &capabilities.tools.iter().find(|tool| tool.name == "py_echo").unwrap().origin,
        ToolOrigin::ManagedPython { package } if package == "py-echo"
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

fn complete_capabilities(
    attachment: &rustx::runtime_client::RuntimeAttachment,
    cut: &rustx::observation::cut::CheckpointReadCut,
) -> rustx::observation::capability_source::CapabilityView {
    rustx::observation::capability_source::CapabilityView {
        revision: capability_value(attachment, cut, "capabilities", 0, &["revision".into()]),
        configured_tools: capability_rows(attachment, cut, "capabilities.configured_tools"),
        tools: capability_rows(attachment, cut, "capabilities.tools"),
        available_tools: capability_rows(attachment, cut, "capabilities.available_tools"),
        skills: capability_rows(attachment, cut, "capabilities.skills"),
        sources: capability_rows(attachment, cut, "capabilities.sources"),
    }
}

fn capability_rows<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    attachment: &rustx::runtime_client::RuntimeAttachment,
    cut: &rustx::observation::cut::CheckpointReadCut,
    domain: &str,
) -> Vec<T> {
    let mut offset = 0;
    let mut rows = Vec::new();
    loop {
        let response = attachment.handle_request(RuntimeClientRequest::CheckpointPageGet {
            id: rustx::runtime_client::RequestId::new(3),
            cut: cut.clone(),
            domain: domain.into(),
            offset,
            limit: 32,
        });
        let rustx::runtime_client::RuntimeClientResult::CheckpointPage { page } =
            response.result.expect("same-cut native capability page")
        else {
            panic!("native capability page");
        };
        assert_eq!(&page.read_cut, cut);
        assert_eq!(page.domain, domain);
        assert!(page.items.len() <= 32);
        for item in page.items {
            assert_eq!(item.offset, rows.len() as u64, "contiguous native catalog");
            let row = if !item.deferred && item.body_complete != Some(false) {
                rustx::observation::bounded::decode_domain_value(
                    item.value.expect("complete inline capability row"),
                )
                .expect("typed native capability row")
            } else {
                capability_value(attachment, cut, domain, item.offset, &[])
            };
            rows.push(row);
        }
        let Some(next) = page.next_offset else {
            assert_eq!(rows.len() as u64, page.total, "complete original catalog");
            return rows;
        };
        assert!(next > offset, "native capability page advances");
        assert_eq!(next, rows.len() as u64);
        offset = next;
    }
}

fn capability_value<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    attachment: &rustx::runtime_client::RuntimeAttachment,
    cut: &rustx::observation::cut::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
) -> T {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let response = attachment.handle_request(RuntimeClientRequest::CheckpointValueRead {
            id: rustx::runtime_client::RequestId::new(4),
            cut: cut.clone(),
            domain: domain.into(),
            item_offset,
            path: path.to_owned(),
            offset,
            max_bytes: 65_536,
        });
        let rustx::runtime_client::RuntimeClientResult::CheckpointValueChunk { chunk } =
            response.result.unwrap_or_else(|| {
                panic!("same-cut original capability bytes: {domain} row {item_offset} path {path:?}: {:?}", response.error)
            })
        else {
            panic!("native capability value chunk");
        };
        assert_eq!(&chunk.cut, cut);
        assert_eq!(chunk.domain, domain);
        assert_eq!(chunk.item_offset, item_offset);
        assert_eq!(chunk.path, path);
        assert_eq!(chunk.offset, offset);
        assert_eq!(
            chunk.representation,
            rustx::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        );
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .expect("native base64 bytes");
        assert!(part.len() <= 65_536);
        bytes.extend_from_slice(&part);
        let Some(next) = chunk.next_offset else {
            assert_eq!(bytes.len() as u64, chunk.readable_end);
            break;
        };
        assert!(next > offset);
        assert_eq!(next, bytes.len() as u64);
        offset = next;
    }
    rustx::observation::bounded::decode_domain_value(
        serde_json::from_slice(&bytes).expect("original native JSON"),
    )
    .expect("complete typed native capability field")
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
