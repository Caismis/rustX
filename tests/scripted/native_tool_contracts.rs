//! Canonical schema and registry-boundary regressions for Issue #91.
//!
//! These tests deliberately exercise the model-facing schema and the actual
//! preflight path. Edit's tolerated malformed model spellings are normalized
//! by its registration before canonical schema validation; no provider or
//! Agent Loop branch knows about those spellings.

use super::{common, support};
use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::ToolCallId;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::PreflightOutcome;
use rustx::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionStatus, ToolInvocationMode,
    ToolInvocationPolicy,
};
use std::sync::Arc;

const NATIVE_TOOL_NAMES: [&str; 7] = [
    "read",
    "write",
    "edit",
    "glob",
    "grep",
    "bash",
    "background_task",
];

fn definition(fixture: &common::NativeFixture, name: &str) -> rustx::tools::types::ToolDefinition {
    fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .unwrap_or_else(|| panic!("{name} is registered"))
}

fn required(schema: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = schema["required"]
        .as_array()
        .expect("required")
        .iter()
        .map(|value| value.as_str().expect("required string").to_owned())
        .collect();
    names.sort();
    names
}

fn properties(schema: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

fn preflight(
    fixture: &common::NativeFixture,
    name: &str,
    arguments: serde_json::Value,
) -> PreflightOutcome {
    let definition = definition(fixture, name);
    fixture
        .registry
        .preflight(&ToolCall {
            id: ToolCallId::new("call-preflight"),
            tool_id: definition.id,
            name: name.to_owned(),
            arguments,
        })
        .expect("identity resolves")
}

#[test]
fn all_native_schemas_are_canonical_and_have_no_file_path_contract() {
    let fixture = common::native_fixture();
    for name in NATIVE_TOOL_NAMES {
        let schema = definition(&fixture, name).input_schema;
        rustx::tools::schema::validate_canonical_schema(&schema)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(!schema.to_string().contains("$ref"));
        assert!(!schema.to_string().contains("file_path"));
    }
}

#[test]
fn read_write_edit_schemas_are_path_oriented_and_read_accepts_zero_offset() {
    let fixture = common::native_fixture();
    let read = definition(&fixture, "read").input_schema;
    assert_eq!(required(&read), ["path"]);
    assert_eq!(properties(&read), ["limit", "offset", "path"]);
    assert_eq!(read["properties"]["offset"]["minimum"], 0);
    assert_eq!(read["properties"]["limit"]["minimum"], 1);
    assert!(read["properties"]["limit"]["maximum"].is_null());

    for name in ["write", "edit"] {
        let schema = definition(&fixture, name).input_schema;
        assert!(required(&schema).contains(&"path".to_owned()));
        assert!(properties(&schema).contains(&"path".to_owned()));
        assert!(!properties(&schema).contains(&"file_path".to_owned()));
    }
    let edit = definition(&fixture, "edit").input_schema;
    assert_eq!(edit["properties"]["edits"]["minItems"], 1);
    assert_eq!(
        edit["properties"]["edits"]["items"]["properties"]["oldText"]["minLength"],
        1
    );
}

#[test]
fn grep_and_glob_expose_unbounded_model_configurable_limits() {
    let fixture = common::native_fixture();
    let grep = definition(&fixture, "grep").input_schema;
    assert_eq!(grep["properties"]["limit"]["minimum"], 1);
    assert!(grep["properties"]["limit"]["maximum"].is_null());
    let glob = definition(&fixture, "glob").input_schema;
    assert_eq!(properties(&glob), ["limit", "path", "pattern"]);
    assert_eq!(glob["properties"]["limit"]["minimum"], 1);
    assert!(glob["properties"]["limit"]["maximum"].is_null());
}

#[test]
fn old_file_path_and_invalid_business_arguments_are_rejected() {
    let fixture = common::native_fixture();
    for name in ["read", "write", "edit"] {
        let result = preflight(
            &fixture,
            name,
            serde_json::json!({"file_path": "/tmp/file.txt", "content": "x", "edits": []}),
        );
        assert!(
            matches!(result, PreflightOutcome::Rejected { .. }),
            "{name}"
        );
    }
    for (name, arguments) in [
        ("read", serde_json::json!({"path": "/tmp/file", "limit": 0})),
        ("write", serde_json::json!({"path": "/tmp/file"})),
        ("edit", serde_json::json!({"path": "/tmp/file", "edits": 7})),
        ("grep", serde_json::json!({"pattern": "x", "limit": 0})),
        ("glob", serde_json::json!({"pattern": "*", "limit": 0})),
    ] {
        assert!(matches!(
            preflight(&fixture, name, arguments),
            PreflightOutcome::Rejected { .. }
        ));
    }
    assert!(matches!(
        preflight(
            &fixture,
            "read",
            serde_json::json!({"path": "relative.txt", "offset": 0})
        ),
        PreflightOutcome::Ready(_)
    ));
}

#[test]
fn edit_model_variants_normalize_to_the_same_canonical_invocation() {
    let fixture = common::native_fixture();
    let canonical_edits = serde_json::json!([{"oldText": "a", "newText": "b"}]);
    let variants = [
        serde_json::json!({"path": "file.txt", "edits": canonical_edits}),
        serde_json::json!({
            "path": "file.txt",
            "edits": serde_json::to_string(&canonical_edits).expect("encoded edits")
        }),
        serde_json::json!({"path": "file.txt", "edits": {"oldText": "a", "newText": "b"}}),
        serde_json::json!({"path": "file.txt", "oldText": "a", "newText": "b"}),
    ];
    let mut canonical = None;
    for variant in variants {
        let PreflightOutcome::Ready(prepared) = preflight(&fixture, "edit", variant) else {
            panic!("supported Edit variant was rejected");
        };
        if let Some(expected) = &canonical {
            assert_eq!(&prepared.invocation.arguments, expected);
        } else {
            canonical = Some(prepared.invocation.arguments);
        }
    }
    assert_eq!(
        canonical.expect("canonical invocation"),
        serde_json::json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}]})
    );
}

