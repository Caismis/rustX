//! Real Client stdio generations coexist across ordinary capability activation.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use super::*;
use crate::capabilities::selection::SourceToolSelection;
use crate::tools::mcp::fixture::{
    FIXTURE_MODE_ENV, FixtureServer, TOOL_PREFIX_ENV, fixture_spawn_args, serve_if_fixture_mode,
};

const CAPACITY: usize = rustx_execution_protocol::mcp::MAX_GENERATION_SESSIONS;
const MARKER: &str = "RUSTX_MCP_GENERATION_MARKER";
const TEST: &str = "capabilities::coordinator::mcp_capacity_tests::full_stdio_generation_rotates_without_revoking_admitted_leases";

fn configure(inputs: &mut CapabilityResourceInputs, phase: &str, count: usize) {
    for index in 0..count {
        let id = McpServerId::new(format!("source-{index}"));
        let source = ToolSourceId::Mcp(id.clone());
        inputs.source_demand.sources.insert(source.clone());
        inputs
            .agent_activation
            .profile
            .tools
            .sources
            .insert(source, SourceToolSelection::All);
        inputs.mcp_servers.insert(
            id,
            crate::tools::mcp::McpServerBinding {
                transport: crate::tools::mcp::McpTransportConfig::Stdio {
                    program: std::env::current_exe()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    args: fixture_spawn_args(TEST),
                    cwd: None,
                    environment: [
                        (FIXTURE_MODE_ENV.into(), "1".into()),
                        (TOOL_PREFIX_ENV.into(), format!("source_{index}_")),
                        (MARKER.into(), format!("{phase}-{index}")),
                    ]
                    .into(),
                },
                policy: crate::tools::types::ToolInvocationPolicy::default(),
                credentials: crate::credentials::SourceCredentials::default(),
                permissions: None,
                tool_permissions: BTreeMap::new(),
            },
        );
    }
}

fn physical_launches(workspace: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn report_generation_measurement(
    launches: usize,
    [cold, preparation, activation, drain]: [Duration; 4],
) {
    if std::env::var_os("RUSTX_MCP_GENERATION_MEASUREMENT").is_some() {
        println!(
            "MCP_GENERATION_MEASUREMENT {}",
            serde_json::json!({
                "sources_per_generation": CAPACITY,
                "physical_launches": launches,
                "cold_preparation_ms": cold.as_secs_f64() * 1000.0,
                "replacement_preparation_ms": preparation.as_secs_f64() * 1000.0,
                "activation_ms": activation.as_secs_f64() * 1000.0,
                "old_lease_drain_ms": drain.as_secs_f64() * 1000.0,
            })
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_stdio_generation_rotates_without_revoking_admitted_leases() {
    if let Some(name) = std::env::var_os(MARKER) {
        std::fs::File::create_new(name).unwrap();
        assert!(serve_if_fixture_mode(FixtureServer::from_env()).await);
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("work");
    std::fs::create_dir(&workspace).unwrap();
    let execution = crate::session_execution_support::ExecutionFixture::new(&workspace);
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: crate::capabilities::source::ToolSourceDemand::default(),
        conversation_id: ConversationId::new("conv_021d548e-9b03-751b-80f9-68be31c19199"),
        workspace: Workspace::for_session(&workspace, execution.scope.clone()).unwrap(),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: crate::extensions::ExtensionToolPlane::none(),
        agent_activation: AgentActivation::default(),
        skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
        mcp_servers: BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: temporary.path().join("store"),
    })
    .unwrap();

    let mut inputs = coordinator.inner.resource_inputs.lock().unwrap().clone();
    configure(&mut inputs, "old", CAPACITY);
    let cold_start = Instant::now();
    let first = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let cold = cold_start.elapsed();
    let retained: Vec<_> = first
        .mcp_runtimes
        .iter()
        .map(|generation| generation.connection().published_runtime().unwrap())
        .collect();
    assert_eq!(retained.len(), CAPACITY);
    let old = coordinator.commit(first).unwrap();
    let leases = old.acquire_mcp_leases().unwrap();

    configure(&mut inputs, "new", CAPACITY);
    let replacement_start = Instant::now();
    let replacement = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let preparation = replacement_start.elapsed();
    assert_eq!(replacement.mcp_runtimes.len(), CAPACITY);
    assert_eq!(
        replacement.candidate_registry.definitions(),
        old.tool_registry().definitions()
    );
    let activation_start = Instant::now();
    let active = coordinator.commit(replacement).unwrap();
    let activation = activation_start.elapsed();
    assert_eq!(active.revision().get(), old.revision().get() + 1);
    assert_eq!(coordinator.pending_mcp_retirements(), CAPACITY);
    for runtime in &retained {
        assert!(
            runtime
                .list_tools()
                .await
                .unwrap()
                .iter()
                .any(|tool| tool.name.ends_with("echo"))
        );
    }
    let expected: Vec<_> = ["new", "old"]
        .into_iter()
        .flat_map(|phase| (0..CAPACITY).map(move |index| format!("{phase}-{index}")))
        .collect();
    assert_eq!(physical_launches(&workspace), expected);
    configure(&mut inputs, "excess", CAPACITY + 1);
    let refused = coordinator.prepare_candidate_with_inputs(inputs).await;
    match refused {
        Err(CapabilityPreparationError::Mcp(detail)) => {
            assert!(detail.contains("at most 8 Client-owned stdio MCP sources"));
        }
        _ => panic!("a generation above capacity must fail before launching"),
    }
    assert_eq!(coordinator.current_snapshot().revision(), active.revision());
    assert_eq!(physical_launches(&workspace), expected);

    let drain_start = Instant::now();
    drop(leases);
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    let drain = drain_start.elapsed();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    for runtime in &retained {
        assert!(runtime.list_tools().await.is_err());
    }
    coordinator.drain_conversation_owned().await.unwrap();
    execution.settle().await;
    report_generation_measurement(expected.len(), [cold, preparation, activation, drain]);
}
