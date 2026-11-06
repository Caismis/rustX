//! Frozen Resource authority survives the real selected-only child composition.

use super::*;
use crate::capabilities::ToolSourceId;
use crate::runtime::identity::McpServerId;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig, fixture};

fn resource_dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([
            ("RUSTX_CHILD_KEY".into(), "test-only-secret".into()),
            ("RUSTX_TEST_FROZEN_KEY".into(), "test-only-secret".into()),
        ]))),
        ..LocalRuntimeDependencies::default()
    }
}

fn resource_binding(test_name: &str, resource_only: bool) -> McpServerBinding {
    let mut binding = fixture_binding(test_name, "documents_");
    let McpTransportConfig::Stdio { environment, .. } = &mut binding.transport else {
        unreachable!("the fixture always uses stdio")
    };
    environment.insert(fixture::RESOURCES_ENV.into(), "1".into());
    if resource_only {
        environment.insert(fixture::TOOLS_DISABLED_ENV.into(), "1".into());
    }
    binding
}

fn resource_spec(root: &std::path::Path, binding: McpServerBinding) -> SubagentChildSpec {
    let mut child = spec_with_extensions(
        root,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        crate::extensions::NativeAgentExtensions::none(),
    );
    let source = ToolSourceId::Mcp(McpServerId::new("documents"));
    child.resolved.resource_sources.insert(source.clone());
    child
        .resolved
        .materialization
        .sources
        .insert(source, binding);
    child
}

