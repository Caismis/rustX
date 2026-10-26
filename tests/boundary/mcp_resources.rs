//! A real resource-only MCP source enters context solely through a committed Tool call.
use std::collections::BTreeMap;
use std::sync::Arc;

use super::{common, support};
use crate::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use crate::capabilities::{
    AgentActivation, CapabilityCoordinator, CapabilityCoordinatorConfig, ToolSourceId,
};
use crate::events::{AttemptOutcome, RuntimeEvent};
use crate::message::types::{
    ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::{ModelEvent, ModelFinishReason};
use crate::runtime::identity::{AgentId, AttemptId, McpServerId, MessageId};
use crate::runtime::types::CancellationReason;
use crate::tools::executor::ToolRegistry;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig, fixture};
use support::fake::{FakeStep, ScriptedCall, fake_model, model_release, tool_call_events};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resource_body_enters_context_only_after_one_committed_normal_tool_result() {
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let wire = fixture.dir().path().join("resource-requests.jsonl");
    let server_id = McpServerId::new("resources");
    let source = ToolSourceId::Mcp(server_id.clone());
    let mut activation = AgentActivation::default();
    activation.profile.tools.builtin.clear();
    activation.profile.tools.sources.insert(
        source.clone(),
        crate::capabilities::selection::SourceToolSelection::All,
    );
    let binding = McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        policy: crate::tools::types::ToolInvocationPolicy::default(),
        transport: McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: fixture::fixture_spawn_args(
                "boundary_suites::mcp_resources::resource_body_enters_context_only_after_one_committed_normal_tool_result",
            ),
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
    };
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        conversation_id: fixture.runtime.conversation_id().clone(),
        workspace: fixture.runtime.workspace().clone(),
        environment_store_root: fixture.dir().path().join("environments"),
        source_demand: crate::capabilities::source::ToolSourceDemand::new(
            [source],
            crate::runtime::resources::ManagedPythonCatalog::default(),
        ),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: fixture.runtime.extension_tool_plane(),
        agent_activation: activation,
        skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
        mcp_servers: [(server_id, binding)].into(),
        base_environment: fixture.runtime.environment().clone(),
    })
    .unwrap();
    let candidate = coordinator.prepare_candidate().await.unwrap();
    let snapshot = coordinator.commit(candidate).unwrap();
    assert!(
        snapshot.available_tools().tools().is_empty(),
        "no fabricated remote Tools"
    );
    let declarations = snapshot.tool_registry().model_definitions();
    assert_eq!(
        declarations
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource"
        ]
    );
    assert!(
        !wire.exists(),
        "capability discovery never enumerates or reads Resources"
    );

    let (release, released) = model_release();
    let call = ScriptedCall {
        id: "resource-read",
        tool_id: "tool-read_mcp_resource",
        name: "read_mcp_resource",
        arguments: serde_json::json!({"server":"resources", "uri":"fixture://text"}),
    };
    let mut first_turn = vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilReleased(released),
    ];
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
                text: "Read complete.".into(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]);
    let model_snapshot = support::attempt_model(model.clone(), "mcp-resource-context");
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
    let execution = AgentExecution::new(
        AgentExecutionRequest {
            agent_id: AgentId::new("mcp-resource-context"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new("mcp-resource-context"),
            conversation: crate::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("resource-user"),
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                        text: "Read the remote resource.".into(),
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
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    let mut run = Box::pin(execution.run());
    let mut parked = model.parked();
    tokio::select! {
        result = &mut run => panic!("attempt completed before its first Tool proposal: {result:?}"),
        () = support::fake::await_started(&mut parked, "first resource model request") => {},
    }
    let initial = model.requests();
    assert_eq!(
        initial.len(),
        1,
        "discovery does not add a model invocation"
    );
    assert_eq!(initial[0].tools, declarations);
    let body = "peer content for fixture://text";
    assert!(!serde_json::to_string(&initial[0]).unwrap().contains(body));
    assert!(!wire.exists(), "declaring helpers cannot read a Resource");
    release.send_replace(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), run)
        .await
        .expect("resource read and next model response settle");
    let audit = common::durable_agent_result(result, fixture.store.as_ref());
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
    let requests = model.requests();
    assert_eq!(
        requests.len(),
        2,
        "one Tool round adds exactly one subsequent model invocation"
    );
    assert_eq!(requests[1].tools, declarations);
    assert_eq!(
        requests[0].effective_system_prompt,
        requests[1].effective_system_prompt
    );
    assert!(
        serde_json::to_string(&requests[1].messages)
            .unwrap()
            .contains(body)
    );
    assert_eq!(
        audit.messages().len(),
        4,
        "only User, Tool-call Assistant, Tool result and final Assistant are canonical"
    );
    assert!(
        audit
            .messages()
            .iter()
            .filter(|message| !matches!(message, MessageBlock::Tool(_)))
            .all(|message| !serde_json::to_string(message).unwrap().contains(body)),
        "resource content is committed only as untrusted Tool output"
    );
    let results: Vec<_> = audit
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].result.status,
        crate::tools::ToolExecutionStatus::Success
    );
    assert!(
        serde_json::to_string(&results[0].result.content)
            .unwrap()
            .contains(body)
    );
    for predicate in [
        (|event: &RuntimeEvent| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            as fn(&RuntimeEvent) -> bool,
        |event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }),
        |event| matches!(event, RuntimeEvent::ToolMessageCommitted { .. }),
    ] {
        assert_eq!(
            audit
                .event_history
                .iter()
                .filter(|event| predicate(event))
                .count(),
            1
        );
    }
    assert!(matches!(
        support::audit::assert_single_terminal(&audit.event_history),
        RuntimeEvent::AttemptCompleted { .. }
    ));
    let requests: Vec<serde_json::Value> = std::fs::read_to_string(wire)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests,
        [serde_json::json!({"method":"resources/read", "value":"fixture://text"})]
    );
    coordinator.retire_current_mcp_runtimes();
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}
