//! Canonical schema and registry-boundary regressions for Issue #91.
//!
//! These tests deliberately exercise the model-facing schema and the actual
//! preflight path. Edit's tolerated malformed model spellings are normalized
//! by its registration before canonical schema validation; no provider or
//! Agent Loop branch knows about those spellings.

use super::super::{common, support};
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

/// Every native Tool implementation, whichever plane composes it.
///
/// `todo` is here because this constant names *implementations* whose schema
/// contract is checked below — not ordinary selectable capabilities. Since
/// Issue #259 the `todo` Tool reaches the fixture registry through the Todo
/// Agent Extension, never through `register_native_tools`.
const NATIVE_TOOL_NAMES: [&str; 9] = [
    "read",
    "write",
    "edit",
    "glob",
    "grep",
    "bash",
    "execution",
    "ask_user",
    "todo",
];

#[test]
fn product_native_defaults_are_independent_and_bash_keeps_background_admission() {
    use ToolConcurrencyPolicy::{Parallel, Sequential};
    use ToolExecutionPolicy::{ForegroundOnly, ModelSelectable};
    use rustx::tools::types::ToolApprovalPolicy::{Always, Never};
    let fixture = common::native_fixture();
    for (name, execution, concurrency, approval) in [
        ("read", ForegroundOnly, Parallel, Never),
        ("write", ForegroundOnly, Sequential, Always),
        ("edit", ForegroundOnly, Sequential, Always),
        ("glob", ForegroundOnly, Parallel, Never),
        ("grep", ForegroundOnly, Parallel, Never),
        ("bash", ModelSelectable, Sequential, Always),
    ] {
        let actual = definition(&fixture, name);
        assert_eq!(
            (
                actual.execution_policy,
                actual.concurrency_policy,
                actual.approval_policy
            ),
            (execution, concurrency, approval),
            "{name}"
        );
    }
    for (mode, expected) in [
        ("foreground", ToolInvocationMode::Foreground),
        ("background", ToolInvocationMode::Background),
    ] {
        let PreflightOutcome::Ready(prepared) = preflight(
            &fixture,
            "bash",
            serde_json::json!({"command":"true","execution_mode":mode}),
        ) else {
            panic!("Bash admission");
        };
        assert_eq!(prepared.invocation.mode, expected);
        assert_eq!(prepared.approval, Always);
    }
}

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
        // Every native contract rejects unknown fields. A plain object
        // schema says so at its root; an action-tagged contract (the
        // `execution` intrinsic) is a root union whose *branches* carry the
        // object properties, and a root `additionalProperties: false` there
        // would forbid every property rather than the unknown ones. The
        // invariant is the strictness, not where it is written.
        if let Some(branches) = schema["oneOf"].as_array() {
            assert!(!branches.is_empty(), "{name}: an empty union");
            for branch in branches {
                assert_eq!(branch["type"], "object", "{name}: union branch");
                assert_eq!(
                    branch["additionalProperties"], false,
                    "{name}: every union branch rejects unknown fields"
                );
            }
        } else {
            assert_eq!(schema["additionalProperties"], false);
        }
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
        assert!(schema["properties"]["execution_mode"].is_null());
        assert!(!schema.to_string().contains("execution_mode"));
        assert!(!schema.to_string().contains("__rustx_execution"));
    }
    for definition in fixture.registry.model_definitions() {
        assert_eq!(
            definition
                .input_schema
                .to_string()
                .contains("execution_mode"),
            definition.name == "bash",
            "only model-selectable Bash requires execution metadata: {}",
            definition.name
        );
        assert!(
            !definition
                .input_schema
                .to_string()
                .contains("__rustx_execution"),
            "the retired reserved selector is gone: {}",
            definition.name
        );
    }
}

