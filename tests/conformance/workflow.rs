//! Registered foreground Workflow boundary and shipped-program conformance.
//!
//! Foundational local fixtures prove explicit registration and the canonical
//! outer Tool boundary. The #223 reference scenarios compose fixed Parallel,
//! real candidate writes and frozen verification, bounded repair, and root HITL
//! through strict provider-emulator sequences and native execution.
//! Private child transcripts remain outside the single parent `ToolResult`.

use crate::launch_fixture::LaunchFixture;
use std::sync::Arc;

use crate::common::provider_emulator::ProviderEmulator;
use rustx::local_runtime::composition::{
    LocalConversationCore, LocalConversationRuntime, LocalRuntimeDependencies,
};
use rustx::message::content::TextBlock;
use rustx::message::types::UserContentBlock;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime::workflow::WorkflowId;
use rustx::runtime::workflow::read_model::{WorkflowNodeKind, WorkflowState};
use rustx::runtime_client::attachment::RuntimeAttachment;
use rustx::runtime_client::host::{EventDelivery, EventSubscription};
use rustx::runtime_client::types::RuntimeClientResult;
use rustx::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEvent, RuntimeClientOutcome,
};

const MODEL: &str = "workflow-model";
const TOOL_WORKFLOW: &str = r"description: Inspect registered workflow files.
tools: [{origin: builtin, name: glob}]
timeout_ms: 10000
block:
  input:
    type: object
    properties: {task: {type: string}}
    required: [task]
    additionalProperties: false
  output:
    type: object
    properties: {files: {type: string}}
    required: [files]
    additionalProperties: false
  entry: inspect
  nodes:
    inspect:
      type: tool
      selector: {origin: builtin, name: glob}
      arguments:
        type: literal
        value: {path: .agents/workflows, pattern: '*.yaml'}
      result: {type: text, part: 0}
    done:
      type: return
      output:
        type: object
        fields: {files: {type: reference, path: [inspect]}}
  edges: [{from: inspect, to: done}]
";

#[tokio::test]
async fn fixed_tool_only_has_one_outer_result_and_zero_additional_provider_requests() {
    let Some(emulator) = ProviderEmulator::start("workflow_tool").await else {
        return;
    };
    let driver = Driver::start_with_workflow(&emulator, TOOL_WORKFLOW).await;
    assert!(
        !driver
            .runtime
            .runtime()
            .runtime_resources()
            .capability()
            .tool_registry()
            .names()
            .contains(&"glob")
    );
    driver.submit();
    let (events, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. }))
            .count(),
        1
    );
    let requests = emulator.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "parent call and continuation; no internal model turn"
    );
    let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
    let messages = serde_json::to_string(&snapshot.messages).unwrap();
    assert!(messages.contains("review_pr.yaml"));
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| matches!(message, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    assert!(
        !messages.contains("tool-glob"),
        "internal native execution is not canonical history"
    );
    emulator.finish().await;
}

const KEY: &str = "RUSTX_ISSUE83_KEY";

#[tokio::test]
async fn bounded_loop_exhaustion_keeps_one_outer_result_and_no_internal_provider_turns() {
    use serde_json::json;
    let Some(emulator) = ProviderEmulator::start("workflow_tool").await else {
        return;
    };
    let mut definition: serde_json::Value = serde_yaml::from_str(TOOL_WORKFLOW).unwrap();
    let body = definition["block"].take();
    let output = json!({"type":"object","properties":{
        "status":{"type":"string","enum":["satisfied","exhausted"]},"iterations":{"type":"integer"},"result":body["output"]
    },"required":["status","iterations","result"],"additionalProperties":false});
    definition["block"] = json!({"input":body["input"],"output":output,"entry":"feedback","nodes":{
        "feedback":{"type":"loop","input":{"type":"reference","path":["args"]},"body":body,"max_iterations":3,
            "until":{"type":"boolean","value":{"type":"literal","value":false}},"carry":{"type":"literal","value":{"task":"inspect again"}}},
        "done":{"type":"return","output":{"type":"reference","path":["feedback"]}}
    },"edges":[{"from":"feedback","port":"satisfied","to":"done"},{"from":"feedback","port":"exhausted","to":"done"}]});
    let driver =
        Driver::start_with_workflow(&emulator, &serde_yaml::to_string(&definition).unwrap()).await;
    driver.submit();
    let (events, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RuntimeClientEvent::ToolExecutionSettled { .. }))
            .count(),
        1
    );
    assert_eq!(emulator.requests().await.len(), 2);
    let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|m| matches!(m, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    let history = serde_json::to_string(&snapshot.messages).unwrap();
    assert!(history.contains("exhausted"));
    assert!(history.contains("review_pr.yaml"));
    assert!(!history.contains("tool-glob"));
    emulator.finish().await;
}

