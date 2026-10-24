//! One benchmark matrix, measured at the actual `ModelAdapter::stream` boundary.
use super::{
    BTreeMap, CapabilitySourceState, ConversationId, ConversationToolRuntime, HttpServer,
    McpServerId, Measurement, Profile, Serialize, Workspace, coordinator, first_use, milliseconds,
    prepare_initial,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct TimedModel {
    inner: Arc<crate::scripted_suites::support::fake::FakeModel>,
    first_request: Mutex<Option<(Instant, Vec<crate::tools::types::ModelToolDefinition>)>>,
}
impl crate::model::ModelAdapter for TimedModel {
    fn protocol(&self) -> crate::model::ModelProtocol {
        self.inner.protocol()
    }
    fn stream(
        &self,
        request: crate::model::ModelRequest,
        cancellation: crate::runtime::cancellation::CancellationSignal,
    ) -> crate::model::ModelStream {
        self.first_request
            .lock()
            .unwrap()
            .get_or_insert_with(|| (Instant::now(), request.tools.clone()));
        self.inner.stream(request, cancellation)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConversationOrder {
    Fresh,
    FreshAfterEarlierConversation,
}

#[derive(Serialize)]
struct Row {
    profile: Profile,
    servers: usize,
    conversation_order: ConversationOrder,
    earlier_conversation_requests: BTreeMap<&'static str, usize>,
    source_batch_settled_ms: f64,
    terminal_capability_ready_ms: Option<f64>,
    ready_sources: usize,
    unavailable_sources: usize,
    first_model_request_ms: Option<f64>,
    attempt_settled_ms: Option<f64>,
    startup_failure: Option<String>,
    model_request_count: usize,
    first_request_tools: Vec<crate::tools::types::ModelToolDefinition>,
    first_tool_dispatch_ms: Option<f64>,
    first_tool_completion_ms: Option<f64>,
    first_tool_elapsed_ms: Option<f64>,
    startup_protocol_requests: BTreeMap<&'static str, usize>,
    total_protocol_requests: BTreeMap<&'static str, usize>,
    http_requests: BTreeMap<String, usize>,
    server_handler_instances: usize,
    peak_in_flight_phases: usize,
    active_phases_after_shutdown: usize,
    spawned_server_processes: usize,
}

#[allow(
    clippy::too_many_lines,
    reason = "one measurement preserves exact request, timestamp, and physical cleanup ordering"
)]
async fn measure(servers: usize, profile: Profile, order: ConversationOrder) -> Row {
    use crate::message::content::TextBlock;
    use crate::message::types::{ContentBlockIndex, UserContentBlock};
    use crate::model::{ModelEvent, ModelFinishReason};
    use crate::runtime::conversation_runtime::{
        ConversationContextConfig, ConversationRuntime, RuntimeConversationConfig,
    };
    use crate::scripted_suites::support::fake::{FakeStep, fake_model};

    let root = tempfile::tempdir().unwrap();
    let meter = Arc::new(Mutex::new(Measurement::new(Profile::Fast)));
    let mut fixtures = Vec::new();
    let mut bindings = BTreeMap::new();
    for index in 0..servers {
        let name = format!("startup-{index:02}");
        let server = HttpServer::start(name.clone(), meter.clone()).await;
        bindings.insert(McpServerId::new(name), server.binding.clone());
        fixtures.push(server);
    }
    if matches!(order, ConversationOrder::FreshAfterEarlierConversation) {
        // This separate native Conversation owner completes live discovery at
        // the same endpoints/workspace, then closes every connection. Its work
        // is outside the measured clock and cannot authorize the fresh owner.
        let seed = coordinator(
            root.path(),
            bindings.clone(),
            "conv_9f19ac05-7a2a-7707-beb8-6dcb7c9070e9",
        );
        let candidate = seed.prepare_candidate().await.unwrap();
        seed.commit(candidate).unwrap();
        seed.drain_conversation_owned().await.unwrap();
    }
    let earlier_conversation_requests = meter.lock().unwrap().requests.clone();
    let owner = coordinator(
        root.path(),
        bindings,
        "conv_9f19ac05-7a2a-7707-beb8-6dcb7c9070e8",
    );
    let workspace = Workspace::new(root.path().join("workspace")).unwrap();
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("conv_9f19ac05-7a2a-7707-beb8-6dcb7c9070e8"),
        workspace.root(),
        root.path().join("artifacts"),
    )
    .unwrap();
    let model = Arc::new(TimedModel {
        inner: fake_model(vec![vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".into(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]]),
        first_request: Mutex::new(None),
    });
    *meter.lock().unwrap() = Measurement::new(profile);
    let start = meter.lock().unwrap().start;
    let candidate = match prepare_initial(&owner).await {
        Ok(candidate) => candidate,
        Err(error) => {
            // Archived main can reject the source-wide selection after an
            // external failure. Record that actual boundary instead of
            // modifying baseline product behavior or inventing a request.
            assert!(matches!(profile, Profile::Failed));
            let source_batch_settled_ms = milliseconds(start.elapsed());
            owner.drain_conversation_owned().await.unwrap();
            for server in fixtures {
                server.shutdown().await;
            }
            let measurement = meter.lock().unwrap();
            assert_eq!(measurement.active, 0);
            assert!(measurement.peak <= 4);
            assert_eq!(measurement.requests.get("handshake"), Some(&servers));
            assert_eq!(measurement.requests.get("tools/list"), Some(&servers));
            assert!(!measurement.requests.contains_key("tools/call"));
            assert!(model.inner.requests().is_empty());
            return Row {
                profile,
                servers,
                conversation_order: order,
                earlier_conversation_requests,
                source_batch_settled_ms,
                terminal_capability_ready_ms: None,
                ready_sources: 0,
                unavailable_sources: 0,
                first_model_request_ms: None,
                attempt_settled_ms: None,
                startup_failure: Some(error.to_string()),
                model_request_count: 0,
                first_request_tools: Vec::new(),
                first_tool_dispatch_ms: None,
                first_tool_completion_ms: None,
                first_tool_elapsed_ms: None,
                startup_protocol_requests: measurement.requests.clone(),
                total_protocol_requests: measurement.requests.clone(),
                http_requests: measurement.http_requests.clone(),
                server_handler_instances: measurement.server_handler_instances,
                peak_in_flight_phases: measurement.peak,
                active_phases_after_shutdown: measurement.active,
                spawned_server_processes: 0,
            };
        }
    };
    let source_batch_settled_ms = milliseconds(start.elapsed());
    let snapshot = owner.commit(candidate).unwrap();
    let terminal_capability_ready_ms = milliseconds(start.elapsed());
    let expected_model_tools = snapshot.tool_registry().model_definitions();
    let expected_tools = if matches!(profile, Profile::Failed) {
        0
    } else {
        servers
    };
    assert_eq!(expected_model_tools.len(), expected_tools);
    let availability = owner.availability();
    let ready_sources = availability
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Ready))
        .count();
    let unavailable_sources = availability
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Unavailable { .. }))
        .count();
    assert_eq!(ready_sources, expected_tools);
    assert_eq!(unavailable_sources, servers - expected_tools);
    let startup_protocol_requests = meter.lock().unwrap().requests.clone();
    assert_eq!(
        startup_protocol_requests
            .get("handshake")
            .copied()
            .unwrap_or(0),
        servers
    );
    assert_eq!(
        startup_protocol_requests
            .get("tools/list")
            .copied()
            .unwrap_or(0),
        servers
    );
    assert_eq!(
        startup_protocol_requests
            .get("tools/call")
            .copied()
            .unwrap_or(0),
        0
    );
    let adapter: Arc<dyn crate::model::ModelAdapter> = model.clone();
    let runtime = ConversationRuntime::new(RuntimeConversationConfig {
        explicit_model: true,
        agent_id: crate::runtime::identity::AgentId::new("startup-measurement"),
        model: crate::scripted_suites::support::model::scripted_session_model(adapter),
        approval_mode: crate::runtime::ApprovalMode::Policy,
        model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        context: ConversationContextConfig {
            policy: crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            estimator: Arc::new(crate::context::DefaultTokenEstimator),
            status_engine: None,
        },
        tool_runtime,
        capability: owner,
        resources: Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
            crate::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            snapshot.clone(),
        )),
        clock: None,
        initial_messages: Vec::new(),
        subagents: None,
        workflow_output: None,
    })
    .unwrap();
    runtime.activate();
    let after = runtime.settlement_ordinal_for_test();
    runtime
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "Reply once without calling a tool.".into(),
        })])
        .unwrap();
    runtime.child_attempt_settlement(after).await;
    let attempt_settled_ms = milliseconds(start.elapsed());
    let (requested, first_request_tools) = model.first_request.lock().unwrap().clone().unwrap();
    let model_request_count = model.inner.requests().len();
    assert_eq!(model_request_count, 1);
    assert_eq!(first_request_tools, expected_model_tools);
    let first_tool_elapsed_ms = first_use(snapshot.tool_registry(), runtime.tool_runtime()).await;
    let first_tool_completion_ms = first_tool_elapsed_ms.map(|_| milliseconds(start.elapsed()));
    drop(snapshot);
    runtime.shutdown().await.unwrap();
    for server in fixtures {
        server.shutdown().await;
    }
    let measurement = meter.lock().unwrap();
    assert_eq!(measurement.active, 0);
    assert!(measurement.peak <= 4);
    assert_eq!(
        measurement.requests.get("handshake").copied().unwrap_or(0),
        servers
    );
    assert_eq!(
        measurement.requests.get("tools/list").copied().unwrap_or(0),
        servers
    );
    assert_eq!(
        measurement.requests.get("tools/call").copied().unwrap_or(0),
        usize::from(expected_tools > 0)
    );
    Row {
        profile,
        servers,
        conversation_order: order,
        earlier_conversation_requests,
        source_batch_settled_ms,
        terminal_capability_ready_ms: Some(terminal_capability_ready_ms),
        ready_sources,
        unavailable_sources,
        first_model_request_ms: Some(milliseconds(requested.duration_since(start))),
        attempt_settled_ms: Some(attempt_settled_ms),
        startup_failure: None,
        model_request_count,
        first_request_tools,
        first_tool_dispatch_ms: measurement.first_call.map(milliseconds),
        first_tool_completion_ms,
        first_tool_elapsed_ms,
        startup_protocol_requests,
        total_protocol_requests: measurement.requests.clone(),
        http_requests: measurement.http_requests.clone(),
        server_handler_instances: measurement.server_handler_instances,
        peak_in_flight_phases: measurement.peak,
        active_phases_after_shutdown: measurement.active,
        spawned_server_processes: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "controlled live-startup benchmark; run explicitly with --ignored --nocapture"]
