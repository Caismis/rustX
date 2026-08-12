//! Deterministic invariant coverage of the native tool plane's module
//! boundaries and typed input contracts.
//!
//! The native tool plane owns one module per capability, and every native
//! tool owns a typed input contract from which its canonical schema is
//! generated. These tests prove the two boundaries that matter at runtime:
//!
//! - the generated schema is exactly the tool's model-facing input contract
//!   (required fields, optional fields, constraints, no reserved property,
//!   no `$ref`/`$defs` indirection);
//! - invalid input is rejected before execution — the registry preflight
//!   rejects the call so no invocation is ever produced, and a direct
//!   executor call rejects the arguments before performing any work.
//!
//! Agent-loop lifecycle and Bash lifecycle regressions live at the end of
//! the file: a real native tool executes through the agent loop with the
//! canonical event ordering, and a rejected native input stays a normal
//! failed result slot.

mod common;

use std::sync::Arc;

use common::fake::{FakeModel, FakeStep, ScriptedCall, tool_call_events};
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::types::{
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ReasoningEffort};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};
use rustx::tools::schema::validate_canonical_schema;
use rustx::tools::types::{
    ToolCall, ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
    ToolInvocationMode,
};

/// The canonical definition of one registered native tool.
fn definition(fixture: &common::NativeFixture, name: &str) -> ToolDefinition {
    fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .unwrap_or_else(|| panic!("{name} is registered"))
}

/// The sorted `required` field list of a generated schema.
fn required(schema: &serde_json::Value) -> Vec<String> {
    let mut required: Vec<String> = schema["required"]
        .as_array()
        .expect("generated schemas declare required")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("required names are strings")
                .to_owned()
        })
        .collect();
    required.sort();
    required
}