fn models_json_for_base_url(base_url: &str) -> String {
    toml::to_string_pretty(&serde_json::json!({
        "providers": {
            "emulator": {
                "base_url": base_url,
                "api_key": "issue83-secret",
                "models": [{
                    "id": MODEL,
                    "protocol": "openai_chat_completions",
                    "context_window": 128_000,
                    "max_output_tokens": 1024,
                    "capabilities": {
                        "input_modalities": ["text"],
                        "output_modalities": ["text"],
                        "tool_calls": true,
                        "reasoning": false
                    },
                    "compat": {"chat_reasoning_replay": "omit"}
                }]
            }
        }
    }))
    .unwrap()
}

fn models_json(emulator: &ProviderEmulator) -> String {
    models_json_for_base_url(&emulator.openai_base_url())
}

const CONFIG: &str = r#"schema_version = 8
agent_id = "agent-issue83"
default_tools = ["read"]

[model]
model = "emulator/workflow-model"

[context]
reserve_tokens = 0
keep_recent_tokens = 0

[subagents]
max_concurrent = 4
definitions = ["reviewer"]
main = []
workflow = ["reviewer"]

[workflows]
definitions = ["review_pr"]
main = ["review_pr"]
"#;

const WORKFLOW: &str = r"description: Review the request with a native child agent.
block:
  input:
    type: object
    properties:
      task:
        type: string
    required:
    - task
    additionalProperties: false
  output:
    type: object
    properties:
      summary:
        type: string
    required:
    - summary
    additionalProperties: false
  entry: review
  nodes:
    review:
      type: agent
      profile: reviewer
      task: Review the request and commit the result.
      input:
        task:
          type: reference
          path:
          - args
          - task
      output:
        type: object
        properties:
          passed:
            type: boolean
          summary:
            type: string
        required:
        - passed
        - summary
        additionalProperties: false
    decision:
      type: branch
      condition:
        type: boolean
        value:
          type: reference
          path:
          - review
          - passed
    success:
      type: return
      output:
        type: object
        fields:
          summary:
            type: reference
            path:
            - review
            - summary
    failure:
      type: return
      output:
        type: object
        fields:
          summary:
            type: reference
            path:
            - args
            - task
  edges:
  - from: review
    to: decision
  - from: decision
    to: success
    port: true
  - from: decision
    to: failure
    port: false
";

struct Driver {
    #[allow(dead_code)]
    root: tempfile::TempDir,
    runtime: LocalConversationRuntime,
    #[allow(dead_code)]
    attachment: RuntimeAttachment,
    events: EventSubscription,
}

impl Driver {
    async fn start(emulator: &ProviderEmulator) -> Self {
        Self::start_with_workflow(emulator, WORKFLOW).await
    }

    async fn start_with_workflow(emulator: &ProviderEmulator, workflow: &str) -> Self {
        let root = tempfile::tempdir().expect("temp root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents/subagents/reviewer"))
            .expect("subagent directory");
        std::fs::create_dir_all(workspace.join(".agents/workflows")).expect("workflow directory");
        std::fs::write(root.path().join("models.toml"), models_json(emulator))
            .expect("models.toml");
        std::fs::write(root.path().join("rustx.toml"), CONFIG).expect("rustx.toml");
        std::fs::write(
            workspace.join(".agents/subagents/reviewer.md"),
            "---\ndescription: The Workflow-only reviewer.\n---\nReview requests carefully.\n",
        )
        .expect("reviewer instructions");
        std::fs::write(workspace.join(".agents/workflows/review_pr.yaml"), workflow)
            .expect("workflow YAML");
        // This deliberately is not registered. It is also malformed, proving
        // that the loader uses configured ids rather than scanning the YAML
        // directory as an implicit admission surface.
        std::fs::write(
            workspace.join(".agents/workflows/inactive.yaml"),
            "this is not a registered Workflow definition: [",
        )
        .expect("inactive workflow YAML");