#[test]
fn edit_normalization_cannot_consume_reserved_or_unrelated_fields() {
    let fixture = common::native_fixture();
    let reserved = preflight(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "file.txt",
            "oldText": "a",
            "newText": "b",
            "__rustx_forged": "value"
        }),
    );
    assert!(matches!(reserved, PreflightOutcome::Rejected { .. }));
    let unrelated = preflight(
        &fixture,
        "edit",
        serde_json::json!({"path": "file.txt", "edits": 42}),
    );
    assert!(matches!(unrelated, PreflightOutcome::Rejected { .. }));
}

#[test]
fn optional_native_properties_are_absent_not_nullable_and_registry_metadata_stays_private() {
    let fixture = common::native_fixture();
    for name in ["read", "grep", "glob"] {
        let schema = definition(&fixture, name).input_schema;
        for property in schema["properties"]
            .as_object()
            .expect("properties")
            .values()
        {
            assert_ne!(property["type"], serde_json::json!(["null"]));
        }
        assert!(schema["properties"]["__rustx_execution"].is_null());
        assert!(!schema.to_string().contains("__rustx_execution"));
    }
    for definition in fixture.registry.model_definitions() {
        assert!(
            !definition
                .input_schema
                .to_string()
                .contains("__rustx_execution"),
            "default native definitions stay provider-neutral: {}",
            definition.name
        );
    }
}

