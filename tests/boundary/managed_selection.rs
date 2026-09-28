//! CFG-03 × `FastMCP` 4: real prepared sources enter the single frozen Agent
//! registry. Source failure and exposure filtering remain independent.
#![cfg(unix)]

use std::sync::{Arc, Mutex};

use crate::runtime::process_runner::{
    CapturedProcessResult, RunnerBackedProcessRunner, RunnerTestControl, SupervisedCommandSpec,
    SupervisedProcessRunner,
};
use tokio::sync::oneshot;

use super::{common, support};
use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::capabilities::{
    AgentActivation, CapabilityCoordinator, CapabilityCoordinatorConfig,
    CapabilityPreparationError, CapabilityResourceInputs, CapabilitySourceState, ToolSourceId,
};
use rustx::events::AttemptOutcome;
use rustx::events::types::{AttemptFailure, RuntimeEvent};
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::{ModelEvent, ModelFinishReason};
use rustx::runtime::identity::{AgentId, AttemptId, MessageId};
use rustx::runtime::types::{CancellationReason, RuntimeError};
use rustx::tools::python::python_server_id;
use support::fake::{FakeStep, ScriptedCall, fake_model, tool_call_events};

/// Hold the real build owner after uv has physically settled, before the
/// store can publish the immutable source. The network/package manager is
/// setup, not a clock that decides whether publication was correctly ordered.
struct PreparationRunner {
    runner: RunnerBackedProcessRunner,
    staged: Mutex<Option<oneshot::Sender<std::path::PathBuf>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl SupervisedProcessRunner for PreparationRunner {
    fn run(
        &self,
        spec: SupervisedCommandSpec,
        control: Option<RunnerTestControl>,
    ) -> futures_util::future::BoxFuture<'_, Result<CapturedProcessResult, String>> {
        Box::pin(async move {
            let sync = spec.command.contains(" sync ");
            let staging = spec.cwd.clone();
            let result = self.runner.run(spec, control).await?;
            if sync {
                assert_eq!(result.exit_code, Some(0), "{result:?}");
                let release = self.release.lock().unwrap().take().expect("one build");
                self.staged
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one build")
                    .send(staging)
                    .expect("preparation observer");
                release.await.expect("publication release");
            }
            Ok(result)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fastmcp4_availability_selection_request_and_invocation_share_one_authority() {
    // CI's boundary jobs provide uv; absence must not make this acceptance green.
    for (source_selection, admitted) in [
        (
            rustx::capabilities::selection::SourceToolSelection::All,
            true,
        ),
        (
            rustx::capabilities::selection::SourceToolSelection::Exact(vec!["ping".into()]),
            true,
        ),
        (
            rustx::capabilities::selection::SourceToolSelection::Exact(vec![]),
            false,
        ),
    ] {
        let mut selection = AgentActivation::default();
        selection.profile.tools.sources.insert(
            ToolSourceId::ManagedPython("healthy".into()),
            source_selection,
        );
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
            source_demand: common::source_demand(
                fixture.runtime.workspace().root(),
                ["healthy", "conflicting"].map(|name| format!("python:{name}")),
            ),
            base_tool_registry: Arc::new(fixture.registry.clone()),
            agent_activation: selection.clone(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: fixture.runtime.environment().clone(),
        };
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: fixture.runtime.conversation_id().clone(),
            workspace: fixture.runtime.workspace().clone(),
            environment_store_root: fixture.dir().path().join("environments"),
            source_demand: inputs.source_demand.clone(),
            base_tool_registry: inputs.base_tool_registry.clone(),
            extension_tools: fixture.runtime.extension_tool_plane(),
            agent_activation: selection.clone(),
            skill_discovery: inputs.skill_discovery.clone(),
            mcp_servers: inputs.mcp_servers.clone(),
            base_environment: inputs.base_environment.clone(),
        })
        .unwrap();
        let (staged_tx, staged_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let store = rustx::tools::python::PythonToolStore::with_runner(
            fixture.dir().path().join("environments/python-tools"),
            Arc::new(PreparationRunner {
                runner: RunnerBackedProcessRunner::default(),
                staged: Mutex::new(Some(staged_tx)),
                release: Mutex::new(Some(release_rx)),
            }),
        )
        .unwrap();
        coordinator.install_python_store(store);
        let preparing = coordinator.clone();
        let mut owner = tokio::spawn(async move { preparing.prepare_candidate().await });
        // This acknowledgement, not an elapsed-time budget over uv resolution,
        // establishes that the owner reached the pre-publication boundary.
        let staging = tokio::select! {
            staged = staged_rx => staged.expect("source preparation reaches publication"),
            outcome = &mut owner => panic!("candidate completed before publication gate: {outcome:?}"),
        };
        assert!(
            !owner.is_finished(),
            "no candidate before owner publication"
        );
        assert!(staging.join("source/server.py").is_file());
        assert!(!staging.join("manifest.json").exists());
        assert!(
            !coordinator
                .current_snapshot()
                .available_tools()
                .definitions()
                .iter()
                .any(|tool| tool.name == "ping")
        );
        // Later workspace edits must not change the source whose availability
        // and executor are about to be published together.
        let source_path = root.join(".agents/tools/healthy/server.py");
        let original_source = std::fs::read(&source_path).unwrap();
        std::fs::write(
            &source_path,
            "raise RuntimeError('live workspace is not prepared authority')\n",
        )
        .unwrap();
        release_tx.send(()).expect("release preparation owner");
        let candidate = tokio::time::timeout(std::time::Duration::from_secs(30), owner)
            .await
            .expect("publication and MCP handoff liveness guard")
            .expect("preparation owner")
            .unwrap();
        assert!(!staging.exists(), "staging was atomically published");
        assert_eq!(
            candidate
                .availability()
                .get(&ToolSourceId::ManagedPython("healthy".into())),
            Some(&CapabilitySourceState::Ready)
        );
        let Some(CapabilitySourceState::Unavailable { reason }) = candidate
            .availability()
            .get(&ToolSourceId::ManagedPython("conflicting".into()))
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
        let mut expected = vec![];
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
            None,
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
        std::fs::write(&source_path, original_source).unwrap();
        // A failed source cannot fabricate an exact-selected identity or
        // publish a fallback over the last successfully admitted registry.
        inputs.agent_activation = AgentActivation {
            profile: {
                let mut profile = rustx::local_runtime::config::AgentProfileDocument::default();
                profile.tools.builtin = vec!["failed_identity".into()];
                profile
            },
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