        let paths = LaunchFixture {
            models: root.path().join("models.toml"),
            config: root.path().join("rustx.toml"),
            skill_paths: Vec::new(),
            no_skills: true,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: rustx::local_runtime::StartupSession::Empty,
            session_name: None,
            tools: None,
            exclude_tools: Vec::new(),
            workspace,
            runtime_root: root.path().join("private"),
        };
        let dependencies = LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                KEY.to_owned(),
                "issue83-secret".to_owned(),
            )]))),
            child_program: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        };
        let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies)
            .await
            .expect("native Workflow runtime composes");
        let resources = runtime.runtime().runtime_resources();
        assert!(
            resources
                .workflows()
                .main()
                .iter()
                .any(|id| id.as_str() == "review_pr")
        );
        assert!(resources.subagent_main_admission().is_empty());
        assert_eq!(
            resources
                .subagent_workflow_admission()
                .iter()
                .map(rustx::runtime::subagent::SubagentName::as_str)
                .collect::<Vec<_>>(),
            vec!["reviewer"]
        );
        let tools = resources.capability().tool_registry().names();
        assert!(tools.contains(&"review_pr"));
        assert!(
            resources
                .workflows()
                .get(&WorkflowId::parse("inactive").expect("workflow id"))
                .is_none()
        );
        assert!(!tools.contains(&"inactive"));
        assert!(!tools.contains(&"subagent"));

        let (attachment, initialized) = runtime
            .host()
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let RuntimeClientResult::Initialized { cursor, .. } = initialized else {
            panic!("initialize returns the initial snapshot");
        };
        let (events, _) = runtime
            .host()
            .subscribe_events(attachment.attachment_id(), cursor)
            .expect("subscribe");
        Self {
            root,
            runtime,
            attachment,
            events,
        }
    }

    fn submit(&self) {
        self.runtime
            .host()
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: "workflow conformance request".to_owned(),
            })])
            .expect("inbound accepted");
    }

    async fn settle(&self) -> (Vec<RuntimeClientEvent>, RuntimeClientOutcome) {
        let mut events = Vec::new();
        loop {
            let delivery =
                tokio::time::timeout(std::time::Duration::from_secs(30), self.events.next())
                    .await
                    .expect("Workflow attempt settles");
            match delivery {
                EventDelivery::Event(published) => {
                    let event = published.event;
                    if let RuntimeClientEvent::AttemptSettled { outcome, .. } = &event {
                        let outcome = outcome.clone();
                        events.push(event);
                        // The client terminal observation precedes transfer
                        // back to the runtime's idle slot. Await the native
                        // handoff notification before attempting reload.
                        self.runtime.runtime().settlement_signal().notified().await;
                        return (events, outcome);
                    }
                    events.push(event);
                }
                other => panic!("Runtime Client event stream ended: {other:?}"),
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_registered_workflow_rejects_the_obsolete_workspace_rustx_path() {
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/subagents/reviewer"))
        .expect("subagent directory");
    std::fs::create_dir_all(workspace.join(".rustx/workflows")).expect("obsolete directory");
    std::fs::write(
        root.path().join("models.toml"),
        models_json_for_base_url("http://127.0.0.1:1/v1"),
    )
    .expect("models.toml");
    std::fs::write(root.path().join("rustx.toml"), CONFIG).expect("rustx.toml");
    std::fs::write(
        workspace.join(".agents/subagents/reviewer.md"),
        "---\ndescription: The Workflow-only reviewer.\n---\nReview requests carefully.\n",
    )
    .expect("reviewer instructions");
    std::fs::write(workspace.join(".rustx/workflows/review_pr.yaml"), WORKFLOW)
        .expect("legacy workflow YAML");

    let paths = LaunchFixture {
        models: root.path().join("models.toml"),
        config: root.path().join("rustx.toml"),
        skill_paths: Vec::new(),
        no_skills: true,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: rustx::local_runtime::StartupSession::Empty,
        session_name: None,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.path().join("private"),
    };
    let detail = paths.try_resolve().expect_err(
        "static analysis rejects the obsolete workspace Workflow path without composing a runtime",
    );
    assert!(
        detail.contains(".agents/workflows/review_pr.yaml"),
        "{detail}"
    );
    assert!(
        !detail.contains(".rustx/workflows/review_pr.yaml"),
        "{detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_workflow_can_remain_out_of_main_model_admission() {
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/subagents/reviewer"))
        .expect("subagent directory");
    std::fs::create_dir_all(workspace.join(".agents/workflows")).expect("workflow directory");
    std::fs::write(
        root.path().join("models.toml"),
        models_json_for_base_url("http://127.0.0.1:1/v1"),
    )
    .expect("models.toml");
    std::fs::write(
        root.path().join("rustx.toml"),
        CONFIG.replace("main = [\"review_pr\"]", "main = []"),
    )
    .expect("rustx.toml");
    std::fs::write(
        workspace.join(".agents/subagents/reviewer.md"),
        "---\ndescription: The Workflow-only reviewer.\n---\nReview requests carefully.\n",
    )
    .expect("reviewer instructions");
    std::fs::write(workspace.join(".agents/workflows/review_pr.yaml"), WORKFLOW)
        .expect("workflow YAML");

    let paths = LaunchFixture {
        models: root.path().join("models.toml"),
        config: root.path().join("rustx.toml"),
        skill_paths: Vec::new(),
        no_skills: true,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: rustx::local_runtime::StartupSession::Empty,
        session_name: None,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.path().join("private"),
    };
    let resources =
        LocalConversationCore::compose(&(paths).resolve(), &LocalRuntimeDependencies::default())
            .await
            .expect("registered but non-main Workflow composes")
            .runtime()
            .runtime_resources();
    assert!(
        resources
            .workflows()
            .definitions()
            .contains_key(&WorkflowId::parse("review_pr").expect("workflow id"))
    );
    assert!(resources.workflows().main().is_empty());
    assert!(
        !resources
            .capability()
            .tool_registry()
            .names()
            .contains(&"review_pr")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_workflow_freezes_files_resources_and_keeps_one_parent_history_boundary() {
    let Some(emulator) = ProviderEmulator::start("workflow_output").await else {
        return;
    };
    let driver = Driver::start(&emulator).await;
    driver.submit();
    emulator.await_gate("workflow-child-admitted").await;
    // The real child's model request proves native admission and profile
    // materialization already happened. Edit every relevant source while
    // that child is held before its terminal output, without a timing race.
    let workspace = driver.root.path().join("workspace");
    std::fs::write(
        workspace.join(".agents/workflows/review_pr.yaml"),
        r"
description: Changed future workflow
block:
  input: {type: object}
  output: {type: object, properties: {summary: {type: string}}, required: [summary]}
  entry: done
  nodes:
    done:
      type: return
      output: {type: literal, value: {summary: changed future output}}
  edges: []
",
    )
    .expect("replace future program");
    std::fs::write(
        workspace.join(".agents/subagents/reviewer.md"),
        "---\ndescription: Future reviewer.\n---\nCHANGED FUTURE PROFILE\n",
    )
    .expect("replace future profile");
    std::fs::write(
        driver.root.path().join("rustx.toml"),
        CONFIG.replace("main = [\"review_pr\"]", "main = []"),
    )
    .expect("replace future exposure");
    emulator.release_gate("workflow-child-admitted").await;
    let (events, outcome) = driver.settle().await;

    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "Workflow Tool completion is one parent attempt result: {outcome:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. }))
            .count(),
        1,
        "the parent sees one bounded Workflow Tool execution"
    );
    let snapshot = driver.runtime.host().snapshot().expect("snapshot");
    let snapshot_json = serde_json::to_string(&snapshot).expect("snapshot JSON");
    assert!(snapshot_json.contains("native workflow child committed"));
    assert!(
        !snapshot_json.contains("Review the request and commit the result"),
        "the child task is not injected into parent canonical history"
    );
    assert!(
        !snapshot_json.contains("\"passed\":true"),
        "the intermediate child value is not injected into parent history"
    );

    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 3, "parent, child, then parent continuation");
    assert!(
        !serde_json::to_string(&requests)
            .unwrap()
            .contains("CHANGED FUTURE PROFILE")
    );
    driver
        .runtime
        .host()
        .reload_resources()
        .await
        .expect("publish edited resources for future attempts");
    assert!(
        driver
            .runtime
            .runtime()
            .runtime_resources()
            .workflows()
            .main()
            .is_empty()
    );
    assert_eq!(
        driver
            .runtime
            .runtime()
            .runtime_resources()
            .workflows()
            .get(&WorkflowId::parse("review_pr").unwrap())
            .unwrap()
            .description(),
        "Changed future workflow"
    );
    emulator.finish().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn fixed_question_and_review_use_root_client_while_parent_model_remains_in_outer_call() {
    use rustx::events::review::{ReviewDecision, ReviewResponse};
    use rustx::runtime::{InteractionKind, InteractionResponse, QuestionnaireResponse};
    for accepted in [false, true] {
        let Some(emulator) = ProviderEmulator::start("workflow_tool").await else {
            return;
        };
        let mut definition: serde_json::Value = serde_yaml::from_str(TOOL_WORKFLOW).unwrap();
        definition["tools"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"origin":"builtin","name":"ask_user"}));
        definition["block"]["entry"] = serde_json::json!("question");
        definition["block"]["nodes"]["question"] = serde_json::json!({
            "type":"tool","selector":{"origin":"builtin","name":"ask_user"},
            "arguments":{"type":"literal","value":{"questions":[{"question":"PRIVATE QUESTION: choose a priority","header":"Priority","options":[{"label":"A","description":"First"},{"label":"B","description":"Second"}]}]}},
            "result":{"type":"json","part":0,"schema":{"type":"object","properties":{"cancelled":{"type":"boolean"},"answers":{"type":"array","items":{"type":"object"}}},"required":["cancelled","answers"],"additionalProperties":false}}
        });
        definition["block"]["nodes"]["human"] = serde_json::json!({"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[{"type":"reference","path":["question"]}]});
        definition["block"]["nodes"]["decision"] = serde_json::json!({"type":"branch","condition":{"type":"boolean","value":{"type":"reference","path":["human","accepted"]}}});
        definition["block"]["nodes"]["rejected"] = serde_json::json!({"type":"return","output":{"type":"literal","value":{"files":"review_pr.yaml review rejected"}}});
        definition["block"]["edges"]
            .as_array_mut()
            .unwrap()
            .extend([
                serde_json::json!({"from":"question","to":"human"}),
                serde_json::json!({"from":"human","to":"decision"}),
                serde_json::json!({"from":"decision","to":"inspect","port":"true"}),
                serde_json::json!({"from":"decision","to":"rejected","port":"false"}),
            ]);
        let driver =
            Driver::start_with_workflow(&emulator, &serde_yaml::to_string(&definition).unwrap())
                .await;
        driver.submit();
        let mut seen = 0;
        while seen < 2 {
            let delivery =
                tokio::time::timeout(std::time::Duration::from_secs(30), driver.events.next())
                    .await
                    .expect("human publication liveness guard");
            let EventDelivery::Event(event) = delivery else {
                panic!("live event stream")
            };
            let RuntimeClientEvent::InteractionPending { interaction } = event.event else {
                continue;
            };
            assert_eq!(
                interaction.interaction,
                interaction.request.interaction_ref()
            );
            assert!(matches!(
                interaction.source,
                rustx::runtime::InteractionSource::Primary
            ));
            assert_eq!(
                emulator.requests().await.len(),
                1,
                "parent remains inside its pending outer Workflow call"
            );
            let response = match interaction.request.kind {
                InteractionKind::Questionnaire { invocation_id, .. } => {
                    assert!(matches!(
                        invocation_id,
                        rustx::tools::types::ToolInvocationId::Workflow { .. }
                    ));
                    // Decline is ordinary data; this fixed program still reaches Review.
                    InteractionResponse::Questionnaire {
                        response: QuestionnaireResponse::Declined,
                    }
                }
                InteractionKind::Review {
                    review,
                    subject_digest,
                } => InteractionResponse::Review {
                    response: ReviewResponse {
                        instance: review.instance,
                        subject_digest,
                        decision: if accepted {
                            ReviewDecision::Accepted
                        } else {
                            ReviewDecision::Rejected {
                                feedback: "PRIVATE FEEDBACK".into(),
                            }
                        },
                    },
                },
                InteractionKind::Approval { .. } => panic!("no approval expected"),
            };
            for wrong in 0..2 {
                let mut target = interaction.interaction.clone();
                if wrong == 0 {
                    target.conversation_id =
                        rustx::runtime::identity::ConversationId::new("different-owner");
                } else {
                    target.interaction_id =
                        rustx::runtime::identity::InteractionId::new("different-interaction");
                }
                let rejected = driver
                    .attachment
                    .handle_request_async(
                        rustx::runtime_client::RuntimeClientRequest::InteractionRespond {
                            id: rustx::runtime_client::RequestId::new(1000 + seen * 2 + wrong),
                            interaction: target,
                            response: response.clone(),
                        },
                    )
                    .await;
                assert!(
                    rejected.error.is_some(),
                    "wrong routed owner/id cannot settle the prompt"
                );
            }
            let result = driver
                .attachment
                .handle_request_async(
                    rustx::runtime_client::RuntimeClientRequest::InteractionRespond {
                        id: rustx::runtime_client::RequestId::new(220 + seen),
                        interaction: interaction.interaction.clone(),
                        response,
                    },
                )
                .await;
            assert!(
                matches!(result.result, Some(RuntimeClientResult::InteractionResponseAccepted { interaction: original }) if original == interaction.interaction)
            );
            seen += 1;
        }
        let (_, outcome) = driver.settle().await;
        assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));
        assert_eq!(
            emulator.requests().await.len(),
            2,
            "one parent call and its ordinary continuation only"
        );
        let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
        let messages = serde_json::to_string(&snapshot.messages).unwrap();
        assert!(!messages.contains("PRIVATE QUESTION"));
        assert!(!messages.contains("PRIVATE FEEDBACK"));
        assert_eq!(
            snapshot
                .messages
                .iter()
                .filter(|message| matches!(message, rustx::message::types::MessageBlock::Tool(_)))
                .count(),
            1
        );
        emulator.finish().await;
    }
}

fn copy_reference(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_reference(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn reference_git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Driver {
    async fn reference(emulator: &ProviderEmulator, git: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime");
        copy_reference(&source, root.path());
        let workspace = root.path().join("workspace");
        let reviewer = workspace.join(".agents/subagents/reviewer.md");
        let role = std::fs::read_to_string(&reviewer)
            .unwrap()
            .replace("example/demo-model", "emulator/workflow-model");
        std::fs::write(reviewer, role).unwrap();
        if git {
            reference_git(&workspace, &["init"]);
            reference_git(&workspace, &["add", "."]);
            reference_git(&workspace, &["commit", "-m", "reference baseline"]);
        }
        std::fs::write(
            root.path().join("models.toml"),
            models_json(emulator).replace("1024", "4096"),
        )
        .unwrap();
        let config = std::fs::read_to_string(root.path().join("rustx.toml"))
            .unwrap()
            .replace("example/demo-model", "emulator/workflow-model")
            .replace(
                "[model.reasoning_profile]\nmode = \"profile\"\nname = \"off\"\n",
                "",
            );
        std::fs::write(root.path().join("rustx.toml"), config).unwrap();
        let paths = LaunchFixture {
            models: root.path().join("models.toml"),
            config: root.path().join("rustx.toml"),
            workspace,
            runtime_root: root.path().join("private"),
            skill_paths: vec![],
            no_skills: false,
            no_builtin_tools: false,
            no_tools: false,
            startup_session: rustx::local_runtime::StartupSession::Empty,
            session_name: None,
            tools: Some(vec![
                "parallel_review".into(),
                "implement_and_review".into(),
            ]),
            exclude_tools: vec![],
        };
        let dependencies = LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                KEY.to_owned(),
                "fixture".to_owned(),
            )]))),
            child_program: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        };
        let runtime = LocalConversationRuntime::compose(&(paths).resolve(), &dependencies)
            .await
            .unwrap();
        let (attachment, initialized) = runtime
            .host()
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let RuntimeClientResult::Initialized { cursor, .. } = initialized else {
            panic!("initialized")
        };
        let (events, _) = runtime
            .host()
            .subscribe_events(attachment.attachment_id(), cursor)
            .unwrap();
        Self {
            root,
            runtime,
            attachment,
            events,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_repair_uses_real_writes_checks_and_root_human_decisions() {
    reference_repair(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_repair_exhaustion_retains_dirty_work_without_another_body() {
    reference_repair(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_repair_candidate_checker_tampering_cannot_redefine_frozen_verification() {
    reference_repair(false, true).await;
}

#[allow(clippy::too_many_lines)]
async fn reference_repair(exhausted: bool, tampered: bool) {
    use rustx::events::review::{ReviewDecision, ReviewResponse};
    use rustx::runtime::{InteractionKind, InteractionResponse, QuestionnaireResponse};
    let scenario = if tampered {
        "workflow_reference_tampering"
    } else if exhausted {
        "workflow_reference_exhaustion"
    } else {
        "workflow_reference_repair"
    };
    let iterations = if exhausted { 3 } else { 2 };
    let Some(emulator) = ProviderEmulator::start(scenario).await else {
        return;
    };
    let driver = Driver::reference(&emulator, true).await;
    let original = std::fs::read(driver.root.path().join("workspace/greeting.py")).unwrap();
    driver.submit();
    let release_writers = async {
        for iteration in 1..=iterations {
            emulator
                .await_gate(&format!("reference-writer-{iteration}"))
                .await;
            assert_eq!(emulator.requests().await.len(), 2 * iteration + 2);
            if tampered && iteration == 2 {
                // The second writer's provider gate is after the first native
                // check's settlement and failed-finding commit, before repair.
                let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
                let run = &snapshot.workflows.runs[0];
                let checks = run
                    .instances
                    .iter()
                    .filter(|row| row.node.as_deref() == Some("check") && row.invocation.is_some())
                    .collect::<Vec<_>>();
                assert_eq!(checks.len(), 1);
                assert!(matches!(checks[0].state, WorkflowState::Settled { .. }));
                assert!(
                    !run.instances
                        .iter()
                        .any(|row| row.node.as_deref() == Some("review_candidate")
                            && row.review_accepted == Some(true))
                );
            }
            emulator
                .release_gate(&format!("reference-writer-{iteration}"))
                .await;
        }
    };
    let interact = async {
        let mut reviews = 0;
        let mut approvals = 0;
        let mut questions = 0;
        let mut check_commands = Vec::new();
        let mut writes = 0;
        loop {
            let EventDelivery::Event(event) =
                tokio::time::timeout(std::time::Duration::from_mins(1), driver.events.next())
                    .await
                    .unwrap()
            else {
                panic!("events")
            };
            match event.event {
                RuntimeClientEvent::InteractionPending { interaction } => {
                    let response = match interaction.request.kind {
                        InteractionKind::Questionnaire {
                            ref questionnaire, ..
                        } => {
                            questions += 1;
                            // The answer names the option by its index in the
                            // published request, exactly as a Runtime Client
                            // resolves a display label locally.
                            let option_index = questionnaire.questions[0]
                                .answer
                                .options()
                                .iter()
                                .position(|option| option.label == "Minimal change")
                                .expect("the questionnaire offers the minimal change option");
                            InteractionResponse::Questionnaire {
                                response: QuestionnaireResponse::Submitted(
                                    rustx::events::QuestionnaireSubmission {
                                        answers: vec![rustx::events::QuestionnaireAnswerEntry {
                                            question_index: 0,
                                            answer: rustx::events::QuestionnaireAnswer::Option(
                                                rustx::events::OptionAnswer { option_index },
                                            ),
                                        }],
                                    },
                                ),
                            }
                        }
                        InteractionKind::Review {
                            review,
                            subject_digest,
                        } => {
                            reviews += 1;
                            assert_eq!(
                                emulator.requests().await.len(),
                                if reviews == 1 { 2 } else { 7 }
                            );
                            InteractionResponse::Review {
                                response: ReviewResponse {
                                    instance: review.instance,
                                    subject_digest,
                                    decision: ReviewDecision::Accepted,
                                },
                            }
                        }
                        InteractionKind::Approval {
                            tool_name,
                            arguments,
                            ..
                        } => {
                            approvals += 1;
                            if tool_name == "bash" {
                                check_commands
                                    .push(arguments["command"].as_str().unwrap().to_owned());
                            } else {
                                assert_eq!(tool_name, "write");
                                writes += 1;
                            }
                            serde_json::from_value::<InteractionResponse>(
                                serde_json::json!({"type":"approval","decision":{"type":"allow"}}),
                            )
                            .unwrap()
                        }
                    };
                    let reply = driver
                        .attachment
                        .handle_request_async(
                            rustx::runtime_client::RuntimeClientRequest::InteractionRespond {
                                id: rustx::runtime_client::RequestId::new(
                                    questions + reviews + approvals,
                                ),
                                interaction: interaction.interaction,
                                response,
                            },
                        )
                        .await;
                    assert!(reply.error.is_none(), "{reply:?}");
                }
                RuntimeClientEvent::AttemptSettled { outcome, .. } => {
                    assert!(
                        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
                        "{outcome:?}"
                    );
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(
            (questions, reviews, approvals),
            (
                2,
                if exhausted { 1 } else { 2 },
                u64::try_from(2 * iterations + usize::from(tampered)).unwrap()
            )
        );
        assert_eq!(writes, iterations + usize::from(tampered));
        assert_eq!(check_commands.len(), iterations);
        let definition: serde_json::Value = serde_yaml::from_str(include_str!(
            "../../examples/local-runtime/workspace/.agents/workflows/implement_and_review.yaml"
        ))
        .unwrap();
        let frozen = definition["block"]["nodes"]["repair"]["body"]["nodes"]["check"]["arguments"]
            ["value"]["command"]
            .as_str()
            .unwrap();
        assert!(frozen.starts_with("python3 -I -B - <<'PY'"));
        assert!(!frozen.contains("checks/verify_greeting.py"));
        assert!(check_commands.iter().all(|command| command == frozen));
    };
    tokio::join!(release_writers, interact);
    assert_eq!(emulator.requests().await.len(), 4 + 2 * iterations);
    assert_eq!(
        std::fs::read(driver.root.path().join("workspace/greeting.py")).unwrap(),
        original
    );
    let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
    let messages = serde_json::to_string(&snapshot.messages).unwrap();
    assert!(!messages.contains("PRIVATE CLAIM"));
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|m| matches!(m, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    assert!(messages.contains("handoff"), "{messages}");
    let run = &snapshot.workflows.runs[0];
    assert_eq!(run.agents_consumed, 1 + iterations);
    assert_eq!(run.candidate_users, 0);
    assert_eq!(
        run.instances
            .iter()
            .filter(|row| row.node.as_deref() == Some("implement"))
            .count(),
        iterations
    );
    assert_eq!(
        run.instances
            .iter()
            .filter(|row| row.node.as_deref() == Some("check"))
            .count(),
        iterations
    );
    assert_eq!(
        run.instances
            .iter()
            .filter(|row| row.kind == WorkflowNodeKind::Tool)
            .count(),
        1 + iterations
    );
    assert!(
        run.instances
            .iter()
            .filter(|row| row.child.is_some() || row.invocation.is_some())
            .all(|row| matches!(row.state, WorkflowState::Settled { .. }))
    );
    let checks = run
        .instances
        .iter()
        .filter(|row| row.node.as_deref() == Some("check"))
        .collect::<Vec<_>>();
    assert!(checks.iter().all(|row| row.candidate.is_some()));
    if !exhausted {
        assert_ne!(checks[0].candidate, checks[1].candidate);
    }
    assert_eq!(checks.last().unwrap().candidate, run.candidate);
    if !exhausted {
        let review = run
            .instances
            .iter()
            .find(|row| row.node.as_deref() == Some("review_candidate"))
            .unwrap();
        assert_eq!(review.candidate, run.candidate);
        assert_eq!(review.review_accepted, Some(true));
    }
    let handoff = run.handoff.as_ref().unwrap();
    assert_eq!(handoff.state, "retained");
    if tampered {
        assert_eq!(
            std::fs::read_to_string(
                std::path::Path::new(&handoff.path).join("checks/verify_greeting.py")
            )
            .unwrap(),
            "print(\"passed\", end=\"\")\n"
        );
        assert!(
            !driver
                .root
                .path()
                .join("workspace/checks/verify_greeting.py")
                .exists()
        );
    }
    let changed =
        std::fs::read_to_string(std::path::Path::new(&handoff.path).join("greeting.py")).unwrap();
    assert_eq!(
        changed,
        if exhausted {
            "def greeting(name):\n    return \"wrong\"\n"
        } else {
            "def greeting(name):\n    return 'Hello, ' + (name or 'friend') + '!'\n"
        }
    );

    emulator.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_parallel_reverses_completion_without_git_or_parent_orchestration() {
    let Some(emulator) = ProviderEmulator::start("workflow_reference_parallel").await else {
        return;
    };
    let driver = Driver::reference(&emulator, false).await;
    driver.submit();
    emulator.await_gate("parallel-child-0").await;
    emulator.await_gate("parallel-child-1").await;
    assert_eq!(emulator.requests().await.len(), 3);
    emulator.release_gate("parallel-child-1").await;
    // The native owner publication, not elapsed time, proves the second child
    // settled while the first provider response is still held.
    loop {
        let EventDelivery::Event(_) =
            tokio::time::timeout(std::time::Duration::from_secs(30), driver.events.next())
                .await
                .unwrap()
        else {
            panic!("events")
        };
        let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
        if snapshot
            .workflows
            .runs
            .iter()
            .flat_map(|run| &run.instances)
            .any(|row| {
                row.kind == WorkflowNodeKind::Agent
                    && matches!(row.state, WorkflowState::Settled { .. })
            })
        {
            break;
        }
    }
    emulator.release_gate("parallel-child-0").await;
    emulator.await_gate("parallel-child-2").await;
    emulator.release_gate("parallel-child-2").await;
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));
    assert_eq!(emulator.requests().await.len(), 5);
    let (snapshot, _) = driver.runtime.host().snapshot().unwrap();
    let run = &snapshot.workflows.runs[0];
    assert_eq!(run.agents_consumed, 3);
    assert!(run.candidate.is_none());
    assert!(run.handoff.is_none());
    assert!(!driver.root.path().join("workspace/.git").exists());
    let history = serde_json::to_string(&snapshot.messages).unwrap();
    assert!(history.contains("quality") && history.contains("security"));
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|m| matches!(m, rustx::message::types::MessageBlock::Tool(_)))
            .count(),
        1
    );
    emulator.finish().await;
}