#[test]
fn native_tools_preserve_legal_execution_policies_and_fixed_background_task_policy() {
    use rustx::runtime::identity::{ConversationId, ToolId};
    use rustx::tools::executor::ToolRegistry;
    use rustx::tools::native::{NativeToolPolicies, NativeToolResources, register_native_tools};
    use rustx::tools::runtime::{ConversationRuntimeConfig, ConversationToolRuntime};

    for execution in [
        ToolExecutionPolicy::ForegroundOnly,
        ToolExecutionPolicy::BackgroundOnly,
        ToolExecutionPolicy::ModelSelectable,
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let runtime = ConversationToolRuntime::from_config(
            ConversationId::new("policy-conversation"),
            ConversationRuntimeConfig::new(&workspace, dir.path().join("artifacts")),
        )
        .expect("runtime");
        let mut registry = ToolRegistry::new();
        register_native_tools(
            &mut registry,
            NativeToolResources {
                background: runtime.background().clone(),
                subagents: None,
            },
            NativeToolPolicies::uniform(ToolInvocationPolicy::new(
                execution,
                ToolConcurrencyPolicy::Sequential,
            )),
        )
        .expect("ordinary native policy registration");

        let read = registry
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "read")
            .expect("read definition");
        assert_eq!(read.execution_policy, execution);
        let arguments = if execution == ToolExecutionPolicy::ModelSelectable {
            serde_json::json!({"path": "a.txt", "__rustx_execution": "foreground"})
        } else {
            serde_json::json!({"path": "a.txt"})
        };
        let outcome = registry
            .preflight(&ToolCall {
                id: ToolCallId::new("policy-call"),
                tool_id: ToolId::new("tool-read"),
                name: "read".to_owned(),
                arguments,
            })
            .expect("policy preflight");
        let PreflightOutcome::Ready(prepared) = outcome else {
            panic!("read must preflight under {execution:?}");
        };
        assert_eq!(
            prepared.invocation.mode,
            if execution == ToolExecutionPolicy::BackgroundOnly {
                ToolInvocationMode::Background
            } else {
                ToolInvocationMode::Foreground
            }
        );

        let background_task = registry
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "background_task")
            .expect("background_task definition");
        assert_eq!(
            background_task.execution_policy,
            ToolExecutionPolicy::ForegroundOnly
        );
        assert_eq!(
            background_task.concurrency_policy,
            ToolConcurrencyPolicy::Sequential
        );
    }
}

