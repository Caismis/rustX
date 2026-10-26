//! Source approval is resolved before any real MCP Resource request is admitted.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::{oneshot, watch};

use super::{common, support};
use crate::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest, AttemptLifecycle};
use crate::capabilities::{
    AgentActivation, CapabilityCoordinator, CapabilityCoordinatorConfig, ToolSourceId,
};
use crate::events::{AttemptOutcome, RuntimeEvent};
use crate::message::types::{
    ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::{ModelEvent, ModelFinishReason};
use crate::runtime::ExecutionCancellation;
use crate::runtime::identity::{AgentId, AttemptId, McpServerId, MessageId, ToolCallId};
use crate::runtime::interaction::{
    ApprovalDecision, ApprovalFacts, InteractionOutcome, InteractionResponse,
    TestInteractionRendezvous,
};
use crate::runtime::types::{ApprovalMode, CancellationReason};
use crate::tools::executor::{PreflightOutcome, ToolRegistry};
use crate::tools::mcp::{McpServerBinding, McpTransportConfig, fixture};
use crate::tools::types::{
    ToolApprovalPolicy, ToolCancellationPhase, ToolExecutionStatus, ToolInvocationId,
    ToolInvocationPolicy,
};
use support::fake::{FakeStep, ScriptedCall, fake_model, tool_call_events};

#[derive(Clone, Copy, Debug)]
struct ResourceCall {
    name: &'static str,
    tool_id: &'static str,
    method: &'static str,
    read: bool,
}

const HELPERS: [ResourceCall; 3] = [
    ResourceCall {
        name: "list_mcp_resources",
        tool_id: "tool-list_mcp_resources",
        method: "resources/list",
        read: false,
    },
    ResourceCall {
        name: "list_mcp_resource_templates",
        tool_id: "tool-list_mcp_resource_templates",
        method: "resources/templates/list",
        read: false,
    },
    ResourceCall {
        name: "read_mcp_resource",
        tool_id: "tool-read_mcp_resource",
        method: "resources/read",
        read: true,
    },
];

#[derive(Clone, Copy, Debug)]
enum Decision {
    Allow,
    Deny,
    Cancel,
    NoApproval,
}

/// The test controls only the human response; the real configured policy,
/// registry preflight, Agent Loop and MCP transport remain on the call path.
struct ApprovalGate {
    requests: Mutex<Vec<ApprovalFacts>>,
    count: watch::Sender<usize>,
    response: Mutex<Option<oneshot::Receiver<InteractionOutcome>>>,
}

impl TestInteractionRendezvous for ApprovalGate {
    fn request_approval(
        &self,
        facts: ApprovalFacts,
        cancellation: ExecutionCancellation,
    ) -> BoxFuture<'_, InteractionOutcome> {
        let count = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(facts);
            requests.len()
        };
        self.count.send_replace(count);
        let response = self
            .response
            .lock()
            .unwrap()
            .take()
            .expect("one approval rendezvous for the single helper invocation");
        Box::pin(async move {
            tokio::select! {
                outcome = response => outcome.expect("approval response owner remains alive"),
                () = cancellation.cancelled() => InteractionOutcome::Cancelled {
                    reason: cancellation.reason(),
                },
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_resource_helpers_apply_only_the_selected_source_approval_policy() {
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let entry = "boundary_suites::mcp_resources::approval::named_resource_helpers_apply_only_the_selected_source_approval_policy";
    for helper in HELPERS {
        for decision in [Decision::Allow, Decision::Deny, Decision::Cancel] {
            run_case(entry, helper, Some("docs"), decision).await;
        }
        run_case(entry, helper, Some("public"), Decision::NoApproval).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aggregate_resource_helpers_wait_for_approval_before_querying_any_source() {
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let entry = "boundary_suites::mcp_resources::approval::aggregate_resource_helpers_wait_for_approval_before_querying_any_source";
    for helper in HELPERS.into_iter().filter(|helper| !helper.read) {
        for decision in [Decision::Allow, Decision::Deny, Decision::Cancel] {
            run_case(entry, helper, None, decision).await;
        }
    }
}

async fn run_case(
    fixture_entry: &str,
    helper: ResourceCall,
    server: Option<&str>,
    decision: Decision,
) {
    let fixture = common::native_fixture_without_extensions();
    let wires = [
        fixture.dir().path().join("docs-resources.jsonl"),
        fixture.dir().path().join("public-resources.jsonl"),
    ];
    let mut activation = AgentActivation::default();
    activation.profile.tools.builtin.clear();
    let mut servers = BTreeMap::new();
    for ((name, approval), wire) in [
        ("docs", ToolApprovalPolicy::Always),
        ("public", ToolApprovalPolicy::Never),
    ]
    .into_iter()
    .zip(&wires)
    {
        let server_id = McpServerId::new(name);
        activation.profile.tools.sources.insert(
            ToolSourceId::Mcp(server_id.clone()),
            crate::capabilities::selection::SourceToolSelection::All,
        );
        servers.insert(
            server_id,
            McpServerBinding {
                credentials: crate::credentials::SourceCredentials::default(),
                policy: ToolInvocationPolicy {
                    approval,
                    ..ToolInvocationPolicy::default()
                },
                transport: McpTransportConfig::Stdio {
                    program: std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    args: fixture::fixture_spawn_args(fixture_entry),
                    cwd: None,
                    environment: BTreeMap::from([
                        (fixture::FIXTURE_MODE_ENV.into(), "1".into()),
                        (fixture::RESOURCES_ENV.into(), "1".into()),
                        (fixture::TOOLS_DISABLED_ENV.into(), "1".into()),
                        (
                            fixture::resources::RESOURCE_OBSERVATION_FILE_ENV.into(),
                            wire.to_string_lossy().into_owned(),
                        ),
                    ]),
                },
            },
        );
    }
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        conversation_id: fixture.runtime.conversation_id().clone(),
        workspace: fixture.runtime.workspace().clone(),
        environment_store_root: fixture.dir().path().join("environments"),
        source_demand: crate::capabilities::source::ToolSourceDemand::new(
            activation.profile.tools.sources.keys().cloned(),
            crate::runtime::resources::ManagedPythonCatalog::default(),
        ),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: fixture.runtime.extension_tool_plane(),
        agent_activation: activation,
        skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
        mcp_servers: servers,
        base_environment: fixture.runtime.environment().clone(),
    })
    .unwrap();
    let candidate = coordinator.prepare_candidate().await.unwrap();
    let snapshot = coordinator.commit(candidate).unwrap();
    assert!(snapshot.available_tools().tools().is_empty());
    assert_eq!(snapshot.tool_registry().model_definitions().len(), 3);

    let mut arguments = serde_json::json!({});
    if let Some(server) = server {
        arguments["server"] = server.into();
    }
    if helper.read {
        arguments["uri"] = "fixture://text".into();
    }
    let expected_approval = if server == Some("public") {
        ToolApprovalPolicy::Never
    } else {
        ToolApprovalPolicy::Always
    };
    let registration = snapshot
        .tool_registry()
        .registrations()
        .into_iter()
        .find(|registration| registration.definition.name == helper.name)
        .unwrap();
    let fixed = registration
        .prepare_fixed(
            ToolInvocationId::Agent {
                call_id: ToolCallId::new("fixed-resource-approval"),
            },
            &registration.definition,
            &arguments,
        )
        .unwrap();
    assert!(
        matches!(fixed, PreflightOutcome::Ready(ref prepared) if prepared.approval == expected_approval),
        "fixed registration preparation resolves the same source approval: {helper:?}, {server:?}, {fixed:?}"
    );
    assert!(wires.iter().all(|wire| wire_requests(wire).is_empty()));

    let call = ScriptedCall {
        id: "resource-approval-call",
        tool_id: helper.tool_id,
        name: helper.name,
        arguments: arguments.clone(),
    };
    let mut first_turn = vec![FakeStep::Emit(ModelEvent::Started)];
    first_turn.extend(tool_call_events(0, &call).into_iter().map(FakeStep::Emit));
    first_turn.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let model = fake_model(vec![
        first_turn,
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "Resource operation settled.".into(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]);
    let model_snapshot = support::attempt_model(model, "mcp-resource-approval");
    let context = crate::context::ContextRuntime::for_attempt(
        crate::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(crate::context::DefaultTokenEstimator),
        None,
        &model_snapshot,
        crate::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .unwrap();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (response, receiver) = oneshot::channel();
    let (count, mut pending) = watch::channel(0);
    let gate = Arc::new(ApprovalGate {
        requests: Mutex::new(Vec::new()),
        count,
        response: Mutex::new(Some(receiver)),
    });
    let execution = AgentExecution::new(
        AgentExecutionRequest {
            agent_id: AgentId::new("mcp-resource-approval"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new("mcp-resource-approval"),
            conversation: crate::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("resource-approval-user"),
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                        text: "Use the requested resource operation.".into(),
                    })],
                    source: UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    timestamp: None,
                }),
            ])
            .unwrap(),
            initial_turn_trigger: crate::agent::InitialTurnTrigger::Continuation,
            model: model_snapshot,
        },
        coordinator.acquire_attempt_lease(),
        &cancellation,
        support::default_execution_policy(),
        context,
        &fixture.runtime,
        AttemptLifecycle::inert()
            .with_approval_mode(ApprovalMode::Policy)
            .with_test_interaction_rendezvous(gate.clone()),
    )
    .unwrap();
    let mut run = Box::pin(execution.run());
    if !matches!(decision, Decision::NoApproval) {
        tokio::time::timeout(Duration::from_secs(30), async {
            tokio::select! {
                result = &mut run => panic!(
                    "source approval must precede completion: {helper:?}, {server:?}, {decision:?}, {result:?}"
                ),
                ready = pending.wait_for(|count| *count == 1) => {
                    ready.expect("approval waiter publishes its rendezvous");
                },
            }
        })
        .await
        .expect("source approval reaches the deterministic pending gate");
        let facts = gate.requests.lock().unwrap()[0].clone();
        assert_eq!(facts.tool_name, helper.name);
        assert_eq!(facts.arguments, arguments);
        assert!(
            wires.iter().all(|wire| wire_requests(wire).is_empty()),
            "pending approval must not query any source, including Never siblings"
        );
        match decision {
            Decision::Allow | Decision::Deny => response
                .send(InteractionOutcome::Responded {
                    response: InteractionResponse::Approval {
                        decision: if matches!(decision, Decision::Allow) {
                            ApprovalDecision::Allow
                        } else {
                            ApprovalDecision::Deny {
                                reason: "resource request denied".into(),
                            }
                        },
                    },
                })
                .expect("the pending approval receiver remains live"),
            Decision::Cancel => {
                assert!(cancellation.request_cancel(CancellationReason::UserRequested));
            }
            Decision::NoApproval => unreachable!(),
        }
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        if matches!(decision, Decision::NoApproval) {
            tokio::select! {
                result = &mut run => result,
                ready = pending.wait_for(|count| *count != 0) => {
                    ready.expect("approval signal stays live");
                    panic!("named Never source must run without approving its Always sibling: {helper:?}");
                },
            }
        } else {
            run.await
        }
    })
    .await
    .expect("resource approval and execution settle");
    let audit = common::durable_agent_result(result, fixture.store.as_ref());
    let tool_results: Vec<_> = audit
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(&tool.result.status),
            _ => None,
        })
        .collect();
    assert_eq!(tool_results.len(), 1, "one canonical result per invocation");
    let executed = matches!(decision, Decision::Allow | Decision::NoApproval);
    match decision {
        Decision::Allow | Decision::NoApproval => {
            assert_eq!(*tool_results[0], ToolExecutionStatus::Success);
            assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
        }
        Decision::Deny => {
            assert!(
                matches!(tool_results[0], ToolExecutionStatus::Denied { reason } if reason == "resource request denied")
            );
            assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
        }
        Decision::Cancel => {
            assert!(matches!(
                tool_results[0],
                ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::UserRequested,
                    phase: ToolCancellationPhase::BeforeStart,
                }
            ));
            assert!(matches!(
                audit.outcome,
                AttemptOutcome::Cancelled {
                    reason: CancellationReason::UserRequested
                }
            ));
        }
    }
    assert_eq!(
        gate.requests.lock().unwrap().len(),
        usize::from(!matches!(decision, Decision::NoApproval))
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        usize::from(executed)
    );
    for (name, wire) in ["docs", "public"].into_iter().zip(&wires) {
        let expected = if executed && server.is_none_or(|selected| selected == name) {
            vec![serde_json::json!({
                "method": helper.method,
                "value": if helper.read { Some("fixture://text") } else { None },
            })]
        } else {
            Vec::new()
        };
        assert_eq!(
            wire_requests(wire),
            expected,
            "only approved selected sources receive exactly one request: {helper:?}, {server:?}, {decision:?}, {name}"
        );
    }
    coordinator.retire_current_mcp_runtimes();
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}

fn wire_requests(path: &Path) -> Vec<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("read resource request journal: {error}"),
    }
}