fn assert_resource_helpers_require_approval(snapshot: &crate::capabilities::CapabilitySnapshot) {
    for definition in snapshot.tool_registry().definitions() {
        let arguments = if definition.name == "read_mcp_resource" {
            serde_json::json!({"server":"documents", "uri":"fixture://text"})
        } else {
            serde_json::json!({})
        };
        let crate::tools::PreflightOutcome::Ready(prepared) = snapshot
            .tool_registry()
            .preflight(&crate::tools::ToolCall {
                id: crate::runtime::identity::ToolCallId::new("child-resource"),
                tool_id: definition.id.clone(),
                name: definition.name.clone(),
                arguments,
            })
            .unwrap()
        else {
            panic!("the frozen resource helper arguments are valid")
        };
        assert_eq!(prepared.approval, ToolApprovalPolicy::Always);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resource_only_child_uses_frozen_binding_despite_new_settings_and_sibling() {
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let root = lab();
    let mut binding = resource_binding(
        "local_runtime::composition::subagent_child::tests::resources::resource_only_child_uses_frozen_binding_despite_new_settings_and_sibling",
        true,
    );
    binding.policy.approval = ToolApprovalPolicy::Always;
    let observation = root.path().join("resource-requests.jsonl");
    let McpTransportConfig::Stdio { environment, .. } = &mut binding.transport else {
        unreachable!("the fixture always uses stdio")
    };
    environment.insert(
        fixture::resources::RESOURCE_OBSERVATION_FILE_ENV.into(),
        observation.display().to_string(),
    );
    let child = resource_spec(root.path(), binding);
    let child: SubagentChildSpec =
        serde_json::from_slice(&serde_json::to_vec(&child).unwrap()).unwrap();
    // A live settings reread would fail; the frozen child's own binding is complete.
    std::fs::write(root.path().join("rustx.toml"), "retired root configuration").unwrap();
    std::fs::write(
        root.path().join("workspace/rustx.toml"),
        "newer global source configuration",
    )
    .unwrap();
    let core = LocalConversationCore::compose_subagent_child(
        &child,
        &resource_dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("a resource-only frozen source is a complete child requirement");
    let snapshot = core.capability().current_snapshot();
    let mut names = snapshot.tool_registry().names();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "list_mcp_resource_templates",
            "list_mcp_resources",
            "read_mcp_resource"
        ]
    );
    assert_eq!(snapshot.tool_registry().model_definitions().len(), 3);
    assert!(snapshot.available_tools().tools().is_empty());
    assert_eq!(
        snapshot.resource_sources(),
        &child.resolved.resource_sources
    );
    assert_eq!(snapshot.mcp_servers().len(), 1);
    assert_resource_helpers_require_approval(&snapshot);
    assert!(
        !observation.exists(),
        "handshake preparation never enumerates resources or injects their bodies"
    );

    let sibling_root = lab();
    let mut sibling = spec_with_extensions(
        sibling_root.path(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        crate::extensions::NativeAgentExtensions::none(),
    );
    sibling.workspace.clone_from(&child.workspace);
    let sibling_core = LocalConversationCore::compose_subagent_child(
        &sibling,
        &resource_dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect("a sibling sharing the workspace still owns an independent profile");
    let sibling_snapshot = sibling_core.capability().current_snapshot();
    assert!(
        sibling_snapshot
            .tool_registry()
            .model_definitions()
            .is_empty()
    );
    assert!(sibling_snapshot.resource_sources().is_empty());
    assert!(sibling_snapshot.mcp_servers().is_empty());
    sibling_core.runtime().activate();
    core.runtime().activate();
    sibling_core.shutdown().await.unwrap();
    core.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_only_child_does_not_gain_resources_from_a_capable_peer() {
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let root = lab();
    let server = McpServerId::new("documents");
    let binding = resource_binding(
        "local_runtime::composition::subagent_child::tests::resources::exact_only_child_does_not_gain_resources_from_a_capable_peer",
        false,
    );
    let workspace = crate::tools::Workspace::new(root.path().join("workspace")).unwrap();
    let definition = frozen_fixture_tool(&server, &binding, &workspace, "documents_echo").await;
    let tool = ResolvedSubagentTool::Source {
        source_id: ToolSourceId::Mcp(server.clone()),
        tool_id: definition.id.clone(),
        name: definition.name.clone(),
        identity: crate::tools::mcp::identity::definition_identity(&definition).unwrap(),
        definition,
    };
    let mut child = spec_with_extensions(
        root.path(),
        vec![tool],
        Vec::new(),
        Vec::new(),
        crate::extensions::NativeAgentExtensions::none(),
    );
    child
        .resolved
        .materialization
        .sources
        .insert(ToolSourceId::Mcp(server), binding);
    let core = LocalConversationCore::compose_subagent_child(
        &child,
        &resource_dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .unwrap();
    let snapshot = core.capability().current_snapshot();
    assert_eq!(snapshot.tool_registry().names(), ["documents_echo"]);
    assert!(
        snapshot
            .resolved_profile()
            .unwrap()
            .resource_sources
            .is_empty()
    );
    assert!(snapshot.resource_sources().is_empty());
    core.runtime().activate();
    core.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_resource_capability_fails_child_before_ready() {
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let root = lab();
    let child = resource_spec(
        root.path(),
        fixture_binding(
            "local_runtime::composition::subagent_child::tests::resources::removed_resource_capability_fails_child_before_ready",
            "documents_",
        ),
    );
    let error = LocalConversationCore::compose_subagent_child(
        &child,
        &resource_dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("a child cannot start without its frozen resource authority");
    assert!(matches!(
        error,
        crate::local_runtime::LocalRuntimeError::Capability { detail }
            if detail.contains("SourceResourcesUnsupported")
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_only_child_requires_a_frozen_binding() {
    let root = lab();
    let mut child = spec(root.path(), Vec::new(), Vec::new(), Vec::new());
    child
        .resolved
        .resource_sources
        .insert(ToolSourceId::Mcp(McpServerId::new("documents")));
    let error = LocalConversationCore::compose_subagent_child(
        &child,
        &resource_dependencies(),
        &ChildPreparation::detached(),
    )
    .await
    .expect_err("resource authority alone cannot discover a mutable global binding");
    assert!(format!("{error:?}").contains("was not given a binding"));
}