#[test]
fn native_tools_preserve_legal_execution_policies_and_fixed_execution_intrinsic_policy() {
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
                subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
                background: runtime.background().clone(),
                subagents: None,
            },
            NativeToolPolicies::uniform(ToolInvocationPolicy::new(
                execution,
                ToolConcurrencyPolicy::Sequential,
                rustx::tools::types::ToolApprovalPolicy::Never,
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
            serde_json::json!({"path": "a.txt", "execution_mode": "foreground"})
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

        let execution = registry
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "execution")
            .expect("execution definition");
        assert_eq!(
            execution.execution_policy,
            ToolExecutionPolicy::ForegroundOnly
        );
        assert_eq!(
            execution.concurrency_policy,
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
            rustx::tools::types::ToolApprovalPolicy::Never,
        ),
        write: ToolInvocationPolicy::new(
            ToolExecutionPolicy::BackgroundOnly,
            ToolConcurrencyPolicy::Parallel,
            rustx::tools::types::ToolApprovalPolicy::Never,
        ),
        edit: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential,
            rustx::tools::types::ToolApprovalPolicy::Never,
        ),
        glob: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
            rustx::tools::types::ToolApprovalPolicy::Never,
        ),
        grep: ToolInvocationPolicy::new(
            ToolExecutionPolicy::BackgroundOnly,
            ToolConcurrencyPolicy::Sequential,
            rustx::tools::types::ToolApprovalPolicy::Never,
        ),
        bash: ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
            rustx::tools::types::ToolApprovalPolicy::Never,
        ),
    };
    let mut registry = ToolRegistry::new();
    register_native_tools(
        &mut registry,
        NativeToolResources {
            subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
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
        Some(rustx::context::AgentStatusEngine::default()),
        &support::attempt_model(model.clone(), "native-contract-model"),
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("context runtime")
}

/// Real generated dispatchers and representative already-prepared MCP
/// identities enter the same available catalog as the native tools.
fn selection_registry(fixture: &common::NativeFixture) -> rustx::tools::executor::ToolRegistry {
    use rustx::runtime::subagent::{SubagentCatalog, SubagentDefinition, SubagentName};
    use rustx::runtime::workflow::{WorkflowCatalog, WorkflowId, WorkflowProgram, WorkflowRuntime};
    let plane = support::execution::subagent_plane_for(fixture.runtime.conversation_id().as_str());
    let catalog = SubagentCatalog::new([SubagentDefinition::new(
        SubagentName::parse("worker").unwrap(),
        "Worker".into(),
        "Do the task".into(),
        "worker.md".into(),
        None,
        None,
        vec![],
        vec![],
        rustx::runtime::subagent::SubagentProjectInstructionPolicy {
            inherit: false,
            files: vec![],
        },
        rustx::runtime::workspace::WorkspacePolicy::default(),
        rustx::extensions::NativeAgentExtensionsDocument::default().resolve(),
    )
    .unwrap()])
    .unwrap();
    let mut registry = rustx::tools::executor::ToolRegistry::new();
    rustx::tools::native::register_native_tools(
        &mut registry,
        rustx::tools::NativeToolResources {
            background: fixture.runtime.background().clone(),
            subagents: Some(plane.registry.clone()),
            subagent_catalog: catalog,
        },
        rustx::tools::NativeToolPolicies::default(),
    )
    .unwrap();
    let id = WorkflowId::parse("review_task").unwrap();
    let program = WorkflowProgram::compile(id.clone(), serde_json::from_value(serde_json::json!({
        "description":"Return admitted input", "timeout_ms":1000,
        "block":{"input":{"type":"object","additionalProperties":false},
        "output":{"type":"object","additionalProperties":false},"entry":"done",
        "nodes":{"done":{"type":"return","output":{"type":"reference","path":["args"]}}},"edges":[]}
    })).unwrap(), &std::collections::BTreeSet::new()).unwrap();
    let workflows = WorkflowCatalog::new([program], [id]).unwrap();
    let runtime = WorkflowRuntime::new(
        plane.registry.clone(),
        fixture.runtime.durable_store(),
        fixture.runtime.workflows().clone(),
    );
    rustx::tools::native::register_workflow_tools(&mut registry, &runtime, &workflows).unwrap();
    // Transport preparation is separately covered by CFG-02. Here both
    // origins are already available; selection cannot activate either.
    for (name, server) in [("external", "mcp-test"), ("python_echo", "python:echo")] {
        let mut definition = definition(fixture, "read");
        definition.id = rustx::runtime::identity::ToolId::new(format!("tool-{name}"));
        definition.name = name.into();
        definition.origin = rustx::tools::types::ToolOrigin::Mcp {
            server_id: rustx::runtime::identity::McpServerId::new(server),
        };
        registry
            .register(
                definition,
                fixture
                    .registry
                    .executor(&rustx::runtime::identity::ToolId::new("tool-read")),
            )
            .unwrap();
    }
    registry
}

async fn selected_capabilities(
    fixture: &common::NativeFixture,
    policy: rustx::capabilities::ToolActivationPolicy,
) -> Result<
    rustx::capabilities::CapabilityCoordinator,
    rustx::capabilities::CapabilityPreparationError,
> {
    let coordinator = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            python_sources: std::collections::BTreeMap::new(),
            conversation_id: fixture.runtime.conversation_id().clone(),
            workspace: fixture.runtime.workspace().clone(),
            base_tool_registry: Arc::new(selection_registry(fixture)),
            extensions: rustx::extensions::NativeAgentExtensions::none(),
            tool_activation: policy,
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![fixture.runtime.workspace().root().join(".agents/skills")],
                explicit_paths: vec![],
            },
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: fixture.runtime.environment().clone(),
            environment_store_root: fixture.dir().path().join("environments"),
        },
    )
    .unwrap();
    assert!(
        coordinator.current_snapshot().tool_registry().is_empty(),
        "bootstrap has no unselected authority"
    );
    let candidate = coordinator.prepare_candidate().await?;
    coordinator.commit(candidate).unwrap();
    Ok(coordinator)
}