/// The sorted property names of a generated schema.
fn properties(schema: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = schema["properties"]
        .as_object()
        .expect("generated schemas declare properties")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// Every native tool's generated schema is a valid canonical tool schema:
/// a self-contained root object schema with no reserved runtime property
/// and no `$ref`/`$defs`/meta-schema indirection reaching the provider
/// surface.
#[test]
fn generated_native_schemas_are_canonical_root_object_schemas() {
    let fixture = common::native_fixture();
    let names = [
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "bash",
        "background_task",
    ];
    for name in names {
        let schema = definition(&fixture, name).input_schema;
        validate_canonical_schema(&schema)
            .unwrap_or_else(|error| panic!("{name} schema is canonical: {error}"));
        let object = schema.as_object().expect("root object schema");
        assert_eq!(object["type"], "object", "{name} is a root object schema");
        assert_eq!(
            object["additionalProperties"],
            serde_json::Value::Bool(false),
            "{name} rejects unknown properties"
        );
        for key in ["$schema", "$defs", "definitions", "title"] {
            assert!(
                !object.contains_key(key),
                "{name} schema must not carry {key}"
            );
        }
        assert!(
            !schema.to_string().contains("$ref"),
            "{name} schema inlines every subschema"
        );
    }
}

/// The generated Read schema is exactly the `ReadInput` contract: one
/// required path, two optional bounded line-window fields.
#[test]
fn read_schema_matches_its_input_contract() {
    let fixture = common::native_fixture();
    let schema = definition(&fixture, "read").input_schema;
    assert_eq!(required(&schema), ["path"]);
    assert_eq!(properties(&schema), ["line_count", "path", "start_line"]);
    assert_eq!(schema["properties"]["path"]["type"], "string");
    for optional in ["start_line", "line_count"] {
        assert_eq!(
            schema["properties"][optional]["minimum"], 1,
            "{optional} is bounded by its contract"
        );
        assert_eq!(
            schema["properties"][optional]["type"],
            serde_json::json!(["integer", "null"]),
            "{optional} is an optional integer"
        );
    }
}

/// The generated Write and Edit schemas are exactly their input contracts,
/// including Edit's non-empty anchor constraint and its defaulted flag.
#[test]
fn write_and_edit_schemas_match_their_input_contracts() {
    let fixture = common::native_fixture();
    let write = definition(&fixture, "write").input_schema;
    assert_eq!(required(&write), ["content", "path"]);
    assert_eq!(properties(&write), ["content", "path"]);

    let edit = definition(&fixture, "edit").input_schema;
    assert_eq!(required(&edit), ["new_text", "old_text", "path"]);
    assert_eq!(
        properties(&edit),
        ["new_text", "old_text", "path", "replace_all"]
    );
    assert_eq!(edit["properties"]["old_text"]["minLength"], 1);
    assert_eq!(edit["properties"]["replace_all"]["type"], "boolean");
    assert_eq!(edit["properties"]["replace_all"]["default"], false);
}

/// The generated Glob and Grep schemas carry their optional search
/// parameters with the defaults their input contracts apply.
#[test]
fn glob_and_grep_schemas_match_their_input_contracts() {
    let fixture = common::native_fixture();
    let glob = definition(&fixture, "glob").input_schema;
    assert_eq!(required(&glob), ["pattern"]);
    assert_eq!(properties(&glob), ["path", "pattern"]);
    assert_eq!(glob["properties"]["path"]["default"], ".");

    let grep = definition(&fixture, "grep").input_schema;
    assert_eq!(required(&grep), ["pattern"]);
    assert_eq!(
        properties(&grep),
        ["case_sensitive", "glob", "path", "pattern"]
    );
    assert_eq!(grep["properties"]["path"]["default"], ".");
    assert_eq!(grep["properties"]["glob"]["default"], "**/*");
    assert_eq!(grep["properties"]["case_sensitive"]["default"], true);
}

/// The generated Bash schema states the non-empty command and the bounded
/// optional deadline; the `background_task` intrinsic keeps its exact
/// two-operation enum.
#[test]
fn bash_and_background_task_schemas_match_their_input_contracts() {
    let fixture = common::native_fixture();
    let bash = definition(&fixture, "bash").input_schema;
    assert_eq!(required(&bash), ["command"]);
    assert_eq!(properties(&bash), ["command", "timeout_ms"]);
    assert_eq!(bash["properties"]["command"]["minLength"], 1);
    assert_eq!(bash["properties"]["timeout_ms"]["minimum"], 1);

    let intrinsic = definition(&fixture, "background_task").input_schema;
    assert_eq!(required(&intrinsic), ["action", "execution_id"]);
    assert_eq!(properties(&intrinsic), ["action", "execution_id"]);
    assert_eq!(intrinsic["properties"]["action"]["type"], "string");
    assert_eq!(
        intrinsic["properties"]["action"]["enum"],
        serde_json::json!(["status", "cancel"])
    );
}

/// Invalid model arguments never produce an invocation: the registry
/// preflight rejects the call against the generated schema, so no native
/// executor is ever reached.
///
/// The cases cover wrong JSON types, missing required fields, unknown
/// fields, invalid values, and an invalid enum variant.
#[test]
fn invalid_native_arguments_are_rejected_before_any_invocation_exists() {
    let fixture = common::native_fixture();
    let cases: [(&str, serde_json::Value); 12] = [
        ("read", serde_json::json!({})),
        ("read", serde_json::json!({"path": 42})),
        (
            "read",
            serde_json::json!({"path": "a.txt", "start_line": 0}),
        ),
        (
            "read",
            serde_json::json!({"path": "a.txt", "line_count": "many"}),
        ),
        ("read", serde_json::json!({"path": "a.txt", "extra": true})),
        ("write", serde_json::json!({"path": "a.txt"})),
        (
            "edit",
            serde_json::json!({"path": "a.txt", "old_text": "", "new_text": "b"}),
        ),
        ("glob", serde_json::json!({"pattern": "*", "path": 3})),
        (
            "grep",
            serde_json::json!({"pattern": "x", "case_sensitive": "yes"}),
        ),
        ("bash", serde_json::json!({"command": ""})),
        (
            "bash",
            serde_json::json!({"command": "true", "timeout_ms": 0}),
        ),
        (
            "background_task",
            serde_json::json!({"execution_id": "exec-1", "action": "list"}),
        ),
    ];
    for (name, arguments) in cases {
        let definition = definition(&fixture, name);
        let call = ToolCall {
            id: ToolCallId::new("call-invalid"),
            tool_id: definition.id.clone(),
            name: name.to_owned(),
            arguments: arguments.clone(),
        };
        let outcome = fixture.registry.preflight(&call).expect("resolvable call");
        assert!(
            matches!(outcome, PreflightOutcome::Rejected { .. }),
            "{name} must reject {arguments} before dispatch"
        );
    }
}

/// A direct executor call with contract-violating arguments — the path a
/// registry preflight would already have rejected — fails without doing any
/// of the tool's work: no file is created, and no file is modified.
#[tokio::test]
async fn rejected_input_never_reaches_the_executed_work() {
    let fixture = common::native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    std::fs::write(workspace.join("kept.txt"), "original").expect("fixture file");

    let created = workspace.join("never.txt");
    let write = execute_directly(
        &fixture,
        "write",
        serde_json::json!({"path": "never.txt", "content": 7}),
    )
    .await;
    assert!(matches!(write.status, ToolExecutionStatus::Failed { .. }));
    assert!(
        !created.exists(),
        "a rejected Write never creates its target"
    );

    let edit = execute_directly(
        &fixture,
        "edit",
        serde_json::json!({"path": "kept.txt", "old_text": "", "new_text": "replaced"}),
    )
    .await;
    assert!(matches!(edit.status, ToolExecutionStatus::Failed { .. }));
    assert_eq!(
        std::fs::read_to_string(workspace.join("kept.txt")).expect("kept file"),
        "original",
        "a rejected Edit never writes back"
    );

    let bash = execute_directly(&fixture, "bash", serde_json::json!({"command": ""})).await;
    assert!(matches!(bash.status, ToolExecutionStatus::Failed { .. }));
    assert!(
        bash.artifacts.is_empty() && bash.exit_code.is_none(),
        "a rejected Bash invocation never owns a process"
    );
}

/// Executes one native tool without the registry preflight, so the typed
/// input boundary inside the executor is what rejects the arguments.
async fn execute_directly(
    fixture: &common::NativeFixture,
    name: &str,
    arguments: serde_json::Value,
) -> ToolExecutionResult {
    let definition = definition(fixture, name);
    let executor = fixture.registry.executor(&definition.id);
    let reporter = common::NoopProgress;
    let context = ToolExecutionContext {
        conversation_id: fixture.runtime.conversation_id(),
        execution_id: None,
        cancellation: rustx::runtime::CancellationSignal::new(),
        workspace: fixture.runtime.workspace(),
        progress: &reporter,
        artifacts: fixture.runtime.artifacts(),
        environment: fixture.runtime.environment(),
    };
    executor
        .execute(
            ToolInvocation {
                call_id: ToolCallId::new("call-direct"),
                tool_id: definition.id.clone(),
                tool_name: name.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            },
            context,
        )
        .await
}

/// A valid native invocation still executes through the Agent Loop with the
/// canonical tool lifecycle: one `ToolExecutionStarted`, one
/// `ToolExecutionCompleted`, one committed tool message, and the canonical
/// `ToolExecutionResult` of the tool itself.
#[tokio::test]
async fn native_tools_still_execute_through_the_agent_loop() {
    let fixture = common::native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("sample.txt"),
        "alpha\nbeta\n",
    )
    .expect("fixture file");
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-read",
        name: "read",
        arguments: serde_json::json!({"path": "sample.txt", "start_line": 2, "line_count": 1}),
    };
    let result = run_through_agent_loop(&fixture, &call).await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].result.status, ToolExecutionStatus::Success);
    let rendered = format!("{:?}", messages[0].result.content);
    assert!(
        rendered.contains("beta") && !rendered.contains("alpha"),
        "the canonical result is the tool's own deterministic slice: {rendered}"
    );
    assert_eq!(
        lifecycle_events(&result),
        vec!["ToolExecutionStarted", "ToolExecutionCompleted"],
        "the tool lifecycle event ordering is unchanged"
    );
}

