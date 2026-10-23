//! Companion to the source timing matrix: an actual `AgentExecution` request at
//! the scripted provider-adapter boundary, rather than a composition proxy.
use super::{
    BTreeMap, CapabilitySourceState, ConversationId, ConversationToolRuntime, HttpServer,
    McpServerId, Measurement, Profile, Serialize, Workspace, coordinator, milliseconds,
    prepare_initial,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct TimedModel {
    inner: Arc<crate::scripted_suites::support::fake::FakeModel>,
    first_request: Mutex<Option<(Instant, usize)>>,
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
            .get_or_insert((Instant::now(), request.tools.len()));
        self.inner.stream(request, cancellation)
    }
}

#[derive(Serialize)]
struct ModelRow {
    profile: Profile,
    servers: usize,
    warm: bool,
    cache_eligible_hits: usize,
    composition_settled_ms: f64,
    composition_error: Option<String>,
    first_model_request_ms: Option<f64>,
    attempt_settled_ms: Option<f64>,
    model_request_count: usize,
    first_request_tool_count: usize,
    initial_connecting_sources: usize,
    startup_requests: BTreeMap<&'static str, usize>,
}

#[allow(
    clippy::too_many_lines,
    reason = "complete real runtime fixture with explicit timing scope"
)]
async fn model_measure(servers: usize, profile: Profile, warm: bool) -> ModelRow {
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
    if warm {
        let seed = coordinator(root.path(), bindings.clone());
        let candidate = seed.prepare_candidate().await.unwrap();
        seed.commit(candidate).unwrap();
        seed.drain_conversation_owned().await.unwrap();
    }
    let owner = coordinator(root.path(), bindings.clone());
    let workspace = Workspace::new(root.path().join("workspace")).unwrap();
    let cache_eligible_hits = bindings
        .iter()
        .filter(|(id, binding)| {
            super::CatalogCacheKey::for_binding(id, binding, &workspace)
                .and_then(|key| super::McpCatalogCache::process().lookup(&key))
                .is_some()
        })
        .count();
    assert_eq!(cache_eligible_hits, if warm { servers } else { 0 });
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
    let prepared = prepare_initial(&owner).await;
    let composition_settled_ms = milliseconds(start.elapsed());
    let candidate = match prepared {
        Ok(candidate) => candidate,
        Err(error) => {
            assert!(owner.settle_failed_startup().await.is_empty());
            for server in fixtures {
                server.shutdown().await;
            }
            return ModelRow {
                profile,
                servers,
                warm,
                cache_eligible_hits,
                composition_settled_ms,
                composition_error: Some(error.to_string()),
                first_model_request_ms: None,
                attempt_settled_ms: None,
                model_request_count: 0,
                first_request_tool_count: 0,
                initial_connecting_sources: 0,
                startup_requests: meter.lock().unwrap().requests.clone(),
            };
        }
    };
    let snapshot = owner.commit(candidate).unwrap();
    let initial_tool_count = snapshot.tool_registry().definitions().len();
    let initial_connecting_sources = owner
        .availability()
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Connecting { .. }))
        .count();
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
        capability: owner.clone(),
        resources: Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
            crate::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            snapshot,
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
    let (requested, tool_count) = model.first_request.lock().unwrap().unwrap();
    assert_eq!(model.inner.requests().len(), 1);
    assert_eq!(tool_count, initial_tool_count);
    // Finish the owned startup batch after measuring the first request, so
    // fixture teardown does not cancel remote handlers or hide network work.
    if let Some(mut readiness) = owner.startup_readiness() {
        readiness.wait_for(|progress| progress.done).await.unwrap();
    }
    runtime.shutdown().await.unwrap();
    for server in fixtures {
        server.shutdown().await;
    }
    ModelRow {
        profile,
        servers,
        warm,
        cache_eligible_hits,
        composition_settled_ms,
        composition_error: None,
        first_model_request_ms: Some(milliseconds(requested.duration_since(start))),
        attempt_settled_ms: Some(attempt_settled_ms),
        model_request_count: model.inner.requests().len(),
        first_request_tool_count: tool_count,
        initial_connecting_sources,
        startup_requests: meter.lock().unwrap().requests.clone(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "controlled actual model-request timing; run explicitly"]
async fn first_model_request_timing() {
    let mut rows = Vec::new();
    for profile in [Profile::Fast, Profile::Slow, Profile::Failed] {
        for servers in [0, 1, 4, 8] {
            for warm in [false, true] {
                let row = tokio::time::timeout(
                    Duration::from_secs(30),
                    model_measure(servers, profile, warm),
                )
                .await
                .expect("model timing row settles");
                println!("{}", serde_json::to_string(&row).unwrap());
                rows.push(row);
            }
        }
    }
    let evidence = serde_json::json!({
        "measurement": "real ConversationRuntime inbound -> AgentExecution -> scripted ModelAdapter::stream(request); timestamp recorded at adapter entry",
        "timing_scope": "starts immediately before native source preparation, includes runtime construction, activation, admission and context assembly; authored config, fixture servers and tool-runtime storage setup are outside the clock",
        "provider": "scripted model, one real ModelRequest per successful composition; no external provider network latency",
        "rows": rows,
    });
    println!("{}", serde_json::to_string_pretty(&evidence).unwrap());
    if let Some(path) = std::env::var_os("RUSTX_MODEL_STARTUP_TIMING_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
}