#[tokio::test]
async fn exact_selection_reaches_provider_requests_and_domain_skill_projection() {
    use rustx::capabilities::ToolActivationPolicy as Selection;
    // The request projector never changes authority based on model capability
    // flags. Production adapter validation separately rejects unsupported
    // nonempty requests; the scripted adapter captures that exact boundary.
    for tool_calls in [true, false] {
        for (selection, expected) in [
            (
                Selection::default(),
                vec![
                    "execution",
                    "ask_user",
                    "read",
                    "write",
                    "edit",
                    "glob",
                    "grep",
                    "bash",
                    "subagent",
                    "review_task",
                    "external",
                    "python_echo",
                ],
            ),
            (
                Selection {
                    no_tools: true,
                    ..Default::default()
                },
                vec![],
            ),
            (
                Selection {
                    exclude_tools: vec!["read".into()],
                    ..Default::default()
                },
                vec![
                    "execution",
                    "ask_user",
                    "write",
                    "edit",
                    "glob",
                    "grep",
                    "bash",
                    "subagent",
                    "review_task",
                    "external",
                    "python_echo",
                ],
            ),
            (
                Selection {
                    tools: Some(vec!["grep".into()]),
                    ..Default::default()
                },
                vec!["grep"],
            ),
            (
                Selection {
                    tools: Some(vec!["grep".into(), "read".into()]),
                    ..Default::default()
                },
                vec!["grep", "read"],
            ),
            (
                Selection {
                    tools: Some(vec!["read".into(), "grep".into()]),
                    exclude_tools: vec!["read".into()],
                    ..Default::default()
                },
                vec!["grep"],
            ),
            (
                Selection {
                    tools: Some(vec!["external".into(), "python_echo".into()]),
                    ..Default::default()
                },
                vec!["external", "python_echo"],
            ),
            (
                Selection {
                    no_builtin_tools: true,
                    exclude_tools: vec!["external".into()],
                    ..Default::default()
                },
                vec!["python_echo"],
            ),
        ] {
            let fixture = common::native_fixture();
            let skill_root = fixture
                .runtime
                .workspace()
                .root()
                .join(".agents/skills/lazy");
            std::fs::create_dir_all(&skill_root).unwrap();
            std::fs::write(
                skill_root.join("SKILL.md"),
                "---\nname: lazy\ndescription: Lazy instructions\n---\nSECRET_SKILL_BODY\n",
            )
            .unwrap();
            let coordinator = selected_capabilities(&fixture, selection.clone())
                .await
                .unwrap();
            let snapshot = coordinator.current_snapshot();
            assert_eq!(snapshot.skills().packages().len(), 1);
            assert!(
                snapshot
                    .available_tools()
                    .definitions()
                    .iter()
                    .any(|tool| tool.name == "subagent")
            );
            assert!(
                snapshot
                    .available_tools()
                    .definitions()
                    .iter()
                    .any(|tool| tool.name == "review_task")
            );
            let resources = rustx::runtime::RuntimeResourceSnapshot::new(
                rustx::runtime::identity::RuntimeResourceRevision::new(1),
                vec![],
                None,
                rustx::context::ContextAssembly::new(),
                snapshot.clone(),
            );
            // A child can independently admit Read and its frozen lazy Skills,
            // even when the main registry admitted none.
            let frozen_read = snapshot
                .available_tools()
                .definitions()
                .into_iter()
                .find(|tool| tool.name == "read")
                .unwrap();
            let mut child = rustx::tools::executor::ToolRegistry::new();
            rustx::tools::native::register_subagent_child_tools(&mut child, &[frozen_read])
                .unwrap();
            assert_eq!(child.names(), ["read"]);
            assert!(
                !rustx::skills::admitted_skill_entries(snapshot.skills().catalog_entries(), &child)
                    .is_empty()
            );
            assert!(
                child
                    .preflight(&ToolCall {
                        id: ToolCallId::new("no-widen"),
                        tool_id: rustx::runtime::identity::ToolId::new("tool-grep"),
                        name: "grep".into(),
                        arguments: serde_json::json!({})
                    })
                    .is_err()
            );
            let model = support::fake::fake_model(vec![vec![
                support::fake::FakeStep::Emit(ModelEvent::Started),
                support::fake::FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: rustx::message::types::ContentBlockIndex::new(0),
                    text: "done".into(),
                }),
                support::fake::FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]]);
            let mut request = native_request(&model, fixture.runtime.conversation_id());
            let mut declaration = support::model::FixtureModel::text(
                "fixture/native-contract-model",
                rustx::model::ModelProtocol::OpenAiChatCompletions,
            );
            declaration.tool_calls = tool_calls;
            request.model = support::model::fixture_session_model(
                &[declaration],
                "fixture/native-contract-model",
                &support::model::ScriptedAdapterFactory::new(model.clone()),
            )
            .snapshot();
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            let result = AgentExecution::new(
                request,
                coordinator.acquire_attempt_lease(),
                &cancellation,
                support::default_execution_policy(),
                native_context_runtime(&model).with_runtime_resources(&resources),
                &fixture.runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await;
            assert!(matches!(
                common::durable_agent_result(result, fixture.store.as_ref()).outcome,
                AttemptOutcome::Completed { .. }
            ));
            let requests = model.requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].invocation.capabilities.tool_calls, tool_calls);
            assert_eq!(
                requests[0].tools,
                snapshot.tool_registry().model_definitions()
            );
            assert_eq!(
                requests[0]
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{selection:?}"
            );
            assert_eq!(
                requests[0]
                    .effective_system_prompt
                    .contains("Use the Read tool"),
                expected.contains(&"read")
            );
            assert!(
                !requests[0]
                    .effective_system_prompt
                    .contains("SECRET_SKILL_BODY")
            );
        }
    }
}