/// A native input-contract violation stays a normal failed result slot in
/// the Agent Loop: the batch commits, the attempt completes, and — because
/// the executor never runs — no tool execution lifecycle event is emitted
/// for the rejected call.
#[tokio::test]
async fn rejected_native_input_is_a_normal_failed_result_slot_in_the_agent_loop() {
    let fixture = common::native_fixture();
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-read",
        name: "read",
        arguments: serde_json::json!({"path": 42}),
    };
    let result = run_through_agent_loop(&fixture, &call).await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert!(
        lifecycle_events(&result).is_empty(),
        "a rejected invocation never starts an execution, so it emits no \
         tool execution lifecycle event"
    );
}

/// The Bash lifecycle is unchanged behind the typed input contract: a
/// valid invocation still spawns its supervised process, settles, captures
/// its output as artifacts, and reports the exit code — including a
/// non-zero exit as a normal failed result.
///
/// The supervisor ownership, timeout, and cancellation regressions in
/// `tests/m5_bash.rs` and the in-crate `tools::native::bash` regressions
/// run over exactly this path.
#[cfg(unix)]
#[tokio::test]
async fn bash_still_settles_its_process_lifecycle_behind_the_typed_contract() {
    let fixture = common::native_fixture();
    let success = common::run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "printf hello"}),
    )
    .await;
    assert_eq!(success.status, ToolExecutionStatus::Success);
    assert_eq!(success.exit_code, Some(0));
    assert!(
        !success.artifacts.is_empty(),
        "the stdout capture is still spooled as an artifact"
    );

    let failure =
        common::run_tool(&fixture, "bash", serde_json::json!({"command": "exit 7"})).await;
    assert!(matches!(failure.status, ToolExecutionStatus::Failed { .. }));
    assert_eq!(failure.exit_code, Some(7));

    let timed_out = common::run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "sleep 30", "timeout_ms": 200}),
    )
    .await;
    assert_eq!(
        timed_out.status,
        ToolExecutionStatus::TimedOut,
        "an explicit timeout_ms from the typed contract still bounds the invocation"
    );
}

