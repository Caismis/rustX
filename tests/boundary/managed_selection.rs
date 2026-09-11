//! CFG-03 × `FastMCP` 4: real prepared sources enter the single frozen Agent
//! registry. Source failure and exposure filtering remain independent.
#![cfg(unix)]

use std::sync::Arc;

use super::{common, support};
use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::capabilities::{
    CapabilityCoordinator, CapabilityCoordinatorConfig, CapabilityPreparationError,
    CapabilityResourceInputs, CapabilitySourceId, CapabilitySourceState, ToolActivationPolicy,
};
use rustx::events::AttemptOutcome;
use rustx::events::types::{AttemptFailure, RuntimeEvent};
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::{ModelEvent, ModelFinishReason};
use rustx::runtime::identity::{AgentId, AttemptId, MessageId};
use rustx::runtime::types::{CancellationReason, RuntimeError};
use rustx::tools::python::python_server_id;
use support::fake::{FakeStep, ScriptedCall, fake_model, tool_call_events};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fastmcp4_availability_selection_request_and_invocation_share_one_authority() {
    // CI's boundary jobs provide uv; absence must not make this acceptance green.
    for (selection, admitted) in [
        (ToolActivationPolicy::default(), true),
        (
            ToolActivationPolicy {
                tools: Some(vec!["ping".into()]),
                ..Default::default()
            },
            true,
        ),
        (
            ToolActivationPolicy {
                no_tools: true,
                ..Default::default()
            },
            false,
        ),
        (
            ToolActivationPolicy {
                exclude_tools: vec!["ping".into()],
                ..Default::default()
            },
            false,
        ),
    ] {
        let fixture = common::native_fixture_without_extensions();
        let root = fixture.runtime.workspace().root();
        let marker = root.join("calls.txt");
        for (name, requirements, tool) in [
            ("healthy", "# none\n", "ping"),
            ("conflicting", "pydantic<2.12\n", "failed_identity"),
        ] {
            let package = root.join(".agents/tools").join(name);
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(package.join("requirements.txt"), requirements).unwrap();
            std::fs::write(package.join("server.py"), format!(
                "from fastmcp import FastMCP\nmcp = FastMCP({name:?})\n@mcp.tool\ndef {tool}() -> str:\n    with open({marker:?}, 'a') as calls:\n        calls.write('called\\n')\n    return 'pong'\n",
                marker = marker.to_str().unwrap(),
            )).unwrap();
        }
        let mut inputs = CapabilityResourceInputs {
            python_sources: ["healthy", "conflicting"]
                .map(|name| {
                    (
                        python_server_id(name),
                        rustx::capabilities::activation::SourceActivation::Enabled,
                    )
                })
                .into(),
            base_tool_registry: Arc::new(fixture.registry.clone()),
            tool_activation: selection.clone(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![],
                explicit_paths: vec![],
            },
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: fixture.runtime.environment().clone(),
        };
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: fixture.runtime.conversation_id().clone(),
            workspace: fixture.runtime.workspace().clone(),
            environment_store_root: fixture.dir().path().join("environments"),
            python_sources: inputs.python_sources.clone(),
            base_tool_registry: inputs.base_tool_registry.clone(),
            extension_tools: fixture.runtime.extension_tool_plane(),
            tool_activation: selection.clone(),
            skill_discovery: inputs.skill_discovery.clone(),
            mcp_servers: inputs.mcp_servers.clone(),
            base_environment: inputs.base_environment.clone(),
        })
        .unwrap();
        let candidate = tokio::time::timeout(
            std::time::Duration::from_mins(2),
            coordinator.prepare_candidate(),
        )
        .await
        .expect("source preparation liveness guard")
        .unwrap();
        assert_eq!(
            candidate
                .availability()
                .get(&CapabilitySourceId::Mcp(python_server_id("healthy"))),
            Some(&CapabilitySourceState::Ready)
        );
        let Some(CapabilitySourceState::Unavailable { reason }) = candidate
            .availability()
            .get(&CapabilitySourceId::Mcp(python_server_id("conflicting")))
        else {
            panic!("the conflicting dependency must fail only its source");
        };
        assert!(reason.contains("python:conflicting dependency preparation failed"));
        let snapshot = coordinator.commit(candidate).unwrap();
        let available = snapshot.available_tools().definitions();
        assert!(available.iter().any(|tool| tool.name == "read"));
        assert!(available.iter().any(|tool| tool.name == "ping"));
        assert!(!available.iter().any(|tool| tool.name == "failed_identity"));
        assert_eq!(
            coordinator
                .current_mcp_runtime(&python_server_id("healthy"))
                .unwrap()
                .protocol_version(),
            &rmcp::model::ProtocolVersion::V_2026_07_28
        );
        let mut expected = if selection.no_tools || selection.tools.is_some() {
            vec![]
        } else {
            fixture.registry.model_definitions()
        };
        if admitted {
            let definition = available.iter().find(|tool| tool.name == "ping").unwrap();
            expected.push(rustx::tools::compile_model_definition(definition).unwrap());
        }
        assert_eq!(snapshot.tool_registry().model_definitions(), expected);

        let call_id = rustx::tools::mcp::mcp_tool_id(&python_server_id("healthy"), "ping");
        let call = ScriptedCall {
            id: "managed-call",
            tool_id: Box::leak(call_id.into_boxed_str()),
            name: "ping",
            arguments: serde_json::json!({}),
        };
        let mut turn = vec![FakeStep::Emit(ModelEvent::Started)];
        turn.extend(tool_call_events(0, &call).into_iter().map(FakeStep::Emit));
        turn.push(FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }));
        let model = fake_model(vec![
            turn,
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: rustx::message::types::ContentBlockIndex::new(0),
                    text: "done".into(),
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
        ]);
        let model_snapshot = support::attempt_model(model.clone(), "managed-selection");
        let context = rustx::context::ContextRuntime::for_attempt(
            rustx::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(rustx::context::DefaultTokenEstimator),
            Some(rustx::context::AgentStatusEngine::default()),
            &model_snapshot,
            rustx::model::ModelTimeoutPolicy::default(),
            support::default_monotonic_clock(),
        )
        .unwrap();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let execution = AgentExecution::new(
            AgentExecutionRequest {
                agent_id: AgentId::new("managed-selection"),
                conversation_id: fixture.runtime.conversation_id().clone(),
                attempt_id: AttemptId::new("managed-selection"),
                conversation: rustx::conversation::ConversationState::from_messages(vec![
                    MessageBlock::User(UserMessageBlock {
                        id: MessageId::new("user"),
                        content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                            text: "ping".into(),
                        })],
                        source: UserSource::Human,
                        kind: rustx::message::types::InboundKind::Message,
                        timestamp: None,
                    }),
                ])
                .unwrap(),
                initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
                model: model_snapshot,
            },
            coordinator.acquire_attempt_lease(),
            &cancellation,
            support::default_execution_policy(),
            context,
            &fixture.runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), execution.run())
            .await
            .expect("invocation liveness guard");
        let audit = common::durable_agent_result(result, fixture.store.as_ref());
        for request in model.requests() {
            assert_eq!(request.tools, expected);
        }
        assert_eq!(model.requests().len(), if admitted { 2 } else { 1 });
        assert_eq!(
            audit
                .event_history
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
                .count(),
            usize::from(admitted)
        );
        if admitted {
            assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), "called\n");
            let results = audit
                .messages()
                .iter()
                .filter_map(|message| match message {
                    MessageBlock::Tool(tool) => Some(tool),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 1);
            assert_eq!(
                results[0].result.status,
                rustx::tools::ToolExecutionStatus::Success
            );
            assert!(
                serde_json::to_string(&results[0].result.content)
                    .unwrap()
                    .contains("pong")
            );
            assert!(matches!(
                audit.event_history.last(),
                Some(RuntimeEvent::AttemptCompleted { .. })
            ));
        } else {
            assert_eq!(
                audit.outcome,
                AttemptOutcome::Failed {
                    error: AttemptFailure::Runtime {
                        error: RuntimeError::UnknownTool {
                            name: "ping".into()
                        }
                    }
                }
            );
            assert!(
                !marker.exists(),
                "available but unselected must never reach tools/call"
            );
            assert!(matches!(
                audit.event_history.last(),
                Some(RuntimeEvent::AttemptFailed { .. })
            ));
        }
        // A failed source cannot fabricate an exact-selected identity or
        // publish a fallback over the last successfully admitted registry.
        inputs.tool_activation = ToolActivationPolicy {
            tools: Some(vec!["failed_identity".into()]),
            ..Default::default()
        };
        assert!(matches!(
            coordinator.prepare_candidate_with_inputs(inputs).await,
            Err(CapabilityPreparationError::ToolActivation(_))
        ));
        assert_eq!(
            coordinator
                .current_snapshot()
                .tool_registry()
                .model_definitions(),
            expected
        );
    }
}
