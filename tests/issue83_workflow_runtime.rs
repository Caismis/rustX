//! Issue #83: the native YAML Workflow path through the existing runtime.
//!
//! This suite crosses the real provider boundary once for the parent Agent
//! and once for the Workflow-owned child. The provider script is strict about
//! the request sequence:
//!
//! ```text
//! parent -> concrete review_pr ToolCall
//! child  -> reserved workflow_output ToolCall
//! parent -> one bounded Workflow ToolResult continuation
//! ```
//!
//! The Workflow itself is loaded only because `workflows.definitions` names
//! its exact YAML file. Its `reviewer` profile is Workflow-admitted but not
//! main-admitted, proving the two domains remain independent.

mod common;

use std::sync::Arc;

use common::provider_emulator::ProviderEmulator;
use rustx::local_runtime::composition::{
    LocalConversationCore, LocalConversationRuntime, LocalRuntimeDependencies, LocalRuntimePaths,
};
use rustx::message::content::TextBlock;
use rustx::message::types::UserContentBlock;
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime::workflow::WorkflowId;
use rustx::runtime_client::attachment::RuntimeAttachment;
use rustx::runtime_client::host::{EventDelivery, EventSubscription};
use rustx::runtime_client::types::RuntimeClientResult;
use rustx::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEvent, RuntimeClientOutcome,
};

const MODEL: &str = "workflow-model";
const KEY: &str = "RUSTX_ISSUE83_KEY";

fn models_json_for_base_url(base_url: &str) -> String {
    serde_json::json!({
        "providers": {
            "emulator": {
                "baseUrl": base_url,
                "apiKey": "issue83-secret",
                "models": [{
                    "id": MODEL,
                    "protocol": "openai_chat_completions",
                    "contextWindow": 128_000,
                    "maxOutputTokens": 1024,
                    "capabilities": {
                        "inputModalities": ["text"],
                        "outputModalities": ["text"],
                        "toolCalls": true,
                        "reasoning": false
                    },
                    "compat": {"chatReasoningReplay": "omit"}
                }]
            }
        }
    })
    .to_string()
}

fn models_json(emulator: &ProviderEmulator) -> String {
    models_json_for_base_url(&emulator.openai_base_url())
}

const CONFIG: &str = r#"{
  "schemaVersion": 5,
  "agentId": "agent-issue83",
  "model": {"model": "emulator/workflow-model"},
  "context": {"reserveTokens": 0, "keepRecentTokens": 0},
  "defaultTools": ["read"],
  "subagents": {
    "maxConcurrent": 4,
    "definitions": {
      "reviewer": {
        "description": "The Workflow-only reviewer.",
        "instructionsFile": ".agents/subagents/reviewer/instructions.md"
      }
    },
    "main": [],
    "workflow": ["reviewer"]
  },
  "workflows": {
    "definitions": ["review_pr"],
    "main": ["review_pr"]
  }
}"#;

const WORKFLOW: &str = r"description: Review the request with a native child agent.
input:
  type: object
  properties:
    task:
      type: string
  required: [task]
  additionalProperties: false
output:
  type: object
  properties:
    summary:
      type: string
  required: [summary]
  additionalProperties: false
entry: review
nodes:
  review:
    type: agent
    profile: reviewer
    task: Review the request and commit the result.
    input:
      task:
        ref: args.task
    output:
      type: object
      properties:
        passed:
          type: boolean
        summary:
          type: string
      required: [passed, summary]
      additionalProperties: false
  decision:
    type: branch
    condition:
      ref: review.passed
  success:
    type: return
    output:
      summary:
        ref: review.summary
  failure:
    type: return
    output:
      summary:
        ref: args.task
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
        let root = tempfile::tempdir().expect("temp root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents/subagents/reviewer"))
            .expect("subagent directory");
        std::fs::create_dir_all(workspace.join(".agents/workflows")).expect("workflow directory");
        std::fs::write(root.path().join("models.jsonc"), models_json(emulator))
            .expect("models.jsonc");
        std::fs::write(root.path().join("rustx.jsonc"), CONFIG).expect("rustx.jsonc");
        std::fs::write(
            workspace.join(".agents/subagents/reviewer/instructions.md"),
            "Review requests carefully.\n",
        )
        .expect("reviewer instructions");
        std::fs::write(workspace.join(".agents/workflows/review_pr.yaml"), WORKFLOW)
            .expect("workflow YAML");
        // This deliberately is not registered. It is also malformed, proving
        // that the loader uses configured ids rather than scanning the YAML
        // directory as an implicit admission surface.
        std::fs::write(
            workspace.join(".agents/workflows/inactive.yaml"),
            "this is not a registered Workflow definition: [",
        )
        .expect("inactive workflow YAML");

        let paths = LocalRuntimePaths {
            models: root.path().join("models.jsonc"),
            config: root.path().join("rustx.jsonc"),
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
            credentials: Arc::new(MapCredentialEnvironment::new([(
                KEY.to_owned(),
                "issue83-secret".to_owned(),
            )])),
            child_program: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        };
        let runtime = LocalConversationRuntime::compose(&paths, &dependencies)
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
        root.path().join("models.jsonc"),
        models_json_for_base_url("http://127.0.0.1:1/v1"),
    )
    .expect("models.jsonc");
    std::fs::write(root.path().join("rustx.jsonc"), CONFIG).expect("rustx.jsonc");
    std::fs::write(
        workspace.join(".agents/subagents/reviewer/instructions.md"),
        "Review requests carefully.\n",
    )
    .expect("reviewer instructions");
    std::fs::write(workspace.join(".rustx/workflows/review_pr.yaml"), WORKFLOW)
        .expect("legacy workflow YAML");

    let paths = LocalRuntimePaths {
        models: root.path().join("models.jsonc"),
        config: root.path().join("rustx.jsonc"),
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
        credentials: Arc::new(MapCredentialEnvironment::new([(
            KEY.to_owned(),
            "issue83-secret".to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    };
    let error = LocalConversationCore::compose(&paths, &dependencies)
        .await
        .expect_err("the obsolete workspace Workflow path is not a fallback");
    let detail = error.to_string();
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
        root.path().join("models.jsonc"),
        models_json_for_base_url("http://127.0.0.1:1/v1"),
    )
    .expect("models.jsonc");
    std::fs::write(
        root.path().join("rustx.jsonc"),
        CONFIG.replace("\"main\": [\"review_pr\"]", "\"main\": []"),
    )
    .expect("rustx.jsonc");
    std::fs::write(
        workspace.join(".agents/subagents/reviewer/instructions.md"),
        "Review requests carefully.\n",
    )
    .expect("reviewer instructions");
    std::fs::write(workspace.join(".agents/workflows/review_pr.yaml"), WORKFLOW)
        .expect("workflow YAML");

    let paths = LocalRuntimePaths {
        models: root.path().join("models.jsonc"),
        config: root.path().join("rustx.jsonc"),
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
    let resources = LocalConversationCore::compose(&paths, &LocalRuntimeDependencies::default())
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
async fn a_yaml_workflow_runs_through_one_native_child_and_returns_one_bounded_result() {
    let Some(emulator) = ProviderEmulator::start("workflow_output").await else {
        return;
    };
    let driver = Driver::start(&emulator).await;
    driver.submit();
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
    emulator.finish().await;
}
