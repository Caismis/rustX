//! Resources capability publication uses the ordinary candidate/epoch boundary.
use super::*;
use crate::capabilities::selection::SourceToolSelection;
use crate::tools::mcp::fixture::{FixtureServer, serve_if_fixture_mode};

fn assert_helpers(snapshot: &CapabilitySnapshot, count: usize) {
    assert_eq!(
        snapshot
            .tool_registry()
            .definitions()
            .iter()
            .filter(|tool| crate::tools::mcp::is_resource_tool(&tool.name))
            .count(),
        count,
    );
    let model_names = snapshot
        .tool_registry()
        .model_definitions()
        .into_iter()
        .filter(|tool| crate::tools::mcp::is_resource_tool(&tool.name))
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(model_names.len(), count);
    if count == 3 {
        assert_eq!(
            model_names,
            [
                "list_mcp_resources",
                "list_mcp_resource_templates",
                "read_mcp_resource"
            ]
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One source-wide/Exact/demand matrix crosses real publication boundaries.
async fn resource_only_sources_publish_three_helpers_only_for_source_wide_authority() {
    if serve_if_fixture_mode(FixtureServer {
        resources_supported: true,
        resources_empty: true,
        tools_disabled: true,
        ..Default::default()
    })
    .await
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "resources",
        "capabilities::coordinator::resource_tests::resource_only_sources_publish_three_helpers_only_for_source_wide_authority",
    );
    let source = ToolSourceId::Mcp(id.clone());
    let candidate = coordinator.prepare_candidate().await.unwrap();
    assert_eq!(
        candidate.availability.get(&source),
        Some(&CapabilitySourceState::Ready)
    );
    assert!(candidate.available_tools().tools().is_empty());
    let snapshot = coordinator.commit(candidate).unwrap();
    assert_helpers(&snapshot, 3);
    assert_eq!(snapshot.resource_sources(), &[source.clone()].into());
    assert_eq!(
        snapshot.resolved_profile().unwrap().resource_sources,
        [source.clone()].into()
    );
    assert_eq!(snapshot.to_capabilities_manifest().mcp[0].server_id, id);

    let mut inputs = coordinator.inner.resource_inputs.lock().unwrap().clone();
    inputs
        .agent_activation
        .profile
        .tools
        .sources
        .insert(source.clone(), SourceToolSelection::Exact(Vec::new()));
    let exact = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let exact = coordinator.commit(exact).unwrap();
    assert_helpers(&exact, 0);
    assert!(exact.to_capabilities_manifest().mcp.is_empty());
    assert!(exact.resource_sources().contains(&source));

    // Workflow demand may prepare a source without granting Root read authority.
    inputs.agent_activation.profile.tools.sources.clear();
    let workflow_only = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let workflow_only = coordinator.commit(workflow_only).unwrap();
    assert_helpers(&workflow_only, 0);
    assert!(
        workflow_only
            .resolved_profile()
            .unwrap()
            .resource_sources
            .is_empty()
    );

    let second_id = McpServerId::new("second");
    let second = ToolSourceId::Mcp(second_id.clone());
    inputs
        .mcp_servers
        .insert(second_id, inputs.mcp_servers[&id].clone());
    inputs.source_demand.sources.insert(second.clone());
    inputs.agent_activation.profile.tools.sources = [
        (source.clone(), SourceToolSelection::All),
        (second.clone(), SourceToolSelection::All),
    ]
    .into();
    let several = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let several = coordinator.commit(several).unwrap();
    assert_helpers(&several, 3);
    assert_eq!(
        several.resolved_profile().unwrap().resource_sources,
        [source.clone(), second.clone()].into()
    );
    assert_eq!(several.to_capabilities_manifest().mcp.len(), 2);

    // Generic declarations stay fixed while executor authority shrinks. This
    // must publish even when Workflow demand retains both available sources.
    inputs
        .agent_activation
        .profile
        .tools
        .sources
        .insert(second, SourceToolSelection::Exact(Vec::new()));
    let narrowed = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let narrowed = coordinator.commit(narrowed).unwrap();
    assert_helpers(&narrowed, 3);
    let schemas = |snapshot: &CapabilitySnapshot| {
        snapshot
            .tool_registry()
            .model_definitions()
            .into_iter()
            .map(|tool| (tool.name, tool.input_schema))
            .collect::<Vec<_>>()
    };
    assert_eq!(schemas(&narrowed), schemas(&several));
    assert!(narrowed.revision() > several.revision());
    assert_eq!(narrowed.resource_sources().len(), 2);
    assert_eq!(
        narrowed.resolved_profile().unwrap().resource_sources,
        [source].into()
    );
    assert_eq!(narrowed.to_capabilities_manifest().mcp.len(), 1);

    // Configured bindings alone contribute neither facts nor helpers.
    inputs.source_demand.sources.clear();
    inputs.agent_activation.profile.tools.sources.clear();
    let inert = coordinator
        .prepare_candidate_with_inputs(inputs)
        .await
        .unwrap();
    let inert = coordinator.commit(inert).unwrap();
    assert_helpers(&inert, 0);
    assert!(inert.resource_sources().is_empty());
    assert!(inert.to_capabilities_manifest().mcp.is_empty());
    coordinator.retire_current_mcp_runtimes();
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resource_candidate_epoch_and_last_good_publication_are_atomic() {
    if serve_if_fixture_mode(FixtureServer {
        resources_supported: true,
        tools_disabled: true,
        ..Default::default()
    })
    .await
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "resources",
        "capabilities::coordinator::resource_tests::resource_candidate_epoch_and_last_good_publication_are_atomic",
    );
    let candidate = coordinator.prepare_candidate().await.unwrap();
    assert_helpers(&coordinator.current_snapshot(), 0);
    coordinator.inner.mcp_invalidation.lock().advance(&id);
    assert!(matches!(
        coordinator.commit(candidate),
        Err(CapabilityCommitError::StaleMcpCandidate { .. })
    ));
    assert_helpers(&coordinator.current_snapshot(), 0);
    coordinator.settle_ready_mcp_runtimes().await.unwrap();

    let candidate = coordinator.prepare_candidate().await.unwrap();
    let original = coordinator.commit(candidate).unwrap();
    assert_helpers(&original, 3);
    let mut inputs = coordinator.inner.resource_inputs.lock().unwrap().clone();
    let crate::tools::mcp::McpTransportConfig::Stdio { program, .. } =
        &mut inputs.mcp_servers.get_mut(&id).unwrap().transport
    else {
        unreachable!()
    };
    *program = dir
        .path()
        .join("missing-resource-server")
        .display()
        .to_string();
    assert!(matches!(
        coordinator.prepare_candidate_with_inputs(inputs).await,
        Err(CapabilityPreparationError::Mcp(_))
    ));
    assert!(Arc::ptr_eq(&original, &coordinator.current_snapshot()));
    assert_helpers(&original, 3);
    coordinator.retire_current_mcp_runtimes();
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}