#[tokio::test]
async fn invalid_exact_selection_rejects_capability_preparation_without_fallback() {
    use rustx::capabilities::ToolActivationPolicy as Selection;
    for selection in [
        Selection {
            tools: Some(vec![]),
            ..Default::default()
        },
        Selection {
            tools: Some(vec!["read".into(), "read".into()]),
            ..Default::default()
        },
        Selection {
            tools: Some(vec!["unknown".into()]),
            ..Default::default()
        },
        Selection {
            tools: Some(vec!["unadmitted_workflow".into()]),
            ..Default::default()
        },
        Selection {
            exclude_tools: vec!["typo".into()],
            ..Default::default()
        },
        Selection {
            exclude_tools: vec![String::new()],
            ..Default::default()
        },
        Selection {
            no_tools: true,
            tools: Some(vec!["read".into()]),
            ..Default::default()
        },
    ] {
        let fixture = common::native_fixture();
        assert!(matches!(
            selected_capabilities(&fixture, selection).await,
            Err(rustx::capabilities::CapabilityPreparationError::ToolActivation(_))
        ));
    }
}

#[test]
fn a_same_named_external_read_does_not_satisfy_the_lazy_skill_dependency() {
    let fixture = common::native_fixture();
    let mut external = definition(&fixture, "read");
    external.origin = rustx::tools::types::ToolOrigin::Mcp {
        server_id: rustx::runtime::identity::McpServerId::new("external"),
    };
    let mut registry = rustx::tools::executor::ToolRegistry::new();
    registry
        .register(external.clone(), fixture.registry.executor(&external.id))
        .unwrap();
    let entries = [rustx::skills::SkillCatalogEntry {
        name: "lazy".into(),
        description: "Lazy instructions".into(),
        location: "/skills/lazy/SKILL.md".into(),
    }];
    assert!(rustx::skills::admitted_skill_entries(&entries, &registry).is_empty());
    assert_eq!(
        rustx::skills::admitted_skill_entries(&entries, &fixture.registry),
        &entries
    );
}