async fn bounded_live_startup_timing() {
    let mut rows = Vec::new();
    for profile in [Profile::Fast, Profile::Slow, Profile::Failed] {
        for servers in [0, 1, 4, 8] {
            for order in [
                ConversationOrder::Fresh,
                ConversationOrder::FreshAfterEarlierConversation,
            ] {
                let row =
                    tokio::time::timeout(Duration::from_secs(30), measure(servers, profile, order))
                        .await
                        .expect("one measurement settles inside its fixture watchdog");
                println!("{}", serde_json::to_string(&row).unwrap());
                rows.push(row);
            }
        }
    }
    let evidence = serde_json::json!({
        "schema": "issue-499-bounded-live-startup-v1",
        "measurement": "real ConversationRuntime inbound -> AgentExecution -> ModelAdapter::stream(request); timestamp recorded at adapter entry",
        "timing_scope": "starts immediately before native source preparation; includes candidate commit, runtime construction, activation, admission and context assembly; authored config, loopback servers and tool storage setup are outside the clock",
        "provider": "scripted adapter; exactly one actual ModelRequest per row; no external provider network latency",
        "transport": "real loopback Streamable HTTP; client and server share one test process; server processes are therefore exactly zero",
        "handler_count_scope": "server_handler_instances counts SDK request-handler factory invocations, not physical connections or MCP sessions; handshake counts independently prove one live initialization per source",
        "first_tool_scope": "first affected MCP tools/call through the captured native executor after the measured model Attempt settles; not a model-generated tool step",
        "capability_scope": "terminal_capability_ready_ms includes Ready and explicit Unavailable outcomes, not successful readiness of failed sources",
        "baseline_failure_scope": "a preparation rejection records startup_failure and null capability/request/Attempt timestamps with zero model requests; it is not converted into optional success by the baseline adapter",
        "fresh_repeat_scope": "a separate Conversation native source owner first discovers the same endpoints/workspace with fast responses, closes its connections, and is excluded from the measured clock and request counts",
        "fast_phase_delay_ms": 10,
        "slow_and_failed_phase_delay_ms": 1500,
        "failed_phase": "tools/list returns a protocol error after its controlled delay",
        "fixture_watchdog_seconds": 30,
        "timing_assertions": "none; exact model/tool/protocol counts and tool inventories are asserted; controlled delays are workload inputs",
        "rows": rows,
    });
    println!("{}", serde_json::to_string_pretty(&evidence).unwrap());
    if let Some(path) = std::env::var_os("RUSTX_STARTUP_TIMING_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
}