/// The canonical tool lifecycle event names of one attempt, in order.
fn lifecycle_events(result: &AgentExecutionResult) -> Vec<&'static str> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionStarted { .. } => Some("ToolExecutionStarted"),
            RuntimeEvent::ToolExecutionCompleted { .. } => Some("ToolExecutionCompleted"),
            RuntimeEvent::ToolExecutionProgress { .. } => Some("ToolExecutionProgress"),
            _ => None,
        })
        .collect()
}

/// Tool messages committed to canonical history, in order.
fn tool_messages(result: &AgentExecutionResult) -> Vec<&ToolMessageBlock> {
    result
        .messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// Runs one scripted native tool call through a real Agent Loop attempt
/// over the native tool registry of the fixture.
async fn run_through_agent_loop(
    fixture: &common::NativeFixture,
    call: &ScriptedCall,
) -> AgentExecutionResult {
    let model = FakeModel::new(tool_turn_then_stop(call));
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let (lease, _coordinator) = capability.into_lease_and_coordinator();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    AgentExecution::new(
        request(fixture.runtime.conversation_id().clone()),
        &model,
        lease,
        &cancellation,
        context_runtime(),
        &fixture.runtime,
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await
}

/// One tool-call turn followed by a plain stop turn.
fn tool_turn_then_stop(call: &ScriptedCall) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in tool_call_events(0, call) {
        first.push(FakeStep::Emit(event));
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![
        first,
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

/// The attempt request bound to the fixture's conversation identity.
fn request(conversation_id: ConversationId) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id,
        attempt_id: AttemptId::new("attempt-1"),
        initial_messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-1"),
            content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                text: "go".to_owned(),
            })],
            source: UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })],
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: "fake-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
    }
}

/// A context runtime with no compaction pressure.
fn context_runtime() -> rustx::context::ContextRuntime<'static> {
    use rustx::context::{
        ContextConfig, ContextEngine, ContextRuntime, DefaultTokenEstimator,
        InMemoryCheckpointStore,
    };
    let estimator: Arc<dyn rustx::context::TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("valid context configuration");
    ContextRuntime::new(
        engine,
        Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
        Arc::new(InMemoryCheckpointStore::new()),
    )
}