#[tokio::test]
async fn filtered_calls_cannot_recover_available_native_or_generated_dispatchers() {
    for (name, id) in [
        ("read", "tool-read"),
        ("subagent", "tool-subagent"),
        ("review_task", "tool-workflow-review_task"),
        ("external", "tool-external"),
        ("python_echo", "tool-python_echo"),
        ("workflow_output", "runtime-workflow-output"),
    ] {
        let fixture = common::native_fixture();
        let coordinator = selected_capabilities(
            &fixture,
            rustx::capabilities::ToolActivationPolicy {
                no_tools: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let model = support::fake::fake_model(native_tool_turn(&support::fake::ScriptedCall {
            id: "filtered",
            tool_id: id,
            name,
            arguments: serde_json::json!({}),
        }));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = AgentExecution::new(
            native_request(&model, fixture.runtime.conversation_id()),
            coordinator.acquire_attempt_lease(),
            &cancellation,
            support::default_execution_policy(),
            native_context_runtime(&model),
            &fixture.runtime,
            rustx::agent::AttemptLifecycle::inert().with_pre_tool_policy(Arc::new(
                crate::agent::lifecycle::ConfiguredApprovalPolicy::new(
                    rustx::runtime::ApprovalMode::FullAccess,
                ),
            )),
        )
        .unwrap()
        .run()
        .await;
        let audit = common::durable_agent_result(result, fixture.store.as_ref());
        assert_eq!(
            audit.outcome,
            AttemptOutcome::Failed {
                error: rustx::events::types::AttemptFailure::Runtime {
                    error: rustx::runtime::types::RuntimeError::UnknownTool { name: name.into() },
                },
            }
        );
        assert_eq!(model.requests().len(), 1, "no automatic replay");
        assert!(model.requests()[0].tools.is_empty());
        assert!(
            !audit
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
        );
        assert!(matches!(
            audit.event_history.last(),
            Some(RuntimeEvent::AttemptFailed { .. })
        ));
    }
}

#[tokio::test]
async fn main_no_tools_coexists_with_independent_workflow_terminal_authority() {
    use rustx::runtime::workflow::{
        WorkflowOutputLatch, WorkflowOutputSubmission, WorkflowOutputTerminal,
    };
    let fixture = common::native_fixture();
    let main = selected_capabilities(
        &fixture,
        rustx::capabilities::ToolActivationPolicy {
            no_tools: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let main_snapshot = main.current_snapshot();
    assert!(main_snapshot.tool_registry().is_empty());
    let admitted_read = main_snapshot
        .available_tools()
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "read")
        .unwrap();
    let mut child = rustx::tools::executor::ToolRegistry::new();
    rustx::tools::native::register_subagent_child_tools(&mut child, &[admitted_read]).unwrap();
    let child_capability = common::capability_lease(child, &fixture.runtime).await;
    let model = support::fake::fake_model(native_tool_turn(&support::fake::ScriptedCall {
        id: "terminal",
        tool_id: "runtime-workflow-output",
        name: "workflow_output",
        arguments: serde_json::json!({"passed":true}),
    }));
    let latch = Arc::new(WorkflowOutputLatch::new(serde_json::json!({
        "type":"object","properties":{"passed":{"type":"boolean"}},"required":["passed"],"additionalProperties":false
    })).unwrap());
    let mut policy = support::default_execution_policy();
    policy.workflow_output = Some(latch.clone());
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        native_request(&model, fixture.runtime.conversation_id()),
        child_capability.into_lease(),
        &cancellation,
        policy,
        native_context_runtime(&model),
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    let audit = common::durable_agent_result(result, fixture.store.as_ref());
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(model.requests().len(), 1);
    assert_eq!(
        model.requests()[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["read", "workflow_output"]
    );
    assert_eq!(
        latch.committed_value(),
        Some(serde_json::json!({"passed":true}))
    );
    assert_eq!(
        latch.submit(serde_json::json!({"passed":false})),
        WorkflowOutputSubmission::Stale
    );
    assert!(!latch.cancel(CancellationReason::UserRequested));
    assert!(!audit.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolExecutionCompleted { .. }
    )));
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCompleted { .. }))
            .count(),
        1
    );
    assert!(matches!(
        audit.event_history.last(),
        Some(RuntimeEvent::AttemptCompleted { .. })
    ));
    assert!(main.current_snapshot().tool_registry().is_empty());
}