#[test]
fn independent_native_execution_policies_coexist_in_one_registry() {
    use rustx::tools::executor::ToolRegistry;
    use rustx::tools::native::{NativeToolPolicies, NativeToolResources, register_native_tools};
    use rustx::tools::runtime::{ConversationRuntimeConfig, ConversationToolRuntime};

    let dir = tempfile::tempdir().expect("temporary policy runtime");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let runtime = ConversationToolRuntime::from_config(
        ConversationId::new("independent-policy-conversation"),
        ConversationRuntimeConfig::new(&workspace, dir.path().join("artifacts")),
    )
    .expect("runtime");
    let policies = NativeToolPolicies {
        read: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        write: ToolInvocationPolicy::new(
            ToolExecutionPolicy::BackgroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        edit: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential,
        ),
        glob: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        grep: ToolInvocationPolicy::new(
            ToolExecutionPolicy::BackgroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        bash: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
    };
    let mut registry = ToolRegistry::new();
    register_native_tools(
        &mut registry,
        NativeToolResources {
            background: runtime.background().clone(),
            subagents: None,
        },
        policies,
    )
    .expect("independent native policy registration");
    let definitions = registry.definitions();
    let policy = |name: &str| {
        definitions
            .iter()
            .find(|definition| definition.name == name)
            .expect("native definition")
    };
    assert_eq!(
        policy("read").execution_policy,
        ToolExecutionPolicy::ForegroundOnly
    );
    assert_eq!(
        policy("write").execution_policy,
        ToolExecutionPolicy::BackgroundOnly
    );
    assert_eq!(
        policy("edit").execution_policy,
        ToolExecutionPolicy::ModelSelectable
    );
    assert_eq!(
        policy("read").concurrency_policy,
        ToolConcurrencyPolicy::Sequential
    );
    assert_eq!(
        policy("write").concurrency_policy,
        ToolConcurrencyPolicy::Parallel
    );
    assert_eq!(
        policy("edit").concurrency_policy,
        ToolConcurrencyPolicy::Sequential
    );
    assert_eq!(
        policy("glob").concurrency_policy,
        ToolConcurrencyPolicy::Parallel
    );
    assert_eq!(
        policy("grep").execution_policy,
        ToolExecutionPolicy::BackgroundOnly
    );
    assert_eq!(
        policy("grep").concurrency_policy,
        ToolConcurrencyPolicy::Sequential
    );
}

fn native_tool_turn(call: &support::fake::ScriptedCall) -> Vec<Vec<support::fake::FakeStep>> {
    let mut first = vec![support::fake::FakeStep::Emit(ModelEvent::Started)];
    first.extend(
        support::fake::tool_call_events(0, call)
            .into_iter()
            .map(support::fake::FakeStep::Emit),
    );
    first.push(support::fake::FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![
        first,
        vec![
            support::fake::FakeStep::Emit(ModelEvent::Started),
            support::fake::FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            support::fake::FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

fn native_request(
    model: &Arc<support::fake::FakeModel>,
    conversation_id: &ConversationId,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-native-contract"),
        conversation_id: conversation_id.clone(),
        attempt_id: AttemptId::new("attempt-native-contract"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("message-native-contract"),
                content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                    text: "inspect".to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx::message::types::InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model(model.clone(), "native-contract-model"),
    }
}

fn native_context_runtime(model: &Arc<support::fake::FakeModel>) -> rustx::context::ContextRuntime {
    rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        &support::attempt_model(model.clone(), "native-contract-model"),
    )
    .expect("context runtime")
}

async fn run_native_script(
    fixture: &common::NativeFixture,
    call: support::fake::ScriptedCall,
) -> common::DurableExecutionAudit {
    let model = support::fake::fake_model(native_tool_turn(&call));
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let (lease, coordinator) = capability.into_lease_and_coordinator();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        native_request(&model, fixture.runtime.conversation_id()),
        lease,
        &cancellation,
        native_context_runtime(&model),
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    let audit = common::durable_agent_result(result, fixture.store.as_ref());
    drop(coordinator);
    audit
}

#[tokio::test]
async fn native_agent_loop_invocation_has_one_start_and_one_completion_in_order() {
    let fixture = common::native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("read.txt"),
        "hello\n",
    )
    .expect("fixture");
    let audit = run_native_script(
        &fixture,
        support::fake::ScriptedCall {
            id: "call-native-read",
            tool_id: "tool-read",
            name: "read",
            arguments: serde_json::json!({"path": "read.txt"}),
        },
    )
    .await;

    let started_positions: Vec<usize> = audit
        .event_history
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
                if tool_call_id.as_str() == "call-native-read" =>
            {
                Some(index)
            }
            _ => None,
        })
        .collect();
    let completed_positions: Vec<usize> = audit
        .event_history
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                if tool_call_id.as_str() == "call-native-read" =>
            {
                Some(index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(started_positions.len(), 1, "one native start event");
    assert_eq!(completed_positions.len(), 1, "one native completion event");
    assert!(started_positions[0] < completed_positions[0]);

    let tool_message = audit
        .messages()
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == "call-native-read" => {
                Some(tool)
            }
            _ => None,
        })
        .expect("committed native tool result");
    assert_eq!(tool_message.result.status, ToolExecutionStatus::Success);
    assert_eq!(
        tool_message.result.content[0],
        rustx::tools::types::ToolResultContent::Text(rustx::message::content::TextBlock {
            text: "hello\n".to_owned(),
        })
    );
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
}

#[tokio::test]
async fn native_agent_loop_preflight_rejection_settles_without_starting_an_executor() {
    let fixture = common::native_fixture();
    let audit = run_native_script(
        &fixture,
        support::fake::ScriptedCall {
            id: "call-invalid-write",
            tool_id: "tool-write",
            name: "write",
            arguments: serde_json::json!({
                "path": "must-not-be-created.txt",
                "content": "x",
                "unknown": true
            }),
        },
    )
    .await;

    assert!(!audit.event_history.iter().any(|event| {
        matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
                if tool_call_id.as_str() == "call-invalid-write"
        )
    }));
    let tool_message = audit
        .messages()
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == "call-invalid-write" => {
                Some(tool)
            }
            _ => None,
        })
        .expect("rejected result slot");
    assert!(matches!(
        tool_message.result.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert!(
        !fixture
            .runtime
            .workspace()
            .root()
            .join("must-not-be-created.txt")
            .exists()
    );
}