#[tokio::test]
async fn native_admission_pins_policy_axes_and_exposure_across_candidate_changes() {
    use rustx::tools::types::ToolApprovalPolicy::{Always, Never};
    let fixture = common::native_fixture();
    let coordinator = selected_capabilities(
        &fixture,
        rustx::capabilities::ToolActivationPolicy {
            tools: Some(vec!["bash".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let lease = coordinator.acquire_attempt_lease();
    let frozen = lease.snapshot().clone();
    let call = ToolCall {
        id: ToolCallId::new("frozen"),
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        name: "bash".into(),
        arguments: serde_json::json!({"command":"true","execution_mode":"foreground"}),
    };
    let PreflightOutcome::Ready(before) = frozen.tool_registry().preflight(&call).unwrap() else {
        panic!("ready");
    };
    let mut replacement = rustx::tools::executor::ToolRegistry::new();
    let policies = rustx::tools::NativeToolPolicies {
        bash: ToolInvocationPolicy::new(
            ToolExecutionPolicy::BackgroundOnly,
            ToolConcurrencyPolicy::Parallel,
            Never,
        ),
        ..Default::default()
    };
    rustx::tools::native::register_native_tools(
        &mut replacement,
        rustx::tools::NativeToolResources {
            background: fixture.runtime.background().clone(),
            subagents: None,
            subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
        },
        policies,
    )
    .unwrap();
    let inputs = rustx::capabilities::CapabilityResourceInputs {
        python_sources: std::collections::BTreeMap::new(),
        base_tool_registry: Arc::new(replacement),
        tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: fixture.runtime.environment().clone(),
    };
    let candidate = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    assert_eq!(
        coordinator.commit(candidate).unwrap_err(),
        rustx::capabilities::CapabilityCommitError::Busy,
        "a leased attempt prevents publication"
    );
    assert_eq!(frozen.tool_registry().names(), ["bash"]);
    assert_eq!(before.approval, Always);
    assert_eq!(before.concurrency, ToolConcurrencyPolicy::Sequential);
    assert_eq!(before.invocation.mode, ToolInvocationMode::Foreground);
    drop(lease);
    let candidate = coordinator
        .prepare_candidate_with_inputs(inputs)
        .await
        .unwrap();
    coordinator.commit(candidate).unwrap();
    assert!(
        coordinator
            .current_snapshot()
            .tool_registry()
            .names()
            .contains(&"write")
    );
    let PreflightOutcome::Ready(after) = frozen.tool_registry().preflight(&call).unwrap() else {
        panic!("frozen ready");
    };
    assert_eq!(after.approval, before.approval);
    assert_eq!(after.concurrency, before.concurrency);
    assert_eq!(after.invocation, before.invocation);
    assert_eq!(frozen.tool_registry().names(), ["bash"]);
}

struct RendezvousNative {
    executor: Arc<dyn rustx::tools::executor::ToolExecutor>,
    barrier: tokio::sync::Barrier,
}

impl rustx::tools::executor::ToolExecutor for RendezvousNative {
    fn start<'a>(
        &'a self,
        invocation: rustx::tools::types::ToolInvocation,
        context: rustx::tools::executor::ToolExecutionContext<'a>,
    ) -> rustx::tools::executor::ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        rustx::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                // Both calls must have entered the same native executor before
                // either reads/searches. A sequential default cannot cross this.
                self.barrier.wait().await;
                self.executor.start(invocation, context).completion.await
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> rustx::tools::ToolProgressCapability {
        rustx::tools::ToolProgressCapability::None
    }
}

#[tokio::test]
async fn read_search_product_defaults_run_adjacent_calls_with_independent_state() {
    for name in ["read", "glob", "grep"] {
        let mut fixture = common::native_fixture();
        std::fs::write(
            fixture.runtime.workspace().root().join("alpha.txt"),
            "ALPHA\n",
        )
        .unwrap();
        std::fs::write(
            fixture.runtime.workspace().root().join("beta.txt"),
            "BETA\n",
        )
        .unwrap();
        let definition = definition(&fixture, name);
        let executor = fixture.registry.executor(&definition.id);
        fixture.registry = rustx::tools::executor::ToolRegistry::new();
        fixture
            .registry
            .register(
                definition.clone(),
                Arc::new(RendezvousNative {
                    executor,
                    barrier: tokio::sync::Barrier::new(2),
                }),
            )
            .unwrap();
        let mut turn = vec![support::fake::FakeStep::Emit(ModelEvent::Started)];
        for (index, (id, path)) in [("first", "alpha.txt"), ("second", "beta.txt")]
            .into_iter()
            .enumerate()
        {
            let args = match name {
                "read" => serde_json::json!({"path":path}),
                "glob" => serde_json::json!({"pattern":path}),
                _ => serde_json::json!({"pattern":".","path":path}),
            };
            let call = support::fake::ScriptedCall {
                id,
                tool_id: match name {
                    "read" => "tool-read",
                    "glob" => "tool-glob",
                    _ => "tool-grep",
                },
                name,
                arguments: args,
            };
            turn.extend(
                support::fake::tool_call_events(u32::try_from(index).unwrap(), &call)
                    .into_iter()
                    .map(support::fake::FakeStep::Emit),
            );
        }
        turn.push(support::fake::FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }));
        let model = support::fake::fake_model(vec![
            turn,
            vec![
                support::fake::FakeStep::Emit(ModelEvent::Started),
                support::fake::FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: rustx::message::types::ContentBlockIndex::new(0),
                    text: "done".into(),
                }),
                support::fake::FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
        ]);
        let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let execution = AgentExecution::new(
            native_request(&model, fixture.runtime.conversation_id()),
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            native_context_runtime(&model),
            &fixture.runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), execution.run())
            .await
            .expect("barrier liveness guard");
        let audit = common::durable_agent_result(result, fixture.store.as_ref());
        let results = audit
            .messages()
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        for (tool, expected) in results.iter().zip(if name == "glob" {
            ["alpha.txt", "beta.txt"]
        } else {
            ["ALPHA", "BETA"]
        }) {
            assert_eq!(tool.result.status, ToolExecutionStatus::Success);
            assert!(
                serde_json::to_string(&tool.result.content)
                    .unwrap()
                    .contains(expected)
            );
        }
        assert_eq!(model.requests().len(), 2);
        assert!(matches!(
            audit.event_history.last(),
            Some(RuntimeEvent::AttemptCompleted { .. })
        ));
    }
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
        support::default_execution_policy(),
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

#[tokio::test]
async fn malformed_ask_user_rejects_before_interaction_publication() {
    let fixture = common::native_fixture();
    let audit = run_native_script(
        &fixture,
        support::fake::ScriptedCall {
            id: "call-invalid-ask-user",
            tool_id: "tool-ask-user",
            name: "ask_user",
            arguments: serde_json::json!({
                "allow_free_text": "true",
                "choices": "[\"Swiss style\", \"Electronic magazine style\"]",
                "prompt": "Which visual style should I use?"
            }),
        },
    )
    .await;

    assert!(
        !audit
            .event_history
            .iter()
            .any(|event| { matches!(event, RuntimeEvent::InteractionRequested { .. }) })
    );
    assert!(!audit.event_history.iter().any(|event| {
        matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
                if tool_call_id.as_str() == "call-invalid-ask-user"
        )
    }));
    let tool_message = audit
        .messages()
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == "call-invalid-ask-user" => {
                Some(tool)
            }
            _ => None,
        })
        .expect("rejected ask_user result slot");
    assert!(matches!(
        tool_message.result.status,
        ToolExecutionStatus::Failed { .. }
    ));
}

/// The native questionnaire is one ordinary, decoratable root object.
///
/// The old `ask_user` schema was a root composition with three interdependent
/// modes. The replacement has one required `questions` property, so the
/// structural contract is visible to every provider adapter and to the
/// registry preflight path.
#[test]
fn ask_user_is_one_plain_questionnaire_schema() {
    use rustx::tools::{
        ToolExecutionPolicy, validate_canonical_schema, validate_execution_metadata_contract,
    };

    let fixture = common::native_fixture();
    let ask_user = definition(&fixture, "ask_user").input_schema;
    assert_eq!(ask_user["type"], "object");
    assert_eq!(required(&ask_user), ["questions"]);
    assert_eq!(properties(&ask_user), ["questions"]);
    assert!(ask_user.get("anyOf").is_none());
    assert!(ask_user.get("oneOf").is_none());
    assert_eq!(ask_user["additionalProperties"], false);
    validate_canonical_schema(&ask_user).expect("the questionnaire root is canonical");
    validate_execution_metadata_contract(ToolExecutionPolicy::ForegroundOnly, &ask_user)
        .expect("the fixed foreground policy preserves the ordinary root");

    // Every model-selectable native tool matches the decoratable root profile.
    for name in ["read", "write", "edit", "glob", "grep", "bash"] {
        let schema = definition(&fixture, name).input_schema;
        validate_execution_metadata_contract(ToolExecutionPolicy::ModelSelectable, &schema)
            .unwrap_or_else(|error| panic!("{name} must stay ModelSelectable-eligible: {error}"));
        for keyword in schema.as_object().expect("root object").keys() {
            assert!(
                rustx::tools::is_decoratable_root_keyword(keyword),
                "{name} uses root keyword {keyword:?}, outside the profile"
            );
        }
        // Native contracts are generated with inlined subschemas, so the
        // reference ban costs them nothing either.
        for keyword in rustx::tools::REFERENCE_APPLICATOR_KEYWORDS {
            assert!(
                !schema.to_string().contains(keyword),
                "{name} must stay reference-free for ModelSelectable: {keyword}"
            );
        }
    }
}
